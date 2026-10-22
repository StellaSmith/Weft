//! Asset lookup. A name like `base:block/stone` resolves to `textures/block/stone.png` inside the
//! `base` mod's assets, unless an enabled resource pack provides `base/textures/block/stone.png`.

use std::path::{Path, PathBuf};

use rayon::prelude::*;
use rustc_hash::FxHashMap;

pub struct Assets {
    mods: FxHashMap<String, PathBuf>,
    /// Highest priority first.
    packs: Vec<PathBuf>,
}

impl Assets {
    pub fn new(mods: impl IntoIterator<Item = (String, PathBuf)>, packs: Vec<PathBuf>) -> Assets {
        Assets { mods: mods.into_iter().collect(), packs }
    }

    /// `kind` is the top folder ("textures", "lang", "ui", "shaders").
    pub fn path(&self, name: &str, kind: &str, ext: &str) -> Option<PathBuf> {
        let (ns, rel) = name.split_once(':').unwrap_or(("core", name));
        let file = format!("{kind}/{rel}.{ext}");
        for p in &self.packs {
            let candidate = p.join(ns).join(&file);
            if candidate.exists() {
                return Some(candidate);
            }
        }
        let candidate = self.mods.get(ns)?.join(&file);
        candidate.exists().then_some(candidate)
    }

    pub fn read(&self, name: &str, kind: &str, ext: &str) -> Option<Vec<u8>> {
        std::fs::read(self.path(name, kind, ext)?).ok()
    }

    /// Every file of a kind across mods (pack overrides win), keyed by `ns:relative/path`.
    pub fn list(&self, kind: &str, ext: &str) -> Vec<(String, PathBuf)> {
        let mut out: FxHashMap<String, PathBuf> = FxHashMap::default();
        let mut add = |ns: &str, root: &Path| {
            let dir = root.join(kind);
            walk(&dir, &mut |p| {
                if p.extension().and_then(|e| e.to_str()) == Some(ext) {
                    let rel = p.strip_prefix(&dir).unwrap().with_extension("");
                    let rel = rel.to_string_lossy().replace('\\', "/");
                    out.insert(format!("{ns}:{rel}"), p.to_path_buf());
                }
            });
        };
        for (ns, root) in &self.mods {
            add(ns, root);
        }
        for pack in self.packs.iter().rev() {
            for (ns, _) in &self.mods {
                add(ns, &pack.join(ns));
            }
        }
        let mut v: Vec<_> = out.into_iter().collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    /// Directories to watch for hot reload.
    pub fn roots(&self) -> Vec<PathBuf> {
        self.mods.values().cloned().chain(self.packs.iter().cloned()).collect()
    }
}

fn walk(dir: &Path, f: &mut impl FnMut(&Path)) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, f);
        } else {
            f(&p);
        }
    }
}

pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub fn decode_png(bytes: &[u8]) -> Option<Image> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16 | png::Transformations::ALPHA);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width, info.height);
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf[..(w * h * 4) as usize].to_vec(),
        png::ColorType::GrayscaleAlpha => buf[..(w * h * 2) as usize]
            .chunks_exact(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Rgb => buf[..(w * h * 3) as usize].chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::Grayscale => buf[..(w * h) as usize].iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    Some(Image { width: w, height: h, rgba })
}

pub fn encode_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(file, width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().map_err(std::io::Error::other)?;
    w.write_image_data(rgba).map_err(std::io::Error::other)?;
    Ok(())
}

fn missing(size: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let on = ((x * 2 / size) + (y * 2 / size)) % 2 == 0;
            v.extend_from_slice(if on { &[255, 0, 220, 255] } else { &[0, 0, 0, 255] });
        }
    }
    v
}

/// All frames of all textures as equal-size layers, plus animation info per named texture.
pub struct TextureArray {
    pub size: u32,
    pub layers: u32,
    /// Mip levels, each holding all layers.
    pub mips: Vec<Vec<u8>>,
    /// Per input texture: (first layer, frame count).
    pub entries: Vec<(u32, u32)>,
}

/// Animated textures are vertical strips of square frames.
pub fn build_array(assets: &Assets, names: &[String]) -> TextureArray {
    let images: Vec<Option<Image>> = names
        .par_iter()
        .map(|n| {
            if n == "core:missing" {
                return None;
            }
            let img = assets.read(n, "textures", "png").and_then(|b| decode_png(&b));
            if img.is_none() {
                log::warn!("missing texture {n}");
            }
            img
        })
        .collect();
    let size = images.iter().flatten().map(|i| i.width).max().unwrap_or(16).max(1).next_power_of_two();
    let mut layers: Vec<Vec<u8>> = Vec::new();
    let mut entries = Vec::new();
    for img in &images {
        let first = layers.len() as u32;
        match img {
            Some(img) if img.height >= img.width && img.height % img.width == 0 => {
                let frames = img.height / img.width;
                for f in 0..frames {
                    let frame = &img.rgba[(f * img.width * img.width * 4) as usize..((f + 1) * img.width * img.width * 4) as usize];
                    layers.push(scale_nearest(frame, img.width, size));
                }
                entries.push((first, frames));
            }
            Some(img) => {
                log::warn!("texture {}x{} is not square or a vertical strip", img.width, img.height);
                layers.push(missing(size));
                entries.push((first, 1));
            }
            None => {
                layers.push(missing(size));
                entries.push((first, 1));
            }
        }
    }
    let mip_count = size.trailing_zeros() + 1;
    let mut mips = Vec::with_capacity(mip_count as usize);
    let mut current = layers;
    let mut s = size;
    loop {
        mips.push(current.concat());
        if s == 1 {
            break;
        }
        current = current.iter().map(|l| downsample(l, s)).collect();
        s /= 2;
    }
    TextureArray { size, layers: entries.last().map_or(0, |e| e.0 + e.1), mips, entries }
}

fn scale_nearest(src: &[u8], from: u32, to: u32) -> Vec<u8> {
    if from == to {
        return src.to_vec();
    }
    let mut out = vec![0u8; (to * to * 4) as usize];
    for y in 0..to {
        for x in 0..to {
            let sx = x * from / to;
            let sy = y * from / to;
            let s = ((sy * from + sx) * 4) as usize;
            let d = ((y * to + x) * 4) as usize;
            out[d..d + 4].copy_from_slice(&src[s..s + 4]);
        }
    }
    out
}

/// Alpha-weighted 2x2 box filter, so transparent texels do not darken cutout edges.
fn downsample(src: &[u8], size: u32) -> Vec<u8> {
    let half = size / 2;
    let mut out = vec![0u8; (half * half * 4) as usize];
    for y in 0..half {
        for x in 0..half {
            let mut rgb = [0u32; 3];
            let mut a_sum = 0u32;
            let mut max_a = 0u32;
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let i = (((y * 2 + dy) * size + x * 2 + dx) * 4) as usize;
                let a = src[i + 3] as u32;
                for c in 0..3 {
                    rgb[c] += src[i + c] as u32 * a;
                }
                a_sum += a;
                max_a = max_a.max(a);
            }
            let d = ((y * half + x) * 4) as usize;
            for c in 0..3 {
                out[d + c] = if a_sum > 0 { (rgb[c] / a_sum) as u8 } else { 0 };
            }
            // Keep cutout textures (leaves) from fading out at distance.
            out[d + 3] = if max_a == 255 && a_sum >= 255 * 2 { 255 } else { (a_sum / 4) as u8 };
        }
    }
    out
}
