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
    /// The client's actions (`open_screen`, `open_world`, `quit`..), for a button's callback:
    /// a button may drive the client, a binding that only shows a value may not.
    pub fn client(&self) -> crate::Client {
        crate::Client::from_raw(self.raw)
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
    /// Translation shown while the mouse is over the part, empty for none.
    pub tooltip: &'static str,
}

impl Default for S {
    fn default() -> Self {
        S { width: None, height: None, grow: 0.0, gap: 0.0, pad: 0.0, row: false, align: abi::ui_align::START, justify: abi::ui_justify::START, tooltip: "" }
    }
}

impl S {
    pub fn row() -> S {
        S { row: true, align: abi::ui_align::CENTER, ..S::default() }
    }
    pub fn col() -> S {
        S::default()
    }
    /// A tooltip over the part (a translation key, with all of rich text).
    pub fn tooltip(mut self, key: &'static str) -> S {
        self.tooltip = key;
        self
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

impl Ui {
    pub(crate) fn from_raw(ui: *mut abi::UiCtx, client: *mut abi::ClientCtx) -> Ui {
        Ui { raw: ui, client: ClientView { raw: client } }
    }
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
        tooltip: Str::new(s.tooltip),
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

    /// What mods add to place `area` of `screen` (`Registry::screen_part`). A screen of your own
    /// that replaces an engine one (`core:title`) keeps other mods' buttons by building its places.
    pub fn screen_parts(&mut self, screen: &str, area: &str) {
        let mut n = node(k::PARTS, S::default());
        n.class = Str::new(screen);
        n.text = Str::new(area);
        self.leaf(n)
    }

    /// In a recipe kind's `display`: the recipe's inputs as the kind's grid.
    pub fn recipe_inputs(&mut self) {
        self.recipe_slot_raw(0, u32::MAX)
    }
    pub fn recipe_outputs(&mut self) {
        self.recipe_slot_raw(1, u32::MAX)
    }
    /// Only input `i`, for layouts that place them apart.
    pub fn recipe_input(&mut self, i: u32) {
        self.recipe_slot_raw(0, i)
    }
    pub fn recipe_output(&mut self, i: u32) {
        self.recipe_slot_raw(1, i)
    }

    fn recipe_slot_raw(&mut self, outputs: u32, i: u32) {
        let mut n = node(k::RECIPE_SLOT, S::default());
        n.data = outputs;
        n.slot = i;
        self.leaf(n)
    }

    /// Every slot of the menu's group `name` (`MenuBuilder::group`), laid out as its grid.
    pub fn group(&mut self, name: &str) {
        self.group_slot_raw(name, u32::MAX)
    }

    /// Only slot `i` of group `name`, for layouts that place a group's slots apart.
    pub fn group_slot(&mut self, name: &str, i: u32) {
        self.group_slot_raw(name, i)
    }

    fn group_slot_raw(&mut self, name: &str, i: u32) {
        let mut n = node(k::SLOT, S::default());
        n.class = Str::new(name);
        n.slot = i;
        self.leaf(n)
    }

    /// The player's inventory and hotbar (`MenuBuilder::player_inventory`), spaced like vanilla,
    /// under the title "Inventory" when `title`.
    pub fn player_inventory(&mut self, title: bool) {
        self.column(S::col().gap(2.0), |ui| {
            if title {
                ui.text("container.inventory");
            }
            ui.column(S::col().gap(4.0), |ui| {
                ui.group(crate::game::INVENTORY_GROUP);
                ui.group(crate::game::HOTBAR_GROUP);
            });
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
