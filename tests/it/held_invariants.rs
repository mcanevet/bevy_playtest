//! `held_invariants` report section (the passing-invariant trace): the
//! pass-side evidence trace — one compact object per PASSING
//! resource_check / stagnant_resource invariant.
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
use bevy_transform::prelude::Transform;

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
        });
    }
    let json = serde_json::to_string(&entries).unwrap();
    // Spec claims ≈3 KB for 20 invariants. Allow generous headroom to
    // a hard bound of 4 KB so formatting drift doesn't flake CI.
    assert!(
        json.len() < 4 * 1024,
        "20 held_invariants serialized to {} bytes (> 4 KB)",
        json.len()
    );
}

// ---------------------------------------------------------------------------
// BatteryConfig::new (the constructor fix)
// ---------------------------------------------------------------------------

/// `BatteryConfig::new` provides the common-path construction the
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
