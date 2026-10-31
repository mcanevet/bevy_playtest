//! End-to-end smoke: run_scenario against a minimal headless app.

// Shared test harness (run-slot serialization)
#[path = "common/mod.rs"]
mod common;

use bevy_app::App;
use bevy_playtest::run_scenario;
use bevy_playtest::scenario::Scenario;
use bevy_transform::prelude::Transform;

fn minimal_app() -> App {
    bevy_playtest::sinks::ensure_global_subscriber();
    App::new()
}

/// A moving entity: keeps the run observable for the oracles
/// (vacuity guard, frozen-world) without asserting anything about it.
fn spawn_observable_entity(app: &mut App) {
    app.add_systems(
        bevy_app::Update,
        |mut q: bevy_ecs::prelude::Query<&mut bevy_transform::prelude::Transform>| {
            for (i, mut t) in q.iter_mut().enumerate() {
                t.translation.x = (i as f32) + 1.0;
            }
        },
    );
    app.world_mut()
        .spawn(bevy_transform::prelude::Transform::from_xyz(0.0, 0.0, 0.0));
}

#[test]
fn chaos_run_passes_on_minimal_app() {
    let _slot = common::run_slot();
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
    let _slot = common::run_slot();
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
    let _slot = common::run_slot();
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
    let _slot = common::run_slot();
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
    let _slot = common::run_slot();
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
    let _slot = common::run_slot();
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
    let _slot = common::run_slot();
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
    let _slot = common::run_slot();
    use bevy_ecs::prelude::*;

    // Spawn an entity with a NaN transform via a system.
    fn spawn_nan_entity(mut commands: Commands) {
        commands.spawn(Transform::from_xyz(f32::NAN, 0.0, 0.0));
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
    let _slot = common::run_slot();
    use bevy_ecs::prelude::*;

    fn spawn_far_entity(mut commands: Commands) {
        commands.spawn(Transform::from_xyz(5000.0, 0.0, 0.0));
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
fn custom_invariant_is_rejected_at_parse_time() {
    let _slot = common::run_slot();
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
    let _slot = common::run_slot();
    fn spawn_far_entity(mut commands: bevy_ecs::prelude::Commands) {
        commands.spawn(Transform::from_xyz(50.0, 9000.0, 0.0));
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
    let _slot = common::run_slot();
    fn spawn_high_entity(mut commands: bevy_ecs::prelude::Commands) {
        commands.spawn(Transform::from_xyz(0.0, 0.0, 1e9));
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

// ---------------------------------------------------------------------------
// Mouse input
// ---------------------------------------------------------------------------

/// A game system observing raw MouseButtonInput messages.
#[test]
fn chaos_mouse_clicks_reach_button_input() {
    let _slot = common::run_slot();
    use bevy_ecs::prelude::*;
    // A game system that counts ButtonInput<MouseButton> presses —
    // the standard way games read mouse clicks.
    #[derive(Resource, Default)]
    struct ObservedClicks(u32);

    fn observe_clicks(
        buttons: Res<bevy_input::prelude::ButtonInput<bevy_input::mouse::MouseButton>>,
        mut clicks: ResMut<ObservedClicks>,
    ) {
        if buttons.just_pressed(bevy_input::mouse::MouseButton::Left) {
            clicks.0 += 1;
        }
    }

    // Default surface (no raw_surface): chaos mouse is DEFAULT-ON —
    // the zero-config path.
    let scenario: Scenario =
        serde_json::from_str(r#"{"bot":{"type":"chaos","seed":7},"duration_s":0.5,"tps":60}"#)
            .unwrap();
    let mut app = minimal_app();
    spawn_observable_entity(&mut app);
    app.add_message::<bevy_input::mouse::MouseButtonInput>();
    app.insert_resource(ObservedClicks::default());
    app.add_systems(bevy_app::Update, observe_clicks);
    run_scenario(&mut app, &scenario).unwrap();
    let clicks = app.world().get_resource::<ObservedClicks>().unwrap().0;
    assert!(
        clicks > 0,
        "the chaos mouse bot must click by default (got {clicks} clicks in 30 frames)"
    );
}

/// Chaos clicks must be deterministic: same seed → same click count and
/// same cursor trajectory.
#[test]
fn chaos_mouse_deterministic_per_seed() {
    let _slot = common::run_slot();
    use bevy_ecs::prelude::*;

    fn run_once() -> (u32, Vec<(f32, f32)>) {
        let scenario: Scenario = serde_json::from_str(
            r#"{"bot":{"type":"chaos","seed":123,"input_rate_hz":20},"duration_s":0.5,"tps":60}"#,
        )
        .unwrap();
        let mut app = App::new();
        bevy_playtest::sinks::ensure_global_subscriber();
        // Observed click cursor positions from the harness cursor resource.
        fn collect_positions(
            cursor: Res<bevy_playtest::driver::CursorInputResource>,
            mut sink: ResMut<PositionSink>,
        ) {
            if let Some(p) = cursor.position() {
                sink.0.push((p.x, p.y));
            }
        }
        #[derive(Resource, Default)]
        struct PositionSink(Vec<(f32, f32)>);
        app.init_resource::<PositionSink>();
        // Force cursor position sampling every frame.
        app.add_systems(bevy_app::Update, collect_positions);
        run_scenario(&mut app, &scenario).unwrap();
        let sink = app.world_mut().remove_resource::<PositionSink>().unwrap();
        let clicks = app
            .world()
            .get_resource::<bevy_input::prelude::ButtonInput<bevy_input::mouse::MouseButton>>()
            .map(|b| b.get_just_pressed().count())
            .unwrap_or(0);
        (clicks as u32, sink.0)
    }

    let (clicks_a, pos_a) = run_once();
    let (clicks_b, pos_b) = run_once();
    assert_eq!(clicks_a, clicks_b, "same seed must yield same click count");
    assert_eq!(pos_a, pos_b, "same seed must yield same cursor trajectory");
}

/// Scripted clicks: `mouse_clicks` entries drive ButtonInput at the
/// scheduled frame, for any bot type.
#[test]
fn scripted_mouse_clicks_fire_on_schedule() {
    let _slot = common::run_slot();
    use bevy_ecs::prelude::*;
    #[derive(Resource, Default)]
    struct ObservedClicks(Vec<(bevy_input::mouse::MouseButton, bevy_input::ButtonState)>);

    fn observe_messages(
        mut events: bevy_ecs::prelude::MessageReader<bevy_input::mouse::MouseButtonInput>,
        mut clicks: ResMut<ObservedClicks>,
    ) {
        for e in events.read() {
            clicks.0.push((e.button, e.state));
        }
    }

    // Empty keyboard surface disables chaos keyboard; empty mouse surface
    // disables chaos mouse. Only the scripted clicks fire.
    let scenario: Scenario = serde_json::from_str(
        r#"{"bot":{"type":"chaos","seed":7,"mouse_clicks":[{"frame":5,"button":"right","x":100.0,"y":200.0,"hold_frames":3}],"raw_surface":{"keys":["w"],"mouse_buttons":[]}},"duration_s":0.2,"tps":60}"#,
    )
    .unwrap();
    let mut app = minimal_app();
    spawn_observable_entity(&mut app);
    app.add_message::<bevy_input::mouse::MouseButtonInput>();
    app.insert_resource(ObservedClicks::default());
    app.add_systems(bevy_app::Update, observe_messages);
    run_scenario(&mut app, &scenario).unwrap();
    let clicks = app.world().get_resource::<ObservedClicks>().unwrap();
    // Press + release of the right button, hold 3 frames.
    let right_clicks: Vec<_> = clicks
        .0
        .iter()
        .filter(|(b, _)| *b == bevy_input::mouse::MouseButton::Right)
        .collect();
    assert!(
        right_clicks
            .iter()
            .any(|(_, s)| *s == bevy_input::ButtonState::Pressed),
        "scripted right-click press must fire"
    );
    assert!(
        right_clicks
            .iter()
            .any(|(_, s)| *s == bevy_input::ButtonState::Released),
        "scripted right-click release must fire after hold_frames"
    );
    // No other buttons fired (both surfaces emptied in the scenario).
    assert!(
        clicks
            .0
            .iter()
            .all(|(b, _)| *b == bevy_input::mouse::MouseButton::Right),
        "chaos mouse must respect an emptied mouse surface"
    );
}

/// Multiple scripted clicks must all fire — same-frame entries AND
/// entries scheduled while another scripted hold is active. Every
/// declared click gets a matching press+release pair; nothing is
/// dropped or stuck held.
#[test]
fn multiple_scripted_clicks_queue_without_dropping() {
    let _slot = common::run_slot();
    use bevy_ecs::prelude::*;
    use bevy_input::mouse::MouseButton;
    use bevy_input::ButtonState;

    #[derive(Resource, Default)]
    struct ObservedClicks(Vec<(MouseButton, ButtonState)>);

    fn observe_messages(
        mut events: bevy_ecs::prelude::MessageReader<bevy_input::mouse::MouseButtonInput>,
        mut clicks: ResMut<ObservedClicks>,
    ) {
        for e in events.read() {
            clicks.0.push((e.button, e.state));
        }
    }

    // Three clicks: two on the SAME frame (different buttons), one
    // scheduled mid-hold (while the frame-5 right hold is still
    // active). Both chaos surfaces emptied so only scripted clicks fire.
    let scenario: Scenario = serde_json::from_str(
        r#"{"bot":{"type":"chaos","seed":7,"mouse_clicks":[{"frame":5,"button":"right","hold_frames":10},{"frame":5,"button":"left","hold_frames":2},{"frame":8,"button":"middle","hold_frames":2}],"raw_surface":{"keys":["w"],"mouse_buttons":[]}},"duration_s":0.3,"tps":60}"#,
    )
    .unwrap();
    let mut app = minimal_app();
    spawn_observable_entity(&mut app);
    app.add_message::<bevy_input::mouse::MouseButtonInput>();
    app.insert_resource(ObservedClicks::default());
    app.add_systems(bevy_app::Update, observe_messages);
    run_scenario(&mut app, &scenario).unwrap();
    let clicks = app.world().get_resource::<ObservedClicks>().unwrap();

    for button in [MouseButton::Left, MouseButton::Right, MouseButton::Middle] {
        let presses = clicks
            .0
            .iter()
            .filter(|(b, s)| *b == button && *s == ButtonState::Pressed)
            .count();
        let releases = clicks
            .0
            .iter()
            .filter(|(b, s)| *b == button && *s == ButtonState::Released)
            .count();
        assert_eq!(
            presses, 1,
            "{button:?} must press exactly once (got {presses})"
        );
        assert_eq!(
            releases, 1,
            "{button:?} must release exactly once (got {releases}) — dropped \
             or stuck holds are both failures"
        );
    }
}

/// Overlapping holds on the same physical button must refcount: chaos
/// and a scripted click can hold `Left` at the same time, and neither's
/// release may clear the other's hold prematurely. Observable
/// consequence: from the scripted press until its release, the button
/// must stay CONTINUOUSLY pressed in `ButtonInput` — no gap where
/// chaos's shorter hold cleared it early.
#[test]
fn overlapping_mouse_holds_refcount_per_button() {
    let _slot = common::run_slot();
    use bevy_ecs::prelude::*;

    #[derive(Resource, Default)]
    struct ObservedFrames(Vec<bool>);

    fn observe_pressed(
        buttons: Res<bevy_input::prelude::ButtonInput<bevy_input::mouse::MouseButton>>,
        mut obs: ResMut<ObservedFrames>,
    ) {
        obs.0
            .push(buttons.pressed(bevy_input::mouse::MouseButton::Left));
    }

    // Chaos mouse (default surface includes Left) fires at up to 60 Hz
    // with 2-frame holds; a scripted left click at frame 2 holds for 20
    // frames. Their windows overlap heavily, so chaos WILL grab Left
    // while the scripted hold is active (seed-deterministic).
    let scenario: Scenario = serde_json::from_str(
        r#"{"bot":{"type":"chaos","seed":7,"input_rate_hz":60,"mouse_clicks":[{"frame":2,"button":"left","x":50.0,"y":50.0,"hold_frames":20}]},"duration_s":0.5,"tps":60}"#,
    )
    .unwrap();
    let mut app = minimal_app();
    spawn_observable_entity(&mut app);
    app.insert_resource(ObservedFrames::default());
    app.add_systems(bevy_app::Update, observe_pressed);
    run_scenario(&mut app, &scenario).unwrap();

    let frames = app.world().get_resource::<ObservedFrames>().unwrap();
    // Scripted press fires at frame 2 and releases at frame 22.
    assert!(
        frames.0.len() > 22,
        "run must cover the scripted hold window (got {} frames)",
        frames.0.len()
    );
    // The press must actually reach ButtonInput (polling observer runs
    // in Update, so the press written during frame 2 is first visible
    // at frame 3 — search rather than index).
    let first_pressed = frames
        .0
        .iter()
        .position(|&p| p)
        .expect("some frame must observe Left pressed");
    assert!(
        first_pressed <= 4,
        "scripted press at frame 2 must be observed promptly (first pressed at \
         frame {first_pressed}): {:?}",
        frames.0
    );
    // Core property: from the FIRST observed press until the scripted
    // release (frame 22), the button never goes un-pressed. A chaos
    // release on the same button clearing the hold would show up as a
    // `false` in that window.
    let mut went_unpressed = false;
    for (f, &pressed) in frames.0.iter().enumerate().take(22) {
        if f >= first_pressed && !pressed {
            went_unpressed = true;
        }
    }
    assert!(
        !went_unpressed,
        "overlapping holds must refcount: Left went un-pressed while the \
         scripted hold was active (sequence: {:?})",
        frames.0
    );
}
