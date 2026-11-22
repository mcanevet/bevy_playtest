//! Engine clock driver coverage: the registered `bevy_time` paths
//! documented in the scenario-schema reference ("Engine clock
//! drivers") spell definition modules, not the crate-root
//! re-exports, and the reference's cookbook table leans on them. These
//! fixtures prove each documented row resolves in a real registry and
//! (for Virtual) that a stagnant watch actually fires on it.
//!
//! The apps here run `TimePlugin` on a bare `App` (not the bare apps
//! of the sibling suites — `time_system` lives in `First` and the
//! fixed loop in `Main`, and both need the plugin's scheduling).
//! `run_scenario` installs its
//! `TimeUpdateStrategy::ManualDuration(1/tps)` update strategy, so
//! Real and Virtual advance deterministically by 1/60 s per update.

// Shared test harness (run-slot serialization).
use crate::common;

use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_playtest::driver::run_scenario;
use bevy_playtest::enums::PlaytestStatus;
use bevy_playtest::scenario::Scenario;
use bevy_reflect::Reflect;
use bevy_time::TimePlugin;
use bevy_transform::prelude::Transform;

/// Same shape as the sibling suites: a scoreboard that never changes
/// while the engine clock runs.
#[derive(Resource, Reflect, Default)]
#[reflect(Resource)]
struct EcScoreboard {
    home: i64,
}

/// Keep the motion oracles honest: the world churns (the point of the
/// rate-form oracle is that churn alone must not read as healthy).
fn wander_players(mut q: Query<&mut Transform>) {
    for mut t in q.iter_mut() {
        t.translation.x = t.translation.x.sin() + 0.01;
    }
}

fn engine_clock_app() -> App {
    let mut app = App::new();
    // A bare App already runs the Main schedule skeleton (the
    // sibling suites' Update systems run without plugins), so
    // TimePlugin alone is enough: it inserts + registers the three
    // engine clocks and schedules `time_system` in First and the
    // fixed loop in Main. The re-export spellings
    // (`bevy_time::Time<...>`) never register — the registry keys
    // are the definition-module paths.
    app.add_plugins(TimePlugin);
    app.register_type::<EcScoreboard>();
    app.register_type::<Transform>();
    app.insert_resource(EcScoreboard::default());
    app.add_systems(Update, wander_players);
    app.world_mut().spawn(Transform::from_xyz(0.0, 1.0, 0.0));
    app
}

fn scenario_with_invariants(invariants: &str, duration_s: f32) -> Scenario {
    let json = format!(
        r#"{{"bot":{{"type":"chaos","seed":42}},"duration_s":{duration_s},"tps":60,
            "invariants":[{{{invariants}}}]}}"#
    );
    serde_json::from_str(&json).unwrap()
}

const VIRTUAL_PATH: &str = "bevy_time::time::Time<bevy_time::virt::Virtual>";
const REAL_PATH: &str = "bevy_time::time::Time<bevy_time::real::Real>";
const FIXED_PATH: &str = "bevy_time::time::Time<bevy_time::fixed::Fixed>";

/// The reference's canonical engine driver: driving on
/// `Time<Virtual>.elapsed_secs_f64` must (a) resolve under the
/// documented definition-module path and (b) actually FIRE on a
/// frozen scoreboard — the driver leaf must strictly advance under
/// the harness's ManualDuration strategy, or the whole row is
/// documentation wishful thinking.
#[test]
fn virtual_clock_drives_a_stagnant_finding() {
    let _slot = common::run_slot();
    let mut app = engine_clock_app();
    let inv = format!(
        r#"
    "name": "score_moves_on_virtual_clock",
    "rule": "stagnant_resource",
    "resource": "EcScoreboard",
    "path": "home",
    "driver_resource": "{VIRTUAL_PATH}",
    "driver_path": "elapsed_secs_f64",
    "window_s": 1.0"#
    );
    let report = run_scenario(&mut app, &scenario_with_invariants(&inv, 4.0)).unwrap();
    assert_eq!(report.status, PlaytestStatus::Fail);
    let entry = report
        .violations
        .iter()
        .find(|v| v.rule == "stagnant_resource")
        .expect("stagnant_resource violation on the virtual-clock driver");
    assert!(
        entry.detail.contains("EcScoreboard.home"),
        "{}",
        entry.detail
    );
    assert!(
        entry
            .detail
            .contains(&format!("{VIRTUAL_PATH}.elapsed_secs_f64")),
        "finding must name the documented registered path: {}",
        entry.detail
    );
    // 1.0 window in seconds of virtual time: with dt = 1/60 s the
    // budget elapses at frame 60 — inside a 240-frame run.
    assert!(
        entry.detail.contains("stayed 0.000 for 1.000"),
        "{}",
        entry.detail
    );
}

/// The other two documented rows (Real, Fixed) must resolve and read
/// as authored `resource_check` leaves — an unknown path would come
/// back as `oracle_error` and fail the run instead of passing it.
#[test]
fn real_and_fixed_clocks_resolve_as_authored_leaves() {
    let _slot = common::run_slot();
    let mut app = engine_clock_app();
    let json = serde_json::json!({
        "bot": { "type": "chaos", "seed": 42 },
        "duration_s": 4.0,
        "tps": 60,
        "invariants": [
            {
                "name": "real_elapsed_nonneg",
                "rule": "resource_check",
                "resource": REAL_PATH,
                "path": "elapsed_secs_f64",
                "check": "ge",
                "value": 0.0
            },
            {
                "name": "fixed_elapsed_nonneg",
                "rule": "resource_check",
                "resource": FIXED_PATH,
                "path": "elapsed_secs_f64",
                "check": "ge",
                "value": 0.0
            }
        ]
    });
    let scenario: Scenario = serde_json::from_value(json).unwrap();
    let report = run_scenario(&mut app, &scenario).unwrap();
    let details: Vec<String> = report.violations.iter().map(|v| v.detail.clone()).collect();
    assert_eq!(
        report.status,
        PlaytestStatus::Pass,
        "both engine-clock leaves must resolve and hold: {details:?}"
    );
}
