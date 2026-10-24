//! Rich text. Formatting lives in the translation, not in code:
//!
//! - `{0}`..`{9}` insert arguments, so each language orders values its own way
//! - `{#rrggbb}` switches colour, `{/}` goes back to the default
//! - `{key:core:inventory}` shows the key the player bound to an action
//! - `{item:base:diamond}` draws an item icon inline
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
}

#[derive(Clone, Debug, PartialEq)]
pub enum Piece {
    Text(String),
    Icon(u32),
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
    /// `None` = element default colour.
    pub color: Option<u32>,
    /// Text effect (`fx_id`), 0 for none.
    pub fx: u16,
    /// Shown only while all of these hold.
    pub when: Vec<Cond>,
    /// Set by `refresh` from `when`.
    pub hidden: bool,
}

/// `hidden` is derived state: spans with the same content are equal whatever it says.
impl PartialEq for Span {
    fn eq(&self, o: &Span) -> bool {
        self.piece == o.piece && self.color == o.color && self.fx == o.fx && self.when == o.when
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

pub struct Ctx<'a> {
    pub lang: &'a Lang,
    pub input: Option<&'a Input>,
    pub item_name: &'a dyn Fn(&str) -> Option<u32>,
}

pub fn format(template: &str, args: &[Arg], cx: &Ctx, out: &mut Vec<Span>) {
    let mut st = State { color: None, fx: 0, when: Vec::new() };
    format_depth(template, args, cx, out, &mut st, None, 0)
}

struct State {
    color: Option<u32>,
    fx: u16,
    when: Vec<Cond>,
}

impl State {
    fn span(&self, piece: Piece, color: Option<u32>) -> Span {
        Span { piece, color, fx: self.fx, when: self.when.clone(), hidden: false }
    }
}

fn push_text(out: &mut Vec<Span>, s: &str, st: &State) {
    if s.is_empty() {
        return;
    }
    if let Some(last) = out.last_mut() {
        if last.color == st.color && last.fx == st.fx && last.when == st.when {
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
            if let Some(id) = (cx.item_name)(item) {
                out.push(st.span(Piece::Icon(id), None));
            }
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
        } else if let Ok(n) = tag.parse::<usize>() {
            match args.get(n) {
                Some(Arg::Text(s)) => push_text(out, s, st),
                Some(Arg::Int(v)) => push_text(out, &v.to_string(), st),
                Some(Arg::Float(v, p)) => push_text(out, &format!("{v:.*}", *p as usize), st),
                Some(Arg::Item(id)) => out.push(st.span(Piece::Icon(*id), None)),
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

    let newline = |out: &mut TextLayout, x: &mut f32, y: &mut f32, max_x: &mut f32, line_start: &mut usize| {
        *max_x = max_x.max(*x);
        *x = 0.0;
        *y += lh;
        *line_start = out.items.len();
    };

    for span in spans.iter().filter(|s| !s.hidden) {
        match &span.piece {
            Piece::Text(t) => {
                for ch in t.chars() {
                    if ch == '\n' {
                        newline(&mut out, &mut x, &mut y, &mut max_x, &mut line_start);
                        last_break = None;
                        continue;
                    }
                    let adv = font.glyph(ch).advance as f32;
                    if x + adv > limit && x > 0.0 {
                        if ch == ' ' {
                            // Wrapping right at a space: the space just disappears.
                            newline(&mut out, &mut x, &mut y, &mut max_x, &mut line_start);
                            last_break = None;
                            continue;
                        }
                        match last_break.take() {
                            Some((idx, bx)) if idx > line_start => {
                                // Move everything after the space to the next line.
                                max_x = max_x.max(bx);
                                y += lh;
                                let shift = match out.items.get(idx + 1) {
                                    Some(Placed::Glyph { x, .. }) | Some(Placed::Icon { x, .. }) => *x,
                                    None => x,
                                };
                                out.items.remove(idx);
                                for it in &mut out.items[idx..] {
                                    match it {
                                        Placed::Glyph { x, y: gy, .. } | Placed::Icon { x, y: gy, .. } => {
                                            *x -= shift;
                                            *gy = y;
                                        }
                                    }
                                }
                                x -= shift;
                                line_start = idx;
                            }
                            _ => newline(&mut out, &mut x, &mut y, &mut max_x, &mut line_start),
                        }
                    }
                    if ch == ' ' {
                        if x == 0.0 && out.items.len() == line_start && y > 0.0 {
                            continue;
                        }
                        last_break = Some((out.items.len(), x));
                    }
                    if mono {
                        let cell = if adv > font.mono + 1.0 { font.mono * 2.0 } else { font.mono };
                        out.items.push(Placed::Glyph { x: x + ((cell - adv) / (2.0 * font.pixel)).floor() * font.pixel, y, ch, color: span.color, fx: span.fx });
                        x += cell;
                    } else {
                        out.items.push(Placed::Glyph { x, y, ch, color: span.color, fx: span.fx });
                        x += adv;
                    }
                }
            }
            Piece::Icon(item) => {
                if x + ICON > limit && x > 0.0 {
                    newline(&mut out, &mut x, &mut y, &mut max_x, &mut line_start);
                }
                out.items.push(Placed::Icon { x, y, item: *item });
                x += ICON + 1.0;
            }
            Piece::LinkStart(target) => link = Some((target.clone(), x, y)),
            Piece::LinkEnd => {
                if let Some((t, x0, y0)) = link.take() {
                    out.links.push((t, [x0, y0, x.max(x0 + 1.0), y + lh]));
                }
            }
        }
    }
    max_x = max_x.max(x);
    out.width = max_x;
    out.height = y + font.line_height;
    out.lines = (y / lh).round() as u32 + 1;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditions_and_effects() {
        let lang = Lang::load(&crate::assets::Assets::new([], Vec::new()), "en_us", false);
        let cx = Ctx { lang: &lang, input: None, item_name: &|_| None };
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
