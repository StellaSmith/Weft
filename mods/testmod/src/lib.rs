//! Checks that the engine is moddable without touching the core: adds its own content on top of
//! the base game through nothing but `voxide_sdk`.

use voxide_sdk::*;

declare_mod!(id: "testmod", name: "Test Mod", version: "0.1.0", deps: ["base"], init: init);

fn init(_r: &mut Registry) {}
