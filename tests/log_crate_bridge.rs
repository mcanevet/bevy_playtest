//! The `log`-crate bridge: bevy's B000x hierarchy validators (and
//! wgpu/naga/asset loaders/any `log`-crate user) emit through `log`,
//! NOT `tracing`. The only log→tracing bridge in a typical process
//! is LogPlugin's `tracing_log::LogTracer::init()` — and
//! PlaytestDefaultPlugins deliberately drops LogPlugin under a
//! scenario run so the harness subscriber can win the write-once
//! global default. Unless the harness installs its own bridge in
//! `ensure_global_subscriber`, every log-crate WARN/ERROR falls into
//! a black hole in exactly the mode that advertises log capture:
//! `log_capture_available: true` would be true but hollow.
//!
//! Own test binary: `run_scenario` holds a process-global active-run
//! lock (concurrent or nested runs are rejected), and installing a
//! global `log` logger is likewise a process-wide, write-once act —
//! so this suite runs its checks sequentially in one test.

use bevy_app::App;
use bevy_playtest::enums::PlaytestStatus;
use bevy_playtest::scenario::Scenario;

fn scenario(strict_warnings: bool) -> Scenario {
    serde_json::from_str(&format!(
        r#"{{"bot":{{"type":"chaos","seed":42}},"duration_s":0.2,"tps":60,"strict_warnings":{strict_warnings}}}"#
    ))
    .unwrap()
}

/// An app whose only system speaks the `log` crate — the same wire
/// protocol as bevy's B000x validators (`bevy_app` hierarchy.rs does
/// `use log::warn;`).
fn log_crate_app() -> App {
    let mut app = App::new();
    app.add_systems(bevy_app::Update, || {
        log::warn!("warning[B0004]: a log-crate WARN must reach the report");
    });
    app.world_mut()
        .spawn(bevy_transform::prelude::Transform::from_xyz(0.0, 0.0, 0.0));
    app
}

#[test]
fn log_crate_warn_is_captured_and_gates_only_when_strict() {
    // Informational by default: captured and reported, non-gating.
    {
        let mut app = log_crate_app();
        let report = bevy_playtest::run_scenario(&mut app, &scenario(false)).unwrap();

        assert_eq!(
            report.log_capture_available,
            Some(true),
            "harness subscriber must win (no competing subscriber here)"
        );
        let entry = report
            .violations
            .iter()
            .find(|v| v.rule == "log_warn")
            .unwrap_or_else(|| {
                panic!("log-crate WARN must surface as a log_warn violation entry: {report:?}")
            });
        assert!(
            entry.detail.contains("log-crate WARN"),
            "entry must carry the emitted message: {:?}",
            entry.detail
        );
        assert_eq!(
            report.status,
            PlaytestStatus::Pass,
            "log_warn is informational unless strict_warnings is set"
        );
    }

    // Strict: the same capture escalates to a gating failure.
    {
        let mut app = log_crate_app();
        let report = bevy_playtest::run_scenario(&mut app, &scenario(true)).unwrap();

        assert!(
            report.violations.iter().any(|v| v.rule == "log_warn"),
            "strict_warnings must not lose the entry, only gate on it: {report:?}"
        );
        assert_eq!(
            report.status,
            PlaytestStatus::Fail,
            "strict_warnings escalates log-crate WARNs to gating failures"
        );
    }
}
