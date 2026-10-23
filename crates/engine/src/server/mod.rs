//! Authoritative simulation. Runs on its own thread; single player talks to it through the
//! in-process transport. Each dimension is a set of regions ticked in parallel.

mod dim;
pub mod energy;
pub mod entity;
pub mod menu;
pub mod persist;
pub mod regions;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, unbounded};
use glam::{DVec3, IVec3};
use parking_lot::Mutex;
use rayon::prelude::*;
use rustc_hash::FxHashSet;
use voxide_abi::{self as abi, ItemStack, component_flags as cf, damage};

pub use dim::{Dim, DimEvent, break_time};

use crate::content::{Content, DimensionDef, content};
use crate::net::{EntityView, ServerConn, ToClient, ToServer};
use crate::saves::Storage;
use crate::world::{Cube, GameMode, cube_of};
use entity::Entity;
use menu::OpenMenu;
use regions::DimSet;

pub const TPS: u32 = 20;
const AUTOSAVE_TICKS: u64 = 20 * 30;
const REACH: f64 = 6.0;

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
    /// View (dim, centre, radius, height) whose cubes were all present at the last check, and the
    /// same plus the settled version when nothing was left to send. Lets idle ticks skip scanning
    /// the whole view volume.
    gen_done: Option<(u32, IVec3, i32, i32, u64)>,
    /// Cubes the last visibility fill reached; only these (and the sky column) get loaded.
    visible: FxHashSet<IVec3>,
    sync_done: Option<(u32, IVec3, i32, i32, u64)>,
    /// Set by teleports: moves sent before the client saw the teleport are stale and would read
    /// as a long fall, so they are ignored until one arrives near this point.
    teleported: Option<DVec3>,
}

pub struct Generated {
    dim: u32,
    pos: IVec3,
    cube: Cube,
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
}

pub const PHASES: [&str; 6] = ["insert", "messages", "regions", "request", "sync", "rest"];

pub struct Server {
    pub cfg: ServerConfig,
    pub dims: Vec<DimSet>,
    pub players: Vec<Player>,
    pub tick: u64,
    gen_tx: Sender<Generated>,
    gen_rx: Receiver<Generated>,
    incoming: Receiver<ServerConn>,
    /// Generated cubes waiting for insertion (lighting is budgeted per tick).
    backlog: std::collections::VecDeque<Generated>,
    storage: Option<Arc<Mutex<Storage>>>,
    shared: Arc<Shared>,
    next_id: Arc<AtomicU64>,
    /// Names last sent to clients as the online list.
    online: Vec<String>,
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
            let step = Duration::from_secs_f64(1.0 / TPS as f64);
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
    ServerHandle { connect, shared, stop, thread: Some(thread) }
}

/// Cubes kept loaded around a view: the visible range plus, above it, the column up to where the
/// generator guarantees open sky (capped), so sky light reaches the visible cubes without having
/// to send or render those extra cubes.
fn loaded_y_range(def: &DimensionDef, center_y: i32, h: i32) -> (i32, i32) {
    let lo = (center_y - h).max(def.min_cube_y);
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
        Server {
            cfg,
            dims,
            players: Vec::new(),
            tick: 0,
            gen_tx,
            gen_rx,
            incoming,
            backlog: Default::default(),
            storage: storage.map(|s| Arc::new(Mutex::new(s))),
            shared,
            next_id,
            online: Vec::new(),
        }
    }

    /// Paused single player: keep talking to the client but do not advance the world.
    fn idle(&mut self) {
        let c = content();
        self.accept();
        self.receive_generated(c);
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
                sync_done: None,
                visible: FxHashSet::default(),
                teleported: None,
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
        self.receive_generated(c);
        lap(0, &self.shared);
        self.handle_messages(c);
        lap(1, &self.shared);

        // Every region of every dimension is independent: tick them all in parallel.
        for d in &mut self.dims {
            let t = d.time;
            for r in &mut d.regions {
                r.time = t;
            }
            d.time += 1;
        }
        self.dims.par_iter_mut().flat_map(|d| d.regions.par_iter_mut()).for_each(|r| r.tick(c));
        lap(2, &self.shared);

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
        }
        self.tick += 1;
        if self.tick % AUTOSAVE_TICKS == 0 {
            self.save_all(false);
        }
        lap(5, &self.shared);
        self.shared.tick_us.store(start.elapsed().as_micros() as u64, Ordering::Relaxed);
    }

    fn dim_events(&mut self, c: &'static Content) {
        let mut events = Vec::new();
        for d in &mut self.dims {
            for r in &mut d.regions {
                events.append(&mut r.events);
            }
        }
        for ev in events {
            match ev {
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
                        p.conn.send(ToClient::Health { health: 0.0, max: c.game.entities[c.game.player_kind as usize].max_health });
                        p.conn.send(ToClient::Died);
                        log::info!("{} died", p.name);
                    }
                }
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
        let m = menu::open(r, c, p.entity, kind, owner, p.window);
        p.conn.send(ToClient::OpenMenu { kind, window: p.window, slots: m.slots.len() as u32 });
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
                for pos in std::mem::take(&mut r.modified) {
                    if let Some(cube) = r.cubes.get(&pos) {
                        let bytes = entities_in(c, r, pos);
                        s.save(c, r.id, pos, cube, &bytes);
                        n += 1;
                    }
                }
            }
        }
        s.meta.times = self.dims.iter().map(|d| (c.dims[d.id as usize].name.clone(), d.time)).collect();
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

    fn receive_generated(&mut self, c: &'static Content) {
        self.backlog.extend(self.gen_rx.try_iter());
        // Top-down so sky light is right the first time: a cube waits while the one above it is
        // still generating or waiting itself. Sorted to keep runs reproducible.
        let mut list: Vec<Generated> = self.backlog.drain(..).collect();
        list.sort_by_key(|g| (g.dim, -g.pos.y, g.pos.x, g.pos.z));
        let mut pending: FxHashSet<(u32, IVec3)> = list.iter().map(|g| (g.dim, g.pos)).collect();
        let start = Instant::now();
        for g in list {
            let above = (g.dim, g.pos + IVec3::Y);
            let late = start.elapsed() > Duration::from_millis(15);
            if late || pending.contains(&above) || self.dims[g.dim as usize].generating.contains(&above.1) {
                self.backlog.push_back(g);
                continue;
            }
            pending.remove(&(g.dim, g.pos));
            let t = Instant::now();
            self.dims[g.dim as usize].insert(c, g.pos, g.cube, &g.entities, g.conn);
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
            ToServer::Move { pos, yaw, pitch, on_ground } => {
                if !pos.is_finite() || self.players[pi].dead {
                    return;
                }
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
                    if p.fall > 3.0 {
                        land = Some(p.fall - 3.0);
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
                    e.yaw = yaw;
                    e.pitch = pitch;
                    e.on_ground = on_ground;
                    e.in_liquid = in_liquid;
                }
                if let Some(amount) = land {
                    r.damage(c, entity, amount, 0, damage::FALL);
                }
                if in_liquid {
                    self.players[pi].fall = 0.0;
                }
            }
            ToServer::Break { .. } if self.mode(pi) == GameMode::Spectator => {}
            ToServer::Break { pos } => {
                let Some((r, entity)) = self.region(pi) else { return };
                let Some(e) = r.entities.get(entity) else { return };
                if !within(e.pos, pos) {
                    return;
                }
                let sel = e.selected;
                let held = r.entities.inventories.get(&entity).and_then(|i| i.get(sel as usize)).copied().unwrap_or_default();
                let free = c.states.get(r.get(pos).unwrap_or(0) as usize).is_some_and(|s| s.hardness <= 0.0);
                if r.player_break(c, pos, entity, held.item) && !free {
                    wear(c, r, entity, sel, 1);
                }
            }
            ToServer::Use { .. } if self.mode(pi) == GameMode::Spectator => {}
            ToServer::Use { pos, face, look, sneak } => {
                let Some((r, entity)) = self.region(pi) else { return };
                let Some(e) = r.entities.get(entity) else { return };
                if !within(e.pos, pos) {
                    return;
                }
                let (sel, epos) = (e.selected, e.pos);
                if !sneak && r.player_use(c, pos, face, entity) {
                    self.dim_events(c);
                    return;
                }
                let held = r.entities.inventories.get(&entity).and_then(|i| i.get(sel as usize)).copied().unwrap_or_default();
                match c.items.get(held.item as usize).and_then(|i| i.block) {
                    Some(state) if !held.is_empty() => {
                        let target = if r.get(pos).is_some_and(|s| c.states[s as usize].flags & abi::block_flags::REPLACEABLE != 0) {
                            pos
                        } else {
                            pos + crate::world::DIRS[face as usize]
                        };
                        let player_box = crate::physics::Aabb::around(epos, 0.3, 1.8);
                        let block_box = crate::physics::Aabb { min: target.as_dvec3(), max: target.as_dvec3() + DVec3::ONE };
                        let solid = c.states[state as usize].flags & abi::block_flags::SOLID != 0;
                        if (!solid || !player_box.intersects(&block_box)) && r.place(c, target, face, state, look, entity) {
                            if !r.entities.get(entity).is_some_and(|e| e.mode == GameMode::Creative) {
                                r.entities.set_slot(entity, sel, ItemStack { count: held.count - 1, ..held });
                            }
                        }
                    }
                    _ => self.handle(c, pi, ToServer::UseItem { target: Some((pos, face)) }),
                }
            }
            ToServer::UseItem { .. } if self.mode(pi) == GameMode::Spectator => {}
            ToServer::UseItem { target } => {
                let Some((r, entity)) = self.region(pi) else { return };
                let Some(sel) = r.entities.get(entity).map(|e| e.selected) else { return };
                let held = r.entities.inventories.get(&entity).and_then(|i| i.get(sel as usize)).copied().unwrap_or_default();
                let Some(def) = c.items.get(held.item as usize) else { return };
                if let Some(cb) = def.on_use {
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
                }
                r.flush_neighbors(c);
                r.finish_tick();
            }
            ToServer::Select(slot) => {
                if slot < 9 {
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
                let dmg = c.items.get(held.item as usize).filter(|_| held.item != 0).map_or(1.0, |i| i.attack);
                if r.damage(c, target, dmg, entity, damage::ATTACK) {
                    let tool = c.items.get(held.item as usize).map_or("", |i| i.tool.as_str());
                    wear(c, r, entity, sel, if tool == "sword" { 1 } else { 2 });
                }
            }
            ToServer::Interact(_) if self.mode(pi) == GameMode::Spectator => {}
            ToServer::Interact(target) => {
                let Some((r, entity)) = self.region(pi) else { return };
                let Some(kind) = r.entities.get(target).map(|e| e.kind) else { return };
                for &(owner, cb) in &c.game.entities[kind as usize].on_interact {
                    r.call(owner, cb, |w| abi::EntityEventArgs { world: w, entity: target, other: entity, amount: 0.0, cause: 0, handled: 0 });
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
            ToServer::CreativeTake { item } => {
                let Server { players, dims, .. } = self;
                let p = &mut players[pi];
                let creative = dims[p.dim as usize].entity_region_ref(p.entity).and_then(|r| r.entities.get(p.entity)).is_some_and(|e| e.mode == GameMode::Creative);
                if let (true, Some(m), Some(def)) = (creative, &mut p.menu, c.items.get(item as usize).filter(|_| item != 0)) {
                    let _ = def;
                    m.cursor = ItemStack::new(item, c.max_stack(item));
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
                let (sel, pos, yaw, pitch) = (e.selected, e.pos, e.yaw, e.pitch);
                let held = r.entities.inventories.get(&entity).and_then(|i| i.get(sel as usize)).copied().unwrap_or_default();
                if held.is_empty() {
                    return;
                }
                let n = if all { held.count } else { 1 };
                r.entities.set_slot(entity, sel, ItemStack { count: held.count - n, ..held });
                let id = r.spawn_item(c, pos + DVec3::Y * 1.3, ItemStack { count: n, ..held });
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
                let overworld = c.dim_names.get("base:overworld").copied().unwrap_or(0);
                let spawn = self.dims[overworld as usize].spawn_point(c);
                let max = c.game.entities[c.game.player_kind as usize].max_health;
                self.change_dim(c, pi, overworld, spawn);
                if let Some((r, entity)) = self.region(pi) {
                    if let Some(e) = r.entities.get_mut(entity) {
                        e.health = max;
                    }
                    let ents = &mut r.entities;
                    for ((id, comp), bytes) in ents.comps.iter_mut() {
                        if *id == entity {
                            bytes.copy_from_slice(&c.game.components[*comp as usize].default);
                            ents.dirty_comp.insert((*id, *comp));
                        }
                    }
                }
                self.players[pi].conn.send(ToClient::Health { health: max, max });
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
        let overworld = c.dim_names.get("base:overworld").copied().unwrap_or(0);
        let (dim, pos, health, inv, comps) = match saved {
            Some(s) => s,
            None => (overworld, self.dims[overworld as usize].spawn_point(c), -1.0, None, Vec::new()),
        };
        let time = self.dims[dim as usize].time;
        let r = self.dims[dim as usize].ensure_region(c, pos);
        let id = r.new_id();
        let kind = &c.game.entities[c.game.player_kind as usize];
        let mut e = Entity::new(id, c.game.player_kind, pos, if health > 0.0 { health } else { kind.max_health });
        e.player = true;
        e.mode = self.storage.as_ref().map_or(GameMode::Survival, |s| saved_mode(&s.lock(), &key));
        let mode = e.mode;
        r.entities.insert(c, e);
        if let Some(inv) = inv {
            r.entities.inventories.insert(id, inv);
        }
        for (k, v) in comps {
            r.entities.comps.insert((id, k), v);
        }
        let worn = worn_of(c, &r.entities, id);
        if let Some(e) = r.entities.get_mut(id) {
            e.worn = worn;
        }
        let health = r.entities.get(id).map_or(kind.max_health, |e| e.health);
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
        p.conn.send(ToClient::Time { dim, time });
        p.conn.send(ToClient::Health { health, max: kind.max_health });
        p.conn.send(ToClient::GameMode(mode));
    }

    fn set_mode(&mut self, pi: usize, mode: GameMode) {
        let (dim, entity) = (self.players[pi].dim, self.players[pi].entity);
        if let Some(e) = self.dims[dim as usize].entity_region(entity).and_then(|r| r.entities.get_mut(entity)) {
            e.mode = mode;
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
        if let Some((mut e, inv, comps)) = taken {
            e.pos = pos;
            let id = e.id;
            let r = dims[to as usize].ensure_region(c, pos);
            r.entities.insert(c, e);
            if let Some(inv) = inv {
                r.entities.inventories.insert(id, inv);
                r.entities.dirty_inv.insert(id);
            }
            for (k, v) in comps {
                r.entities.comps.insert((id, k), v);
                r.entities.dirty_comp.insert((id, k));
            }
        }
        if p.dim != to {
            p.dim = to;
            p.sent.clear();
            p.gen_done = None;
            p.sync_done = None;
            p.visible.clear();
            p.known.clear();
        }
        p.last_y = pos.y;
        p.fall = 0.0;
        p.teleported = Some(pos);
        p.conn.send(ToClient::Teleport { dim: to, pos });
        p.conn.send(ToClient::Time { dim: to, time: dims[to as usize].time });
    }

    fn command(&mut self, c: &'static Content, pi: usize, line: &str) -> String {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        let Some(&name) = tokens.first() else { return String::new() };
        let Some(def) = c.commands.iter().find(|d| d.name == name) else { return format!("unknown command /{name}, try /help") };
        let (dim, entity) = (self.players[pi].dim, self.players[pi].entity);
        let here = self.dims[dim as usize].entity_region_ref(entity).and_then(|r| r.entities.get(entity)).map_or(DVec3::ZERO, |e| e.pos);
        let players: Vec<String> = self.players.iter().filter(|p| p.joined).map(|p| p.name.clone()).collect();
        let args = match crate::commands::parse_args(c, &def.alts, &tokens[1..], here, &players) {
            Ok(a) => a,
            Err(e) => return format!("{e}. usage: /{} {}", def.name, def.usage),
        };
        match def.handler {
            None => self.builtin(c, pi, name, &args, here),
            Some((owner, cb)) => {
                let Some((r, player)) = self.region(pi) else { return String::new() };
                let values: Vec<abi::CommandValue> = args
                    .values
                    .iter()
                    .map(|v| {
                        use crate::commands::Value;
                        let mut out = abi::CommandValue { kind: abi::command_value::NONE, int: 0, float: 0.0, text: abi::Str::new(""), pos: [0.0; 3] };
                        match v {
                            None => {}
                            Some(Value::Int(i)) => (out.kind, out.int, out.float) = (abi::command_value::INT, *i, *i as f64),
                            Some(Value::Float(f)) => (out.kind, out.float) = (abi::command_value::FLOAT, *f),
                            Some(Value::Text(t)) => (out.kind, out.text) = (abi::command_value::TEXT, abi::Str::new(t)),
                            Some(Value::Id(i, n)) => (out.kind, out.int, out.text) = (abi::command_value::ID, *i as i64, abi::Str::new(n)),
                            Some(Value::Pos(p)) => (out.kind, out.pos) = (abi::command_value::POS, p.to_array()),
                        }
                        out
                    })
                    .collect();
                let mut reply = String::new();
                let reply_ptr = &mut reply as *mut String as *mut core::ffi::c_void;
                r.call(owner, cb, |w| abi::CommandArgs { world: w, player, values: values.as_ptr(), count: values.len() as u32, reply: reply_ptr });
                r.flush_neighbors(c);
                r.finish_tick();
                reply
            }
        }
    }

    fn builtin(&mut self, c: &'static Content, pi: usize, name: &str, a: &crate::commands::Args, here: DVec3) -> String {
        let dim = self.players[pi].dim;
        let block_at = |a: &crate::commands::Args| a.pos("pos").map(|p| p.floor().as_ivec3());
        match name {
            "help" => match a.text("command") {
                Some(n) => c.commands.iter().find(|d| d.name == n).map_or(format!("unknown command /{n}"), |d| format!("/{} {}", d.name, d.usage)),
                None => c.commands.iter().map(|d| format!("/{} {}", d.name, d.usage)).collect::<Vec<_>>().join("\n"),
            },
            "tp" => {
                if let Some(p) = a.pos("pos") {
                    self.change_dim(c, pi, dim, p);
                    return format!("teleported to {:.1} {:.1} {:.1}", p.x, p.y, p.z);
                }
                let target = a.text("target").unwrap_or_default().to_string();
                let found = self.players.iter().find(|p| p.joined && p.name == target).map(|p| (p.dim, p.entity));
                match found.and_then(|(d, e)| self.dims[d as usize].entity_region_ref(e)?.entities.get(e).map(|e| (d, e.pos))) {
                    Some((d, pos)) => {
                        self.change_dim(c, pi, d, pos);
                        format!("teleported to {target}")
                    }
                    None => format!("no player {target}"),
                }
            }
            "dim" => {
                let d = a.id("dim").unwrap_or(0);
                let spawn = self.dims[d as usize].spawn_point(c);
                self.change_dim(c, pi, d, spawn);
                format!("now in {}", a.text("dim").unwrap_or_default())
            }
            "give" => {
                let count = a.int("count").unwrap_or(1).clamp(1, 1 << 20) as u32;
                let (item, iname) = (a.id("item").unwrap_or(0), a.text("item").unwrap_or_default().to_string());
                let Some((r, e)) = self.region(pi) else { return String::new() };
                let left = r.entities.insert_item(c, e, ItemStack::new(item, count));
                format!("gave {} x {iname}", count - left.count)
            }
            "time" => {
                let t = a.int("ticks").unwrap_or(0).max(0) as u64;
                let d = &mut self.dims[dim as usize];
                let len = c.dims[d.id as usize].day_length.max(1) as u64;
                let now = d.time - d.time % len + t;
                d.set_time(now);
                format!("time set to {t}")
            }
            "summon" => {
                let count = a.int("count").unwrap_or(1).clamp(1, 100_000);
                let (kind, kname) = (a.id("entity").unwrap_or(0), a.text("entity").unwrap_or_default().to_string());
                let Some((r, _)) = self.region(pi) else { return String::new() };
                for _ in 0..count {
                    r.spawn(c, kind, here);
                }
                format!("summoned {count} {kname}")
            }
            "setblock" => {
                let (Some(p), Some(state)) = (block_at(a), a.id("block")) else { return String::new() };
                match self.dims[dim as usize].region_at(p) {
                    Some(r) => {
                        r.set(c, p, state);
                        r.flush_neighbors(c);
                        r.finish_tick();
                        format!("set {}", a.text("block").unwrap_or_default())
                    }
                    None => "not loaded".into(),
                }
            }
            "insert" => {
                let (Some(p), Some(item)) = (block_at(a), a.id("item")) else { return String::new() };
                let count = a.int("count").unwrap_or(1).clamp(1, 1 << 20) as u32;
                match self.dims[dim as usize].region_at(p) {
                    Some(r) => match r.entities.anchors.get(&p).copied() {
                        Some(owner) => {
                            let left = r.entities.insert_item(c, owner, ItemStack::new(item, count));
                            format!("inserted {} x {}", count - left.count, a.text("item").unwrap_or_default())
                        }
                        None => "no block entity there".into(),
                    },
                    None => "not loaded".into(),
                }
            }
            "block" => {
                let Some(p) = block_at(a) else { return String::new() };
                let d = &self.dims[dim as usize];
                format!("{} in {}", d.get(p).map_or("unloaded".into(), |s| c.state_name(s)), c.dims[d.id as usize].name)
            }
            "gamemode" => {
                let mode = GameMode::ALL.into_iter().find(|m| a.get(m.name()).is_some()).unwrap_or_default();
                self.set_mode(pi, mode);
                format!("game mode: {}", mode.name())
            }
            "heal" => {
                let max = c.game.entities[c.game.player_kind as usize].max_health;
                if let Some((r, e)) = self.region(pi) {
                    if let Some(e) = r.entities.get_mut(e) {
                        e.health = max;
                    }
                }
                self.players[pi].conn.send(ToClient::Health { health: max, max });
                "healed".into()
            }
            "regions" => {
                let parts: Vec<String> = self
                    .dims
                    .iter()
                    .filter(|d| !d.regions.is_empty())
                    .map(|d| {
                        let sizes: Vec<String> = d.regions.iter().map(|r| format!("{}/{}", r.cubes.len(), r.entities.list.len())).collect();
                        format!("{}: {} ({})", c.dims[d.id as usize].name, d.regions.len(), sizes.join(" "))
                    })
                    .collect();
                format!("regions {}", parts.join(", "))
            }
            _ => format!("unknown command /{name}"),
        }
    }

    fn view(&self, p: &Player) -> (IVec3, i32, i32) {
        let pos = self.dims[p.dim as usize].entity_region_ref(p.entity).and_then(|r| r.entities.get(p.entity)).map_or(DVec3::ZERO, |e| e.pos);
        let center = (pos.floor().as_ivec3()) >> 4;
        (center, self.cfg.view_radius, self.cfg.view_height)
    }

    /// Cubes players need, nearest columns first and each column top-down; loading and
    /// generation run on the rayon pool.
    fn request_generation(&mut self, c: &'static Content) {
        // Enough jobs to keep every worker busy for a whole tick; the backlog counts too so
        // generation never runs far ahead of lighting.
        let budget = rayon::current_num_threads() * 16;
        let in_flight: usize = self.dims.iter().map(|d| d.generating.len()).sum::<usize>() + self.backlog.len();
        if in_flight >= budget {
            return;
        }
        let queued: FxHashSet<(u32, IVec3)> = self.backlog.iter().map(|g| (g.dim, g.pos)).collect();
        let mut wanted: Vec<(i32, u32, IVec3)> = Vec::new();
        for pi in 0..self.players.len() {
            let p = &self.players[pi];
            if !p.joined {
                continue;
            }
            let (center, r, h) = self.view(p);
            let pdim = p.dim;
            let key = (pdim, center, r, h, self.dims[pdim as usize].settled_version());
            if p.gen_done == Some(key) {
                continue;
            }
            let def = &c.dims[pdim as usize];
            let (lo, hi) = loaded_y_range(def, center.y, h);
            let in_view = |pos: IVec3| {
                let d = pos - center;
                d.x * d.x + d.z * d.z <= r * r && d.y.abs() <= h && pos.y >= def.min_cube_y && pos.y <= def.max_cube_y
            };
            let dim = &mut self.dims[pdim as usize];
            let (visible, mut missing) = dim.visible(c, center, in_view, NEAR);
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
            let tops: Vec<IVec3> = visible
                .iter()
                .copied()
                .filter(|&p| dim.has(p) && !dim.settled(p) && !dim.has(p + IVec3::Y) && !visible.contains(&(p + IVec3::Y)) && !sky_column(p + IVec3::Y))
                .collect();
            for t in tops {
                dim.settle_down(t);
            }
            let _ = lo;
            for &pos in &missing {
                if !dim.generating.contains(&pos) && !queued.contains(&(pdim, pos)) {
                    // Nearest columns first, each from the top down.
                    let d = pos - center;
                    wanted.push(((d.x * d.x + d.z * d.z) * 64 - pos.y, pdim, pos));
                }
            }
            let p = &mut self.players[pi];
            p.gen_done = missing.is_empty().then_some(key);
            p.visible = visible;
        }
        if wanted.is_empty() {
            return;
        }
        wanted.sort_by_key(|w| w.0);
        wanted.dedup_by_key(|w| (w.1, w.2));
        for &(_, dim, pos) in wanted.iter().take(budget - in_flight) {
            self.dims[dim as usize].generating.insert(pos);
            let tx = self.gen_tx.clone();
            let seed = self.cfg.seed;
            let storage = self.storage.clone();
            let shared = self.shared.clone();
            rayon::spawn(move || {
                let t = Instant::now();
                let loaded = storage.and_then(|s| s.lock().load(dim, pos));
                let (cube, entities) = loaded.unwrap_or_else(|| (dim::generate(content(), dim, seed, pos), Vec::new()));
                let conn = dim::see_through(content(), &cube);
                shared.gen_ns.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
                shared.generated.fetch_add(1, Ordering::Relaxed);
                let _ = tx.send(Generated { dim, pos, cube, entities, conn });
            });
        }
    }

    fn sync_players(&mut self, c: &'static Content) {
        // Entities that moved since last tick and changed equipment, per dimension (all regions).
        let mut moved: Vec<Vec<(u64, DVec3, f32, f32)>> = Vec::with_capacity(self.dims.len());
        let mut worn: Vec<Vec<(u64, Vec<u32>)>> = Vec::with_capacity(self.dims.len());
        for d in &mut self.dims {
            let mut m = Vec::new();
            let mut w = Vec::new();
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
                    let now = (e.pos, e.yaw, e.pitch);
                    if e.sent.is_none_or(|s| (s.0 - now.0).length_squared() > 1e-6 || (s.1 - now.1).abs() > 0.01 || (s.2 - now.2).abs() > 0.01) {
                        if e.sent.is_some() {
                            m.push((e.id, e.pos, e.yaw, e.pitch));
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
            let (center, r, h) = self.view(&self.players[pi]);
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
                for e in reg.entities.list.iter().filter(|e| !e.removed && e.id != p.entity && e.anchor.is_none()).filter(shown) {
                    if (e.pos - me).length_squared() > range2 {
                        continue;
                    }
                    visible.insert(e.id);
                    if !p.known.contains(&e.id) {
                        let worn = e.worn.clone();
                        spawn.push(EntityView { id: e.id, kind: e.kind, pos: e.pos, yaw: e.yaw, pitch: e.pitch, item: e.item, health: e.health, worn });
                    }
                }
            }
            let gone: Vec<u64> = p.known.iter().copied().filter(|id| !visible.contains(id)).collect();
            if !gone.is_empty() {
                p.conn.send(ToClient::Despawn(gone));
            }
            if !spawn.is_empty() {
                p.conn.send(ToClient::Spawn(spawn));
            }
            let moves: Vec<(u64, DVec3, f32, f32)> = moved[p.dim as usize].iter().filter(|m| p.known.contains(&m.0) && visible.contains(&m.0)).copied().collect();
            if !moves.is_empty() {
                p.conn.send(ToClient::Moves(moves));
            }
            let worn: Vec<(u64, Vec<u32>)> = worn[p.dim as usize].iter().filter(|w| p.known.contains(&w.0) && visible.contains(&w.0)).cloned().collect();
            if !worn.is_empty() {
                p.conn.send(ToClient::Worn(worn));
            }
            p.known = visible;
            for reg in &dim.regions {
                let hurt: Vec<(u64, f32)> = reg.entities.hurt.iter().filter(|h| p.known.contains(&h.0)).copied().collect();
                if !hurt.is_empty() {
                    p.conn.send(ToClient::Hurt(hurt));
                }
            }

            if let Some(reg) = dim.entity_region(p.entity) {
                if let Some(&(_, hp)) = reg.entities.hurt.iter().rev().find(|h| h.0 == p.entity) {
                    if !p.dead {
                        p.conn.send(ToClient::Health { health: hp, max: c.game.entities[c.game.player_kind as usize].max_health });
                    }
                }
                // Own inventory and synced components.
                if p.first_sync || reg.entities.dirty_inv.contains(&p.entity) {
                    if let Some(inv) = reg.entities.inventories.get(&p.entity) {
                        p.conn.send(ToClient::Inventory(inv.iter().enumerate().map(|(i, s)| (i as u32, *s)).collect()));
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
                    let (slots, data, cursor) = menu::changes(m, reg, p.entity);
                    if !slots.is_empty() {
                        p.conn.send(ToClient::MenuSlots(slots));
                    }
                    if !data.is_empty() {
                        p.conn.send(ToClient::MenuData(data));
                    }
                    if let Some(cur) = cursor {
                        p.conn.send(ToClient::Cursor(cur));
                    }
                }
            }

            let key = (p.dim, center, r, h, dim.settled_version());
            if p.sync_done == Some(key) {
                continue;
            }
            let in_range = |pos: IVec3| {
                let d = pos - center;
                d.x * d.x + d.z * d.z <= r * r && d.y.abs() <= h && pos.y >= def.min_cube_y && pos.y <= def.max_cube_y
            };
            // Send a cube once its neighbours exist and its sky light is final.
            let mut to_send: Vec<(i32, IVec3)> = Vec::new();
            for dy in -h..=h {
                for dz in -r..=r {
                    for dx in -r..=r {
                        let pos = center + IVec3::new(dx, dy, dz);
                        if !in_range(pos) || p.sent.contains(&pos) || !dim.settled(pos) {
                            continue;
                        }
                        let ready = crate::world::DIRS.iter().all(|d| {
                            let n = pos + *d;
                            dim.has(n) || !in_range(n) || !p.visible.contains(&n)
                        });
                        if ready {
                            to_send.push((dx * dx + dz * dz + dy * dy, pos));
                        }
                    }
                }
            }
            to_send.sort_by_key(|t| t.0);
            unsent += to_send.len().saturating_sub(512) as u64;
            for &(_, pos) in to_send.iter().take(512) {
                if let Some(cube) = dim.snapshot(pos) {
                    let hidden = crate::world::DIRS.iter().enumerate().fold(0u8, |m, (i, d)| {
                        let n = pos + *d;
                        if dim.has(n) || (in_range(n) && p.visible.contains(&n)) { m } else { m | 1 << i }
                    });
                    p.conn.send(ToClient::Cube { dim: p.dim, pos, cube, hidden });
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
            p.sync_done = (to_send.is_empty() && far.is_empty()).then_some(key);
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
                r.resend.clear();
                r.changes.clear();
                r.entities.hurt.clear();
                r.entities.dirty_comp.clear();
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

    fn unload_unused(&mut self, c: &'static Content) {
        if self.tick % 40 != 0 {
            return;
        }
        for di in 0..self.dims.len() {
            let def = &c.dims[di];
            let views: Vec<(IVec3, i32, i32, i32)> = self
                .players
                .iter()
                .filter(|p| p.joined && p.dim as usize == di)
                .map(|p| {
                    let (center, _, _) = self.view(p);
                    let (lo, hi) = loaded_y_range(def, center.y, self.cfg.view_height);
                    (center, self.cfg.view_radius + 3, lo - 3, hi + 3)
                })
                .collect();
            let storage = self.storage.clone();
            let d = &mut self.dims[di];
            for r in &mut d.regions {
                let removed = r.retain_cubes(|pos| {
                    views.iter().any(|&(center, rad, lo, hi)| {
                        let d = pos - center;
                        d.x * d.x + d.z * d.z <= rad * rad && pos.y >= lo && pos.y <= hi
                    })
                });
                for (pos, cube, inside) in removed {
                    if let Some(s) = &storage {
                        let bytes = serialize(c, r, &inside);
                        s.lock().save(c, r.id, pos, &cube, &bytes);
                    }
                    for id in inside {
                        r.entities.remove(id);
                    }
                }
                r.entities.compact();
            }
            d.dirty = true;
        }
    }
}

/// Items in a player's equipment slots that show on the model.
fn worn_of(c: &Content, ents: &entity::Entities, id: u64) -> Vec<u32> {
    let Some(inv) = ents.inventories.get(&id).filter(|i| i.len() == entity::PLAYER_SLOTS as usize) else { return Vec::new() };
    inv[entity::PLAYER_WORN].iter().filter(|s| !s.is_empty() && c.worn(s.item).is_some()).map(|s| s.item).collect()
}

fn within(player: DVec3, block: IVec3) -> bool {
    (block.as_dvec3() + DVec3::splat(0.5) - (player + DVec3::Y * 1.6)).length_squared() < REACH * REACH
}

/// Damages the held tool; breaks it when used up.
fn wear(c: &Content, d: &mut Dim, player: u64, slot: u32, amount: u32) {
    let Some(stack) = d.entities.inventories.get(&player).and_then(|i| i.get(slot as usize)).copied() else { return };
    let Some(def) = c.items.get(stack.item as usize) else { return };
    if def.durability == 0 || stack.is_empty() {
        return;
    }
    let damage = stack.damage + amount;
    let next = if damage >= def.durability { ItemStack::EMPTY } else { ItemStack { damage, ..stack } };
    d.entities.set_slot(player, slot, next);
}

fn serialize(c: &Content, d: &Dim, ids: &[u64]) -> Vec<u8> {
    let list: Vec<(&Entity, Option<&Vec<ItemStack>>, Vec<(u32, &[u8])>)> = ids
        .iter()
        .filter_map(|&id| {
            let e = d.entities.get(id)?;
            let comps = d.entities.comps.iter().filter(|((eid, _), _)| *eid == id).map(|((_, k), v)| (*k, &v[..])).collect();
            Some((e, d.entities.inventories.get(&id), comps))
        })
        .collect();
    let mut out = Vec::new();
    persist::write(c, &mut out, &list);
    out
}

fn entities_in(c: &Content, d: &Dim, cube: IVec3) -> Vec<u8> {
    let ids: Vec<u64> = d.entities.list.iter().filter(|e| !e.removed && !e.player && cube_of(e.pos.floor().as_ivec3()) == cube).map(|e| e.id).collect();
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
            .map(|s| if s.is_empty() { "-".into() } else { format!("{}*{}*{}", c.items[s.item as usize].name, s.count, s.damage) })
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
    src.lines().find_map(|l| l.split_once('=').filter(|(k, _)| k.trim() == "mode").and_then(|(_, v)| GameMode::parse(v.trim()))).unwrap_or_default()
}

type PlayerData = (u32, DVec3, f32, Option<Vec<ItemStack>>, Vec<(u32, Box<[u8]>)>);

fn load_player(c: &Content, s: &Storage, key: &str) -> Option<PlayerData> {
    let src = std::fs::read_to_string(s.player_path(key)).ok()?;
    let (mut dim, mut pos, mut health, mut inv, mut comps) = (None, None, -1.0, None, Vec::new());
    for line in src.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim());
        match k {
            "dim" => dim = c.dim_names.get(v).copied(),
            "pos" => {
                let n: Vec<f64> = v.split_whitespace().filter_map(|x| x.parse().ok()).collect();
                if n.len() == 3 {
                    pos = Some(DVec3::new(n[0], n[1], n[2]));
                }
            }
            "health" => health = v.parse().unwrap_or(-1.0),
            "inventory" => {
                let mut slots = vec![ItemStack::EMPTY; entity::PLAYER_SLOTS as usize];
                for (i, tok) in v.split_whitespace().enumerate().take(slots.len()) {
                    let parts: Vec<&str> = tok.split('*').collect();
                    if parts.len() == 3 {
                        if let Some(&item) = c.item_names.get(parts[0]) {
                            slots[i] = ItemStack { item, count: parts[1].parse().unwrap_or(1), damage: parts[2].parse().unwrap_or(0), extra: 0 };
                        }
                    }
                }
                inv = Some(slots);
            }
            _ => {
                if let Some(name) = k.strip_prefix("comp.") {
                    if let Some(id) = c.game.component(name) {
                        let mut b = persist::unhex(v);
                        b.resize(c.game.components[id as usize].size as usize, 0);
                        comps.push((id, b.into_boxed_slice()));
                    }
                }
            }
        }
    }
    Some((dim?, pos?, health, inv, comps))
}
