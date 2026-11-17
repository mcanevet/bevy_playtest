//! Defense against a foreign `log::set_max_level(Off)`: the bridge
//! (`tracing_log::LogTracer`) owns the write-once `log` logger slot,
//! but the max level is freely mutable and checked at every emission
//! site — game code or a test framework can blind log-crate capture
//! at any time while `bridge_won()` still reports `true`. The harness
//! re-asserts the level at run start and at every tick, bounding any
//! blinding to at most the remainder of one tick.
//!
//! Own test binary: like `log_crate_bridge.rs`, this suite mutates
//! process-global `log` state and `run_scenario` holds an
//! exclusive-run lock, so both phases run sequentially in one test.

use bevy_app::App;
use bevy_playtest::scenario::Scenario;

fn scenario() -> Scenario {
    serde_json::from_str(r#"{"bot":{"type":"chaos","seed":42},"duration_s":0.3,"tps":60}"#).unwrap()
}

/// Blinds the `log` crate once (first tick only), then keeps warning
/// every tick — mimicking a game (or test framework) that silences
/// `log` mid-run. After the first tick, the driver's per-tick
/// re-assert restores the level, so later warns must be captured.
fn blind_then_warn_app() -> App {
    let mut app = App::new();
    let mut blinded = false;
    app.add_systems(bevy_app::Update, move || {
        if !blinded {
            blinded = true;
            log::set_max_level(log::LevelFilter::Off);
        }
        log::warn!("warning[B0004]: must survive a foreign max-level Off");
    });
    app.world_mut()
        .spawn(bevy_transform::prelude::Transform::from_xyz(0.0, 0.0, 0.0));
    app
}

#[test]
fn foreign_set_max_level_off_cannot_blind_the_bridge() {
    // Phase 1: pre-run blinding. A foreign `Off` set BEFORE the first
    // run must be undone at run start — the warn reaches the report.
    {
        log::set_max_level(log::LevelFilter::Off);
        let mut app = blind_then_warn_app();
        let report = bevy_playtest::run_scenario(&mut app, &scenario()).unwrap();

        assert_eq!(
            report.log_capture_available,
            Some(true),
            "harness subscriber must win (no competing subscriber here)"
        );
        assert_eq!(
            report.log_bridge_available,
            Some(true),
            "no competing `log` logger exists in this process: the bridge must own the slot"
        );
        assert!(
            report.violations.iter().any(|v| v.rule == "log_warn"
                && v.detail.contains("must survive a foreign max-level Off")),
            "a pre-run log::set_max_level(Off) must not blind log-crate capture: {report:?}"
        );
    }

    // Phase 2: mid-run blinding. The Off lands during tick 0 (after
    // that tick's re-assert, blinding the rest of tick 0 at most),
    // but every subsequent tick re-asserts, so warns resume. Without
    // the per-tick defense the report would contain NO log_warn.
    {
        let mut app = blind_then_warn_app();
        let report = bevy_playtest::run_scenario(&mut app, &scenario()).unwrap();

        let warns: Vec<_> = report
            .violations
            .iter()
            .filter(|v| v.rule == "log_warn")
            .collect();
        assert!(
            warns
                .iter()
                .any(|v| v.detail.contains("must survive a foreign max-level Off")),
            "mid-run blinding must be bounded to one tick; later ticks must capture the warn: {report:?}"
        );
    }
}
