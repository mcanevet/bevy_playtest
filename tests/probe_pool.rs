//! Discovery probe-pool integration tests: the probe matrix executed
//! as REAL takeover subprocesses against `takeover_demo_input` (the
//! input-delivery demo game — world motion only on
//! `just_pressed(Digit1)`), via the worker pool.
//!
//! These tests pin the pool's contracts end-to-end:
//! - normalized control/probe pairs really reach the game through
//!   the full plugin path (env contract, scenario file loading,
//!   report writing),
//! - the double-control determinism gate: two identical control
//!   runs yield identical digest series,
//! - a probe key the game binds (Digit1) diverges from the control
//!   trace; an inert probe key (KeyQ) does not (at this pool layer
//!   that's visible as IDENTICAL digest series to the control — the
//!   divergence classification itself is Phase 3's `diff.rs`),
//! - no-report runs surface as infrastructure errors (refusal over
//!   silence), never as silent "no effect".

#![cfg(feature = "takeover")]

// Shared test harness (run-slot serialization): the env-sanitization
// test mutates PROCESS-GLOBAL env vars, which races the other tests'
// child spawns (a stray var landing after a worker's strip loop but
// before exec makes the child legitimately exit 3 with no report).
// The run slot serializes the suite — same convention as
// takeover_e2e.
#[path = "common/mod.rs"]
mod common;

use bevy_playtest::discovery::probe_pool::{
    build_job_list, run_jobs, run_jobs_in, ProbeError, ProbeOutcome, ProbePoolConfig,
};
use bevy_playtest::discovery::probe_scenarios::{
    probe_points, ProbePlan, PROBE_DURATION_S, PROBE_TPS,
};
use bevy_playtest::state::PlaytestReport;

type Results = Vec<Result<ProbeOutcome, ProbeError>>;

fn demo_binary() -> std::path::PathBuf {
    let bin = std::path::PathBuf::from(env!("CARGO_BIN_EXE_takeover_demo_input"));
    assert!(bin.is_file(), "demo binary missing: {bin:?}");
    bin
}

/// Locate a no-op binary via PATH (portable: resolves `/usr/bin/true`
/// on macOS, `/bin/true` on Linux). Returns None when absent — the
/// caller decides what that means.
fn which_true() -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join("true");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Digest series of a report, as plain (frame, hash) pairs.
fn digest_series(report: &PlaytestReport) -> Vec<(u64, u64)> {
    report
        .state_digest
        .iter()
        .map(|d| (d.frame, d.hash))
        .collect()
}

/// Reports per job kind, in input order.
struct SplitKinds {
    /// (key, point label, report)
    probes: Vec<(String, &'static str, PlaytestReport)>,
    /// (point label, report)
    controls: Vec<(&'static str, PlaytestReport)>,
}

fn split_kinds(results: &Results) -> SplitKinds {
    use bevy_playtest::discovery::probe_pool::ProbeJobKind;
    let mut probes = vec![];
    let mut controls = vec![];
    for r in results {
        let outcome = r.as_ref().expect("job must succeed");
        match &outcome.kind {
            ProbeJobKind::Probe { key, point } => {
                probes.push((key.clone(), point.label, outcome.report.clone()));
            }
            ProbeJobKind::Control { point } => {
                controls.push((point.label, outcome.report.clone()));
            }
        }
    }
    SplitKinds { probes, controls }
}

#[test]
fn pool_matrix_probes_bound_key_and_gates_determinism() {
    let _slot = common::run_slot();
    // Shrink the matrix to the two keys that matter (bound Digit1,
    // inert KeyQ) to keep this test fast: 2 keys × 2 points = 4
    // probes + 4 controls (2 points × 2 copies).
    let plan = ProbePlan {
        candidate_keys: vec!["Digit1".into(), "KeyQ".into()],
        ..ProbePlan::default()
    };
    assert_eq!(plan.candidate_keys.len(), 2);

    let jobs = build_job_list(&plan).expect("job list");
    // 4 probes + 2 control points × 2 copies.
    assert_eq!(jobs.len(), 4 + 4);

    let config = ProbePoolConfig::new(demo_binary());
    let results = run_jobs(&config, &jobs);
    assert_eq!(results.len(), jobs.len());
    for r in &results {
        r.as_ref().expect("every job produces a report");
    }

    let (probes, controls) = {
        let split = split_kinds(&results);
        (split.probes, split.controls)
    };
    assert_eq!(probes.len(), 4);
    assert_eq!(controls.len(), 4);

    // Determinism gate: control pairs (same point, copies a and b)
    // must have IDENTICAL digest series.
    let points = probe_points(PROBE_DURATION_S, PROBE_TPS);
    for pair_idx in 0..points.len() {
        let (_, a) = &controls[pair_idx * 2];
        let (_, b) = &controls[pair_idx * 2 + 1];
        assert_eq!(
            digest_series(a),
            digest_series(b),
            "double-control determinism gate FAILED: identical control \
             runs produced different digest traces (game is nondeterministic \
             or the pool corrupted a run)"
        );
        assert!(
            !a.state_digest.is_empty(),
            "control runs must record digests (record_digest is on)"
        );
    }

    // Effect visibility at the POOL layer (Phase 3's diff classifies
    // properly): the bound key's trace must differ from the control;
    // the inert key must match the control (nothing happens in
    // either run).
    let control_by_point = |label: &str| -> &PlaytestReport {
        let idx = points.iter().position(|p| p.label == label).unwrap();
        &controls[idx * 2].1
    };

    // Digit1 (bound): differs from control at the readiness point.
    let digit1_readiness = probes
        .iter()
        .find(|(k, label, _)| k == "Digit1" && *label == "readiness")
        .map(|(_, _, r)| digest_series(r))
        .expect("Digit1 readiness probe");
    let control_readiness = digest_series(control_by_point("readiness"));
    assert_ne!(
        digit1_readiness, control_readiness,
        "bound key Digit1 must diverge from the control trace"
    );

    // KeyQ (inert): identical to control at the readiness point.
    let keyq_readiness = probes
        .iter()
        .find(|(k, label, _)| k == "KeyQ" && *label == "readiness")
        .map(|(_, _, r)| digest_series(r))
        .expect("KeyQ readiness probe");
    assert_eq!(
        keyq_readiness, control_readiness,
        "inert key KeyQ must NOT perturb the trace (matches control)"
    );
}

#[test]
fn missing_binary_is_an_infrastructure_error_not_silence() {
    let _slot = common::run_slot();
    // Refusal over silence: a run that cannot produce a report must
    // surface as ProbeError::NoReport, never as a "no effect" result.
    let plan = ProbePlan {
        candidate_keys: vec!["Digit1".into()],
        ..ProbePlan::default()
    };
    let jobs = build_job_list(&plan).expect("job list");

    let mut config = ProbePoolConfig::new("/nonexistent/binary/that/does/not/exist");
    config.pool_size = 2;
    let results = run_jobs(&config, &jobs);
    assert!(!results.is_empty());
    for r in &results {
        match r {
            Err(ProbeError::NoReport { .. }) => {}
            other => panic!("unspawnable binary must yield NoReport, got: {other:?}"),
        }
    }
}

#[test]
fn trace_less_report_is_an_infrastructure_error_not_vacuous_inert() {
    let _slot = common::run_slot();
    // Refusal over silence, digest edition: a game that runs but
    // records NO state digests (record_digest off, or a build that
    // lost digest support) produces reports nothing can compare. If
    // the pool accepted them, every probe would "match" its control
    // vacuously and an all-inert false surface would be aggregated
    // and cached. The pool must refuse the run instead.
    let plan = ProbePlan::default();
    let mut scenario = plan
        .control_scenario(probe_points(PROBE_DURATION_S, PROBE_TPS)[0])
        .expect("scenario");
    scenario.record_digest = false; // Legitimate report, empty trace.
    let job = bevy_playtest::discovery::probe_pool::ProbeJob {
        scenario,
        kind: bevy_playtest::discovery::probe_pool::ProbeJobKind::Control {
            point: probe_points(PROBE_DURATION_S, PROBE_TPS)[0],
        },
        expected_last_frame: plan.expected_last_frame(),
        export_scan: false,
    };

    let config = ProbePoolConfig::new(demo_binary());
    let results = run_jobs(&config, &[job]);
    match &results[0] {
        Err(ProbeError::UnusableReport { .. }) => {}
        other => panic!("trace-less report must be refused as UnusableReport, got: {other:?}"),
    }
}

#[test]
fn pre_tap_death_is_an_infrastructure_error_not_vacuous_inert() {
    let _slot = common::run_slot();
    // Refusal over silence, tap-window edition: `diff_trace` compares
    // from the tap point onward, so a run whose trace ends BEFORE
    // the job's tap frame (the game stopped ticking before the
    // probed key press could happen — early exit, hang-then-kill)
    // would read as inert: a vacuous negative caused by never
    // reaching the press. The pool must refuse it as UnusableReport,
    // not record a false inert key.
    //
    // Deterministic repro through a REAL child: the default matrix
    // taps at frame 60, but this scenario's duration is shrunk to
    // 0.5s (~30 ticks at 60 tps) after the plan built it — the child
    // runs a perfectly healthy, COMPLETE run whose trace simply ends
    // 30 frames short of the job's tap point.
    let plan = ProbePlan::default();
    let points = probe_points(PROBE_DURATION_S, PROBE_TPS);
    let mut scenario = plan.control_scenario(points[0]).expect("scenario");
    scenario.duration_s = 0.5; // ~30 ticks << tap frame ~66.
    let job = bevy_playtest::discovery::probe_pool::ProbeJob {
        scenario,
        kind: bevy_playtest::discovery::probe_pool::ProbeJobKind::Control { point: points[0] },
        expected_last_frame: plan.expected_last_frame(),
        export_scan: false,
    };

    let config = ProbePoolConfig::new(demo_binary());
    let results = run_jobs(&config, &[job]);
    match &results[0] {
        Err(ProbeError::UnusableReport { .. }) => {}
        other => panic!(
            "report with no post-tap frames must be refused as UnusableReport, got: {other:?}"
        ),
    }
}

#[test]
fn trace_ending_exactly_at_the_tap_is_refused_too() {
    let _slot = common::run_slot();
    // Strictly-AFTER, not at-the-tap: the bot writes the keyboard
    // event during `Update`, but Bevy's `keyboard_input_system`
    // consumes it in the NEXT `PreUpdate` — a run whose last digest
    // is recorded AT the tap frame never delivered the key to the
    // game, so its "no effect" verdict is vacuous and must be
    // refused. An at-or-after check would accept this report.
    //
    // Deterministic repro: the default readiness tap sits at frame
    // 60; this scenario's duration is 1.01s → ceil(1.01*60) = 61
    // ticks → digests at frames 0..=60: the last digest lands exactly
    // AT the tap frame, none after it.
    let plan = ProbePlan::default();
    let points = probe_points(PROBE_DURATION_S, PROBE_TPS);
    let mut scenario = plan.control_scenario(points[0]).expect("scenario");
    scenario.duration_s = 1.01; // ceil(1.01*60)=61 ticks; last digest = frame 60.
    let job = bevy_playtest::discovery::probe_pool::ProbeJob {
        scenario,
        kind: bevy_playtest::discovery::probe_pool::ProbeJobKind::Control { point: points[0] },
        expected_last_frame: plan.expected_last_frame(),
        export_scan: false,
    };

    let config = ProbePoolConfig::new(demo_binary());
    let results = run_jobs(&config, &[job]);
    match &results[0] {
        Err(ProbeError::UnusableReport { .. }) => {}
        other => panic!(
            "trace ending AT the tap frame must be refused as UnusableReport, got: {other:?}"
        ),
    }
}

#[test]
fn post_tap_but_incomplete_trace_is_refused_too() {
    let _slot = common::run_slot();
    // COMPLETENESS gate (supersedes the old "a crash AFTER the tap
    // stays usable" carve-out): a run that dies shortly after the
    // tap — while post-tap frames EXIST — leaves a partial trace.
    // `diff_trace` zips only the common prefix, so against a
    // full-length control it would return `None`: a VACUOUS inert
    // that must never become a trusted negative or reach the cache.
    // The pool must refuse the report even though it clears the
    // post-tap gate.
    //
    // Deterministic repro: the readiness tap is frame 60; the
    // default plan expects a complete trace through frame
    // ceil(3.0*60)-1 = 179. Shrinking the SCENARIO's duration to
    // 1.1s (NOT the plan's) makes the child run a perfectly
    // healthy, complete-for-itself run whose 66 digests (frames
    // 0..=65) include five strictly after the tap — it passes the
    // post-tap gate — while covering barely a third of the
    // expected span.
    let plan = ProbePlan::default();
    let points = probe_points(PROBE_DURATION_S, PROBE_TPS);
    let mut scenario = plan.control_scenario(points[0]).expect("scenario");
    scenario.duration_s = 1.1; // ceil(1.1*60) = 66 ticks; last digest = 65.
    let expected = plan.expected_last_frame(); // snapshot: 179.
    assert!(
        expected > 65,
        "scenario must end short of the expected span"
    );
    let job = bevy_playtest::discovery::probe_pool::ProbeJob {
        scenario,
        kind: bevy_playtest::discovery::probe_pool::ProbeJobKind::Control { point: points[0] },
        expected_last_frame: expected,
        export_scan: false,
    };

    let config = ProbePoolConfig::new(demo_binary());
    let results = run_jobs(&config, &[job]);
    match &results[0] {
        Err(ProbeError::UnusableReport { .. }) => {}
        other => panic!(
            "post-tap but incomplete trace must be refused as UnusableReport, got: {other:?}"
        ),
    }
}

#[test]
fn healthy_report_stays_usable_after_the_refusals() {
    let _slot = common::run_slot();
    // Over-rejection guard, pinned end-to-end: the new usability
    // checks (trace covers the tap point; report path cleared before
    // spawn) must NOT refuse a healthy run — a full-length control
    // job at the default tap point must stay usable.
    let plan = ProbePlan::default();
    let points = probe_points(PROBE_DURATION_S, PROBE_TPS);
    let job = bevy_playtest::discovery::probe_pool::ProbeJob {
        scenario: plan.control_scenario(points[0]).expect("scenario"),
        kind: bevy_playtest::discovery::probe_pool::ProbeJobKind::Control { point: points[0] },
        expected_last_frame: plan.expected_last_frame(),
        export_scan: false,
    };
    let config = ProbePoolConfig::new(demo_binary());
    let results = run_jobs(&config, &[job]);
    assert!(
        results[0].is_ok(),
        "healthy control run must stay usable, got: {:?}",
        results[0]
    );
}

#[test]
fn scan_requesting_control_delivers_the_child_scan() {
    let _slot = common::run_slot();
    // Copilot r4466455064: the scan must actually RUN in the game
    // child and come back. This pins the full export path end-to-end:
    // the pool sets ENV_DISCOVER_SCAN on the child, `run_scenario`
    // captures the scan at harness-setup time (BEFORE bot/oracle
    // systems are registered, so harness writes never pollute it),
    // the takeover writes the captured sidecar, and the pool parses
    // it into the outcome.
    //
    // Plausibility assertion, not exact counts: `takeover_demo_input`
    // reads `ButtonInput<KeyCode>` (its motion system) and the stock
    // `keyboard_input_system` writes it, so the keyboard class MUST
    // show at least one reader — an empty scan here would mean the
    // export silently produced no data (the fail-open path the flow's
    // not-run note covers would then mask a real regression).
    let plan = ProbePlan::default();
    let points = probe_points(PROBE_DURATION_S, PROBE_TPS);
    let job = bevy_playtest::discovery::probe_pool::ProbeJob {
        scenario: plan.control_scenario(points[0]).expect("scenario"),
        kind: bevy_playtest::discovery::probe_pool::ProbeJobKind::Control { point: points[0] },
        expected_last_frame: plan.expected_last_frame(),
        export_scan: true,
    };
    let config = ProbePoolConfig::new(demo_binary());
    let results = run_jobs(&config, &[job]);
    let outcome = results[0]
        .as_ref()
        .expect("control run must succeed")
        .clone();
    let scan = outcome
        .scan
        .as_ref()
        .expect("scan-requesting control must deliver the child's scan sidecar");
    assert!(
        scan.reader_count(bevy_playtest::discovery::static_scan::DeviceClass::Keyboard) >= 1,
        "the demo reads ButtonInput<KeyCode>; keyboard readers must be >= 1, got {:?}",
        scan.readers
    );
}

#[test]
fn stale_scan_sidecar_does_not_leak_into_non_requesting_jobs() {
    let _slot = common::run_slot();
    // The scan sidecar's path is deterministic per (job_index,
    // scenario name), exactly like the report path. A workspace
    // reused across sequential pools holds the PREVIOUS export —
    // a job that did NOT request a scan must read `scan: None`, not
    // the stale sidecar from an earlier run (reader counts from
    // another run are wrong run-identity data). The parse must be
    // gated on the job's own `export_scan` request, not on the
    // file's existence.
    let plan = ProbePlan::default();
    let points = probe_points(PROBE_DURATION_S, PROBE_TPS);
    let scan_job = bevy_playtest::discovery::probe_pool::ProbeJob {
        scenario: plan.control_scenario(points[0]).expect("scenario"),
        kind: bevy_playtest::discovery::probe_pool::ProbeJobKind::Control { point: points[0] },
        expected_last_frame: plan.expected_last_frame(),
        export_scan: true,
    };
    let plain_job = bevy_playtest::discovery::probe_pool::ProbeJob {
        export_scan: false,
        ..scan_job.clone()
    };
    let workspace = bevy_playtest::discovery::probe_pool::ProbeWorkspace::new().expect("workspace");
    let config = ProbePoolConfig::new(demo_binary());
    // First pool: the scan export lands in slot 0 of this workspace.
    let first = run_jobs_in(&config, std::slice::from_ref(&scan_job), &workspace);
    first[0]
        .as_ref()
        .expect("setup run must succeed")
        .scan
        .as_ref()
        .expect("setup run must deliver the scan");

    // Second pool, SAME workspace and job slot, scan NOT requested:
    // the stale sidecar file is still on disk (nothing deleted it —
    // only scan-requesting jobs clear it), so this run's `scan: None`
    // can ONLY come from correct request gating, not from the file
    // being absent. Reading it here is the failure this test pins.
    let second = run_jobs_in(&config, std::slice::from_ref(&plain_job), &workspace);
    let outcome = second[0].as_ref().expect("second run must succeed");
    assert!(
        outcome.scan.is_none(),
        "a non-scan job must not read the stale sidecar: {:?}",
        outcome.scan
    );
}

#[test]
#[cfg(unix)] // Uses PATH-resolved `true`; a Windows port needs a
             // native no-op child before this gate can lift.
fn stale_report_cannot_masquerade_as_a_fresh_success() {
    let _slot = common::run_slot();
    // The report path is deterministic per (job_index, scenario
    // name). When a caller reuses one ProbeWorkspace across
    // sequential pools (the documented run_jobs_in pattern), a later
    // child that exits WITHOUT writing a report would otherwise
    // inherit the PREVIOUS job's report file — a stale report read
    // as a false successful probe. The pool must clear the report
    // path before spawning.
    let plan = ProbePlan::default();
    let points = probe_points(PROBE_DURATION_S, PROBE_TPS);
    let good_job = bevy_playtest::discovery::probe_pool::ProbeJob {
        scenario: plan.control_scenario(points[0]).expect("scenario"),
        kind: bevy_playtest::discovery::probe_pool::ProbeJobKind::Control { point: points[0] },
        expected_last_frame: plan.expected_last_frame(),
        export_scan: false,
    };
    let workspace = bevy_playtest::discovery::probe_pool::ProbeWorkspace::new().expect("workspace");
    let config = ProbePoolConfig::new(demo_binary());
    // First pool: the real game writes its report at slot 0.
    let first = run_jobs_in(&config, std::slice::from_ref(&good_job), &workspace);
    assert!(first[0].is_ok(), "setup run must succeed");

    // Second pool, SAME workspace and job slot: a no-op child found
    // via PATH (portable across unixes — no hard-coded `/bin/true`
    // assumption) that exits WITHOUT writing any report. The stale
    // file from the first pool is the only thing the read could
    // succeed on. (An unspawnable path would NOT do: it fails at
    // spawn, before the read, masking the very hole this test pins.
    // The `cause` assertion proves the child RAN — a spawn failure
    // has a different cause and must never satisfy this test.)
    let noop = which_true().expect("a `true` binary on PATH");
    let bad_config = ProbePoolConfig::new(noop);
    let second = run_jobs_in(&bad_config, &[good_job], &workspace);
    match &second[0] {
        Err(ProbeError::NoReport { cause, .. }) if cause.contains("no report file") => {}
        other => panic!("stale report must not be readable as a fresh success, got: {other:?}"),
    }
}

#[test]
fn stray_playtest_env_vars_are_sanitized_in_children() {
    let _slot = common::run_slot();
    // The takeover contract REJECTS unknown BEVY_PLAYTEST_* variables
    // (exit 3, no report). The pool must strip them from child envs —
    // a leaked variable in the test-runner's environment must not
    // turn every probe into an infrastructure error. Mutation-
    // verified: removing the strip loop in execute_job fails this
    // test.
    //
    // The set_var is process-global and std::env has no atomic
    // set-while-spawning guarantee: a var landing between a sibling
    // test's strip loop and exec makes THAT child legitimately exit 3.
    // The run slot serializes the suite, so no sibling spawns children
    // while the stray var is live.
    std::env::set_var("BEVY_PLAYTEST_PROBE_SUITE_STRAY", "1");
    let plan = ProbePlan {
        candidate_keys: vec!["Digit1".into()],
        ..ProbePlan::default()
    };
    let jobs = build_job_list(&plan).expect("job list");

    let config = ProbePoolConfig::new(demo_binary());
    let results = run_jobs(&config, &jobs);
    for r in &results {
        r.as_ref()
            .expect("children must strip stray playtest env vars and report normally");
    }
    std::env::remove_var("BEVY_PLAYTEST_PROBE_SUITE_STRAY");
}
