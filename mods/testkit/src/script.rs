//! A line script language on the client mod API:
//!
//! ```text
//! wait 2.5            # seconds
//! loaded              # until the world around the player is meshed
//! idle                # until nothing is left to generate, light, send or mesh
//! press core:inventory
//! hold core:forward 1.5
//! click 0             # mouse button
//! cursor 0.5 0.4      # mouse at fractions of the window
//! scroll -3
//! type /give dia      # text into the focused field
//! key Tab             # a key by its settings name
//! look 90 -20         # yaw, pitch in degrees
//! chat /give stone 10
//! shot name.png       # into run/screenshots (or --shot-dir)
//! bench 5 label       # BENCH line over 5 seconds
//! host ticket.txt     # host, write the join ticket there
//! join ticket.txt     # join with the ticket from that file (waits for it)
//! quit
//! ```

use std::time::Duration;

use weft_sdk::client::client_flags as cf;
use weft_sdk::launch_option;

use crate::runner::{R, T};

pub async fn run(t: T, src: String) -> R {
    let started = std::time::Instant::now();
    for line in src.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let (cmd, rest) = line.split_once(' ').unwrap_or((line, ""));
        let rest = rest.trim();
        let nums: Vec<f32> = rest.split_whitespace().filter_map(|s| s.parse().ok()).collect();
        let num = |i: usize, d: f32| nums.get(i).copied().unwrap_or(d);
        match cmd {
            "wait" => t.secs(num(0, 1.0)).await?,
            "loaded" => {
                t.loaded().await?;
                weft_sdk::info!("script: loaded at {:.2} s", started.elapsed().as_secs_f32());
            }
            "idle" => {
                t.idle().await?;
                let s = t.state();
                weft_sdk::info!("script: idle at {:.2} s", started.elapsed().as_secs_f32());
                weft_sdk::info!(
                    "load: {} cubes on client, generated {} ({:.0} us each on a worker), inserted {} ({:.0} us each on the server thread)",
                    s.cubes,
                    s.generated,
                    s.gen_us,
                    s.inserted,
                    s.insert_us
                );
            }
            "press" => t.hold(rest, 0.0),
            "hold" => {
                let (a, secs) = rest.rsplit_once(' ').unwrap_or((rest, "1"));
                t.hold(a.trim(), secs.parse().unwrap_or(1.0));
            }
            "click" => t.c().click(num(0, 0.0) as u32, 0.0),
            "cursor" => t.c().cursor(num(0, 0.5), num(1, 0.5)),
            "scroll" => t.c().scroll(num(0, -1.0)),
            "type" => t.c().type_text(rest),
            "key" => {
                if !t.c().key(rest, 0.0) {
                    t.fail(format!("unknown key {rest}"));
                }
            }
            "look" => t.c().look(num(0, 0.0), num(1, 0.0)),
            "chat" => t.c().chat(rest),
            "shot" => {
                let path = match launch_option("shot-dir") {
                    Some(d) => format!("{d}/{rest}"),
                    None => rest.to_string(),
                };
                t.c().screenshot(&path);
            }
            "bench" => {
                let (secs, label) = rest.split_once(' ').unwrap_or((rest, ""));
                t.bench(secs.parse().unwrap_or(5.0), label.trim()).await?;
            }
            "host" => {
                t.c().host_world();
                t.until("hosting", Duration::from_secs(60), |t| t.flag(cf::HOSTING)).await?;
                let _ = std::fs::write(rest, t.c().ticket());
            }
            "join" => {
                let mut ticket = String::new();
                t.until("ticket file", Duration::from_secs(120), |_| {
                    ticket = std::fs::read_to_string(rest).unwrap_or_default().trim().to_string();
                    !ticket.is_empty()
                })
                .await?;
                t.c().join(&ticket);
            }
            "quit" => return Ok(()),
            _ => t.fail(format!("unknown script line '{line}'")),
        }
        // One step per frame.
        t.frames(1).await?;
    }
    Ok(())
}
