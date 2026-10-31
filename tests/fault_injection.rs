//! Fault-injection tests: prove the harness DETECTS the violations it
//! claims to detect. Each test builds a minimal app with a deliberate
//! fault and asserts the resulting report contains the expected
//! violation rule (and not merely a passing run).
//!
//! A harness that only ever sees healthy games proves nothing; these
//! tests are the negative controls for the oracle suite:
//! - `finite_transforms`  (NaN translation)
//! - `nodes_in_bounds`    (entity teleported far outside bounds)
//! - `frozen_world`        (no Transform change for > grace frames)
//! - `panic`              (system panics mid-run)
//! - `log_error`           (error-level log emitted)

// Shared test harness (run-slot serialization)
#[path = "common/mod.rs"]
mod common;

use bevy_app::App;
use bevy_ecs::prelude::*;
use bevy_playtest::enums::PlaytestStatus;
use bevy_playtest::run_scenario;
use bevy_playtest::scenario::Scenario;
use bevy_transform::prelude::Transform;

fn short_scenario() -> Scenario {
    serde_json::from_str(
        r#"{
            "name": "fault-injection",
            "bot": { "type": "chaos", "seed": 42 },
            "duration_s": 1.0,
            "tps": 60,
            "invariants": []
        }"#,
    )
    .expect("scenario parses")
}

/// Longer variant for the frozen-world test: the driver's grace period
/// is 180 frames (3s at 60 TPS), so the run must comfortably exceed it.
/// The scenario arms the liveness oracle (gating) with a 2s timeout,
/// ensuring a static world fails the run.
fn long_scenario() -> Scenario {
    serde_json::from_str(
        r#"{
            "name": "fault-injection-long",
            "bot": { "type": "chaos", "seed": 42 },
            "duration_s": 5.0,
            "tps": 60,
            "invariants": [],
            "liveness": { "mode": "real_time", "timeout_s": 2.0 }
        }"#,
    )
    .expect("scenario parses")
}

/// True if the report failed with a violation of the given rule.
fn violated_with(status: PlaytestStatus, rules: &[String], rule: &str) -> bool {
    status != PlaytestStatus::Pass && rules.iter().any(|r| r == rule)
}

#[test]
fn detects_nan_transform() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.init_resource::<bevy_playtest::driver::RunViolations>();
    // A static NaN transform, mutated every frame (so it counts as
    // "changed" and frozen_world does not fire first).
    app.add_systems(bevy_app::Update, |mut q: Query<&mut Transform>| {
        for mut t in q.iter_mut() {
            t.translation.x = f32::NAN;
        }
    });
    app.world_mut().spawn(Transform::from_xyz(0.0, 0.0, 0.0));

    let report = run_scenario(&mut app, &short_scenario()).expect("run completes");
    assert!(
        violated_with(report.status, &rule_names(&report), "finite_transforms"),
        "expected finite_transforms violation, got {:?}: {:?}",
        report.status,
        report.violations
    );
}

#[test]
fn detects_out_of_bounds() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.add_systems(bevy_app::Update, |mut q: Query<&mut Transform>| {
        for mut t in q.iter_mut() {
            t.translation.x = 5000.0; // beyond DEFAULT_BOUNDS_EXTENT (1000)
        }
    });
    app.world_mut().spawn(Transform::from_xyz(0.0, 0.0, 0.0));

    let report = run_scenario(&mut app, &short_scenario()).expect("run completes");
    assert!(
        violated_with(report.status, &rule_names(&report), "nodes_in_bounds"),
        "expected nodes_in_bounds violation, got {:?}: {:?}",
        report.status,
        report.violations
    );
}

#[test]
fn detects_frozen_world() {
    let _slot = common::run_slot();
    let mut app = App::new();
    // Static transform, nothing ever changes it: the world freezes.
    app.world_mut().spawn(Transform::from_xyz(1.0, 2.0, 3.0));

    let report = run_scenario(&mut app, &long_scenario()).expect("run completes");
    assert!(
        violated_with(report.status, &rule_names(&report), "frozen_world"),
        "expected frozen_world violation, got {:?}: {:?}",
        report.status,
        report.violations
    );
}

/// A system that panics (typed function avoids closure inference issues).
fn panic_system() {
    panic!("fault injection: deliberate system panic")
}

/// A system that emits an error-level log every frame.
fn error_log_system() {
    tracing::error!("fault injection: deliberate error log");
}

/// A system that emits a warn-level log every frame.
fn warn_log_system() {
    tracing::warn!("fault injection: deliberate warn log");
}

#[test]
fn detects_warning_log() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.add_systems(bevy_app::Update, warn_log_system);

    // log_warn is informational by default; strict_warnings escalates it
    // to a gating failure. Use a scenario with strict_warnings: true.
    let scenario: Scenario = serde_json::from_str(
        r#"{
            "name": "fault-injection-warn",
            "bot": { "type": "chaos", "seed": 42 },
            "duration_s": 1.0,
            "tps": 60,
            "invariants": [],
            "strict_warnings": true
        }"#,
    )
    .expect("scenario parses");

    let report = run_scenario(&mut app, &scenario).expect("run completes");
    assert!(
        violated_with(report.status, &rule_names(&report), "log_warn"),
        "expected log_warn violation, got {:?}: {:?}",
        report.status,
        report.violations
    );
}

#[test]
fn detects_panic() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.add_systems(bevy_app::Update, panic_system);

    let report = run_scenario(&mut app, &short_scenario()).expect("run completes");
    let names = rule_names(&report);
    assert!(
        report.status == PlaytestStatus::Crash || violated_with(report.status, &names, "panic"),
        "expected crash/panic status, got {:?}: {:?}",
        report.status,
        report.violations
    );
}

#[test]
fn detects_error_log() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.add_systems(bevy_app::Update, error_log_system);

    let report = run_scenario(&mut app, &short_scenario()).expect("run completes");
    assert!(
        violated_with(report.status, &rule_names(&report), "log_error"),
        "expected log_error violation, got {:?}: {:?}",
        report.status,
        report.violations
    );
}

/// Extract just the rule names from a report (helper).
fn rule_names(report: &bevy_playtest::state::PlaytestReport) -> Vec<String> {
    report.violations.iter().map(|v| v.rule.clone()).collect()
}

/// THE GAP-FIX PIN: an error logged from a NON-main thread (an
/// IoTaskPool worker, exactly where bevy_asset logs "Path not found"
/// failures) must reach the run's report. The pre-fix thread-local
/// sink dropped these silently.
///
/// Synchronization: the waiter system blocks (bounded) inside the run
/// loop until the worker signals AFTER its error log — the run cannot
/// reach drain() before the error is in the buffer. A fire-and-forget
/// task could execute post-run (stragglers are dropped by design),
/// which would make this test flake even with correct capture.
#[test]
fn detects_error_log_from_worker_thread() {
    let _slot = common::run_slot();
    // Initialize the pool before the run (bevy initializes task pools
    // at plugin-build in real games; this minimal app has none).
    bevy_tasks::IoTaskPool::get_or_init(|| bevy_tasks::TaskPoolBuilder::default().build());

    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let rx = std::sync::Arc::new(std::sync::Mutex::new(rx));
    let tx = std::sync::Arc::new(std::sync::Mutex::new(tx));

    let mut app = App::new();
    // Observable entity so the vacuity guard does not fire (the point
    // of this test is log capture, not vacuity).
    app.world_mut().spawn(Transform::from_xyz(0.0, 0.0, 0.0));
    app.add_systems(
        bevy_app::Update,
        (
            // Frame 0 only: spawn the erroring task.
            move |mut spawned: bevy_ecs::prelude::Local<bool>| {
                if *spawned {
                    return;
                }
                *spawned = true;
                // detach(): a dropped Task cancels its future — the
                // erroring future must actually RUN on the worker.
                let tx = tx.lock().unwrap().clone();
                bevy_tasks::IoTaskPool::get()
                    .spawn(async move {
                        tracing::error!("worker thread asset failure");
                        tx.send(()).ok();
                    })
                    .detach();
            },
            // Blocks (bounded) until the worker logged; after that a
            // no-op. Forces the error into the run window.
            move |mut waited: bevy_ecs::prelude::Local<bool>| {
                if *waited {
                    return;
                }
                if rx
                    .lock()
                    .unwrap()
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .is_ok()
                {
                    *waited = true;
                }
            },
        ),
    );

    let report = run_scenario(&mut app, &short_scenario()).expect("run completes");
    assert!(
        violated_with(report.status, &rule_names(&report), "log_error"),
        "worker-thread log_error must be captured, got {:?}: {:?}",
        report.status,
        report.violations
    );
}

/// The same pin for a multi-threaded schedule (single_threaded
/// unset): system code runs on executor worker threads.
#[test]
fn detects_error_log_multithreaded_schedule() {
    let _slot = common::run_slot();
    let scenario: Scenario = serde_json::from_str(
        r#"{
            "name": "fault-injection-mt",
            "bot": { "type": "chaos", "seed": 42 },
            "duration_s": 1.0,
            "tps": 60,
            "single_threaded": false
        }"#,
    )
    .expect("scenario parses");

    fn noisy_system() {
        tracing::error!("multithreaded schedule failure");
    }

    let mut app = App::new();
    app.add_systems(bevy_app::Update, noisy_system);

    let report = run_scenario(&mut app, &scenario).expect("run completes");
    assert!(
        violated_with(report.status, &rule_names(&report), "log_error"),
        "multi-threaded log_error must be captured, got {:?}: {:?}",
        report.status,
        report.violations
    );
}
