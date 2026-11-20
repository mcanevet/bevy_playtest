//! What the harness can learn about a game's input surface and world.
//!
//! - [`source_scan`]: milliseconds-cost text scan of the game's sources
//!   for `KeyCode::` variants; aims the battery's default chaos surface.
//! - [`census`]: load-time capability census, default-on.
//! - [`state_trace`]: which state schedules ran and which game
//!   resources changed, per run (generic; no reflection needed).
//! - [`static_scan`]: schedule walk classifying systems by the input
//!   signals they read (by ComponentId, never by name).
//! - `lw` (`lw-reader`): Leafwing declared tier — reader, intent-tap,
//!   and reconcile, stamped on every report as `lw_reconciled`.
//! - `ui_surface` (`ui-surface`): declared UI/interactable enumeration.
//! - `render_liveness` / `screen_liveness`: camera-health and
//!   pixel-observed liveness probes.
//! - [`surface`], [`provenance`], [`diff`]: shared report/IR types and
//!   digest-trace diffing.
//! - `probe_pool` (`takeover`) + [`probe_scenarios`]: subprocess worker
//!   pool and paired probe scenarios — kept as the substrate for
//!   state-frontier exploration (to be reworked into an episode
//!   runner); the probe-vs-control discovery flow built on them was
//!   removed.

pub mod census;
pub mod diff;
#[cfg(feature = "lw-reader")]
pub mod lw;
#[cfg(feature = "takeover")]
pub mod probe_pool;
pub mod probe_scenarios;
pub mod provenance;
#[cfg(feature = "render-liveness")]
pub mod render_liveness;
#[cfg(feature = "screen-liveness")]
pub mod screen_liveness;
pub mod source_scan;
pub mod state_trace;
pub mod static_scan;
pub mod surface;
#[cfg(feature = "ui-surface")]
pub mod ui_surface;
