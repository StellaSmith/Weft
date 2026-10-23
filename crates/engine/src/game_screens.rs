//! In-game screens: container menus (built by the mod that registered the menu), death, chat.

use std::cell::RefCell;
use std::rc::Rc;

use winit::keyboard::KeyCode;

use crate::app::{Ctx, Request};
use crate::input::Key;
use crate::ui::text::{self, Piece, Span};
use crate::client::modui;
use crate::content::content;
use crate::screens::{BuildCx, Screen, button, label};
use crate::ui::{Kind, Style, Ui};

pub struct MenuScreen {
    pub kind: u32,
    /// Creative inventory filter.
    search: Rc<RefCell<String>>,
}

impl MenuScreen {
    pub fn new(kind: u32) -> MenuScreen {
        MenuScreen { kind, search: Default::default() }
    }
}

/// Every item matching `filter` (in the registry or translated name) as a scrolling grid of
/// creative entries, with the search field above.
fn creative_panel(ui: &mut Ui, cx: &BuildCx, search: &Rc<RefCell<String>>) {
    let c = content();
    let filter = search.borrow().to_lowercase();
    let items: Vec<u32> = (1..c.items.len() as u32)
        .filter(|&i| {
            filter.is_empty() || c.items[i as usize].name.to_lowercase().contains(&filter) || crate::hud::item_name(cx.lang, i).to_lowercase().contains(&filter)
        })
        .collect();
    ui.panel("panel", Style::default().pad(7.0).gap(4.0), |ui| {
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

    fn build(&mut self, ui: &mut Ui, cx: &BuildCx) {
        let c = content();
        ui.root_style(Style::default().stretch());
        let creative = cx.client.is_some_and(|cl| cl.mode == crate::world::GameMode::Creative) && c.game.player_menu == Some(self.kind);
        ui.panel("overlay", Style::default().grow().center(), |ui| {
            ui.container(Style::row().gap(4.0).align(crate::ui::Align::Start), |ui| {
                if creative {
                    creative_panel(ui, cx, &self.search);
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
/// typed (Tab takes the highlighted one, arrows move through them or through sent lines).
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
}

impl Chat {
    pub fn new(text: &str) -> Chat {
        Chat { state: Rc::new(RefCell::new(ChatState { text: text.to_string(), ..Default::default() })) }
    }
}

impl ChatState {
    fn update(&mut self, text: &str, players: &[String]) {
        self.text = text.to_string();
        self.selected = 0;
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
                            ui.text(vec![Span { piece: Piece::Text(s.clone()), color: Some(color) }], "chat");
                        }
                    });
                } else if !comp.hint.is_empty() {
                    ui.container(Style::default().pad(2.0).self_align(crate::ui::Align::Start), |ui| {
                        ui.text(vec![Span { piece: Piece::Text(comp.hint.clone()), color: Some(0x808080) }], "chat");
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
        match key {
            Key::Code(KeyCode::Tab) if n > 0 => {
                let comp = st.completion.as_ref().unwrap();
                let text = format!("{}{}", &st.text[..comp.from.min(st.text.len())], comp.suggestions[st.selected.min(n - 1)]);
                set(&mut st, text, ctx);
                true
            }
            Key::Code(KeyCode::ArrowUp | KeyCode::ArrowDown) if n > 0 => {
                let up = key == Key::Code(KeyCode::ArrowUp);
                st.selected = if up { (st.selected + n - 1) % n } else { (st.selected + 1) % n };
                ctx.requests.push(Request::Rebuild);
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
