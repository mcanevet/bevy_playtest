//! Negative-path test battery: everything here MUST be rejected at load
//! or fail loudly at runtime. These encode the "never a vacuous pass"
//! doctrine at the schema boundary: any knob that would be silently
//! ignored must instead reject — unknown fields fail at parse time
//! (`deny_unknown_fields`), unimplemented-but-planned semantics fail at
//! validation time with NotImplemented.

use bevy_playtest::scenario::{validate_scenario, Scenario, ScenarioError};

fn minimal_scenario(bot_type: &str) -> serde_json::Value {
    serde_json::json!({
        "bot": { "type": bot_type },
        "duration_s": 1.0,
    })
}

fn parse(json: serde_json::Value) -> Scenario {
    serde_json::from_value(json).expect("parse scenario")
}

// ---------------------------------------------------------------------------
// Chaos bot input surface
// ---------------------------------------------------------------------------

/// Bogus key names in a chaos scenario's raw_surface must be rejected —
/// otherwise the bot silently presses nothing and the run passes
/// vacuously.
#[test]
fn rejects_invalid_chaos_keys() {
    let mut json = minimal_scenario("chaos");
    json["bot"]["raw_surface"] = serde_json::json!({ "keys": ["qwerty", "nope"] });
    let s = parse(json);
    match validate_scenario(&s) {
        Err(ScenarioError::Rejected(msg)) => assert!(msg.contains("key"), "{msg}"),
        other => panic!("expected Rejected for bogus chaos keys, got {other:?}"),
    }
}

/// A chaos bot with a fully-empty surface (no keys, no mouse buttons)
/// can do nothing; it must reject rather than produce a vacuous pass.
#[test]
fn rejects_empty_chaos_keys() {
    let mut json = minimal_scenario("chaos");
    json["bot"]["raw_surface"] = serde_json::json!({ "keys": [], "mouse_buttons": [] });
    let s = parse(json);
    assert!(
        matches!(validate_scenario(&s), Err(ScenarioError::Rejected(_))),
        "a chaos bot with a fully-empty surface must reject, not no-op"
    );
}

// ---------------------------------------------------------------------------
// Removed bot fields: reject at parse time (deny_unknown_fields)
// ---------------------------------------------------------------------------

/// Fields that once parsed (persona, actuator, inputs, agent targeting,
/// mouse/gamepad surface knobs) have been removed from the schema until
/// implemented. Any of them in scenario JSON must fail deserialization
/// with a serde unknown-field error naming the offender — never parse
/// successfully and be silently dropped.
#[test]
fn removed_bot_fields_reject_at_parse_time() {
    for (field, value) in [
        ("persona", serde_json::json!("aggressive")),
        ("actuator", serde_json::json!("intents")),
        ("agent_target", serde_json::json!("Player")),
        ("target", serde_json::json!("Boss")),
        (
            "inputs",
            serde_json::json!([{"frame": 0, "intent": {"intent": "move", "dir": [0.0, 1.0]}}]),
        ),
    ] {
        let mut json = minimal_scenario("chaos");
        json["bot"][field] = value;
        let err =
            serde_json::from_value::<Scenario>(json).expect_err("removed bot field must not parse");
        assert!(
            err.to_string().contains(field),
            "serde error must name the removed field '{field}': {err}"
        );
    }
}

/// Removed gamepad/wheel surface knobs: declaring them must fail at
/// parse time, not yield a silently keyboard-only bot. (`mouse_buttons`
/// is now a REAL knob — the chaos mouse bot samples it — and has its
/// own schema tests in scenario_schema.rs.)
#[test]
fn removed_surface_kinds_reject_at_parse_time() {
    for field in ["mouse_motion", "wheel", "gamepad"] {
        let mut json = minimal_scenario("chaos");
        json["bot"]["raw_surface"] = serde_json::json!({ field: true });
        let err = serde_json::from_value::<Scenario>(json)
            .expect_err("removed surface knob must not parse");
        assert!(
            err.to_string().contains(field),
            "serde error must name the removed knob '{field}': {err}"
        );
    }
}

// ---------------------------------------------------------------------------
// Oracle selection
// ---------------------------------------------------------------------------

/// `"oracles": []` parses to "run all" — the same meaning as `None`. A
/// user writing `[]` almost certainly intends "none", which is not
/// implemented. Until it is, the empty list must reject.
#[test]
fn rejects_empty_oracles_list() {
    let mut json = minimal_scenario("chaos");
    json["oracles"] = serde_json::json!([]);
    let s = parse(json);
    match validate_scenario(&s) {
        Err(ScenarioError::Rejected(_)) | Err(ScenarioError::NotImplemented(_)) => {}
        other => panic!("empty oracles list must not silently mean 'all': {other:?}"),
    }
}
