//! In-process driver: runs a scenario against an already-built Bevy App.
//!
//! Panic/log hooks, single-threaded setup, chaos bot,
//! synthetic_keyboard bot, synthetic_pointer bot (feature-gated),
//! frame-timing oracle, frozen_world stub.
//! The full oracle suite (liveness, bounds, NaN, pass_through) ports
//! incrementally.

use bevy_app::{App, PluginsState, Update};
use bevy_ecs::prelude::*;
use bevy_ecs::schedule::{Schedules, SingleThreadedExecutor};
use bevy_input::keyboard::{KeyCode, KeyboardFocusLost, KeyboardInput};
use bevy_input::{prelude::ButtonInput, ButtonState};
use bevy_time::{Time, TimeUpdateStrategy};
use bevy_transform::prelude::Transform;
use std::sync::Arc;

#[cfg(feature = "pointer-input")]
use bevy_picking::pointer::{
    PointerAction, PointerButton as BevyPointerButton, PointerId, PointerInput,
};

#[cfg(feature = "pointer-input")]
use bevy_transform::components::GlobalTransform;

use crate::contract::{CheatHooks, ResetHooks};
use crate::enums::{BotType, PlaytestStatus};
use crate::rules::Rule;
use crate::scenario::{validate_scenario, Scenario, ScenarioError};
use crate::state::PlaytestReport;
use crate::{rules, sinks};

// ---------------------------------------------------------------------------
// Scenario resource
// ---------------------------------------------------------------------------

#[derive(Resource)]
struct ScenarioResource(Arc<Scenario>);

/// Wrapper giving core's Bevy-free Violations Resource status in this crate.
#[derive(Resource, Default)]
pub struct RunViolations(pub crate::state::Violations);

/// Wrapper giving core's Bevy-free ChoiceStream Resource status.
#[derive(Resource)]
pub struct RunChoices(pub(crate) crate::choice::ChoiceStream);

// ---------------------------------------------------------------------------
// Panic hook (global, one-time install via core's sinks)
// ---------------------------------------------------------------------------

fn install_panic_hook() {
    sinks::install_panic_hook();
}

// ---------------------------------------------------------------------------
// Single-threaded executor enforcement
// ---------------------------------------------------------------------------

fn force_single_threaded(app: &mut App) -> Result<(), ScenarioError> {
    let Some(mut schedules) = app.world_mut().remove_resource::<Schedules>() else {
        return Err(ScenarioError::Rejected(
            "force_single_threaded: Schedules not found".into(),
        ));
    };
    for (_, sched) in schedules.iter_mut() {
        sched.set_executor(SingleThreadedExecutor::new());
    }
    app.world_mut().insert_resource(schedules);
    Ok(())
}

/// Placeholder logical key for synthetic events (physical keycode drives
/// behavior; the game's input handling reads key_code/`text`).
const KEY_UNIDENTIFIED: bevy_input::keyboard::Key =
    bevy_input::keyboard::Key::Unidentified(bevy_input::keyboard::NativeKey::Unidentified);

// ---------------------------------------------------------------------------
// Bot systems (chaos + synthetic_keyboard)
// ---------------------------------------------------------------------------

/// Canonical key name -> KeyCode. Shared by the keyboard bot and the
/// chaos bot's raw fallback.
fn keycode_from_canonical(canonical: &str) -> Option<KeyCode> {
    Some(match canonical {
        "Space" => KeyCode::Space,
        "Enter" => KeyCode::Enter,
        "Escape" => KeyCode::Escape,
        "Tab" => KeyCode::Tab,
        "Backspace" => KeyCode::Backspace,
        "ArrowUp" => KeyCode::ArrowUp,
        "ArrowDown" => KeyCode::ArrowDown,
        "ArrowLeft" => KeyCode::ArrowLeft,
        "ArrowRight" => KeyCode::ArrowRight,
        "ShiftLeft" => KeyCode::ShiftLeft,
        "ControlLeft" => KeyCode::ControlLeft,
        "AltLeft" => KeyCode::AltLeft,
        // "KeyW" -> KeyCode::KeyW ... "Digit5" -> KeyCode::Digit5
        other => {
            const KEYS: [&str; 26] = [
                "KeyA", "KeyB", "KeyC", "KeyD", "KeyE", "KeyF", "KeyG", "KeyH", "KeyI", "KeyJ",
                "KeyK", "KeyL", "KeyM", "KeyN", "KeyO", "KeyP", "KeyQ", "KeyR", "KeyS", "KeyT",
                "KeyU", "KeyV", "KeyW", "KeyX", "KeyY", "KeyZ",
            ];
            const DIGITS: [&str; 10] = [
                "Digit0", "Digit1", "Digit2", "Digit3", "Digit4", "Digit5", "Digit6", "Digit7",
                "Digit8", "Digit9",
            ];
            if let Some(i) = KEYS.iter().position(|k| *k == other) {
                return KEY_CODE_LETTERS.get(i).copied();
            }
            if let Some(i) = DIGITS.iter().position(|k| *k == other) {
                return KEY_CODE_DIGITS.get(i).copied();
            }
            return None;
        }
    })
}

const KEY_CODE_LETTERS: [KeyCode; 26] = [
    KeyCode::KeyA,
    KeyCode::KeyB,
    KeyCode::KeyC,
    KeyCode::KeyD,
    KeyCode::KeyE,
    KeyCode::KeyF,
    KeyCode::KeyG,
    KeyCode::KeyH,
    KeyCode::KeyI,
    KeyCode::KeyJ,
    KeyCode::KeyK,
    KeyCode::KeyL,
    KeyCode::KeyM,
    KeyCode::KeyN,
    KeyCode::KeyO,
    KeyCode::KeyP,
    KeyCode::KeyQ,
    KeyCode::KeyR,
    KeyCode::KeyS,
    KeyCode::KeyT,
    KeyCode::KeyU,
    KeyCode::KeyV,
    KeyCode::KeyW,
    KeyCode::KeyX,
    KeyCode::KeyY,
    KeyCode::KeyZ,
];

const KEY_CODE_DIGITS: [KeyCode; 10] = [
    KeyCode::Digit0,
    KeyCode::Digit1,
    KeyCode::Digit2,
    KeyCode::Digit3,
    KeyCode::Digit4,
    KeyCode::Digit5,
    KeyCode::Digit6,
    KeyCode::Digit7,
    KeyCode::Digit8,
    KeyCode::Digit9,
];

/// Chaos bot: emits random raw key presses. Uses the scenario's
/// raw_surface (or the default WASD+arrows surface) — it does not yet
/// consult an IntentSurface. Keys are held for `hold_frames` frames before
/// release to avoid the press+release-in-same-frame cancellation bug.
#[derive(Resource, Default)]
struct ChaosHoldState {
    /// If Some((key_code, release_frame)), this key is currently held.
    held: Option<(bevy_input::keyboard::KeyCode, u64)>,
}

fn chaos_bot_system(
    mut state: ResMut<RunState>,
    scenario: Res<ScenarioResource>,
    mut choices: ResMut<RunChoices>,
    mut hold_state: ResMut<ChaosHoldState>,
    mut writer: MessageWriter<KeyboardInput>,
) {
    let rate = scenario.0.bot.input_rate_hz.max(1);
    let interval = (state.tps / rate as u64).max(1);
    // Chaos presses have no per-press schedule; hold each key long enough
    // (2 frames) that ButtonInput sees Pressed for at least one full frame.
    let hold_frames = 2u64;

    // Release held key if its hold duration expired.
    if let Some((code, release_at)) = hold_state.held {
        if state.frame >= release_at {
            writer.write(KeyboardInput {
                key_code: code,
                logical_key: bevy_input::keyboard::Key::Unidentified(
                    bevy_input::keyboard::NativeKey::Unidentified,
                ),
                state: ButtonState::Released,
                text: None,
                repeat: false,
                window: Entity::PLACEHOLDER,
            });
            hold_state.held = None;
        }
    }

    // Don't pick a new key while one is held.
    if hold_state.held.is_some() {
        return;
    }

    if !state.frame.is_multiple_of(interval) {
        return;
    }
    let default_keys = [
        "KeyW",
        "KeyA",
        "KeyS",
        "KeyD",
        "ArrowUp",
        "ArrowDown",
        "ArrowLeft",
        "ArrowRight",
    ];
    let keys: Vec<&str> = match &scenario.0.bot.raw_surface {
        Some(surface) if !surface.keys.is_empty() => {
            surface.keys.iter().map(String::as_str).collect()
        }
        _ => default_keys.to_vec(),
    };
    let idx = choices.0.below(state.frame, keys.len() as u64) as usize;
    let Some(canonical) = crate::scenario::normalize_key_name(keys[idx]) else {
        return;
    };
    let Some(code) = keycode_from_canonical(canonical) else {
        return;
    };
    writer.write(KeyboardInput {
        key_code: code,
        logical_key: bevy_input::keyboard::Key::Unidentified(
            bevy_input::keyboard::NativeKey::Unidentified,
        ),
        state: ButtonState::Pressed,
        text: None,
        repeat: false,
        window: Entity::PLACEHOLDER,
    });
    state.metrics.input_count += 1;
    hold_state.held = Some((code, state.frame + hold_frames));
}

/// Synthetic keyboard bot: replays scheduled key presses through the real
/// keyboard input chain. Presses are held for `hold_frames` frames before
/// release to avoid the same-frame press+release cancellation bug.
#[derive(Resource, Default)]
struct ScheduledHoldState {
    /// Queue of (key_code, release_frame) for held keys.
    held: Vec<(bevy_input::keyboard::KeyCode, u64)>,
}

fn synthetic_keyboard_bot_system(
    mut state: ResMut<RunState>,
    scenario: Res<ScenarioResource>,
    mut hold_state: ResMut<ScheduledHoldState>,
    mut writer: MessageWriter<KeyboardInput>,
) {
    // Release any keys whose hold duration expired.
    hold_state.held.retain(|(code, release_at)| {
        if state.frame >= *release_at {
            writer.write(KeyboardInput {
                key_code: *code,
                logical_key: KEY_UNIDENTIFIED,
                state: ButtonState::Released,
                text: None,
                repeat: false,
                window: Entity::PLACEHOLDER,
            });
            false
        } else {
            true
        }
    });

    for kp in &scenario.0.bot.key_presses {
        if kp.frame != state.frame {
            continue;
        }
        let Some(canonical) = crate::scenario::normalize_key_name(&kp.key) else {
            continue;
        };
        let Some(code) = keycode_from_canonical(canonical) else {
            continue;
        };
        // Per-press hold duration (default 1 frame after the press).
        let hold_frames = kp.hold_frames.max(1);
        writer.write(KeyboardInput {
            key_code: code,
            logical_key: KEY_UNIDENTIFIED,
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
        state.metrics.input_count += 1;
        hold_state.held.push((code, state.frame + hold_frames));
    }
}

// ---------------------------------------------------------------------------
// synthetic_pointer bot (feature: pointer-input)
// ---------------------------------------------------------------------------

#[cfg(feature = "pointer-input")]
mod pointer_bot {
    use super::*;

    /// One in-flight click gesture: press and release frames scheduled
    /// relative to the initiating move.
    #[derive(Clone)]
    pub(super) struct PendingGesture {
        press_at: u64,
        release_at: u64,
        location: bevy_picking::pointer::Location,
        button: BevyPointerButton,
        press_delivered: bool,
    }

    /// In-flight gestures (front = oldest).
    #[derive(Resource, Default)]
    pub(super) struct PendingGestures {
        queue: std::collections::VecDeque<PendingGesture>,
    }

    fn to_bevy_button(button: crate::enums::PointerButton) -> BevyPointerButton {
        match button {
            crate::enums::PointerButton::Primary => BevyPointerButton::Primary,
            crate::enums::PointerButton::Secondary => BevyPointerButton::Secondary,
            crate::enums::PointerButton::Middle => BevyPointerButton::Middle,
        }
    }

    /// synthetic_pointer: emits RAW `PointerInput` events (Move → Press
    /// → Release) at a named entity's projected viewport position.
    /// Unlike intent-injection bots (chaos), this drives the game
    /// through its real input chain: pointer input → picking backend
    /// raycast → `PointerClick` message → the game's own handlers.
    /// A game whose picking wiring is broken produces clicks that hit
    /// nothing — caught by the scenario's invariants, not silently
    /// skipped.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn synthetic_pointer_bot_system(
        mut state: ResMut<RunState>,
        mut violations: ResMut<RunViolations>,
        mut gestures: ResMut<PendingGestures>,
        mut pointer_inputs: MessageWriter<PointerInput>,
        // Target resolution is Name-first, zero-annotation: any named
        // entity is clickable (matches the ecosystem convention — Name
        // is the Bevy lingua franca for addressing entities). The
        // `Gameplay` marker remains available for invariant scoping,
        // but pointer clicks do not require it.
        q_named: Query<(&Name, &GlobalTransform)>,
        q_ui_named: Query<(
            &Name,
            &bevy_ui::prelude::UiGlobalTransform,
            &bevy_ui::prelude::ComputedNode,
        )>,
        q_camera: Query<(&bevy_camera::Camera, &GlobalTransform)>,
        primary_window: Query<Entity, With<bevy_window::PrimaryWindow>>,
        scenario: Res<ScenarioResource>,
    ) {
        // Deliver pending gesture halves scheduled for this frame.
        deliver_pending_gestures(
            &mut state,
            &mut violations,
            &mut gestures,
            &mut pointer_inputs,
        );
        for click in &scenario.0.bot.pointer_clicks {
            if click.frame != state.frame {
                continue;
            }
            let button = to_bevy_button(click.button);
            // Resolve the target to viewport coordinates. Two spaces:
            // world-space meshes (GlobalTransform + camera projection)
            // and UI nodes (UiGlobalTransform + ComputedNode, already
            // in physical pixels). Try world-space first.
            let mut viewport_pos = None;
            if let Some((_, transform)) = q_named
                .iter()
                .find(|(name, _)| name.as_str() == click.target)
            {
                for (camera, cam_transform) in q_camera.iter() {
                    if let Ok(p) = camera.world_to_viewport(cam_transform, transform.translation())
                    {
                        viewport_pos = Some(p);
                        break;
                    }
                }
            } else if let Some((_, ui_transform, node)) = q_ui_named
                .iter()
                .find(|(name, _, _)| name.as_str() == click.target)
            {
                // UI nodes hit-test in physical pixels; the picking
                // backend multiplies Location.position by the camera's
                // target scaling factor, so divide back out to logical.
                let center_phys = ui_transform.translation;
                let scale = node.inverse_scale_factor;
                viewport_pos = Some(bevy_math::Vec2::new(
                    center_phys.x * scale,
                    center_phys.y * scale,
                ));
            }
            let Some(position) = viewport_pos else {
                violations.0.report(
                    Rule::BotConfig.name(),
                    &click.target,
                    format!(
                        "click target '{}' not found among named entities \
                         (world-space or UI)",
                        click.target
                    ),
                    state.frame,
                );
                continue;
            };
            let Ok(window_entity) = primary_window.single() else {
                violations.0.report(
                    Rule::BotConfig.name(),
                    &click.target,
                    "no primary window entity — cannot synthesize pointer events".into(),
                    state.frame,
                );
                continue;
            };
            let Some(target) =
                bevy_camera::RenderTarget::Window(bevy_window::WindowRef::Entity(window_entity))
                    .normalize(Some(window_entity))
            else {
                violations.0.report(
                    Rule::BotConfig.name(),
                    &click.target,
                    "primary window render-target normalization failed".into(),
                    state.frame,
                );
                continue;
            };
            let location = bevy_picking::pointer::Location { target, position };
            // A click is a THREE-FRAME gesture: Move, then Press, then
            // Release. Rationale: bevy_picking's release/click dispatch
            // reads the PREVIOUS frame's hover map (previous_hover_map),
            // so a press and release in the same frame as the move
            // finds no prior hover and silently drops the click.
            // Spreading the gesture across frames mirrors real mouse
            // timing.
            pointer_inputs.write(PointerInput::new(
                PointerId::Mouse,
                location.clone(),
                PointerAction::Move {
                    delta: bevy_math::Vec2::ZERO,
                },
            ));
            // Pending press/release delivered on subsequent frames.
            let f = state.frame;
            gestures.queue.push_back(PendingGesture {
                press_at: f + 1,
                release_at: f + 1 + click.hold_frames.max(1),
                location,
                button,
                press_delivered: false,
            });
            state.metrics.input_count += 1;
        }
    }

    /// Chaos bot's pointer half: at the scenario's input rate, click the
    /// center of a SEEDED-RANDOM UI node. Zero-annotation: any UI node
    /// is a candidate (clicks bubble up the hierarchy, so hitting a
    /// button's child text or a container still reaches interactive
    /// widgets). Drives menu-style games the keyboard surface can't,
    /// through the real picking chain.
    pub(super) fn chaos_pointer_click_system(
        mut state: ResMut<RunState>,
        mut violations: ResMut<RunViolations>,
        mut choices: ResMut<RunChoices>,
        mut gestures: ResMut<PendingGestures>,
        mut pointer_inputs: MessageWriter<PointerInput>,
        q_ui: Query<(
            &bevy_ui::prelude::UiGlobalTransform,
            &bevy_ui::prelude::ComputedNode,
        )>,
        primary_window: Query<Entity, With<bevy_window::PrimaryWindow>>,
        scenario: Res<ScenarioResource>,
    ) {
        // Deliver pending gesture halves scheduled for this frame.
        deliver_pending_gestures(
            &mut state,
            &mut violations,
            &mut gestures,
            &mut pointer_inputs,
        );

        // One click per input-rate interval, mirroring the keyboard
        // half's pacing.
        let rate = scenario.0.bot.input_rate_hz.max(1);
        let interval = (state.tps / rate as u64).max(1);
        if state.frame == 0 || !state.frame.is_multiple_of(interval) {
            return;
        }

        let nodes: Vec<_> = q_ui.iter().collect();
        if nodes.is_empty() {
            return;
        }
        let idx = choices.0.below(state.frame, nodes.len() as u64) as usize;
        let (ui_transform, node) = nodes[idx];
        // UI nodes hit-test in physical pixels; the picking backend
        // multiplies Location.position by the camera's target scaling
        // factor, so divide back out to logical (same conversion as
        // the synthetic_pointer bot's named-target path).
        let center_phys = ui_transform.translation;
        let scale = node.inverse_scale_factor;
        let position = bevy_math::Vec2::new(center_phys.x * scale, center_phys.y * scale);

        let Ok(window_entity) = primary_window.single() else {
            return;
        };
        let Some(target) =
            bevy_camera::RenderTarget::Window(bevy_window::WindowRef::Entity(window_entity))
                .normalize(Some(window_entity))
        else {
            return;
        };
        let location = bevy_picking::pointer::Location { target, position };
        // Three-frame gesture (Move, then Press, then Release): the
        // picking backend reads the PREVIOUS frame's hover map on
        // release, so same-frame moves would be dropped.
        pointer_inputs.write(PointerInput::new(
            PointerId::Mouse,
            location.clone(),
            PointerAction::Move {
                delta: bevy_math::Vec2::ZERO,
            },
        ));
        let f = state.frame;
        gestures.queue.push_back(PendingGesture {
            press_at: f + 1,
            release_at: f + 2,
            location,
            button: BevyPointerButton::Primary,
            press_delivered: false,
        });
        state.metrics.input_count += 1;
    }

    /// Deliver pending gesture halves scheduled for this (or a past)
    /// frame. Split out for direct testability.
    pub(super) fn deliver_pending_gestures(
        state: &mut RunState,
        violations: &mut RunViolations,
        gestures: &mut PendingGestures,
        pointer_inputs: &mut MessageWriter<PointerInput>,
    ) {
        let frame = state.frame;
        let _ = violations;
        let mut still_pending = std::collections::VecDeque::new();
        while let Some(mut g) = gestures.queue.pop_front() {
            // Deliver as soon as we're at or past the scheduled frame —
            // exact equality loses the press half when the frame jumps
            // by more than 1 (pause, startup jitter, frame drop).
            // press_delivered guarantees at-most-once delivery.
            if !g.press_delivered && g.press_at <= frame {
                pointer_inputs.write(PointerInput::new(
                    PointerId::Mouse,
                    g.location.clone(),
                    PointerAction::Press(g.button),
                ));
                g.press_delivered = true;
            }
            if g.release_at <= frame {
                pointer_inputs.write(PointerInput::new(
                    PointerId::Mouse,
                    g.location.clone(),
                    PointerAction::Release(g.button),
                ));
                // Gesture complete.
                continue;
            }
            // Not yet fully delivered — keep waiting.
            still_pending.push_back(g);
        }
        gestures.queue = still_pending;
    }

    /// Fill `Camera.computed.target_info` from the primary window
    /// when bevy_render's camera_system is absent (headless runs
    /// with RenderPlugin disabled). Without target_info, pointer
    /// picking and viewport projection fail with
    /// NoViewportSize.
    pub(super) fn headless_camera_target_info_system(
        primary_window: Query<&bevy_window::Window, With<bevy_window::PrimaryWindow>>,
        mut cameras: Query<&mut bevy_camera::Camera>,
    ) {
        let Some(window) = primary_window.iter().next() else {
            return;
        };
        let info = bevy_camera::RenderTargetInfo {
            physical_size: window.physical_size(),
            scale_factor: window.resolution.scale_factor(),
        };
        for mut camera in &mut cameras {
            // Only fill when bevy_render's camera_system hasn't
            // already (it sets target_info when RenderPlugin is
            // present).
            if camera.computed.target_info.is_none() {
                camera.computed.target_info = Some(info.clone());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Oracles (frame timing, frozen_world)
// ---------------------------------------------------------------------------

/// Frame timing oracle: Welford running mean + anomaly detection.
fn frame_timing_oracle_system(mut state: ResMut<RunState>, time: Res<Time>) {
    let delta_ms = time.delta_secs_f64() * 1000.0;
    state.frame_timing.observe(delta_ms);
    if state.frame_timing.is_anomalous(delta_ms) {
        state.frame_timing.anomalies += 1;
    }
}

/// Frozen-world oracle: detects when the world stops changing. Counts
/// consecutive frames with no `Changed<Transform>` anywhere and reports
/// a `frozen_world` violation once the streak exceeds the grace window.
/// Needs no marker: "any transform changed" is the liveness evidence.
fn frozen_world_oracle_system(
    mut state: ResMut<RunState>,
    mut violations: ResMut<RunViolations>,
    q: Query<(), Changed<Transform>>,
    q_all: Query<(), With<Transform>>,
    q_entities: Query<Entity>,
) {
    if !q_all.is_empty() {
        state.saw_transform_entity = true;
    }
    // Liveness evidence, in order of strength:
    // 1. A Transform changed somewhere (motion, animation, physics).
    // 2. The world POPULATION changed (entities spawned/despawned).
    //    Covers UI-driven games whose screens are `Node` trees with no
    //    `Transform` at all: a menu transition despawns/respawns the UI
    //    without touching a single Transform, which motion-only
    //    liveness would misreport as a soft-lock. A dead game spawns
    //    and despawns nothing, so the oracle still fires there.
    let population = q_entities.iter().count() as u64;
    let world_changed = !q.is_empty() || population != state.last_population;
    state.last_population = population;
    if world_changed {
        state.frozen_frames = 0;
    } else {
        state.frozen_frames += 1;
    }
    // Report once per crossing (guard so we don't spam every frame).
    if state.frozen_frames == state.frozen_grace_frames + 1 {
        violations.0.report(
            Rule::FrozenWorld.name(),
            "world",
            format!(
                "no world activity (Transform change, spawn/despawn) for {} frames — soft-lock?",
                state.frozen_frames
            ),
            state.frame,
        );
    }
}

// ---------------------------------------------------------------------------
// Run state
// ---------------------------------------------------------------------------

/// State resource tracking the current run.
#[derive(Resource)]
pub struct RunState {
    pub frame: u64,
    tps: u64,
    metrics: crate::state::Metrics,
    frame_timing: FrameTimingStats,
    frozen_frames: u64,
    /// Grace period (frames) before frozen_world is reported.
    frozen_grace_frames: u64,
    /// True once ANY Transform-bearing entity existed during the run.
    /// The vacuity guard uses this instead of a post-hoc world query:
    /// a game whose entities all despawn before the end is NOT vacuous
    /// (the oracles observed them while they lived).
    saw_transform_entity: bool,
    /// Last-frame entity population for detecting spawn/despawn as
    /// liveness evidence. Initialized on first run.
    last_population: u64,
}

impl Default for RunState {
    fn default() -> Self {
        Self {
            frame: 0,
            tps: 60,
            metrics: Default::default(),
            frame_timing: Default::default(),
            frozen_frames: 0,
            // Grace from scenario liveness.timeout_s: short pauses OK,
            // true freezes caught. Injected in run_scenario before the
            // loop runs.
            frozen_grace_frames: 180,
            saw_transform_entity: false,
            last_population: 0,
        }
    }
}

/// Welford statistics for frame timing.
#[derive(Default)]
pub struct FrameTimingStats {
    pub mean: f64,
    pub m2: f64,
    pub count: u64,
    pub worst_ms: f64,
    pub anomalies: u64,
}

fn pretty_cmp(cmp: crate::scenario::CountCmp) -> &'static str {
    match cmp {
        crate::scenario::CountCmp::Lt => "fewer",
        crate::scenario::CountCmp::Gt => "more",
        crate::scenario::CountCmp::Eq => "equal",
    }
}

fn normalize_digits(msg: &str) -> String {
    let mut out = String::with_capacity(msg.len());
    let mut prev_digit = false;
    for c in msg.chars() {
        if c.is_ascii_digit() {
            if !prev_digit {
                out.push('#');
            }
            prev_digit = true;
        } else {
            out.push(c);
            prev_digit = false;
        }
    }
    out
}

fn transform_entity_count(world: &mut bevy_ecs::prelude::World) -> usize {
    world
        .query_filtered::<(), bevy_ecs::query::With<bevy_transform::prelude::Transform>>()
        .iter(world)
        .count()
}

/// Run condition: fingerprint only when the scenario opted in.
fn verify_replay_enabled(scenario: Res<ScenarioResource>) -> bool {
    scenario.0.verify_replay
}

impl FrameTimingStats {
    pub fn observe(&mut self, value: f64) {
        self.count += 1;
        let delta = value - self.mean;
        self.mean += delta / self.count as f64;
        let delta2 = value - self.mean;
        self.m2 += delta * delta2;
        self.worst_ms = self.worst_ms.max(value);
    }

    pub fn variance(&self) -> f64 {
        if self.count < 2 {
            return 0.0;
        }
        self.m2 / (self.count - 1) as f64
    }

    pub fn stddev(&self) -> f64 {
        self.variance().sqrt()
    }

    pub fn is_anomalous(&self, value: f64) -> bool {
        if self.count < 10 {
            return false;
        }
        let z = (value - self.mean) / self.stddev().max(1e-9);
        z > 3.0
    }
}

// ---------------------------------------------------------------------------
// Main run loop
// ---------------------------------------------------------------------------

/// Run one scenario against an already-built `App`.
///
/// Returns a structured report with violations, metrics, and action log.
pub fn run_scenario(app: &mut App, scenario: &Scenario) -> Result<PlaytestReport, ScenarioError> {
    validate_scenario(scenario)?;

    // Install global tracing subscriber (captures ERROR/WARN logs into the
    // run's violation buffer). The panic hook is installed separately below.
    sinks::ensure_global_subscriber();
    install_panic_hook();

    // Acquire the exclusive run buffer FIRST: everything after this
    // point (setup included — plugin finishing, hook invocation, system
    // registration) is attributable to this run. Concurrent or nested
    // run_scenario calls are rejected here, not blocked: a blocked
    // second caller would emit setup-phase events into OUR buffer
    // (misattribution), and a nested call from a worker thread of this
    // very app would deadlock the loop waiting for that worker. The
    // guard must outlive the catch_unwind so the unwinding loop's own
    // panic is captured before the run ends.
    let _run_guard = sinks::try_acquire_run().map_err(ScenarioError::Rejected)?;

    // Finish plugin building exactly like App::run would.
    while matches!(app.plugins_state(), PluginsState::Adding) {
        bevy_tasks::tick_global_task_pools_on_main_thread();
    }
    if matches!(app.plugins_state(), PluginsState::Ready) {
        app.finish();
        app.cleanup();
    }

    if scenario.single_threaded {
        force_single_threaded(app)?;
    }

    let tps = scenario.tps as u64;
    // Frozen-world grace derives from the scenario's liveness timeout:
    // one source of truth, honoring liveness.timeout_s.
    let frozen_grace_frames = (scenario.liveness.timeout_s * scenario.tps as f32).max(1.0) as u64;
    app.insert_resource(ScenarioResource(Arc::new(scenario.clone())));
    app.insert_resource(RunState {
        tps,
        frozen_grace_frames,
        ..Default::default()
    });
    app.insert_resource(RunViolations::default());
    app.insert_resource(RunChoices(crate::choice::ChoiceStream::from_seed(
        scenario.bot.seed,
    )));

    // Resets: validate against registered hooks, then invoke (inside a
    // panic boundary — a panicking hook is reported, not fatal).
    // needs_reset: any cheat/restart semantics in the scenario mean the
    // run depends on reset semantics — without hooks this must fail
    // loudly (never a fake no-op reset).
    let needs_reset =
        !scenario.setup.resets.is_empty() || scenario.setup.cheats.iter().any(|c| c.frame == 0);
    let registered_hooks = app
        .world()
        .get_resource::<ResetHooks>()
        .map(|h| !h.0.is_empty())
        .unwrap_or(false);
    if needs_reset && !scenario.setup.resets.is_empty() && !registered_hooks {
        return Err(ScenarioError::ResetMissing);
    }
    let reset_hooks_registered = needs_reset.then_some(registered_hooks);
    {
        let kinds: Vec<String> = scenario
            .setup
            .resets
            .iter()
            .map(|r| r.kind.clone())
            .collect();
        for k in &kinds {
            let known: Vec<String> = app
                .world()
                .get_resource::<ResetHooks>()
                .map(|h| h.0.keys().cloned().collect())
                .unwrap_or_default();
            if !known.contains(k) {
                return Err(ScenarioError::UnknownReset {
                    kind: k.clone(),
                    known: known.clone(),
                });
            }
        }
        // Invoke (hooks taken out of the world to avoid double-borrow).
        if !kinds.is_empty() {
            let hooks = app
                .world_mut()
                .remove_resource::<ResetHooks>()
                .unwrap_or_default();
            for k in &kinds {
                if let Some(hook) = hooks.0.get(k) {
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        hook(app.world_mut());
                    }))
                    .unwrap_or_else(|_| {
                        if let Some(mut violations) =
                            app.world_mut().get_resource_mut::<RunViolations>()
                        {
                            violations.0.report(
                                rules::Rule::ResetHookPanicked.name(),
                                k,
                                format!("reset hook '{k}' panicked"),
                                0,
                            );
                        }
                    });
                }
            }
            app.insert_resource(hooks);
        }
    }

    // Cheats: validate kinds (scheduled application lands with the
    // cheat scheduler; setup.cheats are applied immediately pre-loop).
    {
        let known: Vec<String> = app
            .world()
            .get_resource::<CheatHooks>()
            .map(|h| h.0.keys().cloned().collect())
            .unwrap_or_default();
        for cheat in &scenario.setup.cheats {
            if !known.contains(&cheat.kind) {
                return Err(ScenarioError::UnknownCheat {
                    kind: cheat.kind.clone(),
                    known: known.clone(),
                });
            }
        }
        let scheduled = !scenario.setup.cheats.is_empty();
        if scheduled {
            let hooks = app
                .world_mut()
                .remove_resource::<CheatHooks>()
                .unwrap_or_default();
            for cheat in &scenario.setup.cheats {
                // Postcondition contract: entity count before the hook
                // vs after, evaluated immediately — catches silent
                // no-op hooks (the anti-lying-oracle).
                let pre_count = cheat
                    .expects
                    .as_ref()
                    .map(|_| transform_entity_count(app.world_mut()));
                if let Some(hook) = hooks.0.get(&cheat.kind) {
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        hook(app.world_mut());
                    }))
                    .unwrap_or_else(|_| {
                        if let Some(mut violations) =
                            app.world_mut().get_resource_mut::<RunViolations>()
                        {
                            violations.0.report(
                                rules::Rule::CheatHookPanicked.name(),
                                &cheat.kind,
                                format!("cheat hook '{}' panicked", cheat.kind),
                                0,
                            );
                        }
                    });
                }
                if let Some(expected) = &cheat.expects {
                    let post_count = transform_entity_count(app.world_mut());
                    let ok = match expected.transform_entity_count {
                        crate::scenario::CountCmp::Lt => {
                            post_count < pre_count.unwrap_or(post_count)
                        }
                        crate::scenario::CountCmp::Gt => {
                            post_count > pre_count.unwrap_or(post_count)
                        }
                        crate::scenario::CountCmp::Eq => {
                            post_count == pre_count.unwrap_or(post_count)
                        }
                    };
                    if !ok {
                        if let Some(mut violations) =
                            app.world_mut().get_resource_mut::<RunViolations>()
                        {
                            violations.0.report(
                                rules::Rule::CheatContractViolated.name(),
                                &cheat.kind,
                                format!(
                                    "cheat '{}' postcondition violated: transform \
                                     entity count was {}, expected {} after the \
                                     hook ran ({} -> {}) — the hook may be a \
                                     silent no-op",
                                    cheat.kind,
                                    pretty_cmp(expected.transform_entity_count),
                                    pre_count.unwrap_or(post_count),
                                    pre_count.unwrap_or(post_count),
                                    post_count
                                ),
                                0,
                            );
                        }
                    }
                }
            }
            app.insert_resource(hooks);
        }
    }

    if scenario.simulated_time {
        app.insert_resource(TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_secs_f64(1.0 / tps as f64),
        ));
    }

    // The harness writes keyboard messages; ensure they are registered
    // even on apps that never set up input. Games commonly read
    // ButtonInput<KeyCode>; MinimalPlugins lacks InputPlugin, so the
    // harness seeds the resource AND runs the event->ButtonInput
    // translation system. Without it, synthetic KeyboardInput events are
    // never observed by ButtonInput-based games (silent vacuous pass).
    app.add_message::<KeyboardInput>();
    app.add_message::<KeyboardFocusLost>();
    #[cfg(feature = "pointer-input")]
    app.add_message::<PointerInput>();
    app.init_resource::<ButtonInput<KeyCode>>();
    app.init_resource::<ButtonInput<bevy_input::keyboard::Key>>();
    // Order: translate BEFORE game systems in Update so games observe the
    // same-frame state (matching InputPlugin's placement in PreUpdate).
    app.add_systems(
        bevy_app::prelude::PreUpdate,
        bevy_input::keyboard::keyboard_input_system,
    );
    // Oracles read wall-clock delta; Time may be absent on minimal apps.
    if app.world().get_resource::<Time>().is_none() {
        app.insert_resource(Time::<()>::default());
    }

    // Register bot + oracle systems once.
    app.init_resource::<ChaosHoldState>();
    app.init_resource::<ScheduledHoldState>();
    #[cfg(feature = "pointer-input")]
    app.init_resource::<pointer_bot::PendingGestures>();

    app.insert_resource(crate::invariant_eval::ScenarioInvariants(
        scenario.invariants.clone(),
    ));
    if scenario.verify_replay {
        app.insert_resource(crate::replay::ReplaySeries::default());
    }

    app.add_systems(
        Update,
        (
            chaos_bot_system.run_if(is_chaos_bot),
            synthetic_keyboard_bot_system.run_if(is_synthetic_keyboard_bot),
            #[cfg(feature = "pointer-input")]
            pointer_bot::synthetic_pointer_bot_system.run_if(is_synthetic_pointer_bot),
            // Chaos bot's pointer half: seeded UI-node clicks so the
            // battery exercises menu-style games the keyboard surface
            // cannot drive.
            #[cfg(feature = "pointer-input")]
            pointer_bot::chaos_pointer_click_system.run_if(is_chaos_bot),
            #[cfg(feature = "pointer-input")]
            pointer_bot::headless_camera_target_info_system.run_if(pointer_bot_needs_camera_info),
            frame_timing_oracle_system,
            frozen_world_oracle_system,
            crate::invariant_eval::evaluate_invariants_system,
            crate::oracles::nan_oracle_system,
            crate::oracles::bounds_oracle_system,
            crate::replay::fingerprint_system.run_if(verify_replay_enabled),
        ),
    );

    let total_ticks = (scenario.duration_s * tps as f32).ceil() as u64;
    let start = std::time::Instant::now();

    let loop_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        for i in 0..total_ticks {
            sinks::set_current_frame(i);
            app.update();
            if let Some(mut state) = app.world_mut().get_resource_mut::<RunState>() {
                state.frame += 1;
            }
        }
    }));
    let crash_detected = loop_result.is_err();

    let wall_time_s = start.elapsed().as_secs_f64();

    let state = app
        .world_mut()
        .remove_resource::<RunState>()
        .unwrap_or_default();
    let choices = app
        .world_mut()
        .remove_resource::<RunChoices>()
        .map(|c| c.0.recorded)
        .unwrap_or_default();
    let violations = app
        .world_mut()
        .remove_resource::<RunViolations>()
        .map(|v| v.0)
        .unwrap_or_default();

    let drained = sinks::drain();
    let observed_info = sinks::drain_observed_info();
    // Coverage v1: log-based observation. A declared observation is
    // "observed" when any captured log/panic message contains the
    // declared name (digit-normalized on both sides per the sinks
    // dedup convention). Observation is opt-in by logging convention.
    let normalized_messages: Vec<String> = drained
        .iter()
        .map(|c| match c {
            sinks::Captured::Log { message, .. } => normalize_digits(message),
            sinks::Captured::Panic { message, .. } => normalize_digits(message),
        })
        .chain(observed_info.iter().map(|m| normalize_digits(m)))
        .collect();
    let mut snap = violations.snapshot();
    // Drain panics/logs captured during the run (including the one
    // that unwound the loop itself) and merge into the snapshot.
    snap.extend(sinks::to_violation_entries(
        drained,
        scenario.strict_warnings,
    ));
    snap.sort_by(|a, b| {
        (a.first_frame, &a.rule, &a.target).cmp(&(
            b.first_frame,
            &b.rule.clone(),
            &b.target.clone(),
        ))
    });

    let gating = snap
        .iter()
        .filter(|v| !is_informational(&v.rule, scenario.strict_warnings))
        .count();
    let mut status = if crash_detected {
        PlaytestStatus::Crash
    } else if gating == 0 {
        PlaytestStatus::Pass
    } else {
        PlaytestStatus::Fail
    };

    let mut metrics = state.metrics;
    metrics.frame_count = state.frame;
    metrics.crash_detected = crash_detected;
    metrics.worst_frame_ms = state.frame_timing.worst_ms;

    // Vacuity guard: if the whole run never had a Transform entity the
    // oracles could check, a Pass proves nothing about the game (bot
    // inputs hitting a void are unobservable activity). Flag loudly.
    // Tracked DURING the run (any-frame observability), not post-hoc:
    // a game whose entities despawn before the end is NOT vacuous.
    let entities_observable = state.saw_transform_entity;
    if status == PlaytestStatus::Pass && !entities_observable {
        snap.push(crate::state::ViolationEntry {
            rule: Rule::VacuousRun.name().to_string(),
            target: "run".to_string(),
            first_frame: 0,
            last_frame: state.frame,
            count: 1,
            detail: "no entities to observe — the run is vacuous; spawn \
                     entities with a Transform so oracles have something to check"
                .to_string(),
            last_detail: "no entities to observe — the run is vacuous".to_string(),
        });
        status = PlaytestStatus::Fail;
    }

    // Per-invariant vacuity guard: an invariant that never had a single
    // entity to check proves nothing. Same any-frame criterion as
    // vacuous_run; reported per invariant name so the offender is named.
    if !entities_observable {
        for inv in &scenario.invariants {
            snap.push(crate::state::ViolationEntry {
                rule: Rule::VacuousInvariant.name().to_string(),
                target: inv.name.clone(),
                first_frame: 0,
                last_frame: state.frame,
                count: 1,
                detail: format!(
                    "invariant '{}' matched zero entities over the whole run; \
                     the check proves nothing until there is something to check",
                    inv.name
                ),
                last_detail: format!("invariant '{}' matched zero entities", inv.name),
            });
        }
        if !scenario.invariants.is_empty() && status == PlaytestStatus::Pass {
            status = PlaytestStatus::Fail;
        }
    }

    // Action log: one entry per emitted input press, so a report's claim
    // of bot activity is auditable.
    let action_log = choices
        .iter()
        .map(|c| crate::state::ActionEntry {
            frame: c.frame,
            source: "bot".to_string(),
            action: format!("key_press:{}", c.value),
            details: None,
        })
        .collect();

    // Coverage: declared observations (log-based) + chaos surface exercise.
    let coverage = if !scenario.observes.is_empty() || scenario.bot.raw_surface.is_some() {
        let fallback = ["KeyW", "KeyA", "KeyS", "KeyD"];
        let declared_keys: Vec<&str> = match &scenario.bot.raw_surface {
            Some(s) => s.keys.iter().map(String::as_str).collect(),
            _ => fallback.to_vec(),
        };
        let drawn_keys: std::collections::HashSet<String> = choices
            .iter()
            .filter_map(|c| {
                let idx = c.bound as usize;
                if idx < declared_keys.len() {
                    Some(crate::scenario::normalize_key_name(declared_keys[idx])?.to_string())
                } else {
                    None
                }
            })
            .collect();
        let unexercised: Vec<String> = declared_keys
            .iter()
            .filter(|k| {
                let norm = crate::scenario::normalize_key_name(k);
                norm.is_none() || !drawn_keys.contains(norm.unwrap())
            })
            .map(|k| k.to_string())
            .collect();
        let surface_coverage = if declared_keys.is_empty() {
            1.0
        } else {
            (declared_keys.len() - unexercised.len()) as f32 / declared_keys.len() as f32
        };
        let mut observations: Vec<crate::state::ObservationRecord> = scenario
            .observes
            .iter()
            .map(|name| {
                let norm_name = normalize_digits(name);
                let observed = normalized_messages.iter().any(|m| m.contains(&norm_name));
                crate::state::ObservationRecord {
                    name: name.clone(),
                    observed,
                }
            })
            .collect();
        observations.sort_by(|a, b| a.name.cmp(&b.name));
        let holes = observations.iter().filter(|o| !o.observed).count();
        if scenario.strict_coverage && holes > 0 {
            snap.push(crate::state::ViolationEntry {
                rule: crate::rules::Rule::CoverageHole.name().to_string(),
                target: "coverage".to_string(),
                first_frame: 0,
                last_frame: state.frame,
                count: holes as u64,
                detail: format!(
                    "{} declared observation(s) were never observed by any \
                     mechanism; declare logging for these events or wire a hook",
                    holes
                ),
                last_detail: "unobserved declarations".to_string(),
            });
            status = PlaytestStatus::Fail;
        }
        Some(crate::state::Coverage {
            observations,
            unexercised_keys: unexercised,
            surface_coverage,
        })
    } else {
        None
    };

    // Log-capture honesty: if the harness's capture subscriber lost the
    // write-once global default (typically DefaultPlugins' LogPlugin
    // installed first), log_error/log_warn findings are under-reported
    // for this process. Surface it as an informational violation +
    // report field rather than letting a green report imply "no errors
    // were logged". Non-gating by design (see Rule::is_informational):
    // the condition is environmental, not a game bug. The report FIELD
    // always records the truth (machine-readable); the VIOLATION entry
    // is additionally silenced by the acknowledgment env var (human-
    // facing noise).
    let log_capture_available = sinks::subscriber_won();
    if !log_capture_available && std::env::var_os(sinks::ENV_ALLOW_MISSING_LOG_CAPTURE).is_none() {
        snap.push(crate::state::ViolationEntry {
            rule: Rule::LogCaptureLost.name().to_string(),
            target: "world".to_string(),
            first_frame: 0,
            last_frame: 0,
            count: 1,
            detail: "log capture was OFF for this process (another tracing \
                     subscriber won the global default): log_error/log_warn \
                     findings are under-reported"
                .to_string(),
            last_detail: "log capture OFF".to_string(),
        });
    }

    Ok(PlaytestReport {
        schema_version: 1,
        status,
        violations: snap,
        metrics,
        coverage,
        frame_count: state.frame,
        tps: state.tps,
        simulated_duration_s: state.frame as f64 / state.tps as f64,
        wall_time_s,
        error: crash_detected.then(|| "run loop panicked".to_string()),
        action_log,
        cheat_count: scenario.setup.cheats.len(),
        choices,
        harness_mode: "in_process".to_string(),
        scenario_source: None,
        reset_hooks_registered,
        brp_parity: None,
        log_capture_available: Some(log_capture_available),
    })
}

fn is_chaos_bot(scenario: Option<Res<ScenarioResource>>) -> bool {
    scenario.is_some_and(|s| s.0.bot.bot_type == BotType::Chaos)
}

fn is_synthetic_keyboard_bot(scenario: Option<Res<ScenarioResource>>) -> bool {
    scenario.is_some_and(|s| s.0.bot.bot_type == BotType::SyntheticKeyboard)
}

#[cfg(feature = "pointer-input")]
fn is_synthetic_pointer_bot(scenario: Option<Res<ScenarioResource>>) -> bool {
    scenario.is_some_and(|s| s.0.bot.bot_type == BotType::SyntheticPointer)
}

/// The pointer bot needs `Camera.target_info` filled when bevy_render's
/// camera system is absent (headless runs). Runs for any bot that emits
/// pointer input: synthetic_pointer and the chaos bot's pointer half.
#[cfg(feature = "pointer-input")]
fn pointer_bot_needs_camera_info(scenario: Option<Res<ScenarioResource>>) -> bool {
    scenario.is_some_and(|s| matches!(s.0.bot.bot_type, BotType::SyntheticPointer | BotType::Chaos))
}

/// Whether a rule is informational (non-gating). When the scenario sets
/// `strict_warnings`, log warnings escalate to gating failures.
fn is_informational(rule: &str, strict_warnings: bool) -> bool {
    if strict_warnings && rule == "log_warn" {
        return false;
    }
    // Delegate to the registry-derived classification so the two cannot
    // drift (currently only log_warn is informational).
    Rule::from_name(rule).is_some_and(rules::Rule::is_informational)
}
