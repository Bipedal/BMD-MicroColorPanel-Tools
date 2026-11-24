//! Logging to helper.log in the config folder (rotated at 1 MB) and to stderr.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

static LOG: Mutex<Option<PathBuf>> = Mutex::new(None);

/// MICROPANEL_HOME, or %APPDATA%\\MicroPanel, or ~/.config/micropanel (macOS: ~/Library/Application Support/MicroPanel).
pub fn config_dir() -> PathBuf {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let home = || env("HOME").unwrap_or_else(|| ".".into());
    let base = env("MICROPANEL_HOME").unwrap_or_else(|| {
        if cfg!(windows) { env("APPDATA").unwrap_or_else(|| ".".into()).join("MicroPanel") }
        else if cfg!(target_os = "macos") { home().join("Library/Application Support/MicroPanel") }
        else { env("XDG_CONFIG_HOME").unwrap_or_else(|| home().join(".config")).join("micropanel") }
    });
    let _ = std::fs::create_dir_all(&base);
    base
}

pub fn init_log() {
    *LOG.lock().unwrap() = Some(config_dir().join("helper.log"));
}

pub fn ts() -> String {
    let d = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let s = d.as_secs();
    format!("{:02}:{:02}:{:02}.{:03}", (s / 3600) % 24, (s / 60) % 60, s % 60, d.subsec_millis())
}

pub fn log(msg: &str) {
    let line = format!("{} {}\n", ts(), msg);
    eprint!("{line}");
    let guard = LOG.lock().unwrap();
    if let Some(p) = guard.as_ref() {
        if std::fs::metadata(p).map(|m| m.len() > 1_000_000).unwrap_or(false) {
            let _ = std::fs::rename(p, p.with_extension("log.1"));
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
            let _ = f.write_all(line.as_bytes());
        }
    }
}
