//! What resource packs change of block and item shapes: a pack's `models/block/<name>.json` or
//! `models/item/<name>.json` redraws that block or item. Only the drawing: collision, selection,
//! light and which neighbors a block hides stay what the mod gave every player, since the server
//! knows nothing of packs.

use std::sync::{Arc, RwLock};

use rustc_hash::FxHashMap;

use crate::assets::Assets;
use crate::block_model::{self, FileModel};
use crate::content::{Content, ModelBox};

#[derive(Default)]
pub struct StateLook {
    /// Drawn instead of the state's own shape.
    pub boxes: Option<Vec<ModelBox>>,
    /// A cube parent's face textures.
    pub textures: Option<[u32; 6]>,
}

#[derive(Default)]
pub struct ItemLook {
    pub icon: Option<u32>,
    pub model: Option<Vec<ModelBox>>,
}

#[derive(Default)]
pub struct Looks {
    states: FxHashMap<u32, StateLook>,
    items: FxHashMap<u32, ItemLook>,
}

impl Looks {
    pub fn state(&self, state: u32) -> Option<&StateLook> {
        self.states.get(&state)
    }
    pub fn item(&self, item: u32) -> Option<&ItemLook> {
        self.items.get(&item)
    }
}

static CURRENT: RwLock<Option<Arc<Looks>>> = RwLock::new(None);

pub fn current() -> Arc<Looks> {
    CURRENT.read().unwrap().clone().unwrap_or_default()
}

pub fn set(looks: Looks) {
    *CURRENT.write().unwrap() = Some(Arc::new(looks));
}

/// The packs' models. Texture ids: the content's, then new names numbered from `next`, returned
/// for the texture table to load.
pub fn load(assets: &Assets, c: &Content, next: u32) -> (Looks, Vec<String>) {
    let mut looks = Looks::default();
    let mut ids: FxHashMap<String, u32> = c.textures.iter().enumerate().map(|(i, n)| (n.clone(), i as u32)).collect();
    let mut extra: Vec<String> = Vec::new();
    let mut id = |name: &str| -> u32 {
        if let Some(&i) = ids.get(name) {
            return i;
        }
        let i = next + extra.len() as u32;
        extra.push(name.to_string());
        ids.insert(name.to_string(), i);
        i
    };
    let read = |name: &str| assets.read(name, "models", "json").map(|b| String::from_utf8_lossy(&b).into_owned());
    let from_pack = |name: &str| -> Option<FileModel> {
        assets.pack_path(name, "models", "json")?;
        block_model::load(name, &read).unwrap_or_else(|e| {
            log::warn!("resource pack {e}");
            None
        })
    };
    let boxes = |m: &FileModel, own: &dyn Fn(usize) -> u32, id: &mut dyn FnMut(&str) -> u32| -> Vec<ModelBox> {
        m.boxes
            .iter()
            .map(|b| ModelBox {
                min: b.min.map(|v| v / 16.0),
                max: b.max.map(|v| v / 16.0),
                textures: std::array::from_fn(|i| if b.textures[i].is_empty() { own(i) } else { id(&b.textures[i]) }),
                uv: b.uv,
            })
            .collect()
    };
    for b in &c.blocks {
        let Some((ns, path)) = b.name.split_once(':') else { continue };
        let Some(m) = from_pack(&format!("{ns}:block/{path}")) else { continue };
        for st in b.first_state..b.first_state + b.states {
            let s = &c.states[st as usize];
            let mut look = StateLook::default();
            if let Some(cube) = &m.cube {
                look.textures = Some(std::array::from_fn(|i| if cube[i].is_empty() { s.textures[i] } else { id(&cube[i]) }));
            }
            if !m.boxes.is_empty() {
                look.boxes = Some(boxes(&m, &|i| s.textures[i], &mut id));
            }
            looks.states.insert(st, look);
        }
    }
    for (i, d) in c.items.iter().enumerate().skip(1) {
        let Some((ns, path)) = d.name.split_once(':') else { continue };
        let Some(m) = from_pack(&format!("{ns}:item/{path}")) else { continue };
        let icon = m.sprite.as_deref().filter(|t| !t.is_empty()).map(&mut id);
        let own = icon.unwrap_or(if d.icon == u32::MAX { 0 } else { d.icon });
        let model = (!m.boxes.is_empty()).then(|| boxes(&m, &|_| own, &mut id));
        looks.items.insert(i as u32, ItemLook { icon, model });
    }
    if !looks.states.is_empty() || !looks.items.is_empty() {
        log::info!("resource packs reshape {} block states and {} items", looks.states.len(), looks.items.len());
    }
    (looks, extra)
}
