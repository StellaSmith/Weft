//! The shape of a player's view in cubes, one definition for the server (what it loads, sends and
//! announces) and the client's terrain thread (what it generates).

use glam::IVec3;

use crate::content::DimensionDef;

/// A cylinder around `center`: `radius` cubes sideways, `up` above and `down` below.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct View {
    pub center: IVec3,
    pub radius: i32,
    pub up: i32,
    pub down: i32,
}

impl View {
    /// As far up and down as sideways; flying higher than that (`air` empty cubes under the
    /// center), down to the ground and a little into it, so the ground shows.
    pub fn around(center: IVec3, radius: i32, air: i32) -> View {
        View { center, radius, up: radius, down: radius.max(air + 8) }
    }

    /// How many cubes under the center to look at for `around`'s `air`.
    pub fn air_probe(radius: i32) -> i32 {
        2 * radius
    }

    #[inline]
    pub fn column(&self, p: IVec3) -> bool {
        let d = p - self.center;
        d.x * d.x + d.z * d.z <= self.radius * self.radius
    }

    #[inline]
    pub fn contains(&self, p: IVec3) -> bool {
        let d = p.y - self.center.y;
        self.column(p) && d <= self.up && d >= -self.down
    }

    /// `contains`, within the dimension's height.
    #[inline]
    pub fn contains_in(&self, def: &DimensionDef, p: IVec3) -> bool {
        self.contains(p) && p.y >= def.min_cube_y && p.y <= def.max_cube_y
    }

    /// `n` more cubes every way: what may stay loaded without flicker at the edge.
    pub fn grown(self, n: i32) -> View {
        View { radius: self.radius + n, up: self.up + n, down: self.down + n, ..self }
    }

    /// Cut to `n` cubes every way (the server's simulation distance).
    pub fn cut(self, n: i32) -> View {
        View { radius: self.radius.min(n), up: self.up.min(n), down: self.down.min(n), ..self }
    }

    /// The cube heights kept loaded: the view plus, above it, the column up to where the
    /// generator guarantees open sky (capped), so sky light reaches the visible cubes without
    /// those extra cubes being sent or drawn.
    pub fn loaded_y(&self, def: &DimensionDef) -> (i32, i32) {
        let lo = (self.center.y - self.down).max(def.min_cube_y);
        let top = self.center.y + self.up;
        let hi = if def.sky_light && def.sky_cube_y != i32::MAX { top.max((def.sky_cube_y - 1).min(top + 12)) } else { top };
        (lo, hi.min(def.max_cube_y))
    }

    /// In the view or the sky column above it (`loaded_y`).
    #[inline]
    pub fn loaded(&self, def: &DimensionDef, p: IVec3) -> bool {
        let (lo, hi) = self.loaded_y(def);
        self.column(p) && p.y >= lo && p.y <= hi
    }

    /// Order to load cubes in: nearest columns first, each from the top down.
    #[inline]
    pub fn order(&self, p: IVec3) -> i32 {
        let d = p - self.center;
        (d.x * d.x + d.z * d.z) * 64 - p.y
    }
}
