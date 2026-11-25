//! Behavioral battery for the `spawn_safety` rule: the
//! archetype-entry spatial oracle. Entities are judged ONCE, at
//! the frame they enter the selected archetype — a newborn bearing
//! the target component, or a pre-existing entity first acquiring
//! it — against anchors snapshotted at the END of the same
//! judging frame (same-frame anchors participate). No episode, no
//! soak: the transition frame is the whole verdict, and the rule
//! GATES (a body appearing inside the danger zone is a state bug
//! at the transition, not a transient fly-by the window doctrine
//! exists to smooth over).
//!
//! The litmus shapes mirror the respawn-ring pile-up incident
//! class on record: a respawned body appearing on top of an anchor.
//! Frame 0 is the scenario's initial scene — the recorder stores
//! its first snapshot there and judges nothing — so every planted
//! spawn happens from frame 1 onward, exactly the mid-run
//! transition the rule exists for. Every fixture keeps the beacon
//! wiggling so frozen_world stays quiet: the spatial verdicts are
//! margin-tested (distance 0 vs threshold 0.5) and a ±0.01 y-wiggle
//! never crosses the line.

use crate::common;

use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_playtest::driver::run_scenario;
use bevy_playtest::enums::PlaytestStatus;
use bevy_reflect::Reflect;
use bevy_transform::prelude::Transform;

/// The judged archetype's marker: what the target selector names.
/// Uniquely named — `Anchor` would be ambiguous against
/// `bevy_sprite::sprite::Anchor` in the shared registry.
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct Projectile;

/// The anchor set's marker (same uniqueness doctrine).
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct Beacon;

fn scenario_with_invariant(inv: &str) -> bevy_playtest::scenario::Scenario {
    let json = format!(
        r#"{{"bot":{{"type":"chaos","seed":42}},"duration_s":4.0,"tps":60,
            "invariants":[{{{inv}}}]}}"#
    );
    serde_json::from_str(&json).unwrap()
}

/// Keep the world changing (frozen_world stays quiet) without ever
/// moving a beacon across a planted distance margin: a ±0.01 y
/// wiggle around each beacon's authored rest position.
fn wiggle_beacons(mut q: Query<(&mut Transform, &Beacon)>, mut t: Local<f32>) {
    *t += 0.05;
    for (mut tr, _) in q.iter_mut() {
        tr.translation.y = 0.01 * t.sin();
    }
}

fn base_app() -> App {
    let mut app = App::new();
    // TaskPoolPlugin + TransformPlugin (the consistency battery's
    // precedent): propagation systems run parallel queries that
    // need the ComputeTaskPool, and without Propagate the spawned
    // locals' required GlobalTransforms stay at identity — every
    // distance would read 0 and the battery would prove nothing.
    app.add_plugins((
        bevy_app::TaskPoolPlugin::default(),
        bevy_transform::prelude::TransformPlugin,
    ));
    app.register_type::<Projectile>();
    app.register_type::<Beacon>();
    app.register_type::<Transform>();
    app.add_systems(Update, wiggle_beacons);
    app
}

const SAFETY_INV: &str = r#"
    "name": "safe_respawns",
    "rule": "spawn_safety",
    "target_selector": {"with_component": {"name": "Projectile"}},
    "min_distance_from": {"with_component": {"name": "Beacon"}},
    "min_distance": 0.5"#;

// ---------------------------------------------------------------------------
// The respawn-ring shape
// ---------------------------------------------------------------------------

/// THE incident shape: a projectile respawns INSIDE the danger zone
/// (exactly on top of the beacon). The entry is judged at its
/// transition frame, the finding fires, and the run FAILS — the
/// rule gates, no soak can pardon a state bug at the transition.
#[test]
fn respawn_inside_the_zone_gates_immediately() {
    let _slot = common::run_slot();
    let mut app = base_app();
    // Frame-0 scene: one beacon at the origin, no projectiles.
    app.world_mut()
        .spawn((Beacon, Transform::from_xyz(0.0, 0.0, 0.0)));
    // The planted respawn: from frame 1 on, one projectile appears
    // exactly on top of the beacon (distance 0).
    app.add_systems(
        Update,
        |mut commands: Commands, mut fired: Local<bool>, mut frame: Local<u64>| {
            *frame += 1;
            // Frame 0 is the initial scene (never a judged entry):
            // the planted spawn fires from frame 1 on.
            if *frame > 1 && !*fired {
                *fired = true;
                commands.spawn((Projectile, Transform::from_xyz(0.0, 0.0, 0.0)));
            }
        },
    );
    let report = run_scenario(&mut app, &scenario_with_invariant(SAFETY_INV)).unwrap();
    let v = report
        .violations
        .iter()
        .find(|v| v.rule == "spawn_safety")
        .expect("expected a spawn_safety finding");
    assert_eq!(report.status, PlaytestStatus::Fail);
    assert!(
        v.detail.contains("within min_distance"),
        "finding names the violated radius: {}",
        v.detail
    );
    assert!(
        v.detail.contains("nearest distance 0."),
        "finding carries the measured distance: {}",
        v.detail
    );
}

/// Control: the same respawn, parked safely OUTSIDE the zone. One
/// entry, judged clean — no finding, run passes.
#[test]
fn respawn_outside_the_zone_passes() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.world_mut()
        .spawn((Beacon, Transform::from_xyz(0.0, 0.0, 0.0)));
    app.add_systems(
        Update,
        |mut commands: Commands, mut fired: Local<bool>, mut frame: Local<u64>| {
            *frame += 1;
            // Frame 0 is the initial scene (never a judged entry):
            // the planted spawn fires from frame 1 on.
            if *frame > 1 && !*fired {
                *fired = true;
                commands.spawn((Projectile, Transform::from_xyz(0.0, 0.0, 5.0)));
            }
        },
    );
    let report = run_scenario(&mut app, &scenario_with_invariant(SAFETY_INV)).unwrap();
    assert_eq!(report.status, PlaytestStatus::Pass);
    assert!(
        !report.violations.iter().any(|v| v.rule == "spawn_safety"),
        "a clean entry must not fire: {:?}",
        report.violations
    );
}

/// Entry is judged at the TRANSITION FRAME ONLY: a projectile
/// born outside the zone and walking INTO it later must never
/// fire — the pairwise soak exists for exactly that other shape.
#[test]
fn walking_into_the_zone_later_is_not_this_rule() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.world_mut()
        .spawn((Beacon, Transform::from_xyz(0.0, 0.0, 0.0)));
    // Born at distance 1.0 (> 0.5, safe entry), then marches to
    // the beacon every frame after — the run ends parked at
    // distance 0, but the ENTRY was clean.
    app.add_systems(Update, |mut q: Query<&mut Transform, With<Projectile>>| {
        for mut t in q.iter_mut() {
            if t.translation.z > 0.0 {
                t.translation.z -= 0.1;
            }
        }
    });
    app.add_systems(
        Update,
        |mut commands: Commands, mut fired: Local<bool>, mut frame: Local<u64>| {
            *frame += 1;
            // Frame 0 is the initial scene (never a judged entry):
            // the planted spawn fires from frame 1 on.
            if *frame > 1 && !*fired {
                *fired = true;
                commands.spawn((Projectile, Transform::from_xyz(0.0, 0.0, 1.0)));
            }
        },
    );
    let report = run_scenario(&mut app, &scenario_with_invariant(SAFETY_INV)).unwrap();
    assert_eq!(report.status, PlaytestStatus::Pass);
    assert!(
        !report.violations.iter().any(|v| v.rule == "spawn_safety"),
        "later overlap is pairwise's shape, not an entry judgment: {:?}",
        report.violations
    );
}

/// Frame-0 population is the scenario's initial scene, never a
/// judged entry: a projectile sitting on top of a beacon FROM THE
/// START must not fire (the recorder stores its first snapshot
/// without judging — nobody spawned those entities mid-run).
#[test]
fn frame_zero_scene_is_never_an_entry() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.world_mut()
        .spawn((Beacon, Transform::from_xyz(0.0, 0.0, 0.0)));
    app.world_mut()
        .spawn((Projectile, Transform::from_xyz(0.0, 0.0, 0.0)));
    let report = run_scenario(&mut app, &scenario_with_invariant(SAFETY_INV)).unwrap();
    assert_eq!(report.status, PlaytestStatus::Pass);
    assert!(
        !report.violations.iter().any(|v| v.rule == "spawn_safety"),
        "the authored initial scene is not a spawn event: {:?}",
        report.violations
    );
}

/// The spawn-vs-component-addition distinction, both directions:
/// by default BOTH are judged (a converted enemy appearing inside
/// the zone is the same bug as a respawned one), and
/// `spawn_only: true` restricts the verdict to brand-new entities.
#[test]
fn component_addition_is_judged_by_default_and_filterable() {
    // Default: a pre-existing entity first ACQUIRING the marker
    // inside the zone fires, exactly like a spawn.
    let _slot = common::run_slot();
    let mut app = base_app();
    app.world_mut()
        .spawn((Beacon, Transform::from_xyz(0.0, 0.0, 0.0)));
    // A body parked on the beacon from frame 0 WITHOUT the marker:
    // frame-0 scene, no entry. It acquires the marker mid-run —
    // the component-addition transition.
    let body = app
        .world_mut()
        .spawn(Transform::from_xyz(0.0, 0.0, 0.0))
        .id();
    app.add_systems(
        Update,
        move |mut commands: Commands, mut fired: Local<bool>, mut frame: Local<u64>| {
            *frame += 1;
            // Frame 0 is the initial scene: the marker is acquired
            // from frame 1 on, a genuine mid-run transition.
            if *frame > 1 && !*fired {
                *fired = true;
                commands.entity(body).insert(Projectile);
            }
        },
    );
    let report = run_scenario(&mut app, &scenario_with_invariant(SAFETY_INV)).unwrap();
    assert_eq!(report.status, PlaytestStatus::Fail);
    assert!(
        report.violations.iter().any(|v| v.rule == "spawn_safety"),
        "component-addition inside the zone must fire by default"
    );

    // spawn_only: the SAME transition, filtered out — a
    // pre-existing pair acquiring the marker is not judged.
    let mut app = base_app();
    app.world_mut()
        .spawn((Beacon, Transform::from_xyz(0.0, 0.0, 0.0)));
    let body = app
        .world_mut()
        .spawn(Transform::from_xyz(0.0, 0.0, 0.0))
        .id();
    app.add_systems(
        Update,
        move |mut commands: Commands, mut fired: Local<bool>, mut frame: Local<u64>| {
            *frame += 1;
            if *frame > 1 && !*fired {
                *fired = true;
                commands.entity(body).insert(Projectile);
            }
        },
    );
    let spawn_only_inv = r#"
        "name": "safe_respawns",
        "rule": "spawn_safety",
        "target_selector": {"with_component": {"name": "Projectile"}},
        "min_distance_from": {"with_component": {"name": "Beacon"}},
        "min_distance": 0.5,
        "spawn_only": true"#;
    let report = run_scenario(&mut app, &scenario_with_invariant(spawn_only_inv)).unwrap();
    assert_eq!(report.status, PlaytestStatus::Pass);
    assert!(
        !report.violations.iter().any(|v| v.rule == "spawn_safety"),
        "spawn_only must exempt the component-addition entry: {:?}",
        report.violations
    );
}

/// Id-reuse soundness: bevy 0.20's entity allocator does NOT
/// bump the generation on despawn/recycle (its own allocator
/// tests assert recycled ids are bit-identical to the freed
/// ones), so a respawned slot re-enters observation with the
/// SAME (index, generation) bits as its dead predecessor. The
/// recorder must still judge that re-entry as a fresh event: the
/// dead pair vanished from the previous frame's snapshot, so
/// the diff must see the re-born pair as an unknown member —
/// never silenced by a stale "stayed a member" record from the
/// despawned predecessor.
#[test]
fn id_reuse_recycled_pairs_are_new_entries() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.world_mut()
        .spawn((Beacon, Transform::from_xyz(0.0, 0.0, 0.0)));
    // A projectile born SAFE at frame 1, despawned at frame 5,
    // its freed index recycled by a respawn INSIDE the zone
    // late in the run.
    app.add_systems(
        Update,
        |mut commands: Commands, q: Query<Entity, With<Projectile>>, mut frame: Local<u64>| {
            *frame += 1;
            match *frame {
                1 => {
                    commands.spawn((Projectile, Transform::from_xyz(0.0, 0.0, 5.0)));
                }
                5 => {
                    if let Ok(e) = q.single() {
                        commands.entity(e).despawn();
                    }
                }
                30 => {
                    // Fresh spawn inside the zone: bevy hands back
                    // the freed index with the same generation bits —
                    // the diff must judge the re-entry despite the
                    // recycled pair colliding with the dead one.
                    commands.spawn((Projectile, Transform::from_xyz(0.0, 0.0, 0.0)));
                }
                _ => {}
            }
        },
    );
    let report = run_scenario(&mut app, &scenario_with_invariant(SAFETY_INV)).unwrap();
    assert_eq!(report.status, PlaytestStatus::Fail);
    assert!(
        report.violations.iter().any(|v| v.rule == "spawn_safety"),
        "a recycled id re-entering inside the zone must be judged as a \
         fresh entry, not silenced by the dead predecessor's record"
    );
    // Soundness both directions: exactly ONE spawn_safety finding
    // over the whole run — the safe birth at frame 1 is judged
    // clean (outside the zone), and the recycled re-entry at
    // frame 30 fires once and only once (single-shot, no episode
    // machinery to multiply it).
    let ss: Vec<_> = report
        .violations
        .iter()
        .filter(|v| v.rule == "spawn_safety")
        .collect();
    assert_eq!(ss.len(), 1, "expected exactly one finding: {ss:?}");
}

/// Same-frame anchor participation: a beacon spawned in the SAME
/// frame as the entry is a participant in the judgment, not an
/// excuse — the anchor snapshot is taken at the END of the judging
/// frame, after both spawned.
#[test]
fn same_frame_anchor_participates() {
    let _slot = common::run_slot();
    let mut app = base_app();
    // Empty frame-0 scene: beacon AND projectile both arrive
    // together mid-run, on top of each other.
    app.add_systems(
        Update,
        |mut commands: Commands, mut fired: Local<bool>, mut frame: Local<u64>| {
            *frame += 1;
            // Frame 0 is the initial scene (never a judged entry):
            // the planted spawn fires from frame 1 on.
            if *frame > 1 && !*fired {
                *fired = true;
                commands.spawn((Beacon, Transform::from_xyz(0.0, 0.0, 0.0)));
                commands.spawn((Projectile, Transform::from_xyz(0.0, 0.0, 0.0)));
            }
        },
    );
    let report = run_scenario(&mut app, &scenario_with_invariant(SAFETY_INV)).unwrap();
    assert_eq!(report.status, PlaytestStatus::Fail);
    assert!(
        report.violations.iter().any(|v| v.rule == "spawn_safety"),
        "an anchor arriving with the entry must participate in the judgment"
    );
}

/// Vacuity: an invariant whose anchors NEVER resolve over the
/// whole run could never judge an entry — the same vacuous-proof
/// shape as pairwise's never-a-pair audit, reported as
/// `vacuous_invariant` (gating).
#[test]
fn never_any_anchor_is_vacuous() {
    let _slot = common::run_slot();
    let mut app = base_app();
    // Entries appear (the rule has work), but no Beacon ever exists.
    app.add_systems(
        Update,
        |mut commands: Commands, mut fired: Local<bool>, mut frame: Local<u64>| {
            *frame += 1;
            // Frame 0 is the initial scene (never a judged entry):
            // the planted spawn fires from frame 1 on.
            if *frame > 1 && !*fired {
                *fired = true;
                commands.spawn((Projectile, Transform::from_xyz(0.0, 0.0, 0.0)));
            }
        },
    );
    let report = run_scenario(&mut app, &scenario_with_invariant(SAFETY_INV)).unwrap();
    assert_eq!(report.status, PlaytestStatus::Fail);
    let v = report
        .violations
        .iter()
        .find(|v| v.rule == "vacuous_invariant" && v.target == "safe_respawns")
        .expect("never-an-anchor must audit as vacuous");
    assert!(
        v.detail.contains("never resolved an anchor"),
        "audit names the unresolvable side: {}",
        v.detail
    );
    assert_eq!(report.status, PlaytestStatus::Fail);
}
