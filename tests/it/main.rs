//! Single integration-test binary for the harness.
//!
//! Every suite is a module here instead of its own `tests/*.rs` target.
//! Each separate target statically linked a full debug Bevy (~400 MB per
//! binary), so ~30 targets meant ~30 link steps and a multi-GB `target/`
//! (and CI cache). One binary links once. Under cargo-nextest every test
//! still runs in its own process, so per-process exclusivity
//! (`run_scenario`'s capture state) is preserved.
//!
//! Under plain `cargo test` all suites share ONE process, so any test
//! that calls `run_scenario`, mutates the environment, or spawns
//! children that read `BEVY_PLAYTEST_*` variables must hold
//! `common::run_slot()` for its whole body.
//!
//! EXCEPTION: the `tests/log_*.rs` suites stay separate binaries. They
//! claim write-once process globals (the `log` logger slot, tracing's
//! global default) irreversibly, which would poison every other test
//! sharing their process under plain `cargo test`.
//!
//! Run one suite with e.g. `cargo test --test it probe_pool::` or
//! `cargo nextest run -E 'test(/^probe_pool::/)'`.

#[path = "../common/mod.rs"]
mod common;

mod battery;
mod census;
mod component_check;
mod driver_smoke;
mod explorer;
mod fault_injection;
mod frozen_world_detail;
mod input_delivery;
mod isolation;
mod negative_validation;
mod probe_pool;
mod progress_liveness;
// Drives the bevy_camera/bevy_render-typed API surface, which only
// exists under the feature; gated so plain `cargo test` stays green.
#[cfg(feature = "render-liveness")]
mod render_liveness;
mod report_integrity;
mod scan_purity;
mod scenario_schema;
// Drives the capture rig and, for its GPU-positive cases, the real
// renderer; both only exist under the feature.
#[cfg(feature = "screen-liveness")]
mod screen_liveness;
mod state_digest;
mod state_trace;
mod takeover_e2e;
mod ui_liveness;
mod ui_surface;
mod vacuous_invariant_rule;
mod verification_depth;
mod wrapped_keyboard;
