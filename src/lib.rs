//! bevy_playtest: playtesting harness for Bevy games.

pub mod brp;
#[cfg(feature = "brp")]
pub mod brp_runner;
pub mod choice;
pub mod contract;
pub mod driver;
pub mod enums;
pub mod invariant_eval;
pub mod junit;
pub mod oracles;
#[cfg(feature = "takeover")]
pub mod plugin;
pub mod rules;
pub mod scenario;
pub mod sinks;
pub mod state;

/// Convenience re-export: run a scenario, get a report.
pub use driver::run_scenario;
#[cfg(feature = "takeover")]
pub use plugin::{exit_code, PlaytestPlugin};
