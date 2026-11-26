//! Integrations: everything the helper knows about one particular program or protocol.
//!
//! The core (panel driver, profiles, layers, places, presets, editor, cheat sheet) has no
//! knowledge of any application. An integration adds
//!   * actions (`do = "osc.value"`), each a small object that gets presses, turns and rolls,
//!   * sub-places of its application (modules, tools), if it is tied to one,
//!   * operands for light conditions (`osc./track/1/mute`),
//!   * words for the cheat sheet, and a script that teaches the mapping editor its actions.
//! Its model follows OSCQuery: a tree of parameters with type, range, unit and current value, which
//! the other side can push. See INTEGRATIONS.md.
//!
//! Integrations are registered once, at start: `micropanel::cli::main(vec![...])`.

use crate::leds::Val;
use crate::profiles::{Kind, Settings};
use serde_json::Value as J;
use std::sync::{Arc, OnceLock};
use toml::{Table, Value};

/// A place inside an application the helper can switch to, in the order the editor shows them.
#[derive(Debug, Clone)]
pub struct Place {
    /// context path: "app", "app/module", "app/module/tool"
    pub ctx: String,
    pub label: String,
    /// behind "More" in the editor
    pub more: bool,
    /// (context path, label); the first is the place itself ("Any tool")
    pub tools: Vec<(String, String)>,
}

/// What an application integration says about its application.
#[derive(Debug, Clone)]
pub struct AppInfo {
    /// "Lightning Photo Pro"
    pub label: String,
    /// short form for titles
    pub short: String,
    /// program files that are this application (lower-case)
    pub exes: Vec<String>,
    /// the first place is the application itself; empty = no sub-places
    pub places: Vec<Place>,
}

/// How a binding touches a setting, so the core can say "press resets what the knob turns".
#[derive(Debug, Clone, PartialEq)]
pub enum Role { Turn, Reset, Set(f64) }
#[derive(Debug, Clone, PartialEq)]
pub struct SettingRef { pub id: String, pub role: Role }

/// What an action can do while it handles an event.
pub trait Io {
    /// One message to the integration (`Integration::send`); false = not delivered.
    fn send(&mut self, msg: J) -> bool;
    /// Whole units out, the fraction is kept for the next event (so slow movement isn't lost).
    /// `slot` tells several counters of one control apart.
    fn carry(&mut self, slot: &str, v: f64) -> i32;
    /// Discrete steps of `per` counts: true = clockwise. The remainder carries over.
    fn steps(&mut self, counts: f64, per: f64) -> Vec<bool>;
}

/// One bound action. Implement what the control kinds it allows need.
pub trait Act: std::fmt::Debug + Send + Sync {
    /// Is it allowed to act now? (e.g. only while its application is in front)
    fn allowed(&self, _stack: &[String], _s: &Settings) -> bool { true }
    /// A layer that is open while the button is held.
    fn layer(&self) -> Option<&str> { None }
    fn press(&self, _io: &mut dyn Io) {}
    fn release(&self, _io: &mut dyn Io) {}
    /// Knob or ring: raw counts (+ = clockwise) and how many make a full turn.
    fn turn(&self, _counts: f64, _per_turn: f64, _io: &mut dyn Io) {}
    /// Ball: steps to the right and up.
    fn ball(&self, _dx: f64, _dy: f64, _io: &mut dyn Io) {}
}

/// (fields the binding may have, the action)
pub type Parsed = (&'static [&'static str], Box<dyn Act>);

pub trait Integration: Send + Sync {
    /// Short lower-case name; for an application integration also its context.
    fn id(&self) -> &'static str;
    /// Prefix of its actions and light operands: "osc" -> `osc.value`, `osc./x`.
    fn prefix(&self) -> &'static str { self.id() }
    /// None: a protocol, usable in every application.
    fn app(&self) -> Option<AppInfo> { None }
    /// Connect; called once. Never blocks.
    fn start(&self) {}
    /// Deliver one message from an action; false = nobody there.
    fn send(&self, _msg: &J) -> bool { false }
    /// Places in force while its application is in front, most specific first, without the
    /// application itself.
    fn places(&self) -> Vec<String> { vec![] }
    /// (do, controls, fields, help) for the reference list.
    fn actions(&self) -> &'static [(&'static str, &'static str, &'static str, &'static str)] { &[] }
    /// Read one binding. `what` starts with this integration's prefix.
    fn parse(&self, what: &str, kind: Kind, t: &Table, path: &str) -> Result<Parsed, String>;
    /// A key in `[settings]` that belongs to it; Some(Err) = known, bad value.
    fn setting(&self, _key: &str, _v: &Value) -> Option<Result<(), String>> { None }
    /// Called whenever the settings were (re)loaded.
    fn configure(&self, _s: &Settings) {}

    // ---- lights
    /// Values light conditions can use: ("prefix.name", value).
    fn facts(&self) -> Vec<(String, Val)> { vec![] }
    /// The operands (without prefix) some condition reads; called about ten times a second.
    fn watch(&self, _operands: &[String]) {}

    // ---- words (cheat sheet; the editor uses the script)
    fn describe(&self, _b: &J, _kind: Kind) -> Option<String> { None }
    /// Compact form of a description that goes on a key; also asked about other actions' words.
    fn shorten(&self, _text: &str) -> Option<String> { None }
    /// What a keyboard shortcut is called in its application.
    fn shortcut(&self, _keys: &str) -> Option<String> { None }
    fn setting_ref(&self, _b: &J, _kind: Kind) -> Option<SettingRef> { None }

    // ---- editor
    /// JavaScript that registers it with the editor (INTEGRATIONS.md).
    fn script(&self) -> &'static str { "" }
    /// For the editor's status chip and script, in /api/state.
    fn status(&self) -> J { J::Null }
    /// GET /api/ext/<id>/<path>
    fn get(&self, _path: &str) -> Option<J> { None }
    /// PUT /api/ext/<id>/<path>: something to do now; Ok/Err = words for the user, None = no such thing.
    fn put(&self, _path: &str, _body: &J) -> Option<Result<String, String>> { None }
    /// Its part of the profile a new user starts with, as TOML tables of its own context.
    fn starter(&self) -> &'static str { "" }
}

static ALL: OnceLock<Vec<Arc<dyn Integration>>> = OnceLock::new();

/// Prefixes of the core's own actions and light operands.
pub const CORE_PREFIXES: [&str; 6] = ["mouse", "midi", "mcu", "layer", "sheet", "restart"];

/// The integrations that come with the helper.
pub fn builtin() -> Vec<Arc<dyn Integration>> { vec![Arc::new(crate::osc::Osc::new())] }

/// Once, before anything else. A second call is ignored (tests).
pub fn register(list: Vec<Arc<dyn Integration>>) {
    for i in &list { assert!(!CORE_PREFIXES.contains(&i.prefix()), "integration prefix {} is taken", i.prefix()); }
    let _ = ALL.set(list);
}
pub fn all() -> &'static [Arc<dyn Integration>] { ALL.get().map(|v| v.as_slice()).unwrap_or(&[]) }
pub fn by_id(id: &str) -> Option<&'static Arc<dyn Integration>> { all().iter().find(|i| i.id() == id) }
pub fn by_prefix(p: &str) -> Option<&'static Arc<dyn Integration>> { all().iter().find(|i| i.prefix() == p) }
/// The integration an action belongs to: "osc.value" -> osc.
pub fn of_action(what: &str) -> Option<&'static Arc<dyn Integration>> { what.split_once('.').and_then(|(p, _)| by_prefix(p)) }
/// Application integrations, in order of registration.
pub fn apps() -> impl Iterator<Item = (&'static Arc<dyn Integration>, AppInfo)> { all().iter().filter_map(|i| i.app().map(|a| (i, a))) }
pub fn app(id: &str) -> Option<AppInfo> { by_id(id).and_then(|i| i.app()) }
pub fn facts() -> std::collections::HashMap<String, Val> { all().iter().flat_map(|i| i.facts()).collect() }
/// All editor scripts, each kept from breaking the others.
pub fn scripts() -> String {
    all().iter().filter(|i| !i.script().is_empty())
        .map(|i| format!("try {{\n{}\n}} catch (e) {{ console.error(\"integration {}\", e); }}\n", i.script(), i.id())).collect()
}

#[cfg(test)]
pub mod testing {
    //! A made-up photo application, so the core's tests can exercise the interface.
    use super::*;
    use serde_json::json;

    #[derive(Debug)]
    struct Turn { id: String, per_turn: f64 }
    impl Act for Turn {
        fn allowed(&self, stack: &[String], s: &Settings) -> bool {
            !stack.first().map_or(false, |c| c.ends_with("/dialog")) && (s.ext.get("photo_anywhere").is_some() || stack.iter().any(|c| c == "photo"))
        }
        fn turn(&self, n: f64, per: f64, io: &mut dyn Io) { io.send(json!({"t":"turn","id":self.id,"by":n / per * self.per_turn})); }
    }
    #[derive(Debug)]
    struct Press { msg: J, layer: Option<String> }
    impl Act for Press {
        fn layer(&self) -> Option<&str> { self.layer.as_deref() }
        fn press(&self, io: &mut dyn Io) { io.send(self.msg.clone()); }
        fn release(&self, io: &mut dyn Io) { if self.layer.is_some() { io.send(json!({"t":"up"})); } }
    }
    #[derive(Debug)]
    struct Frames;
    impl Act for Frames {
        fn turn(&self, n: f64, per: f64, io: &mut dyn Io) { let f = io.carry("", n / per * 48.0); if f != 0 { io.send(json!({"t":"frames","n":f})); } }
        fn ball(&self, dx: f64, dy: f64, io: &mut dyn Io) { io.send(json!({"t":"roll","x":dx,"y":dy})); }
    }
    pub struct Photo;
    impl Integration for Photo {
        fn id(&self) -> &'static str { "photo" }
        fn prefix(&self) -> &'static str { "ph" }
        fn app(&self) -> Option<AppInfo> {
            let p = |c: &str, l: &str, more: bool, tools: &[(&str, &str)]| Place { ctx: c.into(), label: l.into(), more, tools: tools.iter().map(|t| (t.0.into(), t.1.into())).collect() };
            Some(AppInfo { label: "Photo Pro".into(), short: "Photo".into(), exes: vec!["photo pro.exe".into()], places: vec![
                p("photo", "Everywhere", false, &[]), p("photo/library", "Library", false, &[]),
                p("photo/develop", "Develop", false, &[("photo/develop", "Any tool"), ("photo/develop/crop", "Crop"), ("photo/develop/masking", "Masking")]),
                p("photo/map", "Map", true, &[]), p("photo/dialog", "Dialog boxes", true, &[])] })
        }
        fn actions(&self) -> &'static [(&'static str, &'static str, &'static str, &'static str)] {
            &[("ph.param", "knob, ring, button", "id, per_turn | id, reset", "test"), ("ph.cmd", "button", "id, layer", "test"), ("ph.frames", "knob, ring, ball", "", "test")]
        }
        fn parse(&self, what: &str, kind: Kind, t: &Table, p: &str) -> Result<Parsed, String> {
            use crate::profiles::{num, req, text};
            Ok(match (what, kind) {
                ("ph.param", Kind::Knob | Kind::Ring) => (&["id", "per_turn"], Box::new(Turn { id: req(t, "id", p)?.into(), per_turn: num(t, "per_turn", 1.0, p)? })),
                ("ph.param", Kind::Button) => (&["id", "reset"], Box::new(Press { msg: json!({"t":"reset","id":req(t, "id", p)?}), layer: None })),
                ("ph.cmd", Kind::Button) => (&["id", "layer"], Box::new(Press { msg: json!({"t":"cmd","id":req(t, "id", p)?}), layer: text(t, "layer", p)?.map(String::from) })),
                ("ph.frames", Kind::Knob | Kind::Ring | Kind::Ball) => (&[], Box::new(Frames)),
                _ => return Err(crate::profiles::not_allowed(what, kind, self.actions(), p)),
            })
        }
        fn setting(&self, k: &str, v: &Value) -> Option<Result<(), String>> {
            (k == "photo_anywhere").then(|| if v.is_bool() { Ok(()) } else { Err("true or false".into()) })
        }
        fn facts(&self) -> Vec<(String, Val)> { vec![("ph.flag".into(), Val::Str("reject".into())), ("ph.rating".into(), Val::Num(3.0)), ("ph.Exposure".into(), Val::Num(0.5))] }
        fn describe(&self, b: &J, kind: Kind) -> Option<String> {
            let id = b["id"].as_str().unwrap_or("");
            Some(match b["do"].as_str()? { "ph.param" if kind == Kind::Button => format!("Reset {}", id.to_lowercase()), "ph.param" => id.into(), "ph.cmd" => format!("Do {id}"), _ => return None })
        }
        fn shorten(&self, t: &str) -> Option<String> { (t == "Do something long").then(|| "Long".into()) }
        fn shortcut(&self, k: &str) -> Option<String> { (k == "ctrl+shift+c").then(|| "Copy settings".into()) }
        fn setting_ref(&self, b: &J, kind: Kind) -> Option<SettingRef> {
            (b["do"] == "ph.param").then(|| SettingRef { id: b["id"].as_str().unwrap_or("").into(), role: if kind == Kind::Button { Role::Reset } else { Role::Turn } })
        }
        fn starter(&self) -> &'static str { "\n# ---- Photo Pro\n[photo.button]\nUNDO = { do = \"ph.cmd\", id = \"undo\" }\n\n[photo.develop.knob]\nY_GAMMA = { do = \"ph.param\", id = \"Exposure\", per_turn = 2 }\n" }
    }
    pub fn init() { register(vec![Arc::new(Photo)]); }
}
