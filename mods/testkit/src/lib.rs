//! Automated tests and scripted runs, written on the public client and server mod API.
//!
//! `weft --test all` runs every test (`--test furnace,armor` some, `--test list` names them,
//! `--test bench` the benchmarks), each in a fresh throwaway world, then writes
//! `run/test-reports/report.txt` and exits with the number of failed tests. Screenshots of the
//! run go to `run/test-shots/` for looking at; checks never compare pictures with references.
//! `--test-keep` keeps the test worlds. `weft --world w --script file.txt` runs a line script (see `script.rs`).
//! Without those options the mod only registers its message channels.

mod runner;
mod script;
mod server;
mod tests;

use std::sync::OnceLock;

use weft_sdk::*;

declare_mod!(deps: [], init: init);

static PLAN: OnceLock<(Option<String>, Option<String>)> = OnceLock::new();

fn init(r: &mut Registry) {
    server::register(r);
    r.client_packet(&server::REPLY, |_, data| runner::reply(data));
    r.key(&runner::PROBE, "F9");
    // A branch of an engine command, handled by this mod (on the client, like /gizmo).
    r.command_branch("gizmo", "probe <n:int>", |cmd| {
        let n = cmd.int("n").unwrap_or(0);
        cmd.reply(&format!("gizmo probe {n} from testkit"));
    });
    // A command that never reaches the server: answers from the client it runs on.
    r.client_command("probe", "<what:word(alpha,beta)>", |cmd| {
        let side = if cmd.client().is_some() { "client" } else { "server" };
        let what = cmd.text("what").unwrap_or("").to_string();
        cmd.reply(&format!("probe {what} on the {side}"));
    });
    let (test, script) = (launch_option("test"), launch_option("script"));
    if test.is_none() && script.is_none() {
        return;
    }
    let _ = PLAN.set((test, script));
    r.client_frame(|c| {
        static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !STARTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            start(c);
        }
        runner::frame(c);
    });
}

/// Builds the queue on the main thread, where the runner lives.
fn start(c: &Client) {
    let Some((test, script)) = PLAN.get() else { return };
    if let Some(path) = script {
        let src = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                error!("cannot read script {path}: {e}");
                c.quit(2);
                return;
            }
        };
        let name = std::path::Path::new(path).file_stem().map_or("script".into(), |s| s.to_string_lossy().into_owned());
        runner::start(vec![runner::TestDef { name, run: Box::new(move |t| Box::pin(script::run(t, src.clone()))) }]);
        return;
    }
    // Same conditions on every machine; nothing of this is saved.
    for (k, v) in [("language", "en_us"), ("gui_scale", "2"), ("view_distance", "12"), ("fov", "70"), ("brightness", "0.5"), ("vsync", "true"), ("resource_packs", ""), ("shader_pack", ""), ("debug_groups", "4294967295")] {
        c.setting(k, v);
    }
    // The player's packs would change every picture.
    c.reload_assets();
    let want = test.as_deref().unwrap_or("all");
    let all = if want == "bench" { tests::bench() } else { tests::all() };
    if want == "list" {
        for t in &all {
            info!("test {}", t.name);
        }
        c.quit(0);
        return;
    }
    let queue: Vec<runner::TestDef> = if want == "all" || want == "bench" {
        all
    } else {
        let names: Vec<&str> = want.split(',').map(str::trim).collect();
        if let Some(bad) = names.iter().find(|n| !all.iter().any(|t| t.name == **n)) {
            error!("no test named {bad}; --test list shows them");
            c.quit(2);
            return;
        }
        all.into_iter().filter(|t| names.contains(&t.name.as_str())).collect()
    };
    runner::start(queue);
}
