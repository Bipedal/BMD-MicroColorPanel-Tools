//! The command line and the tray: tray helper for the DaVinci Resolve Micro Color Panel.
//!
//!   micropanel                     tray app (normal use; on Windows started at sign-in if enabled)
//!   micropanel --console           also print the log to the console
//!   micropanel --no-tray [--run-for SECS] [--dry] [--trace] [--stop-file F]   headless (tests);
//!              --dry logs key/mouse output, --trace logs every event and its binding, F appearing = quit
//!   micropanel --install-autostart | --remove-autostart
//!   micropanel --editor-only [--port N] [--ctx C --layers L] [--connect]   only the mapping editor and cheat sheet, for trying them next to a
//!              running helper; --connect starts the integrations too
//!   micropanel --sheet-png FILE [--ctx C] [--layers L] [--size WxH] [--list] [--light] [--notice TEXT] [--link usb|bt|none]   a picture of the cheat sheet
//!   micropanel --check FILE        validate a profiles file and exit
//!   micropanel --quit              ask the running helper to quit (graceful: LEDs off, panel dark)

use crate::app::{self, Opts, Shared, EDITOR_PORT};
use crate::ext::Integration;
use crate::util::{config_dir, init_log, log};
use crate::sys;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The whole program. `integrations`: what this build can talk to.
pub fn main(integrations: Vec<Arc<dyn Integration>>) {
    crate::ext::register(integrations);
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |f: &str| args.iter().any(|a| a == f);
    let val = |f: &str| args.iter().position(|a| a == f).and_then(|i| args.get(i + 1)).cloned();
    // A tray app has no console: put panics in the log too.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current().name().unwrap_or("?").to_string();
        log(&format!("PANIC in thread {thread}: {info}"));
        default_hook(info);
    }));
    if has("--console") || has("--no-tray") || has("--check") || has("--quit") || has("--install-autostart") || has("--remove-autostart") {
        sys::attach_console();
    }
    if let Some(f) = val("--check") {
        let r = std::fs::read_to_string(&f).map_err(|e| e.to_string()).and_then(|t| crate::profiles::Profiles::parse(&t));
        match r {
            Ok(p) => { eprintln!("ok: {} contexts", p.ctxs.len());
                for w in &p.warnings { eprintln!("note: {w}"); }
                std::process::exit(0) }
            Err(e) => { eprintln!("error: {e}"); std::process::exit(1) }
        }
    }
    if has("--install-autostart") || has("--remove-autostart") {
        let on = has("--install-autostart");
        let ok = sys::set_autostart(on);
        eprintln!("autostart {}: {}", if on { "on" } else { "off" }, if ok { "ok" } else { "FAILED" });
        std::process::exit(if ok { 0 } else { 1 });
    }
    if has("--quit") {
        // ask a running helper (same config folder) to quit gracefully; it checks every 100 ms
        let f = config_dir().join("QUIT");
        let _ = std::fs::write(&f, b"quit");
        for _ in 0..50 {
            std::thread::sleep(Duration::from_millis(100));
            if !f.exists() { eprintln!("helper quit"); std::process::exit(0); }
        }
        let _ = std::fs::remove_file(&f);
        eprintln!("no running helper answered");
        std::process::exit(1);
    }
    if let Some(f) = val("--sheet-png") {
        let layers: Vec<String> = val("--layers").map(|l| l.split(',').map(String::from).collect()).unwrap_or_default();
        let size = val("--size").and_then(|s| s.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))).unwrap_or((860, 424));
        let r = crate::sheet::png_from_args(&f, &val("--ctx").unwrap_or_else(|| "default".into()), &layers, size, has("--list"), has("--light"), &val("--notice").unwrap_or_default(), val("--link").as_deref());
        if let Err(e) = &r { eprintln!("error: {e}"); }
        std::process::exit(if r.is_ok() { 0 } else { 1 });
    }
    if has("--editor-only") {
        // MICROPANEL_HOME decides which profiles.toml it edits
        let port = val("--port").and_then(|s| s.parse().ok()).unwrap_or(EDITOR_PORT + 1);
        init_log();
        let layers = val("--layers").map(|l| l.split(',').map(String::from).collect()).unwrap_or_default();
        if !app::editor_only(port, val("--ctx"), layers, has("--connect")) { std::process::exit(1); }
        loop { std::thread::sleep(Duration::from_secs(3600)); }
    }
    init_log();
    if !sys::single_instance() {
        log("another micropanel helper is already running; exiting");
        // started again by hand: show that it is there
        if !has("--no-tray") { sys::open_url(&editor_url()); }
        return;
    }
    log(&format!("micropanel {} starting; config in {}", env!("CARGO_PKG_VERSION"), config_dir().display()));
    let shared = Arc::new(Mutex::new(Shared::default()));
    let opts = Opts { dry: has("--dry"), transport: std::env::var("PANEL_TRANSPORT").unwrap_or_else(|_| "auto".into()),
        trace: has("--trace"), stop_file: val("--stop-file").map(std::path::PathBuf::from) };
    let sh = shared.clone();
    let first = !app::profile_path().exists();
    let logic = std::thread::Builder::new().name("logic".into()).spawn(move || app::run_logic(sh, opts)).unwrap();

    if has("--no-tray") {
        let until = val("--run-for").and_then(|s| s.parse::<f64>().ok()).map(|s| Instant::now() + Duration::from_secs_f64(s));
        loop {
            std::thread::sleep(Duration::from_millis(200));
            if until.map_or(false, |u| Instant::now() >= u) || logic.is_finished() { break; }
        }
        shared.lock().unwrap().quit = true;
        let _ = logic.join();
        log("stopped");
        return;
    }
    // the first time nothing else shows that it runs: Windows may hide the tray icon
    if first { std::thread::spawn(|| if editor_is_up() { sys::open_url(&editor_url()); }); }
    sys::tray(shared, logic);
}

fn editor_url() -> String { format!("http://127.0.0.1:{EDITOR_PORT}/") }

/// Wait up to ten seconds for it.
fn editor_is_up() -> bool {
    (0..50).any(|_| {
        let up = std::net::TcpStream::connect(("127.0.0.1", EDITOR_PORT)).is_ok();
        if !up { std::thread::sleep(Duration::from_millis(200)); }
        up
    })
}
