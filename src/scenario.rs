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
    /// stay constant for at most this many **driver-leaf units** of
    /// advance before the invariant fires — NOT seconds unless the
    /// driver literally counts seconds (a tick counter advancing
    /// +1 per FixedUpdate tick consumes the window in ticks; a
    /// 300-unit window at 20 ticks/s elapses in 15 wall seconds).
    /// The budget is consumed in the driver's own unit; the author's
    /// mental unit is the driver's unit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_s: Option<f32>,
    /// Rate-form (`stagnant_resource` only): optional declaration of
    /// the driver leaf's advance rate, in driver units per wall
    /// second. Enables load-time fit validation: with the rate
    /// declared, `window_s / rate` must fit inside `duration_s`, so
    /// an unelidable window is caught at load with a numerically
    /// true message instead of a rejected-by-assumption one. When
    /// omitted, no duration-coupled rejection applies and the
    /// post-run elision audit (`window_unelapsed`) is the only
    /// judge. Doubles as documentation of the driver's unit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver_units_per_second: Option<f32>,
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
    /// Minimum allowed distance between any two selected entities
    /// (`pairwise_separation` only): ε semantics — this is
    /// "coincident bodies", NOT "personal space". The threshold
    /// must sit BELOW gameplay-speed closing distances or
    /// legitimate fly-bys false-positive; the rule's soak
    /// (`window_s`) discards the rest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_distance: Option<f32>,
    /// Numerical tolerance for the compositional identity
    /// (`derived_transform_consistency` only): the absolute
    /// per-element slack between the observed GlobalTransform and
    /// the parent-composed expectation. Composition of healthy
    /// f32 chains sits at ~1e-6..1e-5; real misses diverge by
    /// whole units, so the recorder's default (when this is
    /// omitted) separates noise from signal by orders of
    /// magnitude. Loosen for deep hierarchies
    /// (per-depth growth is a deliberate author decision here,
    /// not a hidden formula) or tighten for near-identity scenes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tolerance: Option<f32>,
    /// Frames to skip at run start before judging consistency
    /// (`derived_transform_consistency` only): relationship
    /// application and first propagation settle asynchronously
    /// during the startup sequence, so the earliest frames can
    /// legitimately disagree. Whole-run skip; judged frames run
    /// [grace_frames, end).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grace_frames: Option<u32>,
    /// Entity/component exclusion prefixes for the consistency
    /// check (`derived_transform_consistency` only): camera
    /// smoothing ghosts and interpolation layers legitimately
    /// write GlobalTransform after propagation, so they look
    /// incoherent EVERY frame. Entries are component type-path
    /// PREFIXES (the progress-surface exclude grammar): an entity
    /// carrying any component whose registered name starts with
    /// a prefix is skipped. Prefer the in-game marker
    /// (`SkipDerivedConsistency`) for per-instance opt-outs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_components: Vec<String>,
    /// Author-declared no-go zone (`forbidden_region` only): an
    /// axis-aligned BOX in world space. A selected entity whose
    /// Transform translation lies inside it (boundaries INCLUDED)
    /// is incursive; incursion frames fold into per-invariant
    /// episodes judged against the `window_s` soak in driver
    /// units.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<crate::scenario::RegionBox>,
    /// Legitimate-presence exemption (`forbidden_region` only):
    /// an entity carrying this registered component does not
    /// count as incursive (per entity, per frame). THE FLIP-TEST
    /// DISCIPLINE IS MANDATORY (proposal §1): the exemption must
    /// name the component that marks LEGITIMATE presence (e.g. a
    /// "mounted on the outer lane" tag), never the state the buggy
    /// run is in — an exemption that turns the pre-fix litmus
    /// green has exempted the bug, not the feature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_if: Option<String>,
    /// Anchor selector (`spawn_safety` only): the same
    /// `target_selector` grammar (with_component / all), naming
    /// the anchor set every entering entity must keep
    /// `min_distance` away from, snapshotted at the END of the
    /// judging frame (post-update — same-frame anchors count: an
    /// anchor spawned alongside the entry is a participant, not
    /// an excuse).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_distance_from: Option<crate::enums::TargetSelector>,
    /// Spawn-only semantics (`spawn_safety` only): filter judged
    /// entries to brand-new (id, generation) pairs — pre-existing
    /// entities first acquiring the target component are NOT
    /// judged. Default false: component-addition is the judged
    /// event (a converted enemy appearing inside the anchor zone
    /// is the same bug as a respawned one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawn_only: Option<bool>,
    /// Scoped watch path (`entity_stagnation` only): the reflect
    /// path on the selected component whose leaf digests are
    /// compared across samples — e.g. `state.Queued.waited` (a
    /// variant-qualified path) or `hunger` (a plain field). The
    /// watch is SCOPED: a change anywhere OUTSIDE this path does
    /// not clear stagnation, so watching `state` does not care
    /// that `hunger` mutated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch_path: Option<String>,
    /// Precondition (`entity_stagnation` only): a narrow grammar
    /// `path == VariantName` — only entities whose reflected
    /// `path` currently holds `VariantName` are watched (e.g.
    /// `state == Queued`). Absent means no precondition. The
    /// path must resolve and the variant must exist (validated
    /// at probe time — a typo there is an oracle_error, never a
    /// silent "never true"). Redundant with a variant-qualified
    /// `watch_path` (`watch_path: "state.Queued.waited"` implies
    /// `leaf_where: "state == Queued"`) — both present is legal
    /// and idempotent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leaf_where: Option<String>,
    /// Driver discriminator (`entity_stagnation` only): the flat
    /// string naming WHAT leaf advances while the watch stays
    /// digest-constant. `"global_time"` (the default) is the
    /// run's frame clock — seconds, advancerate 1 unit/s by
    /// definition. `"resource"` unlocks the driver triple
    /// (`driver_resource` + `driver_path`, plus the optional
    /// `driver_units_per_second`) VERBATIM — the
    /// stagnant_resource grammar. For a `resource` driver,
    /// `driver_units_per_second` is REQUIRED: the sampling
    /// cadence is scheduled in frames, which needs the driver's
    /// rate to convert `sampling_s` out of driver-leaf units.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver: Option<String>,
    /// Sampling cadence (`entity_stagnation` only): the watched
    /// leaf is read every `sampling_s` of driver-leaf advance
    /// (default 1.0) — sampled evaluation is the rule's cost
    /// story (digest reads at a cadence, not every frame). A
    /// stagnation interval spans two consecutive equal-digest
    /// samples, credited `driver(k) - driver(k-1)` and
    /// telescoped across the episode, so firing granularity is
    /// the cadence: `window_s >= 2 * sampling_s` is validated at
    /// load. Coarsen the cadence to hide sub-interval flutters
    /// (with `sampling_s = 10`, an 8s freeze never spans two
    /// samples and is never stuck at all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sampling_s: Option<f32>,
    /// Crowd radius (`occupancy_cap` only): the neighborhood
    /// size a crowd judgment counts within. Counting is
    /// entity-witnessed (settlement §1): more than `cap`
    /// selected entities within `radius` of a SELECTED ENTITY'S
    /// POSITION (the entity itself included — the count is
    /// self-inclusive, so it is always ≥ 1) sustains a violation;
    /// this is a deliberate approximation of the spec's exact
    /// "any point" — near-collinear circumcenters make exact
    /// counting a float-flake factory, so the ring of entities
    /// spaced just past radius around an empty center is a
    /// documented NON-FIRING case. Must be finite and > 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<f32>,
    /// Crowd cap (`occupancy_cap` only): the maximum number of
    /// selected entities allowed within `radius` of a selected
    /// entity's position (self-inclusive). An INTEGER ≥ 1 —
    /// `cap: 0` is rejected (self-inclusive counting means the
    /// count is always ≥ 1, so a zero cap degenerates to "fire
    /// whenever any entity is selected", a nonsense oracle).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cap: Option<u32>,
}

/// An author-declared axis-aligned box region
/// (`forbidden_region` only): world-space center plus per-axis
/// half-extents. Containment is INCLUSIVE on every face — a
/// translation exactly on the boundary counts as inside (pinned by
/// the on-edge battery test; changing this is a doctrine change,
/// not a fix). NaN anywhere in center or half_extents rejects at
/// load (an unjudgeable region must never reach the recorder).
#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RegionBox {
    /// World-space center `[x, y, z]`. Every element must be
    /// finite.
    pub center: [f32; 3],
    /// Per-axis half-extents `[hx, hy, hz]`. Every element must be
    /// finite and > 0 (a zero-thickness wall is a plane, not a
    /// volume; use positive extents).
    pub half_extents: [f32; 3],
}

impl RegionBox {
    /// INCLUSIVE box containment for a world-space translation:
    /// inside when `|t - center| <= half_extent` on EVERY axis.
    /// NaN translations are never inside (NaN comparisons are
    /// false) — the finite_transforms oracle owns NaN findings.
    pub fn contains(&self, t: [f32; 3]) -> bool {
        (t[0] - self.center[0]).abs() <= self.half_extents[0]
            && (t[1] - self.center[1]).abs() <= self.half_extents[1]
            && (t[2] - self.center[2]).abs() <= self.half_extents[2]
    }
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
    // Passive runs promise no inputs — the OBSERVE-ONLY contract.
    // Both honesty guards are HOISTED out of the per-bot-type
    // blocks below: the promise is bot-independent, and the
    // per-bot split left silent holes in the matrix (a passive
    // chaos scenario declaring scripted mouse inputs loaded
    // fine — and the unconditional scripted mouse system fired
    // those clicks while the report claimed observation-only).
    // Whether the declared inputs would FIRE (scheduled
    // unconditionally) or be DISCARDED (per-bot scheduling) is
    // an implementation detail; declaring them at all contradicts
    // the flag, so the loader must reject either way. Either
    // side can give: remove the inputs or drop "passive".
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
    if let Some(surface) = &scenario.bot.raw_surface {
        if scenario.bot.passive && (!surface.keys.is_empty() || !surface.mouse_buttons.is_empty()) {
            errors.push(ScenarioError::Rejected(
                "passive bot declares an input surface (raw_surface with \
                 keys or mouse_buttons) — passive runs emit no inputs; \
                 remove the surface or drop \"passive\""
                    .into(),
            ));
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
    // Cross-invariant rate consistency: one driver leaf has ONE
    // advance rate — two invariants watching the same
    // (driver_resource, driver_path) pair may not declare different
    // `driver_units_per_second` values (a lie about the game's clock
    // either way; the pair with the wrong rate gets silently
    // mis-fit validation). Full paths are compared as full paths —
    // two genuinely different types that merely share a final
    // segment (`a::clock::TickCount` vs `b::TickCount`) are distinct
    // drivers, exactly as runtime resolution treats them; a SHORT
    // name on either side collapses by final segment, since a short
    // name is an unambiguous alias of exactly one registered full
    // path. `my_game::clock::TickCount` and `TickCount` are one
    // driver, not two; two full paths differing anywhere are two. Two FULL paths
    // with the same final segment but different crates/modules are
    // DISTINCT types, however — runtime resolution matches them by
    // full path — so they keep separate keys; final-segment
    // collapsing applies only when a short name is involved on
    // either side.
    let final_segment = |path: &str| path.rsplit("::").next().unwrap_or(path).to_string();
    // Entry (name, path, rate). Full paths must match by full string
    // (distinct modules with the same final segment are distinct
    // types at runtime resolution); a short name on either side
    // makes final-segment comparison the join — mirroring
    // reflect_resource, where a short name IS an unambiguous alias
    // of exactly one full path. The driver PATH keeps every leaf
    // distinct: one resource can drive several leaves.
    let mut declared: Vec<(String, String, f32)> = Vec::new();
    let same_leaf = |a: &str, b: &str| -> bool {
        if a.contains("::") && b.contains("::") {
            a == b
        } else {
            final_segment(a) == final_segment(b)
        }
    };
    for rule in &scenario.invariants {
        // Single-owner optional fields (derived_transform_consistency
        // only): accepted-and-ignored on any other rule would be the
        // reserved-field hole in reverse — the author writes a knob,
        // the harness parses it, no rule ever reads it.
        if rule.rule != InvariantRule::DerivedTransformConsistency {
            let mut foreign: Vec<&str> = Vec::new();
            if rule.tolerance.is_some() {
                foreign.push("'tolerance'");
            }
            if rule.grace_frames.is_some() {
                foreign.push("'grace_frames'");
            }
            if !rule.exclude_components.is_empty() {
                foreign.push("'exclude_components'");
            }
            if !foreign.is_empty() {
                errors.push(ScenarioError::Rejected(format!(
                    "invariant '{}': {} {} not supported by {} — {} \
                     derived_transform_consistency only",
                    rule.name,
                    foreign.join(", "),
                    if foreign.len() == 1 { "is" } else { "are" },
                    rule.rule,
                    if foreign.len() == 1 {
                        "this field is"
                    } else {
                        "these fields are"
                    }
                )));
            }
        }
    }
    for rule in &scenario.invariants {
        // Single-owner optional fields (spawn_safety only): the
        // anchor selector and the spawn-only opt-out accepted on
        // any other rule would be parsed-but-never-read knobs.
        if rule.rule != InvariantRule::SpawnSafety {
            let mut foreign: Vec<&str> = Vec::new();
            if rule.min_distance_from.is_some() {
                foreign.push("'min_distance_from'");
            }
            if rule.spawn_only.is_some() {
                foreign.push("'spawn_only'");
            }
            if !foreign.is_empty() {
                errors.push(ScenarioError::Rejected(format!(
                    "invariant '{}': {} {} not supported by {} — {} \
                     spawn_safety only",
                    rule.name,
                    foreign.join(", "),
                    if foreign.len() == 1 { "is" } else { "are" },
                    rule.rule,
                    if foreign.len() == 1 {
                        "this field is"
                    } else {
                        "these fields are"
                    }
                )));
            }
        }
    }
    for rule in &scenario.invariants {
        // Single-owner optional fields (entity_stagnation only):
        // same reserved-field doctrine as the block above — the
        // author writes a knob, the harness parses it, no rule
        // ever reads it. (`driver_resource`/`driver_path`/
        // `driver_units_per_second` are intentionally absent
        // here: the driver triple is shared grammar, unlocked
        // per-rule by validation.)
        if rule.rule != InvariantRule::EntityStagnation {
            let mut foreign: Vec<&str> = Vec::new();
            if rule.watch_path.is_some() {
                foreign.push("'watch_path'");
            }
            if rule.leaf_where.is_some() {
                foreign.push("'leaf_where'");
            }
            if rule.driver.is_some() {
                foreign.push("'driver'");
            }
            if rule.sampling_s.is_some() {
                foreign.push("'sampling_s'");
            }
            if !foreign.is_empty() {
                errors.push(ScenarioError::Rejected(format!(
                    "invariant '{}': {} {} not supported by {} — {} \
                     entity_stagnation only",
                    rule.name,
                    foreign.join(", "),
                    if foreign.len() == 1 { "is" } else { "are" },
                    rule.rule,
                    if foreign.len() == 1 {
                        "this field is"
                    } else {
                        "these fields are"
                    }
                )));
            }
        }
    }
    for rule in &scenario.invariants {
        // Single-owner optional fields (occupancy_cap only):
        // same reserved-field doctrine — the author writes a
        // knob, the harness parses it, no rule ever reads it.
        if rule.rule != InvariantRule::OccupancyCap {
            let mut foreign: Vec<&str> = Vec::new();
            if rule.radius.is_some() {
                foreign.push("'radius'");
            }
            if rule.cap.is_some() {
                foreign.push("'cap'");
            }
            if !foreign.is_empty() {
                errors.push(ScenarioError::Rejected(format!(
                    "invariant '{}': {} {} not supported by {} — {} \
                     occupancy_cap only",
                    rule.name,
                    foreign.join(", "),
                    if foreign.len() == 1 { "is" } else { "are" },
                    rule.rule,
                    if foreign.len() == 1 {
                        "this field is"
                    } else {
                        "these fields are"
                    }
                )));
            }
        }
    }
    for rule in &scenario.invariants {
        if let Some(rate) = rule.driver_units_per_second {
            let driver_resource = rule
                .driver_resource
                .clone()
                .or_else(|| rule.resource.clone())
                .unwrap_or_default();
            let driver_path = rule.driver_path.clone().unwrap_or_default();
            let prior = declared
                .iter()
                .find(|(name, path, _)| *path == driver_path && same_leaf(name, &driver_resource));
            match prior {
                Some((_, _, existing)) if *existing != rate => {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': driver_units_per_second is {rate} but the same \
                         driver leaf ({driver_resource}.{driver_path}) was already \
                         declared at {existing} units/s — one driver leaf has one rate; \
                         fix the declarations",
                        rule.name
                    )));
                }
                Some(_) => {}
                None => declared.push((driver_resource, driver_path, rate)),
            }
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
        // region/allow_if are forbidden_region's authoring surface;
        // on any other rule they are foreign fields and would
        // silently pass (accepted-and-ignored) unless rejected
        // here, once, generically — the same one-defect-one-error
        // doctrine as the eventually check above.
        if (rule.region.is_some() || rule.allow_if.is_some())
            && rule.rule != InvariantRule::ForbiddenRegion
        {
            errors.push(ScenarioError::Rejected(format!(
                "invariant '{}': 'region'/'allow_if' belong to \
                 forbidden_region only — a no-go zone and its \
                 legitimate-presence exemption are that rule's \
                 authoring surface",
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
                         stagnant_resource, pairwise_separation, or \
                         derived_transform_consistency",
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
                // rule's only temporal knob). The budget is consumed
                // in DRIVER-LEAF units, so the window is judgeable
                // whenever the driver can advance past it — the old
                // `window_s > duration_s` rejection silently assumed
                // drivers advance at >= 1 unit/second, wrongly
                // rejecting tick counters sized in the author's
                // mental unit.
                let expect_window = rule.rule == InvariantRule::StagnantResource;
                match rule.window_s {
                    Some(_window_s) if !expect_window => {
                        errors.push(ScenarioError::Rejected(format!(
                            "invariant '{}': window_s belongs to \
                             stagnant_resource/pairwise_separation/\
                             derived_transform_consistency only",
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
                        // Declared-rate fit check: only when the
                        // author states the driver's unit is the
                        // load-time arithmetic honest. Omitted → the
                        // post-run elision audit judges instead; no
                        // duration-coupled rejection at all.
                        if let Some(rate) = rule.driver_units_per_second {
                            if !rate.is_finite() || rate <= 0.0 {
                                errors.push(ScenarioError::Rejected(format!(
                                    "invariant '{}': driver_units_per_second must be a \
                                     finite number > 0 (got {rate})",
                                    rule.name
                                )));
                                // Unusable rate: skip the fit
                                // arithmetic rather than emitting a
                                // second, spurious "= inf" error for
                                // the same defect in this lint pass.
                            } else {
                                let effective_wall_s = (window_s / rate) as f64;
                                if effective_wall_s > scenario.duration_s as f64 {
                                    errors.push(ScenarioError::Rejected(format!(
                                        "invariant '{}': window ({window_s} driver units at \
                                         declared {rate} units/s = {effective_wall_s:.1}s) \
                                         exceeds the run duration ({duration}s) — either \
                                         the window or the declared rate makes it \
                                         unjudgeable; check driver_units_per_second",
                                        rule.name,
                                        duration = scenario.duration_s
                                    )));
                                }
                            }
                        }
                    }
                    None if expect_window => {
                        errors.push(ScenarioError::Rejected(format!(
                            "invariant '{}': stagnant_resource requires 'window_s' — the \
                             maximum number of driver-leaf units the watched leaf may \
                             stay constant before the invariant fires",
                            rule.name
                        )));
                    }
                    None => {}
                }
                // driver_units_per_second: stagnant_resource-only
                // surface (the rate declaration is meaningful only
                // for the rule whose budget is consumed in driver
                // units).
                if rule.driver_units_per_second.is_some() && !expect_window {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': driver_units_per_second belongs to \
                         stagnant_resource only",
                        rule.name
                    )));
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
            InvariantRule::PairwiseSeparation => {
                // Spatial rule: selector-required (the rule is
                // meaningless over an unbounded entity set),
                // min_distance required and positive, window_s
                // REQUIRED as the fly-by soak (rejecting the
                // instant-fire form at load is cheaper than a
                // finding explaining ε semantics post-run),
                // driver fields allowed (the driver-unit soak
                // grammar carries over from stagnant_resource
                // — when no driver is declared, the frame
                // clock in seconds is the default).
                //
                // Evaluation (the driver-loop episode recorder)
                // ships alongside this grammar: the recorder
                // selects via the same registration machinery,
                // enumerates pairs among the selector's
                // Transform-carrying entities, folds violating
                // frames into episodes, and judges each closed
                // episode against window_s in driver units.
                let Some(crate::enums::TargetSelector::WithComponent { name }) =
                    rule.target_selector.as_ref()
                else {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': pairwise_separation requires \
                         'target_selector: {{\"with_component\": {{\"name\": ...}}}}' \
                         — the rule judges pairs among a named component's \
                         entities, and 'all' would pairwise every Transform \
                         in the world (walls, camera, UI) at O(n²)",
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
                match rule.min_distance {
                    Some(d) if !d.is_finite() || d <= 0.0 => {
                        errors.push(ScenarioError::Rejected(format!(
                            "invariant '{}': min_distance must be a finite \
                             number > 0 (got {d}) — ε semantics: this is \
                             \"coincident bodies\", so a zero-or-negative \
                             threshold fires only on exact overlap",
                            rule.name
                        )));
                    }
                    None => {
                        errors.push(ScenarioError::Rejected(format!(
                            "invariant '{}': pairwise_separation requires \
                             'min_distance' — the ε below which two selected \
                             entities count as coincident",
                            rule.name
                        )));
                    }
                    _ => {}
                }
                // The soak: required (a rule with no soak
                // fires on any single contact frame), must be
                // finite and positive, and — unlike
                // stagnant_resource — the DECLARED-RATE fit
                // check shares the same arithmetic because
                // the window is consumed in the same driver
                // units (default: seconds).
                match rule.window_s {
                    None => {
                        errors.push(ScenarioError::Rejected(format!(
                            "invariant '{}': pairwise_separation requires \
                             'window_s' — the soak in driver-leaf units \
                             (default driver: the frame clock, so seconds); \
                             episodes shorter than the soak are discarded \
                             as transient fly-bys",
                            rule.name
                        )));
                    }
                    Some(window_s) => {
                        if !window_s.is_finite() || window_s <= 0.0 {
                            errors.push(ScenarioError::Rejected(format!(
                                "invariant '{}': window_s must be a finite \
                                 number > 0 (got {window_s})",
                                rule.name
                            )));
                        }
                        // Declared-rate fit check: same doctrine and
                        // arithmetic as stagnant_resource — a soak
                        // the run can never elapse is unjudgeable,
                        // rejected at load rather than discovered
                        // post-run. With no driver declared the soak
                        // is consumed in frame-clock seconds, so the
                        // fit check applies at 1 unit/s: the window
                        // compares straight against duration_s
                        // (mirrors liveness.progress's unelapsed-
                        // window rejection). Without it, an over-long
                        // window silently discards every episode as
                        // a sub-soak fly-by — a run that proves
                        // nothing.
                        let effective_wall_s = match rule.driver_units_per_second {
                            Some(rate) => {
                                if !rate.is_finite() || rate <= 0.0 {
                                    errors.push(ScenarioError::Rejected(format!(
                                        "invariant '{}': driver_units_per_second must be a \
                                         finite number > 0 (got {rate})",
                                        rule.name
                                    )));
                                    None
                                } else {
                                    Some((window_s / rate) as f64)
                                }
                            }
                            None => Some(window_s as f64),
                        };
                        if let Some(effective_wall_s) = effective_wall_s {
                            if effective_wall_s > scenario.duration_s as f64 {
                                errors.push(ScenarioError::Rejected(format!(
                                    "invariant '{}': soak window ({window_s} driver units, \
                                     {effective_wall_s:.1}s effective) exceeds the run \
                                     duration ({duration}s) — a soak the run can never \
                                     elapse is unjudgeable; shorten window_s, or adjust \
                                     driver_units_per_second",
                                    rule.name,
                                    duration = scenario.duration_s
                                )));
                            }
                        }
                    }
                }
                // Driver fields carry stagnant_resource's grammar
                // when declared — with one structural difference:
                // pairwise has no `resource` to default the driver
                // host to, so a driver declaration must name BOTH
                // halves (host resource + numeric leaf) or neither.
                let has_dr = rule.driver_resource.is_some();
                let has_dp = rule.driver_path.is_some();
                if has_dr != has_dp {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': pairwise_separation's driver needs \
                         both 'driver_resource' and 'driver_path' — unlike \
                         stagnant_resource there is no 'resource' to default \
                         the host to; omit both to soak against the frame \
                         clock in seconds",
                        rule.name
                    )));
                }
                // A declared rate with no driver declared at all is a
                // contradiction: the implicit driver is the FRAME
                // CLOCK at 1 unit/s by definition, so accepting a
                // rate would mis-validate the soak fit below (the
                // window judged in the author's fake units, not the
                // real seconds). The rejection also keeps the rate
                // out of the run: a driverless rate would otherwise
                // enter the one-leaf-one-rate join (which runs
                // before this arm) keyed on an empty ("","") leaf.
                if rule.driver_units_per_second.is_some() && !(has_dr && has_dp) {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': driver_units_per_second needs the \
                         driver declared ('driver_resource' + 'driver_path') — \
                         the default frame-clock driver is seconds by \
                         definition and has no declarable rate",
                        rule.name
                    )));
                }
                if let Some(dp) = rule.driver_path.as_deref() {
                    if dp.trim().is_empty() {
                        errors.push(ScenarioError::Rejected(format!(
                            "invariant '{}': 'driver_path' must be non-empty — \
                             a blank reflect path resolves the resource root, \
                             never a numeric leaf",
                            rule.name
                        )));
                    }
                }
                if let Some(dr) = rule.driver_resource.as_deref() {
                    if dr.trim().is_empty() {
                        errors.push(ScenarioError::Rejected(format!(
                            "invariant '{}': 'driver_resource' must be non-empty",
                            rule.name
                        )));
                    }
                }
                // Predicate/threshold fields belong to other
                // rules; silently accepting them would
                // recreate the accepted-and-ignored
                // reserved-field hole. (eventually_s is NOT
                // gated here: the generic eventually-mode check
                // above already rejects it with its own
                // message — one defect, one error.)
                if rule.check.is_some()
                    || rule.value.is_some()
                    || rule.path.is_some()
                    || rule.resource.is_some()
                    || rule.min_x.is_some()
                    || rule.max_x.is_some()
                    || rule.min_y.is_some()
                    || rule.max_y.is_some()
                    || rule.min_z.is_some()
                    || rule.max_z.is_some()
                {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': pairwise_separation does not use \
                         check/value/eventually_s/path/resource/bounds — it judges \
                         pair DISTANCE against min_distance and folds \
                         consecutive violating frames into episodes within \
                         window_s",
                        rule.name
                    )));
                }
            }
            InvariantRule::ForbiddenRegion => {
                // Declarative no-go zone: selector-required ('all'
                // would forbid every Transform in the world —
                // walls, camera, UI — from a region, never what an
                // author means; same O(cost) rationale as
                // pairwise's rejection), region required and
                // validated finite, window_s REQUIRED as the
                // fly-by soak (an incursion must LINGER to count —
                // the episode/window doctrine is this rule's
                // reason to exist), driver fields allowed with the
                // same both-halves-or-neither grammar as pairwise
                // (no `resource` to default the host to), allow_if
                // optional (a single registered component name —
                // the legitimate-presence exemption, flip-test
                // discipline mandatory per the proposal).
                let Some(crate::enums::TargetSelector::WithComponent { name }) =
                    rule.target_selector.as_ref()
                else {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': forbidden_region requires \
                         'target_selector: {{\"with_component\": {{\"name\": ...}}}}' \
                         — the rule judges a named component's entities against \
                         the declared region, and 'all' would forbid every \
                         Transform in the world (walls, camera, UI) from it",
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
                let Some(region) = rule.region.as_ref() else {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': forbidden_region requires 'region' — \
                         an author-declared no-go zone, e.g. \
                         {{\"center\": [0.0, 0.5, 0.0], \"half_extents\": [0.4, 1.0, 12.0]}}",
                        rule.name
                    )));
                    continue;
                };
                if region.center.iter().any(|c| !c.is_finite()) {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': region center must be finite in \
                         every axis (got {:?})",
                        rule.name, region.center
                    )));
                }
                if region
                    .half_extents
                    .iter()
                    .any(|h| !h.is_finite() || *h <= 0.0)
                {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': region half_extents must be finite \
                         and > 0 in every axis (got {:?}) — a zero-thickness \
                         wall is a plane, not a volume; use positive extents",
                        rule.name, region.half_extents
                    )));
                }
                if let Some(allow_if) = rule.allow_if.as_deref() {
                    if allow_if.trim().is_empty() {
                        errors.push(ScenarioError::Rejected(format!(
                            "invariant '{}': 'allow_if' must be a non-empty \
                             registered component name — an exemption naming \
                             nothing exempts nothing",
                            rule.name
                        )));
                    }
                }
                // The soak: required, finite, positive, with the
                // same declared-rate fit arithmetic as pairwise
                // (the window is consumed in driver units; default
                // driver: the frame clock in seconds).
                match rule.window_s {
                    None => {
                        errors.push(ScenarioError::Rejected(format!(
                            "invariant '{}': forbidden_region requires \
                             'window_s' — the soak in driver-leaf units \
                             (default driver: the frame clock, so seconds); \
                             incursion episodes shorter than the soak are \
                             discarded as transient fly-bys",
                            rule.name
                        )));
                    }
                    Some(window_s) => {
                        if !window_s.is_finite() || window_s <= 0.0 {
                            errors.push(ScenarioError::Rejected(format!(
                                "invariant '{}': window_s must be a finite \
                                 number > 0 (got {window_s})",
                                rule.name
                            )));
                        }
                        let effective_wall_s = match rule.driver_units_per_second {
                            Some(rate) => {
                                if !rate.is_finite() || rate <= 0.0 {
                                    errors.push(ScenarioError::Rejected(format!(
                                        "invariant '{}': driver_units_per_second must be a \
                                         finite number > 0 (got {rate})",
                                        rule.name
                                    )));
                                    None
                                } else {
                                    Some((window_s / rate) as f64)
                                }
                            }
                            None => Some(window_s as f64),
                        };
                        if let Some(effective_wall_s) = effective_wall_s {
                            if effective_wall_s > scenario.duration_s as f64 {
                                errors.push(ScenarioError::Rejected(format!(
                                    "invariant '{}': soak window ({window_s} driver units, \
                                     {effective_wall_s:.1}s effective) exceeds the run \
                                     duration ({duration}s) — a soak the run can never \
                                     elapse is unjudgeable; shorten window_s, or adjust \
                                     driver_units_per_second",
                                    rule.name,
                                    duration = scenario.duration_s
                                )));
                            }
                        }
                    }
                }
                // Driver fields: pairwise's both-halves-or-neither
                // doctrine verbatim — no `resource` to default the
                // host to; omit both to soak against the frame
                // clock in seconds.
                let has_dr = rule.driver_resource.is_some();
                let has_dp = rule.driver_path.is_some();
                if has_dr != has_dp {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': forbidden_region's driver needs \
                         both 'driver_resource' and 'driver_path' — unlike \
                         stagnant_resource there is no 'resource' to default \
                         the host to; omit both to soak against the frame \
                         clock in seconds",
                        rule.name
                    )));
                }
                if rule.driver_units_per_second.is_some() && !(has_dr && has_dp) {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': driver_units_per_second needs the \
                         driver declared ('driver_resource' + 'driver_path') — \
                         the default frame-clock driver is seconds by \
                         definition and has no declarable rate",
                        rule.name
                    )));
                }
                if let Some(dp) = rule.driver_path.as_deref() {
                    if dp.trim().is_empty() {
                        errors.push(ScenarioError::Rejected(format!(
                            "invariant '{}': 'driver_path' must be non-empty — \
                             a blank reflect path resolves the resource root, \
                             never a numeric leaf",
                            rule.name
                        )));
                    }
                }
                if let Some(dr) = rule.driver_resource.as_deref() {
                    if dr.trim().is_empty() {
                        errors.push(ScenarioError::Rejected(format!(
                            "invariant '{}': 'driver_resource' must be non-empty",
                            rule.name
                        )));
                    }
                }
                // Foreign fields (eventually_s excluded: the
                // generic eventually-mode check above already
                // rejects it — one defect, one error). allow_if and
                // region belong HERE only; other rules reject them
                // via deny_unknown_fields... no — they are fields
                // on Invariant, so each foreign arm must reject
                // them explicitly (the accepted-and-ignored
                // reserved-field hole).
                if rule.check.is_some()
                    || rule.value.is_some()
                    || rule.path.is_some()
                    || rule.resource.is_some()
                    || rule.min_x.is_some()
                    || rule.max_x.is_some()
                    || rule.min_y.is_some()
                    || rule.max_y.is_some()
                    || rule.min_z.is_some()
                    || rule.max_z.is_some()
                    || rule.min_distance.is_some()
                    || rule.tolerance.is_some()
                    || rule.grace_frames.is_some()
                    || !rule.exclude_components.is_empty()
                {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': forbidden_region does not use \
                         check/value/eventually_s/path/resource/bounds/min_distance/\
                         tolerance/grace_frames/exclude_components — it judges \
                         entity POSITIONS against the declared region and folds \
                         consecutive incursion frames into episodes within \
                         window_s",
                        rule.name
                    )));
                }
            }
            InvariantRule::DerivedTransformConsistency => {
                // The compositional identity over Bevy's one
                // canonical derived relationship: for every entity
                // with a Parent, GlobalTransform must equal the
                // parent's GlobalTransform composed with the
                // entity's own Transform, as observed at the SAME
                // settled frame (the driver-loop recorder doctrine
                // — internal consistency of the stored pair, never
                // "did the game call propagate"). Both FP sources
                // the design doc names are addressed by grammar:
                // the `*` default selector means the whole
                // hierarchy is checked unless scoped, and
                // legitimate late-frame GlobalTransform writers
                // (camera smoothing, interpolation ghosts) opt out
                // via `exclude_components` prefixes or the in-game
                // `SkipDerivedConsistency` marker. The shape checks
                // below validate the full authoring surface; the
                // consistency recorder evaluates it per frame.

                // window_s here is the PERSISTENCE soak, not a
                // fly-by discard: consistency defects are state
                // bugs (frozen/stale globals), so a single
                // violating frame is noise and an episode must
                // persist past the soak to count. Required for the
                // same reason as pairwise's: the instant-fire form
                // is an authoring mistake caught once at load.
                match rule.window_s {
                    None => {
                        errors.push(ScenarioError::Rejected(format!(
                            "invariant '{}': derived_transform_consistency \
                             requires 'window_s' — the persistence soak in \
                             driver-leaf units (default driver: the frame \
                             clock, so seconds); an inconsistency episode \
                             shorter than the soak is transient noise, not a \
                             state defect",
                            rule.name
                        )));
                    }
                    Some(window_s) => {
                        if !window_s.is_finite() || window_s <= 0.0 {
                            errors.push(ScenarioError::Rejected(format!(
                                "invariant '{}': window_s must be a finite \
                                 number > 0 (got {window_s})",
                                rule.name
                            )));
                        }
                        // Declared-rate fit check: same doctrine and
                        // arithmetic as the other driver-leaf rules
                        // — a soak the run can never elapse is
                        // unjudgeable, rejected at load. With no
                        // driver declared the soak is consumed in
                        // frame-clock seconds, so the same fit check
                        // applies at 1 unit/s: the window compares
                        // straight against duration_s (mirrors
                        // liveness.progress's unelapsed-window
                        // rejection). Without it, an over-long window
                        // silently discards every episode as
                        // sub-soak — a run that proves nothing.
                        let effective_wall_s = match rule.driver_units_per_second {
                            Some(rate) => {
                                if !rate.is_finite() || rate <= 0.0 {
                                    errors.push(ScenarioError::Rejected(format!(
                                        "invariant '{}': driver_units_per_second must be a \
                                         finite number > 0 (got {rate})",
                                        rule.name
                                    )));
                                    None
                                } else {
                                    Some((window_s / rate) as f64)
                                }
                            }
                            None => Some(window_s as f64),
                        };
                        if let Some(effective_wall_s) = effective_wall_s {
                            if effective_wall_s > scenario.duration_s as f64 {
                                errors.push(ScenarioError::Rejected(format!(
                                    "invariant '{}': soak window ({window_s} driver units, \
                                     {effective_wall_s:.1}s effective) exceeds the run \
                                     duration ({duration}s) — a soak the run can never \
                                     elapse is unjudgeable; shorten window_s, or adjust \
                                     driver_units_per_second",
                                    rule.name,
                                    duration = scenario.duration_s
                                )));
                            }
                        }
                    }
                }
                // Driver fields: pairwise's both-halves-or-neither
                // doctrine — there is no `resource` to default the
                // host to, and the frame-clock default is seconds
                // by definition (no declarable rate without a
                // driver).
                let has_dr = rule.driver_resource.is_some();
                let has_dp = rule.driver_path.is_some();
                if has_dr != has_dp {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': derived_transform_consistency's driver \
                         needs both 'driver_resource' and 'driver_path' — omit \
                         both to soak against the frame clock in seconds",
                        rule.name
                    )));
                }
                if rule.driver_units_per_second.is_some() && !(has_dr && has_dp) {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': driver_units_per_second needs the \
                         driver declared ('driver_resource' + 'driver_path') — \
                         the default frame-clock driver is seconds by \
                         definition and has no declarable rate",
                        rule.name
                    )));
                }
                for dp in [rule.driver_path.as_deref()].into_iter().flatten() {
                    if dp.trim().is_empty() {
                        errors.push(ScenarioError::Rejected(format!(
                            "invariant '{}': 'driver_path' must be non-empty — \
                             a blank reflect path resolves the resource root, \
                             never a numeric leaf",
                            rule.name
                        )));
                    }
                }
                for dr in [rule.driver_resource.as_deref()].into_iter().flatten() {
                    if dr.trim().is_empty() {
                        errors.push(ScenarioError::Rejected(format!(
                            "invariant '{}': 'driver_resource' must be non-empty",
                            rule.name
                        )));
                    }
                }
                // Tolerance: optional, but a nonsense ε (zero or
                // negative) fires on float noise; a non-finite one
                // accepts everything.
                if let Some(tol) = rule.tolerance {
                    if !tol.is_finite() || tol <= 0.0 {
                        errors.push(ScenarioError::Rejected(format!(
                            "invariant '{}': tolerance must be a finite \
                             number > 0 (got {tol}) — it is the per-element \
                             slack between observed and parent-composed \
                             GlobalTransform; zero fires on f32 composition \
                             noise, non-finite accepts everything",
                            rule.name
                        )));
                    }
                }
                // exclude_components entries must be non-empty
                // strings; blank prefixes match every component.
                for prefix in &rule.exclude_components {
                    if prefix.trim().is_empty() {
                        errors.push(ScenarioError::Rejected(format!(
                            "invariant '{}': exclude_components entries must \
                             be non-empty — a blank prefix excludes every \
                             component",
                            rule.name
                        )));
                    }
                }
                // A grace window that swallows the whole run leaves
                // no judged frame — the rule would silently prove
                // nothing. Same doctrine as liveness.timeout_s
                // shorter than one frame: reject loudly at load.
                // The frame count matches the driver's tick
                // arithmetic (duration_s * tps, ceiled) so the
                // message's number is the truth.
                if let Some(grace) = rule.grace_frames {
                    let total_frames = (scenario.duration_s * scenario.tps as f32).ceil() as u64;
                    if grace as u64 >= total_frames {
                        errors.push(ScenarioError::Rejected(format!(
                            "invariant '{}': grace_frames ({grace}) skips every \
                             frame of the run ({total_frames} frames from \
                             duration_s {duration}s at tps {tps}) — the rule \
                             would never be judged; shorten the grace or \
                             lengthen the run",
                            rule.name,
                            duration = scenario.duration_s,
                            tps = scenario.tps
                        )));
                    }
                    // The compound fit: grace and soak consume the
                    // same frame budget from opposite ends. An
                    // episode can only accrue span on JUDGED frames
                    // (post-grace), so a window longer than the
                    // post-grace remainder is unjudgeable even when
                    // it fits the whole run — the grace ate the only
                    // frames that could have accrued the span.
                    else if let Some(window_s) = rule.window_s {
                        if rule.driver_units_per_second.is_none() {
                            let judged_s =
                                scenario.duration_s - (grace as f32 / scenario.tps as f32);
                            if window_s > judged_s {
                                errors.push(ScenarioError::Rejected(format!(
                                    "invariant '{}': soak window ({window_s}s) does not \
                                     fit the judged frames the grace leaves behind \
                                     ({juded}s after grace_frames {grace} of \
                                     {duration}s at tps {tps}) — the episode \
                                     could never accrue span past the soak; \
                                     shorten window_s or grace_frames",
                                    rule.name,
                                    juded = judged_s,
                                    duration = scenario.duration_s,
                                    tps = scenario.tps
                                )));
                            }
                        }
                    }
                }
                // Foreign fields: the reserved-field doctrine.
                // eventually_s is left to the generic check so one
                // defect yields one error.
                if rule.check.is_some()
                    || rule.value.is_some()
                    || rule.path.is_some()
                    || rule.resource.is_some()
                    || rule.min_distance.is_some()
                    || rule.min_x.is_some()
                    || rule.max_x.is_some()
                    || rule.min_y.is_some()
                    || rule.max_y.is_some()
                    || rule.min_z.is_some()
                    || rule.max_z.is_some()
                {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': derived_transform_consistency does \
                         not use check/value/path/resource/min_distance/bounds — \
                         it checks the GlobalTransform == \
                         parent-GlobalTransform ∘ Transform identity with \
                         tolerance and folds violating frames into \
                         episodes within window_s",
                        rule.name
                    )));
                }
            }
            InvariantRule::SpawnSafety => {
                // The archetype-entry spatial oracle (proposal §1):
                // entities are judged ONCE, at the frame they ENTER
                // the selected archetype — a newborn or a
                // pre-existing entity first acquiring the target
                // component — against anchors snapshotted at the END
                // of the same judging frame. Generation-tracked
                // (id, generation) diff semantics, NOT Added<T>:
                // id reuse can neither forge nor hide an entry.
                // Component-addition is the judged event by
                // default; `spawn_only: true` filters to brand-new
                // (id, generation) pairs.
                let Some(crate::enums::TargetSelector::WithComponent { name }) =
                    rule.target_selector.as_ref()
                else {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': spawn_safety requires \
                         'target_selector: {{\"with_component\": {{\"name\": ...}}}}' — \
                         the archetype whose entries are judged",
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
                let Some(_anchor) = rule.min_distance_from.as_ref() else {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': spawn_safety requires 'min_distance_from' — \
                         the anchor selector (with_component or all) every entering \
                         entity must keep min_distance away from",
                        rule.name
                    )));
                    continue;
                };
                match rule.min_distance {
                    None => {
                        errors.push(ScenarioError::Rejected(format!(
                            "invariant '{}': spawn_safety requires 'min_distance' — \
                             the minimum distance between an entering entity and \
                             every same-frame anchor",
                            rule.name
                        )));
                    }
                    Some(d) => {
                        if !d.is_finite() || d <= 0.0 {
                            errors.push(ScenarioError::Rejected(format!(
                                "invariant '{}': min_distance must be a finite \
                                 number > 0 (got {d})",
                                rule.name
                            )));
                        }
                    }
                }
                // The transition frame is judged, never soaked:
                // temporal knobs of every other rule are foreign
                // here. after_s/before_s are ALREADY rejected
                // universally (not-implemented), so they need no
                // arm; eventually_s is likewise left to the
                // generic eventually-mode check above (one
                // defect, one error); window_s/driver/tolerance/
                // grace/exclude/check/value/path/resource/bounds
                // are this rule's foreign surface.
                if rule.window_s.is_some() {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': spawn_safety does not use window_s — \
                         entries are judged once at the transition frame, \
                         never folded into episodes",
                        rule.name
                    )));
                }
                if rule.driver_resource.is_some() || rule.driver_path.is_some() {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': spawn_safety does not use \
                         driver_resource/driver_path — there is no soak and \
                         therefore no driver budget to consume",
                        rule.name
                    )));
                }
                if rule.driver_units_per_second.is_some() {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': spawn_safety does not use \
                         driver_units_per_second — there is no soak and \
                         therefore no rate to declare",
                        rule.name
                    )));
                }
                if rule.check.is_some()
                    || rule.value.is_some()
                    || rule.path.is_some()
                    || rule.resource.is_some()
                    || rule.min_x.is_some()
                    || rule.max_x.is_some()
                    || rule.min_y.is_some()
                    || rule.max_y.is_some()
                    || rule.min_z.is_some()
                    || rule.max_z.is_some()
                {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': spawn_safety does not use \
                         check/value/path/resource/bounds — it \
                         judges entry-frame distance against min_distance, \
                         once (tolerance/grace_frames/exclude_components are \
                         rejected separately as DTC-only fields)",
                        rule.name
                    )));
                }
                // Pairwise shares min_distance by design (one
                // threshold field, one meaning: a distance floor);
                // `min_distance_from` is spawn_safety-only —
                // rejected elsewhere via the shared foreign-field
                // loop below.
            }
            InvariantRule::EntityStagnation => {
                // Per-entity rate-form liveness (judgement layer,
                // second vocabulary): the watched leaf on each
                // selected entity must not stay digest-constant
                // while the driver advances. Shape checks only
                // here — leaf_where/variant definite resolution is
                // a PROBE-TIME check (the registry is not
                // available at load), so a typo there surfaces as
                // an oracle_error, never a silent "never true".

                // Stagnation is per-ENTITY: `all` would watch
                // every Transform-carrying entity (walls,
                // cameras) which are legitimately inert — the
                // same argument as pairwise's selector
                // requirement.
                let Some(crate::enums::TargetSelector::WithComponent { name }) =
                    rule.target_selector.as_ref()
                else {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': entity_stagnation requires \
                         'target_selector: {{\"with_component\": {{\"name\": ...}}}}' \
                         — the rule watches a named component's entities; 'all' \
                         would stagnation-watch every Transform in the world \
                         (walls, cameras) that are legitimately inert",
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
                // The watched leaf: required, non-empty.
                match rule.watch_path.as_deref() {
                    None => {
                        errors.push(ScenarioError::Rejected(format!(
                            "invariant '{}': entity_stagnation requires \
                             'watch_path' — the scoped reflect path on the \
                             selected component whose leaf digests are \
                             compared across samples (e.g. \"state.Queued.waited\")",
                            rule.name
                        )));
                    }
                    Some(wp) if wp.trim().is_empty() => {
                        errors.push(ScenarioError::Rejected(format!(
                            "invariant '{}': 'watch_path' must be non-empty — \
                             a blank reflect path resolves the component root",
                            rule.name
                        )));
                    }
                    _ => {}
                }
                // leaf_where: narrow grammar `path == VariantName`.
                // Definite resolution (does the path resolve on
                // the component, does the variant exist in the
                // enum) is probe-time; here only the SHAPE is
                // checkable — but a malformed clause is an
                // authoring error caught once at load.
                if let Some(lw) = rule.leaf_where.as_deref() {
                    let mut parts = lw.split("==");
                    let lhs = parts.next();
                    let rhs = parts.next();
                    let extra = parts.next();
                    if lhs.is_none()
                        || rhs.map(str::trim).is_none_or(str::is_empty)
                        || extra.is_some()
                        || lhs.map(str::trim).is_none_or(str::is_empty)
                    {
                        errors.push(ScenarioError::Rejected(format!(
                            "invariant '{}': 'leaf_where' must be \
                             '<path> == <VariantName>' (e.g. \"state == Queued\") \
                             — got {:?}",
                            rule.name, lw
                        )));
                    }
                }
                // Driver discriminator: flat string, default
                // global_time. Anything else is a typo the author
                // must hear about at load, not a silent fallback.
                let driver = rule.driver.as_deref().unwrap_or("global_time");
                let rate = match driver {
                    "global_time" => {
                        // The frame-clock driver is seconds by
                        // definition; the driver triple belongs
                        // to the resource form.
                        if rule.driver_resource.is_some()
                            || rule.driver_path.is_some()
                            || rule.driver_units_per_second.is_some()
                        {
                            errors.push(ScenarioError::Rejected(format!(
                                "invariant '{}': driver \"global_time\" does \
                                 not use driver_resource/driver_path/\
                                 driver_units_per_second — the frame clock \
                                 advances 1 second per second by definition; \
                                 declare 'driver: \"resource\"' to soak against \
                                 a game leaf instead",
                                rule.name
                            )));
                        }
                        1.0
                    }
                    "resource" => {
                        // The driver triple VERBATIM
                        // (stagnant_resource grammar), with one
                        // stagnation-specific addition: the rate
                        // is REQUIRED because the sampling
                        // cadence is scheduled in frames and
                        // converting sampling_s out of driver-leaf
                        // units needs it.
                        let has_dr = rule.driver_resource.is_some();
                        let has_dp = rule.driver_path.is_some();
                        if !has_dr || !has_dp {
                            errors.push(ScenarioError::Rejected(format!(
                                "invariant '{}': driver \"resource\" needs \
                                 both 'driver_resource' and 'driver_path'",
                                rule.name
                            )));
                        }
                        for field in [rule.driver_resource.as_deref(), rule.driver_path.as_deref()]
                            .into_iter()
                            .flatten()
                        {
                            if field.trim().is_empty() {
                                errors.push(ScenarioError::Rejected(format!(
                                    "invariant '{}': '{}' must be non-empty",
                                    rule.name, field
                                )));
                            }
                        }
                        match rule.driver_units_per_second {
                            Some(r) if r.is_finite() && r > 0.0 => r,
                            Some(r) => {
                                errors.push(ScenarioError::Rejected(format!(
                                    "invariant '{}': driver_units_per_second \
                                     must be a finite number > 0 (got {r})",
                                    rule.name
                                )));
                                // Unusable rate: skip the
                                // rate-derived fit checks below
                                // rather than divide by it.
                                continue;
                            }
                            None => {
                                errors.push(ScenarioError::Rejected(format!(
                                    "invariant '{}': driver \"resource\" \
                                     requires 'driver_units_per_second' — the \
                                     sampling cadence is scheduled in frames, \
                                     which needs the driver's advance rate to \
                                     convert sampling_s out of driver-leaf \
                                     units (the global_time default needs no \
                                     rate: the frame clock is seconds)",
                                    rule.name
                                )));
                                continue;
                            }
                        }
                    }
                    other => {
                        errors.push(ScenarioError::Rejected(format!(
                            "invariant '{}': 'driver' must be \"global_time\" \
                             (the default — the run's frame clock) or \
                             \"resource\" (soak against a game leaf via \
                             driver_resource + driver_path); got {:?}",
                            rule.name, other
                        )));
                        continue;
                    }
                };
                // window_s: required (the soak), finite > 0.
                let Some(window_s) = rule.window_s else {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': entity_stagnation requires 'window_s' \
                         — the soak in driver-leaf units (default driver: \
                         the frame clock, so seconds); a stagnant stretch \
                         shorter than the soak is a sub-window lull, not a \
                         livelock",
                        rule.name
                    )));
                    continue;
                };
                if !window_s.is_finite() || window_s <= 0.0 {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': window_s must be a finite number > 0 \
                         (got {window_s})",
                        rule.name
                    )));
                    continue;
                }
                // sampling_s: optional, default 1.0 driver-leaf
                // unit; finite > 0. The floor — window_s >= 2 x
                // sampling_s — is the rule's honesty floor: an
                // episode needs at least two consecutive
                // equal-digest samples to accrue ANY span, so a
                // window narrower than two samples can never fire
                // and would silently prove nothing.
                let sampling_s = rule.sampling_s.unwrap_or(1.0);
                if !sampling_s.is_finite() || sampling_s <= 0.0 {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': sampling_s must be a finite number \
                         > 0 (got {sampling_s}) — it is the sampling cadence \
                         in driver-leaf units (default 1.0)",
                        rule.name
                    )));
                    continue;
                }
                if window_s < 2.0 * sampling_s {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': window_s ({window_s}) must be at \
                         least twice sampling_s ({sampling_s}) — a stagnation \
                         interval is two consecutive equal-digest samples, so \
                         a window narrower than two samples can never accrue \
                         span and the rule would silently prove nothing; \
                         widen window_s or coarsen sampling_s",
                        rule.name
                    )));
                }
                // Fit checks, in the run's own arithmetic: the
                // cadence is scheduled in frames
                // (sampling_wall_s >= one frame, else every frame
                // is a sample and the cadence collapses), and the
                // soak must fit the run's duration. Same doctrine
                // as liveness.progress's sample/window rejection.
                let frame_s = 1.0 / scenario.tps as f32;
                let sampling_wall_s = sampling_s / rate;
                if sampling_wall_s < frame_s {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': sampling cadence ({sampling_s} driver \
                         units, {:.4}s at {rate} units/s) is shorter than one \
                         frame at tps {} ({frame_s:.6}s) — sub-frame sampling \
                         collapses to repeated identical samples; raise \
                         sampling_s to at least one frame",
                        rule.name, sampling_wall_s, scenario.tps
                    )));
                }
                let window_wall_s = window_s / rate;
                if window_wall_s > scenario.duration_s {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': soak window ({window_s} driver units, \
                         {window_wall_s:.1}s effective) exceeds the run \
                         duration ({duration}s) — a soak the run can never \
                         elapse is unjudgeable; shorten window_s, or adjust \
                         driver_units_per_second",
                        rule.name,
                        duration = scenario.duration_s
                    )));
                }
                // Foreign fields: the reserved-field doctrine.
                // eventually_s is left to the generic check so one
                // defect yields one error. tolerance/grace_frames/
                // exclude_components are caught by the
                // derived_transform_consistency-only block above;
                // the stagnation-only fields are caught by the
                // entity_stagnation-only block above.
                if rule.check.is_some()
                    || rule.value.is_some()
                    || rule.path.is_some()
                    || rule.resource.is_some()
                    || rule.min_distance.is_some()
                    || rule.min_x.is_some()
                    || rule.max_x.is_some()
                    || rule.min_y.is_some()
                    || rule.max_y.is_some()
                    || rule.min_z.is_some()
                    || rule.max_z.is_some()
                {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': entity_stagnation does not use \
                         check/value/path/resource/min_distance/bounds — it \
                         digest-compares the leaf at watch_path across \
                         samples and folds the still-stuck set into \
                         episodes within window_s",
                        rule.name
                    )));
                }
            }
            InvariantRule::OccupancyCap => {
                // Local-density rule (judgement layer, spatial
                // family): more than `cap` selected entities
                // within `radius` of a selected entity's
                // position, folded into a union-timeline episode
                // and judged against the window_s soak. The
                // enumeration is ENTITY-WITNESSED (settlement):
                // the ring of entities spaced just past radius
                // around an empty center is a documented
                // NON-FIRING case — approximate counting is the
                // warn-phase doctrine, revisitable at promotion.
                //
                // WARN-ONLY by construction: the rule sits in
                // Rule::is_informational(), so its findings
                // (including this arm's recorder output) never
                // gate run status.
                //
                // Selector: required with_component, `all`
                // rejected (the same argument as pairwise — an
                // unbounded entity set makes a density claim
                // meaningless).
                let Some(crate::enums::TargetSelector::WithComponent { name }) =
                    rule.target_selector.as_ref()
                else {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': occupancy_cap requires \
                         'target_selector: {{\"with_component\": {{\"name\": ...}}}}' \
                         — the rule counts a named component's entities; 'all' \
                         would density-judge every Transform in the world \
                         (walls, cameras) that legitimately cluster",
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
                // radius: required, finite > 0 (min_distance's
                // arithmetic and phrasing — the neighborhood size
                // below which crowd counting degenerates).
                match rule.radius {
                    Some(r) if !r.is_finite() || r <= 0.0 => {
                        errors.push(ScenarioError::Rejected(format!(
                            "invariant '{}': radius must be a finite \
                             number > 0 (got {r}) — the neighborhood size \
                             a crowd judgment counts within",
                            rule.name
                        )));
                    }
                    None => {
                        errors.push(ScenarioError::Rejected(format!(
                            "invariant '{}': occupancy_cap requires 'radius' \
                             — the neighborhood size within which the \
                             selected entities are counted against cap",
                            rule.name
                        )));
                    }
                    _ => {}
                }
                // cap: required integer >= 1. The parse-time
                // INTEGER is the point (serde rejects 3.5/3.0
                // for Option<u32> free — never a float floor);
                // a zero cap is nonsense under self-inclusive
                // counting (the count is always >= 1, so cap 0
                // fires whenever any entity is selected).
                match rule.cap {
                    Some(0) => {
                        errors.push(ScenarioError::Rejected(format!(
                            "invariant '{}': cap must be >= 1 (got 0) — \
                             the count is self-inclusive (a selected entity \
                             is within its own radius), so a zero cap fires \
                             on every populated frame; if the intent is \
                             \"no entities at all\", that is a different rule",
                            rule.name
                        )));
                    }
                    None => {
                        errors.push(ScenarioError::Rejected(format!(
                            "invariant '{}': occupancy_cap requires 'cap' \
                             — the maximum number of selected entities \
                             allowed within radius (self-inclusive) before \
                             the crowd becomes a finding",
                            rule.name
                        )));
                    }
                    _ => {}
                }
                // window_s: REQUIRED (the soak), finite > 0,
                // with the pairwise fit check verbatim — the
                // window is consumed in driver units (default:
                // frame-clock seconds), so an unelidable soak is
                // rejected at load rather than silently proving
                // nothing.
                let Some(window_s) = rule.window_s else {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': occupancy_cap requires 'window_s' \
                         — the soak in driver-leaf units (default driver: \
                         the frame clock, so seconds); crowd episodes shorter \
                         than the soak are discarded as transient gatherings",
                        rule.name
                    )));
                    continue;
                };
                if !window_s.is_finite() || window_s <= 0.0 {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': window_s must be a finite number > 0 \
                         (got {window_s})",
                        rule.name
                    )));
                    continue;
                }
                {
                    let effective_wall_s = match rule.driver_units_per_second {
                        Some(rate) => {
                            if !rate.is_finite() || rate <= 0.0 {
                                errors.push(ScenarioError::Rejected(format!(
                                    "invariant '{}': driver_units_per_second must be a \
                                     finite number > 0 (got {rate})",
                                    rule.name
                                )));
                                None
                            } else {
                                Some((window_s / rate) as f64)
                            }
                        }
                        None => Some(window_s as f64),
                    };
                    if let Some(effective_wall_s) = effective_wall_s {
                        if effective_wall_s > scenario.duration_s as f64 {
                            errors.push(ScenarioError::Rejected(format!(
                                "invariant '{}': soak window ({window_s} driver units, \
                                 {effective_wall_s:.1}s effective) exceeds the run \
                                 duration ({duration}s) — a soak the run can never \
                                 elapse is unjudgeable; shorten window_s, or adjust \
                                 driver_units_per_second",
                                rule.name,
                                duration = scenario.duration_s
                            )));
                        }
                    }
                }
                // Driver fields: pairwise's grammar exactly —
                // both halves or neither (occupancy has no
                // `resource` to default the host to), and a
                // declared rate without a declared driver is a
                // contradiction (the implicit driver is the frame
                // clock at 1 unit/s by definition).
                let has_dr = rule.driver_resource.is_some();
                let has_dp = rule.driver_path.is_some();
                if has_dr != has_dp {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': occupancy_cap's driver needs \
                         both 'driver_resource' and 'driver_path' — like \
                         pairwise_separation there is no 'resource' to \
                         default the host to; omit both to soak against the \
                         frame clock in seconds",
                        rule.name
                    )));
                }
                if rule.driver_units_per_second.is_some() && !(has_dr && has_dp) {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': driver_units_per_second needs the \
                         driver declared ('driver_resource' + 'driver_path') — \
                         the default frame-clock driver is seconds by \
                         definition and has no declarable rate",
                        rule.name
                    )));
                }
                if let Some(dp) = rule.driver_path.as_deref() {
                    if dp.trim().is_empty() {
                        errors.push(ScenarioError::Rejected(format!(
                            "invariant '{}': 'driver_path' must be non-empty — \
                             a blank reflect path resolves the resource root, \
                             never a numeric leaf",
                            rule.name
                        )));
                    }
                }
                if let Some(dr) = rule.driver_resource.as_deref() {
                    if dr.trim().is_empty() {
                        errors.push(ScenarioError::Rejected(format!(
                            "invariant '{}': 'driver_resource' must be non-empty",
                            rule.name
                        )));
                    }
                }
                // Foreign fields: the reserved-field doctrine.
                // eventually_s is left to the generic check so
                // one defect yields one error; tolerance/
                // grace_frames/exclude_components belong to the
                // DTC-only block; watch_path/leaf_where/driver/
                // sampling_s to the stagnation-only block;
                // region/allow_if to the forbidden-only check;
                // min_distance_from/spawn_only to the
                // spawn-safety-only loop.
                if rule.check.is_some()
                    || rule.value.is_some()
                    || rule.path.is_some()
                    || rule.resource.is_some()
                    || rule.min_distance.is_some()
                    || rule.min_x.is_some()
                    || rule.max_x.is_some()
                    || rule.min_y.is_some()
                    || rule.max_y.is_some()
                    || rule.min_z.is_some()
                    || rule.max_z.is_some()
                {
                    errors.push(ScenarioError::Rejected(format!(
                        "invariant '{}': occupancy_cap does not use \
                         check/value/path/resource/min_distance/bounds — it \
                         counts selected entities within radius against cap \
                         and folds crowd frames into episodes within window_s",
                        rule.name
                    )));
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
