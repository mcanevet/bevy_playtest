//! Behavioral battery for the `occupancy_cap` rule: the
//! local-density crowd oracle, WARN-ONLY. More than `cap`
//! selected entities within `radius` of a selected entity's
//! position (count self-inclusive), sustained past a `window_s`
//! soak, becomes a WARN finding — never a status flip.
//!
//! The battery pins GRAMMAR mechanics on generic fixtures and
//! the no-fail-power construction (settlement veto point 1):
//! a crowd that soaks must render as an ordinary violations[]
//! entry on a status:"Pass" report (the juxtaposition IS the
//! honesty device), the never-a-crowd note must never gate, and
//! STRICT WARNINGS must not change either. NO litmus claim
//! anywhere: §8's row stays PLACEHOLDER — these are mechanics
//! pins on generic fixtures, never an owned-incident claim.
//!
//! Fixture math: tps 60, so the frame clock advances 1/60 s per
//! frame; a 3.0s soak elapses at frame 180 — the crowd must
//! persist past frame 180 for a soaked finding, and dissolve
//! before it for a sub-soak transient.

use crate::common;

use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_playtest::driver::run_scenario;
use bevy_playtest::scenario::{lint_scenario, Scenario, ScenarioError};
use bevy_reflect::Reflect;
use bevy_transform::components::Transform;

/// The counted crowd member. The name is unique in the
/// integration binary (short-name resolution is ambiguous
/// across same-named types).
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct CrowdMember;

fn scenario_with_invariant(inv: &str) -> bevy_playtest::scenario::Scenario {
    let json = format!(
        r#"{{"bot":{{"type":"chaos","seed":42}},"duration_s":6.0,"tps":60,
            "invariants":[{{{inv}}}]}}"#
    );
    serde_json::from_str(&json).unwrap()
}

/// A live world for the background: one body keeps moving so
/// frozen_world stays quiet while the crowd stands still.
#[derive(Component)]
struct Background {
    #[allow(dead_code)]
    tag: u32,
}

fn move_background(mut q: Query<&mut Transform, With<Background>>) {
    for mut t in q.iter_mut() {
        t.translation.x += 0.05;
    }
}

fn base_app() -> App {
    let mut app = App::new();
    app.add_plugins(bevy_app::TaskPoolPlugin::default());
    app.add_plugins(bevy_transform::TransformPlugin);
    app.register_type::<CrowdMember>();
    app.register_type::<Transform>();
    app.add_systems(Update, move_background);
    app.world_mut()
        .spawn((Background { tag: 0 }, Transform::default()));
    app
}

/// The standing-crowd watch: four members within radius 1.0 of
/// each other (cap 3), judged against a 3.0s frame-clock soak.
const CROWD_INV: &str = r#"
    "name": "no_more_than_three_in_a_room",
    "rule": "occupancy_cap",
    "target_selector": {"with_component": {"name": "CrowdMember"}},
    "radius": 1.0,
    "cap": 3,
    "window_s": 3.0"#;

/// Places four members at the corners of a tight square with
/// side ~0.5 (every pair well within radius 1.0 — each sees a
/// self-inclusive count of 4 > cap 3).
fn spawn_standing_crowd(world: &mut bevy_ecs::world::World) {
    for (x, y) in [(0.0, 0.0), (0.5, 0.0), (0.0, 0.5), (0.5, 0.5)] {
        world.spawn((CrowdMember, Transform::from_xyz(x, y, 0.0)));
    }
}

// ---------------------------------------------------------------------------
// The headline: a soaked crowd warns, never gates
// ---------------------------------------------------------------------------

/// Four members stand together for the WHOLE run: the episode
/// soaks past 3.0s, the finding fires as occupancy_cap — and
/// the run STILL PASSES. The finding must carry the
/// `unpromoted_rule` mark and the warn-only posture sentence.
/// This is the no-fail-power pin at the heart of the slice.
#[test]
fn soaked_crowd_warns_but_the_run_still_passes() {
    let _slot = common::run_slot();
    let mut app = base_app();
    spawn_standing_crowd(app.world_mut());
    let report = run_scenario(&mut app, &scenario_with_invariant(CROWD_INV)).unwrap();
    assert_eq!(
        report.status,
        bevy_playtest::enums::PlaytestStatus::Pass,
        "a warn-only rule must never flip run status"
    );
    let v = report
        .violations
        .iter()
        .find(|v| v.rule == "occupancy_cap" && v.target == "no_more_than_three_in_a_room")
        .expect("expected an occupancy_cap warning");
    assert!(
        v.detail.contains("UNPROMOTED RULE"),
        "the finding must carry the posture: {}",
        v.detail
    );
    // The `_meta` evidence: four witnesses, the worst count is 4
    // (each member sees all four), radius/cap echoed, roster
    // capped at 32 so no truncation.
    let meta = v.meta.as_ref().expect("episode warning carries _meta");
    assert_eq!(meta.unpromoted_rule, Some(true));
    let occ = meta
        .occupancy_episode
        .as_ref()
        .expect("_meta carries the occupancy episode block");
    assert_eq!(occ.sampled_entities, 4);
    assert!(!occ.roster_truncated);
    assert_eq!(occ.radius, 1.0);
    assert_eq!(occ.cap, 3);
    assert_eq!(occ.roster.len(), 4);
    assert_eq!(occ.roster[0].worst_count, 4);
}

/// Strict warnings change NOTHING: the same soaked crowd under
/// `strict_warnings: true` still passes with the warning present
/// (settlement Q4 — no escalation knob exists for occupancy).
#[test]
fn strict_warnings_do_not_escalate_occupancy() {
    let _slot = common::run_slot();
    let mut app = base_app();
    spawn_standing_crowd(app.world_mut());
    let scenario: bevy_playtest::scenario::Scenario = serde_json::from_str(
        r#"{"bot":{"type":"chaos","seed":42},"duration_s":6.0,"tps":60,
            "strict_warnings":true,
            "invariants":[{"name": "no_more_than_three_in_a_room",
                           "rule": "occupancy_cap",
                           "target_selector": {"with_component": {"name": "CrowdMember"}},
                           "radius": 1.0, "cap": 3, "window_s": 3.0}]}"#,
    )
    .unwrap();
    let report = run_scenario(&mut app, &scenario).unwrap();
    assert_eq!(
        report.status,
        bevy_playtest::enums::PlaytestStatus::Pass,
        "occupancy_cap is informational unconditionally — strict_warnings must not escalate it"
    );
    assert!(
        report.violations.iter().any(|v| v.rule == "occupancy_cap"),
        "the warning must still be present"
    );
}

/// Control: cap 4 admits the same crowd — no warning at all, the
/// boundary is `count > cap` (4 witnesses would be needed at
/// cap 3; at cap 4 the count of 4 is legal).
#[test]
fn crowd_at_exactly_cap_never_warns() {
    let _slot = common::run_slot();
    let mut app = base_app();
    spawn_standing_crowd(app.world_mut());
    let report = run_scenario(
        &mut app,
        &scenario_with_invariant(
            r#"
    "name": "four_is_fine",
    "rule": "occupancy_cap",
    "target_selector": {"with_component": {"name": "CrowdMember"}},
    "radius": 1.0,
    "cap": 4,
    "window_s": 3.0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    assert!(
        !report.violations.iter().any(|v| v.rule == "occupancy_cap"),
        "count == cap is legal: {:?}",
        report.violations
    );
}

// ---------------------------------------------------------------------------
// The soak: transient gatherings are discarded
// ---------------------------------------------------------------------------

/// The crowd dissolves mid-run (well before the 3.0s soak at
/// frame 180): the episode's span never reaches the soak and is
/// discarded as a transient gathering — no warning, the run
/// passes (movement in the middle of the crowd also keeps
/// frozen_world quiet).
#[test]
fn transient_gathering_below_the_soak_is_discarded() {
    let _slot = common::run_slot();
    let mut app = base_app();
    spawn_standing_crowd(app.world_mut());
    // Dissolve at frame ~100 (well inside the 3.0s soak window,
    // and off any phase boundary): one teleport to fixed,
    // pairwise-far positions (bounded coordinates — the world
    // bounds oracle must stay quiet; it is not under test here).
    app.add_systems(
        Update,
        |mut q: Query<&mut Transform, With<CrowdMember>>, mut clock: Local<f32>| {
            *clock += 1.0;
            if *clock > 100.0 {
                for (i, mut t) in q.iter_mut().enumerate() {
                    t.translation.x = 50.0 * (i as f32 + 1.0);
                    t.translation.y = 0.0;
                }
            }
        },
    );
    let report = run_scenario(&mut app, &scenario_with_invariant(CROWD_INV)).unwrap();
    assert_eq!(
        report.status,
        bevy_playtest::enums::PlaytestStatus::Pass,
        "{:#?}",
        report.violations
    );
    assert!(
        !report.violations.iter().any(|v| v.rule == "occupancy_cap"),
        "a sub-soak gathering is a transient, not congestion: {:?}",
        report.violations
    );
}

// ---------------------------------------------------------------------------
// Enumeration semantics (settlement veto point 2)
// ---------------------------------------------------------------------------

/// The RING: eight members spaced just past radius from the
/// ring's center — pairwise distances across the ring can sit
/// below radius, but no member sees a self-inclusive count above
/// cap when cap 3 and spacing is chosen so each member has at
/// most two neighbors within radius. The exact-center crowd is
/// the documented NON-FIRING case of entity-witnessed counting:
/// a finding here would mean the enumeration invented density
/// that no selected entity witnesses.
#[test]
fn ring_of_entities_is_a_documented_non_firing() {
    let _slot = common::run_slot();
    let mut app = base_app();
    // Eight points on a circle of radius 1.2 with radius 1.0:
    // adjacent arc distance 2*1.2*sin(pi/8) ≈ 0.918 < 1.0 (each
    // member sees itself + two neighbors = 3 == cap 3: legal),
    // but the density at the EMPTY CENTER would be 8 > 3 under
    // exact "any point" counting — the ring is the honest
    // non-firing case.
    for k in 0..8 {
        let theta = std::f32::consts::PI * 2.0 * (k as f32 / 8.0);
        app.world_mut().spawn((
            CrowdMember,
            Transform::from_xyz(theta.cos() * 1.2, theta.sin() * 1.2, 0.0),
        ));
    }
    let report = run_scenario(&mut app, &scenario_with_invariant(CROWD_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    assert!(
        !report.violations.iter().any(|v| v.rule == "occupancy_cap"),
        "the ring is the documented non-firing case: {:?}",
        report.violations
    );
}

/// The boundary: a neighbor at EXACTLY radius counts as within
/// (INCLUSIVE `<=`). Two members 1.0 apart with radius 1.0 and
/// cap 1: each sees the other (count 2 > 1) — the pair warns.
/// The pinned arithmetic: squared-distance compare, `d2 <= r2`.
#[test]
fn exact_radius_boundary_is_inclusive() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.world_mut()
        .spawn((CrowdMember, Transform::from_xyz(0.0, 0.0, 0.0)));
    app.world_mut()
        .spawn((CrowdMember, Transform::from_xyz(1.0, 0.0, 0.0)));
    let report = run_scenario(
        &mut app,
        &scenario_with_invariant(
            r#"
    "name": "pairs_at_radius_count",
    "rule": "occupancy_cap",
    "target_selector": {"with_component": {"name": "CrowdMember"}},
    "radius": 1.0,
    "cap": 1,
    "window_s": 3.0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    let v = report
        .violations
        .iter()
        .find(|v| v.rule == "occupancy_cap" && v.target == "pairs_at_radius_count")
        .expect("distance exactly radius counts as within (inclusive)");
    let meta = v.meta.as_ref().unwrap();
    let occ = meta.occupancy_episode.as_ref().unwrap();
    assert_eq!(occ.sampled_entities, 2);
    assert_eq!(occ.roster[0].worst_count, 2);
}

/// JUST outside: two members at 1.001 with radius 1.0 see nobody
/// but themselves — no warning (the flip side of the boundary
/// pin, so the inclusive compare cannot drift into a strict
/// triangle inequality case).
#[test]
fn just_past_radius_counts_nobody() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.world_mut()
        .spawn((CrowdMember, Transform::from_xyz(0.0, 0.0, 0.0)));
    app.world_mut()
        .spawn((CrowdMember, Transform::from_xyz(1.001, 0.0, 0.0)));
    let report = run_scenario(
        &mut app,
        &scenario_with_invariant(
            r#"
    "name": "pairs_just_outside_count_nobody",
    "rule": "occupancy_cap",
    "target_selector": {"with_component": {"name": "CrowdMember"}},
    "radius": 1.0,
    "cap": 1,
    "window_s": 3.0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    assert!(
        !report.violations.iter().any(|v| v.rule == "occupancy_cap"),
        "just-past-radius neighbors are strangers: {:?}",
        report.violations
    );
}

// ---------------------------------------------------------------------------
// Churn: a crowd that keeps forming and dissolving never soaks
// ---------------------------------------------------------------------------

/// Members teleported apart and back every 30 frames: episodes
/// keep opening and closing at spans far below the 3.0s soak —
/// churn is never congestion. Also pins the index-ordered
/// stream: the worst witness per episode varies, but every
/// episode stays sub-soak.
#[test]
fn churning_crowd_never_soaks() {
    let _slot = common::run_slot();
    let mut app = base_app();
    spawn_standing_crowd(app.world_mut());
    app.add_systems(
        Update,
        |mut q: Query<&mut Transform, With<CrowdMember>>, mut clock: Local<f32>| {
            *clock += 1.0;
            // Scatter for 30 frames, gather for 30 frames — the
            // episode spans stay ≤ ~30 frames = 0.5s.
            let scatter = ((*clock / 30.0) as i64) % 2 == 1;
            for (i, mut t) in q.iter_mut().enumerate() {
                let spread = if scatter { 20.0 } else { 0.0 };
                let base = (i % 2) as f32 * 0.5;
                t.translation.x = base + spread * (i as f32 + 1.0).cos();
                t.translation.y = ((i % 2) as f32 * 0.5) + spread * (i as f32 + 1.0).sin();
            }
        },
    );
    let report = run_scenario(&mut app, &scenario_with_invariant(CROWD_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    assert!(
        !report.violations.iter().any(|v| v.rule == "occupancy_cap"),
        "churn never soaks: {:?}",
        report.violations
    );
}

// ---------------------------------------------------------------------------
// Vacuity: never-a-crowd emits under the rule's own name
// ---------------------------------------------------------------------------

/// The selector names a REGISTERED component with zero
/// instances: the run passes, and the post-run audit emits the
/// never-a-crowd note under `occupancy_cap` itself — NEVER
/// `vacuous_invariant` (a GATING rule: emitting it would
/// smuggle fail-power through the audit path — the settlement's
/// central departure from pairwise's shape). The note carries
/// the unpromoted mark too.
#[test]
fn selector_matching_zero_entities_emits_the_own_name_note_not_vacuity() {
    let _slot = common::run_slot();
    let mut app = base_app();
    // CrowdMember is registered but has ZERO instances (only the
    // background body exists).
    let report = run_scenario(&mut app, &scenario_with_invariant(CROWD_INV)).unwrap();
    assert_eq!(
        report.status,
        bevy_playtest::enums::PlaytestStatus::Pass,
        "the never-a-crowd note must not gate"
    );
    let v = report
        .violations
        .iter()
        .find(|v| v.rule == "occupancy_cap" && v.target == "no_more_than_three_in_a_room")
        .expect("expected the never-a-crowd note");
    assert!(
        v.detail.contains("never had a crowd"),
        "the note tells the honest story: {}",
        v.detail
    );
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.rule == "vacuous_invariant"),
        "vacuous_invariant would gate — occupancy must never emit it"
    );
    let meta = v.meta.as_ref().expect("the note carries the mark too");
    assert_eq!(meta.unpromoted_rule, Some(true));
}

/// A live, always-within-cap population gets NO note at all —
/// a population always within cap is the healthy case, and the
/// note only exists to say "the census ran and never had a
/// bigger crowd to bound" when there was NOBODY. Two members
/// (each seeing 2 ≤ cap 3) stay clean: no violations.
#[test]
fn within_cap_population_emits_no_note() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.world_mut()
        .spawn((CrowdMember, Transform::from_xyz(0.0, 0.0, 0.0)));
    app.world_mut()
        .spawn((CrowdMember, Transform::from_xyz(0.1, 0.0, 0.0)));
    let report = run_scenario(&mut app, &scenario_with_invariant(CROWD_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    assert!(
        !report.violations.iter().any(|v| v.rule == "occupancy_cap"),
        "a within-cap population is silent: {:?}",
        report.violations
    );
}

// ---------------------------------------------------------------------------
// Slot recycling: a recycled successor is a fresh identity
// ---------------------------------------------------------------------------

/// A crowd member DESPAWNS and its slot recycles into a fresh
/// member mid-run: the composite (index, generation) key means
/// the successor never inherits the predecessor's episode
/// identity — it enters the still-open episode as a FRESH key
/// (union timeline: the surviving trio keeps the episode open, so
/// the recycled successor's crowd continues the same union
/// episode and the run-end drain judges the whole span).
#[test]
fn slot_recycled_successor_is_a_fresh_identity() {
    let _slot = common::run_slot();
    let mut app = base_app();
    spawn_standing_crowd(app.world_mut());
    // Victim: the FIRST-spawned member. At frame ~90 it despawns
    // and its slot recycles into a fresh member joining the same
    // spot — one despawned identity, one born, never welded.
    let mut q = app
        .world_mut()
        .query_filtered::<Entity, With<CrowdMember>>();
    let victim = q.iter(app.world()).next().expect("crowd is spawned");
    app.add_systems(
        Update,
        move |mut commands: Commands, mut clock: Local<f32>| {
            *clock += 1.0;
            if (*clock - 90.0).abs() < 0.5 {
                commands.entity(victim).despawn();
                commands.spawn((CrowdMember, Transform::from_xyz(0.25, 0.25, 0.0)));
            }
        },
    );
    let report = run_scenario(&mut app, &scenario_with_invariant(CROWD_INV)).unwrap();
    // Union-timeline semantics: the survivors kept the episode
    // open from frame 0 (a witness-set change never closes an
    // episode — only a clean frame does), absorbed the successor,
    // and the run-end drain judges the whole union span past the
    // 3.0s soak. The recycled successor is counted as a FRESH
    // identity — (index, generation) — never welded to its
    // predecessor's stint; five distinct keys in all.
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    let v = report
        .violations
        .iter()
        .find(|v| v.rule == "occupancy_cap" && v.target == "no_more_than_three_in_a_room")
        .expect("the union episode soaks despite the churn");
    let meta = v.meta.as_ref().unwrap();
    let occ = meta.occupancy_episode.as_ref().unwrap();
    assert_eq!(
        occ.sampled_entities, 5,
        "four originals + the recycled successor = five distinct (index, generation) keys"
    );
    assert!(
        !occ.roster_truncated,
        "five witnesses fit the roster cap of 32"
    );
    // And the roster's identities are all DISTINCT pairs: a
    // recycled slot appears at most once (the successor replaces
    // the predecessor — same index, different generation), so no
    // identity is welded or forged.
    let mut ids: Vec<(u32, u32)> = occ
        .roster
        .iter()
        .map(|r| (r.entity, r.generation))
        .collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), occ.roster.len(), "no welded identities: {ids:?}");
}

// ---------------------------------------------------------------------------
// Validation traps (load-time grammar)
// ---------------------------------------------------------------------------

fn occupancy_spec(json_invariant: &str) -> Scenario {
    let json = format!(
        r#"{{"bot":{{"type":"chaos","seed":42}},"duration_s":6.0,"tps":60,
            "invariants":[{{{json_invariant}}} ]}}"#
    );
    serde_json::from_str(&json).unwrap()
}

#[test]
fn cap_zero_is_rejected() {
    let s = occupancy_spec(
        r#""name": "t", "rule": "occupancy_cap",
           "target_selector": {"with_component": {"name": "CrowdMember"}},
           "radius": 1.0, "cap": 0, "window_s": 3.0"#,
    );
    let errs = lint_scenario(&s);
    assert!(
        errs.iter()
            .any(|e| matches!(e, ScenarioError::Rejected(m) if m.contains("cap must be >= 1"))),
        "{errs:?}"
    );
}

#[test]
fn cap_fraction_is_rejected_by_parse() {
    // cap is Option<u32>: serde rejects 3.5 and 3.0 at PARSE
    // time — never a float floor.
    let json = r#"{"bot":{"type":"chaos","seed":42},"duration_s":6.0,"tps":60,
        "invariants":[{"name": "t", "rule": "occupancy_cap",
           "target_selector": {"with_component": {"name": "CrowdMember"}},
           "radius": 1.0, "cap": 3.5, "window_s": 3.0}]}"#;
    assert!(serde_json::from_str::<Scenario>(json).is_err());
}

#[test]
fn missing_radius_cap_window_are_rejected() {
    // The required triplet, removed one at a time: each hole
    // must be NAMED in the rejection, not a generic parse
    // failure.
    for (spec, needle) in [
        (
            r#""name": "t", "rule": "occupancy_cap",
                "target_selector": {"with_component": {"name": "CrowdMember"}},
                "cap": 2, "window_s": 3.0"#,
            "'radius'",
        ),
        (
            r#""name": "t", "rule": "occupancy_cap",
                "target_selector": {"with_component": {"name": "CrowdMember"}},
                "radius": 1.0, "window_s": 3.0"#,
            "'cap'",
        ),
        (
            r#""name": "t", "rule": "occupancy_cap",
                "target_selector": {"with_component": {"name": "CrowdMember"}},
                "radius": 1.0, "cap": 2"#,
            "'window_s'",
        ),
    ] {
        let s = occupancy_spec(spec);
        let errs = lint_scenario(&s);
        assert!(
            errs.iter()
                .any(|e| matches!(&e, ScenarioError::Rejected(m) if m.contains(needle))),
            "expected {needle} rejection for [{spec}]: {errs:?}"
        );
    }
}

#[test]
fn nonpositive_radius_is_rejected() {
    for radius in [-1.0, 0.0] {
        let spec = format!(
            r#""name": "t", "rule": "occupancy_cap",
               "target_selector": {{"with_component": {{"name": "CrowdMember"}}}},
               "radius": {radius}, "cap": 2, "window_s": 3.0"#
        );
        let s = occupancy_spec(&spec);
        let errs = lint_scenario(&s);
        assert!(
            errs.iter()
                .any(|e| matches!(&e, ScenarioError::Rejected(m) if m.contains("radius"))),
            "{errs:?}"
        );
    }
}

#[test]
fn all_selector_is_rejected() {
    let s = occupancy_spec(
        r#""name": "t", "rule": "occupancy_cap",
           "radius": 1.0, "cap": 2, "window_s": 3.0"#,
    );
    let errs = lint_scenario(&s);
    assert!(
        errs.iter().any(
            |e| matches!(e, ScenarioError::Rejected(m) if m.contains("occupancy_cap requires"))
        ),
        "{errs:?}"
    );
}

#[test]
fn unelidable_window_is_rejected() {
    // window_s 100 driver-units at 1 unit/s = 100s > duration 6s.
    let s = occupancy_spec(
        r#""name": "t", "rule": "occupancy_cap",
           "target_selector": {"with_component": {"name": "CrowdMember"}},
           "radius": 1.0, "cap": 2, "window_s": 100.0"#,
    );
    let errs = lint_scenario(&s);
    assert!(
        errs.iter().any(
            |e| matches!(e, ScenarioError::Rejected(m) if m.contains("exceeds the run duration"))
        ),
        "{errs:?}"
    );
}

#[test]
fn driver_rate_without_driver_is_rejected() {
    let s = occupancy_spec(
        r#""name": "t", "rule": "occupancy_cap",
           "target_selector": {"with_component": {"name": "CrowdMember"}},
           "radius": 1.0, "cap": 2, "window_s": 3.0,
           "driver_units_per_second": 5.0"#,
    );
    let errs = lint_scenario(&s);
    assert!(
        errs.iter().any(|e| matches!(e, ScenarioError::Rejected(m) if m.contains("driver_units_per_second needs"))),
        "{errs:?}"
    );
}

#[test]
fn half_declared_driver_is_rejected() {
    // driver_path WITHOUT driver_resource: both halves or
    // neither (no `resource` to default the host to).
    let s = occupancy_spec(
        r#""name": "t", "rule": "occupancy_cap",
           "target_selector": {"with_component": {"name": "CrowdMember"}},
           "window_s": 3.0, "radius": 1.0, "cap": 2,
           "driver_path": "tick""#,
    );
    let errs = lint_scenario(&s);
    assert!(
        errs.iter().any(|e| matches!(e, ScenarioError::Rejected(m) if m.contains("both 'driver_resource' and 'driver_path'"))),
        "{errs:?}"
    );
}

#[test]
fn foreign_fields_are_rejected_on_occupancy() {
    // Fields belonging to OTHER rules must not be silently
    // accepted on occupancy_cap (the reserved-field doctrine).
    // Needles pin the owner each field is bounced back with.
    for (extra, needle) in [
        (
            r#", "check": "lt", "value": 1.0"#,
            "occupancy_cap does not use",
        ),
        (r#", "min_distance": 1.0"#, "occupancy_cap does not use"),
        (r#", "watch_path": "hp""#, "entity_stagnation only"),
        (r#", "sampling_s": 1.0"#, "entity_stagnation only"),
        (
            r#", "tolerance": 0.001"#,
            "derived_transform_consistency only",
        ),
        (r#", "spawn_only": true"#, "spawn_safety only"),
        (
            r#", "region": {"center": [0.0, 0.0, 0.0], "half_extents": [1.0, 1.0, 1.0]}"#,
            "forbidden_region only",
        ),
    ] {
        let spec = format!(
            r#""name": "t", "rule": "occupancy_cap",
               "target_selector": {{"with_component": {{"name": "CrowdMember"}}}},
               "radius": 1.0, "cap": 2, "window_s": 3.0{extra}"#
        );
        let s = occupancy_spec(&spec);
        let errs = lint_scenario(&s);
        assert!(
            errs.iter()
                .any(|e| matches!(&e, ScenarioError::Rejected(m) if m.contains(needle))),
            "expected [{needle}] for [{spec}]: {errs:?}"
        );
    }
}

#[test]
fn radius_cap_on_other_rules_are_rejected() {
    // The reserved-field hole in reverse: radius/cap on another
    // rule must be rejected, not silently ignored.
    let s = occupancy_spec(
        r#""name": "t", "rule": "pairwise_separation",
           "target_selector": {"with_component": {"name": "CrowdMember"}},
           "min_distance": 1.0, "window_s": 3.0,
           "radius": 2.0, "cap": 5"#,
    );
    let errs = lint_scenario(&s);
    assert!(
        errs.iter()
            .any(|e| matches!(e, ScenarioError::Rejected(m) if m.contains("occupancy_cap only"))),
        "{errs:?}"
    );
}

#[test]
fn missing_window_fit_check_uses_duration() {
    // Sanity on the echo: a fitting window with a declared rate
    // passes lint (fit arithmetic mirrors pairwise's).
    let s = occupancy_spec(
        r#""name": "t", "rule": "occupancy_cap",
           "target_selector": {"with_component": {"name": "CrowdMember"}},
           "radius": 1.0, "cap": 2, "window_s": 30.0,
           "driver_resource": "R", "driver_path": "tick",
           "driver_units_per_second": 10.0"#,
    );
    let errs = lint_scenario(&s);
    assert!(
        !errs.iter().any(
            |e| matches!(e, ScenarioError::Rejected(m) if m.contains("exceeds the run duration"))
        ),
        "{errs:?}"
    );
}

// ---------------------------------------------------------------------------
// Held trace: the census a passing run keeps (veto point 1e)
// ---------------------------------------------------------------------------

/// Held-trace admission: a within-cap population that was judged
/// every frame still earns a held entry — the census RAN, and the
/// trace must say so. Two members (each count 2 ≤ cap 3) over the
/// full 6.0s run: every frame populated, so `frames_matched` is
/// the full frame count (360 — the 6.0s run at 60 tps) and
/// `entity_judgments` is the per-frame checkable count summed
/// over those frames (2 members × 360 frames). The near-miss
/// fields stay empty: with nobody ever over cap, no episode ever
/// held a witness, so nothing was observed, no span banked.
#[test]
fn within_cap_population_earns_a_held_entry() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.world_mut()
        .spawn((CrowdMember, Transform::from_xyz(0.0, 0.0, 0.0)));
    app.world_mut()
        .spawn((CrowdMember, Transform::from_xyz(0.1, 0.0, 0.0)));
    let report = run_scenario(&mut app, &scenario_with_invariant(CROWD_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    let held = report
        .held_invariants
        .iter()
        .find(|h| h.name == "no_more_than_three_in_a_room")
        .expect("a judged, passing occupancy census earns a held entry");
    assert_eq!(held.rule, "occupancy_cap");
    assert_eq!(held.target, "CrowdMember");
    assert_eq!(held.predicate.as_deref(), Some("count le 3"));
    assert_eq!(held.window_s, Some(3.0));
    assert_eq!(held.frames_matched, 360);
    assert_eq!(held.entity_judgments, Some(2 * 360));
    assert_eq!(held.episodes_observed, Some(0));
    assert!(held.worst_episode_span.is_none());
    assert!(held.pair_judgments.is_none());
}

/// The near-miss story: a gathering DISSOLVED below the soak
/// (the transient the judgment discards) is exactly what the
/// held trace exists to keep. The dissolve-at-frame-100 fixture:
/// the episode opens at the first frame, closes when the
/// positions jump apart, and its banked span rides the entry
/// while the run still passes.
#[test]
fn sub_soak_gathering_banks_its_span_in_the_held_trace() {
    let _slot = common::run_slot();
    let mut app = base_app();
    spawn_standing_crowd(app.world_mut());
    app.add_systems(
        Update,
        |mut q: Query<&mut Transform, With<CrowdMember>>, mut clock: Local<f32>| {
            *clock += 1.0;
            if *clock > 100.0 {
                for (i, mut t) in q.iter_mut().enumerate() {
                    t.translation.x = 50.0 * (i as f32 + 1.0);
                    t.translation.y = 0.0;
                }
            }
        },
    );
    let report = run_scenario(&mut app, &scenario_with_invariant(CROWD_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    let held = report
        .held_invariants
        .iter()
        .find(|h| h.name == "no_more_than_three_in_a_room")
        .expect("the near-miss run keeps its held entry");
    assert_eq!(held.episodes_observed, Some(1));
    let (span, start_frame) = held
        .worst_episode_span
        .zip(held.worst_episode_frame)
        .expect("the sub-soak span and its start frame are witnessed");
    assert_eq!(start_frame, 0, "the gathering began with the run");
    // Frame clock driver: the span is seconds. The gathering ran
    // from frame 0 to the dissolve at ~101 — just under 1.7s,
    // well under the 3.0s soak.
    assert!(
        (1.0..3.0).contains(&span),
        "the banked sub-soak span: {span}"
    );
}

/// Fired-interplay exclusion (veto point 1e): the SOAKED crowd —
/// the headline's own fixture — produced a violations[] warning,
/// so it must NOT also appear as held evidence. The two sections
/// partition by verdict even for a warn-only rule: the warning
/// entry owns the story; a held twin would double-count it.
#[test]
fn fired_occupancy_gets_no_held_entry() {
    let _slot = common::run_slot();
    let mut app = base_app();
    spawn_standing_crowd(app.world_mut());
    let report = run_scenario(&mut app, &scenario_with_invariant(CROWD_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    assert!(
        report.violations.iter().any(|v| v.rule == "occupancy_cap"),
        "the soaked crowd warned"
    );
    assert!(
        !report
            .held_invariants
            .iter()
            .any(|h| h.name == "no_more_than_three_in_a_room"),
        "a fired occupancy must not also be held"
    );
}

/// Never-a-crowd exclusion: a selector with no matching entities
/// emitted the rule's own informational note — it never judged a
/// crowd, so no held entry may claim a census workload. The note
/// and the empty held trace tell the same honest story.
#[test]
fn never_a_crowd_census_gets_no_held_entry() {
    let _slot = common::run_slot();
    let mut app = base_app();
    let report = run_scenario(&mut app, &scenario_with_invariant(CROWD_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    assert!(
        report.violations.iter().any(|v| v.rule == "occupancy_cap"),
        "the never-a-crowd note fired under the rule's own name"
    );
    assert!(
        !report
            .held_invariants
            .iter()
            .any(|h| h.name == "no_more_than_three_in_a_room"),
        "a census that never had a crowd must not claim held evidence"
    );
}
