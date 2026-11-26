//! Lightroom Classic, through the Micro Panel plugin (lightroom/README.md).

mod link;
mod plugin;
mod sliders;
use link::{LrLink, LrState};
use sliders::{Change, Sliders};
use micropanel::ext::{Act, AppInfo, Integration, Io, Parsed, Place, Role, SettingRef};
use micropanel::leds::Val;
use micropanel::profiles::{default_per, not_allowed, num, pos, req, text, Kind, Settings};
use serde_json::{json, Value as J};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::Mutex;
use toml::{Table, Value};

/// The plugin this was written with (Core.lua); an older one needs a restart of Lightroom.
const PLUGIN: &str = "0.3.7";
/// The oldest one that knows every message we send.
const PLUGIN_OK: [u32; 3] = [0, 2, 0];
/// Is version `v` ("0.3.3") at least `min`?
pub(crate) fn newer(v: &str, min: [u32; 3]) -> bool {
    let v: Vec<u32> = v.split('.').filter_map(|x| x.parse().ok()).collect();
    v.as_slice() >= &min[..]
}
/// Older than the plugin this helper carries?
fn outdated(v: Option<&str>) -> bool {
    let Some(v) = v else { return false }; // it hasn't said yet
    let mut p = PLUGIN.split('.').map(|x| x.parse().unwrap_or(0));
    !newer(v, std::array::from_fn(|_| p.next().unwrap_or(0)))
}
fn at_least(v: Option<&str>) -> bool { newer(v.unwrap_or(""), PLUGIN_OK) }
/// AI Denoise is switched on at this amount.
const DENOISE: i64 = 20;
/// Colour-wheel movement per ball step at speed 1.
const WHEEL_PER_STEP: f64 = 0.1;
/// Light operands the plugin always sends; anything else is a develop setting to watch.
const FIELDS: [&str; 7] = ["flag", "rating", "label", "module", "tool", "cg_view", "photo"];
const PLACES: [(&str, &str, bool); 9] = [
    ("lightroom", "Everywhere", false),
    ("lightroom/library", "Library", false),
    ("lightroom/develop", "Develop", false),
    ("lightroom/map", "Map", true),
    ("lightroom/book", "Book", true),
    ("lightroom/slideshow", "Slideshow", true),
    ("lightroom/print", "Print", true),
    ("lightroom/web", "Web", true),
    ("lightroom/dialog", "Dialog boxes", true),
];
/// Develop's tools; the first is Develop itself.
const TOOLS: [(&str, &str); 5] = [
    ("lightroom/develop", "Any tool"),
    ("lightroom/develop/crop", "Crop"),
    ("lightroom/develop/dust", "Remove"),
    ("lightroom/develop/redeye", "Red eye"),
    ("lightroom/develop/masking", "Masking"),
];
const ACTIONS: [(&str, &str, &str, &str); 4] = [
    ("lr.param", "knob, ring, button", "id, per_turn | id, value, reset", "knob/ring: changes a develop setting by per_turn units per full turn. button: sets `value` or resets (reset = true)"),
    ("lr.xy", "ball", "x, y, per_unit, x_per_unit, y_per_unit, x_dir, y_dir", "ball changes two develop settings; x_per_unit / y_per_unit = setting units per ball step for each (default per_unit, 0.1); x_dir / y_dir = roll direction in degrees that raises each (default 0 = right, 90 = up), e.g. Temp/Tint like a colour wheel: x_dir = 135, y_dir = 45"),
    ("lr.wheel", "ball", "zone, speed", "moves a colour-grading wheel point: zone shadow/midtone/highlight/global"),
    ("lr.cmd", "button, knob, ring", "id | cw, ccw, per", "runs a Lightroom command; knob/ring: cw/ccw command per `per` counts"),
];

/// Not while one of its dialogs is in front, and (by default) only while Lightroom is in front.
fn allowed(stack: &[String], s: &Settings) -> bool {
    if stack.first().map_or(false, |c| c.ends_with("/dialog")) { return false; }
    s.ext.get("lr_actions_need_focus").and_then(|v| v.as_bool()) == Some(false) || stack.iter().any(|c| c == "lightroom")
}

#[derive(Debug)]
struct Param { id: String, per_turn: f64 }
impl Act for Param {
    fn allowed(&self, stack: &[String], s: &Settings) -> bool { allowed(stack, s) }
    fn turn(&self, counts: f64, per: f64, io: &mut dyn Io) {
        let d = counts / per * self.per_turn;
        io.send(json!({"t":"param","id":self.id,"delta":d}));
    }
}

/// one message at each press
#[derive(Debug)]
struct Press(J);
impl Act for Press {
    fn allowed(&self, stack: &[String], s: &Settings) -> bool { allowed(stack, s) }
    fn press(&self, io: &mut dyn Io) { io.send(self.0.clone()); }
}

#[derive(Debug)]
struct CmdStep { cw: Option<String>, ccw: Option<String>, per: f64 }
impl Act for CmdStep {
    fn allowed(&self, stack: &[String], s: &Settings) -> bool { allowed(stack, s) }
    fn turn(&self, counts: f64, _per: f64, io: &mut dyn Io) {
        for cw in io.steps(counts, self.per) {
            if let Some(id) = if cw { &self.cw } else { &self.ccw } { io.send(json!({"t":"cmd","id":id})); }
        }
    }
}

#[derive(Debug)]
struct Xy { x: Option<String>, y: Option<String>, x_per: f64, y_per: f64, m: [f64; 4] }
impl Act for Xy {
    fn allowed(&self, stack: &[String], s: &Settings) -> bool { allowed(stack, s) }
    fn ball(&self, dx: f64, dy: f64, io: &mut dyn Io) {
        let m = self.m;
        let (u, v) = (m[0] * dx + m[1] * dy, m[2] * dx + m[3] * dy);
        if let (Some(id), true) = (&self.x, u.abs() > 1e-9) { io.send(json!({"t":"param","id":id,"delta":u * self.x_per})); }
        if let (Some(id), true) = (&self.y, v.abs() > 1e-9) { io.send(json!({"t":"param","id":id,"delta":v * self.y_per})); }
    }
}

#[derive(Debug)]
struct Wheel { zone: String, speed: f64 }
impl Act for Wheel {
    fn allowed(&self, stack: &[String], s: &Settings) -> bool { allowed(stack, s) }
    fn ball(&self, dx: f64, dy: f64, io: &mut dyn Io) {
        let k = WHEEL_PER_STEP * self.speed;
        io.send(json!({"t":"wheel","zone":self.zone,"dx":dx * k,"dy":dy * k}));
    }
}

pub struct Lightroom {
    link: LrLink,
    /// (connection, list) of the last watch message
    watched: Mutex<(u64, Vec<String>)>,
    /// show values while adjusting; (connection, what it was told)
    show: Mutex<(bool, i64, Option<(u64, bool, i64)>)>,
    /// the masking brush, moved through Lightroom's own sliders
    sliders: Sliders,
    /// a setting was changed while Alt was held
    alt_used: AtomicBool,
}

impl Default for Lightroom { fn default() -> Self { Self::new() } }

impl Lightroom {
    pub fn new() -> Self { Lightroom { link: LrLink::new(), watched: Mutex::default(), show: Mutex::new((true, DENOISE, None)), sliders: Sliders::default(), alt_used: AtomicBool::new(false) } }
    fn st(&self) -> LrState { self.link.state.lock().unwrap().clone() }
    fn label(&self, cmd: bool, id: &str) -> String {
        let l = self.link.state.lock().unwrap();
        if let (false, Some(s)) = (cmd, sliders::SLIDERS.iter().find(|s| s.id == id)) { return s.label.into(); }
        if let (true, Some(s)) = (cmd, sliders::BUTTONS.iter().find(|s| s.id == id)) { return s.label.into(); }
        let list = if cmd { &l.cmds } else { &l.params };
        list.iter().rev().find(|c| c.id == id && !c.label.is_empty()).map(|c| c.label.clone()).unwrap_or_else(|| id.to_string())
    }
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}
fn show(n: f64) -> String { let r = (n * 1e6).round() / 1e6; if r == r.trunc() { format!("{}", r as i64) } else { format!("{r}") } }

impl Integration for Lightroom {
    fn id(&self) -> &'static str { "lightroom" }
    fn prefix(&self) -> &'static str { "lr" }
    fn app(&self) -> Option<AppInfo> {
        let tools = || TOOLS.iter().map(|(c, l)| (c.to_string(), l.to_string())).collect();
        Some(AppInfo { label: "Lightroom Classic".into(), short: "Lightroom".into(), exes: vec!["lightroom.exe".into()],
            places: PLACES.iter().map(|(c, l, more)| Place { ctx: c.to_string(), label: l.to_string(), more: *more,
                tools: if *c == "lightroom/develop" { tools() } else { vec![] } }).collect() })
    }
    fn start(&self) {
        self.link.start();
        let link = self.link.sender();
        let said = Mutex::new(None::<std::time::Instant>);
        self.sliders.start(std::sync::Arc::new(move |r| match r {
            Ok(t) if t.is_empty() => {}
            Ok(t) => { link.send(&json!({"t": "show", "text": t})); }
            Err(e) => {
                let mut s = said.lock().unwrap();
                if s.is_some_and(|x| x.elapsed().as_secs() < 5) { return; }
                *s = Some(std::time::Instant::now());
                micropanel::util::log(&format!("lightroom: {e}"));
                link.send(&json!({"t": "show", "text": e}));
            }
        }));
    }
    fn send(&self, m: &J) -> bool {
        if let Some(id) = m["id"].as_str() {
            let c = match m["t"].as_str() { Some("param") => m["delta"].as_f64().map(Change::By), Some("set") => m["value"].as_f64().map(Change::To), _ => None };
            if let Some(r) = c.and_then(|c| self.sliders.send(id, c)) { return r; }
            if m["t"] == "cmd" { if let Some(r) = self.sliders.press(id) { return r; } }
        }
        // Lightroom shows its clipping overlay while Alt is held, and keeps it for seconds after the last change
        if m["t"] == "param" && micropanel::sys::alt_down() { self.alt_used.store(true, Relaxed); }
        self.link.send(m)
    }
    fn places(&self) -> Vec<String> {
        let lr = self.st();
        let mut v = vec![];
        if let (true, Some(m)) = (lr.connected, &lr.module) {
            if m == "develop" {
                if let Some(t) = lr.tool.as_ref().filter(|t| t.as_str() != "loupe") { v.push(format!("lightroom/develop/{t}")); }
            }
            v.push(format!("lightroom/{m}"));
        }
        v
    }
    fn actions(&self) -> &'static [(&'static str, &'static str, &'static str, &'static str)] { &ACTIONS }

    fn parse(&self, what: &str, kind: Kind, t: &Table, p: &str) -> Result<Parsed, String> {
        let rel = matches!(kind, Kind::Knob | Kind::Ring);
        let own = |k: &str| text(t, k, p).map(|x| x.map(String::from));
        Ok(match (what, kind) {
            ("lr.param", _) if rel => (&["id", "per_turn"], Box::new(Param { id: req(t, "id", p)?.into(), per_turn: num(t, "per_turn", 1.0, p)? })),
            ("lr.param", Kind::Button) => {
                let reset = matches!(t.get("reset"), Some(Value::Boolean(true)));
                if reset == t.contains_key("value") { return Err(format!("{p}: a button lr.param needs exactly one of `value` or `reset = true`")); }
                let id = req(t, "id", p)?;
                let m = if reset { json!({"t":"reset","id":id}) } else { json!({"t":"set","id":id,"value":num(t, "value", 0.0, p)?}) };
                (&["id", "value", "reset"], Box::new(Press(m)))
            }
            ("lr.xy", Kind::Ball) => {
                let (x, y) = (own("x")?, own("y")?);
                if x.is_none() && y.is_none() { return Err(format!("{p}: `x` and/or `y` required")); }
                let per = num(t, "per_unit", 0.1, p)?;
                // x_dir / y_dir: the roll direction (degrees, 0 = right, 90 = up) that raises the x / y
                // setting. The ball's movement is split into those two directions (inverse of the 2x2).
                let (ax, ay) = (num(t, "x_dir", 0.0, p)?.to_radians(), num(t, "y_dir", 90.0, p)?.to_radians());
                let det = (ay - ax).sin();
                if det.abs() < 0.2 { return Err(format!("{p}: x_dir and y_dir must be at least 12 degrees apart")); }
                let m = [ay.sin() / det, -ay.cos() / det, -ax.sin() / det, ax.cos() / det];
                (&["x", "y", "per_unit", "x_per_unit", "y_per_unit", "x_dir", "y_dir"], Box::new(Xy { x, y,
                    x_per: num(t, "x_per_unit", per, p)?, y_per: num(t, "y_per_unit", per, p)?, m }))
            }
            ("lr.wheel", Kind::Ball) => {
                let zone = req(t, "zone", p)?;
                if !["shadow", "midtone", "highlight", "global"].contains(&zone) {
                    return Err(format!("{p}: zone must be shadow, midtone, highlight or global, not {zone:?}"));
                }
                (&["zone", "speed"], Box::new(Wheel { zone: zone.into(), speed: num(t, "speed", 1.0, p)? }))
            }
            ("lr.cmd", Kind::Button) => (&["id"], Box::new(Press(json!({"t":"cmd","id":req(t, "id", p)?})))),
            ("lr.cmd", _) if rel => {
                let (cw, ccw) = (own("cw")?, own("ccw")?);
                if cw.is_none() && ccw.is_none() { return Err(format!("{p}: `cw` and/or `ccw` required")); }
                (&["cw", "ccw", "per"], Box::new(CmdStep { cw, ccw, per: pos(num(t, "per", default_per(kind), p)?, "per", p)? }))
            }
            _ => return Err(not_allowed(what, kind, &ACTIONS, p)),
        })
    }
    fn setting(&self, key: &str, v: &Value) -> Option<Result<(), String>> {
        if key == "lr_denoise_default" { return Some(if v.as_integer().is_some_and(|n| (1..=100).contains(&n)) { Ok(()) } else { Err("1 to 100".into()) }); }
        matches!(key, "lr_actions_need_focus" | "lr_show_values").then(|| if v.is_bool() { Ok(()) } else { Err("true or false".into()) })
    }
    fn configure(&self, s: &Settings) {
        let mut o = self.show.lock().unwrap();
        o.0 = s.ext.get("lr_show_values").and_then(|v| v.as_bool()) != Some(false);
        o.1 = s.ext.get("lr_denoise_default").and_then(|v| v.as_integer()).unwrap_or(DENOISE);
    }

    fn facts(&self) -> Vec<(String, Val)> {
        let lr = self.st();
        if !lr.connected { return vec![]; }
        let s = |o: &Option<String>| o.clone().map(Val::Str).unwrap_or(Val::Null);
        let mut m: HashMap<String, Val> = HashMap::new();
        m.insert("module".into(), s(&lr.module));
        m.insert("tool".into(), s(&lr.tool));
        m.insert("photo".into(), Val::Bool(lr.photo));
        m.insert("flag".into(), s(&lr.flag));
        m.insert("label".into(), s(&lr.label));
        m.insert("cg_view".into(), s(&lr.cg_view));
        m.insert("rating".into(), lr.rating.map(Val::Num).unwrap_or(Val::Null));
        for (k, v) in &lr.values { m.insert(k.clone(), Val::Num(*v)); }
        m.into_iter().map(|(k, v)| (format!("lr.{k}"), v)).collect()
    }
    /// (Re)send the develop settings the light rules watch: on connect and when they change.
    fn watch(&self, operands: &[String]) {
        let ids: Vec<String> = operands.iter().filter(|o| !FIELDS.contains(&o.as_str())).cloned().collect();
        let (connected, n) = { let s = self.link.state.lock().unwrap(); (s.connected && at_least(s.plugin.as_deref()), s.connection) };
        // Alt let go: end the adjustment now
        if connected && self.alt_used.load(Relaxed) && !micropanel::sys::alt_down() {
            self.alt_used.store(false, Relaxed);
            self.link.send(&json!({"t": "done"}));
        }
        {
            let mut s = self.show.lock().unwrap();
            let want = Some((n, s.0, s.1));
            if connected && s.2 != want && self.link.send(&json!({"t": "options", "show_values": s.0, "denoise_default": s.1})) { s.2 = want; }
        }
        let connected = self.link.state.lock().unwrap().connected;
        let mut w = self.watched.lock().unwrap();
        if connected && (w.0 != n || w.1 != ids) && self.link.send(&json!({"t": "watch", "ids": ids})) { *w = (n, ids); }
    }

    fn describe(&self, b: &J, kind: Kind) -> Option<String> {
        let turn = matches!(kind, Kind::Knob | Kind::Ring);
        let t = |k: &str| b[k].as_str().unwrap_or("");
        let p = |id: &str| self.label(false, id);
        let c = |id: &str| if id.is_empty() { String::new() } else { self.label(true, id) };
        Some(match t("do") {
            "lr.param" if turn => p(t("id")),
            "lr.param" => {
                let v = b["value"].as_f64();
                if b["reset"] == true || (t("id").starts_with("local_") && v.unwrap_or(0.0) == 0.0) {
                    let l = p(t("id"));
                    let mut ch = l.chars();
                    let (a, z) = (ch.next(), ch.next());
                    // keep acronyms: only "Exposure" -> "exposure"
                    let l = match (a, z) { (Some(a), Some(z)) if a.is_ascii_uppercase() && z.is_ascii_lowercase() => a.to_lowercase().collect::<String>() + &l[1..], _ => l };
                    format!("Reset {l}")
                } else { format!("{} = {}", p(t("id")), show(v.unwrap_or(0.0))) }
            }
            "lr.xy" => [t("x"), t("y")].iter().filter(|x| !x.is_empty()).map(|x| p(x)).collect::<Vec<_>>().join(" / "),
            "lr.wheel" => { let z = if t("zone").is_empty() { "midtone" } else { t("zone") }; format!("{}{} wheel", cap(z), if z == "global" { "" } else { "s" }) }
            "lr.cmd" if turn => format!("{} / {}", c(t("cw")), c(t("ccw"))),
            "lr.cmd" => c(t("id")),
            _ => return None,
        })
    }
    fn shorten(&self, d: &str) -> Option<String> {
        if let Some(x) = match d {
            "Spot removal tool" => Some("Remove tool"),
            "Reset all adjustments" => Some("Reset all"),
            "Crop move left/right / Crop move up/down" => Some("Move crop"),
            "Mask overlay on/off" => Some("Mask overlay"),
            "White balance: As Shot" => Some("WB as shot"),
            "Mask temperature / Mask tint" | "Temperature / Tint" => Some("Temp / Tint"),
            "Before/after" => Some("Before / after"),
            "Paste settings from previous" => Some("Paste previous"),
            _ => None,
        } { return Some(x.into()); }
        let lower = |s: &str| s.chars().next().map_or(false, |c| c.is_ascii_lowercase());
        let mut s = d.to_string();
        if let Some(r) = s.strip_prefix("Mask ") { if lower(r) && !r.starts_with("overlay") { s = r.to_string(); } }
        if let Some(r) = s.strip_prefix("Reset mask ") { if lower(r) { s = format!("Reset {r}"); } }
        for p in ["Post-crop ", "Profile "] { if let Some(r) = s.strip_prefix(p) { s = r.to_string(); break; } }
        for (p, w) in [("New mask: ", "New "), ("Add to mask: ", "Add "), ("Subtract from mask: ", "Subtract ")] {
            if let Some(r) = s.strip_prefix(p) { s = format!("{w}{r}"); break; }
        }
        (s != d).then_some(s)
    }
    fn shortcut(&self, k: &str) -> Option<String> {
        Some(match k {
            "ctrl+shift+c" => "Copy settings", "ctrl+shift+v" => "Paste settings", "ctrl+alt+v" => "Paste settings from previous", "ctrl+alt+shift+left" => "Copy after to before", "ctrl+alt+shift+right" => "Copy before to after", "ctrl+alt+shift+up" => "Swap before and after", "ctrl+n" => "New snapshot", "backslash" => "Before/after", "f" => "Full-screen preview",
            _ => return None,
        }.into())
    }
    fn setting_ref(&self, b: &J, kind: Kind) -> Option<SettingRef> {
        if b["do"] != "lr.param" { return None; }
        let v = b["value"].as_f64().unwrap_or(0.0);
        let role = if kind != Kind::Button { Role::Turn } else if b["reset"] == true || v == 0.0 { Role::Reset } else { Role::Set(v) };
        Some(SettingRef { id: b["id"].as_str().unwrap_or("").into(), role })
    }

    fn script(&self) -> &'static str { include_str!("editor.js") }
    fn status(&self) -> J {
        let l = self.st();
        json!({"connected": l.connected, "old_plugin": l.connected && outdated(l.plugin.as_deref()), "plugin": l.plugin, "installed": plugin::found().name(), "module": l.module, "tool": l.tool, "flag": l.flag, "rating": l.rating, "label": l.label, "cg_view": l.cg_view, "values": l.values})
    }
    fn get(&self, path: &str) -> Option<J> {
        let l = self.link.state.lock().unwrap();
        let mut params = serde_json::to_value(&l.params).unwrap_or(J::Null);
        if let Some(a) = params.as_array_mut() {
            if !a.is_empty() { for s in &sliders::SLIDERS { a.push(json!({"id": s.id, "label": s.label, "group": s.group})); } }
        }
        let mut cmds = serde_json::to_value(&l.cmds).unwrap_or(J::Null);
        if let Some(a) = cmds.as_array_mut() {
            if !a.is_empty() { for s in &sliders::BUTTONS { a.push(json!({"id": s.id, "label": s.label, "group": s.group})); } }
        }
        (path == "catalog").then(|| json!({"params": params, "cmds": cmds}))
    }
    fn put(&self, path: &str, _body: &J) -> Option<Result<String, String>> {
        let done = |what: &str, r: Result<(), String>, text: &str| {
            match &r { Ok(()) => micropanel::util::log(&format!("lightroom: plugin {what}")), Err(e) => micropanel::util::log(&format!("lightroom: plugin not {what}: {e}")) }
            Some(r.map(|_| text.to_string()))
        };
        match path {
            "install" => done("installed", plugin::install(), "Installed. Restart Lightroom Classic to load it."),
            "remove" => done("removed", plugin::remove(), "Removed. Lightroom Classic stops using it when it is restarted."),
            _ => None,
        }
    }
    fn starter(&self) -> &'static str { include_str!("starter.toml") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use micropanel::engine::{Engine, Out};
    use micropanel::keys::Combo;
    use micropanel::panel::Event;
    use micropanel::profiles::{default_toml, Profiles};
    use std::sync::Arc;

    #[derive(Default)]
    struct Rec { log: Vec<String> }
    impl Out for Rec {
        fn key(&mut self, c: &Combo, d: bool, t: bool) { self.log.push(format!("key {c:?} {d} {t}")); }
        fn mouse_move(&mut self, x: i32, y: i32) { self.log.push(format!("move {x} {y}")); }
        fn scroll(&mut self, v: i32, h: i32, _m: &[u16]) { self.log.push(format!("scroll {v} {h}")); }
        fn mouse_button(&mut self, b: u8, d: bool) { self.log.push(format!("mb {b} {d}")); }
        fn midi(&mut self, m: [u8; 3]) -> bool { self.log.push(format!("midi {m:?}")); true }
        fn sheet(&mut self, _on: Option<bool>) {}
        fn preset(&mut self, _a: &str, _g: bool, _n: &str) {}
        fn ext(&mut self, id: &str, m: J) -> bool { self.log.push(format!("{id} {m}")); true }
    }
    fn init() { micropanel::ext::register(vec![Arc::new(Lightroom::new())]); }
    fn eng(src: &str, stack: &[&str]) -> Engine {
        init();
        let mut e = Engine::new(Profiles::parse(src).unwrap());
        e.stack = stack.iter().map(|s| s.to_string()).collect();
        e
    }
    fn btn(n: &str, down: bool) -> Event { Event::Button { name: n.into(), bit: 8, down, synthetic: false } }
    const DEV: [&str; 3] = ["lightroom/develop", "lightroom", "default"];

    #[test]
    fn starter_fits() {
        init();
        let p = Profiles::parse(default_toml()).unwrap();
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
        assert_eq!(p.app_name("Lightroom.exe"), "lightroom");
        assert_eq!(micropanel::edit::place_title("lightroom/develop/masking"), "Lightroom, Masking");
        let m = micropanel::edit::model(&p);
        assert_eq!(m["apps"][1]["places"].as_array().unwrap().len(), PLACES.len());
    }

    #[test]
    fn develop_knob_and_reset() {
        let mut e = eng(&default_toml_with().replace("start_deg = 2", "start_deg = 0"), &DEV);
        let mut r = Rec::default();
        e.handle(&Event::Knob { name: "Y_GAMMA", index: 1, delta: 2048, raw: 0 }, &mut r);
        e.handle(&btn("Y_GAMMA", true), &mut r);
        assert_eq!(r.log, vec![r#"lightroom {"delta":1.0,"id":"Exposure2012","t":"param"}"#, r#"lightroom {"id":"Exposure2012","t":"reset"}"#]);
    }
    fn default_toml_with() -> &'static str { init(); default_toml() }

    #[test]
    fn xy_axes_scale_and_rotate() {
        let mut e = eng("[lightroom.ball]\nLEFT = { do = \"lr.xy\", x = \"Temperature\", y = \"Tint\", x_per_unit = 2, y_per_unit = 0.1 }\nMIDDLE = { do = \"lr.xy\", x = \"Temperature\", y = \"Tint\", x_dir = 135, y_dir = 45 }\n", &DEV);
        let mut r = Rec::default();
        e.handle(&Event::Ball { name: "LEFT", dx: 10, dy: -5, raw_x: 0, raw_y: 0 }, &mut r);
        let s = r.log.join(" ");
        assert!(s.contains("Temperature") && s.contains("20.0") && s.contains("-0.5"), "{s}");
        // Temp up-left (135), Tint up-right (45): rolling straight up raises both equally
        r.log.clear();
        e.handle(&Event::Ball { name: "MIDDLE", dx: 0, dy: 10, raw_x: 0, raw_y: 0 }, &mut r);
        let d: Vec<f64> = r.log.iter().map(|l| serde_json::from_str::<J>(l.trim_start_matches("lightroom ")).unwrap()["delta"].as_f64().unwrap()).collect();
        assert_eq!(d.len(), 2, "{:?}", r.log);
        assert!((d[0] - d[1]).abs() < 1e-9 && d[0] > 0.0, "{d:?}");
        assert!(Profiles::parse("[default.ball]\nLEFT = { do = \"lr.xy\", x = \"Tint\", x_dir = 10, y_dir = 15 }\n").is_err());
        assert!(Profiles::parse("[default.button]\nSTOP = { do = \"lr.param\", id = \"Exposure2012\" }").unwrap_err().contains("exactly one"));
    }

    #[test]
    fn skipped_outside_lightroom_and_in_dialogs() {
        let mut e = eng(default_toml_with(), &["lightroom/dialog", "lightroom", "default"]);
        let mut r = Rec::default();
        e.handle(&btn("NEXT_FRAME", true), &mut r);
        e.handle(&Event::Ball { name: "RIGHT", dx: 3, dy: 0, raw_x: 0, raw_y: 0 }, &mut r);
        assert_eq!(r.log.len(), 1, "{:?}", r.log);
        assert!(r.log[0].starts_with("move"), "dialog: lr.cmd skipped, pointer still works");
        let src = "[default.button]\nSTOP = { do = \"lr.cmd\", id = \"undo\" }\n";
        let mut r = Rec::default();
        eng(src, &["chrome", "default"]).handle(&btn("STOP", true), &mut r);
        assert!(r.log.is_empty());
        eng(&format!("[settings]\nlr_actions_need_focus = false\n{src}"), &["chrome", "default"]).handle(&btn("STOP", true), &mut r);
        assert_eq!(r.log.len(), 1);
    }

    #[test]
    fn lights_follow_the_photo() {
        init();
        let lr = Lightroom::new();
        assert!(lr.facts().is_empty(), "not connected: nothing is lit");
        *lr.link.state.lock().unwrap() = LrState { connected: true, module: Some("library".into()), photo: true, flag: Some("reject".into()), label: Some("none".into()), ..Default::default() };
        assert_eq!(lr.places(), ["lightroom/library"]);
        let p = Profiles::parse(default_toml()).unwrap();
        let lit = || {
            let f = micropanel::leds::Facts { ext: lr.facts().into_iter().collect(), ..Default::default() };
            let st = ["lightroom".to_string(), "default".into()];
            let mut v: Vec<String> = p.led_rules(&st).into_iter().filter(|(_, c)| micropanel::leds::eval(c, &f)).map(|x| x.0.to_string()).collect();
            v.sort();
            v
        };
        assert_eq!(lit(), ["DISABLE", "PLAY_STILL"]);
        { let mut s = lr.link.state.lock().unwrap(); s.flag = Some("pick".into()); s.label = Some("red".into()); s.module = Some("develop".into()); s.tool = Some("masking".into()); }
        assert_eq!(lit(), ["VIEWER", "WIPE_STILL"]);
        assert_eq!(lr.places(), ["lightroom/develop/masking", "lightroom/develop"]);
    }

    #[test]
    fn plugin_versions() {
        for (v, ok) in [(None, false), (Some("0.1.0"), false), (Some("0.2.0"), true), (Some("0.2.10"), true), (Some("1.0.0"), true)] { assert_eq!(at_least(v), ok, "{v:?}"); }
        for (v, old) in [(None, false), (Some("0.3.4"), true), (Some(PLUGIN), false), (Some("9.0.0"), false)] { assert_eq!(outdated(v), old, "{v:?}"); }
    }

    #[test]
    fn words() {
        init();
        let lr = Lightroom::new();
        let it = |i: &str, l: &str| link::CatItem { id: i.into(), label: l.into(), group: String::new() };
        {
            let mut s = lr.link.state.lock().unwrap();
            s.params = vec![it("Exposure2012", "Exposure"), it("Temperature", "Temperature"), it("Tint", "Tint")];
            s.cmds = vec![it("toolSpot", "Spot removal tool")];
        }
        let d = |b: J, k| { let d = lr.describe(&b, k).unwrap(); lr.shorten(&d).unwrap_or(d) };
        assert_eq!(d(json!({"do":"lr.param","id":"Exposure2012","per_turn":2}), Kind::Knob), "Exposure");
        assert_eq!(d(json!({"do":"lr.param","id":"Exposure2012","reset":true}), Kind::Button), "Reset exposure");
        assert_eq!(d(json!({"do":"lr.param","id":"Exposure2012","value":0.5}), Kind::Button), "Exposure = 0.5");
        assert_eq!(d(json!({"do":"lr.xy","x":"Temperature","y":"Tint"}), Kind::Ball), "Temp / Tint");
        assert_eq!(d(json!({"do":"lr.cmd","id":"toolSpot"}), Kind::Button), "Remove tool");
        assert_eq!(d(json!({"do":"lr.cmd","id":"unknownId"}), Kind::Button), "unknownId");
        assert_eq!(lr.shorten("Mask exposure").as_deref(), Some("exposure"));
        assert_eq!(lr.shorten("Add to mask: Brush").as_deref(), Some("Add Brush"));
        assert_eq!(lr.shorten("Mask overlay on/off").as_deref(), Some("Mask overlay"));
        assert_eq!(lr.shorten("Exposure"), None);
        assert_eq!(lr.shortcut("ctrl+shift+c").as_deref(), Some("Copy settings"));
        assert_eq!(lr.shortcut("ctrl+alt+v").as_deref(), Some("Paste settings from previous"));
    }
}
