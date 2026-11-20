//! Known-answer measurement harness against the planted-truth
//! fixture — OPT-IN: `--features playtest,measure`.
//!
//! Asserts the Leafwing declared-tier commitments (reader + intent-tap
//! + reconcile, as stamped on every report's `lw_reconciled`) against
//! `ground_truth.json`, and guards the ground truth's closed
//! vocabularies. (The probe-sweep lens that used to live here went
//! away with input-surface discovery.)
//!
//! Run (single-threaded; run_scenario exclusivity):
//! ```text
//! cargo test --manifest-path fixtures/Cargo.toml -p fixture_known_answer \
//!   --features playtest,measure --test measure -- --test-threads=1
//! ```

#![cfg(feature = "measure")]

mod support;

use bevy_playtest::discovery::surface::{DiscoveryFindingKind, Evidence};
use std::collections::BTreeSet;
use support::{Context, EffectClass};

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
/// - NO UNEARNED GRADES: the per-run reconcile has no independent
///   liveness evidence, so it emits NO `ContextUnverified` (it would
///   flag every working binding); wiring-live entries still grade
///   `manager_pipeline_ok` and never alarm.
/// - CHORD DISCIPLINE: chord members get notes, never findings —
///   even though the chord FIRED under the scenario.
/// - RETRACTION: `retracted_residue` lists every declared input.
/// - BYTE STABILITY: two identical runs produce identical reconcile
///   outputs.
#[test]
fn lw_reconcile_lens_known_answer_commitments() {
    let gt = support::load_ground_truth();

    // Reconcile runs inside every run with no liveness evidence (`None`
    // live set): retraction lists every declared input, and no
    // ContextUnverified grade is emitted.
    let report_a = run_lw_lens("a");
    let report_b = run_lw_lens("b");
    let rec_a = report_a
        .lw_reconciled
        .clone()
        .expect("lw-reader run with Leafwing state must stamp lw_reconciled");
    let rec_b = report_b
        .lw_reconciled
        .clone()
        .expect("lw-reader run with Leafwing state must stamp lw_reconciled");

    // Byte stability across the two independent child runs.
    assert_eq!(
        rec_a, rec_b,
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

    // Findings beyond the alarm: none. Without liveness evidence the
    // reconcile emits no ContextUnverified, and chord members NEVER
    // carry findings (note-only, even when fired).
    {
        let others: Vec<&str> = rec_a
            .findings
            .iter()
            .filter(|f| !matches!(f.kind, DiscoveryFindingKind::DeclaredButDead))
            .map(|f| f.input.as_str())
            .collect();
        assert!(
            others.is_empty(),
            "per-run reconcile must emit only the alarm, got: {others:?}"
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
            "retracted_residue lists every declared input"
        );
    }
}
