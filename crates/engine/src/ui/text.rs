//! Rich text. Formatting lives in the translation, not in code:
//!
//! - `{0}`..`{9}` insert arguments, so each language orders values its own way
//! - `{#rrggbb}` switches color, `{/}` goes back to the default
//! - `{key:core:inventory}` shows the key the player bound to an action
//! - `{item:base:diamond}` draws an item icon inline, `{block:base:furnace[lit=true]}` a block
//!   state (properties not named at their defaults)
//! - `{img:base:arrow}` draws a GUI texture (`gui/arrow`) inline at its own size, `{img:base:arrow 32x16}`
//!   at another one (GUI pixels); a line grows to fit it
//! - `{align:center}` lays the lines after it out centered (`left`, `right`) until `{/align}`, so an
//!   image alone on a centered line is a centered picture
//! - `{link:target}text{/link}` makes a clickable link to another element or screen
//! - `{lang:some.key}` inserts another translation, so shared phrases are written once
//! - `{on_key:core:sneak}text{/on_key}` shows text only while every listed key is held: actions
//!   (respecting rebinding) or raw keys by their settings name, joined with `+`
//!   (`{on_key:ControlLeft+KeyC}`); `!` in front inverts it
//! - `{if:creative}text{/if}` shows text while a condition holds: the engine's (`creative`,
//!   `survival`, `spectator`, `debug`, `dev`) or a mod's (`mymod:charged`); `!` inverts it
//! - `{fx:core:rainbow}text{/fx}` animates the text with an effect shader
//!   (`shaders/fx/<name>.wgsl` in any mod or resource pack)
//! - `{{` is a literal brace
//!
//! Conditional parts stay in the spans and are hidden at layout time, so a text lays out again
//! when a condition changes without being formatted again.

use super::font::Font;
use super::lang::Lang;
use crate::input::{Input, key_name};

#[derive(Clone, Debug, PartialEq)]
pub enum Arg {
    Text(String),
    /// Translated through the language file.
    Key(String),
    Int(i64),
    Float(f64, u8),
    Item(u32),
    /// The display name of something, as rich as its translation.
    Name(Name),
    /// Another translation with arguments of its own.
    Msg(Msg),
}

/// A translation key and its arguments: what the server and mods send for the client to format
/// in its own language (chat, command replies, labels).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Msg {
    pub key: String,
    pub args: Vec<Arg>,
}

impl Msg {
    pub fn new(key: &str, args: Vec<Arg>) -> Msg {
        Msg { key: key.to_string(), args }
    }

    pub fn spans(&self, lang: &Lang, input: Option<&Input>) -> Vec<Span> {
        let mut out = Vec::new();
        message(&self.key, &self.args, &Ctx::new(lang, input), &mut out);
        out
    }

    /// Ready text shown as it is, no translation.
    pub fn plain(text: impl Into<String>) -> Msg {
        Msg { key: String::new(), args: vec![Arg::Text(text.into())] }
    }

    /// As plain text, without icons or formatting.
    pub fn text(&self, lang: &Lang) -> String {
        plain_text(&self.spans(lang, None))
    }
}

/// Something with a display name. Each kind looks for its translations in order and shows its
/// raw name without one; `{name:<kind>:<id>}` in a template is the same.
#[derive(Clone, Debug, PartialEq)]
pub enum Name {
    Item(u32),
    /// A block state.
    Block(u32),
    /// A liquid, by its block's name.
    Liquid(String),
    Tab(String),
    Mod(String),
    Recipe(String),
    Config(String),
    Key(String),
}

/// `prefix.ns.path` for `ns:path`: how a thing's name becomes its translation key.
pub fn lang_key(prefix: &str, name: &str) -> String {
    format!("{prefix}.{}", dotted(name))
}

/// `ns.path` for `ns:path`, the part of a translation key a name gives.
pub fn dotted(name: &str) -> String {
    name.replace(':', ".")
}

impl Name {
    /// `{name:item:base:coal}` and the like.
    pub fn parse(spec: &str) -> Option<Name> {
        let (kind, id) = spec.split_once(':')?;
        let c = crate::content::try_content();
        Some(match kind {
            "item" => Name::Item(c?.item_by_ref(id)?),
            "block" => Name::Block(c?.state_by_name(id)?),
            "liquid" => Name::Liquid(id.to_string()),
            "tab" => Name::Tab(id.to_string()),
            "mod" => Name::Mod(id.to_string()),
            "recipe" => Name::Recipe(id.to_string()),
            "config" => Name::Config(id.to_string()),
            "key" => Name::Key(id.to_string()),
            _ => return None,
        })
    }

    /// The `{name:..}` form, which `parse` reads back.
    pub fn spec(&self) -> String {
        let c = crate::content::try_content();
        match self {
            Name::Item(i) => format!("item:{}", c.and_then(|c| c.items.get(*i as usize)).map_or("", |d| d.name.as_str())),
            Name::Block(s) => format!("block:{}", c.filter(|c| (*s as usize) < c.states.len()).map_or(String::new(), |c| c.state_name(*s))),
            Name::Liquid(n) => format!("liquid:{n}"),
            Name::Tab(n) => format!("tab:{n}"),
            Name::Mod(n) => format!("mod:{n}"),
            Name::Recipe(n) => format!("recipe:{n}"),
            Name::Config(n) => format!("config:{n}"),
            Name::Key(n) => format!("key:{n}"),
        }
    }

    /// Translation keys to try, in order, and the text shown when none is there.
    pub fn keys(&self) -> (Vec<String>, String) {
        let c = crate::content::try_content();
        match self {
            Name::Item(i) => {
                let n = c.and_then(|c| c.items.get(*i as usize)).map_or(String::new(), |d| d.name.clone());
                (vec![lang_key("item", &n), lang_key("block", &n)], n)
            }
            Name::Block(s) => {
                let n = c.and_then(|c| c.states.get(*s as usize).map(|_| c.block_of(*s).name.clone())).unwrap_or_default();
                (vec![lang_key("block", &n), lang_key("item", &n)], n)
            }
            Name::Liquid(n) => (vec![lang_key("block", n)], n.clone()),
            // A mod's own tab, made for items in none, is called like the mod.
            Name::Tab(n) => (vec![lang_key("tab", n), format!("mod.{}.name", n.split(':').next().unwrap_or(n))], n.clone()),
            Name::Mod(id) => (vec![format!("mod.{id}.name")], id.clone()),
            Name::Recipe(n) => (vec![lang_key("recipe", n)], n.clone()),
            Name::Config(n) => (vec![lang_key("config", n)], n.clone()),
            Name::Key(n) => (vec![lang_key("key", n)], n.clone()),
        }
    }

    /// The translation key that exists, if any.
    pub fn key(&self, lang: &Lang) -> Option<String> {
        self.keys().0.into_iter().find(|k| lang.has(k))
    }

    /// As plain text (searching, sorting).
    pub fn text(&self, lang: &Lang) -> String {
        plain_text(&self.spans(lang, None))
    }

    pub fn spans(&self, lang: &Lang, input: Option<&Input>) -> Vec<Span> {
        let mut out = Vec::new();
        format("{0}", &[Arg::Name(self.clone())], &Ctx::new(lang, input), &mut out);
        out
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Piece {
    Text(String),
    Icon(u32),
    /// A block state, drawn like a block item.
    Block(u32),
    /// A GUI texture (`image_id`), its size in font pixels.
    Image(u16, f32, f32),
    LinkStart(String),
    LinkEnd,
}

impl Default for Piece {
    fn default() -> Self {
        Piece::Text(String::new())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Test {
    /// Actions (names with a namespace) or raw keys, all held.
    Keys(Vec<String>),
    If(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cond {
    pub not: bool,
    pub test: Test,
}

#[derive(Clone, Debug, Default)]
pub struct Span {
    pub piece: Piece,
    /// `None` = element default color.
    pub color: Option<u32>,
    /// Text effect (`fx_id`), 0 for none.
    pub fx: u16,
    /// Shown only while all of these hold.
    pub when: Vec<Cond>,
    /// Set by `refresh` from `when`.
    pub hidden: bool,
    /// `align::LEFT`, `CENTER` or `RIGHT` for the lines this span starts.
    pub align: u8,
    /// Every glyph in a cell as wide as a digit (`{mono}`), so changing numbers don't move what
    /// follows.
    pub mono: bool,
}

pub mod align {
    pub const LEFT: u8 = 0;
    pub const CENTER: u8 = 1;
    pub const RIGHT: u8 = 2;
}

/// Marks a block state among the item icons a text queues.
pub const STATE_ICON: u32 = 1 << 31;

/// Font pixels per GUI pixel at the usual text size, for image sizes given in GUI pixels.
pub const FONT_PER_GUI: f32 = 2.0;

/// `hidden` is derived state: spans with the same content are equal whatever it says.
impl PartialEq for Span {
    fn eq(&self, o: &Span) -> bool {
        self.piece == o.piece && self.color == o.color && self.fx == o.fx && self.when == o.when && self.align == o.align && self.mono == o.mono
    }
}

impl Span {
    pub fn text(s: impl Into<String>, color: Option<u32>) -> Span {
        Span { piece: Piece::Text(s.into()), color, ..Default::default() }
    }
}

static FX: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Session-wide number of an effect name (from 1), so spans stay small and the renderer maps it
/// to whatever its shader has.
pub fn fx_id(name: &str) -> u16 {
    let mut v = FX.lock().unwrap();
    match v.iter().position(|n| n == name) {
        Some(i) => i as u16 + 1,
        None => {
            v.push(name.to_string());
            v.len() as u16
        }
    }
}

pub fn fx_name(id: u16) -> Option<String> {
    FX.lock().unwrap().get((id as usize).checked_sub(1)?).cloned()
}

static IMAGES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Session-wide number of an inline image's texture name, so placed text stays `Copy`.
pub fn image_id(name: &str) -> u16 {
    let mut v = IMAGES.lock().unwrap();
    match v.iter().position(|n| n == name) {
        Some(i) => i as u16,
        None => {
            v.push(name.to_string());
            v.len() as u16 - 1
        }
    }
}

pub fn image_name(id: u16) -> Option<String> {
    IMAGES.lock().unwrap().get(id as usize).cloned()
}

/// What formatting reads: the language and the keys bound (for `{key:..}` and `{on_key:..}`).
/// Names of content come from the loaded content.
pub struct Ctx<'a> {
    pub lang: &'a Lang,
    pub input: Option<&'a Input>,
}

impl<'a> Ctx<'a> {
    pub fn new(lang: &'a Lang, input: Option<&'a Input>) -> Ctx<'a> {
        Ctx { lang, input }
    }
}

pub fn format(template: &str, args: &[Arg], cx: &Ctx, out: &mut Vec<Span>) {
    let mut st = State { color: None, fx: 0, when: Vec::new(), align: align::LEFT, mono: false };
    format_depth(template, args, cx, out, &mut st, None, 0)
}

/// A translation `key` with `args`; an empty key shows its one argument as it is (ready text
/// from code, like a ticket or a profile line).
pub fn message(key: &str, args: &[Arg], cx: &Ctx, out: &mut Vec<Span>) {
    if key.is_empty() {
        format("{0}", args, cx, out);
    } else {
        format(cx.lang.get(key), args, cx, out);
    }
}

/// A template without arguments: a translation, a chat line, a pack's description, text a mod
/// draws. Item names in it resolve against the loaded content.
pub fn rich(template: &str, lang: &Lang, input: Option<&Input>) -> Vec<Span> {
    let mut out = Vec::new();
    format(template, &[], &Ctx::new(lang, input), &mut out);
    out
}

/// `s` as a template that shows exactly `s`: player-typed text (chat, world names) goes through
/// the same formatting as everything else without being able to format.
pub fn escape(s: &str) -> String {
    s.replace('{', "{{").replace('}', "}}")
}

struct State {
    color: Option<u32>,
    fx: u16,
    when: Vec<Cond>,
    align: u8,
    mono: bool,
}

impl State {
    fn span(&self, piece: Piece, color: Option<u32>) -> Span {
        Span { piece, color, fx: self.fx, when: self.when.clone(), hidden: false, align: self.align, mono: self.mono }
    }
}

/// `ns:name` under `gui/`; a name leaving the folder is logged once and shows nothing.
fn gui_texture(name: &str) -> String {
    let (ns, path) = name.split_once(':').unwrap_or(("core", name));
    if path.is_empty() || path.starts_with('/') || path.split('/').any(|p| p == ".." || p.is_empty()) || path.starts_with("gui/") {
        static SAID: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        let mut said = SAID.lock().unwrap();
        if !said.iter().any(|s| s == name) {
            log::error!("{{img:{name}}}: names are inside gui/, without the folder");
            said.push(name.to_string());
        }
        return String::new();
    }
    format!("{ns}:gui/{path}")
}

fn texture_size(name: &str) -> Option<(u32, u32)> {
    crate::assets::texture_size(name)
}

fn push_text(out: &mut Vec<Span>, s: &str, st: &State) {
    if s.is_empty() {
        return;
    }
    if let Some(last) = out.last_mut() {
        if last.color == st.color && last.fx == st.fx && last.when == st.when && last.align == st.align && last.mono == st.mono {
            if let Piece::Text(t) = &mut last.piece {
                t.push_str(s);
                return;
            }
        }
    }
    out.push(st.span(Piece::Text(s.to_string()), st.color));
}

fn cond(spec: &str, keys: bool) -> Cond {
    let (not, rest) = match spec.strip_prefix('!') {
        Some(r) => (true, r),
        None => (false, spec),
    };
    let test = if keys { Test::Keys(rest.split('+').map(|k| k.trim().to_string()).collect()) } else { Test::If(rest.trim().to_string()) };
    Cond { not, test }
}

fn format_depth(template: &str, args: &[Arg], cx: &Ctx, out: &mut Vec<Span>, st: &mut State, base: Option<u32>, depth: u32) {
    st.color = base;
    let mut rest = template;
    while let Some(i) = rest.find(['{', '}']) {
        push_text(out, &rest[..i], st);
        let tail = &rest[i..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            push_text(out, &tail[..1], st);
            rest = &tail[2..];
            continue;
        }
        if tail.starts_with('}') {
            push_text(out, "}", st);
            rest = &tail[1..];
            continue;
        }
        let Some(end) = tail.find('}') else {
            push_text(out, tail, st);
            return;
        };
        let tag = &tail[1..end];
        rest = &tail[end + 1..];
        if let Some(hex) = tag.strip_prefix('#') {
            st.color = u32::from_str_radix(hex, 16).ok().or(st.color);
        } else if tag == "/" {
            st.color = base;
        } else if let Some(action) = tag.strip_prefix("key:") {
            let name = cx.input.and_then(|i| i.key_of(action)).map(|k| display_key(cx.lang, k)).unwrap_or_else(|| "?".into());
            let color = Some(st.color.unwrap_or(0xFFE070));
            out.push(st.span(Piece::Text(format!("[{name}]")), color));
        } else if let Some(item) = tag.strip_prefix("item:") {
            if let Some(id) = crate::content::try_content().and_then(|c| c.item_by_ref(item)) {
                out.push(st.span(Piece::Icon(id), None));
            }
        } else if let Some(spec) = tag.strip_prefix("name:") {
            match Name::parse(spec) {
                Some(n) => push_name(&n, cx, out, st, depth),
                None => push_text(out, spec, st),
            }
        } else if let Some(spec) = tag.strip_prefix("block:") {
            let state = crate::content::try_content().and_then(|c| c.state_by_name(spec));
            if let Some(s) = state {
                out.push(st.span(Piece::Block(s), None));
            }
        } else if let Some(spec) = tag.strip_prefix("img:") {
            let mut parts = spec.split_whitespace();
            // A GUI texture: `base:arrow` is `base:gui/arrow`, like every asset name in a call.
            let name = &gui_texture(parts.next().unwrap_or(""));
            let size = parts.next().and_then(|s| s.split_once('x')).and_then(|(w, h)| Some((w.parse::<f32>().ok()?, h.parse::<f32>().ok()?)));
            if let Some((w, h)) = size.or_else(|| texture_size(name).map(|(w, h)| (w as f32, h as f32))) {
                out.push(st.span(Piece::Image(image_id(name), w * FONT_PER_GUI, h * FONT_PER_GUI), None));
            }
        } else if let Some(a) = tag.strip_prefix("align:") {
            st.align = match a.trim() {
                "center" => align::CENTER,
                "right" => align::RIGHT,
                _ => align::LEFT,
            };
        } else if tag == "/align" {
            st.align = align::LEFT;
        } else if let Some(target) = tag.strip_prefix("link:") {
            out.push(st.span(Piece::LinkStart(target.to_string()), None));
            st.color = Some(st.color.unwrap_or(0x80B0FF));
        } else if let Some(key) = tag.strip_prefix("lang:") {
            if depth < 4 {
                let color = st.color;
                format_depth(cx.lang.get(key), args, cx, out, st, color, depth + 1);
                st.color = color;
            }
        } else if tag == "/link" {
            out.push(st.span(Piece::LinkEnd, None));
            st.color = base;
        } else if let Some(spec) = tag.strip_prefix("on_key:") {
            st.when.push(cond(spec, true));
        } else if let Some(spec) = tag.strip_prefix("if:") {
            st.when.push(cond(spec, false));
        } else if tag == "/on_key" || tag == "/if" {
            st.when.pop();
        } else if let Some(name) = tag.strip_prefix("fx:") {
            st.fx = fx_id(name);
        } else if tag == "/fx" {
            st.fx = 0;
        } else if tag == "mono" {
            st.mono = true;
        } else if tag == "/mono" {
            st.mono = false;
        } else if let Ok(n) = tag.parse::<usize>() {
            match args.get(n) {
                Some(Arg::Text(s)) => push_text(out, s, st),
                Some(Arg::Int(v)) => push_text(out, &v.to_string(), st),
                Some(Arg::Float(v, p)) => push_text(out, &format!("{v:.*}", *p as usize), st),
                Some(Arg::Item(id)) => out.push(st.span(Piece::Icon(*id), None)),
                Some(Arg::Name(n)) => push_name(n, cx, out, st, depth),
                Some(Arg::Msg(m)) if depth < 4 => {
                    let color = st.color;
                    format_depth(cx.lang.get(&m.key), &m.args, cx, out, st, color, depth + 1);
                    st.color = color;
                }
                Some(Arg::Key(k)) if depth < 4 => {
                    let color = st.color;
                    format_depth(cx.lang.get(k), &[], cx, out, st, color, depth + 1);
                    st.color = color;
                }
                _ => push_text(out, &format!("{{{tag}}}"), st),
            }
        } else {
            push_text(out, &format!("{{{tag}}}"), st);
        }
    }
    push_text(out, rest, st);
}

/// A name, formatted from its translation (colors and effects kept) or its raw text.
fn push_name(n: &Name, cx: &Ctx, out: &mut Vec<Span>, st: &mut State, depth: u32) {
    match n.key(cx.lang) {
        Some(k) if depth < 4 => {
            let color = st.color;
            format_depth(cx.lang.get(&k), &[], cx, out, st, color, depth + 1);
            st.color = color;
        }
        _ => push_text(out, &n.keys().1, st),
    }
}

/// Re-evaluates conditional spans; `test` answers a condition without its `!`. True if anything
/// was shown or hidden.
pub fn refresh(spans: &mut [Span], test: &dyn Fn(&Test) -> bool) -> bool {
    let mut changed = false;
    for s in spans.iter_mut().filter(|s| !s.when.is_empty()) {
        let hidden = !s.when.iter().all(|c| test(&c.test) != c.not);
        changed |= hidden != s.hidden;
        s.hidden = hidden;
    }
    changed
}

// Positioned output in unscaled font pixels relative to the text box origin.
#[derive(Clone, Copy, Debug)]
pub enum Placed {
    Glyph { x: f32, y: f32, ch: char, color: Option<u32>, fx: u16 },
    Icon { x: f32, y: f32, item: u32 },
    Block { x: f32, y: f32, state: u32 },
    Image { x: f32, y: f32, w: f32, h: f32, image: u16 },
}

impl Placed {
    fn pos(&mut self) -> (&mut f32, &mut f32) {
        match self {
            Placed::Glyph { x, y, .. } | Placed::Icon { x, y, .. } | Placed::Block { x, y, .. } | Placed::Image { x, y, .. } => (x, y),
        }
    }
}

#[derive(Default, Clone)]
pub struct TextLayout {
    pub items: Vec<Placed>,
    pub width: f32,
    pub height: f32,
    pub lines: u32,
    /// (link target, x0, y0, x1, y1) regions.
    pub links: Vec<(String, [f32; 4])>,
}

const ICON: f32 = 16.0;

/// Lays out spans, wrapping at spaces when a line would exceed `max_width` (font pixels).
pub fn layout(spans: &[Span], font: &Font, max_width: Option<f32>) -> TextLayout {
    layout_with(spans, font, max_width, font.line_height, false)
}

/// `layout` with lines `line_height` font pixels apart (tooltips leave room between lines).
pub fn layout_spaced(spans: &[Span], font: &Font, max_width: Option<f32>, line_height: f32) -> TextLayout {
    layout_with(spans, font, max_width, line_height, false)
}

/// With `mono` every glyph takes a cell as wide as a digit (two for wide characters), so changing
/// numbers do not shift the text after them (debug overlay).
pub fn layout_with(spans: &[Span], font: &Font, max_width: Option<f32>, line_height: f32, mono: bool) -> TextLayout {
    let lh = line_height;
    let mut out = TextLayout::default();
    let (mut x, mut y) = (0.0f32, 0.0f32);
    let mut line_start = 0usize;
    let mut last_break: Option<(usize, f32)> = None;
    let mut link: Option<(String, f32, f32)> = None;
    let mut max_x = 0.0f32;
    let limit = max_width.unwrap_or(f32::INFINITY).max(8.0);
    // Height of the current line (an image makes it taller) and every line's (start, width, align).
    let mut line_h = lh;
    let mut lines: Vec<(usize, f32, u8)> = Vec::new();
    let mut line_align = spans.iter().find(|s| !s.hidden).map_or(align::LEFT, |s| s.align);

    let newline = |out: &mut TextLayout, x: &mut f32, y: &mut f32, max_x: &mut f32, line_start: &mut usize, line_h: &mut f32, lines: &mut Vec<(usize, f32, u8)>, align: u8| {
        lines.push((*line_start, *x, align));
        *max_x = max_x.max(*x);
        *x = 0.0;
        *y += *line_h;
        *line_h = lh;
        *line_start = out.items.len();
    };

    for span in spans.iter().filter(|s| !s.hidden) {
        if x == 0.0 && out.items.len() == line_start {
            line_align = span.align;
        }
        match &span.piece {
            Piece::Text(t) => {
                for ch in t.chars() {
                    if ch == '\n' {
                        newline(&mut out, &mut x, &mut y, &mut max_x, &mut line_start, &mut line_h, &mut lines, line_align);
                        line_align = span.align;
                        last_break = None;
                        continue;
                    }
                    let adv = font.glyph(ch).advance as f32;
                    if x + adv > limit && x > 0.0 {
                        if ch == ' ' {
                            // Wrapping right at a space: the space just disappears.
                            newline(&mut out, &mut x, &mut y, &mut max_x, &mut line_start, &mut line_h, &mut lines, line_align);
                            last_break = None;
                            continue;
                        }
                        match last_break.take() {
                            Some((idx, bx)) if idx > line_start => {
                                // Move everything after the space to the next line.
                                lines.push((line_start, bx, line_align));
                                max_x = max_x.max(bx);
                                y += line_h;
                                line_h = lh;
                                let shift = out.items.get_mut(idx + 1).map_or(x, |p| *p.pos().0);
                                out.items.remove(idx);
                                for it in &mut out.items[idx..] {
                                    if let Placed::Image { h, .. } = it {
                                        line_h = line_h.max(*h + 1.0);
                                    }
                                    let (ix, iy) = it.pos();
                                    *ix -= shift;
                                    *iy = y;
                                }
                                x -= shift;
                                line_start = idx;
                            }
                            _ => newline(&mut out, &mut x, &mut y, &mut max_x, &mut line_start, &mut line_h, &mut lines, line_align),
                        }
                    }
                    if ch == ' ' {
                        if x == 0.0 && out.items.len() == line_start && y > 0.0 {
                            continue;
                        }
                        last_break = Some((out.items.len(), x));
                    }
                    if mono || span.mono {
                        let cell = if adv > font.mono + 1.0 { font.mono * 2.0 } else { font.mono };
                        out.items.push(Placed::Glyph { x: x + ((cell - adv) / (2.0 * font.pixel)).floor() * font.pixel, y, ch, color: span.color, fx: span.fx });
                        x += cell;
                    } else {
                        out.items.push(Placed::Glyph { x, y, ch, color: span.color, fx: span.fx });
                        x += adv;
                    }
                }
            }
            Piece::Icon(_) | Piece::Block(_) => {
                if x + ICON > limit && x > 0.0 {
                    newline(&mut out, &mut x, &mut y, &mut max_x, &mut line_start, &mut line_h, &mut lines, line_align);
                }
                out.items.push(match span.piece {
                    Piece::Block(state) => Placed::Block { x, y, state },
                    Piece::Icon(item) => Placed::Icon { x, y, item },
                    _ => unreachable!(),
                });
                x += ICON + 1.0;
            }
            &Piece::Image(image, w, h) => {
                if x + w > limit && x > 0.0 {
                    newline(&mut out, &mut x, &mut y, &mut max_x, &mut line_start, &mut line_h, &mut lines, line_align);
                }
                out.items.push(Placed::Image { x, y, w, h, image });
                line_h = line_h.max(h + 1.0);
                x += w + 1.0;
            }
            Piece::LinkStart(target) => link = Some((target.clone(), x, y)),
            Piece::LinkEnd => {
                if let Some((t, x0, y0)) = link.take() {
                    out.links.push((t, [x0, y0, x.max(x0 + 1.0), y + lh]));
                }
            }
        }
    }
    lines.push((line_start, x, line_align));
    max_x = max_x.max(x);
    out.width = max_x;
    out.height = y + font.line_height.max(line_h - (lh - font.line_height));
    out.lines = lines.len() as u32;
    // Aligned lines move within the wrapping width, or within the widest line.
    let room = max_width.filter(|w| w.is_finite()).unwrap_or(max_x).max(max_x);
    for (i, &(start, width, a)) in lines.iter().enumerate() {
        if a == align::LEFT {
            continue;
        }
        let end = lines.get(i + 1).map_or(out.items.len(), |l| l.0);
        let shift = if a == align::CENTER { ((room - width) / 2.0).floor() } else { room - width };
        for it in &mut out.items[start..end] {
            *it.pos().0 += shift;
        }
        if max_width.is_some() {
            out.width = out.width.max(room);
        }
    }
    out
}

/// A key's name for players: translated (`keyname.ShiftLeft`) when the language has it.
pub fn display_key(lang: &Lang, k: crate::input::Key) -> String {
    let raw = key_name(k);
    let tr = format!("keyname.{raw}");
    if lang.has(&tr) { lang.get(&tr).to_string() } else { raw }
}

pub fn plain(s: &str) -> Vec<Span> {
    vec![Span::text(s, None)]
}

/// The text of spans, without icons or formatting.
pub fn plain_text(spans: &[Span]) -> String {
    spans.iter().filter_map(|s| if let Piece::Text(t) = &s.piece { Some(t.as_str()) } else { None }).collect()
}

/// Every inline image a layout draws, for loading their textures before drawing.
pub fn images(layout: &TextLayout) -> impl Iterator<Item = u16> + '_ {
    layout.items.iter().filter_map(|p| if let Placed::Image { image, .. } = p { Some(*image) } else { None })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_fall_back_to_the_raw_name() {
        let lang = Lang::load(&crate::assets::Assets::new([], Vec::new()), "en_us", false);
        assert_eq!(Name::parse("mod:base"), Some(Name::Mod("base".into())));
        assert_eq!(Name::parse("key:core:jump"), Some(Name::Key("core:jump".into())));
        assert_eq!(Name::Mod("base".into()).text(&lang), "base");
        let mut spans = Vec::new();
        format("[{name:recipe:base:smelting}]", &[], &Ctx::new(&lang, None), &mut spans);
        assert_eq!(plain_text(&spans), "[base:smelting]");
        assert_eq!(lang_key("config", "mymod:sparks"), "config.mymod.sparks");
    }

    #[test]
    fn conditions_and_effects() {
        let lang = Lang::load(&crate::assets::Assets::new([], Vec::new()), "en_us", false);
        let cx = Ctx::new(&lang, None);
        let mut spans = Vec::new();
        format("a{on_key:!core:sneak}b{if:creative}c{/if}{/on_key}{fx:core:wave}d{/fx}e", &[], &cx, &mut spans);
        let text = |s: &Span| match &s.piece {
            Piece::Text(t) => t.clone(),
            _ => String::new(),
        };
        assert_eq!(spans.iter().map(text).collect::<Vec<_>>(), ["a", "b", "c", "d", "e"]);
        assert_eq!((spans[2].when.len(), spans[3].fx, spans[4].fx), (2, fx_id("core:wave"), 0));
        // Sneak held, not creative: only the unconditional parts and the effect remain.
        let font = Font::parse_hex(include_str!("../../assets/fonts/unifont.hex"));
        assert!(refresh(&mut spans, &|t| matches!(t, Test::Keys(_))));
        let shown: String = layout(&spans, &font, None).items.iter().filter_map(|i| if let Placed::Glyph { ch, .. } = i { Some(*ch) } else { None }).collect();
        assert_eq!(shown, "ade");
        assert!(refresh(&mut spans, &|t| matches!(t, Test::If(_))));
        assert!(!refresh(&mut spans, &|t| matches!(t, Test::If(_))));
        assert_eq!(spans.iter().filter(|s| !s.hidden).count(), 5);
    }

    /// An image on a centered line sits in the middle and makes its line taller.
    #[test]
    fn images_and_alignment() {
        let lang = Lang::load(&crate::assets::Assets::new([], Vec::new()), "en_us", false);
        let cx = Ctx::new(&lang, None);
        let mut spans = Vec::new();
        format("{align:center}{img:base:x 16x12}{/align}
ab", &[], &cx, &mut spans);
        let font = Font::parse_hex(include_str!("../../assets/fonts/unifont.hex"));
        let l = layout(&spans, &font, Some(100.0));
        let (ix, iw, ih) = l.items.iter().find_map(|p| if let Placed::Image { x, w, h, .. } = p { Some((*x, *w, *h)) } else { None }).unwrap();
        assert_eq!((iw, ih), (16.0 * FONT_PER_GUI, 12.0 * FONT_PER_GUI));
        assert_eq!(ix, ((100.0 - (iw + 1.0)) / 2.0).floor());
        let text_y = l.items.iter().find_map(|p| if let Placed::Glyph { y, .. } = p { Some(*y) } else { None }).unwrap();
        assert_eq!(text_y, ih + 1.0);
        assert_eq!(l.lines, 2);
        // Escaped player text shows its braces instead of formatting.
        let mut lit = Vec::new();
        format(&escape("{img:x} {#ff0000}"), &[], &cx, &mut lit);
        assert_eq!(lit, plain("{img:x} {#ff0000}"));
    }

    #[test]
    fn wrapping_never_loses_or_panics() {
        let font = Font::parse_hex(include_str!("../../assets/fonts/unifont.hex"));
        let text = "Cubes: 5583 loaded, 0 meshing, 725 draws  double  spaces and a_very_long_word_without_breaks";
        let inked = text.chars().filter(|c| *c != ' ').count();
        for width in [0.0, 8.0, 13.0, 40.0, 100.0, 1000.0] {
            let l = layout(&plain(text), &font, Some(width));
            let glyphs = l.items.iter().filter(|i| matches!(i, Placed::Glyph { ch, .. } if *ch != ' ')).count();
            assert_eq!(glyphs, inked, "width {width}");
        }
    }
}
