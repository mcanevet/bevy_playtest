//! Shared test-support module for the known_answer fixture tests:
//! a typed loader for `ground_truth.json` plus PLANT-marker↔row
//! cross-checks. Compiled into each integration test via
//! `mod support;` (dev-dependencies carry serde/serde_json; the
//! game binary itself never depends on them).
//!
//! Each test binary compiles its own copy of this module and uses a
//! different subset of the API, so unused-item lints would fire
//! spuriously per binary.
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;

/// Schema drift between this loader and the file must fail
/// COMPILATION of tests, not pass silently (derive-based types).
///
/// `expected_grade` is EITHER the string `"negative"` (adversarial
/// negative plants: the method should say NOTHING/at-most-suspect
/// about them) OR a per-method expectation object. Untagged with a
/// keyword type for the string arm: a unit variant under `untagged`
/// would round-trip as `null`, losing the committed spelling.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum ExpectedGrade {
    Negative(NegativeKeyword),
    Methods(GradeMethods),
}

/// Serde bridge: the literal JSON string `"negative"`.
#[derive(Debug, Clone, PartialEq)]
pub struct NegativeKeyword;

impl Serialize for NegativeKeyword {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str("negative")
    }
}

impl<'de> Deserialize<'de> for NegativeKeyword {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        if s == "negative" {
            Ok(NegativeKeyword)
        } else {
            Err(serde::de::Error::custom(
                "expected the literal \"negative\" for a negative-grade row",
            ))
        }
    }
}

/// Per-method expectations for positive plants.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GradeMethods {
    /// The D0 probe sweep observes divergence at boot context.
    pub probe_visible: bool,
    /// A declared-binding reader (Leafwing/bespoke, D1-d+) would
    /// cite a binding source. False for raw reads today.
    pub source_declared: bool,
    /// The binding actually fires in the boot context (chords and
    /// unreachable-state plants are boot-live but NOT probe-visible).
    pub boot_live: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EffectClass {
    Diverge,
    Inert,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Context {
    MenuFocused,
    Gameplay,
    Boot,
    Unreachable,
}

/// One ground-truth row: one input, one plant marker.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GroundTruthEntry {
    pub input: String,
    pub expected_grade: ExpectedGrade,
    pub expected_effect_class: EffectClass,
    pub context: Context,
    pub mutable: bool,
    /// The `// PLANT Pn` / `// PLANT Nn` id in `src/main.rs`; the
    /// cross-check test pins rows to markers 1:1 (per marker, at
    /// least one row; every row's marker must exist in source).
    pub plant_marker: String,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    /// Live-but-unseeable bindings (chord/debounce): asserted as
    /// expected misses of the naive sweep.
    SweepExpectedMisses,
    /// Real effect behind an unreachable state: not-discovered at
    /// boot, no drift alarm.
    ContextGatedNotDiscovered,
    /// Shape-positive behavior-negative inputs the sweep must
    /// grade inert.
    NegativeShapes,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FindingExpectation {
    pub kind: FindingKind,
    pub inputs: Vec<String>,
    pub detail: String,
}

/// One declared-tier expectation (D1-d): what the Leafwing reader +
/// intent-tap + reconcile pipeline must produce for one (action,
/// binding) plant. These rows are the declared-tier analog of
/// `GroundTruthEntry` — they pin the READER's enumeration and
/// reconcile's grading, not the sweep's probe verdicts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LwRow {
    /// The action's `Actionlike` variant label (e.g. `"Live"`).
    pub action: String,
    /// NORMALIZED input name, same string space as sweep rows.
    pub input: String,
    /// Whether the reader must find the binding declared in the
    /// game's `InputMap`s (always true for the L-plants; a future
    /// bespoke reader row could differ).
    pub source_declared: bool,
    /// What the deterministic intent-tap run must observe for this
    /// binding's action.
    pub tap: LwTapExpectation,
    /// The reconcile grade the binding must carry in the pristine
    /// run (snake_case `DiscoveryFindingKind`-aligned labels).
    pub pristine_reconcile: LwReconcileExpectation,
    /// Human-readable reason, checked into the oracle.
    pub notes: String,
}

/// Intent-tap semantics the deterministic run pins for one action.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LwTapExpectation {
    /// Driver pressed, action fired, enabled throughout.
    PressedAndFired,
    /// Driver pressed, action never fired, and the action was
    /// DISABLED at press time — the proven-dead arm.
    PressedNeverFiredDisabled,
    /// Chord member: never single-key pressed; fires only as part of
    /// the chord. Members are note-material, never alarm-material.
    ChordMember,
}

/// Reconcile grades, aligned with the harness's
/// `DiscoveryFindingKind` spellings (snake_case).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LwReconcileExpectation {
    /// Proven-dead wiring: `DeclaredButDead` alarm.
    DeclaredButDead,
    /// Fires at wiring level but no game effect verifiable:
    /// `ContextUnverified` finding, NEVER an alarm.
    ContextUnverified,
    /// Chord member: no grade, per-binding note only.
    ChordNote,
    /// Healthy live wiring the sweep verified: `ManagerPipelineOk`.
    ManagerPipelineOk,
}

/// The committed ground truth (schema v1 — `lw_rows` is an ADDITIVE
/// key; loaders predating D1-d deserialize it to `vec![]` via
/// `#[serde(default)]`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GroundTruth {
    pub fixture: String,
    pub schema_version: u32,
    pub bevy_profile_note: String,
    pub entries: Vec<GroundTruthEntry>,
    pub findings_expected: Vec<FindingExpectation>,
    /// Declared-tier (Leafwing reader) expectations, D1-d. Absent
    /// in pre-D1-d copies of the file — hence defaulted.
    #[serde(default)]
    pub lw_rows: Vec<LwRow>,
}

/// Locate the ground-truth file relative to this test crate's
/// manifest (CARGO_MANIFEST_DIR points at `fixtures/known_answer`
/// for all of its integration tests).
pub fn ground_truth_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("ground_truth.json")
}

/// Parse `ground_truth.json` into typed structures. Wrong-typed or
/// missing fields are compile-time-checked by serde derives and
/// fail the test loudly at runtime — never a silent pass.
pub fn load_ground_truth() -> GroundTruth {
    let path = ground_truth_path();
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("ground truth schema drift at {}: {e}", path.display()))
}

/// Extract every `// PLANT Pn` / `// PLANT Nn` marker id from the
/// fixture source. The marker grammar: a line comment beginning
/// exactly with `// PLANT ` whose NEXT whitespace-delimited token is
/// the alphanumeric id — so `// PLANT P1 — Menu context: ...`
/// yields `P1`, while prose merely mentioning P1 elsewhere never
/// matches.
pub fn plant_markers_in_source() -> BTreeSet<String> {
    let source = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs"))
        .expect("fixture source readable");
    let mut markers = BTreeSet::new();
    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(id) = trimmed
            .strip_prefix("// PLANT ")
            .and_then(|rest| rest.split_whitespace().next())
            .filter(|id| id.chars().all(|c| c.is_ascii_alphanumeric()))
        {
            markers.insert(id.to_string());
        }
    }
    markers
}

/// PLANT ids the cross-check REQUIRES to exist (the full catalog:
/// P1..P7, N1..N4, L1..L5). Drift either direction fails the test.
pub const REQUIRED_MARKERS: &[&str] = &[
    "P1", "P2", "P3", "P4", "P5", "P6", "P7", "N1", "N2", "N3", "N4", "L1", "L2", "L3", "L4",
    "L5",
];

/// Candidate keys the plant catalog adds on top of the harness's
/// default probe set (measure/mutation harnesses both probe the
/// default set UNION these). Single-sourced here so the two test
/// binaries cannot drift apart. The control key `KeyQ` is never
/// bound and never a candidate.
///
/// `KeyJ` (PLANT L5): the healthy LW `Live` binding whose sweep-
/// verified liveness reconcile must supersede — without probing it,
/// the draft would never emit the DiscoveredButUndeclared row for
/// KeyJ that the retraction case consumes.
pub const EXTRA_CANDIDATE_KEYS: &[&str] = &[
    "KeyU", "KeyL", "KeyT", "KeyG", "KeyV", "KeyX", "KeyM", "KeyJ",
];

// ---------------------------------------------------------------------------
// D1-d declared-tier lens (Leafwing reader + intent-tap + reconcile)
// shared machinery: the deterministic scenario and its battery runner
// live HERE so the measure and mutation test binaries cannot drift
// apart (same discipline as EXTRA_CANDIDATE_KEYS above).
//
// Gated behind `measure`: these reference `bevy_playtest` (present
// only under the self-dev-dependency's `playtest` activation) and
// would be dead code — and break the stripped `cargo package`
// battery build — in the plain battery tier.
// ---------------------------------------------------------------------------
#[cfg(feature = "measure")]
/// The deterministic multi-press scenario driving the declared-tier
/// run: every LW plant binding gets exactly one driver press (the
/// chord gets its members held together), plus ordinary gameplay
/// traffic (W/S/D). Mutation runs reuse it unchanged — `LW_dead`
/// turns the frame-40 KeyJ press into the pressed-never-fired alarm
/// arm.
const LW_LENS_SCENARIO: &str = r#"{
  "bot": {
    "type": "synthetic_keyboard",
    "key_presses": [
      {"frame": 10, "key": "KeyW", "hold_frames": 3},
      {"frame": 25, "key": "KeyS", "hold_frames": 3},
      {"frame": 40, "key": "KeyJ"},
      {"frame": 45, "key": "KeyN"},
      {"frame": 50, "key": "KeyO"},
      {"frame": 55, "key": "ControlLeft", "hold_frames": 10},
      {"frame": 57, "key": "KeyI"},
      {"frame": 60, "key": "KeyP"},
      {"frame": 65, "key": "KeyH"},
      {"frame": 80, "key": "KeyD", "hold_frames": 3}
    ]
  },
  "tps": 60,
  "duration_s": 3.0
}"#;

/// Write the LW-lens scenario to a fresh temp file (RAII-removed on
/// every exit path, panic included) and return its path.
#[cfg(feature = "measure")]
pub fn lw_lens_scenario_file(tag: &str) -> (std::path::PathBuf, impl Drop) {
    let dir = std::env::temp_dir().join(format!(
        "bevy_playtest_known_answer_lwlens_{}_{}_{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("temp scenario dir creatable");
    let path = dir.join("lw_lens_scenario.json");
    std::fs::write(&path, LW_LENS_SCENARIO).expect("scenario file writable");
    /// RAII: remove the temp dir on every exit path, panic included.
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    (path, Cleanup(dir))
}

/// Run the fixture under the deterministic LW-lens scenario (via the
/// battery, headless) and return the parsed report — LW declared
/// rows + tap stats ride the report out of the child process.
#[cfg(feature = "measure")]
pub fn run_lw_lens(tag: &str) -> bevy_playtest::state::PlaytestReport {
    let (scenario, _cleanup) = lw_lens_scenario_file(tag);
    let config = bevy_playtest::battery::BatteryConfig {
        binary: env!("CARGO_BIN_EXE_fixture_known_answer").to_string(),
        scenario: scenario.to_string_lossy().into_owned(),
        timeout_secs: 300,
    };
    bevy_playtest::battery::run_with_config(&config)
        .expect("LW-lens run completes and reports (it is not a discovery run)")
}

/// Reconcile the report's LW evidence against a draft's live input
/// set — the composition measure and mutation tests both need.
#[cfg(feature = "measure")]
pub fn reconcile_report(
    report: &bevy_playtest::state::PlaytestReport,
    draft_live: &std::collections::BTreeSet<String>,
) -> bevy_playtest::discovery::lw::ReconciledLw {
    bevy_playtest::discovery::lw::reconcile(
        &bevy_playtest::discovery::lw::LwRunEvidence {
            declared: report.lw_declared.clone(),
            stats: report.lw_tap_stats.clone(),
        },
        draft_live,
    )
}
