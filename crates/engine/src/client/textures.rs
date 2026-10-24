use voxide_render::*;

use crate::assets::{Assets, build_array};
use crate::content::Content;

/// Block and item texture arrays, shared by the world renderer and the UI (item icons).
pub struct GameTextures {
    pub block_array: TextureId,
    pub block_index: u32,
    /// Per block texture name: (first layer, frames).
    pub block_layers: Vec<(u32, u32)>,
    pub block_anims: Vec<Option<crate::assets::Anim>>,
    /// Glow masks of the block textures (same layers, see `assets::emission_mask`) and which
    /// textures have one; `None` when none has.
    pub block_glow: Option<(TextureId, u32)>,
    pub block_emissive: Vec<bool>,
    pub item_array: TextureId,
    pub item_index: u32,
    pub item_layers: Vec<(u32, u32)>,
    pub item_glow: Option<(TextureId, u32)>,
    pub particle_array: TextureId,
    pub particle_index: u32,
    pub particle_layers: Vec<(u32, u32)>,
    /// Per block texture: its overlay texture + 1, or 0 (see `ctm::Ctm::overlay`).
    pub overlays: Vec<u32>,
}

struct Uploaded {
    tex: TextureId,
    entries: Vec<(u32, u32)>,
    anims: Vec<Option<crate::assets::Anim>>,
    glow: Option<(TextureId, u32)>,
    emissive: Vec<bool>,
}

fn upload(gpu: &mut dyn Backend, assets: &Assets, names: &[String], composites: &[[u32; 4]], label: &str) -> Uploaded {
    let t = std::time::Instant::now();
    let array = build_array(assets, names, composites);
    let layers = array.layers.max(1);
    let tex = gpu.create_texture(&TextureDesc { width: array.size, height: array.size, layers: Some(layers), mips: array.mips.len() as u32, format: Format::Rgba8Srgb, render_target: false }, label);
    for (i, mip) in array.mips.iter().enumerate() {
        gpu.write_texture(tex, i as u32, mip);
    }
    let glow = array.emission.map(|bytes| {
        let g = gpu.create_texture(&TextureDesc { width: array.size, height: array.size, layers: Some(layers), mips: 1, format: Format::R8Unorm, render_target: false }, label);
        gpu.write_texture(g, 0, &bytes);
        (g, gpu.texture_index(g))
    });
    let note = if glow.is_some() { ", with glow masks" } else { "" };
    log::info!("{label}: {} textures, {} layers of {}px{note} in {:.1} ms", names.len(), array.layers, array.size, t.elapsed().as_secs_f64() * 1000.0);
    Uploaded { tex, entries: array.entries, anims: array.anims, glow, emissive: array.emissive }
}

impl GameTextures {
    pub fn destroy(&self, gpu: &mut dyn Backend) {
        for t in [self.block_array, self.item_array, self.particle_array].into_iter().chain(self.block_glow.map(|g| g.0)).chain(self.item_glow.map(|g| g.0)) {
            gpu.destroy_texture(t);
        }
    }

    /// Also loads the connected texture rules, whose tiles go into the block array.
    pub fn new(gpu: &mut dyn Backend, c: &Content, assets: &Assets) -> GameTextures {
        let (ctm, extra) = super::ctm::load(assets, c);
        let overlays = ctm.overlay.clone();
        super::ctm::set(ctm);
        let names: Vec<String> = c.block_textures.iter().chain(&extra.names).cloned().collect();
        let block = upload(gpu, assets, &names, &extra.composites, "block textures");
        let item = upload(gpu, assets, &c.item_textures, &[], "item textures");
        let particle = upload(gpu, assets, &c.game.particle_textures, &[], "particle textures");
        GameTextures {
            block_index: gpu.texture_index(block.tex),
            block_array: block.tex,
            block_layers: block.entries,
            block_anims: block.anims,
            block_glow: block.glow,
            block_emissive: block.emissive,
            item_index: gpu.texture_index(item.tex),
            item_array: item.tex,
            item_layers: item.entries,
            item_glow: item.glow,
            particle_index: gpu.texture_index(particle.tex),
            particle_array: particle.tex,
            particle_layers: particle.entries,
            overlays,
        }
    }

    /// Glow mask array index that goes with texture array `array`, `u32::MAX` if it has none.
    pub fn glow_of(&self, array: u32) -> u32 {
        let pick = |g: Option<(TextureId, u32)>| g.map_or(u32::MAX, |g| g.1);
        if array == self.block_index {
            pick(self.block_glow)
        } else if array == self.item_index {
            pick(self.item_glow)
        } else {
            u32::MAX
        }
    }
}
