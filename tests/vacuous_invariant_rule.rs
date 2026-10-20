//! Regression test: vacuous_invariant must be a registered Rule.
//!
//! If this test fails, the invariant evaluator is emitting a rule name
//! that isn't in the registry, breaking classification and meaning.

use bevy_playtest::rules::Rule;

#[test]
fn vacuous_invariant_is_registered() {
    // The invariant evaluator emits "vacuous_invariant" when a
    // Gameplay-targeted invariant matches zero entities.
    assert!(
        Rule::from_name("vacuous_invariant").is_some(),
        "vacuous_invariant must be in the Rule registry"
    );
}

#[test]
fn vacuous_invariant_has_meaning() {
    let rule = Rule::from_name("vacuous_invariant").expect("must exist");
    let meaning = rule.meaning();
    assert!(
        !meaning.is_empty(),
        "vacuous_invariant must have a non-empty meaning"
    );
}
