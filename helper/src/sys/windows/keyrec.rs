//! Record one key combination system-wide for the mapping editor.
//!
//! A browser page never sees some shortcuts (Ctrl+W, Alt opening the menu bar, Win keys), so the
//! editor asks the helper instead: a low-level keyboard hook catches the next combination, swallows
//! it (the app in front never gets it) and reports its name, e.g. "ctrl+w" or "alt".
//! License: CC0.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, PeekMessageW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx,
    KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG, PM_REMOVE, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

use crate::sys::RecState;

struct Rec { state: RecState, mods: Vec<&'static str>, started: Instant }

static REC: Mutex<Option<Rec>> = Mutex::new(None);

pub fn state() -> RecState {
    REC.lock().unwrap().as_ref().map(|r| r.state.clone()).unwrap_or(RecState::Idle)
}

/// Start recording (stops an earlier one). The result is read with `state()`.
pub fn start(timeout: Duration) {
    {
        let mut g = REC.lock().unwrap();
        if matches!(g.as_ref().map(|r| &r.state), Some(RecState::Waiting)) {
            if let Some(r) = g.as_mut() { r.started = Instant::now(); r.mods.clear(); }
            return; // the hook thread is already running
        }
        *g = Some(Rec { state: RecState::Waiting, mods: vec![], started: Instant::now() });
    }
    std::thread::Builder::new().name("keyrec".into()).spawn(move || unsafe {
        let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(proc_), std::ptr::null_mut(), 0);
        if hook.is_null() {
            crate::util::log("key recording: can't install the keyboard hook");
            if let Some(r) = REC.lock().unwrap().as_mut() { r.state = RecState::TimedOut; }
            return;
        }
        let mut msg: MSG = std::mem::zeroed();
        loop {
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            {
                let mut g = REC.lock().unwrap();
                let Some(r) = g.as_mut() else { break };
                if r.state != RecState::Waiting { break; }
                if r.started.elapsed() >= timeout { r.state = RecState::TimedOut; break; }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        UnhookWindowsHookEx(hook);
    }).ok();
}

pub fn cancel() {
    if let Some(r) = REC.lock().unwrap().as_mut() {
        if r.state == RecState::Waiting { r.state = RecState::TimedOut; }
    }
}

fn mod_name(vk: u32) -> Option<&'static str> {
    match vk {
        0x10 | 0xA0 | 0xA1 => Some("shift"),
        0x11 | 0xA2 | 0xA3 => Some("ctrl"),
        0x12 | 0xA4 | 0xA5 => Some("alt"),
        0x5B | 0x5C => Some("win"),
        _ => None,
    }
}

unsafe extern "system" fn proc_(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let k = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        // keys the helper itself sends pass through
        if k.flags & LLKHF_INJECTED == 0 {
            let mut g = REC.lock().unwrap();
            if let Some(r) = g.as_mut().filter(|r| r.state == RecState::Waiting) {
                let down = wparam as u32 == WM_KEYDOWN || wparam as u32 == WM_SYSKEYDOWN;
                let up = wparam as u32 == WM_KEYUP || wparam as u32 == WM_SYSKEYUP;
                if let Some(m) = mod_name(k.vkCode) {
                    if down && !r.mods.contains(&m) { r.mods.push(m); }
                    if up && !r.mods.is_empty() {
                        // a modifier (or several) on its own
                        let mut ms = r.mods.clone();
                        ms.sort_by_key(|m| ["ctrl", "shift", "alt", "win"].iter().position(|x| x == m));
                        r.state = RecState::Done(ms.join("+"));
                    }
                    return 1;
                }
                if down {
                    match crate::keys::vk_name(k.vkCode as u16) {
                        Some(n) => {
                            let mut ms = r.mods.clone();
                            ms.sort_by_key(|m| ["ctrl", "shift", "alt", "win"].iter().position(|x| x == m));
                            ms.push(&n);
                            r.state = RecState::Done(ms.join("+"));
                        }
                        None => { r.state = RecState::Done(format!("unsupported key (code {})", k.vkCode)); }
                    }
                    return 1;
                }
                if up { return 1; }
            }
        }
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}
