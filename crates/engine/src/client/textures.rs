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
    pub item_array: TextureId,
    pub item_index: u32,
    pub item_layers: Vec<(u32, u32)>,
}

fn upload(gpu: &mut dyn Backend, assets: &Assets, names: &[String], composites: &[[u32; 4]], label: &str) -> (TextureId, Vec<(u32, u32)>, Vec<Option<crate::assets::Anim>>) {
    let t = std::time::Instant::now();
    let array = build_array(assets, names, composites);
    let tex = gpu.create_texture(
        &TextureDesc {
            width: array.size,
            height: array.size,
            layers: Some(array.layers.max(1)),
            mips: array.mips.len() as u32,
            format: Format::Rgba8Srgb,
            render_target: false,
        },
        label,
    );
    for (i, mip) in array.mips.iter().enumerate() {
        gpu.write_texture(tex, i as u32, mip);
    }
    log::info!("{label}: {} textures, {} layers of {}px in {:.1} ms", names.len(), array.layers, array.size, t.elapsed().as_secs_f64() * 1000.0);
    (tex, array.entries, array.anims)
}

impl GameTextures {
    pub fn destroy(&self, gpu: &mut dyn Backend) {
        gpu.destroy_texture(self.block_array);
        gpu.destroy_texture(self.item_array);
    }

    /// Also loads the connected texture rules, whose tiles go into the block array.
    pub fn new(gpu: &mut dyn Backend, c: &Content, assets: &Assets) -> GameTextures {
        let (ctm, extra) = super::ctm::load(assets, c);
        super::ctm::set(ctm);
        let names: Vec<String> = c.block_textures.iter().chain(&extra.names).cloned().collect();
        let (block_array, block_layers, block_anims) = upload(gpu, assets, &names, &extra.composites, "block textures");
        let (item_array, item_layers, _) = upload(gpu, assets, &c.item_textures, &[], "item textures");
        GameTextures {
            block_index: gpu.texture_index(block_array),
            block_array,
            block_layers,
            block_anims,
            item_index: gpu.texture_index(item_array),
            item_array,
            item_layers,
        }
    }
}
