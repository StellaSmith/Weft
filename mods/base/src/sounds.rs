//! Sound events: block groups (`block.<group>.break|place|step|hit`) blocks pick with
//! `.sound_group(group)`, mobs' voices, and the background music. Files are in `assets/sounds/`.

use weft_sdk::abi::music_when as mw;
use weft_sdk::*;

pub fn register(r: &mut Registry) {
    for g in ["stone", "wood", "gravel", "grass", "sand", "glass", "cloth"] {
        let files = |kind: &str| (1..=3).map(|i| format!("{kind}/{g}{i}")).collect::<Vec<_>>();
        let (dig, step) = (files("dig"), files("step"));
        let dig: Vec<&str> = dig.iter().map(String::as_str).collect();
        let step: Vec<&str> = step.iter().map(String::as_str).collect();
        // Vanilla breaks glass with its own sound and knocks on it like on stone.
        let breaks: Vec<&str> = if g == "glass" { vec!["random/glass1", "random/glass2", "random/glass3"] } else { dig.clone() };
        r.sound(Sound::generated(format!("block.{g}.break"))).files(&breaks).pitch(0.75, 0.85);
        r.sound(Sound::generated(format!("block.{g}.place"))).files(&dig).pitch(0.75, 0.85);
        r.sound(Sound::generated(format!("block.{g}.step"))).files(&step).pitch(0.9, 1.1);
        r.sound(Sound::generated(format!("block.{g}.hit"))).files(&step).volume(0.6).pitch(0.5, 0.55);
    }
    for mob in ["zombie", "pig", "cow", "spider"] {
        for (event, file) in [("ambient", "say"), ("hurt", "hurt"), ("death", "death")] {
            r.sound(Sound::generated(format!("entity.{mob}.{event}"))).files(&[&format!("mob/{mob}/{file}")]).category(&sound_categories::CREATURES).pitch(0.8, 1.2);
        }
    }
    r.music(&Music::named("calm")).tracks(&["music/calm1"]).when(mw::SURFACE);
    r.music(&Music::named("deep")).tracks(&["music/deep1"]).when(mw::UNDERGROUND);
}
