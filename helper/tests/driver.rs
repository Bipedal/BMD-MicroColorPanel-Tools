//! Offline driver behaviour with a fake HID backend: init order, keep-alive, light restore after a
//! late keep-alive, USB->BT reconnect with synthetic releases/presses, enumerate rate limit.
use micropanel::panel::*;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct World {
    devs: Vec<DevInfo>,
    queue: VecDeque<Vec<u8>>,
    sent: Vec<(String, Vec<u8>)>, // ("feature"/"output", bytes)
    held_report: Vec<u8>,         // GET input 0x02 answer
    dead: bool,                   // reads fail
    enums: u32,
}
type W = Arc<Mutex<World>>;

struct FakeDev(W);
impl HidDev for FakeDev {
    fn read(&mut self, buf: &mut [u8], _t: i32) -> Result<usize, String> {
        let mut w = self.0.lock().unwrap();
        if w.dead { return Err("device gone".into()); }
        match w.queue.pop_front() {
            Some(r) => { buf[..r.len()].copy_from_slice(&r); Ok(r.len()) }
            None => Ok(0),
        }
    }
    fn send_feature(&mut self, d: &[u8]) -> Result<(), String> { self.0.lock().unwrap().sent.push(("feature".into(), d.to_vec())); Ok(()) }
    fn write(&mut self, d: &[u8]) -> Result<usize, String> { self.0.lock().unwrap().sent.push(("output".into(), d.to_vec())); Ok(d.len()) }
    fn get_feature(&mut self, buf: &mut [u8]) -> Result<usize, String> {
        if buf[0] == 0x08 { buf[1] = 0; buf[2] = 87; }
        Ok(buf.len())
    }
    fn get_input(&mut self, buf: &mut [u8]) -> Result<usize, String> {
        let w = self.0.lock().unwrap();
        if buf[0] == 0x02 && w.held_report.len() == 9 { buf.copy_from_slice(&w.held_report); Ok(9) } else { Ok(buf.len()) }
    }
}
struct FakeBackend(W);
impl HidBackend for FakeBackend {
    fn enumerate(&mut self) -> Vec<DevInfo> { let mut w = self.0.lock().unwrap(); w.enums += 1; w.devs.clone() }
    fn open(&mut self, _p: &str) -> Result<Box<dyn HidDev>, String> {
        self.0.lock().unwrap().dead = false;
        Ok(Box::new(FakeDev(self.0.clone())))
    }
}

fn usb() -> DevInfo { DevInfo { path: r"\\?\hid#vid_1edb&pid_da0f&mi_02#x".into(), bt: false, iface: 2 } }
fn bt() -> DevInfo { DevInfo { path: r"\\?\hid#{00001812-0000-1000-8000-00805f9b34fb}_dev".into(), bt: true, iface: -1 } }
fn held(bits_on: &[u8]) -> Vec<u8> {
    let mut r = vec![0u8; 9];
    r[0] = 2;
    for b in bits_on { r[(*b / 8) as usize] |= 1 << (b % 8); }
    r
}
fn setup(devs: Vec<DevInfo>) -> (W, Panel) {
    let w: W = Arc::new(Mutex::new(World { devs, held_report: held(&[]), ..Default::default() }));
    let p = Panel::new(Box::new(FakeBackend(w.clone())), "auto", 70);
    (w, p)
}
fn sent(w: &W) -> Vec<String> {
    w.lock().unwrap().sent.iter().map(|(k, b)| format!("{k} {}", hex(b))).collect()
}

#[test]
fn open_order_and_held_button_at_start() {
    let (w, mut p) = setup(vec![bt(), usb()]);
    w.lock().unwrap().held_report = held(&[30]); // USER held at start
    let ev = p.poll(1);
    assert_eq!(p.link, Some("usb"), "auto prefers USB");
    assert!(matches!(&ev[0], Event::Link { up: true, .. }));
    assert!(ev.iter().any(|e| matches!(e, Event::Button { bit: 30, down: true, synthetic: true, .. })),
            "held button at start gives a synthetic press: {ev:?}");
    let s = sent(&w);
    assert_eq!(s, vec!["feature 0a 01", "feature 03 46 46", "output 04 00 00 00 00 00 00"], "enable first, then lights");
    // its release now matches
    w.lock().unwrap().queue.push_back(held(&[]));
    let ev = p.poll(1);
    assert!(matches!(&ev[..], [Event::Button { bit: 30, down: false, synthetic: false, .. }]), "{ev:?}");
}

#[test]
fn keepalive_and_late_restore() {
    let (w, mut p) = setup(vec![usb()]);
    p.poll(1);
    p.set_leds(1 << 2 | 1 << 10);
    w.lock().unwrap().sent.clear();
    p.advance(10.0); p.poll(1);
    assert!(sent(&w).is_empty(), "no keep-alive before 20 s");
    p.advance(10.5); p.poll(1);
    assert_eq!(sent(&w), vec!["feature 0a 01"], "keep-alive at 20 s, no restore");
    w.lock().unwrap().sent.clear();
    p.advance(60.0); p.poll(1);
    assert_eq!(sent(&w), vec!["feature 0a 01", "feature 03 46 46", "output 04 04 04 00 00 00 00"],
               "late keep-alive re-enables, then restores backlight and LEDs");
}

#[test]
fn rest_and_wake() {
    let (w, mut p) = setup(vec![bt()]);
    p.poll(1);
    w.lock().unwrap().sent.clear();
    p.set_resting(true);
    p.advance(3600.0); p.poll(1);
    assert!(sent(&w).is_empty(), "left alone: {:?}", sent(&w));
    p.set_resting(false);
    p.poll(1);
    assert_eq!(sent(&w), vec!["feature 0a 01", "feature 03 46 46", "output 04 00 00 00 00 00 00"], "woken, with its lights");
}

#[test]
fn reconnect_usb_to_bt() {
    let (w, mut p) = setup(vec![usb()]);
    p.poll(1);
    w.lock().unwrap().queue.push_back(held(&[32]));
    let ev = p.poll(1);
    assert!(matches!(&ev[..], [Event::Button { bit: 32, down: true, .. }]));
    // USB unplugged: read fails
    { let mut x = w.lock().unwrap(); x.dead = true; x.devs = vec![]; }
    let ev = p.poll(1);
    assert!(matches!(&ev[0], Event::Link { up: false, .. }));
    assert!(matches!(&ev[1], Event::Button { bit: 32, down: false, synthetic: true, .. }), "held released on loss");
    assert!(!p.is_up());
    // nothing present: one enumerate, then rate-limited
    let e0 = w.lock().unwrap().enums;
    for _ in 0..5 { p.poll(1); }
    assert_eq!(w.lock().unwrap().enums, e0 + 1, "device list checked at most once a second");
    // BT appears
    w.lock().unwrap().devs = vec![bt()];
    w.lock().unwrap().sent.clear();
    p.advance(1.1);
    let ev = p.poll(1);
    assert_eq!(p.link, Some("bt"));
    assert!(matches!(&ev[0], Event::Link { up: true, .. }), "{ev:?}");
    assert_eq!(sent(&w)[0], "feature 0a 01");
    assert_eq!(p.stats.reopens, 2);
}

#[test]
fn release_dark_and_resume() {
    let (w, mut p) = setup(vec![usb()]);
    p.poll(1);
    p.set_leds(1);
    w.lock().unwrap().sent.clear();
    p.release(true);
    assert_eq!(sent(&w), vec!["output 04 00 00 00 00 00 00", "feature 03 00 00"]);
    assert!(!p.is_up());
    let e0 = w.lock().unwrap().enums;
    p.advance(5.0); p.poll(1);
    assert_eq!(w.lock().unwrap().enums, e0, "no reopen while released");
    p.resume();
    let ev = p.poll(1);
    assert!(matches!(&ev[0], Event::Link { up: true, .. }));
    assert!(sent(&w).contains(&"output 04 01 00 00 00 00 00".to_string()), "LEDs kept across release: {:?}", sent(&w));
}

#[test]
fn battery_and_axis_lock() {
    let (_w, mut p) = setup(vec![usb()]);
    p.poll(1);
    assert_eq!(p.battery(), Some((false, 87)));
    let mut l = AxisLock::new(0.25);
    let mut last = (0, 0);
    for _ in 0..10 { last = l.apply(20, 2); }
    assert_eq!(last, (20, 0));
}
