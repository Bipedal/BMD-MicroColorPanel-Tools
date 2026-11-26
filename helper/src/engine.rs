//! Panel events -> bindings -> actions (PROFILES.md).

use crate::keys::Combo;
use crate::panel::{led_bit, AxisLock, Event, KNOB_PER_TURN, RING_PER_TURN};
use crate::profiles::{LayerMode, binding_text, Action, Axis, Kind, Profiles, ExtAct};
use std::collections::HashMap;

/// Where actions go. Real: key/mouse/MIDI output and the integrations; tests: a recorder.
pub trait Out {
    fn key(&mut self, c: &Combo, down: bool, up: bool);
    fn mouse_move(&mut self, dx: i32, dy: i32);
    /// wheel units (120 = one notch); +v = away from the user (up), +h = right
    /// `mods`: modifier keys held around the wheel message (empty = none)
    fn scroll(&mut self, v: i32, h: i32, mods: &[u16]);
    fn mouse_button(&mut self, button: u8, down: bool);
    /// give the program in front a moment to notice a key before a click
    fn pause(&mut self) {}
    /// one message to an integration; false if nobody is there
    fn ext(&mut self, id: &str, msg: serde_json::Value) -> bool;
    /// one MIDI channel message; false if no MIDI out port is open
    fn midi(&mut self, m: [u8; 3]) -> bool;
    /// the cheat sheet: Some = show / hide, None = the other way round
    fn sheet(&mut self, _on: Option<bool>) {}
    /// switch presets: the app in front, everything or just it, "next" or a name
    fn preset(&mut self, _app: &str, _global: bool, _name: &str) {}
}

/// Mackie fader position used before the host has reported one (about 0 dB on most hosts) [?]
const MCU_UNITY: f64 = 12_000.0;
fn mcu_touch_note(strip: u8) -> u8 { 0x68 + strip - 1 } // 0x68..0x6F strips, 0x70 master

/// auto layer keys: a press shorter than this (with nothing else touched) latches the layer
/// a control that rested this long has to pass its start threshold again
const START_IDLE: std::time::Duration = std::time::Duration::from_millis(1500);
/// around a press, a knob has to turn this far (degrees) before it counts
const PRESS_DEG: f64 = 20.0;
const AUTO_TAP: std::time::Duration = std::time::Duration::from_millis(400);
/// mouse.move: pixels per ball step at speed 1, and the speed (steps/report) where accel is full.
pub const PX_PER_STEP: f64 = 2.0;
pub const ACCEL_FULL_AT: f64 = 20.0;

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Learn {
    pub control: String,
    pub at: f64,
    pub ctx: Option<String>,
    pub layer: Option<String>,
    pub binding: Option<String>,
}

struct ExtIo<'a> { id: &'static str, key: &'a str, acc: &'a mut HashMap<String, f64>, missed: u64, out: &'a mut dyn Out }
impl crate::ext::Io for ExtIo<'_> {
    fn send(&mut self, msg: serde_json::Value) -> bool {
        let ok = self.out.ext(self.id, msg);
        if !ok { self.missed += 1; }
        ok
    }
    fn carry(&mut self, slot: &str, v: f64) -> i32 {
        carry_in(self.acc, &if slot.is_empty() { self.key.to_string() } else { format!("{}.{slot}", self.key) }, v)
    }
    fn steps(&mut self, counts: f64, per: f64) -> Vec<bool> { steps_in(self.acc, self.key, counts, per) }
}

/// Discrete steps from rotation: true = cw. The remainder carries over.
fn steps_in(acc: &mut HashMap<String, f64>, key: &str, delta: f64, per: f64) -> Vec<bool> {
    let acc = acc.entry(key.to_string()).or_insert(0.0);
    *acc += delta;
    let mut v = vec![];
    while *acc >= per { *acc -= per; v.push(true); }
    while *acc <= -per { *acc += per; v.push(false); }
    v
}

/// Integer part out, fraction carried (so slow movement isn't lost).
fn carry_in(acc: &mut HashMap<String, f64>, key: &str, v: f64) -> i32 {
    let acc = acc.entry(key.to_string()).or_insert(0.0);
    *acc += v;
    let i = (*acc + 1e-9 * acc.signum()).trunc();
    *acc -= i;
    i as i32
}

pub struct Engine {
    pub profiles: Profiles,
    pub stack: Vec<String>,
    pub layers: Vec<String>,
    /// layer name -> button that activated it (for its LED)
    layer_src: HashMap<String, String>,
    /// set by the app: the cheat sheet is open
    pub sheet_open: bool,
    /// button name -> action that got its press (the release goes there)
    pressed: HashMap<String, Action>,
    /// auto-mode layer keys held now: button -> (layer, pressed at, another control used meanwhile,
    /// this press turned a latched layer off)
    auto_held: HashMap<String, (String, std::time::Instant, bool, bool)>,
    acc: HashMap<String, f64>,
    locks: HashMap<String, AxisLock>,
    pub last: Option<Learn>,
    /// integration actions that weren't allowed or found nobody: id -> count
    pub skipped: HashMap<&'static str, u64>,
    /// start thresholds: control -> movement so far
    start: HashMap<String, Start>,
    /// knobs pressed right now, and when they were let go
    knob_down: std::collections::HashSet<String>,
    knob_up: HashMap<String, std::time::Instant>,
    #[cfg(test)]
    now: Option<std::time::Instant>,
    /// Mackie faders being touched: strip -> last move
    mcu_touch: HashMap<u8, (std::time::Instant, u64)>,
    /// MIDI messages that found no open out port
    pub midi_dropped: u64,
    /// MIDI CC toggles: "ch.cc" -> currently on
    midi_toggles: HashMap<String, bool>,
}

fn now_s() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// Movement of a control since it last rested.
struct Start { x: f64, y: f64, last: std::time::Instant, open: bool, settling: bool }

impl Engine {
    fn clock(&self) -> std::time::Instant {
        #[cfg(test)]
        if let Some(t) = self.now { return t; }
        std::time::Instant::now()
    }

    /// The part of a movement that counts: nothing until the control has really been moved.
    fn started(&mut self, kind: Kind, name: &str, dx: f64, dy: f64) -> Option<(f64, f64)> {
        let s = &self.profiles.settings;
        let mut need = match kind {
            Kind::Knob => s.start_deg / 360.0 * KNOB_PER_TURN,
            Kind::Ring => s.start_deg / 360.0 * RING_PER_TURN,
            _ => s.start_ball as f64,
        };
        let press_ms = s.press_ms as u128;
        // the pointer has to follow every movement
        if kind == Kind::Ball && matches!(self.profiles.lookup(kind, name, &self.stack, &self.layers).map(|f| &f.binding.action), Some(Action::MouseMove { .. })) { need = 0.0; }
        let now = self.clock();
        let settling = kind == Kind::Knob && press_ms > 0
            && (self.knob_down.contains(name) || self.knob_up.get(name).is_some_and(|t| now.duration_since(*t).as_millis() < press_ms));
        if settling { need = need.max(PRESS_DEG / 360.0 * KNOB_PER_TURN); }
        if need <= 0.0 { return Some((dx, dy)); }
        let g = self.start.entry(format!("{}.{name}", kind.key())).or_insert(Start { x: 0.0, y: 0.0, last: now, open: false, settling });
        if now.duration_since(g.last) > START_IDLE || g.settling != settling && !(g.open && !settling) {
            (g.x, g.y, g.open) = (0.0, 0.0, false);
        }
        g.last = now;
        g.settling = settling;
        if g.open { return Some((dx, dy)); }
        g.x += dx;
        g.y += dy;
        let m = g.x.hypot(g.y);
        if m < need { return None; }
        g.open = true;
        let k = (m - need) / m;
        Some((g.x * k, g.y * k))
    }

    pub fn new(profiles: Profiles) -> Self {
        Engine { profiles, stack: vec!["default".into()], layers: vec![], layer_src: HashMap::new(), sheet_open: false,
                 pressed: HashMap::new(), auto_held: HashMap::new(), acc: HashMap::new(), locks: HashMap::new(),
                 last: None, skipped: HashMap::new(), start: HashMap::new(), knob_down: Default::default(), knob_up: HashMap::new(),
                 #[cfg(test)]
                 now: None, mcu_touch: HashMap::new(), midi_dropped: 0, midi_toggles: HashMap::new() }
    }

    pub fn set_profiles(&mut self, p: Profiles) {
        self.profiles = p;
        self.acc.clear();
    }

    fn resolve(&mut self, kind: Kind, name: &str) -> Option<Action> {
        let f = self.profiles.lookup(kind, name, &self.stack, &self.layers);
        self.last = Some(Learn {
            control: format!("{}.{name}", kind.key()), at: now_s(),
            ctx: f.as_ref().map(|f| f.ctx.to_string()),
            layer: f.as_ref().and_then(|f| f.layer.map(String::from)),
            binding: f.as_ref().map(|f| binding_text(f.binding)),
        });
        let a = f.map(|f| f.binding.action.clone())?;
        if let Action::Ext(x) = &a {
            if !x.act.allowed(&self.stack, &self.profiles.settings) {
                *self.skipped.entry(x.id).or_default() += 1;
                return None;
            }
        }
        Some(a)
    }

    pub fn handle(&mut self, ev: &Event, out: &mut dyn Out) {
        let moved = match ev {
            Event::Button { name, down, .. } if crate::panel::KNOBS.contains(&name.as_str()) => {
                if *down { self.knob_down.insert(name.clone()); }
                else { self.knob_down.remove(name); let t = self.clock(); self.knob_up.insert(name.clone(), t); }
                None
            }
            Event::Knob { name, delta, .. } => Some(self.started(Kind::Knob, name, *delta as f64, 0.0)),
            Event::Ring { name, delta, .. } => Some(self.started(Kind::Ring, name, *delta as f64, 0.0)),
            Event::Ball { name, dx, dy, .. } => Some(self.started(Kind::Ball, name, *dx as f64, *dy as f64)),
            _ => None,
        };
        let (mx, my) = match moved { Some(None) => return, Some(Some(m)) => m, None => (0.0, 0.0) };
        // anything else touched while an auto layer key is down makes that press momentary
        let other = match ev {
            Event::Button { name, down: true, .. } => Some(name.as_str()),
            Event::Knob { .. } | Event::Ring { .. } | Event::Ball { .. } => Some(""),
            _ => None,
        };
        if let Some(o) = other {
            for (b, h) in self.auto_held.iter_mut() { if b != o { h.2 = true; } }
        }
        match ev {
            Event::Button { name, down: true, .. } => {
                let Some(a) = self.resolve(Kind::Button, name) else { return; };
                self.press(name, &a, out);
                self.pressed.insert(name.clone(), a);
            }
            Event::Button { name, down: false, .. } => {
                if let Some(a) = self.pressed.remove(name) { self.release(name, &a, out); }
            }
            Event::Knob { name, .. } => self.rotate(Kind::Knob, name, mx, KNOB_PER_TURN, out),
            Event::Ring { name, .. } => self.rotate(Kind::Ring, name, mx, RING_PER_TURN, out),
            Event::Ball { name, .. } => self.ball(name, mx.round() as i32, my.round() as i32, out),
            Event::Link { up: false, .. } => {}
            _ => {}
        }
    }

    fn midi(&mut self, out: &mut dyn Out, m: [u8; 3]) {
        if !out.midi(m) { self.midi_dropped += 1; }
    }

    /// Absolute CC: continue from the last value sent or received on that CC.
    fn midi_abs(&mut self, out: &mut dyn Out, ch: u8, cc: u8, delta: f64, key: &str) {
        let acc = self.acc.entry(format!("{key}.abs")).or_insert(f64::NAN);
        let known = crate::midi::value(&format!("{ch}.cc.{cc}"));
        if acc.is_nan() || known.map_or(false, |k| (k - acc.round()).abs() >= 1.0) { *acc = known.unwrap_or(0.0); }
        let old = acc.round();
        *acc = (*acc + delta).clamp(0.0, 127.0);
        let new = acc.round();
        if new != old { self.midi(out, [0xB0 | (ch - 1), cc, new as u8]); }
    }

    fn midi_rel(&mut self, out: &mut dyn Out, ch: u8, cc: u8, enc: crate::midi::RelEnc, ticks: f64, key: &str) {
        let mut n = self.carry(key, ticks);
        while n != 0 {
            let s = n.clamp(-63, 63);
            self.midi(out, [0xB0 | (ch - 1), cc, crate::midi::rel_byte(enc, s)]);
            n -= s;
        }
    }

    /// Let an integration's action handle something.
    fn ext(&mut self, key: &str, x: &ExtAct, out: &mut dyn Out, f: impl FnOnce(&dyn crate::ext::Act, &mut dyn crate::ext::Io)) {
        let mut io = ExtIo { id: x.id, key, acc: &mut self.acc, missed: 0, out };
        f(x.act.as_ref(), &mut io);
        let n = io.missed;
        if n > 0 { *self.skipped.entry(x.id).or_default() += n; }
    }

    /// Timed work: let go of Mackie faders that stopped moving. Call every few ms.
    pub fn tick(&mut self, out: &mut dyn Out) {
        if self.mcu_touch.is_empty() { return; }
        let done: Vec<u8> = self.mcu_touch.iter()
            .filter(|(_, (t, ms))| t.elapsed().as_millis() as u64 >= *ms).map(|(s, _)| *s).collect();
        for s in done {
            self.mcu_touch.remove(&s);
            self.midi(out, [0x90, mcu_touch_note(s), 0]);
        }
    }

    fn press(&mut self, name: &str, a: &Action, out: &mut dyn Out) {
        match a {
            Action::McuButton { note } => self.midi(out, [0x90, *note, 127]),
            Action::Key { combo, hold } => out.key(combo, true, !hold),
            Action::KeySeq { combos } => for c in combos { out.key(c, true, true); },
            Action::MouseButton { button, keys } => {
                if let Some(k) = keys { out.key(k, true, false); out.pause(); }
                out.mouse_button(*button, true);
            }
            Action::Preset { global, name } => {
                let app = self.stack.first().map(|c| c.split('/').next().unwrap_or(c)).unwrap_or("default");
                out.preset(app, *global, name);
            }
            Action::Sheet { hold } => out.sheet(if *hold { Some(true) } else { None }),
            Action::Layer { name: l, mode } => {
                let on = self.layers.contains(l);
                match mode {
                    LayerMode::Toggle | LayerMode::Auto if on => {
                        self.layers.retain(|x| x != l);
                        self.layer_src.remove(l);
                    }
                    _ if !on => {
                        self.layers.push(l.clone());
                        self.layer_src.insert(l.clone(), name.to_string());
                    }
                    _ => {}
                }
                if *mode == LayerMode::Auto {
                    self.auto_held.insert(name.to_string(), (l.clone(), std::time::Instant::now(), false, on));
                }
            }
            Action::Ext(x) => {
                if let Some(l) = x.act.layer().map(String::from).filter(|l| !self.layers.contains(l)) {
                    self.layers.push(l.clone());
                    self.layer_src.insert(l, name.to_string());
                }
                self.ext(&format!("button.{name}"), x, out, |a, io| a.press(io));
            }
            Action::MidiNote { ch, note, vel } => self.midi(out, [0x90 | (ch - 1), *note, *vel]),
            Action::MidiCcKey { ch, cc, on, off, toggle } => {
                let v = if *toggle {
                    let k = format!("{ch}.{cc}");
                    let st = !self.midi_toggles.get(&k).copied().unwrap_or(false);
                    self.midi_toggles.insert(k, st);
                    if st { *on } else { *off }
                } else { *on };
                self.midi(out, [0xB0 | (ch - 1), *cc, v]);
            }
            _ => {}
        }
    }

    fn release(&mut self, name: &str, a: &Action, out: &mut dyn Out) {
        match a {
            Action::McuButton { note } => self.midi(out, [0x90, *note, 0]),
            Action::Ext(x) => {
                if let Some(l) = x.act.layer() { self.layers.retain(|y| y != l); self.layer_src.remove(l); }
                self.ext(&format!("button.{name}"), x, out, |a, io| a.release(io));
            }
            Action::Key { combo, hold: true } => out.key(combo, false, true),
            Action::MouseButton { button, keys } => {
                out.mouse_button(*button, false);
                if let Some(k) = keys { out.pause(); out.key(k, false, true); }
            }
            Action::Sheet { hold: true } => out.sheet(Some(false)),
            Action::MidiNote { ch, note, .. } => self.midi(out, [0x80 | (ch - 1), *note, 0]),
            Action::MidiCcKey { ch, cc, off, toggle: false, .. } => self.midi(out, [0xB0 | (ch - 1), *cc, *off]),
            Action::Layer { name: l, mode: LayerMode::Hold } => {
                self.layers.retain(|x| x != l);
                self.layer_src.remove(l);
            }
            Action::Layer { name: l, mode: LayerMode::Auto } => {
                if let Some((_, t, used, was_off_press)) = self.auto_held.remove(name) {
                    // a tap latches; a long hold or a hold with other controls used is momentary
                    if !was_off_press && (used || t.elapsed() >= AUTO_TAP) {
                        self.layers.retain(|x| x != l);
                        self.layer_src.remove(l);
                    }
                }
            }
            _ => {}
        }
    }

    fn rotate(&mut self, kind: Kind, name: &str, delta: f64, per_turn_counts: f64, out: &mut dyn Out) {
        let Some(a) = self.resolve(kind, name) else { return; };
        let key = format!("{}.{name}", kind.key());
        match a {
            Action::Ext(x) => self.ext(&key, &x, out, |a, io| a.turn(delta, per_turn_counts, io)),
            Action::KeyStep { cw, ccw, per } => {
                for cw_step in self.steps(&key, delta, per) {
                    if let Some(c) = if cw_step { &cw } else { &ccw } { out.key(c, true, true); }
                }
            }
            Action::MidiCc { ch, cc, rel, per_turn } => {
                let units = delta / per_turn_counts * per_turn;
                match rel {
                    Some(enc) => self.midi_rel(out, ch, cc, enc, units, &key),
                    None => self.midi_abs(out, ch, cc, units, &key),
                }
            }
            Action::McuJog { per_turn } => {
                let mut n = self.carry(&key, delta / per_turn_counts * per_turn);
                while n != 0 {
                    let s = n.clamp(-63, 63);
                    self.midi(out, [0xB0, 0x3C, if s > 0 { s as u8 } else { 0x40 | (-s) as u8 }]);
                    n -= s;
                }
            }
            Action::McuFader { strip, per_turn, touch_ms } => {
                let ch = strip - 1;
                let acc = self.acc.entry(format!("{key}.mcu")).or_insert(f64::NAN);
                // follow the host's own fader position (it moves with automation / clip changes),
                // except while we're touching it
                let known = crate::midi::value(&format!("{strip}.pb"));
                if acc.is_nan() || (!self.mcu_touch.contains_key(&strip) && known.map_or(false, |k| (k - *acc).abs() >= 1.0)) {
                    *acc = known.unwrap_or(MCU_UNITY);
                }
                let old = acc.round();
                *acc = (*acc + delta / per_turn_counts * per_turn * 16383.0).clamp(0.0, 16383.0);
                let new = acc.round() as u16;
                if !self.mcu_touch.contains_key(&strip) { self.midi(out, [0x90, mcu_touch_note(strip), 127]); }
                self.mcu_touch.insert(strip, (std::time::Instant::now(), touch_ms));
                if new as f64 != old { self.midi(out, [0xE0 | ch, (new & 0x7F) as u8, (new >> 7) as u8]); }
            }
            Action::MouseScroll { axis, speed, mods, notch, .. } => {
                // one notch (120) per 1/24 turn at speed 1
                let units = self.wheel_units(&key, delta / (per_turn_counts / 24.0) * 120.0 * speed, notch);
                if units != 0 {
                    match axis { Axis::H => out.scroll(0, units, &mods), _ => out.scroll(units, 0, &mods) }
                }
            }
            _ => {}
        }
    }

    fn steps(&mut self, key: &str, delta: f64, per: f64) -> Vec<bool> { steps_in(&mut self.acc, key, delta, per) }

    /// Integer part out, fraction carried (so slow movement isn't lost).
    /// Wheel units with the remainder carried over; `notch` = whole notches (multiples of 120) only.
    fn wheel_units(&mut self, key: &str, units: f64, notch: bool) -> i32 {
        if notch { self.carry(key, units / 120.0) * 120 } else { self.carry(key, units) }
    }

    fn carry(&mut self, key: &str, v: f64) -> i32 { carry_in(&mut self.acc, key, v) }

    fn ball(&mut self, name: &str, dx: i32, dy: i32, out: &mut dyn Out) {
        let Some(a) = self.resolve(Kind::Ball, name) else { return; };
        let key = format!("ball.{name}");
        match a {
            Action::MouseMove { speed, accel, lock } => {
                let (dx, dy) = if lock > 0.0 {
                    self.locks.entry(key.clone()).or_insert_with(|| AxisLock::new(lock)).apply(dx, dy)
                } else { (dx, dy) };
                let v = ((dx * dx + dy * dy) as f64).sqrt();
                let gain = PX_PER_STEP * speed * (1.0 + (accel - 1.0) * (v / ACCEL_FULL_AT).min(1.0));
                let px = self.carry(&format!("{key}.x"), dx as f64 * gain);
                let py = self.carry(&format!("{key}.y"), -(dy as f64) * gain); // +y up -> screen down is +
                if px != 0 || py != 0 { out.mouse_move(px, py); }
            }
            Action::MouseScroll { axis, speed, mods, h_mods, notch } => {
                let v = self.wheel_units(&format!("{key}.v"), dy as f64 * 3.0 * speed, notch);
                let h = self.wheel_units(&format!("{key}.h"), dx as f64 * 3.0 * speed, notch);
                let (v, h) = match axis { Axis::V => (v, 0), Axis::H => (0, h), Axis::Both => (v, h) };
                if mods == h_mods {
                    if v != 0 || h != 0 { out.scroll(v, h, &mods); }
                } else {
                    if v != 0 { out.scroll(v, 0, &mods); }
                    if h != 0 { out.scroll(0, h, &h_mods); }
                }
            }
            Action::Ext(x) => self.ext(&key, &x, out, |a, io| a.ball(dx as f64, dy as f64, io)),
            Action::MidiXy { ch, x, y, rel, per_unit } => {
                for (cc, d, ax) in [(x, dx, "x"), (y, dy, "y")] {
                    let Some(cc) = cc else { continue };
                    if d == 0 { continue; }
                    let k = format!("{key}.{ax}");
                    match rel {
                        Some(enc) => self.midi_rel(out, ch, cc, enc, d as f64 * per_unit, &k),
                        None => self.midi_abs(out, ch, cc, d as f64 * per_unit, &k),
                    }
                }
            }
            _ => {}
        }
    }

    /// LED mask: active layers' buttons, then the context's LED rules (most specific wins; a
    /// rule can also switch off a layer key's LED).
    /// Keys that restart the helper here, now (the panel's thread acts on them).
    pub fn restart_keys(&self) -> Vec<String> {
        Kind::Button.names().into_iter().filter(|n| matches!(self.profiles.lookup(Kind::Button, n, &self.stack, &self.layers).map(|f| &f.binding.action), Some(Action::Restart)))
            .map(String::from).collect()
    }

    pub fn leds(&self) -> u64 {
        let mut m = 0u64;
        for l in &self.layers {
            if let Some(b) = self.layer_src.get(l).and_then(|b| led_bit(b)) { m |= 1 << b; }
        }
        // a key that shows the cheat sheet lights while it is open
        if self.sheet_open {
            for (n, b) in crate::panel::LEDS {
                if matches!(self.profiles.lookup(Kind::Button, n, &self.stack, &self.layers).map(|f| &f.binding.action), Some(Action::Sheet { .. })) { m |= 1 << b; }
            }
        }
        let facts = crate::leds::facts(&self.layers, self.sheet_open);
        for (name, cond) in self.profiles.led_rules(&self.stack) {
            if let Some(b) = led_bit(name) {
                if crate::leds::eval(cond, &facts) { m |= 1 << b; } else { m &= !(1 << b); }
            }
        }
        m
    }

    /// Release everything held (pause, profile reload, quit).
    pub fn release_all(&mut self, out: &mut dyn Out) {
        let held: Vec<(String, Action)> = self.pressed.drain().collect();
        for (n, a) in held { self.release(&n, &a, out); }
        self.layers.clear();
        self.layer_src.clear();
        self.auto_held.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Rec { log: Vec<String>, up: bool }
    impl Out for Rec {
        fn key(&mut self, c: &Combo, d: bool, u: bool) { self.log.push(format!("key {:?}/{} {d} {u}", c.mods, c.key)); }
        fn mouse_move(&mut self, dx: i32, dy: i32) { self.log.push(format!("move {dx} {dy}")); }
        fn scroll(&mut self, v: i32, h: i32, m: &[u16]) {
            if m.is_empty() { self.log.push(format!("scroll {v} {h}")); } else { self.log.push(format!("scroll {v} {h} mods {m:?}")); }
        }
        fn mouse_button(&mut self, b: u8, d: bool) { self.log.push(format!("mb {b} {d}")); }
        fn ext(&mut self, id: &str, m: serde_json::Value) -> bool { self.log.push(format!("{id} {m}")); self.up }
        fn midi(&mut self, m: [u8; 3]) -> bool { self.log.push(format!("midi {:02x} {} {}", m[0], m[1], m[2])); true }
        fn sheet(&mut self, on: Option<bool>) { self.log.push(format!("sheet {on:?}")); }
        fn preset(&mut self, app: &str, g: bool, n: &str) { self.log.push(format!("preset {app} {g} {n}")); }
    }
    fn btn(n: &str, down: bool) -> Event { Event::Button { name: n.into(), bit: 0, down, synthetic: false } }
    fn eng(stack: &[&str]) -> Engine {
        crate::ext::testing::init();
        let mut e = Engine::new(Profiles::parse(crate::profiles::default_toml()).unwrap());
        e.stack = stack.iter().map(|s| s.to_string()).collect();
        e
    }

    #[test]
    fn preset_keys() {
        let p = Profiles::parse("[default.button]\nUSER = { do = \"preset\" }\nLOOP = { do = \"preset\", scope = \"global\", name = \"Travel\" }\n").unwrap();
        let mut e = Engine::new(p);
        e.stack = vec!["photo/develop".into(), "photo".into(), "default".into()];
        let mut r = Rec::default();
        for (n, d) in [("USER", true), ("USER", false), ("LOOP", true), ("LOOP", false)] { e.handle(&btn(n, d), &mut r); }
        assert_eq!(r.log, ["preset photo false next", "preset photo true Travel"]);
        assert!(Profiles::parse("[default.button]\nUSER = { do = \"preset\", scope = \"x\" }\n").is_err());
    }

    #[test]
    fn cheat_sheet_key() {
        let p = Profiles::parse("[default.button]\nUSER = { do = \"sheet\" }\nLOOP = { do = \"sheet\", mode = \"hold\" }\n").unwrap();
        let mut e = Engine::new(p);
        e.stack = vec!["default".into()];
        let mut r = Rec::default();
        for (n, d) in [("USER", true), ("USER", false), ("LOOP", true)] { e.handle(&btn(n, d), &mut r); }
        e.stack = vec!["other".into()];   // the release still belongs to the press
        e.handle(&btn("LOOP", false), &mut r);
        assert_eq!(r.log, ["sheet None", "sheet Some(true)", "sheet Some(false)"]);
        let user = 1u64 << crate::panel::LEDS.iter().find(|x| x.0 == "USER").unwrap().1;
        assert_eq!(e.leds() & user, 0);
        e.sheet_open = true;
        e.stack = vec!["default".into()];
        assert_eq!(e.leds() & user, user);
        assert!(Profiles::parse("[default.button]\nUSER = { do = \"sheet\", mode = \"x\" }\n").is_err());
        assert!(Profiles::parse("[default.knob]\nHUE = { do = \"sheet\" }\n").is_err());
    }

    #[test]
    fn mackie_fader_touches_and_lets_go() {
        let p = Profiles::parse("[default.ring]\nLEFT = { do = \"mcu.fader\", strip = 2, per_turn = 0.5, touch_ms = 50 }\n[default.button]\nPLAY_FWD = { do = \"mcu.button\", button = \"play\" }\nUSER = { do = \"mcu.button\", button = \"mute\", strip = 3 }\n").unwrap();
        let mut e = Engine::new(p);
        e.stack = vec!["default".into()];
        let mut r = Rec::default();
        let turn = crate::panel::RING_PER_TURN;
        e.rotate(Kind::Ring, "LEFT", turn / 10.0, turn, &mut r); // +5% of travel from the default position
        assert_eq!(r.log[0], "midi 90 105 127", "touch strip 2");
        let v = (MCU_UNITY + 0.05 * 16383.0).round() as u32;
        assert_eq!(r.log[1], format!("midi e1 {} {}", v & 0x7F, v >> 7));
        e.rotate(Kind::Ring, "LEFT", turn / 10.0, turn, &mut r);
        assert_eq!(r.log.len(), 3, "no second touch: {:?}", r.log);
        e.tick(&mut r);
        assert_eq!(r.log.len(), 3, "still touched right after a move");
        std::thread::sleep(std::time::Duration::from_millis(70));
        e.tick(&mut r);
        assert_eq!(r.log.last().unwrap(), "midi 90 105 0", "released after touch_ms");
        r.log.clear();
        e.handle(&btn("PLAY_FWD", true), &mut r);
        e.handle(&btn("PLAY_FWD", false), &mut r);
        e.handle(&btn("USER", true), &mut r);
        assert_eq!(r.log, vec!["midi 90 94 127", "midi 90 94 0", "midi 90 18 127"]);
    }

    #[test]
    fn key_sequence() {
        let p = Profiles::parse("[default.button]
UNDO = { do = \"key\", keys = \"shift+3, ctrl+z\" }
").unwrap();
        let mut e = Engine::new(p);
        e.stack = vec!["default".into()];
        let mut r = Rec::default();
        e.handle(&btn("UNDO", true), &mut r);
        assert_eq!(r.log.len(), 2, "{:?}", r.log);
        assert!(r.log[0].contains("[16]") && r.log[1].contains("[17]"), "{:?}", r.log);
    }

    #[test]
    fn scroll_with_mods_and_notches() {
        let p = Profiles::parse("[default.ring]\nRIGHT = { do = \"mouse.scroll\", mods = \"alt\", notch = true }\n[default.ball]\nRIGHT = { do = \"mouse.scroll\", h_mods = \"ctrl\" }\n").unwrap();
        let mut e = Engine::new(p);
        e.stack = vec!["default".into()];
        let mut r = Rec::default();
        // 1/48 turn = half a notch: nothing yet; the second half sends one whole notch with alt
        let half = crate::panel::RING_PER_TURN / 48.0;
        e.rotate(Kind::Ring, "RIGHT", half, crate::panel::RING_PER_TURN, &mut r);
        assert!(r.log.is_empty(), "{:?}", r.log);
        e.rotate(Kind::Ring, "RIGHT", half, crate::panel::RING_PER_TURN, &mut r);
        assert_eq!(r.log, vec!["scroll 120 0 mods [18]"]);
        r.log.clear();
        // ball: vertical plain, horizontal with ctrl, sent separately
        e.ball("RIGHT", 10, 10, &mut r);
        assert_eq!(r.log, vec!["scroll 30 0", "scroll 0 30 mods [17]"]);
        assert!(Profiles::parse("[default.ring]\nRIGHT = { do = \"mouse.scroll\", mods = \"ctrl+w\" }\n").is_err());
    }

    #[test]
    fn auto_layer_tap_latches_hold_is_momentary() {
        let p = Profiles::parse("[photo.button]\nSHIFT_UP = { do = \"layer\", name = \"more\", mode = \"auto\" }\n[photo.layer.more.knob]\nY_LIFT = { do = \"ph.param\", id = \"Dehaze\", per_turn = 100 }\n").unwrap();
        let mut e = Engine::new(p);
        e.stack = vec!["photo".into(), "default".into()];
        let mut r = Rec { up: true, ..Default::default() };
        let su = |e: &Engine| e.leds() & (1 << crate::panel::led_bit("SHIFT_UP").unwrap()) != 0;
        // tap: stays on, LED on
        e.handle(&btn("SHIFT_UP", true), &mut r);
        e.handle(&btn("SHIFT_UP", false), &mut r);
        assert!(e.layers.contains(&"more".to_string()) && su(&e), "tap latches, LED on");
        // tap again: off
        e.handle(&btn("SHIFT_UP", true), &mut r);
        e.handle(&btn("SHIFT_UP", false), &mut r);
        assert!(e.layers.is_empty() && !su(&e), "second tap turns it off");
        // hold + turn a knob: on while held, off after
        e.handle(&btn("SHIFT_UP", true), &mut r);
        e.handle(&Event::Knob { name: "Y_LIFT", index: 0, delta: 100, raw: 0 }, &mut r);
        assert!(r.log.last().unwrap().contains("Dehaze"), "{:?}", r.log);
        assert!(su(&e), "LED on while held");
        e.handle(&btn("SHIFT_UP", false), &mut r);
        assert!(e.layers.is_empty(), "momentary after using another control");
    }

    #[test]
    fn midi_actions() {
        let p = Profiles::parse(concat!(
            "[default.knob]\nY_LIFT = { do = \"midi.cc\", cc = 20 }\nY_GAMMA = { do = \"midi.cc\", cc = 21, mode = \"absolute\", channel = 2 }\n",
            "[default.button]\nUSER = { do = \"midi.note\", note = 60 }\nLOOP = { do = \"midi.cc\", cc = 64, toggle = true }\n",
            "[default.ball]\nLEFT = { do = \"midi.xy\", x = 30, y = 31, mode = \"offset\", per_unit = 1 }\n")).unwrap();
        let mut e = Engine::new(p);
        let mut r = Rec::default();
        let quarter = KNOB_PER_TURN / 4.0;
        e.handle(&Event::Knob { name: "Y_LIFT", index: 0, delta: quarter as i32, raw: 0 }, &mut r);
        assert_eq!(r.log, vec!["midi b0 20 12"], "48 ticks/turn: a quarter turn = +12");
        r.log.clear();
        e.handle(&Event::Knob { name: "Y_LIFT", index: 0, delta: -(quarter as i32), raw: 0 }, &mut r);
        assert_eq!(r.log, vec!["midi b0 20 116"], "two's complement -12");
        r.log.clear();
        e.handle(&Event::Knob { name: "Y_GAMMA", index: 1, delta: (KNOB_PER_TURN / 2.0) as i32, raw: 0 }, &mut r);
        assert_eq!(r.log.last().unwrap(), "midi b1 21 64", "absolute: half a turn from 0 = 64 (rounded)");
        r.log.clear();
        e.handle(&btn("USER", true), &mut r);
        e.handle(&btn("USER", false), &mut r);
        assert_eq!(r.log, vec!["midi 90 60 127", "midi 80 60 0"]);
        r.log.clear();
        for _ in 0..2 { e.handle(&btn("LOOP", true), &mut r); e.handle(&btn("LOOP", false), &mut r); }
        assert_eq!(r.log, vec!["midi b0 64 127", "midi b0 64 0"], "toggle alternates, release sends nothing");
        r.log.clear();
        e.handle(&Event::Ball { name: "LEFT", dx: 3, dy: -2, raw_x: 0, raw_y: 0 }, &mut r);
        assert_eq!(r.log, vec!["midi b0 30 67", "midi b0 31 62"]);
        assert!(Profiles::parse("[default.knob]\nY_LIFT = { do = \"midi.cc\", cc = 200 }\n").is_err());
    }

    #[test]
    fn release_goes_to_press_binding_after_context_change() {
        let mut e = eng(&["chrome", "default"]);
        let mut r = Rec::default();
        e.handle(&btn("PLAY_FWD", true), &mut r);
        e.stack = vec!["photo".into(), "default".into()];
        e.handle(&btn("PLAY_FWD", false), &mut r);
        assert_eq!(r.log, vec!["mb 0 true", "mb 0 false"]);
    }

    #[test]
    fn small_first_movements_do_nothing() {
        use std::time::{Duration, Instant};
        let p = Profiles::parse("[settings]\nstart_deg = 9\nstart_ball = 4\npress_ms = 600\n[default.knob]\nHUE = { do = \"midi.cc\", cc = 1, per_turn = 4096 }\n\
            [default.button]\nHUE = { do = \"midi.note\", note = 5 }\nSHIFT_UP = { do = \"layer\", mode = \"auto\", name = \"m\" }\n[default.ball]\nLEFT = { do = \"mouse.scroll\", speed = 1 }\nRIGHT = { do = \"mouse.move\" }\n").unwrap();
        let mut e = Engine::new(p);
        let t0 = Instant::now();
        let at = |ms: u64| Some(t0 + Duration::from_millis(ms));
        let knob = |d: i32| Event::Knob { name: "HUE", index: 10, delta: d, raw: 0 };
        // sums what the knob sent
        let sum = |r: &mut Rec| { let s: i32 = r.log.drain(..).filter(|l| l.starts_with("midi b0")).map(|l| { let v: i32 = l.rsplit(' ').next().unwrap().parse().unwrap(); if v > 63 { v - 128 } else { v } }).sum(); s };
        let near = |a: i32, b: i32, m: &str| assert!((a - b).abs() <= 1, "{a} {b} {m}"); // fractions carry
        let mut r = Rec::default();
        e.now = at(0);
        for d in [40, -30, 60] { e.handle(&knob(d), &mut r); }   // 9 degrees = 102.4 counts: a bump
        assert_eq!(sum(&mut r), 0);
        assert!(e.last.is_none(), "not a touch either");
        e.handle(&knob(50), &mut r);
        near(sum(&mut r), 17, "only what goes beyond");
        e.handle(&knob(-20), &mut r);
        near(sum(&mut r), -20, "then everything counts");
        e.now = at(1400);
        e.handle(&knob(10), &mut r);
        near(sum(&mut r), 10, "also after a short pause");
        e.now = at(3000);
        e.handle(&knob(100), &mut r);
        assert_eq!(sum(&mut r), 0, "after a rest it starts again");
        // a press: closed while held and for 600 ms, unless it is a clear turn (20 degrees = 227.6)
        e.handle(&knob(100), &mut r);
        assert!(sum(&mut r) > 0);
        e.handle(&btn("HUE", true), &mut r);
        e.handle(&knob(60), &mut r);
        e.handle(&btn("HUE", false), &mut r);
        e.now = at(3500);
        e.handle(&knob(150), &mut r);
        assert_eq!(sum(&mut r), 0);
        e.now = at(3700);
        e.handle(&knob(60), &mut r);
        assert_eq!(sum(&mut r), 0, "what was ignored doesn't add up afterwards");
        e.handle(&knob(60), &mut r);
        near(sum(&mut r), 17, "");
        e.handle(&btn("HUE", true), &mut r);
        e.handle(&knob(300), &mut r);
        near(sum(&mut r), 72, "press and turn");
        e.handle(&btn("HUE", false), &mut r);
        // balls; the pointer is never held back, and a bump doesn't count as using a layer
        e.handle(&Event::Ball { name: "LEFT", dx: 2, dy: 2, raw_x: 0, raw_y: 0 }, &mut r);
        assert!(!r.log.iter().any(|l| l.starts_with("scroll")));
        e.handle(&Event::Ball { name: "RIGHT", dx: 1, dy: 0, raw_x: 0, raw_y: 0 }, &mut r);
        assert!(r.log.iter().any(|l| l.starts_with("move")));
        e.handle(&Event::Ball { name: "LEFT", dx: 0, dy: 9, raw_x: 0, raw_y: 0 }, &mut r);
        assert!(r.log.iter().any(|l| l.starts_with("scroll")));
    }

    #[test]
    fn mouse_button_with_a_key_held() {
        let p = Profiles::parse("[default.button]\nAUTO_COLOR = { do = \"mouse.button\", button = \"left\", keys = \"space\" }\n").unwrap();
        let mut e = Engine::new(p);
        let mut r = Rec::default();
        e.handle(&btn("AUTO_COLOR", true), &mut r);
        assert_eq!(r.log.len(), 2);
        assert!(r.log[0].starts_with("key") && r.log[1] == "mb 0 true", "{:?}", r.log);
        e.release_all(&mut r);
        assert!(r.log[2] == "mb 0 false" && r.log[3].starts_with("key"), "let go when the helper pauses: {:?}", r.log);
        e.handle(&btn("AUTO_COLOR", false), &mut r);
        assert_eq!(r.log.len(), 4);
        assert!(Profiles::parse("[default.button]\nSTOP = { do = \"mouse.button\", button = \"left\", keys = \"nokey\" }").is_err());
    }

    #[test]
    fn integration_actions() {
        crate::ext::testing::init();
        let p = Profiles::parse("[photo.knob]\nHUE = { do = \"ph.param\", id = \"Hue\", per_turn = 100 }\n[photo.ring]\nLEFT = { do = \"ph.frames\" }\n[photo.ball]\nLEFT = { do = \"ph.frames\" }\n\
            [photo.button]\nUSER = { do = \"ph.cmd\", id = \"go\", layer = \"fast\" }\n[photo.layer.fast.knob]\nHUE = { do = \"zz.other\" }\n").unwrap();
        let mut e = Engine::new(p);
        e.stack = vec!["photo".into(), "default".into()];
        let mut r = Rec { up: true, ..Default::default() };
        e.handle(&Event::Knob { name: "HUE", index: 10, delta: 2048, raw: 0 }, &mut r);
        e.handle(&Event::Ball { name: "LEFT", dx: 3, dy: -2, raw_x: 0, raw_y: 0 }, &mut r);
        assert_eq!(r.log, [r#"photo {"by":50.0,"id":"Hue","t":"turn"}"#, r#"photo {"t":"roll","x":3.0,"y":-2.0}"#]);
        r.log.clear();
        let turn = RING_PER_TURN;
        e.rotate(Kind::Ring, "LEFT", turn / 96.0, turn, &mut r);   // half a frame: nothing yet
        assert!(r.log.is_empty());
        e.rotate(Kind::Ring, "LEFT", turn / 96.0, turn, &mut r);
        assert_eq!(r.log, [r#"photo {"n":1,"t":"frames"}"#]);
        r.log.clear();
        // a command with a layer: open while held; inside it, a missing integration does nothing
        e.handle(&btn("USER", true), &mut r);
        assert_eq!(e.layers, ["fast"]);
        e.handle(&Event::Knob { name: "HUE", index: 10, delta: 2048, raw: 0 }, &mut r);
        e.handle(&btn("USER", false), &mut r);
        assert!(e.layers.is_empty());
        assert_eq!(r.log, [r#"photo {"id":"go","t":"cmd"}"#, r#"photo {"t":"up"}"#]);
        // not allowed, and nobody there: counted
        e.stack = vec!["photo/dialog".into(), "photo".into(), "default".into()];
        e.handle(&Event::Knob { name: "HUE", index: 10, delta: 2048, raw: 0 }, &mut r);
        e.stack = vec!["photo".into(), "default".into()];
        r.up = false;
        e.handle(&Event::Knob { name: "HUE", index: 10, delta: 2048, raw: 0 }, &mut r);
        assert_eq!(e.skipped["photo"], 2);
    }

    #[test]
    fn lights_follow_integration_values() {
        crate::ext::testing::init();
        let p = Profiles::parse("[photo.leds]\nDISABLE = \"ph.flag == reject\"\nVIEWER = \"ph.rating > 4\"\nUSER = \"zz.flag == pick\"\n").unwrap();
        assert_eq!(p.watched("ph"), ["flag", "rating"]);
        let mut e = Engine::new(p);
        e.stack = vec!["photo".into(), "default".into()];
        assert_eq!(e.leds(), 1 << led_bit("DISABLE").unwrap());
        e.stack = vec!["default".into()];
        assert_eq!(e.leds(), 0);
    }

    #[test]
    fn layer_hold_switches_and_lights() {
        let p = Profiles::parse("[default.button]\nUSER = { do = \"layer\", name = \"m\", mode = \"hold\" }\n[default.ball]\nRIGHT = { do = \"mouse.scroll\" }\n[default.layer.m.ball]\nRIGHT = { do = \"mouse.move\" }\n").unwrap();
        let mut e = Engine::new(p);
        let mut r = Rec::default();
        e.handle(&btn("USER", true), &mut r);
        assert_eq!(e.leds(), 1 << 1, "USER LED while its layer is held");
        e.handle(&Event::Ball { name: "RIGHT", dx: 10, dy: 0, raw_x: 0, raw_y: 0 }, &mut r);
        assert!(r.log.last().unwrap().starts_with("move "), "{:?}", r.log);
        e.handle(&btn("USER", false), &mut r);
        assert_eq!(e.leds(), 0);
        e.handle(&Event::Ball { name: "RIGHT", dx: 10, dy: 0, raw_x: 0, raw_y: 0 }, &mut r);
        assert!(r.log.last().unwrap().starts_with("scroll "), "{:?}", r.log);
    }

    #[test]
    fn steps_and_scroll_carry() {
        let p = Profiles::parse("[default.ring]\nMIDDLE = { do = \"key\", cw = \"right\", ccw = \"left\", per = 25 }\nRIGHT = { do = \"mouse.scroll\", axis = \"v\" }\n").unwrap();
        let mut e = Engine::new(p);
        let mut r = Rec::default();
        for _ in 0..10 { e.handle(&Event::Ring { name: "MIDDLE", delta: 5, raw: 0 }, &mut r); }
        assert_eq!(r.log.len(), 2, "50 steps / per 25 = 2: {:?}", r.log);
        r.log.clear();
        for _ in 0..25 { e.handle(&Event::Ring { name: "RIGHT", delta: 1, raw: 0 }, &mut r); }
        let total: i32 = r.log.iter().map(|l| l.split(' ').nth(1).unwrap().parse::<i32>().unwrap()).sum();
        assert_eq!(total, 240, "25 steps = 2 notches of 1/24 turn");
    }
}

#[cfg(test)]
mod restart_tests {
    use super::*;

    #[test]
    fn restart_keys_follow_place_and_layer() {
        crate::ext::testing::init();
        let p = Profiles::parse("[default.button]
LOOP = { do = \"restart\" }
SHIFT_UP = { do = \"layer\", name = \"more\" }
[firefox.button]
LOOP = { do = \"key\", keys = \"a\" }
[default.layer.more.button]
USER = { do = \"restart\" }
").unwrap();
        assert!(Profiles::parse("[default.knob]
HUE = { do = \"restart\" }").is_err(), "a key only");
        let mut e = Engine::new(p);
        e.stack = vec!["default".into()];
        assert_eq!(e.restart_keys(), ["LOOP"]);
        e.stack = vec!["firefox".into(), "default".into()];
        assert!(e.restart_keys().is_empty(), "firefox has its own LOOP");
        e.layers = vec!["more".into()];
        assert_eq!(e.restart_keys(), ["USER"]);
    }
}
