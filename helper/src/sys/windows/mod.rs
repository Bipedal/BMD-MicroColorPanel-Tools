//! Windows: keys and mouse (SendInput), the program in front, single instance, autostart.
//! MIDI, key recording, the cheat-sheet window and the tray are in the files next to this one.

pub mod keyrec;
pub mod midi;
pub mod sheet_window;
mod tray;
pub use tray::tray;

use crate::keys::Combo;
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HWND};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    CreateMutexW, OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    MapVirtualKeyW, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, MAPVK_VK_TO_VSC, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEINPUT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindow, GetWindowTextW, GetWindowThreadProcessId, GW_OWNER};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;

fn mouse(flags: u32, dx: i32, dy: i32, data: i32) {
    let inp = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: data as u32, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    };
    let n = unsafe { SendInput(1, &inp, std::mem::size_of::<INPUT>() as i32) };
    if n != 1 { crate::util::log(&format!("SendInput(mouse {flags:#x}) failed: error {}", unsafe { GetLastError() })); }
}

pub fn mouse_move(dx: i32, dy: i32) { mouse(MOUSEEVENTF_MOVE, dx, dy, 0); }
pub fn scroll(v: i32, h: i32, mods: &[u16]) {
    if mods.is_empty() {
        if v != 0 { mouse(MOUSEEVENTF_WHEEL, 0, 0, v); }
        if h != 0 { mouse(MOUSEEVENTF_HWHEEL, 0, 0, h); }
        return;
    }
    // modifiers down, wheel, modifiers up, in one SendInput so nothing gets in between
    let wheel = |flags: u32, data: i32| INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 { mi: MOUSEINPUT { dx: 0, dy: 0, mouseData: data as u32, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    };
    let mut inp: Vec<INPUT> = mods.iter().map(|&m| key_input(m, false)).collect();
    if v != 0 { inp.push(wheel(MOUSEEVENTF_WHEEL, v)); }
    if h != 0 { inp.push(wheel(MOUSEEVENTF_HWHEEL, h)); }
    // a bare Alt (or Win) press-release opens the menu bar (Start menu): an unassigned key
    // in between makes Windows see it as part of a shortcut instead
    if mods.iter().any(|&m| matches!(m, 0x12 | 0xA4 | 0xA5 | 0x5B | 0x5C)) {
        inp.push(key_input(0xE8, false));
        inp.push(key_input(0xE8, true));
    }
    inp.extend(mods.iter().rev().map(|&m| key_input(m, true)));
    let n = unsafe { SendInput(inp.len() as u32, inp.as_ptr(), std::mem::size_of::<INPUT>() as i32) };
    if n as usize != inp.len() { crate::util::log(&format!("SendInput(scroll with keys) accepted {n}/{}", inp.len())); }
}
pub fn mouse_button(b: u8, down: bool) {
    let f = match (b, down) {
        (0, true) => MOUSEEVENTF_LEFTDOWN, (0, false) => MOUSEEVENTF_LEFTUP,
        (1, true) => MOUSEEVENTF_RIGHTDOWN, (1, false) => MOUSEEVENTF_RIGHTUP,
        (_, true) => MOUSEEVENTF_MIDDLEDOWN, (_, false) => MOUSEEVENTF_MIDDLEUP,
    };
    mouse(f, 0, 0, 0);
}
/// How long keyboard and mouse have been untouched.
pub fn idle_for() -> std::time::Duration {
    use windows_sys::Win32::System::SystemInformation::GetTickCount;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    let mut i = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    // can't tell: somebody could be there
    if unsafe { GetLastInputInfo(&mut i) } == 0 { return std::time::Duration::ZERO; }
    std::time::Duration::from_millis(unsafe { GetTickCount() }.wrapping_sub(i.dwTime) as u64)
}

/// Is Alt held, on the keyboard or by the panel?
pub fn alt_down() -> bool {
    unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(0x12) as u16 & 0x8000 != 0 }
}

pub fn key(c: &Combo, down: bool, up: bool) {
    // Letting go of a held Alt (or Win) on its own opens the menu bar (Start menu): an unassigned
    // key before it makes Windows see a shortcut instead.
    if up && !down && matches!(c.key, 0x12 | 0x5B) { send_combo(&Combo { mods: vec![], key: 0xE8 }, true, true); }
    let (ok, want) = send_combo(c, down, up);
    if ok != want { crate::util::log(&format!("SendInput(keys) accepted {ok}/{want} (elevated window in front?)")); }
}

fn exe_of_pid(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() { return None; }
        let mut buf = [0u16; 1024];
        let mut n = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut n);
        CloseHandle(h);
        if ok == 0 { return None; }
        let full = String::from_utf16_lossy(&buf[..n as usize]);
        full.rsplit('\\').next().map(|s| s.to_lowercase())
    }
}

/// (exe name lower-case, is a modal dialog), or None (no foreground window).
/// A modal dialog is an owned window whose owner is disabled; floating tool panels (e.g.
/// tool palettes) are owned too, but leave their owner enabled.
pub fn foreground() -> Option<(String, bool)> {
    unsafe {
        let hwnd: HWND = GetForegroundWindow();
        if hwnd.is_null() { return None; }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        let owner = GetWindow(hwnd, GW_OWNER);
        let modal = !owner.is_null() && IsWindowEnabled(owner) == 0;
        exe_of_pid(pid).map(|e| (e, modal))
    }
}

/// Exe names (lower-case) of programs with a visible main window, for the editor's "Add app".
pub fn running_apps() -> Vec<String> {
    use windows_sys::core::BOOL;
    use windows_sys::Win32::Foundation::LPARAM;
    use windows_sys::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowLongW, GetWindowTextLengthW, IsWindowVisible, GWL_EXSTYLE, WS_EX_TOOLWINDOW};
    unsafe extern "system" fn cb(h: HWND, out: LPARAM) -> BOOL {
        unsafe {
            let pids = &mut *(out as *mut Vec<u32>);
            let tool = GetWindowLongW(h, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW != 0;
            if IsWindowVisible(h) == 0 || tool || GetWindowTextLengthW(h) == 0 || !GetWindow(h, GW_OWNER).is_null() { return 1; }
            // suspended store apps keep an invisible ("cloaked") window
            let mut cloaked = 0u32;
            let hr = DwmGetWindowAttribute(h, DWMWA_CLOAKED as u32, &mut cloaked as *mut u32 as *mut _, 4);
            if hr == 0 && cloaked != 0 { return 1; }
            let mut pid = 0u32;
            GetWindowThreadProcessId(h, &mut pid);
            if pid != 0 && !pids.contains(&pid) { pids.push(pid); }
        }
        1
    }
    let mut pids: Vec<u32> = vec![];
    if unsafe { EnumWindows(Some(cb), &mut pids as *mut Vec<u32> as LPARAM) } == 0 {
        crate::util::log(&format!("EnumWindows failed: error {}", unsafe { GetLastError() }));
    }
    let mut v: Vec<String> = pids.into_iter().filter_map(exe_of_pid).collect();
    v.sort();
    v.dedup();
    v
}

/// Is a process with this exe name (lower-case) running?
pub fn process_running(exe: &str) -> bool {
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap.is_null() || snap as isize == -1 { return false; }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = false;
        let mut ok = Process32FirstW(snap, &mut e);
        while ok != 0 {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            if String::from_utf16_lossy(&e.szExeFile[..len]).to_lowercase() == exe { found = true; break; }
            ok = Process32NextW(snap, &mut e);
        }
        CloseHandle(snap);
        found
    }
}

/// Wait until process `pid` has ended, at most `limit`.
pub fn wait_for_exit(pid: u32, limit: std::time::Duration) {
    use windows_sys::Win32::System::Threading::{WaitForSingleObject, PROCESS_SYNCHRONIZE};
    unsafe {
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if h.is_null() { return; } // gone already
        WaitForSingleObject(h, limit.as_millis() as u32);
        CloseHandle(h);
    }
}

#[cfg(test)]
#[test]
fn waits_for_a_process_to_end() {
    use std::time::{Duration, Instant};
    let mut c = std::process::Command::new("ping").args(["-n", "3", "127.0.0.1"]).stdout(std::process::Stdio::null()).spawn().unwrap();
    let t = Instant::now();
    wait_for_exit(c.id(), Duration::from_secs(10));
    assert!(t.elapsed() >= Duration::from_millis(1500) && t.elapsed() < Duration::from_secs(8), "{:?}", t.elapsed());
    let _ = c.wait();
    let t = Instant::now();
    wait_for_exit(c.id(), Duration::from_secs(10));
    assert!(t.elapsed() < Duration::from_secs(1), "gone already");
}

/// False if another helper is already running (named mutex, kept for the process lifetime).
pub fn single_instance() -> bool {
    let name: Vec<u16> = "Local\\MicroPanelHelper\0".encode_utf16().collect();
    unsafe {
        let h = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        !(h.is_null() || GetLastError() == ERROR_ALREADY_EXISTS)
    }
}

const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

fn reg(args: &[&str]) -> bool {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("reg").args(args).creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .output().map(|o| o.status.success()).unwrap_or(false)
}
pub fn autostart_enabled() -> bool { reg(&["query", RUN_KEY, "/v", "MicroPanel"]) }
pub fn set_autostart(on: bool) -> bool {
    if on {
        let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
        reg(&["add", RUN_KEY, "/v", "MicroPanel", "/t", "REG_SZ", "/d", &format!("\"{exe}\""), "/f"])
    } else {
        reg(&["delete", RUN_KEY, "/v", "MicroPanel", "/f"])
    }
}

pub fn open_url(url: &str) {
    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url])
        .creation_flags(0x0800_0000).spawn();
}
pub fn open_folder(p: &std::path::Path) {
    let _ = std::process::Command::new("explorer").arg(p).spawn();
}

/// Title of the foreground window ("" if none).
pub fn foreground_title() -> String {
    unsafe {
        let hwnd: HWND = GetForegroundWindow();
        if hwnd.is_null() { return String::new(); }
        // ours: GetWindowTextW would wait for the thread that owns it (the tray), which may be waiting for us
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == std::process::id() { return String::new(); }
        let mut buf = [0u16; 512];
        let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }
}

/// A tray program has no console of its own: use the one it was started from.
pub fn attach_console() {
    unsafe { windows_sys::Win32::System::Console::AttachConsole(windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS); }
}

/// True for keys that must be sent with `KEYEVENTF_EXTENDEDKEY`.
fn is_extended(vk: u16) -> bool {
    matches!(
        vk,
        0x21..=0x28      // pageup, pagedown, end, home, arrows
        | 0x2C           // printscreen
        | 0x2D | 0x2E    // insert, delete
        | 0x5B | 0x5C    // lwin, rwin
        | 0x5D           // apps / menu
        | 0x6F           // numpad divide
        | 0x90           // numlock
        | 0xAD..=0xB3    // volume and media keys
    )
}

/// Builds one keyboard INPUT for a key press or release.
fn key_input(vk: u16, key_up: bool) -> INPUT {
    let mut flags = 0u32;
    if key_up {
        flags |= KEYEVENTF_KEYUP;
    }
    if is_extended(vk) {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    // SAFETY: MapVirtualKeyW only reads its arguments and has no preconditions.
    let scan = unsafe { MapVirtualKeyW(u32::from(vk), MAPVK_VK_TO_VSC) } as u16;
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Sends a combo with `SendInput`.
///
/// When `down` is set, the modifiers are pressed in order and then the key.
/// When `up` is set, the key is released and then the modifiers are released
/// in reverse order. Both flags set gives a full tap; only `down` leaves the
/// keys held.
///
/// Returns `(accepted, requested)`: how many events the system accepted out of
/// how many were built. A value below `requested` usually means input is
/// blocked (for example by UIPI or a secure desktop).
///
/// Note: Ctrl+Alt+Delete is a secure attention sequence and cannot be injected.
fn send_combo(c: &Combo, down: bool, up: bool) -> (u32, u32) {
    let mut inputs: Vec<INPUT> = Vec::new();
    if down {
        inputs.extend(c.mods.iter().map(|&m| key_input(m, false)));
        inputs.push(key_input(c.key, false));
    }
    if up {
        inputs.push(key_input(c.key, true));
        inputs.extend(c.mods.iter().rev().map(|&m| key_input(m, true)));
    }

    let requested = inputs.len() as u32;
    if requested == 0 {
        return (0, 0);
    }
    let cb_size = std::mem::size_of::<INPUT>() as i32;
    // SAFETY: `inputs` is a valid, initialized slice of `requested` INPUT
    // structs, and `cb_size` is the size of one INPUT.
    let accepted = unsafe { SendInput(requested, inputs.as_ptr(), cb_size) };
    (accepted, requested)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_extended_flags() {
        for vk in [
            0x25u16, 0x26, 0x27, 0x28, 0x2D, 0x2E, 0x24, 0x23, 0x21, 0x22, 0x5B, 0x5D, 0x2C, 0x6F,
            0x90, 0xAD, 0xAE, 0xAF, 0xB0, 0xB1, 0xB3,
        ] {
            assert!(is_extended(vk), "vk {vk:#x} should be extended");
        }
        for vk in [0x41u16, 0x11, 0x10, 0x12, 0x0D, 0x20, 0x60, 0x70] {
            assert!(!is_extended(vk), "vk {vk:#x} should not be extended");
        }
    }
}
