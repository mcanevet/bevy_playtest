//! Known-answer fixture tests — BATTERY TIER (plain
//! `cargo test --features playtest`): the standard battery run plus
//! the ground-truth integrity tests (round-trip + PLANT-marker
//! cross-check). The discovery-measurement tests live separately
//! in `tests/measure.rs` / `tests/mutation.rs`, gated behind the
//! `measure` feature (they need exclusive probe-pool access and are
//! slow — the plain battery must not pay for them).

mod support;

#[cfg(feature = "playtest")]
#[test]
fn playtest_battery() {
    bevy_playtest::battery::run_and_expect_clean(
        env!("CARGO_BIN_EXE_fixture_known_answer"),
        bevy_playtest::battery::BUILTIN_BATTERY,
    );
}

// `cargo package` strips `[dev-dependencies]`, so the self-dev-dependency
// that auto-activates `playtest` is gone in an unpacked archive. Rather
// than fail to compile (missing `bevy_playtest`) or silently run zero
// playtests, surface it as an ignored test pointing at the fix.
#[cfg(not(feature = "playtest"))]
#[test]
#[ignore = "playtest feature inactive: self-dev-dependency stripped (e.g. cargo package). Run `cargo test --features playtest`."]
fn playtest_battery() {}

// ---------------------------------------------------------------------------
// Ground-truth integrity (always runs: cheap, no binary needed).
// ---------------------------------------------------------------------------

/// Load + re-serialize == original bytes modulo formatting. The
/// committed file is hand-checkable; the typed loader must reproduce
/// it exactly (stable key order via serde's struct-field order).
#[test]
fn ground_truth_round_trip() {
    use support::*;

    let path = ground_truth_path();
    let original = std::fs::read_to_string(&path).unwrap();
    let parsed: GroundTruth =
        serde_json::from_str(&original).expect("ground_truth.json parses against the typed schema");
    let reserialized = serde_json::to_string_pretty(&parsed).unwrap();

    // Semantic equality, byte-stability modulo formatting: reparsing
    // the reserialized text must yield the identical value.
    let reparsed: GroundTruth =
        serde_json::from_str(&reserialized).expect("reserialization reparses");
    assert_eq!(parsed, reparsed, "loader round-trip must be lossless");

    // Basic sanity of the committed artifact itself.
    assert_eq!(parsed.fixture, "known_answer");
    assert_eq!(parsed.schema_version, 1);
    assert!(!parsed.entries.is_empty());

    // No duplicate inputs (each input asserts one claim).
    let mut inputs = parsed
        .entries
        .iter()
        .map(|e| e.input.as_str())
        .collect::<Vec<_>>();
    inputs.sort();
    inputs.dedup();
    assert_eq!(
        inputs.len(),
        parsed.entries.len(),
        "duplicate input rows in ground truth"
    );
}

/// Every `// PLANT` marker in the source has ≥1 ground-truth row,
/// every row's marker exists in source, and the full catalog P1..P7,
/// N1..N4 is present — anti-drift between the planted catalog and
/// the committed truth.
#[test]
fn plant_markers_match_ground_truth() {
    use support::*;

    let gt = load_ground_truth();
    let markers = plant_markers_in_source();

    // The committed catalog is complete.
    use std::collections::BTreeSet;
    for id in REQUIRED_MARKERS {
        assert!(
            markers.contains(*id),
            "// PLANT {id} missing from src/main.rs (catalog drift)"
        );
    }
    assert_eq!(
        markers.iter().map(String::as_str).collect::<BTreeSet<_>>(),
        REQUIRED_MARKERS.iter().copied().collect::<BTreeSet<_>>(),
        "src/main.rs has markers beyond the committed catalog"
    );

    // Every row cites an existing marker.
    let row_markers: std::collections::BTreeSet<String> =
        gt.entries.iter().map(|e| e.plant_marker.clone()).collect();
    for marker in &row_markers {
        assert!(
            markers.contains(marker),
            "ground-truth row cites marker '{marker}' that src/main.rs \
             does not carry"
        );
    }

    // Every marker has at least one row.
    for marker in &markers {
        assert!(
            row_markers.contains(marker),
            "marker '{marker}' has no ground-truth row"
        );
    }

    // Negative grades are reserved for adversarial negatives (and
    // the dormant rebind target), never for expected sweep misses:
    // P4/P5 stay `Methods { probe_visible: false, boot_live: true }`
    // so the miss is legible in the file itself.
    for entry in &gt.entries {
        if matches!(entry.expected_grade, ExpectedGrade::Negative(_)) {
            assert!(
                entry.plant_marker.starts_with('N')
                    || entry.input == "KeyM"
                    || entry.input == "Enter",
                "row {} carries a negative grade but is not an \
                 adversarial negative",
                entry.input
            );
        }
    }
}

/// `findings_expected` must agree with the per-key commitments the
/// measure/mutation harnesses hardcode — the JSON is committed
/// prose otherwise and could drift from the asserted sets
/// invisibly (an asymmetry with the marker↔row cross-check above,
/// which this closes).
#[test]
fn findings_expected_agree_with_hardcoded_sets() {
    use std::collections::BTreeSet;
    use support::*;

    let gt = load_ground_truth();
    let find = |kind: &str| -> Option<&FindingExpectation> {
        gt.findings_expected.iter().find(|f| match (&f.kind, kind) {
            (FindingKind::SweepExpectedMisses, "sweep_expected_misses") => true,
            (FindingKind::ContextGatedNotDiscovered, "context_gated_not_discovered") => true,
            (FindingKind::NegativeShapes, "negative_shapes") => true,
            _ => false,
        })
    };
    fn set<'a>(f: Option<&'a FindingExpectation>) -> BTreeSet<&'a str> {
        match f {
            Some(f) => f.inputs.iter().map(String::as_str).collect(),
            None => BTreeSet::new(),
        }
    }

    // Sweep expected misses: exactly the boot_live-but-not-probe-
    // visible rows (the chord/debounce plants).
    let committed_misses: BTreeSet<&str> = gt
        .entries
        .iter()
        .filter(|r| match &r.expected_grade {
            ExpectedGrade::Methods(m) => m.boot_live && !m.probe_visible,
            ExpectedGrade::Negative(_) => false,
        })
        .map(|r| r.input.as_str())
        .collect();
    let declared_misses = set(find("sweep_expected_misses"));
    assert_eq!(
        committed_misses, declared_misses,
        "sweep_expected_misses finding must equal the boot-live / \
         not-probe-visible rows (measure.rs asserts exactly this set)"
    );

    // Context-gated not-discovered: the boot_dead rows.
    let committed_gated: BTreeSet<&str> = gt
        .entries
        .iter()
        .filter(|r| match &r.expected_grade {
            ExpectedGrade::Methods(m) => !m.boot_live,
            ExpectedGrade::Negative(_) => false,
        })
        .map(|r| r.input.as_str())
        .collect();
    let declared_gated = set(find("context_gated_not_discovered"));
    assert_eq!(
        committed_gated, declared_gated,
        "context_gated_not_discovered finding must equal the \
         not-boot-live rows"
    );

    // Negative shapes: exactly the negative-graded, non-wildcard
    // rows (the adversarial-negatives loop in measure.rs asserts
    // every one of these inert; the sweep-equality assertion covers
    // the rest of the matrix).
    let committed_negatives: BTreeSet<&str> = gt
        .entries
        .iter()
        .filter(|r| r.input != "*")
        .filter(|r| matches!(r.expected_grade, ExpectedGrade::Negative(_)))
        .map(|r| r.input.as_str())
        .collect();
    let declared_negatives = set(find("negative_shapes"));
    assert_eq!(
        committed_negatives, declared_negatives,
        "negative_shapes finding must equal the negative-graded \
         non-wildcard rows"
    );
}
