//! The tray icon.

use crate::app::{Shared, EDITOR_PORT};
use crate::util::{config_dir, log};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn icon() -> tray_icon::Icon {
    // 32x32: a dark ring with three dots (the three balls)
    let n = 32usize;
    let mut px = vec![0u8; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            let (fx, fy) = (x as f32 + 0.5 - 16.0, y as f32 + 0.5 - 16.0);
            let r = (fx * fx + fy * fy).sqrt();
            let mut c = if r < 15.5 { [40u8, 44, 52, 255] } else { [0, 0, 0, 0] };
            for cx in [-8.0f32, 0.0, 8.0] {
                let (dx, dy) = (fx - cx, fy - 2.0);
                if (dx * dx + dy * dy).sqrt() < 3.4 { c = [235, 235, 235, 255]; }
            }
            if fy < -6.0 && fy > -9.0 && fx.abs() < 9.0 { c = [120, 200, 120, 255]; }
            px[(y * n + x) * 4..(y * n + x) * 4 + 4].copy_from_slice(&c);
        }
    }
    tray_icon::Icon::from_rgba(px, n as u32, n as u32).expect("icon")
}

/// The tray icon and its menu, until the helper stops.
pub fn tray(shared: Arc<Mutex<Shared>>, logic: std::thread::JoinHandle<()>) {
    use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
    use tray_icon::TrayIconBuilder;
    use windows_sys::Win32::UI::WindowsAndMessaging::{DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE};

    let status = MenuItem::new("Starting…", false, None);
    let pause = CheckMenuItem::new("Pause", true, false, None);
    let edit = MenuItem::new("Edit mappings…", true, None);
    let sheet_show = CheckMenuItem::new("Show", true, crate::sheet::get().open, None);
    let sheet_list = CheckMenuItem::new("As a list", true, crate::sheet::get().list, None);
    let sheet_through = CheckMenuItem::new("Clicks go through it", true, crate::sheet::get().click_through, None);
    let sheet_menu = Submenu::new("Cheat sheet", true);
    let _ = sheet_menu.append_items(&[&sheet_show, &sheet_list, &sheet_through]);
    let folder = MenuItem::new("Open config folder", true, None);
    let reload = MenuItem::new("Reload profiles", true, None);
    let auto = CheckMenuItem::new("Start with Windows", true, super::autostart_enabled(), None);
    let restart = MenuItem::new("Restart", true, None);
    let quit = MenuItem::new("Quit", true, None);
    let menu = Menu::new();
    let _ = menu.append_items(&[&status, &PredefinedMenuItem::separator(), &pause, &edit, &sheet_menu, &folder, &reload,
        &PredefinedMenuItem::separator(), &auto, &restart, &quit]);
    let tray = match TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip("Micro Color Panel").with_icon(icon()).build() {
        Ok(t) => Some(t),
        Err(e) => { log(&format!("tray icon failed: {e}; running without one")); None }
    };
    let rx = MenuEvent::receiver();
    let mut t_status = Instant::now() - Duration::from_secs(5);
    loop {
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        while let Ok(ev) = rx.try_recv() {
            let id = ev.id;
            if id == *pause.id() {
                shared.lock().unwrap().paused = pause.is_checked();
            } else if id == *edit.id() {
                super::open_url(&format!("http://127.0.0.1:{EDITOR_PORT}/"));
            } else if id == *sheet_show.id() {
                crate::sheet::update(|c| c.open = sheet_show.is_checked());
            } else if id == *sheet_list.id() {
                crate::sheet::update(|c| c.list = sheet_list.is_checked());
            } else if id == *sheet_through.id() {
                crate::sheet::update(|c| c.click_through = sheet_through.is_checked());
            } else if id == *folder.id() {
                super::open_folder(&config_dir());
            } else if id == *reload.id() {
                shared.lock().unwrap().reload = true;
            } else if id == *auto.id() {
                let want = auto.is_checked();
                if !super::set_autostart(want) { log("autostart change failed"); }
                auto.set_checked(super::autostart_enabled());
            } else if id == *restart.id() {
                if crate::app::restart("tray menu").is_ok() { shared.lock().unwrap().quit = true; }
            } else if id == *quit.id() {
                shared.lock().unwrap().quit = true;
            }
        }
        if t_status.elapsed() >= Duration::from_millis(500) {
            t_status = Instant::now();
            let sc = crate::sheet::get();
            for (m, on) in [(&sheet_show, sc.open), (&sheet_list, sc.list), (&sheet_through, sc.click_through)] { if m.is_checked() != on { m.set_checked(on); } }
            let sh = shared.lock().unwrap().clone();
            let panel = match (&sh.panel_link, sh.battery) {
                (Some(l), Some(b)) => format!("Panel: {} · {}%{}{}", l.to_uppercase(), b, if sh.charging { " ⚡" } else { "" }, if sh.resting { " (resting)" } else { "" }),
                (Some(l), None) => format!("Panel: {}", l.to_uppercase()),
                _ if sh.yielded_to_resolve => "Panel: with DaVinci Resolve".into(),
                _ => "Panel: not connected".into(),
            };
            let ctx = sh.context.first().cloned().unwrap_or_default();
            let layer = if sh.layers.is_empty() { String::new() } else { format!(" [{}]", sh.layers.join(",")) };
            let text = format!("{panel}\nContext: {ctx}{layer}{}", if sh.profile_error.is_some() { "\n⚠ profiles.toml has an error" } else { "" });
            status.set_text(text.lines().next().unwrap_or(""));
            if let Some(t) = &tray { let _ = t.set_tooltip(Some(&text)); }
        }
        if logic.is_finished() { break; }
        std::thread::sleep(Duration::from_millis(30));
    }
    log("stopped");
}
