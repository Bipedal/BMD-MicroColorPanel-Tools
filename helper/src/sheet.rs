//! Cheat sheet: what every control does in the app in front, with the layer that is open right now.
//! The window itself is the system's (`sys::sheet_window`).

use crate::edit::{app_label, place_title};
use crate::ext::Role;
use crate::panel::KNOBS;
use crate::profiles::{Action, Kind, Profiles};
use crate::util::{config_dir, log};
use serde_json::{json, Value as J};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub use crate::sys::sheet_window::{png_from_args, start};

// ------------------------------------------------------------------ what the user set

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Cfg {
    pub open: bool,
    /// a list instead of the panel drawing
    pub list: bool,
    /// the mouse goes through it to the window below
    pub click_through: bool,
    /// per cent
    pub opacity: u8,
    pub x: i32,
    pub y: i32,
    /// 0 = not placed yet
    pub w: i32,
    pub h: i32,
}
impl Default for Cfg {
    fn default() -> Self { Cfg { open: false, list: false, click_through: false, opacity: 92, x: 0, y: 0, w: 0, h: 0 } }
}

static CFG: Mutex<Option<Cfg>> = Mutex::new(None);

fn cfg_path() -> std::path::PathBuf { config_dir().join("cheatsheet.json") }

pub fn get() -> Cfg {
    let mut g = CFG.lock().unwrap();
    if g.is_none() {
        let c = match std::fs::read_to_string(cfg_path()) {
            Ok(t) => serde_json::from_str(&t).unwrap_or_else(|e| { log(&format!("cheat sheet: {} is not usable: {e}", cfg_path().display())); Cfg::default() }),
            Err(_) => Cfg::default(), // first run
        };
        *g = Some(c);
    }
    g.clone().unwrap()
}

/// Change the settings and remember them.
pub fn update(f: impl FnOnce(&mut Cfg)) -> Cfg {
    let mut c = get();
    let old = c.clone();
    f(&mut c);
    c.opacity = c.opacity.clamp(30, 100);
    if c != old {
        *CFG.lock().unwrap() = Some(c.clone());
        let r = serde_json::to_string(&c).map_err(|e| e.to_string()).and_then(|t| std::fs::write(cfg_path(), t).map_err(|e| e.to_string()));
        if let Err(e) = r { log(&format!("cheat sheet: can't save its settings: {e}")); }
    }
    c
}

/// A short message in the sheet's header, e.g. which preset a key switched to. It also brings the
/// sheet up for a moment when it is closed.
pub fn notice(t: &str) { *NOTICE.lock().unwrap() = Some((t.to_string(), Instant::now())); }
pub(crate) static NOTICE: Mutex<Option<(String, Instant)>> = Mutex::new(None);
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) const NOTICE_FOR: Duration = Duration::from_millis(2500);

/// From a panel key: Some = show / hide, None = the other way round.
pub fn switch(on: Option<bool>) { update(|c| c.open = on.unwrap_or(!c.open)); }

/// From the editor: {"open": true, "list": false, "click_through": false, "opacity": 90}
pub fn apply(v: &J) -> Result<Cfg, String> {
    let o = v.as_object().ok_or("an object is expected")?;
    for (k, x) in o {
        match k.as_str() {
            "open" | "list" | "click_through" if x.is_boolean() => {}
            "opacity" if x.as_u64().map_or(false, |n| (30..=100).contains(&n)) => {}
            _ => return Err(format!("bad value: {k} = {x}")),
        }
    }
    Ok(update(|c| {
        if let Some(b) = v["open"].as_bool() { c.open = b; }
        if let Some(b) = v["list"].as_bool() { c.list = b; }
        if let Some(b) = v["click_through"].as_bool() { c.click_through = b; }
        if let Some(n) = v["opacity"].as_u64() { c.opacity = n as u8; }
    }))
}

// ------------------------------------------------------------------ words (as the editor writes them)

pub(crate) fn cap(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}
fn num(n: f64) -> String { let r = (n * 1e6).round() / 1e6; if r == r.trunc() { format!("{}", r as i64) } else { format!("{r}") } }

fn keys_txt(t: &str) -> String {
    t.split(',').map(|c| c.trim().split('+').map(|x| match x {
        "rbracket" | "bracketright" => "]".into(), "lbracket" | "bracketleft" => "[".into(), "backslash" => "\\".into(),
        "comma" => ",".into(), "period" => ".".into(), "plus" => "+".into(), "minus" => "-".into(), "slash" => "/".into(),
        x => cap(x),
    }).collect::<Vec<String>>().join("+")).collect::<Vec<_>>().join(", ")
}

fn is_off(b: &J) -> bool { b == "none" || b["do"] == "none" }

/// The name the user gave it.
fn own_name(b: &J) -> Option<&str> { b["label"].as_str().map(str::trim).filter(|l| !l.is_empty()) }

/// What a binding does, in words.
pub fn describe(b: &J, kind: Kind, app: &str) -> String {
    if is_off(b) { return "Off".into(); }
    if let Some(l) = own_name(b) { return l.into(); }
    let turn = matches!(kind, Kind::Knob | Kind::Ring);
    let t = |k: &str| b[k].as_str().unwrap_or("");
    let i = |k: &str| b[k].as_i64().or(b[k].as_f64().map(|x| x as i64));
    let ch = || match i("channel") { Some(c) if c > 1 => format!(" ch{c}"), _ => String::new() };
    match t("do") {
        "key" => {
            let hold = b["hold"] == true;
            if !hold { if let Some(x) = crate::ext::by_id(app).and_then(|i| i.shortcut(t(if turn { "cw" } else { "keys" }))) { return x; } }
            let or_dash = |x: &str| if x.is_empty() { "–".to_string() } else { keys_txt(x) };
            if turn { format!("{} / {}", or_dash(t("cw")), or_dash(t("ccw"))) } else { format!("{}{}", keys_txt(t("keys")), if hold { " (held)" } else { "" }) }
        }
        "mouse.move" => "Mouse pointer".into(),
        "mouse.scroll" => format!("{}Scroll{}", if t("mods").is_empty() { String::new() } else { format!("{}+ ", keys_txt(t("mods"))) }, if t("axis") == "h" { " sideways" } else { "" }),
        "mouse.button" => {
            let b = if t("button").is_empty() { "left" } else { t("button") };
            if t("keys").is_empty() { format!("{} click", cap(b)) } else { format!("{} + {b} click", keys_txt(t("keys"))) }
        }
        "layer" => format!("Layer {}", t("name")),
        "mcu.fader" => format!("Fader {}", i("strip").unwrap_or(1)),
        "mcu.jog" => "Mackie jog".into(),
        "mcu.button" => format!("Mackie {}{}", t("button").replace('_', " "), i("strip").map(|s| format!(" {s}")).unwrap_or_default()),
        "midi.cc" => format!("MIDI CC {}{}", i("cc").unwrap_or(0), ch()),
        "midi.note" => format!("MIDI note {}{}", i("note").unwrap_or(0), ch()),
        "midi.xy" => format!("MIDI CC {}", [i("x"), i("y")].iter().flatten().map(|v| v.to_string()).collect::<Vec<_>>().join(" / ")),
        "sheet" => "Cheat sheet".into(),
        "preset" => {
            let what = if t("scope") == "global" { "global preset" } else { "preset" };
            match t("name") { "" | "next" => format!("Next {what}"), n => format!("{} {n}", cap(what)) }
        }
        x => crate::ext::of_action(x).and_then(|i| i.describe(b, kind)).unwrap_or_else(|| x.to_string()),
    }
}

/// The compact form that goes on a key.
pub fn short_label(b: &J, kind: Kind, app: &str) -> String {
    if let (Some(l), false) = (own_name(b), is_off(b)) { return l.into(); }
    let d = describe(b, kind, app);
    if b["do"] == "layer" { return "Layer".into(); }
    cap(&crate::ext::all().iter().find_map(|i| i.shorten(&d)).unwrap_or(d))
}

// ------------------------------------------------------------------ what to show

#[derive(Debug, Clone, PartialEq)]
pub struct Item { pub label: String, pub off: bool, /// set by the open layer (or no layer is open)
    pub bright: bool, /// a press that resets what its knob turns
    pub plain_reset: bool }

#[derive(Debug, Clone, PartialEq, Default)]
pub struct View {
    pub title: String,
    pub layer: String,
    pub items: HashMap<String, Item>,
    /// control touched just now
    pub touched: Option<String>,
    pub light: bool,
    pub list: bool,
    /// the preset in use, if the app has several
    pub preset: String,
    pub notice: String,
    /// how the panel is connected, and whether that is a problem
    pub link: (String, bool),
}

/// How the panel is connected, for the header. Nothing while it was handed over on purpose.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn link_text(link: Option<&str>, resting: bool, handed_over: bool) -> (String, bool) {
    let t = match link {
        Some("usb") => "USB",
        Some("bt") => "Bluetooth",
        Some(x) => x,
        None if handed_over => return Default::default(),
        None if resting => return ("Resting".into(), false),
        None => return ("Not connected".into(), true),
    };
    (if resting { format!("{t}, resting") } else { t.to_string() }, false)
}

pub(crate) fn key_nm(n: &str) -> String { n.replace('_', " ") }

/// Name of a layer: the key that opens it here.
fn layer_name(p: &Profiles, stack: &[String], layer: &str) -> String {
    let mut best: Option<(usize, String)> = None;
    for (i, c) in stack.iter().enumerate() {
        let Some(x) = p.ctxs.get(c) else { continue };
        for ((_, _, n), b) in &x.bind {
            let hit = match &b.action { Action::Layer { name, .. } => name == layer, Action::Ext(a) => a.act.layer() == Some(layer), _ => false };
            if hit && best.as_ref().map_or(true, |(j, m)| (i, n) < (*j, m)) { best = Some((i, n.clone())); }
        }
    }
    best.map(|(_, n)| key_nm(&n)).unwrap_or_else(|| layer.to_string())
}

pub fn build(p: &Profiles, stack: &[String], layers: &[String]) -> View {
    let root = stack.first().map(|c| c.split('/').next().unwrap_or(c).to_string()).unwrap_or_else(|| "default".into());
    let mut v = View { title: stack.first().map(|c| place_title(c)).unwrap_or_else(|| app_label("default")), ..Default::default() };
    v.layer = layers.iter().rev().map(|l| layer_name(p, stack, l)).collect::<Vec<_>>().join(" + ");
    for k in [Kind::Button, Kind::Knob, Kind::Ring, Kind::Ball] {
        for n in k.names() {
            let Some(f) = p.lookup(k, n, stack, layers) else { continue };
            let raw = serde_json::to_value(&f.binding.raw).unwrap_or(J::Null);
            let mut it = Item { label: short_label(&raw, k, &root), off: is_off(&raw), bright: layers.is_empty() || f.layer.is_some(), plain_reset: false };
            if k == Kind::Button && KNOBS.contains(&n) && own_name(&raw).is_none() {
                if let Some(t) = p.lookup(Kind::Knob, n, stack, layers) {
                    let tr = serde_json::to_value(&t.binding.raw).unwrap_or(J::Null);
                    let r = |b: &J, k: Kind| b["do"].as_str().and_then(crate::ext::of_action).and_then(|i| i.setting_ref(b, k));
                    if let (Some(a), Some(b)) = (r(&tr, Kind::Knob), r(&raw, Kind::Button)) {
                        if a.id == b.id && a.role == Role::Turn && tr["do"] == raw["do"] {
                            it.plain_reset = b.role == Role::Reset;
                            match b.role { Role::Reset => it.label = "Reset".into(), Role::Set(v) => it.label = format!("= {}", num(v)), Role::Turn => {} }
                        }
                    }
                }
            }
            v.items.insert(format!("{}.{n}", k.key()), it);
        }
    }
    v
}

/// For the editor.
pub fn state_json() -> J { let c = get(); json!({"open": c.open, "list": c.list, "click_through": c.click_through, "opacity": c.opacity}) }

#[cfg(test)]
mod tests {
    use super::*;

    fn s(b: J, k: Kind) -> String { crate::ext::testing::init(); short_label(&b, k, "photo") }

    #[test]
    fn labels() {
        assert_eq!(s(json!({"do":"ph.param","id":"Exposure","per_turn":2}), Kind::Knob), "Exposure");
        assert_eq!(s(json!({"do":"ph.param","id":"Exposure","reset":true}), Kind::Button), "Reset exposure");
        assert_eq!(s(json!({"do":"ph.cmd","id":"something long"}), Kind::Button), "Long", "the integration's own short form");
        assert_eq!(s(json!({"do":"zz.thing"}), Kind::Button), "Zz.thing", "an integration that isn't there");
        assert_eq!(s(json!({"do":"key","keys":"ctrl+shift+c"}), Kind::Button), "Copy settings");
        assert_eq!(s(json!({"do":"mouse.button","button":"left","keys":"space"}), Kind::Button), "Space + left click");
        assert_eq!(s(json!({"do":"key","keys":"shift+3, ctrl+z"}), Kind::Button), "Shift+3, Ctrl+Z");
        assert_eq!(s(json!({"do":"key","cw":"right","ccw":"left"}), Kind::Knob), "Right / Left");
        assert_eq!(s(json!({"do":"key","keys":"backquote","label":"Toggle flagged"}), Kind::Button), "Toggle flagged");
        assert_eq!(s(json!({"do":"ph.cmd","id":"something long","label":" "}), Kind::Button), "Long", "an empty name is none");
        assert_eq!(s(json!("none"), Kind::Ball), "Off");
        assert_eq!(short_label(&json!({"do":"key","keys":"ctrl+shift+c"}), Kind::Button, "default"), "Ctrl+Shift+C");
    }

    #[test]
    fn layers_brighten_and_name_themselves() {
        crate::ext::testing::init();
        let p = Profiles::parse("[default.button]\nSHIFT_UP = { do = \"layer\", mode = \"auto\", name = \"more\" }\n\
            [photo.develop.knob]\nHUE = { do = \"ph.param\", id = \"Exposure\", per_turn = 2 }\n\
            [photo.develop.button]\nHUE = { do = \"ph.param\", id = \"Exposure\", reset = true }\n\
            [photo.develop.layer.more.knob]\nHUE = { do = \"ph.param\", id = \"Dehaze\", per_turn = 2 }\n").unwrap();
        let st = crate::edit::stack_for("photo/develop");
        let v = build(&p, &st, &[]);
        assert_eq!((v.title.as_str(), v.layer.as_str()), ("Photo, Develop", ""));
        assert!(v.items["button.HUE"].plain_reset && v.items["button.HUE"].label == "Reset" && v.items["knob.HUE"].bright);
        let v = build(&p, &st, &["more".into()]);
        assert_eq!(v.layer, "SHIFT UP");
        assert_eq!((v.items["knob.HUE"].label.as_str(), v.items["knob.HUE"].bright), ("Dehaze", true));
        // the press still resets the base setting: shown as such, and dimmer
        assert_eq!((v.items["button.HUE"].label.as_str(), v.items["button.HUE"].bright, v.items["button.HUE"].plain_reset), ("Reset exposure", false, false));
        assert_eq!(build(&p, &crate::edit::stack_for("photo/develop/masking"), &[]).title, "Photo, Masking");
        assert_eq!(build(&p, &crate::edit::stack_for("firefox"), &[]).title, "Firefox");
    }

    #[test]
    fn connection_in_words() {
        assert_eq!(link_text(Some("usb"), false, false), ("USB".into(), false));
        assert_eq!(link_text(Some("bt"), true, false), ("Bluetooth, resting".into(), false));
        assert_eq!(link_text(None, true, false), ("Resting".into(), false), "gone quiet while nobody is around");
        assert_eq!(link_text(None, false, true), (String::new(), false), "Resolve has it");
        assert_eq!(link_text(None, false, false), ("Not connected".into(), true));
    }

    #[test]
    fn settings_from_the_editor_are_checked() {
        assert!(apply(&json!({"opacity": 10})).is_err());
        assert!(apply(&json!({"open": "yes"})).is_err());
        assert!(apply(&json!({"size": 3})).is_err());
        assert!(apply(&json!(3)).is_err());
    }
}
