//! Negative-control crash child for the battery's GPU-panic
//! diagnosis regression (tests/battery.rs:
//! generic_panic_keeps_plain_no_report_failure): prints an ordinary
//! panic message — one WITHOUT the GPU marker substring — to
//! stderr and dies with the standard Rust panic exit code (101)
//! and no report. The battery must treat it EXACTLY as before:
//! the plain `BatteryError::NoReport` verdict, never the GPU
//! remediation (detection must not mask or rewrite other
//! failures).
//!
//! Deliberately does NOT panic for real (not an App run): the test
//! observes the parent's CLASSIFICATION of the stderr text.
//!
//! No `#[cfg(not(feature = "takeover"))]` fallback arm: the
//! `[[bin]]` entry carries `required-features = ["takeover"]`, so
//! this binary is only ever BUILT with the feature on.
fn main() {
    eprintln!(
        "thread 'main' panicked at src/game.rs:42:9:\n\
         index out of bounds: the len is 3 but the index is 5\n\
         note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace"
    );
    std::process::exit(101);
}
