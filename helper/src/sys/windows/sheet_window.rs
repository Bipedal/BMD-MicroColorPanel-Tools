//! The cheat-sheet window.
//!
//! The helper works out the context from the window in front, and its key actions go to that
//! window. So this window must never become the active one: it is created with WS_EX_NOACTIVATE,
//! answers WM_MOUSEACTIVATE with MA_NOACTIVATE, and moves and resizes itself on plain mouse
//! messages. With click-through on, the mouse ignores it altogether.
//!
//! It is drawn with GDI+ into a layered window (rounded corners, opacity).

use crate::panel::KNOBS;
use crate::profiles::Profiles;
use crate::sheet::{build, cap, get, key_nm, update, Cfg, Item, View, NOTICE, NOTICE_FOR};
use crate::util::log;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use windows_sys::core::{GUID, PCWSTR};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::Graphics::GdiPlus::*;
use std::result::Result::{self, Err, Ok}; // GDI+ has a status constant named Ok
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

// ------------------------------------------------------------------ the panel's geometry (as in editor.html)

const KNOB_PX: [f32; 12] = [216.0, 288.0, 357.0, 427.0, 518.0, 588.0, 658.0, 727.0, 818.0, 888.0, 958.0, 1027.0];
const KEY_POS: [(&str, f32, usize); 39] = [
    ("AUTO_COLOR", 219.0, 1), ("OFFSET", 269.0, 1), ("COPY", 219.0, 2), ("PASTE", 269.0, 2), ("UNDO", 219.0, 3), ("REDO", 269.0, 3),
    ("DELETE", 219.0, 4), ("RESET", 269.0, 4), ("BYPASS", 219.0, 5), ("DISABLE", 269.0, 5), ("USER", 219.0, 6), ("LOOP", 269.0, 6),
    ("SHIFT_UP", 219.0, 7), ("SHIFT_DOWN", 269.0, 7), ("PLAY_STILL", 358.0, 1), ("WIPE_STILL", 408.0, 1), ("GRAB_STILL", 458.0, 1),
    ("H_LITE", 547.0, 1), ("VIEWER", 597.0, 1), ("CURSOR", 647.0, 1), ("SELECT", 698.0, 1), ("ADD_NODE", 787.0, 1), ("ADD_WINDOW", 837.0, 1),
    ("ADD_KEYFRM", 887.0, 1), ("RESET_LIFT", 573.0, 2), ("RESET_GAMMA", 623.0, 2), ("RESET_GAIN", 673.0, 2), ("PREV_STILL", 976.0, 1),
    ("NEXT_STILL", 1026.0, 1), ("PREV_KEYFRM", 976.0, 2), ("NEXT_KEYFRM", 1026.0, 2), ("PREV_NODE", 976.0, 3), ("NEXT_NODE", 1026.0, 3),
    ("PREV_FRAME", 976.0, 4), ("NEXT_FRAME", 1026.0, 4), ("PREV_CLIP", 976.0, 5), ("NEXT_CLIP", 1026.0, 5), ("PLAY_REV", 976.0, 6), ("PLAY_FWD", 1026.0, 6),
];
const ROW_Y: [f32; 8] = [0.0, 374.0, 411.0, 448.0, 486.0, 523.0, 577.0, 614.0];
const BALL_X: [(&str, f32); 3] = [("LEFT", 254.0), ("MIDDLE", 468.0), ("RIGHT", 682.0)];
const BALL_Y: f32 = 290.0;
/// drawing area: x 4..906, y 4..392
const VIEW: (f32, f32, f32, f32) = (4.0, 4.0, 902.0, 388.0);

// ------------------------------------------------------------------ GDI+

struct Theme { bg: u32, key: u32, key0: u32, fg: u32, mute: u32, faint: u32, accent: u32, accent_fg: u32, err: u32, line: u32 }
const DARK: Theme = Theme { bg: 0xFF15171A, key: 0xFF2A2E35, key0: 0xFF1C1F24, fg: 0xFFECEFF3, mute: 0xFF9AA3B0, faint: 0xFF838C99,
    accent: 0xFF7AA5FF, accent_fg: 0xFF0C1424, err: 0xFFF39A93, line: 0xFF343A43 };
const LIGHT: Theme = Theme { bg: 0xFFF2F3F5, key: 0xFFFFFFFF, key0: 0xFFE6E9EE, fg: 0xFF1B1F26, mute: 0xFF5C6573, faint: 0xFF717A87,
    accent: 0xFF2459D6, accent_fg: 0xFFFFFFFF, err: 0xFFB3261E, line: 0xFFD5DAE1 };

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() }

fn gdiplus_start() -> bool {
    let inp = GdiplusStartupInput { GdiplusVersion: 1, DebugEventCallback: 0, SuppressBackgroundThread: 0, SuppressExternalCodecs: 0 };
    let mut tok = 0usize;
    let r = unsafe { GdiplusStartup(&mut tok, &inp, std::ptr::null_mut()) };
    if r != 0 { log(&format!("cheat sheet: GDI+ did not start (status {r})")); }
    r == 0
}

type Fitted = (f32, Vec<String>);

/// Fonts, text measuring and the cache of fitted labels.
struct Pen {
    fam: *mut GpFontFamily,
    fam_b: *mut GpFontFamily,
    fmt: *mut GpStringFormat,
    meas_bmp: *mut GpBitmap,
    meas: *mut GpGraphics,
    cache: HashMap<(String, i32, i32, i32, bool), Fitted>,
}

impl Pen {
    fn new() -> Option<Pen> {
        unsafe {
            let fam = |n: &str| { let mut f = std::ptr::null_mut(); if GdipCreateFontFamilyFromName(wide(n).as_ptr(), std::ptr::null_mut(), &mut f) == 0 { f } else { std::ptr::null_mut() } };
            let (mut a, mut b) = (fam("Segoe UI"), fam("Segoe UI Semibold"));
            if a.is_null() { a = fam("Arial"); }
            if b.is_null() { b = a; }
            if a.is_null() { log("cheat sheet: no usable font"); return None; }
            let (mut g0, mut fmt) = (std::ptr::null_mut(), std::ptr::null_mut());
            GdipStringFormatGetGenericTypographic(&mut g0);
            GdipCloneStringFormat(g0, &mut fmt);
            GdipSetStringFormatAlign(fmt, 0);
            let (mut bmp, mut g) = (std::ptr::null_mut(), std::ptr::null_mut());
            GdipCreateBitmapFromScan0(8, 8, 0, PARGB, std::ptr::null(), &mut bmp);
            GdipGetImageGraphicsContext(bmp as *mut GpImage, &mut g);
            GdipSetTextRenderingHint(g, 4);
            Some(Pen { fam: a, fam_b: b, fmt, meas_bmp: bmp, meas: g, cache: HashMap::new() })
        }
    }

    fn font(&self, size: f32, bold: bool) -> *mut GpFont {
        let mut f = std::ptr::null_mut();
        unsafe { GdipCreateFont(if bold { self.fam_b } else { self.fam }, size, 0, 2, &mut f); }
        f
    }

    /// Width of `s` at `size`.
    fn width(&self, s: &str, size: f32, bold: bool) -> f32 {
        let w: Vec<u16> = s.encode_utf16().collect();
        let f = self.font(size, bold);
        let lay = RectF { X: 0.0, Y: 0.0, Width: 10_000.0, Height: 1_000.0 };
        let mut b = RectF { X: 0.0, Y: 0.0, Width: 0.0, Height: 0.0 };
        let (mut a, mut c) = (0, 0);
        unsafe {
            GdipMeasureString(self.meas, w.as_ptr(), w.len() as i32, f, &lay, self.fmt, &mut b, &mut a, &mut c);
            GdipDeleteFont(f);
        }
        b.Width
    }

    /// Largest size (and line breaks) at which `s` fits into w x h.
    fn fit(&mut self, s: &str, w: f32, h: f32, max: f32, bold: bool) -> Fitted {
        let key = (s.to_string(), w as i32, h as i32, (max * 10.0) as i32, bold);
        if let Some(f) = self.cache.get(&key) { return f.clone(); }
        // long shortcuts may break after a "+": (piece, space before it)
        let mut words: Vec<(&str, bool)> = vec![];
        for x in s.split_whitespace() {
            if x.len() > 7 && x.contains('+') { for (i, p) in x.split_inclusive('+').enumerate() { words.push((p, i == 0)); } } else { words.push((x, true)); }
        }
        // widths scale with the size: measure once
        let ww: Vec<f32> = words.iter().map(|x| self.width(x.0, 100.0, bold) / 100.0).collect();
        let space = (self.width("a a", 100.0, bold) - self.width("aa", 100.0, bold)) / 100.0;
        let mut size = max;
        let out = loop {
            let mut lines: Vec<String> = vec![];
            let (mut cur, mut cw) = (String::new(), 0.0f32);
            let mut ok = true;
            for (&(x, gap), &m) in words.iter().zip(&ww) {
                let m = m * size;
                let sp = if gap { space * size } else { 0.0 };
                if m > w { ok = false; }
                if cur.is_empty() { cur = x.to_string(); cw = m; }
                else if cw + sp + m <= w { if gap { cur.push(' '); } cur.push_str(x); cw += sp + m; }
                else { lines.push(std::mem::take(&mut cur)); cur = x.to_string(); cw = m; }
            }
            if !cur.is_empty() { lines.push(cur); }
            let fits = ok && lines.len() as f32 * size * 1.14 <= h;
            if fits || size <= 5.0 {
                if !fits {
                    let n = ((h / (size * 1.14)) as usize).max(1);
                    if lines.len() > n { lines.truncate(n); if let Some(l) = lines.last_mut() { l.push('…'); } }
                }
                break (size, lines);
            }
            size -= 0.5;
        };
        if self.cache.len() > 4000 { self.cache.clear(); }
        self.cache.insert(key, out.clone());
        out
    }
}
impl Drop for Pen {
    fn drop(&mut self) {
        unsafe {
            GdipDeleteGraphics(self.meas);
            GdipDisposeImage(self.meas_bmp as *mut GpImage);
            GdipDeleteStringFormat(self.fmt);
            if self.fam_b != self.fam { GdipDeleteFontFamily(self.fam_b); }
            GdipDeleteFontFamily(self.fam);
        }
    }
}

const PARGB: i32 = 0xE200B; // PixelFormat32bppPARGB

/// One drawing pass.
struct Canvas<'a> { g: *mut GpGraphics, pen: &'a mut Pen }

impl Canvas<'_> {
    fn brush(&self, c: u32) -> *mut GpBrush {
        let mut b = std::ptr::null_mut();
        unsafe { GdipCreateSolidFill(c, &mut b); }
        b as *mut GpBrush
    }
    /// Rounded rectangle; radii: top-left, top-right, bottom-right, bottom-left.
    fn rrect(&self, x: f32, y: f32, w: f32, h: f32, r: [f32; 4], fill: u32) {
        unsafe {
            let mut p = std::ptr::null_mut();
            GdipCreatePath(0, &mut p);
            let arc = |cx: f32, cy: f32, r: f32, start: f32, px: f32, py: f32| {
                if r > 0.0 { GdipAddPathArc(p, cx, cy, r * 2.0, r * 2.0, start, 90.0); } else { GdipAddPathLine(p, px, py, px, py); }
            };
            arc(x, y, r[0], 180.0, x, y);
            arc(x + w - 2.0 * r[1], y, r[1], 270.0, x + w, y);
            arc(x + w - 2.0 * r[2], y + h - 2.0 * r[2], r[2], 0.0, x + w, y + h);
            arc(x, y + h - 2.0 * r[3], r[3], 90.0, x, y + h);
            GdipClosePathFigure(p);
            let b = self.brush(fill);
            GdipFillPath(self.g, b, p);
            GdipDeleteBrush(b);
            GdipDeletePath(p);
        }
    }
    fn ellipse(&self, cx: f32, cy: f32, r: f32, fill: u32) {
        let b = self.brush(fill);
        unsafe { GdipFillEllipse(self.g, b, cx - r, cy - r, 2.0 * r, 2.0 * r); GdipDeleteBrush(b); }
    }
    fn ring(&self, cx: f32, cy: f32, r: f32, inner: f32, fill: u32) {
        unsafe {
            let mut p = std::ptr::null_mut();
            GdipCreatePath(0, &mut p); // even-odd
            GdipAddPathEllipse(p, cx - r, cy - r, 2.0 * r, 2.0 * r);
            GdipAddPathEllipse(p, cx - inner, cy - inner, 2.0 * inner, 2.0 * inner);
            let b = self.brush(fill);
            GdipFillPath(self.g, b, p);
            GdipDeleteBrush(b);
            GdipDeletePath(p);
        }
    }
    fn line(&self, x1: f32, y1: f32, x2: f32, y2: f32, w: f32, c: u32) {
        unsafe {
            let mut p = std::ptr::null_mut();
            GdipCreatePen1(c, w, 2, &mut p);
            GdipDrawLine(self.g, p, x1, y1, x2, y2);
            GdipDeletePen(p);
        }
    }
    /// One line of text; y = top. Returns its width.
    fn text(&mut self, s: &str, x: f32, y: f32, size: f32, bold: bool, c: u32) -> f32 {
        let w: Vec<u16> = s.encode_utf16().collect();
        let f = self.pen.font(size, bold);
        let lay = RectF { X: x, Y: y, Width: 10_000.0, Height: size * 2.0 };
        let b = self.brush(c);
        unsafe {
            GdipDrawString(self.g, w.as_ptr(), w.len() as i32, f, &lay, self.pen.fmt, b);
            GdipDeleteBrush(b);
            GdipDeleteFont(f);
        }
        self.pen.width(s, size, bold)
    }
    /// Text centred in a box, as large as it fits up to `max`.
    fn label(&mut self, s: &str, cx: f32, cy: f32, w: f32, h: f32, max: f32, bold: bool, c: u32) {
        let (size, lines) = self.pen.fit(s, w, h, max, bold);
        let lh = size * 1.14;
        let top = cy - lines.len() as f32 * lh / 2.0 - size * 0.10;
        for (i, l) in lines.iter().enumerate() {
            let lw = self.pen.width(l, size, bold);
            self.text(l, cx - lw / 2.0, top + i as f32 * lh, size, bold, c);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Btn { List, Less, More, Close }

/// Draw the whole window into `g` (w x h px). Returns the header buttons' places.
fn paint(g: *mut GpGraphics, pen: &mut Pen, v: &View, w: f32, h: f32, dpi: f32, opacity: u8, buttons: bool) -> Vec<(RectF, Btn)> {
    let t = if v.light { &LIGHT } else { &DARK };
    let mut c = Canvas { g, pen };
    unsafe {
        GdipSetSmoothingMode(g, 4);
        GdipSetPixelOffsetMode(g, 4);
        GdipSetTextRenderingHint(g, 4);
        GdipGraphicsClear(g, 0);
    }
    let rad = 11.0 * dpi;
    c.rrect(0.0, 0.0, w, h, [rad; 4], t.line);
    c.rrect(1.0, 1.0, w - 2.0, h - 2.0, [rad - 1.0; 4], t.bg);
    // header
    let (hh, fs) = (32.0 * dpi, 14.0 * dpi);
    let mut hits = vec![];
    let mut right = w - 10.0 * dpi;
    if buttons {
        let bs = 12.5 * dpi;
        for (txt, b) in [("✕".to_string(), Some(Btn::Close)), ("+".into(), Some(Btn::More)), (format!("{opacity}%"), None), ("–".into(), Some(Btn::Less)),
                         (if v.list { "Panel".into() } else { "List".into() }, Some(Btn::List))] {
            let tw = c.pen.width(&txt, bs, false);
            right -= tw + 14.0 * dpi;
            c.text(&txt, right + 7.0 * dpi, 8.5 * dpi, bs, false, if b.is_some() { t.mute } else { t.faint });
            if let Some(b) = b { hits.push((RectF { X: right, Y: 0.0, Width: tw + 14.0 * dpi, Height: hh }, b)); }
        }
    }
    let mut x = 14.0 * dpi;
    x += c.text(&v.title, x, 7.0 * dpi, fs, true, t.fg) + 10.0 * dpi;
    let (link, bad) = &v.link;
    if !link.is_empty() {
        let ls = 12.5 * dpi;
        let lw = c.pen.width(link, ls, false) + 14.0 * dpi;
        // a small sheet keeps its title
        if x + lw < right {
            right -= lw;
            c.text(link, right + 7.0 * dpi, 8.5 * dpi, ls, false, if *bad { t.err } else { t.mute });
        }
    }
    if !v.notice.is_empty() {
        let (size, lines) = c.pen.fit(&v.notice, (right - x).max(40.0), fs * 1.3, fs, true);
        c.text(lines.first().map(String::as_str).unwrap_or(""), x, 7.0 * dpi + (fs - size), size, true, t.accent);
        x = right;
    } else if !v.preset.is_empty() && x < right - 60.0 * dpi {
        x += c.text(&v.preset, x, 8.0 * dpi, 12.5 * dpi, false, t.faint) + 10.0 * dpi;
    }
    if !v.layer.is_empty() && x < right - 40.0 * dpi {
        let ps = 12.5 * dpi;
        let tw = c.pen.width(&v.layer, ps, true);
        c.rrect(x, 6.5 * dpi, tw + 16.0 * dpi, 20.0 * dpi, [6.0 * dpi; 4], t.accent);
        c.text(&v.layer, x + 8.0 * dpi, 8.0 * dpi, ps, true, t.accent_fg);
    }
    let (m, top) = (10.0 * dpi, hh + 2.0 * dpi);
    if v.items.is_empty() {
        c.label("Nothing is assigned here.", w / 2.0, (h + hh) / 2.0, w - 2.0 * m, 60.0 * dpi, 14.0 * dpi, false, t.mute);
        return hits;
    }
    if v.list { paint_list(&mut c, v, t, m + 4.0 * dpi, top, w - 2.0 * m - 8.0 * dpi, h - top - m, dpi); return hits; }
    let k = ((w - 2.0 * m) / VIEW.2).min((h - top - m) / VIEW.3).max(0.05);
    unsafe {
        GdipTranslateWorldTransform(g, (w - VIEW.2 * k) / 2.0, top + (h - top - m - VIEW.3 * k) / 2.0, 0);
        GdipScaleWorldTransform(g, k, k, 0);
        GdipTranslateWorldTransform(g, -VIEW.0, -VIEW.1, 0);
    }
    paint_panel(&mut c, v, t);
    unsafe { GdipResetWorldTransform(g); }
    hits
}

fn paint_panel(c: &mut Canvas, v: &View, t: &Theme) {
    let it = |k: &str| v.items.get(k);
    let hot = |k: &str| v.touched.as_deref() == Some(k);
    let fill = |k: &str| if hot(k) { t.accent } else if it(k).is_some() { t.key } else { t.key0 };
    let ink = |k: &str, i: &Item| if hot(k) { t.accent_fg } else if i.off { t.err } else if i.bright { t.fg } else { t.faint };
    for (i, n) in KNOBS.iter().enumerate() {
        let (cx, kk, pk) = (KNOB_PX[i] - 154.0, format!("knob.{n}"), format!("button.{n}"));
        let x = cx - 33.0;
        c.rrect(x, 7.0, 66.0, 70.5, [12.0, 12.0, 0.0, 0.0], fill(&kk));
        c.ellipse(cx, 24.0, 12.0, t.bg);
        c.line(cx, 13.5, cx, 17.0, 1.5, t.mute);
        if let Some(a) = it(&kk) { c.label(&a.label, cx, 57.0, 62.0, 38.0, 15.0, a.bright, ink(&kk, a)); }
        c.rrect(x, 79.0, 66.0, 27.0, [0.0, 0.0, 11.0, 11.0], fill(&pk));
        match it(&pk) {
            Some(a) if !a.plain_reset => c.label(&a.label, cx, 92.5, 60.0, 25.0, 10.5, a.bright, ink(&pk, a)),
            Some(_) => {
                // the usual "press = reset": just the push mark
                let col = if hot(&pk) { t.accent_fg } else { t.faint };
                c.line(cx, 87.0, cx, 94.0, 1.2, col);
                c.line(cx - 2.6, 91.4, cx, 94.0, 1.2, col);
                c.line(cx + 2.6, 91.4, cx, 94.0, 1.2, col);
                c.line(cx - 3.3, 97.0, cx + 3.3, 97.0, 1.2, col);
            }
            None => {}
        }
    }
    let mut key = |n: &str, x: f32, y: f32, w: f32| {
        let k = format!("button.{n}");
        c.rrect(x, y, w, 34.5, [5.0; 4], fill(&k));
        if let Some(a) = it(&k) { c.label(&a.label, x + w / 2.0, y + 17.25, w - 4.0, 33.0, 12.5, a.bright, ink(&k, a)); }
    };
    for (n, x, r) in KEY_POS { key(n, x - 154.0 - 23.6, ROW_Y[r] - 245.0 - 16.5, 47.2); }
    key("STOP", 1001.0 - 154.0 - 48.6, 614.0 - 245.0 - 16.5, 97.2);
    for (n, cx) in BALL_X {
        let (rk, bk) = (format!("ring.{n}"), format!("ball.{n}"));
        c.ring(cx, BALL_Y, 90.0, 57.0, fill(&rk));
        if let Some(a) = it(&rk) { c.label(&a.label, cx, BALL_Y + 72.0, 96.0, 28.0, 14.0, a.bright, ink(&rk, a)); }
        c.ellipse(cx, BALL_Y, 54.0, fill(&bk));
        if let Some(a) = it(&bk) { c.label(&a.label, cx, BALL_Y, 88.0, 66.0, 17.0, a.bright, ink(&bk, a)); }
    }
}

/// (heading, [(control key, printed name)])
fn sections() -> Vec<(&'static str, Vec<(String, String)>)> {
    let side = |n: &str| cap(&n.to_lowercase());
    let mut balls = vec![];
    for (n, _) in BALL_X { balls.push((format!("ball.{n}"), format!("{} ball", side(n)))); balls.push((format!("ring.{n}"), format!("{} ring", side(n)))); }
    let keys = KEY_POS.iter().map(|k| k.0).chain(std::iter::once("STOP")).map(|n| (format!("button.{n}"), key_nm(n))).collect();
    vec![
        ("Knobs", KNOBS.iter().map(|n| (format!("knob.{n}"), key_nm(n))).collect()),
        ("Balls and rings", balls),
        ("Keys", keys),
        ("Knob presses", KNOBS.iter().map(|n| (format!("button.{n}"), key_nm(n))).collect()),
    ]
}

fn paint_list(c: &mut Canvas, v: &View, t: &Theme, x0: f32, y0: f32, w: f32, h: f32, dpi: f32) {
    // rows: None = heading
    let mut rows: Vec<(Option<String>, String, String)> = vec![];
    for (head, list) in sections() {
        let body: Vec<_> = list.into_iter().filter(|(k, _)| v.items.get(k).map_or(false, |i| !i.plain_reset)).collect();
        if body.is_empty() { continue; }
        rows.push((None, head.to_string(), String::new()));
        for (k, n) in body { let l = v.items[&k].label.clone(); rows.push((Some(k), n, l)); }
    }
    // the largest text at which everything fits
    let mut fs = 15.0 * dpi;
    let (mut cols, mut per) = (1usize, rows.len());
    while fs > 7.0 * dpi {
        per = ((h / (fs * 1.45)) as usize).max(1);
        cols = rows.len().div_ceil(per);
        if cols as f32 * fs * 15.5 <= w { break; }
        fs -= 0.5 * dpi;
    }
    let lh = fs * 1.45;
    let cw = (w / cols as f32).min(fs * 22.0);
    for (i, (k, n, l)) in rows.iter().enumerate() {
        let (x, y) = (x0 + (i / per) as f32 * cw, y0 + (i % per) as f32 * lh);
        match k {
            None => { c.text(n, x, y + lh * 0.1, fs * 0.82, true, t.faint); }
            Some(k) => {
                let a = &v.items[k];
                if v.touched.as_deref() == Some(k) { c.rrect(x - 4.0 * dpi, y, cw - 4.0 * dpi, lh, [4.0 * dpi; 4], t.key); }
                c.text(n, x, y + fs * 0.16, fs * 0.82, false, t.faint);
                let (size, lines) = c.pen.fit(l, cw - fs * 6.3, lh, fs, a.bright);
                let col = if a.off { t.err } else if a.bright { t.fg } else { t.faint };
                c.text(lines.first().map(String::as_str).unwrap_or(""), x + fs * 6.0, y + (fs - size) * 0.5, size, a.bright, col);
            }
        }
    }
}

/// Draw into a fresh top-down premultiplied BGRA buffer.
fn render(pen: &mut Pen, v: &View, w: i32, h: i32, dpi: f32, opacity: u8, buttons: bool) -> (Vec<u32>, Vec<(RectF, Btn)>) {
    let mut buf = vec![0u32; (w * h) as usize];
    let mut hits = vec![];
    unsafe {
        let (mut bmp, mut g) = (std::ptr::null_mut(), std::ptr::null_mut());
        if GdipCreateBitmapFromScan0(w, h, w * 4, PARGB, buf.as_mut_ptr() as *const u8, &mut bmp) == 0 {
            GdipGetImageGraphicsContext(bmp as *mut GpImage, &mut g);
            hits = paint(g, pen, v, w as f32, h as f32, dpi, opacity, buttons);
            GdipDeleteGraphics(g);
            GdipDisposeImage(bmp as *mut GpImage);
        }
    }
    (buf, hits)
}

/// Save one picture of the sheet (tests, or a printable copy).
pub fn save_png(path: &str, v: &View, w: i32, h: i32) -> Result<(), String> {
    if !gdiplus_start() { return Err("GDI+ did not start".into()); }
    let mut pen = Pen::new().ok_or("no font")?;
    let (mut buf, _) = render(&mut pen, v, w, h, 1.0, 100, false);
    let png = GUID { data1: 0x557CF406, data2: 0x1A04, data3: 0x11D3, data4: [0x9A, 0x73, 0x00, 0x00, 0xF8, 0x1E, 0xF3, 0x2E] };
    unsafe {
        let mut bmp = std::ptr::null_mut();
        GdipCreateBitmapFromScan0(w, h, w * 4, PARGB, buf.as_mut_ptr() as *const u8, &mut bmp);
        let r = GdipSaveImageToFile(bmp as *mut GpImage, wide(path).as_ptr(), &png, std::ptr::null());
        GdipDisposeImage(bmp as *mut GpImage);
        if r != 0 { return Err(format!("can't write {path} (GDI+ status {r})")); }
    }
    Ok(())
}

/// `micropanel --sheet-png FILE [--ctx app/module] [--layers more] [--size 860x424] [--list] [--light] [--link usb|bt|none]`
pub fn png_from_args(path: &str, ctx: &str, layers: &[String], size: (i32, i32), list: bool, light: bool, notice: &str, link: Option<&str>) -> Result<(), String> {
    let text = std::fs::read_to_string(crate::app::profile_path()).map_err(|e| e.to_string())?;
    let p = Profiles::parse(&text)?;
    let mut v = build(&p, &crate::edit::stack_for(ctx), layers);
    v.list = list;
    v.light = light;
    v.notice = notice.to_string();
    if let Some(l) = link { v.link = crate::sheet::link_text(Some(l).filter(|l| *l != "none"), false, false); }
    save_png(path, &v, size.0, size.1)
}

// ------------------------------------------------------------------ the window

fn light_theme() -> bool {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let (mut v, mut n) = (0u32, 4u32);
    let r = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, wide("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize").as_ptr(), wide("AppsUseLightTheme").as_ptr(),
            RRF_RT_REG_DWORD, std::ptr::null_mut(), &mut v as *mut u32 as *mut _, &mut n)
    };
    r == 0 && v == 1
}

#[derive(Default)]
struct Ui { hits: Vec<(RectF, Btn)>, drag: Option<Drag>, dirty: bool, w: i32, h: i32, dpi: f32 }
#[derive(Clone, Copy)]
struct Drag { from: POINT, rect: RECT, right: bool, bottom: bool }

thread_local! { static UI: std::cell::RefCell<Ui> = std::cell::RefCell::new(Ui::default()); }

fn at(l: LPARAM) -> (f32, f32) { ((l & 0xFFFF) as i16 as f32, ((l >> 16) & 0xFFFF) as i16 as f32) }

/// Which edges the point is on (right, bottom).
fn edges(x: f32, y: f32, u: &Ui) -> (bool, bool) {
    let e = 9.0 * u.dpi;
    let corner = 18.0 * u.dpi;
    let (r, b) = (u.w as f32 - x, u.h as f32 - y);
    if r < corner && b < corner { (true, true) } else { (r < e, b < e) }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_MOUSEACTIVATE => return MA_NOACTIVATE as LRESULT,
            WM_LBUTTONDOWN => {
                let (x, y) = at(lp);
                let hit = UI.with(|u| u.try_borrow().ok().and_then(|u| u.hits.iter().find(|(r, _)| x >= r.X && x < r.X + r.Width && y >= r.Y && y < r.Y + r.Height).map(|h| h.1)));
                match hit {
                    Some(Btn::Close) => { update(|c| c.open = false); }
                    Some(Btn::List) => { update(|c| c.list = !c.list); }
                    Some(Btn::Less) => { update(|c| c.opacity = c.opacity.saturating_sub(10)); }
                    Some(Btn::More) => { update(|c| c.opacity = c.opacity.saturating_add(10)); }
                    None => {
                        let (mut p, mut r): (POINT, RECT) = (std::mem::zeroed(), std::mem::zeroed());
                        GetCursorPos(&mut p);
                        GetWindowRect(hwnd, &mut r);
                        UI.with(|u| if let Ok(mut u) = u.try_borrow_mut() {
                            let (right, bottom) = edges(x, y, &u);
                            u.drag = Some(Drag { from: p, rect: r, right, bottom });
                        });
                        SetCapture(hwnd);
                    }
                }
                return 0;
            }
            WM_MOUSEMOVE => {
                let d = UI.with(|u| u.try_borrow().ok().and_then(|u| u.drag));
                if let Some(d) = d {
                    let mut p: POINT = std::mem::zeroed();
                    GetCursorPos(&mut p);
                    let (dx, dy) = (p.x - d.from.x, p.y - d.from.y);
                    let (w, h) = (d.rect.right - d.rect.left, d.rect.bottom - d.rect.top);
                    if d.right || d.bottom {
                        let (w, h) = (if d.right { w + dx } else { w }, if d.bottom { h + dy } else { h });
                        UI.with(|u| if let Ok(mut u) = u.try_borrow_mut() { u.w = w.max(300); u.h = h.max(150); u.dirty = true; });
                    } else {
                        SetWindowPos(hwnd, std::ptr::null_mut(), d.rect.left + dx, d.rect.top + dy, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
                    }
                }
                return 0;
            }
            WM_LBUTTONUP => {
                let had = UI.with(|u| u.try_borrow_mut().ok().and_then(|mut u| u.drag.take()));
                if had.is_some() {
                    ReleaseCapture();
                    let mut r: RECT = std::mem::zeroed();
                    GetWindowRect(hwnd, &mut r);
                    let (w, h) = UI.with(|u| u.try_borrow().map(|u| (u.w, u.h)).unwrap_or((r.right - r.left, r.bottom - r.top)));
                    update(|c| { c.x = r.left; c.y = r.top; c.w = w; c.h = h; });
                }
                return 0;
            }
            WM_SETCURSOR => {
                let mut p: POINT = std::mem::zeroed();
                let mut r: RECT = std::mem::zeroed();
                GetCursorPos(&mut p);
                GetWindowRect(hwnd, &mut r);
                let e = UI.with(|u| u.try_borrow().map(|u| edges((p.x - r.left) as f32, (p.y - r.top) as f32, &u)).unwrap_or((false, false)));
                let id = match e { (true, true) => IDC_SIZENWSE, (true, false) => IDC_SIZEWE, (false, true) => IDC_SIZENS, _ => IDC_ARROW };
                SetCursor(LoadCursorW(std::ptr::null_mut(), id));
                return 1;
            }
            _ => {}
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}

/// Put the picture on screen.
fn present(hwnd: HWND, buf: &[u32], w: i32, h: i32, opacity: u8) {
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        let dc = CreateCompatibleDC(screen);
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader = BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB, ..std::mem::zeroed() };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let dib = CreateDIBSection(screen, &bi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        if !dib.is_null() && !bits.is_null() {
            std::ptr::copy_nonoverlapping(buf.as_ptr(), bits as *mut u32, buf.len());
            let old = SelectObject(dc, dib as HGDIOBJ);
            let (size, src) = (SIZE { cx: w, cy: h }, POINT { x: 0, y: 0 });
            let bf = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: (opacity as u32 * 255 / 100) as u8, AlphaFormat: AC_SRC_ALPHA as u8 };
            UpdateLayeredWindow(hwnd, screen, std::ptr::null(), &size, dc, &src, 0, &bf, ULW_ALPHA);
            SelectObject(dc, old);
            DeleteObject(dib as HGDIOBJ);
        }
        DeleteDC(dc);
        ReleaseDC(std::ptr::null_mut(), screen);
    }
}

/// First place: bottom right of the main screen. A saved place that is off every screen is dropped.
fn place(c: &Cfg, dpi: f32) -> (i32, i32, i32, i32) {
    unsafe {
        let mut wa: RECT = std::mem::zeroed();
        SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut wa as *mut RECT as *mut _, 0);
        let (w, h) = if c.w > 0 { (c.w, c.h) } else { ((860.0 * dpi) as i32, (424.0 * dpi) as i32) };
        let (w, h) = (w.max(300).min(wa.right - wa.left), h.max(150).min(wa.bottom - wa.top));
        if c.w > 0 {
            let mid = POINT { x: c.x + w / 2, y: c.y + 16 };
            if !MonitorFromPoint(mid, MONITOR_DEFAULTTONULL).is_null() { return (c.x, c.y, w, h); }
        }
        (wa.right - w - 24, wa.bottom - h - 24, w, h)
    }
}

pub fn start(shared: Arc<Mutex<crate::app::Shared>>) {
    let r = std::thread::Builder::new().name("sheet".into()).spawn(move || run(shared));
    if let Err(e) = r { log(&format!("cheat sheet: {e}")); }
}

fn run(shared: Arc<Mutex<crate::app::Shared>>) {
    unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2); }
    if !gdiplus_start() { return; }
    let Some(mut pen) = Pen::new() else { return };
    let class = wide("MicroPanelCheatSheet");
    let hwnd = unsafe {
        let inst = GetModuleHandleW(std::ptr::null());
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: inst, lpszClassName: class.as_ptr(), ..std::mem::zeroed() };
        RegisterClassW(&wc);
        let title: PCWSTR = wide_static();
        CreateWindowExW(WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW, class.as_ptr(), title, WS_POPUP, 0, 0, 400, 200,
            std::ptr::null_mut(), std::ptr::null_mut(), inst, std::ptr::null())
    };
    if hwnd.is_null() { log("cheat sheet: the window could not be created"); return; }
    let mut profiles = Profiles::default();
    let mut seen: Option<SystemTime> = None;
    let mut checked = Instant::now() - Duration::from_secs(60);
    let (mut light, mut theme_at) = (light_theme(), Instant::now());
    let mut shown = false;
    let mut last: Option<(View, Cfg, i32, i32, u32)> = None;
    let mut raised = Instant::now();
    let mut preset_seen: (u64, String, Instant) = (u64::MAX, String::new(), Instant::now());
    let mut preset_name = String::new();
    loop {
        unsafe {
            let mut m: MSG = std::mem::zeroed();
            while PeekMessageW(&mut m, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 { TranslateMessage(&m); DispatchMessageW(&m); }
        }
        if shared.lock().unwrap().quit { unsafe { DestroyWindow(hwnd); } return; }
        let c = get();
        let notice = { let mut n = NOTICE.lock().unwrap(); if n.as_ref().map_or(false, |(_, at)| at.elapsed() >= NOTICE_FOR) { *n = None; } n.as_ref().map(|x| x.0.clone()).unwrap_or_default() };
        if !c.open && notice.is_empty() {
            if shown { unsafe { ShowWindow(hwnd, SW_HIDE); } shown = false; last = None; }
            std::thread::sleep(Duration::from_millis(40));
            continue;
        }
        if checked.elapsed() >= Duration::from_millis(500) {
            checked = Instant::now();
            let path = crate::app::profile_path();
            let mt = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
            if mt != seen {
                seen = mt;
                // a file that doesn't load keeps the last good one, like the helper itself
                if let Some(p) = std::fs::read_to_string(&path).ok().and_then(|t| Profiles::parse(&t).ok()) { profiles = p; }
            }
        }
        if theme_at.elapsed() >= Duration::from_secs(3) { theme_at = Instant::now(); light = light_theme(); }
        let (stack, layers, touched, note, link) = {
            let s = shared.lock().unwrap();
            let st = if s.follow_context.is_empty() { s.context.clone() } else { s.follow_context.clone() };
            let now = SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
            let t = s.last.as_ref().filter(|x| now - x.at < 0.35).map(|x| x.control.clone());
            let note = if s.yielded_to_resolve { " (Resolve has the panel)" } else if s.paused { " (paused)" } else { "" };
            let link = crate::sheet::link_text(s.panel_link.as_deref(), s.resting, s.yielded_to_resolve || s.paused);
            (st, s.layers.clone(), t, note, link)
        };
        let mut v = build(&profiles, &stack, &layers);
        v.title.push_str(note);
        let app = stack.first().map(|c| c.split('/').next().unwrap_or(c).to_string()).unwrap_or_default();
        let rev = crate::presets::REV.load(std::sync::atomic::Ordering::Relaxed);
        if preset_seen.0 != rev || preset_seen.1 != app || preset_seen.2.elapsed() >= Duration::from_secs(5) {
            preset_name = crate::presets::Store::real().title_for(&app).unwrap_or_default();
            preset_seen = (rev, app, Instant::now());
        }
        v.preset = preset_name.clone();
        v.notice = notice;
        v.touched = touched;
        v.link = link;
        v.light = light;
        v.list = c.list;
        if !shown {
            let dpi = unsafe { GetDpiForWindow(hwnd) } as f32 / 96.0;
            let (x, y, w, h) = place(&c, dpi.max(1.0));
            unsafe { SetWindowPos(hwnd, HWND_TOPMOST, x, y, w, h, SWP_NOACTIVATE); }
            UI.with(|u| { let mut u = u.borrow_mut(); u.w = w; u.h = h; u.drag = None; });
        }
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        let (w, h) = UI.with(|u| { let mut u = u.borrow_mut(); u.dpi = (dpi as f32 / 96.0).max(0.5); u.dirty = false; (u.w, u.h) });
        let now = (v, c.clone(), w, h, dpi);
        if last.as_ref() != Some(&now) || !shown {
            unsafe {
                let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
                let want = if c.click_through { ex | WS_EX_TRANSPARENT } else { ex & !WS_EX_TRANSPARENT };
                if want != ex { SetWindowLongW(hwnd, GWL_EXSTYLE, want as i32); }
            }
            let (buf, hits) = render(&mut pen, &now.0, w, h, dpi as f32 / 96.0, c.opacity, !c.click_through);
            present(hwnd, &buf, w, h, c.opacity);
            UI.with(|u| u.borrow_mut().hits = hits);
            if !shown { unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE); } shown = true; }
            last = Some(now);
        }
        // another topmost window may have come in front since
        if raised.elapsed() >= Duration::from_secs(1) {
            raised = Instant::now();
            unsafe { SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE); }
        }
        let dragging = UI.with(|u| u.borrow().drag.is_some());
        std::thread::sleep(Duration::from_millis(if dragging { 8 } else { 30 }));
    }
}

fn wide_static() -> PCWSTR {
    static T: std::sync::OnceLock<Vec<u16>> = std::sync::OnceLock::new();
    T.get_or_init(|| wide("Micro Color Panel cheat sheet")).as_ptr()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiles::Kind;

    #[test]
    fn every_control_has_a_place() {
        let all: usize = [Kind::Button, Kind::Knob, Kind::Ring, Kind::Ball].iter().map(|k| k.names().len()).sum();
        let listed: Vec<String> = sections().into_iter().flat_map(|s| s.1.into_iter().map(|x| x.0)).collect();
        assert_eq!(listed.len(), all);
        for k in [Kind::Button, Kind::Knob, Kind::Ring, Kind::Ball] { for n in k.names() { assert!(listed.contains(&format!("{}.{n}", k.key())), "{n}"); } }
    }
}
