//! Minimal no-op child for probe-pool tests: runs, exits 0, and never
//! writes ANY file — the platform-neutral stand-in for "a game that
//! exits without reporting" (no reliance on PATH-resolved `true`,
//! which is unix-only and left the stale-report gate unpinned on
//! Windows). See tests/probe_pool.rs:
//! stale_report_cannot_masquerade_as_a_fresh_success.
//!
//! Deliberately NOT an `App` run: a plugin-less Bevy app under the
//! stock runner would spin until an `AppExit` nobody sends. The test
//! only needs a process that STARTS and EXITS cleanly without
//! touching the pool's report path.
//!
//! No `#[cfg(not(feature = "takeover"))]` fallback arm: the
//! `[[bin]]` entry carries `required-features = ["takeover"]`, so
//! this binary is only ever BUILT with the feature on — an arm for
//! the off case would be unreachable dead code (and would silently
//! mask a future removal of `required-features`).
fn main() {}
