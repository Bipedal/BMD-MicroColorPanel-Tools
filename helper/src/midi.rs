//! MIDI out and in. The ports themselves are the system's (`sys::midi`).
//!
//! Out: the engine's midi.* actions. In: what the app sends back (feedback) is kept per
//! channel/kind/number for LED rules (`midi.cc.20`, `midi.ch2.note.60`) and so absolute knobs
//! continue from the app's value. License: CC0.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use crate::sys::midi as ports;
pub use ports::{in_names, out_names};

struct Ports { out: Option<(usize, String)>, inp: Option<(usize, String)>, want: (String, String) }
static PORTS: Mutex<Ports> = Mutex::new(Ports { out: None, inp: None, want: (String::new(), String::new()) });
/// "ch.cc.n" / "ch.note.n" -> last value (sent or received); ch 1..16
static VALUES: Mutex<Option<HashMap<String, f64>>> = Mutex::new(None);
/// bumps on every received message (the LED loop re-evaluates)
pub static RX_COUNT: AtomicU64 = AtomicU64::new(0);
static TX_COUNT: AtomicU64 = AtomicU64::new(0);

/// Which port a setting means. "" / "off" = none. "auto": out = the first loopback port (Windows
/// MIDI Services "...Loopback (A)", or loopMIDI); in = the same port only for an A/B loopback pair
/// (a single loopMIDI port would echo our own output back). Anything else: first name containing it.
pub fn pick(names: &[String], want: &str, input: bool, out_name: Option<&str>) -> Option<usize> {
    let w = want.trim().to_lowercase();
    if w.is_empty() || w == "off" { return None; }
    let low: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();
    if w == "auto" {
        if input {
            let o = out_name?.to_lowercase();
            if !(o.contains("loopback") && o.contains("(a)")) { return None; }
            return low.iter().position(|n| *n == o);
        }
        return low.iter().position(|n| n.contains("loopback") && n.contains("(a)"))
            .or_else(|| low.iter().position(|n| n.contains("loopback") || n.contains("loopmidi")));
    }
    low.iter().position(|n| n.contains(&w))
}

/// One message came in.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn received(st: u8, d1: u8, d2: u8) {
    if let Some(k) = key_of(st, d1, d2) { remember(&k.0, k.1); RX_COUNT.fetch_add(1, Ordering::Relaxed); }
}

/// (values key, value) for a channel message we track
fn key_of(st: u8, d1: u8, d2: u8) -> Option<(String, f64)> {
    let ch = (st & 0x0F) + 1;
    match st & 0xF0 {
        0xB0 => Some((format!("{ch}.cc.{d1}"), d2 as f64)),
        0x90 => Some((format!("{ch}.note.{d1}"), d2 as f64)), // velocity 0 = off
        0x80 => Some((format!("{ch}.note.{d1}"), 0.0)),
        0xE0 => Some((format!("{ch}.pb"), ((d2 as u32) << 7 | d1 as u32) as f64)), // 0..16383
        _ => None,
    }
}
fn remember(k: &str, v: f64) {
    let mut g = VALUES.lock().unwrap();
    g.get_or_insert_with(HashMap::new).insert(k.to_string(), v);
}
pub fn value(k: &str) -> Option<f64> { VALUES.lock().unwrap().as_ref().and_then(|m| m.get(k).copied()) }
pub fn values() -> HashMap<String, f64> { VALUES.lock().unwrap().clone().unwrap_or_default() }

/// Open / re-open the ports the settings ask for. Cheap when nothing changed; call every ~2 s.
pub fn ensure(want_out: &str, want_in: &str) {
    if want_out.trim().eq_ignore_ascii_case("auto") && pick(&out_names(), "auto", false, None).is_none() {
        ports::make_loopback();
    }
    let outs = out_names();
    let ins = in_names();
    let mut p = PORTS.lock().unwrap();
    let oi = pick(&outs, want_out, false, None);
    let o_name = oi.map(|i| outs[i].clone());
    let ii = pick(&ins, want_in, true, o_name.as_deref());
    let cur_out = p.out.as_ref().map(|x| x.1.clone());
    if cur_out != o_name || p.want.0 != want_out {
        p.out = None;
        match ports::open_out(oi) {
            Ok(()) => if let Some(i) = oi { p.out = Some((i, outs[i].clone())); crate::util::log(&format!("midi: out -> {}", outs[i])); },
            Err(e) => crate::util::log(&format!("midi: can't open out port {} ({e}; in use by another app?)", o_name.as_deref().unwrap_or(""))),
        }
    }
    let i_name = ii.map(|i| ins[i].clone());
    let cur_in = p.inp.as_ref().map(|x| x.1.clone());
    if cur_in != i_name || p.want.1 != want_in {
        p.inp = None;
        match ports::open_in(ii) {
            Ok(()) => if let Some(i) = ii { p.inp = Some((i, ins[i].clone())); crate::util::log(&format!("midi: in <- {}", ins[i])); },
            Err(e) => crate::util::log(&format!("midi: can't open in port {} ({e})", i_name.as_deref().unwrap_or(""))),
        }
    }
    p.want = (want_out.to_string(), want_in.to_string());
}

/// Send one channel message; false if no out port is open.
pub fn send(m: [u8; 3]) -> bool {
    if let Some(k) = key_of(m[0], m[1], m[2]) { remember(&k.0, k.1); }
    let ok = ports::send(m);
    if ok { TX_COUNT.fetch_add(1, Ordering::Relaxed); }
    ok
}

pub fn status() -> serde_json::Value {
    let p = PORTS.lock().unwrap();
    serde_json::json!({
        "out": p.out.as_ref().map(|x| x.1.clone()), "in": p.inp.as_ref().map(|x| x.1.clone()),
        "outs": out_names(), "ins": in_names(),
        "sent": TX_COUNT.load(Ordering::Relaxed), "received": RX_COUNT.load(Ordering::Relaxed),
    })
}

/// Relative encoder byte for a signed tick count (|n| <= 63).
/// twos: 1..63 up, 127..65 down. offset: 65.. up, 63.. down (64 = 0). signbit: 1..63 up, 65..127 down.
pub fn rel_byte(enc: RelEnc, n: i32) -> u8 {
    let n = n.clamp(-63, 63);
    match enc {
        RelEnc::Twos => if n >= 0 { n as u8 } else { (128 + n) as u8 },
        RelEnc::Offset => (64 + n) as u8,
        RelEnc::SignBit => if n >= 0 { n as u8 } else { (64 - n) as u8 },
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RelEnc { Twos, Offset, SignBit }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_encodings() {
        assert_eq!((rel_byte(RelEnc::Twos, 1), rel_byte(RelEnc::Twos, -1)), (1, 127));
        assert_eq!((rel_byte(RelEnc::Offset, 1), rel_byte(RelEnc::Offset, -1)), (65, 63));
        assert_eq!((rel_byte(RelEnc::SignBit, 3), rel_byte(RelEnc::SignBit, -3)), (3, 67));
        assert_eq!(rel_byte(RelEnc::Twos, 500), 63);
    }

    #[test]
    fn port_picking() {
        let n: Vec<String> = ["Microsoft GS Wavetable Synth", "KnobX1", "Default App Loopback (A)", "Default App Loopback (B)"]
            .iter().map(|s| s.to_string()).collect();
        assert_eq!(pick(&n, "auto", false, None), Some(2));
        assert_eq!(pick(&n, "auto", true, Some("Default App Loopback (A)")), Some(2));
        assert_eq!(pick(&n, "knobx", false, None), Some(1));
        assert_eq!(pick(&n, "", false, None), None);
        let l: Vec<String> = ["loopMIDI Port"].iter().map(|s| s.to_string()).collect();
        assert_eq!(pick(&l, "auto", false, None), Some(0));
        assert_eq!(pick(&l, "auto", true, Some("loopMIDI Port")), None, "a single loopMIDI port would echo");
    }

    #[test]
    fn feedback_keys() {
        assert_eq!(key_of(0xB1, 20, 100), Some(("2.cc.20".into(), 100.0)));
        assert_eq!(key_of(0x90, 60, 0), Some(("1.note.60".into(), 0.0)));
        assert_eq!(key_of(0xE1, 0x7F, 0x7F), Some(("2.pb".into(), 16383.0)));
        assert_eq!(key_of(0xA0, 0, 0), None);
    }
}
