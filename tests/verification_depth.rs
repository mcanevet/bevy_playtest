//! Replay-divergence verification depth.

// Shared test harness (run-slot serialization)
#[path = "common/mod.rs"]
mod common;

use bevy_app::App;
use bevy_playtest::run_scenario;
use bevy_playtest::scenario::Scenario;
use bevy_transform::prelude::*;

fn scenario_json(extra: &str) -> Scenario {
    serde_json::from_str(&format!(
        r#"{{"bot":{{"type":"chaos","seed":42}},"duration_s":0.1,"tps":60{extra}}}"#
    ))
    .unwrap()
}

fn moving_cube_app() -> App {
    bevy_playtest::sinks::ensure_global_subscriber();
    let mut app = App::new();
    app.add_systems(
        bevy_app::Update,
        |mut q: bevy_ecs::prelude::Query<&mut Transform>| {
            for mut t in q.iter_mut() {
                t.translation.x += 0.1;
            }
        },
    );
    app.world_mut().spawn(Transform::from_xyz(0.0, 0.0, 0.0));
    app
}

// ---------------------------------------------------------------------------
// Part 1: replay divergence
// ---------------------------------------------------------------------------

#[test]
fn replay_verification_passes_on_deterministic_app() {
    let _slot = common::run_slot();
    let scenario = scenario_json(r#","verify_replay":true"#);
    let report = bevy_playtest::replay::run_scenario_verified(moving_cube_app, &scenario).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    assert!(!report
        .violations
        .iter()
        .any(|v| v.rule == "replay_divergence"));
}

#[test]
fn replay_divergence_fires_on_nondeterministic_app() {
    let _slot = common::run_slot();
    use std::sync::atomic::{AtomicU64, Ordering};
    static CALLS: AtomicU64 = AtomicU64::new(0);
    let scenario = scenario_json(r#","verify_replay":true"#);
    let mk = || {
        let mut a = moving_cube_app();
        a.add_systems(
            bevy_app::Update,
            |mut q: bevy_ecs::prelude::Query<&mut Transform>| {
                // The negative control: a counter SHARED between the two
                // Apps (process-global static), i.e. state that differs
                // between passes — the nondeterminism users hide in games
                // (wall clock, global RNG). NOT via the ChoiceStream.
                let j = CALLS.fetch_add(1, Ordering::SeqCst) as f32;
                for mut t in q.iter_mut() {
                    t.translation.y = j;
                }
            },
        );
        a
    };
    let mut app_a = mk();
    let mut app_b = mk();
    let _ = run_scenario(&mut app_a, &scenario).unwrap();
    let _ = run_scenario(&mut app_b, &scenario).unwrap();
    let sa = app_a
        .world_mut()
        .remove_resource::<bevy_playtest::replay::ReplaySeries>()
        .map(|s| s.0)
        .unwrap();
    let sb = app_b
        .world_mut()
        .remove_resource::<bevy_playtest::replay::ReplaySeries>()
        .map(|s| s.0)
        .unwrap();
    let div = bevy_playtest::replay::compare(&sa, &sb).unwrap_err();
    assert!(
        div.frame < 6,
        "divergence must be detected early, got frame {}",
        div.frame
    );
}

#[test]
fn replay_fingerprint_is_stable_across_identical_constructions() {
    let _slot = common::run_slot();
    let scenario = scenario_json(r#","verify_replay":true"#);
    let mut app_a = moving_cube_app();
    let mut app_b = moving_cube_app();
    let _ = run_scenario(&mut app_a, &scenario).unwrap();
    let _ = run_scenario(&mut app_b, &scenario).unwrap();
    let sa = app_a
        .world_mut()
        .remove_resource::<bevy_playtest::replay::ReplaySeries>()
        .map(|s| s.0)
        .unwrap();
    let sb = app_b
        .world_mut()
        .remove_resource::<bevy_playtest::replay::ReplaySeries>()
        .map(|s| s.0)
        .unwrap();
    assert_eq!(sa.len(), sb.len());
    assert!(!sa.is_empty(), "fingerprints were recorded");
    bevy_playtest::replay::compare(&sa, &sb).expect("identical constructions must agree");
}

/// Mouse draws must not pollute key telemetry: a mouse-only chaos
/// surface must leave the action log free of fake key presses, and
/// mouse button-index draws must not be counted as exercised keys.
#[test]
fn mouse_only_chaos_does_not_fabricate_key_activity() {
    let _slot = common::run_slot();
    // Mouse-only surface (keys explicitly emptied) exercising clicks.
    let scenario: Scenario = serde_json::from_str(
        r#"{"bot":{"type":"chaos","seed":7,"raw_surface":{"keys":[],"mouse_buttons":["left","right","middle"]}},
            "duration_s":0.2,"tps":60}"#,
    )
    .unwrap();
    let mut app = App::new();
    app.world_mut().spawn(Transform::default());
    app.add_systems(
        bevy_app::Update,
        |mut q: bevy_ecs::prelude::Query<&mut Transform>| {
            for mut t in q.iter_mut() {
                t.translation.x += 0.1;
            }
        },
    );
    let report = run_scenario(&mut app, &scenario).unwrap();
    // No key draws occurred: no fake key_press action-log entries, even
    // though the mouse bot clicked (input_count > 0).
    assert!(
        report
            .action_log
            .iter()
            .all(|a| !a.action.starts_with("key_press:")),
        "mouse-only chaos must not fabricate key_press action-log entries: {:?}",
        report.action_log
    );
    assert!(
        report.metrics.input_count > 0,
        "the mouse bot must have clicked"
    );
    // Every recorded choice must be a mouse draw, never a key draw.
    assert!(
        report
            .choices
            .iter()
            .all(|c| c.kind != bevy_playtest::choice::ChoiceKind::Key),
        "mouse-only chaos must record no key draws: {:?}",
        report.choices
    );
}

/// End-to-end: `record_digest: true` alone (no verify_replay) must
/// populate `report.state_digest`, one entry per frame starting at 0,
/// and the digests must evolve as game state changes.
#[test]
fn digest_recording_opt_in_populates_report() {
    let _slot = common::run_slot();

    let scenario: Scenario = serde_json::from_str(
        r#"{"bot":{"type":"chaos","seed":42},"duration_s":0.05,"tps":60,"record_digest":true}"#,
    )
    .unwrap();

    let mut app = App::new();
    app.world_mut().spawn(Transform::default());
    app.add_systems(
        bevy_app::Update,
        |mut q: bevy_ecs::prelude::Query<&mut Transform>| {
            for mut t in q.iter_mut() {
                t.translation.x += 0.1;
            }
        },
    );

    let report = run_scenario(&mut app, &scenario).unwrap();

    assert!(
        !report.state_digest.is_empty(),
        "state_digest must be populated when record_digest is enabled"
    );
    assert_eq!(
        report.state_digest[0].frame, 0,
        "first digest must be at frame 0"
    );
    // Frames must be consecutive.
    for w in report.state_digest.windows(2) {
        assert_eq!(w[1].frame, w[0].frame + 1, "frames must be consecutive");
    }
    // The cube moves every frame: digests must evolve.
    let distinct = report
        .state_digest
        .iter()
        .map(|d| d.hash)
        .collect::<std::collections::HashSet<_>>();
    assert!(
        distinct.len() > 1,
        "moving-world digests must change across frames"
    );
}

/// End-to-end: without `record_digest`, the report must carry no
/// state-digest series (opt-in contract; existing tests unaffected).
#[test]
fn digest_recording_absent_by_default() {
    let _slot = common::run_slot();

    let scenario = scenario_json(r"");
    let mut app = App::new();
    app.world_mut().spawn(Transform::default());
    let report = run_scenario(&mut app, &scenario).unwrap();
    assert!(
        report.state_digest.is_empty(),
        "state_digest must stay empty when record_digest is unset"
    );
}

/// End-to-end: `record_digest: true` + `verify_replay: true` compares
/// the broad digest series between passes — deterministic apps must
/// still Pass.
#[test]
fn digest_replay_verification_passes_on_deterministic_app() {
    let _slot = common::run_slot();
    let scenario = scenario_json(r#","record_digest":true,"verify_replay":true"#);
    let report = bevy_playtest::replay::run_scenario_verified(moving_cube_app, &scenario).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    assert!(!report
        .violations
        .iter()
        .any(|v| v.rule == "replay_divergence"));
}

/// Negative control for the broad digest: a nondeterministic REFLECTED
/// RESOURCE that never touches a Transform is invisible to the narrow
/// fingerprint but MUST be caught by the state digest. Without
/// `record_digest` this app would falsely certify as deterministic.
#[test]
fn digest_divergence_fires_on_nondeterministic_resource() {
    let _slot = common::run_slot();
    use std::sync::atomic::{AtomicU64, Ordering};
    static CALLS: AtomicU64 = AtomicU64::new(0);

    #[derive(bevy_ecs::prelude::Resource, Debug, bevy_reflect::Reflect)]
    #[reflect(bevy_ecs::reflect::ReflectResource)]
    struct GameClock(u64);

    let mk = || {
        let mut app = App::new();
        app.world_mut().spawn(Transform::default());
        app.world_mut()
            .init_resource::<bevy_ecs::reflect::AppTypeRegistry>();
        {
            let registry = app
                .world_mut()
                .resource_mut::<bevy_ecs::reflect::AppTypeRegistry>();
            registry.0.write().register::<GameClock>();
        }
        app.world_mut().insert_resource(GameClock(0));
        app.add_systems(
            bevy_app::Update,
            |mut clock: bevy_ecs::prelude::ResMut<GameClock>| {
                // Cross-pass state (process-global static): differs between
                // pass A and pass B — exactly the hidden nondeterminism the
                // digest exists to expose. No Transform is touched, so the
                // narrow fingerprint stays blind to it.
                clock.0 = CALLS.fetch_add(1, Ordering::SeqCst);
            },
        );
        app
    };

    // Sanity control: the narrow fingerprint alone must NOT flag it
    // (this is the coverage gap the broad digest fills).
    let scenario_narrow = scenario_json(r#""#);
    let report_narrow = bevy_playtest::replay::run_scenario_verified(mk, &scenario_narrow).unwrap();
    assert!(
        !report_narrow
            .violations
            .iter()
            .any(|v| v.rule == "replay_divergence"),
        "control: narrow fingerprint must not see a resource-only divergence"
    );

    // Reset the shared counter so pass/pass bookkeeping from the control
    // run doesn't shift the (irrelevant) absolute values.
    CALLS.store(0, Ordering::SeqCst);

    // With record_digest, the divergence must be caught.
    let scenario = scenario_json(r#","record_digest":true,"verify_replay":true"#);
    let report = bevy_playtest::replay::run_scenario_verified(mk, &scenario).unwrap();
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.rule == "replay_divergence"),
        "state digest must catch a diverging reflected resource"
    );
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
}
