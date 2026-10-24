//! Sound: events (`registry::SoundDef`) play one of their files, positioned in the world or not;
//! every category has its own volume (`core:volume_*` settings). World sounds go through one
//! track whose low-pass filter muffles them under water and whose send feeds a reverb in caves.
//! Music streams on its own track after quiet stretches. Without an audio device everything
//! here does nothing.

use std::time::{Duration, Instant};

use glam::{DVec3, Quat};
use kira::effect::filter::{FilterBuilder, FilterHandle};
use kira::effect::reverb::ReverbBuilder;
use kira::sound::static_sound::StaticSoundData;
use kira::sound::streaming::{StreamingSoundData, StreamingSoundHandle};
use kira::track::{SendTrackBuilder, SendTrackHandle, SpatialTrackBuilder, TrackBuilder, TrackHandle};
use kira::{AudioManager, AudioManagerSettings, Decibels, DefaultBackend, Mix, Tween};
use rustc_hash::FxHashMap;
use voxide_abi::sound_category as cat;

use crate::assets::Assets;
use crate::content::content;

/// A sound to play this frame: event id, position (`None` = not positioned), volume, pitch (0 =
/// the event's random range).
#[derive(Clone, Copy, Debug)]
pub struct Play {
    pub sound: u32,
    pub pos: Option<DVec3>,
    pub volume: f32,
    pub pitch: f32,
}

/// Where the listener is, for positions and the environment.
pub struct Listener {
    pub pos: DVec3,
    pub yaw: f32,
    pub pitch: f32,
    pub underwater: bool,
    /// No sky light at the head: caves echo.
    pub enclosed: bool,
    pub dim: u32,
    pub day: bool,
    pub creative: bool,
}

pub struct Audio {
    mgr: AudioManager<DefaultBackend>,
    listener: kira::listener::ListenerHandle,
    reverb: SendTrackHandle,
    world: TrackHandle,
    lowpass: FilterHandle,
    /// Per category (`sound_category`): its track.
    groups: Vec<TrackHandle>,
    files: FxHashMap<String, Option<StaticSoundData>>,
    music: Option<StreamingSoundHandle<kira::sound::FromFileError>>,
    next_music: Instant,
    state: (bool, bool),
    volumes: [f64; 7],
    rng: u64,
    /// Sounds started so far.
    pub played: u32,
}

const QUICK: Tween = Tween { start_time: kira::StartTime::Immediate, duration: Duration::from_millis(400), easing: kira::Easing::Linear };

fn db(volume: f64) -> Decibels {
    if volume <= 0.001 { Decibels::SILENCE } else { Decibels((20.0 * volume.log10()) as f32) }
}

impl Audio {
    pub fn new() -> Option<Audio> {
        let mut mgr = match AudioManager::<DefaultBackend>::new(AudioManagerSettings::default()) {
            Ok(m) => m,
            Err(e) => {
                log::warn!("no sound: {e}");
                return None;
            }
        };
        let made = (|| -> Result<Audio, kira::ResourceLimitReached> {
            let listener = mgr.add_listener(glam::Vec3::ZERO, Quat::IDENTITY)?;
            let reverb = mgr.add_send_track(SendTrackBuilder::new().with_effect(ReverbBuilder::new().mix(Mix::WET).damping(0.6).feedback(0.85)))?;
            let mut wb = TrackBuilder::new().sub_track_capacity(64).with_send(&reverb, Decibels::SILENCE);
            let lowpass = wb.add_effect(FilterBuilder::new().cutoff(20_000.0));
            let mut world = mgr.add_sub_track(wb)?;
            let mut groups = Vec::new();
            for c in 0..=cat::UI {
                let b = TrackBuilder::new().sub_track_capacity(256).sound_capacity(256);
                groups.push(if c == cat::MUSIC || c == cat::UI { mgr.add_sub_track(b)? } else { world.add_sub_track(b)? });
            }
            Ok(Audio {
                mgr,
                listener,
                reverb,
                world,
                lowpass,
                groups,
                files: FxHashMap::default(),
                music: None,
                next_music: Instant::now() + Duration::from_secs(20),
                state: (false, false),
                volumes: [-1.0; 7],
                rng: 0x2545_F491_4F6C_DD1D,
                played: 0,
            })
        })();
        match made {
            Ok(a) => Some(a),
            Err(e) => {
                log::warn!("no sound: {e:?}");
                None
            }
        }
    }

    fn random(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Decoded once, on first use: a file is a few kilobytes and decodes in well under a
    /// millisecond, so nothing waits for a sound on a screen or in the world.
    fn file(&mut self, assets: &Assets, name: &str) -> Option<StaticSoundData> {
        if let Some(d) = self.files.get(name) {
            return d.clone();
        }
        let path = assets.path(name, "sounds", "ogg").or_else(|| assets.path(name, "sounds", "wav"));
        let data = path.and_then(|p| match StaticSoundData::from_file(&p) {
            Ok(d) => Some(d),
            Err(e) => {
                log::warn!("sound {name}: {e}");
                None
            }
        });
        if data.is_none() {
            log::warn!("missing sound {name}");
        }
        self.files.insert(name.to_string(), data.clone());
        data
    }

    /// Settings, listener, environment and music, once a frame.
    pub fn update(&mut self, assets: &Assets, l: Option<&Listener>) {
        let vol = |name: &str| crate::config::by_name(name).map_or(1.0, |id| crate::config::get(id) / 100.0);
        let volumes = [
            vol("core:volume_blocks"),
            vol("core:volume_creatures"),
            vol("core:volume_players"),
            vol("core:volume_ambient"),
            vol("core:volume_music"),
            1.0,
            vol("core:volume_master"),
        ];
        if volumes != self.volumes {
            self.volumes = volumes;
            for (g, v) in self.groups.iter_mut().zip(volumes) {
                g.set_volume(db(v), Tween::default());
            }
            self.mgr.main_track().set_volume(db(volumes[6]), Tween::default());
        }
        let Some(l) = l else {
            if let Some(m) = &mut self.music {
                m.stop(Tween { duration: Duration::from_secs(2), ..Default::default() });
                self.music = None;
            }
            return;
        };
        let orientation = Quat::from_rotation_y(l.yaw) * Quat::from_rotation_x(l.pitch);
        self.listener.set_position(l.pos.as_vec3(), Tween::default());
        self.listener.set_orientation(orientation, Tween::default());
        if (l.underwater, l.enclosed) != self.state {
            self.state = (l.underwater, l.enclosed);
            self.lowpass.set_cutoff(if l.underwater { 900.0 } else { 20_000.0 }, QUICK);
            if let Err(e) = self.world.set_send(&self.reverb, if l.enclosed && !l.underwater { Decibels(-9.0) } else { Decibels::SILENCE }, QUICK) {
                log::debug!("cave reverb: {e:?}");
            }
        }
        self.music(assets, l);
    }

    fn music(&mut self, assets: &Assets, l: &Listener) {
        use voxide_abi::music_when as mw;
        if self.music.as_ref().is_some_and(|m| m.state() != kira::sound::PlaybackState::Stopped) || Instant::now() < self.next_music {
            return;
        }
        self.music = None;
        let c = content();
        let fits = |m: &crate::registry::MusicDef| {
            let w = m.when;
            m.dimension.is_none_or(|d| d == l.dim)
                && (w & (mw::DAY | mw::NIGHT) == 0 || (w & mw::DAY != 0 && l.day) || (w & mw::NIGHT != 0 && !l.day))
                && (w & (mw::UNDERGROUND | mw::SURFACE) == 0 || (w & mw::UNDERGROUND != 0 && l.enclosed) || (w & mw::SURFACE != 0 && !l.enclosed))
                && (w & mw::CREATIVE == 0 || l.creative)
        };
        // Quiet for a few minutes between tracks, like vanilla's (shorter: there is less music).
        self.next_music = Instant::now() + Duration::from_secs(120 + (self.random() * 180.0) as u64);
        let Some(m) = c.game.music.iter().find(|m| fits(m) && !m.tracks.is_empty()) else { return };
        let track = m.tracks[(self.random() * m.tracks.len() as f32) as usize % m.tracks.len()].clone();
        let Some(path) = assets.path(&track, "sounds", "ogg").or_else(|| assets.path(&track, "sounds", "wav")) else {
            log::warn!("missing music {track}");
            return;
        };
        match StreamingSoundData::from_file(&path) {
            Ok(data) => match self.groups[cat::MUSIC as usize].play(data.fade_in_tween(Tween { duration: Duration::from_secs(3), ..Default::default() })) {
                Ok(h) => {
                    log::debug!("music: {track}");
                    self.music = Some(h);
                }
                Err(e) => log::warn!("music {track}: {e}"),
            },
            Err(e) => log::warn!("music {track}: {e}"),
        }
    }

    /// Ends the current track now and lets the next start soon (tests, `/music`).
    pub fn skip_music(&mut self) {
        if let Some(m) = &mut self.music {
            m.stop(QUICK);
        }
        self.music = None;
        self.next_music = Instant::now();
    }

    pub fn music_playing(&self) -> bool {
        self.music.as_ref().is_some_and(|m| m.state() != kira::sound::PlaybackState::Stopped)
    }

    pub fn play(&mut self, assets: &Assets, p: Play) {
        let c = content();
        let Some(def) = c.game.sounds.get(p.sound as usize) else { return };
        if def.files.is_empty() {
            return;
        }
        let file = def.files[(self.random() * def.files.len() as f32) as usize % def.files.len()].clone();
        let Some(data) = self.file(assets, &file) else { return };
        let pitch = if p.pitch > 0.0 { p.pitch } else { def.pitch[0] + (def.pitch[1] - def.pitch[0]) * self.random() };
        let volume = (p.volume * def.volume).max(0.0);
        let data = data.volume(db(volume as f64)).playback_rate(pitch as f64);
        let g = (def.category as usize).min(self.groups.len() - 1);
        let group = &mut self.groups[g];
        let played = match p.pos {
            Some(pos) => {
                // Heard up to `range` blocks (further for loud ones), full volume within one.
                let reach = def.range * volume.max(1.0);
                let b = SpatialTrackBuilder::new().distances((1.0, reach.max(1.5))).persist_until_sounds_finish(true);
                group.add_spatial_sub_track(&self.listener, pos.as_vec3(), b).map_err(|e| format!("{e:?}")).and_then(|mut t| t.play(data).map(|_| ()).map_err(|e| e.to_string()))
            }
            None => group.play(data).map(|_| ()).map_err(|e| e.to_string()),
        };
        match played {
            Ok(()) => self.played += 1,
            Err(e) => log::debug!("sound {}: {e}", def.name),
        }
    }
}
