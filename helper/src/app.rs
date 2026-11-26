//! The running helper: panel thread, logic thread (context, engine, LEDs, profile reload,
//! Resolve yield), the mapping-editor HTTP server.

use crate::engine::{Engine, Learn, Out};
use crate::keys::Combo;
use crate::panel::{Event, HidApiBackend, Panel};
use crate::profiles::{Kind, Profiles, default_toml, ACTIONS};
use crate::util::log;
use crate::sys;
use serde_json::json;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

pub const EDITOR_PORT: u16 = 58790;

#[derive(Default, Clone, serde::Serialize)]
pub struct Shared {
    pub context: Vec<String>,
    pub layers: Vec<String>,
    pub paused: bool,
    pub yielded_to_resolve: bool,
    pub panel_link: Option<String>,
    pub battery: Option<u8>,
    pub charging: bool,
    pub last: Option<Learn>,
    pub profile_error: Option<String>,
    pub profile_path: String,
    pub foreground: String,
    /// context of the last app in front that isn't the mapping editor (its "follow" target)
    pub follow_context: Vec<String>,
    pub events: u64,
    #[serde(skip)] pub quit: bool,
    #[serde(skip)] pub reload: bool,
    /// editor's backlight slider while dragging (not saved yet)
    #[serde(skip)] pub preview_backlight: Option<u8>,
    /// backlight and LEDs are off after the idle timeout
    pub dimmed: bool,
    /// on battery with nobody around: the panel is left to switch itself off
    pub resting: bool,
}

pub struct Opts { pub dry: bool, pub transport: String, pub trace: bool, pub stop_file: Option<PathBuf> }

enum PCmd { Leds(u64), Backlight(u8), Rest(bool), Release, Resume, Quit }

pub fn profile_path() -> PathBuf { crate::util::config_dir().join("profiles.toml") }

fn load_profiles(path: &PathBuf) -> Result<Profiles, String> {
    if !path.exists() {
        std::fs::write(path, default_toml()).map_err(|e| format!("can't write {}: {e}", path.display()))?;
        log(&format!("wrote default profiles to {}", path.display()));
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("can't read {}: {e}", path.display()))?;
    Profiles::parse(&text)
}
fn mtime(p: &PathBuf) -> Option<SystemTime> { std::fs::metadata(p).and_then(|m| m.modified()).ok() }

struct RealOut { dry: bool, smooth: Arc<Smoother>, shared: Arc<Mutex<Shared>> }

/// Held by everything that rewrites profiles.toml, so an edit and a preset switch can't overlap.
static EDIT_LOCK: Mutex<()> = Mutex::new(());
fn edit_lock() -> std::sync::MutexGuard<'static, ()> { EDIT_LOCK.lock().unwrap_or_else(|e| e.into_inner()) }

/// Pointer smoothing: the panel sends ball movement in bursts ~every 40 ms (logs/ball_rate_*),
/// so each burst is spread over `window` ms in 4 ms steps by a small timer thread.
pub struct Smoother { state: Mutex<(f64, f64, f64, f64)>, pub window_ms: std::sync::atomic::AtomicU32 }
const TICK_MS: u64 = 4;
impl Smoother {
    fn start(window_ms: u32) -> Arc<Smoother> {
        let s = Arc::new(Smoother { state: Mutex::new((0.0, 0.0, 0.0, 0.0)), window_ms: window_ms.into() });
        let t = s.clone();
        std::thread::Builder::new().name("pointer".into()).spawn(move || loop {
            std::thread::sleep(Duration::from_millis(TICK_MS));
            let w = t.window_ms.load(std::sync::atomic::Ordering::Relaxed).max(TICK_MS as u32) as f64;
            let (mx, my) = {
                let mut g = t.state.lock().unwrap();
                let (px, py, cx, cy) = *g;
                if px == 0.0 && py == 0.0 { continue; }
                // emit a fixed share of what's pending: drains a burst in about `w` ms
                let k = (TICK_MS as f64 / w).min(1.0) * 1.6;
                let (ex, ey) = (px * k.min(1.0), py * k.min(1.0));
                let (ex, ey) = (if ex.abs() < 1.0 { px.clamp(-1.0, 1.0) } else { ex }, if ey.abs() < 1.0 { py.clamp(-1.0, 1.0) } else { ey });
                let (fx, fy) = (cx + ex, cy + ey);
                let (ix, iy) = (fx.trunc(), fy.trunc());
                *g = (px - ex, py - ey, fx - ix, fy - iy);
                if g.0.abs() < 1e-6 { g.0 = 0.0; }
                if g.1.abs() < 1e-6 { g.1 = 0.0; }
                (ix as i32, iy as i32)
            };
            if mx != 0 || my != 0 { sys::mouse_move(mx, my); }
        }).unwrap();
        s
    }
    fn push(&self, dx: i32, dy: i32) {
        let mut g = self.state.lock().unwrap();
        g.0 += dx as f64;
        g.1 += dy as f64;
    }
}
impl Out for RealOut {
    fn key(&mut self, c: &Combo, d: bool, u: bool) {
        if self.dry { log(&format!("dry: key {c:?} down={d} up={u}")); } else { sys::key(c, d, u); }
    }
    fn mouse_move(&mut self, dx: i32, dy: i32) {
        if self.dry { log(&format!("dry: move {dx} {dy}")); }
        else if self.smooth.window_ms.load(std::sync::atomic::Ordering::Relaxed) == 0 { sys::mouse_move(dx, dy); }
        else { self.smooth.push(dx, dy); }
    }
    fn scroll(&mut self, v: i32, h: i32, mods: &[u16]) {
        if self.dry { log(&format!("dry: scroll {v} {h} mods {mods:?}")); } else { sys::scroll(v, h, mods); }
    }
    fn pause(&mut self) { if !self.dry { std::thread::sleep(Duration::from_millis(20)); } }
    fn mouse_button(&mut self, b: u8, d: bool) {
        if self.dry { log(&format!("dry: mouse button {b} {d}")); } else { sys::mouse_button(b, d); }
    }
    fn ext(&mut self, id: &str, m: serde_json::Value) -> bool {
        if self.dry { log(&format!("dry: {id} {m}")); return true; }
        crate::ext::by_id(id).map_or(false, |i| i.send(&m))
    }
    fn midi(&mut self, m: [u8; 3]) -> bool {
        if self.dry { log(&format!("dry: midi {:02x} {} {}", m[0], m[1], m[2])); true } else { crate::midi::send(m) }
    }
    fn sheet(&mut self, on: Option<bool>) { crate::sheet::switch(on); }
    fn preset(&mut self, app: &str, global: bool, name: &str) {
        if self.dry { log(&format!("dry: preset {app} global={global} {name}")); return; }
        // files: not on the event thread
        let (app, name, shared) = (app.to_string(), name.to_string(), self.shared.clone());
        let r = std::thread::Builder::new().name("preset".into()).spawn(move || {
            let _g = edit_lock();
            match crate::presets::Store::real().by_key(if global { "global" } else { "app" }, &app, &name) {
                Ok(t) => { log(&format!("presets: {t} (panel key)")); crate::sheet::notice(&t); shared.lock().unwrap().reload = true; }
                Err(e) => { log(&format!("presets: {e} (panel key)")); crate::sheet::notice(&e); }
            }
        });
        if let Err(e) = r { log(&format!("presets: {e}")); }
    }
}

/// Keys that restart the helper in the place in front; written by the main loop, read by the panel's thread.
static RESTART_KEYS: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// Keys from Settings that restart the helper when held together, and for how long; from the main loop.
static RESTART_CHORD: Mutex<(Vec<String>, Duration)> = Mutex::new((Vec::new(), Duration::from_secs(2)));
const RESTART_HOLD: Duration = Duration::from_secs(1);
/// When the main loop last ran (ms since START); the panel's thread notices when it stops.
static BEAT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
fn now_ms() -> u64 { START.get_or_init(Instant::now).elapsed().as_millis() as u64 }

/// Start a new helper that takes over once this one has gone, and go. Ok = this process is about to end.
pub fn restart(why: &str) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    log(&format!("restarting ({why})"));
    std::process::Command::new(exe).args(["--after", &std::process::id().to_string()]).spawn().map_err(|e| { log(&format!("restart failed: {e}")); e.to_string() })?;
    Ok(())
}

fn panel_thread(transport: String, backlight: u8, tx: Sender<Event>, rx: Receiver<PCmd>) {
    let mut p = Panel::new(Box::new(HidApiBackend::new()), &transport, backlight);
    let mut announced = false;
    let mut held: Option<(String, Instant)> = None;
    // keys pressed since we run (a key held through a restart doesn't count until it is pressed again)
    let mut down: std::collections::HashSet<String> = Default::default();
    let mut chord_since: Option<Instant> = None;
    let mut stuck_since: Option<u64> = None;
    loop {
        // the main loop stopped: say so once, and again when it goes on
        let beat = BEAT.load(std::sync::atomic::Ordering::Relaxed);
        let late = now_ms().saturating_sub(beat);
        match stuck_since {
            None if beat > 0 && late > 5000 => { log(&format!("the helper's main loop hasn't run for {:.0} s (hanging)", late as f64 / 1000.0)); stuck_since = Some(beat); }
            Some(b) if beat != b => { log("the main loop runs again"); stuck_since = None; }
            _ => {}
        }
        let (chord, hold) = RESTART_CHORD.lock().unwrap().clone();
        let together = !chord.is_empty() && chord.iter().all(|k| down.contains(k));
        if !together { chord_since = None; } else if chord_since.is_none() { chord_since = Some(Instant::now()); }
        if held.as_ref().is_some_and(|h| h.1.elapsed() >= RESTART_HOLD) || chord_since.is_some_and(|t| t.elapsed() >= hold) {
            held = None;
            chord_since = None;
            p.release(true);
            if restart("panel keys").is_ok() { std::process::exit(0); }
        }
        while let Ok(c) = rx.try_recv() {
            match c {
                PCmd::Leds(m) => { if m != p.leds || !p.is_up() { p.set_leds(m); } }
                PCmd::Backlight(b) => { p.set_backlight(b); }
                PCmd::Release => { for e in p.release(true) { let _ = tx.send(e); } let _ = tx.send(Event::Link { transport: "released".into(), up: false, detail: String::new() }); }
                PCmd::Rest(on) => {
                    p.set_resting(on);
                    // it reports a new level only while awake
                    if !on { if let Some((c, l)) = p.battery() { let _ = tx.send(Event::Battery { charging: c, level: l }); } }
                }
                PCmd::Resume => p.resume(),
                PCmd::Quit => { p.release(true); log(&format!("panel stats: {:?}", p.stats)); return; }
            }
        }
        if !announced && !p.is_up() && p.active {
            log("panel: looking for the Micro Color Panel (USB or Bluetooth)");
            announced = true;
        }
        for e in p.poll(20) {
            if let Event::Button { name, down, synthetic: false, .. } = &e {
                if *down && RESTART_KEYS.lock().unwrap().contains(name) { held = Some((name.clone(), Instant::now())); }
                else if !*down && held.as_ref().is_some_and(|h| &h.0 == name) { held = None; }
            }
            match &e {
                Event::Button { name, down: d, synthetic: false, .. } => { if *d { down.insert(name.clone()); } else { down.remove(name); } }
                Event::Button { name, down: false, .. } => { down.remove(name); }
                Event::Link { up: false, .. } => down.clear(),
                _ => {}
            }
            if let Event::Link { up: true, .. } = &e {
                announced = false;
                let (ver, _) = p.info();
                log(&format!("panel: firmware {}", ver.unwrap_or_else(|| "?".into())));
                if let Some((c, l)) = p.battery() { let _ = tx.send(Event::Battery { charging: c, level: l }); }
            }
            if tx.send(e).is_err() { return; }
        }
    }
}

/// Should the panel be left alone so that it switches itself off? (`after` 0 = never)
fn should_rest(after: Duration, resting: bool, link: Option<&str>, busy: bool, panel_idle: Duration, pc_idle: Duration) -> bool {
    // a lost link keeps it as it is; the cable charges it
    let battery = link == Some("bt") || (link.is_none() && resting);
    !after.is_zero() && battery && !busy && panel_idle >= after && pc_idle >= after
}

/// Context stack from the foreground window and what its integration says.
fn context_stack(pr: &Profiles, fg: &Option<(String, bool)>) -> Vec<String> {
    let mut v = vec![];
    if let Some((exe, owned)) = fg {
        let app = pr.app_name(exe);
        if app != "default" {
            if *owned {
                v.push(format!("{app}/dialog"));
            } else if let Some(i) = crate::ext::by_id(&app) {
                v.extend(i.places());
            }
            v.push(app);
        }
    }
    v.push("default".into());
    v
}

pub fn run_logic(shared: Arc<Mutex<Shared>>, opts: Opts) {
    BEAT.store(now_ms().max(1), std::sync::atomic::Ordering::Relaxed);
    let path = profile_path();
    shared.lock().unwrap().profile_path = path.display().to_string();
    let profiles = match load_profiles(&path) {
        Ok(p) => p,
        Err(e) => {
            log(&format!("profiles: {e}; using the built-in defaults"));
            shared.lock().unwrap().profile_error = Some(e);
            Profiles::parse(default_toml()).expect("built-in profiles parse")
        }
    };
    let mut seen_mtime = mtime(&path);
    for i in crate::ext::all() { i.configure(&profiles.settings); i.start(); }
    let (etx, erx) = channel::<Event>();
    let (ctx_tx, ctx_rx) = channel::<PCmd>();
    let (tr, bl) = (opts.transport.clone(), profiles.settings.backlight);
    std::thread::Builder::new().name("panel".into()).spawn(move || panel_thread(tr, bl, etx, ctx_rx)).unwrap();
    start_web(EDITOR_PORT, shared.clone());
    crate::sheet::start(shared.clone());

    let mut eng = Engine::new(profiles);
    let smooth = Smoother::start(eng_smoothing(&eng));
    let mut out = RealOut { dry: opts.dry, smooth: smooth.clone(), shared: shared.clone() };
    let (mut t_ctx, mut t_file, mut t_resolve) = (Instant::now(), Instant::now(), Instant::now() - Duration::from_secs(10));
    let mut leds_sent: Option<u64> = None;
    let mut paused = false;
    let mut yielded = false;
    let mut last_stack: Vec<String> = vec![];
    let mut skipped_logged: std::collections::HashMap<&'static str, u64> = Default::default();
    let mut last_touch = Instant::now();
    let mut t_midi = Instant::now() - Duration::from_secs(10);
    let mut midi_rx_seen = 0u64;
    let mut dimmed = false;
    let mut resting = false;
    loop {
        if !paused && !yielded { eng.tick(&mut out); }
        match erx.recv_timeout(Duration::from_millis(10)) {
            Ok(ev) => {
                if matches!(ev, Event::Button { .. } | Event::Knob { .. } | Event::Ring { .. } | Event::Ball { .. }) {
                    last_touch = Instant::now();
                    if dimmed {
                        dimmed = false;
                        let _ = ctx_tx.send(PCmd::Backlight(eng.profiles.settings.backlight));
                        leds_sent = None;
                        shared.lock().unwrap().dimmed = false;
                    }
                }
                let mut sh = shared.lock().unwrap();
                sh.events += 1;
                match &ev {
                    Event::Battery { charging, level } => {
                        if (sh.battery, sh.charging) != (Some(*level), *charging) { log(&format!("battery {level} %{}", if *charging { ", charging" } else { "" })); }
                        sh.battery = Some(*level);
                        sh.charging = *charging;
                    }
                    Event::Link { transport, up, .. } => {
                        sh.panel_link = if *up { Some(transport.clone()) } else { None };
                        if *up { leds_sent = None; }
                    }
                    _ => {}
                }
                drop(sh);
                if !paused && !yielded { eng.handle(&ev, &mut out); }
                if opts.trace {
                    let res = match (&ev, &eng.last) {
                        (Event::Button { down: false, .. }, _) => String::new(),
                        (Event::Button { .. } | Event::Knob { .. } | Event::Ring { .. } | Event::Ball { .. }, Some(l)) =>
                            format!(" -> {} {} {}", l.ctx.as_deref().unwrap_or("-"), l.layer.as_deref().unwrap_or(""), l.binding.as_deref().unwrap_or("unbound")),
                        _ => String::new(),
                    };
                    log(&format!("ev {}{res}", ev.to_json()));
                }
                if let Some(l) = &eng.last { shared.lock().unwrap().last = Some(l.clone()); }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => { log("panel thread ended"); return; }
        }

        if t_midi.elapsed() >= Duration::from_secs(2) {
            t_midi = Instant::now();
            let s = &eng.profiles.settings;
            crate::midi::ensure(&s.midi_out, &s.midi_in);
        }
        // MIDI feedback can change LEDs: re-evaluate soon after something arrives
        let rx = crate::midi::RX_COUNT.load(std::sync::atomic::Ordering::Relaxed);
        if rx != midi_rx_seen { midi_rx_seen = rx; t_ctx = Instant::now() - Duration::from_millis(100); }
        if t_ctx.elapsed() >= Duration::from_millis(100) {
            t_ctx = Instant::now();
            let fg = sys::foreground();
            for i in crate::ext::all() { i.watch(&eng.profiles.watched(i.prefix())); }
            let stack = context_stack(&eng.profiles, &fg);
            if stack != last_stack {
                log(&format!("context: {}", stack.join(" > ")));
                last_stack = stack.clone();
            }
            eng.stack = stack;
            eng.sheet_open = crate::sheet::get().open;
            BEAT.store(now_ms().max(1), std::sync::atomic::Ordering::Relaxed);
            let rk = eng.restart_keys();
            let chord = (eng.profiles.settings.restart_keys.clone(), Duration::from_secs(eng.profiles.settings.restart_hold_s as u64));
            if *RESTART_CHORD.lock().unwrap() != chord { *RESTART_CHORD.lock().unwrap() = chord; }
            if *RESTART_KEYS.lock().unwrap() != rk { *RESTART_KEYS.lock().unwrap() = rk; }
            let m = if paused || yielded || dimmed || resting { 0 } else { eng.leds() };
            if leds_sent != Some(m) && !yielded {
                let _ = ctx_tx.send(PCmd::Leds(m));
                leds_sent = Some(m);
            }
            for (id, n) in &eng.skipped {
                if skipped_logged.get(id) != Some(n) && n % 50 == 1 { log(&format!("{id}: {n} actions skipped so far (not connected, or not allowed right now)")); }
                skipped_logged.insert(id, *n);
            }
            let off_after = eng.profiles.settings.backlight_off_after_s;
            if !dimmed && off_after > 0 && last_touch.elapsed() >= Duration::from_secs(off_after as u64) && !yielded {
                dimmed = true;
                let _ = ctx_tx.send(PCmd::Backlight(0));
                let _ = ctx_tx.send(PCmd::Leds(0));
                leds_sent = Some(0);
                log(&format!("panel idle for {off_after} s: lights off until the next touch"));
            }
            // on battery, and neither the panel nor the PC is being used
            let rest_after = Duration::from_secs(eng.profiles.settings.sleep_after_min as u64 * 60);
            let link = shared.lock().unwrap().panel_link.clone();
            let want = should_rest(rest_after, resting, link.as_deref(), paused || yielded, last_touch.elapsed(), sys::idle_for());
            if want != resting {
                resting = want;
                let _ = ctx_tx.send(PCmd::Rest(want));
                leds_sent = None;
                log(if want { "nobody around and on battery: letting the panel rest" } else { "waking the panel" });
            }
            // Never ask Windows about a window while holding `shared`: a window of our own (the tray's menu) is
            // answered by a thread that may be waiting for `shared` itself, and both would wait for ever.
            let editor_in_front = sys::foreground_title().starts_with("Micro Color Panel");
            let mut sh = shared.lock().unwrap();
            sh.dimmed = dimmed;
            sh.resting = resting;
            if let Some(b) = sh.preview_backlight.take() {
                if !dimmed && !resting { let _ = ctx_tx.send(PCmd::Backlight(b)); }
            }
            sh.context = eng.stack.clone();
            sh.layers = eng.layers.clone();
            sh.foreground = fg.map(|f| f.0).unwrap_or_default();
            // the editor's own browser tab is titled "Micro Color Panel"; don't follow it
            if !editor_in_front || sh.follow_context.is_empty() {
                sh.follow_context = eng.stack.clone();
            }
            let quit_flag = crate::util::config_dir().join("QUIT");
            if quit_flag.exists() {
                let _ = std::fs::remove_file(&quit_flag);
                log("quit requested (micropanel --quit)");
                sh.quit = true;
            }
            if opts.stop_file.as_ref().map_or(false, |f| f.exists()) {
                sh.quit = true;
                if let Some(f) = &opts.stop_file { let _ = std::fs::remove_file(f); }
            }
            if sh.quit {
                drop(sh);
                eng.release_all(&mut out);
                let _ = ctx_tx.send(PCmd::Quit);
                std::thread::sleep(Duration::from_millis(300));
                return;
            }
            if sh.paused != paused {
                paused = sh.paused;
                drop(sh);
                eng.release_all(&mut out);
                let _ = ctx_tx.send(if paused { PCmd::Release } else { PCmd::Resume });
                leds_sent = None;
                log(if paused { "paused" } else { "resumed" });
            } else {
                let reload = std::mem::take(&mut sh.reload);
                drop(sh);
                if reload { seen_mtime = None; t_file = Instant::now() - Duration::from_secs(5); }
            }
        }

        if t_file.elapsed() >= Duration::from_secs(1) {
            t_file = Instant::now();
            let m = mtime(&path);
            if m != seen_mtime {
                seen_mtime = m;
                match load_profiles(&path) {
                    Ok(p) => {
                        eng.release_all(&mut out);
                        let bl = p.settings.backlight;
                        if bl != eng.profiles.settings.backlight && !dimmed { let _ = ctx_tx.send(PCmd::Backlight(bl)); }
                        smooth.window_ms.store(p.settings.pointer_smoothing_ms, std::sync::atomic::Ordering::Relaxed);
                        for i in crate::ext::all() { i.configure(&p.settings); }
                        eng.set_profiles(p);
                        shared.lock().unwrap().profile_error = None;
                        log("profiles reloaded");
                    }
                    Err(e) => {
                        log(&format!("profiles: {e} (keeping the previous profiles)"));
                        shared.lock().unwrap().profile_error = Some(e);
                    }
                }
            }
        }

        if t_resolve.elapsed() >= Duration::from_secs(2) {
            t_resolve = Instant::now();
            let want = eng.profiles.settings.resolve_yield && sys::process_running("resolve.exe");
            if want != yielded {
                yielded = want;
                eng.release_all(&mut out);
                if yielded {
                    log("DaVinci Resolve is running: releasing the panel to it");
                    let _ = ctx_tx.send(PCmd::Release);
                } else {
                    log("DaVinci Resolve quit: taking the panel back");
                    if !paused { let _ = ctx_tx.send(PCmd::Resume); }
                }
                leds_sent = None;
                shared.lock().unwrap().yielded_to_resolve = yielded;
            }
        }
    }
}

fn eng_smoothing(e: &Engine) -> u32 { e.profiles.settings.pointer_smoothing_ms }

// ------------------------------------------------------------------ editor HTTP server

const EDITOR_HTML: &str = include_str!("editor.html");

/// Changes when the editor page or an integration's script does, so that a page left open over an
/// update can load the new one.
fn editor_build() -> &'static str {
    static B: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    B.get_or_init(|| {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (EDITOR_HTML, crate::ext::scripts()).hash(&mut h);
        format!("{:016x}", h.finish())
    })
}

/// Only the mapping editor (no panel, integrations not started), for trying the editor
/// next to a running helper: `micropanel --editor-only --port 58791`. False if the port is taken.
pub fn editor_only(port: u16, ctx: Option<String>, layers: Vec<String>, connect: bool) -> bool {
    let shared: Arc<Mutex<Shared>> = Default::default();
    // a first profile, as a normal start writes it
    if let Err(e) = load_profiles(&profile_path()) { log(&format!("profiles: {e}")); }
    // a made-up place, to try the cheat sheet without the panel
    if let Some(c) = ctx {
        let mut s = shared.lock().unwrap();
        s.context = crate::edit::stack_for(&c);
        s.layers = layers;
    }
    if connect {
        let load = || if let Ok(p) = load_profiles(&profile_path()) { for i in crate::ext::all() { i.configure(&p.settings); } };
        load();
        for i in crate::ext::all() { i.start(); }
        let sh = shared.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(200));
            if std::mem::take(&mut sh.lock().unwrap().reload) { load(); }
        });
    }
    crate::sheet::start(shared.clone());
    start_web(port, shared)
}

fn start_web(port: u16, shared: Arc<Mutex<Shared>>) -> bool {
    let server = match tiny_http::Server::http(("127.0.0.1", port)) {
        Ok(s) => s,
        Err(e) => { log(&format!("editor: can't listen on 127.0.0.1:{port}: {e}")); return false; }
    };
    log(&format!("editor: http://127.0.0.1:{port}/"));
    std::thread::Builder::new().name("web".into()).spawn(move || {
        for mut req in server.incoming_requests() {
            let full = req.url().to_string();
            let url = full.split('?').next().unwrap_or("").to_string();
            let method = req.method().as_str().to_string();
            // Only same-machine pages may write: refuse cross-site requests (DNS-rebinding/CSRF).
            let host_ok = req.headers().iter().find(|h| h.field.equiv("Host"))
                .map(|h| { let v = h.value.as_str(); v == format!("127.0.0.1:{port}") || v == format!("localhost:{port}") })
                .unwrap_or(false);
            let origin_ok = method == "GET" || req.headers().iter().find(|h| h.field.equiv("Origin"))
                .map(|h| { let v = h.value.as_str(); v == format!("http://127.0.0.1:{port}") || v == format!("http://localhost:{port}") })
                .unwrap_or(true);
            if !host_ok || !origin_ok {
                let _ = req.respond(tiny_http::Response::from_string("not allowed").with_status_code(403));
                continue;
            }
            let mut body = String::new();
            if method != "GET" { let _ = req.as_reader().read_to_string(&mut body); }
            // A failing request must not take the editor down with it.
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| route(&method, &url, &full, &body, &shared)));
            let (code, ctype, text) = match r {
                Ok(x) => x,
                Err(_) => (500, "application/json", json!({"ok": false, "error": format!("internal error in {method} {url}, see helper.log")}).to_string()),
            };
            let h = tiny_http::Header::from_bytes(&b"Content-Type"[..], ctype.as_bytes()).unwrap();
            let nc = tiny_http::Header::from_bytes(&b"Cache-Control"[..], &b"no-store"[..]).unwrap();
            let nf = tiny_http::Header::from_bytes(&b"X-Frame-Options"[..], &b"SAMEORIGIN"[..]).unwrap();
            let _ = req.respond(tiny_http::Response::from_string(text).with_status_code(code).with_header(h).with_header(nc).with_header(nf));
        }
    }).unwrap();
    true
}

fn route(method: &str, url: &str, full_url: &str, body: &str, shared: &Arc<Mutex<Shared>>) -> (u16, &'static str, String) {
    const J: &str = "application/json";
    let _edit = (method == "PUT" && !matches!(url, "/api/keyrec" | "/api/sheet" | "/api/backlight_preview")).then(edit_lock);
    match (method, url) {
        ("GET", "/") => (200, "text/html; charset=utf-8", EDITOR_HTML.to_string()),
        ("GET", "/api/state") => {
            let sh = shared.lock().unwrap().clone();
            let ext: serde_json::Map<String, serde_json::Value> = crate::ext::all().iter().map(|i| (i.id().to_string(), i.status())).collect();
            let last = sh.last.as_ref().map(|x| json!({
                "control": x.control, "at": x.at,
                "resolved": x.ctx.as_ref().map(|c| json!({"ctx": c, "layer": x.layer,
                    "binding": x.binding.as_ref().map(|b| toml_inline_to_json(b))}))
            }));
            (200, J, json!({
                "context": sh.context, "follow_context": sh.follow_context, "layers": sh.layers, "paused": sh.paused || sh.yielded_to_resolve,
                "panel": {"resting": sh.resting, "link": sh.panel_link, "battery": sh.battery, "charging": sh.charging},
                "ext": ext,
                "build": editor_build(), "sheet": crate::sheet::state_json(), "preset_rev": crate::presets::REV.load(std::sync::atomic::Ordering::Relaxed), "last": last, "profile_error": sh.profile_error, "profile_path": sh.profile_path,
            }).to_string())
        }
        ("GET", "/api/profiles") => (200, "text/plain; charset=utf-8", read_profiles()),
        ("POST", "/api/profiles/check") => (200, J, check_json(body)),
        ("PUT", "/api/profiles") => {
            match Profiles::parse(body) {
                Ok(_) => match crate::presets::write_safe(&profile_path(), body) {
                    Ok(()) => { shared.lock().unwrap().reload = true; (200, J, json!({"ok": true}).to_string()) }
                    Err(e) => (200, J, json!({"ok": false, "error": format!("can't save: {e}")}).to_string()),
                },
                Err(e) => (200, J, json!({"ok": false, "error": e}).to_string()),
            }
        }
        ("GET", "/api/model") => match Profiles::parse(&read_profiles()) {
            Ok(p) => (200, J, crate::edit::model(&p).to_string()),
            Err(e) => (200, J, json!({"error": e}).to_string()),
        },
        ("GET", u) if u.starts_with("/api/effective") => {
            let q = query(full_url);
            let ctx = q.get("ctx").cloned().unwrap_or_else(|| "default".into());
            let layers: Vec<String> = q.get("layers").map(|s| s.split(',').filter(|x| !x.is_empty()).map(String::from).collect()).unwrap_or_default();
            match Profiles::parse(&read_profiles()) {
                Ok(p) => (200, J, crate::edit::effective(&p, &ctx, &layers).to_string()),
                Err(e) => (200, J, json!({"error": e}).to_string()),
            }
        }
        ("PUT", "/api/binding") => {
            let r = serde_json::from_str::<serde_json::Value>(body).map_err(|e| e.to_string()).and_then(|v| {
                let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
                let b = if v["binding"].is_null() { None } else { Some(v["binding"].clone()) };
                let text = profiles_for_edit()?;
                let out = crate::edit::set_binding(&text, &s("ctx"), &s("layer"), &s("kind"), &s("name"), b.as_ref())?;
                crate::presets::write_safe(&profile_path(), &out)?;
                log(&format!("editor: {} {}{}.{} -> {}", s("ctx"), if s("layer").is_empty() { String::new() } else { format!("[{}] ", s("layer")) },
                    s("kind"), s("name"), b.map(|x| x.to_string()).unwrap_or_else(|| "cleared".into())));
                Ok(())
            });
            match r {
                Ok(()) => { shared.lock().unwrap().reload = true; (200, J, json!({"ok": true}).to_string()) }
                Err(e) => (200, J, json!({"ok": false, "error": e}).to_string()),
            }
        }
        // several changes as one step; the answer's id undoes exactly that step
        ("PUT", "/api/edit") => {
            let r = serde_json::from_str::<serde_json::Value>(body).map_err(|e| e.to_string()).and_then(|v| {
                let items = v["items"].as_array().cloned().unwrap_or_default();
                let before = profiles_for_edit()?;
                let after = crate::edit::apply_batch(&before, &items)?;
                crate::presets::write_safe(&profile_path(), &after)?;
                for i in items.iter().take(40) { log(&format!("editor: {i}")); }
                if items.len() > 40 { log(&format!("editor: and {} more", items.len() - 40)); }
                Ok(remember_edit(before, after))
            });
            match r {
                Ok(id) => { shared.lock().unwrap().reload = true; (200, J, json!({"ok": true, "id": id}).to_string()) }
                Err(e) => (200, J, json!({"ok": false, "error": e}).to_string()),
            }
        }
        ("PUT", "/api/edit_undo") => {
            let r = serde_json::from_str::<serde_json::Value>(body).map_err(|e| e.to_string()).and_then(|v| {
                let ids: Vec<u64> = v["ids"].as_array().map(|a| a.iter().filter_map(|x| x.as_u64()).collect()).unwrap_or_default();
                let text = retrace(&ids, v["dir"] == "redo", &profiles_for_edit()?)?;
                crate::presets::write_safe(&profile_path(), &text)?;
                log(&format!("editor: {} {ids:?}", if v["dir"] == "redo" { "redo" } else { "undo" }));
                Ok(())
            });
            match r {
                Ok(()) => { shared.lock().unwrap().reload = true; (200, J, json!({"ok": true}).to_string()) }
                Err(e) => (200, J, json!({"ok": false, "error": e}).to_string()),
            }
        }
        ("PUT", "/api/led") => {
            let r = serde_json::from_str::<serde_json::Value>(body).map_err(|e| e.to_string()).and_then(|v| {
                let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
                let rule = v["rule"].as_str().map(String::from).filter(|x| !x.trim().is_empty());
                let out = crate::edit::set_led(&profiles_for_edit()?, &s("ctx"), &s("led"), rule.as_deref())?;
                crate::presets::write_safe(&profile_path(), &out)?;
                log(&format!("editor: {} LED {} -> {}", s("ctx"), s("led"), rule.unwrap_or_else(|| "cleared".into())));
                Ok(())
            });
            match r {
                Ok(()) => { shared.lock().unwrap().reload = true; (200, J, json!({"ok": true}).to_string()) }
                Err(e) => (200, J, json!({"ok": false, "error": e}).to_string()),
            }
        }
        ("PUT", "/api/layer") => {
            let r = serde_json::from_str::<serde_json::Value>(body).map_err(|e| e.to_string()).and_then(|v| {
                let (name, on) = (v["name"].as_str().unwrap_or(""), v["inherit"].as_bool().unwrap_or(false));
                let out = crate::edit::set_layer_inherit(&profiles_for_edit()?, name, on)?;
                crate::presets::write_safe(&profile_path(), &out)?;
                log(&format!("editor: layer {name}: unset controls {}", if on { "keep their normal job" } else { "do nothing" }));
                Ok(())
            });
            match r {
                Ok(()) => { shared.lock().unwrap().reload = true; (200, J, json!({"ok": true}).to_string()) }
                Err(e) => (200, J, json!({"ok": false, "error": e}).to_string()),
            }
        }
        ("PUT", "/api/setting") => {
            let r = serde_json::from_str::<serde_json::Value>(body).map_err(|e| e.to_string()).and_then(|v| {
                let key = v["key"].as_str().unwrap_or("").to_string();
                let out = crate::edit::set_setting(&profiles_for_edit()?, &key, &v["value"])?;
                crate::presets::write_safe(&profile_path(), &out)?;
                log(&format!("editor: settings.{key} = {}", v["value"]));
                Ok(())
            });
            match r {
                Ok(()) => { shared.lock().unwrap().reload = true; (200, J, json!({"ok": true}).to_string()) }
                Err(e) => (200, J, json!({"ok": false, "error": e}).to_string()),
            }
        }
        ("GET", "/api/apps") => match Profiles::parse(&read_profiles()) {
            Ok(p) => {
                let v: Vec<_> = sys::running_apps().into_iter().filter(|e| e != "micropanel.exe").map(|e| {
                    let ctx = p.app_name(&e);
                    let known = p.apps.contains_key(&e) || p.ctxs.keys().any(|c| c.split('/').next() == Some(ctx.as_str()));
                    let name = if known { ctx } else { crate::edit::app_slug(&e) };
                    json!({"exe": e, "ctx": name, "label": crate::edit::app_label(&name), "known": known})
                }).collect();
                (200, J, json!({"apps": v}).to_string())
            }
            Err(e) => (200, J, json!({"error": e}).to_string()),
        },
        ("PUT", "/api/app") => {
            let r = serde_json::from_str::<serde_json::Value>(body).map_err(|e| e.to_string()).and_then(|v| {
                let name = v["name"].as_str().unwrap_or("").to_string();
                let out = crate::edit::set_app(&profiles_for_edit()?, &name, v["exe"].as_str())?;
                crate::presets::write_safe(&profile_path(), &out)?;
                log(&format!("editor: app {name} -> {}", v["exe"].as_str().unwrap_or("removed")));
                Ok(())
            });
            match r {
                Ok(()) => { shared.lock().unwrap().reload = true; (200, J, json!({"ok": true}).to_string()) }
                Err(e) => (200, J, json!({"ok": false, "error": e}).to_string()),
            }
        }
        ("PUT", "/api/backlight_preview") => {
            let v = serde_json::from_str::<serde_json::Value>(body).ok().and_then(|v| v["value"].as_u64()).filter(|b| *b <= 100);
            match v {
                Some(b) => { shared.lock().unwrap().preview_backlight = Some(b as u8); (200, J, json!({"ok": true}).to_string()) }
                None => (200, J, json!({"ok": false, "error": "value 0..100"}).to_string()),
            }
        }
        ("GET", "/api/presets") => match crate::presets::Store::real().describe() {
            Ok(v) => (200, J, v.to_string()),
            Err(e) => (200, J, json!({"error": e}).to_string()),
        },
        ("PUT", "/api/presets") => {
            let r = serde_json::from_str::<serde_json::Value>(body).map_err(|e| e.to_string()).and_then(|v| crate::presets::api(&crate::presets::Store::real(), &v));
            match r {
                Ok(v) => { shared.lock().unwrap().reload = true; (200, J, json!({"ok": true, "result": v}).to_string()) }
                Err(e) => (200, J, json!({"ok": false, "error": e}).to_string()),
            }
        }
        ("GET", "/api/preset") => {
            let q = query(full_url);
            let g = |k: &str| q.get(k).cloned().unwrap_or_default();
            match crate::presets::Store::real().export(&g("app"), &g("id")) {
                Ok(t) => (200, "text/plain; charset=utf-8", t),
                Err(e) => (404, "text/plain; charset=utf-8", e),
            }
        }
        ("GET", "/api/old_profiles") => (200, J, crate::presets::old_list(&crate::presets::Store::real(), query(full_url).get("app").map(String::as_str).unwrap_or("")).to_string()),
        ("PUT", "/api/sheet") => match serde_json::from_str::<serde_json::Value>(body).map_err(|e| e.to_string()).and_then(|v| crate::sheet::apply(&v)) {
            Ok(_) => (200, J, json!({"ok": true, "sheet": crate::sheet::state_json()}).to_string()),
            Err(e) => (200, J, json!({"ok": false, "error": e}).to_string()),
        },
        ("GET", "/api/midi") => (200, J, crate::midi::status().to_string()),
        ("PUT", "/api/keyrec") => { crate::sys::keyrec::start(Duration::from_secs(15)); (200, J, json!({"ok": true}).to_string()) }
        ("DELETE", "/api/keyrec") => { crate::sys::keyrec::cancel(); (200, J, json!({"ok": true}).to_string()) }
        ("GET", "/api/keyrec") => {
            use crate::sys::RecState::*;
            let v = match crate::sys::keyrec::state() {
                Idle => json!({"state": "idle"}),
                Waiting => json!({"state": "waiting"}),
                Done(c) if c.starts_with("unsupported") => json!({"state": "error", "error": c}),
                Done(c) => json!({"state": "done", "combo": c}),
                TimedOut => json!({"state": "timeout"}),
            };
            (200, J, v.to_string())
        }
        ("GET", "/ext.js") => (200, "text/javascript; charset=utf-8", crate::ext::scripts()),
        ("GET", u) if u.starts_with("/api/ext/") => {
            let (id, path) = u["/api/ext/".len()..].split_once('/').unwrap_or((&u["/api/ext/".len()..], ""));
            match crate::ext::by_id(id).and_then(|i| i.get(path)) {
                Some(v) => (200, J, v.to_string()),
                None => (404, "text/plain", "not found".into()),
            }
        }
        ("PUT", u) if u.starts_with("/api/ext/") => {
            let (id, path) = u["/api/ext/".len()..].split_once('/').unwrap_or((&u["/api/ext/".len()..], ""));
            let v = serde_json::from_str(body).unwrap_or(serde_json::Value::Null);
            match crate::ext::by_id(id).and_then(|i| i.put(path, &v)) {
                Some(Ok(t)) => (200, J, json!({"ok": true, "text": t}).to_string()),
                Some(Err(e)) => (200, J, json!({"ok": false, "error": e}).to_string()),
                None => (404, "text/plain", "not found".into()),
            }
        }
        ("GET", "/api/catalog") => {
            let actions: Vec<_> = ACTIONS.iter().chain(crate::ext::all().iter().flat_map(|i| i.actions())).map(|a| json!({
                "do": a.0, "controls": a.1.split(", ").collect::<Vec<_>>(), "fields": a.2, "help": a.3})).collect();
            let controls: serde_json::Map<String, serde_json::Value> = [Kind::Button, Kind::Knob, Kind::Ring, Kind::Ball]
                .iter().map(|k| (k.key().to_string(), json!(k.names()))).collect();
            (200, J, json!({"actions": actions, "controls": controls}).to_string())
        }
        _ => (404, "text/plain", "not found".into()),
    }
}

/// For rewriting the file: only a missing file means "start from the default". Anything else that
/// can't be read must not be replaced.
fn profiles_for_edit() -> Result<String, String> {
    match std::fs::read_to_string(profile_path()) {
        Ok(t) => Ok(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(default_toml().to_string()),
        Err(e) => Err(format!("can't read profiles.toml: {e}")),
    }
}

/// The file before and after each editor step, so undo can put back the very text (comments, order).
static EDITS: Mutex<(u64, std::collections::VecDeque<(u64, String, String)>)> = Mutex::new((0, std::collections::VecDeque::new()));
const EDITS_KEPT: usize = 200;

fn remember_edit(before: String, after: String) -> u64 {
    let mut g = EDITS.lock().unwrap_or_else(|e| e.into_inner());
    g.0 += 1;
    let id = g.0;
    g.1.push_back((id, before, after));
    if g.1.len() > EDITS_KEPT { g.1.pop_front(); }
    id
}

/// The text to go back (or forward) to over these steps, if the file is still where they left it.
fn retrace(ids: &[u64], redo: bool, now: &str) -> Result<String, String> {
    let g = EDITS.lock().unwrap_or_else(|e| e.into_inner());
    let get = |id: Option<&u64>| id.and_then(|id| g.1.iter().find(|x| x.0 == *id));
    let (Some(first), Some(last)) = (get(ids.first()), get(ids.last())) else { return Err("that step is no longer remembered".into()) };
    let (at, to) = if redo { (&first.1, &last.2) } else { (&last.2, &first.1) };
    if now != at { return Err("the file changed since".into()); }
    Ok(to.clone())
}

fn read_profiles() -> String {
    std::fs::read_to_string(profile_path()).unwrap_or_else(|_| default_toml().to_string())
}

/// ?a=b&c=d -> map (percent-decoding %2F etc.)
fn query(url: &str) -> std::collections::HashMap<String, String> {
    let dec = |s: &str| {
        let b = s.replace('+', " ");
        let bytes = b.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                if let Some(v) = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) { out.push(v); i += 3; continue; }
            }
            out.push(bytes[i]);
            i += 1;
        }
        String::from_utf8_lossy(&out).to_string()
    };
    url.split_once('?').map(|(_, q)| q.split('&').filter_map(|kv| kv.split_once('=').map(|(k, v)| (dec(k), dec(v)))).collect())
        .unwrap_or_default()
}

fn check_json(body: &str) -> String {
    match Profiles::parse(body) {
        Ok(p) => json!({"ok": true, "contexts": p.ctxs.len(), "warnings": p.warnings}).to_string(),
        Err(e) => json!({"ok": false, "error": e}).to_string(),
    }
}

/// "{ do = \"key\", keys = \"f5\" }" -> JSON object (or the plain string, e.g. "none").
fn toml_inline_to_json(s: &str) -> serde_json::Value {
    match format!("x = {s}").parse::<toml::Table>() {
        Ok(t) => serde_json::to_value(&t["x"]).unwrap_or(json!(s)),
        Err(_) => json!(s),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_puts_back_the_very_text() {
        let a = remember_edit("one # c".into(), "two".into());
        let b = remember_edit("two".into(), "three".into());
        assert_eq!(retrace(&[b], false, "three").unwrap(), "two");
        assert_eq!(retrace(&[a, b], false, "three").unwrap(), "one # c", "a run of steps as one");
        assert_eq!(retrace(&[a, b], true, "one # c").unwrap(), "three");
        assert!(retrace(&[a], false, "edited by hand").is_err());
        assert!(retrace(&[999_999], false, "x").is_err() && retrace(&[], false, "x").is_err());
    }
}

#[cfg(test)]
mod rest_tests {
    use super::*;

    #[test]
    fn rests_only_on_battery_when_nobody_is_around() {
        let m = |n: u64| Duration::from_secs(n * 60);
        assert!(should_rest(m(15), false, Some("bt"), false, m(15), m(20)));
        assert!(!should_rest(m(0), false, Some("bt"), false, m(99), m(99)), "switched off");
        assert!(!should_rest(m(15), false, Some("usb"), false, m(99), m(99)), "charging");
        assert!(!should_rest(m(15), true, Some("usb"), false, m(99), m(99)), "plugged in: wake");
        assert!(!should_rest(m(15), false, Some("bt"), false, m(99), m(14)), "somebody at the PC");
        assert!(!should_rest(m(15), true, Some("bt"), false, m(99), m(0)), "the PC is used again: wake");
        assert!(!should_rest(m(15), false, Some("bt"), false, m(1), m(99)), "the panel is in use");
        assert!(!should_rest(m(15), false, Some("bt"), true, m(99), m(99)), "paused or with Resolve");
        assert!(should_rest(m(15), true, None, false, m(99), m(99)), "link lost meanwhile");
        assert!(!should_rest(m(15), false, None, false, m(99), m(99)));
    }
}
