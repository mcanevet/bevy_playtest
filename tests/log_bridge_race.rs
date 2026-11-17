//! The log-bridge race: `log`'s global logger slot is write-once, just
//! like tracing's global default. When another `log` backend wins it
//! BEFORE the harness installs its bridge, log-crate events can never
//! reach the capture layer — the run must admit it via an
//! informational `log_bridge_lost` violation instead of implying the
//! `log` ecosystem was watched.
//!
//! Own test binary: claiming the global `log` logger (and thereby
//! forcing the bridge to lose) is a process-wide, irreversible act,
//! so it cannot share a binary with the capture-positive
//! log_crate_bridge tests.

use bevy_app::App;
use bevy_playtest::enums::PlaytestStatus;
use bevy_playtest::scenario::Scenario;

struct CompetingLogger;

impl log::Log for CompetingLogger {
    fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
        false
    }
    fn log(&self, _record: &log::Record<'_>) {}
    fn flush(&self) {}
}

fn scenario() -> Scenario {
    serde_json::from_str(r#"{"bot":{"type":"chaos","seed":42},"duration_s":0.2,"tps":60}"#).unwrap()
}

#[test]
fn lost_log_bridge_race_reports_log_bridge_lost() {
    // Simulate a game-provided log backend (or an earlier LogTracer)
    // winning the write-once slot BEFORE the harness ever installs
    // its bridge.
    log::set_boxed_logger(Box::new(CompetingLogger)).expect("first log logger wins");
    log::set_max_level(log::LevelFilter::Off);

    // Now the harness's bridge loses; ensure_global_subscriber must
    // record the loss (bridge_won == false).
    bevy_playtest::sinks::ensure_global_subscriber();
    assert!(
        !bevy_playtest::sinks::bridge_won(),
        "harness log bridge must be recorded as having LOST the race"
    );
    // The tracing subscriber itself has no competitor here, so the
    // report must distinguish the two losses.
    assert!(
        bevy_playtest::sinks::subscriber_won(),
        "capture subscriber must still win (only the log slot was taken)"
    );

    // A run under these conditions must report the bridge loss
    // honestly.
    let mut app = App::new();
    app.add_systems(bevy_app::Update, || {
        log::warn!("this log-crate warning is invisible to the harness");
    });
    app.world_mut()
        .spawn(bevy_transform::prelude::Transform::from_xyz(0.0, 0.0, 0.0));

    let report = bevy_playtest::run_scenario(&mut app, &scenario()).unwrap();
    assert_eq!(report.log_capture_available, Some(true));
    assert_eq!(
        report.log_bridge_available,
        Some(false),
        "bridge-lost run must declare log_bridge_available=false (compound-loss honesty field)"
    );
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.rule == "log_capture_lost"),
        "the capture subscriber won; log_capture_lost must NOT be reported: {:?}",
        report.violations
    );
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.rule == "log_bridge_lost"),
        "informational log_bridge_lost violation must be present: {:?}",
        report.violations
    );
    // Informational: must NOT gate the status.
    assert_eq!(report.status, PlaytestStatus::Pass);
}
