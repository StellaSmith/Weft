//! UI building for HUD parts and menu screens. Nodes are sent to the engine as they are created;
//! bound values are callbacks the engine polls every frame and redraws only on change.

use crate::abi::{self, ItemStack, Slice, Str, ui_kind as k};
use crate::component::{Component, Value};
use crate::{callback, host};

/// Read-only client state for bindings.
#[derive(Clone, Copy)]
pub struct ClientView {
    raw: *mut abi::ClientCtx,
}

impl ClientView {
    pub(crate) fn from_raw(raw: *mut abi::ClientCtx) -> ClientView {
        ClientView { raw }
    }
    pub fn player(&self) -> u64 {
        unsafe { (host().client_player)(self.raw) }
    }
    /// Own player's synced component.
    pub fn component<T: Value>(&self, c: &Component<T>) -> Option<T> {
        let p = unsafe { (host().client_component)(self.raw, 0, c.id()) };
        (!p.is_null()).then(|| unsafe { std::ptr::read_unaligned(p as *const T) })
    }
    pub fn me(&self) -> Option<abi::EntityState> {
        let mut s = abi::EntityState::default();
        (unsafe { (host().client_entity)(self.raw, 0, &mut s) } != 0).then_some(s)
    }
    /// The local player is in creative mode.
    pub fn creative(&self) -> bool {
        unsafe { (host().client_creative)(self.raw) != 0 }
    }
    /// Another entity's component the server shares with this client (`SYNC_WATCHERS`, or
    /// `SYNC_MENU` while its menu is open).
    pub fn component_of<T: Value>(&self, entity: u64, c: &Component<T>) -> Option<T> {
        if entity == 0 {
            return None;
        }
        let p = unsafe { (host().client_component)(self.raw, entity, c.id()) };
        (!p.is_null()).then(|| unsafe { std::ptr::read_unaligned(p as *const T) })
    }
    /// The entity whose menu is open (a machine), if any.
    pub fn menu_owner(&self) -> Option<u64> {
        let o = unsafe { (host().client_menu_owner)(self.raw) };
        (o != 0).then_some(o)
    }
    /// A component of the open menu's machine, synced with `SYNC_MENU`, for screens to bind to.
    pub fn menu_component<T: Value>(&self, c: &Component<T>) -> Option<T> {
        self.component_of(self.menu_owner()?, c)
    }
}

/// Text argument for translation templates.
pub enum Arg<'a> {
    Text(&'a str),
    Key(&'a str),
    Int(i64),
    Float(f64),
    Item(u32),
}

fn raw_args<'a>(args: &'a [Arg<'a>]) -> Vec<abi::UiArg> {
    args.iter()
        .map(|a| match a {
            Arg::Text(s) => abi::UiArg { kind: abi::ui_arg::TEXT, text: Str::new(s), ..abi::UiArg::DEFAULT },
            Arg::Key(s) => abi::UiArg { kind: abi::ui_arg::KEY, text: Str::new(s), ..abi::UiArg::DEFAULT },
            Arg::Int(v) => abi::UiArg { kind: abi::ui_arg::INT, num: *v as f64, ..abi::UiArg::DEFAULT },
            Arg::Float(v) => abi::UiArg { kind: abi::ui_arg::FLOAT, num: *v, ..abi::UiArg::DEFAULT },
            Arg::Item(v) => abi::UiArg { kind: abi::ui_arg::ITEM, num: *v as f64, ..abi::UiArg::DEFAULT },
        })
        .collect()
}

/// Layout of a node. Sizes are GUI pixels; `None` = fit content.
#[derive(Clone, Copy)]
pub struct S {
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub grow: f32,
    pub gap: f32,
    pub pad: f32,
    pub row: bool,
    pub align: u32,
    pub justify: u32,
}

impl Default for S {
    fn default() -> Self {
        S { width: None, height: None, grow: 0.0, gap: 0.0, pad: 0.0, row: false, align: abi::ui_align::START, justify: abi::ui_justify::START }
    }
}

impl S {
    pub fn row() -> S {
        S { row: true, align: abi::ui_align::CENTER, ..S::default() }
    }
    pub fn col() -> S {
        S::default()
    }
    pub fn gap(mut self, g: f32) -> S {
        self.gap = g;
        self
    }
    pub fn pad(mut self, p: f32) -> S {
        self.pad = p;
        self
    }
    pub fn size(mut self, w: f32, h: f32) -> S {
        self.width = Some(w);
        self.height = Some(h);
        self
    }
    pub fn w(mut self, w: f32) -> S {
        self.width = Some(w);
        self
    }
    pub fn grow(mut self) -> S {
        self.grow = 1.0;
        self
    }
    pub fn center(mut self) -> S {
        self.align = abi::ui_align::CENTER;
        self.justify = abi::ui_justify::CENTER;
        self
    }
    /// Children fill the cross axis (rows as wide as the column), so `spacer()`s can center.
    pub fn stretch(mut self) -> S {
        self.align = abi::ui_align::STRETCH;
        self
    }
    pub fn end(mut self) -> S {
        self.justify = abi::ui_justify::END;
        self
    }
}

pub struct Ui {
    raw: *mut abi::UiCtx,
    client: ClientView,
}

pub(crate) fn ui_callback(f: impl Fn(&mut Ui) + Send + Sync + 'static) -> abi::Callback {
    callback(move |a: &mut abi::UiArgs| {
        let mut ui = Ui { raw: a.ui, client: ClientView { raw: a.client } };
        f(&mut ui)
    })
}

fn node(kind: u32, s: S) -> abi::UiNode {
    abi::UiNode {
        kind,
        width: s.width.unwrap_or(f32::NAN),
        height: s.height.unwrap_or(f32::NAN),
        grow: s.grow,
        gap: s.gap,
        pad: [s.pad; 4],
        row: s.row as u32,
        align: s.align,
        justify: s.justify,
        data: 0,
        ..abi::UiNode::DEFAULT
    }
}

impl Ui {
    pub fn client(&self) -> ClientView {
        self.client
    }

    fn open(&mut self, n: &abi::UiNode) {
        unsafe { (host().ui_open)(self.raw, n) }
    }

    fn close(&mut self) {
        unsafe { (host().ui_close)(self.raw) }
    }

    fn with(&mut self, n: abi::UiNode, f: impl FnOnce(&mut Ui)) {
        self.open(&n);
        f(self);
        self.close();
    }

    fn leaf(&mut self, n: abi::UiNode) {
        self.open(&n);
        self.close();
    }

    pub fn column(&mut self, s: S, f: impl FnOnce(&mut Ui)) {
        self.with(node(k::CONTAINER, s), f)
    }

    pub fn row(&mut self, s: S, f: impl FnOnce(&mut Ui)) {
        self.with(node(k::CONTAINER, S { row: true, ..s }), f)
    }

    /// Themed 9-slice background (`class` from the theme, e.g. "panel").
    pub fn panel(&mut self, class: &str, s: S, f: impl FnOnce(&mut Ui)) {
        let mut n = node(k::PANEL, s);
        n.class = Str::new(class);
        self.with(n, f)
    }

    /// Translated text.
    pub fn text(&mut self, key: &str) {
        self.text_args(key, &[])
    }

    pub fn text_args(&mut self, key: &str, args: &[Arg]) {
        let a = raw_args(args);
        let mut n = node(k::TEXT, S::default());
        n.text = Str::new(key);
        n.args = Slice::new(&a);
        self.leaf(n)
    }

    /// Text recomputed every frame; redrawn only when it changes.
    pub fn text_bind(&mut self, f: impl Fn(&ClientView, &mut TextOut) + Send + Sync + 'static) {
        let mut n = node(k::TEXT, S::default());
        n.bind = callback(move |a: &mut abi::UiBindArgs| {
            let view = ClientView { raw: a.client };
            let mut out = TextOut { raw: a.text };
            f(&view, &mut out);
        });
        self.leaf(n)
    }

    /// One menu slot.
    pub fn slot(&mut self, slot: u32) {
        let mut n = node(k::SLOT, S::default());
        n.slot = slot;
        self.leaf(n)
    }

    /// A slot showing `icon` (a GUI texture) while empty, like the armor slots.
    pub fn slot_icon(&mut self, slot: u32, icon: &str) {
        let mut n = node(k::SLOT, S::default());
        n.slot = slot;
        n.text = Str::new(icon);
        self.leaf(n)
    }

    /// The big slot vanilla uses for crafting results.
    pub fn slot_big(&mut self, slot: u32) {
        let mut n = node(k::SLOT, S::default().size(26.0, 26.0));
        n.slot = slot;
        self.leaf(n)
    }

    /// The player's main inventory and hotbar (`MenuCtx::player_slots`) from menu slot `start`,
    /// spaced like vanilla, under the title "Inventory" when `title`.
    pub fn player_inventory(&mut self, start: u32, title: bool) {
        let l = crate::player_layout();
        let main = l.slots - l.hotbar - l.worn.len() as u32;
        self.column(S::col().gap(2.0), |ui| {
            if title {
                ui.text("container.inventory");
            }
            ui.column(S::col().gap(4.0), |ui| {
                ui.slots(start, main, l.hotbar.max(1));
                ui.slots(start + main, l.hotbar, l.hotbar.max(1));
            });
        });
    }

    /// `count` menu slots starting at `start`, `per_row` per line.
    pub fn slots(&mut self, start: u32, count: u32, per_row: u32) {
        self.column(S::col(), |ui| {
            for r in 0..count.div_ceil(per_row) {
                ui.row(S::row(), |ui| {
                    for i in 0..per_row.min(count - r * per_row) {
                        ui.slot(start + r * per_row + i);
                    }
                });
            }
        });
    }

    /// Progress from any value 0..1.
    pub fn progress_bind(&mut self, class: &str, s: S, f: impl Fn(&ClientView) -> f32 + Send + Sync + 'static) {
        self.progress_bind_dir(class, s, abi::ui_dir::RIGHT, f)
    }

    /// Like `progress_bind`, filling in direction `dir` (`abi::ui_dir`).
    pub fn progress_bind_dir(&mut self, class: &str, s: S, dir: u32, f: impl Fn(&ClientView) -> f32 + Send + Sync + 'static) {
        let mut n = node(k::PROGRESS, s);
        n.class = Str::new(class);
        n.bind = bind(f);
        n.dir = dir;
        self.leaf(n)
    }

    /// `count` icons showing a value in half-icon steps (hearts, hunger).
    pub fn icons(&mut self, class: &str, count: u32, f: impl Fn(&ClientView) -> f32 + Send + Sync + 'static) {
        let mut n = node(k::ICONS, S::default());
        n.class = Str::new(class);
        n.count = count;
        n.bind = bind(f);
        self.leaf(n)
    }

    /// Like `icons`, filling from the right (hunger).
    pub fn icons_rtl(&mut self, class: &str, count: u32, f: impl Fn(&ClientView) -> f32 + Send + Sync + 'static) {
        let mut n = node(k::ICONS, S::default());
        n.class = Str::new(class);
        n.count = count;
        n.data = 1;
        n.bind = bind(f);
        self.leaf(n)
    }

    pub fn image(&mut self, texture: &str, w: f32, h: f32) {
        let mut n = node(k::IMAGE, S::default().size(w, h));
        n.text = Str::new(texture);
        self.leaf(n)
    }

    /// The local player's model with what it wears.
    pub fn player_model(&mut self, w: f32, h: f32) {
        self.leaf(node(k::ENTITY, S::default().size(w, h)))
    }

    pub fn spacer(&mut self) {
        self.leaf(node(k::SPACER, S::default().grow()))
    }

    pub fn button(&mut self, key: &str, s: S, f: impl Fn(&ClientView) + Send + Sync + 'static) {
        let mut n = node(k::BUTTON, s);
        n.on_click = callback(move |a: &mut abi::UiArgs| f(&ClientView { raw: a.client }));
        n.class = Str::new("button");
        let key = key.to_string();
        self.with(n, |ui| ui.text(&key));
    }
}

fn bind(f: impl Fn(&ClientView) -> f32 + Send + Sync + 'static) -> abi::Callback {
    callback(move |a: &mut abi::UiBindArgs| {
        a.value = f(&ClientView { raw: a.client });
    })
}

pub struct TextOut {
    raw: *mut abi::UiCtx,
}

impl TextOut {
    pub fn set(&mut self, key: &str, args: &[Arg]) {
        let a = raw_args(args);
        unsafe { (host().ui_text)(self.raw, Str::new(key), Slice::new(&a)) }
    }
}

pub fn stack(item: u32, count: u32) -> ItemStack {
    ItemStack::new(item, count)
}
