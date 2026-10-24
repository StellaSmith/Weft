//! In-game screens: container menus (built by the mod that registered the menu), death, chat.

use std::cell::RefCell;
use std::rc::Rc;

use winit::keyboard::KeyCode;

use crate::app::{Ctx, Request};
use crate::input::Key;
use crate::ui::text::{self, Span};
use crate::client::modui;
use crate::content::content;
use crate::screens::{BuildCx, Screen, button, label};
use crate::ui::{Kind, Style, Ui};

pub struct MenuScreen {
    pub kind: u32,
    /// Creative inventory filter and open tab.
    search: Rc<RefCell<String>>,
    tab: Rc<std::cell::Cell<usize>>,
}

impl MenuScreen {
    pub fn new(kind: u32) -> MenuScreen {
        MenuScreen { kind, search: Default::default(), tab: Default::default() }
    }
}

fn tab_title(cx: &BuildCx, name: &str) -> Vec<Span> {
    let key = format!("tab.{}", name.replace(':', "."));
    if cx.lang.has(&key) {
        return cx.t(&key, &[]);
    }
    // Automatic tab of a mod: its display name.
    let id = name.split(':').next().unwrap_or(name);
    let c = content();
    text::plain(c.mods.iter().find(|m| m.id == id).map_or(id, |m| m.name.as_str()))
}

/// Tab row, search field and the items of the open tab (or of every tab matching the search) as
/// a scrolling grid of creative entries.
fn creative_panel(ui: &mut Ui, cx: &BuildCx, search: &Rc<RefCell<String>>, tab: &Rc<std::cell::Cell<usize>>) {
    let c = content();
    let filter = search.borrow().to_lowercase();
    let open = tab.get().min(c.creative_tabs.len().saturating_sub(1));
    let items: Vec<u32> = if filter.is_empty() {
        c.creative_tabs.get(open).map_or(Vec::new(), |t| t.items.clone())
    } else {
        c.creative_tabs
            .iter()
            .flat_map(|t| t.items.iter().copied())
            .filter(|&i| c.items[i as usize].name.to_lowercase().contains(&filter) || crate::hud::item_name(cx.lang, i).to_lowercase().contains(&filter))
            .collect()
    };
    ui.panel("panel", Style::default().pad(7.0).gap(4.0), |ui| {
        ui.container(Style::row().gap(1.0), |ui| {
            for (i, t) in c.creative_tabs.iter().enumerate() {
                let tab = tab.clone();
                let b = ui.button("button", Style::default().size(20.0, 20.0), Rc::new(move |ctx: &mut Ctx| {
                    tab.set(i);
                    ctx.requests.push(Request::Rebuild);
                }), |ui| {
                    ui.add(Kind::Custom { tag: modui::TAG_ICON, data: t.icon as u64 }, Style::default().size(16.0, 16.0));
                });
                if let Kind::Button { selected, .. } = &mut ui.nodes[b].kind {
                    *selected = i == open && filter.is_empty();
                }
                ui.nodes[b].tooltip = Some(tab_title(cx, &t.name));
            }
        });
        if let Some(t) = c.creative_tabs.get(open).filter(|_| filter.is_empty()) {
            ui.text(tab_title(cx, &t.name), "default");
        }
        let s = search.clone();
        let input = ui.add(
            Kind::Input {
                value: search.borrow().clone(),
                caret: search.borrow().chars().count(),
                max: 32,
                on_change: Some(Rc::new(move |ctx: &mut Ctx, v: &str| {
                    *s.borrow_mut() = v.to_string();
                    ctx.requests.push(Request::Rebuild);
                })),
            },
            Style::default().h(14.0).w(162.0),
        );
        ui.set_id(input, "creative_search");
        let grid = ui.container(Style::default().w(162.0).h(18.0 * 8.0).scroll(), |ui| {
            for row in items.chunks(9) {
                ui.container(Style::row(), |ui| {
                    for &i in row {
                        ui.add(Kind::Custom { tag: modui::TAG_CREATIVE, data: i as u64 }, Style::default().size(18.0, 18.0));
                    }
                });
            }
        });
        ui.set_id(grid, "creative_grid");
    });
}

impl Screen for MenuScreen {
    fn name(&self) -> &str {
        "core:menu"
    }

    fn shows_menu(&self) -> bool {
        true
    }

    fn build(&mut self, ui: &mut Ui, cx: &BuildCx) {
        let c = content();
        ui.root_style(Style::default().stretch());
        let creative = cx.client.is_some_and(|cl| cl.mode == crate::world::GameMode::Creative) && c.game.player_menu == Some(self.kind);
        ui.panel("overlay", Style::default().grow().center(), |ui| {
            ui.container(Style::row().gap(4.0).align(crate::ui::Align::Start), |ui| {
                if creative {
                    creative_panel(ui, cx, &self.search, &self.tab);
                }
                if let Some(def) = c.game.menus.get(self.kind as usize) {
                    if let Some(cb) = def.screen {
                        modui::build(ui, cx.lang, Some(cx.input), def.owner, cb, cx.client);
                    }
                }
            });
        });
    }
}

pub struct Death;

impl Screen for Death {
    fn name(&self) -> &str {
        "core:death"
    }

    fn escape_closes(&self) -> bool {
        false
    }

    fn closes_on_respawn(&self) -> bool {
        true
    }

    fn build(&mut self, ui: &mut Ui, cx: &BuildCx) {
        ui.root_style(Style::default().stretch());
        ui.panel("death", Style::default().grow().center().gap(12.0), |ui| {
            let t = label(ui, cx, "death.title", &[], "title");
            if let Some(t) = ui.text_mut(t) {
                t.scale = 2.0;
                t.wrap = false;
            }
            button(ui, cx, "death.respawn", &[], 200.0, |c| c.requests.push(Request::Respawn));
            button(ui, cx, "menu.save_quit", &[], 200.0, |c| c.requests.push(Request::LeaveWorld));
        });
    }
}

/// Chat input with the recent messages above it and, for commands, completions of the word being
/// typed. Tab takes the highlighted one and further presses go on through the same list; the arrows
/// browse sent lines, or the list once Tab started.
pub struct Chat {
    state: Rc<RefCell<ChatState>>,
}

#[derive(Default)]
struct ChatState {
    text: String,
    completion: Option<crate::commands::Completion>,
    selected: usize,
    /// Position in the sent-lines history while browsing it with the arrows.
    history: Option<usize>,
    /// The text as typed before Tab: further presses go on through its suggestions.
    stem: Option<String>,
}

impl Chat {
    /// Opened with `/` the command list shows at once.
    pub fn new(text: &str) -> Chat {
        let mut st = ChatState::default();
        st.update(text, &[]);
        Chat { state: Rc::new(RefCell::new(st)) }
    }
}

impl ChatState {
    fn update(&mut self, text: &str, players: &[String]) {
        self.text = text.to_string();
        self.selected = 0;
        self.stem = None;
        self.completion = text.strip_prefix('/').map(|line| {
            let mut comp = crate::commands::complete(content(), line, players);
            comp.from += 1;
            comp
        });
    }
}

impl Screen for Chat {
    fn name(&self) -> &str {
        "core:chat"
    }

    fn shows_chat(&self) -> bool {
        true
    }

    fn keys_first(&self) -> bool {
        true
    }

    fn build(&mut self, ui: &mut Ui, cx: &BuildCx) {
        let st = self.state.borrow();
        ui.root_style(Style::default().stretch());
        ui.container(Style::default().grow().pad(4.0).gap(1.0).justify(crate::ui::Justify::End).stretch(), |ui| {
            if let Some(client) = cx.client {
                ui.container(Style::default().gap(1.0).pad(2.0).self_align(crate::ui::Align::Start), |ui| {
                    for (line, _) in client.chat.iter().rev().take(20).rev() {
                        ui.text(text::plain(line), "chat");
                    }
                });
            }
            if let Some(comp) = &st.completion {
                if !comp.suggestions.is_empty() {
                    // A window of ten around the highlighted suggestion.
                    let first = st.selected.saturating_sub(9);
                    ui.panel("suggest", Style::default().pad(2.0).self_align(crate::ui::Align::Start), |ui| {
                        for (i, s) in comp.suggestions.iter().enumerate().skip(first).take(10) {
                            let color = if i == st.selected { 0xFFFF55 } else { 0xA0A0A0 };
                            ui.text(vec![Span::text(s.clone(), Some(color))], "chat");
                        }
                    });
                } else if !comp.hint.is_empty() {
                    ui.container(Style::default().pad(2.0).self_align(crate::ui::Align::Start), |ui| {
                        ui.text(vec![Span::text(comp.hint.clone(), Some(0x808080))], "chat");
                    });
                }
            }
            let state = self.state.clone();
            let input = ui.add(
                Kind::Input {
                    value: st.text.clone(),
                    caret: st.text.chars().count(),
                    max: 256,
                    on_change: Some(Rc::new(move |ctx: &mut Ctx, v: &str| {
                        let players = ctx.client.as_ref().map(|c| c.players.clone()).unwrap_or_default();
                        let mut s = state.borrow_mut();
                        s.update(v, &players);
                        s.history = None;
                        ctx.requests.push(Request::Rebuild);
                    })),
                },
                Style::default().h(14.0),
            );
            ui.set_id(input, "chat");
            ui.focused = Some(input);
        });
    }

    fn key(&mut self, key: Key, ctx: &mut Ctx) -> bool {
        let players = ctx.client.as_ref().map(|c| c.players.clone()).unwrap_or_default();
        let sent = ctx.client.as_ref().map(|c| c.sent.clone()).unwrap_or_default();
        let mut st = self.state.borrow_mut();
        let n = st.completion.as_ref().map_or(0, |c| c.suggestions.len());
        let set = |st: &mut ChatState, text: String, ctx: &mut Ctx| {
            st.update(&text, &players);
            ctx.requests.push(Request::SetInput("chat".into(), text));
            ctx.requests.push(Request::Rebuild);
        };
        // Puts the highlighted suggestion of the stem's list into the input, keeping the list.
        let take = |st: &mut ChatState, ctx: &mut Ctx| {
            let stem = st.stem.get_or_insert_with(|| st.text.clone()).clone();
            let comp = st.completion.as_ref().unwrap();
            let text = format!("{}{}", &stem[..comp.from.min(stem.len())], comp.suggestions[st.selected.min(n - 1)]);
            st.text = text.clone();
            ctx.requests.push(Request::SetInput("chat".into(), text));
            ctx.requests.push(Request::Rebuild);
        };
        match key {
            Key::Code(KeyCode::Tab) if n > 0 => {
                if st.stem.is_some() {
                    st.selected = (st.selected + 1) % n;
                }
                take(&mut st, ctx);
                true
            }
            Key::Code(KeyCode::ArrowUp | KeyCode::ArrowDown) if n > 0 && st.stem.is_some() => {
                let up = key == Key::Code(KeyCode::ArrowUp);
                st.selected = if up { (st.selected + n - 1) % n } else { (st.selected + 1) % n };
                take(&mut st, ctx);
                true
            }
            Key::Code(KeyCode::ArrowUp | KeyCode::ArrowDown) if !sent.is_empty() => {
                let up = key == Key::Code(KeyCode::ArrowUp);
                let at = match (st.history, up) {
                    (None, true) => Some(sent.len() - 1),
                    (None, false) => None,
                    (Some(i), true) => Some(i.saturating_sub(1)),
                    (Some(i), false) => (i + 1 < sent.len()).then_some(i + 1),
                };
                let text = at.map_or(String::new(), |i| sent[i].clone());
                set(&mut st, text, ctx);
                st.history = at;
                true
            }
            _ => false,
        }
    }
}

/// Recipes making an item, or using it, of every recipe type (JEI/NEI style). Opened with the
/// recipe/uses keys over any item in a screen; clicking an item in it shows that item's recipes.
/// Ingredients given by tag cycle through the items carrying it.
pub struct RecipeView {
    pub item: u32,
    pub uses: bool,
}

/// What the cursor points at in a screen: a slot, a creative entry, or an icon (also inside a
/// button, like the recipe viewer's).
pub fn hovered_item(ui: &Ui, client: Option<&crate::client::Client>) -> Option<u32> {
    let h = ui.hovered?;
    let item = |n: usize| match ui.nodes[n].kind {
        Kind::Custom { tag: modui::TAG_SLOT, data } => client.and_then(|c| c.menu.as_ref()).and_then(|m| m.slots.get(data as usize)).map(|s| s.item),
        Kind::Custom { tag: modui::TAG_CREATIVE | modui::TAG_ICON, data } => Some(data as u32),
        Kind::Custom { tag: crate::hud::TAG_HOTBAR, data } => client.and_then(|c| c.inventory.get(data as usize)).map(|s| s.item),
        Kind::Item { last, .. } => Some(last.0),
        _ => None,
    };
    item(h).or_else(|| ui.nodes[h].children.iter().find_map(|&c| item(c))).filter(|&i| i != 0)
}

impl Screen for RecipeView {
    fn name(&self) -> &str {
        "core:recipes"
    }

    fn build(&mut self, ui: &mut Ui, cx: &BuildCx) {
        use crate::registry::Ingredient;
        let c = content();
        let item = self.item;
        let mut found: Vec<(&str, &crate::registry::Recipe)> = Vec::new();
        let mut kinds: Vec<&String> = c.game.recipes.keys().collect();
        kinds.sort();
        for kind in kinds {
            for r in &c.game.recipes[kind] {
                let uses = r.inputs.iter().chain(r.shaped.iter().flat_map(|s| s.2.iter())).any(|i| !matches!(i, Ingredient::Empty) && i.matches(item, &c.components));
                if if self.uses { uses } else { r.output == item } {
                    found.push((kind.as_str(), r));
                }
            }
        }
        found.truncate(200);
        let title = if self.uses { "recipes.uses" } else { "recipes.made_by" };
        let name = crate::hud::item_name(cx.lang, item);
        ui.root_style(Style::default().stretch());
        ui.panel("overlay", Style::default().grow().center().pad(10.0).gap(6.0), |ui| {
            label(ui, cx, title, &[text::Arg::Text(name)], "title");
            if found.is_empty() {
                label(ui, cx, "recipes.none", &[], "dim");
            }
            let list = ui.panel("panel", Style::default().pad(7.0).gap(8.0).w(230.0).h(0.0).grow().min_h(40.0).scroll(), |ui| {
                for (kind, r) in &found {
                    recipe_row(ui, cx, kind, r);
                }
            });
            ui.set_id(list, "recipes");
            button(ui, cx, "menu.back", &[], 160.0, |c| c.requests.push(Request::Close));
        });
    }
}

/// An ingredient or result: a slot showing the item (cycling a tag's items), clicking it shows
/// that item's recipes.
fn recipe_slot(ui: &mut Ui, cx: &BuildCx, ing: &crate::registry::Ingredient, count: u32) {
    use crate::registry::Ingredient;
    let c = content();
    let items: Vec<u32> = match ing {
        Ingredient::Empty => Vec::new(),
        Ingredient::Item(i) => vec![*i],
        Ingredient::Tag(t) => (1..c.items.len() as u32).filter(|&i| c.components.has(crate::components::ITEM, i, *t)).collect(),
    };
    if items.is_empty() {
        ui.add(Kind::Container { class: Some("recipe_slot".into()) }, Style::default().size(18.0, 18.0));
        return;
    }
    let first = items[0];
    let shown = items.clone();
    let b = ui.button("recipe_slot", Style::default().size(18.0, 18.0), Rc::new(move |ctx: &mut Ctx| {
        // The item the slot shows right now.
        let i = shown[(std::time::UNIX_EPOCH.elapsed().map_or(0, |d| d.as_secs()) as usize) % shown.len()];
        ctx.requests.push(Request::Open(Box::new(RecipeView { item: i, uses: false })));
    }), |ui| {
        let list = items.clone();
        ui.add(
            Kind::Item { bind: Box::new(move |_: &Ctx| (list[(std::time::UNIX_EPOCH.elapsed().map_or(0, |d| d.as_secs()) as usize) % list.len()], count)), last: (first, count) },
            Style::default().size(16.0, 16.0),
        );
    });
    let tip = match ing {
        Ingredient::Tag(t) => cx.t("recipes.tag", &[text::Arg::Text(c.components.names[*t as usize].clone())]),
        _ => crate::hud::item_tooltip(cx.lang, cx.input, voxide_abi::ItemStack::new(first, count.max(1))),
    };
    ui.nodes[b].tooltip = Some(tip);
}

fn recipe_row(ui: &mut Ui, cx: &BuildCx, kind: &str, r: &crate::registry::Recipe) {
    use crate::registry::Ingredient;
    let c = content();
    ui.container(Style::default().gap(2.0), |ui| {
        ui.container(Style::row().gap(4.0), |ui| {
            for &station in c.game.recipe_stations.get(kind).map_or(&[][..], |v| v.as_slice()) {
                ui.add(Kind::Custom { tag: modui::TAG_ICON, data: station as u64 }, Style::default().size(12.0, 12.0));
            }
            let key = format!("recipe.{}", kind.replace(':', "."));
            let title = if cx.lang.has(&key) { cx.t(&key, &[]) } else { text::plain(kind) };
            ui.text(title, "default");
        });
        ui.container(Style::row().gap(6.0), |ui| {
            match &r.shaped {
                Some((w, h, cells)) => {
                    ui.container(Style::default().gap(1.0), |ui| {
                        for y in 0..*h {
                            ui.container(Style::row().gap(1.0), |ui| {
                                for x in 0..*w {
                                    recipe_slot(ui, cx, &cells[(y * w + x) as usize], 1);
                                }
                            });
                        }
                    });
                }
                None if r.inputs.len() > 1 => {
                    let w = (r.inputs.len() as f32).sqrt().ceil() as usize;
                    ui.container(Style::default().gap(1.0), |ui| {
                        for row in r.inputs.chunks(w) {
                            ui.container(Style::row().gap(1.0), |ui| {
                                for i in row {
                                    recipe_slot(ui, cx, i, 1);
                                }
                            });
                        }
                    });
                }
                None => recipe_slot(ui, cx, r.inputs.first().unwrap_or(&Ingredient::Empty), 1),
            }
            ui.container(Style::default().gap(1.0).center(), |ui| {
                ui.add(Kind::Progress { class: "arrow".into(), bind: Box::new(|_: &Ctx| 1.0), value: 1.0, dir: voxide_abi::ui_dir::RIGHT }, Style::default().size(24.0, 16.0));
                if r.time > 0 {
                    label(ui, cx, "recipes.time", &[text::Arg::Float(r.time as f64 / 20.0, 1)], "dim");
                }
            });
            recipe_slot(ui, cx, &Ingredient::Item(r.output), r.count);
        });
    });
}
