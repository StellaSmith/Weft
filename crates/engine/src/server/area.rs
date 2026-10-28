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
use crate::world::light::Lighter;

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
        let done: Vec<(IVec3, Option<(Cube, Vec<IVec3>)>)> = targets.par_iter().map(|(cp, cube)| (*cp, fill.apply(c, *cp, cube))).collect();
        let mut relight: Vec<IVec3> = Vec::new();
        for (cp, result) in done {
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
            self.grid.cube_unloaded(cp);
            self.grid.cube_loaded(c, cp, &cube);
            self.cubes.insert(cp, Arc::new(cube));
            self.modified.insert(cp);
            self.edited.insert(cp);
            self.conn.remove(&cp);
            relight.push(cp);
        }
        stats.apply_ms = start.elapsed().as_secs_f64() * 1000.0;
        // Light once per cube, the changed ones and those around them (light reaches 15 blocks:
        // less than a cube), from the top down so sky light comes from cubes already done.
        let start = std::time::Instant::now();
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
        stats.light_ms = start.elapsed().as_secs_f64() * 1000.0;
        stats
    }
}
