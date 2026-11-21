//! In-process driver: runs a scenario against an already-built Bevy App.
//!
//! Panic/log hooks, single-threaded executor, chaos + synthetic
//! keyboard/mouse bots, frame-timing oracle, frozen-world (soft-lock)
//! oracle with an optional UI-liveness watcher (`ui-liveness`). The
//! bounds/NaN transform oracles live in [`crate::oracles`]; the full
//! rule-name registry in [`crate::rules`].

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

/// Derives `logical_key` for a key code the way winit would on a US
/// QWERTY layout — the same assumption the `KeyCode` names themselves
/// make. Non-printable keys get their named `Key` variant. When
/// `shift_held` is true, letter keys map to their uppercase `Character`
/// variant, matching winit's modifier-aware logical key.
///
/// This matters for consumers like bevy_text's editable-text path,
/// which derives `TextEdit::Insert` from `KeyboardInput::logical_key`:
/// without a `Key::Character` logical key, text inputs are deaf to bot
/// typing even though `ButtonInput<KeyCode>` sees the press.
fn logical_key_for_code(code: KeyCode, shift_held: bool) -> bevy_input::keyboard::Key {
    use bevy_input::keyboard::{Key, KeyCode as KC};

    // Letter keys: case follows Shift (winit resolves the level before
    // reporting the logical key).
    if let Some(i) = KEY_CODE_LETTERS.iter().position(|&k| k == code) {
        let base = if shift_held {
            (b'a' + i as u8).to_ascii_uppercase()
        } else {
            b'a' + i as u8
        };
        return Key::Character((base as char).to_string().into());
    }

    match code {
        KC::Space => Key::Space,
        KC::Enter => Key::Enter,
        KC::Tab => Key::Tab,
        KC::Backspace => Key::Backspace,
        KC::ArrowUp => Key::ArrowUp,
        KC::ArrowDown => Key::ArrowDown,
        KC::ArrowLeft => Key::ArrowLeft,
        KC::ArrowRight => Key::ArrowRight,
        KC::Escape => Key::Escape,
        KC::ShiftLeft | KC::ShiftRight => Key::Shift,
        KC::ControlLeft | KC::ControlRight => Key::Control,
        KC::AltLeft | KC::AltRight => Key::Alt,
        KC::SuperLeft | KC::SuperRight => Key::Super,
        KC::Digit0 => Key::Character("0".into()),
        KC::Digit1 => Key::Character("1".into()),
        KC::Digit2 => Key::Character("2".into()),
        KC::Digit3 => Key::Character("3".into()),
        KC::Digit4 => Key::Character("4".into()),
        KC::Digit5 => Key::Character("5".into()),
        KC::Digit6 => Key::Character("6".into()),
        KC::Digit7 => Key::Character("7".into()),
        KC::Digit8 => Key::Character("8".into()),
        KC::Digit9 => Key::Character("9".into()),
        _ => KEY_UNIDENTIFIED.clone(),
    }
}

/// Chaos bot: emits random raw key presses. Uses the scenario's
/// raw_surface (or the default WASD+arrows surface) — it does not yet
/// consult an IntentSurface. Up to [`CHAOS_HOLD_SLOTS`] keys are held
/// concurrently, each for a per-press drawn duration, so the bot can
/// form chords (accelerate + steer) and exercise charge/grind-style
/// hold mechanics instead of typing one key at a time.
///
/// The draw selects among surface keys not currently held — a physical
/// user cannot press a key they are already holding — so every key
/// draw delivers a real press. Expiry is nonetheless last-holder-
/// releases (see [`ChaosHoldState::release`]): with two slots the
/// `Released` message for a key fires only when no slot still holds it.
/// (Chaos and the scripted keyboard bot are mutually exclusive per
/// scenario, so slot-vs-slot is the only collision case.) All holds
/// deliver at least 2 frames so `ButtonInput` sees `Pressed` for at
/// least one full frame.
#[derive(Resource, Default)]
struct ChaosHoldState {
    /// Active holds: `(key_code, release_frame)`, one entry per occupied
    /// slot. Kept sorted by `release_frame` so expiry is a pop-front.
    held: Vec<(bevy_input::keyboard::KeyCode, u64)>,
}

/// Concurrent chaos key-hold slots.
const CHAOS_HOLD_SLOTS: usize = 2;

/// Draw bounds for chaos hold durations, INCLUSIVE frame counts:
/// delivered holds span `MIN..=MAX` frames (2..=30).
const CHAOS_HOLD_MIN_FRAMES: u64 = 2;
const CHAOS_HOLD_MAX_FRAMES: u64 = 30;

impl ChaosHoldState {
    /// Whether any slot currently holds `code`.
    fn is_held(&self, code: bevy_input::keyboard::KeyCode) -> bool {
        self.held.iter().any(|&(c, _)| c == code)
    }

    /// Register that one holder of `code` let go (the entry was already
    /// removed from `held`). Returns `true` when no other slot still
    /// holds the key — only then may the caller write the `Released`
    /// message (an early release would clear another slot's hold on
    /// the same key). The draw never selects a held key, so this
    /// guards a collision that cannot happen today — kept because it
    /// is the invariant `MouseHoldState` upholds on the mouse side,
    /// and cheap defense-in-depth if a future policy (or a larger K)
    /// lets two slots share a key.
    fn release(&mut self, code: bevy_input::keyboard::KeyCode) -> bool {
        !self.held.iter().any(|&(c, _)| c == code)
    }

    /// Register a new hold in a free slot, keeping `held` sorted by
    /// `release_frame` so expiry is a front-pop. Same-key re-presses
    /// land in a separate slot entry — `release` handles the
    /// last-holder-releases rule.
    fn press(&mut self, code: bevy_input::keyboard::KeyCode, release_at: u64) {
        let pos = self
            .held
            .iter()
            .position(|&(_, at)| release_at < at)
            .unwrap_or(self.held.len());
        self.held.insert(pos, (code, release_at));
    }
}

#[allow(clippy::too_many_arguments)] // a Bevy system: one parameter per resource
fn chaos_bot_system(
    mut state: ResMut<RunState>,
    scenario: Res<ScenarioResource>,
    mut choices: ResMut<RunChoices>,
    mut hold_state: ResMut<ChaosHoldState>,
    mut explorer: ResMut<crate::explorer::Explorer>,
    ui_targets: Res<UiClickTargets>,
    primary_window: Query<Entity, With<bevy_window::PrimaryWindow>>,
    mut keyboard: BotKeyboardWriters,
) {
    let rate = scenario.0.bot.input_rate_hz.max(1);
    let interval = (state.tps / rate as u64).max(1);

    // Release expired holds, oldest first. `release` reports whether
    // any OTHER slot still holds the key — only the LAST holder's
    // expiry writes the key's `Released` message.
    //
    // Ordering note: expiry runs BEFORE the candidate computation, so
    // a key expiring on a rate tick immediately re-ents the candidate
    // pool and can be re-drawn the SAME frame — the batch then carries
    // Released(W) followed by Pressed(W). That is a physically
    // realizable ultra-fast retap, and the chaos draw order (release
    // then press) keeps the pair from cancelling (net `pressed` stays
    // true), but a game branching on `just_released` observes a key-up
    // the same frame the key is (again) down. Intentional; pinned by
    // `chaos_same_frame_retap_observes_both_edges` in driver_smoke.
    while hold_state
        .held
        .first()
        .is_some_and(|&(_, release_at)| state.frame >= release_at)
    {
        let (code, _) = hold_state.held.remove(0);
        if hold_state.release(code) {
            keyboard.key_event(code, ButtonState::Released, primary_window.single().ok());
            if explorer.enabled {
                explorer.note_key_release(state.frame);
            }
        }
    }

    // `input_rate_hz` still governs press opportunities: one chance per
    // rate tick, taken only when a slot is free.
    if !state.frame.is_multiple_of(interval) {
        return;
    }
    // Explore mode: the keyboard presses on its own tick only (see
    // `explorer::HIT_CREDIT_FRAMES`), or borrows the mouse's tick while a
    // drag holds the mouse (declared keys only then: a probe could never
    // be credited). Without a mouse, every tick is the keyboard's.
    let mouse_off = scenario
        .0
        .bot
        .raw_surface
        .as_ref()
        .is_some_and(|s| s.mouse_buttons.is_empty());
    let mut borrowed = false;
    if explorer.enabled {
        explorer.set_cadence(interval);
        if !mouse_off && !crate::explorer::Explorer::keyboard_turn(state.frame, interval) {
            borrowed = crate::explorer::Explorer::mouse_turn(state.frame, interval)
                && explorer.mouse_busy(state.frame);
            if !borrowed {
                return;
            }
        }
    }
    if hold_state.held.len() >= CHAOS_HOLD_SLOTS {
        return;
    }
    // The absent-surface default is BACKBONE_KEYS — the fallback
    // the source-scan layering contract REPLACES when the scan has
    // hits (the parent rewrites the scenario; the driver itself
    // never decides between the arms). A declared surface is
    // authoritative: an explicitly empty `keys` list opts out of
    // chaos keys (mouse-only chaos).
    let keys: Vec<&str> = match &scenario.0.bot.raw_surface {
        Some(surface) => surface.keys.iter().map(String::as_str).collect(),
        None => crate::scenario::BACKBONE_KEYS.to_vec(),
    };
    if keys.is_empty() {
        return; // chaos keyboard opted out
    }
    // Candidates exclude surface keys a slot currently holds: a
    // physical user cannot press a key they are already holding down.
    // Drawing only among unheld keys guarantees every key draw
    // DELIVERS a press — the action log derives its entries from key
    // draws, so a drawn-but-skipped press would fabricate an audit
    // entry — and prevents the physically-impossible
    // press-press-release ordering (double `just_pressed` fire). The
    // draw bound shrinks as slots occupy surface keys and is recorded
    // per draw, so replay stays faithful.
    //
    // Explore mode draws from the explorer's pool instead: the declared
    // surface plus prior keys untried on this screen plus keys that were
    // followed by novelty (see `Explorer::key_pool`).
    let explore_pool: Vec<String> = if explorer.enabled {
        explorer.key_pool(&keys)
    } else {
        Vec::new()
    };
    let source: Vec<&str> = if explorer.enabled {
        explore_pool.iter().map(String::as_str).collect()
    } else {
        keys.clone()
    };
    let candidates: Vec<&str> = source
        .iter()
        .filter_map(|&k| {
            let canonical = crate::scenario::normalize_key_name(k)?;
            let code = keycode_from_canonical(canonical)?;
            (!hold_state.is_held(code)).then_some(canonical)
        })
        .collect();
    if candidates.is_empty() {
        return; // every surface key is held (slots >= surface size)
    }
    // Explore mode, stuck: nothing new for a few seconds, so back out
    // with Escape (a deliberate press, unlike the blind chaos surface
    // which never contains Escape). Recorded as a one-way Key draw so
    // the action log names it.
    if explorer.enabled && explorer.stuck(state.frame, state.tps) {
        if let Some(escape) = keycode_from_canonical("Escape") {
            if !hold_state.is_held(escape) {
                choices
                    .0
                    .below_kind(state.frame, 1, crate::choice::ChoiceKind::Key);
                choices.0.label_last("Escape".to_string());
                keyboard.key_event(escape, ButtonState::Pressed, primary_window.single().ok());
                state.metrics.input_count += 1;
                hold_state.press(escape, state.frame + CHAOS_HOLD_MIN_FRAMES);
                explorer.backed_out(state.frame);
                return;
            }
        }
    }
    // Explore mode, a screen with click targets: probes (prior keys,
    // combos) only every other keyboard turn, so the mouse keeps enough
    // turns there (a resting turn with nothing declared to press goes
    // to the mouse). Declared and productive keys are not affected.
    // Every other keyboard turn rests (when there is a mouse to take the
    // turn): keys tried a few times on this screen without effect stand
    // down, so a keyboard surface the game ignores (mines gets WASD +
    // arrows by default) does not halve the mouse's clicks for the whole
    // run. On screens with click targets, probes stand down too.
    let resting = explorer.enabled
        && !mouse_off
        && crate::explorer::Explorer::probe_rest(state.frame, interval);
    let ui_screen = !ui_targets.centers.is_empty();
    let probe_rest = resting || borrowed;
    // A combo due on a turn without probes waits for the next one
    // (pressing a single key instead would move the combo pacing on).
    if probe_rest && explorer.combo_due() {
        return;
    }
    let candidates: Vec<&str> = if borrowed {
        candidates
            .into_iter()
            .filter(|k| !explorer.is_probe_key(k))
            .collect()
    } else if resting {
        candidates
            .into_iter()
            .filter(|k| explorer.is_fresh_key(k) && !(ui_screen && explorer.is_probe_key(k)))
            .collect()
    } else {
        candidates
    };
    if candidates.is_empty() {
        return;
    }
    // Explore mode, combo turn: hold a modifier under a declared or
    // productive key (Ctrl+A, Shift+L). A combo needs both hold slots,
    // so the turn waits for them to be free; the decision depends only
    // on presses delivered, so replays wait the same way.
    if explorer.enabled && !probe_rest && explorer.combo_due() {
        let mut combos: Vec<String> = Vec::new();
        for base in explorer.combo_bases() {
            let Some(base) = crate::scenario::normalize_key_name(&base) else {
                continue;
            };
            if keycode_from_canonical(base).is_none() || crate::explorer::is_modifier(base) {
                continue;
            }
            for modifier in crate::explorer::COMBO_MODIFIERS {
                combos.push(format!("{modifier}+{base}"));
            }
        }
        combos.sort();
        combos.dedup();
        if !combos.is_empty() {
            if !hold_state.held.is_empty() {
                return;
            }
            let names: Vec<&str> = combos.iter().map(String::as_str).collect();
            let pool = explorer.least_tried("combo", &names);
            let name = names[pool[choices.0.below_kind(
                state.frame,
                pool.len() as u64,
                crate::choice::ChoiceKind::KeyCombo,
            ) as usize]];
            choices.0.label_last(name.to_string());
            explorer.record("combo", name);
            explorer.note_key_press(name, state.frame);
            let codes = name
                .split_once('+')
                .and_then(|(m, k)| Some((keycode_from_canonical(m)?, keycode_from_canonical(k)?)));
            let Some((modifier, key)) = codes else {
                return;
            };
            // Drawn as for any press (replay keeps its shape), but a
            // combo is a probe: tapped, so its release stays inside the
            // keyboard's turn.
            let _ = choices.0.below_kind(
                state.frame,
                CHAOS_HOLD_MAX_FRAMES - CHAOS_HOLD_MIN_FRAMES + 1,
                crate::choice::ChoiceKind::KeyHoldFrames,
            );
            let hold_frames = CHAOS_HOLD_MIN_FRAMES;
            // Modifier first, key second, same frame: by the game's
            // Update both are down and the key is `just_pressed`. The
            // key lets go one frame before the modifier, as hands do.
            let window = primary_window.single().ok();
            keyboard.key_event(modifier, ButtonState::Pressed, window);
            keyboard.key_event(key, ButtonState::Pressed, window);
            state.metrics.input_count += 2;
            // Both released on the same frame (key first, then the
            // modifier: same-frame entries keep insertion order), right
            // after the press, inside the keyboard's turn.
            let release_at =
                crate::explorer::Explorer::release_frame(state.frame + hold_frames, interval);
            hold_state.press(key, release_at);
            hold_state.press(modifier, release_at);
            return;
        }
    }
    // Explore mode: draw among the least-tried keys on this screen
    // (uniform draw over the tied set, recorded like any key draw).
    let pool: Vec<usize> = if explorer.enabled {
        explorer.offer_keys(&candidates);
        explorer.least_tried("key", &candidates)
    } else {
        (0..candidates.len()).collect()
    };
    let idx = pool[choices.0.below_kind(
        state.frame,
        pool.len() as u64,
        crate::choice::ChoiceKind::Key,
    ) as usize];
    if explorer.enabled {
        explorer.record("key", candidates[idx]);
        explorer.note_key_press(candidates[idx], state.frame);
    }
    // Resolve the index NOW and label the draw this frame: `value`
    // indexes the unheld-candidate list at draw time, which the report
    // does not contain — the label is what lets the action log name a
    // real key.
    choices.0.label_last(candidates[idx].to_string());
    let Some(code) = keycode_from_canonical(candidates[idx]) else {
        return;
    };
    // Per-press hold duration, drawn AFTER the key index so both draws
    // land at the same frame (mouse bot precedent: position + button at
    // one frame). Value is reduced in [0, 28]; delivered hold is 2..=30
    // frames. Tagged KeyHoldFrames, NOT Key — the action log filters
    // on `ChoiceKind::Key`, and a duration draw is not a key press.
    let hold_draw = choices.0.below_kind(
        state.frame,
        CHAOS_HOLD_MAX_FRAMES - CHAOS_HOLD_MIN_FRAMES + 1,
        crate::choice::ChoiceKind::KeyHoldFrames,
    );
    // Explore mode: a probe key (prior, not yet productive) is tapped, so
    // its release stays inside the keyboard's turn.
    let hold_frames = if explorer.enabled && explorer.is_probe_key(candidates[idx]) {
        CHAOS_HOLD_MIN_FRAMES
    } else {
        CHAOS_HOLD_MIN_FRAMES + hold_draw
    };
    // Dual-form emission mirrors winit's pipeline: raw `KeyboardInput`
    // plus wrapped `WindowEvent::KeyboardInput`, with `logical_key`
    // derived for the key code (US QWERTY assumption, same as the
    // `KeyCode` names) and `text` set on printable presses.
    keyboard.key_event(code, ButtonState::Pressed, primary_window.single().ok());
    state.metrics.input_count += 1;
    // Explore mode: a release is key activity too, so it is moved to
    // just after a keyboard tick, never near the mouse's turn.
    let release_at = if explorer.enabled {
        crate::explorer::Explorer::release_frame(state.frame + hold_frames, interval)
    } else {
        state.frame + hold_frames
    };
    hold_state.press(code, release_at);
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
    primary_window: Query<Entity, With<bevy_window::PrimaryWindow>>,
    mut keyboard: BotKeyboardWriters,
) {
    // Release any keys whose hold duration expired.
    hold_state.held.retain(|(code, release_at)| {
        if state.frame >= *release_at {
            keyboard.key_event(*code, ButtonState::Released, primary_window.single().ok());
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
        // Dual-form emission mirrors winit's pipeline: raw `KeyboardInput`
        // plus wrapped `WindowEvent::KeyboardInput`, with `logical_key`
        // derived for the key code and `text` set on printable presses.
        keyboard.key_event(code, ButtonState::Pressed, primary_window.single().ok());
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

/// Bundled message writers for the keyboard bots: the raw
/// `KeyboardInput` form (feeds `ButtonInput<KeyCode>` / `ButtonInput<Key>`)
/// and the wrapped `bevy_window::WindowEvent` form. Mirrors the mouse-bot
/// dual-form pattern from #50 so consumers of either stream see bot keys.
#[derive(SystemParam)]
struct BotKeyboardWriters<'w> {
    raw: MessageWriter<'w, KeyboardInput>,
    window: MessageWriter<'w, bevy_window::WindowEvent>,
    modifiers: ResMut<'w, KeyboardModifierState>,
}

/// Held-modifier tracking for the bot keyboard pipeline, mirroring the
/// OS-side state winit consults when resolving `logical_key`/`text`.
/// Tracked per physical Shift key so overlapping holds (ShiftLeft still
/// down when ShiftRight releases) don't clear the modifier early.
///
/// This state is owned by the harness rather than read from
/// `ButtonInput<KeyCode>` because the bot writes messages during
/// `Update`; the message→`ButtonInput` translation runs in the next
/// frame's `PreUpdate`, so the resource lags the bot by one frame.
#[derive(Resource, Default)]
struct KeyboardModifierState {
    shift_left_held: bool,
    shift_right_held: bool,
    /// Control, Alt and Super keys currently held (left and right
    /// tracked separately): while any is down, a press types no text.
    command_held: std::collections::BTreeSet<KeyCode>,
}

impl KeyboardModifierState {
    fn shift_held(&self) -> bool {
        self.shift_left_held || self.shift_right_held
    }
}

/// Control, Alt and Super: with one held, winit delivers no `text`.
fn is_command_modifier(code: KeyCode) -> bool {
    matches!(
        code,
        KeyCode::ControlLeft
            | KeyCode::ControlRight
            | KeyCode::AltLeft
            | KeyCode::AltRight
            | KeyCode::SuperLeft
            | KeyCode::SuperRight
    )
}

impl BotKeyboardWriters<'_> {
    /// Writes a bot key event through BOTH message forms, mirroring
    /// winit's pipeline: the raw `KeyboardInput` (feeds
    /// `ButtonInput<KeyCode>` via `keyboard_input_system`) and the
    /// wrapped `bevy_window::WindowEvent::KeyboardInput` (what
    /// wrapped-stream consumers read). `logical_key` and `text` are
    /// derived via `logical_key_for_code` (US QWERTY assumption, same
    /// as the `KeyCode` names), with letter case following the tracked
    /// Shift state; text is set only for presses of printable keys
    /// (Character and Space — space is printable in winit's
    /// text-input pipeline), matching winit's shape.
    fn key_event(
        &mut self,
        key_code: KeyCode,
        btn_state: ButtonState,
        primary_window: Option<Entity>,
    ) {
        // Resolve the modifier state the OS would have had at this
        // event, then fold the event itself into the tracked state.
        let shift_held = self.modifiers.shift_held();
        let command_held = !self.modifiers.command_held.is_empty();
        match key_code {
            KeyCode::ShiftLeft => self.modifiers.shift_left_held = btn_state.is_pressed(),
            KeyCode::ShiftRight => self.modifiers.shift_right_held = btn_state.is_pressed(),
            code if is_command_modifier(code) => {
                if btn_state.is_pressed() {
                    self.modifiers.command_held.insert(code);
                } else {
                    self.modifiers.command_held.remove(&code);
                }
            }
            _ => {}
        }
        let logical_key = logical_key_for_code(key_code, shift_held);
        let raw = KeyboardInput {
            key_code,
            logical_key: logical_key.clone(),
            state: btn_state,
            // Ctrl+S is a shortcut, not typing: no text while a
            // Control/Alt/Super key is held (winit's behavior).
            text: match (&logical_key, btn_state) {
                _ if command_held => None,
                (bevy_input::keyboard::Key::Character(s), ButtonState::Pressed) => Some(s.clone()),
                (bevy_input::keyboard::Key::Space, ButtonState::Pressed) => Some(" ".into()),
                _ => None,
            },
            repeat: false,
            window: primary_window.unwrap_or(Entity::PLACEHOLDER),
        };
        self.raw.write(raw.clone());
        if primary_window.is_some() {
            self.window
                .write(bevy_window::WindowEvent::KeyboardInput(raw));
        }
    }
}

/// Theoretical viewport size used for headless chaos cursor placement.
/// Windows don't exist under the takeover (winit is dropped); chaos
/// positions are normalized to [0,1]² and scaled into this box so clicks
/// land somewhere plausible inside a default-sized viewport.
const HEADLESS_CLICK_EXTENT: f32 = 1280.0;

/// Bundled message writers for the mouse bots: the raw
/// `MouseButtonInput` form (feeds `ButtonInput<MouseButton>`), the raw
/// `MouseMotion` form (feeds `AccumulatedMouseMotion`), and the wrapped
/// `bevy_window::WindowEvent` form (what bevy_picking reads).
#[derive(SystemParam)]
struct BotMouseWriters<'w, 's> {
    raw: MessageWriter<'w, bevy_input::mouse::MouseButtonInput>,
    motion: MessageWriter<'w, bevy_input::mouse::MouseMotion>,
    window: MessageWriter<'w, bevy_window::WindowEvent>,
    /// The primary window itself: its cursor position is what bevy_ui's
    /// `ui_focus_system` reads to compute `Interaction` (winit sets it
    /// in a real run; the takeover has no winit).
    primary: Query<'w, 's, &'static mut bevy_window::Window, With<bevy_window::PrimaryWindow>>,
}

impl BotMouseWriters<'_, '_> {
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

    /// Writes the cursor move through BOTH message forms, mirroring
    /// winit's split: the raw `MouseMotion` (what
    /// `accumulate_mouse_motion_system` sums into
    /// `AccumulatedMouseMotion` — Bevy's canonical frame-delta motion
    /// API) and the wrapped `WindowEvent::CursorMoved` (the only form
    /// bevy_picking consumes). Without the raw form, motion-reader
    /// games (camera pan, look-based controls) see NOTHING from the
    /// bot: winit derives `MouseMotion` from device events, and a
    /// headless takeover has no device-event source.
    ///
    /// The raw form is written even WINDOWLESS: `MouseMotion` has no
    /// window field (it is a device event in real winit), so unlike
    /// the wrapped form it needs no primary window — a MinimalPlugins
    /// app still gets motion, matching the seeding block's rationale.
    ///
    /// Parity is approximate, not literal: real winit sources
    /// `MouseMotion` from unconditional device events and
    /// `CursorMoved.delta` from window events, so the two can
    /// disagree; the harness synthesizes both from one logical-px
    /// position stream, so `MouseMotion.delta` and
    /// `CursorMoved.delta` always match. Sound as a takeover model
    /// (same deltas a real OS-level mouse move would produce), but
    /// not what winit literally emits in multi-window or
    /// device-event-only setups.
    ///
    /// Like winit's `CursorMoved.delta`, the motion is `None` when no
    /// prior cursor position existed (first appearance is a
    /// teleport, not a movement).
    fn cursor_moved(&mut self, primary_window: Option<Entity>, pos: Vec2, last_pos: Option<Vec2>) {
        if let Some(delta) = last_pos.map(|last| pos - last) {
            self.motion.write(bevy_input::mouse::MouseMotion { delta });
        }
        if let Some(entity) = primary_window {
            self.window.write(bevy_window::WindowEvent::CursorMoved(
                bevy_window::CursorMoved {
                    window: entity,
                    position: pos,
                    delta: last_pos.map(|last| pos - last),
                },
            ));
        }
        // Third form: the window's own cursor position, which winit
        // updates on every move. bevy_ui's `ui_focus_system` reads ONLY
        // this (`Window::physical_cursor_position`) to compute
        // `Interaction`, so without it no `Interaction`-driven button
        // (bevy_ui `Button`, common despite the 0.20 deprecation) could
        // ever be hovered or pressed under the takeover. This marks the
        // Window `Changed` on every move, exactly as winit does (it sets
        // the position through `&mut Window` on each real cursor move).
        if let Ok(mut window) = self.primary.single_mut() {
            window.set_cursor_position(Some(pos));
        }
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
    /// Pending chaos hold: one button at a time. `(button, release_frame)`.
    chaos_pending: Option<(bevy_input::mouse::MouseButton, u64)>,
    /// Pending scripted holds: one entry per outstanding scripted click
    /// `(button, release_frame)`. A Vec, not a single slot — the schema
    /// accepts multiple clicks per frame and clicks scheduled while a
    /// hold is active; every declared click gets a matching release.
    scripted_pending: Vec<(bevy_input::mouse::MouseButton, u64)>,
    /// Explore-mode drag in progress: the cursor moves from `from` to
    /// `to` while the chaos hold lasts, arriving on frame `end` (the
    /// release frame).
    chaos_drag: Option<ChaosDrag>,
}

#[derive(Clone, Copy)]
struct ChaosDrag {
    from: Vec2,
    to: Vec2,
    start: u64,
    end: u64,
}

/// Frames an explore-mode drag takes from press to release: one short
/// of two default ticks, so the release (a frame with no new press)
/// does not land on the mouse's next tick.
const DRAG_FRAMES: u64 = 11;

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

/// Enumerated UI click targets for the chaos mouse bot
/// (`ui-surface` feature): window-space PHYSICAL-pixel centers of
/// declared interactables, captured after the first frame and
/// RE-MIRRORED on every periodic recapture by the driver loop
/// (never a scheduled system — same census / static-scan neutrality
/// doctrine as the lw capture; see `driver.rs`'s recorder
/// comments). Between recaptures (up to one interval, ~0.5s) the
/// centers lag the world: the bot can briefly aim at a moved or
/// despawned widget's old position and cannot yet aim at a newly
/// opened one. The resource itself is
/// unconditional so the bot's targeting math compiles identically
/// in every build; feature-off builds simply leave `centers`
/// empty, which keeps the legacy targeting path byte-for-byte.
/// Harness bookkeeping is doubly invisible: the digest excludes
/// `bevy_playtest::`-prefixed resources, and insertion happens
/// after the capability-census and static-scan capture points.
#[derive(Resource, Default)]
pub(crate) struct UiClickTargets {
    /// Interactable centers in physical window pixels,
    /// deterministically ordered (entity index — `enumerate_ui`'s
    /// order).
    pub(crate) centers: Vec<Vec2>,
    /// Name and disabled flag of each center, same order (explorer).
    pub(crate) ids: Vec<(String, bool)>,
    /// Whether each center can be dragged, same order.
    pub(crate) draggable: Vec<bool>,
    /// Whether each center accepts drops, same order.
    pub(crate) drop_target: Vec<bool>,
}

/// The node a click on `hovered` should credit, if any. A game handling
/// clicks with one global observer recognizes its targets by one of its
/// own components (`TileCell`, `ButtonAction`), so the credit goes to
/// the hovered node or its nearest ancestor (up to three levels) that
/// carries a game-defined component: a cell's number label credits the
/// cell, and plain labels or layout containers are never learned. When
/// component names are unreadable (bevy `debug` off) the hovered node
/// itself is credited. A node covering half the window or more is
/// layout, never a target.
#[cfg(feature = "ui-surface")]
fn credited_node(world: &World, hovered: Entity, window_area: f32) -> Option<Entity> {
    let mut entity = hovered;
    for _ in 0..=3 {
        let computed = world.get::<bevy_ui::ComputedNode>(entity)?;
        let size = computed.size;
        if window_area > 0.0 && size.x * size.y >= 0.5 * window_area {
            return None;
        }
        match has_game_component(world, entity) {
            Some(true) => return Some(entity),
            None => return Some(hovered),
            Some(false) => {}
        }
        entity = world.get::<ChildOf>(entity)?.parent();
    }
    None
}

/// Whether `entity` carries a component defined outside the engine and
/// the standard library; None when component names are masked.
#[cfg(feature = "ui-surface")]
fn has_game_component(world: &World, entity: Entity) -> Option<bool> {
    let entity_ref = world.get_entity(entity).ok()?;
    let mut game = false;
    for &id in entity_ref.archetype().components() {
        let name = world.components().get_info(id)?.name().to_string();
        if name.starts_with('<') {
            return None;
        }
        if !crate::explorer::is_engine_type_path(&name) {
            game = true;
        }
    }
    Some(game)
}

/// Archetypes of the laid-out UI nodes the mouse pointer hovers.
#[cfg(feature = "ui-surface")]
fn hovered_archetypes(world: &World) -> Vec<Vec<usize>> {
    let Some(hover) = world.get_resource::<bevy_picking::hover::HoverMap>() else {
        return Vec::new();
    };
    let Some(hits) = hover.get(&bevy_picking::pointer::PointerId::Mouse) else {
        return Vec::new();
    };
    let window_area = world
        .iter_entities()
        .find_map(|e| {
            e.get::<bevy_window::PrimaryWindow>()?;
            let w = e.get::<bevy_window::Window>()?;
            Some(w.physical_width() as f32 * w.physical_height() as f32)
        })
        .unwrap_or(0.0);
    let mut entities: Vec<Entity> = hits
        .keys()
        .filter_map(|&e| credited_node(world, e, window_area))
        .collect();
    entities.sort_unstable_by_key(|e| e.index());
    entities.dedup();
    let mut keys: Vec<Vec<usize>> = entities
        .into_iter()
        .filter_map(|e| crate::discovery::ui_surface::archetype_key(world, e))
        .collect();
    keys.sort();
    keys.dedup();
    keys
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
// Eight SystemParams: the standard bot state quintet (state,
// scenario, choices, hold, cursor) + the enumerated UI targets +
// window lookup + message writers. Grouping further would need a
// bespoke SystemParam bundle per system — not worth it for one
// system.
#[allow(clippy::too_many_arguments)]
fn chaos_mouse_system(
    mut state: ResMut<RunState>,
    scenario: Res<ScenarioResource>,
    mut choices: ResMut<RunChoices>,
    mut hold_state: ResMut<MouseHoldState>,
    mut cursor: ResMut<CursorInputResource>,
    ui_targets: Res<UiClickTargets>,
    mut explorer: ResMut<crate::explorer::Explorer>,
    // Entity only: the Window itself is read through `writers.primary`
    // (a second `&Window` query would conflict with its `&mut Window`).
    primary_window: Query<Entity, With<bevy_window::PrimaryWindow>>,
    mut writers: BotMouseWriters,
) {
    let rate = scenario.0.bot.input_rate_hz.max(1);
    let interval = (state.tps / rate as u64).max(1);
    let hold_frames = 2u64;

    // One (window, size, scale) snapshot per frame; None = no window
    // (pure headless apps) — fall back to the theoretical extent and
    // skip the wrapped WindowEvent forms (nothing consumes them
    // without a window anyway).
    let window = primary_window
        .single()
        .ok()
        .zip(writers.primary.single().ok())
        .map(|(entity, w)| {
            (
                entity,
                // Logical extent: scale bot positions INTO the real window
                // so clicks land on-screen (picking ignores off-window hits).
                Vec2::new(w.width(), w.height()),
                // Physical→logical conversion for enumerated widget
                // centers (physical px, see ui_surface.rs).
                w.scale_factor(),
            )
        });

    // Drag in progress: one cursor step per frame (picking needs
    // motion WHILE pressed for DragStart/Drag), reaching `to` on the
    // release frame, before the release below.
    if let Some(drag) = hold_state.chaos_drag {
        if state.frame > drag.start {
            let t = ((state.frame - drag.start) as f32 / (drag.end - drag.start) as f32).min(1.0);
            let pos = drag.from.lerp(drag.to, t);
            let last_pos = cursor.position();
            cursor.set(pos);
            writers.cursor_moved(window.map(|(e, _, _)| e), pos, last_pos);
        }
        if state.frame >= drag.end {
            hold_state.chaos_drag = None;
        }
    }

    // Release held button if its hold duration expired. Only the last
    // outstanding hold on a button emits `Released` — a scripted click
    // holding the same button keeps it pressed.
    let mut released_now = false;
    if let Some((button, release_at)) = hold_state.chaos_pending {
        if state.frame >= release_at {
            if hold_state.release(button) {
                writers.click(window.map(|(e, _, _)| e), button, ButtonState::Released);
            }
            hold_state.chaos_pending = None;
            released_now = true;
            // Cursor stays where it was — hover state persists.
        }
    }
    // Explore mode: no new press on a release frame (a drop's reaction
    // would be credited to the new click).
    if explorer.enabled && released_now {
        return;
    }

    if hold_state.chaos_pending.is_some() || !state.frame.is_multiple_of(interval) {
        return;
    }
    // Explore mode: the mouse presses on its own tick, or takes the
    // keyboard's when the keyboard (which runs first) left it unused.
    let mut own_tick = true;
    if explorer.enabled {
        own_tick = crate::explorer::Explorer::mouse_turn(state.frame, interval);
        let spare = crate::explorer::Explorer::keyboard_turn(state.frame, interval)
            && !explorer.keyboard_acted(state.frame);
        if !own_tick && !spare {
            return;
        }
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
    //
    // UI-target interleave (`ui-surface`): the SAME single draw's
    // lowest bit routes — odd targets an enumerated widget center
    // (selected by the remaining 31 bits over the captured list),
    // even keeps the legacy quantized-random position. Deterministic
    // and replay-safe: `below_kind` stores the REDUCED draw, so the
    // routing bit is part of the recorded value and replays
    // identically. Empty target list (feature off, capture pending,
    // or no declared UI) degenerates to the even path everywhere.
    // ~50% of clicks still explore undeclared regions — counts are
    // annotations, never licenses to prune probe surfaces.
    let extent = window
        .map(|(_, size, _)| size)
        .unwrap_or(Vec2::splat(HEADLESS_CLICK_EXTENT));
    // Explore mode: 3 of 4 clicks go to the least-tried ENABLED widget
    // on this screen (tie broken by the draw's upper bits), 1 of 4 keep
    // the random position so undeclared regions are still probed.
    // Explore mode: every few mouse actions is a drag (often when a
    // node on screen declares drag handling, rarely otherwise, see
    // `Explorer::drag_due`). A drag starts from a draggable node when
    // there is one, else picks its start like a click.
    let enabled: Vec<usize> = (0..ui_targets.centers.len())
        .filter(|&i| ui_targets.ids.get(i).is_some_and(|(_, disabled)| !disabled))
        .collect();
    let draggable: Vec<usize> = enabled
        .iter()
        .copied()
        .filter(|&i| ui_targets.draggable.get(i).copied().unwrap_or(false))
        .collect();
    let drop_targets: Vec<usize> = enabled
        .iter()
        .copied()
        .filter(|&i| ui_targets.drop_target.get(i).copied().unwrap_or(false))
        .collect();
    let is_drag =
        explorer.enabled && explorer.drag_due(!draggable.is_empty() || !drop_targets.is_empty());
    let action = if is_drag { "drag" } else { "click" };
    let starts = if is_drag && !draggable.is_empty() {
        &draggable
    } else {
        &enabled
    };
    let explore_target = if explorer.enabled && !pos_draw.is_multiple_of(4) {
        let names: Vec<&str> = starts
            .iter()
            .map(|&i| ui_targets.ids[i].0.as_str())
            .collect();
        if names.is_empty() {
            None
        } else {
            let tied = explorer.least_tried(action, &names);
            let pick = tied[((pos_draw >> 2) as usize) % tied.len()];
            explorer.record(action, names[pick]);
            // Index into `enabled`, so the drag destination can skip it.
            let start = enabled
                .iter()
                .position(|&i| i == starts[pick])
                .unwrap_or(pick);
            Some((start, ui_targets.centers[starts[pick]]))
        }
    } else {
        None
    };
    let start_widget = explore_target.map(|(pick, _)| pick);
    let pos = if let Some((_, center_physical)) = explore_target {
        let scale = window.map(|(_, _, scale)| scale).unwrap_or(1.0);
        center_physical / scale
    } else if !explorer.enabled && pos_draw % 2 == 1 && !ui_targets.centers.is_empty() {
        // Select the widget by the remaining bits, deterministically:
        // the list is entity-index-ordered (see UiClickTargets).
        let index = ((pos_draw >> 1) as usize) % ui_targets.centers.len();
        let center_physical = ui_targets.centers[index];
        // Centers are physical window px; the cursor is logical px.
        // No window (headless fallback) means no scale factor: the
        // centers were captured against HEADLESS_CLICK_EXTENT-less
        // synthetic viewport geometry and need no conversion.
        let scale = window.map(|(_, _, scale)| scale).unwrap_or(1.0);
        center_physical / scale
    } else {
        Vec2::new(x_norm * extent.x, y_norm * extent.y)
    };
    let last_pos = cursor.position();
    cursor.set(pos);
    // Both forms: raw MouseMotion needs no window (device event in
    // real winit); the wrapped CursorMoved targets a window.
    writers.cursor_moved(window.map(|(e, _, _)| e), pos, last_pos);
    writers.click(window.map(|(e, _, _)| e), button, ButtonState::Pressed);
    state.metrics.input_count += 1;
    hold_state.press(button);
    let mut release_at = state.frame + hold_frames;
    if explorer.enabled {
        explorer.note_mouse_action(is_drag);
    }
    if is_drag {
        // Destination: a second recorded position draw, same frame.
        // 3 of 4: a drop target other than the start when there is one,
        // else another enabled widget (so DragDrop can fire on it);
        // otherwise a random point, as for clicks.
        let to_draw = choices.0.below_kind(
            state.frame,
            1u64 << 32,
            crate::choice::ChoiceKind::MousePosition,
        );
        let scale = window.map(|(_, _, scale)| scale).unwrap_or(1.0);
        let drops: Vec<usize> = (0..enabled.len())
            .filter(|&i| Some(i) != start_widget && drop_targets.contains(&enabled[i]))
            .collect();
        let others: Vec<usize> = if drops.is_empty() {
            (0..enabled.len())
                .filter(|&i| Some(i) != start_widget)
                .collect()
        } else {
            drops
        };
        let to = if !to_draw.is_multiple_of(4) && !others.is_empty() {
            let other = others[((to_draw >> 2) as usize) % others.len()];
            ui_targets.centers[enabled[other]] / scale
        } else {
            Vec2::new(
                (to_draw >> 16) as f32 / 65536.0 * extent.x,
                (to_draw & 0xFFFF) as f32 / 65536.0 * extent.y,
            )
        };
        release_at = state.frame + DRAG_FRAMES;
        hold_state.chaos_drag = Some(ChaosDrag {
            from: pos,
            to,
            start: state.frame,
            end: release_at,
        });
    }
    hold_state.chaos_pending = Some((button, release_at));
    if explorer.enabled {
        // Only a click arms the learning probe: a drag sweeps the cursor
        // over other nodes, whose reactions are not the start node's.
        // Only a click on the mouse's own tick arms the learning probe:
        // a drag sweeps the cursor over other nodes, and a borrowed
        // keyboard tick has key releases landing right after it.
        explorer.note_press(
            state.frame,
            start_widget.is_none() && !is_drag && own_tick,
            release_at,
            is_drag,
        );
    }
}

/// Scripted mouse motion (`mouse_motion`): frame-indexed logical-px
/// deltas, applied for `repeat_frames` frames starting at `frame`.
/// Orthogonal to the bot type like `mouse_clicks`, and to clicks in
/// the same scenario (clicks apply first within a frame — system
/// order in the Update tuple; a same-frame click then moves FROM the
/// click's position, the sequence a real user produces).
///
/// Semantics vs the first appearance: a motion script applies its
/// first delta from the cursor's CURRENT position — the window-space
/// origin (0, 0) when the cursor was previously unset (e.g. a
/// motion-only scenario). Unlike a scripted click (a teleport-in
/// carrying no motion), the delta IS the input here, so the raw
/// `MouseMotion` is delivered on EVERY frame of the window, including
/// the first, and the wrapped `CursorMoved` carries the full delta
/// from the starting position. This mirrors real winit, whose
/// device-event `MouseMotion` is unconditional. Consequence for probe
/// design: probes carry no clicks, so a balanced +d/−d oscillation
/// starts at the origin and returns there — net displacement zero.
/// Overlapping windows are legal: each window containing the current
/// frame applies its delta in entry order, emitting one
/// `MouseMotion` and `CursorMoved` pair per entry — real winit
/// batches multiple device deltas per frame the same way.
///
/// Timing nuance vs real winit: the bot writes `MouseMotion` during
/// `Update`, and `accumulate_mouse_motion_system` (engine or
/// fallback) drains it in `PreUpdate`, so a game reading
/// `AccumulatedMouseMotion` sees a bot delta one frame LATER than
/// under real winit (whose device events arrive before the frame's
/// `PreUpdate`). Probe and control share the timing, so the
/// differential is unaffected.
fn scripted_motion_system(
    mut state: ResMut<RunState>,
    scenario: Res<ScenarioResource>,
    mut cursor: ResMut<CursorInputResource>,
    primary_window: Query<Entity, With<bevy_window::PrimaryWindow>>,
    mut writers: BotMouseWriters,
) {
    let window = primary_window.single().ok();
    for motion in &scenario.0.bot.mouse_motion {
        // Window [frame, frame + repeat_frames): strict bounds so a
        // zero-repeat entry (rejected by validation, but defensive)
        // applies nothing. Saturating add: frame windows are
        // author-controlled u64s and validation only rejects entries
        // starting at/past the run end, not huge spans.
        if state.frame < motion.frame
            || state.frame >= motion.frame.saturating_add(motion.repeat_frames)
        {
            continue;
        }
        let delta = Vec2::new(motion.dx, motion.dy);
        // Park-at-origin on first application (see system docs).
        let last_pos = cursor.position().unwrap_or(Vec2::ZERO);
        let pos = last_pos + delta;
        cursor.set(pos);
        writers.cursor_moved(window, pos, Some(last_pos));
        state.metrics.input_count += 1;
    }
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
        // Both forms: raw MouseMotion needs no window (device event
        // in real winit); the wrapped CursorMoved targets a window.
        writers.cursor_moved(window, pos, last_pos);
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

/// Sorted-bits transform value hash (canonical: equal transform SETS
/// hash equal regardless of entity/spawn order — mirrors the state
/// digest's translation fold). Hashes the FULL Transform — translation
/// AND rotation AND scale bits — so a world rewriting the same
/// translation with rotating/animating orientation or scale cannot
/// counterfeit value-identity. Used by the phantom-write detector to
/// notice that a world which rewrites identical transform values every
/// frame is not changing at all.
fn transform_value_hash<'a>(transforms: impl Iterator<Item = &'a Transform>) -> u64 {
    let mut parts: Vec<[u64; 10]> = transforms
        .map(|t| {
            [
                t.translation.x.to_bits() as u64,
                t.translation.y.to_bits() as u64,
                t.translation.z.to_bits() as u64,
                t.rotation.x.to_bits() as u64,
                t.rotation.y.to_bits() as u64,
                t.rotation.z.to_bits() as u64,
                t.rotation.w.to_bits() as u64,
                t.scale.x.to_bits() as u64,
                t.scale.y.to_bits() as u64,
                t.scale.z.to_bits() as u64,
            ]
        })
        .collect();
    parts.sort_unstable();
    let mut hasher = crate::digest::fnv_hasher();
    hasher = crate::digest::hash_usize(hasher, parts.len());
    for bits in parts {
        for b in bits {
            hasher = crate::digest::hash_u64(hasher, b);
        }
    }
    hasher
}

/// Frozen-world oracle: detects when the world stops changing. Counts
/// consecutive frames with no `Changed<Transform>` anywhere (plus, under
/// the `ui-liveness` feature, no `Changed<UiTransform>`) and reports
/// a `frozen_world` violation once the streak exceeds the grace window.
/// Needs no marker: "anything changed" is the liveness evidence.
///
/// Phantom-write hardening: `Changed<Transform>` firing every frame
/// does NOT prove change — a system can rewrite identical values (the
/// classic churning-but-dead signature). Every
/// [`RunState::PHANTOM_INTERVAL_FRAMES`] frames the oracle hashes the
/// sorted transform VALUES; a bit-identical interval where every frame
/// still flagged `Changed<Transform>` is a PHANTOM INTERVAL, and
/// `phantom_grace_intervals` consecutive phantom intervals report
/// `frozen_world` with a "phantom writes" detail. A softer secondary
/// signal reports "mostly phantom writes" when ≥90% of frames flagged
/// (not necessarily all) and the values stayed bit-identical; its
/// counter resets only on a genuine value change. No streak gating:
/// the phantom regime lives exactly where the frozen streak never
/// builds, so this check is independent of `frozen_frames`.
///
/// Runs after the liveness watchers so same-frame activity is counted.
///
/// Detail refinement: a visually-silent streak is not necessarily a
/// soft-lock. When the broadened state digest ([`crate::digest`]) was
/// recorded AND kept changing through the streak (some reflected game
/// resource mutated, or entities spawned/despawned), the sim was
/// demonstrably stepping and the detail says so — a "soft-lock?" there
/// sends the developer hunting a deadlock that does not exist
/// (field-report case: a match clock ticking in a resource while the
/// presenter never writes Transform/UI). Without digest evidence the
/// honest answer is the legacy "soft-lock?" — the oracle cannot know.
fn frozen_world_oracle_system(
    mut state: ResMut<RunState>,
    mut violations: ResMut<RunViolations>,
    digest: Option<Res<crate::digest::StateDigestRecorder>>,
    // Probe hygiene: all three queries exclude harness-injected
    // entities (the screen-liveness rig's camera). Otherwise the
    // probe's spawn-tick `Added<Transform>` would reset the frozen
    // streak, its mere existence would satisfy
    // `saw_transform_entity`, and its constant pose would join the
    // phantom value-hash — the rig would blind every frozen-world
    // signal. `Without` filters compile in feature-off builds too
    // (the marker type is always defined; no entity carries it).
    q: Query<
        (),
        (
            Changed<Transform>,
            Without<crate::injected::HarnessInjected>,
        ),
    >,
    q_all: Query<(), (With<Transform>, Without<crate::injected::HarnessInjected>)>,
    q_values: Query<&Transform, bevy_ecs::query::Without<crate::injected::HarnessInjected>>,
) {
    if !q_all.is_empty() {
        state.saw_transform_entity = true;
    }
    let transform_activity = !q.is_empty();
    let ui_activity = state.ui_activity_this_frame;
    state.ui_activity_this_frame = false;
    if transform_activity || ui_activity {
        state.frozen_frames = 0;
    } else {
        state.frozen_frames += 1;
    }
    // Report once per crossing (guard so we don't spam every frame).
    if state.frozen_frames == state.frozen_grace_frames + 1 {
        let watched = if cfg!(feature = "ui-liveness") {
            "no Transform or UI change"
        } else {
            "no Transform change"
        };
        // Stepping evidence (tri-state): did the broadened state
        // digest keep changing across the JUST-ENDED streak? Frame
        // alignment: the digest recorder (a driver-loop recorder,
        // invoked after `app.update()` each tick) has, at report tick
        // `frame`, recorded digests for frames 0..=frame-1. Each pair
        // `(d[k-1], d[k])` differs iff digest-visible state changed
        // during tick k. Which ticks are "silent" is decided by THIS
        // oracle's `Changed<Transform>` view: if the game's mutating
        // system runs BEFORE the oracle in Update, the final active
        // tick's change is visible that same tick and the window
        // (`start = len - frozen_frames`, the streak being
        // `frozen_frames` ticks long) starts at the digest whose
        // pair attributes to the FIRST silent tick — genuine stepping
        // evidence. If the game's system runs AFTER the oracle, the
        // final mutation is only visible NEXT tick, the streak starts
        // one tick later, and the change itself sits in the boundary
        // pair that the window EXCLUDES — under-claimed (safe: refusal
        // over silence), never fabricated. In both alignments the
        // window contains no pair that attributes to a tick the
        // oracle counted active; starting one digest FURTHER back is
        // what would fabricate "stepping" from pre-streak activity.
        // Refusals (fail-open honesty): a `stop_after` cutoff that
        // stopped recording before this tick, a too-short window
        // (< 2 comparable digests), and a frame counter that cannot
        // be represented on this target (32-bit `usize`) all yield
        // NO evidence rather than a guess.
        let stepping = digest.as_deref().and_then(|rec| {
            let frame = usize::try_from(state.frame).ok()?;
            let frozen = usize::try_from(state.frozen_frames).ok()?;
            let start = rec.digests.len().saturating_sub(frozen);
            (rec.digests.len() >= frame && rec.digests.len() - start >= 2).then(|| {
                rec.digests[start..]
                    .windows(2)
                    .any(|pair| pair[0].hash != pair[1].hash)
            })
        });
        // Tri-state detail: PROVEN stepping (the digest kept changing
        // through the visually-silent streak) gets the refined wording —
        // a field-reported match-clock tick proved the sim ran while
        // "soft-lock?" sent its developer hunting a deadlock that did
        // not exist. A FLAT digest is weaker evidence than it looks:
        // the digest covers Transforms, spawns, and REFLECTED
        // RESOURCES (plus UiTransforms under the `ui-liveness`
        // feature), but not e.g. sprite-color or material
        // mutations — so it corroborates nothing and the detail keeps
        // the legacy "soft-lock?" wording, with `stepping: false` in
        // `_meta` for machines. No recorded digest ⇒ unknown ⇒ legacy
        // wording, no meta (the battery's discovery annotation can
        // still attach, it only fills `meta == None`).
        let detail = match stepping {
            Some(true) => format!(
                "{watched} for {} frames — stepping but visually silent: \
                 the state digest kept changing while nothing moved, so the \
                 sim is running; presenter layers must write Transform/UI \
                 every frame to be observable",
                state.frozen_frames
            ),
            _ => format!("{watched} for {} frames — soft-lock?", state.frozen_frames),
        };
        violations
            .0
            .report(Rule::FrozenWorld.name(), "world", detail, state.frame);
        // Attach `_meta.stepping` alongside the verbatim detail when
        // (and only when) digest evidence exists; anchored at the
        // FIRST crossing so the entry stays its own reproduction
        // anchor (later crossings only bump count/last_frame).
        if stepping.is_some() {
            if let Some(entry) = violations
                .0
                .entries
                .get_mut(&(Rule::FrozenWorld.name().to_string(), "world".to_string()))
            {
                if entry.meta.is_none() {
                    entry.meta = Some(crate::discovery::surface::ViolationMeta { stepping });
                }
            }
        }
    }

    // ----- Phantom-write detection -----
    // Sample every K frames; within a sample interval count frames
    // where the change-detection flags fired. A PHANTOM interval is
    // one whose value hash equals the previous sample's AND every
    // frame in it flagged Changed<Transform>.
    state.phantom_frames_since_sample += 1;
    if transform_activity {
        state.phantom_changed_frames += 1;
    }
    if state.phantom_frames_since_sample >= RunState::PHANTOM_INTERVAL_FRAMES {
        let hash = transform_value_hash(q_values.iter());
        let frames_in_interval = state.phantom_frames_since_sample;
        let flagged = state.phantom_changed_frames;
        let all_flagged = flagged == frames_in_interval;
        let dominant_flagged = flagged * 10 >= 9 * frames_in_interval;
        let hash_same = state.phantom_last_hash == Some(hash);
        let interval_was_phantom = all_flagged && hash_same;
        // Dominant-phantom interval: ≥90% flagged AND hash unchanged.
        // The hash is the authoritative change signal; flag density is
        // corroborating churn evidence. RESET only on a hash CHANGE
        // (something genuinely moved); a density dip below 90% with an
        // identical hash is still a dead interval.
        let interval_was_dominant_phantom = dominant_flagged && hash_same;
        if interval_was_phantom {
            state.phantom_consecutive_intervals += 1;
        } else {
            state.phantom_consecutive_intervals = 0;
        }
        if interval_was_dominant_phantom {
            state.phantom_dominant_consecutive_intervals += 1;
        } else if !hash_same {
            state.phantom_dominant_consecutive_intervals = 0;
        }
        // Report once per crossing: exactly at the threshold.
        if state.phantom_consecutive_intervals == state.phantom_grace_intervals + 1 {
            violations.0.report(
                Rule::FrozenWorld.name(),
                "world",
                format!(
                    "phantom writes: Changed<Transform> fired on every frame \
                     for {} frames while transform values stayed bit-identical \
                     across every sampled interval — the world is rewriting \
                     itself, not changing",
                    state.phantom_consecutive_intervals * RunState::PHANTOM_INTERVAL_FRAMES
                ),
                state.frame,
            );
        }
        // Secondary crossing: dominant (not every-frame) phantom churn.
        // Only meaningful while the strict counter has NOT fired (the
        // strict report above is the stronger finding for the same
        // underlying regime).
        if state.phantom_dominant_consecutive_intervals == state.phantom_grace_intervals + 1
            && state.phantom_consecutive_intervals <= state.phantom_grace_intervals
        {
            violations.0.report(
                Rule::FrozenWorld.name(),
                "world",
                format!(
                    "mostly phantom writes: Changed<Transform> fired on ≥90% \
                     of frames for {} frames while transform values stayed \
                     bit-identical across every sampled interval — the world \
                     is almost entirely rewriting itself, not changing",
                    state.phantom_dominant_consecutive_intervals
                        * RunState::PHANTOM_INTERVAL_FRAMES
                ),
                state.frame,
            );
        }
        state.phantom_last_hash = Some(hash);
        state.phantom_frames_since_sample = 0;
        state.phantom_changed_frames = 0;
    }
}

/// UI liveness watcher (`ui-liveness` feature): Bevy 0.20 UI nodes carry
/// `UiTransform`, not `Transform`, so a pure-UI game that responds to
/// input looks frozen to the transform-only oracle. This watcher feeds
/// UI changes into the same liveness evidence; it must run BEFORE
/// `frozen_world_oracle_system` (chained at registration).
#[cfg(feature = "ui-liveness")]
fn ui_liveness_system(
    mut state: ResMut<RunState>,
    q: Query<(), Changed<bevy_ui::prelude::UiTransform>>,
    q_all: Query<(), With<bevy_ui::prelude::UiTransform>>,
) {
    if !q_all.is_empty() {
        state.saw_ui_entity = true;
    }
    if !q.is_empty() {
        state.ui_activity_this_frame = true;
    }
}

// ---------------------------------------------------------------------------
// Run state
// ---------------------------------------------------------------------------

/// State resource tracking the current run.
#[derive(Resource)]
pub struct RunState {
    pub frame: u64,
    pub(crate) tps: u64,
    metrics: crate::state::Metrics,
    frame_timing: FrameTimingStats,
    /// Consecutive frames with no liveness evidence; read by the
    /// progress recorder (churn = frozen_frames == 0) — the progress
    /// oracle fires exactly where this streak never builds.
    pub(crate) frozen_frames: u64,
    // ----- Phantom-write detection (M3) -----
    /// Last sampled transform value hash (sorted bitwise
    /// translations). None until the first sample.
    phantom_last_hash: Option<u64>,
    /// Frames since the last interval sample (K = PHANTOM_INTERVAL_FRAMES).
    phantom_frames_since_sample: u64,
    /// Of the frames since the last sample: frames where
    /// `Changed<Transform>` fired (churn evidence).
    phantom_changed_frames: u64,
    /// Consecutive bit-identical intervals with churn on every frame
    /// (the phantom signature). Reset by any genuine change, any
    /// sample mismatch, or a report (one per crossing).
    phantom_consecutive_intervals: u64,
    /// Consecutive DOMINANT-phantom intervals: ≥90% of frames
    /// flagged `Changed<Transform>` AND the value hash unchanged.
    /// Softer than the strict counter above (which demands ALL frames
    /// flagged); it counts worlds where most churn is phantom and a
    /// minority of frames see real writes that land back on the same
    /// values. Reset only by an interval whose hash CHANGED (a
    /// genuine change actually happened); mere flag-density dips do
    /// not reset it. Reaching `phantom_grace_intervals + 1` while the
    /// strict counter has not fired is a SECONDARY detection: same
    /// rule, "mostly phantom writes" detail.
    phantom_dominant_consecutive_intervals: u64,
    /// Threshold in intervals: ceil(frozen_grace_frames / K), set
    /// alongside frozen_grace_frames in run_scenario.
    phantom_grace_intervals: u64,
    /// Grace period (frames) before frozen_world is reported.
    frozen_grace_frames: u64,
    /// True once ANY Transform-bearing entity existed during the run.
    /// The vacuity guard uses this instead of a post-hoc world query:
    /// a game whose entities all despawn before the end is NOT vacuous
    /// (the oracles observed them while they lived).
    saw_transform_entity: bool,
    /// True once any UI-node entity existed (UiTransform-bearing). Same
    /// purpose as `saw_transform_entity`: a pure-UI world is observable
    /// world state, not a vacuous run.
    saw_ui_entity: bool,
    /// Set by `ui_liveness_system` when any UiTransform changed this
    /// frame; consumed (and cleared) by `frozen_world_oracle_system`,
    /// which runs after it in the frame.
    ui_activity_this_frame: bool,
}

impl RunState {
    /// Phantom-detection interval sampling period: a transform VALUE
    /// hash is taken every K frames. Changing it trades detection
    /// latency for per-sample cost, nothing else.
    pub(crate) const PHANTOM_INTERVAL_FRAMES: u64 = 30;
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
            phantom_last_hash: None,
            phantom_frames_since_sample: 0,
            phantom_changed_frames: 0,
            phantom_consecutive_intervals: 0,
            phantom_dominant_consecutive_intervals: 0,
            phantom_grace_intervals: 0,
            saw_transform_entity: false,
            saw_ui_entity: false,
            ui_activity_this_frame: false,
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
/// Returns a structured report with violations, metrics, and action log.
pub fn run_scenario(app: &mut App, scenario: &Scenario) -> Result<PlaytestReport, ScenarioError> {
    validate_scenario(scenario)?;

    // Install global tracing subscriber (captures ERROR/WARN logs into the
    // run's violation buffer). The panic hook is installed separately below.
    sinks::ensure_global_subscriber();
    // While the bridge owns the `log` logger slot, the max level is
    // harness-owned state for the run: re-assert it to undo any
    // foreign log::set_max_level(Off) applied before the run (game
    // code, test frameworks). max_level is checked at emission sites,
    // so re-assert BEFORE setup (plugin finishing, census) and the
    // first frame; the loop re-asserts per tick for mid-run cases.
    sinks::assert_log_bridge_level();
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
    // Phantom-write grace: same one source of truth (liveness.timeout_s),
    // expressed in interval units — a phantom regime must persist as
    // long as a true freeze would before it is called out.
    let phantom_grace_intervals = frozen_grace_frames.div_ceil(RunState::PHANTOM_INTERVAL_FRAMES);
    app.insert_resource(ScenarioResource(Arc::new(scenario.clone())));
    app.insert_resource(RunState {
        tps,
        frozen_grace_frames,
        phantom_grace_intervals,
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
    // Raw mouse motion: same seeding rationale — winit writes
    // `MouseMotion` (derived from device events) and InputPlugin's
    // `accumulate_mouse_motion_system` sums it into
    // `AccumulatedMouseMotion`, which motion-reader games (camera
    // pan, look controls) poll each frame. MinimalPlugins apps lack
    // both, so the harness registers the message, seeds the resource
    // and runs the accumulator. Under InputPlugin graphs the
    // accumulator must NOT be duplicated: two copies double-sum the
    // same messages and games read 2× deltas — vacuously failing
    // motion-magnitude thresholds, desyncing replay comparisons.
    app.add_message::<bevy_input::mouse::MouseMotion>();
    app.init_resource::<bevy_input::mouse::AccumulatedMouseMotion>();
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
                bevy_input::mouse::accumulate_mouse_motion_system,
            ),
        );
    }
    // Oracles read wall-clock delta; Time may be absent on minimal apps.
    if app.world().get_resource::<Time>().is_none() {
        app.insert_resource(Time::<()>::default());
    }

    // CAPTURE the static input-surface scan for discovery runs ONLY:
    // the probe pool sets `ENV_DISCOVER_SCAN` before spawning the
    // game child, so this gate keeps the cost (schedule
    // force-initialization + a full access-set walk) off ordinary
    // playtests, which never read the captured resource. Inside a
    // discovery run, the capture point matters: every input signal
    // type is registered (seeding above) and every harness SYSTEM
    // that touches input signals — the engine fallback translators
    // above excepted, which stay visible for parity with
    // InputPlugin graphs — is still absent. The bots below WRITE
    // `KeyboardInput`, `MouseButtonInput` and the umbrella
    // `WindowEvent` (attributed to keyboard, button, motion AND
    // wheel classes), so a scan taken after their registration
    // would count the harness itself as a reader of every device
    // class and block wheel pruning forever. The captured scan rides
    // OUTSIDE the world — a planted resource is an entity and would
    // shift the digest counts the discovery diff compares — in a
    // process-global slot until the takeover exports it; schedules
    // are force-initialized inside the capture, not run —
    // `Schedule::run` re-initializes whenever the graph changed, so
    // the bot systems added below still take effect.
    #[cfg(feature = "takeover")]
    if std::env::var_os(crate::plugin::ENV_DISCOVER_SCAN).is_some() {
        let _ = crate::discovery::static_scan::capture_static_scan(app);
    }

    // Load-time capability census (default-on, milliseconds-cheap,
    // in-process). MUST run BELOW the scan-capture block above, not
    // before it: the census force-initializes schedules, and
    // `Schedule::initialize` BUILDS a changed graph, which moves
    // run conditions out of `schedule.graph()` and into executor
    // storage (`pub(super)` in bevy_ecs 0.20.1) — a census that ran
    // first would leave BOTH condition harvests inside the capture
    // (`condition_hidden_classes`, `collect_condition_blindness`)
    // staring at empty condition lists in every scan-exporting run.
    // After the capture it is safe: the capture has already built
    // every schedule that built successfully, so the census's own
    // `initialize` calls are no-op rebuilds for those; schedules
    // whose capture-time build FAILED keep a changed graph, and
    // the census retries them, pushing an honest failure note. It still runs before bot/oracle registration
    // below, and the input-signal seeding between the old and new
    // positions cannot inflate counts: every seeded type
    // (`KeyboardInput`, `ButtonInput<..>`, `Time`, ...) lives under
    // an engine `bevy_*` path the census's ownership filter
    // already excludes. Structurally neutral (pinned in
    // tests/it/census.rs); the one-line summary goes to stderr, full
    // rows ride in the report artifact.
    let capability_census = {
        let census = crate::discovery::census::run_capability_census(app);
        eprintln!("{}", census.summary_line());
        census
    };

    // Register bot + oracle systems once. The keyboard modifier state
    // is RESET each run so a previous run's held Shift cannot leak
    // into a fresh scenario's logical-key case mapping.
    app.init_resource::<ChaosHoldState>();
    app.init_resource::<ScheduledHoldState>();
    app.init_resource::<MouseHoldState>();
    app.insert_resource(KeyboardModifierState::default());
    app.insert_resource(CursorInputResource::default());
    // Enumerated widget targets for the chaos mouse bot (`ui-surface`
    // — see the census-neutrality comment above). Populated by the
    // driver loop after the first update; empty until then, which
    // routes every click through the legacy targeting path.
    app.insert_resource(UiClickTargets::default());
    app.insert_resource(crate::explorer::Explorer::new(
        scenario.bot.explore && scenario.bot.bot_type == BotType::Chaos,
    ));

    app.insert_resource(crate::invariant_eval::ScenarioInvariants(
        scenario.invariants.clone(),
    ));
    if scenario
        .invariants
        .iter()
        .any(|i| i.rule == crate::enums::InvariantRule::ComponentCheck)
        || scenario.invariants.iter().any(|i| {
            i.rule == crate::enums::InvariantRule::NodesInBounds
                && matches!(
                    i.target_selector,
                    Some(crate::enums::TargetSelector::WithComponent { .. })
                )
        })
    {
        app.init_resource::<crate::invariant_eval::WatchedInvariants>();
    }
    if scenario.invariants.iter().any(|i| {
        matches!(
            i.rule,
            crate::enums::InvariantRule::ResourceCheck
                | crate::enums::InvariantRule::StagnantResource
        )
    }) {
        app.init_resource::<crate::invariant_eval::WatchedResources>();
    }
    if scenario.verify_replay {
        app.insert_resource(crate::replay::ReplaySeries::default());
    }
    if scenario.record_digest {
        // Opt-in per-frame state digest recording. Cheap when present
        // (a few queries + a reflection walk per frame), zero cost when
        // the resource is absent.
        app.insert_resource(crate::digest::StateDigestRecorder::default());
    }
    // Progress-liveness recorder: driver-loop sampled (NEVER a
    // scheduled system — see the progress module docs). Absent
    // resource = mode 'off' = zero cost and total silence.
    let progress_enabled = scenario.liveness.progress.mode != crate::scenario::ProgressMode::Off;
    if progress_enabled {
        app.insert_resource(crate::progress::ProgressRecorder::new(
            scenario.liveness.progress.clone(),
            tps,
        ));
    }

    app.add_systems(
        Update,
        (
            // Keyboard first: in explore mode the mouse checks whether
            // the keyboard used its turn this frame.
            chaos_bot_system
                .run_if(is_chaos_bot)
                .before(chaos_mouse_system),
            chaos_mouse_system.run_if(is_chaos_bot),
            synthetic_keyboard_bot_system.run_if(is_synthetic_keyboard_bot),
            scripted_mouse_system,
            scripted_motion_system,
            frame_timing_oracle_system,
            frozen_world_oracle_system,
            crate::invariant_eval::evaluate_invariants_system,
            crate::oracles::nan_oracle_system,
            crate::oracles::bounds_oracle_system,
        ),
    );

    // Per-frame RECORDERS (replay fingerprint + state digest) are NOT
    // scheduled systems: they are invoked by the driver loop directly
    // after `app.update()` returns. No schedule placement — not even
    // `Last` — can guarantee ordering relative to the game's OWN
    // systems (a game may register mutating systems in `Last` too),
    // so the only guaranteed end-of-frame observation point is outside
    // all schedules. Recording here means the series capture the fully
    // settled post-update world every frame, including the final frame.
    let record_fingerprint = scenario.verify_replay;
    let record_digest = scenario.record_digest;

    // UI liveness watcher must observe this frame's changes BEFORE the
    // oracle consumes them. Registered separately so the whole system
    // (and its bevy_ui dependency) compiles away when the feature is off.
    #[cfg(feature = "ui-liveness")]
    app.add_systems(
        Update,
        ui_liveness_system.before(frozen_world_oracle_system),
    );

    let total_ticks = (scenario.duration_s * tps as f32).ceil() as u64;
    let start = std::time::Instant::now();

    // State trace: recorder systems go into every OnEnter/OnExit/
    // OnTransition schedule BEFORE the first update (so the initial
    // OnEnter is captured); resource change ticks are sampled between
    // frames. RefCell-wrapped like the trackers below so a panicking
    // run still reports the partial trace.
    let state_trace = std::cell::RefCell::new(
        crate::discovery::state_trace::StateTraceRecorder::install(app),
    );

    // Leafwing declared-tier capture + intent-tap (`lw-reader`). Runs
    // as a driver-loop recorder (like the digest), never a scheduled
    // system — it must not appear in the game's schedule or the
    // static scan's reader census. Declarations are read ONCE, at
    // the first frame where Leafwing state exists (Startup-spawned),
    // and held for the whole run; the tap observes every frame.
    #[cfg(feature = "lw-reader")]
    let mut lw_declared: Vec<crate::discovery::lw::LwDeclaration> = Vec::new();
    #[cfg(feature = "lw-reader")]
    let mut lw_declared_captured = false;
    #[cfg(feature = "lw-reader")]
    let mut lw_tap_stats: Vec<crate::discovery::lw::LwTapStats> = Vec::new();

    // Declared UI surface capture (`ui-surface`). Same recorder
    // doctrine as the lw capture above: invoked from the driver loop,
    // never a scheduled system — it must not appear in the game's
    // schedule or the static scan's reader census. FIRST capture on
    // the first settled frame (layout-dependent surfaces need one
    // `app.update()` so ComputedNode/UiStack exist), then RE-CAPTURED
    // periodically (`UiSurfaceTracker::recapture_interval`, ~twice a
    // second) so surfaces spawned mid-run (menus, dialogs, tooltips)
    // join the surface and the chaos mouse's `UiClickTargets` — the
    // snapshot-in-time blind spot this replaces. Each recapture's
    // entity-set deltas are recorded as notes (refusal over silence,
    // bounded — see UiSurfaceTracker).
    #[cfg(feature = "ui-surface")]
    let mut ui_surface_tracker = crate::discovery::ui_surface::UiSurfaceTracker::default();
    #[cfg(feature = "ui-surface")]
    let ui_recapture_interval =
        crate::discovery::ui_surface::UiSurfaceTracker::recapture_interval(tps);
    #[cfg(feature = "ui-surface")]
    let mut recapture_next = false;

    // Render-liveness sampler (`render-liveness`): identical
    // construction to the two trackers above — borrowed into the
    // `catch_unwind` loop so a panicking run can still report the
    // partial evidence (a camera that existed before the crash is
    // evidence worth keeping; refusal over silence). The RESULT is
    // computed only when the render-sub-app gate passed: the
    // "not evaluated" tier attaches NOTHING.
    #[cfg(feature = "render-liveness")]
    let render_liveness_attached = crate::discovery::render_liveness::render_sub_app_present(app);
    #[cfg(feature = "render-liveness")]
    let render_liveness_tracker = std::cell::RefCell::new(
        crate::discovery::render_liveness::RenderLivenessTracker::default(),
    );
    #[cfg(feature = "render-liveness")]
    let render_liveness_interval = crate::discovery::render_liveness::sample_interval(tps);

    // Screen-liveness capture rig (`screen-liveness`): identical
    // construction doctrine to the render-liveness tracker —
    // RefCell-wrapped so a panicking run still reports partial
    // captures, gated by render sub-app presence AND the rig's own
    // world-side prerequisites (the `Assets<Image>` store). The rig
    // SPAWNS entities mid-run (probe camera, capture requests,
    // observer) — all marker-tagged, census-invisible — so its
    // tick hook must stay OUTSIDE the App::update call itself:
    // between-tick driver-loop placement only, after all schedules.
    #[cfg(feature = "screen-liveness")]
    let screen_liveness_attached = crate::discovery::render_liveness::render_sub_app_present(app)
        && crate::discovery::screen_liveness::ScreenLivenessRig::attachable(app.world());
    #[cfg(feature = "screen-liveness")]
    let screen_liveness_rig =
        std::cell::RefCell::new(crate::discovery::screen_liveness::ScreenLivenessRig::new());
    #[cfg(feature = "screen-liveness")]
    let screen_liveness_interval = crate::discovery::screen_liveness::sample_interval(tps);

    let loop_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        for i in 0..total_ticks {
            // Defense against a foreign log::set_max_level(Off): the
            // bridge's logger is ours, so re-claim the level each
            // tick. Cost is one atomic store. A ONE-TIME Off blinds
            // at most the remainder of this tick; a persistent
            // per-tick Off is flagged after app.update() below.
            sinks::assert_log_bridge_level();
            sinks::set_current_frame(i);
            app.update();
            let ran_states = state_trace.borrow_mut().after_frame(app.world(), i);
            // Detection half of the defense: a level still lowered
            // after app.update() means a game system re-blinds every
            // tick — repaired next tick, but each tick's tail was
            // silently lost. Refusal over silence: report it; the
            // count converges to the number of tampered ticks.
            if sinks::log_level_tampered() == Some(true) {
                if let Some(mut v) = app.world_mut().get_resource_mut::<RunViolations>() {
                    v.0.report(
                        Rule::LogLevelTampered.name(),
                        "world",
                        "the `log` max level was lowered mid-run while the harness \
                         bridge owned the logger (foreign set_max_level): \
                         log_error/log_warn findings are under-reported for the \
                         affected tick tails"
                            .to_string(),
                        i,
                    );
                }
            }
            #[cfg(feature = "ui-surface")]
            {
                let click_targets = app
                    .world_mut()
                    .get_resource_mut::<UiClickTargets>()
                    .map(|mut res| std::mem::take(&mut res.centers))
                    .unwrap_or_default();
                // Borrow juggling: `enumerate_ui` takes `&World`, so
                // the click-targets vec is taken out of the resource,
                // filled, and put back.
                // A state transition means a new screen: refresh the
                // targets now and once more next frame (nodes spawned by
                // deferred commands lay out one frame later).
                let key_window_ends = app
                    .world()
                    .get_resource::<crate::explorer::Explorer>()
                    .is_some_and(|e| e.enabled && e.recapture_due(i));
                if !ran_states.is_empty() || recapture_next || key_window_ends {
                    ui_surface_tracker.request_recapture();
                }
                recapture_next = !ran_states.is_empty();
                let filled = {
                    let world = app.world();
                    let no_learned = Default::default();
                    let learned = world
                        .get_resource::<crate::explorer::Explorer>()
                        .map_or(&no_learned, |e| &e.learned);
                    let mut targets_taken = click_targets;
                    ui_surface_tracker.observe(
                        world,
                        i,
                        ui_recapture_interval,
                        &mut targets_taken,
                        learned,
                    );
                    targets_taken
                };
                let ids = ui_surface_tracker.target_ids();
                let draggable = ui_surface_tracker.target_draggable();
                let drop_target = ui_surface_tracker.target_drop();
                if let Some(mut res) = app.world_mut().get_resource_mut::<UiClickTargets>() {
                    res.centers = filled;
                    res.ids = ids;
                    res.draggable = draggable;
                    res.drop_target = drop_target;
                }
            }
            // Explorer novelty for this frame: state schedules that ran,
            // and the current screen (enabled click targets).
            {
                let world = app.world_mut();
                let targets = world
                    .get_resource::<UiClickTargets>()
                    .map(|t| t.ids.clone())
                    .unwrap_or_default();
                // Learning click targets: the nodes under a random press
                // (read the frame after it, once picking saw the cursor)
                // are credited when the game reacts shortly after.
                #[cfg(feature = "ui-surface")]
                let hovered: Vec<Vec<usize>> = if world
                    .get_resource::<crate::explorer::Explorer>()
                    .is_some_and(|e| e.enabled && e.hover_due(i))
                {
                    hovered_archetypes(world)
                } else {
                    Vec::new()
                };
                #[cfg(not(feature = "ui-surface"))]
                let hovered: Vec<Vec<usize>> = Vec::new();
                if let Some(mut explorer) = world.get_resource_mut::<crate::explorer::Explorer>() {
                    if explorer.enabled {
                        explorer.observe(i, &ran_states, &targets);
                        if explorer.hover_due(i) {
                            explorer.set_hit(hovered);
                        }
                        let window = explorer.window();
                        let reacted = explorer
                            .pending_press()
                            .is_some_and(|press| state_trace.borrow().reacted_after(press, window));
                        explorer.after_reaction(i, reacted, !ran_states.is_empty());
                    }
                }
            }
            // Guaranteed end-of-frame recording (see comment above): after
            // ALL schedules (incl. the game's own `Last` systems) ran.
            if record_fingerprint {
                crate::replay::fingerprint_system(app.world_mut());
            }
            if record_digest {
                crate::digest::record_digest_system(app.world_mut());
            }
            // Render-liveness sample (`render-liveness`): placed AFTER
            // digest recording so the current frame's digest is
            // visible, and reads the CURRENT frame's frozen-world
            // streak (still unreset: `RunState.frame` increments at
            // the end of this block, and `frozen_frames` was last
            // written by this frame's oracle run). Same recorder
            // doctrine as everything above: driver-invoked, never a
            // scheduled system. `sim_active` for this tick: the world
            // produced visible activity this frame
            // (`frozen_frames == 0`), OR the opt-in broadened state
            // digest just changed (visually silent but stepping —
            // the same broadened evidence the frozen_world detail
            // refinement uses). That digest-step signal is computed
            // ONCE per tick below (see `changed_since_last_sample`'s
            // single-call contract: a second call in the same tick
            // would compare current-vs-current and answer `false`,
            // starving the second caller's evidence) — the
            // render-liveness sampler and the screen-liveness rig
            // both consume the shared boolean.
            #[cfg(feature = "render-liveness")]
            let digest_stepped_this_tick = app
                .world_mut()
                .get_resource_mut::<crate::digest::StateDigestRecorder>()
                .is_some_and(|mut r| r.changed_since_last_sample());
            // Sim-active evidence, shared by both liveness rigs
            // (frozen_frames cannot move between here and the frame
            // increment below, so a single read serves both).
            #[cfg(feature = "render-liveness")]
            let sim_active_this_tick = app
                .world()
                .get_resource::<RunState>()
                .is_some_and(|s| s.frozen_frames == 0)
                || digest_stepped_this_tick;
            #[cfg(feature = "render-liveness")]
            if render_liveness_attached {
                render_liveness_tracker.borrow_mut().observe(
                    app.world(),
                    i,
                    render_liveness_interval,
                    sim_active_this_tick,
                );
            }
            // Screen-liveness tick (`screen-liveness`): same
            // driver-loop recorder doctrine, same `sim_active`
            // evidence signal as render-liveness. Placement: AFTER
            // the render-liveness observe (the two evidence
            // timelines align tick-for-tick) and BEFORE the frame
            // counter increments. Spawns (probe camera at the first
            // eligible tick, request entities at cadence ticks) land
            // here, between `app.update()` calls — the extraction of
            // the NEXT frame picks them up; captures land 1–2 ticks
            // later via the game's own ScreenshotPlugin schedule
            // and the rig's global observer.
            #[cfg(feature = "screen-liveness")]
            if screen_liveness_attached {
                screen_liveness_rig.borrow_mut().tick(
                    app.world_mut(),
                    i,
                    screen_liveness_interval,
                    sim_active_this_tick,
                );
            }
            // component_check evaluation: a driver-loop recorder (NOT
            // a scheduled system — `&World` reflection access cannot
            // coexist with ResMut<RunViolations> in one system, and
            // only post-update observation is guaranteed to see the
            // fully-settled frame). See invariant_eval module docs.
            crate::invariant_eval::record_component_checks(app.world_mut(), i);
            // nodes_in_bounds + with_component: same recorder doctrine
            // (dynamic type resolution needs &World).
            crate::invariant_eval::record_bounds_with_component(app.world_mut(), i);
            // resource_check + stagnant_resource: same recorder
            // doctrine (reflected resource access needs &World).
            crate::invariant_eval::record_resource_checks(app.world_mut(), i);
            crate::invariant_eval::record_stagnant_resources(app.world_mut(), i);
            // Progress-surface sampling: same driver-loop recorder
            // doctrine as the digest above.
            crate::progress::record_progress_sample(app.world_mut(), i);
            #[cfg(feature = "lw-reader")]
            {
                if !lw_declared_captured {
                    let declared = crate::discovery::lw::read_declared(app.world());
                    if !declared.is_empty() {
                        lw_declared = declared;
                        lw_declared_captured = true;
                    }
                }
                let pressed = crate::discovery::lw::pressed_keys_this_frame(app.world());
                crate::discovery::lw::tap_manager_pipeline(
                    app.world(),
                    &pressed,
                    &lw_declared,
                    &mut lw_tap_stats,
                );
            }
            if let Some(mut state) = app.world_mut().get_resource_mut::<RunState>() {
                state.frame += 1;
            }
        }
    }));
    let crash_detected = loop_result.is_err();

    let wall_time_s = start.elapsed().as_secs_f64();

    // Render-liveness verdict: derive the tier from the samples the
    // loop took (partial evidence if the run panicked — kept on
    // purpose). The ATTACHMENT decision stays the render-sub-app
    // gate: no renderer ⇒ `None` ⇒ "not evaluated", the field is
    // simply absent (older-schema consumers never see a surprise).
    #[cfg(feature = "render-liveness")]
    let render_liveness_tracker_result = render_liveness_attached
        .then(|| std::cell::RefCell::into_inner(render_liveness_tracker).into_result());

    // Screen-liveness verdict: same partial-evidence doctrine
    // (captures that landed before a crash are kept). The finish
    // hook runs the GPU wait-poll + direct channel drain (escape
    // hatch for slow readbacks) and derives the tier. The ATTACHMENT
    // gate stays the sub-app + asset-store gate from construction:
    // never attached ⇒ `None` ⇒ "not evaluated".
    #[cfg(feature = "screen-liveness")]
    let screen_liveness_result = screen_liveness_attached
        .then(|| std::cell::RefCell::into_inner(screen_liveness_rig).finish(app.world_mut()));

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

    // Extract the per-frame state digest series if the scenario opted in.
    let state_digest = app
        .world_mut()
        .remove_resource::<crate::digest::StateDigestRecorder>()
        .map(|r| r.digests)
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

    // Vacuity guard: if the whole run never had a Transform or UI entity the
    // oracles could check, a Pass proves nothing about the game (bot
    // inputs hitting a void are unobservable activity). Flag loudly.
    // Tracked DURING the run (any-frame observability), not post-hoc:
    // a game whose entities despawn before the end is NOT vacuous.
    let entities_observable = state.saw_transform_entity || state.saw_ui_entity;
    if status == PlaytestStatus::Pass && !entities_observable {
        let remedy = if cfg!(feature = "ui-liveness") {
            "spawn entities with a Transform or UiTransform so oracles have something to check"
        } else {
            "spawn entities with a Transform so oracles have something to check"
        };
        snap.push(crate::state::ViolationEntry {
            rule: Rule::VacuousRun.name().to_string(),
            target: "run".to_string(),
            first_frame: 0,
            last_frame: state.frame,
            count: 1,
            detail: format!("no entities to observe — the run is vacuous; {remedy}"),
            last_detail: "no entities to observe — the run is vacuous".to_string(),
            meta: None,
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
                meta: None,
            });
        }
        if !scenario.invariants.is_empty() && status == PlaytestStatus::Pass {
            status = PlaytestStatus::Fail;
        }
    }

    // component_check post-run audit. Two judgments need the FULL run
    // history, not any single frame:
    // - a selector that matched zero entities across the whole run is
    //   vacuous (with_component can select nothing even while other
    //   entities exist, so the guard above does not cover it);
    // - an `eventually_s` deadline that never fired is a miss known
    //   only at run end (any single frame can still satisfy it).
    // Like the vacuity guards, these may demote a Pass — the missing
    // predicate was provable only after the deadline passed.
    let watched = app
        .world_mut()
        .remove_resource::<crate::invariant_eval::WatchedInvariants>()
        .unwrap_or_default();
    for entry in crate::invariant_eval::audit_component_checks(
        &scenario.invariants,
        &watched,
        tps,
        state.frame,
    ) {
        // Dedupe against post-run guards already pushed (the run-level
        // vacuity guard names every invariant when the whole run was
        // unobservable; do not report the same (rule, target) twice).
        if !snap
            .iter()
            .any(|v| v.rule == entry.rule && v.target == entry.target)
        {
            snap.push(entry);
        }
    }
    if status == PlaytestStatus::Pass
        && snap
            .iter()
            .any(|v| !is_informational(&v.rule, scenario.strict_warnings))
    {
        status = PlaytestStatus::Fail;
    }

    // resource_check / stagnant_resource post-run audit: same full-run-
    // history judgments as the component_check audit (vacuity with
    // any-frame observability; eventually_s deadlines judgeable only
    // at run end; stagnant_resource verdicts complete per episode).
    let watched_resources = app
        .world_mut()
        .remove_resource::<crate::invariant_eval::WatchedResources>()
        .unwrap_or_default();
    for entry in crate::invariant_eval::audit_resource_checks(
        &scenario.invariants,
        &watched_resources,
        tps,
        state.frame,
    ) {
        // Dedupe against post-run guards already pushed (the run-level
        // vacuity guard names every invariant when the whole run was
        // unobservable; do not report the same (rule, target) twice).
        if !snap
            .iter()
            .any(|v| v.rule == entry.rule && v.target == entry.target)
        {
            snap.push(entry);
        }
    }
    if status == PlaytestStatus::Pass
        && snap
            .iter()
            .any(|v| !is_informational(&v.rule, scenario.strict_warnings))
    {
        status = PlaytestStatus::Fail;
    }

    // Progress-liveness post-run audit: per-window verdicts need the
    // FULL sample series, not any single frame (same doctrine as the
    // component_check audit above). Escalation of
    // progress_surface_empty under require_surface is honest: the
    // rule is informational in the registry, and this is the one
    // scenario field whose whole point is demanding verifiability.
    let progress_audit = app
        .world_mut()
        .remove_resource::<crate::progress::ProgressRecorder>()
        .map(crate::progress::audit_progress)
        .map(|mut audit| {
            for entry in audit.entries.drain(..) {
                // Dedupe against guards already pushed (oracle_error
                // entries can coincide with none today, but the
                // pattern keeps the report single-voiced).
                if !snap
                    .iter()
                    .any(|v| v.rule == entry.rule && v.target == entry.target)
                {
                    snap.push(entry);
                }
            }
            audit
        });
    if status == PlaytestStatus::Pass
        && snap.iter().any(|v| {
            !is_progress_informational(
                &v.rule,
                scenario.strict_warnings,
                scenario.liveness.progress.require_surface,
            )
        })
    {
        status = PlaytestStatus::Fail;
    }

    // Action log: one entry per emitted input press, so a report's claim
    // of bot activity is auditable. Only key draws appear here — mouse
    // draws are not key presses. `details` carries the draw's label
    // (canonical key name): the recorded `value` indexes the draw-time
    // unheld-candidate list, which the report does not contain.
    let action_log = choices
        .iter()
        .filter_map(|c| {
            let action = match c.kind {
                crate::choice::ChoiceKind::Key => format!("key_press:{}", c.value),
                crate::choice::ChoiceKind::KeyCombo => format!("key_combo:{}", c.value),
                _ => return None,
            };
            Some(crate::state::ActionEntry {
                frame: c.frame,
                source: "bot".to_string(),
                action,
                details: c.label.clone(),
            })
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
            meta: None,
        });
    }

    // Log-bridge honesty: even when the capture subscriber won the
    // tracing global default, the log→tracing bridge can have lost
    // the `log` crate's write-once logger slot (another LogTracer or
    // custom log backend installed first). Without the bridge, all
    // `log`-crate events — bevy's B000x hierarchy validators, wgpu/
    // naga, asset loaders — bypass the capture layer entirely, so
    // log_error/log_warn under-report. Surface it like
    // log_capture_lost: informational (environmental, not a game
    // bug), machine-readable in the report's violations list.
    if log_capture_available
        && !sinks::bridge_won()
        && std::env::var_os(sinks::ENV_ALLOW_MISSING_LOG_CAPTURE).is_none()
    {
        snap.push(crate::state::ViolationEntry {
            rule: Rule::LogBridgeLost.name().to_string(),
            target: "world".to_string(),
            first_frame: 0,
            last_frame: 0,
            count: 1,
            detail: "the log→tracing bridge could not be installed (another \
                     `log` crate logger won the global slot): events emitted \
                     through `log` (bevy B000x hierarchy validators, wgpu/naga, \
                     any log-crate user) do not reach the log_error/log_warn \
                     oracles"
                .to_string(),
            last_detail: "log bridge OFF".to_string(),
            meta: None,
        });
    }

    // render_liveness (informational, `render-liveness` feature):
    // the structural camera-health verdict as a violation entry so
    // it rides the same violations list consumers already read. The
    // attachment gate (render sub-app present) is the
    // "not evaluated" tier; the tier/`sim_active` nuance lives in
    // the detail text, and the full evidence ALSO rides the
    // report's `render_liveness` field. Informational by registry
    // (src/rules.rs `is_informational`) — never gates status.
    #[cfg(feature = "render-liveness")]
    if let Some(rl) = render_liveness_tracker_result.as_ref() {
        let (detail, last_detail) = match rl.tier {
            crate::discovery::render_liveness::RenderLivenessTier::Live => {
                // Structural findings only; the "live" tier emits
                // NOTHING (refusal over silence: no finding, no noise).
                (String::new(), String::new())
            }
            crate::discovery::render_liveness::RenderLivenessTier::NoCameras => (
                format!(
                    "simulation was observed active during the run ({} \
                     {}) but no camera ever existed in any of them: \
                     the game has nothing wired up to display its state",
                    rl.samples.max(1),
                    if rl.samples.max(1) == 1 {
                        "sample"
                    } else {
                        "samples"
                    }
                ),
                "no camera ever existed".to_string(),
            ),
            crate::discovery::render_liveness::RenderLivenessTier::AllCamerasInactive => (
                format!(
                    "{} camera(s) existed but every sampled camera had \
                     is_active == false: rendering was disabled (a \
                     debug-disabled camera left on, or game logic parking \
                     the camera)",
                    rl.cameras_seen
                ),
                "every sampled camera was inactive".to_string(),
            ),
            crate::discovery::render_liveness::RenderLivenessTier::AllCamerasSkip => (
                format!(
                    "{} camera(s) existed with an active one, but no \
                     sampled active camera used a write-mode \
                     CameraOutputMode, so nothing was written \
                     to any target (may be intentional: compositing or \
                     prepass-only setups park cameras on Skip)",
                    rl.cameras_seen
                ),
                "no sampled active camera wrote output".to_string(),
            ),
        };
        if !detail.is_empty() {
            snap.push(crate::state::ViolationEntry {
                rule: Rule::RenderLiveness.name().to_string(),
                target: "world".to_string(),
                first_frame: 0,
                last_frame: state.frame,
                count: 1,
                detail,
                last_detail,
                meta: None,
            });
        }
    }

    // screen_liveness (informational, `screen-liveness` feature):
    // the pixel-observed verdict as a violation entry — same
    // ride-the-violations-list doctrine as render_liveness. The
    // "live" tier emits NOTHING (refusal over silence). The other
    // non-trivial tiers are reported as evidence, never findings
    // that gate status: `not_evaluated_3d` states the 2D-only
    // boundary plainly, and `insufficient_captures` refuses the
    // change-detection question rather than guessing.
    #[cfg(feature = "screen-liveness")]
    if let Some(sl) = screen_liveness_result.as_ref() {
        let (detail, last_detail) = match sl.tier {
            crate::discovery::screen_liveness::ScreenLivenessTier::Live => {
                // Pixels drifted (or nothing judgeable happened):
                // no finding, no noise.
                (String::new(), String::new())
            }
            crate::discovery::screen_liveness::ScreenLivenessTier::FrozenWhileSimActive => (
                format!(
                    "the captured 2D screen never changed across all \
                     {} capture(s) (hamming distance 0 between every \
                     consecutive pair) while the simulation was \
                     observed active in the same span: pixels did not \
                     flow despite a stepping world (may be \
                     intentional: menus, dialogue, level-load art)",
                    sl.captures.max(1)
                ),
                "captured screen frozen while sim active".to_string(),
            ),
            crate::discovery::screen_liveness::ScreenLivenessTier::InsufficientCaptures => (
                format!(
                    "only {} usable capture(s) landed before the run \
                     ended: the did-the-screen-ever-change question is \
                     refused, not guessed (ultra-short run, or the GPU \
                     readback wire failed under this adapter)",
                    sl.captures
                ),
                "fewer than two usable captures".to_string(),
            ),
            crate::discovery::screen_liveness::ScreenLivenessTier::NotEvaluated3D => (
                "no 2D game camera was observed at any capture \
                 cadence tick: a harness Camera2d cannot faithfully \
                 mirror a purely-3D view, so this run was not \
                 evaluated for screen liveness (the structural \
                 render_liveness rule still judges camera wiring)"
                    .to_string(),
                "not evaluated (no 2D camera)".to_string(),
            ),
        };
        if !detail.is_empty() {
            snap.push(crate::state::ViolationEntry {
                rule: Rule::ScreenLiveness.name().to_string(),
                target: "world".to_string(),
                first_frame: 0,
                last_frame: state.frame,
                count: 1,
                detail,
                last_detail,
                meta: None,
            });
        }
    }

    // Deterministic ordering: stats accumulate in registry-walk order,
    // which is registration-stable within a run but sort anyway so
    // byte-stability holds across the double run.
    #[cfg(feature = "lw-reader")]
    let lw_tap_stats = {
        let mut s = lw_tap_stats;
        s.sort_by(|a, b| (&a.action_map, &a.action).cmp(&(&b.action_map, &b.action)));
        s.dedup();
        s
    };
    // Leafwing reconcile on EVERY run (lw-reader): declared bindings
    // joined with this run's intent-tap evidence. The alarm channel
    // (`declared_but_dead`: proven-dead wiring) needs no probe data.
    // No independent liveness evidence exists, so no
    // `context_unverified` grade is emitted (it would otherwise flag
    // every working binding).
    #[cfg(feature = "lw-reader")]
    let lw_reconciled = if lw_declared.is_empty() && lw_tap_stats.is_empty() {
        None
    } else {
        Some(crate::discovery::lw::reconcile_without_liveness(
            &crate::discovery::lw::LwRunEvidence {
                declared: lw_declared.clone(),
                stats: lw_tap_stats.clone(),
            },
        ))
    };
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
        log_capture_available: Some(log_capture_available),
        log_bridge_available: Some(sinks::bridge_won()),
        // The report-writing code runs in the game's own binary
        // (in-process here, takeover child elsewhere), so the
        // reporting process's profile IS the game's profile.
        game_build_profile: Some(crate::discovery::surface::BuildProfile::current()),
        state_digest,
        progress_samples: progress_audit.map(|a| a.samples).unwrap_or_default(),
        source_derived_keys: None,
        source_derived_notes: None,
        #[cfg(feature = "lw-reader")]
        lw_declared,
        #[cfg(feature = "lw-reader")]
        lw_tap_stats,
        #[cfg(feature = "lw-reader")]
        lw_reconciled,
        state_trace: Some(state_trace.into_inner().into_result()),
        randomness: None,
        exploration: app
            .world()
            .get_resource::<crate::explorer::Explorer>()
            .filter(|e| e.enabled)
            .map(|e| e.summary()),
        capability_census: Some(capability_census),
        #[cfg(feature = "ui-surface")]
        ui_surface: ui_surface_tracker.into_result(),
        #[cfg(feature = "render-liveness")]
        render_liveness: render_liveness_tracker_result,
        #[cfg(feature = "screen-liveness")]
        screen_liveness: screen_liveness_result,
    })
}

fn is_chaos_bot(scenario: Option<Res<ScenarioResource>>) -> bool {
    scenario.is_some_and(|s| s.0.bot.bot_type == BotType::Chaos)
}

fn is_synthetic_keyboard_bot(scenario: Option<Res<ScenarioResource>>) -> bool {
    scenario.is_some_and(|s| s.0.bot.bot_type == BotType::SyntheticKeyboard)
}

/// Public re-export of the child-side informational classifier for
/// the parent-side sweep gate: the gate must replicate WHICH
/// violations the child treated as gating (contextual: escalated
/// `log_warn`/`progress_surface_empty`), not just which are
/// fingerprintable. Sharing one function is the no-drift guarantee.
pub fn is_progress_informational_public(
    rule: &str,
    strict_warnings: bool,
    require_surface: bool,
) -> bool {
    is_progress_informational(rule, strict_warnings, require_surface)
}

/// Whether a rule is informational (non-gating). When the scenario sets
/// `strict_warnings`, log warnings AND progress_surface_empty escalate
/// to gating failures (strict_warnings is the generic "everything you
/// report, I mean it" knob).
fn is_informational(rule: &str, strict_warnings: bool) -> bool {
    if strict_warnings && (rule == "log_warn" || rule == "progress_surface_empty") {
        return false;
    }
    // Delegate to the registry-derived classification so the two cannot
    // drift (see Rule::is_informational for the member list).
    Rule::from_name(rule).is_some_and(rules::Rule::is_informational)
}

/// Whether a rule stays non-gating when BOTH escalation knobs are
/// considered: `strict_warnings` (generic) and
/// `liveness.progress.require_surface` (progress-specific — set when
/// the scenario's purpose requires a verifiable progress surface).
fn is_progress_informational(rule: &str, strict_warnings: bool, require_surface: bool) -> bool {
    if require_surface && rule == "progress_surface_empty" {
        return false;
    }
    is_informational(rule, strict_warnings)
}

#[cfg(test)]
mod chaos_hold_tests {
    //! Unit tests for the multi-slot chaos hold model: slot capacity,
    //! last-holder-releases refcounting on same-key collisions, and
    //! release-order bookkeeping. These mirror the mouse hold-count
    //! tests at the integration level (`overlapping_mouse_holds_
    //! refcount_per_button`) at the unit level of the state machine.

    use super::*;

    #[test]
    fn press_fills_slots_up_to_capacity() {
        let mut st = ChaosHoldState::default();
        st.press(KeyCode::KeyW, 10);
        st.press(KeyCode::ArrowLeft, 20);
        assert_eq!(st.held.len(), CHAOS_HOLD_SLOTS);
        // Sorted by release frame: earliest expiry pops first.
        assert_eq!(st.held[0], (KeyCode::KeyW, 10));
        assert_eq!(st.held[1], (KeyCode::ArrowLeft, 20));
    }

    #[test]
    fn same_key_in_two_slots_last_holder_releases() {
        let mut st = ChaosHoldState::default();
        // Two slots hold the SAME key with different expiries.
        st.press(KeyCode::KeyW, 10);
        st.press(KeyCode::KeyW, 20);
        // EARLY expiry: the other slot still holds W, so no Released
        // message may fire.
        st.held.remove(0);
        assert!(
            !st.release(KeyCode::KeyW),
            "early release must be swallowed"
        );
        // LATE expiry: last holder lets go, Released fires.
        st.held.remove(0);
        assert!(st.release(KeyCode::KeyW), "last holder must release");
        assert!(st.held.is_empty());
    }

    #[test]
    fn distinct_keys_release_independently() {
        let mut st = ChaosHoldState::default();
        st.press(KeyCode::KeyW, 10);
        st.press(KeyCode::KeyA, 10);
        st.held.remove(0); // W expires
        assert!(st.release(KeyCode::KeyW), "distinct keys never refcount");
        assert_eq!(st.held.len(), 1, "A's hold must survive W's release");
        st.held.remove(0); // A expires
        assert!(st.release(KeyCode::KeyA));
        assert!(st.held.is_empty());
    }

    #[test]
    fn held_stays_sorted_by_release_frame() {
        let mut st = ChaosHoldState::default();
        // Insert out of order; expiry must still pop earliest-first.
        st.press(KeyCode::KeyW, 30);
        st.press(KeyCode::KeyA, 10);
        assert_eq!(st.held.first().unwrap().1, 10);
        // Slot freed, press again later than both remaining expiries:
        st.held.remove(0);
        st.press(KeyCode::KeyS, 40);
        assert_eq!(st.held[0].1, 30);
        assert_eq!(st.held[1].1, 40);
    }
}
