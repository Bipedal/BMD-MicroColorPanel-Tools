//! Profiles: TOML -> contexts -> bindings, strict validation, context/layer lookup.
//! Actions of integrations (ext.rs) are read by them; one whose integration isn't part of this build
//! is kept but does nothing.
//! Format and lookup rules: PROFILES.md.

use crate::keys::{parse_combo, Combo};
use crate::panel::{BUTTONS, KNOBS, LEDS};
use std::collections::HashMap;
use toml::{Table, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind { Button, Knob, Ring, Ball }

impl Kind {
    pub fn key(self) -> &'static str {
        match self { Kind::Button => "button", Kind::Knob => "knob", Kind::Ring => "ring", Kind::Ball => "ball" }
    }
    pub fn from_key(s: &str) -> Option<Kind> {
        match s { "button" => Some(Kind::Button), "knob" => Some(Kind::Knob), "ring" => Some(Kind::Ring), "ball" => Some(Kind::Ball), _ => None }
    }
    pub fn names(self) -> Vec<&'static str> {
        match self {
            Kind::Button => BUTTONS.iter().map(|(_, n)| *n).collect(),
            Kind::Knob => KNOBS.to_vec(),
            Kind::Ring | Kind::Ball => vec!["LEFT", "MIDDLE", "RIGHT"],
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Axis { V, H, Both }

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    Key { combo: Combo, hold: bool },
    /// several combinations tapped in order ("shift+3, ctrl+z")
    KeySeq { combos: Vec<Combo> },
    KeyStep { cw: Option<Combo>, ccw: Option<Combo>, per: f64 },
    MouseMove { speed: f64, accel: f64, lock: f64 },
    /// mods: keys held around each wheel message (e.g. alt for a timeline's zoom);
    /// h_mods: the same for the ball's horizontal axis; notch: send whole notches (120) only
    MouseScroll { axis: Axis, speed: f64, mods: Vec<u16>, h_mods: Vec<u16>, notch: bool },
    /// keys: held while the button is (space + left = pan)
    MouseButton { button: u8, keys: Option<Combo> },
    Layer { name: String, mode: LayerMode },
    /// the cheat sheet window: shown or hidden at each press, or (hold) shown while held
    Sheet { hold: bool },
    /// held for a second: the helper starts again (handled by the panel's thread, so that it works when the rest hangs)
    Restart,
    /// switch presets (presets.rs): scope the app in front or everything; name "next" or a preset's name
    Preset { global: bool, name: String },
    /// an integration's action (ext.rs)
    Ext(ExtAct),
    /// an action of an integration this build doesn't have
    Unavailable,
    /// knob/ring: relative ticks (`enc`) or an absolute 0..127 value; `per_turn` ticks or values per turn
    MidiCc { ch: u8, cc: u8, rel: Option<crate::midi::RelEnc>, per_turn: f64 },
    /// key: `on` at press, `off` at release; toggle = alternate on/off per press
    MidiCcKey { ch: u8, cc: u8, on: u8, off: u8, toggle: bool },
    MidiNote { ch: u8, note: u8, vel: u8 },
    /// ball: x / y as two CCs, relative or absolute, `per_unit` per ball step
    MidiXy { ch: u8, x: Option<u8>, y: Option<u8>, rel: Option<crate::midi::RelEnc>, per_unit: f64 },
    /// Mackie Control fader `strip` (1-8, 9 = master) as pitch bend; touch note while it moves
    McuFader { strip: u8, per_turn: f64, touch_ms: u64 },
    /// Mackie Control jog wheel: CC 60 relative (1..63 cw, 65..127 ccw), `per_turn` ticks per turn
    McuJog { per_turn: f64 },
    /// Mackie Control button (note on 127 at press, 0 at release)
    McuButton { note: u8 },
}

/// Mackie Control buttons by name -> note number (MCU protocol). Strip buttons take a strip number.
pub const MCU_BUTTONS: [(&str, u8); 31] = [
    ("play", 0x5E), ("stop", 0x5D), ("record", 0x5F), ("rewind", 0x5B), ("ffwd", 0x5C),
    ("cycle", 0x56), ("marker", 0x54), ("nudge", 0x55), ("drop", 0x57), ("replace", 0x58), ("click", 0x59), ("solo_global", 0x5A),
    ("bank_left", 0x2E), ("bank_right", 0x2F), ("channel_left", 0x30), ("channel_right", 0x31), ("flip", 0x32),
    ("read", 0x4A), ("write", 0x4B), ("trim", 0x4C), ("touch", 0x4D), ("latch", 0x4E), ("group", 0x4F),
    ("save", 0x50), ("undo", 0x51), ("cancel", 0x52), ("enter", 0x53),
    ("up", 0x60), ("down", 0x61), ("left", 0x62), ("right", 0x63),
];
/// per-strip buttons: name -> first note (strip 1); strip n = first + n - 1
pub const MCU_STRIP_BUTTONS: [(&str, u8); 4] = [("arm", 0x00), ("solo", 0x08), ("mute", 0x10), ("select", 0x18)];

/// An integration's action and whose it is.
#[derive(Clone)]
pub struct ExtAct { pub id: &'static str, pub act: std::sync::Arc<dyn crate::ext::Act> }
impl std::fmt::Debug for ExtAct { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{}: {:?}", self.id, self.act) } }
impl PartialEq for ExtAct { fn eq(&self, o: &Self) -> bool { self.id == o.id && format!("{:?}", self.act) == format!("{:?}", o.act) } }

#[derive(Debug, Clone)]
pub struct Binding { pub action: Action, pub raw: Value }

impl Action {
    /// Opens or closes a layer (also an integration's action that holds one open).
    pub fn opens_layer(&self) -> bool { matches!(self, Action::Layer { .. }) || matches!(self, Action::Ext(x) if x.act.layer().is_some()) }
}

#[derive(Debug, Clone, Default)]
pub struct Ctx {
    /// (layer or "" for base, kind, control name) -> binding
    pub bind: HashMap<(String, Kind, String), Binding>,
    /// (LED name, condition, condition text); the list form `leds = ["USER"]` means always on
    pub leds: Vec<(String, crate::leds::Cond, String)>,
}

/// hold: on while the key is held. toggle: each press flips it. auto: a quick tap latches it on
/// (tap again to turn it off); holding the key while using other controls makes it momentary.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LayerMode { Hold, Toggle, Auto }

#[derive(Debug, Clone)]
pub struct Settings {
    pub backlight: u8,
    pub resolve_yield: bool,
    /// mouse.move: spread each ball report over this many ms (the panel sends ball bursts ~every 40 ms)
    pub pointer_smoothing_ms: u32,
    /// backlight and LEDs off after this many seconds without touching the panel (0 = never)
    pub backlight_off_after_s: u32,
    /// first movement of a knob or ring (degrees) / ball (steps) that counts for nothing, so that a bump
    /// of the table changes nothing
    pub start_deg: f64,
    pub start_ball: u32,
    /// after a knob was pressed, only a clear turn counts for this long
    pub press_ms: u32,
    /// on battery, let the panel switch itself off when neither it nor the PC was used this long (0 = never)
    pub sleep_after_min: u32,
    /// keys that restart the helper when held together this long; empty = off. They keep their own jobs.
    pub restart_keys: Vec<String>,
    pub restart_hold_s: u32,
    /// MIDI ports by name ("auto", "off" or part of the name)
    pub midi_out: String,
    pub midi_in: String,
    /// settings of integrations, as written
    pub ext: HashMap<String, Value>,
}
impl Default for Settings {
    fn default() -> Self { Settings { backlight: 60, resolve_yield: true, pointer_smoothing_ms: 40, backlight_off_after_s: 0, start_deg: 0.0, start_ball: 0, press_ms: 0, sleep_after_min: 0, restart_keys: vec![], restart_hold_s: 2, midi_out: "auto".into(), midi_in: "auto".into(), ext: HashMap::new() } }
}

#[derive(Debug, Clone, Default)]
pub struct Profiles {
    pub ctxs: HashMap<String, Ctx>,
    /// exe name (lower-case, with .exe) -> context name
    pub apps: HashMap<String, String>,
    pub settings: Settings,
    /// layers in which an unset control keeps its normal job; in the others it does nothing
    pub inherit: std::collections::HashSet<String>,
    /// things that were accepted but do nothing here
    pub warnings: Vec<String>,
}

/// "USER+LOOP": one or two keys of the panel. Never AUTO COLOR with STOP: held together they reset the panel itself.
pub fn restart_keys(s: &str) -> Result<Vec<String>, String> {
    let v: Vec<String> = s.split('+').map(|k| k.trim().to_uppercase().replace(' ', "_")).filter(|k| !k.is_empty()).collect();
    if v.len() > 2 { return Err("one or two keys".into()); }
    let known = Kind::Button.names();
    if let Some(k) = v.iter().find(|k| !known.contains(&k.as_str())) { return Err(format!("{k} is not a key of the panel")); }
    if v.len() == 2 && v[0] == v[1] { return Err("two different keys".into()); }
    if v.contains(&"AUTO_COLOR".to_string()) && v.contains(&"STOP".to_string()) { return Err("never AUTO COLOR with STOP: held together they reset the panel and clear its Bluetooth pairing".into()); }
    Ok(v)
}

pub struct Found<'a> { pub ctx: &'a str, pub layer: Option<&'a str>, pub binding: &'a Binding }

/// The profile a new user starts with: the core's part and each integration's.
pub fn default_toml() -> &'static str {
    static T: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let mut t = include_str!("default_profiles.toml").replace("\r\n", "\n");
        for i in crate::ext::all() { t.push_str(&i.starter().replace("\r\n", "\n")); }
        t
    })
}
const RESERVED: [&str; 6] = ["button", "knob", "ring", "ball", "layer", "leds"];

// readers of a binding's fields, also for integrations
pub fn num(t: &Table, k: &str, def: f64, p: &str) -> Result<f64, String> {
    match t.get(k) {
        None => Ok(def),
        Some(Value::Float(f)) => Ok(*f),
        Some(Value::Integer(i)) => Ok(*i as f64),
        Some(v) => Err(format!("{p}: `{k}` must be a number, not {}", v.type_str())),
    }
}
pub fn text<'a>(t: &'a Table, k: &str, p: &str) -> Result<Option<&'a str>, String> {
    match t.get(k) {
        None => Ok(None),
        Some(Value::String(x)) => Ok(Some(x.as_str())),
        Some(v) => Err(format!("{p}: `{k}` must be a string, not {}", v.type_str())),
    }
}
pub fn req<'a>(t: &'a Table, k: &str, p: &str) -> Result<&'a str, String> {
    match text(t, k, p)? { Some(x) if !x.is_empty() => Ok(x), _ => Err(format!("{p}: `{k}` is required")) }
}
fn combo(t: &Table, k: &str, p: &str) -> Result<Option<Combo>, String> {
    text(t, k, p)?.map(|x| parse_combo(x).map_err(|e| format!("{p}: `{k}`: {e}"))).transpose()
}

/// Modifier keys held around a wheel message: "alt", "ctrl+shift", ... (empty = none).
fn mods_of(t: &Table, k: &str, p: &str) -> Result<Vec<u16>, String> {
    match text(t, k, p)? {
        None | Some("") => Ok(vec![]),
        Some(x) => {
            let c = parse_combo(x).map_err(|e| format!("{p}: `{k}`: {e}"))?;
            let mut v = c.mods.clone();
            v.push(c.key);
            if v.iter().any(|&vk| !matches!(vk, 0x10 | 0x11 | 0x12 | 0x5B | 0x5C | 0xA0..=0xA5)) {
                return Err(format!("{p}: `{k}` takes modifier keys only (ctrl, shift, alt, win), not {x:?}"));
            }
            Ok(v)
        }
    }
}

pub fn int_in(t: &Table, k: &str, def: Option<i64>, lo: i64, hi: i64, p: &str) -> Result<i64, String> {
    let v = match t.get(k) {
        None => def.ok_or(format!("{p}: `{k}` is required"))?,
        Some(Value::Integer(i)) => *i,
        Some(v) => return Err(format!("{p}: `{k}` must be a whole number, not {}", v.type_str())),
    };
    if v < lo || v > hi { return Err(format!("{p}: `{k}` must be {lo}..{hi}, not {v}")); }
    Ok(v)
}
fn midi_mode(t: &Table, p: &str) -> Result<Option<crate::midi::RelEnc>, String> {
    use crate::midi::RelEnc;
    Ok(match text(t, "mode", p)?.unwrap_or("relative") {
        "relative" | "twos" => Some(RelEnc::Twos),
        "offset" | "relative_offset" => Some(RelEnc::Offset),
        "signbit" | "relative_signbit" => Some(RelEnc::SignBit),
        "absolute" => None,
        x => return Err(format!("{p}: mode must be relative (twos), offset, signbit or absolute, not {x:?}")),
    })
}

/// Parse one binding value for a control of `kind`.
pub fn parse_action(kind: Kind, v: &Value, p: &str) -> Result<Action, String> {
    let t = match v {
        Value::String(x) if x == "none" => return Ok(Action::None),
        Value::String(x) => return Err(format!("{p}: a plain string binding must be \"none\", not {x:?}")),
        Value::Table(t) => t,
        _ => return Err(format!("{p}: binding must be an inline table like {{ do = \"key\", keys = \"f5\" }}")),
    };
    let what = req(t, "do", p)?;
    let rel = matches!(kind, Kind::Knob | Kind::Ring);
    // a dotted name that isn't the core's belongs to an integration
    if let Some((pre, _)) = what.split_once('.').filter(|(p, _)| !crate::ext::CORE_PREFIXES.contains(p)) {
        let Some(i) = crate::ext::by_prefix(pre) else { return Ok(Action::Unavailable) };
        let (allowed, act) = i.parse(what, kind, t, p)?;
        check_fields(t, what, allowed, p)?;
        return Ok(Action::Ext(ExtAct { id: i.id(), act: act.into() }));
    }
    let (allowed, a): (&[&str], Action) = match (what, kind) {
        ("none", _) => (&[], Action::None),
        ("key", Kind::Button) if text(t, "keys", p)?.is_some_and(|k| k.contains(',')) => {
            let combos = text(t, "keys", p)?.unwrap().split(',').map(|c| parse_combo(c.trim()).map_err(|e| format!("{p}: `keys`: {e}")))
                .collect::<Result<Vec<_>, _>>()?;
            if matches!(t.get("hold"), Some(Value::Boolean(true))) { return Err(format!("{p}: hold works with one combination, not a sequence")); }
            (&["keys", "hold"], Action::KeySeq { combos })
        }
        ("key", Kind::Button) => (&["keys", "hold"], Action::Key {
            combo: combo(t, "keys", p)?.ok_or(format!("{p}: `keys` is required"))?,
            hold: matches!(t.get("hold"), Some(Value::Boolean(true))),
        }),
        ("key", _) if rel => {
            let (cw, ccw) = (combo(t, "cw", p)?, combo(t, "ccw", p)?);
            if cw.is_none() && ccw.is_none() { return Err(format!("{p}: `cw` and/or `ccw` required")); }
            (&["cw", "ccw", "per"], Action::KeyStep { cw, ccw, per: pos(num(t, "per", default_per(kind), p)?, "per", p)? })
        }
        ("mouse.move", Kind::Ball) => (&["speed", "accel", "lock"], Action::MouseMove {
            speed: num(t, "speed", 1.0, p)?, accel: num(t, "accel", 1.0, p)?.max(1.0), lock: num(t, "lock", 0.0, p)?,
        }),
        ("mouse.scroll", Kind::Ring | Kind::Knob | Kind::Ball) => {
            let axis = match text(t, "axis", p)?.unwrap_or(if kind == Kind::Ball { "both" } else { "v" }) {
                "v" => Axis::V, "h" => Axis::H, "both" if kind == Kind::Ball => Axis::Both,
                x => return Err(format!("{p}: axis must be \"v\" or \"h\"{}, not {x:?}", if kind == Kind::Ball { " or \"both\"" } else { "" })),
            };
            let mods = mods_of(t, "mods", p)?;
            let h_mods = if kind == Kind::Ball && t.contains_key("h_mods") { mods_of(t, "h_mods", p)? } else { mods.clone() };
            let fields: &[&str] = if kind == Kind::Ball { &["axis", "speed", "mods", "h_mods", "notch"] } else { &["axis", "speed", "mods", "notch"] };
            (fields, Action::MouseScroll { axis, speed: num(t, "speed", 1.0, p)?, mods, h_mods,
                                           notch: matches!(t.get("notch"), Some(Value::Boolean(true))) })
        }
        ("mouse.button", Kind::Button) => {
            let b = match req(t, "button", p)? { "left" => 0, "right" => 1, "middle" => 2,
                x => return Err(format!("{p}: button must be left, right or middle, not {x:?}")) };
            let keys = if text(t, "keys", p)? == Some("") { None } else { combo(t, "keys", p)? };
            (&["button", "keys"], Action::MouseButton { button: b, keys })
        }
        ("layer", Kind::Button) => {
            let mode = match text(t, "mode", p)?.unwrap_or("hold") {
                "hold" => LayerMode::Hold, "toggle" => LayerMode::Toggle, "auto" => LayerMode::Auto,
                x => return Err(format!("{p}: mode must be hold, toggle or auto, not {x:?}")) };
            (&["name", "mode"], Action::Layer { name: req(t, "name", p)?.to_string(), mode })
        }
        ("sheet", Kind::Button) => {
            let hold = match text(t, "mode", p)?.unwrap_or("toggle") { "toggle" => false, "hold" => true,
                x => return Err(format!("{p}: mode must be toggle or hold, not {x:?}")) };
            (&["mode"], Action::Sheet { hold })
        }
        ("restart", Kind::Button) => (&[], Action::Restart),
        ("preset", Kind::Button) => {
            let global = match text(t, "scope", p)?.unwrap_or("app") { "app" => false, "global" => true,
                x => return Err(format!("{p}: scope must be app or global, not {x:?}")) };
            (&["scope", "name"], Action::Preset { global, name: text(t, "name", p)?.filter(|n| !n.trim().is_empty()).unwrap_or("next").to_string() })
        }
        ("mcu.fader", _) if rel => (&["strip", "per_turn", "touch_ms"], Action::McuFader {
            strip: int_in(t, "strip", Some(1), 1, 9, p)? as u8,
            per_turn: num(t, "per_turn", 0.5, p)?,
            touch_ms: int_in(t, "touch_ms", Some(400), 50, 5000, p)? as u64,
        }),
        ("mcu.jog", _) if rel => (&["per_turn"], Action::McuJog { per_turn: num(t, "per_turn", 48.0, p)? }),
        ("mcu.button", Kind::Button) => {
            let name = req(t, "button", p)?;
            let note = if let Some((_, n)) = MCU_BUTTONS.iter().find(|(b, _)| *b == name) { *n }
                else if let Some((_, first)) = MCU_STRIP_BUTTONS.iter().find(|(b, _)| *b == name) {
                    first + int_in(t, "strip", Some(1), 1, 8, p)? as u8 - 1
                } else {
                    return Err(format!("{p}: unknown Mackie button {name:?} (known: {}, {} with strip = 1..8)",
                        MCU_BUTTONS.iter().map(|b| b.0).collect::<Vec<_>>().join(", "), MCU_STRIP_BUTTONS.iter().map(|b| b.0).collect::<Vec<_>>().join(", ")));
                };
            (&["button", "strip"], Action::McuButton { note })
        }
        ("midi.cc", _) if rel => {
            let r = midi_mode(t, p)?;
            (&["cc", "channel", "mode", "per_turn"], Action::MidiCc {
                ch: int_in(t, "channel", Some(1), 1, 16, p)? as u8, cc: int_in(t, "cc", None, 0, 127, p)? as u8, rel: r,
                per_turn: pos(num(t, "per_turn", if r.is_some() { 48.0 } else { 127.0 }, p)?, "per_turn", p)? })
        }
        ("midi.cc", Kind::Button) => (&["cc", "channel", "on", "off", "toggle"], Action::MidiCcKey {
            ch: int_in(t, "channel", Some(1), 1, 16, p)? as u8, cc: int_in(t, "cc", None, 0, 127, p)? as u8,
            on: int_in(t, "on", Some(127), 0, 127, p)? as u8, off: int_in(t, "off", Some(0), 0, 127, p)? as u8,
            toggle: matches!(t.get("toggle"), Some(Value::Boolean(true))) }),
        ("midi.note", Kind::Button) => (&["note", "channel", "velocity"], Action::MidiNote {
            ch: int_in(t, "channel", Some(1), 1, 16, p)? as u8, note: int_in(t, "note", None, 0, 127, p)? as u8,
            vel: int_in(t, "velocity", Some(127), 1, 127, p)? as u8 }),
        ("midi.xy", Kind::Ball) => {
            let x = if t.contains_key("x") { Some(int_in(t, "x", None, 0, 127, p)? as u8) } else { None };
            let y = if t.contains_key("y") { Some(int_in(t, "y", None, 0, 127, p)? as u8) } else { None };
            if x.is_none() && y.is_none() { return Err(format!("{p}: `x` and/or `y` (CC numbers) required")); }
            (&["x", "y", "channel", "mode", "per_unit"], Action::MidiXy {
                ch: int_in(t, "channel", Some(1), 1, 16, p)? as u8, x, y, rel: midi_mode(t, p)?,
                per_unit: pos(num(t, "per_unit", 0.5, p)?, "per_unit", p)? })
        }
        (d, k) if ACTIONS.iter().any(|a| a.0 == d) =>
            return Err(format!("{p}: `{d}` can't be bound to a {} (allowed: {})", k.key(),
                ACTIONS.iter().find(|a| a.0 == d).unwrap().1)),
        (d, _) => return Err(format!("{p}: unknown action `{d}` (known: {})",
            ACTIONS.iter().map(|a| a.0).collect::<Vec<_>>().join(", "))),
    };
    check_fields(t, what, allowed, p)?;
    Ok(a)
}

fn check_fields(t: &Table, what: &str, allowed: &[&str], p: &str) -> Result<(), String> {
    for k in t.keys() {
        if k == "label" {
            match text(t, "label", p)? { Some(l) if l.chars().count() <= 60 => continue, _ => return Err(format!("{p}: `label` must be at most 60 characters")) }
        }
        if k != "do" && !allowed.contains(&k.as_str()) {
            return Err(format!("{p}: unknown field `{k}` for `{what}` (fields: {})",
                if allowed.is_empty() { "none".to_string() } else { allowed.join(", ") }));
        }
    }
    Ok(())
}

/// The usual error of `Integration::parse` for an action it doesn't have, or not for this control.
pub fn not_allowed(what: &str, kind: Kind, actions: &[(&str, &str, &str, &str)], p: &str) -> String {
    match actions.iter().find(|a| a.0 == what) {
        Some(a) => format!("{p}: `{what}` can't be bound to a {} (allowed: {})", kind.key(), a.1),
        None => format!("{p}: unknown action `{what}` (known: {})", actions.iter().map(|a| a.0).collect::<Vec<_>>().join(", ")),
    }
}

pub fn pos(v: f64, k: &str, p: &str) -> Result<f64, String> {
    if v > 0.0 { Ok(v) } else { Err(format!("{p}: `{k}` must be > 0")) }
}
/// Default rotation per step: 1/48 turn.
pub fn default_per(kind: Kind) -> f64 {
    match kind { Kind::Knob => crate::panel::KNOB_PER_TURN / 48.0, _ => crate::panel::RING_PER_TURN / 48.0 }
}

/// (do, controls, fields, help) for validation messages and the editor's reference list.
pub const ACTIONS: [(&str, &str, &str, &str); 15] = [
    ("none", "any", "", "does nothing and stops the fallthrough to less specific contexts"),
    ("key", "button, knob, ring", "keys, hold | cw, ccw, per", "button: taps the combination (\"ctrl+shift+c\"), or several in order (\"shift+3, ctrl+z\"); hold = true mirrors press/release. knob/ring: one tap of cw/ccw per `per` counts"),
    ("mouse.move", "ball", "speed, accel, lock", "moves the pointer; accel > 1 speeds up fast rolls; lock = axis-lock ratio (0 = off, 0.25 typical)"),
    ("mouse.scroll", "ring, knob, ball", "axis, speed, mods, h_mods, notch", "scroll wheel; axis v or h (ball: both); mods = modifier keys held while scrolling, e.g. \"alt\" (ball: h_mods for the horizontal axis); notch = true sends whole notches only, for apps that treat every wheel message as a full notch"),
    ("mouse.button", "button", "button, keys", "left/right/middle; down on press, up on release; keys = a combination held meanwhile (\"space\" + left pans in many programs)"),
    ("layer", "button", "name, mode", "activates a layer while held (mode = hold), until pressed again (mode = toggle), or both (mode = auto: tap = stays on until the next tap, hold while using other controls = only while held)"),
    ("sheet", "button", "mode", "the cheat sheet window: mode toggle (default) shows or hides it at each press, hold shows it while the key is held"),
    ("restart", "button", "", "held for a second: the helper quits and starts again. Works even when the rest of the helper hangs"),
    ("preset", "button", "scope, name", "switches presets: scope app (default, the app in front) or global; name next (default) or a preset's name"),
    ("midi.cc", "knob, ring, button", "cc, channel, mode, per_turn | cc, channel, on, off, toggle", "MIDI control change. knob/ring: mode relative (two's complement, default), offset, signbit or absolute (0..127, per_turn values per turn; default 48 ticks or 127). key: sends `on` (127) at press, `off` (0) at release, or alternates with toggle = true"),
    ("midi.note", "button", "note, channel, velocity", "MIDI note on while the key is held, note off at release"),
    ("mcu.fader", "knob, ring", "strip, per_turn, touch_ms", "Mackie Control fader over MIDI: strip 1-8 (9 = master), per_turn = share of the fader's travel per full turn (default 0.5); touches the fader while turning and lets go touch_ms (400) after the last move"),
    ("mcu.jog", "knob, ring", "per_turn", "Mackie Control jog wheel over MIDI (CC 60, relative), per_turn ticks per full turn (default 48; negative = reversed)"),
    ("mcu.button", "button", "button, strip", "Mackie Control button over MIDI: play, stop, record, rewind, ffwd, cycle, marker, bank_left/right, channel_left/right, read, write, touch, latch, undo, ...; mute, solo, arm, select take strip = 1..8"),
    ("midi.xy", "ball", "x, y, channel, mode, per_unit", "ball x / y as two MIDI CCs (x and y are CC numbers), relative or absolute, per_unit ticks per ball step (default 0.5)"),
];

impl Profiles {
    pub fn parse(text: &str) -> Result<Profiles, String> {
        let root: Table = text.parse::<Table>().map_err(|e| e.to_string())?;
        let mut pr = Profiles::default();
        for (k, v) in &root {
            match k.as_str() {
                "apps" => {
                    let t = v.as_table().ok_or("[apps] must be a table")?;
                    for (name, exes) in t {
                        let arr = exes.as_array().ok_or(format!("apps.{name} must be a list of exe names"))?;
                        for e in arr {
                            let e = e.as_str().ok_or(format!("apps.{name}: exe names must be strings"))?.to_lowercase();
                            pr.apps.insert(if e.ends_with(".exe") { e } else { format!("{e}.exe") }, name.to_lowercase());
                        }
                    }
                }
                "layers" => {
                    let t = v.as_table().ok_or("[layers] must be a table")?;
                    for (name, o) in t {
                        let o = o.as_table().ok_or(format!("layers.{name} must be a table, e.g. {{ inherit = true }}"))?;
                        for (k, x) in o {
                            match (k.as_str(), x) {
                                ("inherit", Value::Boolean(on)) => { if *on { pr.inherit.insert(name.clone()); } }
                                _ => return Err(format!("layers.{name}.{k}: only inherit = true or false")),
                            }
                        }
                    }
                }
                "settings" => {
                    let t = v.as_table().ok_or("[settings] must be a table")?;
                    for (sk, sv) in t {
                        match (sk.as_str(), sv) {
                            ("backlight", Value::Integer(i)) if (0..=100).contains(i) => pr.settings.backlight = *i as u8,
                            ("resolve", Value::String(x)) if x == "yield" || x == "share" => pr.settings.resolve_yield = x == "yield",
                            ("pointer_smoothing_ms", Value::Integer(i)) if (0..=200).contains(i) => pr.settings.pointer_smoothing_ms = *i as u32,
                            ("backlight_off_after_s", Value::Integer(i)) if (0..=86400).contains(i) => pr.settings.backlight_off_after_s = *i as u32,
                            ("start_deg", Value::Integer(i)) if (0..=30).contains(i) => pr.settings.start_deg = *i as f64,
                            ("start_deg", Value::Float(x)) if (0.0..=30.0).contains(x) => pr.settings.start_deg = *x,
                            ("start_ball", Value::Integer(i)) if (0..=100).contains(i) => pr.settings.start_ball = *i as u32,
                            ("sleep_after_min", Value::Integer(i)) if (0..=1440).contains(i) => pr.settings.sleep_after_min = *i as u32,
                            ("press_ms", Value::Integer(i)) if (0..=5000).contains(i) => pr.settings.press_ms = *i as u32,
                            ("restart_hold_s", Value::Integer(i)) if (1..=10).contains(i) => pr.settings.restart_hold_s = *i as u32,
                            ("restart_keys", Value::String(x)) => pr.settings.restart_keys = restart_keys(x).map_err(|e| format!("settings.restart_keys: {e}"))?,
                            ("midi_out", Value::String(x)) => pr.settings.midi_out = x.clone(),
                            ("midi_in", Value::String(x)) => pr.settings.midi_in = x.clone(),
                            ("backlight" | "resolve" | "pointer_smoothing_ms" | "backlight_off_after_s" | "start_deg" | "start_ball" | "press_ms" | "sleep_after_min" | "restart_keys" | "restart_hold_s" | "midi_out" | "midi_in", _) =>
                                return Err(format!("settings.{sk}: bad value (backlight 0..100, resolve \"yield\"/\"share\", pointer_smoothing_ms 0..200, backlight_off_after_s 0..86400, start_deg 0..30, start_ball 0..100, press_ms 0..5000, sleep_after_min 0..1440, restart_keys \"USER+LOOP\", restart_hold_s 1..10, midi_out / midi_in \"auto\" / \"off\" / part of a port name)")),
                            _ => {
                                match crate::ext::all().iter().find_map(|i| i.setting(sk, sv)) {
                                    Some(Ok(())) => {}
                                    Some(Err(e)) => return Err(format!("settings.{sk}: {e}")),
                                    None => pr.warnings.push(format!("settings.{sk}: not a setting of this build")),
                                }
                                pr.settings.ext.insert(sk.clone(), sv.clone());
                            }
                        }
                    }
                }
                _ => {
                    let t = v.as_table().ok_or(format!("`{k}` must be a context table like [{k}.button]"))?;
                    pr.parse_ctx(&k.to_lowercase(), t)?;
                }
            }
        }
        pr.ctxs.entry("default".into()).or_default();
        // programs the integrations know by themselves; the file may say otherwise
        for (i, a) in crate::ext::apps() { for e in &a.exes { pr.apps.entry(e.clone()).or_insert_with(|| i.id().to_string()); } }
        for (c, x) in &pr.ctxs {
            for ((l, k, n), b) in &x.bind {
                if b.action == Action::Unavailable {
                    let d = b.raw.get("do").and_then(|v| v.as_str()).unwrap_or("");
                    pr.warnings.push(format!("{c} {}{}.{n}: `{d}` needs an integration this build doesn't have", if l.is_empty() { String::new() } else { format!("[{l}] ") }, k.key()));
                }
            }
            for (l, cond, _) in &x.leds {
                for p in crate::leds::prefixes(cond) {
                    if crate::ext::by_prefix(&p).is_none() { pr.warnings.push(format!("{c} light {l}: `{p}.` needs an integration this build doesn't have")); }
                }
            }
        }
        pr.warnings.sort();
        Ok(pr)
    }

    fn parse_ctx(&mut self, path: &str, t: &Table) -> Result<(), String> {
        let dotted = path.replace('/', ".");
        let mut ctx = Ctx::default();
        for (k, v) in t {
            if let Some(kind) = Kind::from_key(k) {
                parse_kind(&mut ctx, "", kind, v, &format!("{dotted}.{k}"))?;
            } else if k == "layer" {
                let lt = v.as_table().ok_or(format!("{dotted}.layer must hold tables like [{dotted}.layer.NAME.knob]"))?;
                for (lname, lv) in lt {
                    let lt2 = lv.as_table().ok_or(format!("{dotted}.layer.{lname} must be a table"))?;
                    for (kk, kv) in lt2 {
                        let kind = Kind::from_key(kk).ok_or(format!("{dotted}.layer.{lname}.{kk}: expected button, knob, ring or ball"))?;
                        parse_kind(&mut ctx, lname, kind, kv, &format!("{dotted}.layer.{lname}.{kk}"))?;
                    }
                }
            } else if k == "leds" {
                let led_ok = |l: &str| -> Result<(), String> {
                    if LEDS.iter().any(|(n, _)| *n == l) { Ok(()) } else {
                        Err(format!("{dotted}.leds: no LED {l:?} (LEDs: {})", LEDS.iter().map(|x| x.0).collect::<Vec<_>>().join(" ")))
                    }
                };
                match v {
                    Value::Array(arr) => for l in arr {
                        let l = l.as_str().ok_or(format!("{dotted}.leds: names must be strings"))?;
                        led_ok(l)?;
                        ctx.leds.push((l.to_string(), crate::leds::Cond::Lit(crate::leds::Val::Bool(true)), "true".into()));
                    },
                    Value::Table(tb) => for (l, c) in tb {
                        let l = l.to_uppercase();
                        led_ok(&l)?;
                        let src = match c { Value::String(s) => s.clone(), Value::Boolean(b) => b.to_string(),
                            _ => return Err(format!("{dotted}.leds.{l}: a condition in quotes, e.g. \"layer.fine\"")) };
                        let cond = crate::leds::parse(&src).map_err(|e| format!("{dotted}.leds.{l}: {e}"))?;
                        ctx.leds.push((l, cond, src));
                    },
                    _ => return Err(format!("{dotted}.leds must be a table like [{dotted}.leds] OFFSET = \"layer.fine\"")),
                }
            } else if let Value::Table(sub) = v {
                self.parse_ctx(&format!("{path}/{}", k.to_lowercase()), sub)?;
            } else {
                return Err(format!("{dotted}.{k}: expected a sub-context table or one of {}", RESERVED.join(", ")));
            }
        }
        self.ctxs.insert(path.to_string(), ctx);
        Ok(())
    }

    /// Context name for a foreground exe ("Photoshop.exe" -> "photoshop").
    pub fn app_name(&self, exe: &str) -> String {
        let e = exe.to_lowercase();
        self.apps.get(&e).cloned().unwrap_or_else(|| e.trim_end_matches(".exe").to_string())
    }

    /// Layers first (most recent first), then base; within each, most specific context first.
    /// The open layers first, the latest first; then no layer. A layer that doesn't inherit stops there: an
    /// unset control does nothing in it, except a key that opens or closes layers.
    pub fn lookup<'a>(&'a self, kind: Kind, name: &str, stack: &'a [String], layers: &'a [String]) -> Option<Found<'a>> {
        let at = |layer: Option<&'a str>| stack.iter().find_map(|c| self.ctxs.get(c)
            .and_then(|ctx| ctx.bind.get(&(layer.unwrap_or("").to_string(), kind, name.to_string())))
            .map(|b| Found { ctx: c, layer, binding: b }));
        for l in layers.iter().rev() {
            if let Some(f) = at(Some(l.as_str())) { return Some(f); }
            if !self.inherit.contains(l) { return at(None).filter(|f| f.binding.action.opens_layer()); }
        }
        at(None)
    }

    /// LED rules in force for this context stack: the most specific context's rule per LED wins.
    pub fn led_rules<'a>(&'a self, stack: &[String]) -> Vec<(&'a str, &'a crate::leds::Cond)> {
        let mut out: Vec<(&str, &crate::leds::Cond)> = vec![];
        for c in stack {
            if let Some(ctx) = self.ctxs.get(c) {
                for (l, cond, _) in &ctx.leds {
                    if !out.iter().any(|(x, _)| x == l) { out.push((l.as_str(), cond)); }
                }
            }
        }
        out
    }

    /// What any light condition reads of an integration: "prefix.name" for its prefix, without it.
    pub fn watched(&self, prefix: &str) -> Vec<String> {
        let mut v = vec![];
        for ctx in self.ctxs.values() { for (_, c, _) in &ctx.leds { crate::leds::watched(c, prefix, &mut v); } }
        v.sort();
        v
    }
}

fn parse_kind(ctx: &mut Ctx, layer: &str, kind: Kind, v: &Value, p: &str) -> Result<(), String> {
    let t = v.as_table().ok_or(format!("{p} must be a table of CONTROL = binding"))?;
    let names = kind.names();
    for (name, bv) in t {
        let up = name.to_uppercase();
        if !names.contains(&up.as_str()) {
            return Err(format!("{p}.{name}: no {} named {name:?} (names: {})", kind.key(), names.join(" ")));
        }
        let action = parse_action(kind, bv, &format!("{p}.{name}"))?;
        ctx.bind.insert((layer.to_string(), kind, up), Binding { action, raw: bv.clone() });
    }
    Ok(())
}

/// Inline-TOML text of a binding, for the editor and logs.
pub fn binding_text(b: &Binding) -> String {
    match &b.raw {
        Value::Table(t) => {
            let parts: Vec<String> = t.iter().map(|(k, v)| format!("{k} = {v}")).collect();
            format!("{{ {} }}", parts.join(", "))
        }
        v => v.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init() { crate::ext::testing::init(); }
    fn st(v: &[&str]) -> Vec<String> { v.iter().map(|s| s.to_string()).collect() }

    #[test]
    fn restart_keys_are_checked() {
        assert_eq!(restart_keys("user + loop").unwrap(), ["USER", "LOOP"]);
        assert_eq!(restart_keys("").unwrap(), Vec::<String>::new());
        assert!(restart_keys("AUTO_COLOR+STOP").unwrap_err().contains("never"));
        assert!(restart_keys("STOP+auto color").is_err());
        assert!(restart_keys("USER+USER").is_err() && restart_keys("A+B+C").is_err() && restart_keys("NOPE").is_err());
        let p = Profiles::parse("[settings]\nrestart_keys = \"USER+LOOP\"\nrestart_hold_s = 3\n").unwrap();
        assert_eq!((p.settings.restart_keys.len(), p.settings.restart_hold_s), (2, 3));
        assert!(Profiles::parse("[settings]\nrestart_keys = \"AUTO_COLOR+STOP\"\n").is_err());
        assert!(Profiles::parse("[settings]\nrestart_hold_s = 0\n").is_err());
    }

    #[test]
    fn default_profiles_parse() {
        init();
        let p = Profiles::parse(default_toml()).unwrap();
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
        assert!(p.ctxs.contains_key("photo/develop"), "with the integrations' starters");
        assert_eq!(p.app_name("Photo Pro.exe"), "photo", "the integration knows its program");
        assert_eq!(Profiles::parse("[apps]\nmine = [\"photo pro.exe\"]").unwrap().app_name("photo pro.exe"), "mine", "the file may say otherwise");
    }

    #[test]
    fn integrations_missing_from_this_build() {
        init();
        let p = Profiles::parse("[default.knob]\nHUE = { do = \"zz.param\", whatever = 3 }\n[default.leds]\nUSER = \"zz.flag == pick\"\n[settings]\nzz_thing = true\n").unwrap();
        let stack = st(&["default"]);
        let b = p.lookup(Kind::Knob, "HUE", &stack, &[]).unwrap().binding;
        assert_eq!(b.action, Action::Unavailable);
        assert_eq!(b.raw["whatever"].as_integer(), Some(3), "kept as written");
        assert!(Profiles::parse("[default.button]
STOP = { do = \"key\", keys = \"a\", label = \"Mine\" }
LOOP = { do = \"ph.cmd\", id = \"x\", label = \"Mine\" }").is_ok(), "any binding can have a name");
        assert!(Profiles::parse("[default.button]
STOP = { do = \"key\", keys = \"a\", label = 3 }").is_err());
        assert_eq!(p.warnings.len(), 3, "{:?}", p.warnings);
        assert!(Profiles::parse("[settings]\nphoto_anywhere = 3").unwrap_err().contains("true or false"));
        assert!(Profiles::parse("[default.knob]\nHUE = { do = \"ph.nope\" }").unwrap_err().contains("unknown action"));
        assert!(Profiles::parse("[default.ball]\nLEFT = { do = \"ph.cmd\", id = \"x\" }").unwrap_err().contains("can't be bound to a ball"));
        assert!(Profiles::parse("[default.button]\nSTOP = { do = \"ph.cmd\", id = \"x\", zap = 1 }").unwrap_err().contains("unknown field `zap`"));
    }

    #[test]
    fn lookup_order_layers_and_none() {
        init();
        let p = Profiles::parse(r#"
            [default.ball]
            MIDDLE = { do = "mouse.move" }
            [default.button]
            STOP = { do = "key", keys = "space" }
            PLAY_FWD = { do = "key", keys = "f5" }
            [photo.button]
            STOP = "none"
            USER = { do = "layer", name = "m" }
            [photo.develop.ball]
            MIDDLE = { do = "ph.frames" }
            [photo.develop.layer.m.ball]
            MIDDLE = { do = "mouse.move", speed = 2 }
        "#).unwrap();
        let stack = st(&["photo/develop", "photo", "default"]);
        let f = p.lookup(Kind::Ball, "MIDDLE", &stack, &[]).unwrap();
        assert_eq!(f.ctx, "photo/develop");
        assert!(matches!(f.binding.action, Action::Ext(_)));
        let layers = st(&["m"]);
        let f = p.lookup(Kind::Ball, "MIDDLE", &stack, &layers).unwrap();
        assert_eq!(f.layer, Some("m"));
        assert!(matches!(f.binding.action, Action::MouseMove { speed, .. } if speed == 2.0));
        // a layer that doesn't set a control: it does nothing, unless the layer inherits
        assert!(p.lookup(Kind::Button, "PLAY_FWD", &stack, &layers).is_none());
        // the key that opens the layer still closes it
        assert!(matches!(p.lookup(Kind::Button, "USER", &stack, &layers).unwrap().binding.action, Action::Layer { .. }));
        let mut q = p.clone();
        q.inherit.insert("m".into());
        assert_eq!(q.lookup(Kind::Button, "PLAY_FWD", &stack, &layers).unwrap().ctx, "default");
        assert!(Profiles::parse("[layers]
m = { inherit = true }
").unwrap().inherit.contains("m"));
        assert!(Profiles::parse("[layers]
m = { other = 1 }
").is_err());
        let t = crate::edit::set_layer_inherit("# mine
[default.button]
STOP = \"none\"
", "m", true).unwrap();
        assert!(t.contains("# mine") && Profiles::parse(&t).unwrap().inherit.contains("m"), "{t}");
        let t = crate::edit::set_layer_inherit(&t, "m", false).unwrap();
        assert!(!t.contains("[layers]") && t.contains("# mine"), "{t}");
        assert!(crate::edit::set_layer_inherit("", "Bad Name", true).is_err());
        // explicit none blocks the default
        assert!(matches!(p.lookup(Kind::Button, "STOP", &stack, &[]).unwrap().binding.action, Action::None));
        // another app: default
        let other = st(&["chrome", "default"]);
        assert_eq!(p.lookup(Kind::Ball, "MIDDLE", &other, &[]).unwrap().ctx, "default");
        assert!(p.lookup(Kind::Knob, "HUE", &other, &[]).is_none());
    }

    #[test]
    fn validation_errors_are_specific() {
        for (src, want) in [
            ("[default.knob]\nNOPE = \"none\"", "no knob named"),
            ("[default.ball]\nLEFT = { do = \"key\", keys = \"a\" }", "can't be bound to a ball"),
            ("[default.button]\nSTOP = { do = \"key\", keys = \"ctrl+nope\" }", "nope"),
            ("[default.button]\nSTOP = { do = \"key\", keys = \"a\", hodl = true }", "unknown field `hodl`"),
            ("[default.button]\nSTOP = { do = \"teleport\" }", "unknown action"),
            ("[default]\nleds = [\"STOP\"]", "no LED"),
            ("[settings]\nbacklight = 300", "settings.backlight"),
            ("[default.knob\n", "TOML"),
        ] {
            let e = Profiles::parse(src).err().unwrap_or_default();
            assert!(e.contains(want) || (want == "TOML" && !e.is_empty()), "{src:?} -> {e:?}");
        }
    }
}
