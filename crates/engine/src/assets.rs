//! Asset lookup. A name like `base:block/stone` resolves to `textures/block/stone.png` inside the
//! `base` mod's assets, unless an enabled resource pack provides `base/textures/block/stone.png`.

use std::path::{Path, PathBuf};

use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};

pub struct Assets {
    mods: FxHashMap<String, PathBuf>,
    /// Highest priority first.
    packs: Vec<PathBuf>,
    /// Every file under the roots, listed once: a content-heavy mod asks for thousands of files
    /// (and for optional ones like `.anim` that mostly do not exist), and each existence check
    /// costs a system call.
    files: FxHashSet<PathBuf>,
}

impl Assets {
    pub fn new(mods: impl IntoIterator<Item = (String, PathBuf)>, packs: Vec<PathBuf>) -> Assets {
        let mut a = Assets { mods: mods.into_iter().collect(), packs, files: FxHashSet::default() };
        a.rescan();
        a
    }

    /// Resource pack folders, highest priority first; call `rescan` after.
    pub fn set_packs(&mut self, packs: Vec<PathBuf>) {
        self.packs = packs;
    }

    /// Re-lists files after they changed on disk (hot reload).
    pub fn rescan(&mut self) {
        let mut files = FxHashSet::default();
        for root in self.mods.values().chain(&self.packs) {
            walk(root, &mut |p| {
                files.insert(p.to_path_buf());
            });
        }
        self.files = files;
    }

    /// `kind` is the top folder ("textures", "lang", "ui", "shaders").
    pub fn path(&self, name: &str, kind: &str, ext: &str) -> Option<PathBuf> {
        let (ns, rel) = name.split_once(':').unwrap_or(("core", name));
        let file = format!("{kind}/{rel}.{ext}");
        for p in &self.packs {
            let candidate = p.join(ns).join(&file);
            if self.files.contains(&candidate) {
                return Some(candidate);
            }
        }
        let candidate = self.mods.get(ns)?.join(&file);
        self.files.contains(&candidate).then_some(candidate)
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

pub fn decode_png_file(path: &Path) -> Option<(u32, u32, Vec<u8>)> {
    let img = decode_png(&std::fs::read(path).ok()?)?;
    Some((img.width, img.height, img.rgba))
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
    /// Per input texture: animation, if it has more than one frame.
    pub anims: Vec<Option<Anim>>,
}

/// Frame sequence of an animated texture: (absolute layer, ticks shown).
#[derive(Clone, Debug, PartialEq)]
pub struct Anim {
    pub frames: Vec<(u32, u32)>,
    pub interpolate: bool,
}

/// `<texture>.anim` next to the png, same `key = value` style as other data files:
///
/// ```text
/// frametime = 2          # ticks per frame (default 1)
/// interpolate = true     # blend towards the next frame
/// frames = 0 1 2 3*5 2 1 # order; `3*5` shows frame 3 for 5 ticks
/// ```
pub fn parse_anim(src: &str, frame_count: u32, first: u32) -> Anim {
    let mut frametime = 1u32;
    let mut interpolate = false;
    let mut order: Option<Vec<(u32, Option<u32>)>> = None;
    for line in src.lines() {
        let line = line.split('#').next().unwrap_or("");
        let Some((k, v)) = line.split_once('=') else { continue };
        match k.trim() {
            "frametime" => frametime = v.trim().parse().unwrap_or(1).max(1),
            "interpolate" => interpolate = v.trim() == "true",
            "frames" => {
                order = Some(
                    v.split_whitespace()
                        .filter_map(|t| match t.split_once('*') {
                            Some((i, d)) => Some((i.parse().ok()?, d.parse().ok())),
                            None => Some((t.parse().ok()?, None)),
                        })
                        .filter(|(i, _)| *i < frame_count)
                        .collect(),
                )
            }
            _ => {}
        }
    }
    let order = order.filter(|o| !o.is_empty()).unwrap_or_else(|| (0..frame_count).map(|i| (i, None)).collect());
    Anim { frames: order.into_iter().map(|(i, d)| (first + i, d.unwrap_or(frametime).max(1))).collect(), interpolate }
}

/// Animated textures are vertical strips of square frames. Each composite is one more texture
/// made of the top-left, top-right, bottom-left and bottom-right quarters of the first frames of
/// four textures given by index.
pub fn build_array(assets: &Assets, names: &[String], composites: &[[u32; 4]]) -> TextureArray {
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
    // Frames cut and scaled per texture in parallel, then laid out in name order.
    let per_texture: Vec<(Vec<Vec<u8>>, u32, Option<String>)> = images
        .par_iter()
        .zip(names)
        .map(|(img, name)| {
            let meta = assets.read(name, "textures", "anim").map(|b| String::from_utf8_lossy(&b).into_owned());
            match img {
                Some(img) if img.height >= img.width && img.height % img.width == 0 => {
                    let frames = img.height / img.width;
                    let frame_bytes = (img.width * img.width * 4) as usize;
                    let layers = (0..frames as usize).map(|f| scale_nearest(&img.rgba[f * frame_bytes..(f + 1) * frame_bytes], img.width, size)).collect();
                    (layers, frames, meta)
                }
                Some(img) => {
                    log::warn!("texture {name} is {}x{}, not square or a vertical strip", img.width, img.height);
                    (vec![missing(size)], 1, meta)
                }
                None => (vec![missing(size)], 1, meta),
            }
        })
        .collect();
    let mut layers: Vec<Vec<u8>> = Vec::new();
    let mut entries = Vec::new();
    let mut anims = Vec::new();
    for (frames, count, meta) in per_texture {
        let first = layers.len() as u32;
        anims.push(if count > 1 || meta.is_some() { Some(parse_anim(meta.as_deref().unwrap_or(""), count, first)) } else { None });
        entries.push((first, count));
        layers.extend(frames);
    }
    let half = (size / 2) as usize;
    for parts in composites {
        let mut out = vec![0u8; (size * size * 4) as usize];
        for (q, &src) in parts.iter().enumerate() {
            let Some(&(layer, _)) = entries.get(src as usize) else { continue };
            let from = &layers[layer as usize];
            let (qx, qy) = ((q % 2) * half, (q / 2) * half);
            for y in qy..qy + half {
                let row = (y * size as usize + qx) * 4;
                out[row..row + half * 4].copy_from_slice(&from[row..row + half * 4]);
            }
        }
        entries.push((layers.len() as u32, 1));
        anims.push(None);
        layers.push(out);
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
        current = current.par_iter().map(|l| downsample(l, s)).collect();
        s /= 2;
    }
    TextureArray { size, layers: entries.last().map_or(0, |e| e.0 + e.1), mips, entries, anims }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anim_order_and_durations() {
        let a = parse_anim("frametime = 2\ninterpolate = true\nframes = 0 2*5 1 9", 3, 10);
        assert!(a.interpolate);
        // Frame 9 does not exist and is dropped.
        assert_eq!(a.frames, vec![(10, 2), (12, 5), (11, 2)]);
        let d = parse_anim("", 4, 0);
        assert_eq!(d.frames.len(), 4);
        assert!(d.frames.iter().all(|f| f.1 == 1));
    }
}
