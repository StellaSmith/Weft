//! Built-in screens. They use the same UI building blocks mods get; a mod replaces one by
//! registering a screen with its name (`named`).

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
    /// The window in GUI pixels.
    pub size: glam::Vec2,
}

impl BuildCx<'_> {
    pub fn t(&self, key: &str, args: &[Arg]) -> Vec<Span> {
        let mut out = Vec::new();
        let items = |name: &str| crate::content::try_content().and_then(|c| c.item_by_ref(name));
        let cx = text::Ctx { lang: self.lang, input: Some(self.input), item_name: &items };
        text::format(self.lang.get(key), args, &cx, &mut out);
        out
    }

    /// A template that is not a translation key (a pack's description), formatted the same way.
    pub fn rich(&self, template: &str) -> Vec<Span> {
        text::rich(template, self.lang, Some(self.input))
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
    /// Shows the open server menu: its slots take clicks, closing the screen closes the menu.
    fn shows_menu(&self) -> bool {
        false
    }
    /// The chat line: Enter sends its `chat` input, the HUD leaves the messages to it.
    fn shows_chat(&self) -> bool {
        false
    }
    /// Gets the keys before Escape and the other global keys.
    fn keys_first(&self) -> bool {
        false
    }
    fn escape_closes(&self) -> bool {
        true
    }
    fn closes_on_respawn(&self) -> bool {
        false
    }
    /// Lists the installed languages (read when it opens).
    fn needs_languages(&self) -> bool {
        false
    }
}

/// A screen by name (`core:` may be left out): a mod's of that name, else the engine's.
/// Engine screens open each other through this too, so a mod's replacement is used everywhere.
pub fn named(name: &str, settings: &Settings) -> Option<Box<dyn Screen>> {
    let name = if name.contains(':') { name.to_string() } else { format!("core:{name}") };
    if let Some(s) = crate::content::try_content().and_then(|c| c.game.screens.iter().rev().find(|s| s.name == name)) {
        return Some(Box::new(ModScreen(s.clone())));
    }
    Some(match name.as_str() {
        "core:title" => Box::new(Title),
        "core:pause" => Box::new(Pause),
        "core:settings" => Box::new(SettingsScreen),
        "core:language" => Box::new(LanguageScreen),
        "core:controls" => Box::new(Controls { waiting: None }),
        "core:mod_settings" => Box::new(ModSettings),
        "core:worlds" => Box::new(crate::saves::WorldSelect::default()),
        "core:create_world" => Box::new(crate::saves::CreateWorld),
        "core:chat" => Box::new(crate::game_screens::Chat::new("")),
        "core:death" => Box::new(crate::game_screens::Death),
        "core:multiplayer" => Box::new(crate::net::screens::Multiplayer::default()),
        "core:resource_packs" => Box::new(crate::packs::ResourcePacks::new(&settings.resource_packs)),
        "core:shader_packs" => Box::new(crate::packs::ShaderPacks::new(settings.shader_pack.clone())),
        "core:sound" => Box::new(ConfigScreen { ns: "core".into(), title: Some("settings.sound".into()) }),
        "core:tags" => Box::new(crate::game_screens::TagList::default()),
        "core:mods" => Box::new(ModList::default()),
        "core:profile" => Box::new(Profile::default()),
        _ => return None,
    })
}

/// An engine screen; they always exist.
pub fn engine(name: &str, settings: &Settings) -> Box<dyn Screen> {
    named(name, settings).unwrap_or_else(|| panic!("no screen {name}"))
}

/// Opens a screen by name (a mod's replacement if any).
fn open(c: &mut Ctx, name: &str) {
    c.requests.push(Request::Screen(name.to_string()));
}

/// What mods add to place `area` of `screen` (`Registry::screen_part`).
pub fn parts(ui: &mut Ui, cx: &BuildCx, screen: &str, area: &str) {
    ui.container(Style::default().gap(4.0).stretch(), |ui| {
        crate::client::modui::parts(ui, cx.lang, Some(cx.input), cx.client, screen, area);
    });
}

/// A mod's name as players see it: `mod.<id>.name` from the language files, else its id.
pub fn mod_name(lang: &Lang, id: &str) -> String {
    let key = format!("mod.{id}.name");
    if lang.has(&key) { lang.get(&key).to_string() } else { id.to_string() }
}

// ---------------------------------------------------------------- mods

/// Every loaded mod with its icon, version, authors and description (translations
/// `mod.<id>.description`, `.authors`, `.url`; icon `textures/gui/icon.png`), and its settings.
#[derive(Default)]
pub struct ModList {
    selected: Rc<std::cell::Cell<usize>>,
}

impl Screen for ModList {
    fn name(&self) -> &str {
        "core:mods"
    }

    fn build(&mut self, ui: &mut Ui, cx: &BuildCx) {
        let c = crate::content::content();
        let mut mods: Vec<&crate::content::ModMeta> = c.mods.iter().collect();
        mods.sort_by_key(|m| m.order);
        let sel = self.selected.get().min(mods.len().saturating_sub(1));
        let tall = (cx.size.y - 120.0).clamp(80.0, 300.0);
        let icon = |id: &str| {
            let name = format!("{id}:gui/icon");
            crate::assets::texture_size(&name).map(|_| name)
        };
        frame(ui, cx, "mods.title", |ui| {
            ui.container(Style::row().gap(6.0), |ui| {
                let list = ui.panel("well", Style::default().pad(4.0).gap(2.0).w(160.0).h(tall).scroll().stretch(), |ui| {
                    for (i, m) in mods.iter().enumerate() {
                        let s = self.selected.clone();
                        let b = ui.button("button", Style::row().min_h(22.0).pad_xy(4.0, 2.0).gap(4.0).stretch().justify(Justify::Start), Rc::new(move |c: &mut Ctx| {
                            s.set(i);
                            c.requests.push(Request::Rebuild);
                        }), |ui| {
                            match icon(&m.id) {
                                Some(t) => drop(ui.add(Kind::Image { texture: t, color: 0xFFFFFFFF }, Style::default().size(16.0, 16.0))),
                                None => drop(ui.add(Kind::Container { class: None }, Style::default().size(16.0, 16.0))),
                            }
                            ui.text(text::plain(&mod_name(cx.lang, &m.id)), "button");
                        });
                        if let Kind::Button { selected, .. } = &mut ui.nodes[b].kind {
                            *selected = i == sel;
                        }
                        ui.set_id(b, &format!("mod:{}", m.id));
                    }
                });
                ui.set_id(list, "mod_list");
                let Some(m) = mods.get(sel) else { return };
                let key = |k: &str| format!("mod.{}.{k}", m.id);
                ui.panel("panel", Style::default().pad(8.0).gap(4.0).w(300.0).h(tall).scroll().stretch(), |ui| {
                    ui.container(Style::row().gap(6.0), |ui| {
                        if let Some(t) = icon(&m.id) {
                            ui.add(Kind::Image { texture: t, color: 0xFFFFFFFF }, Style::default().size(32.0, 32.0));
                        }
                        ui.container(Style::default().gap(1.0), |ui| {
                            ui.text(text::plain(&mod_name(cx.lang, &m.id)), "default");
                            label(ui, cx, "mods.version", &[Arg::Text(m.version.clone()), Arg::Text(m.id.clone())], "dim");
                        });
                    });
                    for (k, line) in [("authors", "mods.authors"), ("url", "mods.url")] {
                        if cx.lang.has(&key(k)) {
                            label(ui, cx, line, &[Arg::Key(key(k))], "dim");
                        }
                    }
                    if !m.deps.is_empty() {
                        let deps: Vec<String> = m.deps.iter().map(|d| mod_name(cx.lang, d)).collect();
                        label(ui, cx, "mods.deps", &[Arg::Text(deps.join(", "))], "dim");
                    }
                    ui.add(Kind::Container { class: None }, Style::default().h(4.0));
                    if cx.lang.has(&key("description")) {
                        let d = ui.text(cx.t(&key("description"), &[]), "default");
                        ui.set_id(d, "mod_description");
                    } else {
                        label(ui, cx, "mods.no_description", &[], "dim");
                    }
                });
            });
            ui.container(Style::row().gap(6.0), |ui| {
                let id = mods.get(sel).map(|m| m.id.clone()).unwrap_or_default();
                let has_settings = c.game.configs.iter().any(|d| d.name.split_once(':').is_some_and(|x| x.0 == id));
                let b = button(ui, cx, "mods.settings", &[], 120.0, move |c| c.requests.push(Request::Open(Box::new(ConfigScreen { ns: id.clone(), title: None }))));
                if let Kind::Button { enabled, .. } = &mut ui.nodes[b].kind {
                    *enabled = has_settings;
                }
                let dir = cx.settings.run_dir.join("mods");
                button(ui, cx, "mods.folder", &[], 120.0, move |_| crate::packs::open_folder(&dir));
                button(ui, cx, "menu.done", &[], 120.0, |c| c.requests.push(Request::Close));
            });
        });
    }
}

// ---------------------------------------------------------------- profile

/// Who the player is everywhere: the name, the key servers know them by, and the skin.
#[derive(Default)]
pub struct Profile;

impl Screen for Profile {
    fn name(&self) -> &str {
        "core:profile"
    }

    fn build(&mut self, ui: &mut Ui, cx: &BuildCx) {
        let key = crate::net::p2p::player_key(&cx.settings.run_dir);
        frame(ui, cx, "profile.title", |ui| {
            ui.panel("panel", Style::default().pad(8.0).gap(4.0).w(320.0), |ui| {
                label(ui, cx, "profile.name", &[], "default");
                let input = ui.add(
                    Kind::Input {
                        value: cx.settings.name.clone(),
                        caret: cx.settings.name.chars().count(),
                        max: 16,
                        on_change: Some(Rc::new(|c: &mut Ctx, v: &str| c.settings.name = v.to_string())),
                    },
                    Style::default().h(16.0).w(300.0),
                );
                ui.set_id(input, "profile_name");
                ui.add(Kind::Container { class: None }, Style::default().h(4.0));
                label(ui, cx, "profile.key", &[], "default");
                ui.text(text::plain(if key.is_empty() { "-" } else { &key }), "dim");
                let copy = key.clone();
                button(ui, cx, "profile.copy", &[], 120.0, move |_| {
                    crate::clipboard::set(&copy);
                });
                ui.add(Kind::Container { class: None }, Style::default().h(4.0));
                label(ui, cx, "profile.skin", &[], "default");
                label(ui, cx, "profile.skin_hint", &[], "dim");
                let dir = cx.settings.run_dir.join("skins");
                button(ui, cx, "profile.skins_folder", &[], 120.0, move |_| crate::packs::open_folder(&dir));
            });
            button(ui, cx, "menu.done", &[], 200.0, |c| {
                c.settings.save();
                c.requests.push(Request::Close);
            });
        });
    }
}

/// A screen a mod built (`Registry::screen`).
pub struct ModScreen(crate::registry::ScreenDef);

impl Screen for ModScreen {
    fn name(&self) -> &str {
        &self.0.name
    }

    fn build(&mut self, ui: &mut Ui, cx: &BuildCx) {
        let bg = if cx.in_game { "overlay" } else { "background" };
        ui.root_style(Style::default().stretch());
        ui.panel(bg, Style::default().grow().center(), |ui| {
            crate::client::modui::build(ui, cx.lang, Some(cx.input), self.0.owner, self.0.cb, cx.client);
        });
    }
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

    /// The menu down a bar on the left under the logo, the rest of the window for whatever packs
    /// and mods show there (`menu.news`, the `side` place).
    fn build(&mut self, ui: &mut Ui, cx: &BuildCx) {
        ui.root_style(Style::default().stretch());
        ui.panel("background", Style::row().grow().stretch(), |ui| {
            ui.panel("sidebar", Style::default().w(180.0).pad(12.0).gap(4.0).stretch(), |ui| {
                // A translation, so a pack can make the logo a picture (`{img:..}`).
                let logo = ui.text(cx.t("menu.logo", &[]), "title");
                if let Some(t) = ui.text_mut(logo) {
                    t.scale = 3.0;
                    t.wrap = false;
                }
                ui.spacer();
                button(ui, cx, "menu.singleplayer", &[], 156.0, |c| open(c, "core:worlds"));
                button(ui, cx, "menu.multiplayer", &[], 156.0, |c| open(c, "core:multiplayer"));
                button(ui, cx, "menu.mods", &[Arg::Int(cx.mods as i64)], 156.0, |c| open(c, "core:mods"));
                button(ui, cx, "menu.settings", &[], 156.0, |c| open(c, "core:settings"));
                button(ui, cx, "menu.profile", &[], 156.0, |c| open(c, "core:profile"));
                parts(ui, cx, "core:title", "buttons");
                ui.add(Kind::Container { class: None }, Style::default().h(6.0));
                button(ui, cx, "menu.quit", &[], 156.0, |c| c.requests.push(Request::Quit));
                ui.add(Kind::Container { class: None }, Style::default().h(6.0));
                label(ui, cx, "menu.version", &[Arg::Text(env!("CARGO_PKG_VERSION").into()), Arg::Int(cx.mods as i64)], "dim");
            });
            ui.container(Style::default().grow().pad(16.0).gap(8.0).justify(Justify::End).align(crate::ui::Align::End), |ui| {
                if cx.lang.has("menu.news") && !cx.lang.get("menu.news").is_empty() {
                    ui.panel("news", Style::default().pad(8.0).w(280.0), |ui| {
                        ui.text(cx.t("menu.news", &[]), "default");
                    });
                }
                parts(ui, cx, "core:title", "side");
            });
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
            button(ui, cx, "menu.multiplayer", &[], 200.0, |c| open(c, "core:multiplayer"));
            button(ui, cx, "menu.settings", &[], 200.0, |c| open(c, "core:settings"));
            parts(ui, cx, "core:pause", "buttons");
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
                    let max = crate::settings::max_gui_scale(c.stats.window);
                    c.settings.gui_scale = if c.settings.gui_scale >= max { 0 } else { c.settings.gui_scale + 1 };
                    c.requests.push(Request::Rebuild);
                });
                button(ui, cx, "settings.vsync", &[on_off(s.vsync)], 300.0, |c| {
                    c.settings.vsync = !c.settings.vsync;
                    c.requests.push(Request::ApplySettings);
                    c.requests.push(Request::Rebuild);
                });
                ui.container(Style::row().gap(6.0).justify(Justify::Center).stretch(), |ui| {
                    let b = button(ui, cx, "settings.language", &[], 147.0, |c| open(c, "core:language"));
                    ui.nodes[b].style.grow = 1.0;
                    let b = button(ui, cx, "settings.controls", &[], 147.0, |c| open(c, "core:controls"));
                    ui.nodes[b].style.grow = 1.0;
                });
                ui.container(Style::row().gap(6.0).justify(Justify::Center).stretch(), |ui| {
                    let b = button(ui, cx, "settings.sound", &[], 147.0, |c| open(c, "core:sound"));
                    ui.nodes[b].style.grow = 1.0;
                    let b = button(ui, cx, "settings.mods", &[], 147.0, |c| open(c, "core:mod_settings"));
                    ui.nodes[b].style.grow = 1.0;
                });
                ui.container(Style::row().gap(6.0).justify(Justify::Center).stretch(), |ui| {
                    let b = button(ui, cx, "settings.resource_packs", &[], 147.0, |c| open(c, "core:resource_packs"));
                    ui.nodes[b].style.grow = 1.0;
                    let b = button(ui, cx, "settings.shader_packs", &[], 147.0, |c| open(c, "core:shader_packs"));
                    ui.nodes[b].style.grow = 1.0;
                });
                parts(ui, cx, "core:settings", "buttons");
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
                let name = mod_name(cx.lang, ns);
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
        use weft_abi::config_kind as ck;
        let c = crate::content::content();
        let ns = self.ns.clone();
        let title = self.title.clone().unwrap_or_else(|| "settings.mods.title".into());
        let mod_name = mod_name(cx.lang, &ns);
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
                    for (on, note) in [(locked, "settings.config_synced"), (d.flags & weft_abi::config_flags::RESTART != 0, "settings.config_restart")] {
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
    fn needs_languages(&self) -> bool {
        true
    }

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
                    open(c, "core:title");
                }
            });
        });
    }
}
