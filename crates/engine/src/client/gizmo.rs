//! Debug drawing in the world, switched per group with `/gizmo <group>` (F3+G cube borders, F3+B
//! hitboxes like vanilla). Client groups are built here every frame; server groups and mods'
//! groups arrive as shapes, only for groups this player turned on.

use std::time::Instant;

use glam::{DVec3, IVec3};

use super::Client;
use crate::content::content;
use crate::net::GizmoShape;

pub const BUILTIN: &[(&str, &str)] = &[
    ("cubes", "borders of the cube you are in and its neighbours"),
    ("hitboxes", "entity boxes"),
    ("collision", "collision boxes of blocks nearby"),
    ("light", "block light on blocks nearby: red spawnable, yellow spawnable at night"),
    ("regions", "server regions, the parts of the world simulated in parallel"),
    ("visibility", "cubes the server considers visible (green loaded, orange waiting)"),
];

/// A line between two points, in the colour `0xRRGGBBAA`.
pub type Line = (DVec3, DVec3, u32);
/// A filled box between two corners; its alpha makes it see-through.
pub type Fill = (DVec3, DVec3, u32);
pub type Label = (DVec3, String, u32);

#[derive(Default)]
pub struct Gizmos {
    /// Groups turned on, sorted.
    pub enabled: Vec<String>,
    shapes: Vec<(GizmoShape, Instant)>,
}

impl Gizmos {
    pub fn on(&self, group: &str) -> bool {
        self.enabled.iter().any(|g| g == group)
    }

    /// Switches a group; returns whether it is on now.
    pub fn toggle(&mut self, group: &str) -> bool {
        if let Some(i) = self.enabled.iter().position(|g| g == group) {
            self.enabled.remove(i);
            self.shapes.retain(|s| s.0.group != group);
            false
        } else {
            self.enabled.push(group.to_string());
            self.enabled.sort();
            true
        }
    }

    pub fn receive(&mut self, shapes: Vec<GizmoShape>) {
        let now = Instant::now();
        // A group sent again replaces what it showed before.
        let mut groups: Vec<&str> = shapes.iter().map(|s| s.group.as_str()).collect();
        groups.sort_unstable();
        groups.dedup();
        self.shapes.retain(|(s, _)| groups.binary_search(&s.group.as_str()).is_err());
        for s in shapes {
            if self.on(&s.group) {
                let until = now + std::time::Duration::from_millis(s.ticks as u64 * 50);
                self.shapes.push((s, until));
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.enabled.is_empty()
    }

    /// Lines, filled boxes and labels to draw this frame.
    pub fn collect(&mut self, client: &Client) -> (Vec<Line>, Vec<Fill>, Vec<Label>) {
        let mut lines = Vec::new();
        let mut fills = Vec::new();
        let mut labels = Vec::new();
        if self.enabled.is_empty() {
            return (lines, fills, labels);
        }
        let now = Instant::now();
        self.shapes.retain(|s| s.1 > now);
        for (s, _) in &self.shapes {
            match s.kind {
                0 => lines.push((s.a, s.b, s.color)),
                1 => cuboid(&mut lines, s.a, s.b, s.color),
                3 => fills.push((s.a.min(s.b), s.a.max(s.b), s.color)),
                _ => labels.push((s.a, s.text.clone(), s.color)),
            }
        }
        let c = content();
        let me = client.player.pos;
        let feet = me.floor().as_ivec3();
        if self.on("cubes") {
            let cube: IVec3 = feet >> 4;
            for dz in -1..=1 {
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        let o = ((cube + IVec3::new(dx, dy, dz)) * 16).as_dvec3();
                        let color = if (dx, dy, dz) == (0, 0, 0) { 0xFFE040FF } else { 0x4070FFFF };
                        cuboid(&mut lines, o, o + DVec3::splat(16.0), color);
                    }
                }
            }
        }
        if self.on("hitboxes") {
            for e in client.entities.values().filter(|e| (e.pos - me).length_squared() < 64.0 * 64.0) {
                let k = &c.game.entities[e.kind as usize];
                let h = if e.crouched { crate::physics::CROUCH_HEIGHT } else { k.height as f64 };
                let (hw, h) = if e.item.is_empty() { (k.width as f64 / 2.0, h) } else { (0.125, 0.25) };
                cuboid(&mut lines, e.pos - DVec3::new(hw, 0.0, hw), e.pos + DVec3::new(hw, h, hw), 0xFFFFFFFF);
                let eye = e.pos + DVec3::Y * k.eye as f64;
                let (sy, cy) = (e.yaw as f64).sin_cos();
                lines.push((eye, eye + DVec3::new(-sy, 0.0, -cy) * 1.5, 0x4060FFFF));
            }
        }
        if self.on("collision") {
            let mut boxes = Vec::new();
            for dy in -2..=3 {
                for dz in -3..=3 {
                    for dx in -3..=3 {
                        let p = feet + IVec3::new(dx, dy, dz);
                        let Some(state) = client.world.block_or_air(p) else { continue };
                        boxes.clear();
                        crate::physics::boxes(c, state, &mut boxes);
                        let o = p.as_dvec3();
                        for b in &boxes {
                            cuboid(&mut lines, o + DVec3::new(b[0], b[1], b[2]), o + DVec3::new(b[3], b[4], b[5]), 0x40FF60FF);
                        }
                    }
                }
            }
        }
        if self.on("light") {
            for dy in -4..=4 {
                for dz in -6..=6 {
                    for dx in -6..=6 {
                        let p = feet + IVec3::new(dx, dy, dz);
                        let (Some(below), Some(here)) = (client.world.block_or_air(p - IVec3::Y), client.world.block_or_air(p)) else { continue };
                        let solid = |s: u32| c.states[s as usize].flags & voxide_abi::block_flags::SOLID != 0;
                        if !solid(below) || solid(here) {
                            continue;
                        }
                        let l = client.world.light(p);
                        let (sky, block) = (l & 255, ((l >> 8) & 255).max((l >> 16) & 255).max((l >> 24) & 255));
                        // Modern spawning: hostile mobs need block light 0, and sky light 7 or
                        // less, which open ground only has at night.
                        let color = match (block, sky) {
                            (0, 0..=7) => 0xFF5040FF,
                            (0, _) => 0xFFD040FF,
                            _ => 0x60FF60FF,
                        };
                        labels.push((p.as_dvec3() + DVec3::new(0.5, 0.05, 0.5), block.to_string(), color));
                    }
                }
            }
        }
        (lines, fills, labels)
    }
}

/// The twelve edges of an axis-aligned box.
pub fn cuboid(out: &mut Vec<Line>, a: DVec3, b: DVec3, color: u32) {
    let p = |x: bool, y: bool, z: bool| DVec3::new(if x { b.x } else { a.x }, if y { b.y } else { a.y }, if z { b.z } else { a.z });
    for i in 0..4 {
        let (u, v) = (i & 1 != 0, i & 2 != 0);
        out.push((p(false, u, v), p(true, u, v), color));
        out.push((p(u, false, v), p(u, true, v), color));
        out.push((p(u, v, false), p(u, v, true), color));
    }
}
