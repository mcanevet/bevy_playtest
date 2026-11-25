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
fn rejects_invariant_names_that_break_regression_lines() {
    // The name is the finding target, half of a regression-file line:
    // '#' starts a comment and a line break splits the line.
    for name in ["hp<=100 #boss", "two\nlines", "cr\r"] {
        let mut json = minimal_scenario("chaos");
        json["invariants"] = serde_json::json!([
            { "name": name, "rule": "nodes_in_bounds" }
        ]);
        let s = parse(json);
        let err = validate_scenario(&s).unwrap_err().to_string();
        assert!(err.contains("may not contain '#'"), "{name:?}: {err}");
    }
}

#[test]
fn rejects_entity_shaped_nodes_in_bounds_invariant_names() {
    // The invariant NAME is the finding's TARGET, and the sweep's
    // fingerprint layer canonicalizes entity-debug-shaped
    // nodes_in_bounds targets (and the literal "entity") to the
    // global oracle's canonical key. A name like "3v1" or "entity"
    // would collide: one regression-file line would silence the
    // authored invariant AND the global oracle's finding. Reject at
    // validation so the collision is unreachable.
    for name in ["3v1", "entity", "17v3", "0v0"] {
        let mut json = minimal_scenario("chaos");
        json["invariants"] = serde_json::json!([
            { "name": name, "rule": "nodes_in_bounds" }
        ]);
        let s = parse(json);
        let err = validate_scenario(&s).unwrap_err().to_string();
        assert!(
            err.contains("collide with the global oracle"),
            "name {name:?} must be rejected: {err}"
        );
    }
    // Sanity: the same names stay legal for component_check (its
    // rule string differs, so no canonicalization applies) and
    // ordinary bounds names stay legal.
    let mut json = minimal_scenario("chaos");
    json["invariants"] = serde_json::json!([
        { "name": "3v1", "rule": "component_check", "target_selector": {"with_component": {"name": "Hp"}},
          "path": "current", "check": "le", "value": 100 },
        { "name": "arena_bounds", "rule": "nodes_in_bounds" }
    ]);
    let s = parse(json);
    validate_scenario(&s).expect("non-colliding names are legal");
}

#[test]
fn pairwise_separation_driver_needs_both_halves() {
    // Unlike stagnant_resource there is no 'resource' to default the
    // driver host to: declaring a driver means naming BOTH halves.
    for (field, other) in [
        ("driver_resource", "driver_path"),
        ("driver_path", "driver_resource"),
    ] {
        let mut json = pairwise_json(
            Some("Enemy"),
            serde_json::json!(0.5),
            serde_json::json!(1.0),
        );
        json["invariants"][0][field] = serde_json::json!("MatchClock");
        let errs = lint_errors(json);
        assert!(
            errs.iter()
                .any(|e| e.contains("both 'driver_resource' and 'driver_path'")),
            "{field} without {other}: {errs:?}"
        );
    }
    // Both halves together: the declaration is complete — lint is
    // clean (the rule is fully evaluated at run time now).
    let mut json = pairwise_json(
        Some("Enemy"),
        serde_json::json!(0.5),
        serde_json::json!(1.0),
    );
    json["invariants"][0]["driver_resource"] = serde_json::json!("MatchClock");
    json["invariants"][0]["driver_path"] = serde_json::json!("minutes");
    let errs = lint_errors(json);
    assert!(errs.is_empty(), "both halves + shape-valid: {errs:?}");
}

#[test]
fn pairwise_separation_rejects_a_rate_without_a_driver() {
    // The frame-clock default is seconds BY DEFINITION — accepting
    // a declared rate would mis-validate the soak fit in the
    // author's fake units and cross-contaminate the one-leaf-one-
    // rate join with an empty ("","") leaf key.
    let mut json = pairwise_json(
        Some("Enemy"),
        serde_json::json!(0.5),
        serde_json::json!(1.0),
    );
    json["invariants"][0]["driver_units_per_second"] = serde_json::json!(10.0);
    let errs = lint_errors(json);
    assert!(
        errs.iter()
            .any(|e| e.contains("driver_units_per_second needs the driver declared")),
        "{errs:?}"
    );

    // The same rate WITH a declared driver is coherent (the fit
    // check judges it below).
    let mut json = pairwise_json(
        Some("Enemy"),
        serde_json::json!(0.5),
        serde_json::json!(50.0),
    );
    json["invariants"][0]["driver_resource"] = serde_json::json!("MatchClock");
    json["invariants"][0]["driver_path"] = serde_json::json!("minutes");
    json["invariants"][0]["driver_units_per_second"] = serde_json::json!(10.0);
    let errs = lint_errors(json);
    assert!(
        errs.is_empty(),
        "fitting rate, complete declaration: {errs:?}"
    );
}

#[test]
fn pairwise_separation_declared_rate_must_fit_the_run() {
    // Same declared-rate fit arithmetic as stagnant_resource: a soak
    // the run can never elapse is unjudgeable, rejected at load.
    let mut json = pairwise_json(
        Some("Enemy"),
        serde_json::json!(0.5),
        serde_json::json!(600.0),
    );
    json["invariants"][0]["driver_resource"] = serde_json::json!("MatchClock");
    json["invariants"][0]["driver_path"] = serde_json::json!("minutes");
    json["invariants"][0]["driver_units_per_second"] = serde_json::json!(10.0);
    let errs = lint_errors(json);
    assert!(
        errs.iter().any(|e| e.contains("exceeds the run duration")),
        "600 units at 10/s = 60s > 10s run: {errs:?}"
    );

    // A fitting rate passes the fit check — the declaration lints
    // clean end to end.
    let mut json = pairwise_json(
        Some("Enemy"),
        serde_json::json!(0.5),
        serde_json::json!(50.0),
    );
    json["invariants"][0]["driver_resource"] = serde_json::json!("MatchClock");
    json["invariants"][0]["driver_path"] = serde_json::json!("minutes");
    json["invariants"][0]["driver_units_per_second"] = serde_json::json!(10.0);
    let errs = lint_errors(json);
    assert!(
        errs.is_empty(),
        "fitting rate, complete declaration: {errs:?}"
    );
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

// -----------------------------------------------------------------------
// forbidden_region grammar (the episode-tier no-go-zone oracle)
// -----------------------------------------------------------------------

fn forbidden_json(
    selector: Option<&str>,
    region: serde_json::Value,
    window_s: serde_json::Value,
) -> serde_json::Value {
    let selector = match selector {
        Some(name) => serde_json::json!({ "with_component": { "name": name } }),
        None => serde_json::json!(null),
    };
    serde_json::json!({
        "bot": { "type": "chaos" },
        "duration_s": 10.0,
        "invariants": [{
            "name": "no_rail_tunneling",
            "rule": "forbidden_region",
            "target_selector": selector,
            "region": region,
            "window_s": window_s,
        }],
    })
}

fn good_region() -> serde_json::Value {
    serde_json::json!({
        "center": [0.0, 0.5, 0.0],
        "half_extents": [0.5, 1.0, 0.5],
    })
}

#[test]
fn forbidden_region_parses_and_validates_clean() {
    // The full authoring surface is real: parse succeeds, shape
    // lint is clean, validation accepts — the episode recorder
    // evaluates the rule at run time.
    let s = parse(forbidden_json(
        Some("Wanderer"),
        good_region(),
        serde_json::json!(1.0),
    ));
    assert!(s.invariants[0].region.is_some());
    let errs = lint_errors(forbidden_json(
        Some("Wanderer"),
        good_region(),
        serde_json::json!(1.0),
    ));
    assert!(
        errs.is_empty(),
        "well-formed grammar must lint clean: {errs:?}"
    );
    validate_scenario(&s).expect("well-formed forbidden_region validates");
}

#[test]
fn forbidden_region_requires_a_component_selector() {
    // "all" is the deserialized absence of a selector: forbidding a
    // region to every Transform in the world — walls, camera, UI —
    // is never what an author means.
    let errs = lint_errors(forbidden_json(None, good_region(), serde_json::json!(1.0)));
    assert!(
        errs.iter()
            .any(|e| e.contains("forbidden_region requires") && e.contains("target_selector")),
        "{errs:?}"
    );
}

#[test]
fn forbidden_region_requires_a_region() {
    // No box, no rule: a regionless forbidden_region would vac-pass.
    let mut json = forbidden_json(
        Some("Wanderer"),
        serde_json::json!(null),
        serde_json::json!(1.0),
    );
    json["invariants"][0]
        .as_object_mut()
        .unwrap()
        .remove("region");
    let errs = lint_errors(json);
    assert!(
        errs.iter()
            .any(|e| e.contains("requires a region") || e.contains("region")),
        "{errs:?}"
    );
}

#[test]
fn forbidden_region_requires_finite_positive_extents() {
    // A non-positive half-extent is not a box: a zero-thickness
    // region is a plane (nothing can enter it — vacuous), a
    // negative one is an empty slab. Neither is ever an author's
    // meaning. (JSON cannot carry NaN/Infinity as numbers, so the
    // finiteness half of the check is unreachable from the wire —
    // zero/negative are the wire-representable degenerate shapes.)
    for bad in [
        serde_json::json!({ "center": [0.0, 0.5, 0.0], "half_extents": [0.0, 1.0, 0.5] }),
        serde_json::json!({ "center": [0.0, 0.5, 0.0], "half_extents": [-0.5, 1.0, 0.5] }),
    ] {
        let errs = lint_errors(forbidden_json(
            Some("Wanderer"),
            bad,
            serde_json::json!(1.0),
        ));
        assert!(!errs.is_empty(), "degenerate region must reject: {errs:?}");
    }
}

#[test]
fn forbidden_region_requires_window_s() {
    // window_s is the episode soak: without it there is no
    // judgment — a region could never fire regardless of how long
    // entities tunnel through it.
    let mut json = forbidden_json(Some("Wanderer"), good_region(), serde_json::json!(null));
    json["invariants"][0]
        .as_object_mut()
        .unwrap()
        .remove("window_s");
    let errs = lint_errors(json);
    assert!(errs.iter().any(|e| e.contains("window_s")), "{errs:?}");
}

#[test]
fn forbidden_region_declared_rate_must_fit_the_run() {
    // Same declared-rate fit arithmetic as pairwise_separation: a
    // soak the run can never elapse is unjudgeable, rejected at
    // load.
    let mut json = forbidden_json(Some("Wanderer"), good_region(), serde_json::json!(600.0));
    json["invariants"][0]["driver_resource"] = serde_json::json!("FrMatchClock");
    json["invariants"][0]["driver_path"] = serde_json::json!("minutes");
    json["invariants"][0]["driver_units_per_second"] = serde_json::json!(10.0);
    let errs = lint_errors(json);
    assert!(
        errs.iter().any(|e| e.contains("exceeds the run duration")),
        "600 units at 10/s = 60s > 10s run: {errs:?}"
    );

    // A fitting rate passes the fit check — the declaration lints
    // clean end to end.
    let mut json = forbidden_json(Some("Wanderer"), good_region(), serde_json::json!(50.0));
    json["invariants"][0]["driver_resource"] = serde_json::json!("FrMatchClock");
    json["invariants"][0]["driver_path"] = serde_json::json!("minutes");
    json["invariants"][0]["driver_units_per_second"] = serde_json::json!(10.0);
    let errs = lint_errors(json);
    assert!(
        errs.is_empty(),
        "fitting rate, complete declaration: {errs:?}"
    );
}

#[test]
fn forbidden_region_rejects_foreign_fields() {
    // The one generic foreign-field check: `region`/`allow_if`
    // belong to forbidden_region alone, and every other rule's
    // fields are foreign here (min_distance is pairwise's
    // threshold, tolerance is DTC's). Both directions must reject
    // — the cross-products must never silently no-op.
    for (field, value) in [
        ("min_distance", serde_json::json!(0.5)),
        ("tolerance", serde_json::json!(0.01)),
        ("check", serde_json::json!("le")),
        ("value", serde_json::json!(1.0)),
    ] {
        let mut json = forbidden_json(Some("Wanderer"), good_region(), serde_json::json!(1.0));
        json["invariants"][0][field] = value;
        let errs = lint_errors(json);
        assert!(
            errs.iter().any(|e| e.contains(field)),
            "'{field}' must be rejected on forbidden_region: {errs:?}"
        );
    }

    // And the reverse: `region` on another rule rejects too.
    let mut json = minimal_scenario("chaos");
    json["invariants"] = serde_json::json!([{
        "name": "hp_floor",
        "rule": "component_check",
        "target_selector": {"with_component": {"name": "Hp"}},
        "path": "current",
        "check": "le",
        "value": 100,
        "region": good_region(),
    }]);
    let errs = lint_errors(json);
    assert!(
        errs.iter().any(|e| e.contains("region")),
        "'region' must be rejected on component_check: {errs:?}"
    );
}

fn pairwise_json(
    selector: Option<&str>,
    min_distance: serde_json::Value,
    window_s: serde_json::Value,
) -> serde_json::Value {
    let selector = match selector {
        Some(name) => serde_json::json!({ "with_component": { "name": name } }),
        None => serde_json::json!(null),
    };
    serde_json::json!({
        "bot": { "type": "chaos" },
        "duration_s": 10.0,
        "invariants": [{
            "name": "no_coincident_enemies",
            "rule": "pairwise_separation",
            "target_selector": selector,
            "min_distance": min_distance,
            "window_s": window_s,
        }],
    })
}

fn lint_errors(json: serde_json::Value) -> Vec<String> {
    use bevy_playtest::scenario::lint_scenario;
    let s = parse(json);
    lint_scenario(&s)
        .into_iter()
        .map(|e| e.to_string())
        .collect()
}

#[test]
fn pairwise_separation_parses_and_validates_clean() {
    // The full authoring surface is real: the parse succeeds, the
    // shape lint is clean, and validation accepts the scenario —
    // the episode recorder evaluates the rule at run time.
    let s = parse(pairwise_json(
        Some("Enemy"),
        serde_json::json!(0.5),
        serde_json::json!(1.0),
    ));
    assert_eq!(s.invariants[0].min_distance, Some(0.5));
    let errs = lint_errors(pairwise_json(
        Some("Enemy"),
        serde_json::json!(0.5),
        serde_json::json!(1.0),
    ));
    assert!(
        errs.is_empty(),
        "well-formed grammar must lint clean: {errs:?}"
    );
    validate_scenario(&s).expect("well-formed pairwise_separation validates");
}

#[test]
fn pairwise_separation_requires_a_component_selector() {
    // "all" is the deserialized absence of a selector: pairwising
    // every Transform in the world (walls, camera, UI) at O(n²) is
    // never what an author means.
    let errs = lint_errors(pairwise_json(
        None,
        serde_json::json!(0.5),
        serde_json::json!(1.0),
    ));
    assert!(
        errs.iter()
            .any(|e| e.contains("pairwise_separation requires") && e.contains("target_selector")),
        "{errs:?}"
    );

    let errs = lint_errors(pairwise_json(
        Some("  "),
        serde_json::json!(0.5),
        serde_json::json!(1.0),
    ));
    assert!(
        errs.iter().any(|e| e.contains("name must be non-empty")),
        "{errs:?}"
    );
}

#[test]
fn pairwise_separation_requires_positive_min_distance() {
    for bad in [0.0, -1.0] {
        let errs = lint_errors(pairwise_json(
            Some("Enemy"),
            serde_json::json!(bad),
            serde_json::json!(1.0),
        ));
        assert!(
            errs.iter()
                .any(|e| e.contains("min_distance must be a finite number > 0")),
            "min_distance={bad}: {errs:?}"
        );
    }
    let errs = lint_errors(pairwise_json(
        Some("Enemy"),
        serde_json::json!(null),
        serde_json::json!(1.0),
    ));
    assert!(
        errs.iter()
            .any(|e| e.contains("pairwise_separation requires 'min_distance'")),
        "{errs:?}"
    );
}

#[test]
fn pairwise_separation_requires_a_soak_window() {
    // Instant-fire form (no window_s) is rejected at load: ε
    // semantics plus a soak is the rule's contract, and rejecting
    // the authoring mistake once is cheaper than a post-run
    // finding explaining fly-bys.
    let errs = lint_errors(pairwise_json(
        Some("Enemy"),
        serde_json::json!(0.5),
        serde_json::json!(null),
    ));
    assert!(
        errs.iter()
            .any(|e| e.contains("pairwise_separation requires 'window_s'")),
        "{errs:?}"
    );

    for bad in [0.0, -2.0] {
        let errs = lint_errors(pairwise_json(
            Some("Enemy"),
            serde_json::json!(0.5),
            serde_json::json!(bad),
        ));
        assert!(
            errs.iter()
                .any(|e| e.contains("window_s must be a finite number > 0")),
            "window_s={bad}: {errs:?}"
        );
    }
}

#[test]
fn pairwise_separation_rejects_foreign_fields() {
    // Rule-exclusive fields silently accepted = the reserved-field
    // hole the schema lint exists to close.
    for (field, value) in [
        ("check", serde_json::json!("le")),
        ("value", serde_json::json!(100)),
        ("path", serde_json::json!("translation.z")),
        ("resource", serde_json::json!("Scoreboard")),
        ("min_x", serde_json::json!(-900.0)),
        ("max_z", serde_json::json!(900.0)),
    ] {
        let mut json = pairwise_json(
            Some("Enemy"),
            serde_json::json!(0.5),
            serde_json::json!(1.0),
        );
        json["invariants"][0][field] = value;
        let errs = lint_errors(json);
        assert!(
            errs.iter()
                .any(|e| e.contains("pairwise_separation does not use")),
            "{field}: {errs:?}"
        );
    }
    // eventually_s is rejected by the GENERIC eventually-mode check
    // (one defect, one error) — assert that message, not a
    // pairwise-specific one.
    let mut json = pairwise_json(
        Some("Enemy"),
        serde_json::json!(0.5),
        serde_json::json!(1.0),
    );
    json["invariants"][0]["eventually_s"] = serde_json::json!(2.0);
    let errs = lint_errors(json);
    assert!(
        errs.iter()
            .any(|e| e.contains("eventually mode is only supported")),
        "{errs:?}"
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
fn mouse_motion_defaults_and_parses() {
    // Minimal motion: frame + deltas only; repeat defaults to 1
    // frame. Motion alone satisfies the synthetic_keyboard non-empty
    // check (orthogonal input channel, same doctrine as clicks).
    let mut json = minimal_scenario("synthetic_keyboard");
    json["bot"]["mouse_motion"] = serde_json::json!([{ "frame": 3, "dx": 10.0, "dy": -5.0 }]);
    let s: Scenario = serde_json::from_value(json).unwrap();
    assert_eq!(s.bot.mouse_motion.len(), 1);
    assert_eq!(s.bot.mouse_motion[0].frame, 3);
    assert_eq!(s.bot.mouse_motion[0].dx, 10.0);
    assert_eq!(s.bot.mouse_motion[0].dy, -5.0);
    assert_eq!(s.bot.mouse_motion[0].repeat_frames, 1);
    validate_scenario(&s).expect("motion-only synthetic bot is valid");
}

#[test]
fn mouse_motion_rejects_zero_repeat() {
    let mut json = minimal_scenario("synthetic_keyboard");
    json["bot"]["mouse_motion"] =
        serde_json::json!([{ "frame": 0, "dx": 1.0, "dy": 0.0, "repeat_frames": 0 }]);
    let s = parse(json);
    let err = validate_scenario(&s).unwrap_err().to_string();
    assert!(err.contains("repeat_frames"), "{err}");
}

#[test]
fn mouse_motion_rejects_non_finite_deltas() {
    // serde_json::json! cannot express NaN literally, but arbitrary
    // precision / non-standard sources can deliver huge exponents
    // that overflow f32 to infinity — the finiteness guard is what
    // stands between them and poisoned cursor arithmetic.
    let mut json = minimal_scenario("synthetic_keyboard");
    json["bot"]["mouse_motion"] =
        serde_json::json!([{ "frame": 0, "dx": 1.0e40, "dy": 0.0, "repeat_frames": 5 }]);
    let s = parse(json);
    let err = validate_scenario(&s).unwrap_err().to_string();
    assert!(err.contains("finite"), "{err}");
}

#[test]
fn mouse_motion_rejects_entry_past_run_end() {
    // duration_s 1.0 at the default tps 60 → 60 ticks, so frame 60
    // starts at/past the run end and would apply on no frame — the
    // same silent no-op zero-repeat guards against. Also covers the
    // driver's window-bound arithmetic with huge frames: the
    // rejection keeps author-controlled u64s out of it entirely.
    let mut json = minimal_scenario("synthetic_keyboard");
    json["bot"]["mouse_motion"] = serde_json::json!([{ "frame": 60, "dx": 1.0, "dy": 0.0 }]);
    let s = parse(json);
    let err = validate_scenario(&s).unwrap_err().to_string();
    assert!(err.contains("silent"), "{err}");

    // The extreme end of the same class: u64::MAX would have
    // overflowed the driver's `frame + repeat_frames` bound before
    // the saturating_add fix; the validation now keeps such entries
    // out of the driver entirely.
    let mut json = minimal_scenario("synthetic_keyboard");
    json["bot"]["mouse_motion"] =
        serde_json::json!([{ "frame": u64::MAX, "dx": 1.0, "dy": 0.0, "repeat_frames": u64::MAX }]);
    let s = parse(json);
    let err = validate_scenario(&s).unwrap_err().to_string();
    assert!(err.contains("silent"), "{err}");
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

// ---------------------------------------------------------------------------
// derived_transform_consistency — the authoring grammar (slice 2, part 1)
// ---------------------------------------------------------------------------

fn dtc_json(selector: Option<&str>, window_s: serde_json::Value) -> serde_json::Value {
    let selector = match selector {
        Some(name) => serde_json::json!({ "with_component": { "name": name } }),
        None => serde_json::json!(null),
    };
    serde_json::json!({
        "bot": { "type": "chaos" },
        "duration_s": 10.0,
        "invariants": [{
            "name": "globals_follow_parents",
            "rule": "derived_transform_consistency",
            "target_selector": selector,
            "window_s": window_s,
        }],
    })
}

#[test]
fn dtc_parses_and_lints_the_full_authoring_surface() {
    // The full surface is real and parseable, and the consistency
    // recorder evaluates it at run time — so the shape lint must
    // collect NOTHING: the grammar is sound and the rule is live.
    let full = serde_json::json!({
        "bot": { "type": "chaos" },
        "duration_s": 10.0,
        "invariants": [{
            "name": "globals_follow_parents",
            "rule": "derived_transform_consistency",
            "target_selector": { "with_component": { "name": "Cabin" } },
            "tolerance": 0.0001,
            "grace_frames": 3,
            "exclude_components": ["CameraSmoothGhost"],
            "window_s": 1.0,
        }],
    });
    let s = parse(full.clone());
    assert_eq!(s.invariants[0].tolerance, Some(0.0001));
    assert_eq!(s.invariants[0].grace_frames, Some(3));
    assert_eq!(
        s.invariants[0].exclude_components,
        vec!["CameraSmoothGhost"]
    );
    // The gate is gone: the full authoring surface lints clean.
    let errs = lint_errors(full);
    assert!(
        errs.is_empty(),
        "well-formed grammar must lint clean now that the \
         recorder evaluates it: {errs:?}"
    );
    // validate: a fully-coherent DTC rule is accepted outright
    // (parse-success / lint-collect / validate-accept trio).
    let s = parse(dtc_json(Some("Cabin"), serde_json::json!(1.0)));
    assert!(
        validate_scenario(&s).is_ok(),
        "a well-formed consistency rule must validate"
    );
}

#[test]
fn dtc_all_selector_is_the_documented_default_not_a_requirement() {
    // Unlike pairwise, the '*' default selector is legal: the
    // whole hierarchy is exactly what the rule is FOR. Only an
    // explicit selector is validated at load; absence means all.
    let errs = lint_errors(dtc_json(None, serde_json::json!(1.0)));
    assert!(errs.is_empty(), "gate-free clean lint: {errs:?}");
}

#[test]
fn dtc_requires_a_persistence_soak_window() {
    // Consistency defects are STATE bugs: a single violating
    // frame is noise, so window_s is required and must be a
    // finite positive number — the same instant-fire rejection
    // doctrine as pairwise.
    let errs = lint_errors(dtc_json(Some("Cabin"), serde_json::json!(null)));
    assert!(
        errs.iter()
            .any(|e| e.contains("derived_transform_consistency requires 'window_s'")),
        "{errs:?}"
    );
    for bad in [0.0, -2.0] {
        let errs = lint_errors(dtc_json(Some("Cabin"), serde_json::json!(bad)));
        assert!(
            errs.iter()
                .any(|e| e.contains("window_s must be a finite number > 0")),
            "window_s={bad}: {errs:?}"
        );
    }
}

#[test]
fn dtc_rejects_a_rate_without_a_driver() {
    // Frame-clock default is seconds by definition; a declared
    // rate would mis-validate the soak fit in fake units.
    let mut json = dtc_json(Some("Cabin"), serde_json::json!(1.0));
    json["invariants"][0]["driver_units_per_second"] = serde_json::json!(60.0);
    let errs = lint_errors(json);
    assert!(
        errs.iter()
            .any(|e| e.contains("driver_units_per_second needs the driver declared")),
        "{errs:?}"
    );
}

#[test]
fn dtc_declared_rate_must_fit_the_run() {
    // A soak the run can never elapse is unjudgeable — the shared
    // driver-leaf arithmetic, same as pairwise/stagnant.
    let mut json = dtc_json(Some("Cabin"), serde_json::json!(600.0));
    json["invariants"][0]["driver_resource"] = serde_json::json!("MatchClock");
    json["invariants"][0]["driver_path"] = serde_json::json!("ticks");
    json["invariants"][0]["driver_units_per_second"] = serde_json::json!(10.0);
    // 600 units at 10/s = 60s > the 10s run: rejected.
    let errs = lint_errors(json);
    assert!(
        errs.iter().any(|e| e.contains("exceeds the run duration")),
        "{errs:?}"
    );
}

#[test]
fn dtc_frame_clock_window_must_fit_the_run() {
    // The DEFAULT driver (frame clock, seconds) is the path most
    // authors take — and without this check, an over-long window
    // made the recorder silently discard every episode as
    // sub-soak: a run that proves nothing, reported as Pass.
    // The window compares straight against duration_s (1 unit/s).
    let errs = lint_errors(dtc_json(Some("Cabin"), serde_json::json!(20.0)));
    assert!(
        errs.iter().any(|e| e.contains("exceeds the run duration")),
        "20s soak against a 10s run: {errs:?}"
    );
    // Equal-to-run stays legal: the episode may sustain to the
    // very end of the run and exactly elapse it (the fixture
    // corpus pins this shape: a fly-by control with window ==
    // duration).
    let errs = lint_errors(dtc_json(Some("Cabin"), serde_json::json!(10.0)));
    assert!(errs.is_empty(), "window == duration is judgeable: {errs:?}");
}

#[test]
fn pairwise_frame_clock_window_must_fit_the_run() {
    // Same hole, same fix, for the sibling episode-fold rule.
    let errs = lint_errors(pairwise_json(
        Some("Enemy"),
        serde_json::json!(0.5),
        serde_json::json!(20.0),
    ));
    assert!(
        errs.iter().any(|e| e.contains("exceeds the run duration")),
        "20s soak against a 10s run: {errs:?}"
    );
    let errs = lint_errors(pairwise_json(
        Some("Enemy"),
        serde_json::json!(0.5),
        serde_json::json!(10.0),
    ));
    assert!(errs.is_empty(), "window == duration is judgeable: {errs:?}");
}

#[test]
fn dtc_grace_and_window_must_share_the_frame_budget() {
    // The compound fit: an episode accrues span only on JUDGED
    // (post-grace) frames, so a window that fits the whole run
    // can still be unjudgeable — the grace ate the only frames
    // that could have accrued the span. 3s run at 60tps = 180
    // frames; grace 150 (2.5s) leaves 0.5s judged; a 1.0s window
    // can never be elapsed.
    let json = serde_json::json!({
        "bot": { "type": "chaos" },
        "duration_s": 3.0,
        "tps": 60,
        "invariants": [{
            "name": "globals_follow_parents",
            "rule": "derived_transform_consistency",
            "target_selector": { "with_component": { "name": "Cabin" } },
            "grace_frames": 150,
            "window_s": 1.0,
        }],
    });
    let errs = lint_errors(json);
    assert!(
        errs.iter()
            .any(|e| e.contains("does not fit the judged frames")),
        "1.0s window against a 0.5s judged remainder: {errs:?}"
    );
    // The fix that fits: a window the grace-adjusted run can
    // elapse lints clean.
    let json = serde_json::json!({
        "bot": { "type": "chaos" },
        "duration_s": 3.0,
        "tps": 60,
        "invariants": [{
            "name": "globals_follow_parents",
            "rule": "derived_transform_consistency",
            "target_selector": { "with_component": { "name": "Cabin" } },
            "grace_frames": 150,
            "window_s": 0.4,
        }],
    });
    let errs = lint_errors(json);
    assert!(errs.is_empty(), "fitting grace+window: {errs:?}");
}

#[test]
fn dtc_driver_needs_both_halves() {
    // Both halves or neither: there is no `resource` to default
    // the host to (pairwise's doctrine carried over).
    for (dr, dp) in [(Some("MatchClock"), None), (None, Some("ticks"))] {
        let mut json = dtc_json(Some("Cabin"), serde_json::json!(1.0));
        if let Some(dr) = dr {
            json["invariants"][0]["driver_resource"] = serde_json::json!(dr);
        }
        if let Some(dp) = dp {
            json["invariants"][0]["driver_path"] = serde_json::json!(dp);
        }
        let errs = lint_errors(json);
        assert!(
            errs.iter()
                .any(|e| e.contains("needs both 'driver_resource' and 'driver_path'")),
            "{dr:?}/{dp:?}: {errs:?}"
        );
    }
}

#[test]
fn dtc_rejects_a_bad_tolerance() {
    // Zero fires on f32 composition noise; negative/NaN accept or
    // invert the check. Positive finite is the only sound ε.
    for bad in [0.0, -0.001] {
        let mut json = dtc_json(Some("Cabin"), serde_json::json!(1.0));
        json["invariants"][0]["tolerance"] = serde_json::json!(bad);
        let errs = lint_errors(json);
        assert!(
            errs.iter()
                .any(|e| e.contains("tolerance must be a finite number > 0")),
            "tolerance={bad}: {errs:?}"
        );
    }
}

#[test]
fn dtc_rejects_blank_exclusions_and_driver_blanks() {
    // A blank exclude_components prefix matches every component;
    // a blank driver path resolves the resource root, never a
    // numeric leaf — both authoring mistakes caught at load.
    let mut json = dtc_json(Some("Cabin"), serde_json::json!(1.0));
    json["invariants"][0]["exclude_components"] = serde_json::json!(["   "]);
    json["invariants"][0]["driver_resource"] = serde_json::json!("MatchClock");
    json["invariants"][0]["driver_path"] = serde_json::json!("");
    let errs = lint_errors(json);
    assert!(
        errs.iter()
            .any(|e| e.contains("exclude_components entries must be non-empty")),
        "{errs:?}"
    );
    assert!(
        errs.iter()
            .any(|e| e.contains("'driver_path' must be non-empty")),
        "{errs:?}"
    );
}

#[test]
fn dtc_rejects_foreign_fields() {
    // The reserved-field doctrine: fields that belong to other
    // rules silently accepted = the hole the lint closes. Every
    // foreign field must reject with the rule-named message.
    for (field, value) in [
        ("check", serde_json::json!("le")),
        ("value", serde_json::json!(100)),
        ("path", serde_json::json!("translation.z")),
        ("resource", serde_json::json!("Scoreboard")),
        ("min_distance", serde_json::json!(0.5)),
        ("min_x", serde_json::json!(-900.0)),
        ("max_z", serde_json::json!(900.0)),
    ] {
        let mut json = dtc_json(Some("Cabin"), serde_json::json!(1.0));
        json["invariants"][0][field] = value;
        let errs = lint_errors(json);
        assert!(
            errs.iter()
                .any(|e| e.contains("derived_transform_consistency does not use")),
            "{field}: {errs:?}"
        );
    }
}

#[test]
fn dtc_owned_fields_reject_on_foreign_rules() {
    // The reverse direction of the reserved-field doctrine: the
    // three consistency-owned knobs parsed-and-ignored on any other
    // rule would be a silently dead knob — the author writes it,
    // the harness accepts it, no rule ever reads it. Every
    // combination must reject, naming the fields.
    for (field, value) in [
        ("tolerance", serde_json::json!(0.001)),
        ("grace_frames", serde_json::json!(3)),
        ("exclude_components", serde_json::json!(["Foo"])),
    ] {
        let value = &value;
        for rule in [
            "component_check",
            "pairwise_separation",
            "stagnant_resource",
        ] {
            let mut json = serde_json::json!({
                "bot": { "type": "chaos" },
                "duration_s": 10.0,
                "invariants": [{
                    "name": "foreign_knob",
                    "rule": rule,
                }],
            });
            // Give each rule its required fields so the ONLY error
            // is the foreign-knob rejection.
            match rule {
                "component_check" => {
                    json["invariants"][0]["target_selector"] =
                        serde_json::json!({"with_component": {"name": "Cabin"}});
                    json["invariants"][0]["path"] = serde_json::json!("translation.x");
                    json["invariants"][0]["check"] = serde_json::json!("lt");
                    json["invariants"][0]["value"] = serde_json::json!(900.0);
                }
                "pairwise_separation" => {
                    json["invariants"][0]["target_selector"] =
                        serde_json::json!({"with_component": {"name": "Cabin"}});
                    json["invariants"][0]["min_distance"] = serde_json::json!(0.5);
                    json["invariants"][0]["window_s"] = serde_json::json!(1.0);
                }
                "stagnant_resource" => {
                    json["invariants"][0]["resource"] = serde_json::json!("MatchClock");
                    json["invariants"][0]["path"] = serde_json::json!("ticks");
                    json["invariants"][0]["driver_path"] = serde_json::json!("ticks");
                    json["invariants"][0]["window_s"] = serde_json::json!(1.0);
                }
                _ => unreachable!(),
            }
            json["invariants"][0][field] = value.clone();
            let errs = lint_errors(json.clone());
            let expected = format!("'{field}'");
            assert!(
                errs.iter()
                    .any(|e| e.contains(&expected)
                        && e.contains("derived_transform_consistency only")),
                "{rule} + {field}: {errs:?}"
            );
            // And the rejection is the ONLY error: the required
            // fields are satisfied, so nothing else fires.
            assert!(
                errs.len() == 1,
                "the foreign-knob rejection must be the only error \
                 (rule {rule}, field {field}): {errs:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------
// spawn_safety — the authoring grammar (remaining-spatial-family part 1)
// ---------------------------------------------------------------------------

fn spawn_safety_json(
    selector: Option<&str>,
    min_distance_from: Option<serde_json::Value>,
    min_distance: serde_json::Value,
) -> serde_json::Value {
    let selector = match selector {
        Some(name) => serde_json::json!({ "with_component": { "name": name } }),
        None => serde_json::json!(null),
    };
    serde_json::json!({
        "bot": { "type": "chaos" },
        "duration_s": 10.0,
        "invariants": [{
            "name": "safe_respawns",
            "rule": "spawn_safety",
            "target_selector": selector,
            "min_distance_from": min_distance_from,
            "min_distance": min_distance,
        }],
    })
}

#[test]
fn spawn_safety_parses_and_validates_clean() {
    // The full authoring surface is real: the parse succeeds, the
    // shape lint is clean, and validation accepts the scenario —
    // the diff/judge recorder evaluates the rule at run time.
    let full = serde_json::json!({
        "bot": { "type": "chaos" },
        "duration_s": 10.0,
        "invariants": [{
            "name": "safe_respawns",
            "rule": "spawn_safety",
            "target_selector": { "with_component": { "name": "Projectile" } },
            "min_distance_from": { "with_component": { "name": "Player" } },
            "min_distance": 0.5,
            "spawn_only": true,
        }],
    });
    let s = parse(full.clone());
    assert_eq!(s.invariants[0].spawn_only, Some(true));
    let errs = lint_errors(full);
    assert!(
        errs.is_empty(),
        "well-formed spawn_safety must lint clean: {errs:?}"
    );
    validate_scenario(&s).expect("well-formed spawn_safety validates");
}

#[test]
fn spawn_safety_accepts_all_anchors() {
    // `min_distance_from: "all"` is the broadest anchor set —
    // every spatially observable entity — and must be legal.
    let errs = lint_errors(spawn_safety_json(
        Some("Projectile"),
        Some(serde_json::json!("all")),
        serde_json::json!(0.5),
    ));
    assert!(errs.is_empty(), "all-anchors is legal: {errs:?}");
}

#[test]
fn spawn_safety_requires_a_component_selector() {
    // The judged archetype must be NAMED: unlike DTC (whose '*'
    // default is the whole hierarchy — exactly what that rule is
    // for), an entry-diff over every Transform-carrying entity
    // judges half the world's happenings by accident. The rule
    // demands intent.
    let errs = lint_errors(spawn_safety_json(
        None,
        Some(serde_json::json!("all")),
        serde_json::json!(0.5),
    ));
    assert!(
        errs.iter()
            .any(|e| e.contains("spawn_safety requires") && e.contains("target_selector")),
        "missing selector: {errs:?}"
    );
    // An explicit all-selector is equally rejected: the default
    // is not a stand-in for intent here.
    let mut json = spawn_safety_json(None, Some(serde_json::json!("all")), serde_json::json!(0.5));
    json["invariants"][0]["target_selector"] = serde_json::json!("all");
    let errs = lint_errors(json);
    assert!(
        errs.iter()
            .any(|e| e.contains("spawn_safety requires") && e.contains("target_selector")),
        "explicit all-selector: {errs:?}"
    );
}

#[test]
fn spawn_safety_requires_an_anchor_selector_and_positive_distance() {
    // Both halves of the judgment are required: the anchor set
    // (what entries must keep clear of) and a positive finite
    // min_distance (the keep-clear radius).
    let errs = lint_errors(spawn_safety_json(
        Some("Projectile"),
        None,
        serde_json::json!(0.5),
    ));
    assert!(
        errs.iter()
            .any(|e| e.contains("spawn_safety requires 'min_distance_from'")),
        "missing anchors: {errs:?}"
    );
    for bad in [
        serde_json::json!(null),
        serde_json::json!(0.0),
        serde_json::json!(-2.0),
    ] {
        let errs = lint_errors(spawn_safety_json(
            Some("Projectile"),
            Some(serde_json::json!("all")),
            bad.clone(),
        ));
        assert!(
            errs.iter().any(|e| {
                e.contains("spawn_safety requires 'min_distance'")
                    || e.contains("min_distance must be a finite number > 0")
            }),
            "bad min_distance {bad}: {errs:?}"
        );
    }
}

#[test]
fn spawn_safety_rejects_temporal_and_foreign_knobs() {
    // The transition frame is judged, never soaked: window_s and
    // the driver knobs of the soak-era rules are foreign here, and
    // the check/value/... surface of the scalar rules likewise.
    for (field, values) in [
        (
            "window_s",
            &[&serde_json::json!(1.0)] as &[&serde_json::Value],
        ),
        ("driver_resource", &[&serde_json::json!("MatchClock")]),
        ("driver_path", &[&serde_json::json!("minutes")]),
        ("driver_units_per_second", &[&serde_json::json!(10.0)]),
        ("check", &[&serde_json::json!("lt")]),
        ("value", &[&serde_json::json!(900.0)]),
        ("path", &[&serde_json::json!("translation.x")]),
        ("resource", &[&serde_json::json!("MatchClock")]),
        ("min_x", &[&serde_json::json!(1.0)]),
    ] {
        for value in values {
            let mut json = spawn_safety_json(
                Some("Projectile"),
                Some(serde_json::json!("all")),
                serde_json::json!(0.5),
            );
            json["invariants"][0][field] = (*value).clone();
            let errs = lint_errors(json.clone());
            assert!(
                errs.iter().any(|e| e.contains("spawn_safety does not use")),
                "{field}: {errs:?}"
            );
        }
    }
    // eventually_s rejects too — via the GENERIC eventually-mode
    // arm, not the spawn_safety bundle (one defect, one error,
    // the pairwise doctrine: the generic message is the better
    // error and the rule-specific one would double-report).
    let mut json = spawn_safety_json(
        Some("Projectile"),
        Some(serde_json::json!("all")),
        serde_json::json!(0.5),
    );
    json["invariants"][0]["eventually_s"] = serde_json::json!(2.0);
    let errs = lint_errors(json.clone());
    assert!(
        errs.iter()
            .any(|e| e.contains("eventually mode is only supported")),
        "eventually_s: {errs:?}"
    );
}

#[test]
fn spawn_safety_fields_are_foreign_on_other_rules() {
    // min_distance_from and spawn_only are single-owner fields:
    // on any other rule they are parsed-but-never-read knobs and
    // must reject as spawn_safety-only.
    for (rule, filler) in [
        (
            "pairwise_separation",
            serde_json::json!({
                "target_selector": { "with_component": { "name": "Cabin" } },
                "min_distance": 0.5,
                "window_s": 1.0,
            }),
        ),
        (
            "component_check",
            serde_json::json!({
                "target_selector": { "with_component": { "name": "Cabin" } },
                "path": "translation.x",
                "check": "lt",
                "value": 900.0,
            }),
        ),
    ] {
        for field in ["min_distance_from", "spawn_only"] {
            let mut json = minimal_scenario("chaos");
            let mut inv = serde_json::json!({
                "name": "inv",
                "rule": rule,
            });
            for (k, v) in filler.as_object().unwrap() {
                inv[k] = v.clone();
            }
            inv[field] = if field == "spawn_only" {
                serde_json::json!(true)
            } else {
                serde_json::json!("all")
            };
            json["invariants"] = serde_json::json!([inv]);
            let errs = lint_errors(json);
            assert!(
                errs.iter().any(|e| e.contains("spawn_safety only")),
                "{rule} + {field}: {errs:?}"
            );
        }
    }
}
fn stag_json(
    selector: Option<&str>,
    watch_path: serde_json::Value,
    window_s: serde_json::Value,
) -> serde_json::Value {
    let selector = match selector {
        Some(name) => serde_json::json!({ "with_component": { "name": name } }),
        None => serde_json::json!(null),
    };
    serde_json::json!({
        "bot": { "type": "chaos" },
        "duration_s": 10.0,
        "invariants": [{
            "name": "ride_queue_must_not_stall",
            "rule": "entity_stagnation",
            "target_selector": selector,
            "watch_path": watch_path,
            "window_s": window_s,
        }],
    })
}

#[test]
fn entity_stagnation_parses_and_validates_clean() {
    // The full authoring surface, minimal shape: selector +
    // watch_path + window_s. The driver defaults to the run's
    // frame clock and sampling_s to 1.0 second — a coherent
    // cadence without a single extra knob.
    let s = parse(stag_json(
        Some("Guest"),
        serde_json::json!("state.Queued.waited"),
        serde_json::json!(2.0),
    ));
    assert_eq!(
        s.invariants[0].watch_path.as_deref(),
        Some("state.Queued.waited")
    );
    assert_eq!(s.invariants[0].window_s, Some(2.0));
    assert!(
        s.invariants[0].driver.is_none(),
        "driver defaults to global_time"
    );
    assert_eq!(
        s.invariants[0].sampling_s, None,
        "sampling_s defaults to 1.0"
    );
    let errs = lint_errors(stag_json(
        Some("Guest"),
        serde_json::json!("state.Queued.waited"),
        serde_json::json!(2.0),
    ));
    assert!(errs.is_empty(), "minimal grammar must lint clean: {errs:?}");

    // The full surface: leaf_where precondition, explicit driver,
    // driver triple with rate, explicit sampling cadence.
    let mut json = stag_json(
        Some("Guest"),
        serde_json::json!("service.booked_at"),
        serde_json::json!(240.0),
    );
    json["invariants"][0]["leaf_where"] = serde_json::json!("service == Boarded");
    json["invariants"][0]["driver"] = serde_json::json!("resource");
    json["invariants"][0]["driver_resource"] = serde_json::json!("MatchClock");
    json["invariants"][0]["driver_path"] = serde_json::json!("ticks");
    json["invariants"][0]["driver_units_per_second"] = serde_json::json!(30.0);
    json["invariants"][0]["sampling_s"] = serde_json::json!(60.0);
    let errs = lint_errors(json);
    assert!(errs.is_empty(), "full grammar must lint clean: {errs:?}");
}

#[test]
fn entity_stagnation_requires_named_component_selector() {
    // `all` (or no selector) would stagnation-watch every
    // Transform in the world — walls and cameras are legitimately
    // inert; the rule demands a named component.
    let errs = lint_errors(stag_json(
        None,
        serde_json::json!("state.Queued.waited"),
        serde_json::json!(2.0),
    ));
    assert!(
        errs.iter().any(|e| e.contains("requires")
            && e.contains("target_selector")
            && e.contains("with_component")),
        "missing selector: {errs:?}"
    );
}

#[test]
fn entity_stagnation_requires_non_blank_watch_path() {
    for watch_path in [
        serde_json::json!(null),
        serde_json::json!(""),
        serde_json::json!("   "),
    ] {
        let errs = lint_errors(stag_json(
            Some("Guest"),
            watch_path.clone(),
            serde_json::json!(2.0),
        ));
        assert!(
            errs.iter().any(|e| e.contains("'watch_path'")),
            "watch_path {watch_path}: {errs:?}"
        );
    }
}

#[test]
fn entity_stagnation_leaf_where_shape_is_checked_at_load() {
    // Definite RESOLUTION is probe-time (the registry is not
    // available at load), but the `path == Variant` SHAPE is an
    // authoring error caught here. Note "a===b" is deliberately
    // absent: split_once("==") yields ("a", "=b"), a
    // well-formed shape whose garbage variant surfaces at
    // probe-time — never a silent "never true."
    let shapes = ["", "==", "==Queued", "state =="];
    let mut saw_shape_rejection = false;
    for lw in shapes {
        let mut json = stag_json(
            Some("Guest"),
            serde_json::json!("state.Queued.waited"),
            serde_json::json!(2.0),
        );
        json["invariants"][0]["leaf_where"] = serde_json::json!(lw);
        let errs = lint_errors(json);
        if errs.iter().any(|e| e.contains("'leaf_where' must be")) {
            saw_shape_rejection = true;
        } else {
            panic!("leaf_where {lw:?} must be shape-rejected: {errs:?}");
        }
    }
    assert!(saw_shape_rejection);
    // Redundant-but-consistent leaf_where alongside a
    // variant-qualified watch_path is legal (idempotent), and a
    // plain leaf_where without any variant in watch_path is the
    // normal precondition form.
    let mut json = stag_json(
        Some("Guest"),
        serde_json::json!("state.Queued.waited"),
        serde_json::json!(2.0),
    );
    json["invariants"][0]["leaf_where"] = serde_json::json!("state == Queued");
    let errs = lint_errors(json);
    assert!(errs.is_empty(), "consistent leaf_where is legal: {errs:?}");
}

#[test]
fn entity_stagnation_driver_discriminator_typo_rejects() {
    // The driver is a flat discriminator: global_time (the
    // default) or resource. Anything else is a typo the author
    // must hear about at load, never a silent fallback.
    for driver in ["clock", "frame_time", "Time<Virtual>", "Global"] {
        let mut json = stag_json(
            Some("Guest"),
            serde_json::json!("state.Queued.waited"),
            serde_json::json!(2.0),
        );
        json["invariants"][0]["driver"] = serde_json::json!(driver);
        let errs = lint_errors(json);
        assert!(
            errs.iter().any(|e| e.contains("'driver' must be")
                && e.contains("global_time")
                && e.contains("resource")),
            "driver {driver:?}: {errs:?}"
        );
    }
}

#[test]
fn entity_stagnation_global_time_rejects_driver_triple() {
    // The frame clock advances 1 second per second by definition;
    // the driver triple belongs to the resource form.
    let mut json = stag_json(
        Some("Guest"),
        serde_json::json!("state.Queued.waited"),
        serde_json::json!(2.0),
    );
    json["invariants"][0]["driver"] = serde_json::json!("global_time");
    json["invariants"][0]["driver_resource"] = serde_json::json!("MatchClock");
    json["invariants"][0]["driver_path"] = serde_json::json!("ticks");
    let errs = lint_errors(json);
    assert!(
        errs.iter()
            .any(|e| e.contains("global_time") && e.contains("does not use")),
        "driver triple on global_time: {errs:?}"
    );
}

#[test]
fn entity_stagnation_resource_driver_requires_rate_and_both_halves() {
    // The resource driver unlocks the driver triple verbatim, but
    // the rate is REQUIRED (not optional like stagnant_resource):
    // the sampling cadence is scheduled in frames, and converting
    // sampling_s out of driver-leaf units needs the rate.
    let base = |field: &str, value: serde_json::Value| {
        let mut json = stag_json(
            Some("Guest"),
            serde_json::json!("state.Queued.waited"),
            serde_json::json!(2.0),
        );
        json["invariants"][0]["driver"] = serde_json::json!("resource");
        json["invariants"][0][field] = value;
        json
    };
    // Missing halves.
    for (field, _) in [
        ("driver_resource", serde_json::json!("MatchClock")),
        ("driver_path", serde_json::json!("ticks")),
    ] {
        let other = if field == "driver_resource" {
            "driver_path"
        } else {
            "driver_resource"
        };
        let errs = lint_errors(base(field, serde_json::json!("MatchClock")));
        let _ = other;
        assert!(
            errs.iter()
                .any(|e| e.contains("both 'driver_resource' and 'driver_path'")),
            "driver resource missing {other}: {errs:?}"
        );
    }
    // Missing rate.
    let mut json = base("driver_resource", serde_json::json!("MatchClock"));
    json["invariants"][0]["driver_path"] = serde_json::json!("ticks");
    let errs = lint_errors(json);
    assert!(
        errs.iter()
            .any(|e| e.contains("requires 'driver_units_per_second'")
                && e.contains("sampling cadence")),
        "missing rate: {errs:?}"
    );
    // Bad rates. (NaN and infinities are unrepresentable in JSON,
    // so they cannot reach validation from a scenario document;
    // the finite->0 guard covers the reachable mistakes.)
    for rate in [0.0, -1.0] {
        let mut json = base("driver_resource", serde_json::json!("MatchClock"));
        json["invariants"][0]["driver_path"] = serde_json::json!("ticks");
        json["invariants"][0]["driver_units_per_second"] = serde_json::json!(rate);
        let errs = lint_errors(json);
        assert!(
            errs.iter()
                .any(|e| e.contains("driver_units_per_second must be a finite number > 0")),
            "rate {rate}: {errs:?}"
        );
    }
}

#[test]
fn entity_stagnation_window_and_sampling_floor() {
    // window_s is required and the honesty floor binds:
    // window_s >= 2 x sampling_s — an episode needs two
    // consecutive equal-digest samples to accrue any span.
    let errs = lint_errors(stag_json(
        Some("Guest"),
        serde_json::json!("state.Queued.waited"),
        serde_json::json!(null),
    ));
    assert!(
        errs.iter().any(|e| e.contains("requires 'window_s'")),
        "{errs:?}"
    );

    for (window_s, sampling_s) in [(1.5, 1.0), (3.0, 2.0), (0.0, 0.0)] {
        let mut json = stag_json(
            Some("Guest"),
            serde_json::json!("state.Queued.waited"),
            serde_json::json!(window_s),
        );
        json["invariants"][0]["sampling_s"] = serde_json::json!(sampling_s);
        let errs = lint_errors(json);
        // window_s = 0 trips its own positivity guard before the
        // floor — any of the three rejections satisfies the test.
        assert!(
            errs.iter().any(|e| e.contains("at least twice sampling_s")
                || e.contains("sampling_s must be a finite number > 0")
                || e.contains("window_s must be a finite number > 0")),
            "window {window_s} / sampling {sampling_s}: {errs:?}"
        );
    }
    // Exactly at the floor is legal.
    let mut json = stag_json(
        Some("Guest"),
        serde_json::json!("state.Queued.waited"),
        serde_json::json!(2.0),
    );
    json["invariants"][0]["sampling_s"] = serde_json::json!(1.0);
    let errs = lint_errors(json);
    assert!(errs.is_empty(), "window == 2x sampling is legal: {errs:?}");
}

#[test]
fn entity_stagnation_sub_frame_sampling_rejected() {
    // A cadence finer than one frame collapses to repeated
    // identical samples — the same doctrine as
    // liveness.progress's sample/window rejection. At tps 60 a
    // 0.005s cadence is sub-frame.
    let mut json = stag_json(
        Some("Guest"),
        serde_json::json!("state.Queued.waited"),
        serde_json::json!(0.02),
    );
    json["tps"] = serde_json::json!(60);
    json["invariants"][0]["sampling_s"] = serde_json::json!(0.005);
    let errs = lint_errors(json);
    assert!(
        errs.iter().any(|e| e.contains("sub-frame sampling")),
        "sub-frame cadence: {errs:?}"
    );
}

#[test]
fn entity_stagnation_soak_exceeding_duration_rejected() {
    // A soak the run can never elapse is unjudgeable.
    let errs = lint_errors(stag_json(
        Some("Guest"),
        serde_json::json!("state.Queued.waited"),
        serde_json::json!(3600.0),
    ));
    assert!(
        errs.iter().any(|e| e.contains("exceeds the run duration")),
        "window longer than duration: {errs:?}"
    );
}

#[test]
fn entity_stagnation_rejects_foreign_knobs() {
    // The reserved-field doctrine: check/value/path/resource/
    // min_distance/bounds are other rules' grammar. Values are
    // typed per field so the document PARSES and the rejection
    // comes from validation, not serde.
    for (field, value) in [
        ("check", serde_json::json!("le")),
        ("value", serde_json::json!(1.0)),
        ("path", serde_json::json!("translation.x")),
        ("resource", serde_json::json!("MatchClock")),
        ("min_distance", serde_json::json!(0.5)),
        ("min_x", serde_json::json!(1.0)),
        ("min_y", serde_json::json!(1.0)),
        ("min_z", serde_json::json!(1.0)),
        ("max_x", serde_json::json!(1.0)),
        ("max_y", serde_json::json!(1.0)),
        ("max_z", serde_json::json!(1.0)),
    ] {
        let mut json = stag_json(
            Some("Guest"),
            serde_json::json!("state.Queued.waited"),
            serde_json::json!(2.0),
        );
        json["invariants"][0][field] = value;
        let errs = lint_errors(json);
        assert!(
            errs.iter()
                .any(|e| e.contains("does not use") && e.contains("entity_stagnation")),
            "field {field}: {errs:?}"
        );
    }
}

#[test]
fn other_rules_reject_entity_stagnation_knobs() {
    // The reverse direction: watch_path/leaf_where/driver/
    // sampling_s on any other rule never fire — reject loudly
    // with the entity_stagnation-only message. Values are typed
    // per field so the document parses and the rejection comes
    // from validation.
    for (field, value) in [
        ("watch_path", serde_json::json!("state.Queued.waited")),
        ("leaf_where", serde_json::json!("state == Queued")),
        ("driver", serde_json::json!("resource")),
        ("sampling_s", serde_json::json!(1.0)),
    ] {
        let mut json = minimal_scenario("chaos");
        json["invariants"] = serde_json::json!([
            { "name": "hp_cap", "rule": "component_check",
              "target_selector": {"with_component": {"name": "Hp"}},
              "path": "current", "check": "le", "value": 100 }
        ]);
        json["invariants"][0][field] = value;
        let errs = lint_errors(json);
        assert!(
            errs.iter()
                .any(|e| e.contains(&format!("'{field}'")) && e.contains("entity_stagnation only")),
            "field {field} on component_check: {errs:?}"
        );
    }
}
