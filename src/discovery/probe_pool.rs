//! Subprocess probe pool (discovery Phase 2).
//!
//! Executes the probe-run matrix as TAKEOVER SUBPROCESSES — the game
//! binary is spawned once per scenario, exactly like
//! [`crate::battery`] does — but in parallel, bounded by a worker
//! pool (6–8 processes; the design doc's sizing: Bevy examples run
//! ~5s headless, so 8 parallel ≈ 40s wall-clock for the full matrix
//! versus 6–8 min serial, at ~50–150 MB per process).
//!
//! Semantics shared with the battery (deliberately):
//!
//! - **Env sanitization first**: every `BEVY_PLAYTEST_*` variable is
//!   stripped from the child env before setting the two contract
//!   variables — the takeover rejects unknown ones (strict env
//!   contract; a leaked `REPLAY_PASS_B` variable would turn every run
//!   into a pass-B child).
//! - **Child cwd/HOME isolation**: every job's child runs in its OWN
//!   throwaway directory (with `HOME` repointed at it) instead of
//!   inheriting the parent's — `./`-relative writes (auto-saves,
//!   logs, caches) land in the per-job dir, not the repo tree, and
//!   never collide between sibling runs. Raw `./`-relative READS
//!   still resolve: the per-job dir's top level is a symlink tree
//!   into the parent's cwd (the shared
//!   `crate::battery::populate_symlink_tree` helper), so a game
//!   reading `./assets/...` with plain `fs` calls sees the original
//!   tree. Asset-server loading was never affected (bevy resolves
//!   its asset root from env/exe-dir, never cwd).
//! - **Success = a report was written AND its trace extends PAST
//!   the tap point.** Exit code is annotation (findings live in the
//!   report; the discovery layer treats game findings and passes
//!   alike as "we learned this run's digests"). A missing report, a
//!   parse failure, or a parsed report whose `state_digest` is empty
//!   OR stops at-or-before the job's tap point IS an infrastructure
//!   error ([`ProbeError`]), never a "no effect" verdict (refusal
//!   over silence) — a binary that (accidentally or not) skips
//!   `record_digest`, or a run that stops before the press reached
//!   the game (the bot writes the event in `Update`; Bevy consumes
//!   it in the next `PreUpdate`), produces reports that would read
//!   EVERY key as inert, and that false surface must be rejected
//!   before it can be aggregated or cached.
//!
//!   The same refusal now covers INCOMPLETE traces: a report must
//!   carry digests all the way to the run's expected final frame
//!   (`ceil(duration_s * tps) - 1`, snapshotted into the job at
//!   build time). A crash or truncation mid-run leaves a partial
//!   trace, and a partial trace can only vacuously "match" its
//!   control over the frames both recorded — the classic
//!   trusted-false-negative hole. Only a COMPLETE run supports an
//!   inert verdict; everything shorter is an infrastructure
//!   error.
//!
//! The scheduler is pull-based: workers claim the next unclaimed job
//! off a shared counter until exhaustion, so a slow game does not
//! stall the queue head. Results come back in input order.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::plugin::{ENV_REPORT, ENV_SCENARIO};
use crate::scenario::Scenario;
use crate::state::PlaytestReport;

use super::probe_scenarios::{ProbePlan, ProbePoint};

/// Default concurrency: 8 processes (upper bound of the design doc's
/// 6–8 range; each process is bounded by its scenario duration).
pub const DEFAULT_POOL_SIZE: usize = 8;

/// Wall-clock timeout per subprocess (seconds). Runs are bounded by
/// `duration_s`, but a hung game must fail the probe, not hang the
/// pool forever.
pub const DEFAULT_TIMEOUT_SECS: u64 = 300;

/// One probe run to execute: the scenario plus what it probes.
#[derive(Clone, Debug)]
pub struct ProbeJob {
    /// The validated scenario (already carries a stable `name`).
    pub scenario: Scenario,
    /// What this run is: probing `key` at `point`, or a control run.
    pub kind: ProbeJobKind,
    /// Last digest frame a COMPLETE run of `scenario` records:
    /// `ceil(duration_s * tps) - 1` (one digest per tick at frames
    /// `0..=ticks-1`). Snapshot at BUILD time from the plan, so a
    /// report whose trace stops short of it marks the run as
    /// incomplete — crashed or truncated runs must be refused, not
    /// read as inert (a partial trace can only vacuously "match"
    /// its control over the frames both recorded).
    pub expected_last_frame: u64,
    /// Request a static input-surface scan export from this run's
    /// child (see [`crate::plugin::ENV_DISCOVER_SCAN`]). Set on the
    /// FIRST control job only: every job runs the same game binary
    /// and schedule graph, so one scan describes them all — the
    /// export is idempotent data collection, not a per-probe
    /// measurement.
    pub export_scan: bool,
}

/// What a [`ProbeJob`] is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeJobKind {
    /// Probing this candidate key at this point.
    Probe { key: String, point: ProbePoint },
    /// Normalized control run (this point). Scheduled twice by the
    /// discovery flow — the pair of identical reports is the
    /// determinism gate.
    Control { point: ProbePoint },
}

/// Outcome of one executed job.
#[derive(Clone, Debug)]
pub struct ProbeOutcome {
    pub kind: ProbeJobKind,
    /// The parsed report; presence = success (exit code is
    /// annotation, not verdict).
    pub report: PlaytestReport,
    /// The child's static input-surface scan, when this job
    /// requested the export (`export_scan`) and the child delivered
    /// a parseable file. `None` otherwise — fail-open: the flow
    /// keeps its "static scan not run" note in that case.
    pub scan: Option<crate::discovery::static_scan::StaticScanResult>,
}

/// Infrastructure failures of the pool (never game findings).
#[derive(Clone, Debug)]
pub enum ProbeError {
    /// A probe/control run produced no report (crashed, timed out,
    /// or was killed). Includes the scenario name and cause.
    NoReport { scenario: String, cause: String },
    /// A report existed but did not parse as JSON.
    ReportParse { scenario: String, cause: String },
    /// A parsed report's trace is unusable for this job: either it is
    /// empty (`state_digest == []`, the game build lost
    /// `record_digest` support or the run died before the first
    /// digest tick), or it ENDS before the job's tap point (a panic
    /// before the frame the scenario presses its key), or it is
    /// INCOMPLETE (the trace stops short of the run's expected final
    /// frame `ceil(duration_s * tps) - 1` — a crash or truncation
    /// mid-run). Either way no trustworthy verdict is supported: a
    /// partial trace can only vacuously "match" its control over the
    /// frames both recorded, and an inert read from it would be a
    /// trusted false negative. Refused rather than recorded.
    UnusableReport { scenario: String },
    /// Matrix generation failed (invalid candidate key, ...).
    Plan(String),
    /// The parent could not stage the job's scenario for its child
    /// run — either the JSON encoding failed or the input FILE could
    /// not be written (disk full, permissions, ...). Nothing ran and
    /// no report exists: this is a parent-side setup fault, distinct
    /// from [`ProbeError::ReportParse`] (which means a child DID
    /// write a report we failed to parse) and from
    /// [`ProbeError::NoReport`] (which claims the game crashed,
    /// timed out, or was killed — a lie when nothing was spawned).
    ScenarioSerialize { scenario: String, cause: String },
    /// The parent could not create its staging workspace. NOTHING
    /// ran — no child was ever spawned — so this is a parent-side
    /// infrastructure fault, NOT a `NoReport` (whose Display claims
    /// the game crashed/timed out/was killed; a lie here would send
    /// users debugging a "crash" that never happened).
    Workspace { cause: String },
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoReport { scenario, cause } => write!(
                f,
                "probe run '{scenario}' produced no report ({cause}) — \
                 the game crashed, timed out, or was killed before takeover \
                 could report; refusing to interpret silence as 'no effect'"
            ),
            Self::ReportParse { scenario, cause } => {
                write!(
                    f,
                    "probe report for '{scenario}' is not valid JSON: {cause}"
                )
            }
            Self::UnusableReport { scenario } => {
                write!(
                    f,
                    "probe report for '{scenario}' has no complete digest trace \
                     covering its tap point and full run span — the run ended \
                     early (crash or truncation); refusing to interpret a \
                     vacuous trace as 'no effect'"
                )
            }
            Self::Plan(e) => write!(f, "probe plan generation failed: {e}"),
            Self::ScenarioSerialize { scenario, cause } => {
                write!(
                    f,
                    "probe scenario '{scenario}' could not be staged for its \
                     child run: {cause} — no run was started"
                )
            }
            Self::Workspace { cause } => write!(
                f,
                "probe pool could not create its staging workspace: {cause} — \
                 no probe run was started"
            ),
        }
    }
}

impl std::error::Error for ProbeError {}

/// Pool tuning knobs. Deliberately NOT `Default` — `binary` has no
/// sensible default (see `BatteryConfig` for the precedent).
#[derive(Clone, Debug)]
pub struct ProbePoolConfig {
    /// Feature-enabled game binary
    /// (`env!("CARGO_BIN_EXE_<name>")` from a test).
    pub binary: PathBuf,
    /// Concurrent subprocess cap (6–8 per the design doc).
    pub pool_size: usize,
    /// Wall-clock kill switch per subprocess, in seconds.
    pub timeout_secs: u64,
}

impl ProbePoolConfig {
    /// Config with the design-doc defaults (8 workers, 300s timeout).
    pub fn new(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
            pool_size: DEFAULT_POOL_SIZE,
            timeout_secs: DEFAULT_TIMEOUT_SECS,
        }
    }
}

/// Where each scenario + report pair is staged. One directory per pool
/// run; RAII-removed on drop (every return path — a failing probe in
/// CI must not leak ~50 directories per matrix onto the runner).
pub struct ProbeWorkspace {
    dir: PathBuf,
}

impl ProbeWorkspace {
    /// Create a uniquely-named workspace under the system temp dir.
    /// The name is unique PER CONSTRUCTION (PID + nanoseconds + a
    /// process-local monotonic counter): the clock alone is NOT
    /// enough — two concurrent `discover()` calls in one process
    /// can land within one coarse clock tick and would then share
    /// (and race-cleanup) one directory. The counter separates
    /// in-process callers; the PID separates processes.
    pub fn new() -> std::io::Result<Self> {
        static WS_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = WS_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "bevy_playtest_probe_{}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            seq,
        ));
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    fn sanitize(name: &str) -> String {
        // Scenario names are already validated to [A-Za-z0-9_-]; keep
        // the guard anyway so future name sources can't traverse.
        name.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    }

    /// Path this run's scenario JSON is written to. The job index
    /// disambiguates the TWO control copies that share a scenario
    /// name (each gets its own report — the determinism gate compares
    /// them).
    pub fn scenario_path(&self, job_index: usize, scenario_name: &str) -> PathBuf {
        self.dir.join(format!(
            "{:04}_{}.json",
            job_index,
            Self::sanitize(scenario_name)
        ))
    }

    /// Path this run's report is written to.
    pub fn report_path(&self, job_index: usize, scenario_name: &str) -> PathBuf {
        self.dir.join(format!(
            "{:04}_{}.report.json",
            job_index,
            Self::sanitize(scenario_name)
        ))
    }

    /// Path a scan-requesting run's static-scan sidecar is written
    /// to (see [`crate::plugin::ENV_DISCOVER_SCAN`]).
    pub fn scan_path(&self, job_index: usize, scenario_name: &str) -> PathBuf {
        self.dir.join(format!(
            "{:04}_{}.scan.json",
            job_index,
            Self::sanitize(scenario_name)
        ))
    }

    /// Working directory this job's child runs in: a per-job
    /// subdirectory of the workspace, created lazily by the spawn
    /// path right before the child starts. Jobs must NOT share
    /// one: two children racing `./save.json` in a shared cwd is
    /// exactly the cross-run contamination the isolation exists
    /// to prevent.
    ///
    /// RAII note: the subdirectory lives UNDER `self.dir`, so the
    /// workspace's own drop-cleanup removes it with everything else;
    /// a child killed by the watchdog mid-write may leave a file
    /// behind inside, which the same cleanup swallows.
    pub fn child_cwd_path(&self, job_index: usize) -> PathBuf {
        self.dir.join(format!("{:04}_cwd", job_index))
    }
}

impl Drop for ProbeWorkspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Build the full MVP job list from a plan: every candidate × both
/// probe points (tap only), plus each control run TWICE — the pair
/// of identical control reports is the double-control determinism
/// gate, and the first copy doubles as the diff baseline.
pub fn build_job_list(plan: &ProbePlan) -> Result<Vec<ProbeJob>, ProbeError> {
    let mut jobs = Vec::new();
    // Snapshot the expected final digest frame ONCE from the plan's
    // geometry: a complete run of `duration_s` at `tps` ticks
    // `ceil(duration_s * tps)` times and records its last digest at
    // frame `ticks - 1`. Captured here (not recomputed in
    // `execute_job`) so later mutation of the scenario object —
    // e.g. a caller shortening `duration_s` after plan build — can
    // never shrink the completeness expectation along with the run
    // and slip a partial trace through as "complete".
    let expected_last_frame = plan.expected_last_frame();
    for key in &plan.candidate_keys {
        for point in plan.points().map_err(|e| ProbeError::Plan(e.to_string()))? {
            jobs.push(ProbeJob {
                scenario: plan
                    .probe_scenario(key, point)
                    .map_err(|e| ProbeError::Plan(e.to_string()))?,
                kind: ProbeJobKind::Probe {
                    key: key.clone(),
                    point,
                },
                expected_last_frame,
                export_scan: false,
            });
        }
    }
    for (i, point) in plan
        .points()
        .map_err(|e| ProbeError::Plan(e.to_string()))?
        .into_iter()
        .enumerate()
    {
        let scenario = plan
            .control_scenario(point)
            .map_err(|e| ProbeError::Plan(e.to_string()))?;
        // First control job only: carries the static-scan export
        // request (one scan covers the whole matrix — same binary,
        // same schedule graph).
        let export_scan = i == 0;
        jobs.push(ProbeJob {
            scenario: scenario.clone(),
            kind: ProbeJobKind::Control { point },
            expected_last_frame,
            export_scan,
        });
        jobs.push(ProbeJob {
            scenario,
            kind: ProbeJobKind::Control { point },
            expected_last_frame,
            export_scan: false,
        });
    }
    Ok(jobs)
}

/// Execute one job synchronously: write the scenario JSON, spawn the
/// game binary (env-sanitized, report path pointed into `workspace`),
/// wait with a timeout, parse the report. Success = report written
/// AND the trace covers the job's tap point.
fn execute_job(
    config: &ProbePoolConfig,
    workspace: &ProbeWorkspace,
    job_index: usize,
    job: &ProbeJob,
) -> Result<ProbeOutcome, ProbeError> {
    let name = job
        .scenario
        .name
        .clone()
        .unwrap_or_else(|| "unnamed".into());
    let scenario_path = workspace.scenario_path(job_index, &name);
    let report_path = workspace.report_path(job_index, &name);
    let scan_path = workspace.scan_path(job_index, &name);

    // Refusal over silence, stale-report edition: the report path is
    // deterministic per (job_index, scenario name). A workspace reused
    // across sequential pools (the documented `run_jobs_in` pattern)
    // would otherwise let a later child that exits WITHOUT writing a
    // report inherit the previous job's file — a false successful
    // probe. The file this run reads must be one THIS child wrote.
    match std::fs::remove_file(&report_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            // Parent-side pre-spawn fault: nothing ran yet, so
            // ScenarioSerialize's "no run was started" story is the
            // honest one (NoReport would claim the game crashed).
            return Err(ProbeError::ScenarioSerialize {
                scenario: name.clone(),
                cause: format!("cannot clear stale report path: {e}"),
            });
        }
    }
    // Same staleness guard for the scan sidecar: a child that exits
    // before exporting must not leave the PREVIOUS export in place
    // (the parent would read stale reader data as this run's). Only
    // cleared when this job actually requests an export; other jobs
    // never read it.
    if job.export_scan {
        match std::fs::remove_file(&scan_path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                // Same parent-side pre-spawn taxonomy as the report
                // unlink above (see there).
                return Err(ProbeError::ScenarioSerialize {
                    scenario: name.clone(),
                    cause: format!("cannot clear stale scan path: {e}"),
                });
            }
        }
    }

    let json = serde_json::to_vec(&job.scenario).map_err(|e| ProbeError::ScenarioSerialize {
        scenario: name.clone(),
        cause: e.to_string(),
    })?;
    std::fs::write(&scenario_path, &json).map_err(|e| ProbeError::ScenarioSerialize {
        scenario: name.clone(),
        cause: format!("cannot write scenario: {e}"),
    })?;

    let mut cmd = std::process::Command::new(&config.binary);
    // Strip ALL playtest env vars first (strict takeover contract),
    // then set the two we own.
    for (key, _) in std::env::vars_os() {
        if let Some(k) = key.to_str() {
            if k.starts_with("BEVY_PLAYTEST") {
                cmd.env_remove(k);
            }
        }
    }
    // Child cwd/HOME isolation: a probe child used to inherit the
    // parent's working directory, so a game writing anything
    // `./`-relative (auto-saves, config, logs) scribbled into
    // wherever `cargo test` happened to run — the repo tree in CI —
    // and cross-contaminated sibling runs sharing that cwd. Give
    // every job its own throwaway directory instead, and repoint
    // HOME at it: config caches resolved through $HOME (e.g. the
    // `directories` crates) belong to the throwaway run, not to
    // the developer's real profile. The scenario/report/scan
    // paths above are ABSOLUTE (workspace-rooted), so the parent
    // keeps reading exactly what it staged regardless of the
    // child's cwd.
    //
    // READS keep resolving too — the SAME symlink-tree treatment
    // the battery gives its children
    // (`crate::battery::populate_symlink_tree`, the shared helper
    // behind this call): raw `./`-relative fs READS (a game reading
    // `./assets/...` outside the asset server — real games do this)
    // resolve through the symlink to the original tree. Asset
    // SERVER loading was never affected to begin with (bevy
    // resolves its asset root from `BEVY_ASSET_ROOT` /
    // `CARGO_MANIFEST_DIR` — inherited env — or the executable's
    // directory, never the process cwd;
    // bevy_asset::io::file::get_base_path).
    let child_cwd = workspace.child_cwd_path(job_index);
    std::fs::create_dir_all(&child_cwd).map_err(|e| ProbeError::ScenarioSerialize {
        scenario: name.clone(),
        cause: format!("cannot create child cwd: {e}"),
    })?;
    crate::battery::populate_symlink_tree(&child_cwd);
    cmd.current_dir(&child_cwd).env("HOME", &child_cwd);
    cmd.env(ENV_SCENARIO, &scenario_path)
        .env(ENV_REPORT, &report_path);
    if job.export_scan {
        // Opt-in scan export (see the outcome field docs): the child
        // runs the static scanner on its live `App` and writes the
        // sidecar. Failure is the CHILD's (best-effort, printed to
        // its stderr) and surfaces here as `scan: None` — fail-open.
        cmd.env(crate::plugin::ENV_DISCOVER_SCAN, &scan_path);
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null());
    if job.export_scan {
        // Scan-exporting jobs keep the child's stderr attached: the
        // scan is best-effort and its failures are reported there,
        // so a user left with `scan: None` can actually see WHY.
        // (Other jobs null it — pool output stays clean when the
        // matrix is large; the battery tees stderr instead —
        // live-forwarded and buffered for death diagnosis, see
        // `tee_child_stderr` in src/battery.rs.)
        cmd.stderr(std::process::Stdio::inherit());
    } else {
        cmd.stderr(std::process::Stdio::null());
    }

    let mut child = cmd.spawn().map_err(|e| ProbeError::NoReport {
        scenario: name.clone(),
        cause: format!("spawn failed: {e}"),
    })?;

    let deadline = Instant::now() + Duration::from_secs(config.timeout_secs);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() > deadline {
                    // Same kill sequence as the battery's timeout
                    // path: on Windows the tree-kill MUST precede
                    // the root kill or grandchildren outlive us (see
                    // `battery::kill_process`).
                    crate::battery::kill_process(&mut child);
                    return Err(ProbeError::NoReport {
                        scenario: name.clone(),
                        cause: format!("timed out after {}s", config.timeout_secs),
                    });
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => {
                // The child may STILL BE RUNNING (try_wait can fail
                // for OS reasons while the process is alive): this is
                // the one path that would otherwise leak a live child
                // past the pool, so kill it before refusing — same
                // sequence as the timeout path (tree-kill first on
                // Windows, see battery::kill_process).
                crate::battery::kill_process(&mut child);
                return Err(ProbeError::NoReport {
                    scenario: name.clone(),
                    cause: format!("wait failed: {e}"),
                });
            }
        }
    };

    let bytes = std::fs::read(&report_path).map_err(|_| ProbeError::NoReport {
        scenario: name.clone(),
        cause: format!("exit code {:?}, no report file", status.code()),
    })?;
    let report: PlaytestReport =
        serde_json::from_slice(&bytes).map_err(|e| ProbeError::ReportParse {
            scenario: name.clone(),
            cause: e.to_string(),
        })?;
    // Usable trace = TWO independent gates:
    //
    // 1. POST-TAP COVERAGE: at least one digest STRICTLY AFTER this
    //    job's tap point. Two vacuous classes are refused here:
    //    - an EMPTY trace (the run lost `record_digest` support or
    //      died before the first digest tick): no verdict is
    //      supported at all;
    //    - a trace that ends AT or before the tap point. Strictly-after
    //      is required, not at-the-tap: the bot writes the keyboard
    //      event during `Update`, but Bevy's `keyboard_input_system`
    //      consumes it in the NEXT `PreUpdate` — a run that stops at
    //      the tap frame never delivered the key to the game, so its
    //      "no effect" verdict would be vacuous.
    //
    // 2. COMPLETENESS: the trace must be the run's ENTIRE expected
    //    frame span AS A CONTIGUOUS SEQUENCE — exactly
    //    `expected_last_frame + 1` (= `ceil(duration_s * tps)`, snapshotted
    //    at build time from the plan) digests at frames
    //    `0..=expected_last_frame`, none missing. A maximum-frame
    //    check is not enough: a report holding only a digest at the
    //    final frame (or a sparse trace reaching it) passes it while
    //    most of the run was never observed, and `diff_trace` —
    //    which zips only the frames BOTH traces recorded — would
    //    return `None` over the shared islands: a VACUOUS inert that
    //    must not become a trusted negative or reach the cache.
    //    Contiguity is the recorder's real contract
    //    (`record_digest_system` emits one digest per tick over
    //    frames `0..=ticks-1`), so anything sparser is proof of a
    //    crash, truncation, or tampering — refuse it. Post-tap
    //    coverage alone is likewise insufficient: a run that dies one
    //    frame after the tap still passes gate 1.
    let point_frame = match &job.kind {
        ProbeJobKind::Probe { point, .. } => point.frame,
        ProbeJobKind::Control { point } => point.frame,
    };
    let has_post_tap_coverage = report.state_digest.iter().any(|d| d.frame > point_frame);
    if !has_post_tap_coverage
        || !covers_expected_span(&report.state_digest, job.expected_last_frame)
    {
        return Err(ProbeError::UnusableReport {
            scenario: name.clone(),
        });
    }
    // Scan sidecar: parse when this job requested the export. A
    // missing or malformed file is NOT an infrastructure error —
    // the scan is annotation-grade data, fail-open (the flow falls
    // back to its "static scan not run" note and empty readers).
    let scan = if job.export_scan {
        std::fs::read(&scan_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
    } else {
        None
    };
    Ok(ProbeOutcome {
        kind: job.kind.clone(),
        report,
        scan,
    })
}

/// Gate 2 of trace usability (see [`execute_job`]): the trace must be
/// EXACTLY the expected frame span as a contiguous sequence —
/// `expected_last_frame + 1` digests at frames `0..=expected_last_frame`.
/// Pure, so the sparse/gapped refusal classes are unit-testable
/// without a subprocess.
fn covers_expected_span(trace: &[crate::digest::StateDigest], expected_last_frame: u64) -> bool {
    trace.len() as u64 == expected_last_frame + 1
        && trace
            .iter()
            .enumerate()
            .all(|(idx, d)| d.frame == idx as u64)
}

/// Run the full job list through the pool. Jobs are claimed
/// dynamically by workers (pull-based); each claimed job runs to
/// completion even if a sibling fails (a partially-collected matrix
/// hides strictly more than a torn one — first error wins the
/// result, not fail-fast). Results come back in input order.
pub fn run_jobs(
    config: &ProbePoolConfig,
    jobs: &[ProbeJob],
) -> Vec<Result<ProbeOutcome, ProbeError>> {
    let workspace = match ProbeWorkspace::new() {
        Ok(w) => w,
        Err(e) => {
            // Dedicated variant: nothing ran, so `NoReport`'s
            // "crashed, timed out, or was killed" story would be a
            // lie (see the variant docs).
            let err = ProbeError::Workspace {
                cause: format!("cannot create workspace: {e}"),
            };
            return jobs.iter().map(|_| Err(err.clone())).collect();
        }
    };
    run_jobs_in(config, jobs, &workspace)
}

/// `run_jobs` with a caller-provided workspace (tests reuse one dir
/// across sequential pools).
pub fn run_jobs_in(
    config: &ProbePoolConfig,
    jobs: &[ProbeJob],
    workspace: &ProbeWorkspace,
) -> Vec<Result<ProbeOutcome, ProbeError>> {
    let total = jobs.len();
    if total == 0 {
        return vec![];
    }
    // Deliberately silent: `pool_size: 0` becomes one worker (a pool
    // of zero workers could never finish), and a pool larger than
    // the job count shrinks to the job count (idle workers cost a
    // spawn and nothing else, but why spawn them). Direct-field
    // construction can reach both; the public constructor defaults
    // to `DEFAULT_POOL_SIZE`.
    let workers = config.pool_size.clamp(1, total);
    let next = Mutex::new(0usize);
    // Slot-based result assembly: each worker pushes its completed
    // outcome into its claimed slot of this shared array. Contention
    // is negligible (one lock per JOB, not per frame). Scoped threads
    // borrow `jobs`/`config`/`workspace` — no Arc juggling.
    let results: Mutex<Vec<Option<Result<ProbeOutcome, ProbeError>>>> =
        Mutex::new((0..total).map(|_| None).collect());

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let idx = {
                    let mut guard = next.lock().unwrap_or_else(|poison| poison.into_inner());
                    let idx = *guard;
                    *guard += 1;
                    idx
                };
                if idx >= total {
                    break;
                }
                // execute_job needs no test-harness globals: it only
                // spawns subprocesses and reads files, so parallel
                // workers are safe here (the run_scenario exclusivity
                // applies to IN-PROCESS runs, not spawned binaries).
                let outcome = execute_job(config, workspace, idx, &jobs[idx]);
                let mut slots = results.lock().unwrap_or_else(|poison| poison.into_inner());
                slots[idx] = Some(outcome);
            });
        }
    });
    let mut slots = results.lock().unwrap_or_else(|poison| poison.into_inner());
    std::mem::take(&mut *slots)
        .into_iter()
        // Cannot fire via a worker panic: `thread::scope` propagates
        // a panicking worker to THIS thread before we reach here, so
        // this only guards a hypothetical future slot-writing bug that
        // leaves a slot `None` WITHOUT panicking (e.g. a broken
        // `break` path). Unwritten slots would otherwise surface as a
        // confusing `Option::expect` deep in `map`; this message says
        // what actually went wrong.
        .map(|r| r.expect("every claimed job wrote its slot"))
        .collect()
}

#[cfg(test)]
mod span_gate_tests {
    use super::covers_expected_span;
    use crate::digest::StateDigest;

    fn trace(frames: &[u64]) -> Vec<StateDigest> {
        frames
            .iter()
            .map(|&frame| StateDigest { frame, hash: 1 })
            .collect()
    }

    #[test]
    fn contiguous_full_span_is_accepted() {
        let expected_last = 5u64;
        assert!(covers_expected_span(
            &trace(&[0, 1, 2, 3, 4, 5]),
            expected_last
        ));
    }

    #[test]
    fn empty_trace_is_refused() {
        assert!(!covers_expected_span(&[], 0));
    }

    #[test]
    fn gap_anywhere_refuses_the_trace() {
        // Interior gap: frames 2 and 3 missing, both endpoints present.
        let gapped = trace(&[0, 1, 4, 5]);
        assert!(!covers_expected_span(&gapped, 5));
    }

    #[test]
    fn single_digest_at_the_last_frame_is_refused() {
        // Copilot's exact scenario: only a digest at the final frame.
        let sparse = trace(&[5]);
        assert!(!covers_expected_span(&sparse, 5));
    }

    #[test]
    fn duplicate_frames_are_refused_even_with_matching_len() {
        // len matches (6 == 5+1) but a duplicate shifts the tail:
        // index 3 holds frame 3 twice, frames 4-5 absent.
        let dup = trace(&[0, 1, 2, 3, 3, 5]);
        assert!(!covers_expected_span(&dup, 5));
    }

    #[test]
    fn longer_than_expected_span_is_refused() {
        // Expected 0..=5; a 7-digest trace is over-long: the extra
        // digest means the run and the plan disagree on the span.
        let overlong = trace(&[0, 1, 2, 3, 4, 5, 6]);
        assert!(!covers_expected_span(&overlong, 5));
    }

    #[test]
    fn one_short_of_the_span_is_refused() {
        // Expected 0..=5; only 0..=4 present: crashes one frame
        // before the end still leave an unobserved frame.
        let short = trace(&[0, 1, 2, 3, 4]);
        assert!(!covers_expected_span(&short, 5));
    }

    #[test]
    fn wrong_start_frame_is_refused() {
        // Starts at 1, not 0: never happens via the recorder, but a
        // truncated head must not slip through positional counting.
        let no_zero = trace(&[1, 2, 3, 4, 5, 6]);
        assert!(!covers_expected_span(&no_zero, 5));
    }
}
