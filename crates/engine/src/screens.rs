//! Built-in screens. They use the same UI building blocks mods get; a mod can replace any of them
//! by registering a screen with the same name.

use std::rc::Rc;

use crate::app::{Ctx, Request};
use crate::input::{Input, Key, key_name};
use crate::settings::Settings;
use crate::ui::lang::Lang;
use crate::ui::text::{self, Arg, Span};
use crate::ui::{Justify, Kind, NodeId, Style, Ui};

pub struct BuildCx<'a> {
    pub lang: &'a Lang,
    pub input: &'a Input,
    pub settings: &'a Settings,
    pub languages: &'a [(String, String)],
    pub in_game: bool,
    pub mods: usize,
}

impl BuildCx<'_> {
    pub fn t(&self, key: &str, args: &[Arg]) -> Vec<Span> {
        let mut out = Vec::new();
        let items = |name: &str| crate::content::try_content().and_then(|c| c.item_names.get(name).copied());
        let cx = text::Ctx { lang: self.lang, input: Some(self.input), item_name: &items };
        text::format(self.lang.get(key), args, &cx, &mut out);
        out
    }
}

pub trait Screen {
    fn name(&self) -> &str;
    fn build(&mut self, ui: &mut Ui, cx: &BuildCx);
    /// Single player stops the server tick while this is open.
    fn pauses(&self) -> bool {
        false
    }
    /// Keys go to this screen; `true` means it handled them.
    fn key(&mut self, _key: Key, _ctx: &mut Ctx) -> bool {
        false
    }
    /// List selection forwarded from buttons.
    fn select(&mut self, _index: usize) {}
}

// ---------------------------------------------------------------- helpers

pub fn label(ui: &mut Ui, cx: &BuildCx, key: &str, args: &[Arg], class: &str) -> NodeId {
    ui.text(cx.t(key, args), class)
}

pub fn button(ui: &mut Ui, cx: &BuildCx, key: &str, args: &[Arg], width: f32, action: impl Fn(&mut Ctx) + 'static) -> NodeId {
    let spans = cx.t(key, args);
    ui.button("button", Style::default().min_w(width).min_h(20.0).pad_xy(6.0, 4.0), Rc::new(action), |ui| {
        let t = ui.text(spans, "button");
        if let Some(t) = ui.text_mut(t) {
            t.wrap = false;
        }
    })
}

/// Full-screen dimmed column with a title, used by most menus.
pub fn frame(ui: &mut Ui, cx: &BuildCx, title: &str, f: impl FnOnce(&mut Ui)) {
    let bg = if cx.in_game { "overlay" } else { "background" };
    ui.root_style(Style::default().stretch());
    ui.panel(bg, Style::default().grow().center().pad(12.0).gap(10.0), |ui| {
        let t = label(ui, cx, title, &[], "title");
        if let Some(t) = ui.text_mut(t) {
            t.scale = 2.0;
            t.wrap = false;
        }
        f(ui);
    });
}

// ---------------------------------------------------------------- title

pub struct Title;

impl Screen for Title {
    fn name(&self) -> &str {
        "core:title"
    }

    fn build(&mut self, ui: &mut Ui, cx: &BuildCx) {
        ui.root_style(Style::default().stretch());
        ui.panel("background", Style::default().grow().center().gap(6.0), |ui| {
            let logo = ui.text(text::plain("VOXIDE"), "title");
            if let Some(t) = ui.text_mut(logo) {
                t.scale = 5.0;
                t.wrap = false;
            }
            ui.add(Kind::Container { class: None }, Style::default().h(20.0));
            button(ui, cx, "menu.singleplayer", &[], 200.0, |c| c.requests.push(Request::Open(Box::new(crate::saves::WorldSelect::default()))));
            button(ui, cx, "menu.multiplayer", &[], 200.0, |c| c.requests.push(Request::Open(Box::new(crate::net::screens::Multiplayer::default()))));
            button(ui, cx, "menu.settings", &[], 200.0, |c| c.requests.push(Request::Open(Box::new(SettingsScreen))));
            button(ui, cx, "menu.quit", &[], 200.0, |c| c.requests.push(Request::Quit));
            ui.add(Kind::Container { class: None }, Style::default().h(20.0));
            label(ui, cx, "menu.version", &[Arg::Text(env!("CARGO_PKG_VERSION").into()), Arg::Int(cx.mods as i64)], "dim");
        });
    }
}

// ---------------------------------------------------------------- pause

pub struct Pause;

impl Screen for Pause {
    fn name(&self) -> &str {
        "core:pause"
    }

    fn pauses(&self) -> bool {
        true
    }

    fn build(&mut self, ui: &mut Ui, cx: &BuildCx) {
        frame(ui, cx, "menu.paused", |ui| {
            button(ui, cx, "menu.resume", &[], 200.0, |c| c.requests.push(Request::Close));
            button(ui, cx, "menu.multiplayer", &[], 200.0, |c| c.requests.push(Request::Open(Box::new(crate::net::screens::Multiplayer::default()))));
            button(ui, cx, "menu.settings", &[], 200.0, |c| c.requests.push(Request::Open(Box::new(SettingsScreen))));
            button(ui, cx, "menu.save_quit", &[], 200.0, |c| c.requests.push(Request::LeaveWorld));
        });
    }
}

// ---------------------------------------------------------------- settings

pub struct SettingsScreen;

fn on_off(v: bool) -> Arg {
    Arg::Key(if v { "settings.on".into() } else { "settings.off".into() })
}

impl Screen for SettingsScreen {
    fn name(&self) -> &str {
        "core:settings"
    }

    fn pauses(&self) -> bool {
        true
    }

    fn build(&mut self, ui: &mut Ui, cx: &BuildCx) {
        let s = cx.settings;
        frame(ui, cx, "settings.title", |ui| {
            ui.panel("panel", Style::default().pad(8.0).gap(6.0).stretch().w(320.0), |ui| {
                slider(ui, "settings.view_distance", 4.0, 32.0, 1.0, |s| s.view_distance as f32, |c, v| {
                    if c.settings.view_distance != v as i32 {
                        c.settings.view_distance = v as i32;
                        c.requests.push(Request::ApplySettings);
                    }
                });
                slider(ui, "settings.fov", 50.0, 110.0, 1.0, |s| s.fov, |c, v| c.settings.fov = v);
                slider(ui, "settings.sensitivity", 20.0, 300.0, 5.0, |s| s.sensitivity * 100.0, |c, v| c.settings.sensitivity = v / 100.0);
                let scale = if s.gui_scale == 0 { Arg::Key("settings.gui_scale.auto".into()) } else { Arg::Int(s.gui_scale as i64) };
                button(ui, cx, "settings.gui_scale", &[scale], 300.0, |c| {
                    c.settings.gui_scale = (c.settings.gui_scale + 1) % 7;
                    c.requests.push(Request::Rebuild);
                });
                button(ui, cx, "settings.vsync", &[on_off(s.vsync)], 300.0, |c| {
                    c.settings.vsync = !c.settings.vsync;
                    c.requests.push(Request::ApplySettings);
                    c.requests.push(Request::Rebuild);
                });
                ui.container(Style::row().gap(6.0).justify(Justify::Center).stretch(), |ui| {
                    let b = button(ui, cx, "settings.language", &[], 147.0, |c| c.requests.push(Request::Open(Box::new(LanguageScreen))));
                    ui.nodes[b].style.grow = 1.0;
                    let b = button(ui, cx, "settings.controls", &[], 147.0, |c| c.requests.push(Request::Open(Box::new(Controls { waiting: None }))));
                    ui.nodes[b].style.grow = 1.0;
                });
            });
            button(ui, cx, "menu.done", &[], 200.0, |c| {
                c.settings.save();
                c.requests.push(Request::Close);
            });
        });
    }
}

fn slider(ui: &mut Ui, key: &str, min: f32, max: f32, step: f32, get: fn(&Settings) -> f32, set: impl Fn(&mut Ctx, f32) + 'static) {
    let key = key.to_string();
    let range = max - min;
    ui.container(Style::default().gap(2.0).stretch(), |ui| {
        ui.text_bound("default", Box::new(move |c: &Ctx| (key.clone(), vec![Arg::Int(get(c.settings).round() as i64)])));
        ui.add(
            Kind::Slider {
                value: 0.0,
                bind: Box::new(move |c: &Ctx| (get(c.settings) - min) / range),
                on_change: Rc::new(move |c: &mut Ctx, f: f32| {
                    let v = ((min + f * range) / step).round() * step;
                    set(c, v.clamp(min, max));
                }),
            },
            Style::default().h(12.0).stretch(),
        );
    });
}

// ---------------------------------------------------------------- language

pub struct LanguageScreen;

impl Screen for LanguageScreen {
    fn name(&self) -> &str {
        "core:language"
    }

    fn pauses(&self) -> bool {
        true
    }

    fn build(&mut self, ui: &mut Ui, cx: &BuildCx) {
        let current = cx.languages.iter().find(|l| l.0 == cx.lang.code).map_or(cx.lang.code.clone(), |l| l.1.clone());
        frame(ui, cx, "settings.language", |ui| {
            label(ui, cx, "settings.language.current", &[Arg::Text(current)], "default");
            ui.panel("panel", Style::default().pad(8.0).gap(4.0).stretch().w(240.0), |ui| {
                for (code, name) in cx.languages {
                    let code = code.clone();
                    let selected = code == cx.lang.code;
                    let b = ui.button("button", Style::default().min_h(20.0).pad_xy(6.0, 4.0).stretch(), Rc::new(move |c: &mut Ctx| {
                        c.requests.push(Request::SetLanguage(code.clone()));
                    }), |ui| {
                        ui.text(text::plain(name), "button");
                    });
                    if let Kind::Button { selected: sel, .. } = &mut ui.nodes[b].kind {
                        *sel = selected;
                    }
                }
            });
            label(ui, cx, "settings.language.note", &[], "dim");
            button(ui, cx, "menu.done", &[], 200.0, |c| {
                c.settings.save();
                c.requests.push(Request::Close);
            });
        });
    }
}

// ---------------------------------------------------------------- controls

pub struct Controls {
    pub waiting: Option<String>,
}

impl Screen for Controls {
    fn name(&self) -> &str {
        "core:controls"
    }

    fn pauses(&self) -> bool {
        true
    }

    fn key(&mut self, key: Key, ctx: &mut Ctx) -> bool {
        let Some(action) = self.waiting.take() else { return false };
        if key != Key::Code(winit::keyboard::KeyCode::Escape) {
            ctx.requests.push(Request::Rebind(action, key));
        }
        ctx.requests.push(Request::Rebuild);
        true
    }

    fn build(&mut self, ui: &mut Ui, cx: &BuildCx) {
        let conflicts = cx.input.conflicts();
        let waiting = self.waiting.clone();
        frame(ui, cx, "controls.title", |ui| {
            if let Some(a) = &waiting {
                label(ui, cx, "controls.press", &[Arg::Key(format!("key.{}", a.replace(':', ".")))], "default");
            }
            ui.panel("panel", Style::default().pad(8.0).gap(3.0).stretch().w(380.0), |ui| {
                for b in &cx.input.binds {
                    let action = b.action.clone();
                    let name_key = format!("key.{}", b.action.replace(':', "."));
                    let clash: Vec<String> = conflicts
                        .iter()
                        .filter_map(|(x, y)| if *x == b.action { Some(y.clone()) } else if *y == b.action { Some(x.clone()) } else { None })
                        .collect();
                    ui.container(Style::row().gap(6.0).stretch(), |ui| {
                        let n = ui.text(cx.t(&name_key, &[]), "default");
                        ui.nodes[n].style.grow = 1.0;
                        if !clash.is_empty() {
                            let names: Vec<String> = clash.iter().map(|c| cx.lang.get(&format!("key.{}", c.replace(':', "."))).to_string()).collect();
                            ui.text(cx.t("controls.conflict", &[Arg::Text(names.join(", "))]), "default");
                        }
                        let a2 = action.clone();
                        let kb = ui.button("button", Style::default().w(70.0).h(16.0), Rc::new(move |c: &mut Ctx| {
                            c.requests.push(Request::WaitKey(a2.clone()));
                        }), |ui| {
                            ui.text(text::plain(&key_name(b.key)), "button");
                        });
                        if let Kind::Button { selected, .. } = &mut ui.nodes[kb].kind {
                            *selected = waiting.as_deref() == Some(action.as_str());
                        }
                    });
                }
            });
            ui.container(Style::row().gap(6.0), |ui| {
                button(ui, cx, "controls.reset", &[], 120.0, |c| c.requests.push(Request::ResetBinds));
                button(ui, cx, "menu.done", &[], 120.0, |c| {
                    c.settings.save();
                    c.requests.push(Request::Close);
                });
            });
        });
    }
}

/// Shows a message with a back button (disconnects, errors).
pub struct Message {
    pub title: String,
    pub args: Vec<Arg>,
}

impl Screen for Message {
    fn name(&self) -> &str {
        "core:message"
    }

    fn build(&mut self, ui: &mut Ui, cx: &BuildCx) {
        let (title, args) = (self.title.clone(), self.args.clone());
        ui.root_style(Style::default().stretch());
        ui.panel(if cx.in_game { "overlay" } else { "background" }, Style::default().grow().center().pad(12.0).gap(10.0), |ui| {
            label(ui, cx, &title, &args, "title");
            button(ui, cx, "menu.back", &[], 200.0, |c| c.requests.push(Request::Close));
        });
    }
}
