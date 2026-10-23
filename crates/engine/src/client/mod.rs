pub mod entity_render;
pub mod mesher;
pub mod modui;
pub mod textures;
pub mod world;
pub mod world_render;

use std::time::Instant;

use glam::{DVec3, IVec3, Vec2, Vec3};
use rustc_hash::FxHashMap;
use voxide_abi::{ItemStack, block_flags as bf};
use voxide_render::{Backend, CommandList, FrameInfo};

use crate::content::content;
use crate::input::Input;
use crate::net::{ClientConn, ToClient, ToServer};
use crate::physics::{self, Hit};
use entity_render::EntityRenderer;
use textures::GameTextures;
use world::ClientWorld;
use world_render::{Camera, FrameParams, WorldRenderer};

pub struct LocalPlayer {
    pub pos: DVec3,
    pub vel: DVec3,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    /// Touched the ground at any physics step since the last move sent: a quick re-jump can
    /// leave the ground before the next send, and the server would add up the falls.
    pub landed: bool,
    pub flying: bool,
    pub in_liquid: bool,
}

pub struct ClientEntity {
    pub kind: u32,
    pub pos: DVec3,
    pub target: DVec3,
    pub yaw: f32,
    pub target_yaw: f32,
    pub pitch: f32,
    pub item: ItemStack,
    pub health: f32,
    pub hurt: f32,
    pub worn: Vec<u32>,
    /// Accumulated walk distance and current swing amplitude, for limb animation.
    pub walk: f32,
    pub swing: f32,
    pub age: f32,
}

pub struct ClientMenu {
    pub kind: u32,
    pub window: u32,
    pub slots: Vec<ItemStack>,
    pub data: Vec<i32>,
    pub cursor: ItemStack,
}

pub enum ClientEvent {
    OpenMenu,
    CloseMenu,
    Died,
}

const HALF_WIDTH: f64 = 0.3;
const HEIGHT: f64 = 1.8;
const EYE: f64 = 1.62;
const STEP: f64 = 1.0 / 60.0;

pub struct Client {
    conn: ClientConn,
    pub world: ClientWorld,
    pub render: WorldRenderer,
    pub entity_render: EntityRenderer,
    pub player: LocalPlayer,
    pub joined: bool,
    pub player_id: u64,
    accum: f64,
    tick_accum: f64,
    start: Instant,
    pub target: Option<Hit>,
    pub target_entity: Option<u64>,
    pub selected: usize,
    pub disconnected: Option<String>,
    cooldown: f32,
    pub view_radius: i32,
    pub fov: f32,
    pub entities: FxHashMap<u64, ClientEntity>,
    pub inventory: Vec<ItemStack>,
    pub health: f32,
    pub max_health: f32,
    /// Mouse button and slots of a drag in progress in a menu.
    pub drag: Option<(u8, Vec<u32>)>,
    /// Seconds left of the red screen flash after taking damage.
    pub hurt_flash: f32,
    pub dead: bool,
    pub components: FxHashMap<u32, Vec<u8>>,
    pub menu: Option<ClientMenu>,
    pub events: Vec<ClientEvent>,
    pub chat: Vec<(String, Instant)>,
    /// Block being broken and progress 0..1.
    pub breaking: Option<(IVec3, f32)>,
    /// Hotbar contents last frame, so the HUD knows when to redraw.
    pub hotbar: Vec<u32>,
}

impl Client {
    pub fn new(gpu: &mut dyn Backend, textures: &GameTextures, mut conn: ClientConn, name: &str, key: &str, view_radius: i32) -> Result<Client, String> {
        let c = content();
        let render = WorldRenderer::new(gpu, c, textures)?;
        let entity_render = EntityRenderer::new(gpu)?;
        conn.send(ToServer::Join { name: name.to_string(), key: key.to_string() });
        Ok(Client {
            conn,
            world: ClientWorld::new(0),
            render,
            entity_render,
            player: LocalPlayer {
                pos: DVec3::new(0.0, 80.0, 0.0),
                vel: DVec3::ZERO,
                yaw: 0.0,
                pitch: 0.0,
                on_ground: false,
                landed: false,
                flying: false,
                in_liquid: false,
            },
            joined: false,
            player_id: 0,
            accum: 0.0,
            tick_accum: 0.0,
            start: Instant::now(),
            target: None,
            target_entity: None,
            selected: 0,
            disconnected: None,
            cooldown: 0.0,
            view_radius,
            fov: 70.0,
            entities: FxHashMap::default(),
            inventory: vec![ItemStack::EMPTY; 40],
            health: 20.0,
            max_health: 20.0,
            hurt_flash: 0.0,
            drag: None,
            dead: false,
            components: FxHashMap::default(),
            menu: None,
            events: Vec::new(),
            chat: Vec::new(),
            breaking: None,
            hotbar: vec![0; 9],
        })
    }

    pub fn send(&mut self, m: ToServer) {
        self.conn.send(m);
    }

    pub fn leave(&mut self) {
        self.conn.send(ToServer::Leave);
    }

    pub fn held(&self) -> ItemStack {
        self.inventory.get(self.selected).copied().unwrap_or_default()
    }

    fn receive(&mut self) {
        let c = content();
        while let Some(msg) = self.conn.recv() {
            match msg {
                ToClient::Welcome { player, dim, pos, .. } => {
                    self.player_id = player;
                    self.joined = true;
                    self.player.pos = pos;
                    if dim != self.world.dim {
                        self.world.clear();
                        self.world.dim = dim;
                    }
                }
                ToClient::Cube { dim, pos, cube, hidden } if dim == self.world.dim => self.world.insert(pos, cube, hidden),
                ToClient::Unload { dim, pos } if dim == self.world.dim => self.world.remove(pos),
                ToClient::Blocks { dim, changes } if dim == self.world.dim => {
                    for (p, s) in changes {
                        self.world.set_block(p, s);
                    }
                }
                ToClient::Time { dim, time } if dim == self.world.dim => self.world.time = time,
                ToClient::Teleport { dim, pos } => {
                    if dim != self.world.dim {
                        self.world.clear();
                        self.world.dim = dim;
                        self.entities.clear();
                    }
                    self.player.pos = pos;
                    self.player.vel = DVec3::ZERO;
                }
                ToClient::Spawn(list) => {
                    for e in list {
                        self.entities.insert(
                            e.id,
                            ClientEntity {
                                kind: e.kind,
                                pos: e.pos,
                                target: e.pos,
                                yaw: e.yaw,
                                target_yaw: e.yaw,
                                pitch: e.pitch,
                                item: e.item,
                                health: e.health,
                                hurt: 0.0,
                                worn: e.worn,
                                walk: 0.0,
                                swing: 0.0,
                                age: 0.0,
                            },
                        );
                    }
                }
                ToClient::Moves(list) => {
                    for (id, pos, yaw, pitch) in list {
                        if let Some(e) = self.entities.get_mut(&id) {
                            e.target = pos;
                            e.target_yaw = yaw;
                            e.pitch = pitch;
                        }
                    }
                }
                ToClient::Despawn(ids) => {
                    for id in ids {
                        self.entities.remove(&id);
                    }
                }
                ToClient::Hurt(list) => {
                    for (id, hp) in list {
                        if let Some(e) = self.entities.get_mut(&id) {
                            e.hurt = 0.4;
                            e.health = hp;
                        }
                    }
                }
                ToClient::Worn(list) => {
                    for (id, items) in list {
                        if let Some(e) = self.entities.get_mut(&id) {
                            e.worn = items;
                        }
                    }
                }
                ToClient::Inventory(slots) => {
                    for (i, s) in slots {
                        if let Some(dst) = self.inventory.get_mut(i as usize) {
                            *dst = s;
                        }
                    }
                }
                ToClient::Components(list) => {
                    for (k, v) in list {
                        self.components.insert(k, v);
                    }
                }
                ToClient::Health { health, max } => {
                    if health < self.health {
                        self.player.vel.y = self.player.vel.y.max(2.0);
                        self.hurt_flash = 0.5;
                    }
                    self.health = health;
                    self.max_health = max;
                    if health > 0.0 {
                        self.dead = false;
                    }
                }
                ToClient::OpenMenu { kind, window, slots } => {
                    let data = c.game.menus.get(kind as usize).map_or(0, |m| m.data_count);
                    self.menu = Some(ClientMenu {
                        kind,
                        window,
                        slots: vec![ItemStack::EMPTY; slots as usize],
                        data: vec![0; data as usize],
                        cursor: ItemStack::EMPTY,
                    });
                    self.events.push(ClientEvent::OpenMenu);
                }
                ToClient::MenuSlots(list) => {
                    if let Some(m) = &mut self.menu {
                        for (i, s) in list {
                            if let Some(dst) = m.slots.get_mut(i as usize) {
                                *dst = s;
                            }
                        }
                    }
                }
                ToClient::MenuData(list) => {
                    if let Some(m) = &mut self.menu {
                        for (i, v) in list {
                            if let Some(dst) = m.data.get_mut(i as usize) {
                                *dst = v;
                            }
                        }
                    }
                }
                ToClient::Cursor(s) => {
                    if let Some(m) = &mut self.menu {
                        m.cursor = s;
                    }
                }
                ToClient::CloseMenu => {
                    self.menu = None;
                    self.events.push(ClientEvent::CloseMenu);
                }
                ToClient::Died => {
                    self.dead = true;
                    self.events.push(ClientEvent::Died);
                }
                ToClient::Chat(text) => {
                    log::info!("[chat] {text}");
                    self.chat.push((text, Instant::now()));
                    if self.chat.len() > 50 {
                        self.chat.remove(0);
                    }
                }
                ToClient::Disconnect(reason) => self.disconnected = Some(reason),
                _ => {}
            }
        }
        if self.conn.is_closed() && self.disconnected.is_none() {
            self.disconnected = Some("connection lost".into());
        }
        for pos in std::mem::take(&mut self.world.removed) {
            self.render.remove(pos);
        }
    }

    pub fn close_menu(&mut self) {
        self.drag = None;
        if self.menu.take().is_some() {
            self.conn.send(ToServer::CloseMenu);
        }
    }

    pub fn update(&mut self, gpu: &mut dyn Backend, input: &Input, dt: f64) {
        self.receive();
        let c = content();
        if input.captured {
            let sens = 0.0022 * input.sensitivity;
            self.player.yaw -= input.mouse_delta.x * sens;
            self.player.pitch = (self.player.pitch - input.mouse_delta.y * sens).clamp(-1.55, 1.55);
        }
        if input.pressed("core:fly") {
            self.player.flying = !self.player.flying;
        }
        let old = self.selected;
        for i in 0..9 {
            if input.pressed(&format!("core:hotbar{}", i + 1)) {
                self.selected = i;
            }
        }
        if input.scroll != 0.0 {
            self.selected = (self.selected as i32 - input.scroll.signum() as i32).rem_euclid(9) as usize;
        }
        if old != self.selected {
            self.conn.send(ToServer::Select(self.selected as u32));
        }
        if input.pressed("core:drop") {
            self.conn.send(ToServer::Drop { all: input.held("core:sprint") });
        }
        if input.pressed("core:inventory") && input.captured {
            self.conn.send(ToServer::OpenInventory);
        }

        let world = &self.world;
        let block = |p: IVec3| {
            let s = world.block(p);
            (s != u32::MAX).then_some(s)
        };
        // Do not simulate until the ground under the player arrived.
        let ready = self.joined && !self.dead && block(self.player.pos.floor().as_ivec3()).is_some();
        if ready {
            self.accum += dt.min(0.25);
            while self.accum >= STEP {
                self.accum -= STEP;
                step_player(&mut self.player, input, &block, c.dims[world.dim as usize].gravity as f64);
            }
        }

        let eye = self.player.pos + DVec3::Y * EYE;
        let look = self.camera().forward().as_dvec3();
        self.target = physics::raycast(c, &block, eye, look, 5.0);
        let block_dist = self.target.map_or(f64::INFINITY, |h| (h.block.as_dvec3() + DVec3::splat(0.5) - eye).length());
        self.target_entity = self.pick_entity(eye, look, block_dist.min(4.5));

        self.cooldown -= dt as f32;
        if input.captured && !self.dead {
            if input.held_button(0) {
                if let Some(id) = self.target_entity {
                    if input.pressed_button(0) || self.cooldown <= 0.0 {
                        self.conn.send(ToServer::Attack(id));
                        self.cooldown = 0.4;
                    }
                    self.breaking = None;
                } else if let Some(hit) = self.target {
                    let held = self.held().item;
                    let (time, _) = crate::server::break_time(c, hit.state, held);
                    let progress = match self.breaking {
                        Some((p, v)) if p == hit.block => v,
                        _ => 0.0,
                    } + dt as f32 / time.max(0.001);
                    if progress >= 1.0 && self.cooldown <= 0.0 {
                        self.conn.send(ToServer::Break { pos: hit.block });
                        self.world.set_block(hit.block, crate::content::AIR);
                        self.breaking = None;
                        self.cooldown = 0.15;
                    } else {
                        self.breaking = Some((hit.block, progress.min(1.0)));
                    }
                } else {
                    self.breaking = None;
                }
            } else {
                self.breaking = None;
            }
            if input.held_button(1) && self.cooldown <= 0.0 {
                if let Some(id) = self.target_entity {
                    self.conn.send(ToServer::Interact(id));
                } else if let Some(hit) = self.target {
                    let sneak = input.held("core:sneak");
                    self.conn.send(ToServer::Use { pos: hit.block, face: hit.face, look: look.as_vec3().to_array(), sneak });
                } else {
                    self.conn.send(ToServer::UseItem { target: None });
                }
                self.cooldown = 0.25;
            }
        } else {
            self.breaking = None;
        }

        self.tick_accum += dt;
        if self.tick_accum >= 0.05 {
            self.tick_accum = 0.0;
            if self.joined && !self.dead {
                let landed = std::mem::replace(&mut self.player.landed, false);
                self.conn.send(ToServer::Move {
                    pos: self.player.pos,
                    yaw: self.player.yaw,
                    pitch: self.player.pitch,
                    on_ground: landed || self.player.on_ground || self.player.flying,
                });
            }
        }

        let k = (dt * 15.0).min(1.0);
        for e in self.entities.values_mut() {
            let before = e.pos;
            e.pos = e.pos.lerp(e.target, k);
            let mut dyaw = e.target_yaw - e.yaw;
            dyaw = (dyaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
            e.yaw += dyaw * k as f32;
            let speed = ((e.pos - before).with_y(0.0).length() / dt.max(1e-4)) as f32;
            e.walk += speed * dt as f32;
            e.swing += ((speed / 4.0).min(1.0) - e.swing) * (k as f32);
            e.hurt = (e.hurt - dt as f32).max(0.0);
            e.age += dt as f32;
        }
        self.hurt_flash = (self.hurt_flash - dt as f32).max(0.0);
        self.hotbar = self.inventory[..9].iter().map(|s| if s.is_empty() { 0 } else { s.item | (s.count.min(255) << 24) }).collect();

        let center = (self.player.pos.floor().as_ivec3()) >> 4;
        self.world.schedule_meshing(center, rayon::current_num_threads() * 2);
        for r in self.world.take_meshes() {
            self.render.upload(gpu, r.pos, &r.mesh);
        }
    }

    fn pick_entity(&self, eye: DVec3, dir: DVec3, max: f64) -> Option<u64> {
        let c = content();
        let mut best: Option<(f64, u64)> = None;
        for (&id, e) in &self.entities {
            let k = &c.game.entities[e.kind as usize];
            if !e.item.is_empty() || id == self.player_id {
                continue;
            }
            let hw = k.width as f64 / 2.0;
            let b = physics::Aabb::around(e.pos, hw, k.height as f64);
            if let Some(t) = ray_aabb(eye, dir, &b) {
                if t <= max && best.is_none_or(|(bt, _)| t < bt) {
                    best = Some((t, id));
                }
            }
        }
        best.map(|b| b.1)
    }

    /// Items the local player wears.
    pub fn worn(&self) -> Vec<u32> {
        let c = content();
        let slots = self.inventory.get(crate::server::entity::PLAYER_WORN).unwrap_or_default();
        slots.iter().filter(|s| !s.is_empty() && c.worn(s.item).is_some()).map(|s| s.item).collect()
    }

    pub fn camera(&self) -> Camera {
        Camera { pos: self.player.pos + DVec3::Y * EYE, yaw: self.player.yaw, pitch: self.player.pitch, fov: self.fov }
    }

    pub fn set_view_distance(&mut self, r: i32) {
        self.view_radius = r;
    }

    pub fn destroy(&mut self, gpu: &mut dyn Backend) {
        self.render.destroy(gpu);
        self.entity_render.destroy(gpu);
    }

    /// Fraction of full daylight for the current dimension and time.
    pub fn daylight(&self) -> f32 {
        let def = &content().dims[self.world.dim as usize];
        if !def.sky_light {
            return 0.0;
        }
        let t = if def.day_length == 0 { def.fixed_time as f64 } else { (self.world.time % def.day_length as u64) as f64 };
        let len = if def.day_length == 0 { 24000.0 } else { def.day_length as f64 };
        let phase = t / len;
        // 0 = sunrise, 0.25 = noon, 0.75 = midnight.
        ((phase * std::f64::consts::TAU).sin() * 2.0 + 0.6).clamp(0.12, 1.0) as f32
    }

    pub fn render(&mut self, gpu: &mut dyn Backend, list: &mut CommandList, frame: &FrameInfo, textures: &GameTextures) {
        let view_distance = self.view_radius as f32 * 16.0;
        let def = &content().dims[self.world.dim as usize];
        let day = self.daylight();
        let rgb = |c: u32| Vec3::new(((c >> 16) & 255) as f32, ((c >> 8) & 255) as f32, (c & 255) as f32) / 255.0;
        let lin = |v: Vec3| v.powf(2.2);
        let fog = lin(rgb(def.fog_color)) * (day * day).max(0.02);
        let sky = lin(rgb(def.sky_color)) * (day * day).max(0.02);
        let params = FrameParams {
            daylight: day,
            sky_color: sky,
            fog_color: fog,
            fog_start: view_distance * 0.6,
            fog_end: view_distance,
            time: self.start.elapsed().as_secs_f32(),
            ambient: def.ambient as u32,
        };
        let cam = self.camera();
        self.render.draw(gpu, list, frame, &cam, &params);
        list.push(voxide_render::Cmd::Mark);
        self.entity_render.fog = (params.fog_color.extend(1.0).to_array(), params.fog_start, params.fog_end);
        let world = &self.world;
        let light = |p: IVec3| world.light(p);
        self.entity_render.draw(gpu, list, frame, &cam, &self.entities, textures, &light, day, self.target, self.breaking);
        list.push(voxide_render::Cmd::Mark);
    }

    pub fn fog_color(&self) -> [f32; 4] {
        let def = &content().dims[self.world.dim as usize];
        let day = self.daylight();
        let c = def.fog_color;
        let v = Vec3::new(((c >> 16) & 255) as f32, ((c >> 8) & 255) as f32, (c & 255) as f32) / 255.0;
        (v.powf(2.2) * (day * day).max(0.02)).extend(1.0).to_array()
    }
}

fn ray_aabb(o: DVec3, d: DVec3, b: &physics::Aabb) -> Option<f64> {
    let mut t0: f64 = 0.0;
    let mut t1: f64 = f64::INFINITY;
    for i in 0..3 {
        if d[i].abs() < 1e-12 {
            if o[i] < b.min[i] || o[i] > b.max[i] {
                return None;
            }
            continue;
        }
        let a = (b.min[i] - o[i]) / d[i];
        let c = (b.max[i] - o[i]) / d[i];
        t0 = t0.max(a.min(c));
        t1 = t1.min(a.max(c));
    }
    (t0 <= t1).then_some(t0)
}

fn step_player(p: &mut LocalPlayer, input: &Input, block: &impl Fn(IVec3) -> Option<u32>, gravity: f64) {
    let c = content();
    let mut wish = Vec2::ZERO;
    if input.held("core:forward") {
        wish.y += 1.0;
    }
    if input.held("core:back") {
        wish.y -= 1.0;
    }
    if input.held("core:left") {
        wish.x -= 1.0;
    }
    if input.held("core:right") {
        wish.x += 1.0;
    }
    let wish = wish.normalize_or_zero();
    let (sy, cy) = (p.yaw as f64).sin_cos();
    let fwd = DVec3::new(-sy, 0.0, -cy);
    let right = DVec3::new(cy, 0.0, -sy);
    let dir = fwd * wish.y as f64 + right * wish.x as f64;

    let feet = p.pos.floor().as_ivec3();
    let at = |q: IVec3| block(q).map_or(0, |s| c.states[s as usize].flags);
    p.in_liquid = at(feet) & bf::LIQUID != 0 || at(feet + IVec3::Y) & bf::LIQUID != 0;
    let drag = block(feet).map_or(1.0, |s| c.states[s as usize].drag as f64);

    let sprint = input.held("core:sprint");
    if p.flying {
        let speed = if sprint { 30.0 } else { 11.0 };
        let mut v = dir * speed;
        if input.held("core:jump") {
            v.y += speed * 0.8;
        }
        if input.held("core:sneak") {
            v.y -= speed * 0.8;
        }
        p.vel = p.vel.lerp(v, 0.25);
    } else if p.in_liquid {
        let speed = 3.0;
        let target = dir * speed;
        p.vel.x += (target.x - p.vel.x) * 0.15;
        p.vel.z += (target.z - p.vel.z) * 0.15;
        p.vel.y -= gravity * 0.15 * STEP;
        if input.held("core:jump") {
            p.vel.y = (p.vel.y + 25.0 * STEP).min(4.0);
        }
        p.vel.y *= 0.96;
    } else {
        let speed = if sprint { 5.6 } else if input.held("core:sneak") { 1.3 } else { 4.3 } * drag;
        let target = dir * speed;
        let accel = if p.on_ground { 0.35 } else { 0.06 };
        p.vel.x += (target.x - p.vel.x) * accel;
        p.vel.z += (target.z - p.vel.z) * accel;
        p.vel.y -= gravity * STEP;
        p.vel.y *= 0.993;
        if input.held("core:jump") && p.on_ground {
            p.vel.y = 9.0;
        }
        let climbing = at(feet) & bf::CLIMBABLE != 0;
        if climbing {
            p.vel.y = if input.held("core:jump") || wish.y > 0.0 { 2.5 } else { p.vel.y.max(-2.0) };
        }
    }

    let step_h = if p.on_ground && !p.flying { 0.6 } else { 0.0 };
    let before = p.pos;
    let r = physics::move_box(c, block, p.pos, HALF_WIDTH, HEIGHT, p.vel * STEP, step_h);
    p.pos = r.pos;
    // Sneaking on an edge: do not walk off.
    if input.held("core:sneak") && p.on_ground && !p.flying && !r.on_ground {
        let below = |pos: DVec3| physics::move_box(c, block, pos, HALF_WIDTH, HEIGHT, DVec3::new(0.0, -0.6, 0.0), 0.0).on_ground;
        if !below(p.pos) {
            p.pos = before;
            p.vel.x = 0.0;
            p.vel.z = 0.0;
        }
    }
    for i in 0..3 {
        if r.hit[i] {
            p.vel[i] = 0.0;
        }
    }
    p.on_ground = r.on_ground || (p.pos == before && p.on_ground);
    p.landed |= p.on_ground;
}
