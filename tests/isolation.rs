//! Isolation test battery: prove runs don't contaminate each other.
//! The panic/log capture machinery is process-global (thread-local
//! sinks + a global panic hook), so state leakage between runs is a
//! realistic failure mode, especially under cargo's multi-test-process
//! harness.
//!
//! These tests currently PASS — the reviewer's C6 hypothesis (sink
//! residue leaking between runs) does not reproduce with completed or
//! panic-aborted runs, since run_scenario drains the sink on every
//! exit path. Kept as regression guards; the remaining theoretical
//! gap (a run dropped mid-flight by the caller) is untestable from
//! the public API.

use bevy_app::App;
use bevy_playtest::run_scenario;
use bevy_playtest::scenario::Scenario;
use bevy_playtest::sinks;

fn minimal_app() -> App {
    sinks::ensure_global_subscriber();
    App::new()
}

fn chaos_scenario(duration_s: f32) -> Scenario {
    serde_json::from_str(&format!(
        r#"{{"bot":{{"type":"chaos","seed":7}},"duration_s":{duration_s},"tps":60}}"#
    ))
    .unwrap()
}

/// An aborted first run must not leak captured events into a second
/// run's report. The SINK is never cleared at run start (only drained
/// at end), so a run that errors before draining leaves residue for
/// the next run on the same thread (previously suspected regression).
#[test]
fn aborted_run_does_not_leak_into_next_run() {
    // First run: a system panics mid-run. The harness catches it and
    // the report classifies Crash (events drained at run end).
    let mut app1 = minimal_app();
    app1.add_systems(bevy_app::Update, || panic!("first-run boom"));
    let first = run_scenario(&mut app1, &chaos_scenario(0.2)).unwrap();
    assert_ne!(
        first.status,
        bevy_playtest::enums::PlaytestStatus::Pass,
        "sanity: first run must not pass while panicking"
    );

    // Second run on the same thread, clean app, nothing bad happens.
    let mut app2 = minimal_app();
    let second = run_scenario(&mut app2, &chaos_scenario(0.2)).unwrap();

    // The residue assertion: a clean second run must be clean. If the
    // first run's panic residue leaked, status or error would reflect
    // a crash that didn't happen in THIS run.
    assert_eq!(second.status, bevy_playtest::enums::PlaytestStatus::Pass);
    assert!(
        second.error.is_none(),
        "second run inherited residue from the aborted first run: {:?}",
        second.error
    );
    assert!(
        second.violations.iter().all(|v| !v.rule.contains("panic")),
        "second run inherited panic violations from run #1: {:?}",
        second.violations
    );
}

/// Direct probe of the sink-clearing contract: whatever a prior test
/// left in the thread-local SINK must not surface via drain() after a
/// fresh run starts.
#[test]
fn drain_returns_only_current_run_events() {
    let mut app = minimal_app();
    let report = run_scenario(&mut app, &chaos_scenario(0.2)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);

    // After a completed run the sink is drained; a subsequent drain
    // must be empty. If run_scenario drains only on some paths, stale
    // events accumulate for the next run on this thread.
    let leaked = sinks::drain();
    assert!(
        leaked.is_empty(),
        "sink not empty after a completed run drained it: {leaked:?}"
    );
}
