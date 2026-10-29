//! Flood-fill lighting across cubes. Four 8-bit channels in one u32: sky, then block red, green,
//! blue. Colored light is just three independent block channels; each block can absorb channels
//! differently (stained glass). Sky light keeps full strength going straight down through fully
//! transparent blocks.

use std::cell::Cell;
use std::collections::VecDeque;

use glam::IVec3;
use rustc_hash::FxHashSet;

use super::cube::{Cube, index};
use crate::content::{Content, Tight};

pub trait CubeMap {
    fn cube(&self, c: IVec3) -> Option<&Cube>;
    /// `None` for cubes that may be read but not written (light toward them is left for later).
    fn cube_mut(&mut self, c: IVec3) -> Option<&mut Cube>;
    fn writable(&self, _c: IVec3) -> bool {
        true
    }
}

const DIRS: [IVec3; 6] = [IVec3::NEG_X, IVec3::X, IVec3::NEG_Y, IVec3::Y, IVec3::NEG_Z, IVec3::Z];
pub const SKY_FULL: u8 = 15;

/// The one rule light moves by: from a block at `v` into one taking `op` (its opacity, or the
/// channel's absorption), losing at least one, except full sky going straight down (`down_sky`)
/// through clear blocks.
#[inline]
pub fn step(v: u8, op: u8, down_sky: bool) -> u8 {
    if down_sky && v == SKY_FULL && op == 0 { SKY_FULL } else { v.saturating_sub(op.max(1)) }
}

/// The most light at `v` gives a neighbor (`step` into a clear block).
#[inline]
pub fn reach(v: u8, down_sky: bool) -> u8 {
    step(v, 0, down_sky)
}

/// Nothing above cube height `y` can shade it: open sky (`sky_above`) or the dimension's top.
#[inline]
pub fn open_above(def: &crate::content::DimensionDef, y: i32) -> bool {
    y + 1 >= def.sky_cube_y || y + 1 > def.max_cube_y
}

/// Sky light a cube's bottom gives the cube under it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SkyBelow {
    /// Full sky everywhere: a cube lit as if under open sky (`prelight`) is right.
    Full,
    /// None anywhere: right without its sky light.
    Dark,
    /// Some: it has to be lit again.
    Partial,
}

/// What comes down from `above` (`None`: not loaded; `open`: open sky is there, `open_above`).
pub fn sky_below(above: Option<&Cube>, open: bool) -> SkyBelow {
    let Some(a) = above else { return if open { SkyBelow::Full } else { SkyBelow::Dark } };
    let sky = |i: usize| a.light(crate::world::cube::index(i % 16, 0, i / 16)) & 0xFF;
    let v = match a.uniform_light() {
        Some(l) => l & 0xFF,
        None if (1..256).all(|i| sky(i) == sky(0)) => sky(0),
        None => return SkyBelow::Partial,
    };
    match v {
        0 => SkyBelow::Dark,
        v if v == SKY_FULL as u32 => SkyBelow::Full,
        _ => SkyBelow::Partial,
    }
}
/// Channel index: 0 sky, 1..4 red, green, blue.
const SKY: usize = 0;

#[inline]
fn ch(v: u32, c: usize) -> u8 {
    (v >> (c * 8)) as u8
}

#[inline]
fn with(v: u32, c: usize, x: u8) -> u32 {
    (v & !(0xFF << (c * 8))) | ((x as u32) << (c * 8))
}

pub struct Lighter<'a, M: CubeMap> {
    pub map: &'a mut M,
    c: &'a Content,
    sky: bool,
    /// Cubes whose light changed, the caller resends them.
    pub changed: FxHashSet<IVec3>,
    add: VecDeque<(IVec3, u8)>,
    remove: VecDeque<(IVec3, u8)>,
    /// Last cube looked up. A flood fill stays inside one cube most of the time, so this saves
    /// almost every hash lookup. Null pointer = cached miss.
    cache: Cell<(IVec3, *const Cube)>,
    /// Last cube added to `changed`, to skip the hash insert on every write.
    last_changed: IVec3,
}

#[inline]
fn split(p: IVec3) -> (IVec3, usize) {
    (p >> 4, index((p.x & 15) as usize, (p.y & 15) as usize, (p.z & 15) as usize))
}

impl<'a, M: CubeMap> Lighter<'a, M> {
    pub fn new(map: &'a mut M, c: &'a Content, sky: bool) -> Self {
        Lighter {
            map,
            c,
            sky,
            changed: FxHashSet::default(),
            add: VecDeque::new(),
            remove: VecDeque::new(),
            cache: Cell::new((IVec3::MAX, std::ptr::null())),
            last_changed: IVec3::MAX,
        }
    }

    #[inline]
    fn lookup(&self, cp: IVec3) -> Option<&Cube> {
        let (k, ptr) = self.cache.get();
        if k == cp {
            return unsafe { ptr.as_ref() };
        }
        let r = self.map.cube(cp);
        self.cache.set((cp, r.map_or(std::ptr::null(), |c| c as *const Cube)));
        r
    }

    #[inline]
    fn read(&self, p: IVec3) -> Option<(u32, u32)> {
        let (c, i) = split(p);
        self.lookup(c).map(|cube| (cube.get(i), cube.light(i)))
    }

    #[inline]
    fn get(&self, p: IVec3, c: usize) -> Option<u8> {
        let (cp, i) = split(p);
        self.lookup(cp).map(|cube| ch(cube.light(i), c))
    }

    /// Whether the value there is `v` now.
    #[inline]
    fn put(&mut self, p: IVec3, c: usize, v: u8) -> bool {
        let (cp, i) = split(p);
        let Some(cube) = self.lookup(cp) else { return false };
        let old = cube.light(i);
        let new = with(old, c, v);
        if new != old {
            // Writing may give the map a fresh copy of a shared cube; keep the cache on it.
            let Some(cube) = self.map.cube_mut(cp) else { return false };
            cube.set_light(i, new);
            self.cache.set((cp, cube as *const Cube));
            if cp != self.last_changed {
                self.last_changed = cp;
                self.changed.insert(cp);
            }
        }
        true
    }

    #[inline]
    fn absorb(&self, state: u32, c: usize) -> u8 {
        let s = &self.c.tight[state as usize];
        if c == SKY { s.opacity } else { s.absorb[c - 1] }
    }

    #[inline]
    fn emit(&self, state: u32, c: usize) -> u8 {
        if c == SKY { 0 } else { self.c.tight[state as usize].light[c - 1] }
    }

    fn propagate(&mut self, c: usize) {
        let mut steps = self.add.len() as u64;
        while let Some((p, v)) = self.add.pop_front() {
            // Stale entry, a brighter value was written after it was queued.
            if self.get(p, c) != Some(v) {
                continue;
            }
            for (d, dir) in DIRS.iter().enumerate() {
                let n = p + *dir;
                let Some((state, light)) = self.read(n) else { continue };
                let op = self.absorb(state, c);
                if op >= 15 && self.c.tight[state as usize].opacity >= 15 {
                    continue;
                }
                let nv = step(v, op, c == SKY && d == 2);
                if nv > ch(light, c) && self.put(n, c, nv) {
                    self.add.push_back((n, nv));
                    steps += 1;
                }
            }
        }
        crate::diag::LIGHT_STEPS.add(steps);
    }

    fn unpropagate(&mut self, c: usize) {
        while let Some((p, v)) = self.remove.pop_front() {
            for (d, dir) in DIRS.iter().enumerate() {
                let n = p + *dir;
                let Some(nl) = self.get(n, c) else { continue };
                if nl == 0 {
                    continue;
                }
                let derived = nl <= reach(v, c == SKY && d == 2);
                if derived {
                    let (state, _) = self.read(n).unwrap();
                    let e = self.emit(state, c);
                    self.put(n, c, e);
                    self.remove.push_back((n, nl));
                    if e > 0 {
                        self.add.push_back((n, e));
                    }
                } else {
                    self.add.push_back((n, nl));
                }
            }
        }
    }

    fn channels(&self) -> &'static [usize] {
        if self.sky { &[0, 1, 2, 3] } else { &[1, 2, 3] }
    }

    /// Lights a freshly inserted cube and spreads its light into loaded neighbors.
    /// `open_above`: the cube above is unloaded but known to be open sky.
    pub fn light_new(&mut self, cp: IVec3, open_above: bool) {
        crate::diag::LIGHT_CUBES.add(1);
        let base = cp * 16;
        let Some(cube) = self.map.cube(cp) else { return };
        let states = &self.c.tight;
        let all_clear = cube.palette().iter().all(|&s| states[s as usize].opacity == 0);
        let all_opaque = cube.palette().iter().all(|&s| states[s as usize].opacity >= 15);
        let emits = cube.palette().iter().any(|&s| states[s as usize].light != [0; 3]);
        self.map.cube_mut(cp).unwrap().fill_light(0);
        self.cache.set((IVec3::MAX, std::ptr::null()));
        self.changed.insert(cp);

        if self.sky && !all_opaque {
            let above = cp + IVec3::Y;
            let from_above: Option<[u8; 256]> = match self.map.cube(above) {
                Some(a) => Some(std::array::from_fn(|i| ch(a.light(index(i % 16, 0, i / 16)), SKY))),
                None if open_above => Some([SKY_FULL; 256]),
                None => None,
            };
            let mut full = false;
            if let Some(src) = from_above {
                if all_clear && src.iter().all(|&v| v == SKY_FULL) {
                    full = true;
                    // Open air: the whole cube is full sky, only the borders can spread further.
                    self.map.cube_mut(cp).unwrap().fill_light(SKY_FULL as u32);
                    self.cache.set((IVec3::MAX, std::ptr::null()));
                    // Only faces whose neighbor can still take light need seeds; missing
                    // neighbors pull from this cube when they arrive.
                    for (d, face) in [(2, 0), (0, 1), (1, 2), (4, 3), (5, 4)] {
                        if !self.accepts(cp + DIRS[d], SKY, SKY_FULL) {
                            continue;
                        }
                        let reach = reach(SKY_FULL, d == 2);
                        for a in 0..16 {
                            for b in 0..16 {
                                let p = base
                                    + match face {
                                        0 => IVec3::new(a, 0, b),
                                        1 => IVec3::new(0, a, b),
                                        2 => IVec3::new(15, a, b),
                                        3 => IVec3::new(a, b, 0),
                                        _ => IVec3::new(a, b, 15),
                                    };
                                // Seed only where the cell across can still get brighter: a
                                // neighbor lit earlier is usually full already.
                                let Some((state, light)) = self.read(p + DIRS[d]) else { continue };
                                if ch(light, SKY) < reach && self.c.tight[state as usize].opacity < 15 {
                                    self.add.push_back((p, SKY_FULL));
                                }
                            }
                        }
                    }
                } else {
                    for x in 0..16 {
                        for z in 0..16 {
                            let v = src[(z * 16 + x) as usize];
                            if v == 0 {
                                continue;
                            }
                            let p = base + IVec3::new(x, 15, z);
                            let (state, _) = self.read(p).unwrap();
                            let op = self.absorb(state, SKY);
                            let nv = step(v, op, true);
                            if nv > 0 && op < 15 {
                                self.put(p, SKY, nv);
                                self.add.push_back((p, nv));
                            }
                        }
                    }
                }
            }
            // Full sky everywhere already: neighbors cannot raise anything here.
            if !full {
                self.seed_from_neighbors(cp, SKY, &[0, 1, 2, 4, 5]);
            }
            self.propagate(SKY);
        }

        for c in 1..4 {
            if !all_opaque {
                self.seed_from_neighbors(cp, c, &[0, 1, 2, 3, 4, 5]);
            }
            if emits {
                for i in 0..4096usize {
                    let state = self.map.cube(cp).unwrap().get(i);
                    let e = self.emit(state, c);
                    if e > 0 {
                        let p = base + crate::world::cube::offset(i);
                        self.put(p, c, e);
                        self.add.push_back((p, e));
                    }
                }
            }
            self.propagate(c);
        }
    }

    /// Finishes a cube lit on its own by `prelight` (open sky above, its own emitters): pulls
    /// light in from loaded neighbors and pushes its own out to them. The caller checked that
    /// the cube above gives full sky (or is open), so the inside light needs no redo.
    pub fn light_prelit(&mut self, cp: IVec3) {
        crate::diag::LIGHT_CUBES.add(1);
        self.cache.set((IVec3::MAX, std::ptr::null()));
        self.changed.insert(cp);
        let base = cp * 16;
        let Some(cube) = self.map.cube(cp) else { return };
        let all_opaque = cube.palette().iter().all(|&s| self.c.tight[s as usize].opacity >= 15);
        if all_opaque {
            return;
        }
        let uniform = cube.uniform_light();
        for &c in self.channels() {
            // Sky from above is already in; from below and the sides it may still come.
            let dirs: &[usize] = if c == SKY { &[0, 1, 2, 4, 5] } else { &[0, 1, 2, 3, 4, 5] };
            if !(c == SKY && uniform.is_some_and(|l| ch(l, SKY) == SKY_FULL)) {
                self.seed_from_neighbors(cp, c, dirs);
            }
            // Nothing of its own to push out (an air cube's block light).
            if uniform.is_some_and(|l| ch(l, c) <= 1) {
                self.propagate(c);
                continue;
            }
            for d in 0..6 {
                if c == SKY && d == 3 {
                    continue;
                }
                let n = cp + DIRS[d];
                if !self.accepts(n, c, 2) {
                    continue;
                }
                for a in 0..16 {
                    for b in 0..16 {
                        let local = match d {
                            0 => IVec3::new(0, a, b),
                            1 => IVec3::new(15, a, b),
                            2 => IVec3::new(a, 0, b),
                            3 => IVec3::new(a, 15, b),
                            4 => IVec3::new(a, b, 0),
                            _ => IVec3::new(a, b, 15),
                        };
                        let p = base + local;
                        let Some(v) = self.get(p, c) else { continue };
                        if v <= 1 {
                            continue;
                        }
                        let reach = reach(v, c == SKY && d == 2);
                        let Some((state, light)) = self.read(p + DIRS[d]) else { continue };
                        if ch(light, c) < reach && self.c.tight[state as usize].opacity < 15 {
                            self.add.push_back((p, v));
                        }
                    }
                }
            }
            self.propagate(c);
        }
    }

    /// Whether cube `n` may still gain light on channel `c` from a neighbor at `level`: loaded,
    /// not solid all through, and not already uniformly at that level.
    fn accepts(&self, n: IVec3, c: usize, level: u8) -> bool {
        let Some(cube) = self.map.cube(n).filter(|_| self.map.writable(n)) else { return false };
        if cube.uniform_light().is_some_and(|l| ch(l, c) >= level) {
            return false;
        }
        !cube.palette().iter().all(|&s| self.c.tight[s as usize].opacity >= 15)
    }

    fn seed_from_neighbors(&mut self, cp: IVec3, c: usize, dirs: &[usize]) {
        let base = cp * 16;
        for &d in dirs {
            let Some(n) = self.map.cube(cp + DIRS[d]) else { continue };
            // A uniformly dark neighbor has nothing to give.
            if n.uniform_light().is_some_and(|l| ch(l, c) <= 1) {
                continue;
            }
            for a in 0..16 {
                for b in 0..16 {
                    let local = match d {
                        0 => IVec3::new(-1, a, b),
                        1 => IVec3::new(16, a, b),
                        2 => IVec3::new(a, -1, b),
                        3 => IVec3::new(a, 16, b),
                        4 => IVec3::new(a, b, -1),
                        _ => IVec3::new(a, b, 16),
                    };
                    let p = base + local;
                    if let Some(v) = self.get(p, c) {
                        if v > 1 {
                            self.add.push_back((p, v));
                        }
                    }
                }
            }
        }
    }

    /// Spreads light on channel `c` from cells already holding it (`outward`'s seeds).
    pub fn spread(&mut self, c: usize, seeds: impl Iterator<Item = (IVec3, u8)>) {
        self.add.extend(seeds);
        self.propagate(c);
    }

    /// Relights after the block at `p` changed from `old` to the state now stored there.
    pub fn block_changed(&mut self, p: IVec3, old: u32) {
        let Some((new, _)) = self.read(p) else { return };
        let (so, sn) = (&self.c.states[old as usize], &self.c.states[new as usize]);
        if so.opacity == sn.opacity && so.light == sn.light && so.absorb == sn.absorb {
            return;
        }
        for &c in self.channels() {
            let cur = self.get(p, c).unwrap_or(0);
            self.put(p, c, 0);
            self.remove.push_back((p, cur));
            self.unpropagate(c);
            let e = self.emit(new, c);
            if e > 0 {
                self.put(p, c, e);
                self.add.push_back((p, e));
            }
            for dir in DIRS {
                if let Some(v) = self.get(p + dir, c) {
                    if v > 0 {
                        self.add.push_back((p + dir, v));
                    }
                }
            }
            self.propagate(c);
        }
    }
}

/// Cells of cube `cp` whose light would raise a neighbor across its faces: (cell, channel,
/// value), for `Lighter::spread`. Only reads, so cubes lit apart can be looked at side by side.
pub fn outward<M: CubeMap>(map: &M, c: &Content, sky: bool, cp: IVec3, out: &mut Vec<(IVec3, u8, u8)>) {
    let Some(cube) = map.cube(cp) else { return };
    let opaque = |cube: &Cube| cube.palette().iter().all(|&s| c.tight[s as usize].opacity >= 15);
    if opaque(cube) {
        return;
    }
    let base = cp * 16;
    let uniform = cube.uniform_light();
    let channels: &[usize] = if sky { &[0, 1, 2, 3] } else { &[1, 2, 3] };
    for d in 0..6 {
        let Some(n) = map.cube(cp + DIRS[d]) else { continue };
        if opaque(n) {
            continue;
        }
        let nu = n.uniform_light();
        for &ch_ in channels {
            let most = reach(SKY_FULL, ch_ == SKY && d == 2);
            if uniform.is_some_and(|l| ch(l, ch_) <= 1) || nu.is_some_and(|l| ch(l, ch_) >= most) {
                continue;
            }
            for a in 0..16 {
                for b in 0..16 {
                    let local = match d {
                        0 => IVec3::new(0, a, b),
                        1 => IVec3::new(15, a, b),
                        2 => IVec3::new(a, 0, b),
                        3 => IVec3::new(a, 15, b),
                        4 => IVec3::new(a, b, 0),
                        _ => IVec3::new(a, b, 15),
                    };
                    let v = ch(cube.light(index(local.x as usize, local.y as usize, local.z as usize)), ch_);
                    if v <= 1 {
                        continue;
                    }
                    let reach = reach(v, ch_ == SKY && d == 2);
                    let q = (local + DIRS[d]) & 15;
                    let i = index(q.x as usize, q.y as usize, q.z as usize);
                    if ch(n.light(i), ch_) < reach && c.tight[n.get(i) as usize].opacity < 15 {
                        out.push((base + local, ch_ as u8, v));
                    }
                }
            }
        }
    }
}

/// Lights a cube alone, as if open sky were above it and nothing beside it: the expensive
/// inside part of `light_new`, done on a generation worker.
pub fn prelight(c: &Content, cube: &mut Cube, sky: bool) {
    let t = &c.tight;
    let palette = cube.palette();
    let all_opaque = palette.iter().all(|&s| t[s as usize].opacity >= 15);
    let all_clear = palette.iter().all(|&s| t[s as usize].opacity == 0);
    let emits = palette.iter().any(|&s| t[s as usize].light != [0; 3]);
    let sky = sky && !all_opaque;
    if !emits && (!sky || all_clear) {
        cube.fill_light(if sky { SKY_FULL as u32 } else { 0 });
        return;
    }
    // Flat arrays per property and per channel: the flood reads them for every step, and a
    // generic cube map (hash lookups, palette decoding) cost several times the flood itself.
    let mut states = [0u32; 4096];
    cube.decode(&mut states);
    let pi = |s: u32| palette.iter().position(|&p| p == s).unwrap();
    let props: Vec<Tight> = palette.iter().map(|&s| t[s as usize]).collect();
    let idx: Box<[u8; 4096]> = Box::new(std::array::from_fn(|i| if palette.len() == 1 { 0 } else { pi(states[i]) as u8 }));
    let mut light = [[0u8; 4096]; 4];
    let mut queue: Vec<(u16, u8)> = Vec::new();
    let flood = |light: &mut [u8; 4096], queue: &mut Vec<(u16, u8)>, absorb: &dyn Fn(usize) -> u8, sky: bool| {
        let mut head = 0;
        while head < queue.len() {
            let (i, v) = queue[head];
            head += 1;
            let i = i as usize;
            if light[i] != v {
                continue;
            }
            let (x, y, z) = (i & 15, i >> 8, (i >> 4) & 15);
            for (d, ok, n) in [(0, x > 0, i.wrapping_sub(1)), (1, x < 15, i + 1), (2, y > 0, i.wrapping_sub(256)), (3, y < 15, i + 256), (4, z > 0, i.wrapping_sub(16)), (5, z < 15, i + 16)] {
                if !ok {
                    continue;
                }
                let p = &props[idx[n] as usize];
                let op = absorb(idx[n] as usize);
                if op >= 15 && p.opacity >= 15 {
                    continue;
                }
                let nv = step(v, op, sky && d == 2);
                if nv > light[n] {
                    light[n] = nv;
                    queue.push((n as u16, nv));
                }
            }
        }
        queue.clear();
    };
    if sky {
        // Straight down from open sky first, column by column; the flood spreads the rest.
        for col in 0..256 {
            let mut v = SKY_FULL;
            for y in (0..16).rev() {
                let i = y * 256 + col;
                let op = props[idx[i] as usize].opacity;
                // Only full sky goes on down here; dimmer light is the flood's.
                v = if v == SKY_FULL && (op == 0 || y == 15) && op < 15 { step(v, op, true) } else { 0 };
                if v == 0 {
                    break;
                }
                light[0][i] = v;
                queue.push((i as u16, v));
            }
        }
        flood(&mut light[0], &mut queue, &|k| props[k].opacity, true);
    }
    if emits {
        for ch in 0..3 {
            for i in 0..4096 {
                let e = props[idx[i] as usize].light[ch];
                if e > 0 {
                    light[ch + 1][i] = e;
                    queue.push((i as u16, e));
                }
            }
            flood(&mut light[ch + 1], &mut queue, &|k| props[k].absorb[ch], false);
        }
    }
    let packed: Box<[u32; 4096]> = Box::new(std::array::from_fn(|i| light[0][i] as u32 | (light[1][i] as u32) << 8 | (light[2][i] as u32) << 16 | (light[3][i] as u32) << 24));
    cube.set_light_all(packed);
}

/// The way `prelight` did it before its flat arrays, for `core:terrain_check` to compare with.
pub fn prelight_reference(c: &Content, cube: &mut Cube, sky: bool) {
    struct One<'a>(&'a mut Cube);
    impl CubeMap for One<'_> {
        fn cube(&self, p: IVec3) -> Option<&Cube> {
            (p == IVec3::ZERO).then_some(&*self.0)
        }
        fn cube_mut(&mut self, p: IVec3) -> Option<&mut Cube> {
            (p == IVec3::ZERO).then_some(&mut *self.0)
        }
    }
    let mut one = One(cube);
    Lighter::new(&mut one, c, sky).light_new(IVec3::ZERO, true);
    cube.mask_light(!0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_packing() {
        let v = with(with(0, 0, 15), 3, 200);
        assert_eq!(ch(v, 0), 15);
        assert_eq!(ch(v, 3), 200);
        assert_eq!(ch(v, 1), 0);
    }

    #[test]
    fn light_steps() {
        // Full sky straight down through clear blocks keeps its strength, nothing else does.
        assert_eq!(step(SKY_FULL, 0, true), SKY_FULL);
        assert_eq!(step(SKY_FULL, 0, false), SKY_FULL - 1);
        assert_eq!(step(SKY_FULL, 1, true), SKY_FULL - 1);
        assert_eq!(step(SKY_FULL, 3, true), SKY_FULL - 3);
        assert_eq!(step(14, 0, true), 13);
        assert_eq!(step(2, 5, false), 0);
        assert_eq!(reach(SKY_FULL, true), SKY_FULL);
        assert_eq!(reach(0, false), 0);
    }

    #[test]
    fn sky_from_above() {
        assert_eq!(sky_below(None, true), SkyBelow::Full);
        assert_eq!(sky_below(None, false), SkyBelow::Dark);
        assert_eq!(sky_below(Some(&Cube::uniform(0, SKY_FULL as u32)), false), SkyBelow::Full);
        assert_eq!(sky_below(Some(&Cube::uniform(0, 0)), true), SkyBelow::Dark);
        assert_eq!(sky_below(Some(&Cube::uniform(0, 7)), true), SkyBelow::Partial);
    }
}
