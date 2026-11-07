//! Discovery measurement harness (D0 sweep lens) against the
//! planted-truth fixture — OPT-IN: `--features playtest,measure`.
//!
//! Runs the D0 public discovery API against the compiled fixture
//! binary with a candidate list tailored to the plant catalog, then
//! scores the result against `ground_truth.json` row by row and
//! ASSERTS the known-answer commitments (misses included — they
//! are the point). Cache lives in a fresh temp subdir (NEVER the
//! repo's `.bevy_playtest/`); mutated runs (tests/mutation.rs)
//! disambiguate via `fixture_name`.
//!
//! Run (single-threaded; probe-pool exclusivity):
//! ```text
//! cargo test --manifest-path fixtures/Cargo.toml -p fixture_known_answer \
//!   --features playtest,measure --test measure -- --test-threads=1
//! ```
//!
//! NOTE the serialization discipline: tests in this binary run
//! serially ONLY via --test-threads=1 (the documented battery-tier
//! convention; there is one binary-level resource — the probe
//! pool's exclusive run_scenario — that does not queue).

#![cfg(feature = "measure")]

mod support;

use bevy_playtest::discovery::flow::{DiscoveryConfig, discover};
use bevy_playtest::discovery::probe_scenarios::ProbePlan;
use bevy_playtest::discovery::surface::{KeyConfidence, SurfaceConfidence};
use std::collections::BTreeSet;
use support::{Context, EffectClass, ExpectedGrade};

/// Candidate keys the plant catalog adds on top of the harness's
/// default probe set (measure/mutation harnesses both probe the
/// default set UNION these). Single-sourced in
/// `support::EXTRA_CANDIDATE_KEYS` so the two test binaries cannot
/// drift apart. KeyQ stays the control key and is NEVER a candidate.
fn candidate_keys() -> Vec<String> {
    let mut keys: Vec<String> = bevy_playtest::discovery::probe_scenarios::PROBE_CANDIDATE_KEYS
        .iter()
        .map(|s| s.to_string())
        .collect();
    for extra in support::EXTRA_CANDIDATE_KEYS {
        if !keys.iter().any(|k| k == extra) {
            keys.push(extra.to_string());
        }
    }
    keys
}

/// Fresh per-test cache directory under the system temp dir. The
/// repo's `.bevy_playtest/` cache is NEVER touched.
fn temp_cache(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "bevy_playtest_known_answer_measure_{}_{}_{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("temp cache dir creatable");
    dir
}

/// Discover the fixture's surface (unmutated). One config per test
/// binary run is the norm; callers may pass a cache tag.
fn discover_surface(tag: &str) -> bevy_playtest::discovery::surface::DiscoveredSurface {
    let config = DiscoveryConfig {
        plan: ProbePlan {
            candidate_keys: candidate_keys(),
            ..ProbePlan::default()
        },
        cache_dir: temp_cache(tag),
        fixture_name: Some(format!("known_answer_{tag}")),
        ..DiscoveryConfig::new(env!("CARGO_BIN_EXE_fixture_known_answer"))
    };
    let outcome = discover(&config).expect("discovery completes");
    outcome.surface
}

/// Per-lens confusion counts, printed AND asserted.
#[derive(Default)]
struct Confusion {
    tp: usize,
    fp: usize,
    fn_: usize,
    tn: usize,
}

impl Confusion {
    fn print(&self, lens: &str) {
        println!(
            "[known_answer:{lens}] TP={} FP={} FN={} TN={}",
            self.tp, self.fp, self.fn_, self.tn
        );
    }
}

/// Grade the sweep lens: for each non-wildcard ground-truth row,
/// predicted-live := the surface reports a diverging probe for that
/// key; expected-live := `boot_live` (is it a REAL binding?) — the
/// chord/debounce plants are boot-live bindings the sweep MUST
/// miss, so they must land in FN, never quietly in TN. Adversarial
/// negatives are expected-inert by definition.
fn sweep_grade(surface: &bevy_playtest::discovery::surface::DiscoveredSurface) -> Confusion {
    let gt = support::load_ground_truth();
    let mut c = Confusion::default();
    for row in &gt.entries {
        if row.input == "*" {
            continue; // N3 wildcard: no per-key claim.
        }
        let finding = surface.keys.iter().find(|k| k.key == row.input);
        let predicted_live = finding
            .map(|f| f.effect.is_some() && f.confidence != KeyConfidence::Unclassified)
            .unwrap_or(false);
        let expected_live = match &row.expected_grade {
            ExpectedGrade::Methods(m) => m.boot_live,
            ExpectedGrade::Negative(_) => false,
        };
        match (predicted_live, expected_live) {
            (true, true) => c.tp += 1,
            (true, false) => c.fp += 1,
            (false, true) => c.fn_ += 1,
            (false, false) => c.tn += 1,
        }
    }
    c
}

/// The core known-answer commitments, all asserted:
/// - P1–P3, P6 (dev build) discovered live at boot;
/// - P4/P5 EXPECTED MISSES (chord/debounce) — asserted inert;
/// - P7 context-gated → NOT discovered;
/// - N4 inert; N1 dead hotbar slots inert; N2/N3 shapes invisible;
/// - determinism controls pass (`SurfaceConfidence::Exact`).
#[test]
fn sweep_lens_known_answer_commitments() {
    let surface = discover_surface("sweep");
    assert!(
        matches!(surface.confidence, SurfaceConfidence::Exact),
        "determinism gate failed: controls diverged ({:?}) — \
         trusted negatives are off the table",
        surface.confidence
    );

    let live = |key: &str| {
        surface
            .keys
            .iter()
            .find(|k| k.key == key)
            .map(|f| f.effect.is_some() && f.confidence != KeyConfidence::Unclassified)
            .unwrap_or(false)
    };

    // P1–P3 + N1's live slots: discovered.
    for key in [
        "KeyU", "KeyW", "KeyA", "KeyS", "KeyD", "KeyR", "Digit1", "Digit2",
    ] {
        assert!(live(key), "P-plant {key} must be discovered live at boot");
    }
    // P6: dev-profile build ⇒ KeyG live (release-profile expectation
    // is a recorded TODO — see ground_truth bevy_profile_note).
    if cfg!(debug_assertions) {
        assert!(live("KeyG"), "P6 KeyG must be live in a dev-profile sweep");
    }
    // P4/P5: EXPECTED misses of single-key probing (chord, debounce).
    for key in ["KeyL", "ShiftLeft", "KeyT"] {
        assert!(
            !live(key),
            "{key} must be an expected sweep miss (chord/debounce limitation)"
        );
    }
    // P7: context-gated, dead at boot.
    assert!(!live("KeyV"), "P7 KeyV must NOT be discovered at boot");
    // N1 dead slots + N4 + dormant rebind target: inert.
    for key in [
        "Digit3", "Digit4", "Digit5", "Digit6", "Digit7", "Digit8", "Digit9", "Digit0", "KeyX",
        "KeyM", "Enter",
    ] {
        assert!(
            !live(key),
            "{key} must be graded inert (adversarial negative)"
        );
    }

    // Confusion matrix, printed for --nocapture inspection and
    // asserted: zero false positives, and the false negatives are
    // EXACTLY the documented chord/debounce misses (KeyL,
    // ShiftLeft, KeyT) — plus KeyV only if P7 were graded
    // probe-visible (it is not: context-gated, boot_live false).
    let c = sweep_grade(&surface);
    c.print("sweep");
    assert_eq!(c.fp, 0, "sweep lens must have ZERO false positives");
    assert_eq!(
        c.fn_, 3,
        "expected misses: exactly KeyL, ShiftLeft (P4 chord) and \
         KeyT (P5 debounce) — measured, not hidden"
    );

    // FULL candidate-matrix discipline: the predicted-live SET must
    // equal exactly the ground truth's probe-visible set. This
    // covers the whole probed matrix (~36 keys), not just the rows
    // in ground truth — a spurious divergence on any probed key
    // with no truth row (ArrowUp/Down/Left/Right, Space, Escape,
    // Tab, Backspace, ControlLeft, AltLeft, KeyE, KeyF, ...) is a
    // FALSE POSITIVE here even though it inflates no row counter.
    // This is the false-positive class the fixture exists to catch.
    let predicted_live_set: BTreeSet<&str> = surface
        .keys
        .iter()
        .filter(|f| f.effect.is_some() && f.confidence != KeyConfidence::Unclassified)
        .map(|f| f.key.as_str())
        .collect();
    let gt = support::load_ground_truth();
    let expected_live_set: BTreeSet<&str> = gt
        .entries
        .iter()
        .filter(|r| r.input != "*")
        .filter(|r| match &r.expected_grade {
            ExpectedGrade::Methods(m) => m.probe_visible && m.boot_live,
            ExpectedGrade::Negative(_) => false,
        })
        .map(|r| r.input.as_str())
        .collect();
    // KeyG is probe-visible only in dev-profile builds (P6); the
    // truth file unconditionally marks it live, so subtract it from
    // the expectation when compiled out. (Release-profile
    // measurement is a recorded TODO — see bevy_profile_note.)
    let mut expected_live_set: BTreeSet<&str> = expected_live_set;
    if !cfg!(debug_assertions) {
        expected_live_set.remove("KeyG");
    }
    assert_eq!(
        predicted_live_set, expected_live_set,
        "predicted-live set must equal the truth's probe-visible set \
         over the ENTIRE probed matrix — any extra key is an \
         untruth-row false positive; any missing key is a missed \
         positive"
    );

    // Cross-lens accounting note (honest coverage): D1-c source-scan
    // and D1-d declared-reader commitments live on the same rows —
    // TODO(D1-c): assert source_declared lens once the scan lands;
    // TODO(D1-d): assert DeclaredButDead findings once the
    // Leafwing/bespoke reader exists (see tests/mutation.rs).
}

/// Contexts and effect classes in the ground truth stay within the
/// committed vocabulary (guards hand-edits from drifting schema).
#[test]
fn ground_truth_vocabularies_are_closed() {
    let gt = support::load_ground_truth();
    for entry in &gt.entries {
        // Every entry's context is one of the four closed values.
        match entry.context {
            Context::MenuFocused | Context::Gameplay | Context::Boot | Context::Unreachable => {}
        }
        match entry.expected_effect_class {
            EffectClass::Diverge | EffectClass::Inert => {}
        }
        // Wildcard rows are the logger's only legal use.
        if entry.input == "*" {
            assert_eq!(entry.plant_marker, "N3");
        }
    }
}
