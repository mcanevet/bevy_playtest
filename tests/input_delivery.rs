//! Input-delivery regression tests: pin that bot input actually
//! REACHES game-visible state (`ButtonInput<KeyCode>::just_pressed`)
//! under the plugin graph real games use.
//!
//! Root cause being guarded against: `run_scenario` used to register
//! `keyboard_input_system`/`mouse_button_input_system` in PreUpdate
//! unconditionally. Under `DefaultPlugins`, `InputPlugin` already
//! registers the same systems; the second copy's `clear()` wiped the
//! first copy's fresh `just_pressed` state before Update systems ran,
//! and its re-press was a no-op on keys already in `pressed`. One
//! subtlety made the bug easy to miss: release+repress batches in a
//! single frame self-heal (the re-press re-inserts `just_pressed`),
//! so DENSE input rates hide it — only ISOLATED presses (fresh press
//! on a not-currently-pressed key) are wiped. Games polling
//! `just_pressed` — the standard Bevy idiom — were deaf to the bot's
//! isolated presses while `pressed` still worked, so movement-heavy
//! bots looked healthy and every discrete-input verdict was vacuous.
//!
//! Two layers of coverage:
//! - in-process: an app mirroring the fixture graph, asserting the
//!   probe system observes transitions (fast, precise).
//! - end-to-end: `takeover_demo_input` — a real takeover run where the
//!   world only moves on `just_pressed`. If delivery breaks, the demo
//!   looks frozen and the run FAILS with frozen_world; if it works,
//!   the run passes. This is the exact shape of the original bug
//!   (loading_screen under takeover) and catches whatever in-process
//!   probes miss.
//!
//! Both tests were negative-controlled against the unfixed driver:
//! they fail there and pass with the fix. Both deliberately use a
//! SPARSE input rate (10 Hz at 60 tps → isolated presses every 6
//! frames, held 2) — denser rates produce release+repress batches
//! that self-heal and would pass on buggy code.
//!
//! A second delivery gap is covered below: pointer/picking-driven
//! games. bevy_picking's mouse backend reads the WRAPPED
//! `bevy_window::WindowEvent` messages (what winit writes), not the
//! raw `bevy_input` messages — so a bot writing only raw messages
//! left mines/game_menu-style games totally blind to cursor movement
//! and clicks. The mouse bots now write both forms. The picking
//! regression asserts on the full chain: wrapped events → picking's
//! `PointerInput` → `PointerOver`/`PointerClick` observers firing.

#![cfg(feature = "takeover")]

// Shared test harness (run-slot serialization)
#[path = "common/mod.rs"]
mod common;

use bevy::prelude::*;
use bevy_playtest::plugin::{exit_code, ENV_REPORT, ENV_SCENARIO};
use bevy_playtest::run_scenario;
use bevy_playtest::scenario::Scenario;
use bevy_transform::prelude::Transform;
use std::path::PathBuf;
use std::process::Command;

#[derive(Resource, Default, Debug)]
struct DeliveryProbe {
    just_pressed: u32,
    pressed_frames: u32,
    /// Frames where the key was held but not just_pressed — sanity
    /// that transitions do not stick.
    held_frames: u32,
}

fn delivery_probe(keys: Res<ButtonInput<KeyCode>>, mut probe: ResMut<DeliveryProbe>) {
    if keys.just_pressed(KeyCode::Digit1) {
        probe.just_pressed += 1;
    }
    if keys.pressed(KeyCode::Digit1) {
        probe.pressed_frames += 1;
        if !keys.just_pressed(KeyCode::Digit1) {
            probe.held_frames += 1;
        }
    }
}

#[test]
fn just_pressed_delivery_under_default_plugins_graph() {
    let _slot = common::run_slot();
    let scenario: Scenario = serde_json::from_str(
        r#"{"name":"delivery","bot":{"type":"chaos","seed":42,"raw_surface":{"keys":["Digit1"],"mouse_buttons":[]},"input_rate_hz":10},"duration_s":2.0,"tps":60,"invariants":[]}"#,
    )
    .expect("scenario parses");

    // Mirror the fixture graph: full DefaultPlugins minus WinitPlugin
    // (the graph PlaytestDefaultPlugins produces, and what all seven
    // showcase fixtures run). InputPlugin — and therefore its own
    // `keyboard_input_system` registration — is present, exactly like
    // a real game. input_rate_hz 10 at tps 60: presses arrive ISOLATED
    // (press every 6 frames, held 2) — the shape the duplicate-
    // registration bug wiped. Dense rates (30 Hz here) produce
    // release+repress batches that self-heal and hide it.
    let mut app = App::new();
    app.add_plugins(
        bevy::DefaultPlugins
            .build()
            .disable::<bevy_winit::WinitPlugin>(),
    );
    app.insert_resource(DeliveryProbe::default());
    app.add_systems(Update, delivery_probe);
    // Moving entity so an unrelated frozen_world failure cannot mask
    // the probe assertions (we assert on the probe, not the verdict).
    app.add_systems(Startup, |mut commands: Commands| {
        commands.spawn(Transform::from_xyz(0.0, 0.0, 0.0));
    });
    app.add_systems(Update, |mut q: Query<&mut Transform>| {
        for mut t in q.iter_mut() {
            t.translation.x += 0.05;
        }
    });

    let report = run_scenario(&mut app, &scenario).expect("run completes");

    let probe = app.world().resource::<DeliveryProbe>();
    assert!(
        probe.pressed_frames > 0,
        "bot key presses must reach ButtonInput::pressed (got {probe:?}); \
         inputs delivered: {}",
        report.metrics.input_count
    );
    assert!(
        probe.just_pressed > 0,
        "REGRESSION (just_pressed delivery): the bot delivered {} inputs \
         but no just_pressed transition ever reached Update systems — \
         games polling just_pressed are deaf to the bot (probe: {probe:?})",
        report.metrics.input_count
    );
    // Sanity: just_pressed is a transition detector, not a level.
    // Holding a key must NOT re-fire it every frame.
    assert!(
        probe.held_frames > 0,
        "just_pressed fired on every held frame (no clear between frames?) \
         — probe: {probe:?}"
    );
}

// --------------------------------------------------------------------------
// Picking delivery tests (wrappers)
// --------------------------------------------------------------------------

#[derive(Resource, Default, Debug)]
struct PickingProbe {
    pointer_over: u32,
    pointer_click: u32,
}

fn setup_picking_ui(mut commands: Commands) {
    // Fullscreen UI node: any in-window cursor position hovers it.
    // Layout fills the window regardless of its size.
    commands.spawn((Node {
        width: Val::Percent(100.0),
        height: Val::Percent(100.0),
        ..default()
    },));
    commands.spawn(Camera2d);
}

#[test]
fn wrapping_window_events_reach_picking_in_process() {
    let _slot = common::run_slot();
    let scenario: Scenario = serde_json::from_str(
        r#"{"name":"picking-probe","bot":{"type":"chaos","seed":42,"raw_surface":{"keys":[],"mouse_buttons":["Left","Right"]},"input_rate_hz":10},"duration_s":2.0,"tps":60,"invariants":[]}"#,
    )
    .expect("scenario parses");

    // Same graph as fixtures: DefaultPlugins minus WinitPlugin (what PlaytestDefaultPlugins produces).
    let mut app = App::new();
    app.add_plugins(
        bevy::DefaultPlugins
            .build()
            .disable::<bevy_winit::WinitPlugin>(),
    );
    app.insert_resource(PickingProbe::default());
    app.add_systems(Startup, setup_picking_ui);
    // Observer for PointerOver: fires whenever the cursor enters a selectable node.
    app.add_observer(
        |_trigger: On<PointerOver>, mut probe: ResMut<PickingProbe>| {
            probe.pointer_over += 1;
        },
    );
    // Observer for PointerClick: fires on any pointer click.
    app.add_observer(
        |_trigger: On<PointerClick>, mut probe: ResMut<PickingProbe>| {
            probe.pointer_click += 1;
        },
    );

    let report = run_scenario(&mut app, &scenario).expect("run completes");

    let probe = app.world().resource::<PickingProbe>();
    assert!(
        probe.pointer_over > 0,
        "WRAPPED WindowEvent regression: chaos mouse wrote {} inputs but picking's \
         PointerOver never fired (probe: {probe:?}). The bot may be emitting only raw \
         bevy_input messages — bevy_picking reads `WindowEvent::CursorMoved`.",
        report.metrics.input_count
    );
    assert!(
        probe.pointer_click > 0,
        "WRAPPED WindowEvent regression: chaos mouse wrote {} inputs but picking's \
         PointerClick never fired (probe: {probe:?}). Raw MouseButtonInput alone is \
         insufficient — bevy_picking consumes `WindowEvent::MouseButtonInput`.",
        report.metrics.input_count
    );
}

/// Scripted-click coverage: the chaos test above exercises the chaos
/// mouse path only. Scripted clicks (`bot.mouse_clicks`) go through a
/// DIFFERENT system (`scripted_mouse_system`) with its own wrapped
/// writes; dropping those wrappers would leave every other test green
/// while scripted menu clicks became invisible to picking again.
///
/// Isolation: a `synthetic_keyboard` bot with ONLY `mouse_clicks` (no
/// key_presses) and an empty chaos surface — `chaos_mouse_system` is
/// gated on `is_chaos_bot`, so the chaos path never runs here.
#[test]
fn scripted_clicks_reach_picking_in_process() {
    let _slot = common::run_slot();
    // Clicks land at the window centre (640, 360 of a 1280x720 default
    // window) — comfortably inside the fullscreen probe node.
    let scenario: Scenario = serde_json::from_str(
        r#"{"name":"scripted-picking","bot":{"type":"synthetic_keyboard","seed":1,"raw_surface":{"keys":[],"mouse_buttons":[]},"mouse_clicks":[{"frame":10,"button":"left","x":640.0,"y":360.0,"hold_frames":2},{"frame":40,"button":"left","x":600.0,"y":300.0,"hold_frames":2},{"frame":70,"button":"right","x":700.0,"y":400.0,"hold_frames":2}]},"duration_s":2.0,"tps":60,"invariants":[]}"#,
    )
    .expect("scenario parses");

    let mut app = App::new();
    app.add_plugins(
        bevy::DefaultPlugins
            .build()
            .disable::<bevy_winit::WinitPlugin>(),
    );
    app.insert_resource(PickingProbe::default());
    app.add_systems(Startup, setup_picking_ui);
    app.add_observer(
        |_trigger: On<PointerOver>, mut probe: ResMut<PickingProbe>| {
            probe.pointer_over += 1;
        },
    );
    app.add_observer(
        |_trigger: On<PointerClick>, mut probe: ResMut<PickingProbe>| {
            probe.pointer_click += 1;
        },
    );

    let report = run_scenario(&mut app, &scenario).expect("run completes");

    let probe = app.world().resource::<PickingProbe>();
    assert!(
        probe.pointer_over > 0,
        "SCRIPTED picking regression: {} scripted clicks delivered but picking's \
         PointerOver never fired (probe: {probe:?}) — scripted_mouse_system must \
         emit wrapped WindowEvent::CursorMoved, not just raw messages.",
        report.metrics.input_count
    );
    assert!(
        probe.pointer_click > 0,
        "SCRIPTED picking regression: {} scripted clicks delivered but picking's \
         PointerClick never fired (probe: {probe:?}) — scripted_mouse_system must \
         emit wrapped WindowEvent::MouseButtonInput.",
        report.metrics.input_count
    );
}

#[test]
fn picking_delivery_end_to_end() {
    let _slot = common::run_slot();
    let dir = std::env::temp_dir().join(format!(
        "bevy_playtest_picking_e2e_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();

    let scenario_path = dir.join("scenario.json");
    let report_path = dir.join("report.json");
    // The demo's world only moves when a PointerClick lands on its
    // fullscreen UI node. If wrapped-WindowEvent delivery is blind,
    // picking never fires, the world freezes (frozen_world fires,
    // exit 1); if it works, the run passes cleanly.
    std::fs::write(
        &scenario_path,
        r#"{"name":"picking-e2e","bot":{"type":"chaos","seed":42,"raw_surface":{"keys":[],"mouse_buttons":["Left"]},"input_rate_hz":10},"duration_s":3.0,"tps":60,"liveness":{"timeout_s":1.0}}"#,
    )
    .unwrap();

    let bin = PathBuf::from(env!("CARGO_BIN_EXE_takeover_demo_picking"));
    assert!(bin.is_file(), "demo binary missing: {bin:?}");
    let out = Command::new(&bin)
        .env(ENV_SCENARIO, scenario_path.to_str().unwrap())
        .env(ENV_REPORT, report_path.to_str().unwrap())
        .output()
        .expect("spawn takeover_demo_picking");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(exit_code::PASS),
        "picking-driven game under takeover must pass cleanly; a FAIL here means \
         the bot's mouse input is not reaching bevy_picking — check the report \
         for frozen_world (raw-only message forms are invisible to picking). \
         stdout: {stdout} stderr: {stderr}"
    );
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
    assert_eq!(report["status"], "pass", "{report}");
}

#[test]
fn just_pressed_delivery_end_to_end() {
    let _slot = common::run_slot();
    let dir = std::env::temp_dir().join(format!(
        "bevy_playtest_delivery_e2e_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();

    let scenario_path = dir.join("scenario.json");
    let report_path = dir.join("report.json");
    // The demo's world only moves when just_pressed(Digit1) fires. If
    // input delivery is blind, the world freezes (frozen_world fires,
    // exit 1); if it works, the run passes cleanly.
    // input_rate_hz 10 at tps 60: the bot presses every 6 frames but
    // holds only 2, so presses arrive ISOLATED — exactly the shape
    // the duplicate-registration bug ate (release+repress cycles
    // restore just_pressed; isolated presses don't). A denser rate
    // (e.g. 30 Hz here) makes every batch a release+repress, which
    // self-heals and vacuously passes.
    std::fs::write(
        &scenario_path,
        r#"{"name":"delivery-e2e","bot":{"type":"chaos","seed":42,"raw_surface":{"keys":["Digit1"],"mouse_buttons":[]},"input_rate_hz":10},"duration_s":3.0,"tps":60,"liveness":{"timeout_s":1.0}}"#,
    )
    .unwrap();

    let bin = PathBuf::from(env!("CARGO_BIN_EXE_takeover_demo_input"));
    assert!(bin.is_file(), "demo binary missing: {bin:?}");
    let out = Command::new(&bin)
        .env(ENV_SCENARIO, scenario_path.to_str().unwrap())
        .env(ENV_REPORT, report_path.to_str().unwrap())
        .output()
        .expect("spawn takeover_demo_input");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(exit_code::PASS),
        "just_pressed-driven game under takeover must pass cleanly; \
         a FAIL here means bot input is not reaching game systems \
         (check the report for frozen_world). stdout: {stdout} stderr: {stderr}"
    );
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
    assert_eq!(report["status"], "pass", "{report}");
}
