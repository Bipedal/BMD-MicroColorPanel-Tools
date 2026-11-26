//! Any system: what plain Rust can do. The rest says once that it can't and does nothing.

use crate::app::{Shared, EDITOR_PORT};
use crate::keys::Combo;
use crate::util::log;
use std::collections::HashSet;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn missing(what: &'static str) {
    static SAID: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);
    if SAID.lock().unwrap().get_or_insert_default().insert(what) { log(&format!("{what}: not available on this system (src/sys/portable.rs)")); }
}

// ------------------------------------------------------------------ output
pub fn mouse_move(_dx: i32, _dy: i32) { missing("mouse output"); }
pub fn scroll(_v: i32, _h: i32, _mods: &[u16]) { missing("mouse output"); }
pub fn mouse_button(_b: u8, _down: bool) { missing("mouse output"); }
pub fn key(_c: &Combo, _down: bool, _up: bool) { missing("key output"); }

pub fn alt_down() -> bool { false }
/// Unknown, so somebody could be there.
pub fn idle_for() -> Duration { Duration::ZERO }

// ------------------------------------------------------------------ programs
/// (program file name lower-case, is a modal dialog); None = unknown, so only "All apps" applies.
pub fn foreground() -> Option<(String, bool)> { None }
pub fn foreground_title() -> String { String::new() }
pub fn running_apps() -> Vec<String> { vec![] }

/// "resolve.exe" also finds a process named "resolve" (Linux).
pub fn process_running(exe: &str) -> bool {
    let name = exe.trim_end_matches(".exe");
    let Ok(d) = std::fs::read_dir("/proc") else { return false };
    d.flatten().filter(|e| e.file_name().to_string_lossy().bytes().all(|b| b.is_ascii_digit()))
        .any(|e| std::fs::read_to_string(e.path().join("comm")).is_ok_and(|c| c.trim().eq_ignore_ascii_case(name)))
}

/// Wait until process `pid` has ended, at most `limit`.
pub fn wait_for_exit(pid: u32, limit: Duration) {
    let end = std::time::Instant::now() + limit;
    while std::path::Path::new(&format!("/proc/{pid}")).exists() && std::time::Instant::now() < end { std::thread::sleep(Duration::from_millis(100)); }
}

/// False if another helper is already running: a port taken for as long as we run.
pub fn single_instance() -> bool {
    static LOCK: Mutex<Option<std::net::TcpListener>> = Mutex::new(None);
    match std::net::TcpListener::bind(("127.0.0.1", EDITOR_PORT - 1)) {
        Ok(l) => { *LOCK.lock().unwrap() = Some(l); true }
        Err(_) => false,
    }
}

// ------------------------------------------------------------------ desktop
pub fn autostart_enabled() -> bool { false }
pub fn set_autostart(_on: bool) -> bool { missing("starting at sign-in"); false }

fn open(what: &std::ffi::OsStr) {
    let cmd = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let r = Command::new(cmd).arg(what).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    if let Err(e) = r { log(&format!("can't run {cmd}: {e}")); }
}
pub fn open_url(url: &str) { open(url.as_ref()); }
pub fn open_folder(p: &Path) { open(p.as_os_str()); }
pub fn attach_console() {}

/// No tray: wait until the helper stops (`micropanel --quit`, or Ctrl+C).
pub fn tray(_shared: Arc<Mutex<Shared>>, logic: std::thread::JoinHandle<()>) {
    log(&format!("no tray on this system; mappings: http://127.0.0.1:{EDITOR_PORT}/  stop: micropanel --quit"));
    let _ = logic.join();
    log("stopped");
}

/// The editor records keys in the browser when this one doesn't.
pub mod keyrec {
    use crate::sys::RecState;
    pub fn state() -> RecState { RecState::TimedOut }
    pub fn start(_timeout: std::time::Duration) { super::missing("recording keys system-wide"); }
    pub fn cancel() {}
}

pub mod midi {
    pub fn out_names() -> Vec<String> { vec![] }
    pub fn in_names() -> Vec<String> { vec![] }
    pub fn open_out(_i: Option<usize>) -> Result<(), String> { Ok(()) }
    pub fn open_in(_i: Option<usize>) -> Result<(), String> { Ok(()) }
    pub fn send(_m: [u8; 3]) -> bool { super::missing("MIDI"); false }
    pub fn make_loopback() {}
}

pub mod sheet_window {
    use crate::app::Shared;
    use std::sync::{Arc, Mutex};
    pub fn start(_shared: Arc<Mutex<Shared>>) {
        // opening it from a key or the editor has no effect; say so once it is tried
        let _ = std::thread::Builder::new().name("sheet".into()).spawn(|| loop {
            if crate::sheet::get().open { super::missing("the cheat sheet window"); return; }
            std::thread::sleep(super::Duration::from_secs(1));
        });
    }
    pub fn png_from_args(_path: &str, _ctx: &str, _layers: &[String], _size: (i32, i32), _list: bool, _light: bool, _notice: &str, _link: Option<&str>) -> Result<(), String> {
        Err("the cheat sheet can't be drawn on this system".into())
    }
}
