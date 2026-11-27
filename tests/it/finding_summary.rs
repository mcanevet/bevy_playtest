//! Behavioral battery for the top-level `finding_summary` block
//! (anti-false-confidence): the skim-guard counts joined over the
//! report's own rows — gating vs informational/warn-only entries,
//! and the near-miss aggregates (discarded sub-soak episodes with
//! the worst span vs its authored soak) for passing episode-tier
//! invariants.
//!
//! The incident class behind the block: a passing run was skimmed
//! as clean while its held trace quietly recorded multiple
//! sub-soak episodes that came within a hair of failing it — and a
//! warn-only finding hid among gating-shaped rows. The summary
//! puts both facts at the top of the report where a skimmer
//! cannot miss them.

use crate::common;

use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_playtest::driver::run_scenario;
use bevy_playtest::state::NearMissRow;
use bevy_reflect::Reflect;
use bevy_transform::prelude::{Transform, TransformPlugin};

// ---------------------------------------------------------------------------
// Fixtures (mirroring the held_invariants suite's crossing shapes)
// ---------------------------------------------------------------------------

#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct SumRival;

/// Second marker distinguishing the crossing fixture's MOVER from
/// its static anchor (query iteration is unordered, so per-entity
/// behavior must ride on components, not iteration order).
#[derive(Component, Default)]
struct SumMover;

#[derive(Resource, Default)]
struct SumFrameClock {
    frame: u32,
}

/// The crossing near-miss mover: a static ANCHOR at the origin plus
/// a MOVER sweeping through it — coincidence lasts ≈50 frames ≈
/// 0.83s of frame clock, sub-soak against the 2.0s window. The
/// episode the soak judgment discards is the near-miss the summary
/// must name. The mover's Transform changes every frame
/// (frozen_world stays quiet).
fn sum_sweep_through_anchor(
    mut q: Query<&mut Transform, (With<SumRival>, With<SumMover>)>,
    mut clock: ResMut<SumFrameClock>,
) {
    clock.frame += 1;
    let x = 2.0 - 0.02 * clock.frame as f32;
    for mut t in q.iter_mut() {
        t.translation.x = x;
    }
}

fn crossing_app() -> App {
    let mut app = App::new();
    app.add_plugins((bevy_app::TaskPoolPlugin::default(), TransformPlugin));
    app.register_type::<SumRival>();
    app.register_type::<Transform>();
    app.insert_resource(SumFrameClock::default());
    app.add_systems(Update, sum_sweep_through_anchor);
    app.world_mut()
        .spawn((SumRival, Transform::from_xyz(0.0, 0.0, 0.0)));
    app.world_mut()
        .spawn((SumRival, SumMover, Transform::from_xyz(2.0, 0.0, 0.0)));
    app
}

fn scenario_with_invariants(invs: &str) -> bevy_playtest::scenario::Scenario {
    let json = format!(
        r#"{{"bot":{{"type":"chaos","seed":42}},"duration_s":4.0,"tps":60,
            "invariants":[{{{invs}}}]}}"#
    );
    serde_json::from_str(&json).unwrap()
}

const NEARMISS_INV: &str = r#"
    "name": "rivals_apart",
    "rule": "pairwise_separation",
    "target_selector": {"with_component": {"name": "SumRival"}},
    "min_distance": 0.5,
    "window_s": 2.0"#;

const FLAT_WORLD_INV: &str = r#"
    "name": "globals_follow_parents",
    "rule": "derived_transform_consistency",
    "window_s": 1.0"#;

// ---------------------------------------------------------------------------
// The summary block
// ---------------------------------------------------------------------------

/// A passing run with a discarded sub-soak episode carries the
/// near-miss row at the TOP of the report: invariant, rule, the
/// discarded-episode count, the worst span vs the authored soak.
/// A skimmer who reads only `status: Pass` and the summary cannot
/// mistake this run for an uneventful one.
#[test]
fn passing_run_with_nearmiss_carries_summary_row() {
    let _slot = common::run_slot();
    let mut app = crossing_app();
    let report = run_scenario(&mut app, &scenario_with_invariants(NEARMISS_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    let summary = report
        .finding_summary
        .as_ref()
        .expect("a completed run always carries the summary");
    assert_eq!(summary.gating, 0, "no gating findings on this pass");
    let row = summary
        .near_misses
        .iter()
        .find(|r| r.invariant == "rivals_apart")
        .expect("the discarded episode is summarized");
    assert_eq!(row.rule, "pairwise_separation");
    assert_eq!(row.discarded_subsoak_episodes, 1);
    assert!(
        row.worst_span > 0.0 && row.worst_span < row.soak,
        "the worst span ({}) is sub-soak ({})",
        row.worst_span,
        row.soak
    );
    let expected_ratio = row.worst_span / row.soak;
    assert!((row.span_vs_soak - expected_ratio).abs() < 1e-12);
}

/// Informational entries (the flat-world `hierarchy_absent` note)
/// count as `warn_only`, never `gating` — the two classes a skimmer
/// most easily merges are split in the summary itself.
#[test]
fn informational_note_counts_warn_only_not_gating() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.add_plugins((bevy_app::TaskPoolPlugin::default(), TransformPlugin));
    app.register_type::<SumRival>();
    // A flat but observable world: one moving un-parented entity.
    app.add_systems(Update, |mut q: Query<&mut Transform, With<SumRival>>| {
        for mut t in q.iter_mut() {
            t.translation.x += 0.05;
        }
    });
    app.world_mut()
        .spawn((SumRival, Transform::from_xyz(0.0, 1.0, 0.0)));
    let report = run_scenario(&mut app, &scenario_with_invariants(FLAT_WORLD_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    let summary = report.finding_summary.as_ref().expect("summary present");
    assert_eq!(summary.gating, 0);
    assert_eq!(summary.warn_only, 1, "the hierarchy_absent note");
    assert!(
        summary.near_misses.is_empty(),
        "a rule that never judged cannot have discarded episodes"
    );
}

/// A CLEAN pass — nothing discarded, nothing warned — carries an
/// honest zero-block, not an absent one: absence is reserved for
/// crash stubs and pre-field reports, never for "nothing to say".
#[test]
fn clean_pass_carries_zero_summary() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.add_plugins((bevy_app::TaskPoolPlugin::default(), TransformPlugin));
    app.register_type::<SumRival>();
    // Two bodies drifting monotonically apart: every frame is
    // pairable and clean — the pure-coverage passing shape.
    app.add_systems(Update, |mut q: Query<&mut Transform, With<SumRival>>| {
        for mut t in q.iter_mut() {
            t.translation.x += if t.translation.x > 0.0 { 0.05 } else { -0.05 };
        }
    });
    app.world_mut()
        .spawn((SumRival, Transform::from_xyz(2.0, 0.0, 0.0)));
    app.world_mut()
        .spawn((SumRival, Transform::from_xyz(-2.0, 0.0, 0.0)));
    let report = run_scenario(&mut app, &scenario_with_invariants(NEARMISS_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    let summary = report.finding_summary.as_ref().expect("summary present");
    assert_eq!(summary.gating, 0);
    assert_eq!(summary.warn_only, 0);
    // The invariant judged every frame but no episode ever formed:
    // a clean zero, not a missing row.
    assert!(
        summary
            .near_misses
            .iter()
            .all(|r| r.discarded_subsoak_episodes == 0)
            || summary.near_misses.is_empty()
    );
}

/// The summary serializes with stable wire names and deserializes
/// back — consumers parse it as structured data.
#[test]
fn summary_serializes_stably() {
    let _slot = common::run_slot();
    let mut app = crossing_app();
    let report = run_scenario(&mut app, &scenario_with_invariants(NEARMISS_INV)).unwrap();
    let json = serde_json::to_value(&report).unwrap();
    let fs = json.get("finding_summary").expect("finding_summary key");
    for key in ["gating", "warn_only", "near_misses"] {
        assert!(fs.get(key).is_some(), "summary key {key}");
    }
    let nm = fs
        .get("near_misses")
        .and_then(|n| n.get(0))
        .expect("the near-miss row rides the array");
    for key in [
        "invariant",
        "rule",
        "discarded_subsoak_episodes",
        "worst_span",
        "worst_span_frame",
        "soak",
        "span_vs_soak",
    ] {
        assert!(nm.get(key).is_some(), "near-miss key {key}");
    }
    // Round-trip: the row survives a serialize/deserialize cycle.
    let back: NearMissRow = serde_json::from_value(nm.clone()).unwrap();
    assert_eq!(back.invariant, "rivals_apart");
}

/// Crash stubs never claim a summary: absence is "not summarized",
/// never "clean".
#[test]
fn crash_stub_has_no_summary() {
    let report = bevy_playtest::state::PlaytestReport::crashed_empty();
    assert!(report.finding_summary.is_none());
}
