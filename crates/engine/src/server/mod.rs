//! Authoritative simulation. Runs on its own thread; single player talks to it through the
//! in-process transport.

mod dim;

use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, unbounded};
use glam::{DVec3, IVec3};
use rayon::prelude::*;
use rustc_hash::FxHashSet;

pub use dim::Dim;

use crate::content::{Content, content};
use crate::net::{ServerConn, ToClient, ToServer};
use crate::world::Cube;

pub const TPS: u32 = 20;

#[derive(Clone)]
pub struct ServerConfig {
    pub seed: u64,
    /// Horizontal view radius in cubes.
    pub view_radius: i32,
    /// Vertical view radius in cubes.
    pub view_height: i32,
}

pub struct Player {
    pub id: u64,
    pub name: String,
    conn: ServerConn,
    pub dim: u32,
    pub pos: DVec3,
    pub yaw: f32,
    pub pitch: f32,
    sent: FxHashSet<IVec3>,
    joined: bool,
}

pub struct Generated {
    dim: u32,
    pos: IVec3,
    cube: Cube,
}

pub struct Server {
    pub cfg: ServerConfig,
    pub dims: Vec<Dim>,
    pub players: Vec<Player>,
    pub tick: u64,
    next_player: u64,
    gen_tx: Sender<Generated>,
    gen_rx: Receiver<Generated>,
    incoming: Receiver<ServerConn>,
    pub tick_time: Duration,
}

/// Handle the client side uses to reach a running server thread.
pub struct ServerHandle {
    pub connect: Sender<ServerConn>,
    stop: Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ServerHandle {
    pub fn stop(&mut self) {
        let _ = self.stop.send(());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn spawn(cfg: ServerConfig) -> ServerHandle {
    let (connect, incoming) = unbounded();
    let (stop, stop_rx) = unbounded::<()>();
    let thread = std::thread::Builder::new()
        .name("server".into())
        .spawn(move || {
            let mut server = Server::new(cfg, incoming);
            let step = Duration::from_secs_f64(1.0 / TPS as f64);
            let mut next = Instant::now();
            loop {
                if stop_rx.try_recv().is_ok() {
                    break;
                }
                server.tick();
                next += step;
                let now = Instant::now();
                if next > now {
                    std::thread::sleep(next - now);
                } else if now - next > step * 10 {
                    // Far behind (debugger, huge load): skip instead of fast-forwarding.
                    next = now;
                }
            }
            server.shutdown();
        })
        .expect("spawning server thread");
    ServerHandle { connect, stop, thread: Some(thread) }
}

impl Server {
    pub fn new(cfg: ServerConfig, incoming: Receiver<ServerConn>) -> Server {
        let c = content();
        let dims = (0..c.dims.len() as u32).map(|i| Dim::new(i, cfg.seed)).collect();
        let (gen_tx, gen_rx) = unbounded();
        Server {
            cfg,
            dims,
            players: Vec::new(),
            tick: 0,
            next_player: 1,
            gen_tx,
            gen_rx,
            incoming,
            tick_time: Duration::ZERO,
        }
    }

    fn shutdown(&mut self) {
        for p in &mut self.players {
            p.conn.send(ToClient::Disconnect("server closed".into()));
        }
    }

    pub fn tick(&mut self) {
        let start = Instant::now();
        let c = content();
        while let Ok(conn) = self.incoming.try_recv() {
            let id = self.next_player;
            self.next_player += 1;
            self.players.push(Player {
                id,
                name: String::new(),
                conn,
                dim: 0,
                pos: DVec3::ZERO,
                yaw: 0.0,
                pitch: 0.0,
                sent: FxHashSet::default(),
                joined: false,
            });
        }

        self.receive_generated(c);
        self.handle_messages(c);

        self.dims.par_iter_mut().for_each(|d| d.tick(c));

        self.request_generation(c);
        self.sync_players();
        self.players.retain(|p| !p.conn.is_closed());
        self.tick += 1;
        self.tick_time = start.elapsed();
    }

    fn receive_generated(&mut self, c: &'static Content) {
        let mut batch: Vec<Generated> = self.gen_rx.try_iter().collect();
        // Insertion order decides nothing visible, but sorting keeps runs reproducible.
        batch.sort_by_key(|g| (g.dim, g.pos.x, g.pos.y, g.pos.z));
        for g in batch {
            self.dims[g.dim as usize].insert(c, g.pos, g.cube);
        }
    }

    fn handle_messages(&mut self, c: &'static Content) {
        for pi in 0..self.players.len() {
            while let Some(msg) = self.players[pi].conn.recv() {
                let p = &mut self.players[pi];
                match msg {
                    ToServer::Join { name } => {
                        p.name = name;
                        p.joined = true;
                        let dim = c.dim_names.get("base:overworld").copied().unwrap_or(0);
                        let spawn = self.dims[dim as usize].spawn_point(c);
                        p.dim = dim;
                        p.pos = spawn;
                        log::info!("{} joined", p.name);
                        p.conn.send(ToClient::Welcome { player: p.id, dim, pos: spawn, seed: self.cfg.seed });
                        p.conn.send(ToClient::Time { dim, time: self.dims[dim as usize].time });
                    }
                    ToServer::Move { pos, yaw, pitch, .. } => {
                        if pos.is_finite() {
                            p.pos = pos;
                            p.yaw = yaw;
                            p.pitch = pitch;
                        }
                    }
                    ToServer::Break { pos } => {
                        let d = p.dim as usize;
                        if within_reach(p.pos, pos) {
                            self.dims[d].player_break(c, pos, p.id);
                        }
                    }
                    ToServer::Place { pos, face, state, look } => {
                        let d = p.dim as usize;
                        if within_reach(p.pos, pos) && (state as usize) < c.states.len() {
                            self.dims[d].player_place(c, pos, face, state, look, p.id);
                        }
                    }
                    ToServer::Use { pos, face } => {
                        let d = p.dim as usize;
                        if within_reach(p.pos, pos) {
                            self.dims[d].player_use(c, pos, face, p.id);
                        }
                    }
                    ToServer::Leave => {
                        log::info!("{} left", p.name);
                        p.conn.send(ToClient::Disconnect("bye".into()));
                        p.joined = false;
                    }
                }
            }
        }
    }

    fn view(&self, p: &Player) -> (IVec3, i32, i32) {
        let center = (p.pos.floor().as_ivec3()) >> 4;
        (center, self.cfg.view_radius, self.cfg.view_height)
    }

    /// Cubes players need, nearest first; generation runs on the rayon pool.
    fn request_generation(&mut self, c: &'static Content) {
        let budget = rayon::current_num_threads() * 4;
        let mut wanted: Vec<(i32, u32, IVec3)> = Vec::new();
        for p in self.players.iter().filter(|p| p.joined) {
            let (center, r, h) = self.view(p);
            let dim = &self.dims[p.dim as usize];
            let def = &c.dims[p.dim as usize];
            for dy in -h..=h {
                let y = center.y + dy;
                if y < def.min_cube_y || y > def.max_cube_y {
                    continue;
                }
                for dz in -r..=r {
                    for dx in -r..=r {
                        if dx * dx + dz * dz > r * r {
                            continue;
                        }
                        let pos = IVec3::new(center.x + dx, y, center.z + dz);
                        if !dim.has(pos) && !dim.generating.contains(&pos) {
                            wanted.push((dx * dx + dz * dz + dy * dy * 4, p.dim, pos));
                        }
                    }
                }
            }
        }
        let in_flight: usize = self.dims.iter().map(|d| d.generating.len()).sum();
        if in_flight >= budget || wanted.is_empty() {
            return;
        }
        wanted.sort_by_key(|w| w.0);
        wanted.dedup_by_key(|w| (w.1, w.2));
        for &(_, dim, pos) in wanted.iter().take(budget - in_flight) {
            self.dims[dim as usize].generating.insert(pos);
            let tx = self.gen_tx.clone();
            let seed = self.cfg.seed;
            rayon::spawn(move || {
                let cube = dim::generate(content(), dim, seed, pos);
                let _ = tx.send(Generated { dim, pos, cube });
            });
        }
    }

    fn sync_players(&mut self) {
        let c = content();
        let mut unload: Vec<(u32, IVec3)> = Vec::new();
        for pi in 0..self.players.len() {
            if !self.players[pi].joined {
                continue;
            }
            let (center, r, h) = self.view(&self.players[pi]);
            let p = &mut self.players[pi];
            let dim = &self.dims[p.dim as usize];
            let def = &c.dims[p.dim as usize];

            for &(pos, ref cube) in &dim.resend {
                if p.sent.contains(&pos) {
                    p.conn.send(ToClient::Cube { dim: p.dim, pos, cube: cube.clone() });
                }
            }
            if !dim.changes.is_empty() {
                let changes: Vec<(IVec3, u32)> =
                    dim.changes.iter().filter(|(b, _)| p.sent.contains(&(*b >> 4))).copied().collect();
                if !changes.is_empty() {
                    p.conn.send(ToClient::Blocks { dim: p.dim, changes });
                }
            }
            if self.tick % 20 == 0 {
                p.conn.send(ToClient::Time { dim: p.dim, time: dim.time });
            }

            let in_range = |pos: IVec3| {
                let d = pos - center;
                d.x * d.x + d.z * d.z <= r * r && d.y.abs() <= h && pos.y >= def.min_cube_y && pos.y <= def.max_cube_y
            };
            // Send a cube once its neighbours exist, so its light is final when it arrives.
            let mut to_send: Vec<(i32, IVec3)> = Vec::new();
            for dy in -h..=h {
                for dz in -r..=r {
                    for dx in -r..=r {
                        let pos = center + IVec3::new(dx, dy, dz);
                        if !in_range(pos) || p.sent.contains(&pos) || !dim.has(pos) {
                            continue;
                        }
                        let ready = crate::world::DIRS.iter().all(|d| {
                            let n = pos + *d;
                            dim.has(n) || !in_range(n) || n.y < def.min_cube_y || n.y > def.max_cube_y
                        });
                        if ready {
                            to_send.push((dx * dx + dz * dz + dy * dy, pos));
                        }
                    }
                }
            }
            to_send.sort_by_key(|t| t.0);
            for &(_, pos) in to_send.iter().take(512) {
                if let Some(cube) = dim.snapshot(pos) {
                    p.conn.send(ToClient::Cube { dim: p.dim, pos, cube });
                    p.sent.insert(pos);
                }
            }
            let far: Vec<IVec3> = p
                .sent
                .iter()
                .copied()
                .filter(|&pos| {
                    let d = pos - center;
                    d.x * d.x + d.z * d.z > (r + 2) * (r + 2) || d.y.abs() > h + 2
                })
                .collect();
            for pos in far {
                p.sent.remove(&pos);
                p.conn.send(ToClient::Unload { dim: p.dim, pos });
            }
            let _ = &mut unload;
        }
        for d in &mut self.dims {
            d.resend.clear();
            d.changes.clear();
        }
        self.unload_unused(c);
    }

    fn unload_unused(&mut self, c: &Content) {
        if self.tick % 40 != 0 {
            return;
        }
        for (di, dim) in self.dims.iter_mut().enumerate() {
            let views: Vec<(IVec3, i32, i32)> = self
                .players
                .iter()
                .filter(|p| p.joined && p.dim as usize == di)
                .map(|p| ((p.pos.floor().as_ivec3()) >> 4, self.cfg.view_radius + 3, self.cfg.view_height + 3))
                .collect();
            dim.retain_cubes(c, |pos| {
                views.iter().any(|&(center, r, h)| {
                    let d = pos - center;
                    d.x * d.x + d.z * d.z <= r * r && d.y.abs() <= h
                })
            });
        }
    }
}

fn within_reach(player: DVec3, block: IVec3) -> bool {
    (block.as_dvec3() + DVec3::splat(0.5) - player).length_squared() < 8.0 * 8.0
}
