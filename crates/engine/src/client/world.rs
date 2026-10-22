use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender, unbounded};
use glam::IVec3;
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
}

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
        }
    }

    pub fn clear(&mut self) {
        self.removed.extend(self.cubes.keys().copied());
        self.cubes.clear();
        self.dirty.clear();
        self.versions.clear();
    }

    fn touch(&mut self, pos: IVec3, neighbours: bool) {
        *self.versions.entry(pos).or_insert(0) += 1;
        self.dirty.insert(pos);
        if neighbours {
            for dz in -1..=1 {
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        let n = pos + IVec3::new(dx, dy, dz);
                        if n != pos && self.cubes.contains_key(&n) {
                            *self.versions.entry(n).or_insert(0) += 1;
                            self.dirty.insert(n);
                        }
                    }
                }
            }
        }
    }

    pub fn insert(&mut self, pos: IVec3, cube: Arc<Cube>) {
        self.cubes.insert(pos, cube);
        self.touch(pos, true);
    }

    pub fn remove(&mut self, pos: IVec3) {
        if self.cubes.remove(&pos).is_some() {
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
            .filter(|&p| {
                crate::world::DIRS.iter().all(|d| {
                    let n = p + *d;
                    self.cubes.contains_key(&n) || n.y < def.min_cube_y || n.y > def.max_cube_y
                })
            })
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
            rayon::spawn(move || {
                let n = neighborhood(&around);
                let mesh = mesher::build(content(), &n);
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
            .filter(|&&p| {
                crate::world::DIRS.iter().all(|d| {
                    let n = p + *d;
                    self.cubes.contains_key(&n) || n.y < def.min_cube_y || n.y > def.max_cube_y
                })
            })
            .count();
        ready + self.in_flight.len()
    }
}

fn neighborhood(around: &[Option<Arc<Cube>>; 27]) -> Neighborhood {
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
                let Some(cube) = &around[cy * 9 + cz * 3 + cx] else { continue };
                let li = crate::world::cube::index((x & 15) as usize, (y & 15) as usize, (z & 15) as usize);
                let pi = pidx(x, y, z);
                states[pi] = cube.get(li);
                light[pi] = cube.light(li);
            }
        }
    }
    Neighborhood { states, light, biomes }
}
