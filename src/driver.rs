//! In-process driver: runs a scenario against an already-built Bevy App.
//!
//! Panic/log hooks, single-threaded setup, chaos bot,
//! synthetic_keyboard bot, frame-timing oracle, frozen_world stub.
//! The full oracle suite (liveness, bounds, NaN, pass_through) ports
//! incrementally.

use bevy_app::{App, PluginsState, Update};
use bevy_ecs::prelude::*;
use bevy_ecs::schedule::{Schedules, SingleThreadedExecutor};
use bevy_ecs::system::SystemParam;
use bevy_input::keyboard::{KeyCode, KeyboardFocusLost, KeyboardInput};
use bevy_input::{prelude::ButtonInput, ButtonState};
use bevy_math::Vec2;
use bevy_time::{Time, TimeUpdateStrategy};
use bevy_transform::prelude::Transform;
use std::collections::HashMap;
use std::sync::Arc;

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
    // A declared surface is authoritative: an explicitly empty `keys`
    // list opts out of chaos keys (mouse-only chaos). Absent surface =
    // the default set (default-on).
    let keys: Vec<&str> = match &scenario.0.bot.raw_surface {
        Some(surface) => surface.keys.iter().map(String::as_str).collect(),
        None => default_keys.to_vec(),
    };
    if keys.is_empty() {
        return; // chaos keyboard opted out
    }
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
// Mouse bots (chaos + scripted)
// ---------------------------------------------------------------------------

/// Canonical mouse-button name -> MouseButton. Mirrors the keyboard
/// name-normalization layer (lowercase shortcuts + canonical spellings).
fn mouse_button_from_canonical(canonical: &str) -> Option<bevy_input::mouse::MouseButton> {
    use bevy_input::mouse::MouseButton;
    Some(match canonical.to_ascii_lowercase().as_str() {
        "left" => MouseButton::Left,
        "right" => MouseButton::Right,
        "middle" => MouseButton::Middle,
        "back" => MouseButton::Back,
        "forward" => MouseButton::Forward,
        _ => return None,
    })
}

/// Theoretical viewport size used for headless chaos cursor placement.
/// Windows don't exist under the takeover (winit is dropped); chaos
/// positions are normalized to [0,1]² and scaled into this box so clicks
/// land somewhere plausible inside a default-sized viewport.
const HEADLESS_CLICK_EXTENT: f32 = 1280.0;

/// Bundled message writers for the mouse bots: the raw
/// `MouseButtonInput` form (feeds `ButtonInput<MouseButton>`) and the
/// wrapped `bevy_window::WindowEvent` form (what bevy_picking reads).
#[derive(SystemParam)]
struct BotMouseWriters<'w> {
    raw: MessageWriter<'w, bevy_input::mouse::MouseButtonInput>,
    window: MessageWriter<'w, bevy_window::WindowEvent>,
}

impl BotMouseWriters<'_> {
    /// Writes a bot click through BOTH message forms, mirroring
    /// winit's pipeline: the raw `MouseButtonInput` (feeds
    /// `ButtonInput<MouseButton>`) and the wrapped
    /// `bevy_window::WindowEvent` (what bevy_picking's mouse backend
    /// reads; without it, pointer-driven games never see the click).
    fn click(
        &mut self,
        primary_window: Option<Entity>,
        button: bevy_input::mouse::MouseButton,
        btn_state: ButtonState,
    ) {
        let raw = bevy_input::mouse::MouseButtonInput {
            button,
            state: btn_state,
            window: primary_window.unwrap_or(Entity::PLACEHOLDER),
        };
        self.raw.write(raw);
        if primary_window.is_some() {
            self.window
                .write(bevy_window::WindowEvent::MouseButtonInput(raw));
        }
    }

    /// Writes the cursor move through the wrapped `WindowEvent` form
    /// (the only form bevy_picking consumes). `delta` is computed
    /// against the bot's last position, matching winit's shape.
    fn cursor_moved(&mut self, primary_window: Entity, pos: Vec2, last_pos: Option<Vec2>) {
        self.window.write(bevy_window::WindowEvent::CursorMoved(
            bevy_window::CursorMoved {
                window: primary_window,
                position: pos,
                delta: last_pos.map(|last| pos - last),
            },
        ));
    }
}

/// Shared mouse hold state for BOTH the chaos and scripted mouse bots,
/// keyed by button with a hold count. Overlapping holds on the same
/// physical button (e.g. chaos pressing `Left` while a scripted click
/// still holds it) must NOT emit `Released` while another holder is
/// active — `ButtonInput::pressed` would clear prematurely. The
/// `Released` message is emitted only when the LAST holder lets go.
#[derive(Resource, Default)]
struct MouseHoldState {
    /// Outstanding holds per button across all sources (chaos +
    /// scripted). A button leaves this map when its count reaches 0.
    counts: HashMap<bevy_input::mouse::MouseButton, u32>,
    /// Pending chaos hold: one button at a time, matching the chaos
    /// keyboard bot's one-key-at-a-time model. `(button, release_frame)`.
    chaos_pending: Option<(bevy_input::mouse::MouseButton, u64)>,
    /// Pending scripted holds: one entry per outstanding scripted click
    /// `(button, release_frame)`. A Vec, not a single slot — the schema
    /// accepts multiple clicks per frame and clicks scheduled while a
    /// hold is active; every declared click gets a matching release.
    scripted_pending: Vec<(bevy_input::mouse::MouseButton, u64)>,
}

impl MouseHoldState {
    /// Register a press from one source. Emits `true` when the
    /// 0 -> 1 transition happened (the caller writes the `Pressed`
    /// message — which it does unconditionally anyway).
    fn press(&mut self, button: bevy_input::mouse::MouseButton) {
        *self.counts.entry(button).or_insert(0) += 1;
    }

    /// Register a release from one source. Returns `true` when the
    /// last holder let go — only then must the caller write the
    /// `Released` message (an early release would clear another
    /// source's hold on the same button).
    fn release(&mut self, button: bevy_input::mouse::MouseButton) -> bool {
        match self.counts.get_mut(&button) {
            Some(count) if *count > 1 => {
                *count -= 1;
                false
            }
            Some(_) => {
                self.counts.remove(&button);
                true
            }
            None => false, // unbalanced release; nothing to clear
        }
    }
}

/// Cursor position resource: the harness owns the cursor under the
/// takeover (winit, which normally writes it, is dropped). Headless by
/// construction — a plain f32 pair, never an OS event.
#[derive(Resource, Default)]
pub struct CursorInputResource {
    /// Window-space position in logical pixels. None until the first
    /// mouse bot event.
    position: Option<Vec2>,
}

impl CursorInputResource {
    fn set(&mut self, pos: Vec2) {
        self.position = Some(pos);
    }

    /// Current window-space cursor position, if the bot moved it.
    pub fn position(&self) -> Option<Vec2> {
        self.position
    }
}

/// Chaos mouse bot: on input-rate ticks, picks a random surface button,
/// moves the cursor to a random position, presses, and releases after
/// the hold window — mirroring the chaos keyboard bot's scheduling so
/// `mode_parity` semantics stay aligned between modes.
///
/// Draw order per click event: position first (one draw), then button.
/// Position is quantized into a u32 (16 bits per axis) — identical
/// seeds replay identically with no float-formatting drift.
///
/// Delivery goes through BOTH message forms, mirroring winit's real
/// pipeline: the raw `MouseButtonInput` (feeds ButtonInput<MouseButton>)
/// and the wrapped `WindowEvent` (what bevy_picking's mouse backend
/// reads; without it, pointer-driven games never see the click). The
/// cursor move is likewise written as a wrapped `WindowEvent::CursorMoved`
/// targeted at the primary window entity — picking normalizes the
/// event's window against `PrimaryWindow` and silently drops
/// non-primary targets.
fn chaos_mouse_system(
    mut state: ResMut<RunState>,
    scenario: Res<ScenarioResource>,
    mut choices: ResMut<RunChoices>,
    mut hold_state: ResMut<MouseHoldState>,
    mut cursor: ResMut<CursorInputResource>,
    primary_window: Query<(Entity, &bevy_window::Window), With<bevy_window::PrimaryWindow>>,
    mut writers: BotMouseWriters,
) {
    let rate = scenario.0.bot.input_rate_hz.max(1);
    let interval = (state.tps / rate as u64).max(1);
    let hold_frames = 2u64;

    // One (window, size) snapshot per frame; None = no window (pure
    // headless apps) — fall back to the theoretical extent and skip
    // the wrapped WindowEvent forms (nothing consumes them without a
    // window anyway).
    let window = primary_window.single().ok().map(|(entity, w)| {
        (
            entity,
            // Logical extent: scale bot positions INTO the real window
            // so clicks land on-screen (picking ignores off-window hits).
            Vec2::new(w.width(), w.height()),
        )
    });

    // Release held button if its hold duration expired. Only the last
    // outstanding hold on a button emits `Released` — a scripted click
    // holding the same button keeps it pressed.
    if let Some((button, release_at)) = hold_state.chaos_pending {
        if state.frame >= release_at {
            if hold_state.release(button) {
                writers.click(window.map(|(e, _)| e), button, ButtonState::Released);
            }
            hold_state.chaos_pending = None;
            // Cursor stays where it was — hover state persists.
        }
    }

    if hold_state.chaos_pending.is_some() || !state.frame.is_multiple_of(interval) {
        return;
    }

    // Surface: declared mouse_buttons. `Some(surface)` is authoritative:
    // an explicitly empty list opts OUT of chaos mouse (keyboard-only
    // chaos). Absent surface = default three buttons (default-on).
    let buttons: Vec<String> = match &scenario.0.bot.raw_surface {
        Some(surface) => surface.mouse_buttons.clone(),
        None => vec!["Left".into(), "Right".into(), "Middle".into()],
    };
    if buttons.is_empty() {
        return; // chaos mouse opted out
    }

    // Draw order: position first (single u32 draw), then button.
    let pos_draw = choices.0.below_kind(
        state.frame,
        1u64 << 32,
        crate::choice::ChoiceKind::MousePosition,
    );
    let button_idx = choices.0.below_kind(
        state.frame,
        buttons.len() as u64,
        crate::choice::ChoiceKind::MouseButton,
    ) as usize;
    // Divide by 65536, not 65535: the max quantized axis value is
    // 0xFFFF, and dividing by 65535 would yield exactly 1.0 — placing
    // the cursor AT the width/height edge, outside the viewport's
    // half-open bounds ([0, width) x [0, height)). Picking (and any
    // hit test) would drop those clicks even on a fullscreen target.
    let x_norm = (pos_draw >> 16) as f32 / 65536.0;
    let y_norm = (pos_draw & 0xFFFF) as f32 / 65536.0;

    let Some(button_name) = buttons.get(button_idx) else {
        return;
    };
    let Some(button) = mouse_button_from_canonical(button_name) else {
        return;
    };

    // Scale the normalized position into the target viewport: the real
    // window's logical size when it exists (so clicks land ON-SCREEN —
    // picking ignores off-window positions), else the theoretical
    // extent.
    let extent = window
        .map(|(_, size)| size)
        .unwrap_or(Vec2::splat(HEADLESS_CLICK_EXTENT));
    let pos = Vec2::new(x_norm * extent.x, y_norm * extent.y);
    let last_pos = cursor.position();
    cursor.set(pos);
    if let Some((entity, _)) = window {
        writers.cursor_moved(entity, pos, last_pos);
    }
    writers.click(window.map(|(e, _)| e), button, ButtonState::Pressed);
    state.metrics.input_count += 1;
    hold_state.press(button);
    hold_state.chaos_pending = Some((button, state.frame + hold_frames));
}

/// Scripted mouse clicks (`mouse_clicks`): frame-indexed position +
/// button + hold, through the same MouseButtonInput chain. Orthogonal
/// to the bot type — a chaos-keyboard scenario may still script clicks.
/// Shares `MouseHoldState` with the chaos bot so overlapping holds on
/// the same button refcount: `Released` is emitted only when the last
/// holder lets go.
fn scripted_mouse_system(
    mut state: ResMut<RunState>,
    scenario: Res<ScenarioResource>,
    mut hold_state: ResMut<MouseHoldState>,
    mut cursor: ResMut<CursorInputResource>,
    primary_window: Query<Entity, With<bevy_window::PrimaryWindow>>,
    mut writers: BotMouseWriters,
) {
    let window = primary_window.single().ok();
    // Release scripted holds whose duration expired. Only the last
    // outstanding hold on a button emits `Released`.
    let mut i = 0;
    while i < hold_state.scripted_pending.len() {
        let (button, release_at) = hold_state.scripted_pending[i];
        if state.frame >= release_at {
            if hold_state.release(button) {
                writers.click(window, button, ButtonState::Released);
            }
            hold_state.scripted_pending.remove(i);
        } else {
            i += 1;
        }
    }

    for click in &scenario.0.bot.mouse_clicks {
        if click.frame != state.frame {
            continue;
        }
        let Some(button) = mouse_button_from_canonical(&click.button) else {
            continue;
        };
        let pos = Vec2::new(click.x, click.y);
        let last_pos = cursor.position();
        cursor.set(pos);
        if let Some(entity) = window {
            writers.cursor_moved(entity, pos, last_pos);
        }
        writers.click(window, button, ButtonState::Pressed);
        state.metrics.input_count += 1;
        let hold_frames = click.hold_frames.max(1);
        hold_state.press(button);
        hold_state
            .scripted_pending
            .push((button, state.frame + hold_frames));
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
) {
    if !q_all.is_empty() {
        state.saw_transform_entity = true;
    }
    if q.is_empty() {
        state.frozen_frames += 1;
    } else {
        state.frozen_frames = 0;
    }
    // Report once per crossing (guard so we don't spam every frame).
    if state.frozen_frames == state.frozen_grace_frames + 1 {
        violations.0.report(
            Rule::FrozenWorld.name(),
            "world",
            format!(
                "no Transform changed for {} frames — soft-lock?",
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
    app.init_resource::<ButtonInput<KeyCode>>();
    app.init_resource::<ButtonInput<bevy_input::keyboard::Key>>();
    // Mouse messages: same seeding rationale as keyboard — games read
    // ButtonInput<MouseButton>; MinimalPlugins lacks InputPlugin, so the
    // harness registers the message, seeds the resource and runs the
    // event->ButtonInput translation system.
    app.add_message::<bevy_input::mouse::MouseButtonInput>();
    app.init_resource::<ButtonInput<bevy_input::mouse::MouseButton>>();
    // Wrapped window-event forms: bevy_picking's mouse backend reads
    // `WindowEvent::CursorMoved`/`WindowEvent::MouseButtonInput` (what
    // winit writes). The mouse bots write these alongside the raw
    // forms so pointer-driven games (bevy_ui picking) see bot input.
    app.add_message::<bevy_window::WindowEvent>();
    // Order: translate BEFORE game systems in Update so games observe the
    // same-frame state (matching InputPlugin's placement in PreUpdate).
    // ONLY register when the app has no input plugin of its own: a second
    // copy of `keyboard_input_system` in PreUpdate is not harmless —
    // `ButtonInput::clear()` in the second copy wipes the first copy's
    // fresh `just_pressed` state before Update runs. Both copies read the
    // same message queue, so a release+repress batch in one frame
    // self-heals (the re-press re-inserts `just_pressed`) — but an
    // ISOLATED press on a key already in `pressed` is a no-op in the
    // second copy, and the transition is lost. Games polling
    // `just_pressed` — the standard Bevy idiom — then never see isolated
    // presses, silently: `pressed` still works, so movement bots look
    // fine while menu/selection logic is deaf. Guarding on the plugin
    // (rather than probing schedules) keeps MinimalPlugins apps working:
    // the resource seeding above still applies, and the translation
    // systems run exactly once regardless of plugin graph.
    if !app.is_plugin_added::<bevy_input::InputPlugin>() {
        app.add_systems(
            bevy_app::prelude::PreUpdate,
            (
                bevy_input::keyboard::keyboard_input_system,
                bevy_input::mouse::mouse_button_input_system,
            ),
        );
    }
    // Oracles read wall-clock delta; Time may be absent on minimal apps.
    if app.world().get_resource::<Time>().is_none() {
        app.insert_resource(Time::<()>::default());
    }

    // Register bot + oracle systems once.
    app.init_resource::<ChaosHoldState>();
    app.init_resource::<ScheduledHoldState>();
    app.init_resource::<MouseHoldState>();
    app.insert_resource(CursorInputResource::default());

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
            chaos_mouse_system.run_if(is_chaos_bot),
            synthetic_keyboard_bot_system.run_if(is_synthetic_keyboard_bot),
            scripted_mouse_system,
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
    // of bot activity is auditable. Only key draws appear here — mouse
    // draws are not key presses.
    let action_log = choices
        .iter()
        .filter(|c| c.kind == crate::choice::ChoiceKind::Key)
        .map(|c| crate::state::ActionEntry {
            frame: c.frame,
            source: "bot".to_string(),
            action: format!("key_press:{}", c.value),
            details: None,
        })
        .collect();

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
        frame_count: state.frame,
        tps: state.tps,
        simulated_duration_s: state.frame as f64 / state.tps as f64,
        wall_time_s,
        error: crash_detected.then(|| "run loop panicked".to_string()),
        action_log,
        choices,
        harness_mode: "in_process".to_string(),
        scenario_source: None,
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
