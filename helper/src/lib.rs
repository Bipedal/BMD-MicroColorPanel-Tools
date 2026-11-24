//! Micro Color Panel helper: panel driver, profiles, actions, mapping editor, cheat sheet.
//! Knows no application; integrations plug in through `ext`.
pub mod util;
pub mod panel;
pub mod keys;
pub mod profiles;
pub mod engine;
pub mod sys;
pub mod ext;
pub mod osc;
pub mod cli;
pub mod app;
pub mod edit;
pub mod leds;
pub mod midi;
pub mod sheet;
pub mod presets;
