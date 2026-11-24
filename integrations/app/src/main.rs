//! The helper as it is released: the core with its integrations (OSC, Lightroom Classic).
#![cfg_attr(not(test), windows_subsystem = "windows")]
use std::sync::Arc;

fn main() {
    let mut all = micropanel::ext::builtin();
    all.push(Arc::new(micropanel_lightroom::Lightroom::new()));
    micropanel::cli::main(all);
}
