use bevy_playtest::enums::{BotType, CheckOp};
use bevy_playtest::scenario::{normalize_key_name, validate_scenario, Scenario, ScenarioError};

fn minimal_scenario(bot_type: &str) -> serde_json::Value {
    serde_json::json!({
        "bot": { "type": bot_type },
        "duration_s": 1.0,
    })
}

fn parse(json: serde_json::Value) -> Scenario {
    serde_json::from_value(json).expect("parse scenario")
}

#[test]
fn parses_minimal_chaos_scenario() {
    let s = parse(minimal_scenario("chaos"));
    assert_eq!(s.bot.bot_type, BotType::Chaos);
    assert_eq!(s.tps, 60);
    assert_eq!(s.duration_s, 1.0);
    assert!(s.simulated_time);
    assert!(s.single_threaded);
    validate_scenario(&s).expect("valid");
}

#[test]
fn rejects_zero_and_negative_duration() {
    for dur in [0.0, -1.0] {
        let mut json = minimal_scenario("chaos");
        json["duration_s"] = serde_json::json!(dur);
        let s = parse(json);
        match validate_scenario(&s) {
            Err(ScenarioError::Rejected(_)) => {}
            other => panic!("expected Rejected for duration {dur}, got {other:?}"),
        }
    }
    // NaN/inf are not representable in JSON; test the runtime float path.
    let mut s = parse(minimal_scenario("chaos"));
    s.duration_s = f32::NAN;
    assert!(matches!(
        validate_scenario(&s),
        Err(ScenarioError::Rejected(_))
    ));
    s.duration_s = f32::INFINITY;
    assert!(matches!(
        validate_scenario(&s),
        Err(ScenarioError::Rejected(_))
    ));
}

#[test]
fn rejects_dos_oversize_scenarios() {
    let mut json = minimal_scenario("chaos");
    json["tps"] = serde_json::json!(100_000);
    let s = parse(json);
    assert!(matches!(
        validate_scenario(&s),
        Err(ScenarioError::Rejected(_))
    ));

    let mut json = minimal_scenario("chaos");
    json["duration_s"] = serde_json::json!(10_000.0);
    let s = parse(json);
    assert!(matches!(
        validate_scenario(&s),
        Err(ScenarioError::Rejected(_))
    ));
}

#[test]
fn rejects_low_tps_distorting_dt() {
    let mut json = minimal_scenario("chaos");
    json["tps"] = serde_json::json!(2);
    let s = parse(json);
    let err = validate_scenario(&s).unwrap_err().to_string();
    assert!(err.contains("max_delta"), "{err}");
}

#[test]
fn rejects_path_traversal_in_name() {
    let mut json = minimal_scenario("chaos");
    json["name"] = serde_json::json!("../../etc/passwd");
    let s = parse(json);
    let err = validate_scenario(&s).unwrap_err().to_string();
    assert!(err.contains("invalid"), "{err}");
}

#[test]
fn rejects_removed_bot_types_loudly() {
    // Unimplemented bot types are removed from the enum: they fail
    // deserialization with an unknown-variant error naming the type.
    for bot in [
        "pursuit",
        "planner",
        "custom",
        "synthetic_pointer",
        "replay",
    ] {
        let err = serde_json::from_value::<Scenario>(minimal_scenario(bot))
            .expect_err("removed bot type must not parse");
        assert!(
            err.to_string().contains(bot),
            "serde error must name the bot type '{bot}': {err}"
        );
    }
}

#[test]
fn rejects_liveness_off_loudly() {
    // The mode knob is not implemented; "off" must reject instead of
    // silently behaving like real_time (never a vacuous pass).
    let mut json = minimal_scenario("chaos");
    json["liveness"] = serde_json::json!({ "mode": "off", "timeout_s": 2.0 });
    let s = parse(json);
    match validate_scenario(&s) {
        Err(ScenarioError::NotImplemented(msg)) => {
            assert!(msg.contains("liveness.mode"), "{msg}")
        }
        other => panic!("expected NotImplemented for liveness off, got {other:?}"),
    }
}

#[test]
fn rejects_oracles_selection_loudly() {
    // Oracle selection is not implemented; a populated list would
    // silently change nothing.
    let mut json = minimal_scenario("chaos");
    json["oracles"] = serde_json::json!(["finite_transforms"]);
    let s = parse(json);
    match validate_scenario(&s) {
        Err(ScenarioError::NotImplemented(msg)) => {
            assert!(msg.contains("oracles"), "{msg}")
        }
        other => panic!("expected NotImplemented for oracles, got {other:?}"),
    }
}

#[test]
fn empty_oracles_list_rejects_like_any_selection() {
    // Oracle selection is not implemented at all: `[]` is as much a
    // selection as a populated list (and reads as "none" to a human
    // author). Any non-None oracles value must reject.
    let mut json = minimal_scenario("chaos");
    json["oracles"] = serde_json::json!([]);
    let s = parse(json);
    match validate_scenario(&s) {
        Err(ScenarioError::NotImplemented(msg)) => assert!(msg.contains("oracles"), "{msg}"),
        other => panic!("expected NotImplemented for oracles: [], got {other:?}"),
    }
}

#[test]
fn accepts_real_time_liveness_mode() {
    let mut json = minimal_scenario("chaos");
    json["liveness"] = serde_json::json!({ "mode": "real_time", "timeout_s": 3.0 });
    let s = parse(json);
    validate_scenario(&s).expect("real_time mode is valid");
}

#[test]
fn rejects_unknown_fields() {
    let mut json = minimal_scenario("chaos");
    json["compare_golden"] = serde_json::json!("goldens/");
    let err = serde_json::from_value::<Scenario>(json).unwrap_err();
    assert!(err.to_string().contains("compare_golden"));
}

#[test]
fn rejects_duplicate_invariant_names() {
    let mut json = minimal_scenario("chaos");
    json["invariants"] = serde_json::json!([
        { "name": "hp", "rule": "nodes_in_bounds" },
        { "name": "hp", "rule": "nodes_in_bounds" }
    ]);
    let s = parse(json);
    let err = validate_scenario(&s).unwrap_err().to_string();
    assert!(err.contains("duplicate invariant name"), "{err}");
}

#[test]
fn rejects_custom_invariants_at_parse_time() {
    // "custom" is removed from InvariantRule: it fails deserialization
    // with an unknown-variant error.
    let mut json = minimal_scenario("chaos");
    json["invariants"] = serde_json::json!([
        { "name": "phase", "rule": "custom" }
    ]);
    let err =
        serde_json::from_value::<Scenario>(json).expect_err("custom invariant must not parse");
    assert!(
        err.to_string().contains("custom"),
        "serde error must name the variant 'custom': {err}"
    );
}

#[test]
fn key_normalization_common_forms() {
    assert_eq!(normalize_key_name("space"), Some("Space"));
    assert_eq!(normalize_key_name("SPACE"), Some("Space"));
    assert_eq!(normalize_key_name("w"), Some("KeyW"));
    assert_eq!(normalize_key_name("KeyW"), Some("KeyW"));
    assert_eq!(normalize_key_name("keyw"), Some("KeyW"));
    assert_eq!(normalize_key_name("5"), Some("Digit5"));
    assert_eq!(normalize_key_name("Digit5"), Some("Digit5"));
    assert_eq!(normalize_key_name("up"), Some("ArrowUp"));
    assert_eq!(normalize_key_name("arrowdown"), Some("ArrowDown"));
    assert_eq!(normalize_key_name("shift"), Some("ShiftLeft"));
    assert_eq!(normalize_key_name("ctrl"), Some("ControlLeft"));
    assert_eq!(normalize_key_name("qwerty"), None);
    assert_eq!(normalize_key_name("f13"), None);
    assert_eq!(normalize_key_name(""), None);
}

#[test]
fn synthetic_keyboard_requires_valid_keys() {
    let mut json = minimal_scenario("synthetic_keyboard");
    json["bot"]["key_presses"] = serde_json::json!([
        { "frame": 0, "key": "space", "hold_frames": 2 },
        { "frame": 10, "key": "spce" }
    ]);
    let s = parse(json);
    let err = validate_scenario(&s).unwrap_err().to_string();
    assert!(err.contains("spce"), "{err}");
    assert!(err.contains("did you mean"), "{err}");
}

#[test]
fn synthetic_keyboard_requires_nonempty() {
    let s = parse(minimal_scenario("synthetic_keyboard"));
    let err = validate_scenario(&s).unwrap_err().to_string();
    assert!(err.contains("non-empty"), "{err}");
}

#[test]
fn synthetic_keyboard_accepts_mouse_only() {
    // `mouse_clicks` is documented as orthogonal to the bot type: a
    // scripted scenario with clicks but no key presses is valid.
    let mut json = minimal_scenario("synthetic_keyboard");
    json["bot"]["mouse_clicks"] =
        serde_json::json!([{ "frame": 0, "button": "left", "x": 10.0, "y": 10.0 }]);
    let s = parse(json);
    validate_scenario(&s).unwrap();
}

#[test]
fn mouse_button_normalization_common_forms() {
    use bevy_playtest::scenario::normalize_mouse_button_name;
    assert_eq!(normalize_mouse_button_name("left"), Some("Left"));
    assert_eq!(normalize_mouse_button_name("LEFT"), Some("Left"));
    assert_eq!(normalize_mouse_button_name("Right"), Some("Right"));
    assert_eq!(normalize_mouse_button_name("middle"), Some("Middle"));
    assert_eq!(normalize_mouse_button_name("back"), Some("Back"));
    assert_eq!(normalize_mouse_button_name("Forward"), Some("Forward"));
    assert_eq!(normalize_mouse_button_name("side"), None);
    assert_eq!(normalize_mouse_button_name(""), None);
}

#[test]
fn mouse_clicks_require_valid_buttons() {
    let mut json = minimal_scenario("chaos");
    json["bot"]["mouse_clicks"] = serde_json::json!([
        { "frame": 0, "button": "left", "x": 100.0, "y": 50.0 },
        { "frame": 10, "button": "lift" }
    ]);
    let s = parse(json);
    let err = validate_scenario(&s).unwrap_err().to_string();
    assert!(err.contains("lift"), "{err}");
    assert!(err.contains("did you mean"), "{err}");
}

#[test]
fn mouse_clicks_reject_zero_hold() {
    let mut json = minimal_scenario("chaos");
    json["bot"]["mouse_clicks"] =
        serde_json::json!([{ "frame": 0, "button": "left", "hold_frames": 0 }]);
    let s = parse(json);
    let err = validate_scenario(&s).unwrap_err().to_string();
    assert!(err.contains("hold_frames"), "{err}");
}

#[test]
fn mouse_clicks_default_hold_and_position() {
    // Minimal click: frame + button only; position defaults to (0,0),
    // hold to 2 frames (same-frame press+release would cancel).
    let mut json = minimal_scenario("chaos");
    json["bot"]["mouse_clicks"] = serde_json::json!([{ "frame": 3, "button": "right" }]);
    let s: Scenario = serde_json::from_value(json).unwrap();
    assert_eq!(s.bot.mouse_clicks.len(), 1);
    assert_eq!(s.bot.mouse_clicks[0].frame, 3);
    assert_eq!(s.bot.mouse_clicks[0].button, "right");
    assert_eq!(s.bot.mouse_clicks[0].x, 0.0);
    assert_eq!(s.bot.mouse_clicks[0].y, 0.0);
    assert_eq!(s.bot.mouse_clicks[0].hold_frames, 1);
    validate_scenario(&s).unwrap();
}

#[test]
fn chaos_surface_mouse_buttons_must_be_known() {
    let mut json = minimal_scenario("chaos");
    json["bot"]["raw_surface"] =
        serde_json::json!({ "keys": ["w", "space"], "mouse_buttons": ["left", "midle"] });
    let s = parse(json);
    let err = validate_scenario(&s).unwrap_err().to_string();
    assert!(err.contains("midle"), "{err}");
    assert!(err.contains("did you mean"), "{err}");
}

#[test]
fn raw_surface_defaults_include_mouse_buttons() {
    // An explicit-but-empty raw_surface object takes the serde field
    // defaults: default key set + all three primary buttons (both halves
    // of the chaos bot default on).
    let mut json = minimal_scenario("chaos");
    json["bot"]["raw_surface"] = serde_json::json!({});
    let s: Scenario = serde_json::from_value(json).unwrap();
    let surface = s.bot.raw_surface.clone().expect("raw_surface present");
    assert_eq!(
        surface.keys,
        vec![
            "KeyW".to_string(),
            "KeyA".to_string(),
            "KeyS".to_string(),
            "KeyD".to_string(),
            "ArrowUp".to_string(),
            "ArrowDown".to_string(),
            "ArrowLeft".to_string(),
            "ArrowRight".to_string()
        ]
    );
    assert_eq!(
        surface.mouse_buttons,
        vec![
            "Left".to_string(),
            "Right".to_string(),
            "Middle".to_string()
        ]
    );
    validate_scenario(&s).unwrap();
}

/// Mouse-only chaos: an explicitly empty `keys` list opts out of chaos
/// keys while keeping chaos clicks — the inverse of the documented
/// `mouse_buttons: []` click opt-out.
#[test]
fn mouse_only_chaos_surface_is_valid() {
    let mut json = minimal_scenario("chaos");
    json["bot"]["raw_surface"] = serde_json::json!({ "keys": [] });
    let s: Scenario = serde_json::from_value(json).unwrap();
    let surface = s.bot.raw_surface.clone().expect("raw_surface present");
    assert!(surface.keys.is_empty(), "keys explicitly emptied");
    assert_eq!(surface.mouse_buttons.len(), 3, "mouse defaults retained");
    validate_scenario(&s).unwrap();
}

#[test]
fn check_op_boundary_semantics() {
    // "overall<0 iff fail" must hold at exact boundaries.
    assert!(CheckOp::Le.holds(10.0, 10.0));
    assert!(!CheckOp::Lt.holds(10.0, 10.0));
    assert!(CheckOp::Ge.holds(10.0, 10.0));
    assert!(!CheckOp::Gt.holds(10.0, 10.0));
    assert!(CheckOp::Equals.holds(10.0, 10.0));
    // Approx equality: large floats.
    assert!(CheckOp::Equals.holds(1e15, 1e15 + 1.0));
}

#[test]
fn roundtrips_through_serde() {
    let json = serde_json::json!({
        "name": "basic-suite",
        "bot": { "type": "chaos", "seed": 7, "input_rate_hz": 20 },
        "tps": 30,
        "duration_s": 5.0,
        "invariants": [
            { "name": "bounds", "rule": "nodes_in_bounds", "min_x": -100.0, "max_x": 100.0 }
        ]
    });
    let s: Scenario = serde_json::from_value(json).unwrap();
    let out = serde_json::to_value(&s).unwrap();
    let s2: Scenario = serde_json::from_value(out).unwrap();
    assert_eq!(s.bot.seed, s2.bot.seed);
    assert_eq!(s.invariants.len(), s2.invariants.len());
}

#[test]
fn target_selector_all_parses() {
    let mut json = minimal_scenario("chaos");
    json["invariants"] = serde_json::json!([
        { "name": "bounds", "rule": "nodes_in_bounds", "target_selector": "all",
          "min_x": -1.0, "max_x": 1.0 }
    ]);
    let s = parse(json);
    assert_eq!(
        s.invariants[0].target_selector,
        Some(bevy_playtest::enums::TargetSelector::All)
    );
}

#[test]
fn target_selector_unknown_rejects() {
    let mut json = minimal_scenario("chaos");
    json["invariants"] = serde_json::json!([
        { "name": "b", "rule": "nodes_in_bounds", "target_selector": "al" }
    ]);
    let err = serde_json::from_value::<Scenario>(json).unwrap_err();
    assert!(err.to_string().contains("al"), "{err}");
}

#[test]
fn target_selector_gameplay_now_rejects() {
    // The `gameplay` selector was removed with the Gameplay marker:
    // scenarios using it must fail loudly rather than silently degrade.
    let mut json = minimal_scenario("chaos");
    json["invariants"] = serde_json::json!([
        { "name": "b", "rule": "nodes_in_bounds", "target_selector": "gameplay" }
    ]);
    let err = serde_json::from_value::<Scenario>(json).unwrap_err();
    assert!(err.to_string().contains("gameplay"), "{err}");
}

#[test]
fn legacy_targets_field_now_rejects() {
    let mut json = minimal_scenario("chaos");
    json["invariants"] = serde_json::json!([
        { "name": "b", "rule": "nodes_in_bounds", "targets": ["a"] }
    ]);
    let err = serde_json::from_value::<Scenario>(json).unwrap_err();
    assert!(err.to_string().contains("unknown field"), "{err}");
}

#[test]
fn target_selector_named_rejects_at_parse() {
    // named targets are roadmap: not a variant, so any form of it fails
    // deserialization loudly rather than silently degrading.
    let mut json = minimal_scenario("chaos");
    json["invariants"] = serde_json::json!([
        { "name": "b", "rule": "nodes_in_bounds", "target_selector": "named" }
    ]);
    let err = serde_json::from_value::<Scenario>(json).unwrap_err();
    assert!(err.to_string().contains("named"), "{err}");
}

#[test]
fn verify_replay_defaults_false_and_parses() {
    let s: Scenario =
        serde_json::from_str(r#"{"bot":{"type":"chaos","seed":1},"duration_s":1.0,"tps":60}"#)
            .unwrap();
    assert!(!s.verify_replay);

    let s: Scenario = serde_json::from_str(
        r#"{"bot":{"type":"chaos","seed":1},"duration_s":1.0,"tps":60,
            "verify_replay":true}"#,
    )
    .unwrap();
    assert!(s.verify_replay);
}
