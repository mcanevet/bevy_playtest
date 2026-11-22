//! Scenario schema: DTOs, defaults, and load-time validation.
//!
//! Deferred subsystems (golden trajectories, determinism checks,
//! planner goals, choice-replay, ambiguity gates, expert-rule
//! invariants, synthetic-pointer) are absent from the schema entirely
//! rather than accepted-and-ignored. Scenarios carrying those
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
    /// Escalate warning-class violations (log_warn) to gating
    /// severity. Default false: headless runs routinely trip
    /// render/audio warnings that are benign without a GPU.
    #[serde(default)]
    pub strict_warnings: bool,
    #[serde(default = "default_duration")]
    pub duration_s: f32,
    #[serde(default)]
    pub invariants: Vec<Invariant>,
    /// Typed-oracle selection. None = run ALL registered typed oracles;
    /// Some(names) restricts to the listed ones. Unknown names reject at
    /// load time.
    #[serde(default)]
    pub oracles: Option<Vec<String>>,
    /// Liveness configuration (default 2s timeout, real-time mode).
    #[serde(default)]
    pub liveness: LivenessConfig,
    /// Replay-divergence verification: run the scenario twice against
    /// freshly built Apps and compare per-frame fingerprints. Default
    /// false.
    #[serde(default)]
    pub verify_replay: bool,
    /// Broadened state digest recording: when true, record a u64
    /// per-frame digest covering entity/archetype counts, Transform/
    /// UiTransform translations, and reflected game resources, and
    /// attach the series to the report as `state_digest`. Default
    /// false (zero-cost opt-in). When combined with `verify_replay`,
    /// the digest series of both passes are additionally compared and
    /// a divergence fails the run.
    #[serde(default)]
    pub record_digest: bool,
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
    /// synthetic_mouse (any bot): frame-indexed mouse clicks. Unlike
    /// keys, clicks work for both bot types — they are injected at the
    /// same layer the chaos bot samples.
    #[serde(default)]
    pub mouse_clicks: Vec<MouseClickInput>,
    /// Frame-indexed scripted mouse motion (any bot), orthogonal to
    /// clicks and keys: apply `(dx, dy)` per frame for
    /// `repeat_frames` frames.
    #[serde(default)]
    pub mouse_motion: Vec<MotionInput>,
    /// Input surface for the chaos bot: which keys it may press.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_surface: Option<RawSurface>,
    /// Chaos bot only: explore instead of picking uniformly at random.
    /// Remembers what it tried on each screen (the set of enabled
    /// widgets) and prefers the least-tried key / widget there, presses
    /// Escape to back out after a few seconds without anything new, and
    /// adds an `exploration` section (screens seen, widgets never
    /// activated, saturation verdict) to the report. Still seeded and
    /// replay-faithful: ties are broken by the recorded random draws.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub explore: bool,
    /// Opt in to an input-free run: the bot emits NO inputs, making
    /// the scenario purely observational. Without this flag an empty
    /// synthetic_keyboard input list is rejected at load (a bot that
    /// never acts is usually an authoring slip — a typo'd list, a
    /// dropped key), so observe-only scenarios were unexpressible.
    /// Explicit opt-in keeps the guard for the accidental case while
    /// making intentional observation expressible (the same
    /// unjudgeable-vs-rejected trade as the window_s fix). Chaos bots
    /// may also declare it, though an idle chaos bot is equivalent
    /// to an empty synthetic_keyboard.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub passive: bool,
}

/// A scripted mouse click: position + button + hold, frame-indexed.
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct MouseClickInput {
    pub frame: u64,
    /// Window-space cursor position in logical pixels.
    #[serde(default = "default_click_position")]
    pub x: f32,
    #[serde(default = "default_click_position")]
    pub y: f32,
    /// Button name: "left", "right", "middle", "back", "forward".
    #[serde(default = "default_click_button")]
    pub button: String,
    /// Hold the button for N frames (default 1, mirroring key_presses;
    /// chaos holds 2 to avoid same-frame press+release cancellation).
    #[serde(default = "default_hold_frames")]
    pub hold_frames: u64,
}

fn default_click_position() -> f32 {
    0.0
}

fn default_click_button() -> String {
    "left".into()
}

/// A scripted mouse-motion delta: apply `(dx, dy)` every frame for
/// `repeat_frames` frames starting at `frame`. Orthogonal to the bot
/// type like `mouse_clicks`; the driver advances the cursor position
/// by the delta each frame and writes BOTH the raw `MouseMotion`
/// and the wrapped `WindowEvent::CursorMoved` (winit parity — real
/// winit sources them from different event streams; the takeover
/// synthesizes both from one position stream).
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct MotionInput {
    /// Frame index the motion starts at.
    pub frame: u64,
    /// Logical-pixel delta applied to the cursor each frame.
    pub dx: f32,
    /// See [`Self::dx`].
    pub dy: f32,
    /// Apply the delta for N frames (default 1; 0 is rejected — a
    /// zero-repeat motion writes nothing and would silently no-op).
    #[serde(default = "default_repeat_frames")]
    pub repeat_frames: u64,
}

fn default_repeat_frames() -> u64 {
    1
}

/// Liveness configuration.
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct LivenessConfig {
    #[serde(default)]
    pub mode: LivenessMode,
    #[serde(default = "default_liveness_timeout")]
    pub timeout_s: f32,
    /// Progress-liveness oracle configuration (default: auto mode,
    /// 8.0s window, 0.25s samples). See [`ProgressConfig`].
    #[serde(default, skip_serializing_if = "progress_is_default")]
    pub progress: ProgressConfig,
}

fn progress_is_default(p: &ProgressConfig) -> bool {
    *p == ProgressConfig::default()
}

fn default_liveness_timeout_impl() -> f32 {
    default_liveness_timeout()
}

impl Default for LivenessConfig {
    fn default() -> Self {
        Self {
            mode: LivenessMode::default(),
            timeout_s: default_liveness_timeout_impl(),
            progress: ProgressConfig::default(),
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

// ---------------------------------------------------------------------------
// Progress-liveness configuration
// ---------------------------------------------------------------------------

fn default_progress_window() -> f32 {
    8.0
}

fn default_progress_sample() -> f32 {
    0.25
}

/// What the progress oracle treats as the observable game-state surface.
///
/// - `auto`: counts-only breadth (entity count, archetype count,
///   reflected component-type counts) ∪ marker values ∪
///   `include_paths`-scoped resource values. The counts-only core
///   covers structural progress (spawns, archetype changes,
///   component add/remove) without reflection requirements; the
///   value-bearing parts are additive.
/// - `markers`: only the named markers (see [`ProgressConfig`]).
///   Narrowest and cheapest; requires the game to name its signals.
/// - `off`: the progress oracle does not run at all.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProgressMode {
    #[default]
    Auto,
    Markers,
    Off,
}

/// Progress-liveness configuration: distinguishes a churning-but-dead
/// world (frames keep ticking, no game state evolves) from one that is
/// actually progressing.
///
/// Sampling: the recorder samples the progress surface every
/// `sample_s` seconds of simulated time and evaluates one window every
/// `window_s` seconds (per-window recompute; no cross-window state).
/// Both periods are quantized to whole frames and the window to a
/// whole number of sample intervals (see `ProgressRecorder::new`);
/// authored configs off that grid are rejected by validation.
///
/// Suppression of heartbeat/ticker values: any marker (or, in `auto`,
/// include_paths resource) whose value changes in ≥90% of a window's
/// samples is treated as a ticker for that window and does not count
/// as progress evidence.
///
/// Violation: `stalled_progress` fires for a window where the run was
/// churning (frozen-world saw zero changes) at every sampled frame AND
/// no non-ticker surface change occurred.
///
/// An empty surface (nothing verifiable at all) yields the
/// informational `progress_surface_empty` instead.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProgressConfig {
    /// Surface selection mode (default `auto`).
    #[serde(default)]
    pub mode: ProgressMode,
    /// Named reflected RESOURCES whose values are hashed into the
    /// surface (full type path or short name, resolved like invariant
    /// targets). These are the game's declared progress markers —
    /// score, lap counters, phase enums, objective states.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<String>,
    /// Extra resource type paths whose VALUES are hashed into the
    /// surface in `auto` mode. Entries ending in `::` act as path
    /// PREFIXES matching every reflected resource under that module
    /// path (e.g. `"mygame::"` watches `mygame::sim::Score` and
    /// `mygame::clock::Clock` alike); other entries resolve as exact
    /// full type paths or unambiguous short names, failing loudly on
    /// typos or ambiguity. Without this, `auto` covers counts only
    /// for resources — resource values require explicit scoping to
    /// avoid hashing volatile engine-adjacent state.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include_paths: Vec<String>,
    /// Resource type paths (full-path prefix match) to exclude from
    /// the surface — e.g. known tickers or third-party cache types
    /// that would fake progress. Subtracts everywhere, in every mode.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
    /// Window length in seconds: one progress verdict per window.
    #[serde(default = "default_progress_window")]
    pub window_s: f32,
    /// Sampling period in seconds within a window.
    #[serde(default = "default_progress_sample")]
    pub sample_s: f32,
    /// Escalate `progress_surface_empty` from informational to gating.
    /// Set when the scenario's purpose REQUIRES verifiable progress
    /// (regression gateways); leave off for exploratory playtests.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub require_surface: bool,
}

impl Default for ProgressConfig {
    fn default() -> Self {
        Self {
            mode: ProgressMode::default(),
            markers: Vec::new(),
            include_paths: Vec::new(),
            exclude: Vec::new(),
            window_s: default_progress_window(),
            sample_s: default_progress_sample(),
            require_surface: false,
        }
    }
}

/// Raw input surface declaration. Defines what raw inputs are available
/// for the chaos bot to sample from.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawSurface {
    /// Keys for chaos sampling. Default: the standard WASD+arrows
    /// surface — an explicitly empty list opts out of chaos keys
    /// (mouse-only chaos).
    #[serde(default = "default_surface_keys")]
    pub keys: Vec<String>,
    /// Mouse buttons for chaos sampling. Default: all three primary
    /// buttons; an explicitly empty list opts out of chaos clicks.
    #[serde(default = "default_mouse_buttons")]
    pub mouse_buttons: Vec<String>,
}

fn default_mouse_buttons() -> Vec<String> {
    vec!["Left".into(), "Right".into(), "Middle".into()]
}

/// The default chaos keyboard backbone: WASD + arrows. Movement keys
/// reward repetition (position integration compounds), so this is
/// the FALLBACK surface — what the bot samples when no better
/// information exists: no source scan, a failed/empty scan, or a
/// scan yielding no usable keys. Single source of truth for the
/// runtime default surface, the driver's inline default, and
/// [`crate::discovery::source_scan::layered_chaos_keys`] (whose
/// scan-hit arm builds the surface INSTEAD of this one).
pub const BACKBONE_KEYS: &[&str] = &[
    "KeyW",
    "KeyA",
    "KeyS",
    "KeyD",
    "ArrowUp",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
];

/// Default chaos keyboard surface: WASD + arrows (kept in lockstep with
/// the runtime default).
fn default_surface_keys() -> Vec<String> {
    BACKBONE_KEYS.iter().map(|s| s.to_string()).collect()
}

impl Default for RawSurface {
    fn default() -> Self {
        Self {
            keys: default_surface_keys(),
            mouse_buttons: default_mouse_buttons(),
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
    /// Target selector. `all` (zero-annotation) applies
    /// `nodes_in_bounds` to every entity with a Transform;
    /// `with_component` names a registered component type for
    /// `component_check` (see `TargetSelector`).
    #[serde(default)]
    pub target_selector: Option<crate::enums::TargetSelector>,
    /// Threshold compared against by `component_check` (numeric).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
    /// Reflect path to the numeric leaf `component_check` compares
    /// (e.g. `translation.z`, `hp`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Comparison operator for `component_check`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<CheckOp>,
    /// Registered resource type name (`resource_check` /
    /// `stagnant_resource`): the full registered type path, or an
    /// unambiguous short path (same resolution rules as
    /// `target_selector.with_component`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    /// Rate-form (`stagnant_resource` only): reflect path to the
    /// numeric DRIVER leaf that must strictly advance while the
    /// watched leaf stays constant (e.g. a match clock). Defaults to
    /// the same resource as `resource`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver_resource: Option<String>,
    /// Rate-form (`stagnant_resource` only): reflect path to the
    /// numeric driver leaf (on `driver_resource`, default
    /// `resource`) that must strictly advance for the window to
    /// accumulate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver_path: Option<String>,
    /// Rate-form (`stagnant_resource` only): the watched leaf must
    /// stay constant for at most this many seconds of driver time
    /// before the invariant fires. Must fit inside the run
    /// (a window longer than the run can never elapse).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_s: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_s: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_s: Option<f32>,
    /// Eventually mode (`component_check` and `resource_check`): the
    /// predicate must hold for all selected entities (component) /
    /// for the resource leaf (resource) at least once within this
    /// deadline (seconds). Without it, the predicate must hold on
    /// every frame where the selector matched anything / the
    /// resource was present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eventually_s: Option<f32>,
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

/// Normalize a mouse-button name to its canonical spelling
/// (e.g. "left" -> "Left"). Returns `None` for unknown names.
pub fn normalize_mouse_button_name(name: &str) -> Option<&'static str> {
    let canonical = match name.to_ascii_lowercase().as_str() {
        "left" => "Left",
        "right" => "Right",
        "middle" => "Middle",
        "back" => "Back",
        "forward" => "Forward",
        _ => return None,
    };
    Some(canonical)
}

/// Known mouse-button names, for did-you-mean suggestions.
pub const KNOWN_MOUSE_BUTTONS: &[&str] = &["left", "right", "middle", "back", "forward"];

/// Unknown-mouse-button error with a did-you-mean suggestion.
pub fn unknown_mouse_button_error(button: &str, at: Option<u64>) -> String {
    let mut msg = format!(
        "unknown mouse button '{}'{}",
        button,
        at.map(|f| format!(" at frame {f}")).unwrap_or_default()
    );
    let cands: Vec<String> = KNOWN_MOUSE_BUTTONS.iter().map(|s| s.to_string()).collect();
    if let Some(sug) = suggest_similar(button, &cands) {
        msg.push_str(&format!(" — did you mean '{sug}'?"));
    }
    msg.push_str(" (valid: left, right, middle, back, forward)");
    msg
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
    /// The scenario references a bot type not implemented in this version.
    NotImplemented(String),
}

impl std::fmt::Display for ScenarioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(msg) => write!(f, "scenario rejected: {msg}"),
            Self::NotImplemented(msg) => write!(f, "not implemented: {msg}"),
        }
    }
}

impl std::error::Error for ScenarioError {}

/// Collect every liveness.progress configuration error, in check
/// order (the aggregate-all form the lint walker needs). Same checks,
/// same order as the previous first-error form — only the
/// aggregation differs, and the default-config / mode-off exemptions
/// behave identically (early `return`).
fn validate_progress_config(
    progress: &ProgressConfig,
    scenario: &Scenario,
    errors: &mut Vec<ScenarioError>,
) {
    if progress.mode == ProgressMode::Off {
        return; // Oracle disabled: nothing further to check.
    }
    if progress.mode == ProgressMode::Markers && progress.markers.is_empty() {
        errors.push(ScenarioError::Rejected(
            "liveness.progress.mode 'markers' requires a non-empty markers \
             list — without markers the oracle would watch nothing and \
             silently pass everything"
                .into(),
        ));
    }
    if !progress.window_s.is_finite() || progress.window_s <= 0.0 {
        errors.push(ScenarioError::Rejected(format!(
            "liveness.progress.window_s must be finite and > 0, got {}",
            progress.window_s
        )));
    }
    if !progress.sample_s.is_finite() || progress.sample_s <= 0.0 {
        errors.push(ScenarioError::Rejected(format!(
            "liveness.progress.sample_s must be finite and > 0, got {}",
            progress.sample_s
        )));
    }
    // Blank entries are typos in intent — indistinguishable from a
    // mistyped name, which fails loudly at resolution. Dropping them
    // silently would hide a broken config behind partial coverage.
    for entry in progress.markers.iter().chain(progress.exclude.iter()) {
        if entry.trim().is_empty() {
            errors.push(ScenarioError::Rejected(
                "liveness.progress.markers/exclude entries must not be \
                 blank — a blank entry is indistinguishable from a \
                 typo, and silently dropping it hides a broken config"
                    .into(),
            ));
        }
    }
    // Timing-coupled validation applies only to AUTHORED timings.
    // Like the whole-config exemption it replaces, this protects the
    // DEFAULT config that rides along every scenario — including short
    // smoke runs where progress is not the point — but keyed on the
    // timing fields alone, so toggling a NON-timing field (e.g.
    // `require_surface`) does not suddenly drag grid validation onto
    // timings the author never wrote. Structural equality is the
    // doctrine: a config whose window_s/sample_s equal the defaults
    // behaves exactly like an untouched one.
    if progress.window_s == default_progress_window()
        && progress.sample_s == default_progress_sample()
    {
        return;
    }
    if progress.window_s >= scenario.duration_s {
        errors.push(ScenarioError::Rejected(format!(
            "liveness.progress.window_s {} must be shorter than the \
             scenario duration_s {} — otherwise fewer than one window \
             ever completes and the oracle can never fire",
            progress.window_s, scenario.duration_s
        )));
    }
    let frame_s = 1.0 / scenario.tps as f32;
    if progress.sample_s < frame_s {
        errors.push(ScenarioError::Rejected(format!(
            "liveness.progress.sample_s {} is shorter than one frame at \
             tps {} ({:.6}s) — sub-frame sampling collapses to repeated \
             identical samples; raise sample_s to at least one frame",
            progress.sample_s, scenario.tps, frame_s
        )));
    }
    if progress.sample_s >= progress.window_s {
        errors.push(ScenarioError::Rejected(format!(
            "liveness.progress.sample_s {} must be shorter than \
             window_s {} — otherwise fewer than two samples fall in a \
             window and the change-detection degenerates",
            progress.sample_s, progress.window_s
        )));
    }
    // Sampling-grid coherence (authored configs only; the default
    // config early-returns above and is re-quantized coherently by the
    // recorder instead). The oracle samples every
    // round(sample_s·tps) frames and tiles windows of a WHOLE number
    // of those intervals; periods off that grid leave windows no
    // sample lands on the boundary of, and verdicts miscover the
    // observed time. Reject at authoring time, with the derived frame
    // numbers named, rather than silently re-rounding.
    let sample_product = progress.sample_s * scenario.tps as f32;
    let sample_frames = sample_product.round();
    if (sample_product - sample_frames).abs() > 1e-3 {
        errors.push(ScenarioError::Rejected(format!(
            "liveness.progress.sample_s {} must be exactly a whole \
             number of frames at tps {}: sample_s·tps = {} — pick a \
             sample_s that is a whole frame count ({} frames, i.e. \
             sample_s {} at this tps)",
            progress.sample_s,
            scenario.tps,
            sample_product,
            sample_frames as u64,
            sample_frames as u64 as f32 / scenario.tps as f32
        )));
    }
    let sample_frames = sample_frames.max(1.0) as u64;
    let window_product = progress.window_s * scenario.tps as f32;
    let window_frames = (window_product / sample_frames as f32).round();
    if (window_product / sample_frames as f32 - window_frames).abs() > 1e-3 {
        errors.push(ScenarioError::Rejected(format!(
            "liveness.progress.window_s {} must be a whole number of \
             sample intervals at tps {}: got sample_frames {}, authored \
             window_s·tps = {} frames — the nearest coherent window is \
             {} frames ({} intervals), not the authored value — pick a \
             window_s equal to a whole multiple of sample_s",
            progress.window_s,
            scenario.tps,
            sample_frames,
            window_product,
            window_frames as u64 * sample_frames,
            window_frames as u64
        )));
    }
}

/// Collect EVERY validation error in the scenario, in check order —
/// the aggregate-all-errors form the lint mode (`BEVY_PLAYTEST_LINT`)
/// needs: one pass shows the author every rejection at once instead of
/// one error per full run.
///
/// Adds NOTHING to the accepted/rejected boundary: it walks exactly
/// the checks of [`validate_scenario`], in the same order — a scenario
/// is accepted by `validate_scenario` iff this list is empty. Lint
/// only front-loads identical findings. Errors are `ScenarioError`s
/// (structured, same taxonomy as a normal run's rejection), each
/// already naming its invariant where the underlying check does.
pub fn lint_scenario(scenario: &Scenario) -> Vec<ScenarioError> {
    let mut errors = Vec::new();
    if scenario.tps < 1 {
        errors.push(ScenarioError::Rejected("tps must be >= 1".into()));
    }
    if !scenario.duration_s.is_finite() || scenario.duration_s <= 0.0 {
        errors.push(ScenarioError::Rejected(
            "duration_s must be finite and > 0".into(),
        ));
    }
    // Compute-DoS caps: a hostile scenario with huge tps/duration could
    // otherwise hang the harness or exhaust memory.
    const MAX_TPS: u32 = 1_000;
    const MAX_DURATION_S: f32 = 3_600.0;
    const MAX_TOTAL_TICKS: u64 = 10_000_000;
    if scenario.tps > MAX_TPS {
        errors.push(ScenarioError::Rejected(format!(
            "tps {} exceeds the maximum of {MAX_TPS}",
            scenario.tps
        )));
    }
    // Time<Virtual> clamps incoming deltas to max_delta (250ms default) —
    // a manual frame duration above that silently distorts dt. Reject.
    if scenario.simulated_time && 1.0 / scenario.tps as f64 > 0.250 {
        errors.push(ScenarioError::Rejected(format!(
            "simulated_time frame duration {:.3}s (tps {}) exceeds the \
             Time<Virtual> max_delta clamp (0.250s) — dt would be silently \
             distorted; raise tps above 4",
            1.0 / scenario.tps as f64,
            scenario.tps
        )));
    }
    if scenario.duration_s > MAX_DURATION_S {
        errors.push(ScenarioError::Rejected(format!(
            "duration_s {} exceeds the maximum of {MAX_DURATION_S}",
            scenario.duration_s
        )));
    }
    // Reject NaN/negative liveness timeouts — they silently disable
    // frozen_world detection (timeout_frames 0).
    if !scenario.liveness.timeout_s.is_finite() || scenario.liveness.timeout_s <= 0.0 {
        errors.push(ScenarioError::Rejected(format!(
            "liveness.timeout_s must be finite and > 0, got {}",
            scenario.liveness.timeout_s
        )));
    }
    // A timeout SMALLER than one frame gives timeout_frames 0 (cast
    // truncates) — frozen_world silently disabled. Reject up front.
    if (scenario.liveness.timeout_s * scenario.tps as f32) as u64 == 0 {
        errors.push(ScenarioError::Rejected(format!(
            "liveness.timeout_s {} is shorter than one frame at tps {} \
             (frozen-world detection would be silently disabled)",
            scenario.liveness.timeout_s, scenario.tps
        )));
    }
    // The liveness mode knob is not implemented: "off" must reject
    // loudly instead of silently behaving like real_time.
    if scenario.liveness.mode == LivenessMode::Off {
        errors.push(ScenarioError::NotImplemented(
            "liveness.mode 'off' is not implemented — the liveness oracle \
             always runs; widen timeout_s to accommodate quiet phases"
                .into(),
        ));
    }
    validate_progress_config(&scenario.liveness.progress, scenario, &mut errors);
    // Typed-oracle selection is not implemented. `Some(vec![])` is as
    // much a selection as a populated list (and reads as "none" to a
    // human author) — any non-None value must reject loudly.
    if scenario.oracles.is_some() {
        errors.push(ScenarioError::NotImplemented(
            "oracles selection is not implemented — all registered \
             oracles always run; omit the oracles field"
                .into(),
        ));
    }
    let total_ticks = (scenario.duration_s * scenario.tps as f32).ceil() as u64;
    if total_ticks > MAX_TOTAL_TICKS {
        errors.push(ScenarioError::Rejected(format!(
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
            errors.push(ScenarioError::Rejected(format!(
                "scenario.name '{name}' is invalid: names may only contain \
                 [A-Za-z0-9_-] and must be non-empty",
            )));
        }
    }
    // Bot-specific requirements.
    if scenario.bot.bot_type == BotType::SyntheticKeyboard {
        // `key_presses` OR `mouse_clicks` must be non-empty: a
        // mouse-only scripted scenario is valid (the docs advertise
        // `mouse_clicks` as orthogonal to the bot's keyboard half).
        // `passive: true` opts OUT: an intentional observe-only run
        // (empty lists, no inputs emitted) — the accidental-typo
        // guard applies only without the explicit opt-in.
        if !scenario.bot.passive
            && scenario.bot.key_presses.is_empty()
            && scenario.bot.mouse_clicks.is_empty()
            && scenario.bot.mouse_motion.is_empty()
        {
            errors.push(ScenarioError::Rejected(
                "synthetic_keyboard bot requires a non-empty key_presses, \
                 mouse_clicks or mouse_motion list (or \"passive\": true for \
                 an intentional observe-only run)"
                    .into(),
            ));
        }
        // Reserved-surface honesty: `passive` promises no inputs, so
        // declaring scripted inputs alongside it is a contradiction
        // the loader must not silently swallow (the inputs would be
        // skipped, or worse, honored while the flag claims
        // observation-only).
        if scenario.bot.passive
            && (!scenario.bot.key_presses.is_empty()
                || !scenario.bot.mouse_clicks.is_empty()
                || !scenario.bot.mouse_motion.is_empty())
        {
            errors.push(ScenarioError::Rejected(
                "passive bot declares scripted inputs (key_presses, \
                 mouse_clicks or mouse_motion) — passive runs emit no \
                 inputs; remove the inputs or drop \"passive\""
                    .into(),
            ));
        }
        for kp in &scenario.bot.key_presses {
            if normalize_key_name(&kp.key).is_none() {
                errors.push(ScenarioError::Rejected(unknown_key_error(
                    &kp.key,
                    Some(kp.frame),
                )));
            }
        }
    }
    // Scripted mouse clicks: every button must be a real button the
    // harness can inject (an unknown name would silently no-op).
    for click in &scenario.bot.mouse_clicks {
        if normalize_mouse_button_name(&click.button).is_none() {
            errors.push(ScenarioError::Rejected(unknown_mouse_button_error(
                &click.button,
                Some(click.frame),
            )));
        }
        if click.hold_frames == 0 {
            errors.push(ScenarioError::Rejected(format!(
                "mouse_clicks at frame {}: hold_frames must be >= 1 (a \
                 zero-frame hold is a same-frame press+release, which \
                 ButtonInput cancels)",
                click.frame
            )));
        }
    }
    // Scripted mouse motion: a zero-repeat entry writes nothing (a
    // silent no-op the author probably meant as "repeat forever"),
    // and NaN/infinite deltas would poison cursor arithmetic
    // silently.
    for motion in &scenario.bot.mouse_motion {
        if motion.repeat_frames == 0 {
            errors.push(ScenarioError::Rejected(format!(
                "mouse_motion at frame {}: repeat_frames must be >= 1 (a \
                 zero-repeat motion writes nothing — a silent no-op)",
                motion.frame
            )));
        }
        if !motion.dx.is_finite() || !motion.dy.is_finite() {
            errors.push(ScenarioError::Rejected(format!(
                "mouse_motion at frame {}: dx/dy must be finite (got \
                 ({}, {})) — NaN/infinite deltas poison cursor arithmetic",
                motion.frame, motion.dx, motion.dy
            )));
        }
        // An entry starting at/past the run end writes nothing across
        // the whole run — the same silent no-op zero-repeat guards
        // against, and the driver's window bound would otherwise add
        // two author-controlled u64s unchecked.
        if motion.frame >= total_ticks {
            errors.push(ScenarioError::Rejected(format!(
                "mouse_motion at frame {}: entry starts at/past the run \
                 end ({} ticks) — it would apply on no frame (a silent \
                 no-op)",
                motion.frame, total_ticks
            )));
        }
    }
    // Chaos-bot input surface: every declared key must be a real,
    // recognized key the bot can actually press; an empty or bogus
    // surface yields a bot that silently does nothing.
    if scenario.bot.bot_type == BotType::Chaos {
        if let Some(surface) = &scenario.bot.raw_surface {
            // Reserved-surface honesty (synthetic twin below): passive
            // promises no inputs, and a chaos bot with a non-empty
            // surface would press them — the flag would be a lie.
            if scenario.bot.passive
                && (!surface.keys.is_empty() || !surface.mouse_buttons.is_empty())
            {
                errors.push(ScenarioError::Rejected(
                    "passive bot declares an input surface (raw_surface with \
                     keys or mouse_buttons) — passive runs emit no inputs; \
                     remove the surface or drop \"passive\""
                        .into(),
                ));
            }
            for key in &surface.keys {
                if normalize_key_name(key).is_none() {
                    errors.push(ScenarioError::Rejected(unknown_key_error(key, None)));
                }
            }
            // An explicitly empty `keys` list opts out of chaos keys
            // (mouse-only chaos) — valid only if the mouse surface is
            // non-empty; a fully-empty surface can do nothing.
            // `passive: true` opts into exactly that: an intentional
            // observe-only run.
            if surface.keys.is_empty() && surface.mouse_buttons.is_empty() && !scenario.bot.passive
            {
                errors.push(ScenarioError::Rejected(
                    "raw_surface is empty — a chaos bot with no keys and no \
                     mouse buttons can do nothing (vacuous run; declare \
                     \"passive\": true for an intentional observe-only run)"
                        .into(),
                ));
            }
            for button in &surface.mouse_buttons {
                if normalize_mouse_button_name(button).is_none() {
                    errors.push(ScenarioError::Rejected(unknown_mouse_button_error(
                        button, None,
                    )));
                }
            }
        }
    }
    // Invariant names key the eventual-state and dedup tables —
    // duplicates would silently shadow each other.
    let mut seen = HashSet::new();
    for rule in &scenario.invariants {
        // The name is the finding's target, and the target is half of
        // a seed-sweep fingerprint line: `#` starts a comment there and
        // a line break splits the line, so such a finding could never
        // be recorded (or would silently match another invariant).
        if rule.name.contains(['#', '\n', '\r']) {
            errors.push(ScenarioError::Rejected(format!(
                "invariant name {:?} may not contain '#' or a line break \
                 (it is the finding target in seed-sweep regression files)",
                rule.name
            )));
        }
        if !seen.insert(rule.name.as_str()) {
            errors.push(ScenarioError::Rejected(format!(
                "duplicate invariant name '{}' — names must be unique",
                rule.name
            )));
        }
    }
    for rule in &scenario.invariants {
        if rule.eventually_s.is_some()
            && !matches!(
                rule.rule,
                InvariantRule::ComponentCheck | InvariantRule::ResourceCheck
            )
        {
            errors.push(ScenarioError::NotImplemented(format!(
                "invariant '{}': eventually mode is only supported for component_check \
                 and resource_check",
                rule.name
            )));
        }
        if rule.after_s.is_some() || rule.before_s.is_some() {
            errors.push(ScenarioError::NotImplemented(format!(
                "invariant '{}': time windows (after_s/before_s) are not \
                 supported",
                rule.name
            )));
        }
        // Per-rule field requirements: keep the reserved `value`/
        // `path`/`check` surface honest — accepted for component_check
        // (required there), rejected for nodes_in_bounds (they belong
        // to an unimplemented rule and would silently pass otherwise).
        match rule.rule {
            InvariantRule::ComponentCheck => {
                let Some(crate::enums::TargetSelector::WithComponent { name }) =
                    rule.target_selector.as_ref()
                else {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': component_check requires \
                         'target_selector: {{\"with_component\": {{\"name\": ...}}}}'",
                        rule.name
                    )));
                    continue;
                };
                if name.trim().is_empty() {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': with_component name must be non-empty",
                        rule.name
                    )));
                }
                if rule.path.is_none() {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': component_check requires 'path' \
                         (a reflect path into the component, e.g. \"translation.z\")",
                        rule.name
                    )));
                }
                if rule.check.is_none() {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': component_check requires 'check' \
                         (one of lt/le/gt/ge/equals/ne)",
                        rule.name
                    )));
                }
                match rule.value.as_ref() {
                    Some(serde_json::Value::Number(n)) => {
                        if n.as_f64().is_none() {
                            errors.push(ScenarioError::Rejected(format!(
                                "invariant '{}': 'value' must be a finite number",
                                rule.name
                            )));
                        }
                    }
                    _ => {
                        errors.push(ScenarioError::Rejected(format!(
                            "invariant '{}': component_check requires a numeric \
                             'value' to compare against",
                            rule.name
                        )));
                    }
                }
                // Bounds fields belong to nodes_in_bounds only:
                // silently ignoring them here would recreate the
                // accepted-and-ignored reserved-field hole.
                if rule.min_x.is_some()
                    || rule.max_x.is_some()
                    || rule.min_y.is_some()
                    || rule.max_y.is_some()
                    || rule.min_z.is_some()
                    || rule.max_z.is_some()
                {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': component_check does not use bounds \
                         fields (min_x/max_x/min_y/max_y/min_z/max_z) — those \
                         belong to nodes_in_bounds; use 'path'/'check'/'value'",
                        rule.name
                    )));
                }
                // The eventually deadline must be a judgeable temporal
                // bound: positive, finite, and inside the run. A
                // deadline past the run's end can never elapse, so the
                // check would be unjudgeable, not failed; a
                // non-positive deadline degenerates to "hold at the
                // very first matched frame or never". Same doctrine
                // as liveness.timeout_s and mouse_motion.frame vs
                // total_ticks.
                if let Some(eventually_s) = rule.eventually_s {
                    if !eventually_s.is_finite() || eventually_s <= 0.0 {
                        errors.push(ScenarioError::Rejected(format!(
                            "invariant '{}': eventually_s must be a finite \
                             number > 0 (got {eventually_s})",
                            rule.name
                        )));
                    }
                    if eventually_s > scenario.duration_s {
                        errors.push(ScenarioError::Rejected(format!(
                            "invariant '{}': eventually_s ({eventually_s}s) exceeds \
                             the run duration ({duration}s) — the deadline can never \
                             elapse within the run, so the check would be \
                             unjudgeable; keep the deadline at or inside the run \
                             window",
                            rule.name,
                            duration = scenario.duration_s
                        )));
                    }
                }
                // The resource-rule vocabulary (resource, window_s,
                // driver_resource/driver_path) extends the reserved
                // surface: accepted here, an author would believe a
                // component_check predicates a game-wide resource or
                // expires after window_s — it silently does neither.
                if rule.resource.is_some() {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': component_check selects entities via \
                         target_selector — 'resource' belongs to resource_check / \
                         stagnant_resource",
                        rule.name
                    )));
                }
                if rule.window_s.is_some() {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': component_check has no window — its \
                         predicate must hold every matched frame (or eventually \
                         within 'eventually_s'); 'window_s' belongs to \
                         stagnant_resource",
                        rule.name
                    )));
                }
                if rule.driver_resource.is_some() || rule.driver_path.is_some() {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': component_check does not use \
                         driver_resource/driver_path — those belong to \
                         stagnant_resource",
                        rule.name
                    )));
                }
            }
            InvariantRule::NodesInBounds => {
                if rule.value.is_some() || rule.path.is_some() || rule.check.is_some() {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': nodes_in_bounds does not use value/path/check",
                        rule.name
                    )));
                }
                // Same reserved-surface honesty as above: the
                // resource-rule fields would be silently ignored.
                if rule.resource.is_some() {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': nodes_in_bounds checks per-entity bounds — \
                         'resource' belongs to resource_check / stagnant_resource",
                        rule.name
                    )));
                }
                if rule.window_s.is_some() {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': nodes_in_bounds does not use window_s — \
                         that belongs to stagnant_resource",
                        rule.name
                    )));
                }
                if rule.driver_resource.is_some() || rule.driver_path.is_some() {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': nodes_in_bounds does not use \
                         driver_resource/driver_path — those belong to \
                         stagnant_resource",
                        rule.name
                    )));
                }
                // The invariant NAME is the finding's TARGET
                // (`invariant_eval` reports `rule=nodes_in_bounds`,
                // `target=inv.name`), while the GLOBAL oracle reports
                // per-entity debug targets that the sweep's
                // fingerprint layer canonicalizes to
                // `nodes_in_bounds:entity`. A name shaped like a bevy
                // entity debug string ("3v1") — or the literal
                // "entity" — would collapse to (or collide with)
                // that canonical key: one regression-file line would
                // then silence two DIFFERENT findings. Reject such
                // names for this rule; component_check names never
                // collapse (different rule string), and
                // finite_transforms has no authored counterpart.
                let name = rule.name.trim();
                let entity_shaped = name == "entity"
                    || name.split_once('v').is_some_and(|(idx, gen)| {
                        !idx.is_empty()
                            && !gen.is_empty()
                            && idx.bytes().all(|b| b.is_ascii_digit())
                            && gen.bytes().all(|b| b.is_ascii_digit())
                    });
                if entity_shaped {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': a nodes_in_bounds name may not look like a bevy \
                         entity — '{name}' would collide with the global oracle's \
                         canonicalized `nodes_in_bounds:entity` fingerprint; pick a \
                         descriptive name",
                        rule.name
                    )));
                }
                // with_component selectors are supported (resolved
                // dynamically against the game's registry by the
                // driver-loop recorder; unknown or ambiguous type
                // names fail loudly as oracle_error at run time).
            }
            InvariantRule::ResourceCheck | InvariantRule::StagnantResource => {
                // Shared shape: a named reflected RESOURCE (never a
                // target_selector — the resource universe is keyed by
                // type path, and reusing the component-selector
                // vocabulary here would invite silent misuse).
                if let Some(_selector) = rule.target_selector.as_ref() {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': {} does not use target_selector — name the \
                         resource via the 'resource' field instead (components would \
                         be matched per entity; resources are game-wide singletons)",
                        rule.name, rule.rule
                    )));
                }
                let Some(resource) = rule.resource.as_deref() else {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': {} requires 'resource' — the registered \
                         resource type name (full path, or an unambiguous short path)",
                        rule.name, rule.rule
                    )));
                    continue;
                };
                if resource.trim().is_empty() {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': 'resource' must be non-empty",
                        rule.name
                    )));
                }
                if let Some(path) = rule.path.as_deref() {
                    if path.trim().is_empty() {
                        errors.push(ScenarioError::Rejected(format!(
                            "invariant '{}': 'path' must be non-empty — a blank \
                             reflect path resolves the resource root, never a \
                             numeric leaf",
                            rule.name
                        )));
                    }
                } else {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': {} requires 'path' — the reflect path to the \
                         numeric leaf on the resource (e.g. \"score.home\")",
                        rule.name, rule.rule
                    )));
                }
                if rule.min_x.is_some()
                    || rule.max_x.is_some()
                    || rule.min_y.is_some()
                    || rule.max_y.is_some()
                    || rule.min_z.is_some()
                    || rule.max_z.is_some()
                {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': {} does not use bounds fields — those belong \
                         to nodes_in_bounds",
                        rule.name, rule.rule
                    )));
                }
                // window_s: required for stagnant_resource (the
                // stagnation budget), forbidden elsewhere (it is the
                // rule's only temporal knob).
                let expect_window = rule.rule == InvariantRule::StagnantResource;
                match rule.window_s {
                    Some(_window_s) if !expect_window => {
                        errors.push(ScenarioError::Rejected(format!(
                            "invariant '{}': window_s belongs to stagnant_resource only",
                            rule.name
                        )));
                    }
                    Some(window_s) => {
                        if !window_s.is_finite() || window_s <= 0.0 {
                            errors.push(ScenarioError::Rejected(format!(
                                "invariant '{}': window_s must be a finite number > 0 \
                                 (got {window_s})",
                                rule.name
                            )));
                        }
                        if window_s > scenario.duration_s {
                            errors.push(ScenarioError::Rejected(format!(
                                "invariant '{}': window_s ({window_s}s) exceeds the run \
                                 duration ({duration}s) — the window can never elapse \
                                 within the run, so the check would be unjudgeable; keep \
                                 the window at or inside the run window",
                                rule.name,
                                duration = scenario.duration_s
                            )));
                        }
                    }
                    None if expect_window => {
                        errors.push(ScenarioError::Rejected(format!(
                            "invariant '{}': stagnant_resource requires 'window_s' — the \
                             maximum number of driver seconds the watched leaf may stay \
                             constant before the invariant fires",
                            rule.name
                        )));
                    }
                    None => {}
                }
                // resource_check-only: the predicate triple
                // (path/check/value), same requirements as
                // component_check. stagnant_resource uses `path` as
                // the WATCHED leaf and rejects the predicate fields:
                // its verdict is "changed within window_s?", never a
                // threshold comparison.
                match rule.rule {
                    InvariantRule::ResourceCheck => {
                        if rule.check.is_none() {
                            errors.push(ScenarioError::Rejected(format!(
                                "invariant '{}': resource_check requires 'check' \
                                 (one of lt/le/gt/ge/equals/ne)",
                                rule.name
                            )));
                        }
                        match rule.value.as_ref() {
                            Some(serde_json::Value::Number(n)) => {
                                if n.as_f64().is_none() {
                                    errors.push(ScenarioError::Rejected(format!(
                                        "invariant '{}': 'value' must be a finite number",
                                        rule.name
                                    )));
                                }
                            }
                            _ => {
                                errors.push(ScenarioError::Rejected(format!(
                                    "invariant '{}': resource_check requires a numeric \
                                     'value' to compare against",
                                    rule.name
                                )));
                            }
                        }
                        if rule.driver_resource.is_some() || rule.driver_path.is_some() {
                            errors.push(ScenarioError::Rejected(format!(
                                "invariant '{}': resource_check does not use \
                                 driver_resource/driver_path — those belong to \
                                 stagnant_resource",
                                rule.name
                            )));
                        }
                        // Resource-check deadline judgeability: same
                        // doctrine as component_check's eventually_s.
                        if let Some(eventually_s) = rule.eventually_s {
                            if !eventually_s.is_finite() || eventually_s <= 0.0 {
                                errors.push(ScenarioError::Rejected(format!(
                                    "invariant '{}': eventually_s must be a finite \
                                     number > 0 (got {eventually_s})",
                                    rule.name
                                )));
                            }
                            if eventually_s > scenario.duration_s {
                                errors.push(ScenarioError::Rejected(format!(
                                    "invariant '{}': eventually_s ({eventually_s}s) \
                                     exceeds the run duration ({duration}s) — the \
                                     deadline can never elapse within the run, so the \
                                     check would be unjudgeable; keep the deadline at \
                                     or inside the run window",
                                    rule.name,
                                    duration = scenario.duration_s
                                )));
                            }
                        }
                    }
                    InvariantRule::StagnantResource => {
                        if rule.check.is_some()
                            || rule.value.is_some()
                            || rule.eventually_s.is_some()
                        {
                            errors.push(ScenarioError::Rejected(format!(
                                "invariant '{}': stagnant_resource does not use \
                                 check/value/eventually_s — it watches whether the \
                                 leaf changes within window_s, never a threshold",
                                rule.name
                            )));
                        }
                        match rule.driver_path.as_deref() {
                            None => {
                                errors.push(ScenarioError::Rejected(format!(
                                    "invariant '{}': stagnant_resource requires \
                                 'driver_path' — the reflect path to the numeric \
                                 driver leaf (on 'resource' by default, or on \
                                 'driver_resource') that must strictly advance for \
                                 the stagnation window to accumulate",
                                    rule.name
                                )));
                            }
                            Some(dp) if dp.trim().is_empty() => {
                                errors.push(ScenarioError::Rejected(format!(
                                    "invariant '{}': 'driver_path' must be non-empty — \
                                     a blank reflect path resolves the resource root, \
                                     never a numeric leaf",
                                    rule.name
                                )));
                            }
                            _ => {}
                        }
                        if let Some(dr) = rule.driver_resource.as_deref() {
                            if dr.trim().is_empty() {
                                errors.push(ScenarioError::Rejected(format!(
                                    "invariant '{}': 'driver_resource' must be non-empty \
                                     — omit it to default the driver to 'resource'",
                                    rule.name
                                )));
                            }
                        }
                        if rule.driver_resource.is_some() && rule.driver_resource == rule.resource {
                            errors.push(ScenarioError::Rejected(format!(
                                "invariant '{}': driver_resource is redundant when it \
                                 names the same resource as 'resource' — omit it \
                                 (the driver leaf defaults to the same resource)",
                                rule.name
                            )));
                        }
                    }
                    _ => unreachable!("arm constrained by the match above"),
                }
            }
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
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}' has min_{} ({}) > max_{} ({})",
                        rule.name, axis, lo, axis, hi
                    )));
                }
            }
        }
    }
    errors
}

/// Validate a scenario. Rejects (never silently degrades) on:
/// - bad tps/duration/liveness values
/// - compute-DoS caps
/// - Time\<Virtual\> max_delta clamp violations
/// - path-traversal in scenario names
/// - unimplemented bot types
/// - duplicate invariant names
/// - inconsistent invariant bounds/time windows
///
/// First-error-only form of [`lint_scenario`]: the FIRST error the
/// walker finds, as a `Result`. Both walk the same checks in the same
/// order, so a normal run's rejection reason and a lint run's first
/// finding are always the same error.
pub fn validate_scenario(scenario: &Scenario) -> Result<(), ScenarioError> {
    match lint_scenario(scenario).into_iter().next() {
        Some(err) => Err(err),
        None => Ok(()),
    }
}
