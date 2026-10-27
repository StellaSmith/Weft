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
    /// Own inventory slot by index.
    pub fn inventory_slot(&self, slot: u32) -> ItemStack {
        unsafe { (host().client_slot)(self.raw, 0, slot) }
    }
    /// The stacks of a group of the own inventory (`hotbar`, `core:equipment`...).
    pub fn inventory_group(&self, name: &str) -> Vec<ItemStack> {
        let mut out = [0u32; 2];
        if unsafe { (host().player_group)(Str::new(name), &mut out) } == 0 {
            return Vec::new();
        }
        (out[0]..out[0] + out[1]).map(|i| self.inventory_slot(i)).collect()
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
    /// Shown as it is: never read as a translation or formatting.
    Text(&'a str),
    Key(&'a str),
    Int(i64),
    /// A number with this many decimals.
    Float(f64, u32),
    Item(u32),
    /// A display name, as `{name:..}` writes it: `"item:base:coal"`, `"mod:base"`,
    /// `"block:base:furnace"`, `"key:core:jump"`, `"recipe:base:smelting"`.
    Name(&'a str),
}

pub(crate) fn raw_args<'a>(args: &'a [Arg<'a>]) -> Vec<abi::UiArg> {
    args.iter()
        .map(|a| match a {
            Arg::Text(s) => abi::UiArg { kind: abi::ui_arg::TEXT, text: Str::new(s), ..abi::UiArg::DEFAULT },
            Arg::Key(s) => abi::UiArg { kind: abi::ui_arg::KEY, text: Str::new(s), ..abi::UiArg::DEFAULT },
            Arg::Int(v) => abi::UiArg { kind: abi::ui_arg::INT, num: *v as f64, ..abi::UiArg::DEFAULT },
            Arg::Float(v, digits) => abi::UiArg { kind: abi::ui_arg::FLOAT, num: *v, digits: *digits, ..abi::UiArg::DEFAULT },
            Arg::Item(v) => abi::UiArg { kind: abi::ui_arg::ITEM, num: *v as f64, ..abi::UiArg::DEFAULT },
            Arg::Name(s) => abi::UiArg { kind: abi::ui_arg::NAME, text: Str::new(s), ..abi::UiArg::DEFAULT },
        })
        .collect()
}

/// How a part is laid out and shown: the same properties the engine's own screens use. Sizes are
/// GUI pixels; `None` = fit content.
#[derive(Clone)]
pub struct S {
    pub width: Option<f32>,
    pub height: Option<f32>,
    /// Percent of the parent instead of pixels.
    pub width_pct: Option<f32>,
    pub height_pct: Option<f32>,
    pub min: [f32; 2],
    pub max_width: f32,
    pub grow: f32,
    pub gap: f32,
    pub pad: f32,
    pub row: bool,
    pub align: u32,
    pub justify: u32,
    /// This part's own `ui_align` in its parent.
    pub self_align: Option<u32>,
    /// `abi::ui_flags`.
    pub flags: u32,
    /// Text size, 1 = normal.
    pub scale: f32,
    /// Style class (`title`, `dim`, `button`, `chat`...), empty for the kind's own.
    pub class: &'static str,
    /// Translation shown while the mouse is over the part, empty for none.
    pub tooltip: String,
    tooltip_args: Vec<ArgBuf>,
}

/// An argument kept until the node is sent.
#[derive(Clone)]
enum ArgBuf {
    Text(String, u32),
    Num(f64, u32, u32),
}

impl Default for S {
    fn default() -> Self {
        S {
            width: None,
            height: None,
            width_pct: None,
            height_pct: None,
            min: [0.0; 2],
            max_width: f32::INFINITY,
            grow: 0.0,
            gap: 0.0,
            pad: 0.0,
            row: false,
            align: abi::ui_align::START,
            justify: abi::ui_justify::START,
            self_align: None,
            flags: 0,
            scale: 1.0,
            class: "",
            tooltip: String::new(),
            tooltip_args: Vec::new(),
        }
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
    pub fn tooltip(self, key: &str) -> S {
        self.tooltip_args(key, &[])
    }
    pub fn tooltip_args(mut self, key: &str, args: &[Arg]) -> S {
        self.tooltip = key.to_string();
        self.tooltip_args = raw_args(args)
            .iter()
            .zip(args)
            .map(|(r, a)| match a {
                Arg::Text(t) | Arg::Key(t) | Arg::Name(t) => ArgBuf::Text(t.to_string(), r.kind),
                _ => ArgBuf::Num(r.num, r.digits, r.kind),
            })
            .collect();
        self
    }
    pub fn class(mut self, class: &'static str) -> S {
        self.class = class;
        self
    }
    pub fn h(mut self, h: f32) -> S {
        self.height = Some(h);
        self
    }
    pub fn w_pct(mut self, p: f32) -> S {
        self.width_pct = Some(p);
        self
    }
    pub fn h_pct(mut self, p: f32) -> S {
        self.height_pct = Some(p);
        self
    }
    pub fn min_w(mut self, w: f32) -> S {
        self.min[0] = w;
        self
    }
    pub fn min_h(mut self, h: f32) -> S {
        self.min[1] = h;
        self
    }
    pub fn max_w(mut self, w: f32) -> S {
        self.max_width = w;
        self
    }
    /// Its own place across its parent (`abi::ui_align`).
    pub fn self_align(mut self, a: u32) -> S {
        self.self_align = Some(a);
        self
    }
    /// Children scroll (mouse wheel) and are clipped to the part, with a bar when longer.
    pub fn scroll(mut self) -> S {
        self.flags |= abi::ui_flags::SCROLL;
        self
    }
    /// Text on one line.
    pub fn no_wrap(mut self) -> S {
        self.flags |= abi::ui_flags::NO_WRAP;
        self
    }
    /// Text with every glyph in a digit-wide cell, so changing numbers don't move what follows
    /// (`{mono}..{/mono}` does it for a part of a text).
    pub fn mono(mut self) -> S {
        self.flags |= abi::ui_flags::MONO;
        self
    }
    /// Text size (2 for a screen's title).
    pub fn scale(mut self, s: f32) -> S {
        self.scale = s;
        self
    }
    /// A button that can't be pressed now.
    pub fn disabled(mut self, yes: bool) -> S {
        if yes {
            self.flags |= abi::ui_flags::DISABLED;
        }
        self
    }
    /// A button shown as the chosen one (the open tab).
    pub fn selected(mut self, yes: bool) -> S {
        if yes {
            self.flags |= abi::ui_flags::SELECTED;
        }
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

/// A node with what it points at, alive until it is sent.
struct Node {
    raw: abi::UiNode,
    _s: S,
    _tip: Vec<abi::UiArg>,
}

impl std::ops::Deref for Node {
    type Target = abi::UiNode;
    fn deref(&self) -> &abi::UiNode {
        &self.raw
    }
}

impl std::ops::DerefMut for Node {
    fn deref_mut(&mut self) -> &mut abi::UiNode {
        &mut self.raw
    }
}

fn node(kind: u32, s: S) -> Node {
    let tip: Vec<abi::UiArg> = s
        .tooltip_args
        .iter()
        .map(|a| match a {
            ArgBuf::Text(t, kind) => abi::UiArg { kind: *kind, text: Str::new(t), ..abi::UiArg::DEFAULT },
            ArgBuf::Num(v, digits, kind) => abi::UiArg { kind: *kind, num: *v, digits: *digits, ..abi::UiArg::DEFAULT },
        })
        .collect();
    let raw = abi::UiNode {
        kind,
        class: Str::new(s.class),
        width: s.width.unwrap_or(f32::NAN),
        height: s.height.unwrap_or(f32::NAN),
        width_pct: s.width_pct.unwrap_or(f32::NAN),
        height_pct: s.height_pct.unwrap_or(f32::NAN),
        min: s.min,
        max_width: s.max_width,
        self_align: s.self_align.unwrap_or(u32::MAX),
        flags: s.flags,
        scale: s.scale,
        grow: s.grow,
        gap: s.gap,
        pad: [s.pad; 4],
        row: s.row as u32,
        align: s.align,
        justify: s.justify,
        data: 0,
        tooltip: Str::new(&s.tooltip),
        tooltip_args: Slice::new(&tip),
        ..abi::UiNode::DEFAULT
    };
    Node { raw, _s: s, _tip: tip }
}

impl Ui {
    pub fn client(&self) -> ClientView {
        self.client
    }

    fn open(&mut self, n: &Node) {
        unsafe { (host().ui_open)(self.raw, &n.raw) }
    }

    fn close(&mut self) {
        unsafe { (host().ui_close)(self.raw) }
    }

    fn with(&mut self, n: Node, f: impl FnOnce(&mut Ui)) {
        self.open(&n);
        f(self);
        self.close();
    }

    fn leaf(&mut self, n: Node) {
        self.open(&n);
        self.close();
    }

    pub fn column(&mut self, s: S, f: impl FnOnce(&mut Ui)) {
        self.with(node(k::CONTAINER, s), f)
    }

    pub fn row(&mut self, s: S, f: impl FnOnce(&mut Ui)) {
        self.with(node(k::CONTAINER, S { row: true, ..s }), f)
    }

    /// A background of class `class` (`gui/<class>.png`, e.g. "panel").
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
        self.text_styled(S::default(), key, args)
    }

    /// Text with a class, size, one line, `mono`...: `ui.text_styled(S::col().class("title").scale(2.0), "mymod.title", &[])`.
    pub fn text_styled(&mut self, s: S, key: &str, args: &[Arg]) {
        let a = raw_args(args);
        let mut n = node(k::TEXT, s);
        n.text = Str::new(key);
        n.args = Slice::new(&a);
        self.leaf(n)
    }

    /// Text recomputed every frame; redrawn only when it changes.
    pub fn text_bind(&mut self, f: impl Fn(&ClientView, &mut TextOut) + Send + Sync + 'static) {
        self.text_bind_styled(S::default(), f)
    }

    pub fn text_bind_styled(&mut self, s: S, f: impl Fn(&ClientView, &mut TextOut) + Send + Sync + 'static) {
        let mut n = node(k::TEXT, s);
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
                ui.text("menu.inventory");
            }
            ui.column(S::col().gap(4.0), |ui| {
                ui.group(crate::game::INVENTORY_GROUP);
                ui.group(crate::game::HOTBAR_GROUP);
            });
        });
    }

    /// The open menu's title: what a player named the machine or chest, else the name of the
    /// block it was opened on, else the translation `menu.<ns>.<name>`.
    pub fn title(&mut self) {
        self.leaf(node(k::TITLE, S::default()))
    }

    /// The player's equipment slots of a menu with `.equipment()`, in a column.
    pub fn equipment(&mut self) {
        self.leaf(node(k::EQUIPMENT, S::default()))
    }

    /// The arrow of a menu that makes recipes (`.recipes(..)` on the menu or machine): clicking
    /// it opens them in the recipe viewer. Empty, for a crafting grid.
    pub fn recipe_arrow(&mut self) {
        self.leaf(node(k::RECIPE_ARROW, S::default()))
    }

    /// `recipe_arrow` showing progress (a furnace's cooking).
    pub fn recipe_arrow_bind(&mut self, f: impl Fn(&ClientView) -> f32 + Send + Sync + 'static) {
        let mut n = node(k::RECIPE_ARROW, S::default());
        n.bind = bind(f);
        self.leaf(n)
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
        self.button_args(key, &[], s, f)
    }

    /// A button whose label has arguments (`settings.gui_scale` with the scale).
    pub fn button_args(&mut self, key: &str, args: &[Arg], s: S, f: impl Fn(&ClientView) + Send + Sync + 'static) {
        let mut n = node(k::BUTTON, s);
        n.on_click = callback(move |a: &mut abi::UiArgs| f(&ClientView { raw: a.client }));
        if n.class.len == 0 {
            n.class = Str::new("button");
        }
        self.with(n, |ui| ui.text_styled(S::col().class("button").no_wrap(), key, args));
    }

    /// A one-line text field starting with `value` (at most `max` characters, 0 for any);
    /// `on_change` gets the text after every edit.
    pub fn input(&mut self, value: &str, max: u32, s: S, on_change: impl Fn(&ClientView, &str) + Send + Sync + 'static) {
        let mut n = node(k::INPUT, s);
        n.text = Str::new(value);
        n.count = max;
        n.on_change = callback(move |a: &mut abi::UiChangeArgs| on_change(&ClientView { raw: a.client }, unsafe { a.text.as_str() }));
        self.leaf(n)
    }

    /// A slider at `value` (0..1, read every frame); `on_change` gets where the player drags it.
    pub fn slider(&mut self, s: S, value: impl Fn(&ClientView) -> f32 + Send + Sync + 'static, on_change: impl Fn(&ClientView, f32) + Send + Sync + 'static) {
        let mut n = node(k::SLIDER, s);
        n.bind = bind(value);
        n.on_change = callback(move |a: &mut abi::UiChangeArgs| on_change(&ClientView { raw: a.client }, a.value));
        self.leaf(n)
    }

    /// An item's icon, with `count` under it when above 1.
    pub fn item(&mut self, item: u32, count: u32, s: S) {
        let mut n = node(k::ITEM, s);
        n.data = item;
        n.count = count;
        self.leaf(n)
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

    /// Ready text, shown as it is (not a translation).
    pub fn text(&mut self, text: &str) {
        self.set("", &[Arg::Text(text)])
    }
}

pub fn stack(item: u32, count: u32) -> ItemStack {
    ItemStack::new(item, count)
}
