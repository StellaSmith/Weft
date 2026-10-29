//! Edits of a whole area at once (a crater): the shape is applied cube by cube on every core,
//! cubes it covers whole become one block, light is redone once per cube instead of once per
//! block, only blocks along the edge hear about it, and clients get whole cubes. Cubes that are
//! not loaded get it when they come (`DimSet::insert`), so an area far bigger than anything
//! loaded costs nothing until someone goes there.

use std::sync::Arc;

use glam::{DVec3, IVec2, IVec3};
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};

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

/// Light work for one cube of an area edit.
#[derive(Clone, Copy)]
enum LightJob {
    /// A cube lit on its own (or set to full sky) joins its neighbors (`Lighter::light_prelit`).
    Join(IVec3),
    /// A cube lit from scratch (`Lighter::light_new`), with whether the sky is open above it.
    New(IVec3, bool),
}

impl LightJob {
    fn pos(self) -> IVec3 {
        match self {
            LightJob::Join(p) | LightJob::New(p, _) => p,
        }
    }
}

/// What one light task sees: the 3x3 columns around its own between `low` and `top`, read
/// from the map as they are and copied on the first write (no other task of its round reaches
/// them). Boxed, so the lighter's cached pointers stay put as more are copied.
struct Overlay<'a> {
    base: &'a FxHashMap<IVec3, Arc<Cube>>,
    own: FxHashMap<IVec3, Box<Cube>>,
    key: IVec2,
    low: i32,
    top: i32,
}

impl Overlay<'_> {
    fn reaches(&self, p: IVec3) -> bool {
        (p.x - self.key.x).abs() <= 1 && (p.z - self.key.y).abs() <= 1 && p.y >= self.low && p.y <= self.top
    }
}

impl crate::world::light::CubeMap for Overlay<'_> {
    fn cube(&self, p: IVec3) -> Option<&Cube> {
        if !self.reaches(p) {
            return None;
        }
        self.own.get(&p).map(|b| &**b).or_else(|| self.base.get(&p).map(|a| &**a))
    }

    fn cube_mut(&mut self, p: IVec3) -> Option<&mut Cube> {
        if !self.reaches(p) {
            return None;
        }
        if !self.own.contains_key(&p) {
            let copy = Box::new((**self.base.get(&p)?).clone());
            self.own.insert(p, copy);
        }
        self.own.get_mut(&p).map(|b| &mut **b)
    }
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
    pub fn reach(&self, cp: IVec3) -> (f64, f64) {
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
    /// Applies `fill` to every loaded cube of this region it reaches (see the module), and makes
    /// the `fresh` cubes (never loaded, filled whole) as they end up.
    pub fn area_fill(&mut self, c: &'static Content, fill: &AreaFill, fresh: &[IVec3]) -> FillStats {
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
            self.edited.insert(cp);
            self.conn.remove(&cp);
            // Clients get the blocks now, whatever the light does.
            self.light_dirty.insert(cp);
            relight.push(cp);
        }
        for &cp in fresh {
            let mut cube = Cube::uniform(fill.state, 0);
            cube.edits = fill.id;
            self.grid.cube_loaded(c, cp, &cube);
            self.add_cube(cp, Arc::new(cube));
            relight.push(cp);
            stats.cubes += 1;
            stats.whole += 1;
        }
        stats.apply_ms = start.elapsed().as_secs_f64() * 1000.0;
        let start = std::time::Instant::now();
        stats.relit = relight_after(c, self.id, fill.state, &mut self.cubes, &mut self.light_dirty, &relight, &lost_light);
        stats.light_ms = start.elapsed().as_secs_f64() * 1000.0;
        // Lit for good: they go out to players like any loaded cube.
        let mut fresh = fresh.to_vec();
        fresh.sort_unstable_by_key(|p| -p.y);
        for cp in fresh {
            self.settle_down(cp);
        }
        stats
    }
}

/// Gives a cube coming from storage or generation the area `edits` made while it was away (or
/// before it was ever generated), once each. Whether that changed it.
pub fn catch_up(c: &Content, edits: &[AreaFill], pos: IVec3, cube: &mut Cube) -> bool {
    let Some(last) = edits.last().map(|e| e.id).filter(|&l| cube.edits < l) else { return false };
    let had = cube.edits;
    let mut edited = false;
    for e in edits.iter().filter(|e| e.id > had && e.touches(pos)) {
        if let Some((next, _)) = e.apply(c, pos, cube) {
            *cube = next;
            edited = true;
        }
    }
    cube.edits = last;
    edited
}

/// `fill` made on cubes held outside a region (a client's own world): applied on every core to
/// the ones it reaches and lit; every cube changed, in blocks or light, goes into `dirty`.
pub fn fill_cubes(c: &'static Content, dim: u32, fill: &AreaFill, cubes: &mut FxHashMap<IVec3, Arc<Cube>>, dirty: &mut FxHashSet<IVec3>) {
    let targets: Vec<(IVec3, Arc<Cube>)> = cubes.iter().filter(|(cp, _)| fill.touches(**cp)).map(|(cp, cube)| (*cp, cube.clone())).collect();
    let done: Vec<(IVec3, Option<Cube>, bool)> =
        targets.par_iter().map(|(cp, cube)| (*cp, fill.apply(c, *cp, cube).map(|r| r.0), cube.palette().iter().any(|&s| c.states[s as usize].light != [0; 3]))).collect();
    let (mut changed, mut lost) = (Vec::new(), Vec::new());
    for (cp, result, emitted) in done {
        match result {
            Some(mut cube) => {
                cube.edits = fill.id;
                cubes.insert(cp, Arc::new(cube));
                dirty.insert(cp);
                changed.push(cp);
                if emitted {
                    lost.push(cp);
                }
            }
            None => {
                if let Some(cube) = cubes.get_mut(&cp) {
                    Arc::make_mut(cube).edits = fill.id;
                }
            }
        }
    }
    relight_after(c, dim, fill.state, cubes, dirty, &changed, &lost);
}

/// Light after `state` was filled into the `changed` cubes (`lost_light`: those of them that
/// had a light source), every cube whose light changed going into `dirty`. Returns how many
/// cubes were relit.
pub fn relight_after(c: &'static Content, dim: u32, state: u32, cubes: &mut FxHashMap<IVec3, Arc<Cube>>, dirty: &mut FxHashSet<IVec3>, changed: &[IVec3], lost_light: &[IVec3]) -> usize {
    let st = &c.states[state as usize];
    if st.opacity == 0 && st.light == [0; 3] && st.liquid.is_none() {
        relight_opened(c, dim, cubes, dirty, changed, lost_light)
    } else {
        relight_around(c, dim, cubes, dirty, changed)
    }
}

/// Light for cubes that only lost blocks (a crater): light around them can only grow, so
/// only they are lit again and push it out, except around lost light sources. Cubes open to
/// full sky above need no flood fill: whole air ones are set to full sky, partial ones are
/// lit alone in parallel (as generation does) and joined to their neighbors.
fn relight_opened(c: &'static Content, dim: u32, cubes: &mut FxHashMap<IVec3, Arc<Cube>>, dirty: &mut FxHashSet<IVec3>, changed: &[IVec3], lost_light: &[IVec3]) -> usize {
    let def = &c.dims[dim as usize];
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
        let air = cubes[&cp].uniform_state().is_some();
        if !def.sky_light || !sky_above(cubes, cp) {
            flood.push(cp);
        } else if air {
            Arc::make_mut(cubes.get_mut(&cp).unwrap()).fill_light(full);
            whole.push(cp);
        } else {
            alone.push(cp);
        }
    }
    let lit: Vec<(IVec3, Cube)> = alone
        .par_iter()
        .map(|&cp| {
            let mut cube = (*cubes[&cp]).clone();
            crate::world::light::prelight(c, &mut cube, true);
            (cp, cube)
        })
        .collect();
    for (cp, cube) in lit {
        cubes.insert(cp, Arc::new(cube));
    }
    // Around lost light sources, the cubes left as they were may be too bright now.
    let mut dimmed: Vec<IVec3> = lost_light
        .iter()
        .flat_map(|&cp| (-1..=1).flat_map(move |y| (-1..=1).flat_map(move |z| (-1..=1).map(move |x| cp + IVec3::new(x, y, z)))))
        .filter(|p| cubes.contains_key(p) && !changed.contains(p))
        .collect();
    dimmed.sort_unstable_by_key(|p| (-p.y, p.x, p.z));
    dimmed.dedup();
    let relit = changed.len() + dimmed.len();
    // Whole air cubes among others like them have nothing to push or pull.
    let full: FxHashSet<IVec3> = whole.iter().copied().collect();
    let mut jobs: Vec<LightJob> = alone.iter().map(|&cp| LightJob::Join(cp)).collect();
    jobs.extend(flood.iter().chain(&dimmed).map(|&cp| LightJob::New(cp, open_above(cp))));
    jobs.extend(whole.iter().filter(|&&cp| crate::world::DIRS.iter().any(|d| !full.contains(&(cp + *d)))).map(|&cp| LightJob::Join(cp)));
    run_light_jobs(c, dim, cubes, dirty, jobs);
    relit
}

/// Runs light jobs on every core. Light leaves a cube by at most 15 blocks (less than a
/// cube) except full sky going straight down, so jobs are grouped by column and columns
/// three apart are done at once, each seeing only the 3x3 columns around it (`Overlay`);
/// nine rounds cover every column. Within a column the jobs keep their order (top down).
fn run_light_jobs(c: &'static Content, dim: u32, cubes: &mut FxHashMap<IVec3, Arc<Cube>>, dirty: &mut FxHashSet<IVec3>, jobs: Vec<LightJob>) {
    let def = &c.dims[dim as usize];
    let mut columns: FxHashMap<IVec2, Vec<LightJob>> = FxHashMap::default();
    for job in jobs {
        let p = job.pos();
        columns.entry(IVec2::new(p.x, p.z)).or_default().push(job);
    }
    let opaque = |cube: &Cube| cube.palette().iter().all(|&s| c.states[s as usize].opacity >= 15);
    for round in 0..9 {
        let keys: Vec<IVec2> = columns.keys().copied().filter(|k| k.x.rem_euclid(3) + 3 * k.y.rem_euclid(3) == round).collect();
        let mut work: Vec<(IVec2, Vec<LightJob>, i32, i32)> = Vec::with_capacity(keys.len());
        for key in keys {
            let jobs = columns.remove(&key).unwrap();
            let top = jobs.iter().map(|j| j.pos().y).max().unwrap() + 1;
            // Full sky goes on down whatever shaft is below, and sideways from there.
            let mut low = jobs.iter().map(|j| j.pos().y).min().unwrap();
            while cubes.get(&IVec3::new(key.x, low - 1, key.y)).is_some_and(|cube| !opaque(cube)) {
                low -= 1;
            }
            work.push((key, jobs, low - 1, top));
        }
        let base = &*cubes;
        let done: Vec<(FxHashMap<IVec3, Box<Cube>>, FxHashSet<IVec3>)> = work
            .into_par_iter()
            .map(|(key, jobs, low, top)| {
                let mut map = Overlay { base, own: FxHashMap::default(), key, low, top };
                let mut lighter = Lighter::new(&mut map, c, def.sky_light);
                for job in jobs {
                    match job {
                        LightJob::Join(cp) => lighter.light_prelit(cp),
                        LightJob::New(cp, open) => lighter.light_new(cp, open),
                    }
                }
                let changed = std::mem::take(&mut lighter.changed);
                (map.own, changed)
            })
            .collect();
        for (own, changed) in done {
            for (p, cube) in own {
                cubes.insert(p, Arc::from(cube));
            }
            dirty.extend(changed);
        }
    }
}

/// Light once per cube, the changed ones and those around them (light reaches 15 blocks:
/// less than a cube), from the top down so sky light comes from cubes already done.
fn relight_around(c: &'static Content, dim: u32, cubes: &mut FxHashMap<IVec3, Arc<Cube>>, dirty: &mut FxHashSet<IVec3>, relight: &[IVec3]) -> usize {
    let mut around: Vec<IVec3> = relight.iter().flat_map(|&cp| (-1..=1).flat_map(move |y| (-1..=1).flat_map(move |z| (-1..=1).map(move |x| cp + IVec3::new(x, y, z))))).filter(|p| cubes.contains_key(p)).collect();
    around.sort_unstable_by_key(|p| (-p.y, p.x, p.z));
    around.dedup();
    let relit = around.len();
    let def = &c.dims[dim as usize];
    let jobs = around.iter().map(|&cp| LightJob::New(cp, cp.y + 1 >= def.sky_cube_y || cp.y + 1 > def.max_cube_y)).collect();
    run_light_jobs(c, dim, cubes, dirty, jobs);
    relit
}
