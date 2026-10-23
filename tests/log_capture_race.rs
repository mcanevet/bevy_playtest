//! The LogPlugin race: when a game-installed tracing subscriber wins
//! the write-once global default BEFORE the harness's capture
//! subscriber installs, the run must admit log capture is lost —
//! report field `log_capture_available=false` + an informational
//! `log_capture_lost` violation — instead of implying "no errors
//! were logged" with a green report.
//!
//! Own test binary: installing a competing subscriber is a
//! process-wide, irreversible act (the harness's capture subscriber
//! can never win afterwards), so it cannot share a binary with the
//! capture-positive tests.

use bevy_app::App;
use bevy_playtest::enums::PlaytestStatus;
use bevy_playtest::scenario::Scenario;

fn scenario() -> Scenario {
    serde_json::from_str(r#"{"bot":{"type":"chaos","seed":42},"duration_s":0.2,"tps":60}"#).unwrap()
}

#[test]
fn lost_subscriber_race_reports_log_capture_lost() {
    // Simulate the game's LogPlugin winning the race: claim the
    // global default BEFORE the harness ever installs its capture
    // subscriber.
    use tracing_subscriber::layer::SubscriberExt;
    let noop = tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::sink));
    tracing::subscriber::set_global_default(noop).expect("first subscriber wins");

    // Now the harness's subscriber loses; ensure_global_subscriber
    // records the loss (SUBSCRIBER_WON=false).
    bevy_playtest::sinks::ensure_global_subscriber();
    assert!(
        !bevy_playtest::sinks::subscriber_won(),
        "harness subscriber must be recorded as having LOST the race"
    );

    // A run under these conditions must report the loss honestly.
    let mut app = App::new();
    app.add_systems(bevy_app::Update, || {
        tracing::error!("this error is invisible to the harness")
    });
    app.world_mut()
        .spawn(bevy_transform::prelude::Transform::from_xyz(0.0, 0.0, 0.0));

    let report = bevy_playtest::run_scenario(&mut app, &scenario()).unwrap();
    assert_eq!(
        report.log_capture_available,
        Some(false),
        "lost race must be declared in the report"
    );
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.rule == "log_capture_lost"),
        "informational log_capture_lost violation must be present: {:?}",
        report.violations
    );
    // Informational: must NOT gate the status (the run only logged,
    // did not violate a gating rule).
    assert_eq!(report.status, PlaytestStatus::Pass);
}
