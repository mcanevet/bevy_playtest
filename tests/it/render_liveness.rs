//! render_liveness rule tests: the informational pin (a regression
//! in `is_informational` would turn every fixture battery run red
//! the moment a fixture legitimately lacks a camera — this is the
//! one-line fuse that keeps the rule prose-only), the registry
//! surface, and end-to-end driver behavior.
//!
//! End-to-end cases build in-process apps WITHOUT bevy_render's
//! plugin graph on purpose for the negative pins (absence of the
//! sub-app ⇒ "not evaluated", field absent), and WITH a manually
//! inserted fake render sub-app for the positive cases — the
//! presence gate probes the LABEL, so a `SubApp` registered under
//! `bevy_render::RenderApp` is indistinguishable from a real
//! renderer-initialized one (that initialization is `RenderPlugin`
//! plus GPU availability; bevy's own `RenderApp` presence test does
//! the same). Camera findings need no functioning GPU.

// Shared test harness (run-slot serialization)
use crate::common;

use bevy_app::prelude::*;
use bevy_camera::{Camera, CameraOutputMode};
use bevy_ecs::prelude::*;
use bevy_playtest::enums::PlaytestStatus;
use bevy_playtest::rules::Rule;
use bevy_playtest::run_scenario;

fn scenario() -> bevy_playtest::scenario::Scenario {
    serde_json::from_str(r#"{"bot":{"type":"chaos","seed":7},"duration_s":0.2,"tps":60}"#).unwrap()
}

// ---------------------------------------------------------------------------
// Registry pins
// ---------------------------------------------------------------------------

/// LOAD-BEARING: `render_liveness` must be informational. A missed
/// arm in `is_informational` flips every fixture-battery run where
/// the fixture lacks/holds cameras into a gating failure — prose
/// only, forever.
#[test]
fn render_liveness_rule_is_informational() {
    assert!(
        Rule::from_name("render_liveness").is_some_and(Rule::is_informational),
        "render_liveness MUST be informational (is_informational arm missing?)"
    );
}

#[test]
fn render_liveness_name_and_meaning_are_registered() {
    let rule = Rule::from_name("render_liveness").expect("name registered");
    assert!(Rule::ALL.contains(&rule));
    assert!(rule.meaning().contains("pixels"));
}

// ---------------------------------------------------------------------------
// Driver end-to-end
// ---------------------------------------------------------------------------

/// Inserts a sub-app under `bevy_render::RenderApp` so the
/// attachment gate passes without initializing an actual renderer.
fn fake_render_app() -> App {
    let mut app = App::new();
    app.insert_sub_app(bevy_render::RenderApp, bevy_app::SubApp::default());
    app
}

/// A sim-active game (transforms move every frame) with NO camera
/// and a render sub-app present: expect an informational
/// `render_liveness` violation for `no_cameras`, status still Pass.
#[test]
fn no_camera_game_reports_informational_finding_but_passes() {
    let _slot = common::run_slot();

    let mut app = fake_render_app();
    bevy_playtest::sinks::ensure_global_subscriber();
    app.add_systems(
        Update,
        |mut q: Query<&mut bevy_transform::prelude::Transform>| {
            for mut t in q.iter_mut() {
                t.translation.x += 1.0;
            }
        },
    );
    app.world_mut()
        .spawn(bevy_transform::prelude::Transform::default());

    let report = run_scenario(&mut app, &scenario()).expect("run completes");

    // Informational: never gates.
    assert_eq!(report.status, PlaytestStatus::Pass);
    let rl = report
        .render_liveness
        .as_ref()
        .expect("render sub-app present ⇒ field attached");
    assert_eq!(
        rl.tier,
        bevy_playtest::discovery::render_liveness::RenderLivenessTier::NoCameras
    );
    let entry = report
        .violations
        .iter()
        .find(|v| v.rule == "render_liveness")
        .expect("informational violation rides the violations list");
    assert!(entry.detail.contains("no camera ever existed"));
    // The detail quotes the REAL sample count, not a notes-length
    // stand-in (regression pin: notes fire on camera-count changes
    // only, so a no-camera run has exactly 1 note regardless of
    // how many samples happened).
    let rl = report.render_liveness.as_ref().unwrap();
    let unit = if rl.samples.max(1) == 1 {
        "sample"
    } else {
        "samples"
    };
    assert!(
        entry
            .detail
            .contains(&format!("({} {})", rl.samples.max(1), unit)),
        "detail must quote the tracker's real sample count: {}",
        entry.detail
    );
}

/// No render sub-app (the honest headless case): the field is
/// absent and NO violation fires even with a sim-active, cameraless
/// game — "not evaluated", never accused.
#[test]
fn no_render_sub_app_means_not_evaluated() {
    let _slot = common::run_slot();

    let mut app = App::new();
    bevy_playtest::sinks::ensure_global_subscriber();
    app.add_systems(
        Update,
        |mut q: Query<&mut bevy_transform::prelude::Transform>| {
            for mut t in q.iter_mut() {
                t.translation.x += 1.0;
            }
        },
    );
    app.world_mut()
        .spawn(bevy_transform::prelude::Transform::default());

    let report = run_scenario(&mut app, &scenario()).expect("run completes");
    assert_eq!(report.status, PlaytestStatus::Pass);
    assert!(
        report.render_liveness.is_none(),
        "no render sub-app ⇒ field must stay absent (not evaluated)"
    );
    assert!(!report
        .violations
        .iter()
        .any(|v| v.rule == "render_liveness"));
}

/// Post-cleanup takeover regression pin (the exact shape of a
/// finished `DefaultPlugins` app under takeover): bevy 0.20's
/// `PipelinedRenderingPlugin::cleanup` ships the `RenderApp`
/// sub-app off to the render thread and REMOVES it from the
/// sub-app table, leaving only the `RenderExtractApp` label. The
/// gate must recognize that survivor as "a renderer was
/// initialized" — otherwise every real takeover run silently drops
/// to "not evaluated". Non-wasm only, mirroring upstream's cfg on
/// `pipelined_rendering`.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn pipelined_cleanup_survivor_still_attaches() {
    let _slot = common::run_slot();

    let mut app = App::new();
    app.insert_sub_app(
        bevy_render::pipelined_rendering::RenderExtractApp,
        bevy_app::SubApp::default(),
    );
    bevy_playtest::sinks::ensure_global_subscriber();
    app.add_systems(
        Update,
        |mut q: Query<&mut bevy_transform::prelude::Transform>| {
            for mut t in q.iter_mut() {
                t.translation.x += 1.0;
            }
        },
    );
    app.world_mut()
        .spawn(bevy_transform::prelude::Transform::default())
        .insert(Camera::default());

    let report = run_scenario(&mut app, &scenario()).expect("run completes");
    assert_eq!(report.status, PlaytestStatus::Pass);
    let rl = report
        .render_liveness
        .as_ref()
        .expect("RenderExtractApp survivor ⇒ field attached (post-cleanup takeover shape)");
    assert_eq!(
        rl.tier,
        bevy_playtest::discovery::render_liveness::RenderLivenessTier::Live
    );
}

/// An active, writing camera: live tier, no violation, field
/// attached with the camera counted.
#[test]
fn live_camera_game_is_clean() {
    let _slot = common::run_slot();

    let mut app = fake_render_app();
    bevy_playtest::sinks::ensure_global_subscriber();
    app.add_systems(
        Update,
        |mut q: Query<&mut bevy_transform::prelude::Transform>| {
            for mut t in q.iter_mut() {
                t.translation.x += 1.0;
            }
        },
    );
    app.world_mut()
        .spawn(bevy_transform::prelude::Transform::default())
        .insert(Camera::default());

    let report = run_scenario(&mut app, &scenario()).expect("run completes");
    assert_eq!(report.status, PlaytestStatus::Pass);
    let rl = report
        .render_liveness
        .as_ref()
        .expect("render sub-app present ⇒ field attached");
    assert_eq!(
        rl.tier,
        bevy_playtest::discovery::render_liveness::RenderLivenessTier::Live
    );
    assert_eq!(rl.cameras_seen, 1);
    assert!(!report
        .violations
        .iter()
        .any(|v| v.rule == "render_liveness"));
}

/// A sole camera permanently on `CameraOutputMode::Skip`: finding
/// fires with the "may be intentional" caveat, still informational.
#[test]
fn sole_skip_camera_fires_with_caveat() {
    let _slot = common::run_slot();

    let mut app = fake_render_app();
    bevy_playtest::sinks::ensure_global_subscriber();
    app.add_systems(
        Update,
        |mut q: Query<&mut bevy_transform::prelude::Transform>| {
            for mut t in q.iter_mut() {
                t.translation.x += 1.0;
            }
        },
    );
    let camera = Camera {
        output_mode: CameraOutputMode::Skip,
        ..Default::default()
    };
    app.world_mut()
        .spawn(bevy_transform::prelude::Transform::default())
        .insert(camera);

    let report = run_scenario(&mut app, &scenario()).expect("run completes");
    assert_eq!(report.status, PlaytestStatus::Pass);
    let rl = report
        .render_liveness
        .as_ref()
        .expect("render sub-app present ⇒ field attached");
    assert_eq!(
        rl.tier,
        bevy_playtest::discovery::render_liveness::RenderLivenessTier::AllCamerasSkip
    );
    let entry = report
        .violations
        .iter()
        .find(|v| v.rule == "render_liveness")
        .expect("skip finding fires");
    assert!(entry.detail.contains("may be intentional"));
}
