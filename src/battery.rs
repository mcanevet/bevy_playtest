//! Test-side battery runner: run a playtest scenario from a plain
//! `cargo test`.
//!
//! The intended game integration (feature-gated, self-dev-dependency
//! pattern — see the repo docs):
//!
//! ```toml
//! # Cargo.toml
//! [dependencies]
//! bevy_playtest = { version = "...", optional = true }
//!
//! [dev-dependencies]
//! # Self-dependency: activates OUR OWN `playtest` feature only for
//! # test builds (normal & release builds stay clean).
//! my_game = { path = ".", features = ["playtest"] }
//!
//! [features]
//! playtest = ["dep:bevy_playtest", "bevy_playtest/takeover"]
//! ```
//!
//! ```rust
//! // src/main.rs
//! #[cfg(feature = "playtest")]
//! app.add_plugins(bevy_playtest::PlaytestPlugin);
//! ```
//!
//! ```rust,ignore
//! // tests/playtest.rs — the whole file:
//! # #[test]
//! fn playtest_battery() {
//!     bevy_playtest::battery::run_and_expect_clean(
//!         env!("CARGO_BIN_EXE_my_game"),
//!         bevy_playtest::battery::BUILTIN_BATTERY,
//!     );
//! }
//! ```
//!
//! `cargo test` compiles the game binary WITH the playtest feature
//! active (the self-dev-dep is what activates it), and Cargo hands
//! the test a path to that binary via `CARGO_BIN_EXE_<name>`. The
//! helper spawns it with the chosen scenario (`"battery"` = the
//! built-in zero-config battery run: chaos bot, all oracles; a path =
//! your authored scenario JSON), reads the report, and fails the
//! test with a human-readable verdict on findings — native
//! cargo-test failure output, counted by CI.
//!
//! `cargo build --release` compiles the game WITHOUT the feature: the
//! plugin lines are cfg'd out and zero playtest bytes ship.

use crate::discovery::flow;
use crate::plugin::ENV_REPORT;
use crate::plugin::ENV_SCENARIO;
use crate::state::PlaytestReport;

/// Scenario selector for [`run`]: the built-in default battery
/// (zero-config battery run — chaos bot, all oracles, generous
/// liveness timeout). Pass a scenario file path instead to run an
/// authored scenario.
pub use crate::plugin::BATTERY_LITERAL as BUILTIN_BATTERY;

/// How long to wait for the run to finish before failing the test
/// (runs are bounded by their `duration_s`, but a hung or looping
/// game must fail the test, not hang CI forever).
const DEFAULT_TIMEOUT_SECS: u64 = 300;

/// Run one scenario against a game binary and return the parsed
/// report. `scenario` is either a path to a scenario JSON file or the
/// built-in literal [`BUILTIN_BATTERY`] (`"battery"`). The child
/// inherits stdout/stderr (visible with `cargo test -- --nocapture`).
pub fn run(binary: &str, scenario: &str) -> Result<PlaytestReport, BatteryError> {
    run_with_config(&BatteryConfig {
        binary: binary.to_string(),
        scenario: scenario.to_string(),
        timeout_secs: DEFAULT_TIMEOUT_SECS,
    })
}

/// Battery configuration.
///
/// NOTE: deliberately NOT `Default` — the `binary` field has no
/// sensible default (an empty path can only ever fail to spawn).
/// Construct explicitly; `run`/`run_and_expect_clean` cover the
/// common paths.
#[derive(Debug, Clone)]
pub struct BatteryConfig {
    /// Path to the feature-enabled game binary
    /// (`env!("CARGO_BIN_EXE_<name>")` from a `#[test]`).
    pub binary: String,
    /// Scenario path, or [`BUILTIN_BATTERY`] for the built-in default.
    pub scenario: String,
    /// Wall-clock kill switch for a hung game (seconds).
    pub timeout_secs: u64,
}

/// Battery failure modes that are NOT game findings (infrastructure
/// problems — the game crashed before reporting, timed out, etc.).
/// Game findings surface through the report instead.
#[derive(Debug)]
pub enum BatteryError {
    /// Spawning the game binary failed.
    Spawn(String),
    /// The game did not finish within the timeout (soft-lock/loop).
    Timeout { timeout_secs: u64 },
    /// No report was written (game died before the takeover could
    /// report — includes the exit code).
    NoReport { exit_code: Option<i32> },
    /// A report was written but could not be parsed.
    ReportParse(String),
    /// Opt-in input-surface discovery failed (probe matrix or setup).
    /// Discovery is infrastructure, not a game finding — a game that
    /// crashes under probing surfaces here, while a game that is
    /// merely NONDETERMINISTIC still gets a (statistical) surface.
    Discovery(String),
}

impl std::fmt::Display for BatteryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(e) => write!(f, "failed to spawn game binary: {e}"),
            Self::Timeout { timeout_secs } => {
                write!(f, "run timed out after {timeout_secs}s (hung game?)")
            }
            Self::NoReport { exit_code } => write!(
                f,
                "game exited ({exit_code:?}) without writing a playtest report — \
                 it likely crashed before the takeover took over. Its stdout/stderr \
                 are captured by the failing test output (use --test-threads=1 if \
                 interleaving hides them)."
            ),
            Self::ReportParse(e) => write!(f, "playtest report is not valid JSON: {e}"),
            Self::Discovery(e) => {
                write!(f, "input-surface discovery failed: {e}")
            }
        }
    }
}

impl std::error::Error for BatteryError {}

/// Run with explicit config. Returns the parsed report on any
/// completed run — INCLUDING failing ones (findings are the report's
/// content, not an infrastructure error). Inspect `report.status` /
/// `report.violations`, or use [`run_and_expect_clean`].
///
/// With `BEVY_PLAYTEST_DISCOVER=1` in the environment, the returned
/// report additionally carries a discovered input surface
/// (`report.discovered_surface`, frozen-world findings annotated with
/// probe statistics via `_meta`). Discovery runs AFTER the game child
/// completes, as a report-annotation step — it does not touch the
/// run itself; the game child never sees the opt-in variables. See
/// [`crate::discovery::flow`] for the full opt-in contract (candidate
/// and cache-dir overrides).
///
/// With `BEVY_PLAYTEST_DISCOVER_INJECT=1`, discovery changes the
/// run itself: the parent discovers FIRST (cache-first, ~40s cold
/// at pool=8), and when the scenario is the built-in battery literal
/// (whose key surface is undeclared — the default WASD+arrows set
/// the game may ignore), it writes a scenario variant whose chaos
/// bot samples exactly the discovered effective keys and drives the
/// child with it, recording `report.injected_surface_keys`. An
/// authored scenario file is never injected into — declaration beats
/// inference; discovery then describes rather than replaces.
pub fn run_with_config(config: &BatteryConfig) -> Result<PlaytestReport, BatteryError> {
    let dir = std::env::temp_dir().join(format!(
        "bevy_playtest_battery_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
    ));
    std::fs::create_dir_all(&dir).map_err(|e| BatteryError::Spawn(e.to_string()))?;
    // RAII cleanup: EVERY return path (spawn error, timeout, missing
    // report, bad JSON) removes the temp dir — a failing test in CI
    // must not leak one directory per attempt onto the runner.
    struct TempDirGuard(std::path::PathBuf);
    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _guard = TempDirGuard(dir.clone());
    let report_path = dir.join("report.json");

    // Discovery-driven scenario injection (see
    // `discovery::flow::ENV_DISCOVER_INJECT`). Precedence rules:
    // - An authored scenario FILE always wins — declaration beats
    //   inference, exactly as the report-annotation path treats a
    //   declared `raw_surface`.
    // - Injection targets only the built-in battery literal, whose
    //   surface is undeclared (the default WASD+arrows set).
    // - `inject_requested` implies discovery: the surface feeding
    //   the injected scenario is the very surface annotated onto
    //   the report, so discovery runs ONCE here (cache-first) and
    //   the post-run annotation step reuses it — never re-probes.
    let mut scenario_arg = config.scenario.clone();
    let mut pre_discovered: Option<crate::discovery::surface::DiscoveredSurface> = None;
    let mut injected_keys: Option<Vec<String>> = None;
    if flow::inject_requested() && scenario_arg == BUILTIN_BATTERY {
        let outcome = flow::discover_from_env(
            std::path::Path::new(&config.binary),
            Some(config.scenario.as_str()),
        )
        .map_err(|e| BatteryError::Discovery(e.to_string()))?;
        let keys = outcome.surface.chaos_keys();
        // The injected scenario is the built-in battery with exactly
        // one field added: the chaos bot's key surface, so the bot
        // samples only keys the game demonstrably consumes. Mouse
        // buttons stay at their default (three primaries): the MVP
        // probe matrix is keyboard-only, so there is no discovered
        // mouse surface to inject — an explicitly empty mouse list
        // would DISABLE mouse chaos, which discovery never licensed.
        let mut scenario = crate::plugin::battery_scenario()
            .map_err(|e| BatteryError::Spawn(format!("injected battery scenario: {e}")))?;
        scenario.bot.raw_surface = Some(crate::scenario::RawSurface {
            keys: keys.clone(),
            ..Default::default()
        });
        let injected_path = dir.join("scenario.injected.json");
        let json = serde_json::to_string_pretty(&scenario)
            .map_err(|e| BatteryError::Spawn(format!("serialize injected scenario: {e}")))?;
        std::fs::write(&injected_path, json)
            .map_err(|e| BatteryError::Spawn(format!("write injected scenario: {e}")))?;
        scenario_arg = injected_path.to_string_lossy().into_owned();
        pre_discovered = Some(outcome.surface);
        injected_keys = Some(keys);
    }

    let mut cmd = std::process::Command::new(&config.binary);
    // Isolate the run from the developer's shell FIRST. The takeover
    // REJECTS unknown BEVY_PLAYTEST_* variables (strict env contract),
    // and a leaked BEVY_PLAYTEST_REPLAY_PASS_B_SERIES would turn every
    // run into a pass-B child that never plays the game — so strip
    // every playtest-related variable wholesale. The two we set below
    // are the entire contract, so they must be set AFTER the strip.
    for (key, _) in std::env::vars_os() {
        if let Some(k) = key.to_str() {
            if k.starts_with("BEVY_PLAYTEST") {
                cmd.env_remove(k);
            }
        }
    }
    cmd.env(ENV_SCENARIO, &scenario_arg)
        .env(ENV_REPORT, &report_path);

    let mut child = cmd
        .spawn()
        .map_err(|e| BatteryError::Spawn(format!("{}: {e}", config.binary)))?;

    // Wall-clock watchdog: the run is deterministic and bounded by its
    // duration_s, but a game that blocks forever must fail the test,
    // not CI.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(config.timeout_secs);
    let status = loop {
        let Some(status) = child
            .try_wait()
            .map_err(|e| BatteryError::Spawn(e.to_string()))?
        else {
            if std::time::Instant::now() > deadline {
                kill_process(&mut child);
                return Err(BatteryError::Timeout {
                    timeout_secs: config.timeout_secs,
                });
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
            continue;
        };
        break status;
    };

    let bytes = std::fs::read(&report_path).map_err(|_| BatteryError::NoReport {
        exit_code: status.code(),
    })?;
    let mut report: PlaytestReport =
        serde_json::from_slice(&bytes).map_err(|e| BatteryError::ReportParse(e.to_string()))?;

    // Opt-in input-surface discovery (env contract in
    // `discovery::flow`): the game child never sees these variables
    // (stripped above), so this is purely a parent-side decision.
    // A cache hit makes this a no-op cost-wise; a cold run probes
    // the matrix (~40s at pool=8) before returning.
    //
    // INJECT implies discovery — even when injection itself refuses
    // (an authored scenario file): the promised annotation must not
    // vanish with it. When injection RAN, discovery already
    // happened BEFORE the child (the surface fed the injected
    // scenario) — reuse it; the scenario string would otherwise name
    // the INJECTED file and cold-miss the "battery"-keyed cache
    // entry, re-probing the whole matrix for a surface we already
    // hold.
    if flow::env_requested() || flow::inject_requested() {
        let surface = match pre_discovered {
            Some(surface) => surface,
            None => {
                flow::discover_from_env(
                    std::path::Path::new(&config.binary),
                    Some(config.scenario.as_str()),
                )
                .map_err(|e| BatteryError::Discovery(e.to_string()))?
                .surface
            }
        };
        // Honesty: record WHICH keys injection put in the bot's
        // surface (None ⇒ injection did not happen — e.g. an authored
        // scenario file, or inject not requested). The recorded list
        // is exactly what the injected scenario carried.
        report.injected_surface_keys = injected_keys;
        report.discovered_surface = Some(surface);
        report.annotate_frozen_world_with_surface();
    }
    Ok(report)
}

/// Kill a hung child and reap it (avoid a zombie and — on Windows —
/// a still-running process outliving the test).
///
/// `pub(crate)` because the discovery probe pool
/// (`crate::discovery::probe_pool`) kills the SAME class of children
/// — arbitrary game binaries under a timeout kill switch — and must
/// not diverge from this proven sequence: on Windows, a game that
/// spawned grandchildren (launcher shim, asset compiler, crash
/// handler) leaks them if only the root PID dies.
pub(crate) fn kill_process(child: &mut std::process::Child) {
    #[cfg(windows)]
    {
        // TASKKILL with /T must run BEFORE the root PID is terminated,
        // otherwise it cannot traverse the process tree. Best-effort;
        // errors are swallowed — the caller is already on the timeout
        // failure path.
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .status();
    }
    // Portable direct kill of OUR child (SIGKILL on Unix,
    // TerminateProcess on Windows): no PATH lookup, no external
    // binary to be missing.
    let _ = child.kill();
    // Reap so the OS doesn't hold a zombie entry until test exit.
    // Bounded: the child was just killed, so this returns promptly.
    let _ = child.wait();
}

/// Verdict of a completed run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatteryOutcome {
    /// The game passed: no gating violations.
    Clean,
    /// The game has findings (see the report for the summary).
    Findings,
}

impl PlaytestReport {
    /// Human-readable multi-finding summary (one line per violation
    /// plus a replay hint) — the message a failing `#[test]` panics
    /// with. `original_scenario` is the literal/path passed to the
    /// runner (e.g. `"battery"` or `"playtests/menu.json"`), used in
    /// the reproduce hint.
    pub fn findings_summary(&self, original_scenario: &str) -> String {
        let mut lines = String::from("playtest findings:\n");
        for v in &self.violations {
            lines.push_str(&format!(
                "  [{}] {} (target: {}, frames {}-{}, x{}): {}\n",
                self.status, v.rule, v.target, v.first_frame, v.last_frame, v.count, v.detail,
            ));
        }
        lines.push_str(&format!(
            "  reproduce: BEVY_PLAYTEST_SCENARIO={} cargo run --features playtest --bin <game>",
            original_scenario
        ));
        lines
    }
}

/// Convenience for `#[test]` bodies: run one scenario against the
/// game binary and panic with the findings summary if the game did
/// not pass. Infrastructure errors (spawn/timeout/no-report) also
/// panic, with their own message.
pub fn run_and_expect_clean(binary: &str, scenario: &str) {
    let report = match run(binary, scenario) {
        Ok(report) => report,
        Err(e) => panic!("{e}"),
    };
    match report.status {
        crate::enums::PlaytestStatus::Pass => {}
        _ => panic!("{}", report.findings_summary(scenario)),
    }
}
