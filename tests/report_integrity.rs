//! Report-integrity test battery: prove the REPORT tells the truth.
//! Where the schema tests check what's rejected at load, these check
//! that what survives into a run is reflected faithfully in the
//! PlaytestReport. All encode the "never a vacuous pass" doctrine.
//!
//! NOTE (temporary): expected-to-fail tests are #[ignore]d with the
//! reason; run with `cargo test -- --ignored` to see current truth.

use bevy_app::App;
use bevy_playtest::run_scenario;
use bevy_playtest::scenario::Scenario;
use bevy_playtest::sinks;
use bevy_transform::prelude::Transform;

fn minimal_app() -> App {
    sinks::ensure_global_subscriber();
    App::new()
}

fn scenario_json(bot: &str) -> String {
    format!(r#"{{"bot":{{"type":"{bot}","seed":7}},"duration_s":0.5,"tps":60}}"#)
}

// ---------------------------------------------------------------------------
// Vacuity guards
// ---------------------------------------------------------------------------

/// An app where NOTHING ever happens (no entities, no transforms) must
/// not pass silently. The BRP runner has a zero-row vacuity guard; the
/// in-process driver must have one too (undocumented contract).
#[test]
#[ignore = "in-process driver has no vacuity guard: zero-entity app passes"]
fn zero_entity_game_must_not_pass_vacuously() {
    let scenario: Scenario = serde_json::from_str(&scenario_json("chaos")).unwrap();
    let mut app = minimal_app();
    let report = run_scenario(&mut app, &scenario).unwrap();
    assert_ne!(
        report.status,
        bevy_playtest::enums::PlaytestStatus::Pass,
        "a run with zero entities and zero transforms must not pass; got violations={:?}",
        report.violations
    );
}

// ---------------------------------------------------------------------------
// Frozen-world reporting integrity
// ---------------------------------------------------------------------------

/// A completely static world must report the frozen-world condition,
/// and the frozen_world rule must appear AT MOST ONCE in the violation
/// list — two oracles currently emit it with different semantics and
/// different grace windows , producing duplicate/conflicting
/// entries.
#[test]
#[ignore = "two frozen_world oracles emit conflicting duplicates"]
fn frozen_world_reports_exactly_one_violation() {
    let scenario: Scenario = serde_json::from_str(&scenario_json("chaos")).unwrap();
    let mut app = minimal_app();
    // Spawn a static entity that never moves: world is frozen after grace.
    app.world_mut().spawn(Transform::from_xyz(0.0, 0.0, 0.0));
    app.add_systems(bevy_app::Update, || {}); // no-op system, ticks happen
    let report = run_scenario(&mut app, &scenario).unwrap();

    let frozen: Vec<_> = report
        .violations
        .iter()
        .filter(|v| v.rule.contains("frozen"))
        .collect();
    assert!(
        !frozen.is_empty(),
        "a static world must trigger the frozen-world oracle"
    );
    assert_eq!(
        frozen.len(),
        1,
        "frozen_world must be reported exactly once; got {frozen:?}"
    );
}

/// Changing liveness.timeout_s must affect when a frozen world is
/// caught. The liveness oracle derives its grace window from the
/// scenario's liveness config; this test is the regression guard for
/// that wiring (the parallel frozen_world oracle has a fixed 180-frame
/// grace, tracked by the duplicate-oracle test above).
#[test]
fn liveness_timeout_config_must_be_honored() {
    let short: Scenario = serde_json::from_str(
        r#"{"bot":{"type":"chaos","seed":7},"duration_s":0.5,"tps":60,"liveness":{"mode":"real_time","timeout_s":0.05}}"#,
    )
    .unwrap();
    let long: Scenario = serde_json::from_str(
        r#"{"bot":{"type":"chaos","seed":7},"duration_s":0.5,"tps":60,"liveness":{"mode":"real_time","timeout_s":10.0}}"#,
    )
    .unwrap();

    let run = |s: &Scenario| {
        let mut app = minimal_app();
        app.world_mut().spawn(Transform::from_xyz(0.0, 0.0, 0.0));
        app.add_systems(bevy_app::Update, || {});
        run_scenario(&mut app, s).unwrap()
    };

    let short_report = run(&short);
    let long_report = run(&long);
    let short_frozen = short_report
        .violations
        .iter()
        .filter(|v| v.rule.contains("frozen"))
        .count();
    let long_frozen = long_report
        .violations
        .iter()
        .filter(|v| v.rule.contains("frozen"))
        .count();
    assert!(
        short_frozen > long_frozen,
        "a shorter liveness timeout must catch a frozen world sooner (or at all): \
         short={short_frozen} long={long_frozen}"
    );
}

// ---------------------------------------------------------------------------
// Telemetry truth
// ---------------------------------------------------------------------------

/// A run where the bot definitely emitted input must report
/// input_count > 0 and a populated action log : cheat_count and
/// action_log are currently hardcoded empty/0).
#[test]
#[ignore = "metrics.input_count/action_log are never populated (C5/H9)"]
fn bot_activity_must_be_visible_in_telemetry() {
    let scenario: Scenario = serde_json::from_str(&scenario_json("chaos")).unwrap();
    let mut app = minimal_app();
    // Give the bot something to affect so effects are observable.
    app.add_message::<bevy_input::keyboard::KeyboardInput>();
    let report = run_scenario(&mut app, &scenario).unwrap();
    assert!(
        report.metrics.input_count > 0 || !report.action_log.is_empty(),
        "a chaos bot run must show activity in metrics.input_count ({}) or action_log ({} entries)",
        report.metrics.input_count,
        report.action_log.len()
    );
}

/// A scheduled cheat must bump cheat_count (currently always 0 — C5).
#[test]
#[ignore = "cheat_count is hardcoded 0"]
fn fired_cheat_must_bump_cheat_count() {
    use bevy_playtest::contract::CheatHooks;

    let mut hooks = CheatHooks::default();
    hooks.register("probe", |_world| {});

    let scenario: Scenario = serde_json::from_str(
        r#"{"bot":{"type":"chaos","seed":7},"duration_s":0.3,"tps":60,"setup":{"cheats":[{"frame":0,"kind":"probe"}]}}"#,
    )
    .unwrap();
    let mut app = minimal_app();
    app.insert_resource(hooks);
    let report = run_scenario(&mut app, &scenario).unwrap();
    assert_eq!(
        report.cheat_count, 1,
        "a fired cheat must be auditable in cheat_count"
    );
}

// ---------------------------------------------------------------------------
// Crash-flag consistency
// ---------------------------------------------------------------------------

/// If the report status is Crash, metrics.crash_detected must be true
/// (never set today).
#[test]
#[ignore = "metrics.crash_detected is never set"]
fn crash_status_implies_crash_detected_metric() {
    // A panicking game system aborts the run loop; the harness must
    // classify it as Crash AND set the metric consistently.
    let scenario: Scenario = serde_json::from_str(&scenario_json("chaos")).unwrap();
    let mut app = minimal_app();
    app.add_systems(bevy_app::Update, || panic!("boom"));
    let report = run_scenario(&mut app, &scenario).unwrap();
    assert_eq!(
        report.status,
        bevy_playtest::enums::PlaytestStatus::Crash,
        "a panicking system must be classified as Crash"
    );
    assert!(
        report.metrics.crash_detected,
        "status==Crash must be reflected in metrics.crash_detected"
    );
}
