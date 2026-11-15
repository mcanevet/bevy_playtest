//! Behavioral battery for the `component_check` rule: an authored-
//! intent oracle comparing a numeric field on a reflected component
//! (selected by registered type name) against a threshold.
//!
//! The central scenario mirrors the doom-clone movement bug: a
//! "forward" input step that flips a sign every frame leaves the
//! entity oscillating near its spawn point forever — generic
//! liveness/bounds oracles stay green because the world keeps
//! changing and stays in bounds. Only authored intent ("holding W
//! must drive the camera forward within 3 seconds") can catch it.

// Shared test harness (run-slot serialization)
#[path = "common/mod.rs"]
mod common;

use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_playtest::driver::run_scenario;
use bevy_playtest::rules::Rule;
use bevy_reflect::Reflect;
use bevy_transform::prelude::Transform;

/// A marker component used as the selector. Reflect is required for
/// the harness's reflection-based evaluation path (this mirrors how
/// a game's own components, and engine components like `Camera3d`,
/// are reachable with zero harness-specific code in the game).
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct Marker;

/// Mirror of the doom bug: forward/back alternate every step, so z
/// zig-zags within a narrow band around its spawn area instead of
/// accumulating forward motion (like the camera whose z is rebuilt
/// from eye-height ± one step each frame).
fn oscillate_z(mut q: Query<&mut Transform, With<Marker>>) {
    for mut t in q.iter_mut() {
        let step = if t.translation.z >= 0.4 { -0.05 } else { 0.05 };
        t.translation.z += step;
    }
}

/// Healthy movement: z drifts forward monotonically.
fn advance_z(mut q: Query<&mut Transform, With<Marker>>) {
    for mut t in q.iter_mut() {
        t.translation.z += 0.05;
    }
}

fn scenario_with_invariant(inv: &str) -> bevy_playtest::scenario::Scenario {
    let json = format!(
        r#"{{"bot":{{"type":"chaos","seed":42}},"duration_s":3.0,"tps":60,
            "invariants":[{{{inv}}}]}}"#
    );
    serde_json::from_str(&json).unwrap()
}

/// Registers the marker type + spawns ONE entity, then adds a movement
/// system. Mirrors driver_smoke's `spawn_observable_entity` doctrine.
fn app_with_marker(movement: fn(Query<&mut Transform, With<Marker>>)) -> App {
    let mut app = App::new();
    app.register_type::<Marker>();
    app.register_type::<Transform>();
    app.add_systems(Update, movement);
    app.world_mut()
        .spawn((Marker, Transform::from_xyz(0.0, 1.0, 0.0)));
    app
}

const OSCILLATE_INV: &str = r#"
    "name": "forward_progress",
    "rule": "component_check",
    "target_selector": {"with_component": {"name": "Marker"}},
    "path": "translation.z",
    "check": "gt",
    "value": 1.0,
    "eventually_s": 2.0"#;

/// THE doom-bug shape: forward input that never accumulates. The
/// generic oracles (frozen_world, nodes_in_bounds, NaN) stay quiet —
/// the world keeps changing and stays in bounds. Only the authored
/// component_check catches it.
#[test]
fn oscillating_entity_fails_eventually_check() {
    let _slot = common::run_slot();
    let mut app = app_with_marker(oscillate_z);
    let report = run_scenario(&mut app, &scenario_with_invariant(OSCILLATE_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.rule == "component_check"),
        "expected a component_check violation among {:?}",
        report
            .violations
            .iter()
            .map(|v| v.rule.clone())
            .collect::<Vec<_>>()
    );
}

/// Control: monotonic forward movement satisfies the same intent.
#[test]
fn advancing_entity_passes_eventually_check() {
    let _slot = common::run_slot();
    let mut app = app_with_marker(advance_z);
    let report = run_scenario(&mut app, &scenario_with_invariant(OSCILLATE_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
}

/// Always-mode: a value that crosses the threshold must be reported
/// with the violating value and frame.
#[test]
fn always_mode_reports_first_crossing_with_value() {
    let _slot = common::run_slot();
    let mut app = app_with_marker(advance_z);
    let report = run_scenario(
        &mut app,
        &scenario_with_invariant(
            r#"
    "name": "stay_behind_line",
    "rule": "component_check",
    "target_selector": {"with_component": {"name": "Marker"}},
    "path": "translation.z",
    "check": "lt",
    "value": 0.5"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    let entry = report
        .violations
        .iter()
        .find(|v| v.rule == "component_check")
        .expect("component_check violation");
    // Harness convention: the invariant's name is the violation
    // TARGET (same as nodes_in_bounds); the detail names the entity
    // and the offending value.
    assert_eq!(entry.target, "stay_behind_line");
    assert!(entry.detail.contains("translation.z"), "{}", entry.detail);
    assert!(entry.first_frame > 0, "crossing cannot happen at frame 0");
}

/// The rule must be gating (component_check failures demote Pass → Fail).
#[test]
fn component_check_rule_is_gating() {
    let rule = Rule::from_name("component_check").expect("component_check registered");
    assert!(!rule.is_informational());
}

/// Selector resolving to zero entities the whole run → vacuous_invariant.
/// The component TYPE is registered (so resolution succeeds) but no
/// entity ever carries it: the check never had anything to check.
#[test]
fn selector_matching_nothing_is_vacuous() {
    let _slot = common::run_slot();
    // Register the type but spawn NOTHING with it. A Transform-only
    // world keeps frozen_world and the run-level vacuity guard quiet
    // so the ONLY violation source is the per-invariant vacuity.
    let mut app = App::new();
    app.register_type::<Marker>();
    app.register_type::<Transform>();
    app.add_systems(bevy_app::Update, |mut q: Query<&mut Transform>| {
        for mut t in q.iter_mut() {
            t.translation.x += 0.01;
        }
    });
    app.world_mut().spawn(Transform::default());
    let report = run_scenario(
        &mut app,
        &scenario_with_invariant(
            r#"
    "name": "ghost_watch",
    "rule": "component_check",
    "target_selector": {"with_component": {"name": "Marker"}},
    "path": "translation.z",
    "check": "gt",
    "value": 0.0"#,
        ),
    )
    .unwrap();
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.rule == "vacuous_invariant"),
        "expected vacuous_invariant among {:?}",
        report
            .violations
            .iter()
            .map(|v| v.rule.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
}

/// An unregistered component name surfaces as oracle_error (the
/// author's selector cannot be resolved — infrastructure/config,
/// not a game bug): the run fails but the rule names the config
/// problem, and the invariant name tells the author what to fix.
#[test]
fn unknown_component_name_reports_oracle_error() {
    let _slot = common::run_slot();
    let mut app = app_with_marker(advance_z);
    let report = run_scenario(
        &mut app,
        &scenario_with_invariant(
            r#"
    "name": "typo_watch",
    "rule": "component_check",
    "target_selector": {"with_component": {"name": "Markeh"}},
    "path": "translation.z",
    "check": "gt",
    "value": 0.0"#,
        ),
    )
    .unwrap();
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.rule == "oracle_error" && v.target == "typo_watch"),
        "expected oracle_error targeting typo_watch among {:?}",
        report
            .violations
            .iter()
            .map(|v| format!("{}/{}", v.rule, v.target))
            .collect::<Vec<_>>()
    );
}

/// Engine components (Transform) are selectable by short name with
/// zero game-side harness code — the demo path used for doom's
/// Camera3d player-camera selection.
#[test]
fn engine_component_short_name_selects_entities() {
    let _slot = common::run_slot();
    let mut app = app_with_marker(advance_z);
    let report = run_scenario(
        &mut app,
        &scenario_with_invariant(
            r#"
    "name": "y_is_eye_height",
    "rule": "component_check",
    "target_selector": {"with_component": {"name": "Transform"}},
    "path": "translation.y",
    "check": "equals",
    "value": 1.0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
}

/// always-mode: short name resolution also works for the FULL path
/// variant (both directions covered by other tests; this one locks
/// that a healthy in-bounds value passes an always-check with no
/// eventually deadline).
#[test]
fn healthy_value_passes_always_check() {
    let _slot = common::run_slot();
    let mut app = app_with_marker(advance_z);
    let report = run_scenario(
        &mut app,
        &scenario_with_invariant(
            r#"
    "name": "stays_positive",
    "rule": "component_check",
    "target_selector": {"with_component": {"name": "Marker"}},
    "path": "translation.z",
    "check": "ge",
    "value": 0.0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
}

/// Time windows remain unimplemented regardless of rule.
#[test]
fn time_windows_still_rejected() {
    let inv = r#"
    "name": "windowed",
    "rule": "component_check",
    "target_selector": {"with_component": {"name": "Marker"}},
    "path": "translation.z",
    "check": "gt",
    "value": 1.0,
    "after_s": 1.0"#;
    let scenario = scenario_with_invariant(inv);
    let err = bevy_playtest::scenario::validate_scenario(&scenario).unwrap_err();
    assert!(matches!(
        err,
        bevy_playtest::scenario::ScenarioError::NotImplemented(_)
    ));
}

/// nodes_in_bounds keeps rejecting value/path/check — reserved-field
/// honesty guard for the OLD rule too.
#[test]
fn nodes_in_bounds_rejects_component_fields() {
    let scenario = scenario_with_invariant(
        r#"
    "name": "bounds",
    "rule": "nodes_in_bounds",
    "value": 5,
    "path": "translation.z",
    "check": "gt""#,
    );
    let err = bevy_playtest::scenario::validate_scenario(&scenario).unwrap_err();
    assert!(err.to_string().contains("nodes_in_bounds"));
}

/// component_check without a with_component selector is rejected.
#[test]
fn component_check_requires_selector() {
    let scenario = scenario_with_invariant(
        r#"
    "name": "unselected",
    "rule": "component_check",
    "path": "translation.z",
    "check": "gt",
    "value": 1.0"#,
    );
    let err = bevy_playtest::scenario::validate_scenario(&scenario).unwrap_err();
    assert!(err.to_string().contains("with_component"));
}

// ---------------------------------------------------------------------------
// Deadline-enforcement semantics (the adversarial-review D1 fix): an
// eventually_s predicate that first holds AFTER the deadline must
// FAIL — "eventually" means "within the deadline", not "sometime
// before the run ends".
// ---------------------------------------------------------------------------

/// A marker that sits below the threshold for the first second, then
/// crosses it — the predicate first holds LATE (after the 1s
/// deadline). The run must fail.
fn delayed_cross(
    mut q: Query<&mut Transform, With<Marker>>,
    mut frame: bevy_ecs::prelude::Local<u64>,
) {
    let f = *frame;
    *frame += 1;
    for mut t in q.iter_mut() {
        // Cross "z > 1.0" only from frame 90 onward (t = 1.5s).
        if f >= 90 {
            t.translation.z = 2.0;
        }
    }
}

#[test]
fn late_satisfaction_misses_deadline() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.register_type::<Marker>();
    app.register_type::<Transform>();
    app.add_systems(Update, delayed_cross);
    app.world_mut()
        .spawn((Marker, Transform::from_xyz(0.0, 1.0, 0.0)));
    let report = run_scenario(
        &mut app,
        &scenario_with_invariant(
            r#"
    "name": "fast_response",
    "rule": "component_check",
    "target_selector": {"with_component": {"name": "Marker"}},
    "path": "translation.z",
    "check": "gt",
    "value": 1.0,
    "eventually_s": 1.0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    let entry = report
        .violations
        .iter()
        .find(|v| v.rule == "component_check" && v.target == "fast_response")
        .expect("component_check violation");
    assert!(
        entry.detail.contains("first held only at frame"),
        "detail should say the predicate held late: {}",
        entry.detail
    );
}

/// Crosses "z > 1.0" exactly AT the 60-frame deadline of a 1.0s
/// eventually_s at tps 60 (ceil(1.0 * 60) = 60): the predicate first
/// holds at recorded frame 60 == deadline → inclusive boundary,
/// must PASS. `f` (the Local counter) equals the recorder's frame
/// index during that update.
fn cross_at_deadline(
    mut q: Query<&mut Transform, With<Marker>>,
    mut frame: bevy_ecs::prelude::Local<u64>,
) {
    let f = *frame;
    *frame += 1;
    if f >= 60 {
        for mut t in q.iter_mut() {
            t.translation.z = 2.0;
        }
    }
}

/// Same as `cross_at_deadline` but one frame later: first holds at
/// frame 61, one frame PAST the deadline → must FAIL.
fn cross_after_deadline(
    mut q: Query<&mut Transform, With<Marker>>,
    mut frame: bevy_ecs::prelude::Local<u64>,
) {
    let f = *frame;
    *frame += 1;
    if f >= 61 {
        for mut t in q.iter_mut() {
            t.translation.z = 2.0;
        }
    }
}

/// The deadline comparison is INCLUSIVE (`first_held_frame <=
/// deadline_frames`): holding at exactly the deadline frame counts.
/// Together with `crossing_one_frame_past_deadline_fails` this pins
/// both the boundary convention and the `ceil` rounding.
#[test]
fn satisfaction_at_deadline_boundary_counts() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.register_type::<Marker>();
    app.register_type::<Transform>();
    app.add_systems(Update, cross_at_deadline);
    app.world_mut()
        .spawn((Marker, Transform::from_xyz(0.0, 1.0, 0.0)));
    let report = run_scenario(
        &mut app,
        &scenario_with_invariant(
            r#"
    "name": "boundary_response",
    "rule": "component_check",
    "target_selector": {"with_component": {"name": "Marker"}},
    "path": "translation.z",
    "check": "gt",
    "value": 1.0,
    "eventually_s": 1.0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
}

/// The flip side of the boundary test: first holding ONE frame past
/// the deadline fails, and the violation says exactly that.
#[test]
fn crossing_one_frame_past_deadline_fails() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.register_type::<Marker>();
    app.register_type::<Transform>();
    app.add_systems(Update, cross_after_deadline);
    app.world_mut()
        .spawn((Marker, Transform::from_xyz(0.0, 1.0, 0.0)));
    let report = run_scenario(
        &mut app,
        &scenario_with_invariant(
            r#"
    "name": "boundary_response",
    "rule": "component_check",
    "target_selector": {"with_component": {"name": "Marker"}},
    "path": "translation.z",
    "check": "gt",
    "value": 1.0,
    "eventually_s": 1.0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    let entry = report
        .violations
        .iter()
        .find(|v| v.rule == "component_check" && v.target == "boundary_response")
        .expect("component_check violation");
    assert!(
        entry.detail.contains("first held only at frame 61"),
        "detail should name frame 61: {}",
        entry.detail
    );
}

// ---------------------------------------------------------------------------
// Multi-entity semantics: "eventually" requires ALL matched entities
// to satisfy the predicate SIMULTANEOUSLY, and always-mode names one
// violating entity when only part of the population is bad.
// ---------------------------------------------------------------------------

#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct Other;

/// Two entities take turns crossing the threshold, never together:
/// A is high on even frames, B on odd frames. Individually each
/// satisfies "z > 1.0" infinitely often; never simultaneously.
fn alternating_pair(mut q: Query<&mut Transform>, mut frame: bevy_ecs::prelude::Local<u64>) {
    let f = *frame;
    *frame += 1;
    for (i, mut t) in q.iter_mut().enumerate() {
        t.translation.z = if (f % 2) == (i as u64 % 2) { 2.0 } else { 0.0 };
    }
}

/// Selector scoping control: only entities carrying the SELECTOR
/// component are checked. A second, differently-tagged entity that
/// oscillates independently is invisible to a Marker-scoped
/// invariant; the single selected entity holding in-deadline is a
/// pass even though the OTHER entity never holds.
#[test]
fn selector_scopes_to_the_named_component_only() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.register_type::<Marker>();
    app.register_type::<Other>();
    app.register_type::<Transform>();
    app.add_systems(Update, alternating_pair);
    app.world_mut()
        .spawn((Marker, Transform::from_xyz(0.0, 1.0, 0.0)));
    app.world_mut()
        .spawn((Other, Transform::from_xyz(0.0, 1.0, 0.0)));
    let report = run_scenario(
        &mut app,
        &scenario_with_invariant(
            r#"
    "name": "marker_only",
    "rule": "component_check",
    "target_selector": {"with_component": {"name": "Marker"}},
    "path": "translation.z",
    "check": "gt",
    "value": 1.0,
    "eventually_s": 2.0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
}

/// The real disjoint-satisfaction test: both entities carry the
/// selector; each crosses alone, never together. Must FAIL.
#[test]
fn two_entities_disjoint_satisfaction_fails() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.register_type::<Marker>();
    app.register_type::<Transform>();
    app.add_systems(Update, alternating_pair);
    app.world_mut()
        .spawn((Marker, Transform::from_xyz(0.0, 1.0, 0.0)));
    app.world_mut()
        .spawn((Marker, Transform::from_xyz(0.0, 1.0, 0.0)));
    let report = run_scenario(
        &mut app,
        &scenario_with_invariant(
            r#"
    "name": "all_together",
    "rule": "component_check",
    "target_selector": {"with_component": {"name": "Marker"}},
    "path": "translation.z",
    "check": "gt",
    "value": 1.0,
    "eventually_s": 2.0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.rule == "component_check" && v.target == "all_together"),
        "expected component_check/all_together among {:?}",
        report
            .violations
            .iter()
            .map(|v| format!("{}/{}", v.rule, v.target))
            .collect::<Vec<_>>()
    );
}

/// Tag excluding an entity from the movement system (see
/// `late_spawn_never_satisfying_does_not_flip_verdict`).
#[derive(Component)]
struct Frozen;

fn advance_moving_markers(mut q: Query<&mut Transform, (With<Marker>, Without<Frozen>)>) {
    for mut t in q.iter_mut() {
        t.translation.z += 0.05;
    }
}

/// Subset-snapshot semantics (documented design choice, pinned
/// here): "all matched entities simultaneously" is judged per
/// frame against the CURRENTLY matched set. Entity A holds from
/// frame ~20; entity B (same selector component) spawns frozen at
/// z = 0 at frame 100 and NEVER satisfies. Satisfaction was
/// already recorded at A's solo holding frame — the late,
/// never-satisfying entity does not retroactively invalidate it.
/// A stricter "every entity that EVER matched" semantics would
/// fail this scenario; that is deliberately not implemented.
#[test]
fn late_spawn_never_satisfying_does_not_flip_verdict() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.register_type::<Marker>();
    app.register_type::<Transform>();
    app.add_systems(Update, advance_moving_markers);
    app.add_systems(
        Update,
        (|mut commands: bevy_ecs::prelude::Commands, mut frame: bevy_ecs::prelude::Local<u64>| {
            let f = *frame;
            *frame += 1;
            if f == 100 {
                // Late, never-satisfying selector entity.
                commands.spawn((Marker, Frozen, Transform::from_xyz(0.0, 1.0, 0.0)));
            }
        })
        .after(advance_moving_markers),
    );
    app.world_mut()
        .spawn((Marker, Transform::from_xyz(0.0, 1.0, 0.0)));
    let report = run_scenario(
        &mut app,
        &scenario_with_invariant(
            r#"
    "name": "solo_then_latecomer",
    "rule": "component_check",
    "target_selector": {"with_component": {"name": "Marker"}},
    "path": "translation.z",
    "check": "gt",
    "value": 1.0,
    "eventually_s": 2.0"#,
        ),
    )
    .unwrap();
    assert_eq!(
        report.status,
        bevy_playtest::enums::PlaytestStatus::Pass,
        "subset-snapshot semantics: solo satisfaction recorded at ~frame 20 \
         stands; violations were {:?}",
        report
            .violations
            .iter()
            .map(|v| format!("{}/{}", v.rule, v.target))
            .collect::<Vec<_>>()
    );
}

/// Multi-entity always-mode: one healthy + one violating entity. The
/// report must FAIL and the detail must name the bad entity, not the
/// healthy one.
#[test]
fn always_mode_multi_entity_names_violator() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.register_type::<Marker>();
    app.register_type::<Transform>();
    // Entity A: healthy (y = 1.0 always). Entity B: violating (y = 5.0).
    app.world_mut()
        .spawn((Marker, Transform::from_xyz(0.0, 1.0, 0.0)));
    app.world_mut()
        .spawn((Marker, Transform::from_xyz(0.0, 5.0, 0.0)));
    let report = run_scenario(
        &mut app,
        &scenario_with_invariant(
            r#"
    "name": "eye_height",
    "rule": "component_check",
    "target_selector": {"with_component": {"name": "Marker"}},
    "path": "translation.y",
    "check": "equals",
    "value": 1.0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    let entry = report
        .violations
        .iter()
        .find(|v| v.rule == "component_check")
        .expect("component_check violation");
    assert!(entry.detail.contains("5.000"), "{}", entry.detail);
}

// ---------------------------------------------------------------------------
// Resolution failures: ambiguity and non-numeric leaves are config
// errors (oracle_error), and they DO demote the run to Fail.
// ---------------------------------------------------------------------------

/// Two registered types share the short name `Marker` (different
/// modules) — the short name is ambiguous and must reject loudly.
mod amb_left {
    use bevy_ecs::prelude::*;
    use bevy_reflect::Reflect;
    #[derive(Component, Reflect, Default)]
    #[reflect(Component)]
    pub struct Token;
}
mod amb_right {
    use bevy_ecs::prelude::*;
    use bevy_reflect::Reflect;
    #[derive(Component, Reflect, Default)]
    #[reflect(Component)]
    pub struct Token;
}

#[test]
fn ambiguous_short_name_reports_oracle_error() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.register_type::<amb_left::Token>();
    app.register_type::<amb_right::Token>();
    app.register_type::<Transform>();
    app.world_mut().spawn(Transform::default());
    let report = run_scenario(
        &mut app,
        &scenario_with_invariant(
            r#"
    "name": "ambiguous_watch",
    "rule": "component_check",
    "target_selector": {"with_component": {"name": "Token"}},
    "path": "translation.z",
    "check": "gt",
    "value": 0.0"#,
        ),
    )
    .unwrap();
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.rule == "oracle_error" && v.target == "ambiguous_watch"),
        "expected oracle_error/ambiguous_watch among {:?}",
        report
            .violations
            .iter()
            .map(|v| format!("{}/{}", v.rule, v.target))
            .collect::<Vec<_>>()
    );
    // Config errors demote the run too: the author must fix the
    // scenario before any verdict is meaningful.
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    // And the audit must NOT pile a contradictory vacuous_invariant
    // or deadline entry on top of the config error (the config
    // error already failed the run; "spawn something to check"
    // would misdirect an author whose real problem is ambiguity).
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.rule != "oracle_error" && v.target == "ambiguous_watch"),
        "config-errored invariant should produce no other entries, got {:?}",
        report
            .violations
            .iter()
            .map(|v| format!("{}/{}", v.rule, v.target))
            .collect::<Vec<_>>()
    );
}

/// A path landing on a Vec3 leaf (non-numeric) is a config error,
/// not a silent coercion.
#[test]
fn non_numeric_leaf_reports_oracle_error() {
    let _slot = common::run_slot();
    let mut app = app_with_marker(advance_z);
    let report = run_scenario(
        &mut app,
        &scenario_with_invariant(
            r#"
    "name": "whole_vec",
    "rule": "component_check",
    "target_selector": {"with_component": {"name": "Marker"}},
    "path": "translation",
    "check": "gt",
    "value": 0.0"#,
        ),
    )
    .unwrap();
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.rule == "oracle_error" && v.target == "whole_vec"),
        "expected oracle_error/whole_vec among {:?}",
        report
            .violations
            .iter()
            .map(|v| format!("{}/{}", v.rule, v.target))
            .collect::<Vec<_>>()
    );
    assert!(report
        .violations
        .iter()
        .any(|v| v.detail.contains("non-numeric")));
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
}

// ---------------------------------------------------------------------------
// New validation rejections (D2/D3/D5): deadline sanity and
// reserved-field honesty.
// ---------------------------------------------------------------------------

#[test]
fn eventually_past_run_duration_is_rejected() {
    let inv = r#"
    "name": "slow_load",
    "rule": "component_check",
    "target_selector": {"with_component": {"name": "Marker"}},
    "path": "translation.z",
    "check": "gt",
    "value": 1.0,
    "eventually_s": 5.0"#;
    let scenario = scenario_with_invariant(inv);
    let err = bevy_playtest::scenario::validate_scenario(&scenario).unwrap_err();
    assert!(err.to_string().contains("exceeds the run duration"));
}

#[test]
fn non_positive_eventually_is_rejected() {
    let inv = r#"
    "name": "instant",
    "rule": "component_check",
    "target_selector": {"with_component": {"name": "Marker"}},
    "path": "translation.z",
    "check": "gt",
    "value": 1.0,
    "eventually_s": 0.0"#;
    let scenario = scenario_with_invariant(inv);
    let err = bevy_playtest::scenario::validate_scenario(&scenario).unwrap_err();
    assert!(err.to_string().contains("eventually_s must be"));
}

#[test]
fn component_check_rejects_bounds_fields() {
    let scenario = scenario_with_invariant(
        r#"
    "name": "mixed_fields",
    "rule": "component_check",
    "target_selector": {"with_component": {"name": "Marker"}},
    "path": "translation.z",
    "check": "gt",
    "value": 1.0,
    "max_z": 10.0"#,
    );
    let err = bevy_playtest::scenario::validate_scenario(&scenario).unwrap_err();
    assert!(err.to_string().contains("min_x/max_x"));
}

#[test]
fn nodes_in_bounds_with_component_selector_is_rejected() {
    let scenario = scenario_with_invariant(
        r#"
    "name": "selected_bounds",
    "rule": "nodes_in_bounds",
    "target_selector": {"with_component": {"name": "Marker"}},
    "min_x": -10.0, "max_x": 10.0"#,
    );
    let err = bevy_playtest::scenario::validate_scenario(&scenario).unwrap_err();
    assert!(err.to_string().contains("with_component"));
}

/// Integer leaves are compared losslessly (2^53 range).
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct Counter {
    hits: i32,
}

#[test]
fn integer_leaf_is_compared() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.register_type::<Counter>();
    app.register_type::<Transform>();
    app.add_systems(Update, |mut q: Query<(&mut Counter, &mut Transform)>| {
        for (mut c, mut t) in q.iter_mut() {
            c.hits += 1;
            t.translation.x += 0.01; // keep frozen_world quiet
        }
    });
    app.world_mut()
        .spawn((Counter::default(), Transform::default()));
    let report = run_scenario(
        &mut app,
        &scenario_with_invariant(
            r#"
    "name": "hits_accumulate",
    "rule": "component_check",
    "target_selector": {"with_component": {"name": "Counter"}},
    "path": "hits",
    "check": "gt",
    "value": 10.0,
    "eventually_s": 1.0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
}

// ---------------------------------------------------------------------------
// Round-3 follow-ups: the witness fold-in (W1) and the no-ATR arm (W2)
// of the config-error path need behavioral coverage.
// ---------------------------------------------------------------------------

/// Two entities carry the selector: A has a Transform but VIOLATES
/// the predicate (y = 5.0 vs equals 1.0); B has NO Transform at all,
/// so the path fails on both roots. The config error must be reported
/// AND the already-collected witness for A must be folded into the
/// message ("among them, entity ... violates ..."), proving the
/// witness isn't discarded.
#[test]
fn config_error_folds_collected_witness_into_message() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.register_type::<Marker>();
    app.register_type::<Transform>();
    app.world_mut()
        .spawn((Marker, Transform::from_xyz(0.0, 5.0, 0.0)));
    // Selector component present, but NO Transform: path
    // 'translation.z' fails on both the selector root and the
    // fallback.
    app.world_mut().spawn(Marker);
    let report = run_scenario(
        &mut app,
        &scenario_with_invariant(
            r#"
    "name": "witness_fold",
    "rule": "component_check",
    "target_selector": {"with_component": {"name": "Marker"}},
    "path": "translation.z",
    "check": "gt",
    "value": 100.0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    let entry = report
        .violations
        .iter()
        .find(|v| v.rule == "oracle_error" && v.target == "witness_fold")
        .expect("oracle_error/witness_fold");
    assert!(
        entry.detail.contains("among them"),
        "witness should be folded into the config error: {}",
        entry.detail
    );
    assert!(
        entry.detail.contains("violates > 100.000"),
        "witness value/threshold should appear: {}",
        entry.detail
    );
    // And per the N-B suppression, no contradictory second entry.
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.rule != "oracle_error" && v.target == "witness_fold"),
        "config-errored invariant should produce no other entries: {:?}",
        report
            .violations
            .iter()
            .map(|v| format!("{}/{}", v.rule, v.target))
            .collect::<Vec<_>>()
    );
}

/// An app with NO AppTypeRegistry cannot evaluate reflected checks:
/// every component_check becomes an oracle_error (and, per N-B
/// suppression, NOTHING else — no vacuous entry). The registry is
/// removed from a normal app rather than using App::empty(), whose
/// bare world can't even host plugins that register types.
#[test]
fn missing_type_registry_reports_oracle_error_only() {
    let _slot = common::run_slot();
    let mut app = App::new();
    // Strip the reflected-type registry: reflected checks are now
    // impossible. This is the exact condition the arm guards.
    assert!(
        app.world_mut()
            .remove_resource::<bevy_ecs::reflect::AppTypeRegistry>()
            .is_some(),
        "fixture precondition: App::new() normally has an AppTypeRegistry"
    );
    app.world_mut().spawn(Transform::from_xyz(0.0, 1.0, 0.0));
    let report = run_scenario(
        &mut app,
        &scenario_with_invariant(
            r#"
    "name": "registryless",
    "rule": "component_check",
    "target_selector": {"with_component": {"name": "Marker"}},
    "path": "translation.z",
    "check": "gt",
    "value": 0.0"#,
        ),
    )
    .unwrap();
    let targets: Vec<String> = report
        .violations
        .iter()
        .map(|v| format!("{}/{}", v.rule, v.target))
        .collect();
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.rule == "oracle_error" && v.target == "registryless"),
        "expected oracle_error/registryless among {targets:?}"
    );
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.rule != "oracle_error" && v.target == "registryless"),
        "no-ATR invariant should produce no other entries: {targets:?}"
    );
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
}
