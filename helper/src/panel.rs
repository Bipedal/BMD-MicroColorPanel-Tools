//! DaVinci Resolve Micro Color Panel driver: a port of `panel.py` (see PANEL_PROTOCOL.md).
//!
//! Every behaviour here mirrors the Python reference driver, which was checked on the panel:
//! decode of 0x02/0x05/0x06/0x08, keep-alive `0a 01` every 20 s, lights restored after any
//! re-enable, USB<->BT reconnect with at most one device-list check per second, held buttons
//! re-read with GET input 0x02 (exact size 9). The decoder is checked against panel.py by replaying
//! every recorded report in logs/ (tests/golden.rs, tools/make_golden.py).

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

pub const VID: u16 = 0x1EDB;
pub const PID: u16 = 0xDA0F;
pub const IFACE: i32 = 2;
pub const READ_LEN: usize = 65;
pub const INIT: [u8; 2] = [0x0A, 0x01];
pub const KEEPALIVE_S: f64 = 20.0;
pub const TIMEOUT_GUARD_S: f64 = 55.0;
pub const ENUM_EVERY_S: f64 = 1.0;
pub const KNOB_RAW: i32 = 360;
pub const BALL_RAW: i32 = 16;
/// Knob counts per revolution [A] (spec section 6) and ring steps per revolution [V] (section 5).
pub const KNOB_PER_TURN: f64 = 4096.0;
pub const RING_PER_TURN: f64 = 300.0;

pub const BUTTONS: [(u8, &str); 52] = [
    (8, "Y_LIFT"), (9, "Y_GAMMA"), (10, "Y_GAIN"), (11, "CONTRAST"),
    (12, "PIVOT"), (13, "MID_DETAIL"), (14, "COLOR_BOOST"), (15, "SHADOWS"),
    (16, "HIGHLIGHTS"), (17, "SATURATION"), (18, "HUE"), (19, "LUM_MIX"),
    (20, "AUTO_COLOR"), (21, "OFFSET"), (22, "COPY"), (23, "PASTE"),
    (24, "UNDO"), (25, "REDO"), (26, "DELETE"), (27, "RESET"),
    (28, "BYPASS"), (29, "DISABLE"), (30, "USER"), (31, "LOOP"),
    (32, "SHIFT_UP"), (33, "SHIFT_DOWN"),
    (34, "PLAY_STILL"), (35, "WIPE_STILL"), (36, "GRAB_STILL"),
    (37, "H_LITE"), (38, "VIEWER"), (39, "CURSOR"), (40, "SELECT"),
    (41, "RESET_LIFT"), (42, "RESET_GAMMA"), (43, "RESET_GAIN"),
    (44, "ADD_NODE"), (45, "ADD_WINDOW"), (46, "ADD_KEYFRM"),
    (47, "PREV_STILL"), (48, "NEXT_STILL"), (49, "PREV_KEYFRM"), (50, "NEXT_KEYFRM"),
    (51, "PREV_NODE"), (52, "NEXT_NODE"), (53, "PREV_FRAME"), (54, "NEXT_FRAME"),
    (55, "PREV_CLIP"), (56, "NEXT_CLIP"),
    (57, "PLAY_REV"), (58, "PLAY_FWD"), (59, "STOP"),
];
pub const KNOBS: [&str; 12] = ["Y_LIFT", "Y_GAMMA", "Y_GAIN", "CONTRAST", "PIVOT", "MID_DETAIL",
    "COLOR_BOOST", "SHADOWS", "HIGHLIGHTS", "SATURATION", "HUE", "LUM_MIX"];
/// (name, x slot, y slot) in report 0x05.
pub const BALLS: [(&str, usize, usize); 3] = [("LEFT", 0, 1), ("MIDDLE", 3, 4), ("RIGHT", 6, 7)];
pub const RINGS: [(&str, usize); 3] = [("LEFT", 2), ("MIDDLE", 5), ("RIGHT", 8)];
/// Status LEDs, output 0x04: name -> bit (spec section 4a).
pub const LEDS: [(&str, u8); 11] = [("BYPASS", 0), ("USER", 1), ("OFFSET", 2), ("DISABLE", 3),
    ("SHIFT_UP", 4), ("SHIFT_DOWN", 5), ("PLAY_STILL", 6), ("WIPE_STILL", 7), ("H_LITE", 8),
    ("VIEWER", 9), ("CURSOR", 10)];

pub fn button_name(bit: u8) -> String {
    BUTTONS.iter().find(|(b, _)| *b == bit).map(|(_, n)| n.to_string())
        .unwrap_or_else(|| format!("b{bit:02}"))
}
pub fn led_bit(name: &str) -> Option<u8> {
    LEDS.iter().find(|(n, _)| *n == name).map(|(_, b)| *b)
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Button { name: String, bit: u8, down: bool, synthetic: bool },
    /// delta in encoder counts (4096/rev assumed), CW = +
    Knob { name: &'static str, index: usize, delta: i32, raw: i32 },
    /// steps of 16 raw; +x = right, +y = up (axes negated from the wire)
    Ball { name: &'static str, dx: i32, dy: i32, raw_x: i32, raw_y: i32 },
    /// steps of 16 raw, ~300/rev, CW = +
    Ring { name: &'static str, delta: i32, raw: i32 },
    Battery { charging: bool, level: u8 },
    Link { transport: String, up: bool, detail: String },
}

impl Event {
    /// Same shape as tools/make_golden.py writes for panel.py events.
    pub fn to_json(&self) -> serde_json::Value {
        use serde_json::json;
        match self {
            Event::Button { name, bit, down, .. } => json!({"k":"button","n":name,"bit":bit,"down":down}),
            Event::Knob { name, index, delta, raw } => json!({"k":"knob","n":name,"index":index,"delta":delta,"raw":raw}),
            Event::Ball { name, dx, dy, raw_x, raw_y } => json!({"k":"ball","n":name,"dx":dx,"dy":dy,"raw_x":raw_x,"raw_y":raw_y}),
            Event::Ring { name, delta, raw } => json!({"k":"ring","n":name,"delta":delta,"raw":raw}),
            Event::Battery { charging, level } => json!({"k":"battery","n":"BATTERY","charging":charging,"level":level}),
            Event::Link { transport, up, .. } => json!({"k":"link","n":transport,"up":up}),
        }
    }
}

fn s24(b: &[u8], i: usize) -> i32 {
    let v = b[i] as i32 | (b[i + 1] as i32) << 8 | (b[i + 2] as i32) << 16;
    if v & 0x80_0000 != 0 { v - (1 << 24) } else { v }
}
fn s32(b: &[u8], i: usize) -> i32 {
    i32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}
/// Button bits from bytes 1..8 only (bytes past the payload are host-side leftovers).
pub fn bits(b: &[u8]) -> BTreeSet<u8> {
    let mut s = BTreeSet::new();
    for pos in 1..9usize {
        if pos >= b.len() { break; }
        for i in 0..8 {
            if b[pos] >> i & 1 == 1 { s.insert((pos * 8 + i) as u8); }
        }
    }
    s
}

/// Pure decoder: raw report -> events. Keeps the held-button set.
#[derive(Default)]
pub struct Decoder {
    pub held: BTreeSet<u8>,
    pub unknown_ids: u64,
}

impl Decoder {
    pub fn decode(&mut self, b: &[u8]) -> Vec<Event> {
        let mut ev = Vec::new();
        if b.is_empty() { return ev; }
        let need = match b[0] { 0x02 => 9, 0x05 => 37, 0x06 => 65, 0x08 => 3, _ => 1 };
        if b.len() < need {
            self.unknown_ids += 1;
            return ev;
        }
        match b[0] {
            0x02 => {
                let now = bits(b);
                for bit in now.difference(&self.held) {
                    ev.push(Event::Button { name: button_name(*bit), bit: *bit, down: true, synthetic: false });
                }
                for bit in self.held.difference(&now) {
                    ev.push(Event::Button { name: button_name(*bit), bit: *bit, down: false, synthetic: false });
                }
                self.held = now;
            }
            0x05 => {
                for (name, sx, sy) in BALLS {
                    let (rx, ry) = (s24(b, 2 + 4 * sx), s24(b, 2 + 4 * sy));
                    if rx != 0 || ry != 0 {
                        // negate: +x right, +y up. Floor division like Python's //.
                        let (dx, dy) = ((-rx).div_euclid(BALL_RAW), (-ry).div_euclid(BALL_RAW));
                        if dx != 0 || dy != 0 {
                            ev.push(Event::Ball { name, dx, dy, raw_x: rx, raw_y: ry });
                        }
                    }
                }
                for (name, s) in RINGS {
                    let r = s24(b, 2 + 4 * s);
                    if r != 0 {
                        ev.push(Event::Ring { name, delta: r.div_euclid(BALL_RAW), raw: r });
                    }
                }
            }
            0x06 => {
                for (k, name) in KNOBS.iter().enumerate() {
                    let r = s32(b, 1 + 4 * k);
                    if r != 0 {
                        ev.push(Event::Knob { name, index: k, delta: r.div_euclid(KNOB_RAW), raw: r });
                    }
                }
            }
            0x08 => ev.push(Event::Battery { charging: b[1] != 0, level: b[2] }),
            _ => self.unknown_ids += 1,
        }
        ev
    }

    /// New held set from a resync; emits synthetic releases then presses (as panel.py).
    pub fn resync(&mut self, now: BTreeSet<u8>) -> Vec<Event> {
        let mut ev = Vec::new();
        for bit in self.held.difference(&now) {
            ev.push(Event::Button { name: button_name(*bit), bit: *bit, down: false, synthetic: true });
        }
        for bit in now.difference(&self.held) {
            ev.push(Event::Button { name: button_name(*bit), bit: *bit, down: true, synthetic: true });
        }
        self.held = now;
        ev
    }
}

/// Suppresses the minor ball axis while one axis clearly dominates (panel.py AxisLock).
pub struct AxisLock { pub ratio: f64, pub alpha: f64, ema: (f64, f64) }
impl AxisLock {
    pub fn new(ratio: f64) -> Self { AxisLock { ratio, alpha: 0.3, ema: (0.0, 0.0) } }
    pub fn apply(&mut self, dx: i32, dy: i32) -> (i32, i32) {
        let a = self.alpha;
        let (mut ex, mut ey) = self.ema;
        ex += a * (dx.abs() as f64 - ex);
        ey += a * (dy.abs() as f64 - ey);
        self.ema = (ex, ey);
        if ey < self.ratio * ex { (dx, 0) } else if ex < self.ratio * ey { (0, dy) } else { (dx, dy) }
    }
}

// ------------------------------------------------------------------ transport abstraction

#[derive(Debug, Clone)]
pub struct DevInfo { pub path: String, pub bt: bool, pub iface: i32 }

pub trait HidDev: Send {
    /// Ok(0) on timeout; Err when the device is gone.
    fn read(&mut self, buf: &mut [u8], timeout_ms: i32) -> Result<usize, String>;
    fn send_feature(&mut self, data: &[u8]) -> Result<(), String>;
    fn write(&mut self, data: &[u8]) -> Result<usize, String>;
    /// buf[0] = report id, buf.len() = exact report size.
    fn get_feature(&mut self, buf: &mut [u8]) -> Result<usize, String>;
    fn get_input(&mut self, buf: &mut [u8]) -> Result<usize, String>;
}

pub trait HidBackend: Send {
    fn enumerate(&mut self) -> Vec<DevInfo>;
    fn open(&mut self, path: &str) -> Result<Box<dyn HidDev>, String>;
}

/// "usb", "bt" or "auto" (USB if present, else BT).
pub fn pick(devs: &[DevInfo], transport: &str) -> Option<(String, &'static str)> {
    let usb = devs.iter().find(|d| !d.bt && (d.iface == IFACE || d.path.to_lowercase().contains("mi_02")));
    let bt = devs.iter().find(|d| d.bt);
    for (tr, d) in [("usb", usb), ("bt", bt)] {
        if let Some(d) = d {
            if transport == tr || transport == "auto" { return Some((d.path.clone(), tr)); }
        }
    }
    None
}

// ------------------------------------------------------------------ driver

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct Stats {
    pub sends_ok: u64, pub sends_failed: u64, pub writes_ok: u64, pub writes_failed: u64,
    pub gets_failed: u64, pub reports: u64, pub inits: u64, pub resyncs: u64,
    pub links_lost: u64, pub reopens: u64, pub restores: u64,
}

pub struct Panel {
    backend: Box<dyn HidBackend>,
    dev: Option<Box<dyn HidDev>>,
    pub transport: String,
    pub link: Option<&'static str>,
    pub dec: Decoder,
    pub leds: u64,
    pub backlight: u8,
    pub stats: Stats,
    t0: Instant,
    clock_offset: f64,
    t_init: Option<f64>,
    t_enum: f64,
    t_lost: Option<f64>,
    t_miss_log: f64,
    /// false while yielding the panel to another program (Resolve): no reopen attempts.
    pub active: bool,
    /// no keep-alive: the panel switches itself off after 60 s (saves the battery)
    resting: bool,
}

pub fn log(msg: &str) { crate::util::log(msg); }

impl Panel {
    pub fn new(backend: Box<dyn HidBackend>, transport: &str, backlight: u8) -> Self {
        Panel { backend, dev: None, transport: transport.to_string(), link: None,
                dec: Decoder::default(), leds: 0, backlight: backlight.min(100), stats: Stats::default(),
                t0: Instant::now(), clock_offset: 0.0, t_init: None, t_enum: f64::NEG_INFINITY,
                t_lost: None, t_miss_log: f64::NEG_INFINITY, active: true, resting: false }
    }

    /// Seconds since creation; tests advance it with `advance`.
    pub fn now(&self) -> f64 { self.t0.elapsed().as_secs_f64() + self.clock_offset }
    pub fn advance(&mut self, s: f64) { self.clock_offset += s; }
    pub fn is_up(&self) -> bool { self.dev.is_some() }

    fn send(&mut self, pkt: &[u8]) -> bool {
        let r = match self.dev.as_mut() { Some(d) => d.send_feature(pkt), None => Err("link down".into()) };
        match r {
            Ok(()) => { self.stats.sends_ok += 1; true }
            Err(e) => { self.stats.sends_failed += 1; log(&format!("feature send {} failed: {e}", hex(pkt))); false }
        }
    }
    fn write(&mut self, pkt: &[u8]) -> bool {
        let r = match self.dev.as_mut() { Some(d) => d.write(pkt), None => Err("link down".into()) };
        match r {
            Ok(n) if n > 0 => { self.stats.writes_ok += 1; true }
            Ok(n) => { self.stats.writes_failed += 1; log(&format!("output write {} returned {n}", hex(pkt))); false }
            Err(e) => { self.stats.writes_failed += 1; log(&format!("output write {} failed: {e}", hex(pkt))); false }
        }
    }
    pub fn get_feature(&mut self, id: u8, size: usize) -> Option<Vec<u8>> {
        let mut buf = vec![0u8; size];
        buf[0] = id;
        let r = match self.dev.as_mut() { Some(d) => d.get_feature(&mut buf), None => Err("link down".into()) };
        match r {
            Ok(n) => { buf.truncate(n.max(1)); Some(buf) }
            Err(e) => { self.stats.gets_failed += 1; log(&format!("get feature 0x{id:02x}/{size} failed: {e}")); None }
        }
    }
    fn get_input(&mut self, id: u8, size: usize) -> Option<Vec<u8>> {
        let mut buf = vec![0u8; size];
        buf[0] = id;
        let r = match self.dev.as_mut() { Some(d) => d.get_input(&mut buf), None => Err("link down".into()) };
        match r {
            Ok(n) => { buf.truncate(n.max(1)); Some(buf) }
            Err(e) => { self.stats.gets_failed += 1; log(&format!("get input 0x{id:02x}/{size} failed: {e}")); None }
        }
    }

    /// Held buttons read on request (GET input 0x02, exact size 9).
    pub fn state(&mut self) -> Option<BTreeSet<u8>> {
        let b = self.get_input(0x02, 9)?;
        if b[0] == 0x02 && b.len() >= 9 { Some(bits(&b)) } else { None }
    }

    /// 0a 01, then re-read held buttons; returns synthetic events for any difference.
    fn init(&mut self) -> Vec<Event> {
        self.stats.inits += 1;
        self.send(&INIT);
        self.t_init = Some(self.now());
        let now = self.state().unwrap_or_default();
        self.stats.resyncs += 1;
        self.dec.resync(now)
    }

    /// Resend backlight and LEDs (needed after any re-enable, spec section 3).
    pub fn restore(&mut self) -> bool {
        self.stats.restores += 1;
        let a = self.set_backlight(self.backlight);
        let b = self.set_leds(self.leds);
        a && b
    }

    /// Light exactly the LEDs in `mask` (bit numbers from LEDS). Kept while the link is down.
    pub fn set_leds(&mut self, mask: u64) -> bool {
        self.leds = mask & 0x7FF;
        if self.dev.is_none() { return false; }
        let v = self.leds.to_le_bytes();
        let pkt = [0x04, v[0], v[1], v[2], v[3], v[4], v[5]];
        self.write(&pkt)
    }

    pub fn set_backlight(&mut self, level: u8) -> bool {
        self.backlight = level.min(100);
        if self.dev.is_none() { return false; }
        let l = self.backlight;
        self.send(&[0x03, l, l])
    }

    pub fn battery(&mut self) -> Option<(bool, u8)> {
        let b = self.get_feature(0x08, 3)?;
        if b.len() >= 3 { Some((b[1] != 0, b[2])) } else { None }
    }

    /// (version "2.2.0", serial) where readable.
    pub fn info(&mut self) -> (Option<String>, Option<String>) {
        let v = self.get_feature(0x01, 8).filter(|v| v.len() >= 8)
            .map(|v| format!("{}.{}.{}", v[5], v[6], v[7]));
        let s = self.get_feature(0x09, 33).map(|s| {
            let end = s[1..].iter().position(|&c| c == 0).map(|p| p + 1).unwrap_or(s.len());
            String::from_utf8_lossy(&s[1..end]).to_string()
        });
        (v, s)
    }

    /// Let the panel go to rest, or wake it: the next keep-alive enables it again and restores the lights.
    pub fn set_resting(&mut self, on: bool) { self.resting = on; }

    fn keepalive(&mut self) {
        if self.resting { return; }
        let now = self.now();
        let gap = self.t_init.map(|t| now - t);
        if gap.map_or(true, |g| g >= KEEPALIVE_S) {
            self.send(&INIT);
            self.t_init = Some(self.now());
            if let Some(g) = gap {
                if g >= TIMEOUT_GUARD_S {
                    log(&format!("keep-alive after {g:.0} s: panel may have timed out; restoring lights"));
                    self.restore();
                }
            }
        }
    }

    /// One poll step: keep-alive, read one report (or try to reopen), decode.
    /// Never sleeps longer than `timeout_ms` (dropped reports lose movement).
    pub fn poll(&mut self, timeout_ms: i32) -> Vec<Event> {
        if self.dev.is_none() {
            if !self.active {
                std::thread::sleep(Duration::from_millis(timeout_ms as u64));
                return vec![];
            }
            return self.try_open(timeout_ms);
        }
        self.keepalive();
        let mut buf = [0u8; READ_LEN];
        let r = self.dev.as_mut().unwrap().read(&mut buf, timeout_ms);
        match r {
            Ok(0) => vec![],
            Ok(n) => {
                self.stats.reports += 1;
                self.dec.decode(&buf[..n.max(1)].iter().copied().chain(std::iter::repeat(0)).take(READ_LEN).collect::<Vec<_>>())
            }
            Err(e) => self.lost(&e),
        }
    }

    fn lost(&mut self, err: &str) -> Vec<Event> {
        let tr = self.link.unwrap_or("?").to_string();
        log(&format!("link {tr} lost: {err}; reconnecting"));
        self.stats.links_lost += 1;
        self.dev = None;
        self.link = None;
        self.t_lost = Some(self.now());
        self.t_enum = f64::NEG_INFINITY;
        let mut ev = vec![Event::Link { transport: tr, up: false, detail: err.to_string() }];
        ev.extend(self.dec.resync(BTreeSet::new()));
        ev
    }

    /// One open attempt at most every ENUM_EVERY_S (faster polling stalled USB string reads).
    fn try_open(&mut self, timeout_ms: i32) -> Vec<Event> {
        let now = self.now();
        if now - self.t_enum < ENUM_EVERY_S {
            std::thread::sleep(Duration::from_millis(timeout_ms as u64));
            return vec![];
        }
        self.t_enum = now;
        let devs = self.backend.enumerate();
        let Some((path, tr)) = pick(&devs, &self.transport) else {
            if now - self.t_miss_log >= 30.0 {
                self.t_miss_log = now;
                log(&format!("panel not found over {} ({} matching HID entries: {:?})", self.transport, devs.len(),
                    devs.iter().map(|d| (d.bt, d.iface)).collect::<Vec<_>>()));
            }
            std::thread::sleep(Duration::from_millis(timeout_ms as u64));
            return vec![];
        };
        match self.backend.open(&path) {
            Ok(d) => {
                self.dev = Some(d);
                self.link = Some(tr);
                self.stats.reopens += 1;
                // Enable first, then the lights: 0a 01 after a timeout brings a dim backlight.
                let sync = self.init();
                self.restore();
                let down = self.t_lost.map(|t| format!("{:.1}", self.now() - t)).unwrap_or_default();
                log(&format!("link up over {tr} (down {down} s)"));
                let mut ev = vec![Event::Link { transport: tr.to_string(), up: true, detail: down }];
                ev.extend(sync);
                ev
            }
            Err(e) => { log(&format!("open over {tr} failed: {e}; retrying")); vec![] }
        }
    }

    /// Release the panel (Resolve takes over, or quitting). `dark`: LEDs off and backlight 0
    /// first, like Resolve's quit. Held buttons are released.
    pub fn release(&mut self, dark: bool) -> Vec<Event> {
        if self.dev.is_some() {
            if dark {
                let (l, b) = (self.leds, self.backlight);
                self.set_leds(0);
                self.set_backlight(0);
                self.leds = l;
                self.backlight = b;
            }
            self.dev = None;
            self.link = None;
        }
        self.active = false;
        self.dec.resync(BTreeSet::new())
    }

    pub fn resume(&mut self) {
        self.active = true;
        self.t_enum = f64::NEG_INFINITY;
        self.t_init = None;
    }
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" ")
}

// ------------------------------------------------------------------ hidapi backend

pub struct HidApiBackend { api: Option<hidapi::HidApi> }

impl HidApiBackend {
    pub fn new() -> Self { HidApiBackend { api: None } }
}

struct HidApiDev(hidapi::HidDevice);

impl HidDev for HidApiDev {
    fn read(&mut self, buf: &mut [u8], timeout_ms: i32) -> Result<usize, String> {
        self.0.read_timeout(buf, timeout_ms).map_err(|e| e.to_string())
    }
    fn send_feature(&mut self, data: &[u8]) -> Result<(), String> {
        self.0.send_feature_report(data).map_err(|e| e.to_string())
    }
    fn write(&mut self, data: &[u8]) -> Result<usize, String> {
        self.0.write(data).map_err(|e| e.to_string())
    }
    fn get_feature(&mut self, buf: &mut [u8]) -> Result<usize, String> {
        self.0.get_feature_report(buf).map_err(|e| e.to_string())
    }
    fn get_input(&mut self, buf: &mut [u8]) -> Result<usize, String> {
        self.0.get_input_report(buf).map_err(|e| e.to_string())
    }
}

impl HidBackend for HidApiBackend {
    fn enumerate(&mut self) -> Vec<DevInfo> {
        if self.api.is_none() {
            #[allow(deprecated)]
            match hidapi::HidApi::new_without_enumerate() {
                Ok(a) => self.api = Some(a),
                Err(e) => { log(&format!("hidapi init failed: {e}")); return vec![]; }
            }
        }
        let api = self.api.as_mut().unwrap();
        if let Err(e) = api.reset_devices().and_then(|_| api.add_devices(VID, PID)) {
            log(&format!("hid enumerate failed: {e}"));
            return vec![];
        }
        api.device_list().map(|d| {
            let path = d.path().to_string_lossy().to_string();
            let bt = matches!(d.bus_type(), hidapi::BusType::Bluetooth) || path.to_lowercase().contains("{00001812-");
            DevInfo { path, bt, iface: d.interface_number() }
        }).collect()
    }
    fn open(&mut self, path: &str) -> Result<Box<dyn HidDev>, String> {
        let api = self.api.as_ref().ok_or("hidapi not initialised")?;
        let c = std::ffi::CString::new(path).map_err(|e| e.to_string())?;
        api.open_path(&c).map(|d| Box::new(HidApiDev(d)) as Box<dyn HidDev>).map_err(|e| e.to_string())
    }
}
