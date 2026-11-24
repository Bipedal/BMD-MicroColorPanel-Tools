//! Presets: several sets of assignments per app, and global presets that pick one for every app.
//!
//! `profiles.toml` always holds the assignments in use, so the engine knows nothing of presets. A
//! preset is a small file with one app's tables in the same format; switching puts the app's
//! current tables into the preset that was in use and the other preset's tables into
//! `profiles.toml`. That way every change (editor or text editor) belongs to the preset in use.
//!
//! Files, under `presets/` in the config folder: `presets.json` (names, which one is in use),
//! `<app>/<id>.toml`, `_backups/` (profiles.toml before each switch), `_deleted/`.

use crate::profiles::{default_toml, Profiles};
use crate::util::log;
use serde_json::{json, Value as J};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use toml_edit::DocumentMut;

/// Counts preset changes, so an open editor notices a switch made on the panel.
pub static REV: AtomicU64 = AtomicU64::new(0);

const STARTER: &str = "builtin:starter";
const EMPTY: &str = "builtin:empty";
const KEEP_BACKUPS: usize = 30;

// ------------------------------------------------------------------ text: one app's tables

/// The file as (top-level name, its text) in order, plus what follows the last table.
fn split(text: &str) -> Result<(Vec<(String, String)>, String), String> {
    let doc: DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| e.to_string())?;
    let keys: Vec<String> = doc.as_table().iter().map(|(k, _)| k.to_string()).collect();
    let mut out = vec![];
    for k in &keys {
        let mut d = doc.clone();
        for o in &keys { if o != k { d.as_table_mut().remove(o); } }
        d.set_trailing("");
        let mut t = d.to_string();
        if !t.is_empty() && !t.ends_with('\n') { t.push('\n'); }
        out.push((k.clone(), t));
    }
    Ok((out, doc.trailing().as_str().unwrap_or("").to_string()))
}

/// The tables of one app, with their comments ("" = none).
pub fn app_text(text: &str, app: &str) -> Result<String, String> {
    Ok(split(text)?.0.into_iter().find(|(k, _)| k == app).map(|x| x.1).unwrap_or_default())
}

/// `text` with one app's tables replaced (at the place of the old ones, or at the end).
pub fn replace_app(text: &str, app: &str, piece: &str) -> Result<String, String> {
    let (mut parts, trailing) = split(text)?;
    match parts.iter_mut().find(|(k, _)| k == app) {
        Some(p) => p.1 = piece.to_string(),
        None => parts.push((app.to_string(), piece.to_string())),
    }
    let mut out = String::new();
    for (_, t) in parts {
        if t.is_empty() { continue; }
        if !out.is_empty() && !t.starts_with('\n') && !t.starts_with('#') { out.push('\n'); }
        out.push_str(&t);
    }
    out.push_str(&trailing);
    Profiles::parse(&out).map_err(|e| format!("the result doesn't load: {e}"))?;
    Ok(out)
}

/// A preset file may hold one app only.
fn check_piece(piece: &str, app: &str) -> Result<(), String> {
    Profiles::parse(piece)?;
    let doc: DocumentMut = piece.parse().map_err(|e: toml_edit::TomlError| e.to_string())?;
    match doc.as_table().iter().find(|(k, _)| *k != app) {
        Some((k, _)) => Err(format!("it has [{k}], which is not part of {}", crate::edit::app_label(app))),
        None => Ok(()),
    }
}

/// Number of bindings and lights.
fn count(p: &Profiles) -> usize { p.ctxs.values().map(|c| c.bind.len() + c.leds.len()).sum() }

/// Number of bindings and lights that differ.
fn diff(a: &Profiles, b: &Profiles) -> usize {
    let flat = |p: &Profiles| {
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        for (c, x) in &p.ctxs {
            for ((l, k, n), b) in &x.bind { m.insert(format!("{c}|{l}|{}.{n}", k.key()), format!("{:?}", crate::profiles::binding_text(b))); }
            for (n, _, src) in &x.leds { m.insert(format!("{c}||led.{n}"), src.clone()); }
        }
        m
    };
    let (a, b) = (flat(a), flat(b));
    a.iter().filter(|(k, v)| b.get(*k) != Some(*v)).count() + b.keys().filter(|k| !a.contains_key(*k)).count()
}
fn same(a: &str, b: &str) -> bool {
    match (Profiles::parse(a), Profiles::parse(b)) { (Ok(a), Ok(b)) => diff(&a, &b) == 0, _ => a == b }
}

// ------------------------------------------------------------------ the list of presets

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Preset { pub id: String, pub name: String, #[serde(default)] pub note: String }
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AppPresets { pub active: String, pub list: Vec<Preset> }
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Global { pub id: String, pub name: String, pub pick: BTreeMap<String, String> }
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Index { pub apps: BTreeMap<String, AppPresets>, pub globals: Vec<Global>, /// None = a mix that is none of them
    pub global: Option<String> }

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Switched { pub before: BTreeMap<String, String>, pub after: BTreeMap<String, String>, pub before_global: Option<String>, pub after_global: Option<String> }

pub struct Store { pub dir: PathBuf, pub profile: PathBuf }

fn builtin_name(id: &str) -> Option<&'static str> { match id { STARTER => Some("Starter"), EMPTY => Some("Empty"), _ => None } }

fn slug(name: &str) -> String {
    let mut s = String::new();
    for c in name.to_lowercase().chars() { if c.is_ascii_lowercase() || c.is_ascii_digit() { s.push(c); } else if !s.ends_with('-') && !s.is_empty() { s.push('-'); } }
    let s = s.trim_matches('-').to_string();
    if s.is_empty() { "preset".into() } else { s }
}
fn unique<'a>(base: &str, taken: impl Iterator<Item = &'a str>) -> String {
    let taken: Vec<&str> = taken.collect();
    if !taken.contains(&base) { return base.to_string(); }
    (2..).map(|n| format!("{base}-{n}")).find(|x| !taken.contains(&x.as_str())).unwrap()
}
fn clean_name(n: &str) -> Result<String, String> {
    let n = n.trim();
    if n.is_empty() { return Err("give it a name".into()); }
    if n.chars().count() > 40 { return Err("the name is too long (40 characters at most)".into()); }
    if n.chars().any(char::is_control) { return Err("the name has characters that can't be used".into()); }
    Ok(n.to_string())
}
/// Write through a temporary file, so a crash can't leave half of one.
pub fn write_safe(p: &Path, text: &str) -> Result<(), String> {
    if let Some(d) = p.parent() { std::fs::create_dir_all(d).map_err(|e| format!("can't create {}: {e}", d.display()))?; }
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, p)).map_err(|e| format!("can't write {}: {e}", p.display()))
}

impl Store {
    pub fn real() -> Store { Store { dir: crate::util::config_dir().join("presets"), profile: crate::app::profile_path() } }

    fn index_path(&self) -> PathBuf { self.dir.join("presets.json") }
    fn file(&self, app: &str, id: &str) -> PathBuf { self.dir.join(app).join(format!("{id}.toml")) }
    fn profile_text(&self) -> Result<String, String> { std::fs::read_to_string(&self.profile).map_err(|e| format!("can't read {}: {e}", self.profile.display())) }

    /// The list, with every app the profile knows. Nothing is written until something changes.
    pub fn index(&self) -> Result<Index, String> {
        let mut ix: Index = match std::fs::read_to_string(self.index_path()) {
            Ok(t) => serde_json::from_str(&t).map_err(|e| format!("{} is not usable: {e}", self.index_path().display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Index::default(),
            Err(e) => return Err(format!("can't read {}: {e}", self.index_path().display())),
        };
        let p = Profiles::parse(&self.profile_text()?)?;
        let mut apps: Vec<String> = std::iter::once("default".to_string()).chain(crate::ext::apps().map(|(i, _)| i.id().to_string())).collect();
        apps.extend(p.apps.values().cloned());
        apps.extend(p.ctxs.keys().map(|c| c.split('/').next().unwrap_or(c).to_string()));
        let first = ix.globals.is_empty() && ix.global.is_none() && ix.apps.is_empty();
        for a in apps {
            let e = ix.apps.entry(a).or_default();
            if e.list.is_empty() && e.active.is_empty() {
                e.active = "my-layout".into();
                e.list.push(Preset { id: "my-layout".into(), name: "My layout".into(), note: String::new() });
            }
        }
        if first {
            let pick = ix.apps.iter().map(|(a, x)| (a.clone(), x.active.clone())).collect();
            ix.globals.push(Global { id: "my-setup".into(), name: "My setup".into(), pick });
            ix.global = Some("my-setup".into());
        }
        Ok(ix)
    }
    fn save_index(&self, ix: &Index) -> Result<(), String> {
        write_safe(&self.index_path(), &serde_json::to_string_pretty(ix).map_err(|e| e.to_string())?)?;
        REV.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// A preset's tables. `current` = the app's tables in the profile.
    fn content(&self, ix: &Index, app: &str, id: &str, current: &str) -> Result<String, String> {
        match id {
            STARTER => app_text(default_toml(), app),
            EMPTY => Ok(String::new()),
            _ => {
                let a = ix.apps.get(app).ok_or(format!("no app named {app}"))?;
                if !a.list.iter().any(|p| p.id == id) { return Err(format!("{} has no preset {id:?}", crate::edit::app_label(app))); }
                if a.active == id { return Ok(current.to_string()); }
                let t = std::fs::read_to_string(self.file(app, id)).map_err(|e| format!("can't read the preset: {e}"))?;
                check_piece(&t, app)?;
                Ok(t)
            }
        }
    }
    pub fn name_of(&self, ix: &Index, app: &str, id: &str) -> String {
        builtin_name(id).map(String::from).or_else(|| ix.apps.get(app).and_then(|a| a.list.iter().find(|p| p.id == id)).map(|p| p.name.clone())).unwrap_or_else(|| id.to_string())
    }

    fn backup(&self, text: &str) -> Result<(), String> {
        let d = self.dir.join("_backups");
        let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        write_safe(&d.join(format!("profiles-{ms}.toml")), text)?;
        let mut old: Vec<PathBuf> = std::fs::read_dir(&d).map(|r| r.flatten().map(|e| e.path()).filter(|p| p.extension().map_or(false, |x| x == "toml")).collect()).unwrap_or_default();
        old.sort();
        for p in old.iter().take(old.len().saturating_sub(KEEP_BACKUPS)) {
            if let Err(e) = std::fs::remove_file(p) { log(&format!("presets: can't remove the old backup {}: {e}", p.display())); }
        }
        Ok(())
    }

    /// Store the app's current tables under the preset in use. A built-in one stays as it is; if it
    /// was changed, the changes become a preset of their own.
    fn store_current(&self, ix: &mut Index, app: &str, current: &str) -> Result<(), String> {
        let active = ix.apps[app].active.clone();
        match builtin_name(&active) {
            None => write_safe(&self.file(app, &active), current),
            Some(n) => {
                if same(&self.content(ix, app, &active, current)?, current) { return Ok(()); }
                let a = ix.apps.get_mut(app).unwrap();
                let name = self.free_name(a, &format!("{n}, edited"));
                let id = unique(&slug(&name), a.list.iter().map(|p| p.id.as_str()));
                write_safe(&self.file(app, &id), current)?;
                a.list.push(Preset { id: id.clone(), name, note: format!("From {n}") });
                a.active = id;
                Ok(())
            }
        }
    }
    fn free_name(&self, a: &AppPresets, name: &str) -> String {
        if !a.list.iter().any(|p| p.name.eq_ignore_ascii_case(name)) { return name.to_string(); }
        (2..).map(|n| format!("{name} {n}")).find(|x| !a.list.iter().any(|p| p.name.eq_ignore_ascii_case(x))).unwrap()
    }

    /// Use these presets (app -> id). `global`: what the choice as a whole is now called.
    pub fn switch(&self, picks: &BTreeMap<String, String>, global: Option<Option<String>>) -> Result<Switched, String> {
        let mut ix = self.index()?;
        let text = self.profile_text()?;
        let before_global = ix.global.clone();
        if let Some(Some(g)) = &global { if !ix.globals.iter().any(|x| &x.id == g) && g != STARTER { return Err("that global preset no longer exists".into()); } }
        // everything is read and checked before anything is written
        let mut todo = vec![];
        for (app, id) in picks {
            let a = ix.apps.get(app).ok_or(format!("no app named {app}"))?;
            if &a.active == id { continue; }
            let cur = app_text(&text, app)?;
            let new = self.content(&ix, app, id, &cur)?;
            todo.push((app.clone(), id.clone(), cur, new));
        }
        let mut out = text.clone();
        for (app, _, _, new) in &todo { out = replace_app(&out, app, new)?; }
        let (mut before, mut after) = (BTreeMap::new(), BTreeMap::new());
        if !todo.is_empty() {
            self.backup(&text)?;
            for (app, id, cur, _) in &todo {
                let old = ix.apps[app].active.clone();
                self.store_current(&mut ix, app, cur)?;
                let kept = ix.apps[app].active.clone();
                // a changed built-in preset just got its own name: the global preset being left follows
                if kept != old {
                    if let Some(g) = ix.globals.iter_mut().find(|g| Some(&g.id) == before_global.as_ref()) {
                        if g.pick.get(app) == Some(&old) { g.pick.insert(app.clone(), kept.clone()); }
                    }
                }
                before.insert(app.clone(), kept);
                ix.apps.get_mut(app).unwrap().active = id.clone();
                after.insert(app.clone(), id.clone());
            }
            write_safe(&self.profile, &out)?;
        }
        match global {
            Some(g) => ix.global = g,
            None => {
                // a single app's switch: the global preset in use remembers it
                let g = ix.global.clone();
                match ix.globals.iter_mut().find(|x| Some(&x.id) == g.as_ref()) {
                    Some(x) => for (app, id) in &after { x.pick.insert(app.clone(), id.clone()); },
                    None => ix.global = None,
                }
            }
        }
        self.save_index(&ix)?;
        Ok(Switched { before, after, before_global, after_global: ix.global.clone() })
    }

    pub fn switch_global(&self, id: &str) -> Result<Switched, String> {
        let ix = self.index()?;
        if id == STARTER {
            let picks = ix.apps.keys().filter(|a| app_text(default_toml(), a).map_or(false, |t| !t.is_empty())).map(|a| (a.clone(), STARTER.to_string())).collect();
            return self.switch(&picks, Some(Some(STARTER.into())));
        }
        let g = ix.globals.iter().find(|g| g.id == id).ok_or("that global preset no longer exists")?;
        // a preset deleted since is left out
        let picks = g.pick.iter().filter(|(a, p)| ix.apps.get(*a).map_or(false, |x| builtin_name(p).is_some() || x.list.iter().any(|y| &y.id == *p)))
            .map(|(a, p)| (a.clone(), p.clone())).collect();
        // the one being left keeps what is in use now
        let mut ix2 = ix.clone();
        if let Some(cur) = ix2.globals.iter_mut().find(|x| Some(&x.id) == ix.global.as_ref()) {
            cur.pick = ix.apps.iter().map(|(a, x)| (a.clone(), x.active.clone())).collect();
            self.save_index(&ix2)?;
        }
        self.switch(&picks, Some(Some(id.to_string())))
    }

    pub fn save_as(&self, app: &str, name: &str) -> Result<String, String> {
        let name = clean_name(name)?;
        let mut ix = self.index()?;
        let cur = app_text(&self.profile_text()?, app)?;
        let a = ix.apps.get(app).ok_or(format!("no app named {app}"))?;
        if a.list.iter().any(|p| p.name.eq_ignore_ascii_case(&name)) { return Err(format!("there is already a preset named {name}")); }
        // the preset that was in use keeps today's state; a changed built-in one becomes the new preset
        if builtin_name(&a.active).is_none() { self.store_current(&mut ix, app, &cur)?; }
        let a = ix.apps.get_mut(app).unwrap();
        let id = unique(&slug(&name), a.list.iter().map(|p| p.id.as_str()));
        a.list.push(Preset { id: id.clone(), name, note: String::new() });
        a.active = id.clone();
        write_safe(&self.file(app, &id), &cur)?;
        let g = ix.global.clone();
        match ix.globals.iter_mut().find(|x| Some(&x.id) == g.as_ref()) {
            Some(x) => { x.pick.insert(app.to_string(), id.clone()); }
            None => ix.global = None,
        }
        self.save_index(&ix)?;
        Ok(id)
    }

    pub fn rename(&self, app: &str, id: &str, name: &str) -> Result<(), String> {
        let name = clean_name(name)?;
        let mut ix = self.index()?;
        let a = ix.apps.get_mut(app).ok_or(format!("no app named {app}"))?;
        if a.list.iter().any(|p| p.id != id && p.name.eq_ignore_ascii_case(&name)) { return Err(format!("there is already a preset named {name}")); }
        a.list.iter_mut().find(|p| p.id == id).ok_or("a built-in preset can't be renamed")?.name = name;
        self.save_index(&ix)
    }

    /// Moves the file to `_deleted/`.
    pub fn delete(&self, app: &str, id: &str) -> Result<(), String> {
        let mut ix = self.index()?;
        let a = ix.apps.get_mut(app).ok_or(format!("no app named {app}"))?;
        if a.active == id { return Err("it is in use; switch to another preset first".into()); }
        if !a.list.iter().any(|p| p.id == id) { return Err("a built-in preset can't be deleted".into()); }
        a.list.retain(|p| p.id != id);
        let f = self.file(app, id);
        if f.exists() {
            let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
            let to = self.dir.join("_deleted").join(format!("{app}-{id}-{ms}.toml"));
            std::fs::create_dir_all(to.parent().unwrap()).and_then(|_| std::fs::rename(&f, &to)).map_err(|e| format!("can't move the preset away: {e}"))?;
        }
        for g in &mut ix.globals { if g.pick.get(app).map(String::as_str) == Some(id) { g.pick.remove(app); } }
        self.save_index(&ix)
    }

    /// `text`: a preset file, or a whole profiles.toml (the app's part is taken).
    pub fn import(&self, app: &str, name: &str, note: &str, text: &str) -> Result<String, String> {
        let name = clean_name(name)?;
        let mut ix = self.index()?;
        Profiles::parse(text).map_err(|e| format!("the file doesn't load: {e}"))?;
        let piece = app_text(text, app)?;
        if piece.is_empty() { return Err(format!("the file has nothing for {}", crate::edit::app_label(app))); }
        check_piece(&piece, app)?;
        let a = ix.apps.get_mut(app).ok_or(format!("no app named {app}"))?;
        if a.list.iter().any(|p| p.name.eq_ignore_ascii_case(&name)) { return Err(format!("there is already a preset named {name}")); }
        let id = unique(&slug(&name), a.list.iter().map(|p| p.id.as_str()));
        write_safe(&self.file(app, &id), &piece)?;
        a.list.push(Preset { id: id.clone(), name, note: note.to_string() });
        self.save_index(&ix)?;
        Ok(id)
    }

    pub fn export(&self, app: &str, id: &str) -> Result<String, String> {
        let ix = self.index()?;
        self.content(&ix, app, id, &app_text(&self.profile_text()?, app)?)
    }

    pub fn global_save_as(&self, name: &str) -> Result<String, String> {
        let name = clean_name(name)?;
        let mut ix = self.index()?;
        if ix.globals.iter().any(|g| g.name.eq_ignore_ascii_case(&name)) { return Err(format!("there is already a global preset named {name}")); }
        let id = unique(&slug(&name), ix.globals.iter().map(|g| g.id.as_str()));
        let pick = ix.apps.iter().map(|(a, x)| (a.clone(), x.active.clone())).collect();
        ix.globals.push(Global { id: id.clone(), name, pick });
        ix.global = Some(id.clone());
        self.save_index(&ix)?;
        Ok(id)
    }
    pub fn global_rename(&self, id: &str, name: &str) -> Result<(), String> {
        let name = clean_name(name)?;
        let mut ix = self.index()?;
        if ix.globals.iter().any(|g| g.id != id && g.name.eq_ignore_ascii_case(&name)) { return Err(format!("there is already a global preset named {name}")); }
        ix.globals.iter_mut().find(|g| g.id == id).ok_or("it can't be renamed")?.name = name;
        self.save_index(&ix)
    }
    pub fn global_delete(&self, id: &str) -> Result<(), String> {
        let mut ix = self.index()?;
        if ix.global.as_deref() == Some(id) { return Err("it is in use; switch to another global preset first".into()); }
        let n = ix.globals.len();
        ix.globals.retain(|g| g.id != id);
        if ix.globals.len() == n { return Err("it can't be deleted".into()); }
        self.save_index(&ix)
    }

    /// Copies of profiles.toml in the config folder that hold a different version of this app,
    /// newest first: (file name, text).
    fn old_profiles(&self, app: &str) -> Vec<(String, String)> {
        let Some(dir) = self.profile.parent() else { return vec![] };
        let cur = self.profile_text().and_then(|t| app_text(&t, app)).unwrap_or_default();
        let mut files: Vec<_> = std::fs::read_dir(dir).map(|r| r.flatten().filter_map(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            (n.starts_with("profiles.") && n.ends_with(".toml") && n != "profiles.toml").then(|| (e.metadata().and_then(|m| m.modified()).ok(), n, e.path()))
        }).collect()).unwrap_or_default();
        files.sort_by(|a, b| b.0.cmp(&a.0));
        let mut seen: Vec<String> = vec![cur];
        let mut out = vec![];
        for (_, n, p) in files {
            let Some(piece) = std::fs::read_to_string(&p).ok().and_then(|t| app_text(&t, app).ok()) else { continue };
            if piece.is_empty() || seen.iter().any(|s| same(s, &piece)) { continue; }
            seen.push(piece.clone());
            out.push((n, piece));
        }
        out
    }
    pub fn import_old(&self, app: &str, file: &str, name: &str) -> Result<String, String> {
        let (_, piece) = self.old_profiles(app).into_iter().find(|(n, _)| n == file).ok_or("that backup is not there any more")?;
        self.import(app, name, &format!("From {file}"), &piece)
    }

    /// Everything the editor's menus show.
    pub fn describe(&self) -> Result<J, String> {
        let ix = self.index()?;
        let text = self.profile_text()?;
        let mut apps = serde_json::Map::new();
        for (app, a) in &ix.apps {
            let cur = app_text(&text, app)?;
            let cp = Profiles::parse(&cur)?;
            let row = |id: &str, name: &str, note: &str, builtin: bool| -> J {
                let used: Vec<&str> = ix.globals.iter().filter(|g| g.pick.get(app).map(String::as_str) == Some(id)).map(|g| g.name.as_str()).collect();
                let active = a.active == id;
                // a built-in preset in use is compared with its own content: changes show as "edited"
                match self.content(&ix, app, id, &cur).and_then(|t| Profiles::parse(&t)) {
                    Ok(p) => json!({"id": id, "name": name, "note": note, "builtin": builtin, "active": active, "count": count(&p), "differ": diff(&p, &cp), "globals": used}),
                    Err(e) => json!({"id": id, "name": name, "note": note, "builtin": builtin, "active": active, "error": e, "globals": used}),
                }
            };
            let mut list: Vec<J> = a.list.iter().map(|p| row(&p.id, &p.name, &p.note, false)).collect();
            if !app_text(default_toml(), app)?.is_empty() { list.push(row(STARTER, "Starter", "", true)); }
            list.push(row(EMPTY, "Empty", "", true));
            let act = list.iter().find(|r| r["active"] == true).cloned().unwrap_or(J::Null);
            let edited = act["builtin"] == true && act["differ"].as_u64().unwrap_or(0) > 0;
            apps.insert(app.clone(), json!({"active": a.active, "name": format!("{}{}", self.name_of(&ix, app, &a.active), if edited { ", edited" } else { "" }), "list": list}));
        }
        let mut globals: Vec<J> = ix.globals.iter().map(|g| {
            let on = ix.global.as_deref() == Some(g.id.as_str());
            // the one in use is whatever is in use now
            let pick: serde_json::Map<String, J> = ix.apps.iter().filter_map(|(app, a)| {
                let id = if on { Some(&a.active) } else { g.pick.get(app) };
                id.map(|id| (app.clone(), json!(self.name_of(&ix, app, id))))
            }).collect();
            json!({"id": g.id, "name": g.name, "builtin": false, "active": on, "pick": pick})
        }).collect();
        globals.push(json!({"id": STARTER, "name": "Starter", "builtin": true, "active": ix.global.as_deref() == Some(STARTER), "pick": {}}));
        let name = ix.global.as_ref().and_then(|g| globals.iter().find(|x| x["id"] == g.as_str())).map(|g| g["name"].clone()).unwrap_or(json!("Custom"));
        Ok(json!({"apps": apps, "globals": globals, "global": ix.global, "global_name": name, "rev": REV.load(Ordering::Relaxed)}))
    }

    /// From a panel key. scope "app" (the app in front) or "global"; name "next" or a preset's name.
    /// Returns what is in use now.
    pub fn by_key(&self, scope: &str, app: &str, name: &str) -> Result<String, String> {
        let ix = self.index()?;
        let next = |ids: Vec<&str>, cur: Option<&str>| -> Option<String> {
            if ids.len() < 2 { return None; }
            let i = ids.iter().position(|x| Some(*x) == cur).map_or(0, |i| (i + 1) % ids.len());
            Some(ids[i].to_string())
        };
        if scope == "global" {
            let id = if name == "next" {
                next(ix.globals.iter().map(|g| g.id.as_str()).collect(), ix.global.as_deref()).ok_or("there is only one global preset")?
            } else if name.eq_ignore_ascii_case("starter") { STARTER.to_string() } else {
                ix.globals.iter().find(|g| g.name.eq_ignore_ascii_case(name)).map(|g| g.id.clone()).ok_or(format!("no global preset named {name}"))?
            };
            self.switch_global(&id)?;
            return Ok(format!("global preset {}", ix.globals.iter().find(|g| g.id == id).map_or("Starter", |g| g.name.as_str())));
        }
        let app = if ix.apps.contains_key(app) { app } else { "default" };
        let a = &ix.apps[app];
        let id = if name == "next" {
            next(a.list.iter().map(|p| p.id.as_str()).collect(), Some(a.active.as_str())).ok_or(format!("{} has only one preset", crate::edit::app_label(app)))?
        } else {
            match name.to_lowercase().as_str() {
                "starter" => STARTER.into(), "empty" => EMPTY.into(),
                _ => a.list.iter().find(|p| p.name.eq_ignore_ascii_case(name)).map(|p| p.id.clone()).ok_or(format!("{} has no preset named {name}", crate::edit::app_label(app)))?,
            }
        };
        self.switch(&BTreeMap::from([(app.to_string(), id.clone())]), None)?;
        Ok(format!("{} preset {}", crate::edit::app_label(app), self.name_of(&self.index()?, app, &id)))
    }

    /// Name of the preset in use for the app in front, if it has several.
    pub fn title_for(&self, app: &str) -> Option<String> {
        let ix = self.index().ok()?;
        let a = ix.apps.get(app)?;
        (a.list.len() > 1 || builtin_name(&a.active).is_some()).then(|| self.name_of(&ix, app, &a.active))
    }
}

/// One request from the editor.
pub fn api(st: &Store, v: &J) -> Result<J, String> {
    let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
    let r = match s("op").as_str() {
        "use" => json!(st.switch(&BTreeMap::from([(s("app"), s("id"))]), None)?),
        "use_global" => json!(st.switch_global(&s("id"))?),
        // undo / redo of a switch
        "restore" => {
            let picks = v["picks"].as_object().ok_or("picks missing")?.iter().map(|(a, i)| (a.clone(), i.as_str().unwrap_or("").to_string())).collect();
            json!(st.switch(&picks, Some(v["global"].as_str().map(String::from)))?)
        }
        "save_as" => json!({"id": st.save_as(&s("app"), &s("name"))?}),
        "rename" => { st.rename(&s("app"), &s("id"), &s("name"))?; json!({}) }
        "delete" => { st.delete(&s("app"), &s("id"))?; json!({}) }
        "import" => json!({"id": st.import(&s("app"), &s("name"), "Imported", &s("text"))?}),
        "import_old" => json!({"id": st.import_old(&s("app"), &s("file"), &s("name"))?}),
        "global_save_as" => json!({"id": st.global_save_as(&s("name"))?}),
        "global_rename" => { st.global_rename(&s("id"), &s("name"))?; json!({}) }
        "global_delete" => { st.global_delete(&s("id"))?; json!({}) }
        x => return Err(format!("unknown request {x:?}")),
    };
    log(&format!("presets: {}", v.as_object().map(|o| o.iter().filter(|(k, _)| *k != "text").map(|(k, x)| format!("{k}={x}")).collect::<Vec<_>>().join(" ")).unwrap_or_default()));
    Ok(r)
}

/// Backups the editor offers to import for an app: [{file, count}]
pub fn old_list(st: &Store, app: &str) -> J {
    json!(st.old_profiles(app).into_iter().map(|(f, t)| json!({"file": f, "count": Profiles::parse(&t).map(|p| count(&p)).unwrap_or(0)})).collect::<Vec<_>>())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "# top\n[settings]\nbacklight = 3\n\n[apps]\nphoto = [\"photo pro.exe\"]\n\n\
        # ---- every app\n[default.ring]\nLEFT = { do = \"mouse.scroll\", axis = \"v\" }\n\n\
        # ---- Photo\n[photo.button]\nUNDO = { do = \"ph.cmd\", id = \"undo\" }  # keep\n\n[photo.develop.knob]\nHUE = { do = \"ph.param\", id = \"Tint\", per_turn = 50 }\n\n\
        [photo.develop.leds]\nCURSOR = \"ph.tool == dust\"\n\n\
        # ---- Cutter\n[cutter.button]\nSTOP = { do = \"key\", keys = \"k\" }\n";

    fn store(name: &str) -> Store {
        let d = std::env::temp_dir().join(format!("mp_presets_{}_{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("profiles.toml"), SRC).unwrap();
        Store { dir: d.join("presets"), profile: d.join("profiles.toml") }
    }
    fn text(s: &Store) -> String { s.profile_text().unwrap() }
    fn pick(a: &str, i: &str) -> BTreeMap<String, String> { BTreeMap::from([(a.to_string(), i.to_string())]) }

    #[test]
    fn splitting_and_putting_back_changes_nothing() {
        crate::ext::testing::init();
        for app in ["default", "photo", "cutter"] {
            assert_eq!(replace_app(SRC, app, &app_text(SRC, app).unwrap()).unwrap(), SRC, "{app}");
        }
        let lr = app_text(SRC, "photo").unwrap();
        assert!(lr.contains("# ---- Photo") && lr.contains("# keep") && lr.contains("[photo.develop.leds]") && !lr.contains("cutter") && !lr.contains("settings"), "{lr}");
        assert_eq!(app_text(SRC, "nope").unwrap(), "");
        let gone = replace_app(SRC, "photo", "").unwrap();
        assert!(!gone.contains("photo.button") && gone.contains("[cutter.button]") && gone.contains("# top"));
        let back = replace_app(&gone, "photo", &lr).unwrap();
        assert!(same(&back, SRC) && back.contains("# keep"));
        assert!(replace_app(SRC, "photo", "[photo.knob]\nNOPE = 1\n").is_err());
    }

    #[test]
    fn nothing_is_written_until_something_changes() {
        crate::ext::testing::init();
        let s = store("lazy");
        let d = s.describe().unwrap();
        assert_eq!(d["apps"]["photo"]["name"], "My layout");
        assert_eq!((d["global_name"].as_str(), d["apps"]["photo"]["list"][0]["count"].as_u64()), (Some("My setup"), Some(3)));
        assert!(!s.dir.exists());
    }

    #[test]
    fn switch_there_and_back() {
        crate::ext::testing::init();
        let s = store("back");
        let id = s.save_as("photo", "Portraits").unwrap();
        assert_eq!(text(&s), SRC, "saving under a new name changes no assignment");
        let out = crate::edit::set_binding(&text(&s), "photo", "", "button", "REDO", Some(&json!({"do":"ph.cmd","id":"redo"}))).unwrap();
        std::fs::write(&s.profile, &out).unwrap();
        let sw = s.switch(&pick("photo", "my-layout"), None).unwrap();
        assert_eq!((sw.before["photo"].as_str(), sw.after["photo"].as_str()), (id.as_str(), "my-layout"));
        assert_eq!(text(&s), SRC, "the first preset is back, comments and all");
        s.switch(&pick("photo", &id), None).unwrap();
        assert_eq!(text(&s), out, "and the change stayed with the second one");
        assert_eq!(std::fs::read_dir(s.dir.join("_backups")).unwrap().count(), 2);
        let d = s.describe().unwrap();
        assert_eq!(d["apps"]["photo"]["list"][0]["differ"], 1);
        assert_eq!(d["apps"]["cutter"]["name"], "My layout");
        // same preset again: nothing happens
        assert!(s.switch(&pick("photo", &id), None).unwrap().after.is_empty());
    }

    #[test]
    fn builtin_presets_stay_as_they_are() {
        crate::ext::testing::init();
        let s = store("builtin");
        s.switch(&pick("photo", STARTER), None).unwrap();
        assert!(same(&app_text(&text(&s), "photo").unwrap(), &app_text(default_toml(), "photo").unwrap()));
        assert_eq!(s.describe().unwrap()["apps"]["photo"]["name"], "Starter");
        // unchanged: leaving it creates nothing
        s.switch(&pick("photo", "my-layout"), None).unwrap();
        assert_eq!(s.index().unwrap().apps["photo"].list.len(), 1);
        // changed: the change becomes a preset
        s.switch(&pick("photo", EMPTY), None).unwrap();
        assert_eq!(app_text(&text(&s), "photo").unwrap(), "");
        let out = crate::edit::set_binding(&text(&s), "photo", "", "button", "REDO", Some(&json!("none"))).unwrap();
        std::fs::write(&s.profile, &out).unwrap();
        assert_eq!(s.describe().unwrap()["apps"]["photo"]["name"], "Empty, edited");
        let sw = s.switch(&pick("photo", "my-layout"), None).unwrap();
        assert_eq!(sw.before["photo"], "empty-edited");
        assert_eq!(s.index().unwrap().apps["photo"].list[1].name, "Empty, edited");
        assert!(same(&text(&s), SRC));
        // undo goes back to it
        s.switch(&sw.before, Some(sw.before_global)).unwrap();
        assert!(app_text(&text(&s), "photo").unwrap().contains("REDO"));
    }

    #[test]
    fn global_presets() {
        crate::ext::testing::init();
        let s = store("global");
        s.save_as("photo", "Portraits").unwrap();          // my-setup now picks it
        assert_eq!(s.index().unwrap().globals[0].pick["photo"], "portraits");
        s.global_save_as("Travel").unwrap();
        s.switch(&pick("photo", "my-layout"), None).unwrap(); // Travel remembers this
        s.switch(&pick("cutter", EMPTY), None).unwrap();
        assert!(!text(&s).contains("cutter.button"));
        s.switch_global("my-setup").unwrap();
        let ix = s.index().unwrap();
        assert_eq!((ix.apps["photo"].active.as_str(), ix.apps["cutter"].active.as_str()), ("portraits", "my-layout"));
        assert!(text(&s).contains("cutter.button"));
        s.switch_global("travel").unwrap();
        let ix = s.index().unwrap();
        assert_eq!((ix.apps["photo"].active.as_str(), ix.apps["cutter"].active.as_str(), ix.global.as_deref()), ("my-layout", EMPTY, Some("travel")));
        // Cutter starts empty in Travel; a change there belongs to Travel
        let out = crate::edit::set_binding(&text(&s), "cutter", "", "button", "LOOP", Some(&json!("none"))).unwrap();
        std::fs::write(&s.profile, &out).unwrap();
        s.switch_global("my-setup").unwrap();
        assert!(!text(&s).contains("LOOP"));
        s.switch_global("travel").unwrap();
        assert!(text(&s).contains("LOOP") && s.index().unwrap().apps["cutter"].active == "empty-edited");
        assert!(s.global_delete("travel").is_err(), "in use");
        s.global_delete("my-setup").unwrap();
        let sw = s.switch_global(STARTER).unwrap();
        assert!(!sw.after.contains_key("cutter"), "no starter for Cutter: left alone");
        assert_eq!(s.describe().unwrap()["global_name"], "Starter");
        s.switch(&pick("cutter", "my-layout"), None).unwrap();
        assert_eq!(s.describe().unwrap()["global_name"], "Custom", "a built-in global preset doesn't change");
        assert!(s.switch_global("nope").is_err());
    }

    #[test]
    fn names_delete_import_export() {
        crate::ext::testing::init();
        let s = store("names");
        let id = s.save_as("photo", "  Portraits ").unwrap();
        assert!(s.save_as("photo", "portraits").unwrap_err().contains("already"));
        assert!(s.save_as("photo", " ").is_err());
        assert!(s.save_as("nope", "x").is_err());
        s.rename("photo", &id, "People").unwrap();
        assert!(s.rename("photo", STARTER, "x").is_err());
        assert!(s.delete("photo", &id).is_err(), "in use");
        s.switch(&pick("photo", "my-layout"), None).unwrap();
        s.global_save_as("G").unwrap();
        s.delete("photo", &id).unwrap();
        assert_eq!(std::fs::read_dir(s.dir.join("_deleted")).unwrap().count(), 1);
        assert!(s.switch(&pick("photo", &id), None).is_err());
        let t = s.export("photo", "my-layout").unwrap();
        assert!(t.contains("# keep") && !t.contains("cutter"));
        let a = s.import("photo", "From file", "", &t).unwrap();
        let b = s.import("photo", "From whole", "", SRC).unwrap();
        assert!(same(&s.export("photo", &a).unwrap(), &s.export("photo", &b).unwrap()));
        assert!(s.import("photo", "x", "", "[cutter.button]\nSTOP = \"none\"\n").unwrap_err().contains("nothing for"));
        assert!(s.import("photo", "x", "", "not toml [").is_err());
        assert!(s.import("photo", "x", "", "[photo.knob]\nNOPE = \"none\"\n").is_err());
        assert_eq!(text(&s), SRC);
    }

    #[test]
    fn a_failed_switch_changes_nothing() {
        crate::ext::testing::init();
        let s = store("fail");
        let id = s.save_as("photo", "Broken").unwrap();
        s.switch(&pick("photo", "my-layout"), None).unwrap();
        std::fs::write(s.file("photo", &id), "[photo.knob]\nNOPE = \"none\"\n").unwrap();
        let before = (text(&s), s.index().unwrap());
        assert!(s.switch(&pick("photo", &id), None).is_err());
        std::fs::write(s.file("photo", &id), "[cutter.button]\nSTOP = \"none\"\n").unwrap();
        assert!(s.switch(&pick("photo", &id), None).unwrap_err().contains("cutter"));
        assert_eq!((text(&s), s.index().unwrap()), before);
        assert!(s.describe().unwrap()["apps"]["photo"]["list"][1]["error"].is_string());
        std::fs::write(&s.profile, "broken [").unwrap();
        assert!(s.switch(&pick("photo", STARTER), None).is_err());
        assert_eq!(text(&s), "broken [");
        std::fs::write(s.index_path(), "{").unwrap();
        assert!(s.index().unwrap_err().contains("not usable"), "a damaged list is reported, not replaced");
    }

    #[test]
    fn panel_keys() {
        crate::ext::testing::init();
        let s = store("keys");
        assert!(s.by_key("app", "photo", "next").is_err(), "only one");
        s.save_as("photo", "B").unwrap();
        assert_eq!(s.by_key("app", "photo", "next").unwrap(), "Photo Pro preset My layout");
        assert_eq!(s.by_key("app", "photo", "next").unwrap(), "Photo Pro preset B");
        assert_eq!(s.by_key("app", "photo", "my LAYOUT").unwrap(), "Photo Pro preset My layout");
        assert!(s.by_key("app", "photo", "nope").is_err());
        assert!(s.by_key("app", "firefox", "next").unwrap_err().contains("All apps"), "an app without its own assignments uses All apps");
        assert!(s.by_key("global", "", "next").is_err());
        s.global_save_as("Two").unwrap();
        assert_eq!(s.by_key("global", "", "next").unwrap(), "global preset My setup");
        assert_eq!(s.title_for("photo").as_deref(), Some("My layout"));
        assert_eq!(s.title_for("cutter"), None);
    }

    /// The real profile in MICROPANEL_HOME: `cargo test real_profile -- --ignored`
    #[test]
    #[ignore]
    fn real_profile() {
        crate::ext::testing::init();
        let t = std::fs::read_to_string(crate::app::profile_path()).unwrap();
        let p = Profiles::parse(&t).unwrap();
        let apps: Vec<String> = p.ctxs.keys().map(|c| c.split('/').next().unwrap().to_string()).collect();
        for a in apps { assert_eq!(replace_app(&t, &a, &app_text(&t, &a).unwrap()).unwrap(), t, "{a}"); }
        let d = std::env::temp_dir().join(format!("mp_real_{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("profiles.toml"), &t).unwrap();
        let s = Store { dir: d.join("presets"), profile: d.join("profiles.toml") };
        for a in ["photo", "cutter", "default", "explorer"] {
            s.switch(&pick(a, EMPTY), None).unwrap();
            assert!(app_text(&text(&s), a).unwrap().is_empty());
        }
        s.switch(&["photo", "cutter", "default", "explorer"].iter().map(|a| (a.to_string(), "my-layout".to_string())).collect(), None).unwrap();
        assert!(same(&text(&s), &t), "all assignments back");
        let lines = |x: &str| { let mut v: Vec<String> = x.lines().filter(|l| !l.trim().is_empty()).map(String::from).collect(); v.sort(); v };
        assert_eq!(lines(&text(&s)), lines(&t), "and every comment line");
        println!("identical text: {}", text(&s) == t);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn old_copies_of_the_profile() {
        crate::ext::testing::init();
        let s = store("old");
        let d = s.profile.parent().unwrap().to_path_buf();
        std::fs::write(d.join("profiles.before-x.toml"), SRC.replace("Tint", "Temperature")).unwrap();
        std::fs::write(d.join("profiles.before-y.toml"), SRC.replace("Tint", "Temperature").replace("keys = \"k\"", "keys = \"j\"")).unwrap();
        std::fs::write(d.join("profiles.broken.toml"), "[[[").unwrap();
        let l = old_list(&s, "photo");
        assert_eq!(l.as_array().unwrap().len(), 1, "same Photo part twice: offered once. {l}");
        assert_eq!(old_list(&s, "default").as_array().unwrap().len(), 0, "unchanged part: not offered");
        let f = l[0]["file"].as_str().unwrap().to_string();
        let id = s.import_old("photo", &f, "Older").unwrap();
        assert!(s.export("photo", &id).unwrap().contains("Temperature"));
        assert!(s.import_old("photo", "..\\x.toml", "z").is_err());
    }
}
