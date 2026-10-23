//! Isolation test battery: prove runs don't contaminate each other.
//! The panic/log capture machinery is process-global (an exclusively
//! held active-run buffer + a global panic hook), so state leakage
//! between runs is a realistic failure mode, especially under cargo's
//! multi-test-thread harness.
//!
//! The exclusive-run design makes cross-run contamination structurally
//! impossible (a second concurrent run is rejected; straggler events
//! after a run ends are dropped). These tests pin that contract.

// Shared test harness (run-slot serialization)
#[path = "common/mod.rs"]
mod common;

use bevy_app::App;
use bevy_playtest::run_scenario;
use bevy_playtest::scenario::Scenario;
use bevy_playtest::sinks;
use std::sync::{Mutex, MutexGuard, OnceLock};

/// The panic-hook routing in these tests targets PROCESS-GLOBAL state
/// (orphan panics land in the next active run anywhere in this test
/// binary). cargo runs `#[test]`s on parallel threads, so without
/// serialization test A's "orphan panic" surfaces in test B's report —
/// cross-test interference, exactly the leakage these tests police.
/// One shared lock makes the whole battery sequential.
fn battery_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

fn minimal_app() -> App {
    sinks::ensure_global_subscriber();
    App::new()
}

/// A moving entity keeps the runs observable for the oracles
/// without asserting anything about it.
fn spawn_observable_entity(app: &mut App) {
    app.add_systems(
        bevy_app::Update,
        |mut q: bevy_ecs::prelude::Query<&mut bevy_transform::prelude::Transform>| {
            for mut t in &mut q {
                t.translation.x += 1.0;
            }
        },
    );
    app.world_mut()
        .spawn(bevy_transform::prelude::Transform::from_xyz(0.0, 0.0, 0.0));
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
    let _slot = common::run_slot();
    let _battery = battery_lock();
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
    spawn_observable_entity(&mut app2);
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

/// Direct probe of the exclusive-run contract: after a completed run
/// released the buffer, drain() must yield nothing (no run is active).
#[test]
fn drain_returns_only_current_run_events() {
    let _slot = common::run_slot();
    let _battery = battery_lock();
    let mut app = minimal_app();
    spawn_observable_entity(&mut app);
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

/// Nested run_scenario (a run started from inside a run) is rejected
/// loudly rather than deadlocking on the run serialization mutex.
#[test]
fn nested_run_is_rejected() {
    let _slot = common::run_slot();
    let _battery = battery_lock();

    fn nested_system() {
        let inner = run_scenario(
            &mut bevy_app::App::new(),
            &serde_json::from_str(r#"{"bot":{"type":"chaos","seed":1},"duration_s":0.1,"tps":60}"#)
                .unwrap(),
        );
        assert!(inner.is_err(), "nested run must be rejected");
    }

    let mut app = minimal_app();
    spawn_observable_entity(&mut app);
    app.add_systems(bevy_app::Update, nested_system);
    let report = run_scenario(&mut app, &chaos_scenario(0.2)).unwrap();
    // The outer run itself completes cleanly (the nested rejection
    // happens inside a game system and does not crash the run).
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
}

/// A panic with no active run (post-run straggler) is DROPPED, never
/// reattributed to a later run — the run that owned it is gone; a new
/// run must not inherit false failures from it.
#[test]
fn post_run_panic_is_dropped_not_reattributed() {
    let _slot = common::run_slot();
    let _battery = battery_lock();
    // Install the harness's panic hook explicitly.
    bevy_playtest::sinks::install_panic_hook();

    // Run one scenario to completion.
    let mut app = minimal_app();
    spawn_observable_entity(&mut app);
    let report = run_scenario(&mut app, &chaos_scenario(0.2)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);

    // Panic with no active run (post-run window). The hook should drop
    // it — there is no honest owner to attribute it to.
    let _ = std::panic::catch_unwind(|| panic!("post-run boom"));

    // The next run must NOT inherit that panic.
    let mut app2 = minimal_app();
    spawn_observable_entity(&mut app2);
    let report2 = run_scenario(&mut app2, &chaos_scenario(0.2)).unwrap();
    assert!(
        !report2
            .violations
            .iter()
            .any(|v| v.rule == "panic" && v.detail.contains("post-run boom")),
        "post-run panics must be dropped, not reattributed: {:?}",
        report2.violations
    );
}

/// A panic unwinding through a run's guard poisons RUN_SERIAL (the
/// MutexGuard drops mid-panic). Poison must be RECOVERED, not treated
/// as "run active" — otherwise one setup panic permanently rejects
/// every later run_scenario in the process.
#[test]
fn poisoned_run_serial_is_recovered_not_sticky() {
    let _slot = common::run_slot();
    let _battery = battery_lock();

    // Simulate a setup-phase panic: acquire a run, panic while the
    // guard is held (RunGuard::drop runs during the unwind), then
    // verify the process is still usable.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = sinks::try_acquire_run().expect("second acquisition");
        panic!("boom while holding guard");
    }));
    assert!(result.is_err(), "inner panic must unwind");

    // THE PIN: the poisoned mutex must not read as "run active".
    let report_app = minimal_app();
    drop(report_app);
    let guard = sinks::try_acquire_run();
    assert!(
        guard.is_ok(),
        "poisoned RUN_SERIAL must be recovered, not sticky: {:?}",
        guard.err()
    );
    drop(guard.expect("guard"));
}
