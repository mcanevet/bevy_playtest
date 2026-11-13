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
//! - the mouse matrix (additive MVP): cell probes and the doubled
//!   mouse control really reach a pointer-driven game
//!   (`takeover_demo_picking`) through the wrapped-event path, and
//!   a click the game binds diverges from the control trace,
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

/// The runaway child: never exits on its own, so the watchdog
/// premise ("still running at deadline") holds on every machine,
/// independent of CPU speed.
fn hang_binary() -> std::path::PathBuf {
    let bin = std::path::PathBuf::from(env!("CARGO_BIN_EXE_takeover_demo_hang"));
    assert!(bin.is_file(), "hang binary missing: {bin:?}");
    bin
}

/// A keyboard-focused plan with the mouse matrix switched OFF —
/// the sanctioned `0 × 0` slim mode for tests that pin the keyboard
/// layer. Passing `keys` narrows the keyboard candidates the same
/// way the tests used to narrow the whole plan.
fn keyboard_only(keys: &[&str]) -> ProbePlan {
    ProbePlan {
        candidate_keys: keys.iter().map(|k| k.to_string()).collect(),
        mouse_grid_cols: 0,
        mouse_grid_rows: 0,
        ..ProbePlan::default()
    }
}

/// The pointer-driven demo game: the world moves ONLY after a UI
/// `PointerClick` lands on its fullscreen node (bevy_picking reads
/// wrapped window events — the delivery path mouse cell probes must
/// ride). Any in-window click position hits the node, so every cell
/// of any grid is a bound click for this game.
fn picking_binary() -> std::path::PathBuf {
    let bin = std::path::PathBuf::from(env!("CARGO_BIN_EXE_takeover_demo_picking"));
    assert!(bin.is_file(), "picking binary missing: {bin:?}");
    bin
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
    /// (cell id, report)
    mouse_cells: Vec<(String, PlaytestReport)>,
    /// Mouse control copies, in input order (pair-wise: a/b).
    mouse_controls: Vec<PlaytestReport>,
}

fn split_kinds(results: &Results) -> SplitKinds {
    use bevy_playtest::discovery::probe_pool::ProbeJobKind;
    let mut probes = vec![];
    let mut controls = vec![];
    let mut mouse_cells = vec![];
    let mut mouse_controls = vec![];
    for r in results {
        let outcome = r.as_ref().expect("job must succeed");
        match &outcome.kind {
            ProbeJobKind::Probe { key, point } => {
                probes.push((key.clone(), point.label, outcome.report.clone()));
            }
            ProbeJobKind::Control { point } => {
                controls.push((point.label, outcome.report.clone()));
            }
            ProbeJobKind::MouseCell { cell, .. } => {
                mouse_cells.push((cell.id(), outcome.report.clone()));
            }
            ProbeJobKind::MouseControl { .. } => {
                mouse_controls.push(outcome.report.clone());
            }
        }
    }
    SplitKinds {
        probes,
        controls,
        mouse_cells,
        mouse_controls,
    }
}

#[test]
fn pool_matrix_probes_bound_key_and_gates_determinism() {
    let _slot = common::run_slot();
    // Shrink the matrix to the two keys that matter (bound Digit1,
    // inert KeyQ) to keep this test fast: 2 keys × 2 points = 4
    // probes + 4 controls (2 points × 2 copies). The mouse grid is
    // OFF (0×0) — this test pins the keyboard layer; the picking
    // game below pins the mouse matrix.
    let plan = keyboard_only(&["Digit1", "KeyQ"]);
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
    // either run). Lookup by POINT LABEL from the split controls —
    // deliberately NOT positional (`controls[i*2]`): the pairing
    // copy a/b of each point must never depend on `build_job_list`
    // emitting controls in a particular order.
    let control_by_point = |label: &str| -> &PlaytestReport {
        controls
            .iter()
            .find(|(l, _)| *l == label)
            .map(|(_, r)| r)
            .expect("a control run per probe point")
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
fn mouse_matrix_reaches_pointer_games_and_gates_determinism() {
    let _slot = common::run_slot();
    // The mouse matrix's end-to-end pin, against the PICKING game:
    // every click position hits its fullscreen node (any cell is
    // bound), so cell probes and the mouse control all deliver real
    // clicks through the wrapped-event path — and the control pair
    // must be byte-identical (the mouse determinism gate).
    let plan = ProbePlan {
        candidate_keys: vec!["Digit1".into()], // one key: keeps the matrix slim
        mouse_grid_cols: 3,
        mouse_grid_rows: 2,
        ..ProbePlan::default()
    };
    let jobs = build_job_list(&plan).expect("job list");
    // 1 key × 2 points = 2 probes + 2 points × 2 = 4 key controls,
    // then 3×2 = 6 cells + 2 mouse controls.
    assert_eq!(jobs.len(), 2 + 4 + 6 + 2);

    let config = ProbePoolConfig::new(picking_binary());
    let results = run_jobs(&config, &jobs);
    assert_eq!(results.len(), jobs.len());
    for r in &results {
        r.as_ref().expect("every mouse job must produce a report");
    }

    let split = split_kinds(&results);
    // Every cell ran, once, in row-major order.
    let cell_ids: Vec<&str> = split
        .mouse_cells
        .iter()
        .map(|(id, _)| id.as_str())
        .collect();
    assert_eq!(
        cell_ids,
        vec!["r0c0", "r0c1", "r0c2", "r1c0", "r1c1", "r1c2"],
        "cells arrive row-major with stable ids"
    );
    // Two mouse controls, byte-identical: the determinism gate.
    assert_eq!(split.mouse_controls.len(), 2);
    assert_eq!(
        digest_series(&split.mouse_controls[0]),
        digest_series(&split.mouse_controls[1]),
        "mouse double-control determinism gate FAILED"
    );

    // Every cell probe on this game is BOUND (fullscreen node): it
    // must diverge from the mouse control trace. That is the
    // visible-at-the-pool-layer effect of a click reaching
    // bevy_picking through the wrapped-event path.
    let control_series = digest_series(&split.mouse_controls[0]);
    for (id, report) in &split.mouse_cells {
        assert_ne!(
            digest_series(report),
            control_series,
            "cell {id} must diverge from the mouse control on the picking game"
        );
    }
}

#[test]
fn disabled_mouse_grid_contributes_no_jobs() {
    // No subprocesses needed: `build_job_list` is a pure function of
    // the plan (its subprocess plumbing is exercised by every other
    // test here); pin the 0×0 off-switch at the job-list layer.
    use bevy_playtest::discovery::probe_pool::ProbeJobKind;
    let jobs = build_job_list(&keyboard_only(&["Digit1"])).expect("job list");
    assert_eq!(
        jobs.len(),
        2 + 4,
        "keyboard matrix only: 1 key × 2 points + 2 points × 2 controls"
    );
    assert!(
        jobs.iter().all(|j| !matches!(
            j.kind,
            ProbeJobKind::MouseCell { .. } | ProbeJobKind::MouseControl { .. }
        )),
        "a disabled grid must contribute zero mouse jobs"
    );

    // Default grid: 5×4 = 20 cells plus the doubled mouse control,
    // all appended AFTER the keyboard matrix (stable staging
    // indices for the keyboard jobs — see build_job_list).
    let jobs = build_job_list(&ProbePlan::default()).expect("job list");
    let mouse_count =
        |pred: &dyn Fn(&ProbeJobKind) -> bool| jobs.iter().filter(|j| pred(&j.kind)).count();
    assert_eq!(
        mouse_count(&|k| matches!(k, ProbeJobKind::MouseCell { .. })),
        20
    );
    assert_eq!(
        mouse_count(&|k| matches!(k, ProbeJobKind::MouseControl { .. })),
        2,
        "mouse control is doubled (determinism gate)"
    );
    let first_mouse = jobs
        .iter()
        .position(|j| {
            matches!(
                j.kind,
                ProbeJobKind::MouseCell { .. } | ProbeJobKind::MouseControl { .. }
            )
        })
        .expect("mouse jobs exist");
    assert!(
        jobs[..first_mouse].iter().all(|j| !matches!(
            j.kind,
            ProbeJobKind::MouseCell { .. } | ProbeJobKind::MouseControl { .. }
        )),
        "mouse jobs must be appended after the keyboard matrix"
    );
}

#[test]
fn missing_binary_is_an_infrastructure_error_not_silence() {
    let _slot = common::run_slot();
    // Refusal over silence: a run that cannot produce a report must
    // surface as ProbeError::NoReport, never as a "no effect" result.
    let plan = keyboard_only(&["Digit1"]);
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
fn runaway_child_is_killed_by_the_wall_clock_watchdog() {
    let _slot = common::run_slot();
    // The timeout path was the ONE untested pool branch: deadline
    // loop, kill_process, and the "timed out" refusal. A regression
    // in the watchdog (or in the shared battery::kill_process it
    // calls — whose Windows tree-kill ordering this pins on EVERY
    // platform, since the call itself is platform-neutral) would
    // otherwise hang CI on the first real stuck game instead of
    // failing a test.
    //
    // The child is takeover_demo_hang, which NEVER exits on its
    // own: the "still running at deadline" premise is guaranteed on
    // every machine, independent of CPU speed. (The simulated-time
    // demos would NOT give that guarantee: the headless driver
    // advances time in a tight CPU loop, so wall-clock runtime
    // scales with machine speed — an observed 30s of simulated time
    // took ~8.6s wall on an M-series Mac, and a fast-enough runner
    // would finish before the 1s deadline.)
    let plan = keyboard_only(&["Digit1"]);
    let jobs = build_job_list(&plan).expect("job list");
    assert!(
        !jobs.is_empty(),
        "test premise: at least one job must exist"
    );

    let mut config = ProbePoolConfig::new(hang_binary());
    config.timeout_secs = 1;
    let started = std::time::Instant::now();
    let results = run_jobs(&config, &jobs);
    // Bounded: the watchdog killed the children at the 1s deadline
    // and (crucially) reaped them, or this test would hang —
    // a leak would leave run_jobs blocked in try_wait forever.
    assert!(
        started.elapsed() < std::time::Duration::from_secs(15),
        "watchdog must kill+reap in bounded time, took {:?}",
        started.elapsed()
    );
    for r in &results {
        match r {
            Err(ProbeError::NoReport { cause, .. }) => assert!(
                cause.contains("timed out after 1s"),
                "must name the watchdog: {cause}"
            ),
            other => panic!("watchdog kill must yield the timeout refusal, got: {other:?}"),
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

    // Second pool, SAME workspace and job slot: a PORTABLE no-op
    // child — the dedicated `takeover_demo_noop` demo binary, which
    // runs a plugin-less app, exits 0, and writes NO report file (a
    // PATH-resolved `true` would restrict this gate to unix; the
    // stale-report refusal is platform-neutral code and must be
    // pinned on every platform). The stale file from the first pool
    // is the only thing the read could succeed on. (An unspawnable
    // path would NOT do: it fails at spawn, before the read,
    // masking the very hole this test pins. The `cause` assertion
    // below proves the child RAN — a spawn failure has a different
    // cause and must never satisfy this test.)
    let noop = std::path::PathBuf::from(env!("CARGO_BIN_EXE_takeover_demo_noop"));
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
    let plan = keyboard_only(&["Digit1"]);
    let jobs = build_job_list(&plan).expect("job list");

    let config = ProbePoolConfig::new(demo_binary());
    let results = run_jobs(&config, &jobs);
    for r in &results {
        r.as_ref()
            .expect("children must strip stray playtest env vars and report normally");
    }
    std::env::remove_var("BEVY_PLAYTEST_PROBE_SUITE_STRAY");
}

#[test]
fn probe_children_get_their_own_cwd_and_home() {
    let _slot = common::run_slot();
    // Isolation regression: probe children used to inherit the
    // PARENT's working directory, so a game writing anything
    // `./`-relative (or resolving paths through $HOME) scribbled
    // into wherever `cargo test` ran — the repo tree in CI — and
    // sibling jobs shared one collision-prone cwd. The pool must
    // give every job its own throwaway directory (under the RAII
    // workspace, so no artifacts survive the run) and repoint the
    // child's HOME there.
    //
    // The child is the dedicated `takeover_demo_cwd_write` binary:
    // it writes a cwd-relative marker and a HOME-resolved marker,
    // then exits. It is not a takeover child (no report), and the
    // test ASSERTS the resulting `NoReport` — with "no report
    // file" as the cause — as positive proof the child really ran
    // to completion rather than failing at spawn.
    let plan = ProbePlan::default();
    let points = probe_points(PROBE_DURATION_S, PROBE_TPS);
    let job = bevy_playtest::discovery::probe_pool::ProbeJob {
        scenario: plan.control_scenario(points[0]).expect("scenario"),
        kind: bevy_playtest::discovery::probe_pool::ProbeJobKind::Control { point: points[0] },
        expected_last_frame: plan.expected_last_frame(),
        export_scan: false,
    };
    // Reused workspace so the per-job cwd survives this pool run
    // for inspection before the RAII drop (the run_jobs wrapper
    // would have destroyed it by the time the assertions run).
    let workspace = bevy_playtest::discovery::probe_pool::ProbeWorkspace::new().expect("workspace");
    let writer = std::path::PathBuf::from(env!("CARGO_BIN_EXE_takeover_demo_cwd_write"));
    let config = ProbePoolConfig::new(writer);

    let parent_cwd = std::env::current_dir().expect("parent cwd");
    let results = run_jobs_in(&config, &[job], &workspace);
    // The child exits 0 after writing its markers, but writes NO
    // report — that exact refusal is the proof it executed.
    match &results[0] {
        Err(ProbeError::NoReport { scenario: _, cause }) => assert!(
            cause.contains("no report file"),
            "child must have RUN (exit 0, report never written), got: {cause:?}"
        ),
        other => panic!("marker-writing child must yield the no-report refusal, got: {other:?}"),
    }

    // Both markers land INSIDE the per-job cwd: the cwd-relative
    // write because that IS the child's cwd, and the HOME-resolved
    // write because the pool repointed HOME at the same directory.
    let child_cwd = workspace.child_cwd_path(0);
    assert!(
        child_cwd.join("cwd_marker.txt").is_file(),
        "child's ./-relative write must land in its per-job cwd, not the parent's"
    );
    assert!(
        child_cwd.join("home_marker.txt").is_file(),
        "child's HOME-resolved write must land in the isolated per-job HOME"
    );

    // Read-preservation: the per-job cwd's top level is a SYMLINK
    // TREE into the parent's cwd (same treatment as the battery —
    // see `crate::battery::populate_symlink_tree`), so raw
    // `./`-relative reads keep resolving. `cargo test` runs from
    // the crate root, so `./Cargo.toml` exists in the parent cwd
    // and must be readable through the per-job dir; and the entry
    // must be a SYMLINK (a copy would satisfy the read but blow
    // the per-job dirs up to a full tree each, defeating the
    // throwaway-cwd point).
    assert!(
        child_cwd.join("Cargo.toml").is_file(),
        "read-isolation regression: ./Cargo.toml must resolve through the \
         per-job symlink tree"
    );
    assert!(
        child_cwd
            .join("Cargo.toml")
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink()),
        "the per-job tree must SYMLINK into the parent cwd, not copy it"
    );

    // Nothing leaked into the parent's working directory, and the
    // parent's cwd is untouched (the child's isolation is
    // one-directional).
    assert!(
        !parent_cwd.join("cwd_marker.txt").exists(),
        "isolation failed: the child wrote into the PARENT's cwd"
    );
    assert!(
        std::env::current_dir().is_ok_and(|d| d == parent_cwd),
        "the parent's cwd must be unchanged by the pool run"
    );
}
