//! Behavioral test for the vacuous_invariant rule: an invariant that
//! matches zero entities must produce a violation, and the rule must
//! classify as gating (not informational).

// Shared test harness (run-slot serialization)
#[path = "common/mod.rs"]
mod common;

use bevy_app::prelude::*;
use bevy_playtest::driver::run_scenario;
use bevy_playtest::rules::Rule;
fn scenario() -> bevy_playtest::scenario::Scenario {
    serde_json::from_str(
        r#"{"bot":{"type":"chaos","seed":42},"duration_s":0.05,"tps":60,
            "invariants":[{"name":"bounds","rule":"nodes_in_bounds"}]}"#,
    )
    .unwrap()
}

#[test]
fn invariant_with_zero_entities_is_vacuous() {
    let _slot = common::run_slot();
    // No entities at all: the invariant matches nothing and must be
    // flagged as vacuous rather than silently passing.
    let mut app = App::new();
    let report = run_scenario(&mut app, &scenario()).unwrap();
    let rules: Vec<&str> = report.violations.iter().map(|v| v.rule.as_str()).collect();
    assert!(
        rules.contains(&"vacuous_invariant"),
        "expected vacuous_invariant among {rules:?}"
    );
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    // The rule must be gating, not informational: a vacuous invariant
    // that doesn't fail the run would be a vacuous pass.
    assert!(!Rule::from_name("vacuous_invariant")
        .unwrap()
        .is_informational());
}

/// Regression guard (Copilot review, PR #27): a game that panics before
/// any Transform entity exists, with a declared invariant, must remain
/// `Crash` — the vacuous-invariant reporting must not demote a crash to
/// a plain failure (exit codes and JUnit classification depend on it).
#[test]
fn startup_panic_with_declared_invariant_stays_crash_not_fail() {
    let _slot = common::run_slot();
    fn exploding_startup() {
        panic!("boom at startup!");
    }

    let mut app = App::new();
    app.add_systems(bevy_app::Startup, exploding_startup);
    let report = run_scenario(&mut app, &scenario()).unwrap();
    assert_eq!(
        report.status,
        bevy_playtest::enums::PlaytestStatus::Crash,
        "status: {:?}",
        report.status
    );
    // The vacuity violation is still recorded alongside the crash.
    let rules: Vec<&str> = report.violations.iter().map(|v| v.rule.as_str()).collect();
    assert!(rules.contains(&"panic"), "expected panic among {rules:?}");
    assert!(
        rules.contains(&"vacuous_invariant"),
        "expected vacuous_invariant among {rules:?}"
    );
}
