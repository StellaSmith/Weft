use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use glam::{DVec3, Vec2};
use voxide_render::{Backend, BackendDesc, CommandList, Cmd, Format, Target, TextureDesc, TextureId};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use crate::assets::Assets;
use crate::client::Client;
use crate::input::{Input, Key};
use crate::modloader::{self, LoadedMod};
use crate::net;
use crate::server::{self, ServerConfig, ServerHandle};

pub type BackendFactory = fn(&BackendDesc) -> Result<Box<dyn Backend>, voxide_render::Error>;

#[derive(Clone, Debug)]
pub struct Options {
    pub run_dir: PathBuf,
    pub mod_dirs: Vec<PathBuf>,
    pub gpu_debug: bool,
    pub vsync: bool,
    /// Automation: save a screenshot here once the world finished loading, then quit.
    pub shot: Option<PathBuf>,
    pub shot_delay: f32,
    /// Automation: fixed camera (x, y, z, yaw degrees, pitch degrees).
    pub camera: Option<[f64; 5]>,
    pub seed: u64,
    pub view_radius: i32,
    pub size: (u32, u32),
}

struct Running {
    gpu: Box<dyn Backend>,
    window: Arc<Window>,
    depth: (TextureId, u32, u32),
    list: CommandList,
    client: Client,
    server: ServerHandle,
    input: Input,
    last: Instant,
    started: Instant,
    frames: u64,
    shot_pending: bool,
    fps_timer: Instant,
    fps_frames: u32,
    _mods: Vec<LoadedMod>,
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

impl App {
    fn start(&mut self, el: &ActiveEventLoop) -> Result<Running, String> {
        let attrs = Window::default_attributes()
            .with_title("Voxide")
            .with_inner_size(winit::dpi::PhysicalSize::new(self.opts.size.0, self.opts.size.1));
        let window = Arc::new(el.create_window(attrs).map_err(|e| e.to_string())?);
        let size = window.inner_size();
        let mut gpu = (self.factory)(&BackendDesc {
            window: &*window,
            width: size.width,
            height: size.height,
            vsync: self.opts.vsync,
            debug: self.opts.gpu_debug,
        })
        .map_err(|e| e.to_string())?;

        let cache = self.opts.run_dir.join("cache");
        let mods = modloader::load(&self.opts.mod_dirs, &cache)?;
        let assets = Assets::new(mods.iter().map(|m| (m.id.clone(), m.assets.clone())), Vec::new());

        let server = server::spawn(ServerConfig { seed: self.opts.seed, view_radius: self.opts.view_radius, view_height: 6 });
        let (client_conn, server_conn) = net::local_pair();
        let _ = server.connect.send(server_conn);
        let client = Client::new(&mut *gpu, &assets, client_conn, "Player")?;
        let depth = make_depth(&mut *gpu, size.width, size.height);
        Ok(Running {
            gpu,
            window,
            depth,
            list: CommandList::default(),
            client,
            server,
            input: Input::default(),
            last: Instant::now(),
            started: Instant::now(),
            frames: 0,
            shot_pending: false,
            fps_timer: Instant::now(),
            fps_frames: 0,
            _mods: mods,
        })
    }
}

fn make_depth(gpu: &mut dyn Backend, w: u32, h: u32) -> (TextureId, u32, u32) {
    let t = gpu.create_texture(
        &TextureDesc { width: w.max(1), height: h.max(1), layers: None, mips: 1, format: Format::Depth32, render_target: true },
        "depth",
    );
    (t, w, h)
}

fn set_capture(window: &Window, input: &mut Input, on: bool) {
    if on {
        let _ = window.set_cursor_grab(CursorGrabMode::Locked).or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined));
        window.set_cursor_visible(false);
    } else {
        let _ = window.set_cursor_grab(CursorGrabMode::None);
        window.set_cursor_visible(true);
    }
    input.captured = on;
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
            WindowEvent::CloseRequested => {
                r.client.leave();
                el.exit();
            }
            WindowEvent::Resized(size) => r.gpu.resize(size.width, size.height),
            WindowEvent::Focused(false) => {
                r.input.release_all();
                set_capture(&r.window, &mut r.input, false);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    match event.state {
                        ElementState::Pressed => {
                            if code == KeyCode::Escape && r.input.captured {
                                set_capture(&r.window, &mut r.input, false);
                            }
                            r.input.key_down(Key::Code(code));
                        }
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
                    if !r.input.captured && b == 0 {
                        set_capture(&r.window, &mut r.input, true);
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
                if frame(r, &self.opts) {
                    r.client.leave();
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
            r.server.stop();
            drop(r.client);
            drop(r.gpu);
        }
    }
}

/// One frame. Returns true when the app should quit.
fn frame(r: &mut Running, opts: &Options) -> bool {
    let now = Instant::now();
    let dt = (now - r.last).as_secs_f64();
    r.last = now;

    if r.input.pressed("core:reload") {
        r.client.render.reload_shaders(&mut *r.gpu);
    }
    r.client.update(&mut *r.gpu, &r.input, dt);
    if let Some(cam) = opts.camera {
        r.client.player.pos = DVec3::new(cam[0], cam[1] - 1.62, cam[2]);
        r.client.player.yaw = (cam[3] as f32).to_radians();
        r.client.player.pitch = (cam[4] as f32).to_radians();
        r.client.player.flying = true;
        r.client.player.vel = DVec3::ZERO;
    }
    if let Some(reason) = &r.client.disconnected {
        log::info!("disconnected: {reason}");
        return true;
    }
    r.input.end_frame();

    let Some(info) = r.gpu.begin_frame() else { return false };
    if (r.depth.1, r.depth.2) != (info.width, info.height) {
        r.gpu.destroy_texture(r.depth.0);
        r.depth = make_depth(&mut *r.gpu, info.width, info.height);
    }
    r.list.clear();
    let fog = r.client.fog_color();
    r.list.push(Cmd::BeginPass { color: Some((Target::Surface, Some(fog))), depth: Some((r.depth.0, Some(0.0))) });
    let view = opts.view_radius as f32 * 16.0;
    r.client.render(&mut *r.gpu, &mut r.list, &info, view);
    r.list.push(Cmd::EndPass);

    let capture_now = r.shot_pending;
    if let Some(_) = &opts.shot {
        let loaded = r.client.joined && r.client.world.cubes.len() > 50 && r.client.world.pending_meshes() == 0;
        if !r.shot_pending && loaded && r.started.elapsed().as_secs_f32() > opts.shot_delay {
            r.gpu.request_capture();
            r.shot_pending = true;
        }
    }
    r.gpu.end_frame(&r.list);
    if capture_now {
        if let (Some(cap), Some(path)) = (r.gpu.take_capture(), &opts.shot) {
            match crate::assets::encode_png(path, cap.width, cap.height, &cap.rgba) {
                Ok(()) => log::info!("screenshot saved to {}", path.display()),
                Err(e) => log::error!("screenshot: {e}"),
            }
        }
        return true;
    }
    r.frames += 1;
    r.fps_frames += 1;
    if r.fps_timer.elapsed().as_secs_f32() >= 2.0 {
        let fps = r.fps_frames as f32 / r.fps_timer.elapsed().as_secs_f32();
        log::debug!(
            "fps {fps:.0}, cubes {}, pending meshes {}, draws {}, pos {:.1?}, joined {}",
            r.client.world.cubes.len(),
            r.client.world.pending_meshes(),
            r.client.render.drawn,
            r.client.player.pos,
            r.client.joined
        );
        r.window.set_title(&format!(
            "Voxide | {fps:.0} fps | {} draws, {}k faces | {} cubes",
            r.client.render.drawn,
            r.client.render.faces_drawn / 1000,
            r.client.world.cubes.len()
        ));
        r.fps_timer = Instant::now();
        r.fps_frames = 0;
    }
    false
}
