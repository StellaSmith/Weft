//! Particles, text conditions, screen effects: things that only change what players see.
//!
//! ```ignore
//! static SPARK: Particle = Particle::new();
//! r.particle("spark").handle(&SPARK).textures(&["glint_0", "glint_1"]).life(0.3, 0.6).fullbright();
//! w.particles(&SPARK, pos).count(12).speed(2.0);          // server: everyone nearby sees them
//! r.text_condition("charged", |v| v.player_component(&CHARGE).is_some_and(|c| *c > 0));
//! c.post_effect("mymod:underwater", 1.0);                  // client: until turned off
//! ```
//! Shaders for text (`{fx:..}`), GUI panels (theme `shader`), particles, entities and screen
//! effects are assets: `shaders/fx|gui|particle|entity|post/<name>.wgsl` in the mod's folder.

use std::marker::PhantomData;

use crate::abi::{self, Slice, Str, particle_flags as pf};
use crate::{Client, ClientView, Particle, Registry, World, callback, host, qualify};

pub struct ParticleBuilder<'a> {
    raw: *mut abi::Registrar,
    ns: &'static str,
    name: String,
    textures: Vec<String>,
    shader: String,
    handle: Option<&'static Particle>,
    desc: abi::ParticleDesc,
    _p: PhantomData<&'a mut Registry>,
}

impl ParticleBuilder<'_> {
    pub fn handle(mut self, h: &'static Particle) -> Self {
        self.handle = Some(h);
        self
    }
    /// Frames over its life; relative names are under `particle/`. None: block dust (pieces of
    /// the burst's block).
    pub fn textures(mut self, names: &[&str]) -> Self {
        self.textures = names.iter().map(|t| if t.contains(':') { t.to_string() } else { format!("{}:particle/{t}", self.ns) }).collect();
        self
    }
    /// Seconds, picked between the two.
    pub fn life(mut self, min: f32, max: f32) -> Self {
        self.desc.life = [min, max];
        self
    }
    /// Edge length in blocks, picked between the two.
    pub fn size(mut self, min: f32, max: f32) -> Self {
        self.desc.scale = [min, max];
        self
    }
    /// Blocks per second squared, pulling down (negative rises).
    pub fn gravity(mut self, g: f32) -> Self {
        self.desc.gravity = g;
        self
    }
    /// Fraction of speed kept per tick (vanilla 0.98).
    pub fn drag(mut self, d: f32) -> Self {
        self.desc.drag = d;
        self
    }
    /// 0xRRGGBBAA at birth and at the end of its life.
    pub fn color(mut self, start: u32, end: u32) -> Self {
        (self.desc.color, self.desc.color_end) = (start, end);
        self
    }
    pub fn collide(mut self) -> Self {
        self.desc.flags |= pf::COLLIDE;
        self
    }
    pub fn fullbright(mut self) -> Self {
        self.desc.flags |= pf::FULLBRIGHT;
        self
    }
    pub fn translucent(mut self) -> Self {
        self.desc.flags |= pf::TRANSLUCENT;
        self
    }
    pub fn shrink(mut self) -> Self {
        self.desc.flags |= pf::SHRINK;
        self
    }
    /// Colors it with `shaders/particle/<name>.wgsl` (`fn shade(p: ParticleFrag) -> vec4<f32>`).
    pub fn shader(mut self, name: &str) -> Self {
        self.shader = qualify(self.ns, name);
        self
    }
}

impl Drop for ParticleBuilder<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let textures: Vec<Str> = self.textures.iter().map(|t| Str::new(t)).collect();
        let desc = abi::ParticleDesc {
            name: Str::new(&self.name),
            cell: self.handle.map_or(std::ptr::null(), |h| h.cell()),
            textures: Slice::new(&textures),
            shader: Str::new(&self.shader),
            ..self.desc
        };
        unsafe { (host().reg_particle)(self.raw, &desc) }
    }
}

impl Registry {
    /// A particle kind, see `ParticleBuilder`. Registering `core:block` or `core:poof` changes how
    /// the engine's own block dust and smoke look.
    pub fn particle<'a>(&'a mut self, name: &str) -> ParticleBuilder<'a> {
        ParticleBuilder {
            raw: self.raw,
            ns: self.ns,
            name: qualify(self.ns, name),
            textures: Vec::new(),
            shader: String::new(),
            handle: None,
            desc: abi::ParticleDesc::DEFAULT,
            _p: PhantomData,
        }
    }

    /// `{if:<ns>:<name>}...{/if}` in any text shows its part while `f` says so; asked on the
    /// client every frame such a text is on screen.
    pub fn text_condition(&mut self, name: &str, f: impl Fn(&ClientView) -> bool + Send + Sync + 'static) {
        let name = qualify(self.ns, name);
        let cb = callback(move |a: &mut abi::TextConditionArgs| a.result = f(&ClientView::from_raw(a.client)) as u32);
        unsafe { (host().reg_text_condition)(self.raw, Str::new(&name), cb) }
    }
}

enum Target {
    World(World),
    Client(Client),
}

/// Particles sent when this is dropped: `w.particles(&SPARK, pos).count(10).speed(1.5)`.
pub struct Burst {
    target: Target,
    burst: abi::ParticleBurst,
}

impl Burst {
    pub fn count(mut self, n: u32) -> Self {
        self.burst.count = n;
        self
    }
    /// Each particle starts up to this far from the position on every axis.
    pub fn spread(mut self, s: f32) -> Self {
        self.burst.spread = [s; 3];
        self
    }
    pub fn spread_xyz(mut self, s: [f32; 3]) -> Self {
        self.burst.spread = s;
        self
    }
    /// Starting velocity, blocks per second.
    pub fn velocity(mut self, v: [f32; 3]) -> Self {
        self.burst.velocity = v;
        self
    }
    /// Random extra velocity up to this, in any direction.
    pub fn speed(mut self, s: f32) -> Self {
        self.burst.speed = s;
        self
    }
    /// Block state the textureless kinds (block dust) take their texture from.
    pub fn block(mut self, state: u32) -> Self {
        self.burst.block = state;
        self
    }
}

impl Drop for Burst {
    fn drop(&mut self) {
        if std::thread::panicking() || self.burst.kind == u32::MAX {
            return;
        }
        match &self.target {
            Target::World(w) => unsafe { (host().particles)(w.raw(), &self.burst) },
            Target::Client(c) => unsafe { (host().client_particles)(c.raw(), &self.burst) },
        }
    }
}

fn burst(target: Target, kind: &Particle, pos: [f64; 3]) -> Burst {
    Burst { target, burst: abi::ParticleBurst { kind: kind.id(), pos, ..abi::ParticleBurst::DEFAULT } }
}

impl World {
    /// Particles every player near `pos` sees (one by default).
    pub fn particles(&self, kind: &Particle, pos: [f64; 3]) -> Burst {
        burst(Target::World(*self), kind, pos)
    }

    /// A filled box for players showing `group` (`/gizmo <group>`), see-through by `color`'s alpha.
    pub fn gizmo_fill(&self, group: &str, min: [f64; 3], max: [f64; 3], color: u32, ticks: u32) {
        let g = qualify(crate::mod_id(), group);
        unsafe { (host().gizmo)(self.raw(), Str::new(&g), 3, &min, &max, color, ticks, Str::EMPTY) }
    }
}

impl Client {
    /// Particles only this client shows.
    pub fn particles(&self, kind: &Particle, pos: [f64; 3]) -> Burst {
        burst(Target::Client(*self), kind, pos)
    }

    /// Turns screen effect `name` (`shaders/post/<name>.wgsl`) on at `strength` (0 turns it off).
    pub fn post_effect(&self, name: &str, strength: f32) -> bool {
        let name = qualify(crate::mod_id(), name);
        unsafe { (host().client_command)(self.raw(), abi::client_op::POST_EFFECT, Str::new(&name), strength as f64) != 0 }
    }

    /// A filled box drawn this frame, see-through by `color`'s alpha (range previews, blueprints).
    pub fn draw_fill(&self, min: [f64; 3], max: [f64; 3], color: u32) {
        unsafe { (host().client_draw)(self.raw(), 4, &min, &max, color, Str::EMPTY) }
    }
}

pub struct SoundBuilder<'a> {
    raw: *mut abi::Registrar,
    ns: &'static str,
    name: String,
    files: Vec<String>,
    handle: Option<&'static crate::Sound>,
    desc: abi::SoundDesc,
    category_name: String,
    _p: PhantomData<&'a mut Registry>,
}

impl SoundBuilder<'_> {
    pub fn handle(mut self, h: &'static crate::Sound) -> Self {
        self.handle = Some(h);
        self
    }
    /// Variants picked at random: `sounds/<name>.ogg` or `.wav`, relative to this mod.
    pub fn files(mut self, names: &[&str]) -> Self {
        self.files = names.iter().map(|n| qualify(self.ns, n)).collect();
        self
    }
    /// Category for its volume setting: the engine's (`core:creatures` ...) or one registered
    /// with `Registry::sound_category` (any mod's).
    pub fn category(mut self, name: &str) -> Self {
        self.category_name = qualify(self.ns, name);
        self
    }
    pub fn volume(mut self, v: f32) -> Self {
        self.desc.volume = v;
        self
    }
    /// Playback speed picked between the two.
    pub fn pitch(mut self, min: f32, max: f32) -> Self {
        self.desc.pitch = [min, max];
        self
    }
    /// Blocks away it is still heard.
    pub fn range(mut self, blocks: f32) -> Self {
        self.desc.range = blocks;
        self
    }
}

impl Drop for SoundBuilder<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let files: Vec<Str> = self.files.iter().map(|f| Str::new(f)).collect();
        let desc = abi::SoundDesc { name: Str::new(&self.name), cell: self.handle.map_or(std::ptr::null(), |h| h.cell()), files: Slice::new(&files), category_name: Str::new(&self.category_name), ..self.desc };
        unsafe { (host().reg_sound)(self.raw, &desc) }
    }
}

pub struct MusicBuilder<'a> {
    raw: *mut abi::Registrar,
    ns: &'static str,
    name: String,
    tracks: Vec<String>,
    dimension: String,
    when: u32,
    _p: PhantomData<&'a mut Registry>,
}

impl MusicBuilder<'_> {
    /// Files streamed (`sounds/<name>.ogg|.wav`), relative to this mod.
    pub fn tracks(mut self, names: &[&str]) -> Self {
        self.tracks = names.iter().map(|n| qualify(self.ns, n)).collect();
        self
    }
    pub fn dimension(mut self, name: &str) -> Self {
        self.dimension = qualify(self.ns, name);
        self
    }
    /// `abi::music_when` flags.
    pub fn when(mut self, flags: u32) -> Self {
        self.when = flags;
        self
    }
}

impl Drop for MusicBuilder<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let tracks: Vec<Str> = self.tracks.iter().map(|f| Str::new(f)).collect();
        let desc = abi::MusicDesc { name: Str::new(&self.name), tracks: Slice::new(&tracks), dimension: Str::new(&self.dimension), when: self.when, ..abi::MusicDesc::DEFAULT };
        unsafe { (host().reg_music)(self.raw, &desc) }
    }
}

impl Registry {
    /// A sound event, see `SoundBuilder`; blocks and entities name groups of them with `.sounds()`.
    pub fn sound<'a>(&'a mut self, name: &str) -> SoundBuilder<'a> {
        SoundBuilder { raw: self.raw, ns: self.ns, name: qualify(self.ns, name), files: Vec::new(), handle: None, desc: abi::SoundDesc::DEFAULT, category_name: String::new(), _p: PhantomData }
    }

    /// A sound category with its own volume setting (`<ns>:volume_<name>`, in the mod's
    /// settings); `world`: heard through caves and water like block sounds.
    pub fn sound_category(&mut self, name: &str, world: bool) {
        let name = qualify(self.ns, name);
        unsafe { (host().reg_sound_category)(self.raw, Str::new(&name), world as u32) }
    }

    /// Background music, see `MusicBuilder`.
    pub fn music<'a>(&'a mut self, name: &str) -> MusicBuilder<'a> {
        MusicBuilder { raw: self.raw, ns: self.ns, name: qualify(self.ns, name), tracks: Vec::new(), dimension: String::new(), when: 0, _p: PhantomData }
    }
}

impl World {
    /// Sound event `sound` at `pos` for the players in its range.
    pub fn play_sound(&self, sound: &crate::Sound, pos: [f64; 3], volume: f32) {
        unsafe { (host().play_sound)(self.raw(), sound.id(), &pos, volume, 0.0) }
    }
}

impl Client {
    /// Only this client hears it; `None` = not positioned (UI sounds).
    pub fn play_sound(&self, sound: &crate::Sound, pos: Option<[f64; 3]>, volume: f32) {
        let p = pos.as_ref().map_or(std::ptr::null(), |p| p as *const [f64; 3]);
        unsafe { (host().client_play_sound)(self.raw(), sound.id(), p, volume, 0.0) }
    }
}
