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

/// The committed ground truth (schema v1).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GroundTruth {
    pub fixture: String,
    pub schema_version: u32,
    pub bevy_profile_note: String,
    pub entries: Vec<GroundTruthEntry>,
    pub findings_expected: Vec<FindingExpectation>,
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
/// P1..P7, N1..N4). Drift either direction fails the test.
pub const REQUIRED_MARKERS: &[&str] = &[
    "P1", "P2", "P3", "P4", "P5", "P6", "P7", "N1", "N2", "N3", "N4",
];

/// Candidate keys the plant catalog adds on top of the harness's
/// default probe set (measure/mutation harnesses both probe the
/// default set UNION these). Single-sourced here so the two test
/// binaries cannot drift apart. The control key `KeyQ` is never
/// bound and never a candidate.
pub const EXTRA_CANDIDATE_KEYS: &[&str] = &["KeyU", "KeyL", "KeyT", "KeyG", "KeyV", "KeyX", "KeyM"];
