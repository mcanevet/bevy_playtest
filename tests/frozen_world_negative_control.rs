//! Negative control: the broadened frozen_world oracle still fires on
//! a genuinely stuck world. Prevents the liveness broadening
//! (spawn/despawn counts as activity) from silently disabling the
//! oracle wholesale — a world with entities that never move, spawn,
//! or despawn MUST still be reported frozen.

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

/// A static entity: exists (so the run is not vacuous) but its
/// Transform never changes and nothing ever spawns or despawns —
/// the world is genuinely frozen.
fn spawn_static_entity(app: &mut App) {
    app.world_mut()
        .spawn(Transform::from_xyz(1.0, 2.0, 3.0));
}

#[test]
fn frozen_world_fires_on_genuinely_stuck_world() {
    let _slot = common::run_slot();
    // Short grace (0.25s @ 60tps = 15 frames) so the test is fast.
    let scenario: Scenario = serde_json::from_str(
        r#"{"bot":{"type":"chaos","seed":42},"duration_s":0.5,"tps":60,"liveness":{"timeout_s":0.25}}"#,
    )
    .unwrap();
    let mut app = minimal_app();
    spawn_static_entity(&mut app);
    let report = run_scenario(&mut app, &scenario).unwrap();

    let pairs = report.violation_pairs();
    assert!(
        pairs.contains(&("frozen_world", "world")),
        "expected frozen_world on a stuck world, got {pairs:?}"
    );
}
