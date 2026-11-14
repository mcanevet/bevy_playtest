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
//! `PointerInput` → `PointerOver`/`PointerClick` observers firing —
//! and, critically, that those events TARGET the fullscreen probe
//! node itself. A bare firing count would pass on window-entity
//! (empty-space) events, which propagate to unfiltered observers
//! even when UI hit-testing never engages — the trap the quadrant
//! demo surfaced.

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
    /// Entities that received each event. Asserted to CONTAIN the
    /// fullscreen node: bevy_picking targets the WINDOW ENTITY for
    /// empty-space clicks, so a plain firing count would pass even
    /// if UI hit-testing never engaged — the exact trap the
    /// quadrant demo fell into.
    over_targets: Vec<Entity>,
    click_targets: Vec<Entity>,
}

/// Marker for the probe's fullscreen node, so tests can find its
/// entity without trusting spawn order.
#[derive(Component)]
struct ProbeNode;

fn setup_picking_ui(mut commands: Commands) {
    // Fullscreen UI node: any in-window cursor position hovers it.
    // Layout fills the window regardless of its size.
    commands.spawn((
        Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
        ProbeNode,
    ));
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
        |trigger: On<PointerOver>, mut probe: ResMut<PickingProbe>| {
            probe.pointer_over += 1;
            probe.over_targets.push(trigger.entity);
        },
    );
    // Observer for PointerClick: fires on any pointer click.
    app.add_observer(
        |trigger: On<PointerClick>, mut probe: ResMut<PickingProbe>| {
            probe.pointer_click += 1;
            probe.click_targets.push(trigger.entity);
        },
    );

    let report = run_scenario(&mut app, &scenario).expect("run completes");

    // Resolve the probe node's entity FIRST (needs &mut World for
    // queries) — subsequent assertions borrow `app` immutably.
    let node = {
        let world = app.world_mut();
        world
            .query_filtered::<Entity, With<ProbeNode>>()
            .single(world)
            .expect("probe node exists")
    };
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
    // The events must actually TARGET the UI node — not merely fire
    // (empty-space clicks target the window entity and would satisfy
    // a plain count).
    assert!(
        probe.over_targets.contains(&node),
        "picking fired {} PointerOver events but NONE targeted the fullscreen \
         UI node (targets: {:?}) — hit-testing never engaged; the bot's \
         cursor positions may be off-window or the UI backend is blind.",
        probe.pointer_over,
        probe.over_targets
    );
    assert!(
        probe.click_targets.contains(&node),
        "picking fired {} PointerClick events but NONE targeted the fullscreen \
         UI node (targets: {:?}) — clicks are landing on the window entity \
         (empty-space path), so games binding clicks to UI nodes would see \
         nothing.",
        probe.pointer_click,
        probe.click_targets
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
        |trigger: On<PointerOver>, mut probe: ResMut<PickingProbe>| {
            probe.pointer_over += 1;
            probe.over_targets.push(trigger.entity);
        },
    );
    app.add_observer(
        |trigger: On<PointerClick>, mut probe: ResMut<PickingProbe>| {
            probe.pointer_click += 1;
            probe.click_targets.push(trigger.entity);
        },
    );

    let report = run_scenario(&mut app, &scenario).expect("run completes");

    // Resolve the probe node's entity FIRST (needs &mut World for
    // queries) — subsequent assertions borrow `app` immutably.
    let node = {
        let world = app.world_mut();
        world
            .query_filtered::<Entity, With<ProbeNode>>()
            .single(world)
            .expect("probe node exists")
    };
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
    // Same strengthening as the chaos test: events must TARGET the
    // fullscreen node, not merely fire somewhere (empty-space clicks
    // target the window entity).
    assert!(
        probe.over_targets.contains(&node),
        "scripted clicks produced {} PointerOver events but NONE targeted the \
         fullscreen UI node (targets: {:?}) — the scripted cursor positions \
         are not being hit-tested.",
        probe.pointer_over,
        probe.over_targets
    );
    assert!(
        probe.click_targets.contains(&node),
        "scripted clicks produced {} PointerClick events but NONE targeted the \
         fullscreen UI node (targets: {:?}) — clicks land on the window entity \
         (empty-space path), so UI-bound games see nothing.",
        probe.pointer_click,
        probe.click_targets
    );
}

// --------------------------------------------------------------------------
// Mouse-motion delivery tests (raw MouseMotion / AccumulatedMouseMotion)
// --------------------------------------------------------------------------

/// Probe observing the CANONICAL Bevy motion API. Camera-pan and
/// look-control games poll `AccumulatedMouseMotion.delta` each frame;
/// before the dual-form fix the bot wrote only wrapped
/// `WindowEvent::CursorMoved` (bevy_picking's form), leaving
/// motion-reader games input-starved — the motion twin of the
/// wrapped-WindowEvent click bug.
#[derive(Resource, Default, Debug)]
struct MotionProbe {
    /// Frames with a nonzero accumulated delta.
    moving_frames: u32,
    /// Sum of every accumulated delta observed — must be nonzero, and
    /// tracks direction honestly (signed), unlike a per-frame abs.
    total_delta: Vec2,
}

fn motion_probe(
    motion: Res<bevy::input::mouse::AccumulatedMouseMotion>,
    mut probe: ResMut<MotionProbe>,
) {
    if motion.delta != Vec2::ZERO {
        probe.moving_frames += 1;
        probe.total_delta += motion.delta;
    }
}

/// Chaos mouse path: cursor position is redrawn per input interval,
/// producing real (nonzero) deltas between successive positions.
/// Movement lets the frozen_world oracle pass and keeps the probe
/// from being masked by an unrelated liveness failure.
#[test]
fn chaos_cursor_moves_reach_accumulated_motion_in_process() {
    let _slot = common::run_slot();
    let scenario: Scenario = serde_json::from_str(
        r#"{"name":"motion-chaos","bot":{"type":"chaos","seed":42,"raw_surface":{"keys":[],"mouse_buttons":["Left"]},"input_rate_hz":10},"duration_s":2.0,"tps":60,"invariants":[]}"#,
    )
    .expect("scenario parses");

    // Same graph as fixtures: DefaultPlugins minus WinitPlugin
    // (InputPlugin present — the accumulator runs once, via the
    // plugin, not the harness fallback).
    let mut app = App::new();
    app.add_plugins(
        bevy::DefaultPlugins
            .build()
            .disable::<bevy_winit::WinitPlugin>(),
    );
    app.insert_resource(MotionProbe::default());
    app.add_systems(Update, motion_probe);
    // Moving entity so frozen_world cannot mask the probe assertions.
    app.add_systems(Startup, |mut commands: Commands| {
        commands.spawn(Transform::from_xyz(0.0, 0.0, 0.0));
    });
    app.add_systems(Update, |mut q: Query<&mut Transform>| {
        for mut t in q.iter_mut() {
            t.translation.x += 0.05;
        }
    });

    let report = run_scenario(&mut app, &scenario).expect("run completes");

    let probe = app.world().resource::<MotionProbe>();
    assert!(
        probe.moving_frames > 0,
        "RAW MouseMotion regression (chaos path): the bot delivered {} inputs \
         but AccumulatedMouseMotion never saw a nonzero delta (probe: {probe:?}) \
         — motion-reader games (camera pan, look controls) are deaf to the \
         bot. The bot must write raw MouseMotion alongside the wrapped \
         WindowEvent::CursorMoved, mirroring winit's pipeline.",
        report.metrics.input_count
    );
    assert!(
        probe.total_delta != Vec2::ZERO,
        "accumulated deltas summed to zero over {} moving frames — every \
         chaos cursor redraw produced a zero delta, which cannot happen \
         under a working raw-motion path (probe: {probe:?})",
        probe.moving_frames
    );
}

/// Scripted-clicks path: `scripted_mouse_system` writes
/// `cursor_moved` on its own code path — dropping the raw write there
/// (while keeping it in the chaos path) would leave this test red and
/// every other test green. Positions are deliberately DIFFERENT per
/// click so the moved cursor yields nonzero deltas.
#[test]
fn scripted_cursor_moves_reach_accumulated_motion_in_process() {
    let _slot = common::run_slot();
    let scenario: Scenario = serde_json::from_str(
        r#"{"name":"motion-scripted","bot":{"type":"synthetic_keyboard","seed":1,"raw_surface":{"keys":[],"mouse_buttons":[]},"mouse_clicks":[{"frame":10,"button":"left","x":100.0,"y":100.0,"hold_frames":2},{"frame":40,"button":"left","x":300.0,"y":250.0,"hold_frames":2},{"frame":70,"button":"right","x":700.0,"y":400.0,"hold_frames":2}]},"duration_s":2.0,"tps":60,"invariants":[]}"#,
    )
    .expect("scenario parses");

    let mut app = App::new();
    app.add_plugins(
        bevy::DefaultPlugins
            .build()
            .disable::<bevy_winit::WinitPlugin>(),
    );
    app.insert_resource(MotionProbe::default());
    app.add_systems(Update, motion_probe);
    app.add_systems(Startup, |mut commands: Commands| {
        commands.spawn(Transform::from_xyz(0.0, 0.0, 0.0));
    });
    app.add_systems(Update, |mut q: Query<&mut Transform>| {
        for mut t in q.iter_mut() {
            t.translation.x += 0.05;
        }
    });

    let report = run_scenario(&mut app, &scenario).expect("run completes");

    let probe = app.world().resource::<MotionProbe>();
    // Exactly 2 moving frames, not 3: the FIRST scripted click is a
    // TELEPORT-IN (cursor resource starts at None, so `last_pos` is
    // None and no delta exists) — winit parity, same as a real mouse
    // entering the window. The later hops (300,250) and (700,400)
    // carry real deltas. Pinning the count guards both directions:
    // fewer means the raw write is missing, MORE would mean phantom
    // deltas on teleports.
    assert_eq!(
        probe.moving_frames, 2,
        "RAW MouseMotion regression (scripted path): {} scripted clicks \
         delivered but AccumulatedMouseMotion saw {} moving frames \
         (probe: {probe:?}) — expected exactly 2: the first click is a \
         teleport-in (no delta, winit parity); hops 2 and 3 carry \
         deltas. Fewer means scripted_mouse_system is missing the raw \
         MouseMotion write; more means phantom deltas on teleports.",
        report.metrics.input_count, probe.moving_frames
    );
    // Signed totals: the three hops are (+200,+150), (+400,+150) —
    // everything positive, so the sum must be exactly (600, 300).
    // Pinning exact magnitudes catches a sign-flip AND a wrong
    // baseline, which a mere nonzero or >threshold check would not.
    assert_eq!(
        probe.total_delta,
        Vec2::new(600.0, 300.0),
        "accumulated motion total is wrong: expected exactly (600, 300) from \
         hops (100,100)→(300,250)→(700,400), got {probe:?} — the delta may be \
         computed against the wrong baseline or sign.",
    );
}

/// Windowless path: a bare MinimalPlugins-style app has NO primary
/// window, so the wrapped `WindowEvent::CursorMoved` is impossible
/// (it targets a window) — but raw `MouseMotion` is a device event
/// with no window field and MUST still be delivered. Regression for
/// the window-gate bug: both writer call sites used to skip the
/// entire write chain (including the raw form) when no primary
/// window existed, starving windowless motion-reader games despite
/// the seeding block promising exactly those apps the accumulator.
#[test]
fn scripted_cursor_moves_reach_accumulated_motion_windowless() {
    let _slot = common::run_slot();
    let scenario: Scenario = serde_json::from_str(
        r#"{"name":"motion-windowless","bot":{"type":"synthetic_keyboard","seed":1,"raw_surface":{"keys":[],"mouse_buttons":[]},"mouse_clicks":[{"frame":10,"button":"left","x":100.0,"y":100.0,"hold_frames":2},{"frame":40,"button":"left","x":300.0,"y":250.0,"hold_frames":2}]},"duration_s":2.0,"tps":60,"invariants":[]}"#,
    )
    .expect("scenario parses");

    // Bare app: no plugins, so no InputPlugin (the harness fallback
    // accumulator runs) and NO window entity (primary_window query
    // is None on every frame).
    let mut app = App::new();
    app.insert_resource(MotionProbe::default());
    app.add_systems(Update, motion_probe);
    app.add_systems(Startup, |mut commands: Commands| {
        commands.spawn(Transform::from_xyz(0.0, 0.0, 0.0));
    });
    app.add_systems(Update, |mut q: Query<&mut Transform>| {
        for mut t in q.iter_mut() {
            t.translation.x += 0.05;
        }
    });

    let report = run_scenario(&mut app, &scenario).expect("run completes");

    let probe = app.world().resource::<MotionProbe>();
    assert_eq!(
        probe.moving_frames, 1,
        "RAW MouseMotion regression (windowless path): {} scripted clicks \
         delivered but AccumulatedMouseMotion saw {} moving frames \
         (probe: {probe:?}) — expected exactly 1: no primary window exists, \
         but the raw MouseMotion write must not be gated on one (it is a \
         windowless device event in real winit).",
        report.metrics.input_count, probe.moving_frames
    );
    // Only the second hop carries a delta ((100,100)→(300,250)); the
    // first click is a teleport-in.
    assert_eq!(
        probe.total_delta,
        Vec2::new(200.0, 150.0),
        "windowless accumulated motion total is wrong: expected exactly \
         (200, 150) from the single hop (100,100)→(300,250), got {probe:?}.",
    );
}

/// Scripted-motion path (`bot.mouse_motion`): frame-indexed deltas
/// applied per frame through BOTH delivery forms. Unlike clicks (a
/// first click is a teleport-in carrying no motion), a motion script
/// parks the cursor at the origin and delivers the raw `MouseMotion`
/// from its very FIRST frame — the delta IS the input. The probe here
/// is a balanced oscillation (+10, +5) then (−10, −5), 20 repeats
/// each: net cursor displacement must be ZERO (the zero-net property
/// the motion probes rely on to kill the position confound), while
/// the accumulated motion is nonzero — motion effects observed
/// without any position drift.
#[test]
fn scripted_mouse_motion_reaches_both_delivery_forms_and_nets_zero() {
    let _slot = common::run_slot();
    let scenario: Scenario = serde_json::from_str(
        r#"{"name":"motion-scripted","bot":{"type":"synthetic_keyboard","seed":1,"raw_surface":{"keys":[],"mouse_buttons":[]},"mouse_motion":[{"frame":10,"dx":10.0,"dy":5.0,"repeat_frames":20},{"frame":30,"dx":-10.0,"dy":-5.0,"repeat_frames":20}]},"duration_s":2.0,"tps":60,"invariants":[]}"#,
    )
    .expect("scenario parses");

    let mut app = App::new();
    app.add_plugins(
        bevy::DefaultPlugins
            .build()
            .disable::<bevy_winit::WinitPlugin>(),
    );
    app.insert_resource(MotionProbe::default());
    // Track the wrapped form too: an observer of CursorMoved deltas
    // must see the same oscillation (winit-parity synthesis).
    // WindowEvent is a MESSAGE (not an event), so no On<> observer —
    // a reader system is the direct consumer shape.
    #[derive(Resource, Default, Debug)]
    struct WrappedProbe {
        net: Vec2,
        end_position: Vec2,
        saw_events: bool,
    }
    app.insert_resource(WrappedProbe::default());
    app.add_systems(Update, motion_probe);
    app.add_systems(
        Update,
        |mut reader: bevy::ecs::message::MessageReader<bevy::window::WindowEvent>,
         mut probe: ResMut<WrappedProbe>| {
            for event in reader.read() {
                if let bevy::window::WindowEvent::CursorMoved(moved) = event {
                    probe.saw_events = true;
                    probe.net += moved.delta.unwrap_or_default();
                    probe.end_position = moved.position;
                }
            }
        },
    );
    app.add_systems(Startup, |mut commands: Commands| {
        commands.spawn(Transform::from_xyz(0.0, 0.0, 0.0));
    });
    app.add_systems(Update, |mut q: Query<&mut Transform>| {
        for mut t in q.iter_mut() {
            t.translation.x += 0.05;
        }
    });

    run_scenario(&mut app, &scenario).expect("run completes");

    let probe = app.world().resource::<MotionProbe>();
    // RAW path: 40 frames of nonzero deltas (2 × 20 repeats).
    assert_eq!(
        probe.moving_frames, 40,
        "scripted mouse_motion regression: AccumulatedMouseMotion saw only {} \
         moving frames (probe: {probe:?}) — expected exactly 40 (2 windows × \
         20 repeat_frames). scripted_motion_system must apply the delta \
         every frame of its window.",
        probe.moving_frames
    );
    // Net displacement zero: +20×(10,5) then 20×(−10,−5) cancels.
    assert_eq!(
        probe.total_delta,
        Vec2::ZERO,
        "oscillation did not net zero: accumulated total {probe:?} — the \
         zero-net-displacement property the motion probes rely on is broken.",
    );
    let wrapped = app.world().resource::<WrappedProbe>();
    assert!(
        wrapped.saw_events,
        "scripted motion never delivered the wrapped WindowEvent::CursorMoved \
         — a picking/hover reader would see nothing (winit-parity synthesis \
         requires both forms)."
    );
    assert_eq!(
        wrapped.net,
        Vec2::ZERO,
        "wrapped CursorMoved deltas did not net zero: {wrapped:?}"
    );
    assert_eq!(
        wrapped.end_position,
        Vec2::ZERO,
        "cursor end position is not the origin: {wrapped:?} — a balanced \
         oscillation must park the cursor back where it started."
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
