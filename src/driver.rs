//! In-process driver: runs a scenario against an already-built Bevy App.
//!
//! v0.1 subset: panic/log hooks, single-threaded setup, chaos bot,
//! synthetic_keyboard bot, frame-timing oracle, frozen_world stub.
//! The full oracle suite (liveness, bounds, NaN, pass_through) ports
//! incrementally.

use bevy_app::{App, PluginsState, Update};
use bevy_ecs::prelude::*;
use bevy_ecs::schedule::{Schedules, SingleThreadedExecutor};
use bevy_input::keyboard::{KeyCode, KeyboardFocusLost, KeyboardInput};
use bevy_input::{prelude::ButtonInput, ButtonState};
use bevy_time::{Time, TimeUpdateStrategy};
use bevy_transform::prelude::Transform;
use std::collections::HashMap;
use std::sync::Arc;

use crate::contract::{CheatHooks, ResetHooks};
use crate::enums::{BotType, PlaytestStatus};
use crate::rules::Rule;
use crate::scenario::{validate_scenario, Scenario, ScenarioError};
use crate::state::PlaytestReport;
use crate::{rules, sinks};

/// Minimum audited actions before the vacuity guard may fire (Z1 rule 3).
pub const VACUITY_MIN_ACTIONS: u64 = 20;

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
// Bot systems (v0.1: chaos + synthetic_keyboard)
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

/// Chaos bot: emits random raw key presses. v0.1 uses the scenario's
/// raw_surface (or the default WASD+arrows surface) — it does not yet
/// consult an IntentSurface. Keys are held for `hold_frames` frames before
/// release to avoid the press+release-in-same-frame cancellation bug.
#[derive(Resource, Default)]
struct ChaosHoldState {
    /// If Some((key_code, release_frame)), this key is currently held.
    held: Option<(bevy_input::keyboard::KeyCode, u64)>,
}

fn chaos_bot_system(
    state: Res<RunState>,
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
    state: Res<RunState>,
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
        hold_state.held.push((code, state.frame + hold_frames));
    }
}

// ---------------------------------------------------------------------------
// Oracles (v0.1: frame timing, frozen_world)
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
) {
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
    pre_ready_frames: u64,
}

impl Default for RunState {
    fn default() -> Self {
        Self {
            frame: 0,
            tps: 60,
            metrics: Default::default(),
            frame_timing: Default::default(),
            frozen_frames: 0,
            frozen_grace_frames: 180, // 3s at 60 TPS — short pauses OK, true freezes caught
            pre_ready_frames: 0,
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
/// The app must contain the harness plugin (added via `PlaytestPlugin`).
/// Returns a structured report with violations, metrics, and action log.
pub fn run_scenario(app: &mut App, scenario: &Scenario) -> Result<PlaytestReport, ScenarioError> {
    validate_scenario(scenario)?;

    // Install global tracing subscriber (captures ERROR/WARN logs into the
    // run's violation buffer). The panic hook is installed separately below.
    sinks::ensure_global_subscriber();
    install_panic_hook();

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
    app.insert_resource(ScenarioResource(Arc::new(scenario.clone())));
    app.insert_resource(RunState {
        tps,
        ..Default::default()
    });
    app.insert_resource(RunViolations::default());
    app.insert_resource(RunChoices(crate::choice::ChoiceStream::from_seed(
        scenario.bot.seed,
    )));

    // Resets: validate against registered hooks, then invoke (inside a
    // panic boundary — a panicking hook is reported, not fatal).
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
    // cheat scheduler; v0.1 applies setup.cheats immediately pre-loop).
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

    // Liveness oracle: always on (grace from scenario liveness config).
    app.insert_resource(crate::oracles::LivenessArmed {
        grace_frames: (scenario.liveness.timeout_s * scenario.tps as f32) as u64,
    });
    app.init_resource::<crate::oracles::LivenessSignal>();

    app.insert_resource(crate::invariant_eval::ScenarioInvariants(
        scenario.invariants.clone(),
    ));

    app.add_systems(
        Update,
        (
            chaos_bot_system.run_if(is_chaos_bot),
            synthetic_keyboard_bot_system.run_if(is_synthetic_keyboard_bot),
            frame_timing_oracle_system,
            frozen_world_oracle_system,
            crate::oracles::liveness_oracle_system,
            crate::invariant_eval::evaluate_invariants_system,
            crate::oracles::nan_oracle_system,
            crate::oracles::bounds_oracle_system,
        ),
    );

    let total_ticks = (scenario.duration_s * tps as f32).ceil() as u64;
    let start = std::time::Instant::now();

    sinks::set_capture_active(true);
    let loop_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        for i in 0..total_ticks {
            sinks::set_current_frame(i);
            app.update();
            if let Some(mut state) = app.world_mut().get_resource_mut::<RunState>() {
                state.frame += 1;
            }
        }
    }));
    sinks::set_capture_active(false);
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

    let mut snap = violations.snapshot();
    // Z7: drain panics/logs captured during the run (including the one
    // that unwound the loop itself) and merge into the snapshot.
    snap.extend(sinks::to_violation_entries(
        sinks::drain(),
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
    let status = if crash_detected {
        PlaytestStatus::Crash
    } else if gating == 0 {
        PlaytestStatus::Pass
    } else {
        PlaytestStatus::Fail
    };

    let mut metrics = state.metrics;
    metrics.frame_count = state.frame;

    Ok(PlaytestReport {
        schema_version: 1,
        status,
        violations: snap,
        metrics,
        coverage: Default::default(),
        frame_count: state.frame,
        tps: state.tps,
        simulated_duration_s: state.frame as f64 / state.tps as f64,
        wall_time_s,
        pre_ready_frames: state.pre_ready_frames,
        error: crash_detected.then(|| "run loop panicked".to_string()),
        action_log: vec![],
        game_version: None,
        cheat_count: 0,
        unreplayable_actions: 0,
        warnings: vec![],
        action_effect_rate: HashMap::new(),
        choices,
        robustness: Default::default(),
        scope: Default::default(),
        harness_mode: "in_process".to_string(),
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
    matches!(
        rule,
        "frame_time_anomaly" | "frame_time_p99_below" | "fps_floor" | "log_warn"
    )
}
