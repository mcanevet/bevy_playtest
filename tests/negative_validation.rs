//! Negative-path test battery: everything here MUST be rejected at load
//! or fail loudly at runtime. These encode the "never a vacuous pass"
//! doctrine at the schema boundary: any knob that would be silently
//! ignored must instead reject with NotImplemented/Rejected.
//!
//! NOTE (temporary): some of these tests are EXPECTED TO FAIL against
//! current code — they encode the contract the adversarial review
//! identified as broken. Once the corresponding fixes land, they turn
//! green. They are #[ignore]d so the suite stays green meanwhile; run
//! with `cargo test -- --ignored` to see the current truth.

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
/// vacuously (adversarial review C1).
#[test]
#[ignore = "current code accepts bogus chaos keys (C1): bot no-ops"]
fn rejects_invalid_chaos_keys() {
    let mut json = minimal_scenario("chaos");
    json["raw_surface"] = serde_json::json!({ "keys": ["qwerty", "nope"] });
    let s = parse(json);
    match validate_scenario(&s) {
        Err(ScenarioError::Rejected(msg)) => assert!(msg.contains("key"), "{msg}"),
        other => panic!("expected Rejected for bogus chaos keys, got {other:?}"),
    }
}

/// A chaos bot with an empty key surface can do nothing; it must reject
/// rather than produce a vacuous pass (C1 adjacent).
#[test]
#[ignore = "current code accepts an empty chaos key surface (C1)"]
fn rejects_empty_chaos_keys() {
    let mut json = minimal_scenario("chaos");
    json["raw_surface"] = serde_json::json!({ "keys": [] });
    let s = parse(json);
    assert!(
        matches!(validate_scenario(&s), Err(ScenarioError::Rejected(_))),
        "a chaos bot with zero keys must reject, not no-op"
    );
}

/// Mouse/gamepad/wheel surface declarations are parsed but never read
/// by the chaos bot; they must reject as NotImplemented instead of
/// silently yielding a keyboard-only bot (C2).
#[test]
#[ignore = "current code accepts unread surface knobs (C2)"]
fn rejects_unimplemented_surface_kinds() {
    for field in ["mouse_buttons", "mouse_motion", "wheel", "gamepad"] {
        let mut json = minimal_scenario("chaos");
        json["raw_surface"] = serde_json::json!({ field: true });
        let s = parse(json);
        match validate_scenario(&s) {
            Err(ScenarioError::NotImplemented(msg)) => {
                assert!(msg.contains(field), "{msg}")
            }
            other => panic!("expected NotImplemented for raw_surface.{field}, got {other:?}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Accepted-but-unconsumed bot fields
// ---------------------------------------------------------------------------

/// `persona` parses but nothing consumes it (C2).
#[test]
#[ignore = "persona parses but is never consumed (C2)"]
fn rejects_unimplemented_persona() {
    let mut json = minimal_scenario("chaos");
    json["bot"]["persona"] = serde_json::json!("aggressive");
    let s = parse(json);
    match validate_scenario(&s) {
        Err(ScenarioError::NotImplemented(msg)) => assert!(msg.contains("persona"), "{msg}"),
        other => panic!("expected NotImplemented for persona, got {other:?}"),
    }
}

/// `actuator` validates then is completely ignored (C2).
#[test]
#[ignore = "actuator validates but is never consumed (C2)"]
fn rejects_unimplemented_actuator() {
    let mut json = minimal_scenario("chaos");
    json["bot"]["actuator"] = serde_json::json!("intents");
    let s = parse(json);
    match validate_scenario(&s) {
        Err(ScenarioError::NotImplemented(msg)) => assert!(msg.contains("actuator"), "{msg}"),
        other => panic!("expected NotImplemented for actuator, got {other:?}"),
    }
}

/// Populated `inputs` on a chaos bot is silently ignored today (C2).
#[test]
#[ignore = "chaos bot silently ignores populated inputs (C2)"]
fn rejects_inputs_on_chaos_bot() {
    let mut json = minimal_scenario("chaos");
    json["bot"]["inputs"] =
        serde_json::json!([{ "frame": 0, "intent": { "raw": "press", "keys": ["KeyW"] } }]);
    let s = parse(json);
    match validate_scenario(&s) {
        Err(ScenarioError::Rejected(_)) | Err(ScenarioError::NotImplemented(_)) => {}
        other => panic!("expected rejection for chaos bot inputs, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Cheat scheduling
// ---------------------------------------------------------------------------

/// Cheats currently all fire pre-run; a nonzero `frame` claims scheduled
/// execution that doesn't exist. Until scheduling is implemented, a
/// nonzero frame must reject (review finding: how-to.md previously
/// documented scheduled cheats that never were).
#[test]
#[ignore = "cheats[].frame != 0 is accepted but unscheduled (C5-related)"]
fn rejects_nonzero_cheat_frame() {
    let mut json = minimal_scenario("chaos");
    json["setup"] = serde_json::json!({
        "cheats": [{ "frame": 300, "kind": "some_cheat" }]
    });
    let s = parse(json);
    match validate_scenario(&s) {
        Err(ScenarioError::NotImplemented(msg)) => assert!(msg.contains("frame"), "{msg}"),
        other => panic!("expected NotImplemented for nonzero cheat frame, got {other:?}"),
    }
}
