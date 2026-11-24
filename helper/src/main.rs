//! The helper with the integrations it comes with. See INTEGRATIONS.md for adding more.
#![cfg_attr(not(test), windows_subsystem = "windows")]

fn main() { micropanel::cli::main(micropanel::ext::builtin()) }
