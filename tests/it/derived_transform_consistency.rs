//! Behavioral battery for the `derived_transform_consistency` rule:
//! the compositional-identity spatial oracle. Every parented
//! entity's GlobalTransform must equal its parent's GlobalTransform
//! composed with its own Transform, both read at the same settled,
//! post-propagation frame — the internal-consistency contract, not
//! "did the game call propagate". Violating frames fold into
//! per-entity episodes and only an episode soaking past `window_s`
//! (in driver units) becomes a finding.
//!
//! Warn-phase doctrine, retired: findings rode the report without
//! gating until flip-test evidence promoted the rule (proposal
//! §2) — the pile-up RED/GREEN pair demonstrated the
//! discrimination, and a soaked divergence now fails the run.
//! `hierarchy_absent` stays informational — a flat world is a
//! coherent scene; nothing was judged.
//!
//! The planted-defect shape mirrors the pile-up incident class on
//! record (proposal §1: a Transform edited without propagation):
//! a child whose GlobalTransform is foreign-written (stuck at
//! identity) under a moving parent stays incoherent every settled
//! frame; a healthy propagated hierarchy is exactly coherent.
//!
//! The flip-test ladder anchors the rule's earned trust (proposal
//! §2): the multi-child pile RED / propagated-pile GREEN pair
//! proves the rule discriminates the incident class, and the
//! rotating/scaling hierarchy guards the identity math's
//! false-positive flank.

use crate::common;

use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_playtest::driver::run_scenario;
use bevy_playtest::plugin::SkipDerivedConsistency;
use bevy_reflect::Reflect;
use bevy_transform::prelude::{GlobalTransform, Transform, TransformPlugin, TransformSystems};
/// Marker component acting as the consistency selector (the same
/// doctrine as pairwise's `Enemy`).
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct Cabin;

fn scenario_with_invariant(inv: &str) -> bevy_playtest::scenario::Scenario {
    let json = format!(
        r#"{{"bot":{{"type":"chaos","seed":42}},"duration_s":4.0,"tps":60,
            "invariants":[{{{inv}}}]}}"#
    );
    serde_json::from_str(&json).unwrap()
}

/// Move every ROOT transform (no ChildOf) monotonically along x:
/// the world keeps changing (frozen_world stays quiet) and the
/// parent-child pair keeps recomposing, so only the compositional
/// identity is ever in question.
fn move_roots(mut q: Query<&mut Transform, Without<ChildOf>>) {
    for mut t in q.iter_mut() {
        t.translation.x += 0.05;
    }
}

/// THE planted defect: a late-frame writer pins the child's global
/// at identity AFTER propagation (the pile-up shape — a
/// foreign write that re-lands every frame, so no next-frame
/// propagation ever reconciles it).
fn pin_child_global_to_identity(mut q: Query<&mut GlobalTransform, With<Cabin>>) {
    for mut g in q.iter_mut() {
        *g = GlobalTransform::from_xyz(0.0, 0.0, 0.0);
    }
}

/// The ghost-writer variant: same shape as the pin but writing a
/// far-away constant, for the unmarked/exclude/marker fixtures.
fn scatter_globals<G: Component>(mut q: Query<&mut GlobalTransform, With<G>>) {
    for mut g in q.iter_mut() {
        *g = GlobalTransform::from_xyz(9.0, 9.0, 9.0);
    }
}

/// The canon subject: parent moving monotonically along x, child
/// riding at the origin — healthy propagation keeps the identity
/// exact frame over frame. TaskPoolPlugin: TransformPlugin's
/// propagation systems run parallel queries, which need the
/// ComputeTaskPool the driver otherwise never initializes in a
/// bare App (ui_surface.rs precedent).
fn base_app() -> App {
    let mut app = App::new();
    app.add_plugins((TaskPoolPlugin::default(), TransformPlugin));
    app.register_type::<Cabin>();
    app
}

fn healthy_hierarchy_fixture() -> App {
    let mut app = base_app();
    app.add_systems(Update, move_roots);
    let parent = app
        .world_mut()
        .spawn(Transform::from_xyz(0.0, 0.0, 0.0))
        .id();
    app.world_mut()
        .spawn((Cabin, Transform::from_xyz(0.0, 0.0, 0.0), ChildOf(parent)));
    app
}

fn add_defect(app: &mut App) {
    app.add_systems(
        PostUpdate,
        (pin_child_global_to_identity,).after(TransformSystems::Propagate),
    );
}

const CONSISTENCY_INV: &str = r#"
    "name": "globals_follow_parents",
    "rule": "derived_transform_consistency",
    "target_selector": {"with_component": {"name": "Cabin"}},
    "window_s": 1.0"#;

// ---------------------------------------------------------------------------
// The compositional identity
// ---------------------------------------------------------------------------

/// Backing resource for the transient tests: counts PostUpdate
/// executions so the foreign write lands on a chosen JUDGED
/// frame (post-grace) — a blip inside the grace window is never
/// observed by the recorder, and an unobserved blip proves
/// nothing about the soak.
#[derive(Resource, Default)]
struct BlipClock {
    frame: u32,
    fired: bool,
}

/// One-frame variant: the foreign write lands on frame 10
/// (judged), corrected by the next frame's propagation.
fn blip_once(mut q: Query<&mut GlobalTransform, With<Cabin>>, mut clock: ResMut<BlipClock>) {
    clock.frame += 1;
    if clock.frame == 10 && !clock.fired {
        clock.fired = true;
        for mut g in q.iter_mut() {
            *g = GlobalTransform::from_xyz(9.0, 9.0, 9.0);
        }
    }
}

/// Multi-frame variant: a 20-frame (1/3s) divergence streak on
/// frames 10–29 — observed, folded, but sub-soak.
fn blip_for_a_third_of_a_second(
    mut q: Query<&mut GlobalTransform, With<Cabin>>,
    mut clock: ResMut<BlipClock>,
) {
    clock.frame += 1;
    // Frames 10-29: past grace, well inside the observation
    // window, but a 0.317s span against the 1.0s soak.
    if (10..30).contains(&clock.frame) {
        for mut g in q.iter_mut() {
            *g = GlobalTransform::from_xyz(9.0, 9.0, 9.0);
        }
    }
}

/// THE incident shape: the child's GlobalTransform is
/// foreign-written to identity while its parent keeps moving —
/// the stored pair is incoherent every settled frame, the episode
/// soaks past the window, and the finding GATES the run (the rule
/// is promoted: flip-test evidence demonstrated the pile-up
/// discrimination, proposal §2).
#[test]
fn stale_child_global_soak_fails_the_run() {
    let _slot = common::run_slot();
    let mut app = healthy_hierarchy_fixture();
    add_defect(&mut app);
    let report = run_scenario(&mut app, &scenario_with_invariant(CONSISTENCY_INV)).unwrap();
    // Promoted: a soaked divergence failure fails the run.
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    let v = report
        .violations
        .iter()
        .find(|v| v.rule == "derived_transform_consistency")
        .expect("expected a consistency finding");
    assert!(v.detail.contains("diverged"), "{}", v.detail);
    // The `_meta` episode evidence: one distinct entity, not
    // truncated, with the worst deviation recorded.
    let meta = v.meta.as_ref().expect("episode finding carries _meta");
    let dtc = meta
        .consistency_episode
        .as_ref()
        .expect("_meta carries the consistency episode block");
    assert_eq!(dtc.sampled_entities, 1, "one distinct entity");
    assert!(!dtc.roster_truncated);
    assert_eq!(dtc.roster.len(), 1);
    assert!(
        dtc.roster[0].worst_deviation > 0.0,
        "worst deviation: {}",
        dtc.roster[0].worst_deviation
    );
    assert!(dtc.roster[0].violating_frames >= 1);
}

/// Control: a propagated hierarchy is exactly coherent — the same
/// moving parent, no foreign writer, no finding at all.
#[test]
fn healthy_hierarchy_finds_nothing() {
    let _slot = common::run_slot();
    let mut app = healthy_hierarchy_fixture();
    let report = run_scenario(&mut app, &scenario_with_invariant(CONSISTENCY_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    assert!(!report
        .violations
        .iter()
        .any(|v| v.rule == "derived_transform_consistency"));
}

/// The transient doctrine, observed: a foreign write landing on a
/// JUDGED frame (post-grace, frame 10) and corrected by the next
/// frame's propagation makes a single-frame episode — its
/// frame-clock span is 0, sub-soak, discarded. The blip must land
/// past the default 3-frame grace (frames 0–2 are never observed,
/// so a grace-interval blip would prove nothing about the soak),
/// and the run lasts long enough for the correction to actually
/// occur.
#[test]
fn one_frame_blip_is_discarded_by_the_soak() {
    let _slot = common::run_slot();
    let mut app = healthy_hierarchy_fixture();
    app.insert_resource(BlipClock::default());
    app.add_systems(PostUpdate, blip_once.after(TransformSystems::Propagate));
    let report = run_scenario(&mut app, &scenario_with_invariant(CONSISTENCY_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.rule == "derived_transform_consistency"),
        "a one-frame blip must not soak past the window: {:?}",
        report.violations
    );
}

/// The sub-soak discard is doing the work (control for the blip
/// test): the same foreign writer running for SEVERAL judged
/// frames — an observed, folded episode — stays below the 1.0s
/// window and is still discarded. Distinguishes "the blip was
/// never observed" (a vacuous pass) from "the soak machinery
/// actively suppressed an observed episode".
#[test]
fn multi_frame_subsoak_episode_is_discarded_too() {
    let _slot = common::run_slot();
    let mut app = healthy_hierarchy_fixture();
    app.insert_resource(BlipClock::default());
    app.add_systems(
        PostUpdate,
        blip_for_a_third_of_a_second.after(TransformSystems::Propagate),
    );
    let report = run_scenario(&mut app, &scenario_with_invariant(CONSISTENCY_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.rule == "derived_transform_consistency"),
        "a 1/3-second streak must stay sub-soak against the 1.0s window: {:?}",
        report.violations
    );
}

// ---------------------------------------------------------------------------
// Flip-test evidence: the promotion ladder (proposal §2)
//
// The rule's findings stay non-gating until a demonstrated
// flip-test proves the rule discriminates the pile-up incident
// class: the same hierarchy must come out RED with the defect
// present and GREEN with it absent. The two pile tests below are
// that pair (one shared fixture builder, one branch), and the
// rotation/scale test guards the identity math's false-positive
// flank: `mul_transform` composition plus the affine-column
// deviation must hold EXACTLY for non-translation parents, where
// float error grows fastest.
// ---------------------------------------------------------------------------

/// Every AUTHORED Transform advances monotonically — root and
/// children alike — the incident class's defining motion: the
/// authored local poses keep moving while the stored globals are
/// the thing under test.
fn advance_authored_transforms(mut q: Query<&mut Transform>) {
    for mut t in q.iter_mut() {
        t.translation.x += 0.05;
    }
}

/// THE flip-test RED half: a pile of foreign-written children.
/// Six `Cabin` children under one moving parent, every one of
/// them pinned at identity after propagation — the incident-class
/// shape (authored Transforms advancing, every GlobalTransform
/// stuck), not the single-child defect. Each child must fold
/// into its own roster entry: the episode evidence has to name
/// every diverging entity individually, not just the worst one.
/// After the promotion the soaked pile GATES the run.
#[test]
fn multi_child_pile_soaks_every_child_into_the_roster() {
    let _slot = common::run_slot();
    let mut app = pileup_fixture(true);
    let report = run_scenario(&mut app, &scenario_with_invariant(CONSISTENCY_INV)).unwrap();
    // Promoted: the soaked pile-up episode is a gating failure.
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    let findings: Vec<_> = report
        .violations
        .iter()
        .filter(|v| v.rule == "derived_transform_consistency")
        .collect();
    assert_eq!(findings.len(), 1, "one soaked episode, one finding");
    let v = findings[0];
    assert!(v.detail.contains("diverged"), "{}", v.detail);
    // ...and the finding names the crowd, not just its worst
    // member.
    assert!(v.detail.contains("6 diverging entities"), "{}", v.detail);
    let meta = v.meta.as_ref().expect("episode finding carries _meta");
    let dtc = meta
        .consistency_episode
        .as_ref()
        .expect("_meta carries the consistency episode block");
    assert_eq!(dtc.sampled_entities, 6, "six distinct diverging entities");
    assert!(!dtc.roster_truncated, "six fits the roster cap");
    assert_eq!(dtc.roster.len(), 6, "every piled child gets an entry");
    let mut named = std::collections::HashSet::new();
    for entry in &dtc.roster {
        assert!(
            named.insert(entry.entity),
            "roster entry {:?} appears twice",
            entry.entity
        );
        assert!(
            entry.worst_deviation > 0.0,
            "entry {:?} worst deviation: {}",
            entry.entity,
            entry.worst_deviation
        );
        assert!(entry.violating_frames >= 1);
    }
}

/// The truncation disclosure, exercised: a pile BEYOND the roster
/// cap (33 children against the cap of 32) still names every
/// diverging entity in `sampled_entities` — the count is the
/// honest population — while the per-entity roster carries only
/// the cap's worth, flagged `roster_truncated`. The evidence must
/// not silently lose entities nor silently shorten the count.
#[test]
fn pile_beyond_the_roster_cap_discloses_truncation() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.add_systems(Update, advance_authored_transforms);
    let parent = app
        .world_mut()
        .spawn(Transform::from_xyz(0.0, 0.0, 0.0))
        .id();
    for i in 0..33u32 {
        app.world_mut().spawn((
            Cabin,
            Transform::from_xyz(i as f32 * 0.5, i as f32 * 0.25, 0.0),
            ChildOf(parent),
        ));
    }
    add_defect(&mut app);
    let report = run_scenario(&mut app, &scenario_with_invariant(CONSISTENCY_INV)).unwrap();
    // Promoted: the soaked pile gates the run.
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    let v = report
        .violations
        .iter()
        .find(|v| v.rule == "derived_transform_consistency")
        .expect("expected a consistency finding");
    assert!(v.detail.contains("33 diverging entities"), "{}", v.detail);
    let meta = v.meta.as_ref().expect("episode finding carries _meta");
    let dtc = meta
        .consistency_episode
        .as_ref()
        .expect("_meta carries the consistency episode block");
    // The count is honest...
    assert_eq!(dtc.sampled_entities, 33, "every diverging entity counted");
    // ...the roster is capped...
    assert_eq!(
        dtc.roster.len(),
        bevy_playtest::episode::DEFAULT_ROSTER_CAP,
        "roster stops at the cap"
    );
    assert!(dtc.roster_truncated, "truncation is disclosed");
    // ...and every carried entry is real evidence.
    let mut named = std::collections::HashSet::new();
    for entry in &dtc.roster {
        assert!(
            named.insert(entry.entity),
            "roster entry {:?} appears twice",
            entry.entity
        );
        assert!(entry.worst_deviation > 0.0);
    }
}

/// THE flip-test GREEN half: the identical pile, defect absent —
/// the same six children, the same advancing authored Transforms,
/// propagation reconciling every frame. A rule that fired here
/// would be false-positive noise, and the pair discriminates:
/// defect-present RED, defect-absent GREEN.
#[test]
fn propagated_pile_of_children_finds_nothing() {
    let _slot = common::run_slot();
    let mut app = pileup_fixture(false);
    let report = run_scenario(&mut app, &scenario_with_invariant(CONSISTENCY_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.rule == "derived_transform_consistency"),
        "a propagated pile must stay coherent: {:?}",
        report.violations
    );
}

/// The flip-test fixture builder: one moving parent, six piled
/// `Cabin` children with distinct authored offsets, and — on the
/// defect branch — the late-frame foreign writer pinning every
/// child's global at identity. One builder, two tests: the RED
/// and GREEN halves differ ONLY in whether the defect runs.
fn pileup_fixture(defect: bool) -> App {
    let mut app = base_app();
    app.add_systems(Update, advance_authored_transforms);
    let parent = app
        .world_mut()
        .spawn(Transform::from_xyz(0.0, 0.0, 0.0))
        .id();
    for i in 0..6u32 {
        app.world_mut().spawn((
            Cabin,
            Transform::from_xyz(i as f32 * 0.5, i as f32 * 0.25, 0.0),
            ChildOf(parent),
        ));
    }
    if defect {
        add_defect(&mut app);
    }
    app
}

/// Marker for the spin/scale rig: a Transform its owners animate
/// with rotation and scale every frame — the composed pose where
/// naive parent-vs-child comparisons accumulate float error the
/// fastest. Never named by any scenario; local to the rig.
#[derive(Component, Default)]
struct SpinsAndScales;

/// Rotate and grow every tagged Transform per frame: the
/// non-translation motion the identity math must survive. The
/// accumulated pose compounds across the hierarchy, so any
/// reconstruction that ISN'T propagation's own `mul_transform`
/// arithmetic would drift visibly within a few dozen frames.
fn spin_and_grow(mut q: Query<&mut Transform, With<SpinsAndScales>>) {
    for mut t in q.iter_mut() {
        t.rotate_z(0.02);
        t.scale *= 1.005;
    }
}

/// The identity math's false-positive flank: a healthy hierarchy
/// whose parents ROTATE and SCALE every frame, at varied depths
/// (a depth-2 leaf under the root, depth-3 leaves under a
/// rotating middle hop). Propagation is exactly coherent frame
/// over frame — the elementwise affine-column contract must not
/// mistake compounded rotation/scale motion for divergence.
#[test]
fn rotating_scaling_hierarchy_stays_coherent() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.add_systems(Update, move_roots);
    app.add_systems(Update, spin_and_grow);
    let root = app
        .world_mut()
        .spawn((SpinsAndScales, Transform::from_xyz(0.0, 0.0, 0.0)))
        .id();
    // A rotating, scaling middle hop under the root...
    let mid = app
        .world_mut()
        .spawn((
            SpinsAndScales,
            Transform::from_xyz(2.0, 1.0, 0.0),
            ChildOf(root),
        ))
        .id();
    // ...carrying two depth-3 leaves...
    for (x, y) in [(1.0, 0.0), (0.0, 2.0)] {
        app.world_mut()
            .spawn((Cabin, Transform::from_xyz(x, y, 0.5), ChildOf(mid)));
    }
    // ...and one depth-2 leaf straight under the root.
    app.world_mut()
        .spawn((Cabin, Transform::from_xyz(0.0, 0.0, -1.0), ChildOf(root)));
    let report = run_scenario(&mut app, &scenario_with_invariant(CONSISTENCY_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.rule == "derived_transform_consistency"),
        "compounded rotation and scale motion is coherent when propagated: {:?}",
        report.violations
    );
}

// ---------------------------------------------------------------------------
// The opt-outs: selector scoping, exclude prefixes, the marker
// ---------------------------------------------------------------------------

/// A defect outside the selector's scope is not this rule's
/// story: the diverging child does NOT carry `Cabin`, so the
/// scoped rule never checks it.
#[test]
fn defect_outside_the_selector_is_ignored() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.add_plugins((TaskPoolPlugin::default(), TransformPlugin));
    app.register_type::<Cabin>();
    app.add_systems(Update, move_roots);
    let parent = app
        .world_mut()
        .spawn(Transform::from_xyz(0.0, 0.0, 0.0))
        .id();
    // The healthy, selected child...
    app.world_mut()
        .spawn((Cabin, Transform::from_xyz(0.0, 0.0, 0.0), ChildOf(parent)));
    // ...and a second, unmarked, permanently diverging child.
    app.world_mut()
        .spawn((Transform::from_xyz(0.0, 0.0, 0.0), ChildOf(parent)));
    app.add_systems(
        PostUpdate,
        (scatter_globals::<NotCabin>,).after(TransformSystems::Propagate),
    );
    let report = run_scenario(&mut app, &scenario_with_invariant(CONSISTENCY_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.rule == "derived_transform_consistency"),
        "the unscoped defect must stay out of the scoped rule's findings: {:?}",
        report.violations
    );
}

/// Helper marker for the scope test's diverging child: NOT the
/// selector, and not registered (no reflect needed — never named
/// by any scenario).
#[derive(Component, Default)]
struct NotCabin;

/// The excluded ghost type: the camera-smoothing component's
/// shape. Module-level so its full type path is stable for the
/// exclude_components PREFIX (exclusion matches registered type
/// paths, so a locally-scoped struct's path would embed the test
/// function's module — unwritable ahead of time).
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct InterpGhost;

/// The exclude prefix grammar: a diverging child carrying an
/// excluded COMPONENT TYPE (the camera-smoothing ghost's shape)
/// is opted out by type-path prefix.
#[test]
fn exclude_prefix_opts_out_the_ghost_type() {
    let _slot = common::run_slot();

    let mut app = base_app();
    app.register_type::<InterpGhost>();
    app.add_systems(Update, move_roots);
    let parent = app
        .world_mut()
        .spawn(Transform::from_xyz(0.0, 0.0, 0.0))
        .id();
    app.world_mut().spawn((
        Cabin,
        InterpGhost,
        Transform::from_xyz(0.0, 0.0, 0.0),
        ChildOf(parent),
    ));
    app.add_systems(
        PostUpdate,
        (scatter_globals::<InterpGhost>,).after(TransformSystems::Propagate),
    );
    // Exclude prefixes match REGISTERED TYPE PATHS, so the prefix
    // must be the ghost's full path, not its short name.
    let inv = format!(
        r#"
    "name": "globals_follow_parents",
    "rule": "derived_transform_consistency",
    "target_selector": {{"with_component": {{"name": "Cabin"}}}},
    "exclude_components": ["{}"],
    "window_s": 1.0"#,
        std::any::type_name::<InterpGhost>()
    );
    let scen = scenario_with_invariant(&inv);
    let report = run_scenario(&mut app, &scen).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.rule == "derived_transform_consistency"),
        "an excluded ghost type must never become a finding: {:?}",
        report.violations
    );
}

/// The per-instance marker: the same diverging child, tagged with
/// the harness-registered `SkipDerivedConsistency`, is invisible
/// to every selection mode.
#[test]
fn skip_marker_opts_the_entity_out() {
    let _slot = common::run_slot();
    let mut app = healthy_hierarchy_fixture();
    // Tag the (only) Cabin child with the marker.
    let mut q = app.world_mut().query_filtered::<Entity, With<Cabin>>();
    let child = q.single(app.world()).unwrap();
    app.world_mut()
        .entity_mut(child)
        .insert(SkipDerivedConsistency);
    app.add_systems(
        PostUpdate,
        (scatter_globals::<Cabin>,).after(TransformSystems::Propagate),
    );
    let report = run_scenario(&mut app, &scenario_with_invariant(CONSISTENCY_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.rule == "derived_transform_consistency"),
        "a marked entity must never become a finding: {:?}",
        report.violations
    );
}

// ---------------------------------------------------------------------------
// Vacuity: the flat world
// ---------------------------------------------------------------------------

/// A FLAT world (no parented entities at all) means the rule never
/// found a parented entity — `hierarchy_absent`, INFORMATIONAL,
/// never a gating vacuous_invariant.
#[test]
fn flat_world_is_hierarchy_absent_not_vacuous() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.add_plugins((TaskPoolPlugin::default(), TransformPlugin));
    app.register_type::<Cabin>();
    // A flat but observable world: one moving un-parented entity.
    app.add_systems(Update, move_roots);
    app.world_mut()
        .spawn((Cabin, Transform::from_xyz(0.0, 1.0, 0.0)));
    let scen = scenario_with_invariant(
        r#"
    "name": "globals_follow_parents",
    "rule": "derived_transform_consistency",
    "window_s": 1.0"#,
    );
    let report = run_scenario(&mut app, &scen).unwrap();
    // Informational: the status stays Pass.
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    let v = report
        .violations
        .iter()
        .find(|v| v.rule == "hierarchy_absent" && v.target == "globals_follow_parents")
        .expect("hierarchy_absent names the invariant");
    assert!(
        v.detail.contains("never found a parented entity"),
        "{}",
        v.detail
    );
    // And never the gating vacuity.
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.rule == "vacuous_invariant" && v.target == "globals_follow_parents"),
        "a flat world must not be a vacuous_invariant failure"
    );
}

// ---------------------------------------------------------------------------
// Config errors
// ---------------------------------------------------------------------------

/// An unregistered selector type is an authoring mistake:
/// `oracle_error` (config), never the game's fault, and never a
/// double-reporting vacuity.
#[test]
fn unregistered_selector_reports_config_error() {
    let _slot = common::run_slot();
    let mut app = healthy_hierarchy_fixture();
    let scen = scenario_with_invariant(
        r#"
    "name": "globals_follow_parents",
    "rule": "derived_transform_consistency",
    "target_selector": {"with_component": {"name": "NoSuchComponent"}},
    "window_s": 1.0"#,
    );
    let report = run_scenario(&mut app, &scen).unwrap();
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.rule == "oracle_error" && v.target == "globals_follow_parents"),
        "unregistered selector must be an oracle_error naming the invariant"
    );
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.rule == "vacuous_invariant" && v.target == "globals_follow_parents"),
        "config-errored invariant must not double-report vacuity"
    );
}

// ---------------------------------------------------------------------------
// Metadata serialization smoke
// ---------------------------------------------------------------------------

/// The `_meta.consistency_episode` block serializes with stable
/// field names — consumers parse it as structured data.
#[test]
fn episode_meta_serializes_stably() {
    let _slot = common::run_slot();
    let mut app = healthy_hierarchy_fixture();
    add_defect(&mut app);
    let report = run_scenario(&mut app, &scenario_with_invariant(CONSISTENCY_INV)).unwrap();
    let v = report
        .violations
        .iter()
        .find(|v| v.rule == "derived_transform_consistency")
        .expect("finding present");
    let meta = v.meta.as_ref().expect("_meta present");
    let json = serde_json::to_value(meta).unwrap();
    let dtc = json
        .get("consistency_episode")
        .expect("consistency_episode key present");
    for key in ["sampled_entities", "roster", "roster_truncated"] {
        assert!(dtc.get(key).is_some(), "episode key {key}");
    }
    let entry = dtc
        .get("roster")
        .and_then(|r| r.get(0))
        .expect("roster non-empty");
    for key in [
        "entity",
        "first_frame",
        "worst_deviation",
        "worst_frame",
        "violating_frames",
    ] {
        assert!(entry.get(key).is_some(), "roster entry key {key}");
    }
}

// ---------------------------------------------------------------------------
// Note vocabulary: flat world vs unjudgeable parent (anti-false-confidence)
// ---------------------------------------------------------------------------

/// THE incident shape: a parented child holds the full readable
/// triple, but its parent exposes NO GlobalTransform over the whole
/// run — the walk can neither confirm nor deny the identity, and
/// the old vocabulary folded this world into the flattering
/// `hierarchy_absent`. The audit must now say
/// `unjudgeable_parent` instead.
#[test]
fn unjudgeable_parent_world_is_not_reported_flat() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.add_systems(Update, move_roots);
    // Parent WITHOUT GlobalTransform: removed after spawn so the
    // required-components machinery cannot re-add it.
    let parent = app
        .world_mut()
        .spawn(Transform::from_xyz(0.0, 0.0, 0.0))
        .id();
    app.world_mut()
        .entity_mut(parent)
        .remove::<GlobalTransform>();
    app.world_mut()
        .spawn((Cabin, Transform::from_xyz(0.0, 0.0, 0.0), ChildOf(parent)));
    let scen = scenario_with_invariant(
        r#"
    "name": "globals_follow_parents",
    "rule": "derived_transform_consistency",
    "window_s": 1.0"#,
    );
    let report = run_scenario(&mut app, &scen).unwrap();
    // The RED assertion: the world is NOT flat — the note vocabulary
    // must distinguish it from a coherent flat scene.
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.rule == "unjudgeable_parent" && v.target == "globals_follow_parents"),
        "an unjudgeable parent must never read as `hierarchy_absent`: {:?}",
        report.violations
    );
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.rule == "hierarchy_absent"),
        "the unjudgeable-parent world is not a flat world: {:?}",
        report.violations
    );
    // Informational: the run itself must PASS.
    assert_eq!(
        report.status,
        bevy_playtest::enums::PlaytestStatus::Pass,
        "unjudgeable_parent is informational: {:?}",
        report.violations
    );
}

/// The partial-blindness shape: the identity was judged for a
/// readable subtree AND at least one in-scope parent stayed
/// unreadable — the audit discloses both facts, once, instead of
/// letting the unchecked subtree vanish into the green.
#[test]
fn partial_blindness_is_disclosed_when_some_subtrees_judged() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.add_systems(Update, move_roots);
    // A healthy, fully readable pair.
    let readable_parent = app
        .world_mut()
        .spawn(Transform::from_xyz(0.0, 0.0, 0.0))
        .id();
    app.world_mut().spawn((
        Cabin,
        Transform::from_xyz(0.0, 0.0, 0.0),
        ChildOf(readable_parent),
    ));
    // An unreadable-parent pair: parent stripped of GlobalTransform
    // after spawn (required components would otherwise re-add it).
    let unreadable_parent = app
        .world_mut()
        .spawn(Transform::from_xyz(1.0, 0.0, 0.0))
        .id();
    app.world_mut()
        .entity_mut(unreadable_parent)
        .remove::<GlobalTransform>();
    app.world_mut().spawn((
        Cabin,
        Transform::from_xyz(0.0, 0.0, 0.0),
        ChildOf(unreadable_parent),
    ));
    let scen = scenario_with_invariant(
        r#"
    "name": "globals_follow_parents",
    "rule": "derived_transform_consistency",
    "window_s": 1.0"#,
    );
    let report = run_scenario(&mut app, &scen).unwrap();
    // Both facts disclosed: the readable pair was judged (so no
    // hierarchy_absent, no vacuous anything) and the unreadable
    // subtree is named once, informationally.
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.rule == "hierarchy_absent"),
        "a judged subtree exists: {:?}",
        report.violations
    );
    let notes = report
        .violations
        .iter()
        .filter(|v| v.rule == "unjudgeable_parent")
        .count();
    assert_eq!(
        notes, 1,
        "exactly one disclosure, not per-frame spam: {:?}",
        report.violations
    );
    assert_eq!(
        report.status,
        bevy_playtest::enums::PlaytestStatus::Pass,
        "partial blindness is informational: {:?}",
        report.violations
    );
}

/// The GREEN control for the vocabulary: a coherent, fully
/// readable hierarchy produces NEITHER note — a world judged
/// everywhere must read as fully judged, not partially blind.
#[test]
fn coherent_hierarchy_yields_no_vocabulary_notes() {
    let _slot = common::run_slot();
    let mut app = healthy_hierarchy_fixture();
    let report = run_scenario(&mut app, &scenario_with_invariant(CONSISTENCY_INV)).unwrap();
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.rule == "hierarchy_absent" || v.rule == "unjudgeable_parent"),
        "a fully judged coherent hierarchy carries no vocabulary notes: {:?}",
        report.violations
    );
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
}
