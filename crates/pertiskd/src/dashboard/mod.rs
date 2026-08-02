//! Serial console status banner (text, no TUI alternate screen).

mod snapshot;
mod ui;

pub use ui::{should_enable_dashboard, start_dashboard};
