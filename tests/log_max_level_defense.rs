//! Defense against a foreign `log::set_max_level(Off)`: the bridge
//! (`tracing_log::LogTracer`) owns the write-once `log` logger slot,
//! but the max level is freely mutable and checked at every emission
//! site — game code or a test framework can blind log-crate capture
//! at any time while `bridge_won()` still reports `true`. While the
//! bridge owns the slot, the max level is harness-owned state for the
//! run: the driver re-asserts it at run start and at every tick, so a
//! ONE-TIME `Off` blinds at most the remainder of the tick it lands
//! in. A `Off` re-applied every tick is NOT repaired — each tick's
//! tail stays lost — but it is DETECTED and reported as the
//! informational `log_level_tampered` violation (once per tampered
//! tick).
//!
//! Own test binary: like `log_crate_bridge.rs`, this suite mutates
//! process-global `log` state and `run_scenario` holds an
//! exclusive-run lock, so all phases run sequentially in one test.

use bevy_app::{App, Plugin};
use bevy_playtest::scenario::Scenario;

fn scenario() -> Scenario {
    serde_json::from_str(r#"{"bot":{"type":"chaos","seed":42},"duration_s":0.3,"tps":60}"#).unwrap()
}

/// Blinds the `log` crate once (first tick only), then keeps warning
/// every tick — mimicking a game (or test framework) that silences
/// `log` mid-run. After the first tick, the driver's per-tick
/// re-assert restores the level, so later warns must be captured.
fn blind_once_then_warn_app() -> App {
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

/// Re-blinds the `log` crate EVERY tick, before warning — the
/// persistent-tamper case the re-assert cannot repair (each tick's
/// tail is lost) but the post-tick detection must surface as
/// `log_level_tampered`.
fn blind_every_tick_app() -> App {
    let mut app = App::new();
    app.add_systems(bevy_app::Update, || {
        log::set_max_level(log::LevelFilter::Off);
        log::warn!("warning[B0004]: suppressed tail of every tick");
    });
    app.world_mut()
        .spawn(bevy_transform::prelude::Transform::from_xyz(0.0, 0.0, 0.0));
    app
}

/// Warns from `Plugin::finish` — the setup window (plugin finishing,
/// census) BETWEEN run start and the first tick. Only the RUN-START
/// re-assert covers it: the per-tick assert at tick 0 runs after
/// `finish()` has already dropped the event. Pins that re-assert
/// against deletion (the loop assert alone would not catch it).
struct FinishWarnPlugin;

impl Plugin for FinishWarnPlugin {
    fn build(&self, _app: &mut App) {}

    fn finish(&self, _app: &mut App) {
        log::warn!("warning[B0004]: finish() must survive a foreign max-level Off");
    }
}

#[test]
fn foreign_set_max_level_off_cannot_blind_the_bridge() {
    // Phase 1: pre-run blinding. A foreign `Off` set BEFORE the run
    // must be undone at run start: the finish/setup window AND every
    // tick except the one the game itself blinds mid-run capture the
    // warn. Only tick 0's tail (blinded by the test system, not the
    // pre-run Off) is lost, so `log_level_tampered` fires exactly
    // once and later warns stream in.
    {
        log::set_max_level(log::LevelFilter::Off);
        let mut app = blind_once_then_warn_app();
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
        let warns = report
            .violations
            .iter()
            .find(|v| v.rule == "log_warn")
            .expect("pre-run Off must not blind capture");
        assert!(
            warns
                .detail
                .contains("must survive a foreign max-level Off"),
            "wrong warn captured: {}",
            warns.detail
        );
        assert!(warns.count >= 5, "most ticks' warns must be captured");
        let tampered: Vec<_> = report
            .violations
            .iter()
            .filter(|v| v.rule == "log_level_tampered")
            .collect();
        assert_eq!(
            tampered.len(),
            1,
            "exactly one tampered tick (the game's own tick-0 Off), detected not repaired: {:?}",
            report.violations
        );
    }

    // Phase 2: mid-run one-shot blinding. The Off lands during tick 0
    // (blinding the rest of that tick at most); every subsequent
    // tick re-asserts first, so warns resume, and the one lost tail
    // is reported via `log_level_tampered` (count 1), not silently.
    {
        let mut app = blind_once_then_warn_app();
        let report = bevy_playtest::run_scenario(&mut app, &scenario()).unwrap();

        let warns = report
            .violations
            .iter()
            .find(|v| v.rule == "log_warn")
            .expect("mid-run one-shot blinding must be bounded to one tick");
        assert!(
            warns
                .detail
                .contains("must survive a foreign max-level Off"),
            "wrong warn captured: {}",
            warns.detail
        );
        assert!(warns.count >= 5, "later ticks must stream warns");
        let tampered = report
            .violations
            .iter()
            .find(|v| v.rule == "log_level_tampered")
            .expect("the one tampered tick must be detected");
        assert_eq!(tampered.count, 1, "only tick 0 was tampered");
        // Informational: must NOT gate the status.
        assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    }

    // Phase 3: persistent per-tick re-blinding. The re-assert repairs
    // the level before each tick, the game re-blinds within every
    // tick — every tick's warn is lost, and NO repair is possible.
    // Detection must fire on EVERY tampered tick (≈18), keeping the
    // report honest that log-crate capture was compromised while
    // `log_bridge_available` stays `Some(true)`.
    {
        let mut app = blind_every_tick_app();
        let report = bevy_playtest::run_scenario(&mut app, &scenario()).unwrap();

        assert_eq!(
            report.log_bridge_available,
            Some(true),
            "the bridge owns the slot; the level tampering is a separate signal"
        );
        let tampered = report
            .violations
            .iter()
            .find(|v| v.rule == "log_level_tampered")
            .expect("persistent re-blinding must be detected, not silently absorbed");
        assert!(
            tampered.count > 10,
            "every tampered tick must be counted (18 ticks in this scenario): {}",
            tampered.count
        );
        assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    }

    // Phase 4: setup-window pin for the RUN-START re-assert. A
    // plugin that warns from `finish()` emits between run start and
    // the first tick; a pre-run `Off` must already be undone by then,
    // or the event is dropped before the loop ever re-asserts.
    // Deleting the run-start assert makes this phase fail even with
    // the per-tick assert intact — the two defenses are not
    // interchangeable.
    {
        log::set_max_level(log::LevelFilter::Off);
        let mut app = App::new();
        app.add_plugins(FinishWarnPlugin);
        app.world_mut()
            .spawn(bevy_transform::prelude::Transform::from_xyz(0.0, 0.0, 0.0));
        let report = bevy_playtest::run_scenario(&mut app, &scenario()).unwrap();

        assert!(
            report.violations.iter().any(|v| v.rule == "log_warn"
                && v.detail
                    .contains("finish() must survive a foreign max-level Off")),
            "the setup window (Plugin::finish) needs the run-start re-assert: {report:?}"
        );
        assert!(
            !report
                .violations
                .iter()
                .any(|v| v.rule == "log_level_tampered"),
            "nothing re-blinds mid-run here: the pre-run Off is undone before tick 0: {:?}",
            report.violations
        );
    }
}
