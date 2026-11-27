//! Behavioral battery for the `forbidden_region` rule: the
//! declarative no-go-zone oracle. Selected entities must not be
//! inside an author-declared box region; incursion frames fold
//! into episodes. With `window_s` declared (the soak form), only
//! an episode soaking past the window (in driver units) becomes
//! a finding — sub-soak incursions are transient fly-bys,
//! discarded. With `window_s` omitted (the instant-fire form),
//! every closed incursion episode is a finding — no minimum
//! dwell, the one-frame clip counts.
//!
//! The central scenario mirrors the rail-tunneling incident
//! shape: a marked walker that wanders into the forbidden box and
//! lingers. Generic oracles stay quiet (the world keeps changing,
//! everything stays in bounds) — only the incursion soak can name
//! the tunneling.

use crate::common;

use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_playtest::driver::run_scenario;
use bevy_playtest::scenario::Scenario;
use bevy_reflect::Reflect;
use bevy_transform::prelude::Transform;

/// Marker component acting as the region selector. Reflect is
/// required for the recorder's dynamic-type resolution (same
/// doctrine as the pairwise selector). Uniquely named: other
/// suites register their own markers, and short-name resolution
/// is ambiguous across same-named types.
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct Wanderer;

/// The legitimate-presence exemption: an entity mounted on the
/// outer lane legally overlaps the rail-surface region (the
/// allow_if flip-test's GOOD half — it exempts legitimate
/// presence, never the buggy state).
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct LaneMounted;

/// A game clock resource usable as the DECLARED driver leaf, in
/// minutes (a non-seconds unit proving the soak never converts
/// through tps). Uniquely named for the same reason as the
/// markers.
#[derive(Resource, Reflect, Default)]
#[reflect(Resource)]
struct FrMatchClock {
    minutes: f64,
}

fn scenario_with_invariant(inv: &str) -> Scenario {
    let json = format!(
        r#"{{"bot":{{"type":"chaos","seed":42}},"duration_s":4.0,"tps":60,
            "invariants":[{{{inv}}}]}}"#
    );
    serde_json::from_str(&json).unwrap()
}

/// The forbidden box: centered at x=0, extending 0.5 along x/z and
/// 1.0 along y. Half-extents are generous on y so vertical wobble
/// cannot mask or fake incursions.
const REGION_INV: &str = r#"
    "name": "no_rail_tunneling",
    "rule": "forbidden_region",
    "target_selector": {"with_component": {"name": "Wanderer"}},
    "region": {"center": [0.0, 0.5, 0.0], "half_extents": [0.5, 1.0, 0.5]},
    "window_s": 1.0"#;

/// The lounging walk: step x toward the center, then lounge with a
/// micro-wiggle driven by a FRAME-phase counter (never by x — a
/// wiggle derived from a settled x is a constant, and a constant
/// transform trips frozen_world). Amplitude ≪ the region's faces so
/// incursion state never flickers.
fn lounging_walk(mut q: Query<&mut Transform, With<Wanderer>>, mut phase: Local<f32>) {
    *phase += 0.37;
    for mut t in q.iter_mut() {
        t.translation.x += if t.translation.x < 0.0 { 0.1 } else { 0.0 };
        t.translation.z = phase.sin() * 0.01;
    }
}

/// The lounging fixture: one wanderer walks to the region center
/// and lounges. Spawned at x=-1.0, so it is planted OUTSIDE the
/// region — the frame-0 test overrides the spawn to inside.
fn lounging_fixture() -> App {
    let mut app = App::new();
    app.register_type::<Wanderer>();
    app.register_type::<Transform>();
    app.add_systems(Update, lounging_walk);
    app.world_mut()
        .spawn((Wanderer, Transform::from_xyz(-1.0, 0.5, 0.0)));
    app
}

// ---------------------------------------------------------------------------
// The tunneling shape
// ---------------------------------------------------------------------------

/// THE incident shape: the walker crosses into the box and
/// LOUNGES there — inside from the earliest observed frames and
/// staying for most of the run, well past the 1.0s soak. One
/// finding, naming the deepest incursion, with the episode
/// evidence in `_meta`.
#[test]
fn lounging_wanderer_fails_the_soak() {
    let _slot = common::run_slot();
    let mut app = lounging_fixture();
    let report = run_scenario(&mut app, &scenario_with_invariant(REGION_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    let v = report
        .violations
        .iter()
        .find(|v| v.rule == "forbidden_region")
        .expect("expected a forbidden_region finding");
    assert!(
        v.detail.contains("entered the forbidden region"),
        "{}",
        v.detail
    );
    // The `_meta` roster evidence: one distinct incursive entity,
    // not truncated, with the worst depth recorded.
    let meta = v.meta.as_ref().expect("episode finding carries _meta");
    let fr = meta
        .forbidden_episode
        .as_ref()
        .expect("_meta carries the forbidden episode block");
    assert_eq!(fr.sampled_entities, 1, "one distinct incursive entity");
    assert!(!fr.roster_truncated);
    assert_eq!(fr.roster.len(), 1);
    // At the box center the depth equals the smallest half-extent
    // (0.5): the worst depth observed while lounging near center.
    assert!(
        fr.roster[0].worst_depth > 0.2 && fr.roster[0].worst_depth <= 0.5 + 1e-3,
        "worst depth: {}",
        fr.roster[0].worst_depth
    );
    assert!(fr.roster[0].violating_frames >= 60);
    // One episode: a single continuous lounging stretch (the
    // run-end drain folded out the still-open episode). Spawned
    // at x=-1.0 stepping 0.1/frame, the walker first lands on the
    // inclusive -x face (x=-0.5) on observed frame 4.
    assert_eq!(fr.roster[0].first_frame, 4, "first observed inside frame");
}

/// The boundary fixtures: planted INSIDE at the center, lounging
/// for planner frames 1..=LEAVE_AT−1, then out. Observed frames
/// are planner-minus-one (the harness snapshots post-update), so
/// LEAVE_AT = 61 lounges observed frames 0..=60 — the episode's
/// driver span is exactly 60/60 = 1.0 = window_s (the folder
/// credits forward inter-sample deltas, sample 0.0 → 1.0), and
/// LEAVE_AT = 60 gives observed 0..=59 = 59/60, one frame under.
/// Together they pin the boundary from BOTH sides: the discard is
/// strict (`driver_span < window_s`), so the exactly-at-window
/// episode FIRES and one-frame-under DISCARDS — a silent flip to
/// `<=` or an off-by-one in the span credit would break exactly
/// one of the pair. After the exit the walker keeps MOVING
/// outside (a parked transform trips frozen_world).
fn lounging_until_frame(
    leave_at: u32,
) -> impl FnMut(Query<&mut Transform, With<Wanderer>>, Local<f32>, Local<u32>) {
    move |mut q, mut phase, mut frame| {
        *phase += 0.37;
        *frame += 1;
        for mut t in q.iter_mut() {
            if *frame > leave_at {
                t.translation.x = 10.0;
            } else {
                t.translation.x += if t.translation.x < 0.0 { 0.1 } else { 0.0 };
            }
            t.translation.z = phase.sin() * 0.01;
        }
    }
}

#[test]
fn episode_exactly_at_the_window_fires() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.register_type::<Wanderer>();
    app.register_type::<Transform>();
    app.add_systems(Update, lounging_until_frame(61));
    // Planted INSIDE (x=0.0): observed inside frames 0..=60, span
    // exactly window_s (60 frame-ticks at 60 tps) — must FIRE, a
    // strict discard (`driver_span < window_s`) keeps the
    // == boundary on the firing side. Family precedent:
    // resource_check's window_boundary_is_strictly_greater.
    app.world_mut()
        .spawn((Wanderer, Transform::from_xyz(0.0, 0.5, 0.0)));
    let report = run_scenario(&mut app, &scenario_with_invariant(REGION_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    let v = report
        .violations
        .iter()
        .find(|v| v.rule == "forbidden_region")
        .expect("a span of exactly window_s must fire, not discard");
    let fr = v
        .meta
        .as_ref()
        .expect("episode finding carries _meta")
        .forbidden_episode
        .as_ref()
        .expect("_meta carries the forbidden episode block");
    assert_eq!(fr.roster.len(), 1);
    assert_eq!(fr.roster[0].first_frame, 0);
}

/// The under-boundary control, one frame less: observed frames
/// 0..=59, span 59/60 < 1.0 — sub-soak, discarded, the run passes.
/// The pair brackets the boundary so an off-by-one ANYWHERE in
/// the span credit or the comparison breaks one of the two.
#[test]
fn episode_one_frame_under_the_window_discards() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.register_type::<Wanderer>();
    app.register_type::<Transform>();
    app.add_systems(Update, lounging_until_frame(60));
    app.world_mut()
        .spawn((Wanderer, Transform::from_xyz(0.0, 0.5, 0.0)));
    let report = run_scenario(&mut app, &scenario_with_invariant(REGION_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
}

/// Control: a wanderer that stays outside the box passes. It
/// circles outside the region's x-extent forever, moving
/// monotonically.
#[test]
fn outside_wanderer_passes() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.register_type::<Wanderer>();
    app.register_type::<Transform>();
    app.add_systems(Update, |mut q: Query<&mut Transform, With<Wanderer>>| {
        for mut t in q.iter_mut() {
            t.translation.x += 0.05;
            t.translation.z = (t.translation.x * 0.7).sin() * 0.3;
        }
    });
    app.world_mut()
        .spawn((Wanderer, Transform::from_xyz(10.0, 0.5, 0.0)));
    let report = run_scenario(&mut app, &scenario_with_invariant(REGION_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    assert!(!report
        .violations
        .iter()
        .any(|v| v.rule == "forbidden_region"));
}

/// The fly-by doctrine: an incursion episode shorter than
/// `window_s` is DISCARDED — brushing a no-go zone in passing is
/// gameplay, not a bug. The walker crosses straight through the
/// box (0.2/frame crossing 1.0 of width ≈ 5 frames ≈ 0.08s, far
/// below the 1.0s soak).
#[test]
fn quick_crossing_is_discarded() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.register_type::<Wanderer>();
    app.register_type::<Transform>();
    app.add_systems(Update, |mut q: Query<&mut Transform, With<Wanderer>>| {
        for mut t in q.iter_mut() {
            t.translation.x += 0.2;
        }
    });
    app.world_mut()
        .spawn((Wanderer, Transform::from_xyz(-5.0, 0.5, 0.0)));
    let report = run_scenario(&mut app, &scenario_with_invariant(REGION_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.rule == "forbidden_region"),
        "a sub-soak crossing must not become a finding: {:?}",
        report
            .violations
            .iter()
            .map(|v| (v.rule.clone(), v.target.clone()))
            .collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// The instant-fire form (window_s omitted)
// ---------------------------------------------------------------------------

/// Omission flips the rule's form: the quick crossing that the
/// SOAK form discards (sub-1.0s, a transient fly-by) becomes a
/// FINDING when window_s is absent — every closed incursion
/// episode counts, no minimum dwell. The same app, the same
/// crossing, only the soak removed: instant catches what soak
/// forgives.
#[test]
fn windowless_crossing_fires_instead_of_discarding() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.register_type::<Wanderer>();
    app.register_type::<Transform>();
    app.add_systems(Update, |mut q: Query<&mut Transform, With<Wanderer>>| {
        for mut t in q.iter_mut() {
            t.translation.x += 0.2;
        }
    });
    app.world_mut()
        .spawn((Wanderer, Transform::from_xyz(-5.0, 0.5, 0.0)));
    let scenario = scenario_with_invariant(
        r#"
    "name": "no_rail_tunneling",
    "rule": "forbidden_region",
    "target_selector": {"with_component": {"name": "Wanderer"}},
    "region": {"center": [0.0, 0.5, 0.0], "half_extents": [0.5, 1.0, 0.5]}"#,
    );
    let report = run_scenario(&mut app, &scenario).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    let v = report
        .violations
        .iter()
        .find(|v| v.rule == "forbidden_region")
        .expect("instant form: the one-frame clip must count");
    // The instant tail: the finding says entering at all is the
    // bug — no soak language, no driver units.
    assert!(v.detail.contains("no minimum dwell"), "{}", v.detail);
}

/// The one-frame CLIP: a single incursion frame — the shortest
/// episode the folder can close — still fires in the instant
/// form. Planted outside; the walker jumps inside on the first
/// Update and out again on the second, so the closed episode
/// spans exactly one incursion frame.
#[test]
fn one_frame_clip_fires_in_the_instant_form() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.register_type::<Wanderer>();
    app.register_type::<Transform>();
    // Enter on the first Update, exit on the second: the span
    // inside is exactly one frame.
    app.add_systems(
        Update,
        |mut q: Query<&mut Transform, With<Wanderer>>, mut frame: Local<u32>| {
            *frame += 1;
            for mut t in q.iter_mut() {
                if *frame == 1 {
                    t.translation.x = 0.0; // inside (planted outside)
                } else {
                    t.translation.x = 10.0; // outside, for good
                }
            }
        },
    );
    app.world_mut()
        .spawn((Wanderer, Transform::from_xyz(-5.0, 0.5, 0.0)));
    let scenario = scenario_with_invariant(
        r#"
    "name": "no_rail_tunneling",
    "rule": "forbidden_region",
    "target_selector": {"with_component": {"name": "Wanderer"}},
    "region": {"center": [0.0, 0.5, 0.0], "half_extents": [0.5, 1.0, 0.5]}"#,
    );
    let report = run_scenario(&mut app, &scenario).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    let v = report
        .violations
        .iter()
        .find(|v| v.rule == "forbidden_region")
        .expect("instant form: the one-frame clip must count");
    // Evidence shape: one entity, one violating frame.
    let meta = v.meta.as_ref().expect("episode finding carries _meta");
    let fr = meta
        .forbidden_episode
        .as_ref()
        .expect("_meta carries the forbidden episode block");
    assert_eq!(fr.sampled_entities, 1);
    assert_eq!(fr.roster[0].violating_frames, 1);
}

// ---------------------------------------------------------------------------
// Boundary inclusivity (the on-edge doctrine)
// ---------------------------------------------------------------------------

/// Containment is INCLUSIVE on every face: a translation lying
/// exactly on the boundary counts as inside. The walker is planted
/// EXACTLY on the +x face (x = 0.5) before the run and lounges
/// there, past the soak — the finding fires, proving `<` vs `<=`
/// was pinned the inclusive way. (Planted, not stepped-to: f32
/// stepping accumulates drift past the face, which would turn the
/// test into a coin flip between inside and outside.)
#[test]
fn on_edge_translation_counts_as_inside() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.register_type::<Wanderer>();
    app.register_type::<Transform>();
    app.add_systems(Update, lounging_walk);
    app.world_mut()
        .spawn((Wanderer, Transform::from_xyz(0.5, 0.5, 0.0)));
    let report = run_scenario(&mut app, &scenario_with_invariant(REGION_INV)).unwrap();
    assert_eq!(
        report.status,
        bevy_playtest::enums::PlaytestStatus::Fail,
        "an on-face lounging incursion must fire (inclusive containment)"
    );
    assert!(report
        .violations
        .iter()
        .any(|v| v.rule == "forbidden_region"));
}

// ---------------------------------------------------------------------------
// Episode splitting (the maximal-stretch doctrine)
// ---------------------------------------------------------------------------

/// Leaving and re-entering splits episodes (the maximal-stretch
/// doctrine): two separate incursion episodes form, each judged
/// against the soak separately. Here EACH visit is short (a quick
/// in-out-in-out), so every episode is sub-soak — no finding —
/// but the held trace must show TWO episodes observed, the
/// near-miss evidence that the fold split correctly. This pins
/// the every-frame-push discipline: the recorder pushes the EMPTY
/// frames while the walker is outside, closing the episode — a
/// recorder that skipped empty frames would weld both visits into
/// one long stretch and the combined span would cross the soak
/// and FIRE.
#[test]
fn quick_leave_and_reenter_stays_sub_soak_per_episode() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.register_type::<Wanderer>();
    app.register_type::<Transform>();
    // Square wave on x: 20 frames outside, 20 inside, repeated —
    // each inside stretch is 20 frames ≈ 0.33s < the 1.0s soak.
    app.add_systems(Update, |mut q: Query<&mut Transform, With<Wanderer>>| {
        for mut t in q.iter_mut() {
            let phase = t.translation.x;
            // Outside band: |x| > 0.5. Walk right while outside
            // the box, jump back left when past +0.5 (a teleport
            // is fine — a position discontinuity is just travel).
            t.translation.x += 0.1;
            if t.translation.x > 0.55 {
                t.translation.x = -0.6;
            }
            t.translation.z = (phase * 3.0).sin() * 0.01;
            let _ = phase;
        }
    });
    app.world_mut()
        .spawn((Wanderer, Transform::from_xyz(-0.6, 0.5, 0.0)));
    let report = run_scenario(&mut app, &scenario_with_invariant(REGION_INV)).unwrap();
    assert_eq!(
        report.status,
        bevy_playtest::enums::PlaytestStatus::Pass,
        "split sub-soak episodes must not fire: {:?}",
        report
            .violations
            .iter()
            .map(|v| (v.rule.clone(), v.target.clone(), v.detail.clone()))
            .collect::<Vec<_>>()
    );
    // The held trace discloses the split: multiple episodes
    // observed across the run.
    let held = report
        .held_invariants
        .iter()
        .find(|h| h.name == "no_rail_tunneling")
        .expect("held entry for the passing region invariant");
    assert!(
        held.episodes_observed.unwrap_or(0) >= 2,
        "the fold must split re-entries into separate episodes, held: {held:?}"
    );
}

// ---------------------------------------------------------------------------
// The allow_if exemption
// ---------------------------------------------------------------------------

/// The GOOD half of the flip-test: a lane-mounted entity whose
/// PRESENCE in the region is legitimate (the rail's outer lane
/// runs along the region's surface) is exempt, per entity, per
/// frame. A lifetime-mounted lounger produces NO finding AND no
/// episode — silence, not vacuity (the selector matched, so the
/// rule is not vacuous either).
#[test]
fn lane_mounted_lounger_is_exempt() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.register_type::<Wanderer>();
    app.register_type::<LaneMounted>();
    app.register_type::<Transform>();
    app.add_systems(Update, lounging_walk);
    app.world_mut()
        .spawn((Wanderer, LaneMounted, Transform::from_xyz(-1.0, 0.5, 0.0)));
    let scen = scenario_with_invariant(&format!(
        r#"{REGION_INV},
    "allow_if": "LaneMounted""#
    ));
    let report = run_scenario(&mut app, &scen).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    assert!(!report
        .violations
        .iter()
        .any(|v| v.rule == "forbidden_region"));
    // Silence, not vacuity: the selector matched, the exemption did
    // its work — the held entry keeps the rule's evidence.
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.rule == "vacuous_invariant" && v.target == "no_rail_tunneling"),
        "an exempt presence is not a vacuous rule"
    );
}

/// The exemption is PER-ENTITY: a non-carrier lounging alongside
/// an exempt carrier is still judged. One entity carries
/// LaneMounted and lounges legitimately; the twin never acquires
/// it and lingers just as long — the finding fires and the roster
/// names ONLY the non-carrier.
#[test]
fn exemption_is_per_entity() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.register_type::<Wanderer>();
    app.register_type::<LaneMounted>();
    app.register_type::<Transform>();
    app.add_systems(Update, lounging_walk);
    app.world_mut()
        .spawn((Wanderer, LaneMounted, Transform::from_xyz(-1.0, 0.5, 0.0)));
    // The twin starts offset so its incursion is its own episode,
    // not shared-but-exempt evidence.
    app.world_mut()
        .spawn((Wanderer, Transform::from_xyz(-2.0, 0.5, 0.0)));
    let scen = scenario_with_invariant(&format!(
        r#"{REGION_INV},
    "allow_if": "LaneMounted""#
    ));
    let report = run_scenario(&mut app, &scen).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    let v = report
        .violations
        .iter()
        .find(|v| v.rule == "forbidden_region")
        .expect("the non-carrier's lounging must fire");
    let meta = v.meta.as_ref().expect("_meta present");
    let fr = meta
        .forbidden_episode
        .as_ref()
        .expect("forbidden episode block");
    assert_eq!(
        fr.sampled_entities, 1,
        "only the non-carrier is incursive: {fr:?}"
    );
}

/// Mid-incursion ACQUISITION of the exemption closes the episode
/// at the acquisition frame: the entity walks in un-mounted, soaks
/// a while, then acquires LaneMounted mid-region and keeps
/// lounging legitimately. The fold must end the incursion episode
/// when the exemption kicks in — the pre-acquisition stretch is
/// judged on its own span. Here the pre-acquisition stretch soaks
/// past window_s, so the finding fires with an episode END at the
/// acquisition frame, well before the run's end.
#[test]
fn mid_incursion_allow_if_acquisition_closes_the_episode() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.register_type::<Wanderer>();
    app.register_type::<LaneMounted>();
    app.register_type::<Transform>();
    app.add_systems(
        Update,
        |mut q: Query<&mut Transform, With<Wanderer>>,
         mut phase: Local<f32>,
         mut local: Local<u32>,
         mut commands: Commands,
         entities: Query<Entity, With<Wanderer>>| {
            *phase += 0.37;
            *local += 1;
            for mut t in q.iter_mut() {
                t.translation.x += if t.translation.x < 0.0 { 0.1 } else { 0.0 };
                t.translation.z = phase.sin() * 0.01;
            }
            // Grant the exemption at frame 180 (3s in) — long
            // after the walker entered (frame ~10) and long after
            // the 1.0s soak elapsed.
            if *local == 180 {
                for e in entities.iter() {
                    commands.entity(e).insert(LaneMounted);
                }
            }
        },
    );
    app.world_mut()
        .spawn((Wanderer, Transform::from_xyz(-1.0, 0.5, 0.0)));
    let scen = scenario_with_invariant(&format!(
        r#"{REGION_INV},
    "allow_if": "LaneMounted""#
    ));
    let report = run_scenario(&mut app, &scen).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    let v = report
        .violations
        .iter()
        .find(|v| v.rule == "forbidden_region")
        .expect("the pre-acquisition stretch must fire");
    // The episode ended at the acquisition frame: the entry's
    // last_frame is the acquisition-adjacent frame (180ish), not
    // the run end (~239). Tolerances absorb ±0.01-style drift;
    // assert well away from both boundaries.
    assert!(
        v.last_frame < 220 && v.last_frame >= 170,
        "episode must close at acquisition, last_frame: {}",
        v.last_frame
    );
}

// ---------------------------------------------------------------------------
// Vacuity and config errors
// ---------------------------------------------------------------------------

/// A selector naming a registered-but-never-instantiated
/// component never has anything to select: `vacuous_invariant`,
/// naming the invariant — a region rule with nothing to select
/// proves nothing.
#[test]
fn never_spawned_selector_is_vacuous() {
    let _slot = common::run_slot();
    #[derive(Component, Reflect, Default)]
    #[reflect(Component)]
    struct UnopenedGate;

    let mut app = App::new();
    app.register_type::<UnopenedGate>();
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
    "name": "ghost_region",
    "rule": "forbidden_region",
    "target_selector": {"with_component": {"name": "UnopenedGate"}},
    "region": {"center": [0.0, 0.5, 0.0], "half_extents": [0.5, 1.0, 0.5]},
    "window_s": 1.0"#,
    );
    let report = run_scenario(&mut app, &scen).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    let v = report
        .violations
        .iter()
        .find(|v| v.rule == "vacuous_invariant" && v.target == "ghost_region")
        .expect("vacuous_invariant names the ghost-region invariant");
    assert!(
        v.detail.contains("never resolved a checkable entity"),
        "{}",
        v.detail
    );
}

/// An unregistered selector type is an authoring mistake:
/// `oracle_error` (config), never the game's fault — and the
/// config error suppresses the vacuity entry.
#[test]
fn unregistered_selector_reports_config_error() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.register_type::<Wanderer>();
    app.register_type::<Transform>();
    app.add_systems(Update, |mut q: Query<&mut Transform, With<Wanderer>>| {
        for mut t in q.iter_mut() {
            t.translation.x += 0.05;
        }
    });
    app.world_mut()
        .spawn((Wanderer, Transform::from_xyz(10.0, 0.5, 0.0)));
    let scen = scenario_with_invariant(
        r#"
    "name": "no_rail_tunneling",
    "rule": "forbidden_region",
    "target_selector": {"with_component": {"name": "NoSuchComponent"}},
    "region": {"center": [0.0, 0.5, 0.0], "half_extents": [0.5, 1.0, 0.5]},
    "window_s": 1.0"#,
    );
    let report = run_scenario(&mut app, &scen).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.rule == "oracle_error" && v.target == "no_rail_tunneling"),
        "unregistered selector must be an oracle_error naming the invariant"
    );
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.rule == "vacuous_invariant" && v.target == "no_rail_tunneling"),
        "config-errored invariant must not double-report vacuity"
    );
}

// ---------------------------------------------------------------------------
// Driver-unit soak
// ---------------------------------------------------------------------------

/// The declared driver leaf is the soak's clock: `window_s` is in
/// MINUTES here, and the clock advances slowly — the lounging
/// episode must soak against the slow clock, not the frame clock.
/// The same physical lounging that fails a 1.0-second frame-clock
/// soak is sub-soak when window_s counts minutes of MatchClock.
#[test]
fn declared_driver_leaf_is_the_soak_clock() {
    let _slot = common::run_slot();
    let mut app = lounging_fixture();
    app.register_type::<FrMatchClock>();
    app.insert_resource(FrMatchClock::default());
    app.add_systems(Update, |mut clock: ResMut<FrMatchClock>| {
        clock.minutes += 1.0 / 600.0;
    });

    // window_s: 1.0 MINUTE of MatchClock advance, judged purely
    // in the leaf's own units. The clock ticks 1/600 min/frame, so
    // over the whole ~4s run (~240 frames) it advances only ~0.4
    // minutes — sub-soak against the 1.0-minute window, discarded
    // as a fly-by. On the FRAME clock (seconds) the identical
    // lounging would fire (≈3.8s > 1.0s): the proof the soak never
    // converts through tps.
    let scen = scenario_with_invariant(
        r#"
    "name": "no_rail_tunneling",
    "rule": "forbidden_region",
    "target_selector": {"with_component": {"name": "Wanderer"}},
    "region": {"center": [0.0, 0.5, 0.0], "half_extents": [0.5, 1.0, 0.5]},
    "window_s": 1.0,
    "driver_resource": "FrMatchClock",
    "driver_path": "minutes""#,
    );
    let report = run_scenario(&mut app, &scen).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.rule == "forbidden_region"),
        "slow-clock lounging is sub-soak: {:?}",
        report.violations
    );
}

// ---------------------------------------------------------------------------
// Frame-0 and the roster cap
// ---------------------------------------------------------------------------

/// An entity planted INSIDE the region before the run starts is
/// incursive from the first observed frame: the episode starts at
/// the first OBSERVED inside-frame (frame 0 is a post-update
/// snapshot of the authored scene — spawn_safety's transition
/// doctrine does not apply; an episode rule judges observed time).
/// The lounging soaks past window_s → one finding whose
/// first_frame is 0.
#[test]
fn planted_inside_starts_the_episode_at_frame_zero() {
    let _slot = common::run_slot();
    let mut app = lounging_fixture();
    // The fixture spawned OUTSIDE (x=-1.0); move the wanderer to
    // the region center before the run starts so the very first
    // observed frame is already incursive. Despawn-and-replant is
    // the simplest mutation of a hand-built fixture.
    app.world_mut()
        .query_filtered::<&mut Transform, With<Wanderer>>()
        .single_mut(app.world_mut())
        .unwrap()
        .translation = bevy_math::Vec3::new(0.0, 0.5, 0.0);
    let report = run_scenario(&mut app, &scenario_with_invariant(REGION_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    let v = report
        .violations
        .iter()
        .find(|v| v.rule == "forbidden_region")
        .expect("planted-inside lounging must fire");
    assert_eq!(
        v.first_frame, 0,
        "the episode begins at the first OBSERVED inside frame"
    );
}

/// A crowd beyond the roster cap: 33 wanderers lounge inside the
/// region (cap is 32) — `sampled_entities` reports 33,
/// `roster_truncated` discloses the drop, and the roster holds 32.
/// The evidence never lies by omission.
#[test]
fn crowd_beyond_roster_cap_discloses_truncation() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.register_type::<Wanderer>();
    app.register_type::<Transform>();
    app.add_systems(Update, |mut q: Query<&mut Transform, With<Wanderer>>| {
        for mut t in q.iter_mut() {
            // Micro-jitter keeps the world unfrozen without ever
            // leaving the region.
            t.translation.x += 0.001 * (t.translation.z + 1.0);
        }
    });
    for i in 0..33u32 {
        app.world_mut().spawn((
            Wanderer,
            Transform::from_xyz(
                ((i % 7) as f32) * 0.01,
                0.5 + ((i % 3) as f32) * 0.01,
                ((i % 5) as f32) * 0.01,
            ),
        ));
    }
    let report = run_scenario(&mut app, &scenario_with_invariant(REGION_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    let v = report
        .violations
        .iter()
        .find(|v| v.rule == "forbidden_region")
        .expect("the crowd's lounging must fire");
    let meta = v.meta.as_ref().expect("_meta present");
    let fr = meta
        .forbidden_episode
        .as_ref()
        .expect("forbidden episode block");
    assert_eq!(fr.sampled_entities, 33, "all 33 counted distinctly");
    assert!(fr.roster_truncated, "the drop must be disclosed");
    assert_eq!(fr.roster.len(), 32, "the cap holds the roster at 32");
}

// ---------------------------------------------------------------------------
// Metadata serialization smoke
// ---------------------------------------------------------------------------

/// The `_meta` block serializes with the documented underscore key
/// and stable field names — consumers parse it as structured data.
#[test]
fn episode_meta_serializes_stably() {
    let _slot = common::run_slot();
    let mut app = lounging_fixture();
    let report = run_scenario(&mut app, &scenario_with_invariant(REGION_INV)).unwrap();
    let v = report
        .violations
        .iter()
        .find(|v| v.rule == "forbidden_region")
        .expect("finding present");
    let meta = v.meta.as_ref().expect("_meta present");
    let json = serde_json::to_value(meta).unwrap();
    let fr = json
        .get("forbidden_episode")
        .expect("forbidden_episode key present");
    assert!(fr.get("sampled_entities").is_some());
    assert!(fr.get("roster").is_some());
    assert!(fr.get("roster_truncated").is_some());
    let entry = fr
        .get("roster")
        .and_then(|r| r.get(0))
        .expect("roster non-empty");
    for key in [
        "entity",
        "first_frame",
        "worst_depth",
        "worst_frame",
        "violating_frames",
    ] {
        assert!(entry.get(key).is_some(), "roster entry key {key}");
    }
}
