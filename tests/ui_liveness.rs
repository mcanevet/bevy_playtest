//! UI liveness oracle coverage: pure-UI games (Bevy 0.20 nodes carry
//! `UiTransform`, not `Transform`) must count as ALIVE when responding.
//! Previously the frozen_world oracle watched only `Changed<Transform>`,
//! so any UI-only game reported a false `frozen_world` violation even
//! while actively handling clicks/animation.
//!
//! Under the `ui-liveness` feature (default-on via takeover), the
//! `ui_liveness_system` watches `Changed<UiTransform>` and feeds activity
//! into the same liveness evidence, eliminating the coverage gap.
//!
//! These tests exercise the oracle wiring directly (spawn/mutate
//! `UiTransform`, no full UI stack): the realistic layout-system path is
//! validated end-to-end by the mines/game_menu fixture battery.
//!
//! Note: These tests use `bevy_app::App` + `bevy_ui::UiTransform` directly,
//! avoiding the `bevy` meta-crate. This allows testing the `in-process,
//! ui-liveness` combination where the full `bevy` crate is not available.

#[path = "common/mod.rs"]
mod common;

#[cfg(feature = "ui-liveness")]
mod ui_tests {
    use super::common;
    use bevy_app::prelude::*;
    use bevy_app::NoopPluginGroup;
    use bevy_ecs::prelude::*;
    use bevy_playtest::{
        enums::PlaytestStatus, run_scenario, scenario::Scenario, state::PlaytestReport,
    };
    use bevy_ui::prelude::*;

    fn passive_scenario() -> Scenario {
        serde_json::from_str(
            r#"{"name":"ui-liveness","bot":{"type":"chaos","seed":1,"raw_surface":{"keys":["Escape"],"mouse_buttons":[]}},"duration_s":4.0,"tps":60,"invariants":[]}"#,
        )
        .expect("scenario parses")
    }

    fn frozen_fired(report: &PlaytestReport) -> bool {
        report.violations.iter().any(|v| v.rule.contains("frozen"))
    }

    fn vacuous_fired(report: &PlaytestReport) -> bool {
        report.violations.iter().any(|v| v.rule.contains("vacuous"))
    }

    /// Nudge every UiTransform's translation one pixel per frame —
    /// pure-UI "animation" without touching any `Transform`.
    fn nudge_ui_transforms(mut query: Query<&mut UiTransform>) {
        for mut transform in &mut query {
            if let Val::Px(v) = transform.translation.x {
                transform.translation.x = Val::Px(v + 1.0);
            }
        }
    }

    /// Positive case: a pure-UI world that changes every frame must NOT
    /// trigger frozen_world. The UI liveness watcher sees
    /// `Changed<UiTransform>` and resets the frozen counter.
    #[test]
    fn ui_changes_prevent_frozen_world() {
        let _slot = common::run_slot();
        let scenario = passive_scenario();

        let mut app = App::new();
        app.add_plugins(NoopPluginGroup);
        app.add_systems(Startup, |mut commands: Commands| {
            commands.spawn(UiTransform::default());
        });
        app.add_systems(Update, nudge_ui_transforms);

        let report = run_scenario(&mut app, &scenario).expect("run completes");
        assert_ne!(report.status, PlaytestStatus::Crash, "run crashed");

        assert!(
            !frozen_fired(&report),
            "pure-UI animated world falsely reported frozen_world; the UI \
             liveness watcher should see Changed<UiTransform>: {report:?}"
        );
        assert!(
            !vacuous_fired(&report),
            "pure-UI world must not be vacuous — UiTransform entities are \
             observable world state: {report:?}"
        );
    }

    /// Negative control: a static pure-UI world MUST still fire
    /// frozen_world. This proves the oracle was not just disabled.
    #[test]
    fn static_ui_triggers_frozen_world() {
        let _slot = common::run_slot();
        let scenario = passive_scenario();

        let mut app = App::new();
        app.add_plugins(NoopPluginGroup);
        app.add_systems(Startup, |mut commands: Commands| {
            commands.spawn(UiTransform::default());
        });

        let report = run_scenario(&mut app, &scenario).expect("run completes");
        assert_ne!(report.status, PlaytestStatus::Crash, "run crashed");

        assert!(
            frozen_fired(&report),
            "static UI world should trigger frozen_world but didn't: {report:?}"
        );
        assert!(
            !vacuous_fired(&report),
            "pure-UI world must not be vacuous — UiTransform entities are \
             observable world state: {report:?}"
        );
    }

    /// Edge case: world with both Transform and UI entities. UI activity
    /// alone must keep the world alive (OR logic between the watchers).
    #[test]
    fn mixed_world_ui_activity_keeps_alive() {
        let _slot = common::run_slot();
        let scenario = passive_scenario();

        let mut app = App::new();
        app.add_plugins(NoopPluginGroup);
        app.add_systems(Startup, |mut commands: Commands| {
            commands.spawn(bevy_transform::prelude::Transform::default());
            commands.spawn(UiTransform::default());
        });
        app.add_systems(Update, nudge_ui_transforms);

        let report = run_scenario(&mut app, &scenario).expect("run completes");
        assert_ne!(report.status, PlaytestStatus::Crash, "run crashed");

        assert!(
            !frozen_fired(&report),
            "mixed world with UI activity falsely reported frozen_world: {report:?}"
        );
    }
}
