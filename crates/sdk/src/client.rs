//! Client side for mods: frame and screen hooks that see the local game and may drive it
//! (simulated input, the local player, worlds, screens, screenshots), mod key bindings, and
//! message channels between a mod's client and server parts.
//!
//! ```ignore
//! r.key("jetpack", "KeyJ");
//! r.client_frame(|c| if c.held("mymod:jetpack") { c.send("boost", &[1]) });
//! r.packet("boost", |p| { let w = p.world(); /* server side */ });
//! ```

use crate::abi::{self, ItemStack, Slice, Str, Versioned};
use crate::{ClientView, Registry, World, callback, host, qualify};

pub use abi::{ClientState, client_flags};

/// The local client during a client callback. Only valid for the duration of that callback.
#[derive(Clone, Copy)]
pub struct Client {
    raw: *mut abi::ClientCtx,
}

fn s(x: &str) -> Str {
    Str::new(x)
}

impl Client {
    pub(crate) fn from_raw(raw: *mut abi::ClientCtx) -> Client {
        Client { raw }
    }

    pub(crate) fn raw(&self) -> *mut abi::ClientCtx {
        self.raw
    }

    /// The read-only view UI bindings get (own player, components, menu data).
    pub fn view(&self) -> ClientView {
        ClientView::from_raw(self.raw)
    }

    pub fn state(&self) -> ClientState {
        let mut st = ClientState::DEFAULT;
        unsafe { (host().client_state)(self.raw, &mut st) };
        st
    }

    /// Top screen name (`core:menu`, `core:pause`, ...), empty in the world.
    pub fn screen(&self) -> String {
        unsafe { self.state().screen.as_str().to_string() }
    }

    fn input(&self, op: u32, text: &str, x: f32, y: f32) -> bool {
        unsafe { (host().client_input)(self.raw, op, s(text), x, y) != 0 }
    }

    /// Presses the key bound to `action` for `secs` (0 = a tap). False for an unknown action.
    pub fn press(&self, action: &str, secs: f32) -> bool {
        self.input(abi::client_input::ACTION, action, secs, 0.0)
    }

    /// A key by its settings name (`Enter`, `Tab`, `KeyE`, `F3`).
    pub fn key(&self, key: &str, secs: f32) -> bool {
        self.input(abi::client_input::KEY, key, secs, 0.0)
    }

    /// Mouse button 0 left, 1 right, 2 middle.
    pub fn click(&self, button: u32, secs: f32) {
        self.input(abi::client_input::CLICK, "", secs, button as f32);
    }

    /// Moves the mouse to fractions of the window.
    pub fn cursor(&self, x: f32, y: f32) {
        self.input(abi::client_input::CURSOR, "", x, y);
    }

    pub fn scroll(&self, steps: f32) {
        self.input(abi::client_input::SCROLL, "", steps, 0.0);
    }

    /// Text typed into the focused field.
    pub fn type_text(&self, text: &str) {
        self.input(abi::client_input::TYPE, text, 0.0, 0.0);
    }

    pub fn release_all(&self) {
        self.input(abi::client_input::RELEASE_ALL, "", 0.0, 0.0);
    }

    /// The action (yours from `Registry::key`, or `core:*`) was pressed this frame.
    pub fn pressed(&self, action: &str) -> bool {
        unsafe { (host().client_pressed)(self.raw, s(action)) & 1 != 0 }
    }

    pub fn held(&self, action: &str) -> bool {
        unsafe { (host().client_pressed)(self.raw, s(action)) & 2 != 0 }
    }

    pub fn control(&self, c: &abi::PlayerControl) {
        unsafe { (host().client_set_player)(self.raw, c) }
    }

    /// Turns the local player, in degrees like the F3 screen.
    pub fn look(&self, yaw_deg: f32, pitch_deg: f32) {
        self.control(&abi::PlayerControl { mask: abi::control_mask::LOOK, yaw: yaw_deg.to_radians(), pitch: pitch_deg.to_radians(), ..abi::PlayerControl::DEFAULT });
    }

    /// Puts the local player's feet at `pos` (the server takes it as a move).
    pub fn teleport(&self, pos: [f64; 3]) {
        self.control(&abi::PlayerControl { mask: abi::control_mask::POS | abi::control_mask::VEL, pos, ..abi::PlayerControl::DEFAULT });
    }

    pub fn velocity(&self, vel: [f64; 3]) {
        self.control(&abi::PlayerControl { mask: abi::control_mask::VEL, vel, ..abi::PlayerControl::DEFAULT });
    }

    pub fn fly(&self, on: bool) {
        self.control(&abi::PlayerControl { mask: abi::control_mask::FLYING, flying: on as u32, ..abi::PlayerControl::DEFAULT });
    }

    pub fn fly_speed(&self, mul: f32) {
        self.control(&abi::PlayerControl { mask: abi::control_mask::FLY_SPEED, fly_speed: mul, ..abi::PlayerControl::DEFAULT });
    }

    /// Hotbar slot 0..9.
    pub fn select(&self, slot: u32) {
        self.control(&abi::PlayerControl { mask: abi::control_mask::SELECTED, selected: slot, ..abi::PlayerControl::DEFAULT });
    }

    fn command(&self, op: u32, text: &str, num: f64) -> u64 {
        unsafe { (host().client_command)(self.raw, op, s(text), num) }
    }

    /// A chat line or `/command`, as if typed.
    pub fn chat(&self, line: &str) {
        self.command(abi::client_op::CHAT, line, 0.0);
    }

    /// Captures the next frame to `path` (a bare name goes into `screenshots/`). The pixels are
    /// available a frame later through `image(id)`.
    pub fn screenshot(&self, path: &str) -> u64 {
        self.command(abi::client_op::SCREENSHOT, path, 0.0)
    }

    pub fn quit(&self, code: i32) {
        self.command(abi::client_op::QUIT, "", code as f64);
    }

    pub fn open_world(&self, name: &str, seed: u64) {
        self.command(abi::client_op::OPEN_WORLD, name, seed as f64);
    }

    /// Opens a fresh world, deleting a save of the same name first.
    pub fn new_world(&self, name: &str, seed: u64) {
        self.command(abi::client_op::NEW_WORLD, name, seed as f64);
    }

    pub fn leave_world(&self) {
        self.command(abi::client_op::LEAVE_WORLD, "", 0.0);
    }

    pub fn host_world(&self) {
        self.command(abi::client_op::HOST, "", 0.0);
    }

    pub fn join(&self, ticket: &str) {
        self.command(abi::client_op::JOIN, ticket, 0.0);
    }

    /// See `abi::client_op::OPEN_SCREEN` for the names.
    pub fn open_screen(&self, name: &str) -> bool {
        self.command(abi::client_op::OPEN_SCREEN, name, 0.0) != 0
    }

    pub fn close_screen(&self) {
        self.command(abi::client_op::CLOSE_SCREEN, "", 0.0);
    }

    pub fn reload_assets(&self) {
        self.command(abi::client_op::RELOAD_ASSETS, "", 0.0);
    }

    /// Statistics over `secs`, logged as a `BENCH` line; numbers in `state().bench_*` once
    /// `state().benches` goes up.
    pub fn bench(&self, secs: f32, label: &str) {
        self.command(abi::client_op::BENCH, label, secs as f64);
    }

    pub fn respawn(&self) {
        self.command(abi::client_op::RESPAWN, "", 0.0);
    }

    /// A settings.txt value for this session only (`gui_scale`, `language`, `view_distance`...).
    pub fn setting(&self, key: &str, value: &str) -> bool {
        self.command(abi::client_op::SETTING, &format!("{key} = {value}"), 0.0) != 0
    }

    fn text(&self, which: u32, index: u32) -> String {
        unsafe { (host().client_text)(self.raw, which, index).as_str().to_string() }
    }

    pub fn ticket(&self) -> String {
        self.text(abi::client_text::TICKET, 0)
    }

    pub fn run_dir(&self) -> String {
        self.text(abi::client_text::RUN_DIR, 0)
    }

    /// Chat line `index` back from the newest, empty past the end.
    pub fn chat_line(&self, index: u32) -> String {
        self.text(abi::client_text::CHAT, index)
    }

    pub fn bench_line(&self) -> String {
        self.text(abi::client_text::BENCH, 0)
    }

    /// Text of the top screen's focused input field.
    pub fn input_text(&self) -> String {
        self.text(abi::client_text::INPUT, 0)
    }

    /// The block the client has at `pos`, `None` while that cube is not loaded.
    pub fn block(&self, pos: [i32; 3]) -> Option<u32> {
        let b = unsafe { (host().client_block)(self.raw, &pos) };
        (b != u32::MAX).then_some(b)
    }

    /// Own inventory slot (0..9 hotbar, 9..36 main, 36..40 armour).
    pub fn inventory_slot(&self, slot: u32) -> ItemStack {
        unsafe { (host().client_slot)(self.raw, 0, slot) }
    }

    pub fn menu_slot(&self, slot: u32) -> ItemStack {
        unsafe { (host().client_slot)(self.raw, 1, slot) }
    }

    pub fn cursor_stack(&self) -> ItemStack {
        unsafe { (host().client_slot)(self.raw, 2, 0) }
    }

    /// The open menu as (kind, window, slot count).
    pub fn menu(&self) -> Option<[u32; 3]> {
        let mut out = [0; 3];
        (unsafe { (host().client_menu)(self.raw, &mut out) } != 0).then_some(out)
    }

    /// Entities the client knows, the local player left out.
    pub fn entities(&self) -> Vec<u64> {
        let mut buf = vec![0u64; 256];
        let n = unsafe { (host().client_entities)(self.raw, buf.as_mut_ptr(), buf.len()) };
        if n > buf.len() {
            buf.resize(n, 0);
            unsafe { (host().client_entities)(self.raw, buf.as_mut_ptr(), buf.len()) };
        }
        buf.truncate(n);
        buf
    }

    pub fn entity(&self, id: u64) -> Option<abi::EntityState> {
        let mut st = abi::EntityState::default();
        (unsafe { (host().client_entity)(self.raw, id, &mut st) } != 0).then_some(st)
    }

    /// Where an element of the top screen is (fractions of the window: x, y, w, h):
    /// `slot:N`, `id:name` or `text:words`.
    pub fn ui_rect(&self, query: &str) -> Option<[f32; 4]> {
        let mut r = [0.0; 4];
        (unsafe { (host().client_ui_rect)(self.raw, s(query), &mut r) } != 0).then_some(r)
    }

    /// Moves the mouse to the middle of a UI element and clicks it with `button`.
    pub fn click_ui(&self, query: &str, button: u32) -> bool {
        let Some([x, y, w, h]) = self.ui_rect(query) else { return false };
        self.cursor(x + w / 2.0, y + h / 2.0);
        self.click(button, 0.0);
        true
    }

    fn draw(&self, kind: u32, a: [f64; 3], b: [f64; 3], color: u32, text: &str) {
        unsafe { (host().client_draw)(self.raw, kind, &a, &b, color, s(text)) }
    }

    /// Lines, boxes and labels for this frame only; colours are 0xRRGGBBAA.
    pub fn draw_line(&self, a: [f64; 3], b: [f64; 3], color: u32) {
        self.draw(0, a, b, color, "");
    }

    pub fn draw_box(&self, min: [f64; 3], max: [f64; 3], color: u32) {
        self.draw(1, min, max, color, "");
    }

    pub fn draw_text(&self, pos: [f64; 3], text: &str, color: u32) {
        self.draw(2, pos, pos, color, text);
    }

    /// Text centred at fractions of the window.
    pub fn screen_text(&self, x: f32, y: f32, text: &str, color: u32) {
        self.draw(3, [x as f64, y as f64, 0.0], [0.0; 3], color, text);
    }

    /// Sends `data` on this mod's channel `name` to the server.
    pub fn send(&self, channel: &str, data: &[u8]) {
        let name = qualify(crate::mod_id(), channel);
        unsafe { (host().client_send_packet)(self.raw, s(&name), Slice::new(data)) }
    }
}

/// RGBA pixels of a screenshot or loaded image: (width, height, pixels).
pub fn image(id: u64) -> Option<(u32, u32, &'static [u8])> {
    let (mut w, mut h) = (0, 0);
    let p = unsafe { (host().image_get)(id, &mut w, &mut h) };
    (!p.is_null()).then(|| (w, h, unsafe { std::slice::from_raw_parts(p, (w * h * 4) as usize) }))
}

pub fn image_load(path: &str) -> Option<u64> {
    let id = unsafe { (host().image_load)(s(path)) };
    (id != 0).then_some(id)
}

pub fn image_save(path: &str, w: u32, h: u32, rgba: &[u8]) -> bool {
    unsafe { (host().image_save)(s(path), Slice::new(rgba), w, h) != 0 }
}

pub fn image_free(id: u64) {
    unsafe { (host().image_free)(id) }
}

/// `--name value` from the command line.
pub fn launch_option(name: &str) -> Option<String> {
    let v = unsafe { (host().launch_option)(s(name)).as_str() };
    (!v.is_empty()).then(|| v.to_string())
}

/// A message on a channel, on the server.
pub struct Packet {
    args: abi::PacketArgs,
}

impl Packet {
    pub fn world(&self) -> World {
        World::from_raw(self.args.world)
    }
    pub fn player(&self) -> u64 {
        self.args.player
    }
    pub fn data(&self) -> &[u8] {
        unsafe { self.args.data.as_slice() }
    }
    /// Answers on `channel` to the sender.
    pub fn reply(&self, channel: &str, data: &[u8]) {
        self.world().send(self.args.player, channel, data);
    }
}

impl World {
    /// Sends `data` on this mod's channel `name` to `player`'s client.
    pub fn send(&self, player: u64, channel: &str, data: &[u8]) {
        let name = qualify(crate::mod_id(), channel);
        unsafe { (host().send_packet)(self.raw(), player, s(&name), Slice::new(data)) }
    }

    /// Opens `menu` on the block entity at `pos`; false (and a logged reason) if there is none.
    pub fn open_menu_at(&self, player: u64, menu: &crate::Menu, pos: [i32; 3]) -> bool {
        self.open_menu_kind_at(player, menu.id(), pos)
    }

    pub fn open_menu_kind_at(&self, player: u64, menu: u32, pos: [i32; 3]) -> bool {
        unsafe { (host().open_menu_at)(self.raw(), player, menu, &pos) != 0 }
    }

    /// An inventory of `slots` not tied to a block (tests, backpacks): menus open on it like on
    /// a chest. Never saved; remove it with `remove`.
    pub fn virtual_inventory(&self, slots: u32, pos: [f64; 3]) -> u64 {
        unsafe { (host().inventory_create)(self.raw(), slots, &pos) }
    }

    /// Moves an entity (players included) at the end of the tick, to `dim` or the same dimension.
    pub fn teleport(&self, id: u64, dim: Option<&crate::Dimension>, pos: [f64; 3]) {
        let d = dim.map_or(u32::MAX, |d| d.id());
        unsafe { (host().entity_teleport)(self.raw(), id, d, &pos) }
    }

    pub fn set_time(&self, time: u64) {
        unsafe { (host().set_time)(self.raw(), time) }
    }

    /// 0 survival, 1 creative, 2 spectator.
    pub fn set_game_mode(&self, player: u64, mode: u32) {
        unsafe { (host().set_game_mode)(self.raw(), player, mode) }
    }
}

impl Registry {
    /// Every frame on the client, before input is handled.
    pub fn client_frame(&mut self, f: impl Fn(&Client) + Send + Sync + 'static) {
        let cb = callback(move |a: &mut abi::ClientFrameArgs| f(&Client::from_raw(a.client)));
        unsafe { (host().reg_client_hook)(self.raw, abi::client_hook::FRAME, cb) }
    }

    /// When the top screen changes; the name is empty back in the world.
    pub fn client_screen(&mut self, f: impl Fn(&Client, &str) + Send + Sync + 'static) {
        let cb = callback(move |a: &mut abi::ClientScreenArgs| f(&Client::from_raw(a.client), unsafe { a.screen.as_str() }));
        unsafe { (host().reg_client_hook)(self.raw, abi::client_hook::SCREEN, cb) }
    }

    /// Server side of channel `name`: called when a client sends on it.
    pub fn packet(&mut self, name: &str, f: impl Fn(&Packet) + Send + Sync + 'static) {
        let name = qualify(self.ns, name);
        let cb = callback(move |a: &mut abi::PacketArgs| f(unsafe { &*(a as *const abi::PacketArgs as *const Packet) }));
        unsafe { (host().reg_packet)(self.raw, s(&name), 0, cb) }
    }

    /// Client side of channel `name`: called when the server sends on it.
    pub fn client_packet(&mut self, name: &str, f: impl Fn(&Client, &[u8]) + Send + Sync + 'static) {
        let name = qualify(self.ns, name);
        let cb = callback(move |a: &mut abi::ClientPacketArgs| f(&Client::from_raw(a.client), unsafe { a.data.as_slice() }));
        unsafe { (host().reg_packet)(self.raw, s(&name), 1, cb) }
    }

    /// Key binding `<ns>:<name>` with a default key (settings name: `KeyJ`, `F6`, `mouse3`),
    /// listed in Controls as `key.<ns>.<name>`; read with `Client::pressed`/`held`.
    pub fn key(&mut self, name: &str, default: &str) {
        let name = qualify(self.ns, name);
        unsafe { (host().reg_key)(self.raw, s(&name), s(default)) }
    }
}
