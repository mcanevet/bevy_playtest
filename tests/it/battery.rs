#![cfg(feature = "takeover")]

//! Battery-runner regressions: child cwd/HOME isolation and
//! no-report death diagnosis. These pin two behaviors: the
//! cwd/HOME treatment the probe pool already guarantees its own
//! children (#74 — the battery spawns children the same way and
//! deserves the same guarantee), and the diagnosis of a child that
//! dies without a report — a GPU-adapter panic at startup gets a
//! NAMED failure with host-side remediation instead of an opaque
//! exit code.
//!
//! The battery runner spawns the game as a REAL subprocess
//! (`crate::battery::run_with_config`), so these tests exercise
//! actual OS process spawn/exit — no `run_scenario` exclusivity
//! involved, no run-slot serialization needed.

use bevy_playtest::battery::{run_with_config, BatteryConfig, BatteryError, BUILTIN_BATTERY};

#[test]
fn battery_child_runs_in_its_own_cwd_and_home() {
    // Isolation regression: battery children used to inherit the
    // PARENT's working directory, so a game writing anything
    // `./`-relative (auto-saves, config, logs) scribbled into
    // wherever `cargo test` happened to run — the repo tree in CI.
    // The battery must run every child in its own throwaway
    // directory and repoint HOME there. Unlike the probe pool
    // (whose per-job workspace survives long enough for the test to
    // inspect markers), the battery's temp dir is RAII-removed on
    // every return path, so the child self-checks and the test
    // asserts on its exit code relayed through the no-report
    // failure.
    let err = run_with_config(&BatteryConfig {
        binary: env!("CARGO_BIN_EXE_takeover_demo_cwd_check").to_string(),
        scenario: BUILTIN_BATTERY.to_string(),
        timeout_secs: 30,
    })
    .expect_err("marker child writes no report, so the run must fail");
    // The child exits 0 after its self-checks — that refusal to
    // report is itself proof it really ran to completion, and the
    // exit code relays the self-check verdict: exit 0 means the cwd
    // was isolated, HOME repointed, and ./-relative reads still
    // resolve through the symlink tree.
    match &err {
        BatteryError::NoReport { exit_code } => assert_eq!(
            exit_code,
            &Some(0),
            "child must have completed its self-checks (exit 0), got {err}"
        ),
        other => panic!("marker-writing child must yield the no-report refusal, got: {other:?}"),
    }
    // Self-checks passed => cwd was NOT the parent's directory and
    // HOME WAS the isolated cwd; markers were written INSIDE the
    // throwaway dir, which the RAII guard has since removed. Belt
    // and braces: nothing leaked into THIS process's cwd either.
    let parent_cwd = std::env::current_dir().expect("parent cwd");
    assert!(
        !parent_cwd.join("cwd_marker.txt").exists(),
        "isolation failed: the child wrote into the PARENT's cwd"
    );
    assert!(
        !parent_cwd.join("home_marker.txt").exists(),
        "isolation failed: the child's HOME-resolved write landed in the PARENT's cwd"
    );
}

#[test]
fn gpu_panic_gets_named_failure_with_remediation() {
    // A child dying with bevy_render's GPU_NOT_FOUND_ERROR_MESSAGE
    // on its stderr (exact Linux-arm string, with URL suffix) and
    // no report must yield the NAMED GpuMissing failure whose
    // Display names the cause (renderer still needs an adapter
    // headless) and the fix (mesa-vulkan-drivers / the how-to doc).
    // Simulated — no GPU-less host needed: the battery diagnoses
    // the child's stderr TEXT, not real renderer behavior.
    let err = run_with_config(&BatteryConfig {
        binary: env!("CARGO_BIN_EXE_takeover_demo_gpu_panic").to_string(),
        scenario: BUILTIN_BATTERY.to_string(),
        timeout_secs: 30,
    })
    .expect_err("GPU-panic child writes no report, so the run must fail");
    match &err {
        BatteryError::GpuMissing { .. } => {}
        other => panic!("GPU-panic child must yield GpuMissing, got: {other:?}"),
    }
    let msg = err.to_string();
    assert!(
        msg.contains("GPU adapter"),
        "remediation must name the cause (missing GPU adapter): {msg}"
    );
    assert!(
        msg.contains("mesa-vulkan-drivers"),
        "remediation must name the CI-host fix: {msg}"
    );
    assert!(
        msg.contains("docs/how-to/headless-ci-gpu.md"),
        "remediation must point at the how-to doc: {msg}"
    );
}

#[test]
fn generic_panic_keeps_plain_no_report_failure() {
    // Negative control: a generic panic (no GPU marker substring)
    // with no report must keep the PLAIN NoReport verdict —
    // detection must never mask or rewrite other failures.
    let err = run_with_config(&BatteryConfig {
        binary: env!("CARGO_BIN_EXE_takeover_demo_panic_generic").to_string(),
        scenario: BUILTIN_BATTERY.to_string(),
        timeout_secs: 30,
    })
    .expect_err("generic-panic child writes no report, so the run must fail");
    match &err {
        BatteryError::NoReport { exit_code } => assert_eq!(exit_code, &Some(101)),
        other => panic!("generic-panic child must yield plain NoReport, got: {other:?}"),
    }
}

#[test]
fn bounded_drain_survives_a_held_pipe() {
    // Grandchild-holds-the-pipe regression (timeout path): the child
    // parks forever, so the watchdog trips; its parked GRANDCHILD
    // inherits the stderr pipe and keeps the write end open, so the
    // tee never sees EOF. The old unbounded join() blocked FOREVER
    // on exactly this shape — turning the watchdog's bounded kill
    // into a whole-job hang. The drain must be BOUNDED: the run
    // returns the Timeout verdict within the timeout plus the drain
    // grace, never hanging.
    let started = std::time::Instant::now();
    let err = run_with_config(&BatteryConfig {
        binary: env!("CARGO_BIN_EXE_takeover_demo_stderr_grandchild").to_string(),
        scenario: BUILTIN_BATTERY.to_string(),
        timeout_secs: 3,
    })
    .expect_err("parking child writes no report, so the run must fail");
    match &err {
        BatteryError::Timeout { timeout_secs } => assert_eq!(timeout_secs, &3),
        other => panic!("parking child must trip the watchdog, got: {other:?}"),
    }
    // The bound must FAIL against an unbounded (or absurdly
    // lenient) drain, not merely pass on a fast machine: the
    // expected run time is ~3s (watchdog) and a generous drain
    // window would still land near 6s, so 10s leaves room for CI
    // scheduling while keeping a sloppy drain (e.g. a grace in the
    // minutes) a visible FAILURE, not a green run. The parked
    // grandchild (60s) outlives this bound, so the pipe is held
    // for the WHOLE run — the test cannot pass by racing the
    // grandchild's death.
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "the run hung: watchdog tripped at 3s but the run took \
         {:.1}s — the stderr tee drain must be bounded",
        started.elapsed().as_secs_f32()
    );
}

#[test]
fn held_pipe_does_not_block_exit_diagnosis() {
    // Grandchild-holds-the-pipe regression (NORMAL path): the child
    // exits promptly (with 101, no report) while its parked
    // grandchild keeps the stderr pipe open — the tee never sees
    // EOF even though the direct child is reaped. The verdict must
    // still arrive bounded, as plain NoReport (the child's stderr
    // carries no GPU marker), with the grandchild's pipe-holding
    // unable to stall it. Same 10s-bound rationale as the
    // timeout-path sibling above.
    let started = std::time::Instant::now();
    let err = run_with_config(&BatteryConfig {
        binary: env!("CARGO_BIN_EXE_takeover_demo_stderr_grandchild_exit").to_string(),
        scenario: BUILTIN_BATTERY.to_string(),
        timeout_secs: 3,
    })
    .expect_err("grandchild-spawning child writes no report, so the run must fail");
    match &err {
        BatteryError::NoReport { exit_code } => assert_eq!(exit_code, &Some(101)),
        other => panic!("exit-101 child must yield plain NoReport, got: {other:?}"),
    }
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "the run hung: the child exited promptly but the stderr tee \
         drain must not block on the parked grandchild's pipe \
         ({:.1}s)",
        started.elapsed().as_secs_f32()
    );
}
