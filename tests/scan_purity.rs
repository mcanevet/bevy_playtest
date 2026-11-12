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
            !scan.complete,
            "run_scenario's capture is BY CONSTRUCTION pre-first-update, \
             so runtime-registered readers are invisible and the scan \
             must never claim completeness"
        );
        assert!(
            scan.notes
                .iter()
                .any(|n| n.contains("scan was taken before the world's first update")),
            "pre-update blindness must be surfaced as a note, got {:?}",
            scan.notes
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
        assert!(
            scan.notes.iter().any(|n| n.contains(
                "scan was taken before the world's first update: runtime-added readers \
                 (systems/observers registered from inside a system) are invisible"
            )),
            "pre-update blindness must be surfaced as a note, got {:?}",
            scan.notes
        );
        assert!(!scan.complete);
        assert_eq!(scan.reader_count(DeviceClass::Keyboard), 1);
        assert_eq!(scan.reader_count(DeviceClass::MouseWheel), 0);
        assert_eq!(scan.reader_count(DeviceClass::MouseButton), 0);
        // The parked copy is what the takeover exports.
        let parked = take_captured_scan().expect("capture must park the scan");
        assert_eq!(parked, scan);
    }

    /// PRODUCTION-COMPOSITION guard: the census and the scan
    /// capture must coexist in one `run_scenario` process without
    /// either blinding the other. The census force-initializes
    /// schedules, and `Schedule::initialize` BUILDS a changed graph
    /// — which moves run conditions out of `schedule.graph()` into
    /// executor storage (`pub(super)` in bevy_ecs 0.20.1). When the
    /// census ran ABOVE the capture block, both condition harvests
    /// (`condition_hidden_classes` and
    /// `collect_condition_blindness`) read EMPTY condition lists in
    /// every scan-exporting run: a channel whose only consumer is a
    /// system-set-level `run_if` was flagged dead with
    /// `complete=true`, and the scan's run-condition blindness note
    /// vanished. This test replays that composition through the
    /// real `run_scenario` path — RED on the unfixed ordering,
    /// green on capture-before-census.
    #[test]
    fn census_running_first_must_not_poison_condition_harvests() {
        use bevy_ecs::message::{Message, MessageWriter};
        use bevy_ecs::prelude::{IntoScheduleConfigs, Res, ResMut, Resource, SystemSet};
        use bevy_playtest::discovery::suggestions::SuggestionKind;

        #[derive(Message)]
        struct SetGatedMessage;

        #[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone)]
        struct GatedSet;

        // A run condition READING the channel, attached to the SET:
        // the exact shape the poisoned harvest would miss.
        fn set_condition_reads_channel(
            msgs: Res<bevy_ecs::message::Messages<SetGatedMessage>>,
        ) -> bool {
            !msgs.is_empty()
        }

        // A member of the gated set (the set condition must apply to
        // member systems, so at least one member must exist).
        #[derive(Resource, Default)]
        struct Marker {
            touched: u32,
        }

        // A named member system of the gated set: its own body
        // touches nothing the harvest cares about — the set
        // condition is the only channel consumer to find.
        fn gated_member(mut marker: ResMut<Marker>) {
            marker.touched += 1;
        }

        let _slot = common::run_slot();
        reset_scan_state();
        request_scan_export();
        let mut app = App::new();
        app.add_message::<SetGatedMessage>()
            .init_resource::<Marker>()
            .add_systems(
                bevy::app::Update,
                |mut msgs: MessageWriter<SetGatedMessage>| {
                    msgs.write(SetGatedMessage);
                },
            )
            .add_systems(bevy::app::Update, gated_member.in_set(GatedSet))
            .configure_sets(
                bevy::app::Update,
                GatedSet.run_if(set_condition_reads_channel),
            );
        let scenario = short_chaos_scenario();
        run_scenario(&mut app, &scenario).expect("run completes");
        clear_scan_export();
        let scan = take_captured_scan().expect("captured scan must exist");

        // The suggestions section must have seen the set condition.
        let suggestions = scan
            .suggestions
            .expect("the capture always collects the suggestions section");
        // `Suggestion.target` carries the FULL type path (module
        // chain + type name), so compare by suffix — an equality
        // check against the bare type name can never match and
        // would leave this row-half of the regression pin inert.
        assert!(
            !suggestions
                .rows
                .iter()
                .any(|r| r.kind == SuggestionKind::DeadMessageChannel
                    && r.target.ends_with("::SetGatedMessage")),
            "a set-condition-held read means the channel is consumed — \
             the census poisoned the harvest (empty condition lists): {:?}",
            suggestions.rows
        );

        // The scan-side harvest must survive the census too: the
        // driver-built schedules must still report the
        // run-condition blindness class (belt) and the suggestions
        // must never claim completeness on a blinded walk.
        assert!(
            suggestions
                .notes
                .iter()
                .any(|n| n.contains("run condition")),
            "the demotion is explained, not silent: {:?}",
            suggestions.notes
        );
    }

    /// DRIVER-LEVEL census pin (composition the pure-module tests
    /// cannot see): after a real `run_scenario`, the report must
    /// carry the census (`Some`, never dropped on the floor), and
    /// the `game_systems` count must be EXACT for the game — the
    /// harness registers nine of its own bot/oracle systems into
    /// `Update` AFTER the census point, plus the engine's
    /// `run_main`/`message_update_system` drivers; none may appear
    /// in the count. An exact-match assertion is the strongest pin:
    /// any of (a) the census drifting below bot/oracle
    /// registration, (b) the ownership filter weakening so
    /// `bevy_playtest`-pathed systems count as game, (c) a plain
    /// counting regression — trips it. (Precise today by path:
    /// harness systems live under `bevy_playtest::`, which the
    /// `bevy_` engine prefix already excludes — but this test is
    /// what makes that ACCIDENT durable as design.)
    #[test]
    fn report_carries_a_census_that_counts_only_game_systems() {
        use bevy_playtest::discovery::census::CapabilityKind;

        let _slot = common::run_slot();
        reset_scan_state();
        let mut app = keyboard_only_app();
        let scenario = short_chaos_scenario();
        let report = run_scenario(&mut app, &scenario).expect("run completes");

        let census = report
            .capability_census
            .as_ref()
            .expect("run_scenario must attach the census to every report");
        assert!(
            census.rows().len() == 5,
            "the census must carry all five capability rows, got {:?}",
            census.rows()
        );

        // keyboard_only_app registers exactly three game systems
        // (two in Update, one in Startup); the census must agree
        // to the digit — bot systems, oracles and engine drivers
        // excluded.
        let row = census
            .row(CapabilityKind::GameSystems)
            .expect("the GameSystems row must exist");
        assert_eq!(
            row.count, 3,
            "game_systems must count exactly the game's scheduled systems: \
             harness bots/oracles registered after the census point and \
             engine drivers must be excluded: {census:#?}"
        );
    }
}
