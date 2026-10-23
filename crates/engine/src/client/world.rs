use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender, unbounded};
use glam::{IVec2, IVec3};
use rustc_hash::{FxHashMap, FxHashSet};

use super::mesher::{self, Mesh, Neighborhood, PAD_VOLUME, pidx};
use crate::content::{AIR, content};
use crate::world::{Cube, cube_of, local_index};

pub struct MeshResult {
    pub pos: IVec3,
    pub version: u32,
    pub mesh: Mesh,
}

pub struct ClientWorld {
    pub dim: u32,
    pub cubes: FxHashMap<IVec3, Arc<Cube>>,
    pub time: u64,
    dirty: FxHashSet<IVec3>,
    versions: FxHashMap<IVec3, u32>,
    in_flight: FxHashSet<IVec3>,
    tx: Sender<MeshResult>,
    rx: Receiver<MeshResult>,
    pub removed: Vec<IVec3>,
    next_version: u32,
    /// Per cube, faces whose neighbour the server is not going to send: meshed as solid.
    hidden: FxHashMap<IVec3, u8>,
    /// An opaque state standing in for hidden neighbours.
    filler: u32,
    columns: FxHashMap<IVec2, Column>,
    /// Columns whose blended biome colours are out of date.
    tints_dirty: FxHashSet<IVec2>,
}

/// Biomes of a block column, taken from the first of its cubes that has them; cubes store none
/// when every biome is 0.
struct Column {
    biomes: Option<Box<[u8; 256]>>,
    cubes: u32,
}

/// Biome colours are averaged over (2 BLEND + 1)^2 blocks.
const BLEND: i32 = 4;
/// About 9 us each; while loading, columns get redone as neighbours arrive.
const TINTS_PER_FRAME: usize = 64;

/// Grass, foliage and water colours of one block column, 0xFFRRGGBB.
pub type ColumnTints = [[u32; 4]; 256];

impl ClientWorld {
    pub fn new(dim: u32) -> Self {
        let (tx, rx) = unbounded();
        ClientWorld {
            dim,
            cubes: FxHashMap::default(),
            time: 0,
            dirty: FxHashSet::default(),
            versions: FxHashMap::default(),
            in_flight: FxHashSet::default(),
            tx,
            rx,
            removed: Vec::new(),
            next_version: 0,
            hidden: FxHashMap::default(),
            filler: content().states.iter().position(|s| s.flags & voxide_abi::block_flags::OPAQUE != 0).unwrap_or(0) as u32,
            columns: FxHashMap::default(),
            tints_dirty: FxHashSet::default(),
        }
    }

    /// Whether the neighbour of `p` in direction `d` is present or never coming.
    fn neighbour_known(&self, p: IVec3, d: usize, min_y: i32, max_y: i32) -> bool {
        let n = p + crate::world::DIRS[d];
        self.cubes.contains_key(&n) || n.y < min_y || n.y > max_y || self.hidden.get(&p).is_some_and(|h| h & (1 << d) != 0)
    }

    pub fn clear(&mut self) {
        self.removed.extend(self.cubes.keys().copied());
        self.cubes.clear();
        self.dirty.clear();
        self.versions.clear();
        self.hidden.clear();
        self.columns.clear();
        self.tints_dirty.clear();
    }

    /// Versions come from one counter so a mesh job of a removed cube can never match a new cube
    /// at the same position (dimension change).
    fn bump(&mut self, pos: IVec3) {
        self.next_version += 1;
        self.versions.insert(pos, self.next_version);
    }

    fn touch(&mut self, pos: IVec3, neighbours: bool) {
        self.bump(pos);
        self.dirty.insert(pos);
        if neighbours {
            for dz in -1..=1 {
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        let n = pos + IVec3::new(dx, dy, dz);
                        if n != pos && self.cubes.contains_key(&n) {
                            self.bump(n);
                            self.dirty.insert(n);
                        }
                    }
                }
            }
        }
    }

    pub fn insert(&mut self, pos: IVec3, cube: Arc<Cube>, hidden: u8) {
        if hidden != crate::net::KEEP_HIDDEN {
            self.hidden.insert(pos, hidden);
        }
        let col = IVec2::new(pos.x, pos.z);
        let mut fresh = false;
        let column = self.columns.entry(col).or_insert_with(|| {
            fresh = true;
            Column { biomes: None, cubes: 0 }
        });
        if fresh || column.biomes.is_none() && cube.biomes.is_some() {
            column.biomes = column.biomes.take().or_else(|| cube.biomes.clone());
            for dz in -1..=1 {
                for dx in -1..=1 {
                    self.tints_dirty.insert(col + IVec2::new(dx, dz));
                }
            }
        }
        if self.cubes.insert(pos, cube).is_none() {
            self.columns.get_mut(&col).unwrap().cubes += 1;
        }
        self.touch(pos, true);
    }

    pub fn remove(&mut self, pos: IVec3) {
        if self.cubes.remove(&pos).is_some() {
            let col = IVec2::new(pos.x, pos.z);
            if let Some(c) = self.columns.get_mut(&col) {
                c.cubes -= 1;
                if c.cubes == 0 {
                    self.columns.remove(&col);
                }
            }
            self.hidden.remove(&pos);
            self.dirty.remove(&pos);
            self.versions.remove(&pos);
            self.removed.push(pos);
        }
    }

    pub fn set_block(&mut self, p: IVec3, state: u32) {
        let cp = cube_of(p);
        let Some(cube) = self.cubes.get_mut(&cp) else { return };
        Arc::make_mut(cube).set(local_index(p), state);
        let l = p & 15;
        let border = l.x == 0 || l.x == 15 || l.y == 0 || l.y == 15 || l.z == 0 || l.z == 15;
        self.touch(cp, border);
    }

    /// Remeshes every cube, e.g. after textures were reloaded.
    pub fn remesh_all(&mut self) {
        let all: Vec<IVec3> = self.cubes.keys().copied().collect();
        for p in all {
            self.touch(p, false);
        }
    }

    pub fn retint_all(&mut self) {
        self.tints_dirty.extend(self.columns.keys().copied());
    }

    /// Recomputes the blended colours of columns whose biomes or neighbours changed. Only columns
    /// at most `reach` from `center` are done: the GPU keeps a window of 2 reach + 1 columns, and
    /// cubes of an area the player just left would overwrite the ones around them.
    pub fn take_tints(&mut self, center: IVec2, reach: i32) -> Vec<(IVec2, Box<ColumnTints>)> {
        let mut out = Vec::new();
        let dirty: Vec<IVec2> = self.tints_dirty.iter().copied().collect();
        for c in dirty {
            if !self.columns.contains_key(&c) {
                self.tints_dirty.remove(&c);
            } else if (c - center).abs().max_element() <= reach && out.len() < TINTS_PER_FRAME {
                self.tints_dirty.remove(&c);
                out.push((c, self.blend_column(c)));
            }
        }
        out
    }

    fn blend_column(&self, col: IVec2) -> Box<ColumnTints> {
        const W: usize = 16 + 2 * BLEND as usize;
        let c = content();
        let fallback = crate::content::BiomeDef { name: String::new(), grass: 0x7fb238, foliage: 0x48b518, water: 0x3f76e4, temperature: 0.5, rainfall: 0.5 };
        let get = |c: &Column, x: i32, z: i32| c.biomes.as_ref().map_or(0, |b| b[(z * 16 + x) as usize]);
        let own = &self.columns[&col];
        // Blocks of missing neighbour columns repeat this column's edge.
        let biome_at = |x: i32, z: i32| -> u8 {
            match self.columns.get(&(col + IVec2::new(x.div_euclid(16), z.div_euclid(16)))) {
                Some(other) => get(other, x.rem_euclid(16), z.rem_euclid(16)),
                None => get(own, x.clamp(0, 15), z.clamp(0, 15)),
            }
        };
        // Per tint and channel, summed along x then along z.
        let mut rows = vec![[[0u32; 3]; 3]; W * 16];
        for gz in 0..W {
            let mut line = [[[0u32; 3]; 3]; W];
            for (gx, v) in line.iter_mut().enumerate() {
                let b = c.biomes.get(biome_at(gx as i32 - BLEND, gz as i32 - BLEND) as usize).unwrap_or(c.biomes.first().unwrap_or(&fallback));
                for (t, rgb) in [b.grass, b.foliage, b.water].into_iter().enumerate() {
                    v[t] = [rgb >> 16 & 255, rgb >> 8 & 255, rgb & 255];
                }
            }
            for x in 0..16 {
                let out = &mut rows[gz * 16 + x];
                for v in &line[x..x + 2 * BLEND as usize + 1] {
                    for t in 0..3 {
                        for ch in 0..3 {
                            out[t][ch] += v[t][ch];
                        }
                    }
                }
            }
        }
        let n = ((2 * BLEND + 1) * (2 * BLEND + 1)) as u32;
        let mut out = Box::new([[0u32; 4]; 256]);
        for z in 0..16 {
            for x in 0..16 {
                let mut sum = [[0u32; 3]; 3];
                for gz in z..z + 2 * BLEND as usize + 1 {
                    for t in 0..3 {
                        for ch in 0..3 {
                            sum[t][ch] += rows[gz * 16 + x][t][ch];
                        }
                    }
                }
                for t in 0..3 {
                    let [r, g, b] = sum[t].map(|s| (s + n / 2) / n);
                    out[z * 16 + x][t] = 0xFF00_0000 | r << 16 | g << 8 | b;
                }
            }
        }
        out
    }

    /// `u32::MAX` when not loaded.
    pub fn block(&self, p: IVec3) -> u32 {
        self.cubes.get(&cube_of(p)).map_or(u32::MAX, |c| c.get(local_index(p)))
    }

    pub fn light(&self, p: IVec3) -> u32 {
        self.cubes.get(&cube_of(p)).map_or(15, |c| c.light(local_index(p)))
    }

    /// Starts meshing jobs for dirty cubes nearest to `center` first.
    pub fn schedule_meshing(&mut self, center: IVec3, max_in_flight: usize) {
        if self.dirty.is_empty() || self.in_flight.len() >= max_in_flight {
            return;
        }
        let c = content();
        let def = &c.dims[self.dim as usize];
        let mut ready: Vec<(i32, IVec3)> = self
            .dirty
            .iter()
            .copied()
            .filter(|&p| !self.in_flight.contains(&p))
            .filter(|&p| (0..6).all(|d| self.neighbour_known(p, d, def.min_cube_y, def.max_cube_y)))
            .map(|p| ((p - center).length_squared(), p))
            .collect();
        ready.sort_by_key(|r| r.0);
        for (_, pos) in ready.into_iter().take(max_in_flight - self.in_flight.len()) {
            self.dirty.remove(&pos);
            let center_cube = self.cubes[&pos].clone();
            // Air cubes never produce faces themselves; neighbours draw the faces facing them.
            if center_cube.uniform_state() == Some(AIR) {
                let _ = self.tx.send(MeshResult { pos, version: self.versions[&pos], mesh: Mesh::default() });
                self.in_flight.insert(pos);
                continue;
            }
            let mut around: [Option<Arc<Cube>>; 27] = Default::default();
            for dz in -1..=1 {
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        let i = ((dy + 1) * 9 + (dz + 1) * 3 + dx + 1) as usize;
                        around[i] = self.cubes.get(&(pos + IVec3::new(dx, dy, dz))).cloned();
                    }
                }
            }
            let version = self.versions[&pos];
            let tx = self.tx.clone();
            self.in_flight.insert(pos);
            let hidden = self.hidden.get(&pos).copied().unwrap_or(0);
            let filler = self.filler;
            let ctm = super::ctm::current();
            rayon::spawn(move || {
                let n = neighborhood(&around, hidden, filler, pos);
                let mesh = mesher::build(content(), &n, &ctm);
                let _ = tx.send(MeshResult { pos, version, mesh });
            });
        }
    }

    /// Finished meshes that are still current.
    pub fn take_meshes(&mut self) -> Vec<MeshResult> {
        let mut out = Vec::new();
        while let Ok(r) = self.rx.try_recv() {
            self.in_flight.remove(&r.pos);
            if self.versions.get(&r.pos) == Some(&r.version) {
                out.push(r);
            }
        }
        out
    }

    /// Jobs running plus dirty cubes that could be meshed now (edge cubes wait for neighbours
    /// that may never come and are not counted).
    pub fn pending_meshes(&self) -> usize {
        let def = &content().dims[self.dim as usize];
        let ready = self
            .dirty
            .iter()
            .filter(|&&p| (0..6).all(|d| self.neighbour_known(p, d, def.min_cube_y, def.max_cube_y)))
            .count();
        ready + self.in_flight.len()
    }
}

fn neighborhood(around: &[Option<Arc<Cube>>; 27], hidden: u8, filler: u32, pos: IVec3) -> Neighborhood {
    let mut states = Box::new([AIR; PAD_VOLUME]);
    let mut light = Box::new([15u32; PAD_VOLUME]);
    let mut biomes = [0u8; 256];
    let center = around[13].as_ref().unwrap();
    for z in 0..16 {
        for x in 0..16 {
            biomes[z * 16 + x] = center.biome(x, z);
        }
    }
    for y in -1..17i32 {
        let cy = if y < 0 { 0 } else if y > 15 { 2 } else { 1 };
        for z in -1..17i32 {
            let cz = if z < 0 { 0 } else if z > 15 { 2 } else { 1 };
            for x in -1..17i32 {
                let cx = if x < 0 { 0 } else if x > 15 { 2 } else { 1 };
                let Some(cube) = &around[cy * 9 + cz * 3 + cx] else {
                    // A face neighbour that will never be sent is solid ground: no faces toward it.
                    let face = match (cx, cy, cz) {
                        (0, 1, 1) => Some(0),
                        (2, 1, 1) => Some(1),
                        (1, 0, 1) => Some(2),
                        (1, 2, 1) => Some(3),
                        (1, 1, 0) => Some(4),
                        (1, 1, 2) => Some(5),
                        _ => None,
                    };
                    if face.is_some_and(|f| hidden & (1 << f) != 0) {
                        let pi = pidx(x, y, z);
                        states[pi] = filler;
                        light[pi] = 0;
                    }
                    continue;
                };
                let li = crate::world::cube::index((x & 15) as usize, (y & 15) as usize, (z & 15) as usize);
                let pi = pidx(x, y, z);
                states[pi] = cube.get(li);
                light[pi] = cube.light(li);
            }
        }
    }
    Neighborhood { states, light, biomes, origin: (pos * 16).to_array() }
}
