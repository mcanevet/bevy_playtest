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
            "invariants": [],
            "setup": {}
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
            "liveness": { "mode": "real_time", "timeout_s": 2.0 },
            "setup": {}
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
            "strict_warnings": true,
            "setup": {}
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
fn detects_reset_hook_panic() {
    use bevy_playtest::contract::ResetHooks;
    let mut app = App::new();
    let mut hooks = ResetHooks::default();
    hooks.register("boom", |_world: &mut bevy_ecs::world::World| {
        panic!("fault injection: reset hook panic")
    });
    app.insert_resource(hooks);

    let scenario: Scenario = serde_json::from_str(
        r#"{
            "name": "fault-injection-reset",
            "bot": { "type": "chaos", "seed": 42 },
            "duration_s": 0.5,
            "tps": 60,
            "invariants": [],
            "setup": { "resets": [ { "kind": "boom" } ] }
        }"#,
    )
    .expect("scenario parses");

    let report = run_scenario(&mut app, &scenario).expect("run completes");
    assert!(
        violated_with(report.status, &rule_names(&report), "reset_hook_panicked"),
        "expected reset_hook_panicked violation, got {:?}: {:?}",
        report.status,
        report.violations
    );
}

#[test]
fn detects_cheat_hook_panic() {
    use bevy_playtest::contract::CheatHooks;
    let mut app = App::new();
    let mut hooks = CheatHooks::default();
    hooks.register("boom", |_world: &mut bevy_ecs::world::World| {
        panic!("fault injection: cheat hook panic")
    });
    app.insert_resource(hooks);

    let scenario: Scenario = serde_json::from_str(
        r#"{
            "name": "fault-injection-cheat",
            "bot": { "type": "chaos", "seed": 42 },
            "duration_s": 0.5,
            "tps": 60,
            "invariants": [],
            "setup": { "cheats": [ { "frame": 0, "kind": "boom" } ] }
        }"#,
    )
    .expect("scenario parses");

    let report = run_scenario(&mut app, &scenario).expect("run completes");
    assert!(
        violated_with(report.status, &rule_names(&report), "cheat_hook_panicked"),
        "expected cheat_hook_panicked violation, got {:?}: {:?}",
        report.status,
        report.violations
    );
}

#[test]
fn detects_panic() {
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
