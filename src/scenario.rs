//! Scenario schema: DTOs, defaults, and load-time validation.
//!
//! Slimmed for v0.1: the deferred subsystems (golden trajectories,
//! determinism checks, planner goals, choice-replay, ambiguity gates,
//! expert-rule invariants, synthetic-pointer) are absent from the schema
//! entirely rather than accepted-and-ignored. Scenarios carrying those
//! fields are rejected by `deny_unknown_fields` with a clear message.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::enums::{BotType, CheckOp, InvariantRule};

fn default_tps() -> u32 {
    60
}
fn default_true() -> bool {
    true
}
fn default_duration() -> f32 {
    15.0
}
fn default_seed() -> u64 {
    42
}
fn default_rate() -> u32 {
    10
}
fn default_hold_frames() -> u64 {
    1
}
fn default_liveness_timeout() -> f32 {
    2.0
}

/// A playtest scenario.
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    /// Stable scenario name. Optional for plain playtests; required when
    /// a report needs to be attributable to a named case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub bot: BotConfig,
    /// Simulation ticks per second. Each `app.update()` advances simulated
    /// time by exactly 1/tps when `simulated_time` is true.
    #[serde(default = "default_tps")]
    pub tps: u32,
    /// Drive `Time` with a fixed delta of 1/tps (deterministic, the
    /// default) instead of the wall clock. Turn off only for soak tests
    /// that want real-time behaviour.
    #[serde(default = "default_true")]
    pub simulated_time: bool,
    /// Run every schedule on Bevy's SingleThreadedExecutor (deterministic
    /// system order for a given schedule graph). Default true: playtests
    /// value reproducibility over throughput.
    #[serde(default = "default_true")]
    pub single_threaded: bool,
    /// Escalate warning-class violations (bevy_warning, log_warn) to
    /// gating severity. Default false: headless runs routinely trip
    /// render/audio warnings that are benign without a GPU.
    #[serde(default)]
    pub strict_warnings: bool,
    #[serde(default = "default_duration")]
    pub duration_s: f32,
    #[serde(default)]
    pub invariants: Vec<Invariant>,
    #[serde(default)]
    pub setup: Setup,
    /// Typed-oracle selection. None = run ALL registered typed oracles;
    /// Some(names) restricts to the listed ones. Unknown names reject at
    /// load time.
    #[serde(default)]
    pub oracles: Option<Vec<String>>,
    /// Liveness configuration (default 2s timeout, real-time mode).
    #[serde(default)]
    pub liveness: LivenessConfig,
}

/// Bot configuration.
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct BotConfig {
    #[serde(rename = "type")]
    pub bot_type: BotType,
    #[serde(default = "default_seed")]
    pub seed: u64,
    #[serde(default = "default_rate")]
    pub input_rate_hz: u32,
    /// synthetic_keyboard: frame-indexed raw key presses.
    #[serde(default)]
    pub key_presses: Vec<KeyPressInput>,
    /// Input surface for the chaos bot: which keys it may press.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_surface: Option<RawSurface>,
}

/// Liveness configuration.
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct LivenessConfig {
    #[serde(default)]
    pub mode: LivenessMode,
    #[serde(default = "default_liveness_timeout")]
    pub timeout_s: f32,
}

fn default_liveness_timeout_impl() -> f32 {
    default_liveness_timeout()
}

impl Default for LivenessConfig {
    fn default() -> Self {
        Self {
            mode: LivenessMode::default(),
            timeout_s: default_liveness_timeout_impl(),
        }
    }
}

/// Liveness mode.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LivenessMode {
    /// Real-time: violation if no liveness signal for timeout_s.
    #[default]
    RealTime,
    /// Disable the liveness oracle (loud: recorded in report warnings).
    Off,
}

/// Raw input surface declaration. Defines what raw inputs are available
/// for the chaos bot to sample from.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawSurface {
    #[serde(default)]
    pub keys: Vec<String>,
}

impl Default for RawSurface {
    fn default() -> Self {
        // Non-empty default surface for zero-contract chaos. Standard set:
        // arrows + WASD (keyboard only).
        Self {
            keys: vec![
                "KeyW".into(),
                "KeyA".into(),
                "KeyS".into(),
                "KeyD".into(),
                "ArrowUp".into(),
                "ArrowDown".into(),
                "ArrowLeft".into(),
                "ArrowRight".into(),
            ],
        }
    }
}

/// synthetic_keyboard: press+release a key at a given frame, through the
/// real keyboard input chain.
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct KeyPressInput {
    pub frame: u64,
    /// Key name: "space", "enter", "escape", "tab", "backspace",
    /// "up"/"down"/"left"/"right", single letters a-z, digits 0-9.
    pub key: String,
    /// Hold the key for N frames (default 1).
    #[serde(default = "default_hold_frames")]
    pub hold_frames: u64,
}

/// A scenario invariant.
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct Invariant {
    pub name: String,
    pub rule: InvariantRule,
    // nodes_in_bounds
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_x: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_x: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_y: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_y: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_z: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_z: Option<f32>,
    /// Entity Names. Empty = all Gameplay entities.
    #[serde(default)]
    pub targets: Vec<String>,
    /// custom thresholds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
    /// custom: "TestApi.score" (numeric) or "TestApi.game_phase" (string).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<CheckOp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_s: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_s: Option<f32>,
    /// Eventually mode: the rule must hold AT LEAST ONCE within this
    /// deadline (seconds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eventually_s: Option<f32>,
}

/// Setup: resets and cheats invoked at scheduled frames.
#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct Setup {
    #[serde(default)]
    pub resets: Vec<ResetCall>,
    /// Cheats invoked at scheduled frames (AUDITED — every invocation
    /// lands in the action log and bumps cheat_count).
    #[serde(default)]
    pub cheats: Vec<CheatCall>,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ResetCall {
    pub kind: String,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct CheatCall {
    pub frame: u64,
    pub kind: String,
}

// ---------------------------------------------------------------------------
// Key-name parsing (bevy-free: maps to canonical KeyCode names, not types)
// ---------------------------------------------------------------------------

/// Canonical key names accepted by scenarios. The in-process driver maps
/// these to `bevy_input::keyboard::KeyCode`; the schema layer only knows
/// strings, keeping `bevy_playtest` bevy-free.
pub const KNOWN_KEY_NAMES: &[&str] = &[
    "Space",
    "Enter",
    "Escape",
    "Tab",
    "Backspace",
    "ArrowUp",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
    "ShiftLeft",
    "ControlLeft",
    "AltLeft",
    "KeyA",
    "KeyB",
    "KeyC",
    "KeyD",
    "KeyE",
    "KeyF",
    "KeyG",
    "KeyH",
    "KeyI",
    "KeyJ",
    "KeyK",
    "KeyL",
    "KeyM",
    "KeyN",
    "KeyO",
    "KeyP",
    "KeyQ",
    "KeyR",
    "KeyS",
    "KeyT",
    "KeyU",
    "KeyV",
    "KeyW",
    "KeyX",
    "KeyY",
    "KeyZ",
    "Digit0",
    "Digit1",
    "Digit2",
    "Digit3",
    "Digit4",
    "Digit5",
    "Digit6",
    "Digit7",
    "Digit8",
    "Digit9",
];

/// Normalize a scenario key name to its canonical `KeyCode` spelling
/// (e.g. "w" -> "KeyW", "space" -> "Space"). Returns `None` for unknown
/// names. The driver converts the canonical name to a real KeyCode.
pub fn normalize_key_name(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    let canonical: &'static str = match lower.as_str() {
        "space" => "Space",
        "enter" | "return" => "Enter",
        "escape" | "esc" => "Escape",
        "tab" => "Tab",
        "backspace" => "Backspace",
        "up" | "arrowup" => "ArrowUp",
        "down" | "arrowdown" => "ArrowDown",
        "left" | "arrowleft" => "ArrowLeft",
        "right" | "arrowright" => "ArrowRight",
        "shift" | "shiftleft" => "ShiftLeft",
        "ctrl" | "control" | "controlleft" => "ControlLeft",
        "alt" | "altleft" => "AltLeft",
        s => {
            // Accept "KeyA"-style names as well as bare letters and digits.
            if let Some(rest) = s.strip_prefix("key") {
                if rest.len() == 1 {
                    if let Some(c) = rest.chars().next() {
                        if c.is_ascii_lowercase() {
                            return letter_name(c);
                        }
                    }
                }
            }
            if let Some(rest) = s.strip_prefix("digit") {
                if rest.len() == 1 {
                    if let Some(c) = rest.chars().next() {
                        if c.is_ascii_digit() {
                            return digit_name(c);
                        }
                    }
                }
            }
            let mut chars = s.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            match c {
                'a'..='z' => return letter_name(c),
                '0'..='9' => return digit_name(c),
                _ => return None,
            }
        }
    };
    Some(canonical)
}

fn letter_name(c: char) -> Option<&'static str> {
    let idx = c as usize - 'a' as usize;
    KNOWN_KEY_NAMES
        .iter()
        .copied()
        .find(|n| {
            n.len() == 4
                && n.starts_with("Key")
                && n.as_bytes()[3] as char == c.to_ascii_uppercase()
        })
        .filter(|_| idx < 26)
}

fn digit_name(c: char) -> Option<&'static str> {
    KNOWN_KEY_NAMES
        .iter()
        .copied()
        .find(|n| n.len() == 6 && n.starts_with("Digit") && n.as_bytes()[5] as char == c)
}

// ---------------------------------------------------------------------------
// Load-time validation (reject, never run)
// ---------------------------------------------------------------------------

/// "Did you mean" suggestions for typos. Levenshtein distance capped at 2.
pub fn suggest_similar(input: &str, candidates: &[String]) -> Option<String> {
    let lower = input.to_ascii_lowercase();
    let mut best: Option<(usize, &String)> = None;
    for cand in candidates {
        let d = levenshtein(&lower, &cand.to_ascii_lowercase());
        if d <= 2 && best.map(|(bd, _)| d < bd).unwrap_or(true) {
            best = Some((d, cand));
        }
    }
    best.map(|(_, c)| c.clone())
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            cur[j + 1] = (prev[j + 1] + 1)
                .min(cur[j] + 1)
                .min(prev[j] + if ca == cb { 0 } else { 1 });
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

fn key_format_help() -> String {
    "accepted key formats: 'Space'/'space', 'Enter'/'enter', 'Escape', 'Tab', 'Backspace', \
     'ArrowUp'/'up', 'ArrowDown'/'down', 'ArrowLeft'/'left', 'ArrowRight'/'right', 'Shift', \
     'Ctrl', 'Alt', single letters ('w') or 'KeyW' style, digits 0-9"
        .to_string()
}

/// Unknown-key error with a did-you-mean suggestion.
pub fn unknown_key_error(key: &str, at: Option<u64>) -> String {
    let mut msg = format!(
        "unknown key name '{}'{}",
        key,
        at.map(|f| format!(" at frame {f}")).unwrap_or_default()
    );
    let cands: Vec<String> = KNOWN_KEY_NAMES.iter().map(|s| s.to_string()).collect();
    if let Some(sug) = suggest_similar(key, &cands) {
        msg.push_str(&format!(" — did you mean '{sug}'?"));
    }
    msg.push_str(&format!(" ({})", key_format_help()));
    msg
}

/// Errors rejecting a scenario before/at run start. Structured so callers
/// can match on the category instead of parsing strings.
#[derive(Debug, Clone, PartialEq)]
pub enum ScenarioError {
    /// The scenario JSON is structurally invalid (bad field combos).
    Rejected(String),
    /// The game does not implement the contract (missing TestApi).
    ContractMissing(String),
    /// The scenario references a reset kind the game never registered.
    UnknownReset { kind: String, known: Vec<String> },
    /// The scenario references a cheat kind the game never registered.
    UnknownCheat { kind: String, known: Vec<String> },
    /// The scenario references a bot type not implemented in this version.
    NotImplemented(String),
}

impl std::fmt::Display for ScenarioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(msg) => write!(f, "scenario rejected: {msg}"),
            Self::ContractMissing(msg) => write!(f, "contract missing: {msg}"),
            Self::UnknownReset { kind, known } => {
                let base = format!(
                    "unknown reset kind '{}' — known kinds: {}",
                    kind,
                    if known.is_empty() {
                        "(none registered)".to_string()
                    } else {
                        known.join(", ")
                    }
                );
                write!(
                    f,
                    "scenario rejected: {base}{}",
                    suggestion_suffix(kind, known)
                )
            }
            Self::UnknownCheat { kind, known } => {
                let base = format!(
                    "unknown cheat kind '{}' — known kinds: {}",
                    kind,
                    if known.is_empty() {
                        "(none registered)".to_string()
                    } else {
                        known.join(", ")
                    }
                );
                write!(
                    f,
                    "scenario rejected: {base}{}",
                    suggestion_suffix(kind, known)
                )
            }
            Self::NotImplemented(msg) => write!(f, "not implemented: {msg}"),
        }
    }
}

impl std::error::Error for ScenarioError {}

fn suggestion_suffix(input: &str, candidates: &[String]) -> String {
    match suggest_similar(input, candidates) {
        Some(s) => format!(" — did you mean '{s}'?"),
        None => String::new(),
    }
}

/// Validate a scenario. Rejects (never silently degrades) on:
/// - bad tps/duration/liveness values
/// - compute-DoS caps
/// - Time\<Virtual\> max_delta clamp violations
/// - path-traversal in scenario names
/// - unimplemented bot types
/// - duplicate invariant names
/// - inconsistent invariant bounds/time windows
pub fn validate_scenario(scenario: &Scenario) -> Result<(), ScenarioError> {
    if scenario.tps < 1 {
        return Err(ScenarioError::Rejected("tps must be >= 1".into()));
    }
    if !scenario.duration_s.is_finite() || scenario.duration_s <= 0.0 {
        return Err(ScenarioError::Rejected(
            "duration_s must be finite and > 0".into(),
        ));
    }
    // Compute-DoS caps: a hostile scenario with huge tps/duration could
    // otherwise hang the harness or exhaust memory.
    const MAX_TPS: u32 = 1_000;
    const MAX_DURATION_S: f32 = 3_600.0;
    const MAX_TOTAL_TICKS: u64 = 10_000_000;
    if scenario.tps > MAX_TPS {
        return Err(ScenarioError::Rejected(format!(
            "tps {} exceeds the maximum of {MAX_TPS}",
            scenario.tps
        )));
    }
    // Time<Virtual> clamps incoming deltas to max_delta (250ms default) —
    // a manual frame duration above that silently distorts dt. Reject.
    if scenario.simulated_time && 1.0 / scenario.tps as f64 > 0.250 {
        return Err(ScenarioError::Rejected(format!(
            "simulated_time frame duration {:.3}s (tps {}) exceeds the \
             Time<Virtual> max_delta clamp (0.250s) — dt would be silently \
             distorted; raise tps above 4",
            1.0 / scenario.tps as f64,
            scenario.tps
        )));
    }
    if scenario.duration_s > MAX_DURATION_S {
        return Err(ScenarioError::Rejected(format!(
            "duration_s {} exceeds the maximum of {MAX_DURATION_S}",
            scenario.duration_s
        )));
    }
    // Reject NaN/negative liveness timeouts — they silently disable
    // frozen_world detection (timeout_frames 0).
    if !scenario.liveness.timeout_s.is_finite() || scenario.liveness.timeout_s <= 0.0 {
        return Err(ScenarioError::Rejected(format!(
            "liveness.timeout_s must be finite and > 0, got {}",
            scenario.liveness.timeout_s
        )));
    }
    // A timeout SMALLER than one frame gives timeout_frames 0 (cast
    // truncates) — frozen_world silently disabled. Reject up front.
    if (scenario.liveness.timeout_s * scenario.tps as f32) as u64 == 0 {
        return Err(ScenarioError::Rejected(format!(
            "liveness.timeout_s {} is shorter than one frame at tps {} \
             (frozen-world detection would be silently disabled)",
            scenario.liveness.timeout_s, scenario.tps
        )));
    }
    // The liveness mode knob is not implemented: "off" must reject
    // loudly instead of silently behaving like real_time.
    if scenario.liveness.mode == LivenessMode::Off {
        return Err(ScenarioError::NotImplemented(
            "liveness.mode 'off' is not implemented — the liveness oracle \
             always runs; widen timeout_s to accommodate quiet phases"
                .into(),
        ));
    }
    // Typed-oracle selection is not implemented. `Some(vec![])` is as
    // much a selection as a populated list (and reads as "none" to a
    // human author) — any non-None value must reject loudly.
    if scenario.oracles.is_some() {
        return Err(ScenarioError::NotImplemented(
            "oracles selection is not implemented — all registered \
             oracles always run; omit the oracles field"
                .into(),
        ));
    }
    let total_ticks = (scenario.duration_s * scenario.tps as f32).ceil() as u64;
    if total_ticks > MAX_TOTAL_TICKS {
        return Err(ScenarioError::Rejected(format!(
            "total ticks ({total_ticks}) exceed the maximum of {MAX_TOTAL_TICKS} \
             (duration_s {} x tps {})",
            scenario.duration_s, scenario.tps
        )));
    }
    // Path safety: scenario.name may be embedded into report file names.
    // Reject anything outside [A-Za-z0-9_-] so path traversal cannot
    // write outside the report directory.
    if let Some(name) = &scenario.name {
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(ScenarioError::Rejected(format!(
                "scenario.name '{name}' is invalid: names may only contain \
                 [A-Za-z0-9_-] and must be non-empty",
            )));
        }
    }
    // Bot-specific requirements.
    if !scenario.bot.bot_type.implemented() {
        return Err(ScenarioError::NotImplemented(format!(
            "bot type '{}' is not implemented in this version \
             (implemented: chaos, synthetic_keyboard)",
            scenario.bot.bot_type
        )));
    }
    if scenario.bot.bot_type == BotType::SyntheticKeyboard {
        if scenario.bot.key_presses.is_empty() {
            return Err(ScenarioError::Rejected(
                "synthetic_keyboard bot requires a non-empty key_presses list".into(),
            ));
        }
        for kp in &scenario.bot.key_presses {
            if normalize_key_name(&kp.key).is_none() {
                return Err(ScenarioError::Rejected(unknown_key_error(
                    &kp.key,
                    Some(kp.frame),
                )));
            }
        }
    }
    // Chaos-bot input surface: every declared key must be a real,
    // recognized key the bot can actually press; an empty or bogus
    // surface yields a bot that silently does nothing.
    if scenario.bot.bot_type == BotType::Chaos {
        if let Some(surface) = &scenario.bot.raw_surface {
            for key in &surface.keys {
                if normalize_key_name(key).is_none() {
                    return Err(ScenarioError::Rejected(unknown_key_error(key, None)));
                }
            }
            if surface.keys.is_empty() {
                return Err(ScenarioError::Rejected(
                    "raw_surface.keys must not be empty — a chaos bot with no \
                     keys can do nothing (vacuous run)"
                        .into(),
                ));
            }
        }
    }
    // Scheduled cheats: the `frame` field parses but all cheats fire
    // pre-run in v0.1. A nonzero frame claims scheduling that doesn't
    // exist.
    for (idx, cheat) in scenario.setup.cheats.iter().enumerate() {
        if cheat.frame != 0 {
            return Err(ScenarioError::NotImplemented(format!(
                "cheats[{idx}].frame {}: scheduled cheats are not implemented in \
                 v0.1 — all cheats fire pre-run (frame 0); omit the frame field",
                cheat.frame
            )));
        }
    }
    // Invariant names key the eventual-state and dedup tables —
    // duplicates would silently shadow each other.
    let mut seen = HashSet::new();
    for rule in &scenario.invariants {
        if !seen.insert(rule.name.as_str()) {
            return Err(ScenarioError::Rejected(format!(
                "duplicate invariant name '{}' — names must be unique",
                rule.name
            )));
        }
    }
    for rule in &scenario.invariants {
        if rule.rule == InvariantRule::Custom {
            return Err(ScenarioError::NotImplemented(format!(
                "invariant '{}': custom invariants are not implemented in v0.1 \
                 (only 'nodes_in_bounds' is wired)",
                rule.name
            )));
        }
        if rule.eventually_s.is_some() {
            return Err(ScenarioError::NotImplemented(format!(
                "invariant '{}': eventually mode is not implemented in v0.1",
                rule.name
            )));
        }
        if !rule.targets.is_empty() {
            return Err(ScenarioError::NotImplemented(format!(
                "invariant '{}': target filtering is not implemented in v0.1 \
                 (checks apply to all Gameplay entities)",
                rule.name
            )));
        }
        if rule.after_s.is_some() || rule.before_s.is_some() {
            return Err(ScenarioError::NotImplemented(format!(
                "invariant '{}': time windows (after_s/before_s) are not \
                 implemented in v0.1",
                rule.name
            )));
        }
    }
    for rule in &scenario.invariants {
        let bound_pairs = [
            (rule.min_x, rule.max_x, "x"),
            (rule.min_y, rule.max_y, "y"),
            (rule.min_z, rule.max_z, "z"),
        ];
        for (min, max, axis) in bound_pairs {
            if let (Some(lo), Some(hi)) = (min, max) {
                if lo > hi {
                    return Err(ScenarioError::Rejected(format!(
                        "invariant '{}' has min_{} ({}) > max_{} ({})",
                        rule.name, axis, lo, axis, hi
                    )));
                }
            }
        }
    }
    Ok(())
}
