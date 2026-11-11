//! Crash-simulating child for the battery's GPU-panic diagnosis
//! regression (tests/battery.rs:
//! gpu_panic_gets_named_failure_with_remediation): reproduces —
//! WITHOUT needing a GPU-less host — what a taken-over game prints
//! when bevy_render's `initialize_renderer` finds no adapter, then
//! dies with the standard Rust panic exit code (101) and no report.
//!
//! The printed string is the exact `GPU_NOT_FOUND_ERROR_MESSAGE`
//! panic from bevy_render 0.20.1 (src/renderer/mod.rs:308) — more
//! precisely the LINUX arm of the cfg-split constant, which appends
//! a linux_dependencies.md URL (other platforms stop at "required
//! drivers!"). The battery matches only the PLATFORM-COMMON PREFIX
//! (`GPU_PANIC_MARKER` in src/battery.rs), so exercising the long
//! Linux arm here proves the match survives both arms.
//!
//! Deliberately does NOT panic for real (not an App run): the test
//! observes the parent's DIAGNOSIS of the stderr text, not any
//! actual renderer behavior — the string below is byte-quoted from
//! the vendored bevy_render source, so a bevy upgrade that changes
//! it breaks this child's own `cargo test` run, loudly.
//!
//! No `#[cfg(not(feature = "takeover"))]` fallback arm: the
//! `[[bin]]` entry carries `required-features = ["takeover"]`, so
//! this binary is only ever BUILT with the feature on.
fn main() {
    eprintln!(
        "thread 'main' panicked at bevy_render-0.20.1/src/renderer/mod.rs:308:5:\n\
         Unable to find a GPU! Make sure you have installed required drivers! \
         For extra information, see: \
         https://github.com/bevyengine/bevy/blob/latest/docs/linux_dependencies.md\n\
         note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace"
    );
    std::process::exit(101);
}
