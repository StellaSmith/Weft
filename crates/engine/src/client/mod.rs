pub mod mesher;
pub mod textures;
pub mod world;
pub mod world_render;

use std::time::Instant;

use glam::{DVec3, IVec3, Vec2, Vec3};
use voxide_abi::block_flags as bf;
use voxide_render::{Backend, CommandList, FrameInfo};

use crate::content::{AIR, content};
use crate::input::Input;
use crate::net::{ClientConn, ToClient, ToServer};
use crate::physics::{self, Hit};
use textures::GameTextures;
use world::ClientWorld;
use world_render::{Camera, FrameParams, WorldRenderer};

pub struct LocalPlayer {
    pub pos: DVec3,
    pub vel: DVec3,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    pub flying: bool,
    pub in_liquid: bool,
}

const HALF_WIDTH: f64 = 0.3;
const HEIGHT: f64 = 1.8;
const EYE: f64 = 1.62;
const STEP: f64 = 1.0 / 60.0;

pub struct Client {
    conn: ClientConn,
    pub world: ClientWorld,
    pub render: WorldRenderer,
    pub player: LocalPlayer,
    pub joined: bool,
    pub player_id: u64,
    accum: f64,
    tick_accum: f64,
    start: Instant,
    pub target: Option<Hit>,
    pub hotbar: Vec<u32>,
    pub selected: usize,
    pub disconnected: Option<String>,
    break_cooldown: f32,
    pub view_radius: i32,
    pub fov: f32,
}

impl Client {
    pub fn new(gpu: &mut dyn Backend, textures: &GameTextures, mut conn: ClientConn, name: &str, view_radius: i32) -> Result<Client, String> {
        let c = content();
        let render = WorldRenderer::new(gpu, c, textures)?;
        conn.send(ToServer::Join { name: name.to_string() });
        let hotbar: Vec<u32> = ["base:grass", "base:stone", "base:oak_planks", "base:glass", "base:torch", "base:red_lamp", "base:green_lamp", "base:blue_lamp", "base:glowstone"]
            .iter()
            .filter_map(|n| c.item_names.get(*n).copied())
            .collect();
        Ok(Client {
            conn,
            world: ClientWorld::new(0),
            render,
            player: LocalPlayer {
                pos: DVec3::new(0.0, 80.0, 0.0),
                vel: DVec3::ZERO,
                yaw: 0.0,
                pitch: 0.0,
                on_ground: false,
                flying: false,
                in_liquid: false,
            },
            joined: false,
            player_id: 0,
            accum: 0.0,
            tick_accum: 0.0,
            start: Instant::now(),
            target: None,
            hotbar,
            selected: 0,
            disconnected: None,
            break_cooldown: 0.0,
            view_radius,
            fov: 70.0,
        })
    }

    pub fn leave(&mut self) {
        self.conn.send(ToServer::Leave);
    }

    fn receive(&mut self, gpu: &mut dyn Backend) {
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
                ToClient::Cube { dim, pos, cube } if dim == self.world.dim => self.world.insert(pos, cube),
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
                    }
                    self.player.pos = pos;
                    self.player.vel = DVec3::ZERO;
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
        let _ = gpu;
    }

    pub fn update(&mut self, gpu: &mut dyn Backend, input: &Input, dt: f64) {
        self.receive(gpu);
        let c = content();
        if input.captured {
            let sens = 0.0022 * input.sensitivity;
            self.player.yaw -= input.mouse_delta.x * sens;
            self.player.pitch = (self.player.pitch - input.mouse_delta.y * sens).clamp(-1.55, 1.55);
        }
        if input.pressed("core:fly") {
            self.player.flying = !self.player.flying;
        }
        for (i, key) in ["core:hotbar1", "core:hotbar2", "core:hotbar3", "core:hotbar4", "core:hotbar5", "core:hotbar6", "core:hotbar7", "core:hotbar8", "core:hotbar9"].iter().enumerate() {
            if input.pressed(key) && i < self.hotbar.len() {
                self.selected = i;
            }
        }
        if input.scroll != 0.0 && !self.hotbar.is_empty() {
            let n = self.hotbar.len() as i32;
            self.selected = (self.selected as i32 - input.scroll.signum() as i32).rem_euclid(n) as usize;
        }

        let world = &self.world;
        let block = |p: IVec3| {
            let s = world.block(p);
            (s != u32::MAX).then_some(s)
        };
        // Do not simulate until the ground under the player arrived.
        let ready = self.joined && block(self.player.pos.floor().as_ivec3()).is_some();
        if ready {
            self.accum += dt.min(0.25);
            while self.accum >= STEP {
                self.accum -= STEP;
                step_player(&mut self.player, input, &block, c.dims[world.dim as usize].gravity as f64);
            }
        }

        let eye = self.player.pos + DVec3::Y * EYE;
        let look = Camera { pos: eye, yaw: self.player.yaw, pitch: self.player.pitch, fov: 70.0 }.forward().as_dvec3();
        self.target = physics::raycast(c, &block, eye, look, 5.0);

        self.break_cooldown -= dt as f32;
        if input.captured && self.break_cooldown <= 0.0 {
            if input.held_button(0) {
                if let Some(hit) = self.target {
                    self.conn.send(ToServer::Break { pos: hit.block });
                    self.world.set_block(hit.block, AIR);
                    self.break_cooldown = 0.25;
                }
            } else if input.held_button(1) {
                if let Some(hit) = self.target {
                    let n = hit.block + crate::world::DIRS[hit.face as usize];
                    let state = self.hotbar.get(self.selected).and_then(|&i| c.items[i as usize].block).unwrap_or(AIR);
                    let player_box = physics::Aabb::around(self.player.pos, HALF_WIDTH, HEIGHT);
                    let block_box = physics::Aabb { min: n.as_dvec3(), max: n.as_dvec3() + DVec3::ONE };
                    let solid = c.states[state as usize].flags & bf::SOLID != 0;
                    let free = c.states[self.world.block(n).min(c.states.len() as u32 - 1) as usize].flags & bf::REPLACEABLE != 0;
                    if state != AIR && free && !(solid && player_box.intersects(&block_box)) {
                        self.conn.send(ToServer::Place { pos: n, face: hit.face, state, look: look.as_vec3().to_array() });
                        self.break_cooldown = 0.25;
                    }
                }
            }
        }

        self.tick_accum += dt;
        if self.tick_accum >= 0.05 {
            self.tick_accum = 0.0;
            if self.joined {
                self.conn.send(ToServer::Move {
                    pos: self.player.pos,
                    yaw: self.player.yaw,
                    pitch: self.player.pitch,
                    on_ground: self.player.on_ground,
                });
            }
        }

        let center = (self.player.pos.floor().as_ivec3()) >> 4;
        self.world.schedule_meshing(center, rayon::current_num_threads() * 2);
        for r in self.world.take_meshes() {
            self.render.upload(gpu, r.pos, &r.mesh);
        }
    }

    pub fn camera(&self) -> Camera {
        Camera { pos: self.player.pos + DVec3::Y * EYE, yaw: self.player.yaw, pitch: self.player.pitch, fov: self.fov }
    }

    pub fn set_view_distance(&mut self, r: i32) {
        self.view_radius = r;
    }

    pub fn destroy(&mut self, gpu: &mut dyn Backend) {
        self.render.destroy(gpu);
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

    pub fn render(&mut self, gpu: &mut dyn Backend, list: &mut CommandList, frame: &FrameInfo) {
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
    }

    pub fn fog_color(&self) -> [f32; 4] {
        let def = &content().dims[self.world.dim as usize];
        let day = self.daylight();
        let c = def.fog_color;
        let v = Vec3::new(((c >> 16) & 255) as f32, ((c >> 8) & 255) as f32, (c & 255) as f32) / 255.0;
        (v.powf(2.2) * (day * day).max(0.02)).extend(1.0).to_array()
    }
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
        let speed = if sprint { 5.6 } else { 4.3 } * drag;
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
    let r = physics::move_box(c, block, p.pos, HALF_WIDTH, HEIGHT, p.vel * STEP, step_h);
    p.pos = r.pos;
    for i in 0..3 {
        if r.hit[i] {
            p.vel[i] = 0.0;
        }
    }
    p.on_ground = r.on_ground;
}
