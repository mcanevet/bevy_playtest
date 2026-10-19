//! End-to-end smoke: run_scenario against a minimal headless app.

use bevy_app::App;
use bevy_playtest::run_scenario;
use bevy_playtest::scenario::Scenario;
use bevy_transform::prelude::Transform;

fn minimal_app() -> App {
    bevy_playtest::sinks::ensure_global_subscriber();
    App::new()
}

/// A moving Gameplay entity: keeps the run observable for the oracles
/// (vacuity guard, frozen-world) without asserting anything about it.
fn spawn_observable_entity(app: &mut App) {
    use bevy_playtest::contract::Gameplay;
    app.add_systems(
        bevy_app::Update,
        |mut q: bevy_ecs::prelude::Query<
            &mut bevy_transform::prelude::Transform,
            bevy_ecs::prelude::With<Gameplay>,
        >| {
            for (i, mut t) in q.iter_mut().enumerate() {
                t.translation.x = (i as f32) + 1.0;
            }
        },
    );
    app.world_mut().spawn((
        bevy_playtest::contract::Gameplay,
        bevy_transform::prelude::Transform::from_xyz(0.0, 0.0, 0.0),
    ));
}

#[test]
fn chaos_run_passes_on_minimal_app() {
    let scenario: Scenario =
        serde_json::from_str(r#"{"bot":{"type":"chaos","seed":7},"duration_s":0.1,"tps":60}"#)
            .unwrap();
    let mut app = minimal_app();
    spawn_observable_entity(&mut app);
    let report = run_scenario(&mut app, &scenario).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    assert_eq!(report.frame_count, 6);
    assert!(report.violations.is_empty());
    assert!(report.wall_time_s >= 0.0);
    assert_eq!(report.harness_mode, "in_process");
}

#[test]
fn key_presses_drive_a_real_system() {
    use bevy_ecs::prelude::*;
    // A game system that flips a resource when W is pressed.
    #[derive(Resource, Default)]
    struct ObservedPresses(u32);

    fn observe_keys(
        mut events: bevy_ecs::prelude::MessageReader<bevy_input::keyboard::KeyboardInput>,
        mut presses: ResMut<ObservedPresses>,
    ) {
        for e in events.read() {
            if e.key_code == bevy_input::keyboard::KeyCode::KeyW
                && e.state == bevy_input::ButtonState::Pressed
            {
                presses.0 += 1;
            }
        }
    }

    let scenario: Scenario = serde_json::from_str(
        r#"{"bot":{"type":"synthetic_keyboard","seed":7,"key_presses":[{"key":"w","frame":3,"hold_frames":1}]},"duration_s":0.2,"tps":60}"#,
    )
    .unwrap();
    let mut app = minimal_app();
    spawn_observable_entity(&mut app);
    app.add_message::<bevy_input::keyboard::KeyboardInput>();
    app.insert_resource(ObservedPresses::default());
    app.add_systems(bevy_app::Update, observe_keys);
    let report = run_scenario(&mut app, &scenario).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    assert_eq!(
        app.world().get_resource::<ObservedPresses>().unwrap().0,
        1,
        "the scheduled key press must reach the game's input system"
    );
}

#[test]
fn rejected_scenario_returns_error_not_report() {
    let scenario: Scenario =
        serde_json::from_str(r#"{"bot":{"type":"chaos","seed":7},"duration_s":-1.0,"tps":60}"#)
            .unwrap();
    let mut app = minimal_app();
    let err = run_scenario(&mut app, &scenario).unwrap_err();
    assert!(matches!(
        err,
        bevy_playtest::scenario::ScenarioError::Rejected(_)
    ));
}

#[test]
fn panicking_game_system_yields_crash_status_with_violation() {
    fn exploding_system() {
        panic!("grenade!");
    }

    let scenario: Scenario =
        serde_json::from_str(r#"{"bot":{"type":"chaos","seed":7},"duration_s":0.1,"tps":60}"#)
            .unwrap();
    let mut app = minimal_app();
    app.add_systems(bevy_app::Update, exploding_system);
    let report = run_scenario(&mut app, &scenario).unwrap();
    assert_eq!(
        report.status,
        bevy_playtest::enums::PlaytestStatus::Crash,
        "status: {:?}",
        report.status
    );
    let panic_violations: Vec<_> = report
        .violations
        .iter()
        .filter(|v| v.rule == "panic")
        .collect();
    assert!(!panic_violations.is_empty(), "{:?}", report.violations);
    assert!(panic_violations[0].detail.contains("grenade!"));
}

#[test]
fn chaos_bot_records_choices() {
    let scenario: Scenario =
        serde_json::from_str(r#"{"bot":{"type":"chaos","seed":42},"duration_s":0.2,"tps":60}"#)
            .unwrap();
    let mut app = minimal_app();
    let report = run_scenario(&mut app, &scenario).unwrap();
    assert!(!report.choices.is_empty(), "choices should be recorded");
    // Verify determinism: same seed -> same choices
    let mut app2 = minimal_app();
    let report2 = run_scenario(&mut app2, &scenario).unwrap();
    assert_eq!(report.choices, report2.choices);
}

#[test]
fn game_log_error_yields_log_error_violation() {
    fn noisy_system() {
        tracing::error!("score went negative: {}", -5);
    }

    let scenario: Scenario =
        serde_json::from_str(r#"{"bot":{"type":"chaos","seed":7},"duration_s":0.1,"tps":60}"#)
            .unwrap();
    let mut app = minimal_app();
    app.add_systems(bevy_app::Update, noisy_system);
    let report = run_scenario(&mut app, &scenario).unwrap();
    let log_violations: Vec<_> = report
        .violations
        .iter()
        .filter(|v| v.rule == "log_error")
        .collect();
    assert!(!log_violations.is_empty(), "{:?}", report.violations);
    assert!(log_violations[0].detail.contains("score went negative"));
    assert_eq!(
        report.status,
        bevy_playtest::enums::PlaytestStatus::Fail,
        "log_error must gate by default"
    );
}

#[test]
fn log_warn_is_informational_by_default() {
    fn nagging_system() {
        tracing::warn!("fps is low: {}", 12);
    }

    let scenario: Scenario =
        serde_json::from_str(r#"{"bot":{"type":"chaos","seed":7},"duration_s":0.1,"tps":60}"#)
            .unwrap();
    let mut app = minimal_app();
    spawn_observable_entity(&mut app);
    app.add_systems(bevy_app::Update, nagging_system);
    let report = run_scenario(&mut app, &scenario).unwrap();
    assert!(report.violations.iter().any(|v| v.rule == "log_warn"));
    assert_eq!(
        report.status,
        bevy_playtest::enums::PlaytestStatus::Pass,
        "log_warn must NOT gate unless strict_warnings"
    );
}

#[test]
fn nan_transform_yields_finite_transforms_violation() {
    use bevy_ecs::prelude::*;

    // Spawn a Gameplay entity with a NaN transform via a system.
    fn spawn_nan_entity(mut commands: Commands) {
        commands.spawn((
            bevy_playtest::contract::Gameplay,
            Transform::from_xyz(f32::NAN, 0.0, 0.0),
        ));
    }

    let scenario: Scenario =
        serde_json::from_str(r#"{"bot":{"type":"chaos","seed":7},"duration_s":0.1,"tps":60}"#)
            .unwrap();
    let mut app = minimal_app();
    app.add_systems(bevy_app::Update, spawn_nan_entity);
    let report = run_scenario(&mut app, &scenario).unwrap();
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.rule == "finite_transforms"),
        "{:?}",
        report.violations
    );
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
}

#[test]
fn out_of_bounds_entity_yields_nodes_in_bounds_violation() {
    use bevy_ecs::prelude::*;

    fn spawn_far_entity(mut commands: Commands) {
        commands.spawn((
            bevy_playtest::contract::Gameplay,
            Transform::from_xyz(5000.0, 0.0, 0.0),
        ));
    }

    let scenario: Scenario = serde_json::from_str(
        r#"{"bot":{"type":"chaos","seed":7},"duration_s":0.1,"tps":60,"invariants":[{"name":"stay_in_arena","rule":"nodes_in_bounds","min_x":-1000,"max_x":1000}]}"#,
    )
    .unwrap();
    let mut app = minimal_app();
    app.add_systems(bevy_app::Update, spawn_far_entity);
    let report = run_scenario(&mut app, &scenario).unwrap();
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.rule == "nodes_in_bounds"),
        "{:?}",
        report.violations
    );
}

#[test]
fn unknown_reset_kind_is_rejected_at_load_time() {
    let scenario: Scenario = serde_json::from_str(
        r#"{"bot":{"type":"chaos","seed":7},"duration_s":0.1,"tps":60,"setup":{"resets":[{"kind":"hard_reset"}]}}"#,
    )
    .unwrap();
    // no hooks at all: ResetMissing is the louder, more actionable error
    let mut app = minimal_app();
    let err = run_scenario(&mut app, &scenario).unwrap_err();
    assert!(matches!(
        err,
        bevy_playtest::scenario::ScenarioError::ResetMissing
    ));

    // hooks present but the kind is unknown: UnknownReset with suggestions
    let mut app = minimal_app();
    let mut hooks = bevy_playtest::contract::ResetHooks::default();
    hooks.register("soft", |_world: &mut bevy_ecs::world::World| {});
    app.world_mut().insert_resource(hooks);
    let err = run_scenario(&mut app, &scenario).unwrap_err();
    assert!(matches!(
        err,
        bevy_playtest::scenario::ScenarioError::UnknownReset { .. }
    ));
}

#[test]
fn registered_reset_hook_is_invoked_and_counts() {
    use bevy_ecs::prelude::*;

    #[derive(Resource, Default)]
    struct ResetCount(u32);

    let scenario: Scenario = serde_json::from_str(
        r#"{"bot":{"type":"chaos","seed":7},"duration_s":0.05,"tps":60,"setup":{"resets":[{"kind":"wipe"}]}}"#,
    )
    .unwrap();
    let mut app = minimal_app();
    spawn_observable_entity(&mut app);
    app.insert_resource(ResetCount(0));
    let mut hooks = bevy_playtest::contract::ResetHooks::default();
    hooks.register("wipe", |world: &mut bevy_ecs::world::World| {
        world.resource_mut::<ResetCount>().0 += 1;
    });
    app.insert_resource(hooks);
    let report = run_scenario(&mut app, &scenario).unwrap();
    assert_eq!(
        app.world().get_resource::<ResetCount>().unwrap().0,
        1,
        "reset hook must run exactly once"
    );
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
}

#[test]
fn custom_invariant_is_rejected_at_parse_time() {
    // "custom" is not a known InvariantRule variant anymore; it fails
    // serde deserialization with an unknown-variant error.
    let err = serde_json::from_str::<Scenario>(
        r#"{"bot":{"type":"chaos","seed":7},"duration_s":0.1,"tps":60,"invariants":[{"name":"score_cap","rule":"custom","path":"TestApi.score","value":100,"check":"le"}]}"#,
    ).unwrap_err();
    assert!(
        err.to_string().contains("custom"),
        "serde error must name the unknown variant 'custom': {err}"
    );
}

#[test]
fn scenario_bounds_are_enforced_per_axis() {
    use bevy_playtest::contract::Gameplay;

    fn spawn_far_entity(mut commands: bevy_ecs::prelude::Commands) {
        commands.spawn((Gameplay, Transform::from_xyz(50.0, 9000.0, 0.0)));
    }

    // Y breach only: x within [-100, 100], y limited to [-100, 100].
    let scenario: Scenario = serde_json::from_str(
        r#"{"bot":{"type":"chaos","seed":7},"duration_s":0.05,"tps":60,"invariants":[{"name":"arena","rule":"nodes_in_bounds","min_x":-100,"max_x":100,"min_y":-100,"max_y":100}]}"#,
    )
    .unwrap();
    let mut app = minimal_app();
    app.add_systems(bevy_app::Update, spawn_far_entity);
    let report = run_scenario(&mut app, &scenario).unwrap();
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.rule == "nodes_in_bounds"),
        "{:?}",
        report.violations
    );
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
}

/// An entity escaping along the Z axis must be caught like X/Y escapes.
/// The standalone bounds oracle checks all three axes (oracles.rs:34-36);
/// this test is a regression guard that the oracle remains wired.
#[test]
fn z_axis_escape_is_caught_by_nodes_in_bounds() {
    use bevy_playtest::contract::Gameplay;

    fn spawn_high_entity(mut commands: bevy_ecs::prelude::Commands) {
        commands.spawn((Gameplay, Transform::from_xyz(0.0, 0.0, 1e9)));
    }

    let scenario: Scenario = serde_json::from_str(
        r#"{"bot":{"type":"chaos","seed":7},"duration_s":0.05,"tps":60,"invariants":[{"name":"arena","rule":"nodes_in_bounds","min_x":-1000,"max_x":1000,"min_y":-1000,"max_y":1000}]}"#,
    )
    .unwrap();
    let mut app = minimal_app();
    app.add_systems(bevy_app::Update, spawn_high_entity);
    let report = run_scenario(&mut app, &scenario).unwrap();
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.rule == "nodes_in_bounds"),
        "an entity at z=1e9 must not pass a bounds invariant unflagged; got {:?}",
        report.violations
    );
}

#[test]
fn reset_missing_fires_for_reset_dependent_scenario_without_hooks() {
    let scenario: Scenario = serde_json::from_str(
        r#"{"bot":{"type":"chaos","seed":7},"duration_s":0.1,"tps":60,"setup":{"resets":[{"kind":"anything"}]}}"#,
    )
    .unwrap();
    let mut app = minimal_app();
    let err = run_scenario(&mut app, &scenario).unwrap_err();
    assert!(matches!(
        err,
        bevy_playtest::scenario::ScenarioError::ResetMissing
    ));
}

#[test]
fn report_distinguishes_reset_hook_presence() {
    use bevy_playtest::contract::ResetHooks;
    let scenario: Scenario =
        serde_json::from_str(r#"{"bot":{"type":"chaos","seed":7},"duration_s":0.1,"tps":60}"#)
            .unwrap();
    let mut app = minimal_app();
    spawn_observable_entity(&mut app);
    // scenario has no reset semantics -> field absent even without hooks
    let report = run_scenario(&mut app, &scenario).unwrap();
    assert_eq!(report.reset_hooks_registered, None);

    // now with reset semantics and hooks registered: presence is reported
    let scenario: Scenario = serde_json::from_str(
        r#"{"bot":{"type":"chaos","seed":7},"duration_s":0.1,"tps":60,"setup":{"resets":[{"kind":"soft"}]}}"#,
    )
    .unwrap();
    let mut app = minimal_app();
    spawn_observable_entity(&mut app);
    let mut hooks = ResetHooks::default();
    hooks.register("soft", |_world: &mut bevy_ecs::world::World| {});
    app.world_mut().insert_resource(hooks);
    let report = run_scenario(&mut app, &scenario).unwrap();
    assert_eq!(report.reset_hooks_registered, Some(true));
}
