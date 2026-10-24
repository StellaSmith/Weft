//! Automated tests and scripted runs, written on the public client and server mod API.
//!
//! `voxide --test all` runs every test (`--test furnace,armor` some, `--test list` names them),
//! each in a fresh throwaway world, then writes `run/test-reports/report.txt` and exits with the
//! number of failed tests. `--test-record` re-records screenshot references, `--test-keep` keeps
//! the test worlds. `voxide --world w --script file.txt` runs a line script (see `script.rs`).
//! Without those options the mod only registers its message channels.

mod runner;
mod script;
mod server;
mod tests;

use std::sync::OnceLock;

use voxide_sdk::*;

declare_mod!(id: "testkit", name: "Test Kit", version: "0.1.0", deps: [], init: init);

static PLAN: OnceLock<(Option<String>, Option<String>)> = OnceLock::new();

fn init(r: &mut Registry) {
    server::register(r);
    r.client_packet("reply", |_, data| runner::reply(data));
    r.key("probe", "F9");
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
    for (k, v) in [("language", "en_us"), ("gui_scale", "2"), ("view_distance", "12"), ("fov", "70"), ("brightness", "0.5"), ("vsync", "true")] {
        c.setting(k, v);
    }
    let all = tests::all();
    let want = test.as_deref().unwrap_or("all");
    if want == "list" {
        for t in &all {
            info!("test {}", t.name);
        }
        c.quit(0);
        return;
    }
    let queue: Vec<runner::TestDef> = if want == "all" {
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
