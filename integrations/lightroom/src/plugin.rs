//! The Lightroom plugin's files are inside the helper; it can put them where Lightroom looks for plugins.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const FOLDER: &str = "MicroPanel.lrplugin";
macro_rules! file { ($n:literal) => { ($n, include_str!(concat!("../../../lightroom/MicroPanel.lrplugin/", $n))) } }
pub const FILES: [(&str, &str); 7] = [
    file!("Info.lua"), file!("MicroPanelInit.lua"), file!("MicroPanelShutdown.lua"), file!("MicroPanelStatus.lua"),
    file!("Core.lua"), file!("Catalog.lua"), file!("Json.lua"),
];

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Found {
    /// no Lightroom Classic settings here
    NoLightroom,
    Missing,
    /// another version, or not all of it
    Different,
    Same,
}
impl Found {
    pub fn name(self) -> &'static str { match self { Found::NoLightroom => "no_lightroom", Found::Missing => "missing", Found::Different => "different", Found::Same => "same" } }
}

/// Lightroom Classic's own folder for the signed-in user; it loads plugins from `Modules` by itself.
fn lightroom_folder() -> Option<PathBuf> {
    if !cfg!(windows) { return None; }
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Adobe").join("Lightroom"))
}

/// Line ends don't count: git may have changed them.
fn same(path: &Path, text: &str) -> bool { std::fs::read_to_string(path).is_ok_and(|t| t.replace("\r\n", "\n") == text.replace("\r\n", "\n")) }

fn look(lightroom: &Path) -> Found {
    if !lightroom.is_dir() { return Found::NoLightroom; }
    let d = lightroom.join("Modules").join(FOLDER);
    // a folder without any of our files counts as none
    if !FILES.iter().any(|(n, _)| d.join(n).exists()) { return Found::Missing; }
    if FILES.iter().all(|(n, t)| same(&d.join(n), t)) { Found::Same } else { Found::Different }
}

static SEEN: Mutex<Option<(Instant, Found)>> = Mutex::new(None);

/// Looked up every few seconds at most.
pub fn found() -> Found {
    let mut s = SEEN.lock().unwrap();
    if let Some((at, f)) = *s { if at.elapsed() < Duration::from_secs(3) { return f; } }
    let f = lightroom_folder().map_or(Found::NoLightroom, |l| look(&l));
    *s = Some((Instant::now(), f));
    f
}

fn install_in(lightroom: &Path) -> Result<(), String> {
    if !lightroom.is_dir() { return Err(format!("{} doesn't exist. Start Lightroom Classic once, then try again.", lightroom.display())); }
    let d = lightroom.join("Modules").join(FOLDER);
    std::fs::create_dir_all(&d).map_err(|e| format!("{}: {e}", d.display()))?;
    for (n, t) in FILES {
        let p = d.join(n);
        if same(&p, t) { continue; }
        // whole or not at all
        let tmp = d.join(format!("{n}.new"));
        std::fs::write(&tmp, t).and_then(|_| std::fs::rename(&tmp, &p)).map_err(|e| { let _ = std::fs::remove_file(&tmp); format!("{}: {e}", p.display()) })?;
    }
    Ok(())
}

pub fn install() -> Result<(), String> {
    let l = lightroom_folder().ok_or("The plugin can only be installed like this on Windows.")?;
    let r = install_in(&l);
    *SEEN.lock().unwrap() = None;
    r
}

/// Only our own files; the folder too, unless something else is in it.
fn remove_in(lightroom: &Path) -> Result<(), String> {
    let d = lightroom.join("Modules").join(FOLDER);
    for (n, _) in FILES {
        let p = d.join(n);
        match std::fs::remove_file(&p) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(format!("{}: {e}", p.display())),
            _ => {}
        }
    }
    // fails if it isn't empty, which is what we want
    let _ = std::fs::remove_dir(&d);
    Ok(())
}

pub fn remove() -> Result<(), String> {
    let l = lightroom_folder().ok_or("The plugin can only be removed like this on Windows.")?;
    let r = remove_in(&l);
    *SEEN.lock().unwrap() = None;
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installs_updates_and_leaves_other_files_alone() {
        let l = std::env::temp_dir().join(format!("mp_lr_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&l);
        assert_eq!(look(&l), Found::NoLightroom);
        assert!(install_in(&l).unwrap_err().contains("Start Lightroom"));
        std::fs::create_dir_all(&l).unwrap();
        assert_eq!(look(&l), Found::Missing);
        install_in(&l).unwrap();
        assert_eq!(look(&l), Found::Same);
        let d = l.join("Modules").join(FOLDER);
        std::fs::write(d.join("Json.lua"), "-- old").unwrap();
        std::fs::write(d.join("mine.txt"), "keep").unwrap();
        assert_eq!(look(&l), Found::Different);
        install_in(&l).unwrap();
        assert_eq!(look(&l), Found::Same);
        assert_eq!(std::fs::read_to_string(d.join("mine.txt")).unwrap(), "keep");
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), FILES.len() + 1, "nothing left over");
        remove_in(&l).unwrap();
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1, "what isn't ours stays");
        std::fs::remove_file(d.join("mine.txt")).unwrap();
        install_in(&l).unwrap();
        remove_in(&l).unwrap();
        assert!(!d.exists() && l.join("Modules").is_dir());
        assert_eq!(look(&l), Found::Missing);
        remove_in(&l).unwrap();
        std::fs::remove_dir_all(&l).unwrap();
    }

    #[test]
    fn every_plugin_file_is_in() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../lightroom").join(FOLDER);
        let mut on_disk: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        let mut here: Vec<String> = FILES.iter().map(|f| f.0.to_string()).collect();
        on_disk.sort();
        here.sort();
        assert_eq!(on_disk, here);
        let core = FILES.iter().find(|f| f.0 == "Core.lua").unwrap().1;
        assert!(core.contains(&format!("PLUGIN_VERSION = '{}'", crate::PLUGIN)), "version of lib.rs");
        let v: Vec<&str> = crate::PLUGIN.split('.').collect();
        assert!(FILES[0].1.contains(&format!("major = {}, minor = {}, revision = {}", v[0], v[1], v[2])), "version of Info.lua");
    }
}
