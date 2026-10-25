//! Authoritative simulation. Runs on its own thread; single player talks to it through the
//! in-process transport. Each dimension is a set of regions ticked in parallel.

mod builtin;
mod dim;
pub mod energy;
pub mod entity;
mod extend;
mod liquid;
pub mod path;
pub use builtin::COMMANDS as BUILTIN_COMMANDS;
pub use extend::{MAX_HEALTH, MOVEMENT_SPEED, explanation};
pub mod menu;
pub mod persist;
pub mod regions;
mod structure;
pub mod terrain;
pub use structure::origin as structure_origin;
mod visibility;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, unbounded};
use glam::{DVec3, IVec3};
use parking_lot::Mutex;
use rayon::prelude::*;
use rustc_hash::FxHashSet;
use weft_abi::{self as abi, ItemStack, component_flags as cf, damage};

pub use dim::{Dim, DimEvent, break_time};

use crate::content::{Content, DimensionDef, content};
use crate::net::{EntityView, ServerConn, ToClient, ToServer};
use crate::saves::Storage;
use crate::commands::{Origin, Selector};
use crate::world::{Cube, GameMode, cube_of};
use entity::Entity;
use menu::OpenMenu;
use regions::DimSet;

pub const TPS: u32 = 20;
const AUTOSAVE_TICKS: u64 = 20 * 30;
const REACH: f64 = 6.0;
/// Players further away than vanilla's particle distance get no particles.
const PARTICLE_RANGE: f64 = 32.0;

#[derive(Clone)]
pub struct ServerConfig {
    pub seed: u64,
    /// Horizontal view radius in cubes.
    pub view_radius: i32,
    /// Vertical view radius in cubes.
    pub view_height: i32,
}

pub struct Player {
    pub name: String,
    /// Public key: the identity saves are keyed by.
    pub key: String,
    conn: ServerConn,
    pub dim: u32,
    pub entity: u64,
    sent: FxHashSet<IVec3>,
    known: FxHashSet<u64>,
    joined: bool,
    menu: Option<OpenMenu>,
    window: u32,
    fall: f32,
    last_y: f64,
    dead: bool,
    first_sync: bool,
    /// View (dim, center, radius, height) whose cubes were all present at the last check, and the
    /// same plus the settled version when nothing was left to send. Lets idle ticks skip scanning
    /// the whole view volume.
    gen_done: Option<(u32, IVec3, i32, i32, u64)>,
    /// What the player can see; only these cubes (and the sky column) get loaded.
    vis: Option<visibility::Visibility>,
    /// Settled cubes in range not sent yet (waiting for their neighbors), and the range they
    /// were gathered for: only cubes newly settled or newly in range are looked at.
    unsent: FxHashSet<IVec3>,
    sync_range: Option<(IVec3, i32, i32, i32)>,
    /// Set by teleports: moves sent before the client saw the teleport are stale and would read
    /// as a long fall, so they are ignored until one arrives near this point.
    teleported: Option<DVec3>,
    /// Gizmo groups the client shows.
    gizmos: Vec<String>,
    /// Health and maximum as last sent.
    sent_health: (f32, f32),
    /// Burning entities this player was last told about (sorted).
    sent_burning: Vec<u64>,
}

pub struct Generated {
    dim: u32,
    pos: IVec3,
    /// `None`: dropped unmade, nobody's view reached it any more when its turn came.
    cube: Option<Cube>,
    entities: Vec<u8>,
    conn: u16,
}

/// Cubes around a player that are loaded whether visible or not, so digging and walking into
/// sealed ground always finds blocks there.
const NEAR: i32 = 2;

/// State the client thread may read or poke while the server runs.
#[derive(Default)]
pub struct Shared {
    pub paused: AtomicBool,
    pub hosting: AtomicBool,
    pub tick_us: AtomicU64,
    pub view: AtomicI32,
    pub regions: AtomicU64,
    /// Load pipeline counters for profiling: cubes generating or waiting for insertion, cubes a
    /// player should get but has not yet, and work done so far.
    pub pending: AtomicU64,
    pub unsent: AtomicU64,
    pub generated: AtomicU64,
    pub gen_ns: AtomicU64,
    pub inserted: AtomicU64,
    pub insert_ns: AtomicU64,
    /// Last tick split into `PHASES`, microseconds.
    pub phase_us: [AtomicU64; 6],
    /// Longest tick since a reader last swapped it out.
    pub tick_max_us: AtomicU64,
    /// Loaded cubes and live entities over all dimensions, refreshed every 10 ticks.
    pub cubes: AtomicU64,
    pub entities: AtomicU64,
    /// Where players load cubes (dim, center, radius, up, down), for generation jobs to skip
    /// cubes a fast player already left behind.
    pub views: parking_lot::RwLock<Vec<(u32, IVec3, i32, i32, i32)>>,
}

pub const PHASES: [&str; 6] = ["insert", "messages", "regions", "request", "sync", "rest"];

/// Cubes sent to one player per tick; a client meshes about this many per frame anyway.
const SEND_PER_TICK: usize = 8192;

pub struct Server {
    pub cfg: ServerConfig,
    pub dims: Vec<DimSet>,
    pub players: Vec<Player>,
    pub tick: u64,
    /// `/tick`: ticks per second, the world held still (players still move), ticks left to step
    /// while frozen, and a sprint (ticks left, ticks in all, started) run without waiting.
    pub tick_rate: u32,
    pub frozen: bool,
    pub steps: u32,
    pub sprint: Option<(u32, u32, Instant)>,
    gen_tx: Sender<Generated>,
    gen_rx: Receiver<Generated>,
    incoming: Receiver<ServerConn>,
    /// Generated cubes waiting for insertion (lighting is budgeted per tick).
    backlog: std::collections::VecDeque<Generated>,
    /// Cubes some player still needs, as of the last request: a cube whose upper neighbor is
    /// among them waits for it, or it would be lit dark and relit when the sky arrives.
    wanted: FxHashSet<(u32, IVec3)>,
    storage: Option<Arc<Mutex<Storage>>>,
    shared: Arc<Shared>,
    next_id: Arc<AtomicU64>,
    /// Names last sent to clients as the online list.
    online: Vec<String>,
    /// Generated and inserted totals at the last `load` report.
    load_seen: (u64, u64),
    /// World rule values by rule id.
    rules: Arc<[i64]>,
}

/// Handle the client side uses to reach a running server thread.
pub struct ServerHandle {
    pub connect: Sender<ServerConn>,
    pub shared: Arc<Shared>,
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

    pub fn set_paused(&self, p: bool) {
        self.shared.paused.store(p, Ordering::Relaxed);
    }

    pub fn is_hosting(&self) -> bool {
        self.shared.hosting.load(Ordering::Relaxed)
    }

    pub fn tick_ms(&self) -> f32 {
        self.shared.tick_us.load(Ordering::Relaxed) as f32 / 1000.0
    }

    /// Nothing left to generate, insert or send.
    pub fn idle(&self) -> bool {
        self.shared.pending.load(Ordering::Relaxed) == 0 && self.shared.unsent.load(Ordering::Relaxed) == 0
    }

    /// Cubes generated and their average cost on a worker, cubes inserted and their average
    /// cost (lighting) on the server thread, in microseconds.
    pub fn load_stats(&self) -> (u64, f64, u64, f64) {
        let s = &self.shared;
        let (g, i) = (s.generated.load(Ordering::Relaxed), s.inserted.load(Ordering::Relaxed));
        let avg = |ns: u64, n: u64| ns as f64 / n.max(1) as f64 / 1000.0;
        (g, avg(s.gen_ns.load(Ordering::Relaxed), g), i, avg(s.insert_ns.load(Ordering::Relaxed), i))
    }

    pub fn phases(&self) -> [f32; 6] {
        std::array::from_fn(|i| self.shared.phase_us[i].load(Ordering::Relaxed) as f32 / 1000.0)
    }

    pub fn regions(&self) -> u32 {
        self.shared.regions.load(Ordering::Relaxed) as u32
    }

    pub fn set_view(&self, radius: i32) {
        self.shared.view.store(radius, Ordering::Relaxed);
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn spawn(cfg: ServerConfig, storage: Option<Storage>) -> ServerHandle {
    let (connect, incoming) = unbounded();
    let (stop, stop_rx) = unbounded::<()>();
    let shared = Arc::new(Shared::default());
    shared.view.store(cfg.view_radius, Ordering::Relaxed);
    let sh = shared.clone();
    let thread = std::thread::Builder::new()
        .name("server".into())
        .spawn(move || {
            let mut server = Server::new(cfg, incoming, storage, sh);
            let mut next = Instant::now();
            loop {
                if stop_rx.try_recv().is_ok() {
                    break;
                }
                if server.shared.paused.load(Ordering::Relaxed) {
                    server.idle();
                } else {
                    server.tick();
                }
                if server.sprint.is_some() {
                    next = Instant::now();
                    continue;
                }
                let step = Duration::from_secs_f64(1.0 / server.tick_rate.max(1) as f64);
                next += step;
                // The rest of the tick goes to loading instead of sleeping.
                server.load_until(next.checked_sub(Duration::from_millis(2)).unwrap_or(next));
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
    ServerHandle { connect, shared, stop, thread: Some(thread) }
}

/// Cubes kept loaded around a view: the visible range plus, above it, the column up to where the
/// generator guarantees open sky (capped), so sky light reaches the visible cubes without having
/// to send or render those extra cubes.
fn loaded_y_range(def: &DimensionDef, center_y: i32, h: i32, down: i32) -> (i32, i32) {
    let lo = (center_y - down).max(def.min_cube_y);
    let top = center_y + h;
    let hi = if def.sky_light && def.sky_cube_y != i32::MAX { top.max((def.sky_cube_y - 1).min(top + 12)) } else { top };
    (lo, hi.min(def.max_cube_y))
}

impl Server {
    pub fn new(cfg: ServerConfig, incoming: Receiver<ServerConn>, storage: Option<Storage>, shared: Arc<Shared>) -> Server {
        let c = content();
        let next_id = Arc::new(AtomicU64::new(storage.as_ref().map_or(1, |s| s.meta.next_entity.max(1))));
        let mut dims: Vec<DimSet> = (0..c.dims.len() as u32).map(|i| DimSet::new(i, cfg.seed, next_id.clone())).collect();
        if let Some(s) = &storage {
            for (name, t) in &s.meta.times {
                if let Some(&i) = c.dim_names.get(name) {
                    dims[i as usize].set_time(*t);
                }
            }
        }
        let (gen_tx, gen_rx) = unbounded();
        let rules: Arc<[i64]> = c
            .game
            .rules
            .iter()
            .map(|(name, default)| storage.as_ref().and_then(|s| s.meta.rules.iter().find(|r| &r.0 == name)).map_or(*default, |r| r.1))
            .collect();
        Server {
            rules,
            cfg,
            dims,
            players: Vec::new(),
            tick: 0,
            gen_tx,
            gen_rx,
            incoming,
            backlog: Default::default(),
            wanted: FxHashSet::default(),
            storage: storage.map(|s| Arc::new(Mutex::new(s))),
            shared,
            next_id,
            online: Vec::new(),
            tick_rate: TPS,
            frozen: false,
            steps: 0,
            sprint: None,
            load_seen: (0, 0),
        }
    }

    /// Inserts generated cubes and keeps the workers fed until `deadline`, between ticks.
    fn load_until(&mut self, deadline: Instant) {
        let c = content();
        let mut refilled = Instant::now();
        loop {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let busy = |s: &Self| !s.backlog.is_empty() || s.dims.iter().any(|d| !d.generating.is_empty());
            if !busy(self) {
                // What just arrived may have opened up more to load: look before sleeping.
                self.request_generation(c);
                refilled = Instant::now();
                if !busy(self) {
                    break;
                }
            }
            let before = self.backlog.len();
            self.receive_generated(c, deadline);
            // Keep the queue full without redoing the visibility walk on every pass.
            if now - refilled > Duration::from_millis(5) {
                self.request_generation(c);
                refilled = Instant::now();
            }
            if self.backlog.len() == before {
                // Nothing could be inserted: wait for the next cube from the workers.
                match self.gen_rx.recv_timeout((deadline - Instant::now().min(deadline)).min(Duration::from_millis(2))) {
                    Ok(g) => self.backlog.push_back(g),
                    Err(_) => continue,
                }
            }
        }
    }

    /// Paused single player: keep talking to the client but do not advance the world.
    fn idle(&mut self) {
        let c = content();
        self.accept();
        self.receive_generated(c, Instant::now() + Duration::from_millis(15));
        self.handle_messages(c);
        self.sync_players(c);
    }

    fn shutdown(&mut self) {
        for p in &mut self.players {
            p.conn.send(ToClient::Disconnect("server closed".into()));
        }
        self.save_all(true);
    }

    fn accept(&mut self) {
        while let Ok(conn) = self.incoming.try_recv() {
            self.players.push(Player {
                name: String::new(),
                key: String::new(),
                conn,
                dim: 0,
                entity: 0,
                sent: FxHashSet::default(),
                known: FxHashSet::default(),
                joined: false,
                menu: None,
                window: 0,
                fall: 0.0,
                last_y: 0.0,
                dead: false,
                first_sync: true,
                gen_done: None,
                unsent: FxHashSet::default(),
                sync_range: None,
                vis: None,
                teleported: None,
                gizmos: Vec::new(),
                sent_health: (-1.0, -1.0),
                sent_burning: Vec::new(),
            });
        }
    }

    pub fn tick(&mut self) {
        let start = Instant::now();
        let c = content();
        self.cfg.view_radius = self.shared.view.load(Ordering::Relaxed);
        let mut phase = Instant::now();
        let mut lap = |i: usize, shared: &Shared| {
            let now = Instant::now();
            shared.phase_us[i].store((now - phase).as_micros() as u64, Ordering::Relaxed);
            phase = now;
        };
        self.accept();
        self.receive_generated(c, start + Duration::from_millis(15));
        lap(0, &self.shared);
        self.handle_messages(c);
        lap(1, &self.shared);

        // Every region of every dimension is independent: tick them all in parallel. A frozen
        // world only runs the ticks it is stepped.
        let runs = !self.frozen || self.steps > 0 || self.sprint.is_some();
        self.steps -= (self.frozen && self.steps > 0) as u32;
        for d in self.dims.iter_mut().filter(|_| runs) {
            let mut groups: Vec<String> = self.players.iter().filter(|p| p.dim == d.id).flat_map(|p| p.gizmos.iter().cloned()).collect();
            groups.sort();
            groups.dedup();
            let groups: Arc<[String]> = groups.into();
            let t = d.time;
            for r in &mut d.regions {
                r.time = t;
                r.gizmo_groups = groups.clone();
                r.rules = self.rules.clone();
            }
            d.time += 1;
        }
        if runs {
            self.dims.par_iter_mut().flat_map(|d| d.regions.par_iter_mut()).for_each(|r| r.tick(c));
        }
        lap(2, &self.shared);
        if let Some((left, all, start)) = &mut self.sprint {
            *left -= 1;
            if *left == 0 {
                let note = format!("sprinted {all} ticks in {} ms", start.elapsed().as_millis());
                for p in &mut self.players {
                    p.conn.send(ToClient::Chat(note.clone()));
                }
                self.sprint = None;
            }
        }

        self.dim_events(c);
        for pi in 0..self.players.len() {
            let Server { players, dims, .. } = self;
            let p = &mut players[pi];
            if let (Some(m), Some(r)) = (&mut p.menu, dims[p.dim as usize].entity_region(p.entity)) {
                menu::tick(m, r, c, p.entity);
            }
        }
        self.request_generation(c);
        lap(3, &self.shared);
        self.sync_players(c);
        lap(4, &self.shared);
        self.players.retain(|p| !p.conn.is_closed());
        let online: Vec<String> = self.players.iter().filter(|p| p.joined).map(|p| p.name.clone()).collect();
        if online != self.online {
            for p in self.players.iter_mut().filter(|p| p.joined) {
                p.conn.send(ToClient::Players(online.clone()));
            }
            self.online = online;
        }
        if self.tick % 10 == 0 {
            for d in &mut self.dims {
                if d.dirty || self.tick % 100 == 0 {
                    d.regionize(c);
                }
            }
            let n: usize = self.dims.iter().map(|d| d.regions.len()).sum();
            self.shared.regions.store(n as u64, Ordering::Relaxed);
            let regions = || self.dims.iter().flat_map(|d| d.regions.iter());
            self.shared.cubes.store(regions().map(|r| r.cubes.len() as u64).sum(), Ordering::Relaxed);
            self.shared.entities.store(regions().map(|r| r.entities.list.iter().filter(|e| !e.removed).count() as u64).sum(), Ordering::Relaxed);
        }
        if self.tick % TPS as u64 == 0 && crate::log::on(crate::log::Channel::Load, log::Level::Debug) {
            self.report_load();
        }
        self.tick += 1;
        if self.tick % AUTOSAVE_TICKS == 0 {
            self.save_all(false);
        }
        lap(5, &self.shared);
        let us = start.elapsed().as_micros() as u64;
        self.shared.tick_us.store(us, Ordering::Relaxed);
        self.shared.tick_max_us.fetch_max(us, Ordering::Relaxed);
    }

    /// `load` channel: where the pipeline stands; at trace level also why waiting cubes wait.
    fn report_load(&mut self) {
        let sh = &self.shared;
        let (made, put) = (sh.generated.load(Ordering::Relaxed), sh.inserted.load(Ordering::Relaxed));
        let (made_before, put_before) = std::mem::replace(&mut self.load_seen, (made, put));
        let generating: usize = self.dims.iter().map(|d| d.generating.len()).sum();
        log::debug!(
            target: "load",
            "generating {generating}, waiting to insert {}, wanted {}, unsent {} | last second: {} generated, {} inserted",
            self.backlog.len(),
            self.wanted.len(),
            sh.unsent.load(Ordering::Relaxed),
            made - made_before,
            put - put_before,
        );
        if !crate::log::on(crate::log::Channel::Load, log::Level::Trace) || self.backlog.is_empty() {
            return;
        }
        // Same rule as `receive_generated`: a cube waits for the one above it.
        let queued: FxHashSet<(u32, IVec3)> = self.backlog.iter().map(|g| (g.dim, g.pos)).collect();
        let (mut above_queued, mut above_generating, mut above_unrequested, mut ready) = (0, 0, 0, 0);
        let mut stuck = Vec::new();
        for g in &self.backlog {
            let above = (g.dim, g.pos + IVec3::Y);
            if queued.contains(&above) {
                above_queued += 1;
            } else if self.dims[g.dim as usize].generating.contains(&above.1) {
                above_generating += 1;
            } else if self.wanted.contains(&above) {
                above_unrequested += 1;
                if stuck.len() < 5 {
                    stuck.push(format!("{} {}", g.dim, g.pos));
                }
            } else {
                ready += 1;
            }
        }
        log::trace!(
            target: "load",
            "waiting: {above_queued} for a queued cube above, {above_generating} for one generating, {above_unrequested} for one not requested yet {stuck:?}, {ready} ready (out of time)"
        );
    }

    fn dim_events(&mut self, c: &'static Content) {
        let mut events = Vec::new();
        for d in &mut self.dims {
            for r in &mut d.regions {
                events.append(&mut r.events);
            }
        }
        // Particles and sounds of the whole tick go out as one message of each per player.
        let mut bursts: Vec<Vec<abi::ParticleBurst>> = vec![Vec::new(); self.players.len()];
        let mut sounds: Vec<Vec<(u32, DVec3, f32, f32)>> = vec![Vec::new(); self.players.len()];
        for ev in events {
            match ev {
                DimEvent::Animation { id, name, looping } => {
                    for p in self.players.iter_mut().filter(|p| p.joined && (p.known.contains(&id) || p.entity == id)) {
                        p.conn.send(ToClient::Animate(vec![(id, name.clone(), looping)]));
                    }
                }
                DimEvent::Sound { dim, sound, pos: at, volume, pitch } => {
                    let range = c.game.sounds.get(sound as usize).map_or(16.0, |d| d.range * volume.max(1.0)) as f64;
                    for (pi, p) in self.players.iter().enumerate() {
                        let pos = self.dims[p.dim as usize].entity_region_ref(p.entity).and_then(|r| r.entities.get(p.entity)).map(|e| e.pos);
                        if p.joined && p.dim == dim && pos.is_some_and(|pos| (pos - at).length_squared() < range * range) {
                            sounds[pi].push((sound, at, volume, pitch));
                        }
                    }
                }
                DimEvent::Particles { dim, burst } => {
                    let at = DVec3::from_array(burst.pos);
                    for (pi, p) in self.players.iter().enumerate() {
                        let pos = self.dims[p.dim as usize].entity_region_ref(p.entity).and_then(|r| r.entities.get(p.entity)).map(|e| e.pos);
                        if p.joined && p.dim == dim && pos.is_some_and(|pos| (pos - at).length_squared() < PARTICLE_RANGE * PARTICLE_RANGE) {
                            bursts[pi].push(burst);
                        }
                    }
                }
                DimEvent::OpenMenu { player, menu, owner } => {
                    if let Some(pi) = self.players.iter().position(|p| p.entity == player) {
                        self.open_menu(c, pi, menu, owner);
                    }
                }
                DimEvent::Died { player } => {
                    let Server { players, dims, .. } = self;
                    if let Some(p) = players.iter_mut().find(|p| p.entity == player) {
                        p.dead = true;
                        if let (Some(m), Some(r)) = (p.menu.take(), dims[p.dim as usize].entity_region(player)) {
                            menu::close(m, r, c, player);
                            p.conn.send(ToClient::CloseMenu);
                        }
                        p.sent_health.0 = 0.0;
                        p.conn.send(ToClient::Health { health: 0.0, max: p.sent_health.1 });
                        p.conn.send(ToClient::Died);
                        log::info!("{} died", p.name);
                    }
                }
                DimEvent::Packet { player, channel, data } => {
                    if let Some(p) = self.players.iter_mut().find(|p| p.joined && p.entity == player) {
                        p.conn.send(ToClient::Packet { channel, data });
                    }
                }
                DimEvent::Teleport { from, id, dim, pos } => {
                    let to = if dim == u32::MAX { from } else { dim };
                    if (to as usize) < self.dims.len() && pos.is_finite() {
                        self.teleport(c, from, id, to, pos);
                    }
                }
                DimEvent::SetTime { dim, time } => self.dims[dim as usize].set_time(time),
                DimEvent::SetMode { player, mode } => {
                    if let Some(pi) = self.player_index(player) {
                        self.set_mode(pi, mode);
                    }
                }
            }
        }
        for ((p, list), sounds) in self.players.iter_mut().zip(bursts).zip(sounds) {
            if !list.is_empty() {
                p.conn.send(ToClient::Particles(list));
            }
            if !sounds.is_empty() {
                p.conn.send(ToClient::Sounds(sounds));
            }
        }
    }

    fn open_menu(&mut self, c: &'static Content, pi: usize, kind: u32, owner: u64) {
        if kind as usize >= c.game.menus.len() {
            return;
        }
        let Server { players, dims, .. } = self;
        let p = &mut players[pi];
        let Some(r) = dims[p.dim as usize].entity_region(p.entity) else { return };
        if let Some(m) = p.menu.take() {
            menu::close(m, r, c, p.entity);
        }
        p.window += 1;
        let m = menu::open(r, c, p.entity, kind, owner);
        p.conn.send(ToClient::OpenMenu { kind, window: p.window, slots: m.slots.len() as u32, owner });
        p.menu = Some(m);
    }

    fn save_all(&mut self, players: bool) {
        let Some(storage) = self.storage.clone() else { return };
        let c = content();
        let t = Instant::now();
        let mut s = storage.lock();
        let mut n = 0;
        for d in &mut self.dims {
            for r in &mut d.regions {
                // Cubes where something happened, and the ones entities are in (they move and
                // spawn in untouched cubes too).
                let mut cubes: Vec<IVec3> = std::mem::take(&mut r.modified).into_iter().collect();
                cubes.extend(r.entities.list.iter().filter(|e| !e.removed && !e.player && e.kind != c.game.virtual_kind).map(|e| cube_of(e.pos.floor().as_ivec3())));
                cubes.sort_unstable_by_key(|p| (p.x, p.y, p.z));
                cubes.dedup();
                for pos in cubes {
                    if let Some(cube) = r.cubes.get(&pos) {
                        let bytes = entities_in(c, r, pos);
                        s.save(c, r.id, pos, cube, &bytes);
                        n += 1;
                    }
                }
            }
        }
        s.meta.times = self.dims.iter().map(|d| (c.dims[d.id as usize].name.clone(), d.time)).collect();
        s.meta.rules = c.game.rules.iter().zip(self.rules.iter()).map(|(r, v)| (r.0.clone(), *v)).collect();
        s.meta.next_entity = self.next_id.load(Ordering::Relaxed);
        for p in &self.players {
            if p.joined || players {
                if let Some(r) = self.dims[p.dim as usize].entity_region_ref(p.entity) {
                    save_player(&s, p, r);
                }
            }
        }
        s.flush();
        if n > 0 {
            log::info!("saved {n} cubes in {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);
        }
    }

    fn receive_generated(&mut self, c: &'static Content, deadline: Instant) {
        self.backlog.extend(self.gen_rx.try_iter());
        // Top-down so sky light is right the first time: a cube waits while the one above it is
        // still generating or waiting itself. Sorted to keep runs reproducible.
        let mut list: Vec<Generated> = self.backlog.drain(..).collect();
        list.sort_by_key(|g| (g.dim, -g.pos.y, g.pos.x, g.pos.z));
        let mut pending: FxHashSet<(u32, IVec3)> = list.iter().map(|g| (g.dim, g.pos)).collect();
        for g in list {
            if g.cube.is_none() {
                self.dims[g.dim as usize].generating.remove(&g.pos);
                pending.remove(&(g.dim, g.pos));
                continue;
            }
            let above = (g.dim, g.pos + IVec3::Y);
            let late = Instant::now() >= deadline;
            let above_coming = pending.contains(&above) || self.dims[g.dim as usize].generating.contains(&above.1) || self.wanted.contains(&above);
            if late || above_coming {
                self.backlog.push_back(g);
                continue;
            }
            pending.remove(&(g.dim, g.pos));
            self.wanted.remove(&(g.dim, g.pos));
            let t = Instant::now();
            self.dims[g.dim as usize].insert(c, g.pos, g.cube.unwrap(), &g.entities, g.conn, true);
            self.shared.insert_ns.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
            self.shared.inserted.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn handle_messages(&mut self, c: &'static Content) {
        for pi in 0..self.players.len() {
            while let Some(msg) = self.players[pi].conn.recv() {
                self.handle(c, pi, msg);
            }
        }
    }

    /// The region holding player `pi`'s entity, with the entity id.
    fn region(&mut self, pi: usize) -> Option<(&mut Dim, u64)> {
        let p = &self.players[pi];
        let id = p.entity;
        self.dims[p.dim as usize].entity_region(id).map(|r| (r, id))
    }

    fn handle(&mut self, c: &'static Content, pi: usize, msg: ToServer) {
        match msg {
            ToServer::Join { name, key } => self.join(c, pi, name, key),
            ToServer::Move { pos, yaw, pitch, on_ground, crouched, frame } => {
                if !pos.is_finite() || self.players[pi].dead {
                    return;
                }
                // On a body: where the client has the player on it, on the body as it is here.
                let dim = &self.dims[self.players[pi].dim as usize];
                let on = frame.filter(|f| f.1.is_finite()).and_then(|(b, rel)| dim.entity_region_ref(b).and_then(|r| r.entities.get(b)).map(|e| (b, e.pos + rel)));
                let pos = on.map_or(pos, |o| o.1);
                let p = &mut self.players[pi];
                if let Some(t) = p.teleported {
                    if (pos - t).length_squared() > 16.0 {
                        return;
                    }
                    p.teleported = None;
                    p.last_y = pos.y;
                }
                let dy = pos.y - p.last_y;
                p.last_y = pos.y;
                let mut land = None;
                if on_ground {
                    if p.fall > entity::SAFE_FALL {
                        land = Some(p.fall - entity::SAFE_FALL);
                    }
                    p.fall = 0.0;
                } else if dy < 0.0 {
                    p.fall += (-dy) as f32;
                }
                let Some((r, entity)) = self.region(pi) else { return };
                let in_liquid = r.get(pos.floor().as_ivec3()).is_some_and(|s| c.states[s as usize].flags & abi::block_flags::LIQUID != 0);
                if in_liquid {
                    land = None;
                }
                if let Some(e) = r.entities.get_mut(entity) {
                    e.moved = ((pos.x - e.pos.x).powi(2) + (pos.z - e.pos.z).powi(2)).sqrt() as f32;
                    e.pos = pos;
                    e.support = on.map(|o| o.0);
                    e.yaw = yaw;
                    e.pitch = pitch;
                    e.on_ground = on_ground;
                    e.in_liquid = in_liquid;
                    e.crouched = crouched;
                }
                if let Some(amount) = land {
                    r.damage(c, entity, amount, 0, damage::FALL);
                }
                if in_liquid {
                    self.players[pi].fall = 0.0;
                }
            }
            ToServer::Break { .. } if self.mode(pi) == GameMode::Spectator => {}
            ToServer::Break { pos, frame } => {
                let Some((r, entity)) = self.region(pi) else { return };
                let Some(e) = r.entities.get(entity) else { return };
                let (eye, sel, creative) = (e.pos + DVec3::Y * e.eye(c), e.selected, e.mode == GameMode::Creative);
                let held = r.entities.inventories.get(&entity).and_then(|i| i.get(sel as usize)).copied().unwrap_or_default();
                let worn = r.with_space(c, frame, |d, o| {
                    if !within(eye - o, pos) {
                        return false;
                    }
                    let free = c.states.get(d.get(pos).unwrap_or(0) as usize).is_some_and(|s| s.hardness <= 0.0);
                    let done = d.break_block(c, pos, held, entity, !creative);
                    d.flush_neighbors(c);
                    d.finish_tick(c);
                    done && !free
                });
                if worn == Some(true) {
                    wear(c, r, entity, sel, 1);
                }
            }
            ToServer::Use { .. } if self.mode(pi) == GameMode::Spectator => {}
            ToServer::Use { pos, face, look, sneak, frame } => {
                enum Did {
                    Used,
                    Placed,
                    Nothing,
                    NoBlock,
                }
                let Some((r, entity)) = self.region(pi) else { return };
                let Some(e) = r.entities.get(entity) else { return };
                let (eye, sel, player_box, creative) = (e.pos + DVec3::Y * e.eye(c), e.selected, e.aabb(c), e.mode == GameMode::Creative);
                let held = r.entities.inventories.get(&entity).and_then(|i| i.get(sel as usize)).copied().unwrap_or_default();
                let did = r.with_space(c, frame, |d, o| {
                    if !within(eye - o, pos) {
                        return Did::Nothing;
                    }
                    if !sneak && d.player_use(c, pos, face, entity) {
                        return Did::Used;
                    }
                    let Some(state) = c.items.get(held.item as usize).and_then(|i| i.block).filter(|_| !held.is_empty()) else { return Did::NoBlock };
                    let target = if d.get(pos).is_some_and(|s| c.states[s as usize].flags & abi::block_flags::REPLACEABLE != 0) {
                        pos
                    } else {
                        pos + crate::world::DIRS[face as usize]
                    };
                    let block_box = crate::physics::Aabb { min: target.as_dvec3(), max: target.as_dvec3() + DVec3::ONE };
                    let solid = c.states[state as usize].flags & abi::block_flags::SOLID != 0;
                    if (!solid || !player_box.shifted(-o).intersects(&block_box)) && d.place(c, target, face, state, look, entity, held.item) {
                        Did::Placed
                    } else {
                        Did::Nothing
                    }
                });
                match did {
                    Some(Did::Used) => {
                        self.dim_events(c);
                        self.players[pi].conn.send(ToClient::Swing);
                    }
                    Some(Did::Placed) => {
                        if !creative {
                            r.entities.set_slot(entity, sel, ItemStack { count: held.count - 1, ..held });
                        }
                        self.players[pi].conn.send(ToClient::Swing);
                    }
                    // Items used on a block (buckets) work on the world's blocks only, so far.
                    Some(Did::NoBlock) => self.handle(c, pi, ToServer::UseItem { target: (frame == 0).then_some((pos, face)) }),
                    _ => {}
                }
            }
            ToServer::UseItem { .. } if self.mode(pi) == GameMode::Spectator => {}
            ToServer::UseItem { target } => {
                let Some((r, entity)) = self.region(pi) else { return };
                let Some(sel) = r.entities.get(entity).map(|e| e.selected) else { return };
                let held = r.entities.inventories.get(&entity).and_then(|i| i.get(sel as usize)).copied().unwrap_or_default();
                let Some(def) = c.items.get(held.item as usize) else { return };
                let mut used = false;
                if def.on_use.is_none() {
                    if let Some(after) = r.use_container(c, entity, held) {
                        if after != held {
                            r.entities.set_slot(entity, sel, after);
                            used = true;
                        }
                    }
                } else if let Some(cb) = def.on_use {
                    let (pos, face) = target.unwrap_or((IVec3::ZERO, 0));
                    let a = r.call(def.owner, cb, |w| abi::ItemUseArgs {
                        world: w,
                        player: entity,
                        stack: held,
                        pos: pos.to_array(),
                        face: face as u32,
                        has_target: target.is_some() as u32,
                        handled: 0,
                    });
                    if a.handled != 0 && a.stack != held {
                        r.entities.set_slot(entity, sel, a.stack);
                    }
                    used = a.handled != 0;
                }
                r.flush_neighbors(c);
                r.finish_tick(c);
                if used {
                    self.players[pi].conn.send(ToClient::Swing);
                }
            }
            ToServer::Pick { item } => {
                let Some((r, entity)) = self.region(pi) else { return };
                let Some(e) = r.entities.get(entity) else { return };
                let (sel, creative) = (e.selected, e.mode == GameMode::Creative);
                let Some(inv) = r.entities.inventories.get(&entity) else { return };
                let current = inv.get(sel as usize).copied().unwrap_or_default();
                if let Some(j) = c.game.player_layout.main().map(|j| j as usize).find(|&j| inv.get(j).is_some_and(|s| s.item == item && !s.is_empty())) {
                    let found = inv[j];
                    r.entities.set_slot(entity, j as u32, current);
                    r.entities.set_slot(entity, sel, found);
                } else if creative && c.items.get(item as usize).is_some() {
                    r.entities.set_slot(entity, sel, ItemStack::new(item, 1));
                }
            }
            ToServer::Select(slot) => {
                if slot < c.game.player_layout.hotbar {
                    if let Some((r, entity)) = self.region(pi) {
                        if let Some(e) = r.entities.get_mut(entity) {
                            e.selected = slot;
                        }
                    }
                }
            }
            ToServer::Attack(_) if self.mode(pi) == GameMode::Spectator => {}
            ToServer::Attack(target) => {
                let Some((r, entity)) = self.region(pi) else { return };
                let (Some(me), Some(t)) = (r.entities.get(entity), r.entities.get(target)) else { return };
                if (me.pos - t.pos).length_squared() > REACH * REACH || target == entity || !t.item.is_empty() || t.anchor.is_some() {
                    return;
                }
                let sel = me.selected;
                let held = r.entities.inventories.get(&entity).and_then(|i| i.get(sel as usize)).copied().unwrap_or_default();
                let mut q = abi::Query { entity, other: target, item: held.item, ..abi::Query::DEFAULT };
                if !r.ask(c, abi::question::ATTACK, &mut q) {
                    return;
                }
                let base = c.items.get(held.item as usize).filter(|_| held.item != 0).map_or(1.0, |i| i.attack);
                let dmg = r.compute(c, abi::chain::ATTACK_DAMAGE, base as f64, &q, false) as f32;
                if r.damage(c, target, dmg, entity, damage::ATTACK) {
                    wear(c, r, entity, sel, c.item_value::<u32>(held.item, c.ids.wear_per_hit).unwrap_or(2));
                }
            }
            ToServer::Interact(_) if self.mode(pi) == GameMode::Spectator => {}
            ToServer::Interact(target) => {
                let Some((r, entity)) = self.region(pi) else { return };
                let Some(kind) = r.entities.get(target).map(|e| e.kind) else { return };
                let mut handled = false;
                for &(owner, cb) in &c.game.entities[kind as usize].on_interact {
                    handled |= r.call(owner, cb, |w| abi::EntityEventArgs { world: w, entity: target, other: entity, amount: 0.0, cause: 0, handled: 0 }).handled != 0;
                }
                if handled {
                    self.players[pi].conn.send(ToClient::Swing);
                }
            }
            ToServer::OpenInventory => {
                if let Some(m) = c.game.player_menu {
                    self.open_menu(c, pi, m, 0);
                }
            }
            ToServer::Click { slot, button, shift } => {
                let Server { players, dims, .. } = self;
                let p = &mut players[pi];
                if let (Some(m), Some(r)) = (&mut p.menu, dims[p.dim as usize].entity_region(p.entity)) {
                    menu::click(m, r, c, p.entity, slot, button, shift);
                }
            }
            ToServer::CreativeTake { item, button } => {
                let Server { players, dims, .. } = self;
                let p = &mut players[pi];
                let creative = dims[p.dim as usize].entity_region_ref(p.entity).and_then(|r| r.entities.get(p.entity)).is_some_and(|e| e.mode == GameMode::Creative);
                if let (true, Some(m), true) = (creative, &mut p.menu, item != 0 && (item as usize) < c.items.len()) {
                    // Like vanilla: the same item adds one, anything else held is thrown away
                    // first, an empty hand takes a stack (left) or one (right).
                    let max = c.max_stack(item);
                    m.cursor = if m.cursor.is_empty() {
                        ItemStack::new(item, if button == 1 { 1 } else { max })
                    } else if m.cursor.item == item && m.cursor.data == 0 {
                        ItemStack { count: (m.cursor.count + 1).min(max), ..m.cursor }
                    } else {
                        ItemStack::EMPTY
                    };
                }
            }
            ToServer::Drag { slots, button } => {
                let Server { players, dims, .. } = self;
                let p = &mut players[pi];
                if let (Some(m), Some(r)) = (&mut p.menu, dims[p.dim as usize].entity_region(p.entity)) {
                    menu::drag(m, r, c, p.entity, &slots, button);
                }
            }
            ToServer::CloseMenu => {
                let Server { players, dims, .. } = self;
                let p = &mut players[pi];
                if let (Some(m), Some(r)) = (p.menu.take(), dims[p.dim as usize].entity_region(p.entity)) {
                    menu::close(m, r, c, p.entity);
                }
            }
            ToServer::Drop { .. } if self.mode(pi) == GameMode::Spectator => {}
            ToServer::Drop { all } => {
                let Some((r, entity)) = self.region(pi) else { return };
                let Some(e) = r.entities.get(entity) else { return };
                let (sel, from, yaw, pitch) = (e.selected, e.drop_point(c), e.yaw, e.pitch);
                let held = r.entities.inventories.get(&entity).and_then(|i| i.get(sel as usize)).copied().unwrap_or_default();
                if held.is_empty() {
                    return;
                }
                let n = if all { held.count } else { 1 };
                r.entities.set_slot(entity, sel, ItemStack { count: held.count - n, ..held });
                let id = r.spawn_item(c, from, ItemStack { count: n, ..held });
                if let Some(it) = r.entities.get_mut(id) {
                    let (sy, cy) = (yaw as f64).sin_cos();
                    let (sp, cp) = (pitch as f64).sin_cos();
                    it.vel = DVec3::new(-sy * cp, sp + 0.2, -cy * cp) * 6.0;
                    it.pickup_delay = 40;
                }
            }
            ToServer::Respawn => {
                if !self.players[pi].dead {
                    return;
                }
                self.players[pi].dead = false;
                self.players[pi].fall = 0.0;
                let spawn = self.dims[c.spawn_dim as usize].spawn_point(c);
                self.change_dim(c, pi, c.spawn_dim, spawn);
                if let Some((r, entity)) = self.region(pi) {
                    let ents = &mut r.entities;
                    for ((id, comp), bytes) in ents.comps.iter_mut() {
                        if *id == entity {
                            bytes.copy_from_slice(&c.game.components[*comp as usize].default);
                        }
                    }
                    // After the components: modifiers are gone with them. Sent as any change.
                    let max = r.max_health(c, entity);
                    if let Some(e) = r.entities.get_mut(entity) {
                        e.health = max;
                    }
                }
            }
            ToServer::Chat(text) => {
                let name = self.players[pi].name.clone();
                if let Some(cmd) = text.strip_prefix('/') {
                    let reply = self.command(c, pi, cmd);
                    self.players[pi].conn.send(ToClient::Chat(reply));
                } else {
                    log::info!("<{name}> {text}");
                    for p in &mut self.players {
                        p.conn.send(ToClient::Chat(format!("<{name}> {text}")));
                    }
                }
            }
            ToServer::Packet { channel, data } => {
                let Some((owner, cb)) = c.game.packets.get(channel as usize).and_then(|d| d.server) else { return };
                let Some((r, player)) = self.region(pi) else { return };
                let slice = abi::Slice::new(&data);
                r.call(owner, cb, |w| abi::PacketArgs { world: w, player, data: slice });
                r.flush_neighbors(c);
                r.finish_tick(c);
                self.dim_events(c);
            }
            ToServer::Gizmos(mut names) => {
                names.retain(|n| n.len() <= 64);
                names.truncate(64);
                self.players[pi].gizmos = names;
            }
            ToServer::Leave => {
                if !self.players[pi].joined {
                    return;
                }
                log::info!("{} left", self.players[pi].name);
                let Server { players, dims, storage, .. } = self;
                let p = &mut players[pi];
                if let Some(r) = dims[p.dim as usize].entity_region(p.entity) {
                    if let Some(m) = p.menu.take() {
                        menu::close(m, r, c, p.entity);
                    }
                    if let Some(s) = storage {
                        save_player(&s.lock(), p, r);
                    }
                    r.entities.remove(p.entity);
                }
                p.conn.send(ToClient::Disconnect("bye".into()));
                p.joined = false;
            }
        }
    }

    fn join(&mut self, c: &'static Content, pi: usize, name: String, key: String) {
        let key = if key.is_empty() { name.clone() } else { key };
        if self.players.iter().any(|p| p.joined && p.key == key) {
            self.players[pi].conn.send(ToClient::Disconnect(format!("{name} is already playing")));
            return;
        }
        let saved = self.storage.as_ref().and_then(|s| load_player(c, &s.lock(), &key));
        let spawn = || (c.spawn_dim, self.dims[c.spawn_dim as usize].spawn_point(c));
        let (dim, pos, health, inv, comps) = match saved {
            // A dimension that is gone (its mod removed), or the void while a real one exists:
            // everything else is kept, the player starts at the spawn.
            Some(s) => {
                let ((dim, pos), keep) = match s.0 {
                    Some(d) if d != c.void_dim || c.spawn_dim == c.void_dim => ((d, s.1), true),
                    _ => (spawn(), false),
                };
                if !keep {
                    log::warn!("player {name} was in a dimension that is not there now, placed at the spawn");
                }
                (dim, pos, s.2, s.3, s.4)
            }
            None => {
                let (d, p) = spawn();
                (d, p, -1.0, None, Vec::new())
            }
        };
        let time = self.dims[dim as usize].time;
        let r = self.dims[dim as usize].ensure_region(c, pos);
        let id = r.new_id();
        let mut e = Entity::new(id, c.game.player_kind, pos, health);
        e.player = true;
        e.mode = self.storage.as_ref().map_or(GameMode::Survival, |s| saved_mode(&s.lock(), &key));
        let mode = e.mode;
        r.entities.insert_with(c, e, inv, comps);
        mode_modifiers(c, r, id, mode);
        let worn = worn_of(c, &r.entities, id);
        // Known only now that the modifiers are back; a new player starts full.
        let max = r.max_health(c, id);
        if let Some(e) = r.entities.get_mut(id) {
            e.worn = worn;
            if e.health <= 0.0 {
                e.health = max;
            }
        }
        r.emit_engine(c, abi::event::PLAYER_JOINED, &abi::PlayerEvent { player: id });
        let p = &mut self.players[pi];
        p.name = name;
        p.key = key;
        p.joined = true;
        p.dim = dim;
        p.entity = id;
        p.last_y = pos.y;
        p.first_sync = true;
        log::info!("{} joined", p.name);
        p.conn.send(ToClient::Welcome { player: id, dim, pos, seed: self.cfg.seed });
        p.conn.send(ToClient::Configs(crate::config::synced()));
        p.conn.send(ToClient::Time { dim, time });
        p.conn.send(ToClient::GameMode(mode));
        if dim == c.void_dim {
            p.conn.send(ToClient::Chat(void_note(c)));
        }
    }

    fn set_mode(&mut self, pi: usize, mode: GameMode) {
        let (dim, entity) = (self.players[pi].dim, self.players[pi].entity);
        if let Some(r) = self.dims[dim as usize].entity_region(entity) {
            if let Some(e) = r.entities.get_mut(entity) {
                e.mode = mode;
            }
            mode_modifiers(content(), r, entity, mode);
        }
        self.players[pi].conn.send(ToClient::GameMode(mode));
    }

    fn mode(&self, pi: usize) -> GameMode {
        let p = &self.players[pi];
        self.dims[p.dim as usize].entity_region_ref(p.entity).and_then(|r| r.entities.get(p.entity)).map_or(GameMode::Survival, |e| e.mode)
    }

    /// Moves a player (with inventory and components) to a position, possibly in another
    /// dimension or region, without reloading anything.
    pub fn change_dim(&mut self, c: &'static Content, pi: usize, to: u32, pos: DVec3) {
        let Server { players, dims, .. } = self;
        let p = &mut players[pi];
        let from = p.dim as usize;
        if let Some(r) = dims[from].entity_region(p.entity) {
            if let Some(m) = p.menu.take() {
                menu::close(m, r, c, p.entity);
                p.conn.send(ToClient::CloseMenu);
            }
        }
        let taken = dims[from].entity_region(p.entity).and_then(|r| r.entities.take(p.entity));
        let mut frame = None;
        if let Some((mut e, inv, comps)) = taken {
            e.pos = pos;
            let id = e.id;
            let r = dims[to as usize].ensure_region(c, pos);
            let (hw, ..) = e.dims(c);
            frame = r.body_under(c, pos, hw).map(|(b, at)| (b, pos - at));
            e.support = frame.map(|f| f.0);
            r.entities.insert_with(c, e, inv, comps);
            // The client hears its inventory again (component changes are found by comparing).
            r.entities.dirty_inv.insert(id);
        }
        if p.dim != to {
            p.dim = to;
            p.sent.clear();
            p.gen_done = None;
            p.unsent.clear();
            p.sync_range = None;
            p.vis = None;
            p.known.clear();
        }
        p.last_y = pos.y;
        p.fall = 0.0;
        p.teleported = Some(pos);
        p.conn.send(ToClient::Teleport { dim: to, pos, frame });
        p.conn.send(ToClient::Time { dim: to, time: dims[to as usize].time });
        if to == c.void_dim && from != to as usize {
            p.conn.send(ToClient::Chat(void_note(c)));
        }
    }

    fn command(&mut self, c: &'static Content, pi: usize, line: &str) -> String {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        let Some(&name) = tokens.first() else { return String::new() };
        let Some(def) = c.commands.iter().find(|d| d.name == name) else { return format!("unknown command /{name}, try /help") };
        if def.client {
            return format!("/{name} runs on the client that types it");
        }
        let (dim, entity) = (self.players[pi].dim, self.players[pi].entity);
        let here = self.dims[dim as usize]
            .entity_region_ref(entity)
            .and_then(|r| r.entities.get(entity))
            .map_or_else(Origin::default, |e| Origin { pos: e.pos, yaw: e.yaw, pitch: e.pitch });
        let players: Vec<String> = self.players.iter().filter(|p| p.joined).map(|p| p.name.clone()).collect();
        let (args, handler) = match def.parse(c, &tokens[1..], here, &players) {
            Ok(a) => a,
            Err(e) => return format!("{e}. usage: /{} {}", def.name, def.usage),
        };
        // Selectors become entity ids before mods see them.
        let selected: Vec<Option<String>> = args
            .values
            .iter()
            .map(|v| match v {
                Some(crate::commands::Value::Targets(s)) => Some(self.select(pi, s, here.pos).iter().map(|t| t.1.to_string()).collect::<Vec<_>>().join(" ")),
                _ => None,
            })
            .collect();
        match handler {
            None => self.builtin(c, pi, name, &args, here.pos),
            Some((owner, cb)) => {
                let Some((r, player)) = self.region(pi) else { return String::new() };
                let values = crate::commands::abi_values(&args, &selected);
                let mut reply = String::new();
                let reply_ptr = &mut reply as *mut String as *mut core::ffi::c_void;
                r.call(owner, cb, |w| abi::CommandArgs { world: w, player, values: values.as_ptr(), count: values.len() as u32, reply: reply_ptr, client: std::ptr::null_mut() });
                r.flush_neighbors(c);
                r.finish_tick(c);
                reply
            }
        }
    }

    /// Entities a selector picks as (dimension, id), in the order it asks for. `@e` looks in the
    /// runner's dimension, player selectors in all of them.
    fn select(&self, pi: usize, s: &Selector, here: DVec3) -> Vec<(u32, u64)> {
        let me = &self.players[pi];
        let mut found: Vec<(u32, u64, f64)> = Vec::new();
        let mut consider = |dim: u32, e: &Entity| {
            if let Some((kind, neg)) = s.kind {
                if (e.kind == kind) == neg {
                    return;
                }
            }
            let d = if dim == me.dim { (e.pos - here).length() } else { f64::INFINITY };
            if s.distance.is_some_and(|(lo, hi)| d < lo || d > hi) {
                return;
            }
            found.push((dim, e.id, d));
        };
        if s.base == 'e' {
            for r in &self.dims[me.dim as usize].regions {
                for e in r.entities.list.iter().filter(|e| !e.removed) {
                    if s.name.as_ref().is_none_or(|n| self.players.iter().any(|p| p.joined && p.entity == e.id && &p.name == n)) {
                        consider(me.dim, e);
                    }
                }
            }
        } else {
            for p in self.players.iter().filter(|p| p.joined) {
                let mine = p.entity == me.entity;
                if (s.base == 's' && !mine) || s.name.as_ref().is_some_and(|n| &p.name != n) {
                    continue;
                }
                if let Some(e) = self.dims[p.dim as usize].entity_region_ref(p.entity).and_then(|r| r.entities.get(p.entity)) {
                    consider(p.dim, e);
                }
            }
        }
        let sort = s.sort.as_deref().unwrap_or(match s.base {
            'p' => "nearest",
            'r' => "random",
            _ => "arbitrary",
        });
        match sort {
            "nearest" => found.sort_by(|a, b| a.2.total_cmp(&b.2)),
            "furthest" => found.sort_by(|a, b| b.2.total_cmp(&a.2)),
            "random" => {
                let mut seed = self.tick.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
                for i in (1..found.len()).rev() {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    found.swap(i, (seed % (i as u64 + 1)) as usize);
                }
            }
            _ => {}
        }
        let limit = s.limit.unwrap_or(if matches!(s.base, 'p' | 'r' | 's') { 1 } else { usize::MAX });
        found.into_iter().take(limit).map(|f| (f.0, f.1)).collect()
    }

    fn player_index(&self, entity: u64) -> Option<usize> {
        self.players.iter().position(|p| p.joined && p.entity == entity)
    }

    /// Moves any entity; players go through `change_dim` so their client follows. Changing
    /// dimension asks `enter_dimension` first.
    fn teleport(&mut self, c: &'static Content, dim: u32, id: u64, to_dim: u32, pos: DVec3) {
        if dim != to_dim {
            let Some(r) = self.dims[dim as usize].entity_region(id) else { return };
            let mut q = abi::Query { entity: id, dim: to_dim, ..abi::Query::DEFAULT };
            if !r.ask(c, abi::question::ENTER_DIMENSION, &mut q) {
                return;
            }
        }
        if let Some(pi) = self.player_index(id) {
            self.change_dim(c, pi, to_dim, pos);
            return;
        }
        let Some((mut e, inv, comps)) = self.dims[dim as usize].entity_region(id).and_then(|r| r.entities.take(id)) else { return };
        e.pos = pos;
        let r = self.dims[to_dim as usize].ensure_region(c, pos);
        r.entities.insert_with(c, e, inv, comps);
    }

    fn position_of(&self, dim: u32, id: u64) -> Option<DVec3> {
        self.dims[dim as usize].entity_region_ref(id).and_then(|r| r.entities.get(id)).map(|e| e.pos)
    }

    fn builtin(&mut self, c: &'static Content, pi: usize, name: &str, a: &crate::commands::Args, here: DVec3) -> String {
        let me = (self.players[pi].dim, self.players[pi].entity);
        match builtin::COMMANDS.iter().find(|b| b.0 == name) {
            Some(b) => (b.2)(self, &builtin::Ctx { c, pi, a, here, me }),
            None => format!("unknown command /{name}"),
        }
    }

    /// Center cube, radius, cubes above and cubes below. Flying high, the view reaches down past
    /// the air under the player so the ground shows; on the ground it stays shallow, since
    /// reaching deeper only loads caves.
    fn view(&self, p: &Player) -> (IVec3, i32, i32, i32) {
        let dim = &self.dims[p.dim as usize];
        let pos = dim.entity_region_ref(p.entity).and_then(|r| r.entities.get(p.entity)).map_or(DVec3::ZERO, |e| e.pos);
        let center = (pos.floor().as_ivec3()) >> 4;
        let (r, h) = (self.cfg.view_radius, self.cfg.view_height);
        (center, r, h, h + dim.air_below(center, r))
    }

    /// Cubes players need, nearest columns first and each column top-down; loading and
    /// generation run on the rayon pool.
    fn request_generation(&mut self, c: &'static Content) {
        // Enough jobs to keep every worker busy for a whole tick; the backlog counts too so
        // generation never runs far ahead of lighting.
        let budget = crate::background().current_num_threads() * 64;
        let queued: FxHashSet<(u32, IVec3)> = self.backlog.iter().map(|g| (g.dim, g.pos)).collect();
        // Cubes the backlog waits on (the ones above, see `receive_generated`) go first, whatever
        // the budget: a backlog that fills the budget waiting for them would stall loading for good.
        let mut blockers: Vec<(u32, IVec3)> = self
            .backlog
            .iter()
            .map(|g| (g.dim, g.pos + IVec3::Y))
            .filter(|&(d, p)| self.wanted.contains(&(d, p)) && !self.dims[d as usize].generating.contains(&p) && !queued.contains(&(d, p)))
            .collect();
        blockers.sort_unstable_by_key(|&(d, p)| (d, p.x, p.y, p.z));
        blockers.dedup();
        for (d, p) in blockers {
            self.spawn_generation(d, p);
        }
        let in_flight: usize = self.dims.iter().map(|d| d.generating.len()).sum::<usize>() + self.backlog.len();
        if in_flight >= budget {
            return;
        }
        *self.shared.views.write() = self.players.iter().filter(|p| p.joined).map(|p| (p.dim, self.view(p))).map(|(d, (c, r, h, down))| (d, c, r, h, down)).collect();
        let mut wanted: Vec<(i32, u32, IVec3)> = Vec::new();
        self.wanted.clear();
        let edited: Vec<Vec<IVec3>> = self.dims.iter_mut().map(|d| d.take_edited()).collect();
        for pi in 0..self.players.len() {
            let p = &self.players[pi];
            if !p.joined {
                continue;
            }
            let (center, r, h, down) = self.view(p);
            let pdim = p.dim;
            let edited = &edited[pdim as usize];
            let key = (pdim, center, r, h + down * 1000, self.dims[pdim as usize].settled_version());
            if p.gen_done == Some(key) && edited.is_empty() {
                continue;
            }
            let def = &c.dims[pdim as usize];
            let (_, hi) = loaded_y_range(def, center.y, h, down);
            let in_view = |pos: IVec3| {
                let d = pos - center;
                d.x * d.x + d.z * d.z <= r * r && d.y <= h && d.y >= -down && pos.y >= def.min_cube_y && pos.y <= def.max_cube_y
            };
            let Server { players, dims, wanted: wanted_set, .. } = &mut *self;
            let dim = &mut dims[pdim as usize];
            let p = &mut players[pi];
            // The visibility fill follows the player and goes on from cubes that arrived.
            let vis = p.vis.get_or_insert_with(visibility::Visibility::new);
            let (arrived, left) = vis.update(dim, c, (center, r, h, down), &in_view, NEAR, edited);
            let vis = p.vis.as_ref().unwrap();
            let mut missing: Vec<IVec3> = vis.waiting.keys().copied().collect();
            // Above the view the column is loaded up to open sky, so light coming down is right.
            let sky_column = |pos: IVec3| {
                let d = pos - center;
                d.x * d.x + d.z * d.z <= r * r && pos.y > center.y + h && pos.y <= hi
            };
            for y in center.y + h + 1..=hi {
                for dz in -r..=r {
                    for dx in -r..=r {
                        let pos = IVec3::new(center.x + dx, y, center.z + dz);
                        if sky_column(pos) && !dim.has(pos) {
                            missing.push(pos);
                        }
                    }
                }
            }
            // Columns capped by ground nobody will load are dark from above: their light is final.
            // Candidates: cubes just reached, and the ones under cubes that left the view.
            let tops: Vec<IVec3> = arrived
                .iter()
                .copied()
                .chain(left.iter().map(|&q| q - IVec3::Y))
                .filter(|&q| dim.has(q) && !dim.settled(q) && !dim.has(q + IVec3::Y) && !vis.seen.contains(&(q + IVec3::Y)) && !sky_column(q + IVec3::Y))
                .collect();
            for t in tops {
                dim.settle_down(t);
            }
            // Everything above a wanted cube up to the top of the range comes first, so light
            // always arrives from above before a cube is lit; otherwise it would be lit dark and
            // the whole column relit when the sky shows up. Solid cubes cost almost nothing.
            if def.sky_light {
                let mut seen: FxHashSet<IVec3> = missing.iter().copied().collect();
                let mut above = Vec::new();
                for &pos in &missing {
                    let mut q = pos + IVec3::Y;
                    while q.y <= hi && !dim.has(q) && seen.insert(q) {
                        above.push(q);
                        q += IVec3::Y;
                    }
                }
                missing.extend(above);
            }
            for &pos in &missing {
                if !dim.generating.contains(&pos) && !queued.contains(&(pdim, pos)) {
                    // Nearest columns first, each from the top down.
                    let d = pos - center;
                    wanted.push(((d.x * d.x + d.z * d.z) * 64 - pos.y, pdim, pos));
                    wanted_set.insert((pdim, pos));
                }
            }
            self.players[pi].gen_done = missing.is_empty().then_some(key);
        }
        if wanted.is_empty() {
            return;
        }
        wanted.sort_by_key(|w| w.0);
        wanted.dedup_by_key(|w| (w.1, w.2));
        for &(_, dim, pos) in wanted.iter().take(budget - in_flight) {
            self.spawn_generation(dim, pos);
        }
    }

    /// Loads or generates a cube on the worker pool; it arrives through `gen_rx`.
    fn spawn_generation(&mut self, dim: u32, pos: IVec3) {
        self.dims[dim as usize].generating.insert(pos);
        let tx = self.gen_tx.clone();
        let seed = self.cfg.seed;
        let storage = self.storage.clone();
        let shared = self.shared.clone();
        crate::background().spawn(move || {
            // Columns go up to open sky, so nothing above the view's floor counts as left behind.
            let wanted = shared.views.read().iter().any(|&(d, c, r, _, down)| {
                let o = pos - c;
                d == dim && o.x * o.x + o.z * o.z <= (r + 2) * (r + 2) && o.y >= -down - 2
            });
            if !wanted {
                let _ = tx.send(Generated { dim, pos, cube: None, entities: Vec::new(), conn: 0 });
                return;
            }
            let t = Instant::now();
            let loaded = storage.and_then(|s| s.lock().load(dim, pos));
            let (mut cube, entities) = loaded.unwrap_or_else(|| (dim::generate(content(), dim, seed, pos), Vec::new()));
            let conn = dim::see_through(content(), &cube);
            crate::world::light::prelight(content(), &mut cube, content().dims[dim as usize].sky_light);
            shared.gen_ns.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
            shared.generated.fetch_add(1, Ordering::Relaxed);
            let _ = tx.send(Generated { dim, pos, cube: Some(cube), entities, conn });
        });
    }

    fn sync_players(&mut self, c: &'static Content) {
        let server_tick = self.tick;
        let settled: Vec<Vec<IVec3>> = self.dims.iter_mut().map(|d| d.take_settled()).collect();
        for d in &mut self.dims {
            for r in &mut d.regions {
                r.entities.find_changes(&c.game.synced_comps);
            }
        }
        // Entities that moved since last tick and changed equipment, per dimension (all regions).
        let mut moved: Vec<Vec<crate::net::Move>> = Vec::with_capacity(self.dims.len());
        let mut worn: Vec<Vec<(u64, Vec<u32>)>> = Vec::with_capacity(self.dims.len());
        let mut burning: Vec<Vec<u64>> = Vec::with_capacity(self.dims.len());
        for d in &mut self.dims {
            let mut m = Vec::new();
            let mut w = Vec::new();
            let mut b: Vec<u64> = d.regions.iter().flat_map(|r| r.entities.list.iter()).filter(|e| e.fire > 0 && !e.removed).map(|e| e.id).collect();
            b.sort_unstable();
            burning.push(b);
            for r in &mut d.regions {
                let changed: Vec<u64> = r.entities.dirty_inv.iter().copied().collect();
                for id in changed {
                    let now = worn_of(c, &r.entities, id);
                    if let Some(e) = r.entities.get_mut(id).filter(|e| e.worn != now) {
                        e.worn = now.clone();
                        w.push((id, now));
                    }
                }
                for e in r.entities.list.iter_mut().filter(|e| !e.removed && e.anchor.is_none()) {
                    let now = (e.pos, e.yaw, e.pitch, e.crouched);
                    // Solid bodies every tick: clients show them between server positions, and a
                    // body that stops must say so (two equal positions).
                    let solid = c.game.entities[e.kind as usize].is(abi::entity_flags::SOLID);
                    if solid || e.sent.is_none_or(|s| (s.0 - now.0).length_squared() > 1e-6 || (s.1 - now.1).abs() > 0.01 || (s.2 - now.2).abs() > 0.01 || s.3 != now.3) {
                        if e.sent.is_some() {
                            m.push((e.id, e.pos, e.yaw, e.pitch, e.crouched, e.support.unwrap_or(0)));
                        }
                        e.sent = Some(now);
                    }
                }
            }
            moved.push(m);
            worn.push(w);
        }

        let mut unsent = 0u64;
        for pi in 0..self.players.len() {
            if !self.players[pi].joined {
                continue;
            }
            let (center, r, h, down) = self.view(&self.players[pi]);
            let Server { players, dims, tick, .. } = self;
            let p = &mut players[pi];
            let dim = &mut dims[p.dim as usize];
            let def = &c.dims[p.dim as usize];

            for reg in &dim.regions {
                for (pos, cube) in &reg.resend {
                    if p.sent.contains(pos) {
                        p.conn.send(ToClient::Cube { dim: p.dim, pos: *pos, cube: cube.clone(), hidden: crate::net::KEEP_HIDDEN });
                    }
                }
                if !reg.changes.is_empty() {
                    let changes: Vec<(IVec3, u32)> = reg.changes.iter().filter(|(b, _)| p.sent.contains(&(*b >> 4))).copied().collect();
                    if !changes.is_empty() {
                        p.conn.send(ToClient::Blocks { dim: p.dim, changes });
                    }
                }
            }
            if *tick % 20 == 0 {
                p.conn.send(ToClient::Time { dim: p.dim, time: dim.time });
            }

            // Entities in range: spawn new ones, despawn gone ones, stream movement.
            let (me, me_mode) = dim.entity_region_ref(p.entity).and_then(|r| r.entities.get(p.entity)).map_or((DVec3::ZERO, GameMode::Survival), |e| (e.pos, e.mode));
            let range2 = ((r * 16) as f64).powi(2);
            let mut spawn = Vec::new();
            let mut visible = FxHashSet::default();
            for reg in &dim.regions {
                // Spectators are seen only by other spectators.
                let shown = |e: &&Entity| e.mode != GameMode::Spectator || me_mode == GameMode::Spectator;
                for e in reg.entities.list.iter().filter(|e| !e.removed && e.id != p.entity && e.anchor.is_none() && e.kind != c.game.virtual_kind).filter(shown) {
                    if (e.pos - me).length_squared() > range2 {
                        continue;
                    }
                    visible.insert(e.id);
                    if !p.known.contains(&e.id) {
                        let worn = e.worn.clone();
                        spawn.push(EntityView { id: e.id, kind: e.kind, pos: e.pos, yaw: e.yaw, pitch: e.pitch, item: crate::stack::for_client(e.item), health: e.health, worn, block: e.block });
                    }
                }
            }
            // Before despawns: a mob killed this tick is gone already, and the client needs its
            // last health to play the death animation.
            for reg in &dim.regions {
                let hurt: Vec<(u64, f32)> = reg.entities.hurt.iter().filter(|h| p.known.contains(&h.0)).copied().collect();
                if !hurt.is_empty() {
                    p.conn.send(ToClient::Hurt(hurt));
                }
            }
            let taken: Vec<(u64, u64)> = dim.regions.iter().flat_map(|r| r.entities.taken.iter()).filter(|t| p.known.contains(&t.0)).copied().collect();
            if !taken.is_empty() {
                p.conn.send(ToClient::Taken(taken));
            }
            let gone: Vec<u64> = p.known.iter().copied().filter(|id| !visible.contains(id)).collect();
            if !gone.is_empty() {
                p.conn.send(ToClient::Despawn(gone));
            }
            // Components every viewer gets: all of newly seen entities, changes of known ones.
            let mut shared: Vec<(u64, u32, Vec<u8>)> = Vec::new();
            for reg in &dim.regions {
                for &(id, comp) in &reg.entities.dirty_comp {
                    if id != p.entity && p.known.contains(&id) && visible.contains(&id) && c.game.components[comp as usize].flags & cf::SYNC_WATCHERS != 0 {
                        shared.extend(reg.entities.comps.get(&(id, comp)).map(|b| (id, comp, b.to_vec())));
                    }
                }
                for v in &spawn {
                    for &comp in &c.game.watched_comps {
                        shared.extend(reg.entities.comps.get(&(v.id, comp)).map(|b| (v.id, comp, b.to_vec())));
                    }
                }
            }
            // Sizes and looks of entities seen for the first time, and changes of known ones.
            let mut shapes: Vec<(u64, [f32; 3], u32)> = Vec::new();
            for reg in &dim.regions {
                let fresh = spawn.iter().map(|v| v.id);
                let changed = reg.entities.reshaped.iter().copied().filter(|id| p.known.contains(id) && visible.contains(id));
                for id in fresh.chain(changed) {
                    let Some(e) = reg.entities.get(id) else { continue };
                    if e.size.is_some() || e.look.is_some() || reg.entities.reshaped.contains(&id) {
                        shapes.push((id, e.size.unwrap_or([0.0; 3]), e.look.unwrap_or(u32::MAX)));
                    }
                }
            }
            if let Some(me) = dim.regions.iter().find(|r| r.entities.reshaped.contains(&p.entity)).and_then(|r| r.entities.get(p.entity)) {
                shapes.push((p.entity, me.size.unwrap_or([0.0; 3]), me.look.unwrap_or(u32::MAX)));
            }
            // Structures: all their cubes with the entity, then the cubes that change.
            let mut structures = Vec::new();
            for reg in &dim.regions {
                for (&id, st) in &reg.structures {
                    if spawn.iter().any(|v| v.id == id) {
                        structures.push(ToClient::Structure { id, cubes: st.cubes.iter().map(|(p, c)| (*p, c.clone())).collect(), full: true });
                    } else if !st.resend.is_empty() && p.known.contains(&id) && visible.contains(&id) {
                        structures.push(ToClient::Structure { id, cubes: st.resend.clone(), full: false });
                    }
                }
            }
            if !spawn.is_empty() {
                p.conn.send(ToClient::Spawn(spawn));
            }
            for m in structures {
                p.conn.send(m);
            }
            if !shapes.is_empty() {
                p.conn.send(ToClient::Shapes(shapes));
            }
            if !shared.is_empty() {
                p.conn.send(ToClient::EntityComponents(shared));
            }
            let moves: Vec<crate::net::Move> = moved[p.dim as usize].iter().filter(|m| p.known.contains(&m.0) && visible.contains(&m.0)).copied().collect();
            if !moves.is_empty() {
                p.conn.send(ToClient::Moves(server_tick, moves));
            }
            let worn: Vec<(u64, Vec<u32>)> = worn[p.dim as usize].iter().filter(|w| p.known.contains(&w.0) && visible.contains(&w.0)).cloned().collect();
            if !worn.is_empty() {
                p.conn.send(ToClient::Worn(worn));
            }
            p.known = visible;
            // Only when it changes: fire shows at once and nothing is sent while nothing burns.
            let burning: Vec<u64> = burning[p.dim as usize].iter().copied().filter(|id| p.known.contains(id)).collect();
            if burning != p.sent_burning {
                p.conn.send(ToClient::Burning(burning.clone()));
                p.sent_burning = burning;
            }

            if let Some(reg) = dim.entity_region(p.entity) {
                // Any change counts (regeneration, mods healing), not only hits.
                let hp = reg.entities.get(p.entity).map_or(p.sent_health.0, |e| e.health);
                let max = reg.max_health(c, p.entity);
                if !p.dead && (hp, max) != p.sent_health {
                    p.sent_health = (hp, max);
                    p.conn.send(ToClient::Health { health: hp, max });
                }
                if let Some(&(_, dir)) = reg.entities.knocked.iter().rev().find(|h| h.0 == p.entity) {
                    p.conn.send(ToClient::Knockback(dir));
                }
                // Own inventory and synced components.
                if p.first_sync || reg.entities.dirty_inv.contains(&p.entity) {
                    if let Some(inv) = reg.entities.inventories.get(&p.entity) {
                        p.conn.send(ToClient::Inventory(inv.iter().enumerate().map(|(i, s)| (i as u32, crate::stack::for_client(*s))).collect()));
                    }
                }
                let comps: Vec<(u32, Vec<u8>)> = reg
                    .entities
                    .comps
                    .iter()
                    .filter(|((id, comp), _)| {
                        *id == p.entity
                            && c.game.components[*comp as usize].flags & cf::SYNC_OWNER != 0
                            && (p.first_sync || reg.entities.dirty_comp.contains(&(*id, *comp)))
                    })
                    .map(|((_, comp), b)| (*comp, b.to_vec()))
                    .collect();
                if !comps.is_empty() {
                    p.conn.send(ToClient::Components(comps));
                }
                p.first_sync = false;
                if let Some(m) = &mut p.menu {
                    // The machine's menu-synced components, all at first, then what changed.
                    let owner = m.owner;
                    let first = !std::mem::replace(&mut m.comps_sent, true);
                    let comps: Vec<(u64, u32, Vec<u8>)> = c
                        .game
                        .menu_comps
                        .iter()
                        .filter(|&&comp| owner != 0 && (first || reg.entities.dirty_comp.contains(&(owner, comp))))
                        .filter_map(|&comp| reg.entities.comps.get(&(owner, comp)).map(|b| (owner, comp, b.to_vec())))
                        .collect();
                    if !comps.is_empty() {
                        p.conn.send(ToClient::EntityComponents(comps));
                    }
                    let (slots, cursor) = menu::changes(m, reg, p.entity);
                    if !slots.is_empty() {
                        p.conn.send(ToClient::MenuSlots(slots));
                    }
                    if let Some(cur) = cursor {
                        p.conn.send(ToClient::Cursor(cur));
                    }
                }
            }

            if !p.gizmos.is_empty() {
                let shapes = gizmos_for(p, dim, *tick, me);
                if !shapes.is_empty() {
                    p.conn.send(ToClient::Gizmos(shapes));
                }
            }

            let in_range = |pos: IVec3| {
                let d = pos - center;
                d.x * d.x + d.z * d.z <= r * r && d.y <= h && d.y >= -down && pos.y >= def.min_cube_y && pos.y <= def.max_cube_y
            };
            let range = (center, r, h, down);
            let moved = p.sync_range != Some(range);
            if moved {
                // Settled cubes that came into range; the ones that left are unloaded below.
                let was = p.sync_range.map(|(oc, or, oh, od)| {
                    move |pos: IVec3| {
                        let d = pos - oc;
                        d.x * d.x + d.z * d.z <= or * or && d.y <= oh && d.y >= -od
                    }
                });
                p.unsent.retain(|&q| in_range(q));
                for dy in -down..=h {
                    for dz in -r..=r {
                        for dx in -r..=r {
                            let pos = center + IVec3::new(dx, dy, dz);
                            if in_range(pos) && !was.as_ref().is_some_and(|w| w(pos)) && !p.sent.contains(&pos) && dim.settled(pos) {
                                p.unsent.insert(pos);
                            }
                        }
                    }
                }
                p.sync_range = Some(range);
            }
            p.unsent.extend(settled[p.dim as usize].iter().copied().filter(|&q| in_range(q) && !p.sent.contains(&q)));
            // Send a cube once its neighbors exist and its sky light is final.
            let mut to_send: Vec<(i32, IVec3)> = p
                .unsent
                .iter()
                .copied()
                .filter(|&pos| {
                    crate::world::DIRS.iter().all(|d| {
                        let n = pos + *d;
                        dim.has(n) || !in_range(n) || !p.vis.as_ref().is_some_and(|v| v.seen.contains(&n))
                    })
                })
                .map(|pos| {
                    let d = pos - center;
                    (d.x * d.x + d.z * d.z + d.y * d.y, pos)
                })
                .collect();
            to_send.sort_by_key(|t| t.0);
            unsent += to_send.len().saturating_sub(SEND_PER_TICK) as u64;
            for &(_, pos) in to_send.iter().take(SEND_PER_TICK) {
                if let Some(cube) = dim.snapshot(pos) {
                    let hidden = crate::world::DIRS.iter().enumerate().fold(0u8, |m, (i, d)| {
                        let n = pos + *d;
                        if dim.has(n) || (in_range(n) && p.vis.as_ref().is_some_and(|v| v.seen.contains(&n))) { m } else { m | 1 << i }
                    });
                    p.conn.send(ToClient::Cube { dim: p.dim, pos, cube, hidden });
                    p.sent.insert(pos);
                    p.unsent.remove(&pos);
                }
            }
            if !moved {
                continue;
            }
            let far: Vec<IVec3> = p
                .sent
                .iter()
                .copied()
                .filter(|&pos| {
                    let d = pos - center;
                    d.x * d.x + d.z * d.z > (r + 2) * (r + 2) || d.y > h + 2 || d.y < -down - 2
                })
                .collect();
            for pos in far {
                p.sent.remove(&pos);
                p.conn.send(ToClient::Unload { dim: p.dim, pos });
            }
        }
        self.shared.unsent.store(unsent, Ordering::Relaxed);
        let pending = self.backlog.len() + self.dims.iter().map(|d| d.generating.len()).sum::<usize>();
        self.shared.pending.store(pending as u64, Ordering::Relaxed);
        for d in &mut self.dims {
            for r in &mut d.regions {
                for st in r.structures.values_mut() {
                    st.resend.clear();
                    st.changes.clear();
                    st.edited.clear();
                }
                r.resend.clear();
                r.changes.clear();
                r.entities.hurt.clear();
                r.entities.taken.clear();
                r.entities.reshaped.clear();
                r.entities.knocked.clear();
                r.entities.dirty_comp.clear();
                r.gizmos.clear();
                // Block entity inventory changes must reach the save.
                let anchored: Vec<IVec3> = r
                    .entities
                    .list
                    .iter()
                    .filter(|e| e.anchor.is_some() && r.entities.dirty_inv.contains(&e.id))
                    .map(|e| cube_of(e.pos.floor().as_ivec3()))
                    .collect();
                r.modified.extend(anchored);
                r.entities.dirty_inv.clear();
                r.entities.spawned.clear();
                r.entities.despawned.clear();
            }
        }
        self.unload_unused(c);
    }

    /// Every half second, so a fast flight drops a few thousand cubes at a time, not tens.
    fn unload_unused(&mut self, c: &'static Content) {
        if self.tick % 10 != 0 {
            return;
        }
        for di in 0..self.dims.len() {
            let def = &c.dims[di];
            let views: Vec<(IVec3, i32, i32, i32)> = self
                .players
                .iter()
                .filter(|p| p.joined && p.dim as usize == di)
                .map(|p| {
                    let (center, _, h, down) = self.view(p);
                    let (lo, hi) = loaded_y_range(def, center.y, h, down);
                    (center, self.cfg.view_radius + 3, lo - 3, hi + 3)
                })
                .collect();
            let storage = self.storage.clone();
            let d = &mut self.dims[di];
            let before: usize = d.regions.iter().map(|r| r.cubes.len()).sum();
            for r in &mut d.regions {
                let removed = r.retain_cubes(c, |pos| {
                    views.iter().any(|&(center, rad, lo, hi)| {
                        let d = pos - center;
                        d.x * d.x + d.z * d.z <= rad * rad && pos.y >= lo && pos.y <= hi
                    })
                });
                for (pos, cube, inside) in removed {
                    if let Some(s) = &storage {
                        // Written in the background; loads queue behind it there.
                        let (s, bytes, dim) = (s.clone(), serialize(c, r, &inside), r.id);
                        crate::background().spawn(move || s.lock().save(c, dim, pos, &cube, &bytes));
                    }
                    for id in inside {
                        r.entities.remove(id);
                    }
                }
                r.entities.compact();
            }
            d.dirty |= d.regions.iter().map(|r| r.cubes.len()).sum::<usize>() != before;
        }
    }
}

/// The nearest structure `name` of dimension `dim` within `radius` of `pos`: its lowest corner.
pub fn structure_near(c: &Content, dim: u32, seed: u64, name: &str, pos: IVec3, radius: i32) -> Option<IVec3> {
    let def = &c.dims[dim as usize];
    let s = def.structures.iter().find(|s| s.name == name)?;
    let cells = |v: i32| (v - radius).div_euclid(s.spacing)..=(v + radius).div_euclid(s.spacing);
    let mut best: Option<IVec3> = None;
    for cz in cells(pos.z) {
        for cx in cells(pos.x) {
            if let Some(a) = dim::structure_in(def, c, s, seed, (cx, cz)) {
                let d = |p: IVec3| (p - pos).with_y(0).as_vec3().length();
                if d(a) <= radius as f32 && best.is_none_or(|b| d(a) < d(b)) {
                    best = Some(a);
                }
            }
        }
    }
    best
}

/// Gizmo shapes for one player: what mods drew in the groups it shows, and every second the
/// engine's server-side groups.
fn gizmos_for(p: &Player, dim: &DimSet, tick: u64, me: DVec3) -> Vec<crate::net::GizmoShape> {
    use crate::net::GizmoShape;
    let mut out: Vec<GizmoShape> = Vec::new();
    for r in &dim.regions {
        out.extend(r.gizmos.iter().filter(|g| p.gizmos.contains(&g.group)).cloned());
    }
    if tick % TPS as u64 != 0 {
        return out;
    }
    let on = |g: &str| p.gizmos.iter().any(|x| x == g);
    let shape = |group: &str, kind: u8, a: DVec3, b: DVec3, color: u32, text: String| GizmoShape { group: group.into(), kind, a, b, color, ticks: TPS + 2, text };
    if on("regions") {
        const COLORS: [u32; 6] = [0xFF4040FF, 0x40FF40FF, 0x4080FFFF, 0xFFFF40FF, 0xFF40FFFF, 0x40FFFFFF];
        for (i, r) in dim.regions.iter().enumerate() {
            let (Some(lo), Some(hi)) = (r.cubes.keys().copied().reduce(IVec3::min), r.cubes.keys().copied().reduce(IVec3::max)) else { continue };
            let (a, b) = ((lo * 16).as_dvec3() + 0.05, ((hi + 1) * 16).as_dvec3() - 0.05);
            out.push(shape("regions", 1, a, b, COLORS[i % COLORS.len()], String::new()));
        }
    }
    if let Some(v) = p.vis.as_ref().filter(|_| on("visibility")) {
        let here: IVec3 = me.floor().as_ivec3() >> 4;
        for &c in &v.seen {
            let d = c - here;
            if d.x.abs() > 8 || d.z.abs() > 8 || d.y.abs() > 4 {
                continue;
            }
            let color = if dim.has(c) { 0x40FF60FF } else { 0xFFA030FF };
            let m = (c * 16).as_dvec3() + 8.0;
            for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
                out.push(shape("visibility", 0, m - axis, m + axis, color, String::new()));
            }
        }
    }
    out
}

/// What a game mode changes through attributes: in creative mobs do not notice the player.
fn mode_modifiers(c: &Content, r: &mut Dim, id: u64, mode: GameMode) {
    if mode == GameMode::Survival {
        r.remove_attr_modifier(c, id, extend::VISIBILITY, "core:game_mode");
    } else {
        r.set_attr_modifier(c, id, extend::VISIBILITY, "core:game_mode", abi::attr_op::MUL_TOTAL, -1.0, 0);
    }
}

/// Items in a player's equipment slots that show on the model.
fn worn_of(c: &Content, ents: &entity::Entities, id: u64) -> Vec<u32> {
    let worn = &c.game.player_layout.worn;
    let Some(inv) = ents.inventories.get(&id).and_then(|i| i.get(worn.start as usize..worn.end as usize)) else { return Vec::new() };
    inv.iter().filter(|s| !s.is_empty() && c.worn(s.item).is_some()).map(|s| s.item).collect()
}

/// The block is within reach of these eyes.
fn within(eye: DVec3, block: IVec3) -> bool {
    (block.as_dvec3() + DVec3::splat(0.5) - eye).length_squared() < REACH * REACH
}

/// Damages the held tool; breaks it when used up. Creative players' tools never wear.
fn wear(c: &Content, d: &mut Dim, player: u64, slot: u32, amount: u32) {
    if d.entities.get(player).is_some_and(|e| e.mode == GameMode::Creative) {
        return;
    }
    let Some(stack) = d.entities.inventories.get(&player).and_then(|i| i.get(slot as usize)).copied() else { return };
    let Some(def) = c.items.get(stack.item as usize) else { return };
    if def.durability == 0 || stack.is_empty() {
        return;
    }
    let damage = crate::stack::damage(&stack) + amount;
    let next = if damage >= def.durability { ItemStack::EMPTY } else { crate::stack::with_damage(stack, damage) };
    d.entities.set_slot(player, slot, next);
}

fn serialize(c: &Content, d: &Dim, ids: &[u64]) -> Vec<u8> {
    let list: Vec<persist::ToSave> = ids
        .iter()
        .filter_map(|&id| {
            // Virtual inventories are never saved.
            let e = d.entities.get(id).filter(|e| e.kind != c.game.virtual_kind)?;
            let comps = d.entities.comps.iter().filter(|((eid, _), _)| *eid == id).map(|((_, k), v)| (*k, &v[..])).collect();
            Some((e, d.entities.inventories.get(&id), comps, d.structures.get(&id).map(|s| s.structure_blocks(c))))
        })
        .collect();
    let mut out = Vec::new();
    persist::write(c, &mut out, &list);
    out
}

fn entities_in(c: &Content, d: &Dim, cube: IVec3) -> Vec<u8> {
    let ids: Vec<u64> = d.entities.list.iter().filter(|e| !e.removed && !e.player && e.kind != c.game.virtual_kind && cube_of(e.pos.floor().as_ivec3()) == cube).map(|e| e.id).collect();
    serialize(c, d, &ids)
}

fn save_player(s: &Storage, p: &Player, d: &Dim) {
    let c = content();
    let path = s.player_path(&p.key);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let Some(e) = d.entities.get(p.entity) else { return };
    let mut text = format!(
        "name = {}\ndim = {}\npos = {} {} {}\nlook = {} {}\nhealth = {}\n",
        p.name, c.dims[p.dim as usize].name, e.pos.x, e.pos.y, e.pos.z, e.yaw, e.pitch, e.health
    );
    if e.mode != GameMode::Survival {
        text.push_str(&format!("mode = {}\n", e.mode.name()));
    }
    if let Some(inv) = d.entities.inventories.get(&p.entity) {
        let items: Vec<String> = inv
            .iter()
            .map(|s| if s.is_empty() { "-".into() } else { format!("{}*{}*{}", c.items[s.item as usize].name, s.count, crate::stack::to_text(crate::stack::for_save(s.data))) })
            .collect();
        text.push_str(&format!("inventory = {}\n", items.join(" ")));
    }
    for ((id, comp), bytes) in &d.entities.comps {
        if *id == p.entity && c.game.components[*comp as usize].flags & cf::SAVE != 0 {
            text.push_str(&format!("comp.{} = {}\n", c.game.components[*comp as usize].name, persist::hex(bytes)));
        }
    }
    if let Err(err) = std::fs::write(&path, text) {
        log::error!("saving player {}: {err}", p.name);
    }
}

fn saved_mode(s: &Storage, key: &str) -> GameMode {
    let src = std::fs::read_to_string(s.player_path(key)).unwrap_or_default();
    crate::kv::get(&src, "mode").and_then(GameMode::parse).unwrap_or_default()
}

/// Said to a player arriving in the engine's void.
fn void_note(c: &Content) -> String {
    if c.spawn_dim == c.void_dim {
        "No dimension was found (no mods?), so you are in the void, where nothing can be changed".into()
    } else {
        "This is the void: nothing here can be changed".into()
    }
}

/// Dimension (`None`: unknown now), position, health, inventory, components.
type PlayerData = (Option<u32>, DVec3, f32, Option<Vec<ItemStack>>, Vec<(u32, Box<[u8]>)>);

/// A saved player. Content that no longer exists is dropped with a log line; a file that cannot
/// be read is kept as `.bak` before it gets overwritten by a fresh player.
fn load_player(c: &Content, s: &Storage, key: &str) -> Option<PlayerData> {
    let path = s.player_path(key);
    let src = match std::fs::read_to_string(&path) {
        Ok(src) => src,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            log::error!("reading player {key}: {e}; kept as .bak, the player starts anew");
            let _ = std::fs::copy(&path, path.with_extension("bak"));
            return None;
        }
    };
    let (mut dim, mut pos, mut health, mut inv, mut comps) = (None, None, -1.0, None, Vec::new());
    for (_, k, v) in crate::kv::pairs(&src) {
        match k {
            "dim" => {
                dim = c.dim_names.get(v).copied();
                if dim.is_none() {
                    log::warn!("player {key}: dimension {v} does not exist now");
                }
            }
            "pos" => {
                let n: Vec<f64> = v.split_whitespace().filter_map(|x| x.parse().ok()).collect();
                if n.len() == 3 {
                    pos = Some(DVec3::new(n[0], n[1], n[2]));
                }
            }
            "health" => health = v.parse().unwrap_or(-1.0),
            "inventory" => {
                let mut slots = vec![ItemStack::EMPTY; c.game.player_layout.slots as usize];
                for (i, tok) in v.split_whitespace().enumerate().take(slots.len()) {
                    let parts: Vec<&str> = tok.split('*').collect();
                    if parts.len() == 3 {
                        let item = c.item_names.get(parts[0]).copied();
                        if item.is_none() {
                            log::warn!("player {key}: dropped {} {} (no such item now)", parts[1], parts[0]);
                        }
                        if let Some(item) = item {
                            slots[i] = ItemStack { item, count: parts[1].parse().unwrap_or(1), data: crate::stack::from_text(parts[2]) };
                        }
                    }
                }
                inv = Some(slots);
            }
            _ => {
                if let Some(name) = k.strip_prefix("comp.") {
                    let id = c.game.component(name);
                    if id.is_none() {
                        log::warn!("player {key}: dropped component {name} (not registered now)");
                    }
                    if let Some(id) = id {
                        let mut b = persist::unhex(v);
                        let size = c.game.components[id as usize].size as usize;
                        if size > 0 {
                            b.resize(size, 0);
                        }
                        comps.push((id, b.into_boxed_slice()));
                    }
                }
            }
        }
    }
    let Some(pos) = pos else {
        log::error!("player {key}: no position in the file; kept as .bak, the player starts anew");
        let _ = std::fs::copy(&path, path.with_extension("bak"));
        return None;
    };
    Some((dim, pos, health, inv, comps))
}
