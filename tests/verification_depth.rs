//! Replay-divergence, cheat postcondition contracts, coverage.

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

// ---------------------------------------------------------------------------
// Part 2: cheat postcondition contracts
// ---------------------------------------------------------------------------

#[test]
fn cheating_hook_satisfying_lt_contract_proves_action() {
    use bevy_playtest::contract::CheatHooks;
    use std::collections::HashMap;
    let mut hooks = CheatHooks(HashMap::new());
    hooks.0.insert(
        "purge".to_string(),
        Box::new(|world| {
            let entities: Vec<bevy_ecs::entity::Entity> = world
                .query_filtered::<bevy_ecs::entity::Entity, bevy_ecs::query::With<Transform>>()
                .iter(world)
                .collect();
            for e in entities {
                world.despawn(e);
            }
        }),
    );
    let mut app = App::new();
    app.insert_resource(hooks);
    app.world_mut()
        .spawn_batch((0..5).map(|_| Transform::default()));
    let scenario: Scenario = serde_json::from_str(
        r#"{"bot":{"type":"chaos","seed":1},"duration_s":0.05,"tps":60,
            "setup":{"cheats":[{"frame":0,"kind":"purge",
                                  "expects":{"transform_entity_count":"lt"}}]}}"#,
    )
    .unwrap();
    let report = run_scenario(&mut app, &scenario).unwrap();
    assert!(!report
        .violations
        .iter()
        .any(|v| v.rule == "cheat_contract_violated"));
}

#[test]
fn noop_cheat_hook_fails_lt_contract_negative_control() {
    use bevy_playtest::contract::CheatHooks;
    use std::collections::HashMap;
    let mut hooks = CheatHooks(HashMap::new());
    // The lying hook: registered, invoked, does nothing.
    hooks.0.insert("noop".to_string(), Box::new(|_world| {}));
    let mut app = App::new();
    app.insert_resource(hooks);
    app.world_mut()
        .spawn_batch((0..3).map(|_| Transform::default()));
    let scenario: Scenario = serde_json::from_str(
        r#"{"bot":{"type":"chaos","seed":1},"duration_s":0.05,"tps":60,
            "setup":{"cheats":[{"frame":0,"kind":"noop",
                                  "expects":{"transform_entity_count":"lt"}}]}}"#,
    )
    .unwrap();
    let report = run_scenario(&mut app, &scenario).unwrap();
    let v = report
        .violations
        .iter()
        .find(|v| v.rule == "cheat_contract_violated")
        .expect("no-op hook must violate the lt contract");
    assert!(v.detail.contains("silent no-op"), "detail: {}", v.detail);
    assert!(
        v.detail.contains("3"),
        "both values in detail: {}",
        v.detail
    );
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
}

#[test]
fn cheating_hook_eq_contract_asserts_stability() {
    use bevy_playtest::contract::CheatHooks;
    use std::collections::HashMap;
    let mut hooks = CheatHooks(HashMap::new());
    hooks.0.insert("stable".to_string(), Box::new(|_world| {}));
    let mut app = App::new();
    app.insert_resource(hooks);
    app.world_mut().spawn(Transform::default());
    let scenario: Scenario = serde_json::from_str(
        r#"{"bot":{"type":"chaos","seed":1},"duration_s":0.05,"tps":60,
            "setup":{"cheats":[{"frame":0,"kind":"stable",
                                  "expects":{"transform_entity_count":"eq"}}]}}"#,
    )
    .unwrap();
    let report = run_scenario(&mut app, &scenario).unwrap();
    assert!(!report
        .violations
        .iter()
        .any(|v| v.rule == "cheat_contract_violated"));
}

// ---------------------------------------------------------------------------
// Part 3: behavioral coverage
// ---------------------------------------------------------------------------

#[test]
fn declared_observation_matched_by_log_message_is_observed() {
    let mut app = App::new();
    app.add_systems(bevy_app::Update, || {
        tracing::info!("enemy_killed at range 42");
    });
    let scenario: Scenario = serde_json::from_str(
        r#"{"bot":{"type":"chaos","seed":1},"duration_s":0.1,"tps":60,
            "observes":["enemy_killed"]}"#,
    )
    .unwrap();
    let report = run_scenario(&mut app, &scenario).unwrap();
    let cov = report.coverage.expect("coverage present when declared");
    assert_eq!(cov.observations.len(), 1);
    assert!(cov.observations[0].observed);
    assert!(!report.violations.iter().any(|v| v.rule == "coverage_hole"));
}

#[test]
fn declared_observation_never_logged_is_a_hole_not_a_fail() {
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
    let scenario: Scenario = serde_json::from_str(
        r#"{"bot":{"type":"chaos","seed":1},"duration_s":0.1,"tps":60,
            "observes":["boss_defeated","treasure_opened"]}"#,
    )
    .unwrap();
    let report = run_scenario(&mut app, &scenario).unwrap();
    let cov = report.coverage.unwrap();
    assert_eq!(cov.observations.len(), 2);
    assert!(cov.observations.iter().all(|o| !o.observed));
    // Default: holes are report-only, not Fail.
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
}

#[test]
fn strict_coverage_makes_holes_fail() {
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
    let scenario: Scenario = serde_json::from_str(
        r#"{"bot":{"type":"chaos","seed":1},"duration_s":0.1,"tps":60,
            "observes":["never_happens"],"strict_coverage":true}"#,
    )
    .unwrap();
    let report = run_scenario(&mut app, &scenario).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    let v = report
        .violations
        .iter()
        .find(|v| v.rule == "coverage_hole")
        .unwrap();
    assert!(v.detail.contains("never_happens") || v.count >= 1);
}

#[test]
fn chaos_surface_coverage_reports_unexercised_keys() {
    let scenario: Scenario = serde_json::from_str(
        r#"{"bot":{"type":"chaos","seed":1,"raw_surface":{"keys":["KeyW","Space","Escape"]}},
            "duration_s":0.1,"tps":60}"#,
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
    let cov = report
        .coverage
        .as_ref()
        .expect("raw_surface declared -> coverage");
    assert!(cov.surface_coverage >= 0.0 && cov.surface_coverage <= 1.0);
    let drawn: usize = (cov.surface_coverage * 3.0).round() as usize;
    assert_eq!(drawn + cov.unexercised_keys.len(), 3);
}

#[test]
fn no_declarations_means_no_coverage_field() {
    let mut app = moving_cube_app();
    let scenario = scenario_json("");
    let report = run_scenario(&mut app, &scenario).unwrap();
    assert!(report.coverage.is_none());
}
