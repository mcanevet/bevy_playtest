//! Discovery measurement harness (D1-c SWEEP lens) against the
//! planted-truth fixture — OPT-IN: `--features playtest,measure`.
//!
//! Runs the tier-0 draft sweep (`bevy_playtest::discovery::draft`,
//! the same layer the `bevy_playtest_discover` CLI drives) against
//! the compiled fixture binary with a candidate list tailored to the
//! plant catalog, then scores the DRAFT — live entries, inert
//! entries, findings, and honest coverage — against
//! `ground_truth.json` row by row and ASSERTS the known-answer
//! commitments (misses included — they are the point, and they must
//! be ACKNOWLEDGED in the coverage strings, not hidden).
//!
//! Negative rows measure against inert ENTRIES (probe_run_inert
//! evidence), not absence: an inert key the sweep probed and ruled
//! out must appear in the draft, because a curated surface built
//! from silence could not tell "ruled out" from "never looked".
//!
//! Cache lives in a fresh temp subdir (NEVER the repo's
//! `.bevy_playtest/`); mutated runs (tests/mutation.rs) disambiguate
//! via `fixture_name`.
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

use bevy_playtest::discovery::draft::{draft_surface, DraftOutcome, MISSED_BY_DESIGN};
use bevy_playtest::discovery::flow::DiscoveryConfig;
use bevy_playtest::discovery::probe_scenarios::ProbePlan;
use bevy_playtest::discovery::surface::{
    BuildProfile, DiscoveryFindingKind, Evidence, SurfaceConfidence, SurfaceEntry,
};
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

/// Run the tier-0 draft sweep on the fixture (unmutated). One cold
/// config per call; callers pass a cache tag.
fn draft_fixture(tag: &str) -> DraftOutcome {
    let config = DiscoveryConfig {
        plan: ProbePlan {
            candidate_keys: candidate_keys(),
            ..ProbePlan::default()
        },
        cache_dir: temp_cache(tag),
        fixture_name: Some(format!("known_answer_{tag}")),
        ..DiscoveryConfig::new(env!("CARGO_BIN_EXE_fixture_known_answer"))
    };
    let draft = draft_surface(&config).expect("draft sweep completes");
    assert!(
        !draft.from_cache,
        "fresh temp cache must probe, not recall a stale entry"
    );
    draft
}

/// The draft's entry for one input, if any.
fn entry<'a>(draft: &'a DraftOutcome, key: &str) -> Option<&'a SurfaceEntry> {
    draft.entries.iter().find(|e| e.input == key)
}

/// Sweep-lens liveness: a live entry — grade `probe_verified` WITH an
/// observed effect signature. Inert entries never count as live.
fn live(draft: &DraftOutcome, key: &str) -> bool {
    entry(draft, key)
        .map(|e| e.effect_signature.is_some())
        .unwrap_or(false)
}

/// Sweep-lens inertness: an entry backed by `probe_run_inert`
/// evidence — the trap-pinning upgrade this harness exists for.
/// Negative ground-truth rows measure against inert ENTRIES, never
/// against mere absence in the draft.
fn inert(draft: &DraftOutcome, key: &str) -> bool {
    entry(draft, key)
        .map(|e| {
            e.effect_signature.is_none()
                && e
                    .evidence
                    .iter()
                    .any(|ev| matches!(ev, Evidence::ProbeRunInert { .. }))
        })
        .unwrap_or(false)
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

/// Grade the sweep lens over the DRAFT: for each non-wildcard
/// ground-truth row, predicted-live := a live entry exists for that
/// input; expected-live := `boot_live` (is it a REAL binding?) — the
/// chord/debounce plants are boot-live bindings the sweep MUST
/// miss, so they must land in FN, never quietly in TN. Adversarial
/// negatives are expected-inert by definition.
///
/// L-marker rows (the D1-d declared-tier plants) are EXCLUDED: they
/// are committed for a different lens (see
/// `lw_reconcile_lens_known_answer_commitments`), and the D1-c
/// confusion matrix — TP=9/FN=3/TN=12 over the sweep-lens rows — is
/// a frozen measurement contract this loop must not perturb.
fn sweep_grade(draft: &DraftOutcome) -> Confusion {
    let gt = support::load_ground_truth();
    let mut c = Confusion::default();
    for row in &gt.entries {
        if row.input == "*" {
            continue; // N3 wildcard: no per-key claim.
        }
        if row.plant_marker.starts_with('L') {
            continue; // Declared-tier rows: graded by the LW lens.
        }
        let predicted_live = live(draft, &row.input);
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

/// The core known-answer commitments of the D1-c sweep lens, all
/// asserted:
/// - P1–P3, P6 (dev build) discovered live at boot, each carrying a
///   `DiscoveredButUndeclared` finding;
/// - P4/P5 EXPECTED MISSES (chord/debounce) — asserted as PINNED
///   INERT ENTRIES and asserted ACKNOWLEDGED in the coverage's
///   named limitation strings;
/// - P7 context-gated → NOT discovered, inert entry, no drift claim;
/// - N1 dead hotbar slots, N4, N2/N3 shapes: pinned inert entries;
/// - determinism gate passes; build profile promoted from the game
///   child; coverage partitions; artifact round-trips.
#[test]
fn sweep_lens_known_answer_commitments() {
    let draft = draft_fixture("sweep");
    assert_eq!(
        draft.surface_confidence,
        SurfaceConfidence::Exact,
        "determinism gate failed: controls diverged ({:?}) — \
         trusted negatives are off the table",
        draft.surface_confidence
    );

    // The game child's build profile was promoted onto the draft
    // (roadmap §4 item 5): the fixture binary compiled for this test
    // run is a dev build, so the draft must say so — a surface
    // without its profile invites cross-profile comparisons.
    assert_eq!(
        draft.build_profile,
        Some(BuildProfile::Debug),
        "the fixture child is a dev build; the draft must carry its profile"
    );

    // P1–P3 + N1's live slots: discovered live.
    for key in [
        "KeyU", "KeyW", "KeyA", "KeyS", "KeyD", "KeyR", "Digit1", "Digit2",
    ] {
        assert!(
            live(&draft, key),
            "P-plant {key} must be discovered live at boot"
        );
    }
    // P6: dev-profile build ⇒ KeyG live (release-profile expectation
    // is a recorded TODO — see ground_truth bevy_profile_note).
    if cfg!(debug_assertions) {
        assert!(live(&draft, "KeyG"), "P6 KeyG must be live in a dev-profile sweep");
    } else {
        assert!(
            !live(&draft, "KeyG"),
            "P6 is cfg(debug_assertions)-gated: a release sweep must NOT see it"
        );
    }

    // P4/P5: EXPECTED misses of single-key probing (chord, debounce) —
    // and the honest-coverage commitment: the draft must both PIN
    // them as inert entries AND acknowledge them in the named
    // limitation strings. Measured, not hidden.
    for key in ["KeyL", "ShiftLeft", "KeyT"] {
        assert!(
            !live(&draft, key),
            "{key} must be an expected sweep miss (chord/debounce limitation)"
        );
        assert!(
            inert(&draft, key),
            "{key} must be a PINNED inert entry — an expected miss is \
             measured (probed, ruled out), never silently absent"
        );
    }
    // Every named limitation class is present VERBATIM: the draft
    // acknowledges exactly what it cannot see.
    for class in MISSED_BY_DESIGN {
        assert!(
            draft.coverage.missed_by_design.iter().any(|m| m == class),
            "missed-by-design must acknowledge the limitation class verbatim:\n{class}"
        );
    }
    let acknowledged = draft.coverage.missed_by_design.join(" ").to_lowercase();
    assert!(acknowledged.contains("chord"), "chord limitation named");
    assert!(
        acknowledged.contains("debounce") || acknowledged.contains("cooldown"),
        "debounce limitation named"
    );
    assert!(acknowledged.contains("context"), "context limitation named");

    // P7: context-gated, dead at boot — ruled out AT BOOT, never a
    // claim about other contexts (that is the context limitation's
    // own row above).
    assert!(
        !live(&draft, "KeyV"),
        "P7 KeyV must NOT be discovered at boot"
    );
    assert!(
        inert(&draft, "KeyV"),
        "P7 KeyV must be a pinned inert entry (ruled out at boot)"
    );

    // N1 dead slots + N4 + dormant rebind target: pinned inert
    // entries (trap-pinning: the negatives are ENTRY-visible, not
    // absence-visible).
    for key in [
        "Digit3", "Digit4", "Digit5", "Digit6", "Digit7", "Digit8", "Digit9", "Digit0", "KeyX",
        "KeyM", "Enter",
    ] {
        assert!(
            !live(&draft, key),
            "{key} must be graded inert (adversarial negative)"
        );
        assert!(
            inert(&draft, key),
            "{key} must be a pinned inert entry — silence about a probed \
             negative is the residue hole the draft exists to close"
        );
    }

    // The control key supports NO verdict (its self-probe equals its
    // baseline by construction) — no entry of ANY kind may exist.
    assert!(
        entry(&draft, "KeyQ").is_none(),
        "the control key's self-probe is structurally unclassifiable: no entry"
    );

    // Findings ↔ live entries: a bijection of
    // discovered_but_undeclared findings, sorted, one per live input.
    let live_inputs: BTreeSet<&str> = draft
        .entries
        .iter()
        .filter(|e| e.effect_signature.is_some())
        .map(|e| e.input.as_str())
        .collect();
    let finding_inputs: BTreeSet<&str> =
        draft.findings.iter().map(|f| f.input.as_str()).collect();
    assert_eq!(
        finding_inputs, live_inputs,
        "every live entry carries exactly one finding naming it, and vice versa"
    );
    for f in &draft.findings {
        assert!(
            matches!(f.kind, bevy_playtest::discovery::surface::DiscoveryFindingKind::DiscoveredButUndeclared),
            "D1-c findings vocabulary is exactly discovered_but_undeclared: {:?}",
            f.kind
        );
    }

    // Per-ground-truth-row findings: every boot-live row the sweep
    // CAN see (probe-visible ∧ boot-live) must be named by a
    // finding — the residue report is row-complete.
    let gt = support::load_ground_truth();
    for row in &gt.entries {
        if row.input == "*" {
            continue;
        }
        let expected_live = match &row.expected_grade {
            ExpectedGrade::Methods(m) => m.boot_live,
            ExpectedGrade::Negative(_) => false,
        };
        // P6's cfg gate: KeyG is genuinely absent from release builds.
        if row.input == "KeyG" && !cfg!(debug_assertions) {
            continue;
        }
        if expected_live && !live(&draft, &row.input) {
            continue; // Expected misses (FN) are asserted below.
        }
        if expected_live {
            assert!(
                finding_inputs.contains(row.input.as_str()),
                "ground-truth live row {} must carry a discovered_but_undeclared finding",
                row.input
            );
        }
    }

    // Confusion matrix, printed for --nocapture inspection and
    // asserted: zero false positives, and the false negatives are
    // EXACTLY the documented chord/debounce misses (KeyL,
    // ShiftLeft, KeyT). Counts are the dev-profile commitments
    // (TP=9 incl. KeyG, TN=12 over 24 graded rows); the
    // release-profile sweep is the ground truth's recorded TODO.
    let c = sweep_grade(&draft);
    c.print("sweep");
    assert_eq!(c.fp, 0, "sweep lens must have ZERO false positives");
    assert_eq!(
        c.fn_, 3,
        "expected misses: exactly KeyL, ShiftLeft (P4 chord) and \
         KeyT (P5 debounce) — measured, not hidden"
    );
    assert_eq!(c.tp, 9, "dev-profile commitments: 9 true positives");
    assert_eq!(c.tn, 12, "dev-profile commitments: 12 true negatives");

    // FULL candidate-matrix discipline: the predicted-live SET must
    // equal exactly the ground truth's probe-visible set. This
    // covers the whole probed matrix (~36 keys), not just the rows
    // in ground truth — a spurious divergence on any probed key
    // with no truth row (ArrowUp/Down/Left/Right, Space, Escape,
    // Tab, Backspace, ControlLeft, AltLeft, KeyE, KeyF, ...) is a
    // FALSE POSITIVE here even though it inflates no row counter.
    // This is the false-positive class the fixture exists to catch.
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
    // the expectation when compiled out.
    let mut expected_live_set: BTreeSet<&str> = expected_live_set;
    if !cfg!(debug_assertions) {
        expected_live_set.remove("KeyG");
    }
    assert_eq!(
        live_inputs, expected_live_set,
        "predicted-live set must equal the truth's probe-visible set \
         over the ENTIRE probed matrix — any extra key is an \
         untruth-row false positive; any missing key is a missed \
         positive"
    );

    // Coverage partitions the probed candidates exactly, and the
    // entry kinds partition with it: verified == live entries,
    // inert == inert entries.
    assert_eq!(
        draft.coverage.candidates_probed,
        draft.coverage.verified + draft.coverage.inert,
        "coverage must PARTITION the probed candidates (Exact confidence only)"
    );
    assert_eq!(
        draft.coverage.verified,
        live_inputs.len(),
        "verified count == live entries"
    );
    let inert_entries = draft
        .entries
        .iter()
        .filter(|e| e.effect_signature.is_none())
        .count();
    assert_eq!(
        draft.coverage.inert, inert_entries,
        "inert count == inert entries"
    );

    // Determinism of shape: entries and findings are sorted by input.
    let entry_order: Vec<&str> = draft.entries.iter().map(|e| e.input.as_str()).collect();
    let mut sorted = entry_order.clone();
    sorted.sort();
    assert_eq!(entry_order, sorted, "entries sorted by input");
    let finding_order: Vec<&str> = draft.findings.iter().map(|f| f.input.as_str()).collect();
    let mut sorted = finding_order.clone();
    sorted.sort();
    assert_eq!(finding_order, sorted, "findings sorted by input");

    // Artifact round-trip: the draft is JSON with `draft: true` in
    // its header and survives serde intact.
    let json = serde_json::to_string_pretty(&draft).unwrap();
    assert!(
        json.contains("\"draft\": true"),
        "the artifact must wear its draft marker in the header"
    );
    let back: DraftOutcome = serde_json::from_str(&json).unwrap();
    assert_eq!(back, draft, "artifact round-trips through the typed shape");

    // Cross-lens accounting note (honest coverage): D1-c source-scan
    // commitments live on the same rows — TODO(D1-c scan): assert
    // source_declared lens once the scan lands. The D1-d declared-
    // reader lens IS asserted: see
    // `lw_reconcile_lens_known_answer_commitments` below (and the
    // alarm direction under mutation in tests/mutation.rs).
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

// ---------------------------------------------------------------------------
// D1-d declared-tier lens: Leafwing reader + intent-tap + reconcile.
// The scenario + battery runner live in `tests/support` so the
// measure and mutation binaries share ONE definition.
// ---------------------------------------------------------------------------

/// Run the fixture under the deterministic LW-lens scenario —
/// single-sourced in `tests/support` (measure + mutation share it).
fn run_lw_lens(tag: &str) -> bevy_playtest::state::PlaytestReport {
    support::run_lw_lens(tag)
}

/// The D1-d declared-tier commitments, all asserted:
///
/// - READER COMPLETENESS: the run's declared rows enumerate ALL SEVEN
///   bindings across BOTH maps (multi-map, multi-`Actionlike`),
///   including the RON-loaded one — reflection only, no Leafwing
///   code in the harness.
/// - INTENT-TAP: press/fire correlation matches `lw_rows.tap`
///   row by row; chords fire without member-press attribution.
/// - ALARM PRECISION: exactly ONE `DeclaredButDead` (L1 KeyN,
///   disabled arm) across the whole pristine reconcile — zero false
///   taps is the channel's survival condition.
/// - HONESTY GRADES: Ignored/Gated/RonBound fire at wiring level
///   but carry no verified game effect → `ContextUnverified`, never
///   alarms (wiring liveness ≠ game liveness).
/// - CHORD DISCIPLINE: chord members get notes, never findings —
///   even though the chord FIRED under the scenario.
/// - RETRACTION: KeyJ's draft `DiscoveredButUndeclared` finding is
///   listed in `retracted_residue`; its entry grade is
///   `manager_pipeline_ok` (declared ∧ tap-fired ∧ sweep-verified).
/// - BYTE STABILITY: two identical runs produce identical reconcile
///   outputs (wire-form comparison).
#[test]
fn lw_reconcile_lens_known_answer_commitments() {
    let gt = support::load_ground_truth();
    let draft = draft_fixture("lwlens");

    // Draft live input set feeds reconcile's retraction and
    // ContextUnverified logic. KeyJ IS draft-live (LwUse witness).
    let draft_live: BTreeSet<String> = draft
        .entries
        .iter()
        .filter(|e| e.effect_signature.is_some())
        .map(|e| e.input.clone())
        .collect();

    let report_a = run_lw_lens("a");
    let report_b = run_lw_lens("b");
    let rec_a = support::reconcile_report(&report_a, &draft_live);
    let rec_b = support::reconcile_report(&report_b, &draft_live);

    // Byte stability across the two independent child runs (same
    // wire-form trick: `ReconciledLw` has no Eq derive, so compare
    // serde forms; each field serializes by
    // REFERENCE so `rec_a` stays usable below).
    let wire = |rec: &bevy_playtest::discovery::lw::ReconciledLw| {
        (
            serde_json::to_string(&rec.entries).unwrap(),
            serde_json::to_string(&rec.findings).unwrap(),
            serde_json::to_string(&rec.retracted_residue).unwrap(),
        )
    };
    let wire_a = wire(&rec_a);
    let wire_b = wire(&rec_b);
    assert_eq!(
        wire_a, wire_b,
        "two identical deterministic runs must produce identical reconcile \
         outputs — the determinism gate extends to the declared tier"
    );

    // Reader completeness: the declared rows are exactly the seven
    // `lw_rows` (action, input) pairs — across both maps, including
    // the RON-loaded binding. Sort-insensitive, per-field asserted.
    {
        use std::collections::BTreeSet as Set;
        let declared: Set<(String, String)> = report_a
            .lw_declared
            .iter()
            .map(|d| (d.action.clone(), d.input.clone()))
            .collect();
        let expected: Set<(String, String)> = gt
            .lw_rows
            .iter()
            .map(|r| (r.action.clone(), r.input.clone()))
            .collect();
        assert_eq!(
            declared, expected,
            "the reader must enumerate ALL declared bindings — multi-map, \
             multi-Actionlike, RON-loaded included — exactly once each"
        );
        for row in &gt.lw_rows {
            let d = report_a
                .lw_declared
                .iter()
                .find(|d| d.action == row.action && d.input == row.input)
                .unwrap_or_else(|| {
                    panic!("reader missed declared binding {}({})", row.action, row.input)
                });
            assert_eq!(
                d.input_kind, "KeyCode",
                "{}({}): normalized KeyCode spelling expected",
                row.action,
                row.input
            );
            let expect_chord = matches!(row.tap, support::LwTapExpectation::ChordMember);
            assert_eq!(
                d.chord, expect_chord,
                "{}({}): reader chord flag must match the RON/code map truth",
                row.action,
                row.input
            );
        }
    }

    // Intent-tap row-by-row against `lw_rows.tap`. The stats join is
    // (action_map, action) — matching on action ALONE would silently
    // cross-join same-named actions from different maps. The row's
    // map comes from the declared rows (one map per (action, input)
    // pair in this fixture's truth).
    for row in &gt.lw_rows {
        let action_map = report_a
            .lw_declared
            .iter()
            .find(|d| d.action == row.action && d.input == row.input)
            .map(|d| d.action_map.clone())
            .expect("every lw_row must have a matching declared row");
        let stat = report_a
            .lw_tap_stats
            .iter()
            .find(|s| s.action == row.action && s.action_map == action_map)
            .expect("one tap-stats row per (action-map, action) with a press");
        let is_chord_member = report_a
            .lw_declared
            .iter()
            .any(|d| d.action == row.action && d.input == row.input && d.chord);
        match row.tap {
            support::LwTapExpectation::PressedAndFired => {
                assert!(
                    stat.pressed_frames > 0 && stat.fired_frames > 0 && !stat.disabled,
                    "{}({}): pressed-and-fired expected, got pressed={} fired={} disabled={}",
                    row.action,
                    row.input,
                    stat.pressed_frames,
                    stat.fired_frames,
                    stat.disabled
                );
            }
            support::LwTapExpectation::PressedNeverFiredDisabled => {
                assert!(
                    stat.pressed_frames > 0
                        && stat.fired_frames == 0
                        && stat.disabled,
                    "{}({}): pressed-never-fired-disabled expected, got \
                     pressed={} fired={} disabled={}",
                    row.action,
                    row.input,
                    stat.pressed_frames,
                    stat.fired_frames,
                    stat.disabled
                );
            }
            support::LwTapExpectation::ChordMember => {
                assert!(
                    is_chord_member,
                    "{}({}): lw_rows declares a chord member but the reader \
                     did not mark chord: true",
                    row.action,
                    row.input
                );
                // Chord members are EXCLUDED from press attribution
                // (a lone member press cannot fire the chord): the
                // deterministic scenario's coordinated multi-press
                // fires the chord WITHOUT attributing member presses.
                assert_eq!(
                    stat.pressed_frames, 0,
                    "{}({}): chord members must not accumulate driver presses",
                    row.action,
                    row.input
                );
            }
        }
    }

    // Alarm precision: exactly ONE DeclaredButDead (L1 KeyN) in the
    // pristine run — zero false taps keeps the alarm channel alive.
    let alarms: Vec<&bevy_playtest::discovery::surface::DiscoveryFinding> = rec_a
        .findings
        .iter()
        .filter(|f| matches!(f.kind, DiscoveryFindingKind::DeclaredButDead))
        .collect();
    assert_eq!(
        alarms.len(),
        1,
        "pristine reconcile must raise exactly one alarm (disabled-arm L1), \
         got: {:?}",
        alarms
            .iter()
            .map(|f| f.input.as_str())
            .collect::<Vec<_>>()
    );
    assert_eq!(alarms[0].input, "KeyN", "the one pristine alarm is L1 KeyN");

    // Findings beyond the alarm: exactly the ContextUnverified set
    // {KeyO, KeyP, KeyH} — wiring live, game effect unverifiable.
    // Chord members NEVER appear (note-only, even when fired).
    {
        let unverified: BTreeSet<&str> = rec_a
            .findings
            .iter()
            .filter(|f| matches!(f.kind, DiscoveryFindingKind::ContextUnverified))
            .map(|f| f.input.as_str())
            .collect();
        let expected: BTreeSet<&str> = ["KeyO", "KeyP", "KeyH"]
            .into_iter()
            .collect();
        assert_eq!(
            unverified, expected,
            "ContextUnverified must name exactly the tap-ok-but-unverified \
             bindings: Ignored, Gated, RonBound"
        );
        assert!(
            !unverified.contains("ControlLeft") && !unverified.contains("KeyI"),
            "chord members must NEVER carry findings"
        );
    }

    // Entry grades row by row: healthy L5 gets manager_pipeline_ok;
    // everything declared-but-not-fired gets `declared` (the
    // reconcile emits entry grade Declared for wiring-dead/unfired
    // rows; the ALARM distinction lives on the findings channel).
    for row in &gt.lw_rows {
        let entry = rec_a
            .entries
            .iter()
            .find(|e| e.input == row.input)
            .unwrap_or_else(|| {
                panic!("reconcile dropped declared entry for {}", row.input)
            });
        match row.pristine_reconcile {
            support::LwReconcileExpectation::ManagerPipelineOk => {
                assert!(
                    entry.evidence.iter().any(|ev| matches!(
                        ev,
                        Evidence::TapObserved { stage } if stage == "manager_pipeline"
                    )),
                    "{} must carry manager_pipeline tap evidence",
                    row.input
                );
            }
            support::LwReconcileExpectation::ChordNote => {
                assert!(
                    entry.notes.iter().any(|n| n.contains("chord member")),
                    "{} must carry the chord-member note",
                    row.input
                );
            }
            _ => {}
        }
    }

    // Retraction: the reader's declared inputs are ALL named in
    // retracted_residue (sorted, deduped — two chord rows share the
    // Combo action but contribute distinct inputs).
    {
        let mut expected: Vec<String> = gt
            .lw_rows
            .iter()
            .map(|r| r.input.clone())
            .collect();
        expected.sort();
        expected.dedup();
        assert_eq!(
            rec_a.retracted_residue, expected,
            "retract ALL declared inputs' draft residue: any leftover \
             DiscoveredButUndeclared on a declared input is duplication"
        );
    }
}
