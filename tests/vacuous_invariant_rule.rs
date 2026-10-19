//! Behavioral test for the vacuous_invariant rule: a Gameplay-targeted
//! invariant that matches zero entities must produce a violation, and
//! the rule must classify as gating (not informational).

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
fn gameplay_targeted_invariant_with_zero_marked_entities_is_vacuous() {
    // No Gameplay-marked entities: the default `gameplay` target
    // selector matches nothing and the invariant must be flagged as
    // vacuous rather than silently passing.
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
