//! Demon-stack litmus — the pairwise_separation rule's fixture-tier
//! golden verdict (judgement layer §8 acceptance: "demon stack,
//! contact pile = one episode, worst pair, bounded roster,
//! soak-discarded fly-bys").
//!
//! Plain `cargo test` compiles this file's target with the
//! `playtest` feature via the self-dev-dependency and runs the
//! battery against the game binary. Assertions:
//!
//! 1. RED side — the planted contact pile is caught: the report
//!    status is FAIL and carries EXACTLY ONE finding, the
//!    `pairwise_separation` episode — nothing else, so a future
//!    refactor making any OTHER oracle false-positive on this
//!    deterministic scene fails the litmus, not silently rides
//!    along. The finding's `_meta.pairwise_episode` evidence must
//!    show all C(6,2) = 15 pairs, roster UNtruncated (the §8
//!    "bounded roster" clause exercised in its non-truncating
//!    direction), first violating frame early (the spiral tightens
//!    well before the doc'd frame 100), and the episode spanning
//!    far past the 2.0s soak.
//! 2. SILENT side — the fly-by control: the same world with a soak
//!    window sitting JUST ABOVE the episode's span — a boundary
//!    discard (span 19.43s < 20.0s window, ~3% margin), so a units
//!    or arithmetic bug that inflates OR deflates the effective
//!    soak flips the verdict. The run must report PASS and zero
//!    spatial findings: the ε doctrine refuses to judge sub-soak
//!    episodes rather than false-positive on them.
//!
//! Lose the RED finding and the rule regressed (or the fixture
//! healed itself); gain any other finding and an oracle misfired on
//! a deterministic scene whose only planted defect is the pile.

/// The litmus scenario: drive the game 20 seconds (TPS 60), judge
/// the demon pile with a 2.0s soak. The spiral tightens into
/// sustained coincidence at frame 33 (adjacent-ring pairs crossing
/// the 0.5 ε), so the episode soaks with ~10x margin. Liveness is
/// unaffected (world keeps changing).
const PILE_SCENARIO: &str = r#"{
  "bot": { "type": "synthetic_keyboard", "passive": true },
  "tps": 60,
  "duration_s": 20.0,
  "liveness": {
    "progress": { "mode": "off" }
  },
  "invariants": [{
    "name": "demons_keep_personal_space",
    "rule": "pairwise_separation",
    "target_selector": { "with_component": { "name": "Demon" } },
    "min_distance": 0.5,
    "window_s": 2.0
  }]
}"#;

/// The fly-by control: identical game, identical contacts, but the
/// soak window sits just above the episode's 19.43s span (20.0s)
/// — a boundary discard. Any soak-arithmetic drift in EITHER
/// direction (span under-measured, window over-applied, units
/// confusion) flips this verdict, so the control discriminates
/// the soak's correctness, not just its existence. Same world,
/// same contacts — the ε doctrine must stay silent.
const FLY_BY_CONTROL: &str = r#"{
  "bot": { "type": "synthetic_keyboard", "passive": true },
  "tps": 60,
  "duration_s": 20.0,
  "liveness": {
    "progress": { "mode": "off" }
  },
  "invariants": [{
    "name": "demons_keep_personal_space",
    "rule": "pairwise_separation",
    "target_selector": { "with_component": { "name": "Demon" } },
    "min_distance": 0.5,
    "window_s": 20.0
  }]
}"#;

/// Write a scenario constant to a fresh temp file (RAII-removed on
/// every exit path, panic included) and return its path — the
/// thetawave precedent for authored scenarios.
fn scenario_file(tag: &str, body: &str) -> (std::path::PathBuf, impl Drop) {
    let dir = std::env::temp_dir().join(format!(
        "bevy_playtest_demon_stack_{}_{}_{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("temp scenario dir creatable");
    let path = dir.join(format!("{tag}.json"));
    std::fs::write(&path, body).expect("scenario file writable");
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    (path, Cleanup(dir))
}

#[cfg(feature = "playtest")]
#[test]
fn pile_is_one_judged_episode_with_roster_evidence() {
    let (scenario, _cleanup) = scenario_file("pile", PILE_SCENARIO);
    let report = bevy_playtest::battery::run_with_config(&bevy_playtest::battery::BatteryConfig {
        seed_override: None,
        binary: env!("CARGO_BIN_EXE_fixture_demon_stack").to_string(),
        scenario: scenario.to_string_lossy().into_owned(),
        timeout_secs: 300,
    })
    .expect("litmus run completes and reports");

    // The spatial finding is THE thing that fails the run: a crash
    // or early-exit never satisfies a litmus (a crash report's
    // findings are drained, not adjudicated).
    assert_eq!(
        report.status,
        bevy_playtest::enums::PlaytestStatus::Fail,
        "the planted pile must fail the run — status {:?} with violations {:?}",
        report.status,
        report
            .violations
            .iter()
            .map(|v| (v.rule.clone(), v.target.clone()))
            .collect::<Vec<_>>()
    );

    // The golden verdict is a COMPLEMENT pin too: exactly one
    // finding, and it is the pairwise one. A future refactor that
    // makes any other oracle fire on this deterministic scene (the
    // world never freezes, nothing sits out of bounds, the camera
    // exists) fails here instead of silently riding along.
    assert_eq!(
        report.violations.len(),
        1,
        "the contact pile must be the ONLY finding: {:?}",
        report
            .violations
            .iter()
            .map(|v| (v.rule.clone(), v.target.clone()))
            .collect::<Vec<_>>()
    );
    let finding = &report.violations[0];
    assert_eq!(
        finding.rule, "pairwise_separation",
        "the single finding must be the spatial one"
    );
    assert!(
        finding.detail.contains("of each other"),
        "worst-pair detail must name the coincident pair: {}",
        finding.detail
    );
    assert!(
        finding.detail.contains("not a fly-by"),
        "sustained-overlap doctrine must be stated: {}",
        finding.detail
    );

    // The evidence: `_meta.pairwise_episode` rides the finding out
    // of the child process. Six demons is exactly C(6,2) = 15
    // distinct pairs — pin the count exactly (a hallucinated 16th
    // pair or a dropped one is a regression), pin the roster
    // UNtruncated (§8's "bounded roster" in its non-truncating
    // direction — a lowered cap silently subsampling is a
    // regression), and pin the timeline: the spiral crosses the ε
    // early (well before the doc's frame-100 claim) and sustains
    // far past the 2.0s soak (frames 33–1199 = 19.43s of frame
    // clock, ~10x margin).
    let meta = finding
        .meta
        .as_ref()
        .and_then(|m| m.pairwise_episode.as_ref())
        .expect("finding carries pairwise_episode evidence");
    assert_eq!(
        meta.sampled_pairs, 15,
        "all six demons pile: exactly C(6,2) = 15 distinct pairs"
    );
    assert_eq!(meta.roster.len(), 15);
    assert!(
        !meta.roster_truncated,
        "15 pairs fit the roster cap — truncation here means the \
         cap regressed below the fixture's needs"
    );
    assert!(
        meta.roster.iter().all(|r| r.worst_distance < 0.5),
        "every rostered pair violated the 0.5 min_distance: {:?}",
        meta.roster
    );
    assert!(
        finding.first_frame <= 60,
        "the spiral must cross the ε early (doc: well before frame \
         100); first violating frame was {}",
        finding.first_frame
    );
    assert!(
        finding.last_frame >= 1100,
        "the episode must sustain to the end of the run (soak \
         margin); last frame was {}",
        finding.last_frame
    );
}

#[cfg(feature = "playtest")]
#[test]
fn fly_by_control_stays_silent() {
    let (scenario, _cleanup) = scenario_file("flyby", FLY_BY_CONTROL);
    let report = bevy_playtest::battery::run_with_config(&bevy_playtest::battery::BatteryConfig {
        seed_override: None,
        binary: env!("CARGO_BIN_EXE_fixture_demon_stack").to_string(),
        scenario: scenario.to_string_lossy().into_owned(),
        timeout_secs: 300,
    })
    .expect("fly-by control run completes and reports");

    // A PASS status, not merely "no pairwise findings": a crashed
    // child produces zero spatial findings too, and crash-silence
    // must never satisfy a negative control.
    assert_eq!(
        report.status,
        bevy_playtest::enums::PlaytestStatus::Pass,
        "the sub-soak run must be a genuine clean pass — status {:?} \
         with violations {:?}",
        report.status,
        report
            .violations
            .iter()
            .map(|v| (v.rule.clone(), v.target.clone()))
            .collect::<Vec<_>>()
    );
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.rule == "pairwise_separation"),
        "a soak window just above the episode span must yield zero \
         pairwise findings — the ε doctrine refuses to judge; \
         violations: {:?}",
        report
            .violations
            .iter()
            .map(|v| (v.rule.clone(), v.target.clone()))
            .collect::<Vec<_>>()
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
