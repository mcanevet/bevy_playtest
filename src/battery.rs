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
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

/// Scenario selector for [`run`]: the built-in default battery
/// (zero-config battery run — chaos bot, all oracles, generous
/// liveness timeout). Pass a scenario file path instead to run an
/// authored scenario.
pub use crate::plugin::BATTERY_LITERAL as BUILTIN_BATTERY;

/// How long to wait for the run to finish before failing the test
/// (runs are bounded by their `duration_s`, but a hung or looping
/// game must fail the test, not hang CI forever).
const DEFAULT_TIMEOUT_SECS: u64 = 300;

/// Stable sentence shared by BOTH arms of bevy_render's
/// `GPU_NOT_FOUND_ERROR_MESSAGE` (bevy_render 0.20.1,
/// src/renderer/mod.rs:159): the constant is cfg-split per platform —
/// Linux appends a linux_dependencies.md URL, other platforms stop
/// here — so only this common part is safe to match. Found in the
/// child's stderr when `initialize_renderer`'s
/// `selected_adapter.expect(...)` fires at startup: headless takeover
/// drops the window, not the renderer, so a camera-bearing game
/// panics on any host without a GPU adapter (stock CI runners).
const GPU_PANIC_MARKER: &str =
    "Unable to find a GPU! Make sure you have installed required drivers!";

/// How much of the child's stderr to retain for failure diagnosis.
/// The tail (not the head) is kept: the panic that explains a death
/// is at the END of the output, after any startup log noise.
const STDERR_TAIL_BYTES: usize = 64 * 1024;

/// How long [`StderrTee::drain`] waits for the stderr tee to observe
/// EOF after the child is gone before DETACHING it (dropping the
/// receiver and letting the thread leak — the tail buffer lives
/// behind an `Arc`, so a detached tee cannot dangle). EOF normally
/// arrives within microseconds of child death; the only way it
/// does not is a GRANDCHILD inheriting the stderr pipe (launcher
/// shims, asset compilers, crash handlers — the same hazard
/// [`kill_process`] documents), which would otherwise turn the
/// watchdog's bounded timeout into a permanent join hang. If grace
/// expires while stderr is still legitimately in flight, the
/// detached snapshot loses whatever has not been copied yet: up to
/// the pipe's OS capacity (typically 64 KiB) plus one in-flight
/// chunk — a bounded evidence trade against an unbounded hang.
const TEE_EOF_GRACE: std::time::Duration = std::time::Duration::from_secs(3);

/// Whether the buffered child stderr contains bevy_render's
/// GPU-adapter panic. Lossy UTF-8 conversion is fine for substring
/// presence, and stderr frames from panics are plain ASCII here.
fn stderr_reports_gpu_panic(tail: &[u8]) -> bool {
    String::from_utf8_lossy(tail).contains(GPU_PANIC_MARKER)
}

/// Tee the child's stderr: forward every chunk LIVE (the same
/// content a child with inherited stderr would print, modulo
/// interleaving position — chunks are emitted as they arrive, so
/// ordering relative to the parent's own output can shift) while
/// retaining a bounded tail for the failure path —
/// a child that dies before writing a report gets DIAGNOSED (see
/// [`BatteryError::GpuMissing`]) instead of leaving an opaque exit
/// code. The thread exits at the child's stderr EOF, including
/// after a watchdog kill, and signals EOF over the returned channel
/// so [`StderrTee::drain`] can bound its wait (see `TEE_EOF_GRACE`).
fn tee_child_stderr(stderr: std::process::ChildStderr) -> StderrTee {
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&buffer);
    let (eof_tx, eof_rx) = std::sync::mpsc::channel::<()>();
    let join = std::thread::spawn(move || {
        let mut stderr = stderr;
        let mut chunk = [0u8; 4096];
        loop {
            match stderr.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    // Live-forward first: visibility must not depend
                    // on the child's eventual fate.
                    let _ = std::io::stderr().write_all(&chunk[..n]);
                    // Then retain the tail (poison-tolerant lock: a
                    // panicking sibling must not lose the evidence).
                    let mut tail = sink.lock().unwrap_or_else(|p| p.into_inner());
                    tail.extend_from_slice(&chunk[..n]);
                    let excess = tail.len().saturating_sub(STDERR_TAIL_BYTES);
                    if excess > 0 {
                        tail.drain(..excess);
                    }
                }
            }
        }
        // Signal EOF even if nobody listens; the receiver may have
        // been dropped (drained after a timeout) — that's fine.
        let _ = eof_tx.send(());
    });
    StderrTee {
        tail: buffer,
        eof: eof_rx,
        join: Some(join),
    }
}

/// Running handle to a [`tee_child_stderr`] thread: the bounded-tail
/// buffer (behind an `Arc`, shared with the thread) plus the EOF
/// signal and join handle. DROPPING the handle detaches the thread
/// — it finishes at child-stderr EOF or, if a grandchild inherited
/// the pipe, leaks harmlessly (the `Arc` keeps the tail valid and
/// the live-forwarding already happened).
struct StderrTee {
    tail: Arc<Mutex<Vec<u8>>>,
    eof: std::sync::mpsc::Receiver<()>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl StderrTee {
    /// Wait for the tee to observe EOF — BOUNDED by `TEE_EOF_GRACE`
    /// — then return the retained tail. On grace expiry (a
    /// grandchild holds the stderr pipe open past its parent's
    /// death) the thread is DETACHED and leaked: the tail lives
    /// behind the shared `Arc`, so the returned clone is still
    /// whatever the tee managed to copy, and the run must conclude
    /// with its verdict rather than hang on `join`.
    fn drain(mut self) -> Vec<u8> {
        if self.eof.recv_timeout(TEE_EOF_GRACE).is_ok() {
            // EOF observed: the thread is about to return, so this
            // join is a formality (never unbounded).
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
        }
        // Else: grace expired — drop the JoinHandle and leak the
        // thread. `Option::take` above leaves nothing to wait on.
        self.tail.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

/// Run one scenario against a game binary and return the parsed
/// report. `scenario` is either a path to a scenario JSON file or the
/// built-in literal [`BUILTIN_BATTERY`] (`"battery"`). The child runs
/// in its own throwaway directory with `HOME` repointed at it (see
/// [`run_with_config`] — note this blinds `~`-resolved READS to the
/// throwaway dir, a deliberate determinism gain), inherits stdout,
/// and its stderr is TEED — forwarded live (visible with
/// `cargo test -- --nocapture`, though interleaving position can
/// shift) and buffered so the failure path can diagnose the child's
/// death (see [`BatteryError::GpuMissing`]).
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
///
/// `#[non_exhaustive]`: new failure modes arrive as the harness grows
/// (e.g. `GpuMissing`), and downstream match arms must not break —
/// a caller matching exhaustively today should already carry a
/// catch-all, but this makes that a guarantee instead of a hope.
/// Note the interaction on 0.x: adding `#[non_exhaustive]` to an
/// existing enum is itself breaking (hence it ships with the other
/// breaking changes in the same release).
#[derive(Debug)]
#[non_exhaustive]
pub enum BatteryError {
    /// Spawning the game binary failed.
    Spawn(String),
    /// The game did not finish within the timeout (soft-lock/loop).
    Timeout { timeout_secs: u64 },
    /// No report was written (game died before the takeover could
    /// report — includes the exit code).
    NoReport { exit_code: Option<i32> },
    /// No report was written AND the buffered child stderr matches
    /// bevy_render's GPU-adapter panic (`initialize_renderer`'s
    /// `GPU_NOT_FOUND_ERROR_MESSAGE`). Headless≠GPU-less:
    /// `RenderPlugin` still initializes a real `wgpu` device, so a
    /// camera-bearing game panics at startup on hosts without a GPU
    /// adapter (stock CI runners). Named failure + remediation, not
    /// a mystery crash.
    GpuMissing { exit_code: Option<i32> },
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
            Self::GpuMissing { exit_code } => write!(
                f,
                "game exited ({exit_code:?}) without writing a playtest report \
                 because it could not find a GPU adapter: headless takeover drops \
                 the window, not the renderer — RenderPlugin still initializes a \
                 real wgpu device, which needs a GPU adapter even without a \
                 display. Fix the CI HOST (not the game): install a software \
                 Vulkan adapter — on Debian/Ubuntu, `sudo apt-get install -y \
                 mesa-vulkan-drivers` (lavapipe); see \
                 docs/how-to/headless-ci-gpu.md in the bevy_playtest repo for \
                 the full recipe."
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
    // Child cwd/HOME isolation (same treatment as the probe pool's
    // `execute_job` — see #74): the child used to inherit the
    // parent's working directory, so a game writing anything
    // `./`-relative (auto-saves, config, logs) scribbled into
    // wherever `cargo test` happened to run — the repo tree in CI.
    // Run the child in its own throwaway subdirectory and repoint
    // HOME at it: `./`-relative writes land there, and config caches
    // resolved through $HOME belong to the throwaway run, not to the
    // developer's real profile. `report_path` is ABSOLUTE
    // (workspace-rooted), so the parent keeps reading exactly what
    // it staged regardless of the child's cwd.
    //
    // The HOME repoint has a READ-side consequence too (a
    // deliberate determinism win, shared with the pool): a game
    // reading `~/.config/...` no longer sees the developer's
    // profile — those reads now resolve against the throwaway
    // dir and come up empty, exactly as they would on a fresh
    // CI machine. (Windows note: only HOME is repointed, not
    // USERPROFILE — the `directories`-style crates' degradations
    // there are weaker than on Unix.)
    //
    // Reads still resolve: the throwaway dir's top level is a
    // SYMLINK TREE — one symlink per entry of the original cwd
    // (created BEFORE the child starts). A game reading
    // `./assets/...` with raw `fs` calls (not the asset server —
    // real games do this; the thetawave fixture's background
    // picker is the shipped proof) resolves through the symlink to
    // the original tree, exactly as it did before isolation. New
    // `./`-relative file CREATION (auto-saves, logs, caches) has
    // no symlink to follow and lands in the throwaway dir; the
    // RAII guard removes it with everything else. Overwriting a
    // tracked file still writes through to the real tree — the
    // same exposure the inherited-cwd status quo had for those
    // paths, never worse.
    //
    // Asset SERVER loading is doubly unaffected: bevy resolves its
    // default asset root from `BEVY_ASSET_ROOT` / `CARGO_MANIFEST_DIR`
    // (inherited env) or the executable's directory — never the
    // process cwd (bevy_asset::io::file::get_base_path).
    //
    // Platform note: symlinks need no privileges on Unix; Windows
    // may require developer mode for unprivileged symlink creation
    // — if population fails there the run degrades to a plain
    // throwaway cwd (reads fall back to creating their own files),
    // never to a spawn failure.
    let child_cwd = dir.join("child_cwd");
    std::fs::create_dir_all(&child_cwd)
        .map_err(|e| BatteryError::Spawn(format!("cannot create battery child cwd: {e}")))?;
    populate_symlink_tree(&child_cwd);

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

    // cwd/HOME isolation (see the comment at `child_cwd` above) +
    // the stderr tee: stdout still inherited, stderr PIPED so the
    // tee thread can forward it live and buffer a copy for the
    // failure path (see `tee_child_stderr`).
    cmd.current_dir(&child_cwd).env("HOME", &child_cwd);
    cmd.stdout(std::process::Stdio::inherit());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| BatteryError::Spawn(format!("{}: {e}", config.binary)))?;

    let stderr_handle = child.stderr.take();
    // Any EARLY return below (spawn-adjacent io failures surfaced by
    // `?`) DROPS this handle, which DETACHES the tee thread mid-copy
    // — deliberate and safe: the tail lives behind an `Arc` and the
    // live-forwarding already happened (see `StderrTee`'s docs).
    let stderr_tee = stderr_handle.map(tee_child_stderr);

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
                // Deliberately NO tee drain here: the tail is
                // unused on the timeout path (this early return
                // discards it — see the comment at `break status`
                // for why the normal path DOES drain), and waiting
                // the grace window would stall the verdict by up
                // to `TEE_EOF_GRACE` on a pipe a grandchild may
                // hold forever. Dropping the tee DETACHES the
                // thread: harmless (tail behind an `Arc`); the
                // verdict stays within the watchdog's budget
                // rather than paying any grace on top of it.
                // (The regression test pins this only as "well
                // under ten seconds" — the real guarantee is
                // budget + poll quantum, but a drain-reinstating
                // regression with an UNBOUNDED grace is caught:
                // it blows the test's bound.)
                drop(stderr_tee);
                return Err(BatteryError::Timeout {
                    timeout_secs: config.timeout_secs,
                });
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
            continue;
        };
        break status;
    };
    // Child reaped: the tail drives the no-report diagnosis below,
    // so here the tee IS drained — BOUNDED by `TEE_EOF_GRACE` (a
    // grandchild may still hold the pipe's write end). On grace
    // expiry the tee thread is leaked deliberately: the tail it
    // copied so far is still returned. (The timeout path above
    // drops the tee WITHOUT draining — its verdict does not need
    // the tail, and waiting the grace there would only delay it.)
    let stderr_tail = stderr_tee.map(StderrTee::drain);

    // GPU-panic diagnosis: a no-report death whose stderr matches
    // bevy_render's adapter panic gets a NAMED failure with
    // remediation guidance; any other death keeps the generic
    // NoReport message — detection must never mask other failures.
    let bytes = std::fs::read(&report_path).map_err(|_| {
        if stderr_tail.as_deref().is_some_and(stderr_reports_gpu_panic) {
            BatteryError::GpuMissing {
                exit_code: status.code(),
            }
        } else {
            BatteryError::NoReport {
                exit_code: status.code(),
            }
        }
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

/// Populate a fresh directory with one symlink per top-level entry
/// of the process's current working directory. Best-effort by
/// design: a skipped entry degrades the child's read isolation for
/// that path only, never the spawn; an ALREADY-symlinked source
/// tree simply chains (symlink to symlink), and name collisions
/// (hidden files etc.) are overwritten harmlessly inside the
/// throwaway dir.
///
/// `pub(crate)` because the discovery probe pool
/// (`crate::discovery::probe_pool`) isolates the SAME class of
/// children — arbitrary game binaries reading `./`-relative assets
/// with raw `fs` calls — and must not diverge from this proven
/// tree (the same sharing precedent as [`kill_process`]).
pub(crate) fn populate_symlink_tree(child_cwd: &std::path::Path) {
    // Entries under the temp root get NO symlink: if the parent cwd
    // itself lives inside `std::env::temp_dir()` (developer ran
    // `cargo test` from a scratch checkout), the freshly-created
    // throwaway dir sits there too, and a symlink pointing at (a
    // parent of) the tree's own destination would loop the tree
    // into itself.
    let skip_prefix = std::fs::canonicalize(std::env::temp_dir()).ok();
    let Ok(entries) = std::fs::read_dir(".") else {
        return;
    };
    for entry in entries.flatten() {
        // Canonicalize the target: `read_dir(".")` yields RELATIVE
        // paths, and a symlink target resolves relative to the
        // SYMLINK's directory (the child cwd), which would make it
        // dangle. The absolute spelling never dangles.
        let Ok(target) = std::fs::canonicalize(entry.path()) else {
            continue;
        };
        if skip_prefix
            .as_deref()
            .is_some_and(|prefix| target.starts_with(prefix))
        {
            continue;
        }
        let link = child_cwd.join(entry.file_name());
        #[cfg(unix)]
        let _ = std::os::unix::fs::symlink(target, link);
        #[cfg(windows)]
        {
            // Windows needs the RIGHT symlink flavor for the entry
            // type: `symlink_dir` on a file (or vice versa) fails,
            // which would silently blind HALF the tree. An entry
            // whose type cannot be read (race) gets both flavors
            // tried, best-effort like everything here.
            let is_dir = entry.file_type().map(|t| t.is_dir());
            let _ = match is_dir {
                Some(true) => std::os::windows::fs::symlink_dir(&target, &link),
                Some(false) => std::os::windows::fs::symlink_file(&target, &link),
                None => std::os::windows::fs::symlink_dir(&target, &link)
                    .or_else(|_| std::os::windows::fs::symlink_file(&target, &link)),
            };
        }
    }
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
