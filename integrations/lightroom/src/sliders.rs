//! Controls of Lightroom's panels that its plugin interface can't reach: the masking brush, mask
//! refinement, the Remove tool's opacity, some checkboxes. They are ordinary Windows controls; this
//! moves a slider as a drag would (checked live 2026-11-23, Lightroom Classic 16: the number and the
//! brush circle follow at once) and clicks a button.
//!
//! A control is found by the English labels that stand next to it in the same group, so a change of
//! Lightroom's layout or language makes this find nothing; it then does nothing and says so. Only what
//! is on screen is ever touched.

use std::collections::HashMap;
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Where a control is: the labels its group has (`only` = no others), those of the group around it, its own.
struct At { has: &'static [&'static str], only: bool, around: &'static [&'static str], own: &'static str, /// highest position, to tell look-alikes apart
    max: i32 }
const fn at(has: &'static [&'static str], own: &'static str, max: i32) -> At { At { has, only: false, around: &[], own, max } }
const fn only(has: &'static [&'static str], own: &'static str, max: i32) -> At { At { has, only: true, around: &[], own, max } }

pub struct Slider { pub id: &'static str, pub label: &'static str, pub group: &'static str, places: &'static [At] }
/// Read from Lightroom Classic 16 on 2026-11-23. Several places = whichever is on screen.
pub const SLIDERS: [Slider; 7] = [
    Slider { id: "mask.size", label: "Mask brush size", group: "Mask", places: &[
        at(&["A", "B", "Erase", "Size"], "Size", 1000),
        At { has: &["Size"], only: true, around: &["Mode :"], own: "Size", max: 100000 }] },  // selecting an object
    Slider { id: "mask.feather", label: "Mask feather", group: "Mask", places: &[
        at(&["Feather", "Flow", "Density", "Auto Mask"], "Feather", 1000),
        only(&["Feather", "Edge"], "Feather", 100000),                                         // subject, sky, object...
        only(&["Feather"], "Feather", 100000)] },                                              // radial gradient
    Slider { id: "mask.flow", label: "Mask brush flow", group: "Mask", places: &[at(&["Feather", "Flow", "Density", "Auto Mask"], "Flow", 1000)] },
    Slider { id: "mask.density", label: "Mask brush density", group: "Mask", places: &[at(&["Feather", "Flow", "Density", "Auto Mask"], "Density", 1000)] },
    Slider { id: "mask.edge", label: "Mask edge", group: "Mask", places: &[only(&["Feather", "Edge"], "Edge", 50000)] },
    Slider { id: "mask.refine", label: "Mask color range refine", group: "Mask", places: &[only(&["Refine"], "Refine", 100000)] },
    Slider { id: "remove.opacity", label: "Remove opacity", group: "Remove", places: &[at(&["Opacity"], "Opacity", 1000)] },
];

pub struct Button { pub id: &'static str, pub label: &'static str, pub group: &'static str, place: At }
const BRUSH: &[&str] = &["A", "B", "Erase", "Size"];
pub const BUTTONS: [Button; 7] = [
    Button { id: "maskAutoMask", label: "Auto Mask on/off", group: "Masks", place: at(&["Feather", "Flow", "Density", "Auto Mask"], "Auto Mask", 0) },
    Button { id: "maskBrushA", label: "Brush A", group: "Masks", place: at(BRUSH, "A", 0) },
    Button { id: "maskBrushB", label: "Brush B", group: "Masks", place: at(BRUSH, "B", 0) },
    Button { id: "maskErase", label: "Brush: erase", group: "Masks", place: at(BRUSH, "Erase", 0) },
    Button { id: "maskLuminanceMap", label: "Show luminance map on/off", group: "Masks", place: at(&["Show Luminance Map"], "Show Luminance Map", 0) },
    Button { id: "removeVisualize", label: "Visualize spots on/off", group: "Remove", place: at(&["Visualize Spots"], "Visualize Spots", 0) },
    Button { id: "removeGenAI", label: "Use generative AI on/off", group: "Remove", place: at(&["Use generative AI"], "Use generative AI", 0) },
];

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Change { By(f64), To(f64) }
#[derive(Debug, Clone, Copy, PartialEq)]
enum Job { Slide(usize, Change), Click(usize) }

/// What a finished change looks like: text to show, or why it didn't work.
pub type Done = Arc<dyn Fn(Result<String, String>) + Send + Sync>;

#[derive(Clone, Default)]
pub struct Sliders { tx: Arc<Mutex<Option<Sender<Job>>>> }

impl Sliders {
    pub fn start(&self, done: Done) {
        let (tx, rx) = channel::<Job>();
        *self.tx.lock().unwrap() = Some(tx);
        let _ = std::thread::Builder::new().name("lr-sliders".into()).spawn(move || {
            let mut w = Worker { found: HashMap::new(), at: HashMap::new(), failed: HashMap::new() };
            while let Ok(first) = rx.recv() {
                // what piled up meanwhile goes out as one step
                let mut todo = vec![first];
                let until = Instant::now() + Duration::from_millis(25);
                loop {
                    match rx.recv_timeout(until.saturating_duration_since(Instant::now())) {
                        Ok(x) => todo.push(x),
                        Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
                for i in 0..SLIDERS.len() {
                    let mine: Vec<Change> = todo.iter().filter_map(|x| match x { Job::Slide(k, c) if *k == i => Some(*c), _ => None }).collect();
                    if !mine.is_empty() { done(w.slide(i, &mine)); }
                }
                for j in &todo { if let Job::Click(i) = j { done(click(*i)); } }
            }
        });
    }

    fn put(&self, j: Job) -> bool { self.tx.lock().unwrap().as_ref().is_some_and(|t| t.send(j).is_ok()) }
    /// Is it one of these settings? Then it is queued.
    pub fn send(&self, id: &str, c: Change) -> Option<bool> { SLIDERS.iter().position(|x| x.id == id).map(|i| self.put(Job::Slide(i, c))) }
    /// Is it one of these buttons?
    pub fn press(&self, id: &str) -> Option<bool> { BUTTONS.iter().position(|x| x.id == id).map(|i| self.put(Job::Click(i))) }
}

/// Slider positions per unit shown: 120 = 12.0 on the short ones, 62000 = 62 on the long ones.
fn scale(hi: i32) -> f64 { if hi <= 1000 { 10.0 } else { 1000.0 } }

/// Where a moving control goes on: its own exact value for a second, else the slider's.
fn next(own: Option<(f64, Instant)>, slider: f64, changes: &[Change], lo: f64, hi: f64, scale: f64, now: Instant) -> f64 {
    let mut v = own.filter(|o| now.duration_since(o.1) < Duration::from_secs(1)).map_or(slider, |o| o.0);
    for c in changes {
        v = match c { Change::By(d) => v + d * scale, Change::To(x) => x * scale };
        v = v.clamp(lo, hi);
    }
    v
}

/// A child window on screen, as far as finding things needs it. `max` = a slider's highest position.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Win { pub h: isize, pub parent: isize, pub around: isize, pub class: String, pub text: String, pub x: i32, pub y: i32, pub height: i32, pub max: i32 }

fn labels(all: &[Win], parent: isize) -> Vec<&str> {
    all.iter().filter(|w| w.parent == parent && w.class != BAR && !w.text.is_empty()).map(|w| w.text.as_str()).collect()
}
const BAR: &str = "msctls_trackbar32";

fn groups<'a>(all: &'a [Win], a: &At, own_class: &str) -> Vec<&'a Win> {
    all.iter().filter(|w| w.class == own_class && w.text == a.own).filter(|w| {
        let l = labels(all, w.parent);
        a.has.iter().all(|x| l.contains(x)) && (!a.only || l.len() == a.has.len())
            && (a.around.is_empty() || { let o = labels(all, w.around); a.around.iter().all(|x| o.contains(x)) })
    }).collect()
}

/// The slider next to its label.
fn find_slider(all: &[Win], places: &[At]) -> Option<isize> {
    places.iter().find_map(|a| groups(all, a, "Static").into_iter().find_map(|l| {
        all.iter().find(|w| w.class == BAR && w.parent == l.parent && w.max == a.max && w.x > l.x
            && ((2 * w.y + w.height) - (2 * l.y + l.height)).abs() <= l.height).map(|w| w.h)
    }))
}
/// The one button, if there is exactly one.
fn find_button(all: &[Win], a: &At) -> Option<isize> {
    match groups(all, a, "Button").as_slice() { [w] => Some(w.h), _ => None }
}

fn click(i: usize) -> Result<String, String> {
    let b = &BUTTONS[i];
    let name = b.label.trim_end_matches(" on/off");
    let h = find_button(&os::children(), &b.place).ok_or(format!("“{}” is not on screen", b.place.own))?;
    let on = os::click(h).ok_or("Lightroom did not take the click")?;
    Ok(if b.label.ends_with(" on/off") { format!("{name} {}", if on { "on" } else { "off" }) } else { name.into() })
}

struct Worker {
    /// slider windows, by index
    found: HashMap<usize, isize>,
    at: HashMap<usize, (f64, Instant)>,
    /// when they weren't there: not searched again at once
    failed: HashMap<usize, Instant>,
}

impl Worker {
    fn slide(&mut self, i: usize, changes: &[Change]) -> Result<String, String> {
        let s = &SLIDERS[i];
        if !self.found.get(&i).is_some_and(|h| os::usable(*h)) {
            self.found.remove(&i);
            self.at.remove(&i);
            if self.failed.get(&i).is_some_and(|t| t.elapsed() < Duration::from_millis(700)) { return Ok(String::new()); }
            match find_slider(&os::children(), s.places) {
                Some(h) => { self.found.insert(i, h); self.failed.remove(&i); }
                None => { self.failed.insert(i, Instant::now()); return Err(format!("“{}” is not on screen", s.label)); }
            }
        }
        let h = self.found[&i];
        let (pos, lo, hi) = os::read(h).ok_or("Lightroom did not answer")?;
        let (now, k) = (Instant::now(), scale(hi));
        let v = next(self.at.get(&i).copied(), pos as f64, changes, lo as f64, hi as f64, k, now);
        self.at.insert(i, (v, now));
        // Lightroom shows tenths; the long sliders would take thousandths
        let n = (v / (k / 10.0)).round() as i32 * (k / 10.0) as i32;
        if n != pos && !os::set(h, n) { self.found.remove(&i); return Err("Lightroom did not take the change".into()); }
        Ok(format!("{}  {:.1}", s.label, n as f64 / k))
    }
}

#[cfg(windows)]
mod os {
    use super::{Win, BAR};
    use windows_sys::core::BOOL;
    use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT};
    use windows_sys::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumChildWindows, EnumWindows, GetClassNameW, GetParent, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsWindow, IsWindowVisible,
        SendMessageTimeoutW, SMTO_ABORTIFHUNG,
    };

    const TBM_GETPOS: u32 = 0x400;
    const TBM_GETRANGEMIN: u32 = 0x401;
    const TBM_GETRANGEMAX: u32 = 0x402;
    const TBM_SETPOS: u32 = 0x405;
    const WM_HSCROLL: u32 = 0x114;
    const TB_LINEDOWN: usize = 1;
    const TB_THUMBPOSITION: usize = 4;
    const TB_THUMBTRACK: usize = 5;
    const TB_ENDTRACK: usize = 8;
    const BM_GETCHECK: u32 = 0xF0;
    const BM_CLICK: u32 = 0xF5;

    fn msg(h: isize, m: u32, w: usize, l: isize) -> Option<i32> {
        let mut r = 0usize;
        let ok = unsafe { SendMessageTimeoutW(h as HWND, m, w, l, SMTO_ABORTIFHUNG, 300, &mut r) };
        (ok != 0).then_some(r as i32)
    }

    pub fn usable(h: isize) -> bool {
        unsafe { IsWindow(h as HWND) != 0 && IsWindowVisible(h as HWND) != 0 && IsWindowEnabled(h as HWND) != 0 }
    }
    /// (position, lowest, highest)
    pub fn read(h: isize) -> Option<(i32, i32, i32)> {
        let r = (msg(h, TBM_GETPOS, 0, 0)?, msg(h, TBM_GETRANGEMIN, 0, 0)?, msg(h, TBM_GETRANGEMAX, 0, 0)?);
        (r.1 < r.2).then_some(r)
    }
    /// What a drag to this position and letting go there sends. Letting go at every step keeps
    /// Lightroom's number up to date while turning; without it the number waits for the end, and the
    /// picture is no faster (tried by the user 2026-11-23).
    pub fn set(h: isize, pos: i32) -> bool {
        let parent = unsafe { GetParent(h as HWND) } as isize;
        if parent == 0 || msg(h, TBM_SETPOS, 1, pos as isize).is_none() { return false; }
        let end = || msg(parent, WM_HSCROLL, TB_ENDTRACK, h).is_some();
        // a drag message carries 16 bits of position; beyond that, one without a position, after
        // which the program has to ask the slider
        let Ok(p) = u16::try_from(pos) else { return msg(parent, WM_HSCROLL, TB_LINEDOWN, h).is_some() && end() };
        let at = (p as usize) << 16;
        msg(parent, WM_HSCROLL, TB_THUMBTRACK | at, h).is_some() && msg(parent, WM_HSCROLL, TB_THUMBPOSITION | at, h).is_some() && end()
    }
    /// Click it; is it checked afterwards?
    pub fn click(h: isize) -> Option<bool> {
        if !usable(h) { return None; }
        msg(h, BM_CLICK, 0, 0)?;
        std::thread::sleep(std::time::Duration::from_millis(60));
        Some(msg(h, BM_GETCHECK, 0, 0)? == 1)
    }

    fn is_lightroom(h: HWND) -> bool {
        unsafe {
            let mut pid = 0u32;
            GetWindowThreadProcessId(h, &mut pid);
            let p = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if p.is_null() { return false; }
            let mut buf = [0u16; 1024];
            let mut n = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(p, 0, buf.as_mut_ptr(), &mut n);
            CloseHandle(p);
            ok != 0 && String::from_utf16_lossy(&buf[..n as usize]).to_lowercase().ends_with("\\lightroom.exe")
        }
    }
    unsafe extern "system" fn child(h: HWND, out: LPARAM) -> BOOL {
        unsafe {
            if IsWindowVisible(h) == 0 { return 1; }
            let mut b = [0u16; 64];
            let n = GetClassNameW(h, b.as_mut_ptr(), b.len() as i32).max(0) as usize;
            let class = String::from_utf16_lossy(&b[..n]);
            let bar = class == BAR;
            if !bar && class != "Static" && class != "Button" || bar && IsWindowEnabled(h) == 0 { return 1; }
            let n = if bar { 0 } else { GetWindowTextW(h, b.as_mut_ptr(), b.len() as i32).max(0) as usize };
            let mut r: RECT = std::mem::zeroed();
            if GetWindowRect(h, &mut r) == 0 { return 1; }
            let parent = GetParent(h);
            let max = if bar { msg(h as isize, TBM_GETRANGEMAX, 0, 0).unwrap_or(0) } else { 0 };
            (*(out as *mut Vec<Win>)).push(Win { h: h as isize, parent: parent as isize, around: GetParent(parent) as isize, class,
                text: String::from_utf16_lossy(&b[..n]), x: r.left, y: r.top, height: r.bottom - r.top, max });
        }
        1
    }
    unsafe extern "system" fn top(h: HWND, out: LPARAM) -> BOOL {
        unsafe { if IsWindowVisible(h) != 0 && is_lightroom(h) { EnumChildWindows(h, Some(child), out); } }
        1
    }
    pub fn children() -> Vec<Win> {
        let mut v: Vec<Win> = vec![];
        unsafe { EnumWindows(Some(top), &mut v as *mut Vec<Win> as LPARAM); }
        v
    }
}
#[cfg(not(windows))]
mod os {
    pub fn usable(_h: isize) -> bool { false }
    pub fn read(_h: isize) -> Option<(i32, i32, i32)> { None }
    pub fn set(_h: isize, _pos: i32) -> bool { false }
    pub fn click(_h: isize) -> Option<bool> { None }
    pub fn children() -> Vec<super::Win> { vec![] }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(h: isize, parent: isize, around: isize, class: &str, text: &str, y: i32, max: i32) -> Win {
        Win { h, parent, around, class: class.into(), text: text.into(), x: if class == BAR { 3586 } else { 3502 }, y, height: 19, max }
    }
    /// groups as read from Lightroom Classic 16 on 2026-11-23
    fn brush() -> Vec<Win> {
        let (s, b) = ("Static", "Button");
        vec![w(1, 50, 5, b, "A", 648, 0), w(2, 50, 5, b, "B", 648, 0), w(3, 50, 5, b, "Erase", 648, 0), w(4, 50, 5, s, "|", 647, 0), w(5, 50, 5, s, "Size", 671, 0), w(6, 50, 5, BAR, "", 674, 1000),
            w(10, 51, 50, s, "Feather", 696, 0), w(11, 51, 50, BAR, "", 698, 1000), w(12, 51, 50, s, "Flow", 720, 0), w(13, 51, 50, BAR, "", 722, 1000),
            w(14, 51, 50, s, "Density", 739, 0), w(15, 51, 50, BAR, "", 742, 1000), w(16, 51, 50, b, "Auto Mask", 764, 0)]
    }
    fn effects() -> Vec<Win> {
        let s = "Static";
        vec![w(100, 90, 9, s, "Grain", 2142, 0), w(101, 90, 9, s, "Size", 2189, 0), w(102, 90, 9, BAR, "", 2191, 100000), w(103, 90, 9, s, "Feather", 2078, 0), w(104, 90, 9, BAR, "", 2080, 100000)]
    }
    fn edge() -> Vec<Win> {
        vec![w(20, 60, 6, "Static", "Feather", 652, 0), w(21, 60, 6, BAR, "", 654, 100000), w(22, 60, 6, "Static", "Edge", 676, 0), w(23, 60, 6, BAR, "", 678, 50000)]
    }
    fn radial() -> Vec<Win> { vec![w(30, 61, 6, "Static", "Feather", 652, 0), w(31, 61, 6, BAR, "", 654, 100000)] }
    fn remove() -> Vec<Win> {
        vec![w(40, 70, 71, "Static", "Opacity", 2802, 0), w(41, 70, 71, BAR, "", 2805, 1000), w(42, 71, 7, "Static", "Feather", 2834, 0), w(43, 71, 7, BAR, "", 2836, 1000)]
    }
    fn blur() -> Vec<Win> {
        // Lens Blur has a brush of its own
        vec![w(80, 85, 8, "Static", "Size", 2417, 0), w(81, 85, 8, BAR, "", 2419, 1000), w(82, 86, 85, "Static", "Feather", 2442, 0), w(83, 86, 85, BAR, "", 2444, 1000),
            w(84, 86, 85, "Static", "Flow", 2466, 0), w(85, 86, 85, BAR, "", 2468, 1000), w(86, 86, 85, "Button", "Auto Mask", 2495, 0)]
    }
    fn find(id: &str, all: &[Win]) -> Option<isize> { find_slider(all, SLIDERS.iter().find(|s| s.id == id).unwrap().places) }
    fn button(id: &str, all: &[Win]) -> Option<isize> { find_button(all, &BUTTONS.iter().find(|s| s.id == id).unwrap().place) }
    fn with(a: Vec<Win>) -> Vec<Win> { [effects(), blur(), a].concat() }

    #[test]
    fn finds_the_right_group() {
        let b = with(brush());
        assert_eq!(["mask.size", "mask.feather", "mask.flow", "mask.density", "mask.edge", "remove.opacity"].map(|i| find(i, &b)), [Some(6), Some(11), Some(13), Some(15), None, None]);
        let e = with(edge());
        assert_eq!(["mask.size", "mask.feather", "mask.edge"].map(|i| find(i, &e)), [None, Some(21), Some(23)]);
        assert_eq!(find("mask.feather", &with(radial())), Some(31));
        let r = with(remove());
        assert_eq!(["remove.opacity", "mask.feather", "mask.size"].map(|i| find(i, &r)), [Some(41), None, None], "Remove's feather is not a mask's");
        let obj = vec![w(1, 50, 5, "Static", "Size", 698, 0), w(2, 50, 5, BAR, "", 700, 100000), w(3, 5, 4, "Static", "Mode :", 660, 0)];
        assert_eq!(find("mask.size", &with(obj.clone())), Some(2));
        assert_eq!(find("mask.size", &with(obj[..2].to_vec())), None, "a Size on its own could be anything");
        assert_eq!(find("mask.refine", &with(vec![w(1, 50, 5, "Static", "Refine", 652, 0), w(2, 50, 5, BAR, "", 654, 100000)])), Some(2));
        for s in &SLIDERS { assert_eq!(find(s.id, &with(vec![])), None, "{}", s.id); }
    }

    #[test]
    fn finds_the_right_button() {
        let b = with(brush());
        assert_eq!(["maskAutoMask", "maskBrushA", "maskBrushB", "maskErase", "maskLuminanceMap"].map(|i| button(i, &b)), [Some(16), Some(1), Some(2), Some(3), None]);
        assert_eq!(button("maskAutoMask", &with(vec![])), None, "not the one of Lens Blur");
        let two = [brush(), brush()].concat();
        assert_eq!(button("maskErase", &two), None, "not sure which: none");
    }

    #[test]
    fn slow_turns_add_up_and_stay_in_range() {
        let t = Instant::now();
        let mut own = None;
        for _ in 0..10 { let v = next(own, 120.0, &[Change::By(0.04)], 1.0, 1000.0, 10.0, t); own = Some((v, t)); }
        assert!((own.unwrap().0 - 124.0).abs() < 1e-9, "ten times 0.04 = 0.4 = 4 steps");
        assert_eq!(next(own, 300.0, &[Change::By(1.0)], 1.0, 1000.0, 10.0, t + Duration::from_secs(2)), 310.0, "after a rest from the slider");
        assert_eq!(next(None, 120.0, &[Change::By(-500.0), Change::By(1.0)], 1.0, 1000.0, 10.0, t), 11.0);
        assert_eq!(next(None, 120.0, &[Change::To(25.0)], 1.0, 1000.0, 10.0, t), 250.0);
        assert_eq!(next(None, 120.0, &[Change::To(0.0)], 10.0, 1000.0, 10.0, t), 10.0, "flow can't be 0");
        assert_eq!(next(None, 0.0, &[Change::By(-3.0)], -50000.0, 50000.0, scale(50000), t), -3000.0, "edge");
    }

    #[test]
    fn only_its_own_settings() {
        let s = Sliders::default();
        assert_eq!((s.send("Exposure2012", Change::By(1.0)), s.press("undo")), (None, None));
        assert_eq!((s.send("mask.size", Change::By(1.0)), s.press("maskErase")), (Some(false), Some(false)), "not started");
    }
}

#[cfg(all(test, windows))]
mod live {
    /// `cargo test -p micropanel-lightroom live -- --ignored --nocapture`: what is on screen now. Reads only.
    #[test]
    #[ignore]
    fn live_find() {
        let t = std::time::Instant::now();
        let all = super::os::children();
        println!("{} windows, {:?}", all.len(), t.elapsed());
        for s in &super::SLIDERS { println!("{} {:?}", s.id, super::find_slider(&all, s.places).and_then(super::os::read)); }
        for b in &super::BUTTONS { println!("{} {:?}", b.id, super::find_button(&all, &b.place).is_some()); }
    }
}
