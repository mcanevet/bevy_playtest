//! Scan-purity regression tests: the static input-surface scan that
//! leaves `run_scenario` (and reaches the discovery flow's sidecar
//! export) must count only the GAME's systems — never the harness's
//! own bot and oracle instrumentation.
//!
//! Root cause being guarded against (Copilot r4469158382): the scan
//! used to run AFTER the scenario, on the live App. But by then
//! `run_scenario` had registered its bots: `BotKeyboardWriters`
//! WRITES `KeyboardInput` + the umbrella `WindowEvent`, and
//! `BotMouseWriters` writes `MouseButtonInput` + `WindowEvent`.
//! Writes count as observing (by design — a game that clears
//! key state is part of the input pipeline), and `WindowEvent`
//! attributes to the keyboard, button, motion AND wheel classes —
//! so every exported scan showed phantom readers on all mouse
//! channels in every game, and the wheel class could never be
//! pruned again.
//!
//! The fix captures the scan INSIDE `run_scenario` at harness-setup
//! time: after input-signal seeding (so a MinimalPlugins game's
//! seeded `ButtonInput<KeyCode>` readers still count) but BEFORE any
//! bot or oracle system is registered. The captured result is
//! parked OUTSIDE the world (a planted resource is an entity and
//! would shift the digest counts the discovery diff compares
//! between runs); the takeover drains and exports the parked scan.
//!
//! These tests are negative-controlled against the unfixed driver
//! by construction: they assert that the mouse and wheel channels
//! are EMPTY in a keyboard-only game — exactly the phantom-reader
//! signature the old post-run scan exhibited.

// Shared test harness (run-slot serialization)
#[path = "common/mod.rs"]
mod common;

#[cfg(feature = "takeover")]
mod in_process {
    use super::common;
    use bevy::app::App;
    use bevy::ecs::system::Res;
    use bevy::input::keyboard::KeyCode;
    use bevy::input::prelude::ButtonInput;
    use bevy_playtest::discovery::static_scan::{
        capture_static_scan, take_captured_scan, DeviceClass,
    };
    use bevy_playtest::run_scenario;
    use bevy_playtest::scenario::Scenario;
    use bevy_transform::prelude::Transform;

    /// Request the discovery-scan export for the duration of one
    /// `run_scenario` call. The capture is gated on this env var
    /// (ordinary playtests pay no scan cost), so the in-process
    /// purity tests must request it. The takeover EXPORT block that
    /// drains the parked scan lives in the child-process runner,
    /// NOT in in-process `run_scenario` — so here the parked scan
    /// survives the call and the tests drain it directly. Cleared
    /// afterwards — env is process-global and run_slot already
    /// serializes these tests.
    fn request_scan_export() {
        std::env::set_var(bevy_playtest::plugin::ENV_DISCOVER_SCAN, "/dev/null");
    }

    /// Remove the export request WITHOUT touching the parked scan —
    /// called between `run_scenario` and reading the parked result.
    fn clear_scan_export() {
        std::env::remove_var(bevy_playtest::plugin::ENV_DISCOVER_SCAN);
    }

    /// Test-start reset: env cleared AND the parking slot drained, so
    /// one test's capture can never leak into the next test's
    /// assertions. (run_slot serializes these tests, so no other
    /// thread can race the drain.)
    fn reset_scan_state() {
        clear_scan_export();
        let _ = take_captured_scan();
    }

    /// A keyboard-only game: one system reading
    /// `ButtonInput<KeyCode>`, a moving entity for oracle
    /// observability. No mouse, wheel, motion, or gamepad readers —
    /// deliberately purpose-built (NOT the takeover demo, whose
    /// DefaultPlugins graph carries picking-side `WindowEvent`
    /// readers that would muddle exact counts).
    fn keyboard_only_app() -> App {
        let mut app = App::new();
        app.add_systems(bevy::app::Update, |keys: Res<ButtonInput<KeyCode>>| {
            let _ = keys.any_pressed([KeyCode::KeyW, KeyCode::Digit1]);
        });
        app.add_systems(
            bevy::app::Startup,
            |mut commands: bevy::ecs::prelude::Commands| {
                commands.spawn(Transform::from_xyz(0.0, 0.0, 0.0));
            },
        );
        app.add_systems(
            bevy::app::Update,
            |mut q: bevy::ecs::prelude::Query<&mut Transform>| {
                for mut t in q.iter_mut() {
                    t.translation.x += 0.05;
                }
            },
        );
        app
    }

    fn short_chaos_scenario() -> Scenario {
        serde_json::from_str(
            r#"{"name":"scan-purity","bot":{"type":"chaos","seed":42},"duration_s":0.2,"tps":60}"#,
        )
        .expect("scenario parses")
    }

    /// Without a scan-export request, `run_scenario` must NOT plant
    /// the capture at all — this is the cost gate r4470755368 asked
    /// for: ordinary playtests keep their previous world setup and
    /// pay no schedule-initialization/scan cost.
    #[test]
    fn run_scenario_without_request_plants_no_capture() {
        let _slot = common::run_slot();
        reset_scan_state();
        let mut app = keyboard_only_app();
        let scenario = short_chaos_scenario();
        run_scenario(&mut app, &scenario).expect("run completes");
        assert!(
            take_captured_scan().is_none(),
            "capture must stay off the ordinary (non-discovery) playtest path"
        );
    }

    /// The scan captured by `run_scenario` must contain only the
    /// game's own input readers.
    #[test]
    fn run_scenario_captures_only_game_readers() {
        let _slot = common::run_slot();
        reset_scan_state();
        request_scan_export();
        let mut app = keyboard_only_app();
        let scenario = short_chaos_scenario();
        run_scenario(&mut app, &scenario).expect("run completes");
        clear_scan_export();
        let scan = take_captured_scan()
            .expect("run_scenario must park the captured scan when export was requested");
        assert!(
            scan.reader_count(DeviceClass::Keyboard) >= 1,
            "the keyboard-only game's reader must be counted: {:?}",
            scan.readers
        );
        // THE purity pin: the harness's bots write MouseButtonInput
        // and the umbrella WindowEvent (which attributes to keyboard,
        // button, motion AND wheel), so a post-run scan shows nonzero
        // on every mouse channel. The captured scan must not.
        //
        // Expected exact counts on this bare app: MouseButton 1 and
        // Keyboard 2 are the `!InputPlugin` FALLBACK translators the
        // harness registers in PreUpdate — engine-named systems
        // (`keyboard_input_system`, `mouse_button_input_system`) that
        // a DefaultPlugins/InputPlugin graph carries too (there the
        // engine registers the identical systems itself). Counting
        // them is the deliberate parity decision; they are engine
        // plumbing, not harness instrumentation. Everything the BOTS
        // write must stay invisible:
        assert_eq!(
            scan.reader_count(DeviceClass::MouseMotion),
            0,
            "harness wheel/window writers leaked into the captured scan: {:?}",
            scan.readers
        );
        assert_eq!(
            scan.reader_count(DeviceClass::MouseWheel),
            0,
            "harness wheel/window writers leaked into the captured scan: {:?}",
            scan.readers
        );
        assert_eq!(
            scan.reader_count(DeviceClass::Gamepad),
            0,
            "harness writers leaked into the captured scan: {:?}",
            scan.readers
        );
        // The run loop ran: verify the capture was taken BEFORE the
        // loop by checking the bot systems really got registered
        // (live world now contains them) — the scan must not have.
        let live = bevy_playtest::discovery::static_scan::static_device_readers(&app);
        assert!(
            live.reader_count(DeviceClass::MouseWheel) >= 1,
            "sanity: the live (post-install) world does contain harness writers"
        );
        assert!(
            scan.complete,
            "capture on a cold fully-initializable app must be complete"
        );
    }

    /// The capture must happen AFTER seeding (a MinimalPlugins-style
    /// game never registers `ButtonInput<KeyCode>` itself; the
    /// harness's `init_resource` must precede the capture or the
    /// keyboard reader would be under-counted — fail-closed).
    #[test]
    fn capture_sees_harness_seeded_resources() {
        let _slot = common::run_slot();
        reset_scan_state();
        request_scan_export();
        // Do NOT init ButtonInput<KeyCode> ourselves: only the
        // harness's seeding brings that resource's existence.
        let mut app = App::new();
        app.add_systems(bevy::app::Update, |keys: Res<ButtonInput<KeyCode>>| {
            let _ = keys.any_pressed([KeyCode::Space]);
        });
        app.add_systems(
            bevy::app::Startup,
            |mut commands: bevy::ecs::prelude::Commands| {
                commands.spawn(Transform::from_xyz(0.0, 0.0, 0.0));
            },
        );
        app.add_systems(
            bevy::app::Update,
            |mut q: bevy::ecs::prelude::Query<&mut Transform>| {
                for mut t in q.iter_mut() {
                    t.translation.x += 0.05;
                }
            },
        );
        let scenario = short_chaos_scenario();
        run_scenario(&mut app, &scenario).expect("run completes");
        clear_scan_export();
        let scan = take_captured_scan().expect("captured scan must exist");
        assert!(
            scan.reader_count(DeviceClass::Keyboard) >= 1,
            "seeding must precede the capture: the game's ButtonInput reader \
             must still count on a MinimalPlugins-style app: {:?}",
            scan.readers
        );
    }

    /// The capture itself (unit-level, without `run_scenario`) must
    /// return capture-time semantics on a cold app — and must NOT
    /// touch the world structurally: every resource is an ENTITY in
    /// bevy 0.20, so a planted result resource shifts exactly the
    /// counts the digest feeds on (`count_spawned` + archetype
    /// count), and the scan-exporting control run would diverge
    /// from every other run at frame 0 — poisoning the double-
    /// control determinism gate and every probe differential.
    #[test]
    fn capture_on_cold_app_counts_keyboard_only() {
        // Serialize with the run_scenario tests even though this test
        // never calls run_scenario: `capture_static_scan` writes the
        // process-global CAPTURED_SCAN parking slot, so a concurrent
        // purity test mid-`run_scenario` could have its parked scan
        // overwritten (or consumed) before its own assertions read it.
        let _slot = common::run_slot();
        let mut app = keyboard_only_app();
        let spawned_before = app.world().entities().count_spawned();
        let archetypes_before = app.world().archetypes().len();
        let scan = capture_static_scan(&mut app);
        assert_eq!(
            app.world().entities().count_spawned(),
            spawned_before,
            "capture must not touch the world's spawned-entity count \
             (the digest hashes it)"
        );
        assert_eq!(
            app.world().archetypes().len(),
            archetypes_before,
            "capture must not touch the world's archetype count (the \
             digest hashes it)"
        );
        assert!(scan.complete);
        assert_eq!(scan.reader_count(DeviceClass::Keyboard), 1);
        assert_eq!(scan.reader_count(DeviceClass::MouseWheel), 0);
        assert_eq!(scan.reader_count(DeviceClass::MouseButton), 0);
        // The parked copy is what the takeover exports.
        let parked = take_captured_scan().expect("capture must park the scan");
        assert_eq!(parked, scan);
    }
}
