//! Authoritative simulation. Runs on its own thread; single player talks to it through the
//! in-process transport.

mod dim;
pub mod energy;
pub mod entity;
pub mod menu;
pub mod persist;

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

use crate::content::{Content, content};
use crate::net::{EntityView, ServerConn, ToClient, ToServer};
use crate::saves::Storage;
use crate::world::{Cube, cube_of};
use entity::Entity;
use menu::OpenMenu;

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
}

pub struct Generated {
    dim: u32,
    pos: IVec3,
    cube: Cube,
    entities: Vec<u8>,
}

/// State the client thread may read or poke while the server runs.
#[derive(Default)]
pub struct Shared {
    pub paused: AtomicBool,
    pub hosting: AtomicBool,
    pub tick_us: AtomicU64,
    pub view: AtomicI32,
}

pub struct Server {
    pub cfg: ServerConfig,
    pub dims: Vec<Dim>,
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

fn view(e: &EntityView) -> EntityView {
    *e
}

impl Server {
    pub fn new(cfg: ServerConfig, incoming: Receiver<ServerConn>, storage: Option<Storage>, shared: Arc<Shared>) -> Server {
        let c = content();
        let next_id = Arc::new(AtomicU64::new(storage.as_ref().map_or(1, |s| s.meta.next_entity.max(1))));
        let mut dims: Vec<Dim> = (0..c.dims.len() as u32).map(|i| Dim::new(i, cfg.seed, next_id.clone())).collect();
        if let Some(s) = &storage {
            for (name, t) in &s.meta.times {
                if let Some(&i) = c.dim_names.get(name) {
                    dims[i as usize].time = *t;
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
            });
        }
    }

    pub fn tick(&mut self) {
        let start = Instant::now();
        let c = content();
        self.cfg.view_radius = self.shared.view.load(Ordering::Relaxed);
        self.accept();
        self.receive_generated(c);
        self.handle_messages(c);

        self.dims.par_iter_mut().for_each(|d| d.tick(c));

        self.dim_events(c);
        for pi in 0..self.players.len() {
            let p = &mut self.players[pi];
            if let Some(m) = &mut p.menu {
                menu::tick(m, &mut self.dims[p.dim as usize], c, p.entity);
            }
        }
        self.request_generation(c);
        self.sync_players(c);
        self.players.retain(|p| !p.conn.is_closed());
        self.tick += 1;
        if self.tick % AUTOSAVE_TICKS == 0 {
            self.save_all(false);
        }
        self.shared.tick_us.store(start.elapsed().as_micros() as u64, Ordering::Relaxed);
    }

    fn dim_events(&mut self, c: &'static Content) {
        for di in 0..self.dims.len() {
            for ev in std::mem::take(&mut self.dims[di].events) {
                match ev {
                    DimEvent::OpenMenu { player, menu, owner } => {
                        if let Some(pi) = self.players.iter().position(|p| p.entity == player) {
                            self.open_menu(c, pi, menu, owner);
                        }
                    }
                    DimEvent::Died { player } => {
                        if let Some(p) = self.players.iter_mut().find(|p| p.entity == player) {
                            p.dead = true;
                            if let Some(m) = p.menu.take() {
                                menu::close(m, &mut self.dims[p.dim as usize], c, player);
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
    }

    fn open_menu(&mut self, c: &'static Content, pi: usize, kind: u32, owner: u64) {
        if kind as usize >= c.game.menus.len() {
            return;
        }
        let p = &mut self.players[pi];
        let d = &mut self.dims[p.dim as usize];
        if let Some(m) = p.menu.take() {
            menu::close(m, d, c, p.entity);
        }
        p.window += 1;
        let m = menu::open(d, c, p.entity, kind, owner, p.window);
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
            // Cubes whose block entities changed inventory count as modified.
            let anchored: Vec<IVec3> = d
                .entities
                .list
                .iter()
                .filter(|e| e.anchor.is_some() && d.entities.dirty_inv.contains(&e.id))
                .map(|e| cube_of(e.pos.floor().as_ivec3()))
                .collect();
            d.modified.extend(anchored);
            for pos in std::mem::take(&mut d.modified) {
                if let Some(cube) = d.cubes.get(&pos) {
                    let bytes = entities_in(c, d, pos);
                    s.save(c, d.id, pos, cube, &bytes);
                    n += 1;
                }
            }
        }
        s.meta.times = self.dims.iter().map(|d| (c.dims[d.id as usize].name.clone(), d.time)).collect();
        s.meta.next_entity = self.next_id.load(Ordering::Relaxed);
        for p in &self.players {
            if p.joined || players {
                save_player(&s, p, &self.dims[p.dim as usize]);
            }
        }
        s.flush();
        if n > 0 {
            log::info!("saved {n} cubes in {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);
        }
    }

    fn receive_generated(&mut self, c: &'static Content) {
        let mut batch: Vec<Generated> = self.gen_rx.try_iter().collect();
        // Top-down so sky light is right the first time; sorted to keep runs reproducible.
        batch.sort_by_key(|g| (g.dim, -g.pos.y, g.pos.x, g.pos.z));
        self.backlog.extend(batch);
        let start = Instant::now();
        while let Some(g) = self.backlog.pop_front() {
            if start.elapsed() > Duration::from_millis(15) {
                self.backlog.push_front(g);
                break;
            }
            let d = &mut self.dims[g.dim as usize];
            d.insert(c, g.pos, g.cube);
            for (e, inv, comps) in persist::read(c, &g.entities) {
                let id = e.id;
                if d.entities.get(id).is_some() {
                    continue;
                }
                d.entities.insert(c, e);
                if let Some(inv) = inv {
                    d.entities.inventories.insert(id, inv);
                }
                for (k, v) in comps {
                    d.entities.comps.insert((id, k), v);
                }
            }
        }
    }

    fn player_entity(&mut self, c: &'static Content, pi: usize) -> Option<&mut Entity> {
        let p = &self.players[pi];
        self.dims[p.dim as usize].entities.get_mut(p.entity).filter(|_| c.game.player_kind != u32::MAX)
    }

    fn handle_messages(&mut self, c: &'static Content) {
        for pi in 0..self.players.len() {
            while let Some(msg) = self.players[pi].conn.recv() {
                self.handle(c, pi, msg);
            }
        }
    }

    fn handle(&mut self, c: &'static Content, pi: usize, msg: ToServer) {
        match msg {
            ToServer::Join { name, key } => self.join(c, pi, name, key),
            ToServer::Move { pos, yaw, pitch, on_ground } => {
                if !pos.is_finite() || self.players[pi].dead {
                    return;
                }
                let (entity, dim) = (self.players[pi].entity, self.players[pi].dim as usize);
                let p = &mut self.players[pi];
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
                let d = &mut self.dims[dim];
                let in_liquid = d.get(pos.floor().as_ivec3()).is_some_and(|s| c.states[s as usize].flags & abi::block_flags::LIQUID != 0);
                if in_liquid {
                    self.players[pi].fall = 0.0;
                    land = None;
                }
                if let Some(e) = d.entities.get_mut(entity) {
                    let moved = ((pos.x - e.pos.x).powi(2) + (pos.z - e.pos.z).powi(2)).sqrt() as f32;
                    e.moved = moved;
                    e.pos = pos;
                    e.yaw = yaw;
                    e.pitch = pitch;
                    e.on_ground = on_ground;
                    e.in_liquid = in_liquid;
                }
                if let Some(amount) = land {
                    d.damage(c, entity, amount, 0, damage::FALL);
                }
            }
            ToServer::Break { pos } => {
                let (entity, dim) = (self.players[pi].entity, self.players[pi].dim as usize);
                let d = &mut self.dims[dim];
                let Some(e) = d.entities.get(entity) else { return };
                if !within(e.pos, pos) {
                    return;
                }
                let held = d.entities.inventories.get(&entity).and_then(|i| i.get(e.selected as usize)).copied().unwrap_or_default();
                if d.player_break(c, pos, entity, held.item) {
                    wear(c, d, entity, e_selected(d, entity), 1);
                }
            }
            ToServer::Use { pos, face, look, sneak } => {
                let (entity, dim) = (self.players[pi].entity, self.players[pi].dim as usize);
                let d = &mut self.dims[dim];
                let Some(e) = d.entities.get(entity) else { return };
                if !within(e.pos, pos) {
                    log::debug!("use at {pos} out of reach from {:.2?}", e.pos);
                    return;
                }
                let (sel, epos) = (e.selected, e.pos);
                log::debug!("use at {pos} face {face} held slot {sel}: {}", d.get(pos).map(|s| c.state_name(s)).unwrap_or_default());
                if !sneak && d.player_use(c, pos, face, entity) {
                    self.dim_events(c);
                    return;
                }
                let held = d.entities.inventories.get(&entity).and_then(|i| i.get(sel as usize)).copied().unwrap_or_default();
                match c.items.get(held.item as usize).and_then(|i| i.block) {
                    Some(state) if !held.is_empty() => {
                        let target = if d.get(pos).is_some_and(|s| c.states[s as usize].flags & abi::block_flags::REPLACEABLE != 0) {
                            pos
                        } else {
                            pos + crate::world::DIRS[face as usize]
                        };
                        let player_box = crate::physics::Aabb::around(epos, 0.3, 1.8);
                        let block_box = crate::physics::Aabb { min: target.as_dvec3(), max: target.as_dvec3() + DVec3::ONE };
                        let solid = c.states[state as usize].flags & abi::block_flags::SOLID != 0;
                        log::debug!("place {} at {target}: blocked {}", c.state_name(state), solid && player_box.intersects(&block_box));
                        if (!solid || !player_box.intersects(&block_box)) && d.place(c, target, face, state, look, entity) {
                            d.entities.set_slot(entity, sel, ItemStack { count: held.count - 1, ..held });
                        }
                    }
                    _ => self.handle(c, pi, ToServer::UseItem { target: Some((pos, face)) }),
                }
            }
            ToServer::UseItem { target } => {
                let (entity, dim) = (self.players[pi].entity, self.players[pi].dim as usize);
                let d = &mut self.dims[dim];
                let Some(sel) = d.entities.get(entity).map(|e| e.selected) else { return };
                let held = d.entities.inventories.get(&entity).and_then(|i| i.get(sel as usize)).copied().unwrap_or_default();
                let Some(def) = c.items.get(held.item as usize) else { return };
                if let Some(cb) = def.on_use {
                    let (pos, face) = target.unwrap_or((IVec3::ZERO, 0));
                    let a = d.call(def.owner, cb, |w| abi::ItemUseArgs {
                        world: w,
                        player: entity,
                        stack: held,
                        pos: pos.to_array(),
                        face: face as u32,
                        has_target: target.is_some() as u32,
                        handled: 0,
                    });
                    if a.handled != 0 && a.stack != held {
                        d.entities.set_slot(entity, sel, a.stack);
                    }
                }
                d.flush_neighbors(c);
                d.finish_tick();
            }
            ToServer::Select(slot) => {
                if slot < 9 {
                    if let Some(e) = self.player_entity(c, pi) {
                        e.selected = slot;
                    }
                }
            }
            ToServer::Attack(target) => {
                let (entity, dim) = (self.players[pi].entity, self.players[pi].dim as usize);
                let d = &mut self.dims[dim];
                let (Some(me), Some(t)) = (d.entities.get(entity), d.entities.get(target)) else { return };
                if (me.pos - t.pos).length_squared() > REACH * REACH || target == entity || !t.item.is_empty() || t.anchor.is_some() {
                    return;
                }
                let sel = me.selected;
                let held = d.entities.inventories.get(&entity).and_then(|i| i.get(sel as usize)).copied().unwrap_or_default();
                let dmg = c.items.get(held.item as usize).filter(|_| held.item != 0).map_or(1.0, |i| i.attack);
                if d.damage(c, target, dmg, entity, damage::ATTACK) {
                    let tool = c.items.get(held.item as usize).map_or("", |i| i.tool.as_str());
                    wear(c, d, entity, sel, if tool == "sword" { 1 } else { 2 });
                }
            }
            ToServer::Interact(target) => {
                let (entity, dim) = (self.players[pi].entity, self.players[pi].dim as usize);
                let d = &mut self.dims[dim];
                let Some(kind) = d.entities.get(target).map(|e| e.kind) else { return };
                for &(owner, cb) in &c.game.entities[kind as usize].on_interact {
                    d.call(owner, cb, |w| abi::EntityEventArgs { world: w, entity: target, other: entity, amount: 0.0, cause: 0, handled: 0 });
                }
            }
            ToServer::OpenInventory => {
                if let Some(m) = c.game.player_menu {
                    self.open_menu(c, pi, m, 0);
                }
            }
            ToServer::Click { slot, button, shift } => {
                let p = &mut self.players[pi];
                if let Some(m) = &mut p.menu {
                    menu::click(m, &mut self.dims[p.dim as usize], c, p.entity, slot, button, shift);
                }
            }
            ToServer::CloseMenu => {
                let p = &mut self.players[pi];
                if let Some(m) = p.menu.take() {
                    menu::close(m, &mut self.dims[p.dim as usize], c, p.entity);
                }
            }
            ToServer::Drop { all } => {
                let (entity, dim) = (self.players[pi].entity, self.players[pi].dim as usize);
                let d = &mut self.dims[dim];
                let Some(e) = d.entities.get(entity) else { return };
                let (sel, pos, yaw, pitch) = (e.selected, e.pos, e.yaw, e.pitch);
                let held = d.entities.inventories.get(&entity).and_then(|i| i.get(sel as usize)).copied().unwrap_or_default();
                if held.is_empty() {
                    return;
                }
                let n = if all { held.count } else { 1 };
                d.entities.set_slot(entity, sel, ItemStack { count: held.count - n, ..held });
                let id = d.spawn_item(c, pos + DVec3::Y * 1.3, ItemStack { count: n, ..held });
                if let Some(it) = d.entities.get_mut(id) {
                    let (sy, cy) = (yaw as f64).sin_cos();
                    let (sp, cp) = (pitch as f64).sin_cos();
                    it.vel = DVec3::new(-sy * cp, sp + 0.2, -cy * cp) * 6.0;
                    it.pickup_delay = 40;
                }
            }
            ToServer::Respawn => {
                let p = &mut self.players[pi];
                if !p.dead {
                    return;
                }
                p.dead = false;
                p.fall = 0.0;
                let overworld = c.dim_names.get("base:overworld").copied().unwrap_or(0);
                let spawn = self.dims[overworld as usize].spawn_point(c);
                let max = c.game.entities[c.game.player_kind as usize].max_health;
                self.change_dim(c, pi, overworld, spawn);
                let p = &mut self.players[pi];
                if let Some(e) = self.dims[p.dim as usize].entities.get_mut(p.entity) {
                    e.health = max;
                }
                let ents = &mut self.dims[p.dim as usize].entities;
                for ((id, comp), bytes) in ents.comps.iter_mut() {
                    if *id == p.entity {
                        bytes.copy_from_slice(&c.game.components[*comp as usize].default);
                        ents.dirty_comp.insert((*id, *comp));
                    }
                }
                p.conn.send(ToClient::Health { health: max, max });
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
                let p = &mut self.players[pi];
                log::info!("{} left", p.name);
                if let Some(m) = p.menu.take() {
                    menu::close(m, &mut self.dims[p.dim as usize], c, p.entity);
                }
                if let Some(s) = &self.storage {
                    save_player(&s.lock(), p, &self.dims[p.dim as usize]);
                }
                self.dims[p.dim as usize].entities.remove(p.entity);
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
        let d = &mut self.dims[dim as usize];
        let id = d.new_id();
        let kind = &c.game.entities[c.game.player_kind as usize];
        let mut e = Entity::new(id, c.game.player_kind, pos, if health > 0.0 { health } else { kind.max_health });
        e.player = true;
        d.entities.insert(c, e);
        if let Some(inv) = inv {
            d.entities.inventories.insert(id, inv);
        }
        for (k, v) in comps {
            d.entities.comps.insert((id, k), v);
        }
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
        p.conn.send(ToClient::Time { dim, time: d.time });
        let health = d.entities.get(id).map_or(kind.max_health, |e| e.health);
        p.conn.send(ToClient::Health { health, max: kind.max_health });
    }

    /// Moves a player (with inventory and components) to another dimension without reloading.
    pub fn change_dim(&mut self, c: &'static Content, pi: usize, to: u32, pos: DVec3) {
        let p = &mut self.players[pi];
        if let Some(m) = p.menu.take() {
            menu::close(m, &mut self.dims[p.dim as usize], c, p.entity);
            p.conn.send(ToClient::CloseMenu);
        }
        if p.dim != to {
            let taken = self.dims[p.dim as usize].entities.take(p.entity);
            if let Some((mut e, inv, comps)) = taken {
                e.pos = pos;
                let d = &mut self.dims[to as usize];
                let id = e.id;
                d.entities.insert(c, e);
                if let Some(inv) = inv {
                    d.entities.inventories.insert(id, inv);
                    d.entities.dirty_inv.insert(id);
                }
                for (k, v) in comps {
                    d.entities.comps.insert((id, k), v);
                    d.entities.dirty_comp.insert((id, k));
                }
            }
            p.dim = to;
            p.sent.clear();
            p.known.clear();
        } else if let Some(e) = self.dims[to as usize].entities.get_mut(p.entity) {
            e.pos = pos;
        }
        p.last_y = pos.y;
        p.fall = 0.0;
        p.conn.send(ToClient::Teleport { dim: to, pos });
        p.conn.send(ToClient::Time { dim: to, time: self.dims[to as usize].time });
    }

    fn command(&mut self, c: &'static Content, pi: usize, cmd: &str) -> String {
        let args: Vec<&str> = cmd.split_whitespace().collect();
        let p = &self.players[pi];
        let here = self.dims[p.dim as usize].entities.get(p.entity).map_or(DVec3::ZERO, |e| e.pos);
        match args.as_slice() {
            ["tp", x, y, z] => match (x.parse(), y.parse(), z.parse()) {
                (Ok(x), Ok(y), Ok(z)) => {
                    let dim = p.dim;
                    self.change_dim(c, pi, dim, DVec3::new(x, y, z));
                    format!("teleported to {x} {y} {z}")
                }
                _ => "usage: /tp x y z".into(),
            },
            ["dim", name] => {
                let full = if name.contains(':') { name.to_string() } else { format!("base:{name}") };
                match c.dim_names.get(&full) {
                    Some(&d) => {
                        let spawn = self.dims[d as usize].spawn_point(c);
                        self.change_dim(c, pi, d, spawn);
                        format!("now in {full}")
                    }
                    None => format!("unknown dimension {full}"),
                }
            }
            ["give", item, rest @ ..] => {
                let full = if item.contains(':') { item.to_string() } else { format!("base:{item}") };
                let count = rest.first().and_then(|n| n.parse().ok()).unwrap_or(1);
                match c.item_names.get(&full) {
                    Some(&i) => {
                        let left = self.dims[p.dim as usize].entities.insert_item(c, p.entity, ItemStack::new(i, count));
                        format!("gave {} x {full}", count - left.count)
                    }
                    None => format!("unknown item {full}"),
                }
            }
            ["time", "set", t] => match t.parse::<u64>() {
                Ok(t) => {
                    let d = &mut self.dims[p.dim as usize];
                    let len = c.dims[d.id as usize].day_length.max(1) as u64;
                    d.time = d.time - d.time % len + t;
                    format!("time set to {t}")
                }
                Err(_) => "usage: /time set ticks".into(),
            },
            ["summon", kind] => {
                let full = if kind.contains(':') { kind.to_string() } else { format!("base:{kind}") };
                match c.game.entity_names.get(&full) {
                    Some(&k) => {
                        let (sy, cy) = (self.dims[p.dim as usize].entities.get(p.entity).map_or(0.0, |e| e.yaw) as f64).sin_cos();
                        self.dims[p.dim as usize].spawn(c, k, here + DVec3::new(-sy, 0.0, -cy) * 3.0);
                        format!("summoned {full}")
                    }
                    None => format!("unknown entity {full}"),
                }
            }
            ["setblock", x, y, z, name] => {
                let full = if name.contains(':') { name.to_string() } else { format!("base:{name}") };
                match (x.parse(), y.parse(), z.parse(), c.state_by_name(&full)) {
                    (Ok(x), Ok(y), Ok(z), Some(state)) => {
                        let d = &mut self.dims[p.dim as usize];
                        d.set(c, IVec3::new(x, y, z), state);
                        d.flush_neighbors(c);
                        d.finish_tick();
                        format!("set {full}")
                    }
                    _ => "usage: /setblock x y z block".into(),
                }
            }
            ["insert", x, y, z, item, rest @ ..] => {
                let full = if item.contains(':') { item.to_string() } else { format!("base:{item}") };
                let count = rest.first().and_then(|n| n.parse().ok()).unwrap_or(1);
                let d = &mut self.dims[p.dim as usize];
                match (x.parse(), y.parse(), z.parse(), c.item_names.get(&full)) {
                    (Ok(x), Ok(y), Ok(z), Some(&i)) => match d.entities.anchors.get(&IVec3::new(x, y, z)).copied() {
                        Some(owner) => {
                            let left = d.entities.insert_item(c, owner, ItemStack::new(i, count));
                            format!("inserted {} x {full}", count - left.count)
                        }
                        None => "no block entity there".into(),
                    },
                    _ => "usage: /insert x y z item [count]".into(),
                }
            }
            ["block", x, y, z] => match (x.parse(), y.parse(), z.parse()) {
                (Ok(x), Ok(y), Ok(z)) => {
                    let d = &self.dims[p.dim as usize];
                    let s = d.get(IVec3::new(x, y, z));
                    format!("{} in {}", s.map_or("unloaded".into(), |s| c.state_name(s)), c.dims[d.id as usize].name)
                }
                _ => "usage: /block x y z".into(),
            },
            ["heal"] => {
                let max = c.game.entities[c.game.player_kind as usize].max_health;
                if let Some(e) = self.dims[p.dim as usize].entities.get_mut(p.entity) {
                    e.health = max;
                }
                self.players[pi].conn.send(ToClient::Health { health: max, max });
                "healed".into()
            }
            _ => format!("unknown command /{cmd}"),
        }
    }

    fn view(&self, p: &Player) -> (IVec3, i32, i32) {
        let pos = self.dims[p.dim as usize].entities.get(p.entity).map_or(DVec3::ZERO, |e| e.pos);
        let center = (pos.floor().as_ivec3()) >> 4;
        (center, self.cfg.view_radius, self.cfg.view_height)
    }

    /// Cubes players need, nearest first; loading/generation runs on the rayon pool.
    fn request_generation(&mut self, c: &'static Content) {
        let budget = rayon::current_num_threads() * 4;
        let in_flight: usize = self.dims.iter().map(|d| d.generating.len()).sum();
        if in_flight >= budget {
            return;
        }
        let mut wanted: Vec<(i32, u32, IVec3)> = Vec::new();
        for p in self.players.iter().filter(|p| p.joined) {
            let (center, r, h) = self.view(p);
            let dim = &self.dims[p.dim as usize];
            let def = &c.dims[p.dim as usize];
            let (lo, hi) = loaded_y_range(def, center.y, h);
            for y in lo..=hi {
                for dz in -r..=r {
                    for dx in -r..=r {
                        if dx * dx + dz * dz > r * r {
                            continue;
                        }
                        let pos = IVec3::new(center.x + dx, y, center.z + dz);
                        if !dim.has(pos) && !dim.generating.contains(&pos) && !self.backlog.iter().any(|g| g.pos == pos && g.dim == p.dim) {
                            // Nearest columns first, each column from the top down.
                            wanted.push(((dx * dx + dz * dz) * 64 - y, p.dim, pos));
                        }
                    }
                }
            }
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
            rayon::spawn(move || {
                let loaded = storage.and_then(|s| s.lock().load(dim, pos));
                let (cube, entities) = loaded.unwrap_or_else(|| (dim::generate(content(), dim, seed, pos), Vec::new()));
                let _ = tx.send(Generated { dim, pos, cube, entities });
            });
        }
    }

    fn sync_players(&mut self, c: &'static Content) {
        // Entities that moved since last tick, per dimension.
        let mut moved: Vec<Vec<(u64, DVec3, f32, f32)>> = Vec::with_capacity(self.dims.len());
        for d in &mut self.dims {
            let mut m = Vec::new();
            for e in d.entities.list.iter_mut().filter(|e| !e.removed && e.anchor.is_none()) {
                let now = (e.pos, e.yaw, e.pitch);
                if e.sent.is_none_or(|s| (s.0 - now.0).length_squared() > 1e-6 || (s.1 - now.1).abs() > 0.01 || (s.2 - now.2).abs() > 0.01) {
                    if e.sent.is_some() {
                        m.push((e.id, e.pos, e.yaw, e.pitch));
                    }
                    e.sent = Some(now);
                }
            }
            moved.push(m);
        }

        for pi in 0..self.players.len() {
            if !self.players[pi].joined {
                continue;
            }
            let (center, r, h) = self.view(&self.players[pi]);
            let p = &mut self.players[pi];
            let dim = &mut self.dims[p.dim as usize];
            let def = &c.dims[p.dim as usize];

            for (pos, cube) in &dim.resend {
                if p.sent.contains(pos) {
                    p.conn.send(ToClient::Cube { dim: p.dim, pos: *pos, cube: cube.clone() });
                }
            }
            if !dim.changes.is_empty() {
                let changes: Vec<(IVec3, u32)> = dim.changes.iter().filter(|(b, _)| p.sent.contains(&(*b >> 4))).copied().collect();
                if !changes.is_empty() {
                    p.conn.send(ToClient::Blocks { dim: p.dim, changes });
                }
            }
            if self.tick % 20 == 0 {
                p.conn.send(ToClient::Time { dim: p.dim, time: dim.time });
            }

            // Entities in range: spawn new ones, despawn gone ones, stream movement.
            let me = dim.entities.get(p.entity).map_or(DVec3::ZERO, |e| e.pos);
            let range2 = ((r * 16) as f64).powi(2);
            let mut spawn = Vec::new();
            let mut visible = FxHashSet::default();
            for e in dim.entities.list.iter().filter(|e| !e.removed && e.id != p.entity && e.anchor.is_none()) {
                if (e.pos - me).length_squared() > range2 {
                    continue;
                }
                visible.insert(e.id);
                if !p.known.contains(&e.id) {
                    spawn.push(view(&EntityView { id: e.id, kind: e.kind, pos: e.pos, yaw: e.yaw, pitch: e.pitch, item: e.item, health: e.health }));
                }
            }
            let gone: Vec<u64> = p.known.iter().copied().filter(|id| !visible.contains(id)).collect();
            if !gone.is_empty() {
                p.conn.send(ToClient::Despawn(gone));
            }
            if !spawn.is_empty() {
                p.conn.send(ToClient::Spawn(spawn));
            }
            let moves: Vec<(u64, DVec3, f32, f32)> =
                moved[p.dim as usize].iter().filter(|m| p.known.contains(&m.0) && visible.contains(&m.0)).copied().collect();
            if !moves.is_empty() {
                p.conn.send(ToClient::Moves(moves));
            }
            p.known = visible;
            let hurt: Vec<(u64, f32)> = dim.entities.hurt.iter().filter(|h| p.known.contains(&h.0)).copied().collect();
            if !hurt.is_empty() {
                p.conn.send(ToClient::Hurt(hurt));
            }
            if let Some(&(_, hp)) = dim.entities.hurt.iter().rev().find(|h| h.0 == p.entity) {
                if !p.dead {
                    p.conn.send(ToClient::Health { health: hp, max: c.game.entities[c.game.player_kind as usize].max_health });
                }
            }

            // Own inventory and synced components.
            if p.first_sync || dim.entities.dirty_inv.contains(&p.entity) {
                if let Some(inv) = dim.entities.inventories.get(&p.entity) {
                    p.conn.send(ToClient::Inventory(inv.iter().enumerate().map(|(i, s)| (i as u32, *s)).collect()));
                }
            }
            let comps: Vec<(u32, Vec<u8>)> = dim
                .entities
                .comps
                .iter()
                .filter(|((id, comp), _)| {
                    *id == p.entity
                        && c.game.components[*comp as usize].flags & cf::SYNC_OWNER != 0
                        && (p.first_sync || dim.entities.dirty_comp.contains(&(*id, *comp)))
                })
                .map(|((_, comp), b)| (*comp, b.to_vec()))
                .collect();
            if !comps.is_empty() {
                p.conn.send(ToClient::Components(comps));
            }
            p.first_sync = false;

            if let Some(m) = &mut p.menu {
                let (slots, data, cursor) = menu::changes(m, dim, p.entity);
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
                        if !in_range(pos) || p.sent.contains(&pos) || !dim.has(pos) || !dim.settled.contains(&pos) {
                            continue;
                        }
                        let ready = crate::world::DIRS.iter().all(|d| {
                            let n = pos + *d;
                            dim.has(n) || !in_range(n)
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
        }
        for d in &mut self.dims {
            d.resend.clear();
            d.changes.clear();
            d.entities.hurt.clear();
            d.entities.dirty_comp.clear();
            // Block entity inventory changes must reach the save.
            let anchored: Vec<IVec3> = d
                .entities
                .list
                .iter()
                .filter(|e| e.anchor.is_some() && d.entities.dirty_inv.contains(&e.id))
                .map(|e| cube_of(e.pos.floor().as_ivec3()))
                .collect();
            d.modified.extend(anchored);
            d.entities.dirty_inv.clear();
            d.entities.spawned.clear();
            d.entities.despawned.clear();
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
            let dim = &mut self.dims[di];
            let removed = dim.retain_cubes(|pos| {
                views.iter().any(|&(center, r, lo, hi)| {
                    let d = pos - center;
                    d.x * d.x + d.z * d.z <= r * r && pos.y >= lo && pos.y <= hi
                })
            });
            let storage = self.storage.clone();
            for (pos, cube, inside) in removed {
                if let Some(s) = &storage {
                    let bytes = serialize(c, dim, &inside);
                    s.lock().save(c, dim.id, pos, &cube, &bytes);
                }
                for id in inside {
                    dim.entities.remove(id);
                }
            }
            dim.entities.compact();
        }
    }
}

/// Cubes kept loaded around a view: the visible range plus, above it, the column up to where the
/// generator guarantees open sky (capped), so sky light reaches the visible cubes without having
/// to send or render those extra cubes.
fn loaded_y_range(def: &crate::content::DimensionDef, center_y: i32, h: i32) -> (i32, i32) {
    let lo = (center_y - h).max(def.min_cube_y);
    let top = center_y + h;
    let hi = if def.sky_light && def.sky_cube_y != i32::MAX { top.max((def.sky_cube_y - 1).min(top + 12)) } else { top };
    (lo, hi.min(def.max_cube_y))
}

fn within(player: DVec3, block: IVec3) -> bool {
    (block.as_dvec3() + DVec3::splat(0.5) - (player + DVec3::Y * 1.6)).length_squared() < REACH * REACH
}

fn e_selected(d: &Dim, entity: u64) -> u32 {
    d.entities.get(entity).map_or(0, |e| e.selected)
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
    let ids: Vec<u64> = d
        .entities
        .list
        .iter()
        .filter(|e| !e.removed && !e.player && cube_of(e.pos.floor().as_ivec3()) == cube)
        .map(|e| e.id)
        .collect();
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
        "dim = {}\npos = {} {} {}\nlook = {} {}\nhealth = {}\n",
        c.dims[p.dim as usize].name, e.pos.x, e.pos.y, e.pos.z, e.yaw, e.pitch, e.health
    );
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

type PlayerData = (u32, DVec3, f32, Option<Vec<ItemStack>>, Vec<(u32, Box<[u8]>)>);

fn load_player(c: &Content, s: &Storage, name: &str) -> Option<PlayerData> {
    let src = std::fs::read_to_string(s.player_path(name)).ok()?;
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
