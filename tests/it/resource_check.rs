//! Behavioral battery for the resource half of the pluggable
//! invariant tier (the v0.14 "judgement layer" first slice):
//!
//! - `resource_check`: an authored-predicate oracle over a reflected
//!   resource (always, or eventually within a deadline) — the
//!   resource sibling of `component_check`. Game-wide state like
//!   scores, economies and cooldowns.
//! - `stagnant_resource`: the rate-form livelock oracle born from the
//!   canonical livelock fixture: a match whose scoreboard stays [0,0] the
//!   whole game while the match clock advances IS a livelock, but
//!   every motion/state oracle stays green: the world churns, the
//!   clock runs, only the value under judgment stands still.
//!
//! Time in these fixtures is frame-counter driven (60 fps ⇒ 1/60 per
//! frame), mirroring the other suites: bare test apps do not run
//! TimePlugin, so `Res<Time>` would never advance.

// Shared test harness (run-slot serialization).
use crate::common;

use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_playtest::driver::run_scenario;
use bevy_playtest::rules::Rule;
use bevy_reflect::Reflect;
use bevy_transform::prelude::Transform;

// ---------------------------------------------------------------------------
// Game-side resources
// ---------------------------------------------------------------------------

/// The livelock shape: a scoreboard that never changes while the clock
/// runs.
#[derive(Resource, Reflect, Default)]
#[reflect(Resource)]
struct RcScoreboard {
    home: i64,
    away: i64,
}

/// Match clock in in-game seconds, always running while the match is
/// live.
#[derive(Resource, Reflect, Default)]
#[reflect(Resource)]
struct RcMatchClock {
    elapsed_s: f32,
}

/// RcEconomy: the money-non-conservation shape.
#[derive(Resource, Reflect, Default)]
#[reflect(Resource)]
struct RcEconomy {
    coins: i64,
}

// ---------------------------------------------------------------------------
// Systems (frame-counter time: 60 tps ⇒ dt = 1/60)
// ---------------------------------------------------------------------------

const DT: f32 = 1.0 / 60.0;

fn tick_clock(mut clock: ResMut<RcMatchClock>) {
    clock.elapsed_s += DT;
}

/// The livelock: players wander (world churns), but the scoreboard
/// NEVER changes — the livelock that motion oracles cannot see.
fn wander_players(mut q: Query<&mut Transform>) {
    for mut t in q.iter_mut() {
        t.translation.x = t.translation.x.sin() + 0.01;
    }
}

/// Healthy: goals at every 1.5 s multiple of the clock.
fn score_at(clock: Res<RcMatchClock>, mut board: ResMut<RcScoreboard>) {
    let ticks = (clock.elapsed_s / 1.5) as i64;
    if ticks > board.home {
        board.home = ticks;
    }
}

fn drain_economy(mut eco: ResMut<RcEconomy>) {
    eco.coins -= 1;
}

fn grow_economy(mut eco: ResMut<RcEconomy>) {
    eco.coins += 1;
}

fn freeze_economy(_: ResMut<RcEconomy>) {}

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

fn match_app() -> App {
    let mut app = App::new();
    app.register_type::<RcScoreboard>();
    app.register_type::<RcMatchClock>();
    app.register_type::<Transform>();
    app.insert_resource(RcScoreboard::default());
    app.insert_resource(RcMatchClock::default());
    app.add_systems(Update, (tick_clock, wander_players));
    app.world_mut().spawn(Transform::from_xyz(0.0, 1.0, 0.0));
    app
}

fn economy_app(economy: fn(ResMut<RcEconomy>)) -> App {
    let mut app = App::new();
    app.register_type::<RcEconomy>();
    app.register_type::<Transform>();
    app.insert_resource(RcEconomy::default());
    app.add_systems(Update, (economy, wander_players));
    app.world_mut().spawn(Transform::from_xyz(0.0, 1.0, 0.0));
    app
}

const STAGNANT_SCORE: &str = r#"
    "name": "score_moves",
    "rule": "stagnant_resource",
    "resource": "RcScoreboard",
    "path": "home",
    "driver_resource": "RcMatchClock",
    "driver_path": "elapsed_s",
    "window_s": 1.0"#;

// ---------------------------------------------------------------------------
// stagnant_resource
// ---------------------------------------------------------------------------

/// THE livelock shape: a "match" that churns forever, clock running,
/// score frozen at 0-0. Motion, liveness, progress oracles stay
/// green; only the rate-form invariant catches the livelock.
#[test]
fn frozen_scoreboard_on_running_clock_is_a_livelock() {
    let _slot = common::run_slot();
    let mut app = match_app();
    let report = run_scenario(&mut app, &scenario_with_invariants(STAGNANT_SCORE)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    let entry = report
        .violations
        .iter()
        .find(|v| v.rule == "stagnant_resource")
        .expect("stagnant_resource violation");
    assert_eq!(entry.target, "score_moves");
    assert!(
        entry.detail.contains("RcScoreboard.home"),
        "{}",
        entry.detail
    );
    assert!(
        entry.detail.contains("RcMatchClock.elapsed_s"),
        "{}",
        entry.detail
    );
}

/// Control: a goal scored at ~1.5 s resets the window (window_s =
/// 2.5 s of clock) — the match is healthy, the run passes.
#[test]
fn goal_within_window_passes() {
    let _slot = common::run_slot();
    let mut app = match_app();
    app.add_systems(Update, score_at);
    let report = run_scenario(
        &mut app,
        &scenario_with_invariants(
            r#"
    "name": "score_moves",
    "rule": "stagnant_resource",
    "resource": "RcScoreboard",
    "path": "home",
    "driver_resource": "RcMatchClock",
    "driver_path": "elapsed_s",
    "window_s": 2.5"#,
        ),
    )
    .unwrap();

    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
}

/// Window tighter than the goal interval: the stagnation between
/// the run start and the 1.5 s goal (window_s = 1.0) fires — proving
/// the window is judged against DRIVER advance, not run length.
#[test]
fn stagnation_between_goals_fires() {
    let _slot = common::run_slot();
    let mut app = match_app();
    app.add_systems(Update, score_at);
    let report = run_scenario(&mut app, &scenario_with_invariants(STAGNANT_SCORE)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
}

/// A non-advancing driver freezes the window: time stopped, so
/// stagnation is not provable (pause screens, victory states). A
/// paused clock may legitimately hold a frozen scoreboard — the
/// deliberate distinction from frozen_world.
#[test]
fn frozen_clock_freezes_the_window() {
    let _slot = common::run_slot();
    // A match app whose clock NEVER advances: time stopped, so
    // stagnation is not provable (pause screens, victory states). A
    // paused clock may legitimately hold a frozen scoreboard — the
    // deliberate distinction from frozen_world.
    let mut frozen = App::new();
    frozen.register_type::<RcScoreboard>();
    frozen.register_type::<RcMatchClock>();
    frozen.register_type::<Transform>();
    frozen.insert_resource(RcScoreboard::default());
    frozen.insert_resource(RcMatchClock::default());
    frozen.add_systems(Update, wander_players);
    frozen.world_mut().spawn(Transform::from_xyz(0.0, 1.0, 0.0));
    let report = run_scenario(&mut frozen, &scenario_with_invariants(STAGNANT_SCORE)).unwrap();
    // The clock never advances, so the window can never elapse: the
    // invariant holds its peace. The watched leaf resolved (the
    // scoreboard is present), so this is NOT vacuity either.
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.rule == "stagnant_resource"),
        "a frozen driver must not fire stagnant_resource, got: {:?}",
        report
            .violations
            .iter()
            .map(|v| format!("{}|{}|{}", v.rule, v.target, v.detail))
            .collect::<Vec<_>>()
    );
}

/// One finding per stagnation episode: constant → change → constant
/// produces exactly two episode findings, never per-frame spam.
#[test]
fn one_finding_per_episode() {
    let _slot = common::run_slot();
    #[derive(Resource, Reflect, Default)]
    #[reflect(Resource)]
    struct EpValues {
        watched: f32,
        driver: f32,
    }
    #[derive(Component, Reflect, Default)]
    #[reflect(Component)]
    struct EpMarker;

    fn episode_driver(mut v: ResMut<EpValues>, mut q: Query<&mut Transform, With<EpMarker>>) {
        v.driver += DT;
        // Watched: constant for the first 2 s, changes at t=2, then
        // constant for the rest (a second episode).
        if v.driver >= 2.0 && v.watched == 0.0 {
            v.watched = 1.0;
        }
        for mut t in q.iter_mut() {
            t.translation.x = t.translation.x.sin() + 0.01;
        }
    }

    let mut app = App::new();
    app.register_type::<EpValues>();
    app.register_type::<EpMarker>();
    app.register_type::<Transform>();
    app.insert_resource(EpValues::default());
    app.add_systems(Update, episode_driver);
    app.world_mut()
        .spawn((EpMarker, Transform::from_xyz(0.0, 1.0, 0.0)));
    let report = run_scenario(
        &mut app,
        &scenario_with_invariants(
            r#"
    "name": "ep",
    "rule": "stagnant_resource",
    "resource": "EpValues",
    "path": "watched",
    "driver_path": "driver",
    "window_s": 1.0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    // House dedupe doctrine: report() keys by (rule, target), so both
    // episodes collapse into ONE entry whose count is the number of
    // firings and whose frames span both episodes. The invariant is
    // "one finding per EPISODE" at the recorder level (never
    // per-frame spam); the report layer then coalesces by target as
    // it does for every other rule.
    let findings = report
        .violations
        .iter()
        .filter(|v| v.rule == "stagnant_resource" && v.target == "ep")
        .collect::<Vec<_>>();
    assert_eq!(
        findings.len(),
        1,
        "expected exactly one coalesced finding, got: {:?}",
        findings
            .iter()
            .map(|v| (&v.first_frame, &v.last_frame, &v.count, &v.detail))
            .collect::<Vec<_>>()
    );
    let finding = findings[0];
    assert_eq!(
        finding.count, 2,
        "two stagnation episodes must produce two recorder firings (coalesced)"
    );
    // Episode 1 spans the run start; episode 2 spans from the change.
    assert!(finding.last_frame > finding.first_frame);
}

// ---------------------------------------------------------------------------
// resource_check: always mode
// ---------------------------------------------------------------------------

const COINS_NONNEGATIVE: &str = r#"
    "name": "coins_nonnegative",
    "rule": "resource_check",
    "resource": "RcEconomy",
    "path": "coins",
    "check": "ge",
    "value": 0"#;

/// The money shape: coins going negative violates the
/// authored always-intent.
#[test]
fn negative_coins_fail_always_check() {
    let _slot = common::run_slot();
    let mut app = economy_app(drain_economy);
    let report = run_scenario(&mut app, &scenario_with_invariants(COINS_NONNEGATIVE)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    let entry = report
        .violations
        .iter()
        .find(|v| v.rule == "resource_check")
        .expect("resource_check violation");
    assert_eq!(entry.target, "coins_nonnegative");
    assert!(entry.detail.contains("RcEconomy.coins"), "{}", entry.detail);
}

/// Control: a growing economy satisfies always mode.
#[test]
fn growing_economy_passes_always_check() {
    let _slot = common::run_slot();
    let mut app = economy_app(grow_economy);
    let report = run_scenario(&mut app, &scenario_with_invariants(COINS_NONNEGATIVE)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
}

// ---------------------------------------------------------------------------
// resource_check: eventually mode
// ---------------------------------------------------------------------------

const RICH_EVENTUALLY: &str = r#"
    "name": "rich_eventually",
    "rule": "resource_check",
    "resource": "RcEconomy",
    "path": "coins",
    "check": "gt",
    "value": 100,
    "eventually_s": 3.0"#;

/// Growth at 60/s crosses 100 within the 3 s deadline.
#[test]
fn eventually_satisfied_by_growth() {
    let _slot = common::run_slot();
    let mut app = economy_app(grow_economy);
    let report = run_scenario(&mut app, &scenario_with_invariants(RICH_EVENTUALLY)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
}

/// A frozen economy never crosses: deadline miss reported at run end
/// by the post-run audit.
#[test]
fn eventually_deadline_miss_fails() {
    let _slot = common::run_slot();
    let mut app = economy_app(freeze_economy);
    let report = run_scenario(&mut app, &scenario_with_invariants(RICH_EVENTUALLY)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    let entry = report
        .violations
        .iter()
        .find(|v| v.rule == "resource_check")
        .expect("resource_check deadline violation");
    assert_eq!(entry.target, "rich_eventually");
    assert!(entry.detail.contains("deadline"), "{}", entry.detail);
}

// ---------------------------------------------------------------------------
// Vacuity / oracle-error honesty
// ---------------------------------------------------------------------------

/// A resource that is registered but never inserted: the check
/// resolves nothing the whole run → vacuous_invariant at audit time
/// (never a silent green).
#[test]
fn never_inserted_resource_is_vacuous() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.register_type::<RcEconomy>();
    app.register_type::<Transform>();
    app.add_systems(Update, wander_players);
    app.world_mut().spawn(Transform::from_xyz(0.0, 1.0, 0.0));
    let report = run_scenario(&mut app, &scenario_with_invariants(COINS_NONNEGATIVE)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.rule == "vacuous_invariant" && v.target == "coins_nonnegative"),
        "expected vacuous_invariant for the unresolved resource, got: {:?}",
        report
            .violations
            .iter()
            .map(|v| format!("{}: {}", v.rule, v.target))
            .collect::<Vec<_>>()
    );
}

/// Unknown resource name → oracle_error (author-side failure, never
/// silent, never blamed on the game).
#[test]
fn unknown_resource_name_is_oracle_error() {
    let _slot = common::run_slot();
    let mut app = economy_app(grow_economy);
    let report = run_scenario(
        &mut app,
        &scenario_with_invariants(
            r#"
    "name": "typo_check",
    "rule": "resource_check",
    "resource": "ScoreboarD",
    "path": "home",
    "check": "ge",
    "value": 0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.rule == "oracle_error" && v.target == "typo_check"),
        "expected oracle_error, got: {:?}",
        report
            .violations
            .iter()
            .map(|v| format!("{}: {}", v.rule, v.target))
            .collect::<Vec<_>>()
    );
}

/// A component (not a resource) named via `resource` → oracle_error,
/// not a silent zero-entity pass.
#[test]
fn component_type_as_resource_is_oracle_error() {
    let _slot = common::run_slot();
    #[derive(Component, Reflect, Default)]
    #[reflect(Component)]
    struct NotAResource {
        v: f32,
    }
    let mut app = App::new();
    app.register_type::<NotAResource>();
    app.register_type::<Transform>();
    app.add_systems(Update, wander_players);
    app.world_mut()
        .spawn((NotAResource::default(), Transform::from_xyz(0.0, 1.0, 0.0)));
    let report = run_scenario(
        &mut app,
        &scenario_with_invariants(
            r#"
    "name": "comp_as_res",
    "rule": "resource_check",
    "resource": "NotAResource",
    "path": "v",
    "check": "ge",
    "value": 0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.rule == "oracle_error" && v.target == "comp_as_res"),
        "expected oracle_error for a component-not-resource, got: {:?}",
        report
            .violations
            .iter()
            .map(|v| format!("{}: {}", v.rule, v.target))
            .collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// Gates and registry invariants
// ---------------------------------------------------------------------------

#[test]
fn resource_rules_are_registered_and_gating() {
    let rc = Rule::from_name("resource_check").expect("resource_check registered");
    let sr = Rule::from_name("stagnant_resource").expect("stagnant_resource registered");
    assert!(!rc.is_informational());
    assert!(!sr.is_informational());
}

/// Validation battery: every misauthored shape rejects at load.
#[test]
fn resource_invariant_validation_rejections() {
    use bevy_playtest::scenario::{validate_scenario, Scenario, ScenarioError};
    let parse = |invariants: &str| -> Result<(), ScenarioError> {
        let json = format!(
            r#"{{"bot":{{"type":"chaos","seed":42}},"duration_s":4.0,"tps":60,
                "invariants":[{invariants}]}}"#
        );
        let s: Scenario = serde_json::from_str(&json).unwrap();
        validate_scenario(&s).map(|_| ())
    };
    // resource_check: missing resource.
    assert!(matches!(
        parse(r#"{"name":"a","rule":"resource_check","path":"x","check":"ge","value":0}"#),
        Err(ScenarioError::Rejected(_))
    ));
    // resource_check: missing path.
    assert!(matches!(
        parse(r#"{"name":"a","rule":"resource_check","resource":"R","check":"ge","value":0}"#),
        Err(ScenarioError::Rejected(_))
    ));
    // resource_check: missing check.
    assert!(matches!(
        parse(r#"{"name":"a","rule":"resource_check","resource":"R","path":"x","value":0}"#),
        Err(ScenarioError::Rejected(_))
    ));
    // resource_check: missing value.
    assert!(matches!(
        parse(r#"{"name":"a","rule":"resource_check","resource":"R","path":"x","check":"ge"}"#),
        Err(ScenarioError::Rejected(_))
    ));
    // resource_check: target_selector is the component vocabulary.
    assert!(matches!(
        parse(
            r#"{"name":"a","rule":"resource_check","resource":"R","target_selector":{"all":null},"path":"x","check":"ge","value":0}"#
        ),
        Err(ScenarioError::Rejected(_))
    ));
    // resource_check: window_s belongs to stagnant_resource.
    assert!(matches!(
        parse(
            r#"{"name":"a","rule":"resource_check","resource":"R","path":"x","check":"ge","value":0,"window_s":1.0}"#
        ),
        Err(ScenarioError::Rejected(_))
    ));
    // resource_check: driver fields belong to stagnant_resource.
    assert!(matches!(
        parse(
            r#"{"name":"a","rule":"resource_check","resource":"R","path":"x","check":"ge","value":0,"driver_path":"y"}"#
        ),
        Err(ScenarioError::Rejected(_))
    ));
    // stagnant_resource: missing path.
    assert!(matches!(
        parse(
            r#"{"name":"a","rule":"stagnant_resource","resource":"R","driver_path":"y","window_s":1.0}"#
        ),
        Err(ScenarioError::Rejected(_))
    ));
    // stagnant_resource: missing driver_path.
    assert!(matches!(
        parse(
            r#"{"name":"a","rule":"stagnant_resource","resource":"R","path":"x","window_s":1.0}"#
        ),
        Err(ScenarioError::Rejected(_))
    ));
    // stagnant_resource: missing window_s.
    assert!(matches!(
        parse(
            r#"{"name":"a","rule":"stagnant_resource","resource":"R","path":"x","driver_path":"y"}"#
        ),
        Err(ScenarioError::Rejected(_))
    ));
    // stagnant_resource: predicate fields rejected.
    assert!(matches!(
        parse(
            r#"{"name":"a","rule":"stagnant_resource","resource":"R","path":"x","driver_path":"y","window_s":1.0,"check":"ge","value":0}"#
        ),
        Err(ScenarioError::Rejected(_))
    ));
    // stagnant_resource: eventually_s rejected (the rule gates on it
    // via the global eventually-mode support check → NotImplemented;
    // asserting any load error keeps this robust to which guard fires
    // first).
    assert!(parse(
        r#"{"name":"a","rule":"stagnant_resource","resource":"R","path":"x","driver_path":"y","window_s":1.0,"eventually_s":1.0}"#
    ).is_err());
    // stagnant_resource: driver_resource identical to resource is
    // redundant noise (and a likely authoring confusion).
    assert!(matches!(
        parse(
            r#"{"name":"a","rule":"stagnant_resource","resource":"R","path":"x","driver_resource":"R","driver_path":"y","window_s":1.0}"#
        ),
        Err(ScenarioError::Rejected(_))
    ));
    // stagnant_resource: window_s is consumed in DRIVER-LEAF units,
    // not seconds — a window larger than the run no longer rejects
    // outright (the tick-counter trap: the driver
    // may advance many units per second). Judgeability moves to the
    // declared-rate fit check and the post-run elision audit.
    assert!(parse(
        r#"{"name":"a","rule":"stagnant_resource","resource":"R","path":"x","driver_path":"y","window_s":5.0}"#
    ).is_ok());
    // ...but WITH a declared rate the fit check rejects at load:
    // window / rate must fit the run duration.
    assert!(matches!(
        parse(
            r#"{"name":"a","rule":"stagnant_resource","resource":"R","path":"x","driver_path":"y","window_s":5.0,"driver_units_per_second":1.0}"#
        ),
        Err(ScenarioError::Rejected(_))
    ));
    // ...and a fast-enough declared rate makes the same window fit.
    assert!(parse(
        r#"{"name":"a","rule":"stagnant_resource","resource":"R","path":"x","driver_path":"y","window_s":5.0,"driver_units_per_second":4.0}"#
    ).is_ok());
    // driver_units_per_second: stagnant_resource-only surface.
    assert!(matches!(
        parse(
            r#"{"name":"a","rule":"resource_check","resource":"R","path":"x","check":"ge","value":0,"driver_units_per_second":20.0}"#
        ),
        Err(ScenarioError::Rejected(_))
    ));
    // driver_units_per_second: finite and > 0.
    assert!(matches!(
        parse(
            r#"{"name":"a","rule":"stagnant_resource","resource":"R","path":"x","driver_path":"y","window_s":1.0,"driver_units_per_second":0.0}"#
        ),
        Err(ScenarioError::Rejected(_))
    ));
    // resource_check: eventually_s exceeding the run duration is
    // unjudgeable.
    assert!(matches!(
        parse(
            r#"{"name":"a","rule":"resource_check","resource":"R","path":"x","check":"gt","value":0,"eventually_s":5.0}"#
        ),
        Err(ScenarioError::Rejected(_))
    ));
    // resource_check: bounds fields belong to nodes_in_bounds.
    assert!(matches!(
        parse(
            r#"{"name":"a","rule":"resource_check","resource":"R","path":"x","check":"ge","value":0,"min_x":0.0}"#
        ),
        Err(ScenarioError::Rejected(_))
    ));
    // Reserved-surface honesty for the OLD rule arms: the resource
    // vocabulary must not be silently ignored by component_check /
    // nodes_in_bounds.
    assert!(matches!(
        parse(
            r#"{"name":"a","rule":"component_check","target_selector":{"with_component":{"name":"Marker"}},"path":"x","check":"ge","value":0,"resource":"R"}"#
        ),
        Err(ScenarioError::Rejected(_))
    ));
    assert!(matches!(
        parse(
            r#"{"name":"a","rule":"component_check","target_selector":{"with_component":{"name":"Marker"}},"path":"x","check":"ge","value":0,"window_s":1.0}"#
        ),
        Err(ScenarioError::Rejected(_))
    ));
    assert!(matches!(
        parse(
            r#"{"name":"a","rule":"component_check","target_selector":{"with_component":{"name":"Marker"}},"path":"x","check":"ge","value":0,"driver_path":"y"}"#
        ),
        Err(ScenarioError::Rejected(_))
    ));
    assert!(matches!(
        parse(r#"{"name":"a","rule":"nodes_in_bounds","resource":"R","max_x":10.0}"#),
        Err(ScenarioError::Rejected(_))
    ));
    assert!(matches!(
        parse(r#"{"name":"a","rule":"nodes_in_bounds","window_s":1.0,"max_x":10.0}"#),
        Err(ScenarioError::Rejected(_))
    ));
    assert!(matches!(
        parse(r#"{"name":"a","rule":"nodes_in_bounds","driver_resource":"R","max_x":10.0}"#),
        Err(ScenarioError::Rejected(_))
    ));
    // Blank string fields reject at load (not silently at run time):
    // a blank path resolves the resource root, never a leaf.
    assert!(matches!(
        parse(
            r#"{"name":"a","rule":"resource_check","resource":"R","path":"","check":"ge","value":0}"#
        ),
        Err(ScenarioError::Rejected(_))
    ));
    assert!(matches!(
        parse(
            r#"{"name":"a","rule":"resource_check","resource":"R","path":"   ","check":"ge","value":0}"#
        ),
        Err(ScenarioError::Rejected(_))
    ));
    assert!(matches!(
        parse(
            r#"{"name":"a","rule":"stagnant_resource","resource":"R","path":"x","driver_path":"","window_s":1.0}"#
        ),
        Err(ScenarioError::Rejected(_))
    ));
    assert!(matches!(
        parse(
            r#"{"name":"a","rule":"stagnant_resource","resource":"R","path":"x","driver_path":"y","driver_resource":"   ","window_s":1.0}"#
        ),
        Err(ScenarioError::Rejected(_))
    ));
}

// ---------------------------------------------------------------------------
// Absence / removal honesty
//
// Bevy 0.20's remove_resource takes the component off the resource
// entity but leaves the resource_entities entry behind, so a removed
// resource reaches the reflect step and reads as "component gone".
// Removal is a legal game event (level teardown, back to menu): the
// invariant must read it as Absence (audit territory), never as an
// author-side oracle_error.
// ---------------------------------------------------------------------------

/// A game that REMOVES the watched resource mid-run: no oracle_error,
/// and the run is judged only by the pre-removal observation.
#[test]
fn removed_watched_resource_is_absence_not_oracle_error() {
    let _slot = common::run_slot();
    #[derive(Component, Reflect, Default)]
    #[reflect(Component)]
    struct RcTeardownMarker;

    fn teardown_game(
        mut clock: ResMut<RcMatchClock>,
        mut commands: Commands,
        mut torn_down: Local<bool>,
        mut q: Query<&mut Transform, With<RcTeardownMarker>>,
    ) {
        clock.elapsed_s += DT;
        if clock.elapsed_s >= 1.0 && !*torn_down {
            *torn_down = true;
            commands.remove_resource::<RcScoreboard>();
        }
        for mut t in q.iter_mut() {
            t.translation.x = t.translation.x.sin() + 0.01;
        }
    }

    let mut app = App::new();
    app.register_type::<RcScoreboard>();
    app.register_type::<RcMatchClock>();
    app.register_type::<Transform>();
    app.register_type::<RcTeardownMarker>();
    app.insert_resource(RcScoreboard::default());
    app.insert_resource(RcMatchClock::default());
    app.add_systems(Update, teardown_game);
    app.world_mut()
        .spawn((RcTeardownMarker, Transform::from_xyz(0.0, 1.0, 0.0)));

    let report = run_scenario(&mut app, &scenario_with_invariants(STAGNANT_SCORE)).unwrap();
    // The removal is a legal runtime event — absence, not config
    // failure. Remediation text telling the author to register their
    // types would be gaslighting: they are registered.
    assert!(
        !report.violations.iter().any(|v| v.rule == "oracle_error"),
        "mid-run resource removal must read as absence, not oracle_error: {:?}",
        report
            .violations
            .iter()
            .map(|v| format!("{}: {}", v.rule, v.detail))
            .collect::<Vec<_>>()
    );
    // Watched leaf removed at ~1 s with window_s=1.0: the
    // pre-removal episode (constant 0 from frame 0) hits the window
    // exactly at removal time... it FIRES before the removal (the
    // freeze happens around the same tick). Assert the coarse truth:
    // the finding, if any, is stagnation — never a false oracle
    // error. The precise boundary is pinned by the boundary test.
    let stagnant = report
        .violations
        .iter()
        .filter(|v| v.rule == "stagnant_resource")
        .count();
    assert!(stagnant <= 1);
}

/// Gap honesty: the DRIVER resource disappearing mid-run breaks
/// observation continuity. The returning frame must start a FRESH
/// window — a cross-gap delta may not be credited as witnessed
/// stagnation (false fire), and changes during the gap may not be
/// claimed as "stayed X for Y".
#[test]
fn driver_absence_gap_starts_a_fresh_window() {
    let _slot = common::run_slot();
    #[derive(Resource, Reflect, Default)]
    #[reflect(Resource)]
    struct RemovableClock {
        elapsed_s: f32,
    }
    #[derive(Component, Reflect, Default)]
    #[reflect(Component)]
    struct RcGapMarker;

    // Timeline (window_s = 1.0): the clock ticks to 0.5 s, is then
    // REMOVED (gap of unattributable time), and later RE-INSERTED
    // with a time skip (elapsed_s jumps to 1.4 s: 0.9 s of
    // unobserved advance). Without gap tracking the cross-gap delta
    // 0.9 s folds into the budget (0.5 + 0.9 = 1.4 > 1.0) and the
    // frozen scoreboard falsely fires. With gap tracking the return
    // frame re-seeds: fresh window, no fire.
    fn game(
        mut commands: Commands,
        clock: Option<ResMut<RemovableClock>>,
        board: Option<ResMut<RcScoreboard>>,
        mut q: Query<&mut Transform, With<RcGapMarker>>,
    ) {
        match (clock, board) {
            (Some(mut clock), Some(_board)) => {
                clock.elapsed_s += DT;
                if clock.elapsed_s >= 0.5 {
                    // Teardown: the clock resource goes away.
                    commands.remove_resource::<RemovableClock>();
                }
            }
            (None, Some(_board)) => {
                // Re-insert after ~1 s of wall-run gap, with the
                // time skip baked in.
                commands.insert_resource(RemovableClock { elapsed_s: 1.4 });
            }
            _ => {}
        }
        for mut t in q.iter_mut() {
            t.translation.x = t.translation.x.sin() + 0.01;
        }
    }

    let mut app = App::new();
    app.register_type::<RcScoreboard>();
    app.register_type::<RemovableClock>();
    app.register_type::<Transform>();
    app.register_type::<RcGapMarker>();
    app.insert_resource(RcScoreboard::default());
    app.insert_resource(RemovableClock::default());
    app.add_systems(Update, game);
    app.world_mut()
        .spawn((RcGapMarker, Transform::from_xyz(0.0, 1.0, 0.0)));

    let report = run_scenario(
        &mut app,
        &scenario_with_invariants(
            r#"
    "name": "gap_test",
    "rule": "stagnant_resource",
    "resource": "RcScoreboard",
    "path": "home",
    "driver_resource": "RemovableClock",
    "driver_path": "elapsed_s",
    "window_s": 1.0"#,
        ),
    )
    .unwrap();
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.rule == "stagnant_resource"),
        "a fresh window after the gap must not fire from a cross-gap delta: {:?}",
        report
            .violations
            .iter()
            .map(|v| format!("{}: {}", v.rule, v.detail))
            .collect::<Vec<_>>()
    );
    assert!(
        !report.violations.iter().any(|v| v.rule == "oracle_error"),
        "the removal/re-insert is a legal game event, never oracle_error: {:?}",
        report
            .violations
            .iter()
            .map(|v| format!("{}: {}", v.rule, v.detail))
            .collect::<Vec<_>>()
    );
}

/// A `stagnant_resource` whose watched leaf resolves but whose DRIVER
/// never does: the vacuity audit must name the driver (never claim
/// the resource/leaf never resolved — it did).
#[test]
fn never_arriving_driver_is_named_in_vacuity() {
    let _slot = common::run_slot();
    #[derive(Component, Reflect, Default)]
    #[reflect(Component)]
    struct RcGhostMarker;

    // The board is present (and frozen) the whole run; the clock
    // resource is REGISTERED but never inserted — the driver never
    // resolves. Distinct from the frozen-clock test: a PRESENT
    // non-advancing driver holds the invariant's peace, but one that
    // never arrives makes stagnation unprovable → vacuous, and the
    // message must say so.
    let mut app = App::new();
    app.register_type::<RcScoreboard>();
    app.register_type::<RcMatchClock>();
    app.register_type::<Transform>();
    app.register_type::<RcGhostMarker>();
    app.insert_resource(RcScoreboard::default());
    app.add_systems(Update, wander_players);
    app.world_mut()
        .spawn((RcGhostMarker, Transform::from_xyz(0.0, 1.0, 0.0)));
    let report = run_scenario(&mut app, &scenario_with_invariants(STAGNANT_SCORE)).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    let entry = report
        .violations
        .iter()
        .find(|v| v.rule == "vacuous_invariant" && v.target == "score_moves")
        .expect("vacuous_invariant naming the invariant");
    assert!(
        entry.detail.contains("driver"),
        "the vacuity message must name the never-arriving driver: {}",
        entry.detail
    );
    assert!(
        entry.detail.contains("RcMatchClock"),
        "the vacuity message must name the driver resource: {}",
        entry.detail
    );
}

/// A NaN watched leaf must be reported loudly (the rule would
/// otherwise be permanently disarmed: NaN != NaN reads as "change").
#[test]
fn nan_watched_leaf_is_reported_not_swallowed() {
    let _slot = common::run_slot();
    #[derive(Resource, Reflect, Default)]
    #[reflect(Resource)]
    struct NanState {
        value: f32,
        driver: f32,
    }

    fn poison_at_two_s(mut s: ResMut<NanState>, mut q: Query<&mut Transform>) {
        s.driver += DT;
        if s.driver >= 2.0 {
            // An arithmetic fault: the watched leaf goes NaN.
            s.value = f32::NAN;
        } else {
            // Legitimately changing until the fault strikes — the
            // NaN oracle_error must be the ONLY finding.
            s.value = (s.driver * 60.0) % 2.0;
        }
        for mut t in q.iter_mut() {
            t.translation.x = t.translation.x.sin() + 0.01;
        }
    }

    let mut app = App::new();
    app.register_type::<NanState>();
    app.register_type::<Transform>();
    app.insert_resource(NanState::default());
    app.add_systems(Update, poison_at_two_s);
    app.world_mut().spawn(Transform::from_xyz(0.0, 1.0, 0.0));
    let report = run_scenario(
        &mut app,
        &scenario_with_invariants(
            r#"
    "name": "nan_watch",
    "rule": "stagnant_resource",
    "resource": "NanState",
    "path": "value",
    "driver_path": "driver",
    "window_s": 1.0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    assert!(
        report.violations.iter().any(|v| v.rule == "oracle_error"
            && v.target == "nan_watch"
            && v.detail.contains("non-finite")
            && v.detail.contains("watched")),
        "a NaN watched leaf must fail loudly, got: {:?}",
        report
            .violations
            .iter()
            .map(|v| format!("{}: {}", v.rule, v.detail))
            .collect::<Vec<_>>()
    );
}

/// A non-finite DRIVER leaf must be reported just as loudly as a
/// non-finite watched leaf: without the guard, a single NaN driver
/// sample seeds a NaN baseline so every later delta compares
/// NaN-side and the window silently freezes (or disarms) forever.
/// The driver here genuinely RECOVERS one frame after the glitch,
/// so this test also pins the re-arm: post-recovery stagnation is
/// judged over a fresh window of witnessed advance.
#[test]
fn nan_driver_leaf_is_reported_and_does_not_fold() {
    let _slot = common::run_slot();
    #[derive(Resource, Reflect, Default)]
    #[reflect(Resource)]
    struct NanDriverState {
        value: f32,
        driver: f32,
        clean: f32,
        poisoned: bool,
    }

    // The clean clock ticks for the whole run; the exposed driver
    // ticks along until ~1.5 s, emits ONE NaN frame (a transient
    // arithmetic fault), then snaps back to the clean clock. The
    // watched leaf churns (honestly changing) until the glitch and
    // freezes after it. Expectations:
    //  - exactly one oracle_error, naming the DRIVER leaf;
    //  - exactly one stagnant_resource fire, POST-recovery: the gap
    //    re-seed means it may only accrue witnessed post-glitch
    //    advance, so it lands ~1 s after the recovery frame —
    //    a permanently-disarmed window (no guard) yields no fire
    //    at all, and a fold-across-the-NaN fires far earlier.
    fn glitchy_clock(mut s: ResMut<NanDriverState>, mut q: Query<&mut Transform>) {
        s.clean += DT;
        if !s.poisoned && s.clean >= 1.5 {
            // One-frame arithmetic fault on the CLOCK...
            s.poisoned = true;
            s.driver = f32::NAN;
        } else {
            // ...then recovery: the driver snaps back to the clean
            // clock (NaN + dt would stay NaN forever).
            s.driver = s.clean;
        }
        if !s.poisoned {
            // Honestly changing until the fault strikes.
            s.value = (s.clean * 60.0) % 2.0;
        }
        for mut t in q.iter_mut() {
            t.translation.x = t.translation.x.sin() + 0.01;
        }
    }

    let mut app = App::new();
    app.register_type::<NanDriverState>();
    app.register_type::<Transform>();
    app.insert_resource(NanDriverState::default());
    app.add_systems(Update, glitchy_clock);
    app.world_mut().spawn(Transform::from_xyz(0.0, 1.0, 0.0));
    let report = run_scenario(
        &mut app,
        &scenario_with_invariants(
            r#"
    "name": "nan_driver",
    "rule": "stagnant_resource",
    "resource": "NanDriverState",
    "path": "value",
    "driver_path": "driver",
    "window_s": 1.0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    // The fault is reported ONCE, naming the driver leaf...
    let errors: Vec<_> = report
        .violations
        .iter()
        .filter(|v| v.rule == "oracle_error" && v.target == "nan_driver")
        .collect();
    assert_eq!(
        errors.len(),
        1,
        "exactly one oracle_error for the NaN driver frame: {:?}",
        errors
    );
    assert!(
        errors[0].detail.contains("driver leaf"),
        "the message must name the driver side: {}",
        errors[0].detail
    );
    // ...and the invariant RE-ARMS: the frozen post-glitch value
    // fires once, on witnessed post-recovery advance alone. The
    // glitch lands at frame ~90 (clean 1.5 s); recovery at ~91;
    // a fresh window then needs > 1 s of witnessed driver advance,
    // so the fire lands around frame 152 — well past the glitch.
    // (Report dedupes by (rule, target), so the single fire's
    // first_frame is the witness.)
    let fires: Vec<_> = report
        .violations
        .iter()
        .filter(|v| v.rule == "stagnant_resource" && v.target == "nan_driver")
        .collect();
    assert_eq!(
        fires.len(),
        1,
        "exactly one stagnation fire (post-recovery): {:?}",
        fires
    );
    assert!(
        fires[0].first_frame >= 140,
        "the fire must land on witnessed POST-recovery advance (expected ~frame 152), got {}: {}",
        fires[0].first_frame,
        fires[0].detail
    );
}

/// A retreating driver (rewind/seek) resets the window: stagnation
/// observed before the rewind may not fire afterwards. The branch
/// existed but was never exercised.
#[test]
fn retreating_driver_resets_the_window() {
    let _slot = common::run_slot();
    #[derive(Resource, Reflect, Default)]
    #[reflect(Resource)]
    struct SeekState {
        watched: f32,
        driver: f32,
    }

    // Driver advances 0→1.5 s while watched is CONSTANT (window_s=
    // 2.0: no fire yet), then the driver RETREATS to 0 (seek/replay
    // rewind) and advances again to 2.5 s. Naively the accumulated
    // advance looks like 4 s, but the rewind resets the window:
    // only 2.5 consecutive seconds ever elapsed — under window_s=2.0
    // this fires AFTER the rewind reaches 2.0 again. Watch: the
    // asserted invariant is that the pre-rewind advance was reset,
    // so the firing happens (roughly) at the SECOND crossing.
    fn seek_game(mut s: ResMut<SeekState>, mut rewound: Local<bool>, mut q: Query<&mut Transform>) {
        s.driver += DT;
        if !*rewound && s.driver > 1.5 {
            // One-shot rewind (seek/replay).
            *rewound = true;
            s.driver = 0.0;
        }
        for mut t in q.iter_mut() {
            t.translation.x = t.translation.x.sin() + 0.01;
        }
    }

    let mut app = App::new();
    app.register_type::<SeekState>();
    app.register_type::<Transform>();
    app.insert_resource(SeekState::default());
    app.add_systems(Update, seek_game);
    app.world_mut().spawn(Transform::from_xyz(0.0, 1.0, 0.0));
    let report = run_scenario(
        &mut app,
        &scenario_with_invariants(
            r#"
    "name": "seek_reset",
    "rule": "stagnant_resource",
    "resource": "SeekState",
    "path": "watched",
    "driver_path": "driver",
    "window_s": 2.0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    let entry = report
        .violations
        .iter()
        .find(|v| v.rule == "stagnant_resource" && v.target == "seek_reset")
        .expect("stagnant_resource fires after the rewind's second crossing");
    // The firing frame must be in the SECOND (post-rewind) epoch:
    // the run is 4 s at 60 tps = 240 frames; the rewind lands ~
    // frame 90 (1.5 s); the second crossing of window 2.0 lands ~
    // frame 90 + 120 = 210. Assert it fired well after the rewind.
    assert!(
        entry.first_frame >= 150,
        "the window must be RESET by the rewind (fire after frame ~210, got {}): {}",
        entry.first_frame,
        entry.detail
    );
}

/// The window boundary is STRICT: budget > window_s fires; budget ==
/// window_s does not. Drive with EXACT DYADIC steps (0.25 is exact in
/// f32) so the budget hits the window boundary precisely, then assert
/// the FIRE FRAME — an `>=` flip shifts it one frame earlier and this
/// test kills the mutant.
#[test]
fn window_boundary_is_strictly_greater() {
    let _slot = common::run_slot();
    #[derive(Resource, Reflect, Default)]
    #[reflect(Resource)]
    struct ExactState {
        watched: f32,
        driver: f32,
        frames: u32,
    }

    const STEP: f32 = 0.25; // exact in f32: budget arithmetic is exact

    // The watched leaf NEVER changes. The driver advances in exact
    // 0.25 steps, so at the frame where the budget EQUALS the window
    // (1.0 after 4 steps) the invariant must hold its peace; the
    // NEXT frame (budget 1.25) is the first strict exceed and must
    // fire there — not one frame earlier.
    fn exact_game(mut s: ResMut<ExactState>, mut q: Query<&mut Transform>) {
        s.frames += 1;
        if s.frames > 1 {
            // Frame 1 seeds the baseline (driver 0); every later
            // frame advances by one exact step.
            s.driver += STEP;
        }
        for mut t in q.iter_mut() {
            t.translation.x = t.translation.x.sin() + 0.01;
        }
    }

    let mut app = App::new();
    app.register_type::<ExactState>();
    app.register_type::<Transform>();
    app.insert_resource(ExactState::default());
    app.add_systems(Update, exact_game);
    app.world_mut().spawn(Transform::from_xyz(0.0, 1.0, 0.0));
    let report = run_scenario(
        &mut app,
        &serde_json::from_str(
            r#"{"bot":{"type":"chaos","seed":42},"duration_s":1.0,"tps":60,
                "invariants":[{"name":"exact_window","rule":"stagnant_resource",
                "resource":"ExactState","path":"watched","driver_path":"driver",
                "window_s":1.0}]}"#,
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    let entry = report
        .violations
        .iter()
        .find(|v| v.rule == "stagnant_resource" && v.target == "exact_window")
        .expect("the strictly-exceeded window must fire");
    // Fire lands at recorded frame 5: frame 0 seeds the baseline
    // (driver 0), frames 1..4 add 0.25 each (budget 0.25/0.50/0.75/
    // 1.00 — equality, no fire), and frame 5 is the first budget >
    // 1.0 (1.25). An >= flip fires one frame earlier (frame 4,
    // budget == 1.0) and fails this assertion.
    assert_eq!(
        entry.first_frame, 5,
        "fire must land on the first frame strictly past the window, not at \
         equality: {}",
        entry.detail
    );
    assert!(
        entry.detail.contains("1.250"),
        "the reported advance must be the exact dyadic overshoot: {}",
        entry.detail
    );
}

// ---------------------------------------------------------------------------
// oracle_error count semantics: once per error class
// ---------------------------------------------------------------------------

/// A dogfooded authoring failure: one typo'd path under
/// `resource_check` used to inflate `count` to the frame count
/// (observed `count: 300` on a 5 s run; a 20 s run hit 1200).
/// Under the once-per-error-class doctrine the entry is
/// reported exactly once per run it was present on,
/// `count` pinned to 1, and the recurrence folded into the detail.
#[test]
fn one_typo_path_is_counted_once_not_per_frame() {
    let _slot = common::run_slot();
    let mut app = economy_app(freeze_economy);
    let report = run_scenario(
        &mut app,
        &scenario_with_invariants(
            r#"
    "name": "typo_path",
    "rule": "resource_check",
    "resource": "RcEconomy",
    "path": "lumens",
    "check": "ge",
    "value": 0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    let entry = report
        .violations
        .iter()
        .find(|v| v.rule == "oracle_error" && v.target == "typo_path")
        .expect("oracle_error targeting typo_path");
    // THE fix: one authoring mistake, one count — not one per frame
    // (the 240-frame duration here used to yield count: 240).
    assert_eq!(
        entry.count, 1,
        "a static config error must not inflate count per frame: {entry:?}"
    );
    assert_eq!(entry.first_frame, 0);
    // Recurrence is folded into the detail: the entry spans the whole
    // run and says so, exactly once.
    assert!(
        entry
            .detail
            .contains("recurred across frames 0\u{2013}239; counted once"),
        "recurrence must be folded into the detail: {}",
        entry.detail
    );
    // The noun fix rides along: resource_check errors say "resource",
    // not the leaked "component".
    assert!(
        entry.detail.contains("does not resolve on this resource"),
        "resource_check path errors must name the resource noun: {}",
        entry.detail
    );
    assert!(
        !entry.detail.contains("does not resolve on this component"),
        "the old component-noun leak must be gone: {}",
        entry.detail
    );
}

/// A config error that onsets mid-run (driver resource appears only
/// partway through) is still counted once per error class; two
/// distinct mistakes on ONE invariant stay two findings — the
/// dedup key is (invariant, error class), never invariant alone.
#[test]
fn two_distinct_config_errors_on_one_invariant_are_two_findings() {
    let _slot = common::run_slot();
    let mut app = match_app();
    let report = run_scenario(
        &mut app,
        &scenario_with_invariants(
            r#"
    "name": "twice_bad",
    "rule": "resource_check",
    "resource": "NoSuchResource",
    "path": "x",
    "check": "ge",
    "value": 0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    let entries: Vec<_> = report
        .violations
        .iter()
        .filter(|v| v.rule == "oracle_error" && v.target == "twice_bad")
        .collect();
    // One distinct error (unknown resource, every frame) → exactly
    // one entry, counted once, recurrence folded.
    assert_eq!(
        entries.len(),
        1,
        "one error class must be one finding: {entries:?}"
    );
    assert_eq!(entries[0].count, 1);
}

/// The bounds recorder (`nodes_in_bounds` with a `with_component`
/// selector) gets the same once-guard: an unregistered selector
/// component names the invariant, fails the run, and is counted
/// once — never once per frame.
#[test]
fn bounds_recorder_config_error_is_counted_once() {
    let _slot = common::run_slot();
    let mut app = match_app();
    let report = run_scenario(
        &mut app,
        &scenario_with_invariants(
            r#"
    "name": "bounds_typo",
    "rule": "nodes_in_bounds",
    "target_selector": {"with_component": {"name": "NoSuchMarker"}},
    "min_x": -1.0, "max_x": 1.0, "min_y": -1.0, "max_y": 1.0,
    "min_z": -1.0, "max_z": 1.0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    let entry = report
        .violations
        .iter()
        .find(|v| v.rule == "oracle_error" && v.target == "bounds_typo")
        .expect("oracle_error targeting bounds_typo");
    assert_eq!(
        entry.count, 1,
        "bounds-recorder config error counted once: {entry:?}"
    );
}

/// Resolving the bare short name "Time" as a driver_resource is
/// technically "unknown" (the short-name scan is exact-match, and
/// Time<()>/Time<Real>/Time<Virtual>/Time<Fixed> are all distinct
/// registered short paths) — but that verdict alone is helpless.
/// The did-you-mean hint must list the substring-matching short
/// paths so the author sees the concrete generic spellings to
/// choose from, without resolution semantics changing at all.
#[test]
fn unknown_short_name_lists_similar_registered_types() {
    let _slot = common::run_slot();
    let mut app = match_app();
    // The engine clocks are registered by TimePlugin; without it
    // "Time" has nothing to match against.
    app.add_plugins(bevy_time::TimePlugin);
    let report = run_scenario(
        &mut app,
        &scenario_with_invariants(
            r#"
    "name": "time_hint",
    "rule": "stagnant_resource",
    "resource": "RcScoreboard",
    "path": "home",
    "driver_resource": "Time",
    "driver_path": "elapsed",
    "window_s": 1.0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    let entry = report
        .violations
        .iter()
        .find(|v| v.rule == "oracle_error" && v.target == "time_hint")
        .unwrap_or_else(|| {
            panic!(
                "expected oracle_error, got: {:?}",
                report
                    .violations
                    .iter()
                    .map(|v| format!("{}: {}", v.rule, v.detail))
                    .collect::<Vec<_>>()
            )
        });
    assert!(
        entry.detail.contains("known similar registered types"),
        "missing did-you-mean hint in: {}",
        entry.detail
    );
    assert!(
        entry.detail.contains("Time<Fixed>"),
        "hint should name the Time<*> generic spellings: {}",
        entry.detail
    );
}

// ---------------------------------------------------------------------------
// window_s driver-unit semantics (windows measured in the driver leaf's own unit)
// ---------------------------------------------------------------------------

/// A tick counter driving the stagnation window,
/// advancing +1 per fixed tick at 20 ticks/s (every 3rd frame at
/// 60 tps). One driver unit = 1/20 s — NOT one second: the exact
/// class of driver the old `window_s > duration_s` validation
/// misjudged.
#[derive(Resource, Reflect, Default)]
#[reflect(Resource)]
struct RcSimTick {
    tick: u64,
}

/// A tick-driven economy a tick-driven economy: a drone
/// trip takes ~42-48 ticks; here a deposit lands every 45 ticks
/// (2.25 sim-seconds), so a window of 300 ticks never fires while a
/// window of 19 ticks (the R2 over-correction: < 1 s of activity
/// demand) fires repeatedly.
#[derive(Resource, Reflect, Default)]
#[reflect(Resource)]
struct RcHarvest {
    lumen: i64,
}

fn harvest(sim: Res<RcSimTick>, mut eco: ResMut<RcHarvest>) {
    eco.lumen = (sim.tick / 45) as i64;
}

/// One frame of the tick/harvest world: advances the tick counter
/// +1 every 3rd frame (20 ticks/s at 60 tps — a tick-counter shape)
/// and pays the harvest cadence. `Local<u32>` supplies the frame
/// count Bevy-free (same doctrine as the suite's other fixtures:
/// bare test apps do not run TimePlugin).
fn tick_sim_frame(mut frame: Local<u32>, mut sim: ResMut<RcSimTick>) {
    *frame += 1;
    if frame.is_multiple_of(3) {
        sim.tick += 1;
    }
}

/// Build the R1/R2 world: the tick counter, the harvest economy and
/// a wandering entity keeping the world observable.
fn sim_app() -> App {
    let mut app = App::new();
    app.register_type::<RcSimTick>();
    app.register_type::<RcHarvest>();
    app.register_type::<Transform>();
    app.insert_resource(RcSimTick::default());
    app.insert_resource(RcHarvest::default());
    app.add_systems(Update, (tick_sim_frame, harvest, wander_players));
    app.world_mut().spawn(Transform::from_xyz(0.0, 1.0, 0.0));
    app
}

/// A window sized in the author's mental unit —
/// ticks — used to be rejected at load as "can never elapse"
/// because validation silently assumed drivers advance at >= 1
/// unit/second. The run is 4 s / 240 frames / 80 sim ticks, and the
/// 45-tick harvest cadence sits comfortably inside a 60-unit window
/// (an honest window: elapses at tick 60).
/// Under the fix the scenario LOADS, RUNS, and PASSES.
#[test]
fn window_in_driver_units_loads_and_passes() {
    let _slot = common::run_slot();
    let mut app = sim_app();
    let report = run_scenario(
        &mut app,
        &scenario_with_invariants(
            r#"
    "name": "economy_flows_honest_window",
    "rule": "stagnant_resource",
    "resource": "RcHarvest",
    "path": "lumen",
    "driver_resource": "RcSimTick",
    "driver_path": "tick",
    "window_s": 60.0"#,
        ),
    )
    .unwrap();
    assert_eq!(
        report.status,
        bevy_playtest::enums::PlaytestStatus::Pass,
        "the tick-sized window is judgeable and satisfied: {:?}",
        report.violations
    );
    // No window_unelapsed note either: the driver (80 ticks) passed
    // the window (60 units).
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.rule == "window_unelapsed"),
        "the window elapsed; the elision audit must stay silent: {:?}",
        report.violations
    );
}

/// The R2 over-correction, suite-scale: the author HERDED BY R1's old
/// rejection sizes the window at 19 ticks (< 1 s of activity demand
/// against a 45-tick cadence). That shape is no longer forced on
/// anyone — but if authored anyway, the oracle still fires, honestly
/// and once per episode.
#[test]
fn forced_small_window_over_fires_as_documented() {
    let _slot = common::run_slot();
    let mut app = sim_app();
    let report = run_scenario(
        &mut app,
        &scenario_with_invariants(
            r#"
    "name": "economy_flows",
    "rule": "stagnant_resource",
    "resource": "RcHarvest",
    "path": "lumen",
    "driver_resource": "RcSimTick",
    "driver_path": "tick",
    "window_s": 19.0"#,
        ),
    )
    .unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    let entry = report
        .violations
        .iter()
        .find(|v| v.rule == "stagnant_resource" && v.target == "economy_flows")
        .expect("the over-firing shape still fires");
    // Cadence: change every 45 ticks; window 19 ticks ⇒ an episode
    // fires roughly every 45 ticks. The oracle behaves exactly as
    // specified — the FIX is that nobody is herded into writing
    // this window anymore.
    assert!(entry.count >= 1, "{}", entry.detail);
}

/// The elision audit (the post-run fallback when no rate is declared): a window the
/// driver never passed is INFORMATIONAL, never a pass->fail flip —
/// a present-but-slow driver holding the invariant's peace is the
/// 0.14 frozen-driver doctrine.
#[test]
fn window_never_elapsed_is_informational_window_unelapsed() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.register_type::<RcScoreboard>();
    app.register_type::<RcMatchClock>();
    app.register_type::<Transform>();
    app.insert_resource(RcScoreboard::default());
    app.insert_resource(RcMatchClock::default());
    app.add_systems(Update, (tick_clock, wander_players));
    app.world_mut().spawn(Transform::from_xyz(0.0, 1.0, 0.0));
    // Window 5.0 driver units on a clock advancing 1/60 per frame:
    // total advance over the 4 s run is 4.0 — the window never
    // elapses, the invariant is unjudgeable FOR THIS RUN.
    let report = run_scenario(
        &mut app,
        &scenario_with_invariants(
            r#"
    "name": "score_moves",
    "rule": "stagnant_resource",
    "resource": "RcScoreboard",
    "path": "home",
    "driver_resource": "RcMatchClock",
    "driver_path": "elapsed_s",
    "window_s": 5.0"#,
        ),
    )
    .unwrap();
    // INFORMATIONAL: the run still passes.
    assert_eq!(
        report.status,
        bevy_playtest::enums::PlaytestStatus::Pass,
        "window_unelapsed must never gate: {:?}",
        report.violations
    );
    let entry = report
        .violations
        .iter()
        .find(|v| v.rule == "window_unelapsed")
        .expect("the elision audit must fire");
    // The wording names the observed advance.
    assert!(
        entry.detail.contains("never elapsed"),
        "elision detail: {}",
        entry.detail
    );
    assert!(
        entry.detail.contains("advanced only"),
        "elision detail must name the observed advance: {}",
        entry.detail
    );
}

/// The declared-rate load-time fit check: a window that
/// cannot elapse at the DECLARED rate rejects at load with the
/// spec's wording — a numerically true message instead of the old
/// assumed-seconds rejection.
#[test]
fn declared_rate_fit_check_rejects_unelapsible_window_at_load() {
    let _slot = common::run_slot();
    let scenario = scenario_with_invariants(
        r#"
    "name": "economy_flows_honest_window",
    "rule": "stagnant_resource",
    "resource": "R",
    "path": "x",
    "driver_path": "y",
    "window_s": 300.0,
    "driver_units_per_second": 20.0"#,
    );
    // Suite scale: duration is 4 s, so 300 units at 20/s = 15 s does
    // not fit — the exact expected error shape.
    let err = bevy_playtest::scenario::validate_scenario(&scenario).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("driver units at declared 20 units/s = 15.0s"),
        "fit error must show the true arithmetic: {msg}"
    );
    assert!(
        msg.contains("check driver_units_per_second"),
        "fit error must point at the rate: {msg}"
    );
}

/// Two invariants sharing ONE driver leaf with DIFFERENT declared
/// rates are a consistency error: one driver leaf has one rate.
#[test]
fn conflicting_declared_rates_on_one_driver_reject() {
    let _slot = common::run_slot();
    let json = r#"{"bot":{"type":"chaos","seed":42},"duration_s":4.0,"tps":60,
        "invariants":[
          {"name":"a","rule":"stagnant_resource","resource":"R","path":"x",
           "driver_resource":"Sim","driver_path":"tick","window_s":10.0,
           "driver_units_per_second":20.0},
          {"name":"b","rule":"stagnant_resource","resource":"Q","path":"z",
           "driver_resource":"Sim","driver_path":"tick","window_s":20.0,
           "driver_units_per_second":60.0}
        ]}"#;
    let scenario: bevy_playtest::scenario::Scenario = serde_json::from_str(json).unwrap();
    let err = bevy_playtest::scenario::validate_scenario(&scenario).unwrap_err();
    assert!(
        err.to_string().contains("one driver leaf has one rate"),
        "conflicting rates must reject: {err}"
    );
}
