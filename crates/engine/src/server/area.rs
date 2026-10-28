//! Edits of a whole area at once (a crater): the shape is applied cube by cube on every core,
//! cubes it covers whole become one block, light is redone once per cube instead of once per
//! block, only blocks along the edge hear about it, and clients get whole cubes. Cubes that are
//! not loaded get it when they come (`DimSet::insert`), so an area far bigger than anything
//! loaded costs nothing until someone goes there.

use std::sync::Arc;

use glam::{DVec3, IVec3};
use rayon::prelude::*;

use super::dim::Dim;
use crate::content::Content;
use crate::world::cube::{Cube, VOLUME};
use crate::world::light::{CubeMap, Lighter};

/// A ball of one block: every block whose middle is within `radius` of `center` becomes
/// `state` (unbreakable ones stay with `keep_unbreakable`).
#[derive(Clone, Copy, Debug)]
pub struct AreaFill {
    /// Order of the dimension's edits, from 1; cubes remember the last one they got.
    pub id: u32,
    pub center: DVec3,
    pub radius: f64,
    pub state: u32,
    pub keep_unbreakable: bool,
}

/// Light work an area edit left for the following ticks.
#[derive(Clone, Copy)]
pub enum LightJob {
    /// A cube lit on its own (or set to full sky) joins its neighbors (`Lighter::light_prelit`).
    Join(IVec3),
    /// A cube lit from scratch (`Lighter::light_new`), with whether the sky is open above it.
    New(IVec3, bool),
}

/// What one fill did, for the log.
#[derive(Default)]
pub struct FillStats {
    pub cubes: usize,
    pub whole: usize,
    pub relit: usize,
    pub edges: usize,
    pub apply_ms: f64,
    pub light_ms: f64,
}

impl AreaFill {
    /// As saved in `world.txt`: `id x y z radius block keep`.
    pub fn spec(&self, c: &Content) -> String {
        let p = self.center;
        format!("{} {} {} {} {} {} {}", self.id, p.x, p.y, p.z, self.radius, c.state_name(self.state), self.keep_unbreakable as u8)
    }

    pub fn parse(c: &Content, spec: &str) -> Option<AreaFill> {
        let w: Vec<&str> = spec.split_whitespace().collect();
        let [id, x, y, z, r, block, keep] = w[..] else { return None };
        Some(AreaFill {
            id: id.parse().ok()?,
            center: DVec3::new(x.parse().ok()?, y.parse().ok()?, z.parse().ok()?),
            radius: r.parse().ok()?,
            state: c.state_by_name(block)?,
            keep_unbreakable: keep == "1",
        })
    }

    /// Squared distance from the center to the nearest and farthest point of cube `cp`.
    fn reach(&self, cp: IVec3) -> (f64, f64) {
        let lo = (cp * 16).as_dvec3();
        let hi = lo + DVec3::splat(16.0);
        let near = self.center.clamp(lo, hi) - self.center;
        let far = DVec3::new(
            (self.center.x - lo.x).abs().max((hi.x - self.center.x).abs()),
            (self.center.y - lo.y).abs().max((hi.y - self.center.y).abs()),
            (self.center.z - lo.z).abs().max((hi.z - self.center.z).abs()),
        );
        (near.length_squared(), far.length_squared())
    }

    /// The cube may hold a block it changes, or one next to the edge.
    pub fn touches(&self, cp: IVec3) -> bool {
        self.reach(cp).0 <= (self.radius + 1.0).powi(2)
    }

    fn inside(&self, p: IVec3) -> bool {
        (p.as_dvec3() + DVec3::splat(0.5) - self.center).length_squared() <= self.radius * self.radius
    }

    /// The cube with the fill applied, if that changes it, and the blocks outside next to a
    /// changed one (they get a neighbor update: sand on the rim falls).
    pub fn apply(&self, c: &Content, cp: IVec3, cube: &Cube) -> Option<(Cube, Vec<IVec3>)> {
        // Liquids count as unbreakable for mining, not here: a crater takes the lake too.
        let keep = |s: u32| self.keep_unbreakable && c.states[s as usize].hardness < 0.0 && c.states[s as usize].liquid.is_none();
        let (_, far) = self.reach(cp);
        let mut out = if far <= self.radius * self.radius && !cube.palette().iter().any(|&s| keep(s)) {
            if cube.palette() == [self.state] {
                return None;
            }
            (Cube::uniform(self.state, 0), Vec::new())
        } else {
            let base = cp * 16;
            let mut states = [0u32; VOLUME];
            let mut changed = false;
            let mut edges = Vec::new();
            for (i, s) in states.iter_mut().enumerate() {
                let old = cube.get(i);
                *s = old;
                let p = base + IVec3::new((i & 15) as i32, (i >> 8) as i32, ((i >> 4) & 15) as i32);
                if self.inside(p) {
                    if old != self.state && !keep(old) {
                        *s = self.state;
                        changed = true;
                    }
                } else if old != self.state && crate::world::DIRS.iter().any(|&d| self.inside(p + d)) {
                    edges.push(p);
                }
            }
            if !changed {
                return (!edges.is_empty()).then(|| (cube.clone(), edges));
            }
            (Cube::from_states(&states), edges)
        };
        out.0.biomes = cube.biomes.clone();
        out.0.ticked = cube.ticked;
        out.0.edits = cube.edits;
        Some(out)
    }
}

impl Dim {
    /// Applies `fill` to every loaded cube of this region it reaches (see the module).
    pub fn area_fill(&mut self, c: &'static Content, fill: &AreaFill) -> FillStats {
        let mut stats = FillStats::default();
        let start = std::time::Instant::now();
        let targets: Vec<(IVec3, Arc<Cube>)> = self.cubes.iter().filter(|(cp, _)| fill.touches(**cp)).map(|(cp, cube)| (*cp, cube.clone())).collect();
        // With whether the cube had a light source.
        let done: Vec<(IVec3, Option<(Cube, Vec<IVec3>)>, bool)> =
            targets.par_iter().map(|(cp, cube)| (*cp, fill.apply(c, *cp, cube), cube.palette().iter().any(|&s| c.states[s as usize].light != [0; 3]))).collect();
        let mut relight: Vec<IVec3> = Vec::new();
        // Cubes that lost a light source: those around them may get darker.
        let mut lost_light: Vec<IVec3> = Vec::new();
        for (cp, result, emitted) in done {
            let Some((mut cube, edges)) = result else {
                // Reached but unchanged: it still counts as having had this edit.
                if let Some(cube) = self.cubes.get_mut(&cp) {
                    Arc::make_mut(cube).edits = fill.id;
                }
                continue;
            };
            stats.edges += edges.len();
            self.edge_updates.extend(edges);
            cube.edits = fill.id;
            stats.cubes += 1;
            stats.whole += cube.uniform_state().is_some() as usize;
            // Block entities whose block went: gone with it (an explosion takes what they held).
            let gone: Vec<(IVec3, u64)> = self
                .entities
                .anchors
                .iter()
                .filter(|(p, _)| crate::world::cube_of(**p) == cp && cube.get(crate::world::local_index(**p)) != self.get(**p).unwrap_or(u32::MAX))
                .map(|(p, id)| (*p, *id))
                .collect();
            for (_, id) in gone {
                self.entities.remove(id);
            }
            if emitted {
                lost_light.push(cp);
            }
            self.grid.cube_unloaded(cp);
            self.grid.cube_loaded(c, cp, &cube);
            self.cubes.insert(cp, Arc::new(cube));
            self.modified.insert(cp);
            self.edited.insert(cp);
            self.conn.remove(&cp);
            relight.push(cp);
        }
        stats.apply_ms = start.elapsed().as_secs_f64() * 1000.0;
        let start = std::time::Instant::now();
        let st = &c.states[fill.state as usize];
        if st.opacity == 0 && st.light == [0; 3] && st.liquid.is_none() {
            self.relight_opened(c, &relight, &lost_light, &mut stats);
        } else {
            self.relight_around(c, &relight, &mut stats);
        }
        stats.light_ms = start.elapsed().as_secs_f64() * 1000.0;
        stats
    }

    /// Light for cubes that only lost blocks (a crater): light around them can only grow, so
    /// only they are lit again and push it out, except around lost light sources. Cubes open to
    /// full sky above need no flood fill: whole air ones are set to full sky, partial ones are
    /// lit alone in parallel (as generation does) and joined to their neighbors.
    fn relight_opened(&mut self, c: &'static Content, changed: &[IVec3], lost_light: &[IVec3], stats: &mut FillStats) {
        let def = &c.dims[self.id as usize];
        let full = crate::world::light::SKY_FULL as u32;
        let mut order = changed.to_vec();
        order.sort_unstable_by_key(|p| (-p.y, p.x, p.z));
        let open_above = |cp: IVec3| cp.y + 1 >= def.sky_cube_y || cp.y + 1 > def.max_cube_y;
        // Full sky across the bottom of the cube above (or open sky).
        let sky_above = |cubes: &rustc_hash::FxHashMap<IVec3, Arc<Cube>>, cp: IVec3| match cubes.get(&(cp + IVec3::Y)) {
            Some(a) => match a.uniform_light() {
                Some(l) => l & 0xFF == full,
                None => (0..256).all(|i| a.light(crate::world::cube::index(i % 16, 0, i / 16)) & 0xFF == full),
            },
            None => open_above(cp),
        };
        let mut whole = Vec::new();
        let mut alone = Vec::new();
        let mut flood = Vec::new();
        for &cp in &order {
            let air = self.cubes[&cp].uniform_state().is_some();
            if !def.sky_light || !sky_above(&self.cubes, cp) {
                flood.push(cp);
            } else if air {
                Arc::make_mut(self.cubes.get_mut(&cp).unwrap()).fill_light(full);
                whole.push(cp);
            } else {
                alone.push(cp);
            }
        }
        let lit: Vec<(IVec3, Cube)> = alone
            .par_iter()
            .map(|&cp| {
                let mut cube = (*self.cubes[&cp]).clone();
                crate::world::light::prelight(c, &mut cube, true);
                (cp, cube)
            })
            .collect();
        for (cp, cube) in lit {
            self.cubes.insert(cp, Arc::new(cube));
        }
        // Around lost light sources, the cubes left as they were may be too bright now.
        let mut dimmed: Vec<IVec3> = lost_light
            .iter()
            .flat_map(|&cp| (-1..=1).flat_map(move |y| (-1..=1).flat_map(move |z| (-1..=1).map(move |x| cp + IVec3::new(x, y, z)))))
            .filter(|p| self.cubes.contains_key(p) && !changed.contains(p))
            .collect();
        dimmed.sort_unstable_by_key(|p| (-p.y, p.x, p.z));
        dimmed.dedup();
        stats.relit = changed.len() + dimmed.len();
        // The rest is flood fills reaching into walls and caves (seconds for a crater of
        // hundreds of blocks at a view of 64): queued and done a share per tick
        // (`run_light_jobs`) so the world keeps ticking meanwhile.
        self.light_dirty.extend(whole.iter().copied());
        self.light_jobs.extend(alone.iter().map(|&cp| LightJob::Join(cp)));
        self.light_jobs.extend(flood.iter().chain(&dimmed).map(|&cp| LightJob::New(cp, open_above(cp))));
        self.light_jobs.extend(whole.iter().map(|&cp| LightJob::Join(cp)));
    }

    /// Light jobs left by area edits, for up to `budget` of this tick, in the order queued.
    pub fn run_light_jobs(&mut self, c: &'static Content, budget: std::time::Duration) {
        if self.light_jobs.is_empty() {
            return;
        }
        let start = std::time::Instant::now();
        let def = &c.dims[self.id as usize];
        let mut map = super::dim::Cubes(&mut self.cubes);
        let mut lighter = Lighter::new(&mut map, c, def.sky_light);
        let mut done = 0;
        while let Some(job) = self.light_jobs.pop_front() {
            // A cube unloaded or handed to another region since is lit when it comes back.
            match job {
                LightJob::Join(cp) if lighter.map.cube(cp).is_some() => lighter.light_prelit(cp),
                LightJob::New(cp, open) if lighter.map.cube(cp).is_some() => lighter.light_new(cp, open),
                _ => {}
            }
            done += 1;
            if done % 16 == 0 && start.elapsed() >= budget {
                break;
            }
        }
        let changed = std::mem::take(&mut lighter.changed);
        self.light_dirty.extend(changed);
    }

    /// Light once per cube, the changed ones and those around them (light reaches 15 blocks:
    /// less than a cube), from the top down so sky light comes from cubes already done.
    fn relight_around(&mut self, c: &'static Content, relight: &[IVec3], stats: &mut FillStats) {
        let mut around: Vec<IVec3> = relight.iter().flat_map(|&cp| (-1..=1).flat_map(move |y| (-1..=1).flat_map(move |z| (-1..=1).map(move |x| cp + IVec3::new(x, y, z))))).filter(|p| self.cubes.contains_key(p)).collect();
        around.sort_unstable_by_key(|p| (-p.y, p.x, p.z));
        around.dedup();
        stats.relit = around.len();
        let def = &c.dims[self.id as usize];
        let mut map = super::dim::Cubes(&mut self.cubes);
        let mut lighter = Lighter::new(&mut map, c, def.sky_light);
        for &cp in &around {
            let open_above = cp.y + 1 >= def.sky_cube_y || cp.y + 1 > def.max_cube_y;
            lighter.light_new(cp, open_above);
        }
        let changed = std::mem::take(&mut lighter.changed);
        self.light_dirty.extend(changed);
    }
}
