//! End-to-end smoke: run_scenario against a minimal headless app.

// Shared test harness (run-slot serialization)
use crate::common;

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
        use bevy_input::mouse::MouseButton;
        for button in [MouseButton::Left, MouseButton::Right, MouseButton::Middle] {
            if buttons.just_pressed(button) {
                clicks.0 += 1;
            }
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

    #[derive(Resource, Default)]
    struct ObservedLeftClicks(u32);

    fn observe_left(
        buttons: Res<bevy_input::prelude::ButtonInput<bevy_input::mouse::MouseButton>>,
        mut clicks: ResMut<ObservedLeftClicks>,
    ) {
        if buttons.just_pressed(bevy_input::mouse::MouseButton::Left) {
            clicks.0 += 1;
        }
    }

    // Left-specific delivery through the CHAOS path, pinned without
    // seed luck: narrowing the surface to `["left"]` makes Left the
    // only drawable button, so every click this run emits is a Left
    // click, deterministically, whatever the stream draws.
    let scenario_left: Scenario = serde_json::from_str(
        r#"{"bot":{"type":"chaos","seed":7,"raw_surface":{"keys":[],"mouse_buttons":["left"]},"input_rate_hz":60},"duration_s":0.5,"tps":60}"#,
    )
    .unwrap();
    let mut app = minimal_app();
    spawn_observable_entity(&mut app);
    app.add_message::<bevy_input::mouse::MouseButtonInput>();
    app.insert_resource(ObservedLeftClicks::default());
    app.add_systems(bevy_app::Update, observe_left);
    run_scenario(&mut app, &scenario_left).unwrap();
    let left_clicks = app.world().get_resource::<ObservedLeftClicks>().unwrap().0;
    assert!(
        left_clicks > 0,
        "chaos with a left-only mouse surface must deliver Left clicks to \
         ButtonInput::just_pressed (got {left_clicks})"
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

/// Multi-slot chaos: with two hold slots the bot must be able to hold
/// TWO DISTINCT keys at once — a chord, the capability one-key-at-a-time
/// chaos structurally could not deliver (accelerate + steer). Proven at
/// the level games read: `ButtonInput<KeyCode>` shows both keys
/// `pressed` in the same frame for at least one full frame.
#[test]
fn chaos_bot_forms_two_key_chord_in_button_input() {
    let _slot = common::run_slot();
    use bevy_ecs::prelude::*;
    use bevy_input::keyboard::KeyCode;

    #[derive(Resource, Default)]
    struct ObservedChords {
        /// Per frame: how many DISTINCT keys ButtonInput reports pressed.
        concurrent: Vec<usize>,
    }

    fn observe_chords(
        keys: Res<bevy_input::prelude::ButtonInput<KeyCode>>,
        mut obs: ResMut<ObservedChords>,
    ) {
        // Intersect with the surface so the assertion is precise: a
        // chord is two DISTINCT SURFACE keys down at once, not merely
        // two keys from any source.
        const SURFACE: [KeyCode; 4] = [KeyCode::KeyW, KeyCode::KeyA, KeyCode::KeyS, KeyCode::KeyD];
        obs.concurrent
            .push(keys.get_pressed().filter(|k| SURFACE.contains(k)).count());
    }

    // Four-key surface so distinct-key chords are common (a same-key
    // re-press occupies a slot but is not a chord); 60 Hz at tps 60
    // means a press opportunity EVERY frame, so both slots refill
    // almost immediately after any expiry.
    let scenario: Scenario = serde_json::from_str(
        r#"{"bot":{"type":"chaos","seed":42,"raw_surface":{"keys":["w","a","s","d"],"mouse_buttons":[]},"input_rate_hz":60},"duration_s":1.0,"tps":60}"#,
    )
    .unwrap();
    let mut app = minimal_app();
    spawn_observable_entity(&mut app);
    app.insert_resource(ObservedChords::default());
    app.add_systems(bevy_app::Update, observe_chords);
    run_scenario(&mut app, &scenario).unwrap();

    let obs = app.world().get_resource::<ObservedChords>().unwrap();
    let max_concurrent = obs.concurrent.iter().copied().max().unwrap_or(0);
    assert!(
        max_concurrent >= 2,
        "two hold slots must yield at least one frame with two distinct keys \
         simultaneously pressed in ButtonInput (per-frame concurrency: {:?})",
        obs.concurrent
    );
}

/// Multi-slot chaos hold bookkeeping, asserted from the outside via the
/// raw `KeyboardInput` message stream. Two properties:
/// 1. No orphan releases: a `Released` for a key may only arrive when a
///    message-level unmatched `Pressed` for that key is outstanding —
///    the last-holder-releases rule. A premature same-key release (one
///    slot expiring while the other still holds) would emit a second
///    `Released` with no `Pressed` in between.
/// 2. Balanced ledger: at run end, per-key outstanding (unreleased)
///    presses are bounded by the slot count (2) — holds still active at
///    the final frame legitimately never release, but they cannot
///    exceed the slots that exist.
#[test]
fn chaos_holds_never_emit_orphan_releases() {
    let _slot = common::run_slot();
    use bevy_ecs::prelude::*;
    use bevy_input::keyboard::KeyCode;

    #[derive(Resource, Default)]
    struct Ledger {
        /// Per key: presses minus releases seen so far.
        outstanding: std::collections::HashMap<KeyCode, i64>,
        /// Released-without-outstanding-press occurrences (key, frame).
        orphans: Vec<(KeyCode, usize)>,
        frame: usize,
    }

    fn track_ledger(
        mut events: bevy_ecs::prelude::MessageReader<bevy_input::keyboard::KeyboardInput>,
        mut ledger: ResMut<Ledger>,
    ) {
        for e in events.read() {
            let outstanding = *ledger.outstanding.entry(e.key_code).or_insert(0);
            match e.state {
                bevy_input::ButtonState::Pressed => {
                    ledger.outstanding.insert(e.key_code, outstanding + 1);
                }
                bevy_input::ButtonState::Released => {
                    let frame = ledger.frame;
                    if outstanding <= 0 {
                        ledger.orphans.push((e.key_code, frame));
                    }
                    ledger.outstanding.insert(e.key_code, outstanding - 1);
                }
            }
        }
        ledger.frame += 1;
    }

    // Two-key surface makes the candidate filter the binding constraint:
    // whenever a key is held it MUST be excluded from the draw — with
    // only one unheld key left the bound shrinks to 1 — and the ledger
    // asserts per-key outstanding presses never exceed ONE, proving a
    // key never occupies two slots at once (the impossible
    // double-hold). The refcount's last-holder-releases branch is
    // defense-in-depth unreachable by this draw policy; it is covered
    // synthetically in `same_key_in_two_slots_last_holder_releases`.
    let scenario: Scenario = serde_json::from_str(
        r#"{"bot":{"type":"chaos","seed":7,"raw_surface":{"keys":["w","a"],"mouse_buttons":[]},"input_rate_hz":60},"duration_s":1.0,"tps":60}"#,
    )
    .unwrap();
    let mut app = minimal_app();
    spawn_observable_entity(&mut app);
    app.add_message::<bevy_input::keyboard::KeyboardInput>();
    app.insert_resource(Ledger::default());
    app.add_systems(bevy_app::Update, track_ledger);
    let report = run_scenario(&mut app, &scenario).unwrap();

    let ledger = app.world().get_resource::<Ledger>().unwrap();
    assert!(
        ledger.orphans.is_empty(),
        "orphan Released messages (no outstanding Pressed) — a slot released \
         a key another slot still holds: {:?}",
        ledger.orphans
    );
    let max_outstanding = ledger.outstanding.values().copied().max().unwrap_or(0);
    // ONE per key, not two: the candidate filter must prevent a key
    // from occupying two slots simultaneously. The old `<= 2` bound
    // would pass even if double-holds slipped through.
    assert!(
        max_outstanding <= 1,
        "per-key outstanding holds at run end ({max_outstanding}) — a key \
         occupied two slots at once or a release went missing: {:?}",
        ledger.outstanding
    );
    let min_outstanding = ledger.outstanding.values().copied().min().unwrap_or(0);
    assert!(
        min_outstanding >= 0,
        "more releases than presses for some key: {:?}",
        ledger.outstanding
    );
    assert!(
        report.metrics.input_count > 0,
        "the chaos bot must have pressed keys"
    );
}

/// Hold-duration draws are recorded with their own `ChoiceKind` so they
/// never masquerade as key presses: the action log (filtered on key
/// draws) must have exactly one `key_press` entry per key-kind draw, and
/// hold draws must appear in `choices` as `key_hold_frames` with the
/// reduced value bounded by the 2..=30-frame range.
#[test]
fn chaos_hold_duration_draws_do_not_masquerade_as_key_presses() {
    let _slot = common::run_slot();
    let scenario: Scenario = serde_json::from_str(
        r#"{"bot":{"type":"chaos","seed":42,"raw_surface":{"keys":["w","a","s","d"],"mouse_buttons":[]},"input_rate_hz":60},"duration_s":0.5,"tps":60}"#,
    )
    .unwrap();
    let mut app = minimal_app();
    spawn_observable_entity(&mut app);
    let report = run_scenario(&mut app, &scenario).unwrap();

    use bevy_playtest::choice::ChoiceKind;
    let key_draws = report
        .choices
        .iter()
        .filter(|c| c.kind == ChoiceKind::Key)
        .count();
    let hold_draws: Vec<&bevy_playtest::choice::Choice> = report
        .choices
        .iter()
        .filter(|c| c.kind == ChoiceKind::KeyHoldFrames)
        .collect();
    assert!(
        key_draws > 0 && !hold_draws.is_empty(),
        "a keyboard-chaos run must record both key draws ({key_draws}) and \
         hold-duration draws ({})",
        hold_draws.len()
    );
    // Every key draw pairs with a hold draw at the same frame...
    for c in report.choices.iter().filter(|c| c.kind == ChoiceKind::Key) {
        assert!(
            hold_draws.iter().any(|h| h.frame == c.frame),
            "key draw at frame {} has no paired hold-duration draw",
            c.frame
        );
    }
    // ...and the delivered durations (draw + 2) stay in 2..=30.
    for h in &hold_draws {
        assert_eq!(h.bound, 29, "hold draws reduce over a 29-bound");
        assert!(h.value < 29, "reduced hold draw out of range: {:?}", h);
    }
    // Action log: exactly one key_press entry per key draw, none from
    // hold draws.
    assert_eq!(
        report.action_log.len(),
        key_draws,
        "action log must have one entry per key draw (hold draws excluded): \
         {:?} vs {:?}",
        report.action_log,
        report.choices
    );
    // F1 follow-through: every key_press entry must resolve to a real
    // surface key — the `details` field carries the canonical key name
    // the drawn index resolved to at draw time.
    let surface: Vec<String> = ["w", "a", "s", "d"]
        .iter()
        .filter_map(|k| bevy_playtest::scenario::normalize_key_name(k).map(|c| c.to_string()))
        .collect();
    for entry in &report.action_log {
        let name = entry
            .details
            .as_deref()
            .unwrap_or_else(|| panic!("key_press entry without a key label: {entry:?}"));
        assert!(
            surface.iter().any(|s| s == name),
            "action-log label \"{name}\" is not a canonical surface key"
        );
    }
}

/// A key whose hold expires on a rate tick immediately re-enters the
/// candidate pool (expiry runs before the draw) and can be re-drawn the
/// SAME frame — the message batch then carries `Released(W)` followed
/// by `Pressed(W)`. Deterministic by construction: with a ONE-key
/// surface and a press opportunity every frame (60 Hz at tps 60), the
/// sole key is redrawn every time its hold lapses, whatever the seed.
/// Semantics to pin: `ButtonInput` must observe the retap as a new
/// `just_pressed` WITHOUT losing `pressed` continuity — the release
/// precedes the press in the batch, so the pair does not cancel —
/// while `just_released` fires the same frame the key is down again.
/// This is the intentional fast-retap behavior; the alternative
/// (excluded-from-candidates-for-a-frame) would add a phantom rule
/// with no physical basis.
#[test]
fn chaos_same_frame_retap_observes_both_edges() {
    let _slot = common::run_slot();
    use bevy_ecs::prelude::*;
    use bevy_input::keyboard::KeyCode;

    #[derive(Resource, Default, Clone)]
    struct RetapObservations {
        /// Frames where `just_pressed(W)` fired (index = frame).
        just_pressed_frames: Vec<usize>,
        /// Frames where `just_released(W)` fired (index = frame).
        just_released_frames: Vec<usize>,
        /// Per frame: whether W was `pressed`.
        pressed_frames: Vec<bool>,
    }

    fn observe(
        keys: Res<bevy_input::prelude::ButtonInput<KeyCode>>,
        mut obs: ResMut<RetapObservations>,
    ) {
        let frame = obs.pressed_frames.len();
        if keys.just_pressed(KeyCode::KeyW) {
            obs.just_pressed_frames.push(frame);
        }
        if keys.just_released(KeyCode::KeyW) {
            obs.just_released_frames.push(frame);
        }
        obs.pressed_frames.push(keys.pressed(KeyCode::KeyW));
    }

    let scenario: Scenario = serde_json::from_str(
        r#"{"bot":{"type":"chaos","seed":99,"raw_surface":{"keys":["w"],"mouse_buttons":[]},"input_rate_hz":60},"duration_s":1.0,"tps":60}"#,
    )
    .unwrap();
    let mut app = minimal_app();
    spawn_observable_entity(&mut app);
    app.add_message::<bevy_input::keyboard::KeyboardInput>();
    app.insert_resource(RetapObservations::default());
    app.add_systems(bevy_app::Update, observe);
    run_scenario(&mut app, &scenario).unwrap();

    let obs = app
        .world()
        .get_resource::<RetapObservations>()
        .unwrap()
        .clone();
    // Multiple press edges must have occurred (retaps) ...
    assert!(
        obs.just_pressed_frames.len() >= 2,
        "a one-key surface with a free slot every frame must produce \
         multiple W press edges; got {:?}",
        obs.just_pressed_frames
    );
    // ... including at least one frame with BOTH edges (the retap:
    // release-then-press in one batch).
    let retap_frames: Vec<usize> = obs
        .just_released_frames
        .iter()
        .filter(|f| obs.just_pressed_frames.contains(f))
        .copied()
        .collect();
    assert!(
        !retap_frames.is_empty(),
        "a key expiring on a rate tick must be drawable the same frame; \
         just_pressed={:?} just_released={:?}",
        obs.just_pressed_frames,
        obs.just_released_frames
    );
    // The release precedes the press in the batch, so the retap never
    // cancels: no frame where W was released-and-not-down after having
    // been down continuously... in fact `pressed` continuity means
    // every just_released frame must also show W as pressed (again).
    for f in &retap_frames {
        assert!(
            obs.pressed_frames.get(*f).copied().unwrap_or(false),
            "retap frame {f} must leave W pressed (release precedes press \
             in the batch — no cancellation)"
        );
    }
}
