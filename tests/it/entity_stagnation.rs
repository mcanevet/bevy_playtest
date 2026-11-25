//! Behavioral battery for the `entity_stagnation` rule: the
//! per-entity rate-form liveness oracle. Each selected entity's
//! watched leaf must not stay digest-constant while the declared
//! driver advances; per-entity stuck spans fold into episodes and
//! only an episode soaking past `window_s` (in driver units)
//! becomes a finding.
//!
//! The litmus mirrors the queue-on-dead-ride soft-lock incident
//! class (proposal §8 row 5): guests arrive at a demolished ride,
//! enter `Queued`, and wait forever — `waited` frozen while the
//! park's clock keeps advancing. The world looks perfectly alive:
//! no panic, no error log, resources flow; only the per-entity
//! soak can name the immortal guest.
//!
//! Fixture math (the cadence is the battery's critical path): at
//! tps 60 with the default `sampling_s` 1.0 the stride is
//! `round(1.0 / (1/60))` = 60 — samples land at frames 0, 60,
//! 120, 180… The first equal-digest pair accrues at frame 120
//! (span 1.0s of frame clock), so a run that stays stuck from
//! frame 0 onward crosses a 2.0s soak between frames 180 and 240.
//! Events (despawn, state change) are placed well OFF sample
//! frames (mid-interval, ±5-frame phase margin) so the assertion
//! never rides on a sample-boundary coincidence. Counters are
//! `Local<f32>` and incremented BEFORE the branch (frame-phase
//! fixtures must never freeze when a system skips).

use crate::common;

use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_playtest::driver::run_scenario;
use bevy_reflect::Reflect;
use bevy_transform::components::Transform;

/// The standard guest shape: a component carrying an enum whose
/// struct variant holds the watched leaf — the reflection-layer
/// machinery this vocabulary exists to exercise. The name is
/// unique in the integration binary (short-name resolution is
/// ambiguous across same-named types).
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct StagGuest {
    state: StagGuestState,
}

#[derive(Reflect, Clone, Default)]
enum StagGuestState {
    #[default]
    Idle,
    Queued {
        waited: f32,
    },
    Riding {
        car: u32,
    },
}

fn scenario_with_invariant(inv: &str) -> bevy_playtest::scenario::Scenario {
    let json = format!(
        r#"{{"bot":{{"type":"chaos","seed":42}},"duration_s":6.0,"tps":60,
            "invariants":[{{{inv}}}]}}"#
    );
    serde_json::from_str(&json).unwrap()
}

/// A live park: the world keeps churning (frozen_world stays
/// quiet) while our marked guest's watched leaf stays frozen —
/// the immortal-guest shape. `mover` keeps one background body's
/// transform advancing every frame.
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
    app.register_type::<StagGuest>();
    app.register_type::<StagGuestState>();
    app.register_type::<Transform>();
    app.add_systems(Update, move_background);
    app.world_mut()
        .spawn((Background { tag: 0 }, Transform::default()));
    app
}

/// The queue-wait watch in its minimal form: `waited` under the
/// Queued variant, judged against the default frame clock.
const QUEUE_INV: &str = r#"
    "name": "queued_guests_must_not_wait_forever",
    "rule": "entity_stagnation",
    "target_selector": {"with_component": {"name": "StagGuest"}},
    "watch_path": "state.Queued.waited",
    "window_s": 2.0"#;

// ---------------------------------------------------------------------------
// The queue-on-dead-ride litmus
// ---------------------------------------------------------------------------

/// The softlock shape: a guest arrives, enters Queued, and the
/// patience loop (dead ride) never advances `waited`. Samples at
/// frames 0, 60, 120, 180, 240, 300...; the digest is equal from
/// sample pair (60,120) onward, crossing the 2.0s soak inside the
/// (180,240] interval — the episode must CLOSE AT RUN END judging
/// ≥ 2.0s and GATE the run (RED).
#[test]
fn queued_forever_guest_fails_the_soak() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.world_mut().spawn((
        StagGuest {
            state: StagGuestState::Queued { waited: 0.0 },
        },
        Transform::default(),
    ));
    let report = run_scenario(&mut app, &scenario_with_invariant(QUEUE_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    let v = report
        .violations
        .iter()
        .find(|v| v.rule == "entity_stagnation")
        .expect("expected an entity_stagnation finding");
    assert!(v.detail.contains("stayed digest-constant"), "{}", v.detail);
    assert!(
        v.detail.contains("livelock"),
        "the finding must name the incident class: {}",
        v.detail
    );
    // The `_meta` episode evidence: one stuck guest, not
    // truncated, with the accrued stuck-span frozen in.
    let meta = v.meta.as_ref().expect("episode finding carries _meta");
    let stag = meta
        .stagnation_episode
        .as_ref()
        .expect("_meta carries the stagnation episode block");
    assert_eq!(stag.sampled_entities, 1, "one distinct stuck guest");
    assert!(!stag.roster_truncated);
    assert_eq!(stag.roster.len(), 1);
    assert!(
        stag.roster[0].stuck_span >= 2.0,
        "the banked stuck-span must reach the soak: {}",
        stag.roster[0].stuck_span
    );
    assert!(
        stag.roster[0].stuck_frames >= 3,
        "at least three stuck samples: {}",
        stag.roster[0].stuck_frames
    );
}

/// Control: the patience loop LIVES (waits tick, then the guest
/// boards and the leaf leaves the watch) — no finding, the run
/// passes. `waited` advances every frame; boarding at a
/// mid-interval frame (well off any sample boundary) removes the
/// key from the sample — acquittal, not a stuck span.
#[test]
fn ticking_patience_loop_passes() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.add_systems(Update, |mut q: Query<&mut StagGuest>| {
        for mut g in q.iter_mut() {
            if let StagGuestState::Queued { waited } = &mut g.state {
                *waited += 1.0 / 60.0;
            }
        }
    });
    app.world_mut().spawn((
        StagGuest {
            state: StagGuestState::Queued { waited: 0.0 },
        },
        Transform::default(),
    ));
    let report = run_scenario(&mut app, &scenario_with_invariant(QUEUE_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.rule == "entity_stagnation"),
        "a ticking clock is never stagnation: {:?}",
        report.violations
    );
}

// ---------------------------------------------------------------------------
// Turnover doctrine: despawn is acquittal
// ---------------------------------------------------------------------------

/// A stuck guest DESPAWNED mid-stint (gate-only despawn — the
/// park's turnover) never becomes a finding: the episode closes
/// with the guest's accrued span, but the run's duration and the
/// despawn placement (mid-interval, frame 150) keep that span
/// below the 2.0s soak. Turnover is acquittal, never a refund:
/// the guest's stuck frames still COUNT (the held trace keeps
/// the near-miss), they just never reach the threshold.
#[test]
fn despawn_mid_watch_acquits_without_a_finding() {
    let _slot = common::run_slot();
    let mut app = base_app();
    let guest = app
        .world_mut()
        .spawn((
            StagGuest {
                state: StagGuestState::Queued { waited: 0.0 },
            },
            Transform::default(),
        ))
        .id();
    // Frame 150: mid-interval between samples 120 and 180 — well
    // off any sample boundary.
    app.add_systems(
        Update,
        move |mut commands: Commands, mut clock: Local<f32>| {
            *clock += 1.0;
            if (*clock - 150.0).abs() < 0.5 {
                commands.entity(guest).despawn();
            }
        },
    );
    let report = run_scenario(&mut app, &scenario_with_invariant(QUEUE_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.rule == "entity_stagnation"),
        "a despawned guest's sub-soak stint must not become a finding: {:?}",
        report.violations
    );
}

// ---------------------------------------------------------------------------
// Scoped digest: change-clears on the WATCHED leaf only
// ---------------------------------------------------------------------------

/// A sibling-field mutation must NOT clear stagnation: the watch
/// is the SCOPED leaf (`waited`), not the whole component — but
/// `car` lives in a DIFFERENT variant, invisible to the scoped
/// digest even though the whole component changes every frame.
/// The guest sits in Queued forever while `happiness` (same
/// struct, different field) ticks: the component-level digest
/// would flip every frame, the scoped digest stays constant —
/// and the scoped doctrine fires.
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct HappyGuest {
    happiness: f32,
    state: HappyState,
}

#[derive(Reflect, Clone, Default)]
enum HappyState {
    #[default]
    Idle,
    Queued {
        waited: f32,
    },
}

#[test]
fn sibling_field_mutation_does_not_clear_stagnation() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.register_type::<HappyGuest>();
    app.register_type::<HappyState>();
    app.add_systems(Update, |mut q: Query<&mut HappyGuest>| {
        for mut g in q.iter_mut() {
            // The component churns (happiness ticks) but the
            // watched leaf `state.Queued.waited` stays frozen.
            g.happiness += 1.0 / 60.0;
        }
    });
    app.world_mut().spawn((
        HappyGuest {
            happiness: 0.0,
            state: HappyState::Queued { waited: 0.0 },
        },
        Transform::default(),
    ));
    let report = run_scenario(
        &mut app,
        &scenario_with_invariant(
            r#"
    "name": "happy_but_stuck",
    "rule": "entity_stagnation",
    "target_selector": {"with_component": {"name": "HappyGuest"}},
    "watch_path": "state.Queued.waited",
    "window_s": 2.0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    let v = report
        .violations
        .iter()
        .find(|v| v.rule == "entity_stagnation")
        .expect("the scoped digest must fire despite component churn");
    assert!(v.detail.contains("stayed digest-constant"), "{}", v.detail);
}

/// Control for the scoped digest: mutating the WATCHED leaf
/// itself clears stagnation, even sparsely — `waited` hops at a
/// cadence slower than the sampling stride (every 90 frames,
/// 1.5s) so consecutive samples still USUALLY see equal digests,
/// but the run never accrues 2.0s of stuck span because every
/// hop resets the stint. Sparse progress is still progress.
#[test]
fn sparse_watched_leaf_hops_reset_the_stint() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.add_systems(
        Update,
        |mut q: Query<&mut StagGuest>, mut clock: Local<u32>| {
            *clock += 1;
            if clock.is_multiple_of(90) {
                for mut g in q.iter_mut() {
                    if let StagGuestState::Queued { waited } = &mut g.state {
                        *waited += 1.0;
                    }
                }
            }
        },
    );
    app.world_mut().spawn((
        StagGuest {
            state: StagGuestState::Queued { waited: 0.0 },
        },
        Transform::default(),
    ));
    let report = run_scenario(&mut app, &scenario_with_invariant(QUEUE_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.rule == "entity_stagnation"),
        "a hopping leaf never accrues a soaked span: {:?}",
        report.violations
    );
}

// ---------------------------------------------------------------------------
// leaf_where: the variant precondition
// ---------------------------------------------------------------------------

/// The flat precondition form: leaf_where (`state == Queued`)
/// restricts the watch to Queued guests. A guest whose state
/// MUTATES within the Queued variant still stagnates (the
/// precondition is per-frame; `waited` frozen is the finding).
#[test]
fn leaf_where_plain_form_gates_on_live_variant() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.world_mut().spawn((
        StagGuest {
            state: StagGuestState::Queued { waited: 0.0 },
        },
        Transform::default(),
    ));
    let report = run_scenario(
        &mut app,
        &scenario_with_invariant(
            r#"
    "name": "queued_soft_lock",
    "rule": "entity_stagnation",
    "target_selector": {"with_component": {"name": "StagGuest"}},
    "watch_path": "state.Queued.waited",
    "leaf_where": "state == Queued",
    "window_s": 2.0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.rule == "entity_stagnation"),
        "the redundant-but-consistent leaf_where must not disable the watch"
    );
}

/// Precondition lapse = acquittal: a guest BOARDS (state flips to
/// Riding) mid-interval at frame 150. From that sample onward the
/// entity is out of the watch entirely (leaf_where fails, the
/// variant-qualified watch path stops resolving) — the stint
/// closes with its accrued span, sub-soak, no finding.
#[test]
fn boarding_mid_watch_lapses_the_precondition_and_acquits() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.add_systems(
        Update,
        |mut q: Query<&mut StagGuest>, mut clock: Local<u32>| {
            *clock += 1;
            if *clock == 150 {
                for mut g in q.iter_mut() {
                    g.state = StagGuestState::Riding { car: 3 };
                }
            }
        },
    );
    app.world_mut().spawn((
        StagGuest {
            state: StagGuestState::Queued { waited: 0.0 },
        },
        Transform::default(),
    ));
    let report = run_scenario(&mut app, &scenario_with_invariant(QUEUE_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.rule == "entity_stagnation"),
        "boarding is acquittal: {:?}",
        report.violations
    );
}

// ---------------------------------------------------------------------------
// The resource driver: the soak in the leaf's own units
// ---------------------------------------------------------------------------

/// A slow game clock resource (advance rate 1.0 unit/s, the
/// clock ticks slowly in its own unit) proves the soak is judged
/// in DRIVER-LEAF UNITS, never converted through tps: the same
/// physical freeze that fails a 2.0s frame-clock soak is sub-soak
/// when window_s counts slow clock units.
#[derive(Resource, Reflect, Default)]
#[reflect(Resource)]
struct StagMatchClock {
    ticks: f64,
}

#[test]
fn slow_resource_driver_extends_the_effective_soak() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.register_type::<StagMatchClock>();
    app.insert_resource(StagMatchClock::default());
    // 0.5 ticks/s: the sampling cadence (sampling_s 1.0 tick) maps
    // to a 2-second stride; window_s 2.0 ticks = 4 seconds of run.
    app.add_systems(Update, |mut clock: ResMut<StagMatchClock>| {
        clock.ticks += 0.5 / 60.0;
    });
    app.world_mut().spawn((
        StagGuest {
            state: StagGuestState::Queued { waited: 0.0 },
        },
        Transform::default(),
    ));
    let report = run_scenario(
        &mut app,
        &scenario_with_invariant(
            r#"
    "name": "queued_clock_units",
    "rule": "entity_stagnation",
    "target_selector": {"with_component": {"name": "StagGuest"}},
    "watch_path": "state.Queued.waited",
    "window_s": 2.0,
    "driver": "resource",
    "driver_resource": "StagMatchClock",
    "driver_path": "ticks",
    "driver_units_per_second": 0.5,
    "sampling_s": 1.0"#,
        ),
    )
    .unwrap();
    // 6.0s run = 3.0 ticks: the stuck span reaches only ~2.0-1.0
    // ticks depending on phase — right at the soak boundary. The
    // run length is chosen to make this SUB-soak (the last
    // sampled interval ends before the soak crosses): the
    // physical 3.9-second freeze is sub-soak against 2.0 SLOW
    // CLOCK ticks.
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.rule == "entity_stagnation"),
        "a slow driver stretches the effective soak past the run: {:?}",
        report.violations
    );
}

// ---------------------------------------------------------------------------
// The relay doctrine (decision B): a relay of sub-window stints
// with obs never empty fires once the UNION timeline crosses
// window_s
// ---------------------------------------------------------------------------

/// Nobody soaks INDIVIDUALLY — each guest's stint stays at 2.0s
/// against a 2.5s window — but the shared timeline never goes
/// empty between the first and last push, so the episode's union
/// span (sample 60 to sample 240 = 3.0s) crosses the soak and
/// ONE finding fires naming both participants. A world where
/// every individual wait looks defensible but the relief valve
/// never opens is exactly the livelock this doctrine exists to
/// name. Deliberate semantics, pinned (AS-BUILT: proposal §7's
/// per-entity phrasing is overridden by the shared-timeline
/// fold).
#[test]
fn relay_of_sub_window_stints_fires_on_the_union_timeline() {
    let _slot = common::run_slot();
    let mut app = base_app();
    // A: frozen until frame 155 (hop mid-interval 120-180).
    // B: hops at frame 100 (mid 60-120), frozen until 265
    // (mid 240-300). Duty cycle: A stuck samples 60,120; B stuck
    // samples 180,240 — obs never empty 60..240.
    app.add_systems(
        Update,
        |mut q: Query<&mut StagGuest>, mut clock: Local<u32>| {
            *clock += 1;
            for mut g in q.iter_mut() {
                if let StagGuestState::Queued { waited } = &mut g.state {
                    // Guests are distinguished by their leaf VALUE, so
                    // they never weld into one another's stints. Float
                    // equality with the constants we add ourselves is
                    // exact. A: 0.0 → 5.0 at 155. B: 10.0 → 20.0 at
                    // 100, → 30.0 at 265.
                    if *waited == 0.0 {
                        if *clock == 155 {
                            *waited += 5.0;
                        }
                    } else if *waited == 10.0 {
                        if *clock == 100 {
                            *waited += 10.0;
                        }
                    } else if *waited == 20.0 && *clock == 265 {
                        *waited += 10.0;
                    }
                }
            }
        },
    );
    app.world_mut().spawn((
        StagGuest {
            state: StagGuestState::Queued { waited: 0.0 },
        },
        Transform::default(),
    ));
    app.world_mut().spawn((
        StagGuest {
            state: StagGuestState::Queued { waited: 10.0 },
        },
        Transform::default(),
    ));
    let report = run_scenario(
        &mut app,
        &scenario_with_invariant(
            r#"
    "name": "relay_soft_lock",
    "rule": "entity_stagnation",
    "target_selector": {"with_component": {"name": "StagGuest"}},
    "watch_path": "state.Queued.waited",
    "window_s": 2.5"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    let v = report
        .violations
        .iter()
        .find(|v| v.rule == "entity_stagnation")
        .expect("the relay union must cross the soak");
    let meta = v.meta.as_ref().expect("episode finding carries _meta");
    let stag = meta
        .stagnation_episode
        .as_ref()
        .expect("_meta carries the stagnation episode block");
    assert_eq!(stag.sampled_entities, 2, "both relay legs in the roster");
    assert_eq!(stag.roster.len(), 2);
    for entry in &stag.roster {
        assert!(
            entry.stuck_span < 2.5,
            "each individual stint is sub-window: {}",
            entry.stuck_span
        );
    }
}

// ---------------------------------------------------------------------------
// Id reuse: (index, generation) keys
// ---------------------------------------------------------------------------

/// A despawned guest's slot is RECYCLED into a new guest that
/// also stagnates. The composite episode key must open a FRESH
/// stint for the successor — never weld it to the predecessor's
/// accrued span — and the roster entry must serialize BOTH the
/// index and the generation so slot recycling is legible. The
/// span math IS the weld detector: the successor first appears
/// at sample 120 (frame 90 spawn), so its stint seeds there and
/// accrues 3.0s by sample 300; a weld to the predecessor's
/// frame-0 sample base would credit 5.0s.
#[derive(Resource, Default)]
struct SuccessorId(Option<bevy_ecs::entity::Entity>);

#[test]
fn id_reuse_recycled_slot_is_a_fresh_entry() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.insert_resource(SuccessorId::default());
    let first = app
        .world_mut()
        .spawn((
            StagGuest {
                state: StagGuestState::Queued { waited: 0.0 },
            },
            Transform::default(),
        ))
        .id();
    // Despawn at frame 30 (seen only at sample 0, digest seeded,
    // zero span), respawn into the recycled slot at frame 90 —
    // the successor stagnates the rest of the run.
    app.add_systems(
        Update,
        move |mut commands: Commands, mut clock: Local<u32>, mut succ: ResMut<SuccessorId>| {
            *clock += 1;
            if *clock == 30 {
                commands.entity(first).despawn();
            } else if *clock == 90 {
                succ.0 = Some(
                    commands
                        .spawn((
                            StagGuest {
                                state: StagGuestState::Queued { waited: 0.0 },
                            },
                            Transform::default(),
                        ))
                        .id(),
                );
            }
        },
    );
    let report = run_scenario(&mut app, &scenario_with_invariant(QUEUE_INV)).unwrap();
    let successor = app
        .world()
        .get_resource::<SuccessorId>()
        .and_then(|s| s.0)
        .expect("successor spawned");
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    let v = report
        .violations
        .iter()
        .find(|v| v.rule == "entity_stagnation")
        .expect("the successor's soak must fire");
    let meta = v.meta.as_ref().expect("episode finding carries _meta");
    let stag = meta
        .stagnation_episode
        .as_ref()
        .expect("_meta carries the stagnation episode block");
    // The roster carries the SUCCESSOR's exact composite key —
    // slot recycling is legible in the evidence, and the span is
    // measured from ITS OWN seed (samples 120→300 = 3.0s), never
    // welded to the predecessor's frame-0 base (5.0s).
    assert_eq!(stag.roster.len(), 1, "only the successor soaked: {stag:?}");
    assert_eq!(stag.roster[0].entity, successor.index().index());
    assert_eq!(
        stag.roster[0].generation,
        successor.generation().to_bits(),
        "the roster must name the successor's exact generation"
    );
    assert!(
        stag.roster[0].stuck_span > 2.0 && stag.roster[0].stuck_span < 4.0,
        "span measured from the successor's own seed (weld would read 5.0): {}",
        stag.roster[0].stuck_span
    );
}

// ---------------------------------------------------------------------------
// Fast driver complement: the rate SHORTENS the wall-clock soak
// ---------------------------------------------------------------------------

/// The mirror of `slow_resource_driver_extends_the_effective_`
/// `soak`: the same frozen leaf against a FAST clock (4 units/s)
/// soaks 2.0 units in 0.5 wall seconds — the finding fires EARLY
/// and the detail names DRIVER UNITS, not seconds.
#[test]
fn fast_resource_driver_soaks_in_leaf_units_early() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.register_type::<StagMatchClock>();
    app.insert_resource(StagMatchClock::default());
    app.add_systems(Update, |mut clock: ResMut<StagMatchClock>| {
        clock.ticks += 4.0 / 60.0;
    });
    app.world_mut().spawn((
        StagGuest {
            state: StagGuestState::Queued { waited: 0.0 },
        },
        Transform::default(),
    ));
    let report = run_scenario(
        &mut app,
        &scenario_with_invariant(
            r#"
    "name": "queued_fast_clock",
    "rule": "entity_stagnation",
    "target_selector": {"with_component": {"name": "StagGuest"}},
    "watch_path": "state.Queued.waited",
    "window_s": 2.0,
    "driver": "resource",
    "driver_resource": "StagMatchClock",
    "driver_path": "ticks",
    "driver_units_per_second": 4.0,
    "sampling_s": 1.0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    let v = report
        .violations
        .iter()
        .find(|v| v.rule == "entity_stagnation")
        .expect("the fast clock must cross the soak early");
    assert!(
        v.detail.contains("driver units"),
        "the soak is named in the leaf's own unit: {}",
        v.detail
    );
}

// ---------------------------------------------------------------------------
// Vacuity: a selector that never resolved proves nothing
// ---------------------------------------------------------------------------

/// The selector names a REGISTERED component with zero instances:
/// the type probe passes (definite resolution), the run
/// proceeds, and the post-run audit fails it as
/// `vacuous_invariant` — a watch that matched nothing proves
/// nothing (the parse-shape distinction from the sibling suites:
/// a typo'd name is oracle_error at the recorder; a valid name
/// with zero entities is vacuity at the audit).
#[test]
fn selector_matching_zero_entities_is_vacuous() {
    let _slot = common::run_slot();
    let mut app = base_app();
    // Registered, reflected, ZERO instances.
    app.register_type::<HappyGuest>();
    app.register_type::<HappyState>();
    let report = run_scenario(
        &mut app,
        &scenario_with_invariant(
            r#"
    "name": "watch_nobody",
    "rule": "entity_stagnation",
    "target_selector": {"with_component": {"name": "HappyGuest"}},
    "watch_path": "state.Queued.waited",
    "window_s": 2.0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    let v = report
        .violations
        .iter()
        .find(|v| v.rule == "vacuous_invariant" && v.target == "watch_nobody")
        .expect("expected a vacuous_invariant finding");
    assert!(v.detail.contains("matched zero entities"), "{}", v.detail);
}

/// Control: a selector whose entities exist but never satisfy
/// leaf_where is a PASS WITH EVIDENCE — a clean world is the
/// SUCCESS case for a stagnation rule (nobody was ever stuck),
/// not vacuity. (The held trace carries the story; asserted in
/// the held-invariants slice.)
#[test]
fn leaf_where_never_true_is_a_clean_pass_not_vacuity() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.world_mut().spawn((
        StagGuest {
            state: StagGuestState::Idle,
        },
        Transform::default(),
    ));
    let report = run_scenario(
        &mut app,
        &scenario_with_invariant(
            r#"
    "name": "queued_watch",
    "rule": "entity_stagnation",
    "target_selector": {"with_component": {"name": "StagGuest"}},
    "watch_path": "state.Queued.waited",
    "leaf_where": "state == Queued",
    "window_s": 2.0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.rule == "vacuous_invariant" || v.rule == "entity_stagnation"),
        "never-queued guests are a pass, not vacuity: {:?}",
        report.violations
    );
}

// ---------------------------------------------------------------------------
// Held trace: the near-miss story a passing run tells
// ---------------------------------------------------------------------------

/// A PASSING stagnation watch still leaves a held entry when it
/// judged entities: the sub-soak episodes it observed (and the
// soak judgment discarded) are exactly the run's near-misses, and
/// the trace must carry them. `waited` ticks — no stint ever
/// accrues — but `entity_judgments` counts the digests read at
/// every sampled frame (1 entity × every sample), and the entry
/// survives as evidence the watch was live.
#[test]
fn passing_watch_reports_entity_judgments_in_the_held_trace() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.add_systems(Update, |mut q: Query<&mut StagGuest>| {
        for mut g in q.iter_mut() {
            if let StagGuestState::Queued { waited } = &mut g.state {
                *waited += 1.0 / 60.0;
            }
        }
    });
    app.world_mut().spawn((
        StagGuest {
            state: StagGuestState::Queued { waited: 0.0 },
        },
        Transform::default(),
    ));
    let report = run_scenario(&mut app, &scenario_with_invariant(QUEUE_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    let held = report
        .held_invariants
        .iter()
        .find(|h| h.name == "queued_guests_must_not_wait_forever")
        .expect("a passing stagnation watch still earns a held entry");
    assert_eq!(held.rule, "entity_stagnation");
    // 6.0 s at 60 tps = 360 ticks, stride 60: samples at frames
    // 0, 60, ..., 300 — six samples, one digest each.
    assert_eq!(held.entity_judgments, Some(6));
    assert_eq!(held.episodes_observed, Some(0), "nothing was ever stuck");
    // NB: `episodes_fired` is the stagnant-resource tier's field;
    // the episode tier's near-miss story is `episodes_observed`.
    assert!(held.worst_episode_span.is_none());
}

/// The near-miss story: a guest stuck only BELOW the soak (frozen
/// 1.5 s against a 2.0 s window, then hops) is the discard the
/// held trace exists to keep. The stint must show up as exactly
/// one observed episode with its banked span, and the worst-span
/// witness must carry it — while the run still passes.
#[test]
fn sub_soak_stint_lands_in_the_held_trace_as_a_near_miss() {
    let _slot = common::run_slot();
    let mut app = base_app();
    // Frozen until frame 155 (between samples 120 and 180), then
    // ticking continuously: the stint seeds at sample 60 (first
    // 0.0), accrues one inter-sample interval (1.0 s of frame
    // clock) at sample 120, and the hop+tick closes it at sample
    // 180 — exactly one sub-soak near-miss, never a second stint.
    app.add_systems(
        Update,
        |mut q: Query<&mut StagGuest>, mut clock: Local<u32>| {
            *clock += 1;
            if *clock > 155 {
                for mut g in q.iter_mut() {
                    if let StagGuestState::Queued { waited } = &mut g.state {
                        *waited += 1.0 / 60.0;
                    }
                }
            }
        },
    );
    app.world_mut().spawn((
        StagGuest {
            state: StagGuestState::Queued { waited: 0.0 },
        },
        Transform::default(),
    ));
    let report = run_scenario(&mut app, &scenario_with_invariant(QUEUE_INV)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    let held = report
        .held_invariants
        .iter()
        .find(|h| h.name == "queued_guests_must_not_wait_forever")
        .expect("the near-miss run keeps its held entry");
    // Exactly one closed sub-soak episode; its span is ≥ the one
    // inter-sample interval the stint covered (1.0 s) and < the
    // 2.0 s soak. The worst-span witness names the same episode.
    assert_eq!(held.episodes_observed, Some(1));
    let (span, frame) = held
        .worst_episode_span
        .zip(held.worst_episode_frame)
        .expect("the near-miss span and its start frame are witnessed");
    assert!(
        (1.0..2.0).contains(&span),
        "the sub-soak span must be banked: {span}"
    );
    assert!(frame > 0, "the stint started after the first sample");
}

/// Vacuous-not-held doctrine: a watch whose selector resolved the
/// TYPE but matched zero entities gets NO held entry — an entry
/// would claim a workload that never happened. (The audit already
/// fails the run as `vacuous_invariant`; the collector's guard is
/// the belt-and-braces mirror for the manual-App path.)
#[test]
fn vacuous_watch_gets_no_held_entry() {
    let _slot = common::run_slot();
    let mut app = base_app();
    // Registered, reflected, ZERO instances.
    app.register_type::<HappyGuest>();
    app.register_type::<HappyState>();
    let report = run_scenario(
        &mut app,
        &scenario_with_invariant(
            r#"
    "name": "watch_nobody_held",
    "rule": "entity_stagnation",
    "target_selector": {"with_component": {"name": "HappyGuest"}},
    "watch_path": "state.Queued.waited",
    "window_s": 2.0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    assert!(
        !report
            .held_invariants
            .iter()
            .any(|h| h.name == "watch_nobody_held"),
        "a zero-instance watch must not claim held evidence"
    );
}

/// The never-stuck complement: the watch judged entities every
/// sample but no stint ever accrued (leaf ticks, or a never-true
/// leaf_where), and the run passes. Distinct from vacuity: the
/// held entry EXISTS (judged entities) and reports zero episodes.
#[test]
fn never_stuck_watch_keeps_a_held_entry_with_zero_episodes() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.world_mut().spawn((
        StagGuest {
            state: StagGuestState::Idle,
        },
        Transform::default(),
    ));
    let report = run_scenario(
        &mut app,
        &scenario_with_invariant(
            r#"
    "name": "queued_watch",
    "rule": "entity_stagnation",
    "target_selector": {"with_component": {"name": "StagGuest"}},
    "watch_path": "state.Queued.waited",
    "leaf_where": "state == Queued",
    "window_s": 2.0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    let held = report
        .held_invariants
        .iter()
        .find(|h| h.name == "queued_watch")
        .expect("a watch with a live selector keeps its held entry");
    assert_eq!(held.entity_judgments, Some(0), "no digests were ever read");
    assert_eq!(held.episodes_observed, Some(0));
}
