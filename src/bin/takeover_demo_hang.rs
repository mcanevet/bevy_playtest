//! Minimal runaway child for the probe-pool watchdog test: starts
//! and then NEVER exits on its own — the stand-in for "a stuck game"
//! that the pool's wall-clock watchdog must kill (see
//! tests/probe_pool.rs: runaway_child_is_killed_by_the_wall_clock_watchdog).
//!
//! Deliberately NOT an `App` run: the demo apps advance SIMULATED
//! time in a tight CPU loop with no pacing, so their wall-clock
//! runtime scales with machine speed — a fast runner could finish a
//! 30s-simulated run inside the 1s deadline and the watchdog would
//! never fire (an observed ~3.5x on an M-series Mac). A real child
//! that must outrun the deadline on EVERY machine cannot rely on
//! doing work; it must rely on doing NOTHING. `park()` blocks
//! forever with zero syscalls to spin, and the OS still delivers
//! the kill signal normally.
//!
//! No `#[cfg(not(feature = "takeover"))]` fallback arm: the
//! `[[bin]]` entry carries `required-features = ["takeover"]`, so
//! this binary is only ever BUILT with the feature on — an arm for
//! the off case would be unreachable dead code (and would silently
//! mask a future removal of `required-features`).
fn main() {
    std::thread::park();
}
