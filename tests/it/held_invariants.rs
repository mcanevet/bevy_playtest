//! `held_invariants` report section (the passing-invariant trace): the
//! pass-side evidence trace — one compact object per PASSING
//! resource_check / stagnant_resource / component_check /
//! nodes_in_bounds invariant.
//!
//! Fixture doctrine mirrors `resource_check.rs`: frame-counter time
//! (60 tps ⇒ dt = 1/60), the scenario JSON harness, and
//! `common::run_slot()` around every `run_scenario` call.
//!
//! Suite separated from `resource_check.rs` on purpose: this is a
//! NEW report surface, own file per the work-split (no collisions
//! with other agents' edits to that suite).

use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_playtest::driver::run_scenario;
use bevy_playtest::held::HeldInvariant;
use bevy_reflect::Reflect;
use bevy_transform::prelude::{GlobalTransform, Transform, TransformPlugin, TransformSystems};

use crate::common;

const DT: f32 = 1.0 / 60.0;

// ---------------------------------------------------------------------------
// Game-side resources
// ---------------------------------------------------------------------------

#[derive(Resource, Reflect, Default)]
#[reflect(Resource)]
struct HeldEconomy {
    coins: i64,
}

#[derive(Resource, Reflect, Default)]
#[reflect(Resource)]
struct HeldClock {
    elapsed_s: f32,
}

fn tick_clock(mut clock: ResMut<HeldClock>) {
    clock.elapsed_s += DT;
}

fn grow_economy(mut eco: ResMut<HeldEconomy>) {
    eco.coins += 1;
}

fn wander(mut q: Query<&mut Transform>) {
    for mut t in q.iter_mut() {
        t.translation.x = t.translation.x.sin() + 0.01;
    }
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn scenario_with_invariants(invariants: &str) -> bevy_playtest::scenario::Scenario {
    let json = format!(
        r#"{{"bot":{{"type":"chaos","seed":42}},"duration_s":4.0,"tps":60,
            "invariants":[{{{invariants}}}]}}"#
    );
    serde_json::from_str(&json).unwrap()
}

fn economy_app(economy: fn(ResMut<HeldEconomy>)) -> App {
    let mut app = App::new();
    app.register_type::<HeldEconomy>();
    app.register_type::<HeldClock>();
    app.register_type::<Transform>();
    app.insert_resource(HeldEconomy::default());
    app.insert_resource(HeldClock::default());
    app.add_systems(Update, (economy, tick_clock, wander));
    app.world_mut().spawn(Transform::from_xyz(0.0, 1.0, 0.0));
    app
}

const COINS_NONNEGATIVE: &str = r#"
    "name": "coins_nonnegative",
    "rule": "resource_check",
    "resource": "HeldEconomy",
    "path": "coins",
    "check": "ge",
    "value": 0"#;

const COINS_EVENTUALLY_RICH: &str = r#"
    "name": "coins_eventually_rich",
    "rule": "resource_check",
    "resource": "HeldEconomy",
    "path": "coins",
    "check": "ge",
    "value": 100,
    "eventually_s": 3.0"#;

// ---------------------------------------------------------------------------
// resource_check: always-mode fields
// ---------------------------------------------------------------------------

/// A passing always-mode `resource_check` carries its extrema and the
/// margin frame: the "how close did it get" evidence a passing run
/// previously discarded.
#[test]
fn always_check_reports_extrema_and_margin_frame() {
    let _slot = common::run_slot();
    let mut app = economy_app(grow_economy);
    let report = run_scenario(&mut app, &scenario_with_invariants(COINS_NONNEGATIVE)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    let held = report
        .held_invariants
        .iter()
        .find(|h| h.name == "coins_nonnegative")
        .expect("held entry for passing invariant");
    assert_eq!(held.rule, "resource_check");
    assert_eq!(held.target, "HeldEconomy.coins");
    assert_eq!(held.predicate.as_deref(), Some("ge 0"));
    // Recorders observe post-update: by the first observation the
    // economy has already grown once, so the minimum sampled value
    // is 1 (not the pre-run 0). The extrema describe OBSERVED
    // frames — the frames the audit would call observable.
    assert_eq!(held.observed_min, Some(1.0));
    assert!(held.observed_max.unwrap() > held.observed_min.unwrap());
    // Observed every simulated frame (resource present from frame 0).
    assert_eq!(held.frames_matched, report.frame_count);
    // ge 0: worst = the MIN (closest to violating) = the first
    // observed frame of a monotonically growing economy.
    assert_eq!(held.worst_frame, Some(0));
    assert!(held.met_at_frame.is_none());
    assert!(held.deadline_frame.is_none());
    assert!(held.episodes_fired.is_none());
    assert!(held.driver_advanced.is_none());
    assert!(held.driver.is_none());
}

/// A FAILING invariant appears in violations, never in
/// held_invariants — the two sections partition by verdict.
#[test]
fn failing_invariant_is_not_held() {
    let _slot = common::run_slot();
    let mut app = economy_app(grow_economy);
    // ge 1000000 can never hold: always-mode violation.
    let report = run_scenario(
        &mut app,
        &scenario_with_invariants(
            r#""name": "impossible", "rule": "resource_check",
               "resource": "HeldEconomy", "path": "coins",
               "check": "ge", "value": 1000000"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    assert!(report.held_invariants.is_empty());
}

/// A config error (unknown resource) excludes the invariant from the
/// held section: an oracle_error already failed the run; a "held"
/// entry would be evidence for a check that never judged anything.
#[test]
fn config_error_excludes_held_entry() {
    let _slot = common::run_slot();
    let mut app = economy_app(grow_economy);
    let report = run_scenario(
        &mut app,
        &scenario_with_invariants(
            r#""name": "typo", "rule": "resource_check",
               "resource": "NoSuchResource", "path": "coins",
               "check": "ge", "value": 0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    assert!(report.held_invariants.is_empty());
}

/// A vacuous invariant (resource never inserted) carries a
/// vacuous_invariant finding and gets NO held entry: it proved
/// nothing, and the finding already fails the run.
#[test]
fn vacuous_invariant_gets_no_held_entry() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.register_type::<HeldEconomy>();
    app.register_type::<HeldClock>();
    app.register_type::<Transform>();
    // The CLOCK runs (so the run itself is healthy); only
    // HeldEconomy is missing: the check's resource never arrives.
    app.insert_resource(HeldClock::default());
    app.add_systems(Update, (tick_clock, wander));
    app.world_mut().spawn(Transform::from_xyz(0.0, 1.0, 0.0));
    let report = run_scenario(&mut app, &scenario_with_invariants(COINS_NONNEGATIVE)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    assert!(report.held_invariants.is_empty());
}

/// Eventually-mode: met_at_frame + deadline_frame are reported, with
/// the audited ceiling formula (eventually_s × tps, ceiled). A late
/// satisfier shows its first-held frame so a pass at frame 299 of
/// 300 is finally distinguishable from a pass at frame 3.
///
/// Note: the run's overall status can still be Fail (liveness
/// oracles against a bare test app); the held section reports
/// EVIDENCE for the passing invariant, independent of the run-wide
/// verdict — the deadline demotes only THIS invariant's entry.
#[test]
fn eventually_check_reports_met_and_deadline_frames() {
    let _slot = common::run_slot();
    let mut app = economy_app(grow_economy);
    let report = run_scenario(&mut app, &scenario_with_invariants(COINS_EVENTUALLY_RICH)).unwrap();
    let held = report
        .held_invariants
        .iter()
        .find(|h| h.name == "coins_eventually_rich")
        .expect("held entry for passing eventually check");
    assert_eq!(held.deadline_frame, Some(180));
    let met = held.met_at_frame.expect("predicate held in time");
    assert!(
        met <= 180,
        "met_at_frame {met} exceeds the 180-frame deadline"
    );
    // The observed max should be the final grown value (≥ threshold).
    assert!(held.observed_max.unwrap() >= 100.0);
    assert_eq!(held.frames_matched, report.frame_count);
    // ge: worst = min sample = 0.0 at frame 0 for a growing economy.
    assert_eq!(held.worst_frame, Some(0));
}

/// The wire round-trip: the section deserializes with additive-field
/// discipline (older-reader simulation — absent section parses as
/// empty; present section parses field-by-field).
#[test]
fn held_section_round_trips_through_serde() {
    let entry = HeldInvariant {
        name: "demo".to_string(),
        rule: "resource_check".to_string(),
        target: "HeldEconomy.coins".to_string(),
        predicate: Some("ge 0".to_string()),
        frames_matched: 1200,
        observed_min: Some(25.0),
        observed_max: Some(132.0),
        worst_frame: Some(480),
        met_at_frame: None,
        deadline_frame: None,
        episodes_fired: None,
        driver_advanced: None,
        driver: None,
        eventually_s: None,
        window_s: None,
        entity_judgments: None,
        peak_entities: None,
        peak_entities_frame: None,
        nearest_margin: None,
        nearest_margin_frame: None,
        episodes_observed: None,
        worst_episode_span: None,
        worst_episode_frame: None,
        pair_judgments: None,
        entries_clean: None,
    };
    let json = serde_json::to_string(&entry).unwrap();
    let back: HeldInvariant = serde_json::from_str(&json).unwrap();
    assert_eq!(back.name, entry.name);
    assert_eq!(back.frames_matched, 1200);
    assert_eq!(back.worst_frame, entry.worst_frame);
    // Old-report compatibility: a REAL pre-section report (built by
    // the current `crashed_empty`, which omits the section entirely
    // thanks to skip_serializing_if) round-trips into this struct —
    // the additive-field discipline that keeps `schema_version` at 1.
    let old = bevy_playtest::state::PlaytestReport::crashed_empty();
    let json = serde_json::to_string(&old).unwrap();
    let back: bevy_playtest::state::PlaytestReport = serde_json::from_str(&json).unwrap();
    assert!(back.held_invariants.is_empty());
    assert_eq!(back.schema_version, 1);
    // Component-tier wire contract: the component fields' wire names
    // and types round-trip exactly (the entry shapes finding 1's
    // all-selector fix emits depend on).
    let component_entry = HeldInvariant {
        name: "marker".to_string(),
        rule: "component_check".to_string(),
        target: "HeldMarker.translation.z".to_string(),
        predicate: Some("ge 0".to_string()),
        frames_matched: 240,
        observed_min: None,
        observed_max: None,
        worst_frame: None,
        met_at_frame: Some(100),
        deadline_frame: Some(180),
        episodes_fired: None,
        driver_advanced: None,
        driver: None,
        eventually_s: Some(3.0),
        window_s: None,
        entity_judgments: Some(240),
        peak_entities: Some(4),
        peak_entities_frame: Some(57),
        nearest_margin: Some(0.0107),
        nearest_margin_frame: Some(12),
        episodes_observed: None,
        worst_episode_span: None,
        worst_episode_frame: None,
        pair_judgments: None,
        entries_clean: None,
    };
    let json = serde_json::to_string(&component_entry).unwrap();
    let back: HeldInvariant = serde_json::from_str(&json).unwrap();
    assert_eq!(back.entity_judgments, Some(240));
    assert_eq!(back.peak_entities, Some(4));
    assert_eq!(back.peak_entities_frame, Some(57));
    assert!((back.nearest_margin.unwrap() - 0.0107).abs() < 1e-9);
    assert_eq!(back.nearest_margin_frame, Some(12));
    assert_eq!(back.met_at_frame, Some(100));
    assert_eq!(back.deadline_frame, Some(180));
    // Episode-tier wire contract: the near-miss fields' wire names
    // and types round-trip exactly (a declared-driver scenario's
    // span is a bare number — no unit suffix to lose).
    let episode_entry = HeldInvariant {
        name: "no_stacking".to_string(),
        rule: "pairwise_separation".to_string(),
        target: "Enemy".to_string(),
        predicate: Some("ge 0.5".to_string()),
        frames_matched: 237,
        observed_min: None,
        observed_max: None,
        worst_frame: None,
        met_at_frame: None,
        deadline_frame: None,
        episodes_fired: None,
        driver_advanced: None,
        driver: None,
        eventually_s: None,
        window_s: Some(1.0),
        entity_judgments: None,
        peak_entities: None,
        peak_entities_frame: None,
        nearest_margin: None,
        nearest_margin_frame: None,
        episodes_observed: Some(2),
        worst_episode_span: Some(0.317),
        worst_episode_frame: Some(10),
        pair_judgments: Some(282_015),
        entries_clean: None,
    };
    let json = serde_json::to_string(&episode_entry).unwrap();
    let back: HeldInvariant = serde_json::from_str(&json).unwrap();
    assert_eq!(back.episodes_observed, Some(2));
    assert_eq!(back.worst_episode_span, Some(0.317));
    assert_eq!(back.worst_episode_frame, Some(10));
    assert_eq!(back.pair_judgments, Some(282_015));
    assert_eq!(back.window_s, Some(1.0));
    assert_eq!(back.predicate.as_deref(), Some("ge 0.5"));
}

// ---------------------------------------------------------------------------
// stagnant_resource fields
// ---------------------------------------------------------------------------

/// A passing stagnant_resource carries its episode + driver-advance
/// evidence: episodes_fired: 0 plus the run-total clock advance.
#[test]
fn stagnant_check_reports_driver_advance_total() {
    let _slot = common::run_slot();
    let mut app = economy_app(grow_economy);
    // economy.churns each frame, clock ticks: healthy.
    let report = run_scenario(
        &mut app,
        &scenario_with_invariants(
            r#""name": "coins_move", "rule": "stagnant_resource",
               "resource": "HeldEconomy", "path": "coins",
               "driver_resource": "HeldClock", "driver_path": "elapsed_s",
               "window_s": 2.0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    let held = report
        .held_invariants
        .iter()
        .find(|h| h.name == "coins_move")
        .expect("held entry for passing stagnant check");
    assert_eq!(held.rule, "stagnant_resource");
    assert_eq!(held.episodes_fired, Some(0));
    let advanced = held.driver_advanced.expect("run-total driver advance");
    // 4 s duration at 60 tps: the clock advanced ~4 s minus one
    // frame's worth of dt at each boundary; approximate match only.
    let expected = report.frame_count as f64 * DT as f64;
    assert!(
        (advanced - expected).abs() < DT as f64 * 2.0,
        "advanced={advanced} expected≈{expected}"
    );
    assert_eq!(held.driver.as_deref(), Some("HeldClock.elapsed_s"));
    // Stagnant checks judge RATE, not value margins: no extrema.
    assert!(held.observed_min.is_none());
    assert!(held.observed_max.is_none());
    assert!(held.worst_frame.is_none());
    assert!(held.predicate.is_none());
}

/// A failing (fired-episode) stagnant check lands in violations, not
/// held — partition by verdict holds for stagnant too. The held
/// section's exclusion test relies on the post-audit snapshot.
#[test]
fn fired_stagnant_check_is_not_held() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.register_type::<HeldEconomy>();
    app.register_type::<HeldClock>();
    app.register_type::<Transform>();
    app.insert_resource(HeldEconomy::default());
    app.insert_resource(HeldClock::default());
    // Economy FROZEN while the clock runs: the livelock shape.
    app.add_systems(Update, (tick_clock, wander));
    app.world_mut().spawn(Transform::from_xyz(0.0, 1.0, 0.0));
    let report = run_scenario(
        &mut app,
        &scenario_with_invariants(
            r#""name": "coins_frozen", "rule": "stagnant_resource",
               "resource": "HeldEconomy", "path": "coins",
               "driver_resource": "HeldClock", "driver_path": "elapsed_s",
               "window_s": 1.0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.rule == "stagnant_resource" && v.target == "coins_frozen"),
        "stagnant violation present"
    );
    assert!(report.held_invariants.is_empty());
}

// ---------------------------------------------------------------------------
// Component-tier fixtures
// ---------------------------------------------------------------------------

/// Selector marker for the component tier (mirrors `component_check.rs`:
/// a reflected marker so the judge resolves entities by type name).
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct HeldMarker;

/// Slow forward drift: z advances 0.01/frame from 1.0. Under a
/// `ge 0` always-check the value holds every frame and the FIRST
/// observed frame is already the worst (minimum) sample.
fn drift_forward(mut q: Query<&mut Transform, With<HeldMarker>>) {
    for mut t in q.iter_mut() {
        t.translation.z += 0.01;
    }
}

fn marker_app(movement: fn(Query<&mut Transform, With<HeldMarker>>)) -> App {
    let mut app = App::new();
    app.register_type::<HeldMarker>();
    app.register_type::<Transform>();
    app.add_systems(Update, movement);
    app.world_mut()
        .spawn((HeldMarker, Transform::from_xyz(0.0, 1.0, 1.0)));
    app
}

const MARKER_Z_NONNEGATIVE: &str = r#"
    "name": "marker_z_nonnegative",
    "rule": "component_check",
    "target_selector": {"with_component": {"name": "HeldMarker"}},
    "path": "translation.z",
    "check": "ge",
    "value": 0.0"#;

// ---------------------------------------------------------------------------
// component_check: held fields (workload + nearest margin)
// ---------------------------------------------------------------------------

/// A passing always-mode `component_check` names what was judged:
/// entity-judgment totals, peak scope, and the nearest-margin
/// sample — the component tier's analogue of the resource tier's
/// extrema-and-margin-frame story.
#[test]
fn component_check_reports_workload_and_nearest_margin() {
    let _slot = common::run_slot();
    let mut app = marker_app(drift_forward);
    let report = run_scenario(&mut app, &scenario_with_invariants(MARKER_Z_NONNEGATIVE)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    let held = report
        .held_invariants
        .iter()
        .find(|h| h.name == "marker_z_nonnegative")
        .expect("held entry for passing component_check");
    assert_eq!(held.rule, "component_check");
    assert_eq!(held.target, "HeldMarker.translation.z");
    assert_eq!(held.predicate.as_deref(), Some("ge 0"));
    // One entity judged every frame: frames_matched == the run's
    // frame count, every frame was a judged frame, and the run-total
    // judgment count equals frames × entities.
    assert_eq!(held.frames_matched, report.frame_count);
    assert_eq!(held.entity_judgments, Some(report.frame_count));
    assert_eq!(held.peak_entities, Some(1));
    assert_eq!(held.peak_entities_frame, Some(0));
    // Resource-tier fields stay absent: no single leaf exists to
    // take extrema over (each entity has its own).
    assert!(held.observed_min.is_none());
    assert!(held.observed_max.is_none());
    assert!(held.worst_frame.is_none());
    // z starts at 1.0 and grows: the closest any judged entity came
    // to breaching `ge 0` is the FIRST observed frame's sample
    // (post-drift 1.0 + 0.01; later frames only get further).
    // Epsilon sized for f32 accumulation, comfortably above the
    // representational error (~1e-7 here) so drift-constant tweaks
    // don't silently exhaust the headroom.
    let eps = 1e-4;
    assert!((held.nearest_margin.unwrap() - 1.01).abs() < eps);
    assert_eq!(held.nearest_margin_frame, Some(0));
    // Stagnant-tier fields stay absent too.
    assert!(held.episodes_fired.is_none());
    assert!(held.driver_advanced.is_none());
    assert!(held.driver.is_none());
}

/// A FAILING component_check lands in violations, never in the
/// held section — the partition-by-verdict doctrine holds for the
/// component tier exactly as for the resource tier.
#[test]
fn failing_component_check_is_not_held() {
    let _slot = common::run_slot();
    let mut app = marker_app(drift_forward);
    let report = run_scenario(
        &mut app,
        &scenario_with_invariants(
            r#""name": "impossible", "rule": "component_check",
               "target_selector": {"with_component": {"name": "HeldMarker"}},
               "path": "translation.z", "check": "lt", "value": -100.0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    assert!(report.held_invariants.is_empty());
    assert!(report
        .violations
        .iter()
        .any(|v| v.rule == "component_check" && v.target == "impossible"));
}

/// An unknown selector component is a config error: the run fails
/// with an oracle_error and the invariant never reaches the held
/// section — a held entry would be evidence for a check that never
/// judged anything.
#[test]
fn component_check_config_error_excludes_held_entry() {
    let _slot = common::run_slot();
    let mut app = marker_app(drift_forward);
    let report = run_scenario(
        &mut app,
        &scenario_with_invariants(
            r#""name": "typo", "rule": "component_check",
               "target_selector": {"with_component": {"name": "NoSuchComponent"}},
               "path": "translation.z", "check": "ge", "value": 0.0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    assert!(report.held_invariants.is_empty());
}

/// A vacuous component_check (selector matches nothing all run)
/// carries a vacuous_invariant finding and no held entry. The shape
/// needs a REGISTERED-but-unspawned component: an unregistered name
/// is a config error instead.
#[test]
fn vacuous_component_check_gets_no_held_entry() {
    let _slot = common::run_slot();
    #[derive(Component, Reflect, Default)]
    #[reflect(Component)]
    struct SpawnlessMarker;
    let mut app = marker_app(drift_forward);
    app.register_type::<SpawnlessMarker>();
    let report = run_scenario(
        &mut app,
        &scenario_with_invariants(
            r#""name": "ghost_scope", "rule": "component_check",
               "target_selector": {"with_component": {"name": "SpawnlessMarker"}},
               "path": "translation.z", "check": "ge", "value": 0.0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.rule == "vacuous_invariant" && v.target == "ghost_scope"),
        "expected a vacuous_invariant finding among {:?}",
        report
            .violations
            .iter()
            .map(|v| (v.rule.clone(), v.target.clone()))
            .collect::<Vec<_>>()
    );
    assert!(!report
        .held_invariants
        .iter()
        .any(|h| h.name == "ghost_scope"));
}

// ---------------------------------------------------------------------------
// nodes_in_bounds: held fields (workload, no margin)
// ---------------------------------------------------------------------------

/// A passing always-mode `nodes_in_bounds` (with_component selector)
/// reports the judge's workload — entity judgments, peak scope, its
/// frame — but no nearest margin: bounds judge containment in a
/// box, not a thresholded leaf.
#[test]
fn nodes_in_bounds_reports_workload_without_margin() {
    let _slot = common::run_slot();
    let mut app = marker_app(drift_forward);
    let report = run_scenario(
        &mut app,
        &scenario_with_invariants(
            r#""name": "markers_stay_close", "rule": "nodes_in_bounds",
               "target_selector": {"with_component": {"name": "HeldMarker"}},
               "min_z": -10.0, "max_z": 10.0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    let held = report
        .held_invariants
        .iter()
        .find(|h| h.name == "markers_stay_close")
        .expect("held entry for passing nodes_in_bounds");
    assert_eq!(held.rule, "nodes_in_bounds");
    // The selector component is the judged scope, not a leaf.
    assert_eq!(held.target, "HeldMarker");
    assert_eq!(held.frames_matched, report.frame_count);
    assert_eq!(held.entity_judgments, Some(report.frame_count));
    assert_eq!(held.peak_entities, Some(1));
    assert_eq!(held.peak_entities_frame, Some(0));
    // No thresholds judged ⇒ no predicate, no margin.
    assert!(held.predicate.is_none());
    assert!(held.nearest_margin.is_none());
    assert!(held.nearest_margin_frame.is_none());
    assert!(held.observed_min.is_none());
    assert!(held.episodes_fired.is_none());
}

/// The all-selector `nodes_in_bounds` shape: no writer ever inserts
/// it into `WatchedInvariants.matched` (the scheduled judge cannot
/// touch that bookkeeping), so the collector MUST admit it on
/// judged-frames evidence alone — `frames_judged > 0` is its sole
/// vacuity test. Regression pin: this entry was previously computed
/// by the judge, accumulated by the fold, then silently discarded by
/// the collector's matched-set guard (evidence-less pass — the exact
/// hole this slice closes).
#[test]
fn all_selector_nodes_in_bounds_gets_held_entry() {
    let _slot = common::run_slot();
    let mut app = marker_app(drift_forward);
    let report = run_scenario(
        &mut app,
        &scenario_with_invariants(
            r#""name": "everything_stays_close", "rule": "nodes_in_bounds",
               "min_z": -10.0, "max_z": 10.0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    let held = report
        .held_invariants
        .iter()
        .find(|h| h.name == "everything_stays_close")
        .expect("held entry for passing all-selector nodes_in_bounds");
    assert_eq!(held.rule, "nodes_in_bounds");
    // Documented leaf target for the all-selector: the whole world
    // is the judged scope.
    assert_eq!(held.target, "all");
    assert_eq!(held.frames_matched, report.frame_count);
    // One judged entity (the marker) every frame.
    assert_eq!(held.entity_judgments, Some(report.frame_count));
    assert_eq!(held.peak_entities, Some(1));
    assert!(held.nearest_margin.is_none());
    assert!(held.predicate.is_none());
}

// ---------------------------------------------------------------------------
// Sizing: 20 invariants ≈ 3 KB (the documented constant-size claim)
// ---------------------------------------------------------------------------

/// The documented sizing claim, made executable: 20 serialized
/// held-invariant objects stay in the low-KB range — the reason the
/// section ships default-on with no opt-in tier.
#[test]
fn twenty_held_invariants_serialize_under_4kb() {
    let mut entries = Vec::new();
    for i in 0..20 {
        entries.push(HeldInvariant {
            name: format!("lumen_never_negative_{i}"),
            rule: "resource_check".to_string(),
            target: "demo_game::economy::Economy.lumen".to_string(),
            predicate: Some("ge 0".to_string()),
            frames_matched: 1200,
            observed_min: Some(25.0),
            observed_max: Some(132.0),
            worst_frame: Some(480),
            met_at_frame: None,
            deadline_frame: None,
            episodes_fired: None,
            driver_advanced: None,
            driver: None,
            eventually_s: None,
            window_s: None,
            entity_judgments: None,
            peak_entities: None,
            peak_entities_frame: None,
            nearest_margin: None,
            nearest_margin_frame: None,
            episodes_observed: None,
            worst_episode_span: None,
            worst_episode_frame: None,
            pair_judgments: None,
            entries_clean: None,
        });
    }
    let json = serde_json::to_string(&entries).unwrap();
    // The documented estimate is ≈3 KB for 20 invariants. Allow generous headroom to
    // a hard bound of 4 KB so formatting drift doesn't flake CI.
    assert!(
        json.len() < 4 * 1024,
        "20 held_invariants serialized to {} bytes (> 4 KB)",
        json.len()
    );
}

// ---------------------------------------------------------------------------
// Episode-fold rules: held evidence (pairwise_separation +
// derived_transform_consistency)
// ---------------------------------------------------------------------------

/// The pairwise selector marker. Unique short name: other suites
/// register their own markers and short-name resolution is
/// ambiguous across same-named types (the MatchClock doctrine).
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct HeldRival;

/// Second marker distinguishing the crossing fixture's MOVER from
/// its static anchor (query iteration is unordered, so per-entity
/// behavior must ride on components, not iteration order).
#[derive(Component, Default)]
struct HeldMover;

/// Bodies drifting monotonically apart: every frame is pairable and
/// clean — the pure-coverage passing shape (the rule was exercised,
/// nothing ever came close; zero episodes observed). Sign-anchored:
/// positive-side bodies accelerate +x, negative-side −x, so the
/// pair only ever separates.
fn drift_apart(mut q: Query<&mut Transform, With<HeldRival>>) {
    for mut t in q.iter_mut() {
        t.translation.x += if t.translation.x > 0.0 { 0.05 } else { -0.05 };
    }
}

/// Marker for the DTC selector (unique-name doctrine).
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct HeldChild;

/// Move every root monotonically: the world keeps changing and a
/// healthy propagated hierarchy stays exactly coherent (the DTC
/// suite's move_roots doctrine).
fn held_move_roots(mut q: Query<&mut Transform, Without<ChildOf>>) {
    for mut t in q.iter_mut() {
        t.translation.x += 0.05;
    }
}

fn rival_app(movement: fn(Query<&mut Transform, With<HeldRival>>)) -> App {
    let mut app = App::new();
    app.register_type::<HeldRival>();
    app.register_type::<Transform>();
    app.add_systems(Update, movement);
    // Mirrored spawns: ±2.0 → the pair starts 4.0 apart and drifts
    // apart (only one pair is ever checked).
    app.world_mut()
        .spawn((HeldRival, Transform::from_xyz(2.0, 0.0, 0.0)));
    app.world_mut()
        .spawn((HeldRival, Transform::from_xyz(-2.0, 0.0, 0.0)));
    app
}

/// The crossing near-miss fixture's mover system: a static ANCHOR
/// at the origin plus a MOVER sweeping through it —
/// `x = 2 − 0.02·f`, entering `min_distance` (0.5) around frame 75
/// and exiting by frame ~125. Coincidence lasts ≈50 frames ≈ 0.83s
/// of frame clock — sub-soak against the 2.0s window, and the
/// episode the soak judgment discards is exactly the near-miss the
/// held record must name. The mover's Transform changes every frame
/// (frozen_world stays quiet).
#[derive(Resource, Default)]
struct HeldFrameClock {
    frame: u32,
}

fn sweep_through_anchor(
    mut q: Query<&mut Transform, (With<HeldRival>, With<HeldMover>)>,
    mut clock: ResMut<HeldFrameClock>,
) {
    clock.frame += 1;
    let x = 2.0 - 0.02 * clock.frame as f32;
    for mut t in q.iter_mut() {
        t.translation.x = x;
    }
}

fn crossing_app() -> App {
    let mut app = App::new();
    app.register_type::<HeldRival>();
    app.register_type::<Transform>();
    app.insert_resource(HeldFrameClock::default());
    app.add_systems(Update, sweep_through_anchor);
    // The anchor at the origin; the mover sweeping from +2 toward
    // −2 across the run.
    app.world_mut()
        .spawn((HeldRival, Transform::from_xyz(0.0, 0.0, 0.0)));
    app.world_mut()
        .spawn((HeldRival, HeldMover, Transform::from_xyz(2.0, 0.0, 0.0)));
    app
}

const SEPARATION_NEARMISS_INV: &str = r#"
    "name": "rivals_apart",
    "rule": "pairwise_separation",
    "target_selector": {"with_component": {"name": "HeldRival"}},
    "min_distance": 0.5,
    "window_s": 2.0"#;

/// A passing pairwise run with zero episodes: the held record
/// carries the workload figures and NO span witness (nothing ever
/// came close). `pair_judgments == frames_matched` (one pair, one
/// per frame); `frames_matched == frame_count` (recorders observe
/// every frame; the selector resolves from frame 0).
#[test]
fn pairwise_clean_pass_reports_workload_without_span() {
    let _slot = common::run_slot();
    let mut app = rival_app(drift_apart);
    let report =
        run_scenario(&mut app, &scenario_with_invariants(SEPARATION_NEARMISS_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    let held = report
        .held_invariants
        .iter()
        .find(|h| h.name == "rivals_apart")
        .expect("held entry for passing pairwise invariant");
    assert_eq!(held.rule, "pairwise_separation");
    // The selector component is the judged scope.
    assert_eq!(held.target, "HeldRival");
    // min_distance renders in predicate syntax.
    assert_eq!(held.predicate.as_deref(), Some("ge 0.5"));
    // One pair checked every frame: workload == frames == all of
    // them (the selector resolves from frame 0).
    assert_eq!(held.frames_matched, report.frame_count);
    assert_eq!(held.pair_judgments, Some(report.frame_count));
    // Clean run: no episode ever formed.
    assert_eq!(held.episodes_observed, Some(0));
    assert!(held.worst_episode_span.is_none());
    assert!(held.worst_episode_frame.is_none());
    // Pairwise never carries the entity fields.
    assert!(held.entity_judgments.is_none());
    assert!(held.nearest_margin.is_none());
    // The window rides along for the span-vs-window story.
    assert_eq!(held.window_s, Some(2.0));
}

/// A crossing body pair leaves a sub-soak episode: the held record
/// names it — `episodes_observed: 1`, a `worst_episode_span` honest
/// about being sub-window, and the frame the episode started. This
/// is the slice's core promise: the near-miss the soak judgment
/// silently discarded now has a trace.
#[test]
fn pairwise_subsoak_nearmiss_gets_span_witness() {
    let _slot = common::run_slot();
    let mut app = crossing_app();
    let report =
        run_scenario(&mut app, &scenario_with_invariants(SEPARATION_NEARMISS_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    let held = report
        .held_invariants
        .iter()
        .find(|h| h.name == "rivals_apart")
        .expect("held entry for passing pairwise invariant");
    assert_eq!(held.episodes_observed, Some(1));
    let span = held
        .worst_episode_span
        .expect("the crossing's episode carries a span witness");
    // The span must be sub-window on a passing run (the soak
    // judgment discarded exactly this episode as transient).
    assert!(
        span < 2.0,
        "passing run's worst span {span} must be sub-window (2.0)"
    );
    // Frame-clock driver: span counts seconds, so a real episode
    // of a crossing lasts > 0.
    assert!(span > 0.0);
    assert!(held.worst_episode_frame.is_some());
}

/// A pairwise invariant with nothing to pair (a single marker) is
/// VACUOUS — the run fails and there is no held entry (the
/// violation owns the story).
#[test]
fn vacuous_pairwise_gets_no_held_entry() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.register_type::<HeldRival>();
    app.register_type::<Transform>();
    app.add_systems(Update, drift_apart);
    // A lone survivor: never a pair to check.
    app.world_mut()
        .spawn((HeldRival, Transform::from_xyz(0.0, 0.0, 0.0)));
    let report =
        run_scenario(&mut app, &scenario_with_invariants(SEPARATION_NEARMISS_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    assert!(report
        .violations
        .iter()
        .any(|v| v.rule == "vacuous_invariant" && v.target == "rivals_apart"));
    assert!(report
        .held_invariants
        .iter()
        .all(|h| h.name != "rivals_apart"));
}

// ---------------------------------------------------------------------------
// derived_transform_consistency held evidence
// ---------------------------------------------------------------------------

fn held_hierarchy_app() -> App {
    let mut app = App::new();
    app.add_plugins((TaskPoolPlugin::default(), TransformPlugin));
    app.register_type::<HeldChild>();
    app.add_systems(Update, held_move_roots);
    let parent = app
        .world_mut()
        .spawn(Transform::from_xyz(0.0, 0.0, 0.0))
        .id();
    app.world_mut().spawn((
        HeldChild,
        Transform::from_xyz(0.0, 0.0, 0.0),
        ChildOf(parent),
    ));
    app
}

/// Backing clock for the blip writer (frames 10–29: a one-third
/// second divergence streak, well inside the 2.0s soak — the DTC
/// suite's blip doctrine).
#[derive(Resource, Default)]
struct HeldBlipClock {
    frame: u32,
}

fn blip_child_global(
    mut q: Query<&mut GlobalTransform, With<HeldChild>>,
    mut clock: ResMut<HeldBlipClock>,
) {
    clock.frame += 1;
    if (10..30).contains(&clock.frame) {
        for mut g in q.iter_mut() {
            *g = GlobalTransform::from_xyz(9.0, 9.0, 9.0);
        }
    }
}

const HELD_DTC_INV: &str = r#"
    "name": "globals_track_parents",
    "rule": "derived_transform_consistency",
    "target_selector": {"with_component": {"name": "HeldChild"}},
    "window_s": 2.0"#;

/// A healthy propagated hierarchy: coherent every judged frame, zero
/// episodes — the pure-coverage DTC held record. Grace frames (3 by
/// default) are skipped by the recorder, so `frames_matched` is
/// `frame_count − grace` frames, and `entity_judgments` counts ONE
/// child per judged frame.
#[test]
fn dtc_healthy_pass_reports_workload_without_span() {
    let _slot = common::run_slot();
    let mut app = held_hierarchy_app();
    let report = run_scenario(&mut app, &scenario_with_invariants(HELD_DTC_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    let held = report
        .held_invariants
        .iter()
        .find(|h| h.name == "globals_track_parents")
        .expect("held entry for passing DTC invariant");
    assert_eq!(held.rule, "derived_transform_consistency");
    assert_eq!(held.target, "HeldChild");
    // tolerance defaults when the author omits it (le 1e-4).
    assert_eq!(held.predicate.as_deref(), Some("le 0.0001"));
    // Grace frames are never judged.
    let grace = bevy_playtest::invariant_eval::DEFAULT_GRACE_FRAMES as u64;
    let judged = report.frame_count - grace;
    assert_eq!(held.frames_matched, judged);
    assert_eq!(held.entity_judgments, Some(judged));
    // Healthy run: zero episodes.
    assert_eq!(held.episodes_observed, Some(0));
    assert!(held.worst_episode_span.is_none());
    // DTC never carries the pairwise field.
    assert!(held.pair_judgments.is_none());
}

/// A one-third-second divergence streak is sub-soak against the 2.0s
/// window: the run passes and the held record names the discarded
/// episode — `episodes_observed: 1` plus the span witness. The
/// blip lands post-grace (frames 10–29), so the streak is observed.
#[test]
fn dtc_subsoak_nearmiss_gets_span_witness() {
    let _slot = common::run_slot();
    let mut app = held_hierarchy_app();
    app.insert_resource(HeldBlipClock::default());
    app.add_systems(
        PostUpdate,
        blip_child_global.after(TransformSystems::Propagate),
    );
    let report = run_scenario(&mut app, &scenario_with_invariants(HELD_DTC_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.rule == "derived_transform_consistency"),
        "a 1/3-second streak is sub-soak: {:?}",
        report.violations
    );
    let held = report
        .held_invariants
        .iter()
        .find(|h| h.name == "globals_track_parents")
        .expect("held entry for passing DTC invariant");
    assert_eq!(held.episodes_observed, Some(1));
    let span = held
        .worst_episode_span
        .expect("the blip episode carries a span witness");
    // 20 frames ≈ 0.317s of frame clock (sub-window, > 0).
    assert!(span > 0.0 && span < 2.0, "span {span}");
    assert!(held.worst_episode_frame.is_some());
}

/// A flat world is `hierarchy_absent` — INFORMATIONAL, the run
/// still PASSES — so the collector's own zero-judged-frames guard
/// must step in: no held entry may claim evidence for a rule that
/// was never exercised. THE mirror of the all-selector bounds
/// doctrine (an informational audit cannot gate, so the collector
/// gates instead).
#[test]
fn dtc_flat_world_gets_no_held_entry() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.add_plugins((TaskPoolPlugin::default(), TransformPlugin));
    app.register_type::<HeldChild>();
    // Flat but observable: one moving un-parented entity.
    app.add_systems(Update, held_move_roots);
    app.world_mut()
        .spawn((HeldChild, Transform::from_xyz(0.0, 1.0, 0.0)));
    let report = run_scenario(&mut app, &scenario_with_invariants(HELD_DTC_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    assert!(report
        .violations
        .iter()
        .any(|v| v.rule == "hierarchy_absent" && v.target == "globals_track_parents"));
    assert!(report
        .held_invariants
        .iter()
        .all(|h| h.name != "globals_track_parents"));
}

// ---------------------------------------------------------------------------
// BatteryConfig::new (the constructor fix)
// ---------------------------------------------------------------------------

/// deferral note wanted — WITHOUT a trait Default (the `binary`
/// field has no sensible default; the rationale stands, only the
/// convenience was missing). Mirrors `ProbePoolConfig::new`.
#[test]
fn battery_config_new_matches_run_defaults() {
    let cfg = bevy_playtest::battery::BatteryConfig::new("/path/to/game");
    assert_eq!(cfg.binary, "/path/to/game");
    assert_eq!(cfg.scenario, bevy_playtest::battery::BUILTIN_BATTERY);
    assert_eq!(
        cfg.timeout_secs,
        bevy_playtest::battery::DEFAULT_TIMEOUT_SECS
    );
    assert_eq!(cfg.seed_override, None);
}

// ---------------------------------------------------------------------------
// spawn_safety held evidence
// ---------------------------------------------------------------------------

/// The judged archetype's marker and the anchor set's marker —
/// uniquely named (the shared registry holds bevy_sprite's
/// `Anchor`, so the anchor marker cannot be called that).
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct HeldRespawn;

#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct HeldBeacon;

/// Keep the beacon wiggling so frozen_world stays quiet without
/// crossing the planted distance margin (±0.01 vs threshold 0.5).
fn wiggle_held_beacon(mut q: Query<(&mut Transform, &HeldBeacon)>, mut t: Local<f32>) {
    *t += 0.05;
    for (mut tr, _) in q.iter_mut() {
        tr.translation.y = 0.01 * t.sin();
    }
}

/// The held-trace spawn_safety fixture: a beacon from the initial
/// scene, one CLEAN projectile entry planted at frame 1 (frame 0 is
/// the authored scene, never a judged entry).
fn spawn_safety_app(spawn_z: f32) -> App {
    let mut app = App::new();
    app.add_plugins((TaskPoolPlugin::default(), TransformPlugin));
    app.register_type::<HeldRespawn>();
    app.register_type::<HeldBeacon>();
    app.register_type::<Transform>();
    app.add_systems(Update, wiggle_held_beacon);
    app.world_mut()
        .spawn((HeldBeacon, Transform::from_xyz(0.0, 0.0, 0.0)));
    app.add_systems(
        Update,
        move |mut commands: Commands, mut fired: Local<bool>, mut frame: Local<u64>| {
            *frame += 1;
            if *frame > 1 && !*fired {
                *fired = true;
                commands.spawn((HeldRespawn, Transform::from_xyz(0.0, 0.0, spawn_z)));
            }
        },
    );
    app
}

const SPAWN_SAFETY_HELD_INV: &str = r#"
    "name": "safe_respawns",
    "rule": "spawn_safety",
    "target_selector": {"with_component": {"name": "HeldRespawn"}},
    "min_distance_from": {"with_component": {"name": "HeldBeacon"}},
    "min_distance": 0.5"#;

/// A passing spawn_safety run carries its entry-count story: one
/// entry judged, one entry clean, and the anchor-present frame
/// count as `frames_matched`. The workload figure rides
/// `entity_judgments` (the field's spawn_safety doctrine: once per
/// ENTRY, not per frame), and `entries_clean` is the pass-side
/// proof that the one judged entry cleared every anchor.
#[test]
fn spawn_safety_clean_pass_reports_entry_counts() {
    let _slot = common::run_slot();
    let mut app = spawn_safety_app(5.0);
    let report = run_scenario(&mut app, &scenario_with_invariants(SPAWN_SAFETY_HELD_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    let held = report
        .held_invariants
        .iter()
        .find(|h| h.name == "safe_respawns")
        .expect("held entry for passing spawn_safety invariant");
    assert_eq!(held.rule, "spawn_safety");
    // The selector component is the judged scope.
    assert_eq!(held.target, "HeldRespawn");
    // min_distance renders in predicate syntax, pairwise-shaped.
    assert_eq!(held.predicate.as_deref(), Some("ge 0.5"));
    // Exactly one entry, judged once at its transition frame.
    assert_eq!(held.entity_judgments, Some(1));
    assert_eq!(held.entries_clean, Some(1));
    // The beacon was present (an anchor) on every recorded frame.
    assert_eq!(held.frames_matched, report.frame_count);
    // No episode/soak machinery exists for this rule.
    assert_eq!(held.episodes_observed, None);
    assert!(held.worst_episode_span.is_none());
    assert!(held.pair_judgments.is_none());
    assert!(held.nearest_margin.is_none());
    assert!(held.window_s.is_none());
}

/// A violating entry belongs to the violations list, not the held
/// trace — the run FAILS and no held entry claims evidence.
#[test]
fn failing_spawn_safety_is_not_held() {
    let _slot = common::run_slot();
    let mut app = spawn_safety_app(0.0);
    let report = run_scenario(&mut app, &scenario_with_invariants(SPAWN_SAFETY_HELD_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    assert!(report
        .violations
        .iter()
        .any(|v| v.rule == "spawn_safety" && v.target == "safe_respawns"));
    assert!(report
        .held_invariants
        .iter()
        .all(|h| h.name != "safe_respawns"));
}

/// A vacuous spawn_safety invariant (anchors never resolved) fails
/// the run and earns no held entry — the violation owns the story.
#[test]
fn vacuous_spawn_safety_gets_no_held_entry() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.add_plugins((TaskPoolPlugin::default(), TransformPlugin));
    app.register_type::<HeldRespawn>();
    app.register_type::<HeldBeacon>();
    app.register_type::<Transform>();
    // Entries appear, but no beacon ever exists.
    app.add_systems(
        Update,
        |mut commands: Commands, mut fired: Local<bool>, mut frame: Local<u64>| {
            *frame += 1;
            if *frame > 1 && !*fired {
                *fired = true;
                commands.spawn((HeldRespawn, Transform::from_xyz(0.0, 0.0, 5.0)));
            }
        },
    );
    let report = run_scenario(&mut app, &scenario_with_invariants(SPAWN_SAFETY_HELD_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    assert!(report
        .violations
        .iter()
        .any(|v| v.rule == "vacuous_invariant" && v.target == "safe_respawns"));
    assert!(report
        .held_invariants
        .iter()
        .all(|h| h.name != "safe_respawns"));
}
