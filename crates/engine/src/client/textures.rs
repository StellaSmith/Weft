use voxide_render::*;

use crate::assets::{Assets, build_array};
use crate::content::Content;

/// Block and item texture arrays, shared by the world renderer and the UI (item icons).
pub struct GameTextures {
    pub block_array: TextureId,
    pub block_index: u32,
    /// Per block texture name: (first layer, frames).
    pub block_layers: Vec<(u32, u32)>,
    pub item_array: TextureId,
    pub item_index: u32,
    pub item_layers: Vec<(u32, u32)>,
}

fn upload(gpu: &mut dyn Backend, assets: &Assets, names: &[String], label: &str) -> (TextureId, Vec<(u32, u32)>) {
    let t = std::time::Instant::now();
    let array = build_array(assets, names);
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
    (tex, array.entries)
}

impl GameTextures {
    pub fn new(gpu: &mut dyn Backend, c: &Content, assets: &Assets) -> GameTextures {
        let (block_array, block_layers) = upload(gpu, assets, &c.block_textures, "block textures");
        let (item_array, item_layers) = upload(gpu, assets, &c.item_textures, "item textures");
        GameTextures {
            block_index: gpu.texture_index(block_array),
            block_array,
            block_layers,
            item_index: gpu.texture_index(item_array),
            item_array,
            item_layers,
        }
    }
}
