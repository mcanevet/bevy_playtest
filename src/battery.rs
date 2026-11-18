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
use crate::discovery::source_scan;
use crate::plugin::ENV_REPORT;
use crate::plugin::ENV_SCENARIO;
use crate::scenario::BACKBONE_KEYS;
use crate::state::PlaytestReport;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

/// [`BACKBONE_KEYS`] as an owned `Vec<String>` in CANONICAL
/// (sorted) order — the comparison form for set-equality checks
/// against scan-derived surfaces, which emit alphabetically (the
/// backbone itself keeps its declared order).
fn backbone_keys_vec_sorted() -> Vec<String> {
    let mut keys: Vec<String> = BACKBONE_KEYS.iter().map(|s| s.to_string()).collect();
    keys.sort();
    keys
}

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

/// A unique per-run temp-directory path (not yet created).
///
/// Uniqueness = pid + nanosecond timestamp + a process-local monotonic
/// counter. The pid only separates *processes* and the clock is coarse
/// — two battery runs started by the same test binary (parallel
/// `#[test]`s) share the pid and can land within one clock tick, which
/// without the counter would hand them ONE shared directory: one
/// run's cleanup guard deletes it out from under the other's
/// still-running child, so the child cannot write its report.
fn battery_temp_dir() -> std::path::PathBuf {
    battery_temp_dir_at(std::time::SystemTime::now())
}

/// [`battery_temp_dir`] with an injectable clock, so tests can pin the
/// same-tick collision the counter exists to close.
fn battery_temp_dir_at(now: std::time::SystemTime) -> std::path::PathBuf {
    static RUN_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = RUN_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "bevy_playtest_battery_{}_{}_{}",
        std::process::id(),
        now.duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        seq,
    ))
}

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
/// bot samples the discovered effective keys UNIONED with the
/// source-scan candidates (the probe matrix is bounded by its
/// candidate prior; the scan fills the prior's gaps — see
/// [`crate::discovery::source_scan::scan_extras`]), driving the
/// child with it and recording the full union as
/// `report.injected_surface_keys` (the scan-derived portion is
/// stamped separately in `report.source_derived_keys`). An authored
/// scenario file is never injected into — declaration beats
/// inference; discovery then describes rather than replaces.
pub fn run_with_config(config: &BatteryConfig) -> Result<PlaytestReport, BatteryError> {
    // Unique temp dir (see `battery_temp_dir`): one throwaway root per
    // run, shared by the report, child cwd and any `./`-relative child
    // writes.
    let dir = battery_temp_dir();
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
    // Scan extras unioned into the INJECTED surface (distinct
    // provenance channel from `injected_keys`; stamped onto
    // `report.source_derived_keys` after the run).
    let mut injected_scan_extras: Option<Vec<String>> = None;
    // Decision notes for the INJECTED surface (withheld probe poisons,
    // scan filtering decisions): stamped onto
    // `report.source_derived_notes` after the run — possibly WITHOUT
    // `source_derived_keys`, when a scan applied but contributed no
    // keys (refusal over silence still demands the decisions be
    // reported).
    let mut inject_notes: Option<Vec<String>> = None;
    if flow::inject_requested() && scenario_arg == BUILTIN_BATTERY {
        let outcome = flow::discover_from_env(
            std::path::Path::new(&config.binary),
            Some(config.scenario.as_str()),
        )
        .map_err(|e| BatteryError::Discovery(e.to_string()))?;
        let mut keys = outcome.surface.chaos_keys();
        // Decision notes for this injected surface — withheld
        // probe-verified poison keys, scan filtering decisions.
        // Refusal over silence: every filtering decision the parent
        // made when aiming the bot is reported, never dropped.
        let mut notes: Vec<String> = Vec::new();
        // POISON FILTER, probe tier (Escape/Tab close menus and grab
        // UI focus — a blind chaos bot pressing them aborts the very
        // scene it is supposed to exercise). The scan tier filters
        // poison inside `scan_extras`; before this guard the probe
        // tier had NO such filter, so a probe-VERIFIED Escape could
        // reach the injected bot's surface. Withheld, and the
        // withholding is a REPORTED decision, never a silent drop.
        let (poison_free, withheld_note) = withhold_probe_poison(&keys);
        if let Some(note) = withheld_note {
            notes.push(note);
        }
        keys = poison_free;

        // SCAN ∪ PROBE UNION (tier-order fix): `chaos_keys()` is
        // bounded by the probe matrix's candidate prior, while the
        // source scan reads the game's whole emittable key universe.
        // Injecting the probe surface alone could therefore LOSE scan
        // keys the default battery had (a key textually present in
        // the game's sources but outside the probe prior —
        // thetawave's KeyV is the concrete case). Union the scan
        // extras in: the probe prior stays the EVIDENCE tier
        // (probe-observed effectiveness, Inferred siblings), the
        // scan fills its gaps with candidates under the same
        // poison/cap discipline as the default-battery layering
        // (`scan_extras`). Same applicability as injection itself:
        // only here, only for the undeclared builtin surface.
        let mut scan_applied = false;
        if !source_scan::scan_disabled() {
            if let Some(root) = source_scan::scan_root_from_env() {
                let result = source_scan::scan_from_root(&root);
                let extras = source_scan::scan_extras(&result);
                scan_applied = true;
                // Every filtering decision the scan made (poison,
                // cap, unsupported, unreadable) rides along — the
                // same decisions the default layering reports.
                notes.extend(extras.notes);
                let mut unioned: Vec<String> = Vec::new();
                for key in extras.keys {
                    if !keys.contains(&key) {
                        keys.push(key.clone());
                        unioned.push(key);
                    }
                }
                if !unioned.is_empty() {
                    // Canonical order for the injected surface too.
                    keys.sort();
                    injected_scan_extras = Some(unioned.clone());
                    notes.push(format!(
                        "unioned {} scan keys into the injected (probe-verified) surface: {}",
                        unioned.len(),
                        unioned.join(", ")
                    ));
                }
            }
        }
        if scan_applied && injected_scan_extras.is_none() {
            // A scan ran but contributed nothing beyond the probe
            // surface — still a decision worth reporting (the union
            // considered the scan; it added nothing), never silence.
            notes.push(
                "source scan ran but added no keys beyond the probe-verified surface".to_string(),
            );
        }
        // The injected scenario is the built-in battery with exactly
        // one field added: the chaos bot's key surface — the
        // probe-verified keys UNIONED with scan candidates, i.e.
        // exactly `injected_keys` below. Mouse buttons stay at their
        // default (three primaries): the MVP probe matrix is
        // keyboard-only, so there is no discovered mouse surface to
        // inject — an explicitly empty mouse list would DISABLE mouse
        // chaos, which discovery never licensed.
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
        if !notes.is_empty() {
            inject_notes = Some(notes);
        }
    }

    // Source-derived surface layering (D1.5 slice 5, parent-side).
    // Same precedence shape as injection above, one tier below:
    // an authored scenario file always wins (declaration beats
    // inference). Applicability mirrors injection's: only the
    // built-in battery literal (a scenario file may declare its
    // own surface). When injection RAN, the scan's role is the
    // UNION inside the injected surface (see the inject branch
    // above) — it must never REPLACE the probe-verified keys, and
    // equally the probe prior must never CROWD OUT scan-only keys
    // (the pre-fix tier inversion: a textually-present key outside
    // the probe prior, e.g. thetawave's KeyV, was silently LOST
    // when inject was enabled).
    //
    // Cost: milliseconds (a text pass over the game's `src/`),
    // paid per battery run — no probing, no game execution beyond
    // the run itself. Provenance: report.source_derived_keys +
    // report.source_derived_notes today; emitting schema entries
    // with `ProvenanceGrade::SourceCited` evidence is the planned
    // next step (the scan's hit records already carry file:line).
    let mut source_layered: Option<crate::discovery::source_scan::LayeredKeys> = None;
    if scenario_arg == BUILTIN_BATTERY && !flow::inject_requested() && !source_scan::scan_disabled()
    {
        if let Some(root) = source_scan::scan_root_from_env() {
            let result = source_scan::scan_from_root(&root);
            let layered = source_scan::layered_chaos_keys(&result);
            // SET equality, not order: the scan-hit arm emits
            // alphabetically while the fallback arm keeps backbone
            // order, so a scan citing exactly the backbone 8 must
            // count as "no change". NOTE order is LOAD-BEARING in
            // the driver (the bot indexes keys positionally), so
            // "no change" here really means: the surface stays
            // the DECLARED backbone — no rewrite, no stamp.
            let is_backbone_set = {
                let mut scan_sorted = layered.keys.clone();
                scan_sorted.sort();
                scan_sorted == backbone_keys_vec_sorted()
            };
            if !is_backbone_set {
                // The scan REPLACED the blind default with its own
                // poison-filtered surface: rewrite the scenario the
                // child receives, same pattern as injection (the
                // parent writes a variant file). A set-equal
                // surface (empty scan or exactly the backbone)
                // leaves the declared default surface untouched
                // and stays
                // unstamped, preserving the None honesty contract.
                let mut scenario = crate::plugin::battery_scenario().map_err(|e| {
                    BatteryError::Spawn(format!("source-layered battery scenario: {e}"))
                })?;
                scenario.bot.raw_surface = Some(crate::scenario::RawSurface {
                    keys: layered.keys.clone(),
                    ..Default::default()
                });
                let layered_path = dir.join("scenario.source_layered.json");
                let json = serde_json::to_string_pretty(&scenario).map_err(|e| {
                    BatteryError::Spawn(format!("serialize source-layered scenario: {e}"))
                })?;
                std::fs::write(&layered_path, json).map_err(|e| {
                    BatteryError::Spawn(format!("write source-layered scenario: {e}"))
                })?;
                scenario_arg = layered_path.to_string_lossy().into_owned();
                source_layered = Some(layered);
            }
        }
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
    // Source-scan honesty fields: stamped whenever the parent put
    // scan-derived keys into the run's surface (independent of
    // discovery). For a DEFAULT battery that is the scan-derived
    // surface that REPLACED the WASD/arrows default (Some ⇒ the
    // scan-hit arm fired; the fallback arm leaves None — no
    // scan, scan-empty, or backbone-equal scan, all of which run
    // the unchanged default); for an INJECTED run it is exactly
    // the scan keys unioned into the probe-verified surface (the
    // full bot surface lives in `injected_surface_keys` — the two
    // channels stay distinct so provenance is never coalesced).
    if let Some(layered) = source_layered {
        report.source_derived_keys = Some(layered.keys);
        report.source_derived_notes = Some(layered.notes);
    }
    if let Some(extras) = &injected_scan_extras {
        report.source_derived_keys = Some(extras.clone());
    }
    if let Some(notes) = inject_notes {
        report.source_derived_notes = Some(notes);
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

/// Golden-verdict twin of [`run_and_expect_clean`]: run one scenario
/// and assert the report carries EXACTLY the expected findings —
/// the same rule on the same target with a detail containing the
/// given needle — and nothing else. For a game whose honest verdict
/// under the battery IS findings (e.g. a stock demo exposing no
/// verifiable progress surface), pinning the exact verdict turns CI
/// red into a regression guard: the finding is expected, so it must
/// stay present AND stay alone — a NEW finding (a regression or an
/// oracle misfire) fails the pin, and so does the pinned finding
/// changing shape beyond its needle.
///
/// `expected` items are `(rule, target, detail_needle)` triples; the
/// needle must appear in the entry's detail (a short stable phrase —
/// full details embed frame counts and churn statistics that vary
/// with machine timing). Panic messages name every difference.
/// Returns the report on success so callers can pin additional
/// fields (e.g. `source_derived_keys`).
pub fn run_and_expect_findings(
    binary: &str,
    scenario: &str,
    expected: &[(&str, &str, &str)],
) -> crate::state::PlaytestReport {
    let report = match run(binary, scenario) {
        Ok(report) => report,
        Err(e) => panic!("{e}"),
    };
    let mut problems: Vec<String> = Vec::new();
    for (rule, target, needle) in expected {
        let needle_desc = format!("{rule}/{target} (detail contains {needle:?})");
        match report
            .violations
            .iter()
            .find(|v| v.rule == *rule && v.target == *target)
        {
            Some(entry) => {
                if !entry.detail.contains(needle) {
                    problems.push(format!(
                        "expected finding {needle_desc} has detail:\n  {}",
                        entry.detail
                    ));
                }
            }
            None => problems.push(format!("expected finding {needle_desc} is missing")),
        }
    }
    let expected_pairs: std::collections::BTreeSet<(&str, &str)> =
        expected.iter().map(|(r, t, _)| (*r, *t)).collect();
    for v in &report.violations {
        if !expected_pairs.contains(&(v.rule.as_str(), v.target.as_str())) {
            problems.push(format!(
                "UNEXPECTED finding {} (target: {}): {}",
                v.rule, v.target, v.detail
            ));
        }
    }
    if !problems.is_empty() {
        panic!(
            "golden verdict mismatch:\n  {}\n{}",
            problems.join("\n  "),
            report.findings_summary(scenario)
        );
    }
    report
}

/// A declared expectation that one specific, scoped finding will be
/// present in the battery report. This is NOT an allowlist: an
/// expectation is a predicate that consumes at most ONE matching
/// violation, and the run still fails on (a) any violation not
/// covered by a declared expectation, (b) any expectation that found
/// no matching violation (so an expectation can never silently rot
/// into covering nothing — if the game stops exhibiting the finding,
/// the test goes red until the expectation is removed), and (c) a
/// `pass` report with expectations declared (the declared finding
/// did not occur).
///
/// Occurrence counts are part of the contract: the tracker dedups
/// violations by `(rule, target)` and bumps `count` on recurrence, so
/// an entry's `count` says how many times the finding occurred. An
/// expectation only consumes an entry whose `count` equals the
/// declared [`ExpectFinding::times`] (default 1) — a merged entry
/// covering MORE occurrences than declared (e.g. the adjudicated
/// menu freeze plus a later mid-game wedge of the same rule/target)
/// is left uncovered and fails the run loudly.
///
/// Scope beyond the rule name is mandatory for discriminating uses:
/// e.g. expecting the menu-idle `frozen_world` that fires at the end
/// of the liveness window after onset-at-frame-0 must NOT swallow a
/// mid-game wedge, which fires much later (if it can fire at all in
/// the run window).
#[derive(Debug, Clone)]
pub struct ExpectFinding {
    rule: crate::rules::Rule,
    target: Option<String>,
    /// Inclusive window the violation's `first_frame` must lie in.
    first_frame_in: (u64, u64),
    /// Occurrence count (`ViolationEntry::count`) the matched entry
    /// must have EXACTLY — one-for-one consumption, occurrence-level.
    times: u64,
    /// Why this expectation exists — mandatory (see [`Self::new`]),
    /// lands in failure messages, so the claim is reviewable rather
    /// than a magic incantation.
    justification: String,
}

impl ExpectFinding {
    /// Expect a violation of `rule` (rule + target scoped by default;
    /// add `.first_frame_in` for phase discrimination).
    ///
    /// `justification` is a mandatory reviewable reason: it appears
    /// verbatim in failure messages. An expectation is a published
    /// claim about the game's behavior; it must say why. Empty or
    /// whitespace-only justifications are rejected.
    pub fn new(rule: crate::rules::Rule, justification: impl Into<String>) -> Self {
        let justification = justification.into();
        assert!(
            !justification.trim().is_empty(),
            "ExpectFinding::new: a justification is mandatory — say WHY the \
             finding is expected (it appears verbatim in failure messages)"
        );
        Self {
            rule,
            target: None,
            first_frame_in: (0, u64::MAX),
            times: 1,
            justification,
        }
    }

    /// Require the violation's `target` to equal `target` exactly.
    pub fn target(mut self, target: impl Into<String>) -> Self {
        self.target = Some(target.into());
        self
    }

    /// Require the violation's `first_frame` to lie in `range`
    /// (inclusive) — the phase discriminator.
    pub fn first_frame_in(mut self, range: std::ops::RangeInclusive<u64>) -> Self {
        self.first_frame_in = (*range.start(), *range.end());
        self
    }

    /// Declare that the finding occurred exactly `n` times. The
    /// matched entry's occurrence `count` must EQUAL `n` — not "at
    /// least". Default 1. More occurrences than declared are never
    /// absorbed; fewer are an unsatisfied expectation. Values below
    /// 1 are clamped to 1: this API expresses presence, never
    /// absence.
    pub fn times(mut self, n: u64) -> Self {
        self.times = n.max(1);
        self
    }

    fn matches(&self, v: &crate::state::ViolationEntry) -> bool {
        self.rule.name() == v.rule
            && self.target.as_deref().is_none_or(|t| t == v.target)
            && v.first_frame >= self.first_frame_in.0
            && v.first_frame <= self.first_frame_in.1
            && v.count == self.times
    }

    fn describe(&self) -> String {
        let window = if self.first_frame_in == (0, u64::MAX) {
            "<any>".to_string()
        } else {
            format!("{}..={}", self.first_frame_in.0, self.first_frame_in.1)
        };
        format!(
            "{} on target {:?} firing in frames {}, occurring {} time(s) ({})",
            self.rule.name(),
            self.target.as_deref().unwrap_or("<any>"),
            window,
            self.times,
            self.justification,
        )
    }
}

/// Like [`run_and_expect_clean`], but with declared expected
/// findings. Each expectation must consume exactly one matching
/// violation entry — occurrence-level one-for-one: the entry's
/// `count` must equal the expectation's declared `times` (default
/// 1), so more recurrences than declared are left uncovered and
/// fail. Any uncovered violation still fails the run, any
/// unsatisfied expectation fails it too, and a `crash` status fails
/// it outright regardless of matching. Note the difference from
/// [`run_and_expect_clean`]: informational findings (warnings,
/// capture notes) also fail unless covered by an expectation.
pub fn run_and_expect_scoped_findings(binary: &str, scenario: &str, expects: &[ExpectFinding]) {
    let report = match run(binary, scenario) {
        Ok(report) => report,
        Err(e) => panic!("{e}"),
    };
    let mut problems: Vec<String> = Vec::new();
    if report.status == crate::enums::PlaytestStatus::Crash {
        // A crashed run is never a satisfied one, however the
        // violation bookkeeping shook out.
        problems.push(format!(
            "RUN CRASHED (status: {}) — expected findings cannot certify a crashed run",
            report.status
        ));
    }
    // Greedy one-for-one consumption. Expectations are meant to be
    // few and non-overlapping (each describes one adjudicated
    // finding); ambiguity itself is the error to fix.
    let mut covered = vec![false; report.violations.len()];
    let mut unsatisfied: Vec<&ExpectFinding> = Vec::new();
    for exp in expects {
        match report
            .violations
            .iter()
            .zip(covered.iter_mut())
            .find(|(v, used)| !**used && exp.matches(v))
        {
            Some((_, slot)) => *slot = true,
            None => {
                problems.push(format!("expected finding NOT observed: {}", exp.describe()));
                unsatisfied.push(exp);
            }
        }
    }
    for exp in &unsatisfied {
        // Diagnose sibling competition: an expectation can miss
        // because an EARLIER, looser one consumed its entry. Bind
        // target too, so a genuinely-absent finding on a DIFFERENT
        // target isn't misattributed to scope overlap.
        let stolen = report
            .violations
            .iter()
            .zip(covered.iter())
            .any(|(v, used)| {
                *used
                    && v.rule == exp.rule.name()
                    && exp.target.as_deref().is_none_or(|t| t == v.target)
            });
        if stolen {
            problems.push(format!(
                "possible cause: another declared expectation consumed an entry with \
                 rule {} — check declaration order for overlapping scopes",
                exp.rule.name()
            ));
        }
    }
    for (v, used) in report.violations.iter().zip(covered.iter()) {
        if !used {
            problems.push(format!(
                "UNEXPECTED violation: [{}] {} (target: {}, frames {}-{}, x{}): {}",
                report.status, v.rule, v.target, v.first_frame, v.last_frame, v.count, v.detail,
            ));
        }
    }
    if !problems.is_empty() {
        let mut msg = String::from("playtest findings diverge from declared expectations:\n");
        for p in &problems {
            msg.push_str(&format!("  - {p}\n"));
        }
        msg.push_str(&format!(
            "  report status: {}\n  reproduce: BEVY_PLAYTEST_SCENARIO={} cargo run --features playtest --bin <game>\n",
            report.status, scenario
        ));
        panic!("{}", msg.trim_end());
    }
}

/// Withhold poison keys ([`crate::discovery::source_scan::POISON_KEYS`])
/// from a probe-verified injectable surface, returning the filtered
/// list plus the refusal note naming what was withheld (refusal over
/// silence: `None` means nothing was withheld — no decision, no
/// note). Pure function: the inject path is otherwise unreachable
/// from unit tests (it needs a real probe run over a game binary).
pub fn withhold_probe_poison(keys: &[String]) -> (Vec<String>, Option<String>) {
    let withheld: Vec<String> = keys
        .iter()
        .filter(|k| source_scan::POISON_KEYS.contains(&k.as_str()))
        .cloned()
        .collect();
    let note = if withheld.is_empty() {
        None
    } else {
        Some(format!(
            "probe-verified poison keys withheld from the injected surface: {}",
            withheld.join(", ")
        ))
    };
    let kept: Vec<String> = keys
        .iter()
        .filter(|k| !source_scan::POISON_KEYS.contains(&k.as_str()))
        .cloned()
        .collect();
    (kept, note)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_poison_is_withheld_and_reported_by_name() {
        // A probe-VERIFIED Escape/Tab must never reach the injected
        // blind bot's surface (frozen rule: poison never reaches the
        // chaos bot), and the withholding is reported, naming the
        // keys — never a silent drop.
        let keys = vec![
            "Digit1".to_string(),
            "Escape".to_string(),
            "KeyV".to_string(),
            "Tab".to_string(),
        ];
        let (kept, note) = withhold_probe_poison(&keys);
        assert_eq!(
            kept,
            vec!["Digit1".to_string(), "KeyV".to_string()],
            "poison keys must be withheld from the injected surface"
        );
        let note = note.expect("withholding must be reported (refusal over silence)");
        assert!(
            note.contains("Escape") && note.contains("Tab"),
            "the note must name the withheld keys: {note}"
        );
    }

    #[test]
    fn no_poison_no_note() {
        // Nothing withheld => no decision => no note (a note here
        // would LIE about a withholding that did not happen).
        let (kept, note) = withhold_probe_poison(&["Digit1".to_string(), "KeyV".to_string()]);
        assert_eq!(kept, vec!["Digit1".to_string(), "KeyV".to_string()]);
        assert_eq!(note, None, "no withholding happened — stay silent");
    }

    #[test]
    fn battery_temp_dirs_are_unique_per_call() {
        // Regression: two same-tick calls (parallel #[test]s in one
        // test binary share the pid AND can share a clock tick) must
        // still get distinct directories — a shared dir means one
        // run's cleanup guard deletes the other's report mid-write.
        // Freezing the clock pins the exact collision the original
        // pid+nanos naming suffered; only the counter separates them.
        let frozen = std::time::SystemTime::now();
        let a = battery_temp_dir_at(frozen);
        let b = battery_temp_dir_at(frozen);
        assert_ne!(a, b, "same-tick calls must never reuse a dir");
        // And the separator really is the counter: every path ends
        // in a strictly increasing seq component.
        let c = battery_temp_dir_at(frozen);
        assert_ne!(a, c);
        assert_ne!(b, c);
    }

    fn frozen_entry(first_frame: u64) -> crate::state::ViolationEntry {
        crate::state::ViolationEntry {
            rule: "frozen_world".to_string(),
            target: "world".to_string(),
            first_frame,
            last_frame: first_frame,
            count: 1,
            detail: "no Transform or UI change for N frames — soft-lock?".to_string(),
            last_detail: "no Transform or UI change for N frames — soft-lock?".to_string(),
            meta: None,
        }
    }

    #[test]
    fn expect_finding_scopes_discriminate_phases() {
        use crate::rules::Rule;
        // The menu-idle expectation: frozen_world fires at
        // onset + grace (20 s battery, 15 s liveness @ 60 tps ⇒
        // ~901 frames). A TIGHT window around the minimum possible
        // fire-frame admits only freezes beginning within ~20 frames
        // of startup — the menu phase.
        let menu_idle = ExpectFinding::new(
            Rule::FrozenWorld,
            "menu is an idle world; escape needs a lucky New Game click",
        )
        .target("world")
        .first_frame_in(880..=920);
        // A mid-game wedge (onset ~100) fires at ~1001 — the menu
        // expectation must NOT absorb it, so the run stays red.
        let midgame_wedge = frozen_entry(1001);
        assert!(
            !menu_idle.matches(&midgame_wedge),
            "a menu-scoped first_frame_in window must not absorb a mid-run freeze"
        );
        // The actual menu finding (onset 0) fires at ~901: matches.
        let menu_freeze = frozen_entry(901);
        assert!(menu_idle.matches(&menu_freeze));
        // Default (no window): any first_frame matches, but rule +
        // target still bind.
        let loose = ExpectFinding::new(Rule::FrozenWorld, "why");
        assert!(loose.matches(&midgame_wedge));
        assert!(
            !loose.matches(&{
                let mut v = frozen_entry(901);
                v.rule = "input_sink".to_string();
                v
            }),
            "a different rule never matches"
        );
        let targeted = ExpectFinding::new(Rule::FrozenWorld, "why").target("camera");
        assert!(
            !targeted.matches(&menu_freeze),
            "a different target never matches"
        );
    }

    #[test]
    fn expect_finding_occurrence_count_is_part_of_the_contract() {
        use crate::rules::Rule;
        // The tracker merges recurrences of the same (rule, target)
        // into ONE entry with count = occurrences (state.rs dedup).
        // The reviewer's wedge-hiding attack: menu freeze + a later
        // mid-game freeze merge to count=2; a default (times=1)
        // expectation must REFUSE the merged entry — the wedge stays
        // uncovered and the run stays red.
        let menu_only = ExpectFinding::new(
            Rule::FrozenWorld,
            "menu idle world; escape needs a lucky New Game click",
        )
        .target("world")
        .first_frame_in(880..=920);
        let mut merged = frozen_entry(901);
        merged.count = 2; // menu freeze at 901 + wedge at ~1000+
        merged.last_frame = 1000;
        assert!(
            !menu_only.matches(&merged),
            "a times=1 expectation must not absorb a count=2 merged entry (wedge hiding)"
        );
        // Declaring .times(2) consumes the merged entry — the caller
        // has to SAY the finding occurred twice.
        let declared_twice = menu_only.clone().times(2);
        assert!(declared_twice.matches(&merged));
        assert!(
            !declared_twice.matches(&frozen_entry(901)),
            "times=2 must not absorb a single occurrence either — exact equality"
        );
    }

    #[test]
    fn expect_finding_describe_carries_the_claim() {
        use crate::rules::Rule;
        let exp = ExpectFinding::new(
            Rule::FrozenWorld,
            "adjudicated true positive (see FIXTURES.md)",
        )
        .target("world")
        .first_frame_in(0..=1000);
        let d = exp.describe();
        assert!(d.contains("frozen_world"), "{d}");
        assert!(d.contains("0..=1000"), "{d}");
        assert!(d.contains("adjudicated true positive"), "{d}");
        // The full-range default window renders as <any>, not
        // 0..=u64::MAX.
        let loose = ExpectFinding::new(Rule::FrozenWorld, "why");
        assert!(loose.describe().contains("<any>"), "{}", loose.describe());
        assert!(
            !loose.describe().contains("18446744073709551615"),
            "{}",
            loose.describe()
        );
    }
}
