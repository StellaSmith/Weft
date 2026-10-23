use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use glam::{DVec3, Vec2};
use voxide_render::{Backend, BackendDesc, Cmd, CommandList, Format, Target, TextureDesc, TextureId};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use crate::assets::Assets;
use crate::client::Client;
use crate::client::textures::GameTextures;
use crate::content::content;
use crate::hud::Hud;
use crate::input::{Input, Key};
use crate::modloader::{self, LoadedMod};
use crate::net;
use crate::saves::{self, WorldMeta};
use crate::screens::{self, BuildCx, Screen};
use crate::server::{self, ServerConfig, ServerHandle};
use crate::settings::Settings;
use crate::ui::lang::Lang;
use crate::ui::render::UiRenderer;
use crate::ui::text::{self, Arg};
use crate::ui::theme::Theme;
use crate::ui::{Kind, Ui};

pub type BackendFactory = fn(&BackendDesc) -> Result<Box<dyn Backend>, voxide_render::Error>;

#[derive(Clone, Debug)]
pub struct Options {
    pub run_dir: PathBuf,
    pub mod_dirs: Vec<PathBuf>,
    pub gpu_debug: bool,
    /// Automation: save a screenshot here once the world finished loading, then quit.
    pub shot: Option<PathBuf>,
    pub shot_delay: f32,
    /// Automation: fixed camera (x, y, z, yaw degrees, pitch degrees).
    pub camera: Option<[f64; 5]>,
    /// Automation: skip the menu and play this world (created if missing).
    pub world: Option<String>,
    pub seed: Option<u64>,
    /// Automation: open this built-in screen.
    pub screen: Option<String>,
    pub size: (u32, u32),
    pub dev: bool,
    /// Automation: scripted input, see `autopilot.rs`.
    pub script: Option<PathBuf>,
    /// Benchmarks: force vsync off / a view distance without touching saved settings.
    pub no_vsync: bool,
    pub view: Option<i32>,
}

pub enum Request {
    Open(Box<dyn Screen>),
    Close,
    Quit,
    Rebuild,
    ApplySettings,
    SetLanguage(String),
    WaitKey(String),
    Rebind(String, Key),
    ResetBinds,
    SelectWorld(usize),
    StartWorld(WorldMeta),
    CreateWorldFromForm,
    LeaveWorld,
    Host,
    Join(String),
    Respawn,
    SendChat(String),
}

/// What UI actions and bindings can see and change.
pub struct Ctx<'a> {
    pub settings: &'a mut Settings,
    pub client: Option<&'a mut Client>,
    pub input: &'a Input,
    pub requests: &'a mut Vec<Request>,
    pub fps: f32,
    pub frame_ms: f32,
    pub server_ms: f32,
    pub regions: u32,
    pub debug: bool,
}

pub struct Game {
    pub client: Client,
    pub server: Option<ServerHandle>,
}

struct Running {
    gpu: Box<dyn Backend>,
    window: Arc<Window>,
    depth: (TextureId, u32, u32),
    list: CommandList,
    input: Input,
    settings: Settings,
    lang: Lang,
    languages: Vec<(String, String)>,
    theme: Theme,
    assets: Assets,
    textures: GameTextures,
    ui: UiRenderer,
    screens: Vec<(Box<dyn Screen>, Ui)>,
    hud: Hud,
    game: Option<Game>,
    last: Instant,
    started: Instant,
    shot_pending: bool,
    fps: f32,
    frame_ms: f32,
    fps_timer: Instant,
    fps_frames: u32,
    quads_dirty: bool,
    waiting_key: Option<String>,
    debug: bool,
    quit: bool,
    mods: Vec<LoadedMod>,
    client_hash: u64,
    /// CPU milliseconds of the last frame, see `bench::CPU_PARTS`.
    prof: [f32; 5],
    bench: Option<crate::bench::Bench>,
    /// Where UI asked for the player model, physical pixels.
    previews: Vec<[f32; 4]>,
    /// Texture holding the rendered inventory figure, drawn by the UI.
    preview_tex: Option<u32>,
    autopilot: Option<crate::autopilot::Autopilot>,
    shot_path: Option<PathBuf>,
    ticket_file: Option<PathBuf>,
}

struct App {
    factory: BackendFactory,
    opts: Options,
    run: Option<Running>,
    error: Option<String>,
}

pub fn run(factory: BackendFactory, opts: Options) -> Result<(), String> {
    let event_loop = EventLoop::new().map_err(|e| e.to_string())?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App { factory, opts, run: None, error: None };
    event_loop.run_app(&mut app).map_err(|e| e.to_string())?;
    match app.error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

fn engine_assets() -> PathBuf {
    let dev = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    if dev.exists() {
        return dev;
    }
    std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("assets").join("core"))).unwrap_or(dev)
}

impl App {
    fn start(&mut self, el: &ActiveEventLoop) -> Result<Running, String> {
        let mut settings = Settings::load(&self.opts.run_dir);
        if self.opts.no_vsync {
            settings.vsync = false;
        }
        if let Some(v) = self.opts.view {
            settings.view_distance = v;
        }
        let attrs = Window::default_attributes()
            .with_title("Voxide")
            .with_inner_size(winit::dpi::PhysicalSize::new(self.opts.size.0, self.opts.size.1));
        let window = Arc::new(el.create_window(attrs).map_err(|e| e.to_string())?);
        let size = window.inner_size();
        let mut gpu = (self.factory)(&BackendDesc {
            window: &*window,
            width: size.width,
            height: size.height,
            vsync: settings.vsync,
            debug: self.opts.gpu_debug,
        })
        .map_err(|e| e.to_string())?;

        let cache = self.opts.run_dir.join("cache");
        let mods = modloader::load(&self.opts.mod_dirs, &cache)?;
        let mut roots: Vec<(String, PathBuf)> = vec![("core".into(), engine_assets())];
        roots.extend(mods.iter().map(|m| (m.id.clone(), m.assets.clone())));
        let packs = std::fs::read_dir(self.opts.run_dir.join("resourcepacks"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        let assets = Assets::new(roots, packs);
        let lang = Lang::load(&assets, &settings.language, self.opts.dev);
        let theme = Theme::load(&assets);
        let textures = GameTextures::new(&mut *gpu, content(), &assets);
        let ui = UiRenderer::new(&mut *gpu, &assets)?;
        let mut input = Input::default();
        settings.apply_binds(&mut input);
        let depth = make_depth(&mut *gpu, size.width, size.height);
        let mut r = Running {
            gpu,
            window,
            depth,
            list: CommandList::default(),
            input,
            settings,
            lang,
            languages: Vec::new(),
            theme,
            assets,
            textures,
            ui,
            screens: Vec::new(),
            hud: Hud::default(),
            game: None,
            last: Instant::now(),
            started: Instant::now(),
            shot_pending: false,
            fps: 0.0,
            frame_ms: 0.0,
            fps_timer: Instant::now(),
            fps_frames: 0,
            quads_dirty: true,
            waiting_key: None,
            debug: false,
            quit: false,
            mods,
            client_hash: 0,
            prof: [0.0; 5],
            bench: None,
            previews: Vec::new(),
            preview_tex: None,
            autopilot: self.opts.script.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).map(|s| crate::autopilot::Autopilot::load(&s)),
            shot_path: None,
            ticket_file: None,
        };
        if let Some(world) = &self.opts.world {
            let saves_dir = r.settings.saves_dir();
            let meta = saves::list(&saves_dir)
                .into_iter()
                .find(|w| &w.name == world)
                .unwrap_or_else(|| saves::create(&saves_dir, world, self.opts.seed.unwrap_or(1)));
            r.start_world(meta);
        } else {
            r.open(Box::new(screens::Title));
            match self.opts.screen.as_deref() {
                Some("settings") => r.open(Box::new(screens::SettingsScreen)),
                Some("language") => r.open(Box::new(screens::LanguageScreen)),
                Some("controls") => r.open(Box::new(screens::Controls { waiting: None })),
                Some("worlds") => r.open(Box::new(saves::WorldSelect::default())),
                Some("create") => r.open(Box::new(saves::CreateWorld)),
                _ => {}
            }
        }
        Ok(r)
    }
}

fn make_depth(gpu: &mut dyn Backend, w: u32, h: u32) -> (TextureId, u32, u32) {
    let t = gpu.create_texture(
        &TextureDesc { width: w.max(1), height: h.max(1), layers: None, mips: 1, format: Format::Depth32, render_target: true },
        "depth",
    );
    (t, w, h)
}

impl Running {
    fn gui_scale(&self) -> f32 {
        let (w, h) = self.gpu.surface_size();
        let max = (w / 320).min(h / 240).max(1);
        let s = if self.settings.gui_scale == 0 { max } else { self.settings.gui_scale.min(max) };
        s as f32
    }

    fn build_cx(&self) -> BuildCx<'_> {
        BuildCx {
            lang: &self.lang,
            input: &self.input,
            settings: &self.settings,
            languages: &self.languages,
            in_game: self.game.is_some(),
            mods: self.mods.len(),
            client: self.game.as_ref().map(|g| &g.client),
        }
    }

    fn open(&mut self, mut s: Box<dyn Screen>) {
        if s.name() == "core:language" && self.languages.is_empty() {
            self.languages = Lang::available(&self.assets);
        }
        let mut ui = Ui::new();
        s.build(&mut ui, &self.build_cx());
        self.screens.push((s, ui));
        self.quads_dirty = true;
        self.set_capture(false);
    }

    fn rebuild_all(&mut self) {
        let mut screens = std::mem::take(&mut self.screens);
        for (s, ui) in &mut screens {
            let (hover, focus) = (ui.hovered, ui.focused);
            let inputs: Vec<(String, String)> = ui
                .nodes
                .iter()
                .filter_map(|n| match (&n.id, &n.kind) {
                    (Some(id), Kind::Input { value, .. }) => Some((id.clone(), value.clone())),
                    _ => None,
                })
                .collect();
            ui.clear();
            s.build(ui, &self.build_cx());
            // Typed text survives rebuilds (language switch, resize).
            for (id, v) in inputs {
                if let Some(n) = ui.find(&id) {
                    if let Kind::Input { value, .. } = &mut ui.nodes[n].kind {
                        *value = v;
                    }
                }
            }
            ui.hovered = hover.filter(|&h| h < ui.nodes.len());
            ui.focused = focus.filter(|&h| h < ui.nodes.len());
        }
        self.screens = screens;
        self.hud.invalidate();
        self.quads_dirty = true;
    }

    fn set_capture(&mut self, on: bool) {
        let on = on && self.screens.is_empty() && self.game.is_some();
        if on {
            let _ = self.window.set_cursor_grab(CursorGrabMode::Locked).or_else(|_| self.window.set_cursor_grab(CursorGrabMode::Confined));
            self.window.set_cursor_visible(false);
        } else {
            let _ = self.window.set_cursor_grab(CursorGrabMode::None);
            self.window.set_cursor_visible(true);
        }
        self.input.captured = on;
    }

    fn start_world(&mut self, meta: WorldMeta) {
        log::info!("opening world '{}' (seed {})", meta.name, meta.seed);
        let storage = saves::Storage::open(meta.clone(), content());
        let server = server::spawn(
            ServerConfig { seed: meta.seed, view_radius: self.settings.view_distance, view_height: 6 },
            Some(storage),
        );
        let (client_conn, server_conn) = net::local_pair();
        let _ = server.connect.send(server_conn);
        let key = net::p2p::player_key(&self.settings.run_dir);
        match Client::new(&mut *self.gpu, &self.textures, client_conn, &self.settings.name, &key, self.settings.view_distance) {
            Ok(client) => {
                self.game = Some(Game { client, server: Some(server) });
                self.screens.clear();
                self.hud.invalidate();
                self.quads_dirty = true;
                self.set_capture(true);
            }
            Err(e) => log::error!("{e}"),
        }
    }

    fn leave_world(&mut self) {
        self.leave_world_silent();
        self.screens.clear();
        self.open(Box::new(screens::Title));
    }

    fn leave_world_silent(&mut self) {
        if let Some(mut g) = self.game.take() {
            g.client.leave();
            self.gpu.wait_idle();
            g.client.destroy(&mut *self.gpu);
            if let Some(mut s) = g.server.take() {
                s.stop();
                net::p2p::stop_hosting();
            }
        }
        self.set_capture(false);
    }

    fn handle(&mut self, reqs: Vec<Request>) {
        for r in reqs {
            match r {
                Request::Open(s) => self.open(s),
                Request::Close => {
                    self.screens.pop();
                    self.quads_dirty = true;
                    if self.screens.is_empty() {
                        self.set_capture(true);
                    } else {
                        self.rebuild_all();
                    }
                }
                Request::Quit => self.quit = true,
                Request::Rebuild => self.rebuild_all(),
                Request::ApplySettings => {
                    self.gpu.set_vsync(self.settings.vsync);
                    if let Some(g) = &mut self.game {
                        g.client.set_view_distance(self.settings.view_distance);
                        if let Some(s) = &g.server {
                            s.set_view(self.settings.view_distance);
                        }
                    }
                }
                Request::SetLanguage(code) => {
                    self.settings.language = code.clone();
                    self.lang = Lang::load(&self.assets, &code, self.lang.dev);
                    self.rebuild_all();
                }
                Request::WaitKey(a) => {
                    self.waiting_key = Some(a.clone());
                    if let Some((s, _)) = self.screens.last_mut() {
                        *s = Box::new(screens::Controls { waiting: Some(a) });
                    }
                    self.rebuild_all();
                }
                Request::Rebind(action, key) => {
                    self.input.rebind(&action, key);
                    self.settings.set_bind(&action, key);
                    self.waiting_key = None;
                    if let Some((s, _)) = self.screens.last_mut() {
                        *s = Box::new(screens::Controls { waiting: None });
                    }
                }
                Request::ResetBinds => {
                    let defaults: Vec<(String, Key)> = self.input.binds.iter().map(|b| (b.action.clone(), b.default)).collect();
                    for (a, k) in defaults {
                        self.input.rebind(&a, k);
                    }
                    self.settings.binds.clear();
                    self.rebuild_all();
                }
                Request::SelectWorld(i) => {
                    if let Some((s, _)) = self.screens.last_mut() {
                        s.select(i);
                    }
                    self.rebuild_all();
                }
                Request::StartWorld(meta) => self.start_world(meta),
                Request::CreateWorldFromForm => {
                    let Some((_, ui)) = self.screens.last() else { continue };
                    let field = |id: &str| {
                        ui.find(id).and_then(|n| match &ui.nodes[n].kind {
                            Kind::Input { value, .. } => Some(value.clone()),
                            _ => None,
                        })
                    };
                    let name = field("name").unwrap_or_default();
                    let seed = saves::parse_seed(&field("seed").unwrap_or_default());
                    let name = if name.trim().is_empty() { "World".to_string() } else { name.trim().to_string() };
                    let meta = saves::create(&self.settings.saves_dir(), &name, seed);
                    self.start_world(meta);
                }
                Request::LeaveWorld => self.leave_world(),
                Request::Host => {
                    if let Some(Game { server: Some(s), .. }) = &self.game {
                        net::host(s, &self.settings);
                    }
                    self.rebuild_all();
                }
                Request::Respawn => {
                    if let Some(g) = &mut self.game {
                        g.client.send(net::ToServer::Respawn);
                    }
                    self.screens.retain(|(s, _)| s.name() != "core:death");
                    self.set_capture(true);
                }
                Request::SendChat(text) => {
                    if let Some(g) = &mut self.game {
                        if !text.trim().is_empty() {
                            g.client.send(net::ToServer::Chat(text));
                        }
                    }
                }
                Request::Join(ticket) => match {
                    let typed = self.screens.last().and_then(|(_, ui)| {
                        ui.find("ticket").and_then(|n| match &ui.nodes[n].kind {
                            Kind::Input { value, .. } => Some(value.clone()),
                            _ => None,
                        })
                    });
                    let ticket = if ticket.is_empty() { typed.unwrap_or_default() } else { ticket };
                    net::join(&ticket, &self.settings)
                } {
                    Ok(conn) => {
                        self.leave_world_silent();
                        let key = net::p2p::player_key(&self.settings.run_dir);
                        match Client::new(&mut *self.gpu, &self.textures, conn, &self.settings.name, &key, self.settings.view_distance) {
                            Ok(client) => {
                                self.game = Some(Game { client, server: None });
                                self.screens.clear();
                                self.hud.invalidate();
                                self.set_capture(true);
                            }
                            Err(e) => log::error!("{e}"),
                        }
                    }
                    Err(e) => self.open(Box::new(screens::Message { title: "multiplayer.disconnected".into(), args: vec![Arg::Text(e)] })),
                },
            }
        }
    }

    /// Mouse and keyboard for the top screen.
    fn screen_input(&mut self, scale: f32, requests: &mut Vec<Request>) {
        let cursor = self.input.cursor / scale;
        let Some((_, ui)) = self.screens.last_mut() else { return };
        let hovered = ui.hit(cursor);
        if hovered != ui.hovered {
            ui.hovered = hovered;
            self.quads_dirty = true;
        }
        let mut actions: Vec<crate::ui::Action> = Vec::new();
        let mut slider: Option<(std::rc::Rc<dyn Fn(&mut Ctx, f32)>, f32)> = None;
        let mut text_changed: Option<(std::rc::Rc<dyn Fn(&mut Ctx, &str)>, String)> = None;
        let menu_screen = self.screens.last().is_some_and(|(s, _)| s.name() == "core:menu");
        if menu_screen {
            let Some((_, ui)) = self.screens.last_mut() else { return };
            let slot = hovered.and_then(|h| match ui.nodes[h].kind {
                Kind::Custom { tag: crate::client::modui::TAG_SLOT, data } => Some(data as u32),
                _ => None,
            });
            if let Some(g) = &mut self.game {
                let shift = self.input.held_key(Key::Code(KeyCode::ShiftLeft)) || self.input.held_key(Key::Code(KeyCode::ShiftRight));
                for button in [0u8, 1] {
                    if self.input.pressed_button(button) {
                        let inside = ui.nodes.iter().any(|n| matches!(n.kind, Kind::Container { class: Some(_) }) && n.rect.contains(cursor) && n.rect.w < ui.size.x);
                        match slot {
                            Some(s) => g.client.send(net::ToServer::Click { slot: s, button, shift }),
                            None if !inside => g.client.send(net::ToServer::Click { slot: u32::MAX, button, shift: false }),
                            None => {}
                        }
                    }
                }
                // Tooltip with the hovered item's name.
                if let Some(h) = hovered {
                    if let Kind::Custom { data, .. } = ui.nodes[h].kind {
                        let stack = g.client.menu.as_ref().and_then(|m| m.slots.get(data as usize)).copied().unwrap_or_default();
                        let tip = (!stack.is_empty()).then(|| text::plain(&crate::hud::item_name(&self.lang, stack.item)));
                        if ui.nodes[h].tooltip != tip {
                            ui.nodes[h].tooltip = tip;
                            self.quads_dirty = true;
                        }
                    }
                }
            }
        }
        let Some((_, ui)) = self.screens.last_mut() else { return };
        if self.input.pressed_button(0) {
            ui.pressed = hovered;
            ui.focused = hovered.filter(|&h| matches!(ui.nodes[h].kind, Kind::Input { .. }));
            if let Some(h) = hovered {
                if let Kind::Text(t) = &ui.nodes[h].kind {
                    let r = ui.nodes[h].rect;
                    let unit = ui.text_unit * t.scale;
                    let p = (cursor - Vec2::new(r.x, r.y)) / unit;
                    let target = t.layout.links.iter().find(|(_, b)| p.x >= b[0] && p.x <= b[2] && p.y >= b[1] && p.y <= b[3]).map(|l| l.0.clone());
                    if let Some(n) = target.and_then(|t| ui.find(&t)) {
                        ui.focused = Some(n);
                    }
                }
            }
            self.quads_dirty = true;
        }
        if self.input.held_button(0) {
            if let Some(p) = ui.pressed {
                let r = ui.nodes[p].rect;
                if let Kind::Slider { value, on_change, .. } = &mut ui.nodes[p].kind {
                    let v = ((cursor.x - r.x) / r.w).clamp(0.0, 1.0);
                    if (v - *value).abs() > 1e-4 {
                        *value = v;
                        slider = Some((on_change.clone(), v));
                        self.quads_dirty = true;
                    }
                }
            }
        } else if let Some(p) = ui.pressed.take() {
            if Some(p) == hovered {
                if let Kind::Button { action: Some(a), enabled: true, .. } = &ui.nodes[p].kind {
                    actions.push(a.clone());
                }
            }
            self.quads_dirty = true;
        }
        if let Some(f) = ui.focused {
            if let Kind::Input { value, max, on_change } = &mut ui.nodes[f].kind {
                let mut changed = false;
                let ctrl = self.input.held_key(Key::Code(KeyCode::ControlLeft)) || self.input.held_key(Key::Code(KeyCode::ControlRight));
                if ctrl && self.input.pressed_key(Key::Code(KeyCode::KeyV)) {
                    if let Some(clip) = crate::clipboard::get() {
                        value.extend(clip.trim().chars().take(max.saturating_sub(value.chars().count())));
                        changed = true;
                    }
                } else {
                    for ch in self.input.typed.chars() {
                        if !ch.is_control() && value.chars().count() < *max {
                            value.push(ch);
                            changed = true;
                        }
                    }
                }
                if self.input.pressed_key(Key::Code(KeyCode::Backspace)) {
                    value.pop();
                    changed = true;
                }
                if changed {
                    ui.dirty = true;
                    self.quads_dirty = true;
                    if let Some(cb) = on_change {
                        text_changed = Some((cb.clone(), value.clone()));
                    }
                }
            }
        }
        let mut ctx = Ctx {
            settings: &mut self.settings,
            client: self.game.as_mut().map(|g| &mut g.client),
            input: &self.input,
            requests,
            fps: self.fps,
            frame_ms: self.frame_ms,
            server_ms: 0.0,
            regions: 0,
            debug: self.debug,
        };
        for a in actions {
            a(&mut ctx);
        }
        if let Some((f, v)) = slider {
            f(&mut ctx, v);
        }
        if let Some((f, v)) = text_changed {
            f(&mut ctx, &v);
        }
    }

    /// Re-evaluates bound values; layout and geometry only change when a value did.
    fn poll_bindings(&mut self) {
        let mut reqs = Vec::new();
        let server = self.game.as_ref().and_then(|g| g.server.as_ref());
        let (server_ms, regions) = server.map_or((0.0, 0), |s| (s.tick_ms(), s.regions()));
        let ctx = Ctx {
            settings: &mut self.settings,
            client: self.game.as_mut().map(|g| &mut g.client),
            input: &self.input,
            requests: &mut reqs,
            fps: self.fps,
            frame_ms: self.frame_ms,
            server_ms,
            regions,
            debug: self.debug,
        };
        let items = |name: &str| crate::content::try_content().and_then(|c| c.item_names.get(name).copied());
        let tcx = text::Ctx { lang: &self.lang, input: Some(ctx.input), item_name: &items };
        let mut uis: Vec<&mut Ui> = self.screens.iter_mut().map(|(_, u)| u).collect();
        uis.push(&mut self.hud.ui);
        for ui in uis {
            let mut relayout = false;
            let mut redraw = false;
            for n in &mut ui.nodes {
                match &mut n.kind {
                    Kind::Text(t) => {
                        if let Some(b) = &t.bind {
                            let v = b(&ctx);
                            if t.last.as_ref() != Some(&v) {
                                t.spans.clear();
                                text::format(tcx.lang.get(&v.0), &v.1, &tcx, &mut t.spans);
                                t.last = Some(v);
                                relayout = true;
                            }
                        }
                    }
                    Kind::Progress { bind, value, .. } | Kind::Icons { bind, value, .. } => {
                        let v = bind(&ctx);
                        if (v - *value).abs() > 1e-4 {
                            *value = v;
                            redraw = true;
                        }
                    }
                    Kind::Slider { bind, value, .. } => {
                        let v = bind(&ctx);
                        if v.is_finite() && (v - *value).abs() > 1e-4 {
                            *value = v;
                            redraw = true;
                        }
                    }
                    Kind::Item { bind, last } => {
                        let v = bind(&ctx);
                        if v != *last {
                            *last = v;
                            redraw = true;
                        }
                    }
                    _ => {}
                }
            }
            if relayout {
                ui.dirty = true;
            }
            if relayout || redraw {
                self.quads_dirty = true;
            }
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.run.is_some() {
            return;
        }
        match self.start(el) {
            Ok(r) => self.run = Some(r),
            Err(e) => {
                self.error = Some(e);
                el.exit();
            }
        }
    }

    fn device_event(&mut self, _el: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let (Some(r), DeviceEvent::MouseMotion { delta }) = (&mut self.run, event) {
            if r.input.captured {
                r.input.mouse_delta += Vec2::new(delta.0 as f32, delta.1 as f32);
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(r) = &mut self.run else { return };
        match event {
            WindowEvent::CloseRequested => r.quit = true,
            WindowEvent::Resized(size) => {
                r.gpu.resize(size.width, size.height);
                r.rebuild_all();
            }
            WindowEvent::Focused(false) if self.opts.script.is_none() => {
                r.input.release_all();
                if r.game.is_some() && r.screens.is_empty() && self.opts.shot.is_none() {
                    r.open(Box::new(screens::Pause));
                }
                r.set_capture(false);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    match event.state {
                        ElementState::Pressed => r.input.key_down(Key::Code(code)),
                        ElementState::Released => r.input.key_up(Key::Code(code)),
                    }
                }
                if event.state == ElementState::Pressed {
                    if let Some(t) = &event.text {
                        r.input.typed.push_str(t);
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let b = match button {
                    MouseButton::Left => 0,
                    MouseButton::Right => 1,
                    MouseButton::Middle => 2,
                    MouseButton::Back => 3,
                    MouseButton::Forward => 4,
                    MouseButton::Other(n) => (5 + n).min(255) as u8,
                };
                if state == ElementState::Pressed {
                    if !r.input.captured && r.screens.is_empty() && r.game.is_some() && b == 0 {
                        r.set_capture(true);
                        return;
                    }
                    r.input.key_down(Key::Mouse(b));
                } else {
                    r.input.key_up(Key::Mouse(b));
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                r.input.scroll += match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => (p.y / 40.0) as f32,
                };
            }
            WindowEvent::CursorMoved { position, .. } => {
                r.input.cursor = Vec2::new(position.x as f32, position.y as f32);
            }
            WindowEvent::RedrawRequested => {
                if frame(r, &self.opts) || r.quit {
                    r.leave_world_silent();
                    r.settings.save();
                    el.exit();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _el: &ActiveEventLoop) {
        if let Some(r) = &self.run {
            r.window.request_redraw();
        }
    }

    fn exiting(&mut self, _el: &ActiveEventLoop) {
        if let Some(mut r) = self.run.take() {
            r.gpu.wait_idle();
            r.leave_world_silent();
            drop(r.gpu);
        }
    }
}

/// One frame. Returns true when the app should quit.
fn frame(r: &mut Running, opts: &Options) -> bool {
    let now = Instant::now();
    let dt = (now - r.last).as_secs_f64();
    r.last = now;
    let scale = r.gui_scale();

    if let Some(ap) = r.autopilot.as_mut().filter(|_| r.bench.is_none()) {
        let loaded = r.game.as_ref().is_some_and(|g| g.client.joined && g.client.world.cubes.len() > 50 && g.client.world.pending_meshes() == 0);
        let idle = loaded && r.game.as_ref().is_some_and(|g| g.server.as_ref().is_none_or(|s| s.idle()) && g.client.world.pending_meshes() == 0);
        match ap.frame(&mut r.input, r.gpu.surface_size(), loaded, idle) {
            crate::autopilot::Action::Look(yaw, pitch) => {
                if let Some(g) = &mut r.game {
                    g.client.player.yaw = yaw.to_radians();
                    g.client.player.pitch = pitch.to_radians();
                }
            }
            crate::autopilot::Action::Chat(t) => {
                if let Some(g) = &mut r.game {
                    g.client.send(net::ToServer::Chat(t));
                }
            }
            crate::autopilot::Action::Shot(name) => {
                let dir = opts.shot.clone().unwrap_or_else(|| r.settings.run_dir.join("screenshots"));
                r.shot_path = Some(if dir.extension().is_some() { dir.with_file_name(name) } else { dir.join(name) });
                r.gpu.request_capture();
                r.shot_pending = true;
            }
            crate::autopilot::Action::Host(file) => {
                r.handle(vec![Request::Host]);
                r.ticket_file = Some(PathBuf::from(file));
            }
            crate::autopilot::Action::Join(ticket) => r.handle(vec![Request::Join(ticket)]),
            crate::autopilot::Action::Bench(secs, label) => r.bench = Some(crate::bench::Bench::new(&label, secs)),
            crate::autopilot::Action::Idle => {
                if let Some(g) = r.game.as_ref().filter(|_| idle) {
                    if let Some((made, gen_us, ins, ins_us)) = g.server.as_ref().map(|s| s.load_stats()) {
                        log::info!(
                            "load: {} cubes on client, generated {made} ({gen_us:.0} us each on a worker), inserted {ins} ({ins_us:.0} us each on the server thread)",
                            g.client.world.cubes.len()
                        );
                    }
                }
            }
            crate::autopilot::Action::Quit => {
                if let Some(b) = r.bench.take() {
                    log::info!("{}", b.report(""));
                }
                return true;
            }
            crate::autopilot::Action::None => {}
        }
        if r.screens.is_empty() && r.game.is_some() {
            r.input.captured = true;
        }
    }
    if let Some(f) = &r.ticket_file {
        if let Some(t) = net::current_ticket() {
            let _ = std::fs::write(f, t);
            r.ticket_file = None;
        }
    }
    let mut requests = Vec::new();
    if r.waiting_key.is_some() {
        for k in r.input.pressed_list() {
            let mut ctx = Ctx {
                settings: &mut r.settings,
                client: None,
                input: &r.input,
                requests: &mut requests,
                fps: r.fps,
                frame_ms: r.frame_ms,
                    server_ms: 0.0,
                regions: 0,
                debug: r.debug,
            };
            if let Some((s, _)) = r.screens.last_mut() {
                if s.key(k, &mut ctx) {
                    r.waiting_key = None;
                    break;
                }
            }
        }
    } else if r.input.pressed_key(Key::Code(KeyCode::Escape)) {
        if r.screens.last().is_some_and(|(s, _)| s.name() == "core:death") {
        } else if r.screens.len() > 1 || (r.game.is_some() && !r.screens.is_empty()) {
            if r.screens.last().is_some_and(|(s, _)| s.name() == "core:menu") {
                if let Some(g) = &mut r.game {
                    g.client.close_menu();
                }
            }
            requests.push(Request::Close);
        } else if r.game.is_some() && r.screens.is_empty() {
            r.open(Box::new(screens::Pause));
        }
    }
    if r.input.pressed("core:debug") {
        r.debug = !r.debug;
        r.hud.invalidate();
    }
    if r.input.pressed("core:reload") {
        r.assets.rescan();
        r.ui.reload(&mut *r.gpu);
        r.theme = Theme::load(&r.assets);
        r.lang = Lang::load(&r.assets, &r.settings.language, r.lang.dev);
        if let Some(g) = &mut r.game {
            g.client.render.reload_shaders(&mut *r.gpu);
            g.client.entity_render.reload(&mut *r.gpu);
        }
        r.rebuild_all();
        log::info!("assets reloaded");
    }
    if r.input.pressed("core:screenshot") {
        r.gpu.request_capture();
        r.shot_pending = true;
    }
    r.screen_input(scale, &mut requests);
    r.handle(requests);

    let paused = r.screens.iter().any(|(s, _)| s.pauses());
    let world_input = r.screens.is_empty();
    if let Some(g) = &mut r.game {
        if let Some(s) = &g.server {
            s.set_paused(paused && !s.is_hosting());
        }
        g.client.fov = r.settings.fov;
        let t = Instant::now();
        if world_input {
            g.client.update(&mut *r.gpu, &r.input, dt);
        } else {
            g.client.update(&mut *r.gpu, &Input::default(), dt);
        }
        r.prof[0] = t.elapsed().as_secs_f32() * 1000.0;
        r.prof[1] = Instant::now().duration_since(now).as_secs_f32() * 1000.0;
        if let Some(cam) = opts.camera {
            g.client.player.pos = DVec3::new(cam[0], cam[1] - 1.62, cam[2]);
            g.client.player.yaw = (cam[3] as f32).to_radians();
            g.client.player.pitch = (cam[4] as f32).to_radians();
            g.client.player.flying = true;
            g.client.player.vel = DVec3::ZERO;
        }
    }
    let events = r.game.as_mut().map(|g| std::mem::take(&mut g.client.events)).unwrap_or_default();
    for ev in events {
        match ev {
            crate::client::ClientEvent::OpenMenu => {
                let kind = r.game.as_ref().and_then(|g| g.client.menu.as_ref()).map_or(0, |m| m.kind);
                r.screens.retain(|(s, _)| s.name() != "core:menu");
                r.open(Box::new(crate::game_screens::MenuScreen { kind }));
            }
            crate::client::ClientEvent::CloseMenu => {
                r.screens.retain(|(s, _)| s.name() != "core:menu");
                r.quads_dirty = true;
                r.set_capture(true);
            }
            crate::client::ClientEvent::Died => {
                r.screens.clear();
                r.open(Box::new(crate::game_screens::Death));
            }
        }
    }
    if r.game.is_some() && r.screens.is_empty() && r.input.pressed("core:chat") {
        r.open(Box::new(crate::game_screens::Chat));
        r.input.typed.clear();
    }
    let top = r.screens.last().map(|(s, _)| s.name().to_string());
    if top.as_deref() == Some("core:chat") && r.input.pressed_key(Key::Code(KeyCode::Enter)) {
        let text = r.screens.last().and_then(|(_, ui)| {
            ui.find("chat").and_then(|n| match &ui.nodes[n].kind {
                Kind::Input { value, .. } => Some(value.clone()),
                _ => None,
            })
        });
        r.screens.pop();
        r.set_capture(true);
        r.quads_dirty = true;
        if let Some(t) = text {
            r.handle(vec![Request::SendChat(t)]);
        }
    }
    if top.as_deref() == Some("core:menu") && r.input.pressed("core:inventory") {
        if let Some(g) = &mut r.game {
            g.client.close_menu();
        }
        r.screens.retain(|(s, _)| s.name() != "core:menu");
        r.set_capture(true);
        r.quads_dirty = true;
    }
    if let Some(reason) = r.game.as_ref().and_then(|g| g.client.disconnected.clone()) {
        log::info!("disconnected: {reason}");
        r.leave_world_silent();
        r.screens.clear();
        r.open(Box::new(screens::Title));
        r.open(Box::new(screens::Message { title: "multiplayer.disconnected".into(), args: vec![Arg::Text(reason)] }));
    }
    r.input.sensitivity = r.settings.sensitivity;
    r.input.end_frame();

    if let Some(g) = &r.game {
        if r.hud.update(&g.client, r.debug, &r.lang) {
            r.quads_dirty = true;
        }
        let mut h = g.client.selected as u64;
        for s in g.client.inventory.iter().chain(g.client.menu.iter().flat_map(|m| m.slots.iter().chain([&m.cursor]))) {
            h = crate::world::mix(h ^ ((s.item as u64) << 32 | s.count as u64) ^ (s.damage as u64) << 48);
        }
        h = crate::world::mix(h ^ (g.client.hurt_flash * 60.0) as u64);
        if h != r.client_hash {
            r.client_hash = h;
            r.quads_dirty = true;
        }
    }
    r.poll_bindings();
    let surface = r.gpu.surface_size();
    let gui_size = Vec2::new(surface.0 as f32, surface.1 as f32) / scale;
    let text_unit = (scale * 0.5).round().max(1.0) / scale;
    {
        let mut uis: Vec<&mut Ui> = Vec::new();
        if r.game.is_some() {
            uis.push(&mut r.hud.ui);
        }
        uis.extend(r.screens.iter_mut().map(|(_, u)| u));
        for ui in uis {
            if ui.dirty || ui.size != gui_size || ui.text_unit != text_unit {
                ui.text_unit = text_unit;
                ui.layout(gui_size, &r.ui.font);
                ui.dirty = false;
                r.quads_dirty = true;
            }
        }
    }

    let t = Instant::now();
    r.prof[1] = (t.duration_since(now).as_secs_f32() * 1000.0 - r.prof[1]).max(0.0);
    let Some(info) = r.gpu.begin_frame() else { return false };
    let t2 = Instant::now();
    r.prof[2] = t2.duration_since(t).as_secs_f32() * 1000.0;
    if (r.depth.1, r.depth.2) != (info.width, info.height) {
        r.gpu.destroy_texture(r.depth.0);
        r.depth = make_depth(&mut *r.gpu, info.width, info.height);
    }
    if r.quads_dirty {
        r.quads_dirty = false;
        let mut all = Vec::new();
        let game = r.game.as_ref().map(|_| (content(), &r.textures));
        let client = r.game.as_ref().map(|g| &g.client);
        let mut previews = Vec::new();
        let preview_tex = r.preview_tex;
        let mut layers: Vec<&Ui> = Vec::new();
        if r.game.is_some() {
            layers.push(&r.hud.ui);
        }
        layers.extend(r.screens.iter().map(|(_, u)| u));
        if let Some(flash) = client.map(|c| c.hurt_flash).filter(|&f| f > 0.0) {
            r.ui.quads.clear();
            if let Some((tex, _, _)) = r.ui.texture(&mut *r.gpu, &r.assets, "core:gui/vignette") {
                let a = (flash / 0.5 * 200.0) as u32;
                r.ui.image(crate::ui::Rect { x: 0.0, y: 0.0, w: surface.0 as f32, h: surface.1 as f32 }, tex, [0.0, 0.0, 1.0, 1.0], 0xD0101000 | a);
            }
            all.append(&mut r.ui.quads);
        }
        for ui in layers {
            let theme = &r.theme;
            let textures = &r.textures;
            let unit = ui.text_unit;
            let mut custom = |ren: &mut UiRenderer, tag: u32, data: u64, rect: crate::ui::Rect, hovered: bool| {
                crate::hud::draw_custom(ren, theme, textures, client, tag, data, rect, hovered, scale, unit);
                if tag == crate::client::modui::TAG_ENTITY {
                    let inner = crate::ui::Rect { x: rect.x + scale, y: rect.y + scale, w: rect.w - 2.0 * scale, h: rect.h - 2.0 * scale };
                    previews.push([inner.x, inner.y, inner.w, inner.h]);
                    if let Some(t) = preview_tex {
                        ren.image(inner, t, [0.0, 0.0, 1.0, 1.0], 0xFFFFFFFF);
                    }
                }
            };
            r.ui.build(&mut *r.gpu, &r.assets, ui, &r.theme, scale, game, &mut custom);
            all.append(&mut r.ui.quads);
        }
        // The stack held by the mouse in container menus.
        if let Some(cursor) = client.and_then(|c| c.menu.as_ref()).map(|m| m.cursor).filter(|s| !s.is_empty()) {
            let unit = text_unit;
            let p = r.input.cursor;
            let rect = crate::ui::Rect { x: p.x - 9.0 * scale, y: p.y - 9.0 * scale, w: 18.0 * scale, h: 18.0 * scale };
            crate::hud::stack_icon(&mut r.ui, &r.textures, rect, cursor, scale, unit);
            all.append(&mut r.ui.quads);
        }
        r.ui.quads = all;
        r.previews = previews;
    }

    r.list.clear();
    if let Some(g) = r.game.as_mut().filter(|_| !r.screens.is_empty()) {
        if let Some(&[x, y, w, h]) = r.previews.first() {
            let worn = g.client.worn();
            let look = r.input.cursor - Vec2::new(x, y);
            let tex = g.client.entity_render.draw_preview(&mut *r.gpu, &mut r.list, &info, [w.round() as u32, h.round() as u32], look, &worn);
            if tex != r.preview_tex {
                r.preview_tex = tex;
                r.quads_dirty = true;
            }
        }
    }
    let clear = r.game.as_ref().map_or([0.02, 0.02, 0.03, 1.0], |g| g.client.fog_color());
    r.list.push(Cmd::BeginPass { color: Some((Target::Surface, Some(clear))), depth: Some((r.depth.0, Some(0.0))) });
    if let Some(g) = &mut r.game {
        let worn = g.client.worn();
        g.client.entity_render.preload(&mut *r.gpu, &r.assets, &g.client.entities, &worn);
        g.client.render(&mut *r.gpu, &mut r.list, &info, &r.textures);
    }
    r.ui.draw(&mut *r.gpu, &mut r.list, &info);
    r.list.push(Cmd::Mark);
    r.list.push(Cmd::EndPass);

    let capture_now = r.shot_pending;
    if opts.shot.is_some() && opts.script.is_none() && !r.shot_pending {
        let ready = match &r.game {
            Some(g) => g.client.joined && g.client.world.cubes.len() > 50 && g.client.world.pending_meshes() == 0,
            None => true,
        };
        if ready && r.started.elapsed().as_secs_f32() > opts.shot_delay {
            r.gpu.request_capture();
            r.shot_pending = true;
        }
    }
    let t3 = Instant::now();
    r.prof[3] = t3.duration_since(t2).as_secs_f32() * 1000.0;
    r.gpu.end_frame(&r.list);
    r.prof[4] = t3.elapsed().as_secs_f32() * 1000.0;
    if let Some(b) = &mut r.bench {
        let server = r.game.as_ref().and_then(|g| g.server.as_ref());
        let (server_ms, phases) = server.map_or((0.0, [0.0; 6]), |s| (s.tick_ms(), s.phases()));
        b.sample(dt as f32 * 1000.0, &r.prof, r.gpu.gpu_times(), server_ms, phases);
        if b.done() {
            let extra = match &r.game {
                Some(g) => format!(
                    "pos {:.1} {:.1} {:.1}, cubes {}, draws {}, faces {}k, entities {}, regions {}",
                    g.client.player.pos.x,
                    g.client.player.pos.y,
                    g.client.player.pos.z,
                    g.client.world.cubes.len(),
                    g.client.render.drawn,
                    g.client.render.faces_drawn / 1000,
                    g.client.entities.len(),
                    g.server.as_ref().map_or(0, |s| s.regions())
                ),
                None => String::new(),
            };
            log::info!("{}", b.report(&extra));
            r.bench = None;
        }
    }
    if capture_now {
        r.shot_pending = false;
        if let Some(cap) = r.gpu.take_capture() {
            let path = r.shot_path.take().or_else(|| opts.shot.clone().filter(|_| opts.script.is_none())).unwrap_or_else(|| {
                r.settings.run_dir.join("screenshots").join(format!("{}.png", saves::now_secs()))
            });
            match crate::assets::encode_png(&path, cap.width, cap.height, &cap.rgba) {
                Ok(()) => log::info!("screenshot saved to {}", path.display()),
                Err(e) => log::error!("screenshot: {e}"),
            }
        }
        if opts.shot.is_some() && opts.script.is_none() {
            return true;
        }
    }
    r.fps_frames += 1;
    let el = r.fps_timer.elapsed().as_secs_f32();
    if el >= 0.5 {
        r.fps = r.fps_frames as f32 / el;
        r.frame_ms = el * 1000.0 / r.fps_frames as f32;
        r.fps_timer = Instant::now();
        r.fps_frames = 0;
    }
    false
}
