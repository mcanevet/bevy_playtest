//! bevy_playtest: playtesting harness for Bevy games.

#[cfg(feature = "takeover")]
pub mod battery;
pub mod choice;
pub mod driver;
pub mod enums;
pub mod invariant_eval;
pub mod junit;
pub mod oracles;
#[cfg(feature = "takeover")]
pub mod plugin;
pub mod replay;
pub mod rules;
pub mod scenario;
pub mod sinks;
pub mod state;

/// Convenience re-export: run a scenario, get a report.
pub use driver::run_scenario;

#[cfg(feature = "takeover")]
pub use battery::BatteryConfig;
#[cfg(feature = "takeover-winit")]
pub use plugin::PlaytestDefaultPlugins;
#[cfg(feature = "takeover")]
pub use plugin::{exit_code, PlaytestPlugin};
