//! Player-chosen client packs. Resource packs (`resourcepacks/<id>/`) are laid out like mod assets
//! per namespace (`<ns>/textures/...`, `<ns>/lang/...`, `.ctm`, themes) and stack in the order the
//! player picks. A shader pack (`shaderpacks/<id>/`) replaces engine shaders file by file
//! (`world.wgsl`, `entity.wgsl`, `model.wgsl`, `particle.wgsl`, `post.wgsl`) and has to keep
//! their entry points and bindings; a file that fails to compile falls back to the built-in one.
//! Snippets mods ship (`shaders/<kind>/`, see `shaders.rs`) are assets, overridden by resource
//! packs like textures. Both may have a `pack.cfg` with
//! `name = ...` and `description = ...`.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::RwLock;

use voxide_render::{Shader, ShaderError};

use crate::app::{Ctx, Request};
use crate::screens::{BuildCx, Screen, button, frame, label};
use crate::ui::text::{self, Arg};
use crate::ui::{Justify, Kind, Style, Ui};

pub struct PackInfo {
    pub id: String,
    pub name: String,
    pub description: String,
}

pub fn list(dir: &Path) -> Vec<PackInfo> {
    let mut out: Vec<PackInfo> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| {
            let id = e.file_name().to_string_lossy().into_owned();
            let cfg = std::fs::read_to_string(e.path().join("pack.cfg")).unwrap_or_default();
            let get = |key: &str| cfg.lines().find_map(|l| l.split_once('=').filter(|(k, _)| k.trim() == key).map(|(_, v)| v.trim().to_string()));
            PackInfo { name: get("name").unwrap_or_else(|| id.clone()), description: get("description").unwrap_or_default(), id }
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

pub fn resource_dir(run_dir: &Path) -> PathBuf {
    run_dir.join("resourcepacks")
}

pub fn shader_dir(run_dir: &Path) -> PathBuf {
    run_dir.join("shaderpacks")
}

/// Folders of the enabled resource packs that exist, highest priority first.
pub fn enabled(run_dir: &Path, ids: &[String]) -> Vec<PathBuf> {
    ids.iter().map(|id| resource_dir(run_dir).join(id)).filter(|p| p.is_dir()).collect()
}

static SHADER_PACK: RwLock<Option<PathBuf>> = RwLock::new(None);

pub fn set_shader_pack(dir: Option<PathBuf>) {
    *SHADER_PACK.write().unwrap() = dir;
}

/// Source of engine shader `name`: the shader pack's if it has one, else the engine's (read from
/// the source tree in development builds so it can be edited live).
pub fn source(name: &str, builtin: &str) -> String {
    let pack = SHADER_PACK.read().unwrap().as_ref().map(|d| d.join(name));
    if let Some(src) = pack.and_then(|p| std::fs::read_to_string(p).ok()) {
        return src;
    }
    builtin_source(name, builtin)
}

fn builtin_source(name: &str, builtin: &str) -> String {
    let dev = Path::new(env!("CARGO_MANIFEST_DIR")).join("shaders").join(name);
    std::fs::read_to_string(dev).unwrap_or_else(|_| builtin.to_string())
}

/// `name` from the shader pack if it has a valid one, else the engine's.
pub fn shader(name: &str, builtin: &str) -> Result<Shader, ShaderError> {
    let pack = SHADER_PACK.read().unwrap().as_ref().map(|d| d.join(name));
    if let Some(src) = pack.and_then(|p| std::fs::read_to_string(p).ok()) {
        match Shader::from_wgsl(name, &src) {
            Ok(s) => return Ok(s),
            Err(e) => log::error!("shader pack {name}: {}", e.0),
        }
    }
    Shader::from_wgsl(name, &builtin_source(name, builtin))
}

fn open_folder(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    let opener = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    if let Err(e) = std::process::Command::new(opener).arg(dir).spawn() {
        log::warn!("opening {}: {e}", dir.display());
    }
}

fn pack_row(ui: &mut Ui, info: &PackInfo, buttons: impl FnOnce(&mut Ui)) {
    ui.container(Style::row().gap(4.0).stretch(), |ui| {
        ui.container(Style::default().gap(1.0).grow(), |ui| {
            ui.text(text::plain(&info.name), "default");
            if !info.description.is_empty() {
                ui.text(text::plain(&info.description), "dim");
            }
        });
        buttons(ui);
    });
}

fn small(ui: &mut Ui, glyph: &str, action: impl Fn(&mut Ctx) + 'static) {
    ui.button("button", Style::default().w(20.0).h(20.0), Rc::new(action), |ui| {
        ui.text(text::plain(glyph), "button");
    });
}

/// Available and selected resource packs side by side; the top selected pack wins.
pub struct ResourcePacks {
    /// As when the screen opened, to reload only when something changed.
    initial: Vec<String>,
}

impl ResourcePacks {
    pub fn new(current: &[String]) -> ResourcePacks {
        ResourcePacks { initial: current.to_vec() }
    }
}

impl Screen for ResourcePacks {
    fn name(&self) -> &str {
        "core:resource_packs"
    }

    fn pauses(&self) -> bool {
        true
    }

    fn build(&mut self, ui: &mut Ui, cx: &BuildCx) {
        let dir = resource_dir(&cx.settings.run_dir);
        let all = list(&dir);
        let selected = cx.settings.resource_packs.clone();
        let initial = self.initial.clone();
        frame(ui, cx, "settings.resource_packs", |ui| {
            ui.container(Style::row().gap(8.0).grow().h(0.0).stretch(), |ui| {
                ui.container(Style::default().gap(4.0).w(260.0).stretch(), |ui| {
                    label(ui, cx, "packs.available", &[], "default");
                    ui.panel("panel", Style::default().pad(6.0).gap(4.0).stretch().grow().h(0.0).scroll(), |ui| {
                        let available: Vec<&PackInfo> = all.iter().filter(|p| !selected.contains(&p.id)).collect();
                        if available.is_empty() {
                            label(ui, cx, if all.is_empty() { "packs.none" } else { "packs.all_selected" }, &[], "dim");
                        }
                        for p in available {
                            let id = p.id.clone();
                            pack_row(ui, p, |ui| {
                                small(ui, "+", move |c| {
                                    c.settings.resource_packs.insert(0, id.clone());
                                    c.requests.push(Request::Rebuild);
                                });
                            });
                        }
                    });
                });
                ui.container(Style::default().gap(4.0).w(260.0).stretch(), |ui| {
                    label(ui, cx, "packs.selected", &[], "default");
                    ui.panel("panel", Style::default().pad(6.0).gap(4.0).stretch().grow().h(0.0).scroll(), |ui| {
                        for (i, id) in selected.iter().enumerate() {
                            let info = all.iter().find(|p| &p.id == id);
                            let missing = PackInfo { id: id.clone(), name: id.clone(), description: cx.lang.get("packs.missing").to_string() };
                            pack_row(ui, info.unwrap_or(&missing), |ui| {
                                if i > 0 {
                                    small(ui, "\u{25b2}", move |c| {
                                        c.settings.resource_packs.swap(i, i - 1);
                                        c.requests.push(Request::Rebuild);
                                    });
                                }
                                if i + 1 < selected.len() {
                                    small(ui, "\u{25bc}", move |c| {
                                        c.settings.resource_packs.swap(i, i + 1);
                                        c.requests.push(Request::Rebuild);
                                    });
                                }
                                small(ui, "-", move |c| {
                                    c.settings.resource_packs.remove(i);
                                    c.requests.push(Request::Rebuild);
                                });
                            });
                        }
                    });
                });
            });
            ui.container(Style::row().gap(6.0).justify(Justify::Center), |ui| {
                button(ui, cx, "packs.open_folder", &[], 140.0, move |_| open_folder(&dir));
                button(ui, cx, "menu.done", &[], 140.0, move |c| {
                    c.settings.save();
                    if c.settings.resource_packs != initial {
                        c.requests.push(Request::ReloadAssets);
                    }
                    c.requests.push(Request::Close);
                });
            });
        });
    }
}

/// One shader pack or the built-in shaders.
pub struct ShaderPacks {
    initial: Option<String>,
}

impl ShaderPacks {
    pub fn new(current: Option<String>) -> ShaderPacks {
        ShaderPacks { initial: current }
    }
}

impl Screen for ShaderPacks {
    fn name(&self) -> &str {
        "core:shader_packs"
    }

    fn pauses(&self) -> bool {
        true
    }

    fn build(&mut self, ui: &mut Ui, cx: &BuildCx) {
        let dir = shader_dir(&cx.settings.run_dir);
        let all = list(&dir);
        let current = cx.settings.shader_pack.clone();
        let initial = self.initial.clone();
        frame(ui, cx, "settings.shader_packs", |ui| {
            ui.panel("panel", Style::default().pad(8.0).gap(4.0).w(320.0).h(0.0).grow().min_h(60.0).scroll().stretch(), |ui| {
                let choices = std::iter::once((None, cx.t("packs.shaders_off", &[]), String::new()))
                    .chain(all.iter().map(|p| (Some(p.id.clone()), text::plain(&p.name), p.description.clone())));
                for (id, name, description) in choices {
                    let selected = id == current;
                    let b = ui.button("button", Style::default().min_h(20.0).pad_xy(6.0, 4.0).stretch(), Rc::new(move |c: &mut Ctx| {
                        c.settings.shader_pack = id.clone();
                        c.requests.push(Request::Rebuild);
                    }), |ui| {
                        ui.text(name, "button");
                        if !description.is_empty() {
                            ui.text(text::plain(&description), "button");
                        }
                    });
                    if let Kind::Button { selected: sel, .. } = &mut ui.nodes[b].kind {
                        *sel = selected;
                    }
                }
            });
            label(ui, cx, "packs.shaders_note", &[Arg::Text("world.wgsl, entity.wgsl, model.wgsl, particle.wgsl, post.wgsl".into())], "dim");
            ui.container(Style::row().gap(6.0).justify(Justify::Center), |ui| {
                button(ui, cx, "packs.open_folder", &[], 140.0, move |_| open_folder(&dir));
                button(ui, cx, "menu.done", &[], 140.0, move |c| {
                    c.settings.save();
                    if c.settings.shader_pack != initial {
                        c.requests.push(Request::ReloadAssets);
                    }
                    c.requests.push(Request::Close);
                });
            });
        });
    }
}
