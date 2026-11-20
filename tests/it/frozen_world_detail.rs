//! frozen_world detail refinement: a visually-silent streak is not
//! necessarily a soft-lock, and the oracle must SAY SO when it has
//! proof. Field-report case: a game whose match clock ticks in a
//! reflected resource while its presenter never writes Transform/UI
//! reported "soft-lock?" — sending the developer hunting for a
//! deadlock that did not exist (the sim was demonstrably stepping).
//!
//! The proof is the broadened state digest
//! (`record_digest: true`): it sees reflected game resources and
//! spawns/despawns that the Transform/UI liveness watchers cannot.
//! Tri-state behavior under test:
//!
//! - digest kept changing through the streak → detail says
//!   "stepping but visually silent…", `_meta.stepping == Some(true)`
//! - digest was flat too → legacy "soft-lock?" detail,
//!   `_meta.stepping == Some(false)` (corroborates a freeze but does
//!   not rewrite the detail — the digest is blind to e.g. sprite
//!   colors, so "flat" cannot PROVE a deadlock)
//! - no digest recorded → legacy "soft-lock?" detail, no `_meta`
//!   (honest unknown; byte-identical to pre-refinement reports)

// Shared test harness (run-slot serialization)
use crate::common;

use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_playtest::{
    enums::PlaytestStatus, run_scenario, scenario::Scenario, state::PlaytestReport,
};
use bevy_transform::prelude::*;

/// Short grace so tests run fast: liveness.timeout_s drives the grace
/// window (0.5 s at 60 tps = 30 frames) — the one source of truth.
fn frozen_scenario(record_digest: bool) -> Scenario {
    serde_json::from_str(&format!(
        r#"{{"bot":{{"type":"chaos","seed":7}},"duration_s":1.2,"tps":60,
            "liveness":{{"timeout_s":0.5}},
            "record_digest":{record_digest}}}"#
    ))
    .unwrap()
}

fn frozen_entry(report: &PlaytestReport) -> &bevy_playtest::state::ViolationEntry {
    report
        .violations
        .iter()
        .find(|v| v.rule == "frozen_world")
        .expect("frozen_world must fire")
}

/// The match-clock game: a STATIC transform plus a reflected resource
/// mutated every frame — the exact field-report shape. Only the
/// broadened digest can see the ticking clock.
fn match_clock_app() -> App {
    bevy_playtest::sinks::ensure_global_subscriber();
    #[derive(Resource, Debug, bevy_reflect::Reflect)]
    #[reflect(bevy_ecs::reflect::ReflectResource)]
    struct MatchClock(f32);

    let mut app = App::new();
    // Register reflection BEFORE inserting the resource (same
    // pattern as takeover_demo_nondet_resource).
    app.world_mut()
        .get_resource_or_insert_with(AppTypeRegistry::default)
        .0
        .write()
        .register::<MatchClock>();
    app.world_mut().insert_resource(MatchClock(0.0));
    // Static transform: nothing the liveness watchers observe changes.
    app.world_mut().spawn(Transform::from_xyz(1.0, 2.0, 0.0));
    app.add_systems(Update, |mut clock: ResMut<MatchClock>| {
        clock.0 += 1.0 / 60.0;
    });
    app
}

/// A fully static game: nothing changes anywhere, digest included.
fn static_app() -> App {
    bevy_playtest::sinks::ensure_global_subscriber();
    let mut app = App::new();
    app.world_mut().spawn(Transform::from_xyz(1.0, 2.0, 0.0));
    app
}

/// Positive case: with a digest proving the sim kept stepping, the
/// detail MUST drop the misleading "soft-lock?" and say what really
/// happened; `_meta.stepping` is `Some(true)` for machines.
#[test]
fn stepping_evidence_refines_the_detail() {
    let _slot = common::run_slot();
    let scenario = frozen_scenario(true);
    let mut app = match_clock_app();

    let report = run_scenario(&mut app, &scenario).expect("run completes");
    assert_ne!(report.status, PlaytestStatus::Crash, "run crashed");

    let entry = frozen_entry(&report);
    assert!(
        entry.detail.contains("stepping but visually silent"),
        "proven-stepping streak must say so, got: {:?}",
        entry.detail
    );
    assert!(
        !entry.detail.contains("soft-lock?"),
        "proven-stepping streak must NOT ask soft-lock?, got: {:?}",
        entry.detail
    );
    assert_eq!(
        entry.meta.as_ref().and_then(|m| m.stepping),
        Some(true),
        "_meta.stepping must be Some(true): {:?}",
        entry.meta
    );
}

/// Negative control: a fully static world with digests recorded gets
/// the LEGACY wording (a flat digest corroborates the freeze but
/// cannot prove a deadlock — the digest is blind to sprite colors and
/// materials), with `_meta.stepping == Some(false)` for machines.
#[test]
fn flat_digest_keeps_legacy_wording() {
    let _slot = common::run_slot();
    let scenario = frozen_scenario(true);
    let mut app = static_app();

    let report = run_scenario(&mut app, &scenario).expect("run completes");
    let entry = frozen_entry(&report);
    assert!(
        entry.detail.contains("soft-lock?"),
        "static world keeps the legacy wording, got: {:?}",
        entry.detail
    );
    assert_eq!(
        entry.meta.as_ref().and_then(|m| m.stepping),
        Some(false),
        "_meta.stepping must be Some(false): {:?}",
        entry.meta
    );
}

/// Honest-unknown control: WITHOUT digest recording there is no
/// evidence either way, so the report must be byte-equivalent to the
/// pre-refinement shape — legacy wording, and NO `_meta` at all (so
/// the battery's discovery annotation can still attach later).
#[test]
fn no_digest_keeps_legacy_wording_and_no_meta() {
    let _slot = common::run_slot();
    let scenario = frozen_scenario(false);
    let mut app = static_app();

    let report = run_scenario(&mut app, &scenario).expect("run completes");
    let entry = frozen_entry(&report);
    assert!(
        entry.detail.contains("soft-lock?"),
        "no-digest run keeps the legacy wording, got: {:?}",
        entry.detail
    );
    assert!(
        entry.meta.is_none(),
        "no-digest run must attach no _meta (unknown stepping): {:?}",
        entry.meta
    );
}

/// A match clock ticking WITHOUT digest recording stays unknown —
/// the ticking clock is invisible to the liveness watchers and there
/// is no digest to redeem it. Guards against the refinement ever
/// inventing evidence the run did not collect.
#[test]
fn stepping_without_digest_stays_honest_unknown() {
    let _slot = common::run_slot();
    let scenario = frozen_scenario(false);
    let mut app = match_clock_app();

    let report = run_scenario(&mut app, &scenario).expect("run completes");
    let entry = frozen_entry(&report);
    assert!(
        entry.detail.contains("soft-lock?"),
        "without digest evidence the oracle must not claim stepping: {:?}",
        entry.detail
    );
    assert!(
        entry.meta.is_none(),
        "no invented evidence: {:?}",
        entry.meta
    );
}

/// Interior-silent-tick evidence pin: digest state mutated ONLY at a
/// tick squarely INSIDE the silent streak must count as stepping.
/// The evidence window (`start = len - frozen_frames`) contains every
/// pair attributing to a tick the oracle counted silent — under BOTH
/// system alignments (game systems before OR after the oracle in
/// Update; in the latter the final mutation is only visible next
/// tick, shifting the streak start by one). The boundary pair itself
/// is intentionally EXCLUDED: including it would let a one-tick
/// `Changed<Transform>` visibility LAG fabricate "stepping" from the
/// pre-streak activity's digest pair, and starting there is the
/// conservative edge — under-claiming the very first silent tick is
/// safe (refusal over silence), never fabrication.
#[test]
fn interior_silent_frame_digest_change_counts_as_stepping() {
    let _slot = common::run_slot();
    #[derive(Resource, Debug, bevy_reflect::Reflect)]
    #[reflect(bevy_ecs::reflect::ReflectResource)]
    struct TicksOnce(f32);

    bevy_playtest::sinks::ensure_global_subscriber();
    let mut app = App::new();
    app.world_mut()
        .get_resource_or_insert_with(AppTypeRegistry::default)
        .0
        .write()
        .register::<TicksOnce>();
    app.world_mut().insert_resource(TicksOnce(0.0));
    app.world_mut().spawn(Transform::from_xyz(0.0, 0.0, 0.0));
    app.add_systems(
        Update,
        |state: Res<bevy_playtest::driver::RunState>,
         mut q: Query<&mut Transform>,
         mut clock: ResMut<TicksOnce>| {
            // Move through tick 9 (activity), then go silent…
            if state.frame < 10 {
                for mut t in q.iter_mut() {
                    t.translation.x += 0.1;
                }
            }
            // …but take exactly ONE digest-visible step midway through
            // the silent streak (well clear of either alignment's
            // boundary: silent from ~tick 11, crossing at ~tick 41),
            // then never again.
            if state.frame == 20 {
                clock.0 += 1.0;
            }
        },
    );

    let scenario = frozen_scenario(true);
    let report = run_scenario(&mut app, &scenario).expect("run completes");
    let entry = frozen_entry(&report);
    assert_eq!(
        entry.meta.as_ref().and_then(|m| m.stepping),
        Some(true),
        "an interior silent tick's digest change IS stepping \
         evidence; got detail {:?} meta {:?}",
        entry.detail,
        entry.meta
    );
    assert!(
        entry.detail.contains("stepping but visually silent"),
        "got: {:?}",
        entry.detail
    );
}
