//! Rich text. Formatting lives in the translation, not in code:
//!
//! - `{0}`..`{9}` insert arguments, so each language orders values its own way
//! - `{#rrggbb}` switches colour, `{/}` goes back to the default
//! - `{key:core:inventory}` shows the key the player bound to an action
//! - `{item:base:diamond}` draws an item icon inline
//! - `{link:target}text{/link}` makes a clickable link to another element or screen
//! - `{{` is a literal brace

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

#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub piece: Piece,
    /// `None` = element default colour.
    pub color: Option<u32>,
}

pub struct Ctx<'a> {
    pub lang: &'a Lang,
    pub input: Option<&'a Input>,
    pub item_name: &'a dyn Fn(&str) -> Option<u32>,
}

pub fn format(template: &str, args: &[Arg], cx: &Ctx, out: &mut Vec<Span>) {
    format_depth(template, args, cx, out, None, 0)
}

fn push_text(out: &mut Vec<Span>, s: &str, color: Option<u32>) {
    if s.is_empty() {
        return;
    }
    if let Some(Span { piece: Piece::Text(t), color: c }) = out.last_mut() {
        if *c == color {
            t.push_str(s);
            return;
        }
    }
    out.push(Span { piece: Piece::Text(s.to_string()), color });
}

fn format_depth(template: &str, args: &[Arg], cx: &Ctx, out: &mut Vec<Span>, base: Option<u32>, depth: u32) {
    let mut color = base;
    let mut rest = template;
    while let Some(i) = rest.find(['{', '}']) {
        push_text(out, &rest[..i], color);
        let tail = &rest[i..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            push_text(out, &tail[..1], color);
            rest = &tail[2..];
            continue;
        }
        if tail.starts_with('}') {
            push_text(out, "}", color);
            rest = &tail[1..];
            continue;
        }
        let Some(end) = tail.find('}') else {
            push_text(out, tail, color);
            return;
        };
        let tag = &tail[1..end];
        rest = &tail[end + 1..];
        if let Some(hex) = tag.strip_prefix('#') {
            color = u32::from_str_radix(hex, 16).ok().or(color);
        } else if tag == "/" {
            color = base;
        } else if let Some(action) = tag.strip_prefix("key:") {
            let name = cx.input.and_then(|i| i.key_of(action)).map(|k| display_key(cx.lang, k)).unwrap_or_else(|| "?".into());
            push_text(out, &format!("[{name}]"), Some(color.unwrap_or(0xFFE070)));
        } else if let Some(item) = tag.strip_prefix("item:") {
            if let Some(id) = (cx.item_name)(item) {
                out.push(Span { piece: Piece::Icon(id), color: None });
            }
        } else if let Some(target) = tag.strip_prefix("link:") {
            out.push(Span { piece: Piece::LinkStart(target.to_string()), color: None });
            color = Some(color.unwrap_or(0x80B0FF));
        } else if tag == "/link" {
            out.push(Span { piece: Piece::LinkEnd, color: None });
            color = base;
        } else if let Ok(n) = tag.parse::<usize>() {
            match args.get(n) {
                Some(Arg::Text(s)) => push_text(out, s, color),
                Some(Arg::Int(v)) => push_text(out, &v.to_string(), color),
                Some(Arg::Float(v, p)) => push_text(out, &format!("{v:.*}", *p as usize), color),
                Some(Arg::Item(id)) => out.push(Span { piece: Piece::Icon(*id), color: None }),
                Some(Arg::Key(k)) if depth < 4 => format_depth(cx.lang.get(k), &[], cx, out, color, depth + 1),
                _ => push_text(out, &format!("{{{tag}}}"), color),
            }
        } else {
            push_text(out, &format!("{{{tag}}}"), color);
        }
    }
    push_text(out, rest, color);
}

/// Positioned output in unscaled font pixels relative to the text box origin.
#[derive(Clone, Copy, Debug)]
pub enum Placed {
    Glyph { x: f32, y: f32, ch: char, color: Option<u32> },
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
    let lh = font.line_height;
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

    for span in spans {
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
                    out.items.push(Placed::Glyph { x, y, ch, color: span.color });
                    x += adv;
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
    out.height = y + lh;
    out.lines = (out.height / lh).round() as u32;
    out
}

/// A key's name for players: translated (`keyname.ShiftLeft`) when the language has it.
pub fn display_key(lang: &Lang, k: crate::input::Key) -> String {
    let raw = key_name(k);
    let tr = format!("keyname.{raw}");
    if lang.has(&tr) { lang.get(&tr).to_string() } else { raw }
}

pub fn plain(s: &str) -> Vec<Span> {
    vec![Span { piece: Piece::Text(s.to_string()), color: None }]
}

#[cfg(test)]
mod tests {
    use super::*;

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
