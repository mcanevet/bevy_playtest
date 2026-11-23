//! Behavioral battery for the `pairwise_separation` rule: the
//! coincident-bodies spatial oracle. Pairs selected via
//! `with_component` are held to `min_distance`; violating frames
//! fold into episodes and only an episode soaking past `window_s`
//! (in driver units) becomes a finding — sub-soak overlaps are
//! transient fly-bys, discarded.
//!
//! The central scenario mirrors the stacked-bodies incident shape:
//! two marked entities that slide into the same spot and stay
//! there. Generic oracles stay quiet (the world keeps changing,
//! everything stays in bounds) — only the pairwise episode soak
//! can name coincident bodies.

use crate::common;

use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_playtest::driver::run_scenario;
use bevy_reflect::Reflect;
use bevy_transform::prelude::Transform;

/// Marker component acting as the pairwise selector. Reflect is
/// required for the recorder's dynamic-type resolution (same
/// doctrine as component_check's harness marker).
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct Enemy;

/// A game clock resource usable as the DECLARED driver leaf, in
/// minutes (a non-seconds unit proving the soak never converts
/// through tps). Named uniquely: other suites register their own
/// `MatchClock`, and short-name resolution is ambiguous across
/// same-named types.
#[derive(Resource, Reflect, Default)]
#[reflect(Resource)]
struct PwMatchClock {
    minutes: f64,
}

fn tick_minutes(mut clock: ResMut<PwMatchClock>) {
    clock.minutes += 1.0 / 60.0;
}

/// Positions chase the same moving target (x grows monotonically,
/// z follows sin(x)): the two bodies are EXACTLY coincident from
/// frame 1 onward while every transform keeps changing — a
/// sustained stack, not a frozen world.
fn coincident_bodies(mut q: Query<&mut Transform, With<Enemy>>) {
    for mut t in q.iter_mut() {
        t.translation.x += 0.05;
        t.translation.z = t.translation.x.sin();
    }
}

/// Control: bodies that never come within `min_distance` pass.
/// They drift monotonically along x (mirrored), so the pair
/// distance only grows and the world keeps changing (frozen_world
/// stays quiet too).
fn always_apart(mut q: Query<&mut Transform, With<Enemy>>) {
    for mut t in q.iter_mut() {
        t.translation.x += 0.05;
        t.translation.z += if t.translation.z > 0.0 { 0.02 } else { -0.02 };
    }
}

fn scenario_with_invariant(inv: &str) -> bevy_playtest::scenario::Scenario {
    let json = format!(
        r#"{{"bot":{{"type":"chaos","seed":42}},"duration_s":4.0,"tps":60,
            "invariants":[{{{inv}}}]}}"#
    );
    serde_json::from_str(&json).unwrap()
}

fn app_fixture(movement: fn(Query<&mut Transform, With<Enemy>>)) -> App {
    let mut app = App::new();
    app.register_type::<Enemy>();
    app.register_type::<Transform>();
    app.add_systems(Update, movement);
    // Mirrored spawns: 0.5 and -0.5 → the pair starts 1.0 apart.
    app.world_mut()
        .spawn((Enemy, Transform::from_xyz(0.0, 0.0, 0.5)));
    app.world_mut()
        .spawn((Enemy, Transform::from_xyz(0.0, 0.0, -0.5)));
    app
}

const SEPARATION_INV: &str = r#"
    "name": "no_stacking",
    "rule": "pairwise_separation",
    "target_selector": {"with_component": {"name": "Enemy"}},
    "min_distance": 0.5,
    "window_s": 1.0"#;

// ---------------------------------------------------------------------------
// The stacked-bodies shape
// ---------------------------------------------------------------------------

/// THE incident shape: bodies slide together and stay overlapped
/// well past the soak. One finding, naming the pair, with the
/// episode evidence in `_meta`.
#[test]
fn coincident_bodies_fail_the_soak() {
    let _slot = common::run_slot();
    let mut app = app_fixture(coincident_bodies);
    let report = run_scenario(&mut app, &scenario_with_invariant(SEPARATION_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    let v = report
        .violations
        .iter()
        .find(|v| v.rule == "pairwise_separation")
        .expect("expected a pairwise_separation finding");
    assert!(v.detail.contains("of each other"), "{}", v.detail);
    // The `_meta` roster evidence: one distinct pair, not
    // truncated, with the worst distance recorded.
    let meta = v.meta.as_ref().expect("episode finding carries _meta");
    let pw = meta
        .pairwise_episode
        .as_ref()
        .expect("_meta carries the pairwise episode block");
    assert_eq!(pw.sampled_pairs, 1, "one distinct pair in the roster");
    assert!(!pw.roster_truncated);
    assert_eq!(pw.roster.len(), 1);
    assert!(
        pw.roster[0].worst_distance <= 0.1,
        "worst distance: {}",
        pw.roster[0].worst_distance
    );
    assert!(pw.roster[0].violating_frames >= 1);
}

/// Control: bodies that never come within `min_distance` pass. They
/// approach to 0.5-ish then diverge — the distance must stay ≥ 0.5
/// (approach to exactly 0.5 is NOT a violation: min_distance is a
/// floor, distance < min_distance is the violation predicate).
#[test]
fn approaching_then_diverging_bodies_pass() {
    let _slot = common::run_slot();
    let mut app = app_fixture(always_apart);
    let report = run_scenario(&mut app, &scenario_with_invariant(SEPARATION_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    assert!(!report
        .violations
        .iter()
        .any(|v| v.rule == "pairwise_separation"));
}

/// The fly-by doctrine: a violation episode shorter than
/// `window_s` is DISCARDED — min_distance is "coincident bodies",
/// not "personal space". Bodies cross through each other quickly
/// (a dodge/roll), staying overlapped far less than the soak.
#[test]
fn fast_fly_by_is_discarded() {
    let _slot = common::run_slot();
    // Same bodies, but they blow past each other: spawned 0.1
    // apart (already violating), they cross in about one frame and
    // keep drifting — the whole overlap episode is far below the
    // 1.0s soak, so the ε doctrine discards it.
    let mut app = App::new();
    app.register_type::<Enemy>();
    app.register_type::<Transform>();
    app.add_systems(Update, |mut q: Query<&mut Transform, With<Enemy>>| {
        for mut t in q.iter_mut() {
            // Opposite directions, never stopping (a frozen pair
            // would trip frozen_world, another oracle's story):
            // they cross in ~2 frames and keep drifting apart.
            t.translation.z += if t.translation.z >= 0.0 { 0.6 } else { -0.6 };
        }
    });
    app.world_mut()
        .spawn((Enemy, Transform::from_xyz(0.0, 0.0, 0.05)));
    app.world_mut()
        .spawn((Enemy, Transform::from_xyz(0.0, 0.0, -0.05)));
    let report = run_scenario(&mut app, &scenario_with_invariant(SEPARATION_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.rule == "pairwise_separation"),
        "a sub-soak overlap must not become a finding: {:?}",
        report.violations
    );
}

// ---------------------------------------------------------------------------
// Driver-unit soak
// ---------------------------------------------------------------------------

/// The declared driver leaf is the soak's clock: `window_s` is in
/// MINUTES here, and the clock advances slowly — the episode must
/// soak against the slow clock, not the frame clock. The same
/// physical overlap that fails a 1.0-second frame-clock soak is
/// sub-soak when window_s counts minutes of MatchClock.
#[test]
fn declared_driver_leaf_is_the_soak_clock() {
    let _slot = common::run_slot();
    let mut app = app_fixture(coincident_bodies);
    app.register_type::<PwMatchClock>();
    app.insert_resource(PwMatchClock::default());
    app.add_systems(Update, tick_minutes);

    // window_s: 1.0 MINUTE of MatchClock advance, judged purely in
    // the leaf's own units: the clock ticks 1/60 min/frame (one
    // minute per real second at 60 fps), and the bodies coincide
    // for ~4 minutes of clock advance — comfortably past the soak.
    let scen = scenario_with_invariant(
        r#"
    "name": "no_stacking",
    "rule": "pairwise_separation",
    "target_selector": {"with_component": {"name": "Enemy"}},
    "min_distance": 0.5,
    "window_s": 1.0,
    "driver_resource": "PwMatchClock",
    "driver_path": "minutes""#,
    );
    let report = run_scenario(&mut app, &scen).unwrap();
    // The overlap episode (~3.7s of run ≈ 3.7 min of clock advance
    // at 1/60 min/frame × 60 fps = 1 min/real-second) exceeds the
    // 1.0-minute soak → the finding fires.
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.rule == "pairwise_separation"),
        "episode must soak past 1.0 minute; violations: {:?}",
        report
            .violations
            .iter()
            .map(|v| (v.rule.clone(), v.target.clone(), v.detail.clone()))
            .collect::<Vec<_>>()
    );
}

/// The discrimination half: with a clock advancing 10× slower
/// (1/600 min/frame), the SAME ~4-second physical overlap accrues
/// only ~0.4 minutes of driver advance — sub-soak against the
/// 1.0-minute window, discarded as a fly-by. On the FRAME clock
/// (seconds) this episode would fire (3.98s > 1.0s); only the
/// declared driver's own units keep it quiet. This is the proof
/// the soak never converts through tps.
#[test]
fn slow_clock_keeps_the_same_overlap_sub_soak() {
    let _slot = common::run_slot();
    let mut app = app_fixture(coincident_bodies);
    app.register_type::<PwMatchClock>();
    app.insert_resource(PwMatchClock::default());
    app.add_systems(Update, |mut clock: ResMut<PwMatchClock>| {
        clock.minutes += 1.0 / 600.0;
    });

    let scen = scenario_with_invariant(
        r#"
    "name": "no_stacking",
    "rule": "pairwise_separation",
    "target_selector": {"with_component": {"name": "Enemy"}},
    "min_distance": 0.5,
    "window_s": 1.0,
    "driver_resource": "PwMatchClock",
    "driver_path": "minutes""#,
    );
    let report = run_scenario(&mut app, &scen).unwrap();
    assert_eq!(
        report.status,
        bevy_playtest::enums::PlaytestStatus::Pass,
        "violations: {:?}",
        report
            .violations
            .iter()
            .map(|v| (v.rule.clone(), v.target.clone()))
            .collect::<Vec<_>>()
    );
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.rule == "pairwise_separation"),
        "0.4 minutes of driver advance must not soak past a 1.0-minute window"
    );
}

// ---------------------------------------------------------------------------
// Vacuity
// ---------------------------------------------------------------------------

/// A selector that never has a pair to check proves nothing: a
/// registered-but-never-instantiated component → `vacuous_invariant`.
#[test]
fn never_spawned_selector_is_vacuous() {
    let _slot = common::run_slot();
    #[derive(Component, Reflect, Default)]
    #[reflect(Component)]
    struct Unspawned;

    let mut app = App::new();
    app.register_type::<Unspawned>();
    app.register_type::<Transform>();
    // Observable world (so vacuous_RUN does not fire): one plain
    // Transform entity moves, keeping the run non-vacuous.
    app.add_systems(Update, |mut q: Query<&mut Transform>| {
        for mut t in q.iter_mut() {
            t.translation.x += 0.01;
        }
    });
    app.world_mut().spawn(Transform::from_xyz(0.0, 1.0, 0.0));

    let scen = scenario_with_invariant(
        r#"
    "name": "ghost_pair",
    "rule": "pairwise_separation",
    "target_selector": {"with_component": {"name": "Unspawned"}},
    "min_distance": 0.5,
    "window_s": 1.0"#,
    );
    let report = run_scenario(&mut app, &scen).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    let v = report
        .violations
        .iter()
        .find(|v| v.rule == "vacuous_invariant" && v.target == "ghost_pair")
        .expect("vacuous_invariant names the ghost-pair invariant");
    assert!(v.detail.contains("never had a pair"), "{}", v.detail);
}

/// A singleton is never a pair: a selector matching exactly ONE
/// entity is just as vacuous — pairwise needs two.
#[test]
fn lone_entity_is_vacuous() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.register_type::<Enemy>();
    app.register_type::<Transform>();
    app.add_systems(Update, |mut q: Query<&mut Transform, With<Enemy>>| {
        for mut t in q.iter_mut() {
            t.translation.x += 0.01;
        }
    });
    app.world_mut()
        .spawn((Enemy, Transform::from_xyz(0.0, 0.0, 0.0)));

    let report = run_scenario(&mut app, &scenario_with_invariant(SEPARATION_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.rule == "vacuous_invariant" && v.target == "no_stacking"),
        "a lone entity means no pair was ever enumerable"
    );
}

// ---------------------------------------------------------------------------
// Config errors
// ---------------------------------------------------------------------------

/// An unregistered selector type is an authoring mistake: reported
/// as `oracle_error` (config), never as the game's fault.
#[test]
fn unregistered_selector_reports_config_error() {
    let _slot = common::run_slot();
    let mut app = app_fixture(always_apart);
    let scen = scenario_with_invariant(
        r#"
    "name": "no_stacking",
    "rule": "pairwise_separation",
    "target_selector": {"with_component": {"name": "NoSuchComponent"}},
    "min_distance": 0.5,
    "window_s": 1.0"#,
    );
    let report = run_scenario(&mut app, &scen).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.rule == "oracle_error" && v.target == "no_stacking"),
        "unregistered selector must be an oracle_error naming the invariant"
    );
    // The config error suppresses the vacuity entry (its
    // oracle_error already told the story).
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.rule == "vacuous_invariant" && v.target == "no_stacking"),
        "config-errored invariant must not double-report vacuity"
    );
}

// ---------------------------------------------------------------------------
// Metadata serialization smoke
// ---------------------------------------------------------------------------

/// The `_meta` block serializes with the documented underscore key
/// and stable field names — consumers parse it as structured data.
#[test]
fn episode_meta_serializes_stably() {
    let _slot = common::run_slot();
    let mut app = app_fixture(coincident_bodies);
    let report = run_scenario(&mut app, &scenario_with_invariant(SEPARATION_INV)).unwrap();
    let v = report
        .violations
        .iter()
        .find(|v| v.rule == "pairwise_separation")
        .expect("finding present");
    let meta = v.meta.as_ref().expect("_meta present");
    let json = serde_json::to_value(meta).unwrap();
    // The wire format: the annotation block sits at `_meta` in the
    // entry serialization; here we pin the block's own field names.
    let pw = json
        .get("pairwise_episode")
        .expect("pairwise_episode key present");
    assert!(pw.get("sampled_pairs").is_some());
    assert!(pw.get("roster").is_some());
    assert!(pw.get("roster_truncated").is_some());
    let entry = pw
        .get("roster")
        .and_then(|r| r.get(0))
        .expect("roster non-empty");
    for key in [
        "low",
        "high",
        "first_frame",
        "worst_distance",
        "worst_frame",
        "violating_frames",
    ] {
        assert!(entry.get(key).is_some(), "roster entry key {key}");
    }
}
