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

#![cfg(feature = "takeover")]

// Shared test harness (run-slot serialization)
#[path = "common/mod.rs"]
mod common;

use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_input::prelude::*;
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
