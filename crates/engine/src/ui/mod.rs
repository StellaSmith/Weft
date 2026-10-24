//! Retained UI: a tree rebuilt when a screen opens (or the language/theme/scale changes), bound
//! values polled every frame, geometry regenerated only when something actually changed.
//!
//! Layout is a small flexbox: rows/columns, padding, gap, grow, alignment. Sizes are in GUI pixels
//! and multiplied by the GUI scale when drawn. Text never overflows its box: it wraps, or shrinks
//! when wrapping is off.

pub mod edit;
pub mod font;
pub mod lang;
pub mod render;
pub mod text;
pub mod theme;

use std::rc::Rc;

use glam::Vec2;
use text::{Arg, Span, TextLayout};

use crate::app::Ctx;

pub type NodeId = usize;
pub type Action = Rc<dyn Fn(&mut Ctx)>;
pub type TextBind = Box<dyn Fn(&Ctx) -> (String, Vec<Arg>)>;
pub type FloatBind = Box<dyn Fn(&Ctx) -> f32>;
pub type ItemBind = Box<dyn Fn(&Ctx) -> (u32, u32)>;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Len {
    Auto,
    Px(f32),
    Pct(f32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Start,
    Center,
    End,
    Stretch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Justify {
    Start,
    Center,
    End,
    Between,
}

#[derive(Clone, Copy, Debug)]
pub struct Style {
    pub row: bool,
    pub pad: [f32; 4],
    pub gap: f32,
    pub w: Len,
    pub h: Len,
    pub min: [f32; 2],
    pub max_w: f32,
    pub grow: f32,
    pub align: Align,
    pub justify: Justify,
    pub self_align: Option<Align>,
    /// Children scroll inside this node (mouse wheel) and are clipped to it.
    pub scroll: bool,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            row: false,
            pad: [0.0; 4],
            gap: 0.0,
            w: Len::Auto,
            h: Len::Auto,
            min: [0.0; 2],
            max_w: f32::INFINITY,
            grow: 0.0,
            align: Align::Start,
            justify: Justify::Start,
            self_align: None,
            scroll: false,
        }
    }
}

impl Style {
    pub fn row() -> Style {
        Style { row: true, align: Align::Center, ..Default::default() }
    }
    pub fn col() -> Style {
        Style::default()
    }
    pub fn pad(mut self, p: f32) -> Self {
        self.pad = [p; 4];
        self
    }
    pub fn pad_xy(mut self, x: f32, y: f32) -> Self {
        self.pad = [x, y, x, y];
        self
    }
    pub fn gap(mut self, g: f32) -> Self {
        self.gap = g;
        self
    }
    pub fn w(mut self, w: f32) -> Self {
        self.w = Len::Px(w);
        self
    }
    pub fn h(mut self, h: f32) -> Self {
        self.h = Len::Px(h);
        self
    }
    pub fn size(self, w: f32, h: f32) -> Self {
        self.w(w).h(h)
    }
    pub fn w_pct(mut self, p: f32) -> Self {
        self.w = Len::Pct(p);
        self
    }
    pub fn h_pct(mut self, p: f32) -> Self {
        self.h = Len::Pct(p);
        self
    }
    pub fn min_w(mut self, w: f32) -> Self {
        self.min[0] = w;
        self
    }
    pub fn min_h(mut self, h: f32) -> Self {
        self.min[1] = h;
        self
    }
    pub fn max_w(mut self, w: f32) -> Self {
        self.max_w = w;
        self
    }
    pub fn grow(mut self) -> Self {
        self.grow = 1.0;
        self
    }
    pub fn center(mut self) -> Self {
        self.align = Align::Center;
        self.justify = Justify::Center;
        self
    }
    pub fn align(mut self, a: Align) -> Self {
        self.align = a;
        self
    }
    pub fn justify(mut self, j: Justify) -> Self {
        self.justify = j;
        self
    }
    pub fn scroll(mut self) -> Self {
        self.scroll = true;
        self
    }
    pub fn stretch(mut self) -> Self {
        self.align = Align::Stretch;
        self
    }
    pub fn self_align(mut self, a: Align) -> Self {
        self.self_align = Some(a);
        self
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn intersect(&self, o: Rect) -> Rect {
        let (x0, y0) = (self.x.max(o.x), self.y.max(o.y));
        let (x1, y1) = ((self.x + self.w).min(o.x + o.w), (self.y + self.h).min(o.y + o.h));
        Rect { x: x0, y: y0, w: (x1 - x0).max(0.0), h: (y1 - y0).max(0.0) }
    }

    pub fn contains(&self, p: Vec2) -> bool {
        p.x >= self.x && p.y >= self.y && p.x < self.x + self.w && p.y < self.y + self.h
    }
}

pub struct TextNode {
    pub spans: Vec<Span>,
    pub bind: Option<TextBind>,
    pub last: Option<(String, Vec<Arg>)>,
    pub layout: TextLayout,
    pub wrap: bool,
    /// Fixed-width glyphs, so changing numbers do not move the text after them.
    pub mono: bool,
    pub scale: f32,
    pub color: Option<u32>,
    pub center: bool,
    /// Theme class for the default colour.
    pub class: String,
}

pub enum Kind {
    Container { class: Option<String> },
    Text(TextNode),
    Button { class: String, action: Option<Action>, enabled: bool, selected: bool },
    Image { texture: String, color: u32 },
    Item { bind: ItemBind, last: (u32, u32) },
    /// `dir`: `abi::ui_dir` the bar fills towards.
    Progress { class: String, bind: FloatBind, value: f32, dir: u32 },
    /// `count` icons showing `value` in half steps (hearts): theme `class.full/half/empty`.
    Icons { class: String, count: u32, bind: FloatBind, value: f32, rtl: bool },
    /// Single-line text field; `caret` is a char index.
    Input { value: String, caret: usize, max: usize, on_change: Option<Rc<dyn Fn(&mut Ctx, &str)>> },
    Slider { value: f32, bind: FloatBind, on_change: Rc<dyn Fn(&mut Ctx, f32)> },
    /// Engine-drawn content (slots, 3D previews) identified by a tag.
    Custom { tag: u32, data: u64 },
}

pub struct Node {
    pub kind: Kind,
    pub style: Style,
    pub children: Vec<NodeId>,
    pub rect: Rect,
    pub id: Option<String>,
    pub tooltip: Option<Vec<Span>>,
    /// GUI texture drawn by custom nodes that want one (the silhouette of an empty slot).
    pub icon: Option<String>,
    measured: [f32; 2],
    /// Visible part when inside a scrolling node, GUI units.
    pub clip: Option<Rect>,
}

pub struct Ui {
    pub nodes: Vec<Node>,
    stack: Vec<NodeId>,
    pub hovered: Option<NodeId>,
    pub pressed: Option<NodeId>,
    pub focused: Option<NodeId>,
    /// Selection end in the focused text field, the caret being the other.
    pub anchor: Option<usize>,
    pub dirty: bool,
    /// Scroll offsets by node id (or index), kept across rebuilds.
    scroll: rustc_hash::FxHashMap<String, f32>,
    pub size: Vec2,
    /// GUI pixels per font pixel, chosen by the caller so glyphs land on whole physical pixels.
    pub text_unit: f32,
    /// When it opened (`shaders::clock`), for `open` animations.
    pub shown: f32,
}

impl Default for Ui {
    fn default() -> Self {
        Ui::new()
    }
}

impl Kind {
    pub fn input(value: &str, max: usize) -> Kind {
        Kind::Input { value: value.to_string(), caret: value.chars().count(), max, on_change: None }
    }
}

impl Ui {
    pub fn new() -> Ui {
        let root = Node {
            kind: Kind::Container { class: None },
            style: Style::default().center(),
            children: Vec::new(),
            rect: Rect::default(),
            id: None,
            tooltip: None,
            icon: None,
            measured: [0.0; 2],
            clip: None,
        };
        Ui {
            nodes: vec![root],
            stack: vec![0],
            hovered: None,
            pressed: None,
            focused: None,
            anchor: None,
            dirty: true,
            scroll: Default::default(),
            size: Vec2::ZERO,
            text_unit: 0.5,
            shown: crate::shaders::clock(),
        }
    }

    pub fn clear(&mut self) {
        let (scroll, shown, size) = (std::mem::take(&mut self.scroll), self.shown, self.size);
        *self = Ui::new();
        (self.scroll, self.shown, self.size) = (scroll, shown, size);
    }

    fn scroll_key(&self, id: NodeId) -> String {
        self.nodes[id].id.clone().unwrap_or_else(|| format!("#{id}"))
    }

    /// Scrolls the innermost scrolling node under `p` by `lines` wheel steps; true if one moved.
    pub fn scroll_at(&mut self, p: Vec2, lines: f32) -> bool {
        let Some(id) = (0..self.nodes.len()).rev().find(|&i| self.nodes[i].style.scroll && self.nodes[i].rect.contains(p) && self.nodes[i].clip.is_none_or(|c| c.contains(p))) else {
            return false;
        };
        let key = self.scroll_key(id);
        let v = self.scroll.entry(key).or_default();
        *v = (*v - lines * 20.0).max(0.0);
        self.dirty = true;
        true
    }

    pub fn root_style(&mut self, s: Style) {
        self.nodes[0].style = s;
    }

    pub fn add(&mut self, kind: Kind, style: Style) -> NodeId {
        let id = self.nodes.len();
        self.nodes.push(Node { kind, style, children: Vec::new(), rect: Rect::default(), id: None, tooltip: None, icon: None, measured: [0.0; 2], clip: None });
        let parent = *self.stack.last().unwrap();
        self.nodes[parent].children.push(id);
        self.dirty = true;
        id
    }

    pub fn open(&mut self, kind: Kind, style: Style) -> NodeId {
        let id = self.add(kind, style);
        self.stack.push(id);
        id
    }

    /// Makes an already added node the parent for following adds (until `close`).
    pub fn open_existing(&mut self, id: NodeId) {
        self.stack.push(id);
    }

    pub fn close(&mut self) {
        if self.stack.len() > 1 {
            self.stack.pop();
        }
    }

    pub fn with(&mut self, kind: Kind, style: Style, f: impl FnOnce(&mut Ui)) -> NodeId {
        let id = self.open(kind, style);
        f(self);
        self.close();
        id
    }

    pub fn container(&mut self, style: Style, f: impl FnOnce(&mut Ui)) -> NodeId {
        self.with(Kind::Container { class: None }, style, f)
    }

    pub fn panel(&mut self, class: &str, style: Style, f: impl FnOnce(&mut Ui)) -> NodeId {
        self.with(Kind::Container { class: Some(class.to_string()) }, style, f)
    }

    pub fn spacer(&mut self) -> NodeId {
        self.add(Kind::Container { class: None }, Style::default().grow())
    }

    pub fn text(&mut self, spans: Vec<Span>, class: &str) -> NodeId {
        self.add(
            Kind::Text(TextNode {
                spans,
                bind: None,
                last: None,
                layout: TextLayout::default(),
                wrap: true,
                mono: false,
                scale: 1.0,
                color: None,
                center: false,
                class: class.to_string(),
            }),
            Style::default(),
        )
    }

    pub fn text_bound(&mut self, class: &str, bind: TextBind) -> NodeId {
        self.add(
            Kind::Text(TextNode {
                spans: Vec::new(),
                bind: Some(bind),
                last: None,
                layout: TextLayout::default(),
                wrap: true,
                mono: false,
                scale: 1.0,
                color: None,
                center: false,
                class: class.to_string(),
            }),
            Style::default(),
        )
    }

    pub fn button(&mut self, class: &str, style: Style, action: Action, f: impl FnOnce(&mut Ui)) -> NodeId {
        self.with(Kind::Button { class: class.to_string(), action: Some(action), enabled: true, selected: false }, style.center(), f)
    }

    pub fn text_mut(&mut self, id: NodeId) -> Option<&mut TextNode> {
        match &mut self.nodes[id].kind {
            Kind::Text(t) => Some(t),
            _ => None,
        }
    }

    pub fn set_id(&mut self, node: NodeId, id: &str) {
        self.nodes[node].id = Some(id.to_string());
    }

    pub fn find(&self, id: &str) -> Option<NodeId> {
        self.nodes.iter().position(|n| n.id.as_deref() == Some(id))
    }

    // ------------------------------------------------------------ layout

    pub fn layout(&mut self, size: Vec2, font: &font::Font) {
        self.size = size;
        self.measure(0, [size.x, size.y], font);
        self.arrange(0, Rect { x: 0.0, y: 0.0, w: size.x, h: size.y }, font);
        self.assign_clips(0, None);
    }

    fn assign_clips(&mut self, id: NodeId, clip: Option<Rect>) {
        self.nodes[id].clip = clip;
        let inner = if self.nodes[id].style.scroll {
            let r = self.nodes[id].rect;
            let p = self.nodes[id].style.pad;
            let own = Rect { x: r.x + p[0], y: r.y + p[1], w: (r.w - p[0] - p[2]).max(0.0), h: (r.h - p[1] - p[3]).max(0.0) };
            Some(match clip {
                Some(c) => c.intersect(own),
                None => own,
            })
        } else {
            clip
        };
        for c in self.nodes[id].children.clone() {
            self.assign_clips(c, inner);
        }
    }

    fn resolve(len: Len, avail: f32) -> Option<f32> {
        match len {
            Len::Auto => None,
            Len::Px(v) => Some(v),
            Len::Pct(p) => Some(avail * p / 100.0),
        }
    }

    fn measure(&mut self, id: NodeId, avail: [f32; 2], font: &font::Font) -> [f32; 2] {
        let style = self.nodes[id].style;
        let fixed_w = Self::resolve(style.w, avail[0]);
        let fixed_h = Self::resolve(style.h, avail[1]);
        let pad_w = style.pad[0] + style.pad[2];
        let pad_h = style.pad[1] + style.pad[3];
        let inner_avail = [fixed_w.unwrap_or(avail[0]).min(style.max_w) - pad_w, fixed_h.unwrap_or(avail[1]) - pad_h];

        let unit = self.text_unit;
        let content = if let Kind::Text(t) = &mut self.nodes[id].kind {
            let max = if t.wrap { Some(inner_avail[0].max(8.0) / (t.scale * unit)) } else { None };
            t.layout = text::layout_with(&t.spans, font, max, font.line_height, t.mono);
            [t.layout.width * t.scale * unit, t.layout.height * t.scale * unit]
        } else {
            match &self.nodes[id].kind {
                Kind::Item { .. } => [16.0, 16.0],
                Kind::Icons { count, .. } => [*count as f32 * 8.0 + 1.0, 9.0],
                Kind::Slider { .. } => [100.0, 12.0],
                Kind::Input { .. } => [120.0, 18.0],
                _ => {
                    let children = self.nodes[id].children.clone();
                    let (mut main, mut cross) = (0.0f32, 0.0f32);
                    for (i, c) in children.iter().enumerate() {
                        let m = self.measure(*c, inner_avail, font);
                        let (cm, cc) = if style.row { (m[0], m[1]) } else { (m[1], m[0]) };
                        main += cm + if i > 0 { style.gap } else { 0.0 };
                        cross = cross.max(cc);
                    }
                    if style.row { [main, cross] } else { [cross, main] }
                }
            }
        };
        let w = fixed_w.unwrap_or(content[0] + pad_w).max(style.min[0]).min(style.max_w);
        let h = fixed_h.unwrap_or(content[1] + pad_h).max(style.min[1]);
        self.nodes[id].measured = [w, h];
        [w, h]
    }

    fn arrange(&mut self, id: NodeId, rect: Rect, font: &font::Font) {
        self.nodes[id].rect = rect;
        let style = self.nodes[id].style;
        let inner = Rect {
            x: rect.x + style.pad[0],
            y: rect.y + style.pad[1],
            w: (rect.w - style.pad[0] - style.pad[2]).max(0.0),
            h: (rect.h - style.pad[1] - style.pad[3]).max(0.0),
        };
        let unit = self.text_unit;
        if let Kind::Text(t) = &mut self.nodes[id].kind {
            // Final fit: wrap to the box we actually got, or shrink if wrapping is off.
            if t.layout.width * t.scale * unit > inner.w + 0.5 {
                if t.wrap {
                    t.layout = text::layout_with(&t.spans, font, Some(inner.w / (t.scale * unit)), font.line_height, t.mono);
                } else if t.layout.width > 0.0 {
                    t.scale = (inner.w / (t.layout.width * unit)).max(0.25);
                }
            }
            return;
        }
        let children = self.nodes[id].children.clone();
        if children.is_empty() {
            return;
        }
        let (main_size, cross_size) = if style.row { (inner.w, inner.h) } else { (inner.h, inner.w) };
        let mut sizes: Vec<[f32; 2]> = children.iter().map(|&c| self.nodes[c].measured).collect();
        let main_of = |s: &[f32; 2]| if style.row { s[0] } else { s[1] };
        let gaps = style.gap * (children.len() as f32 - 1.0);
        let total: f32 = sizes.iter().map(main_of).sum::<f32>() + gaps;
        let mut free = main_size - total;
        let grow_sum: f32 = children.iter().map(|&c| self.nodes[c].style.grow).sum();
        if free > 0.0 && grow_sum > 0.0 {
            for (i, &c) in children.iter().enumerate() {
                let g = self.nodes[c].style.grow;
                if g > 0.0 {
                    let add = free * g / grow_sum;
                    if style.row { sizes[i][0] += add } else { sizes[i][1] += add }
                }
            }
            free = 0.0;
        } else if free < 0.0 && style.row {
            // Too wide: squeeze wrapping text children, they re-measure with less width.
            let texts: Vec<usize> = (0..children.len()).filter(|&i| matches!(self.nodes[children[i]].kind, Kind::Text(_))).collect();
            if !texts.is_empty() {
                let take = -free / texts.len() as f32;
                for i in texts {
                    let c = children[i];
                    let nw = (sizes[i][0] - take).max(16.0);
                    let m = self.measure(c, [nw, cross_size], font);
                    sizes[i] = [nw, m[1]];
                }
                free = 0.0;
            }
        }
        let mut scrolled = 0.0;
        if style.scroll {
            // Content longer than the node scrolls; the offset stays within the content.
            let key = self.scroll_key(id);
            let max = (-free).max(0.0);
            let v = self.scroll.entry(key).or_default();
            *v = v.min(max);
            scrolled = *v;
            free = free.max(0.0);
        }
        let (mut pos, spacing) = match style.justify {
            Justify::Start => (-scrolled, style.gap),
            Justify::Center => (free.max(0.0) / 2.0, style.gap),
            // Overflowing content sticks out at the start, the end edge stays put.
            Justify::End => (free, style.gap),
            Justify::Between if children.len() > 1 => (0.0, style.gap + free.max(0.0) / (children.len() - 1) as f32),
            Justify::Between => (free.max(0.0) / 2.0, style.gap),
        };
        for (i, &c) in children.iter().enumerate() {
            let s = sizes[i];
            let (cm, mut cc) = if style.row { (s[0], s[1]) } else { (s[1], s[0]) };
            let align = self.nodes[c].style.self_align.unwrap_or(style.align);
            let fixed_cross = if style.row { self.nodes[c].style.h != Len::Auto } else { self.nodes[c].style.w != Len::Auto };
            if align == Align::Stretch && !fixed_cross {
                cc = cross_size;
            }
            let off = match align {
                Align::Start | Align::Stretch => 0.0,
                Align::Center => (cross_size - cc) / 2.0,
                Align::End => cross_size - cc,
            };
            let r = if style.row {
                Rect { x: inner.x + pos, y: inner.y + off, w: cm, h: cc }
            } else {
                Rect { x: inner.x + off, y: inner.y + pos, w: cc, h: cm }
            };
            // Stretched text must re-wrap to the new width.
            if cc != (if style.row { s[1] } else { s[0] }) {
                self.measure(c, [r.w, r.h], font);
            }
            self.arrange(c, r, font);
            pos += cm + spacing;
        }
    }

    /// Deepest interactive node under the cursor.
    pub fn hit(&self, p: Vec2) -> Option<NodeId> {
        fn walk(ui: &Ui, id: NodeId, p: Vec2, best: &mut Option<NodeId>) {
            let n = &ui.nodes[id];
            if (!n.rect.contains(p) || n.clip.is_some_and(|c| !c.contains(p))) && id != 0 {
                return;
            }
            let interactive = matches!(n.kind, Kind::Button { .. } | Kind::Input { .. } | Kind::Slider { .. } | Kind::Custom { .. })
                || n.tooltip.is_some()
                || matches!(&n.kind, Kind::Text(t) if !t.layout.links.is_empty());
            if interactive && id != 0 {
                *best = Some(id);
            }
            // What a button contains (labels, icons) is part of the button.
            if matches!(n.kind, Kind::Button { .. }) && id != 0 {
                return;
            }
            for &c in &n.children {
                walk(ui, c, p, best);
            }
        }
        let mut best = None;
        walk(self, 0, p, &mut best);
        best
    }
}
