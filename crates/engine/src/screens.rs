//! Built-in screens. They use the same UI building blocks mods get; a mod can replace any of them
//! by registering a screen with the same name.

use std::rc::Rc;

use crate::app::{Ctx, Request};
use crate::input::{Input, Key};
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
    /// Joined someone else's world (their synced settings apply).
    pub remote: bool,
    pub mods: usize,
    pub client: Option<&'a crate::client::Client>,
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
                slider(ui, "settings.view_distance", 4.0, 64.0, 1.0, |s| s.view_distance as f32, |c, v| {
                    if c.settings.view_distance != v as i32 {
                        c.settings.view_distance = v as i32;
                        c.requests.push(Request::ApplySettings);
                    }
                });
                slider(ui, "settings.fov", 50.0, 110.0, 1.0, |s| s.fov, |c, v| c.settings.fov = v);
                slider(ui, "settings.sensitivity", 20.0, 300.0, 5.0, |s| s.sensitivity * 100.0, |c, v| c.settings.sensitivity = v / 100.0);
                slider(ui, "settings.brightness", 0.0, 100.0, 1.0, |s| s.brightness * 100.0, |c, v| c.settings.brightness = v / 100.0);
                let scale = if s.gui_scale == 0 { Arg::Key("settings.gui_scale.auto".into()) } else { Arg::Int(s.gui_scale as i64) };
                button(ui, cx, "settings.gui_scale", &[scale], 300.0, |c| {
                    // Auto, then every scale the window can actually show.
                    let (w, h) = c.stats.window;
                    let max = (w / 320).min(h / 240).max(1);
                    c.settings.gui_scale = if c.settings.gui_scale >= max { 0 } else { c.settings.gui_scale + 1 };
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
                ui.container(Style::row().gap(6.0).justify(Justify::Center).stretch(), |ui| {
                    let b = button(ui, cx, "settings.sound", &[], 147.0, |c| c.requests.push(Request::Open(Box::new(ConfigScreen { ns: "core".into(), title: Some("settings.sound".into()) }))));
                    ui.nodes[b].style.grow = 1.0;
                    let b = button(ui, cx, "settings.mods", &[], 147.0, |c| c.requests.push(Request::Open(Box::new(ModSettings))));
                    ui.nodes[b].style.grow = 1.0;
                });
                ui.container(Style::row().gap(6.0).justify(Justify::Center).stretch(), |ui| {
                    let b = button(ui, cx, "settings.resource_packs", &[], 147.0, |c| {
                        let current = c.settings.resource_packs.clone();
                        c.requests.push(Request::Open(Box::new(crate::packs::ResourcePacks::new(&current))));
                    });
                    ui.nodes[b].style.grow = 1.0;
                    let b = button(ui, cx, "settings.shader_packs", &[], 147.0, |c| {
                        let current = c.settings.shader_pack.clone();
                        c.requests.push(Request::Open(Box::new(crate::packs::ShaderPacks::new(current))));
                    });
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

// ---------------------------------------------------------------- settings mods declare

/// Mods that declared settings (`Host::reg_config`).
pub struct ModSettings;

impl Screen for ModSettings {
    fn name(&self) -> &str {
        "core:mod_settings"
    }

    fn pauses(&self) -> bool {
        true
    }

    fn build(&mut self, ui: &mut Ui, cx: &BuildCx) {
        let c = crate::content::content();
        let mut namespaces: Vec<&str> = c.game.configs.iter().filter_map(|d| d.name.split_once(':').map(|x| x.0)).filter(|ns| *ns != "core").collect();
        namespaces.dedup();
        frame(ui, cx, "settings.mods", |ui| {
            if namespaces.is_empty() {
                label(ui, cx, "settings.mods.none", &[], "dim");
            }
            for ns in namespaces {
                let name = c.mods.iter().find(|m| m.id == ns).map_or(ns.to_string(), |m| m.name.clone());
                let ns = ns.to_string();
                button(ui, cx, "settings.mods.entry", &[Arg::Text(name)], 220.0, move |c| c.requests.push(Request::Open(Box::new(ConfigScreen { ns: ns.clone(), title: None }))));
            }
            button(ui, cx, "menu.done", &[], 200.0, |c| c.requests.push(Request::Close));
        });
    }
}

/// Every setting of one namespace, built from its declarations: toggles, sliders, choices.
pub struct ConfigScreen {
    pub ns: String,
    /// Title key; the mod's name when `None`.
    pub title: Option<String>,
}

impl Screen for ConfigScreen {
    fn name(&self) -> &str {
        "core:config"
    }

    fn pauses(&self) -> bool {
        true
    }

    fn build(&mut self, ui: &mut Ui, cx: &BuildCx) {
        use voxide_abi::config_kind as ck;
        let c = crate::content::content();
        let ns = self.ns.clone();
        let title = self.title.clone().unwrap_or_else(|| "settings.mods.title".into());
        let mod_name = c.mods.iter().find(|m| m.id == ns).map_or(ns.clone(), |m| m.name.clone());
        frame(ui, cx, &title, |ui| {
            if self.title.is_none() {
                label(ui, cx, "settings.mods.of", &[Arg::Text(mod_name)], "default");
            }
            let list = ui.panel("panel", Style::default().pad(8.0).gap(6.0).stretch().w(320.0).h(0.0).grow().min_h(40.0).scroll(), |ui| {
                for (id, d) in c.game.configs.iter().enumerate().filter(|(_, d)| d.name.split_once(':').is_some_and(|x| x.0 == ns)) {
                    let id = id as u32;
                    let key = format!("config.{}", d.name.replace(':', "."));
                    let locked = d.synced() && cx.remote;
                    let show = |d: &crate::config::ConfigDef, key: &str, v: f64| -> Arg {
                        match d.kind {
                            ck::BOOL => on_off(v != 0.0),
                            ck::CHOICE => {
                                let choice = d.choices.get(v as usize).cloned().unwrap_or_default();
                                Arg::Key(format!("{key}.{choice}"))
                            }
                            _ => Arg::Text(d.show(v)),
                        }
                    };
                    let row = match d.kind {
                        ck::BOOL | ck::CHOICE => {
                            let (key2, d2) = (key.clone(), d.clone());
                            let (kind, count) = (d.kind, d.choices.len().max(1) as f64);
                            let b = ui.button(
                                "button",
                                Style::default().min_h(20.0).pad_xy(6.0, 4.0).stretch(),
                                Rc::new(move |_: &mut Ctx| {
                                    let v = crate::config::get(id);
                                    let next = if kind == ck::BOOL { 1.0 - v } else { (v + 1.0) % count };
                                    crate::config::set(id, next);
                                }),
                                |ui| {
                                    let (k, d3) = (key2.clone(), d2.clone());
                                    ui.text_bound("button", Box::new(move |_: &Ctx| ("settings.config_value".into(), vec![Arg::Key(k.clone()), show(&d3, &k, crate::config::get(id))])));
                                },
                            );
                            if let Kind::Button { enabled, .. } = &mut ui.nodes[b].kind {
                                *enabled = !locked;
                            }
                            b
                        }
                        _ => ui.container(Style::default().gap(2.0).stretch(), |ui| {
                            let (k, d2) = (key.clone(), d.clone());
                            ui.text_bound("default", Box::new(move |_: &Ctx| ("settings.config_value".into(), vec![Arg::Key(k.clone()), show(&d2, &k, crate::config::get(id))])));
                            if !locked {
                                let (min, range) = (d.min, (d.max - d.min).max(1e-9));
                                ui.add(
                                    Kind::Slider {
                                        value: 0.0,
                                        bind: Box::new(move |_: &Ctx| ((crate::config::get(id) - min) / range) as f32),
                                        on_change: Rc::new(move |_: &mut Ctx, f: f32| crate::config::set(id, min + f as f64 * range)),
                                    },
                                    Style::default().h(20.0).stretch(),
                                );
                            }
                        }),
                    };
                    let tip = format!("{key}.tooltip");
                    let mut spans = if cx.lang.has(&tip) { cx.t(&tip, &[]) } else { Vec::new() };
                    for (on, note) in [(locked, "settings.config_synced"), (d.flags & voxide_abi::config_flags::RESTART != 0, "settings.config_restart")] {
                        if on {
                            if !spans.is_empty() {
                                spans.push(Span::text("\n", None));
                            }
                            spans.extend(cx.t(note, &[]));
                        }
                    }
                    if !spans.is_empty() {
                        ui.nodes[row].tooltip = Some(spans);
                    }
                }
            });
            ui.set_id(list, "config");
            button(ui, cx, "menu.done", &[], 200.0, |c| c.requests.push(Request::Close));
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
            Style::default().h(20.0).stretch(),
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
            let list = ui.panel("panel", Style::default().pad(8.0).gap(3.0).stretch().w(380.0).h(0.0).grow().min_h(60.0).scroll(), |ui| {
                for b in &cx.input.binds {
                    let action = b.action.clone();
                    let name_key = format!("key.{}", b.action.replace(':', "."));
                    let clash: Vec<String> = conflicts
                        .iter()
                        .filter_map(|(x, y)| if *x == b.action { Some(y.clone()) } else if *y == b.action { Some(x.clone()) } else { None })
                        .collect();
                    ui.container(Style::row().gap(6.0).self_align(crate::ui::Align::Stretch), |ui| {
                        let n = ui.text(cx.t(&name_key, &[]), "default");
                        ui.nodes[n].style.grow = 1.0;
                        if !clash.is_empty() {
                            let names: Vec<String> = clash.iter().map(|c| cx.lang.get(&format!("key.{}", c.replace(':', "."))).to_string()).collect();
                            ui.text(cx.t("controls.conflict", &[Arg::Text(names.join(", "))]), "default");
                        }
                        let a2 = action.clone();
                        let kb = ui.button("button", Style::default().w(70.0).h(20.0), Rc::new(move |c: &mut Ctx| {
                            c.requests.push(Request::WaitKey(a2.clone()));
                        }), |ui| {
                            ui.text(text::plain(&text::display_key(cx.lang, b.key)), "button");
                        });
                        if let Kind::Button { selected, .. } = &mut ui.nodes[kb].kind {
                            *selected = waiting.as_deref() == Some(action.as_str());
                        }
                    });
                }
            });
            ui.set_id(list, "controls");
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
            let in_game = cx.in_game;
            button(ui, cx, "menu.back", &[], 200.0, move |c| {
                c.requests.push(Request::Close);
                if !in_game {
                    c.requests.push(Request::Open(Box::new(Title)));
                }
            });
        });
    }
}
