//! The test list. Each test is an `async fn(T) -> R`; failed checks are collected, `?` stops it.

mod extend;
mod gameplay;
mod liquids;
mod sync;
mod ui;
mod visual;
mod world;

use std::time::Duration;

use crate::runner::{R, T, TestDef};

pub fn all() -> Vec<TestDef> {
    crate::tests![
        ui::title_screens,
        ui::f3_overlay,
        ui::inventory_crafting,
        ui::creative_inventory,
        ui::chat_completion,
        ui::client_command_completion,
        world::flatlands_sync,
        world::save_and_reload,
        world::dimensions,
        world::virtual_menus,
        world::sand_and_support,
        gameplay::furnace,
        gameplay::crafting_table_tags,
        gameplay::mining_and_wear,
        gameplay::armor,
        gameplay::bucket,
        liquids::water_flow,
        liquids::water_finds_hole,
        liquids::infinite_water,
        liquids::lava_and_water,
        liquids::lava_burns,
        liquids::mod_liquid_bucket,
        liquids::mod_sand_soil,
        gameplay::fall_damage,
        gameplay::sneak_edge,
        gameplay::commands,
        gameplay::mobs,
        gameplay::energy,
        gameplay::eating,
        gameplay::drop_and_pickup,
        extend::events_batched,
        extend::interceptor_protects,
        extend::damage_chain,
        extend::attributes,
        extend::loot_tables,
        extend::tool_tiers,
        extend::healing_food,
        sync::menu_sync,
        sync::watcher_sync,
        sync::server_only_stack_data,
        visual::held_items,
        visual::tooltips,
        visual::night_light,
        visual::third_person,
        visual::ladder_placement,
        visual::pick_block,
        visual::gizmos,
        visual::multiplayer,
        visual::client_hooks,
        world::load_benchmark,
    ]
}

impl T {
    /// A fresh world moved to the flat test dimension, survival, at noon, loaded and idle.
    pub async fn flat(&self) -> R {
        self.world(1).await?;
        self.cmd("/dim base:flatlands").await?;
        self.until("flatlands", Duration::from_secs(20), |t| t.state().dim != 0 || t.state().pos[1] < 10.0).await?;
        self.loaded().await?;
        self.op("mode survival").await?;
        self.idle().await
    }

    /// Turns the local player to look at the middle of block `p` from its eyes.
    pub fn look_at(&self, p: [i32; 3]) {
        let s = self.state();
        let eye = [s.pos[0], s.pos[1] + 1.62, s.pos[2]];
        let d = [p[0] as f64 + 0.5 - eye[0], p[1] as f64 + 0.5 - eye[1], p[2] as f64 + 0.5 - eye[2]];
        let yaw = (-d[0]).atan2(-d[2]).to_degrees() as f32;
        let pitch = d[1].atan2((d[0] * d[0] + d[2] * d[2]).sqrt()).to_degrees() as f32;
        self.c().look(yaw, pitch);
    }

    /// Counts entities of a kind near the player as the server has them.
    pub async fn count_near(&self, kind: &str, r: f64) -> Result<usize, String> {
        let list = self.op(&format!("near ~ ~ ~ {r}")).await?;
        Ok(list.split_whitespace().filter(|e| e.ends_with(&format!(":{kind}"))).count())
    }
}
