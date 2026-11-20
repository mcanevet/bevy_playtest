//! screen_liveness rule tests: registry pins, negative driver cases
//! (feature gate boundaries — field absent / not-evaluated tiers),
//! and GPU-positive end-to-end cases driving the REAL renderer
//! headlessly (the T2 capture spike's proven app shape:
//! DefaultPlugins − WinitPlugin − LogPlugin, no primary window,
//! synchronous pipeline compilation).
//!
//! The capture rig is exercised through `run_scenario` only — the
//! driver spawns the probe camera + requests; the assertions read
//! the report's `screen_liveness` field and the informational
//! violations entry.
//!
//! GPU-positive cases are skipped on demand via
//! `BEVY_PLAYTEST_TEST_SKIP_GPU` (whole suite) — and the pixel
//! CONTENT pins additionally self-skip on SOFTWARE adapters
//! (`DeviceType::Cpu`, e.g. CI's lavapipe), whose readback is
//! content-unfaithful in both directions: observed inversions are
//! tier inversions, not game findings. Bookkeeping-only GPU cases
//! (tier refusal, not_evaluated_3d) still run on software adapters
//! — they assert structure, not pixels.

// run_scenario is exclusive per process (shared capture state).
use crate::common;

use bevy::app::PluginsState;
use bevy::prelude::*;
use bevy::render::RenderPlugin;
use bevy_playtest::discovery::screen_liveness::ScreenLivenessTier;
use bevy_playtest::enums::PlaytestStatus;
use bevy_playtest::rules::Rule;
use bevy_playtest::run_scenario;

/// Short run: 0.5 s at 60 tps = 30 ticks → cadence interval 30 →
/// requests at tick 0 and tick 30 (boundary), captures land 1–2
/// ticks later. Enough for one hash pair (the tier derivation needs
/// >= 2 captures).
fn scenario() -> bevy_playtest::scenario::Scenario {
    serde_json::from_str(r#"{"bot":{"type":"chaos","seed":7},"duration_s":0.5,"tps":60}"#).unwrap()
}

/// Longer run: 1.2 s → several capture pairs (moving-scene drift
/// and freeze-span evidence both need headroom).
fn long_scenario() -> bevy_playtest::scenario::Scenario {
    serde_json::from_str(r#"{"bot":{"type":"chaos","seed":7},"duration_s":1.2,"tps":60}"#).unwrap()
}

fn gpu_tests_skipped() -> bool {
    std::env::var_os("BEVY_PLAYTEST_TEST_SKIP_GPU").is_some()
}

/// Whether the RENDER adapter backing this app is a SOFTWARE
/// rasterizer (`DeviceType::Cpu` — lavapipe on CI). Observed
/// empirically: its readback content is unfaithful in both
/// directions (a moving 2D scene hashed IDENTICAL across every
/// capture pair; a static one DRIFTED) — so pixel-content pins
/// (live-vs-frozen tiers) self-skip there.
///
/// `RenderAdapterInfo` only reaches the MAIN world during plugin
/// FINISH (`RenderResources::unpack_into`, driven by
/// `RenderPlugin::finish`) — plugin `build` stashes the adapter
/// query in the non-public `FutureRenderResources`. Normally the
/// driver finishes plugins inside `run_scenario` (after every
/// check site here), so callers must first run `warm_up_plugins`
/// (the driver's own finish sequence verbatim) to make the
/// resource observable. An absent resource after that warm-up is
/// a STRUCTURAL break (this function's whole basis collapsed) and
/// panics loudly — the skip can never silently mask a vacuous
/// pass the way a truthy fallback would.
fn software_adapter(app: &App) -> bool {
    let info = app
        .world()
        .get_resource::<bevy::render::renderer::RenderAdapterInfo>()
        .unwrap_or_else(|| {
            panic!(
                "RenderAdapterInfo absent from the main world after \
                 plugin warm-up: the software-adapter skip check is \
                 broken — refusing to guess"
            )
        });
    info.device_type == wgpu_types::DeviceType::Cpu
}

/// Finish plugins exactly the way the driver (and `App::run`)
/// would, so renderer resources (in particular
/// `RenderAdapterInfo`) become visible in the main world BEFORE
/// `run_scenario`. Idempotent: the driver's finish gate in
/// `run_scenario` is `plugins_state() == Ready`, which is false
/// once we finish here, so its own finish becomes a no-op —
/// nothing runs twice.
fn warm_up_plugins(app: &mut App) {
    while matches!(app.plugins_state(), PluginsState::Adding) {
        bevy::tasks::tick_global_task_pools_on_main_thread();
    }
    if matches!(app.plugins_state(), PluginsState::Ready) {
        app.finish();
        app.cleanup();
    }
}

// ---------------------------------------------------------------------------
// Registry pins
// ---------------------------------------------------------------------------

/// LOAD-BEARING: `screen_liveness` must be informational. A missed
/// arm in `is_informational` would gate every legitimately-static
/// screen (menus, dialogue) into a failure — prose only, forever.
#[test]
fn screen_liveness_rule_is_informational() {
    assert!(
        Rule::from_name("screen_liveness").is_some_and(Rule::is_informational),
        "screen_liveness MUST be informational (is_informational arm missing?)"
    );
}

#[test]
fn screen_liveness_name_and_meaning_are_registered() {
    let rule = Rule::from_name("screen_liveness").expect("name registered");
    assert!(Rule::ALL.contains(&rule));
    assert!(rule.meaning().contains("screen"));
}

// ---------------------------------------------------------------------------
// Negative driver pins (no GPU needed)
// ---------------------------------------------------------------------------

/// No render sub-app: the field is absent and no violation fires —
/// "not evaluated", never accused. Same gate doctrine as
/// render_liveness.
#[test]
fn no_render_sub_app_means_field_absent() {
    let _slot = common::run_slot();

    let mut app = bevy::app::App::new();
    bevy_playtest::sinks::ensure_global_subscriber();
    // Sim-active, camera-less world.
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
        report.screen_liveness.is_none(),
        "no render sub-app ⇒ the field must stay absent (\"not evaluated\")"
    );
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.rule == "screen_liveness"),
        "no finding without an attached result"
    );
}

/// Fake render sub-app (label-only) but NO Assets<Image> store: the
/// rig's attachable() gate refuses — field absent. This pins the
/// second half of the attachment gate (render_liveness attaches on
/// the same app; its field IS present — proving the gates are
/// independent).
#[test]
fn no_asset_store_means_field_absent_even_with_render_sub_app() {
    let _slot = common::run_slot();

    let mut app = bevy::app::App::new();
    app.insert_sub_app(bevy::render::RenderApp, bevy::app::SubApp::default());
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

    assert!(
        report.screen_liveness.is_none(),
        "no Assets<Image> ⇒ the rig cannot attach ⇒ field absent"
    );
    #[cfg(feature = "render-liveness")]
    assert!(
        report.render_liveness.is_some(),
        "the structural rule attaches on the SAME app — the gates are independent"
    );
}

// ---------------------------------------------------------------------------
// GPU-positive end-to-end (real renderer, headless)
// ---------------------------------------------------------------------------

/// The spike-proven headless GPU app shape.
fn gpu_app(setup: impl FnOnce(&mut App) + Send + Sync + 'static) -> App {
    bevy_playtest::sinks::ensure_global_subscriber();
    let mut app = App::new();
    app.add_plugins(
        bevy::DefaultPlugins
            .set(bevy::window::WindowPlugin {
                primary_window: None,
                exit_condition: bevy::window::ExitCondition::DontExit,
                ..default()
            })
            .set(RenderPlugin {
                synchronous_pipeline_compilation: true,
                ..default()
            })
            .disable::<bevy::winit::WinitPlugin>()
            .disable::<bevy::log::LogPlugin>(),
    );
    setup(&mut app);
    app
}

/// A game-owned 2D camera (so the rig attaches) + a static sprite
/// scene + an out-of-view mover keeping the sim observably active
/// without touching pixels. Captures must hash identical →
/// frozen_while_sim_active (informational; status still Pass).
#[test]
fn static_2d_scene_freezes_while_sim_active() {
    if gpu_tests_skipped() {
        eprintln!("skipping: BEVY_PLAYTEST_TEST_SKIP_GPU set");
        return;
    }
    let _slot = common::run_slot();

    let mut app = gpu_app(|app| {
        app.add_systems(
            Startup,
            (|mut commands: Commands| {
                commands.spawn(Camera2d);
                commands.spawn((
                    Sprite::from_color(Color::srgb(0.8, 0.1, 0.1), Vec2::new(200.0, 100.0)),
                    Transform::from_xyz(0.0, 0.0, 0.0),
                ));
                // Out-of-view mover: sim-active for the digest/
                // frozen-world oracles, invisible to pixels.
                commands.spawn((
                    Sprite::from_color(Color::srgb(0.5, 0.5, 0.5), Vec2::new(10.0, 10.0)),
                    Transform::from_xyz(-800.0, 0.0, 0.0),
                    OffScreenMover,
                ));
            },),
        );
        app.add_systems(Update, off_screen_mover);
    });

    // Warm up plugins first: the adapter info only reaches the
    // main world at plugin FINISH (the driver normally does this
    // inside run_scenario — too late for the check below).
    warm_up_plugins(&mut app);

    // Content pin: pixel-readback fidelity is assumed. Lavapipe
    // (software Vulkan) readback is content-unfaithful (observed
    // inversion: static scenes drift) — self-skip rather than pin
    // a tier the adapter cannot honestly deliver.
    if software_adapter(&app) {
        eprintln!("skipping content pin: software adapter");
        return;
    }

    let report = run_scenario(&mut app, &long_scenario()).expect("run completes");

    assert_eq!(
        report.status,
        PlaytestStatus::Pass,
        "frozen screen is informational, never gating"
    );
    let sl = report.screen_liveness.as_ref().expect("rig attached");
    assert_eq!(sl.tier, ScreenLivenessTier::FrozenWhileSimActive);
    assert_eq!(sl.max_hamming, 0);
    assert!(
        sl.captures >= 2,
        "need >= 2 captures to judge; got {}",
        sl.captures
    );
    let entry = report
        .violations
        .iter()
        .find(|v| v.rule == "screen_liveness")
        .expect("informational entry rides the violations list");
    assert!(entry.detail.contains("never changed"));
}

#[derive(Component)]
struct OffScreenMover;

fn off_screen_mover(mut q: Query<&mut Transform, With<OffScreenMover>>) {
    // Deterministic Local-stepped sweep (NOT Time::elapsed_secs —
    // harness-driven time semantics must not couple to the test).
    for mut t in q.iter_mut() {
        t.translation.y += 1.0;
    }
}

#[derive(Component)]
struct VisibleMover;

/// A 2D scene with a big sprite sweeping across the view: captures
/// must drift (hamming >= 1) → tier live, NO screen_liveness
/// violation (refusal over silence).
#[test]
fn moving_2d_scene_is_live() {
    if gpu_tests_skipped() {
        eprintln!("skipping: BEVY_PLAYTEST_TEST_SKIP_GPU set");
        return;
    }
    let _slot = common::run_slot();

    let mut app = gpu_app(|app| {
        app.add_systems(Startup, |mut commands: Commands| {
            commands.spawn(Camera2d);
            commands.spawn((
                Sprite::from_color(Color::srgb(0.8, 0.1, 0.1), Vec2::new(140.0, 140.0)),
                Transform::from_xyz(-100.0, 0.0, 0.0),
                VisibleMover,
            ));
        });
        // 30 px/frame crosses ~1 cell (40 px) per cadence interval:
        // hash bits flip between consecutive captures.
        app.add_systems(
            Update,
            |mut q: Query<&mut Transform, With<VisibleMover>>| {
                for mut t in q.iter_mut() {
                    t.translation.x += 30.0;
                }
            },
        );
    });

    // Warm up plugins first: the adapter info only reaches the
    // main world at plugin FINISH (the driver normally does this
    // inside run_scenario — too late for the check below).
    warm_up_plugins(&mut app);

    // Content pin: lavapipe observed a moving scene hashing
    // IDENTICAL across every capture pair (the exact inversion
    // this test pins against) — self-skip on software adapters.
    if software_adapter(&app) {
        eprintln!("skipping content pin: software adapter");
        return;
    }

    let report = run_scenario(&mut app, &long_scenario()).expect("run completes");

    let sl = report.screen_liveness.as_ref().expect("rig attached");
    assert_eq!(sl.tier, ScreenLivenessTier::Live);
    assert!(
        sl.max_hamming >= 1,
        "a full-view sweep must flip hash bits; max_hamming = {}",
        sl.max_hamming
    );
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.rule == "screen_liveness"),
        "live tier emits NOTHING"
    );
}

/// A purely-3D game (Camera3d only, sprite-less): the rig attaches
/// (render sub-app + asset store) but no 2D camera is ever observed
/// → not_evaluated_3d, informational entry says so plainly.
#[test]
fn purely_3d_scene_is_not_evaluated() {
    if gpu_tests_skipped() {
        eprintln!("skipping: BEVY_PLAYTEST_TEST_SKIP_GPU set");
        return;
    }
    let _slot = common::run_slot();

    let mut app = gpu_app(|app| {
        app.add_systems(Startup, |mut commands: Commands| {
            commands.spawn(Camera3d::default());
            commands.spawn((
                Sprite::from_color(Color::srgb(0.5, 0.5, 0.5), Vec2::new(10.0, 10.0)),
                Transform::from_xyz(-800.0, 0.0, 0.0),
                OffScreenMover,
            ));
        });
        app.add_systems(Update, off_screen_mover);
    });

    let report = run_scenario(&mut app, &long_scenario()).expect("run completes");

    let sl = report.screen_liveness.as_ref().expect("rig attached");
    assert_eq!(sl.tier, ScreenLivenessTier::NotEvaluated3D);
    let entry = report
        .violations
        .iter()
        .find(|v| v.rule == "screen_liveness")
        .expect("boundary stated in the violations list");
    assert!(entry.detail.contains("not evaluated"));
}

/// Ultra-short run end-to-end: 0.2 s at 60 tps = 12 ticks. The
/// request cadence is tps/2 = 30 ticks (first request at tick 0,
/// next due at tick 30 — never reached), so exactly ONE capture can
/// ever land. The question is REFUSED, not guessed:
/// `insufficient_captures`, with the refusal stated in the
/// violations list. Covers request/capture accounting end-to-end —
/// a regression that wrongly yields Live or FrozenWhileSimActive on
/// short runs dies here.
#[test]
fn ultra_short_run_refuses_instead_of_guessing() {
    if gpu_tests_skipped() {
        eprintln!("skipping: BEVY_PLAYTEST_TEST_SKIP_GPU set");
        return;
    }
    let _slot = common::run_slot();

    let mut app = gpu_app(|app| {
        app.add_systems(Startup, |mut commands: Commands| {
            commands.spawn(Camera2d);
        });
    });

    let tiny =
        serde_json::from_str(r#"{"bot":{"type":"chaos","seed":7},"duration_s":0.2,"tps":60}"#)
            .unwrap();
    let report = run_scenario(&mut app, &tiny).expect("run completes");

    let sl = report.screen_liveness.as_ref().expect("rig attached");
    assert_eq!(
        sl.tier,
        ScreenLivenessTier::InsufficientCaptures,
        "one capture cannot answer a change question"
    );
    assert_eq!(sl.requests, 1);
    assert_eq!(sl.captures, 1);
    let entry = report
        .violations
        .iter()
        .find(|v| v.rule == "screen_liveness")
        .expect("refusal is stated, not silent");
    assert!(entry.detail.contains("refused"));
}

/// Regression pin for the shared digest-step evidence (adversarial
/// review finding): a VISUALLY SILENT but STEPPING world — static
/// sprites, a reflected game resource mutating every tick, and
/// `record_digest: true`. Nothing moves on screen, so
/// `frozen_frames > 0` and the only "the sim is alive" signal is
/// the digest stepping. The screen-liveness rig consumes the SAME
/// once-per-tick digest-step boolean as render-liveness; the buggy
/// shape this pins against had the rig call the recorder a SECOND
/// time per tick, which always compared current-vs-current and
/// answered `false` — starving the evidence and misclassifying this
/// exact world as Live. It must be judged FROZEN while sim-active.
#[test]
fn visually_silent_but_stepping_world_is_frozen_not_live() {
    if gpu_tests_skipped() {
        eprintln!("skipping: BEVY_PLAYTEST_TEST_SKIP_GPU set");
        return;
    }
    let _slot = common::run_slot();

    #[derive(Resource, Debug, bevy_reflect::Reflect)]
    #[reflect(bevy_ecs::reflect::ReflectResource)]
    struct MatchClock(f32);

    let mut app = gpu_app(|app| {
        // Register reflection BEFORE inserting the resource (same
        // pattern as the match-clock game in frozen_world_detail).
        app.world_mut()
            .get_resource_or_insert_with(AppTypeRegistry::default)
            .0
            .write()
            .register::<MatchClock>();
        app.world_mut().insert_resource(MatchClock(0.0));
        app.add_systems(Startup, |mut commands: Commands| {
            commands.spawn(Camera2d);
            commands.spawn((
                Sprite::from_color(Color::srgb(0.8, 0.1, 0.1), Vec2::new(200.0, 100.0)),
                Transform::from_xyz(0.0, 0.0, 0.0),
            ));
        });
        // Static scene; only the invisible clock steps.
        app.add_systems(Update, |mut clock: ResMut<MatchClock>| {
            clock.0 += 1.0 / 60.0;
        });
    });

    // Warm up plugins first: the adapter info only reaches the
    // main world at plugin FINISH (the driver normally does this
    // inside run_scenario — too late for the check below).
    warm_up_plugins(&mut app);

    // Content pin: the tier hinges on pixel-content stability
    // across captures (static scene must hash identical) —
    // unfaithful on software adapters (observed: static scenes
    // drift there). The DIGEST-STEP half of the pin (sim-active
    // from a stepping digest) is structural and re-verified by the
    // non-GPU frozen_world_detail suite. Self-skip on lavapipe.
    if software_adapter(&app) {
        eprintln!("skipping content pin: software adapter");
        return;
    }

    let scenario = serde_json::from_str::<bevy_playtest::scenario::Scenario>(
        r#"{"bot":{"type":"chaos","seed":7},"duration_s":1.2,"tps":60,"record_digest":true}"#,
    )
    .unwrap();
    let report = run_scenario(&mut app, &scenario).expect("run completes");

    let sl = report.screen_liveness.as_ref().expect("rig attached");
    assert_eq!(
        sl.tier,
        ScreenLivenessTier::FrozenWhileSimActive,
        "digest-proven stepping is sim-active evidence; a frozen \
         capture pair across it is frozen-while-sim-active, NOT live \
         (second-observer starvation regression)"
    );
}
