//! Everything that depends on the operating system, behind one set of functions.
//!
//! `windows` is complete. `portable` runs anywhere, but only does what plain Rust can: the panel, the
//! editor, layers, lights, presets and OSC work; key and mouse output, MIDI, the cheat-sheet window,
//! the tray and telling which program is in front do not. A port replaces those functions one by one.
//!
//! `--features portable` builds the portable one on Windows too, to check that it still compiles.
//!
//! What a system provides:
//!   mouse_move, scroll, mouse_button, key     output
//!   alt_down     is Alt held (by anyone)
//!   idle_for     how long keyboard and mouse have been untouched
//!   foreground, foreground_title, running_apps, process_running     which programs there are
//!   single_instance, autostart_enabled, set_autostart, open_url, open_folder, attach_console, tray
//!   keyrec      record a key combination system-wide
//!   midi        ports
//!   sheet_window     the cheat sheet

#[cfg(all(windows, not(feature = "portable")))]
mod windows;
#[cfg(all(windows, not(feature = "portable")))]
pub use windows::*;

#[cfg(not(all(windows, not(feature = "portable"))))]
mod portable;
#[cfg(not(all(windows, not(feature = "portable"))))]
pub use portable::*;

/// A key recording: see `keyrec`.
#[derive(Clone, Debug, PartialEq)]
pub enum RecState { Idle, Waiting, Done(String), TimedOut }
