//! One-line game integration: `PlaytestPlugin`.
//!
//! Add the plugin to your `App` (AFTER any plugin group such as
//! `DefaultPlugins`). When `BEVY_PLAYTEST_SCENARIO=<path>` is set at
//! process start, the plugin replaces the app's runner: the scenario
//! JSON is loaded, `run_scenario` drives the (already-built) app to
//! completion, the report is written to `BEVY_PLAYTEST_REPORT`
//! (default: scenario path + `.report.json`), the path is printed, and
//! the process exits with:
//!
//! - 0: Pass
//! - 1: Fail (gating violations)
//! - 2: Crash
//! - 3: harness/config error (unreadable scenario, invalid env value)
//!
//! When the env var is absent the plugin is inert: no runner swap, no
//! resources, no systems, no env reads past the one `var_os` check.
//!
//! With [`ENV_LINT`] also set, the takeover
//! LINTS the scenario instead of running it: every config error is
//! collected and reported at once (never first-error-only), the
//! report is written, and the process exits 0 (coherent config) or
//! 1 (findings) — see [`ENV_LINT`] for the mode's honest scope.
//!
//! # Headless by construction
//!
//! `set_runner` is last-set-wins: adding `PlaytestPlugin` after
//! `DefaultPlugins` overwrites winit's runner with the takeover runner,
//! and window creation (which happens inside winit's runner) never
//! executes. A stock `DefaultPlugins` game therefore runs headless
//! under takeover with no plugin-group swap and no window.
//!
//! # Ordering responsibility (no runtime guard)
//!
//! Plugin ordering is the user's responsibility, enforced by contract
//! rather than runtime detection: if `PlaytestPlugin` is added BEFORE
//! a group that sets a runner (e.g. `DefaultPlugins`), the group's
//! runner wins and the takeover is silently inert. No cheap reliable
//! runtime detection exists at plugin-build time (the competing runner
//! is not yet installed), so we document the requirement instead of
//! shipping a phantom guard. [`PlaytestDefaultPlugins`] remains as
//! optional sugar for an explicitly headless plugin graph.
//!
//! Unknown `BEVY_PLAYTEST_*` variables are rejected with exit code 3:
//! the scenario schema is strict (`deny_unknown_fields`), and env
//! strictness follows the same doctrine — a typo'd variable must never
//! be mistaken for a configured option.

use bevy_app::prelude::*;

/// Env var holding the scenario path. Absent => plugin inert.
/// Special literals: `"smoke"` (built-in smoke scenario) and
/// `"battery"` (built-in default battery — the zero-config verdict
/// run: single-seed chaos + all oracles).
pub const ENV_SCENARIO: &str = "BEVY_PLAYTEST_SCENARIO";
/// Optional report output path (default: `<scenario>.report.json`).
pub const ENV_REPORT: &str = "BEVY_PLAYTEST_REPORT";
/// Set: assert headlessness even without a scenario. When set, the
/// plugin (added after DefaultPlugins) replaces winit's runner with a
/// schedule loop so no window opens and no display is needed.
pub const ENV_HEADLESS: &str = "BEVY_PLAYTEST_HEADLESS";
/// Optional static input-surface scan export path (takeover runs):
/// when set, the takeover writes the serialized
/// [`crate::discovery::static_scan::StaticScanResult`] — captured by
/// [`crate::driver::run_scenario`] BEFORE the harness installs its
/// bot and oracle systems, so the counts describe the GAME, not the
/// harness — to this path after the scenario completes. Set by the
/// discovery probe pool on its first control run so the parent-side
/// flow can merge real reader data into the surface (fail-open: a
/// missing/failed export never fails the run).
pub const ENV_DISCOVER_SCAN: &str = "BEVY_PLAYTEST_DISCOVER_SCAN";
/// Optional seed override (takeover runs): when set, overrides the
/// loaded scenario's `bot.seed` — including the built-in literals —
/// AFTER loading, so it composes with scan-layered scenario
/// variants. Set by the battery parent
/// ([`crate::battery::BatteryConfig::seed_override`], driven by the
/// seed sweep) and by hand for a quick single-run approximation:
/// `BEVY_PLAYTEST_SCENARIO=battery BEVY_PLAYTEST_SEED=<seed> cargo
/// run --features playtest --bin <game>` (without the battery's
/// source-scan key layering and deterministic OS randomness; the seed
/// var alone is inert — the takeover needs its scenario).
pub const ENV_SEED: &str = "BEVY_PLAYTEST_SEED";
/// Lint mode (takeover): when set alongside [`ENV_SCENARIO`], the
/// takeover runs the scenario linter INSTEAD of driving the game:
/// every config error is collected and reported at once (aggregate,
/// never first-error-only), followed by a type-registry probe of the
/// booted App (one warm-up frame) that catches definite resolution
/// errors — typo'd resource/component names, paths that cannot reach
/// a numeric leaf — before the process exits without running the
/// frame loop. The probe is silent about what it cannot confirm
/// (a component with no instances yet). A clean lint means "the
/// config is coherent", never "the game will pass": frame-level
/// judgments are out of scope. Alone (no scenario) the variable is
/// inert, like [`ENV_SEED`].
pub const ENV_LINT: &str = "BEVY_PLAYTEST_LINT";
pub use crate::sinks::ENV_ALLOW_MISSING_LOG_CAPTURE;
/// Seed for the deterministic-randomness shim the battery preloads into
/// the game process (Linux). Set by the battery parent; read by the shim
/// (`detrng/shim.rs`), never by the takeover itself.
pub const ENV_DETRNG_SEED: &str = "BEVY_PLAYTEST_DETRNG_SEED";

/// Opt-out for the deterministic-randomness shim, read by the battery
/// parent. Listed as known so an inherited value is never rejected.
pub const ENV_DETRNG_DISABLE: &str = "BEVY_PLAYTEST_DETRNG_DISABLE";

const KNOWN_ENV_VARS: [&str; 12] = [
    ENV_SCENARIO,
    ENV_REPORT,
    ENV_HEADLESS,
    ENV_REPLAY_PASS_B,
    ENV_REPLAY_PASS_B_DIGEST,
    ENV_ALLOW_MISSING_LOG_CAPTURE,
    ENV_DISCOVER_SCAN,
    ENV_DETRNG_SEED,
    // Battery-parent opt-out for the shim; harmless if inherited.
    ENV_DETRNG_DISABLE,
    // Test-selection var (set by the acceptance suite / CI matrix to
    // pick one fixture); inherited by the game process harmlessly.
    "BEVY_PLAYTEST_FIXTURE",
    ENV_SEED,
    // Lint mode; inert without ENV_SCENARIO.
    ENV_LINT,
];

/// Exit codes for the takeover process contract.
pub mod exit_code {
    pub const PASS: i32 = 0;
    pub const FAIL: i32 = 1;
    pub const CRASH: i32 = 2;
    pub const CONFIG_ERROR: i32 = 3;
}

/// One-line integration plugin.
///
/// Inert unless [`ENV_SCENARIO`] is set; see the module docs.
#[derive(Default)]
pub struct PlaytestPlugin;

impl Plugin for PlaytestPlugin {
    fn build(&self, app: &mut App) {
        // Log-capture preflight: this plugin is contracted to be added
        // AFTER DefaultPlugins, so if the game uses DefaultPlugins its
        // LogPlugin has already claimed the write-once global tracing
        // default — log_error/log_warn findings would be silently
        // under-reported for the whole process. Detect NOW (plugin
        // presence is reliably queryable at this point) and warn with
        // remediation. Not fatal: headless games never capture
        // logs anyway, and ENV_ALLOW_MISSING_LOG_CAPTURE acknowledges
        // the loss deliberately.
        if std::env::var_os(ENV_SCENARIO).is_some()
            && app.is_plugin_added::<bevy::log::LogPlugin>()
            && std::env::var_os(ENV_ALLOW_MISSING_LOG_CAPTURE).is_none()
        {
            eprintln!(
                "bevy_playtest: LogPlugin was added before PlaytestPlugin — its \
                 tracing subscriber wins the global default and log capture will \
                 be OFF for this process. Use PlaytestDefaultPlugins (drops \
                 LogPlugin under a scenario run) or acknowledge with \
                 {}=1.",
                ENV_ALLOW_MISSING_LOG_CAPTURE
            );
        }

        match read_env_config() {
            Ok(Takeover::Inert) => {}
            Ok(Takeover::HeadlessAssert) => assert_headless(app),
            Ok(Takeover::Run(config)) => take_over(app, config),
            Err(err) => {
                eprintln!("bevy_playtest: {err}");
                std::process::exit(exit_code::CONFIG_ERROR);
            }
        }
    }
}

/// Env-derived takeover decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Takeover {
    /// No env vars set: the plugin does nothing.
    Inert,
    /// Only [`ENV_HEADLESS`] set: assert headlessness, then run the app
    /// normally. The assertion must not degrade to a silent windowed
    /// run, and must not hijack the app either.
    HeadlessAssert,
    /// [`ENV_SCENARIO`] set: full takeover.
    Run(TakeoverConfig),
}

/// Env-derived takeover configuration (full takeover only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TakeoverConfig {
    pub scenario_path: String,
    pub report_path: String,
}

/// Per-entity opt-out honored by `derived_transform_consistency` in
/// ANY selection mode: attach it once to an entity that legitimately
/// writes its `GlobalTransform` after transform propagation (camera
/// smoothing ghosts, interpolation layers) and the rule ignores that
/// entity, scoped to that rule alone — no other enumeration or
/// oracle consults it. Reflect-registered under takeover so the
/// selector/exclude grammar can coherently co-name it, but never a
/// requirement: the component is plain ECS, usable whether or not
/// this game reflects its own types.
///
/// Defined in `invariant_eval` (always compiled) because the
/// consistency recorder — built under every feature combination,
/// including in-process builds with no `plugin` module — resolves
/// its component ID to honor the marker. Re-exported here under the
/// original path so consumer code and docs are unchanged.
pub use crate::invariant_eval::SkipDerivedConsistency;

fn read_env_config() -> Result<Takeover, String> {
    for (key, _) in std::env::vars_os() {
        let Some(key) = key.to_str() else { continue };
        if key.starts_with("BEVY_PLAYTEST_") && !KNOWN_ENV_VARS.contains(&key) {
            return Err(format!(
                "unknown environment variable '{key}' (known: {KNOWN_ENV_VARS:?})"
            ));
        }
    }
    let scenario_path = match std::env::var_os(ENV_SCENARIO) {
        Some(path) => path.to_string_lossy().into_owned(),
        None => {
            return Ok(if std::env::var_os(ENV_HEADLESS).is_some() {
                Takeover::HeadlessAssert
            } else {
                Takeover::Inert
            })
        }
    };
    if scenario_path.trim().is_empty() {
        return Err(format!("{ENV_SCENARIO} is empty"));
    }
    let report_path = std::env::var_os(ENV_REPORT)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| {
            if scenario_path == SMOKE_LITERAL {
                "bevy_playtest-smoke.report.json".to_string()
            } else if scenario_path == BATTERY_LITERAL {
                "bevy_playtest-battery.report.json".to_string()
            } else {
                format!("{scenario_path}.report.json")
            }
        });
    Ok(Takeover::Run(TakeoverConfig {
        scenario_path,
        report_path,
    }))
}

/// Replace the app's runner with the playtest takeover.
///
/// [`run_scenario`] owns the tick loop, so the takeover replaces
/// `App::run` semantics entirely rather than running as a system
/// inside the app's own schedule: winit's (or any other) runner is
/// swapped out before it can drive a single frame, the scenario drains
/// to completion under `run_scenario`'s loop, then the process exits.
/// Headless-assert contract: with only [`ENV_HEADLESS`] set there is
/// no runner swap, so a `WinitPlugin` present at build time means the
/// app WILL open a window. Refuse loudly instead (the documented
/// correct plugin order — groups before PlaytestPlugin — is exactly
/// when this detection is reliable).
#[cfg(feature = "takeover-winit")]
fn assert_headless(app: &App) {
    if app.is_plugin_added::<bevy_winit::WinitPlugin>() {
        eprintln!(
            "bevy_playtest: {} is set but WinitPlugin is present and no \
             BEVY_PLAYTEST_SCENARIO is set to swap the runner — a window \
             would open. Remove WinitPlugin or set a scenario.",
            ENV_HEADLESS
        );
        std::process::exit(exit_code::CONFIG_ERROR);
    }
}

#[cfg(not(feature = "takeover-winit"))]
fn assert_headless(_app: &App) {}

fn take_over(app: &mut App, config: TakeoverConfig) {
    // Register the derived-consistency opt-out marker: like the other
    // takeover-only mutations below, the game world only pays for
    // harness vocabulary when a harness run is actually happening —
    // the inert plugin adds no registry entries. Registration (not
    // just definition) keeps the type resolvable in the registry the
    // lint probe and the consistency recorder share.
    app.register_type::<SkipDerivedConsistency>();
    // Silence game audio for the run. Audio keeps working (sinks are
    // still created and played, so games that inspect their audio
    // behave as usual); only the output volume is zero. Bevy multiplies
    // every sink's volume by `GlobalVolume` when it creates the sink, and
    // this plugin is added after DefaultPlugins, so this overrides the
    // value AudioPlugin inserted.
    app.insert_resource(bevy::audio::GlobalVolume::new(bevy::audio::Volume::SILENT));
    app.set_runner(move |mut app: App| -> AppExit {
        run_takeover(&mut app, &config);
    });
}

fn run_takeover(app: &mut App, config: &TakeoverConfig) -> ! {
    let (mut scenario, source) = if config.scenario_path == SMOKE_LITERAL {
        (
            match smoke_scenario() {
                Ok(s) => s,
                Err(err) => {
                    eprintln!("bevy_playtest: {err}");
                    std::process::exit(exit_code::CONFIG_ERROR);
                }
            },
            "builtin_smoke",
        )
    } else if config.scenario_path == BATTERY_LITERAL {
        (
            match battery_scenario() {
                Ok(s) => s,
                Err(err) => {
                    eprintln!("bevy_playtest: {err}");
                    std::process::exit(exit_code::CONFIG_ERROR);
                }
            },
            "builtin_battery",
        )
    } else {
        (
            match load_scenario(&config.scenario_path) {
                Ok(scenario) => scenario,
                Err(err) => {
                    eprintln!("bevy_playtest: {err}");
                    std::process::exit(exit_code::CONFIG_ERROR);
                }
            },
            "file",
        )
    };
    // Seed override: applied AFTER scenario loading and BEFORE the
    // pass/replay dispatch so it covers every branch the takeover
    // can take — builtin literals, file scenarios and
    // scan-layered variants alike. Pass B (replay) is a fresh child
    // process re-running the same scenario env, so it inherits the
    // override from the environment automatically. A non-u64 value
    // is a config error, not a warning — this is a REPRO knob, and
    // a typo'd seed that silently runs the declared seed instead is
    // the worst possible failure mode (the operator believes they
    // reproduced a member they did not). Matches the strict env
    // contract: a typo'd setting must never be mistaken for a
    // configured option.
    if let Some(value) = std::env::var_os(ENV_SEED) {
        match value.to_string_lossy().parse::<u64>() {
            Ok(seed) => scenario.bot.seed = seed,
            Err(_) => {
                eprintln!(
                    "bevy_playtest: {ENV_SEED} is not a u64 ('{}')",
                    value.to_string_lossy()
                );
                std::process::exit(exit_code::CONFIG_ERROR);
            }
        }
    }
    // Lint mode intercept: with BEVY_PLAYTEST_LINT set, aggregate
    // every scenario error and exit — never reaching the frame loop.
    // Intercept BEFORE the verify_replay/run_scenario dispatch so
    // lint covers replay scenarios too.
    if std::env::var_os(ENV_LINT).is_some() {
        run_lint(app, &scenario, &config.report_path, source);
    }
    // Replay verification needs TWO freshly built Apps. The takeover
    // already owns process lifetime and `set_runner` hands us the App
    // after plugin-build; a true rebuild requires re-running the
    // game's construction, which only the game's own entry point can
    // do. Mechanism chosen: pass B re-runs the game executable as a
    // child process with the same scenario env and a marker variable;
    // the child runs one pass and writes its fingerprint series to a
    // file, which the parent compares against pass A. This is a true
    // rebuild — no mutable runtime state is shared between passes.
    let mut report = if scenario.verify_replay {
        match verify_replay_subprocess(app, &scenario, &config.report_path) {
            Ok(report) => report,
            Err(err) => {
                eprintln!("bevy_playtest: {err}");
                std::process::exit(exit_code::CONFIG_ERROR);
            }
        }
    } else {
        match crate::driver::run_scenario(app, &scenario) {
            Ok(report) => report,
            Err(err) => {
                eprintln!("bevy_playtest: {err}");
                // Fail-fast is kept, but if the error was a scenario
                // VALIDATION rejection, point the author at lint mode:
                // it collects every rejection in one pass instead of
                // one per run. A validation rejection is exactly a
                // first element of the lint list (same walker), so the
                // identity check can never fire on unrelated errors
                // (run-guard, infra).
                if crate::scenario::lint_scenario(&scenario).first() == Some(&err) {
                    eprintln!(
                        "bevy_playtest: run with {}=1 to see all scenario \
                         errors at once",
                        ENV_LINT
                    );
                }
                std::process::exit(exit_code::CONFIG_ERROR);
            }
        }
    };
    report.scenario_source = Some(source.to_string());
    match write_report(&config.report_path, &report) {
        Ok(()) => {}
        Err(err) => {
            eprintln!("bevy_playtest: {err}");
            std::process::exit(exit_code::CONFIG_ERROR);
        }
    }
    // Opt-in static input-surface scan export (set by the discovery
    // probe pool on its first control run). The scan was CAPTURED by
    // `run_scenario` BEFORE the harness registered its bot and oracle
    // systems: the bots write `KeyboardInput`/`MouseButtonInput`/
    // `WindowEvent`, and writes count as observing — a scan taken
    // after the scenario would attribute those harness WRITES to the
    // game and inflate every device class with phantom readers. The
    // captured result was parked OUTSIDE the world (a resource would
    // shift the digest's entity/archetype counts and break the
    // byte-identity the discovery diff needs between the exporting
    // control run and every other run); drain it and export verbatim.
    // Fail-open by contract: an export failure costs the parent its
    // reader annotations, never the run its report (the flow's
    // fail-open note covers the absence).
    if let Some(scan_path) = std::env::var_os(ENV_DISCOVER_SCAN) {
        match crate::discovery::static_scan::take_captured_scan() {
            Some(scan) => match serde_json::to_vec(&scan) {
                Ok(bytes) => {
                    if let Err(err) = std::fs::write(&scan_path, bytes) {
                        eprintln!(
                            "bevy_playtest: static scan export failed ('{}'): {err}",
                            scan_path.to_string_lossy()
                        );
                    }
                }
                Err(err) => {
                    eprintln!("bevy_playtest: static scan serialization failed: {err}")
                }
            },
            None => {
                eprintln!("bevy_playtest: static scan export skipped (no captured scan was parked)")
            }
        }
    }
    println!("bevy_playtest: report written to {}", config.report_path);
    let code = match report.status {
        crate::enums::PlaytestStatus::Pass => exit_code::PASS,
        crate::enums::PlaytestStatus::Fail => exit_code::FAIL,
        crate::enums::PlaytestStatus::Crash => exit_code::CRASH,
    };
    std::process::exit(code)
}

/// Run the scenario linter instead of the frame loop:
/// `BEVY_PLAYTEST_LINT=1 BEVY_PLAYTEST_SCENARIO=... <game>` collects
/// every scenario validation error at once (aggregate, never
/// first-error-only) and exits without running a single frame.
///
/// Exit codes: findings → [`exit_code::FAIL`] (rejected config,
/// front-loading the same rejections a normal run would give one at
/// a time), lint infra/config failures (report unwritable, etc.) →
/// [`exit_code::CONFIG_ERROR`]. A CLEAN lint exits
/// [`exit_code::PASS`] with the standard report — meaning "the
/// config is coherent", NEVER "the game will pass": frame-level
/// judgments are out of lint's scope entirely.
///
/// The report is the standard `PlaytestReport` (same schema every
/// other takeover run produces): status `fail`, `harness_mode` set
/// to the lint mode marker, and each finding riding `violations` as
/// a `lint_finding` entry (mirroring `scenario_error`'s channel) —
/// the rule name is lint-specific, so findings never mix with a
/// real run's oracle entries.
fn run_lint(
    app: &mut App,
    scenario: &crate::scenario::Scenario,
    report_path: &str,
    source: &str,
) -> ! {
    let mut findings = crate::scenario::lint_scenario(scenario);
    // Stage 2 — the type-registry probe: when stage 1 found nothing,
    // or when its findings are all NotImplemented gates (a rule whose
    // shape is fully validated but whose EVALUATION is gated off —
    // a gated rule's author still needs selector/exclude feedback).
    // Hard Rejected errors still suppress stage 2, because every
    // stage-2 check presupposes the stage-1 shape (a resource_check
    // with no `resource` field, say, has nothing to probe): findings
    // append to the same aggregate list, and the author fixes stage
    // 1 first when both stages fire, exactly like fixing the first
    // error before seeing the second today. A typo'd selector under
    // a gated rule surfaces at lint time instead of after the
    // recorder ships — the docs' "probe is live while gated" claim
    // is thereby true at every commit of the slice, not just after
    // the gate lifts.
    //
    // The probe runs REAL game schedules (plugin finishing plus one
    // warm-up frame) and shares the run's crash contract exactly: a
    // game that panics at boot must not turn a lint invocation into
    // an uncompilable-looking exit-101 crash with no report. The
    // panic hook, run guard, and catch_unwind mirror run_scenario's
    // doctrine — the game crashed, the config was never linted —
    // and the report says so in lint's schema (exit CRASH).
    let probe_gated_only = findings
        .iter()
        .all(|f| matches!(f, crate::scenario::ScenarioError::NotImplemented(_)));
    let probe_crashed = if (findings.is_empty() || probe_gated_only) && lint_probe_wanted(scenario)
    {
        crate::driver::install_panic_hook();
        let guard = crate::sinks::try_acquire_run().ok();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            lint_registry_probe(app, scenario)
        }));
        drop(guard);
        match result {
            Ok(found) => {
                findings.extend(found);
                false
            }
            Err(_) => true,
        }
    } else {
        false
    };
    // Findings print to stderr ONCE — here, after both stages have
    // contributed (printing per-stage would show stage-1 errors
    // twice: once eagerly, once beside their stage-2 siblings).
    for err in &findings {
        eprintln!("bevy_playtest: {err}");
    }
    if probe_crashed {
        eprintln!(
            "bevy_playtest: the game panicked during the lint warm-up frame — \
             this is a game boot crash, not a lint finding; the scenario was \
             not probed"
        );
    }
    let mut report = crate::state::PlaytestReport::crashed_empty();
    report.status = if probe_crashed {
        crate::enums::PlaytestStatus::Crash
    } else if findings.is_empty() {
        crate::enums::PlaytestStatus::Pass
    } else {
        crate::enums::PlaytestStatus::Fail
    };
    if probe_crashed {
        // The run's crash path fills `error` ("run loop panicked"); the
        // lint schema must be equally self-describing for machine
        // consumers — a crash report with no reason field leaves the
        // panic narrative stranded on stderr only.
        report.error = Some("game panicked during the lint warm-up frame".to_string());
    }
    report.harness_mode = HARNESS_MODE_LINT.to_string();
    report.scenario_source = Some(source.to_string());
    report.violations = findings
        .iter()
        .map(|err| crate::state::ViolationEntry {
            rule: crate::rules::Rule::LintFinding.name().to_string(),
            target: "scenario".to_string(),
            first_frame: 0,
            last_frame: 0,
            count: 1,
            detail: err.to_string(),
            last_detail: err.to_string(),
            meta: None,
        })
        .collect();
    // Same summary contract as a real run: pure function of the
    // report's own rows (lint findings gate; a lint run has no held
    // trace, so no near-miss rows).
    report.compute_finding_summary();
    if let Err(err) = write_report(report_path, &report) {
        eprintln!("bevy_playtest: {err}");
        std::process::exit(exit_code::CONFIG_ERROR);
    }
    println!("bevy_playtest: report written to {report_path}");
    let code = match report.status {
        crate::enums::PlaytestStatus::Pass => exit_code::PASS,
        crate::enums::PlaytestStatus::Fail => exit_code::FAIL,
        crate::enums::PlaytestStatus::Crash => exit_code::CRASH,
    };
    std::process::exit(code)
}

/// Whether the scenario has anything the type-registry probe could
/// ever find a DEFINITE error for. When it does not, the probe's
/// cost and risk (plugin finishing, one real warm-up frame — asset
/// loads, saves, anything a game does at Startup) would buy
/// nothing: the probe cannot produce a finding. Skipping the boot
/// keeps linting a probe-free scenario side-effect-free.
fn lint_probe_wanted(scenario: &crate::scenario::Scenario) -> bool {
    use crate::enums::InvariantRule;
    let invariant_probe = scenario.invariants.iter().any(|inv| {
        match inv.rule {
            InvariantRule::ComponentCheck
            | InvariantRule::ResourceCheck
            | InvariantRule::StagnantResource => true,
            // The probe arm for nodes_in_bounds resolves a
            // `with_component` selector against the registry; a
            // selector-less (all-entities) declaration gives it
            // nothing to resolve, so the boot would be paid for a
            // guaranteed-empty probe.
            InvariantRule::NodesInBounds => inv.target_selector.is_some(),
            // pairwise_separation resolves its selector via the
            // same registry machinery, at both probe and run time
            // (the driver-loop episode recorder uses the same
            // resolution).
            InvariantRule::PairwiseSeparation => true,
            // derived_transform_consistency resolves its optional
            // selector and exclude_components prefixes via the
            // same registry machinery at probe time (its
            // run-time recorder ships with the consistency
            // slice; until then the rule rejects at stage 1 with
            // NotImplemented).
            InvariantRule::DerivedTransformConsistency => true,
            // forbidden_region resolves its selector (and the
            // optional allow_if exemption component) via the same
            // registry machinery at probe time — the driver-loop
            // episode recorder uses the same resolution.
            InvariantRule::ForbiddenRegion => true,
            // spawn_safety resolves BOTH selectors via the same
            // registry machinery at probe time.
            InvariantRule::SpawnSafety => true,
            // entity_stagnation resolves its selector, its
            // variant-qualified watch_path, and leaf_where (path
            // + variant existence) via the registry at probe
            // time — the typos there are the probe's whole
            // purpose (a variant typo would otherwise
            // masquerade as "never true" for the entire run).
            InvariantRule::EntityStagnation => true,
            // occupancy_cap resolves its selector via the same
            // registry machinery at probe time (radius/cap are
            // literals — no reflect paths to confirm).
            InvariantRule::OccupancyCap => true,
        }
    });
    invariant_probe || progress_surface_probe_wanted(scenario)
}

/// Whether the progress configuration names any surface entry the
/// probe could flag: markers (every non-off mode) and exact
/// `include_paths` entries (auto mode). The recorder resolves these
/// against registered-and-currently-inserted pairs at the run's
/// first sample, so a typo in them is a DEFINITE run error — the
/// class lint exists to front-load. Prefix entries (`foo::`) and
/// mode-off need no boot.
fn progress_surface_probe_wanted(scenario: &crate::scenario::Scenario) -> bool {
    let progress = &scenario.liveness.progress;
    if progress.mode == crate::scenario::ProgressMode::Off {
        return false;
    }
    let has_markers = progress.markers.iter().any(|m| !m.trim().is_empty());
    let has_exact_include = progress.mode == crate::scenario::ProgressMode::Auto
        && progress
            .include_paths
            .iter()
            .any(|p| !p.trim().is_empty() && !p.ends_with("::"));
    has_markers || has_exact_include
}

/// Stage 2: the type-registry probe. Booted-App checks stage 1 cannot
/// do — names and reflect paths resolve against the game's ACTUAL
/// TypeRegistry, not just the config's internal coherence.
///
/// Zero-frame contract (relaxed from a strict no-frames doctrine
/// to the warm-up doctrine): exactly ONE warm-up `app.update()` runs
/// before probing, so plugin building finishes, Startup schedules
/// run, and Startup-spawned entities exist for instance-level path
/// confirmation. One frame is not a playtest: no bot input, no
/// oracle evaluation, no frame-level judgments — the probe still
/// cannot say ANYTHING about how the game behaves across a run.
///
/// Doctrine — definite errors only:
/// - Resolution failures (unknown name, ambiguous short name, wrong
///   reflection data) are findings: these errors are certain to
///   resurface as `oracle_error` config errors at run time, so lint
///   front-loads them.
/// - Absence is NEVER a finding (the `reflect_resource` Absent
///   doctrine): a registered-but-never-inserted resource may arrive
///   later; entities may spawn after the single warm-up frame.
/// - Instance-level path failures are findings ONLY when an instance
///   exists to check against (present at the warm-up frame). When no
///   instance exists, the path stays UNCONFIRMED — stage 2 is silent
///   about it rather than guessing (a "cannot confirm" line for every
///   unprovable thing would bury the definite errors in noise).
///
/// Every stage-2 finding mirrors the run's resolution machinery —
/// the probe reuses `invariant_eval`'s builders
/// (`resolve_registration`, `resolve_entity_numeric_path`,
/// `reflect_resource`, `reflect_numeric_path`) so the wording
/// cannot drift between lint and run.
fn lint_registry_probe(
    app: &mut App,
    scenario: &crate::scenario::Scenario,
) -> Vec<crate::scenario::ScenarioError> {
    use crate::enums::InvariantRule;
    use crate::scenario::ScenarioError;

    // Finish plugin building exactly like the frame loop would, then
    // run exactly ONE warm-up update: Startup schedules execute and
    // their entities exist for the instance-level checks below.
    while matches!(app.plugins_state(), bevy_app::PluginsState::Adding) {
        bevy_tasks::tick_global_task_pools_on_main_thread();
    }
    if matches!(app.plugins_state(), bevy_app::PluginsState::Ready) {
        app.finish();
        app.cleanup();
    }
    app.update();

    let mut findings = Vec::new();
    let world = app.world_mut();
    let Some(atr) = world
        .get_resource::<bevy_ecs::reflect::AppTypeRegistry>()
        .cloned()
    else {
        // No registry at all: the same definite infra error the run's
        // oracle raises at frame 0 — front-load it. One finding per
        // probeable invariant, each naming its own tier, mirroring
        // the run's per-invariant oracle_error wording exactly (the
        // shared builder guarantees the match).
        for inv in &scenario.invariants {
            use crate::enums::InvariantRule;
            let tier = match inv.rule {
                InvariantRule::NodesInBounds => "with_component bounds checks",
                InvariantRule::ComponentCheck => "reflected component checks",
                InvariantRule::ResourceCheck | InvariantRule::StagnantResource => {
                    "reflected resource checks"
                }
                InvariantRule::PairwiseSeparation => "pairwise separation selector probes",
                InvariantRule::DerivedTransformConsistency => {
                    "derived-transform consistency selector probes"
                }
                InvariantRule::ForbiddenRegion => "forbidden region selector probes",
                InvariantRule::SpawnSafety => "spawn safety selector probes",
                InvariantRule::EntityStagnation => "entity stagnation selector probes",
                InvariantRule::OccupancyCap => "occupancy cap selector probes",
            };
            findings.push(ScenarioError::Rejected(format!(
                "invariant '{}': {}",
                inv.name,
                crate::invariant_eval::no_atr_message(tier)
            )));
        }
        return findings;
    };
    let registry = atr.read();
    for inv in &scenario.invariants {
        let inv_name = |finding: &str| format!("invariant '{}': {finding}", inv.name);
        match inv.rule {
            InvariantRule::ComponentCheck => {
                // Stage 1 already guaranteed the shape (the probe runs
                // only on a stage-1-clean scenario).
                let Some(crate::enums::TargetSelector::WithComponent { name }) =
                    inv.target_selector.as_ref()
                else {
                    continue;
                };
                let Some(path) = inv.path.as_deref() else {
                    continue;
                };
                probe_component_check(world, &registry, inv_name, name, path)
                    .into_iter()
                    .for_each(|f| findings.push(f));
            }
            InvariantRule::NodesInBounds => {
                // Selector names resolve dynamically at run time via
                // the same machinery the component_check probe below
                // uses; nothing extra to probe here beyond it.
                if let Some(crate::enums::TargetSelector::WithComponent { name }) =
                    inv.target_selector.as_ref()
                {
                    // Only the TYPE needs to resolve for bounds
                    // recording; there is no numeric path to confirm.
                    if let Err(why) =
                        crate::invariant_eval::resolve_registration(&registry, name, "component")
                    {
                        findings.push(ScenarioError::Rejected(inv_name(&why)));
                    }
                }
            }
            InvariantRule::PairwiseSeparation => {
                // Same probe doctrine as nodes_in_bounds: the
                // selector names a component TYPE that must resolve
                // at run time; there is no numeric path to confirm.
                if let Some(crate::enums::TargetSelector::WithComponent { name }) =
                    inv.target_selector.as_ref()
                {
                    if let Err(why) =
                        crate::invariant_eval::resolve_registration(&registry, name, "component")
                    {
                        findings.push(ScenarioError::Rejected(inv_name(&why)));
                    }
                }
            }
            InvariantRule::ForbiddenRegion => {
                // Same probe doctrine as pairwise: the selector
                // names a component TYPE that must resolve at run
                // time. allow_if is probed too — the exemption is a
                // definite component resolution (exact name, not a
                // prefix), so a typo in it is a DEFINITE run error
                // the lint front-loads.
                if let Some(crate::enums::TargetSelector::WithComponent { name }) =
                    inv.target_selector.as_ref()
                {
                    if let Err(why) =
                        crate::invariant_eval::resolve_registration(&registry, name, "component")
                    {
                        findings.push(ScenarioError::Rejected(inv_name(&why)));
                    }
                }
                if let Some(allow_if) = inv.allow_if.as_deref() {
                    if let Err(why) = crate::invariant_eval::resolve_registration(
                        &registry,
                        allow_if,
                        "component",
                    ) {
                        findings.push(ScenarioError::Rejected(inv_name(&why)));
                    }
                }
            }
            InvariantRule::DerivedTransformConsistency => {
                // Same probe doctrine: an optional selector names a
                // component TYPE that must resolve at run time
                // (absent = the `*` default, the whole hierarchy).
                // exclude_components entries are deliberately NOT
                // probed — they are type-path PREFIXES
                // (progress-surface exclude grammar), so exact
                // resolution is the wrong question, and the
                // progress surface sets the precedent of leaving
                // excludes unprobed.
                if let Some(crate::enums::TargetSelector::WithComponent { name }) =
                    inv.target_selector.as_ref()
                {
                    if let Err(why) =
                        crate::invariant_eval::resolve_registration(&registry, name, "component")
                    {
                        findings.push(ScenarioError::Rejected(inv_name(&why)));
                    }
                }
            }
            InvariantRule::SpawnSafety => {
                // Same probe doctrine, applied to BOTH selectors:
                // the target selector names the judged archetype
                // and min_distance_from names the anchors — both
                // are component TYPES that must resolve.
                for selector in [inv.target_selector.as_ref(), inv.min_distance_from.as_ref()]
                    .into_iter()
                    .flatten()
                {
                    if let crate::enums::TargetSelector::WithComponent { name } = selector {
                        if let Err(why) = crate::invariant_eval::resolve_registration(
                            &registry,
                            name,
                            "component",
                        ) {
                            findings.push(ScenarioError::Rejected(inv_name(&why)));
                        }
                    }
                }
            }
            InvariantRule::ResourceCheck | InvariantRule::StagnantResource => {
                let Some(resource) = inv.resource.as_deref() else {
                    continue;
                };
                // The watched leaf lives on the primary resource.
                let primary = crate::invariant_eval::reflect_resource(world, &registry, resource);
                if let Err(why) = &primary {
                    findings.push(ScenarioError::Rejected(inv_name(why)));
                } else if let Ok(crate::invariant_eval::ResourceView::Value(value)) = &primary {
                    if let Err(why) = crate::invariant_eval::reflect_numeric_path(
                        *value,
                        inv.path.as_deref().expect("stage-1 guaranteed"),
                        "resource",
                    ) {
                        findings.push(ScenarioError::Rejected(inv_name(&why)));
                    }
                }
                // stagnant_resource: the driver leaf lives on
                // `driver_resource`, or the primary resource when the
                // field is omitted (the documented default).
                if inv.rule == InvariantRule::StagnantResource {
                    let driver_root = inv.driver_resource.as_deref().unwrap_or(resource);
                    let driver =
                        crate::invariant_eval::reflect_resource(world, &registry, driver_root);
                    if let Err(why) = &driver {
                        findings.push(ScenarioError::Rejected(inv_name(why)));
                    } else if let Ok(crate::invariant_eval::ResourceView::Value(value)) = &driver {
                        if let Err(why) = crate::invariant_eval::reflect_numeric_path(
                            *value,
                            inv.driver_path.as_deref().expect("stage-1 guaranteed"),
                            "resource",
                        ) {
                            findings.push(ScenarioError::Rejected(inv_name(&why)));
                        }
                    }
                }
            }
            InvariantRule::EntityStagnation => {
                // The probe resolves everything the recorder will:
                // the selector component, the variant-qualified
                // watch_path, leaf_where (path + variant), and
                // the resource driver when declared. All
                // definite-error — a typo in any would
                // masquerade as "never true" for the whole run.
                let Some(crate::enums::TargetSelector::WithComponent { name }) =
                    inv.target_selector.as_ref()
                else {
                    // Stage-1 guaranteed; the match above stayed
                    // fallible for symmetry with the other arms.
                    continue;
                };
                // Selector type must resolve with component
                // reflection data. Instance-level watch-path
                // resolution is NOT probed per-entity: an entity
                // whose enum is in a different variant fails
                // path resolution legitimately (the leaf_where
                // precondition, not an error) — only
                // definite-type-level failures count, and the
                // registry holds those.
                if let Err(why) =
                    crate::invariant_eval::resolve_registration(&registry, name, "component")
                {
                    findings.push(ScenarioError::Rejected(inv_name(&why)));
                    continue;
                }
                // The variant-bearing portion of watch_path:
                // every segment must exist on the component's
                // reflect data — fields resolve via the registry
                // without needing a live instance, and an enum
                // VARIANT name is checkable against the enum's
                // variant list. This is the leaf_where "definite
                // resolution" hinge (Q2): a variant typo can
                // never masquerade as "never true."
                if let Some(watch_path) = inv.watch_path.as_deref() {
                    if let Err(why) =
                        crate::invariant_eval::probe_entity_watch_path(&registry, name, watch_path)
                    {
                        findings.push(ScenarioError::Rejected(inv_name(&why)));
                    }
                }
                if let Some(leaf_where) = inv.leaf_where.as_deref() {
                    // Stage-1 guaranteed the `path == Variant`
                    // shape.
                    let (lw_path, variant) = leaf_where
                        .split_once("==")
                        .expect("stage-1 guaranteed shape");
                    if let Err(why) = crate::invariant_eval::probe_entity_watch_path(
                        &registry,
                        name,
                        format!("{}.{}", lw_path.trim(), variant.trim()).as_str(),
                    ) {
                        findings.push(ScenarioError::Rejected(inv_name(&why)));
                    }
                }
                // A declared resource driver: resolve the leaf
                // on the resource, mirroring the
                // stagnant_resource probe above.
                if inv.driver.as_deref() == Some("resource") {
                    let Some(resource) = inv.driver_resource.as_deref() else {
                        continue;
                    };
                    let view = crate::invariant_eval::reflect_resource(world, &registry, resource);
                    if let Err(why) = &view {
                        findings.push(ScenarioError::Rejected(inv_name(why)));
                    } else if let Ok(crate::invariant_eval::ResourceView::Value(value)) = &view {
                        if let Err(why) = crate::invariant_eval::reflect_numeric_path(
                            *value,
                            inv.driver_path.as_deref().expect("stage-1 guaranteed"),
                            "resource",
                        ) {
                            findings.push(ScenarioError::Rejected(inv_name(&why)));
                        }
                    }
                }
            }
            InvariantRule::OccupancyCap => {
                // Same probe doctrine as pairwise: the selector
                // names a component TYPE that must resolve at run
                // time. radius/cap are literals — no reflect
                // paths to confirm. A declared driver leaf
                // (pairwise's grammar) resolves like
                // pairwise's would; the probe front-loads the
                // definite resolution error.
                if let Some(crate::enums::TargetSelector::WithComponent { name }) =
                    inv.target_selector.as_ref()
                {
                    if let Err(why) =
                        crate::invariant_eval::resolve_registration(&registry, name, "component")
                    {
                        findings.push(ScenarioError::Rejected(inv_name(&why)));
                    }
                }
                if let (Some(res_name), Some(path)) =
                    (inv.driver_resource.as_deref(), inv.driver_path.as_deref())
                {
                    let view = crate::invariant_eval::reflect_resource(world, &registry, res_name);
                    if let Err(why) = &view {
                        findings.push(ScenarioError::Rejected(inv_name(why)));
                    } else if let Ok(crate::invariant_eval::ResourceView::Value(value)) = &view {
                        if let Err(why) =
                            crate::invariant_eval::reflect_numeric_path(*value, path, "resource")
                        {
                            findings.push(ScenarioError::Rejected(inv_name(&why)));
                        }
                    }
                }
            }
        }
    }
    // Progress-surface probe: markers and exact include_paths
    // entries resolve at the run's FIRST sampled frame, so a typo in
    // them is a definite run error (`oracle_error` at frame 0 plus
    // whatever second-order finding the missing channel causes) —
    // the exact class lint front-loads. Shared resolution with the
    // recorder (lint_probe_surface reuses initialize's flow), so the
    // message cannot drift between lint and run.
    for problem in crate::progress::lint_probe_surface(world, &scenario.liveness.progress) {
        findings.push(ScenarioError::Rejected(format!(
            "progress surface entry failed to resolve — {problem}"
        )));
    }
    findings
}

/// Probe one `component_check` invariant: the selector type must
/// resolve with component reflection data (definite errors — the run
/// would emit them as `oracle_error` at its first matched frame), and
/// the numeric `path` must resolve on every entity bearing the
/// selector at the warm-up frame (selector first, Transform
/// fallback — `eval_one`'s doctrine, mirrored word-for-word). Zero
/// matched entities: unconfirmed, no finding (the component may
/// spawn later).
fn probe_component_check(
    world: &bevy_ecs::prelude::World,
    registry: &bevy_reflect::TypeRegistry,
    inv_name: impl Fn(&str) -> String,
    name: &str,
    path: &str,
) -> Vec<crate::scenario::ScenarioError> {
    use crate::scenario::ScenarioError;
    let mut findings = Vec::new();
    let registration =
        match crate::invariant_eval::resolve_registration(registry, name, "component") {
            Err(why) => {
                findings.push(ScenarioError::Rejected(inv_name(&why)));
                return findings;
            }
            Ok(r) => r,
        };
    let Some(reflect_component) = registration.data::<bevy_ecs::reflect::ReflectComponent>() else {
        findings.push(ScenarioError::Rejected(inv_name(
            &crate::invariant_eval::not_a_reflected_component_message(name),
        )));
        return findings;
    };
    for entity_ref in world.iter_entities() {
        let Some(reflect) = reflect_component.reflect(entity_ref) else {
            continue;
        };
        // Selector first, Transform fallback — the shared resolver
        // guarantees the message matches eval_one's word-for-word,
        // including the entity annotation.
        let value = crate::invariant_eval::resolve_entity_numeric_path(reflect, &entity_ref, path);
        if let Err(why) = value {
            // One definite instance-level failure is enough: the run
            // hits it on the first matched frame.
            findings.push(ScenarioError::Rejected(inv_name(&why)));
            break;
        }
    }
    findings
}

/// Case-sensitive magic literal for the built-in smoke scenario.
pub const SMOKE_LITERAL: &str = "smoke";

/// `harness_mode` marker for lint-mode reports. Distinct from
/// `"takeover"` (a real driven run) and `"in_process"` (library API):
/// a lint report is a config-coherence artifact about a scenario —
/// a crash-marked lint report means the GAME crashed at boot, not
/// that the scenario was executed.
pub const HARNESS_MODE_LINT: &str = "lint";

/// Case-sensitive magic literal for the built-in default battery:
/// the zero-config battery run. Accepted as a `BEVY_PLAYTEST_SCENARIO`
/// value.
pub const BATTERY_LITERAL: &str = "battery";

/// The built-in smoke scenario: chaos bot, no authored invariants (the
/// global oracles still run), ~5s at default tps. Constructed in code
/// through the same `validate_scenario` path as a parsed file so it can
/// never drift from the schema contract.
pub fn smoke_scenario() -> Result<crate::scenario::Scenario, String> {
    let scenario: crate::scenario::Scenario = serde_json::from_str(
        r#"{"name":"smoke","bot":{"type":"chaos","seed":42},"duration_s":5.0,"liveness":{"progress":{"mode":"off"}}}"#,
    )
    .map_err(|err| format!("built-in smoke scenario failed to parse: {err}"))?;
    crate::scenario::validate_scenario(&scenario)
        .map_err(|err| format!("built-in smoke scenario failed validation: {err}"))?;
    Ok(scenario)
}

/// The built-in default battery: the zero-config battery run. Single
/// seeded chaos pass with all oracles (20s run, 15s frozen-world
/// grace — shorter than the run so the liveness oracle CAN fire;
/// static-screen games are covered by the grace, not by an unreachable
/// oracle). Progress-liveness demands a verifiable surface
/// (require_surface): a battery pass that could not check progress
/// is not a green battery. **Breaking** relative to 0.11: games that
/// never register game-state types now fail the battery with
/// progress_surface_empty instead of passing vacuously.
/// Multi-seed aggregation lands with the runner-side battery
/// API; the takeover runs one pass per process by design (replay
/// verification needs process-clean state).
pub fn battery_scenario() -> Result<crate::scenario::Scenario, String> {
    let scenario: crate::scenario::Scenario = serde_json::from_str(
        r#"{"name":"battery","bot":{"type":"chaos","seed":42,"explore":true},"duration_s":20.0,"liveness":{"timeout_s":15.0,"progress":{"require_surface":true}}}"#,
    )
    .map_err(|err| format!("built-in battery scenario failed to parse: {err}"))?;
    crate::scenario::validate_scenario(&scenario)
        .map_err(|err| format!("built-in battery scenario failed validation: {err}"))?;
    Ok(scenario)
}

/// Env var marking a replay-verification PASS-B child process: the
/// takeover skips verification (avoiding recursion), runs one pass,
/// writes the fingerprint series to the path in the value, and exits.
const ENV_REPLAY_PASS_B: &str = "BEVY_PLAYTEST_REPLAY_PASS_B_SERIES";
const ENV_REPLAY_PASS_B_DIGEST: &str = "BEVY_PLAYTEST_REPLAY_PASS_B_DIGEST";

/// Replay verification on the takeover path: pass A runs in this
/// process, writing its fingerprint series to a file; a child process
/// re-runs the game binary + scenario for pass B; the series are
/// compared and divergence becomes a ReplayDivergence violation.
fn verify_replay_subprocess(
    app: &mut App,
    scenario: &crate::scenario::Scenario,
    report_path: &str,
) -> Result<crate::state::PlaytestReport, String> {
    if !scenario.single_threaded {
        return Err(
            "verify_replay requires single_threaded: true (multi-threaded \
             schedules have nondeterministic system ordering)"
                .to_string(),
        );
    }

    // Pass A: this process.
    let mut report = crate::driver::run_scenario(app, scenario)
        .map_err(|err| format!("replay pass A failed: {err}"))?;
    let series_a = app
        .world_mut()
        .remove_resource::<crate::replay::ReplaySeries>()
        .map(|s| s.0)
        .unwrap_or_default();

    if std::env::var_os(ENV_REPLAY_PASS_B).is_some() {
        // We ARE the child: write the series, exit 0 (parent compares).
        let series_path = std::env::var(ENV_REPLAY_PASS_B).unwrap();
        let bytes = serde_json::to_vec(&series_fingerprints(&series_a))
            .map_err(|err| format!("cannot serialize replay series: {err}"))?;
        std::fs::write(&series_path, bytes)
            .map_err(|err| format!("cannot write replay series '{series_path}': {err}"))?;
        // Opt-in broad digest series travels alongside the fingerprint
        // series so takeover replay verification covers the same widened
        // state surface as the in-process path.
        if let Some(digest_path) = std::env::var_os(ENV_REPLAY_PASS_B_DIGEST) {
            let digest_bytes = serde_json::to_vec(&digest_series(&report.state_digest))
                .map_err(|err| format!("cannot serialize digest series: {err}"))?;
            let digest_path = digest_path.to_string_lossy();
            std::fs::write(&*digest_path, digest_bytes).map_err(|err| {
                format!("cannot write replay digest series '{digest_path}': {err}")
            })?;
        }
        std::process::exit(0);
    }

    // Pass B: re-run the game binary as a child with the same env.
    let exe = std::env::current_exe()
        .map_err(|err| format!("cannot locate current executable for replay pass B: {err}"))?;
    let series_path = format!("{report_path}.replay-series-b.json");
    let digest_path = format!("{report_path}.digest-series-b.json");
    let output = std::process::Command::new(&exe)
        // Pass-B child must behave identically to a fresh run: strip
        // OUR report target (set explicitly below) and never inherit
        // takeover-adjacent env from the parent's environment —
        // including the scan-export path, which would otherwise make
        // the child re-export ITS captured scan over the parent's
        // control-run export (the child's game is identical, but its
        // scan rides a different capture moment and the parent would
        // never notice the overwrite).
        .env_remove("BEVY_PLAYTEST_REPORT")
        .env_remove("BEVY_PLAYTEST_HEADLESS")
        .env_remove(ENV_DISCOVER_SCAN)
        .env(ENV_REPLAY_PASS_B, &series_path)
        .env(ENV_REPLAY_PASS_B_DIGEST, &digest_path)
        .output()
        .map_err(|err| format!("cannot spawn replay pass B ({exe:?}): {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "replay pass B exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let bytes = std::fs::read(&series_path)
        .map_err(|err| format!("cannot read replay pass B series: {err}"))?;
    let series_b: Vec<(u64, u64)> = serde_json::from_slice(&bytes)
        .map_err(|err| format!("corrupt replay pass B series: {err}"))?;
    let _ = std::fs::remove_file(&series_path);

    let frames_b: Vec<crate::replay::FrameFingerprint> = series_b
        .into_iter()
        .map(|(frame, hash)| crate::replay::FrameFingerprint { frame, hash })
        .collect();

    // Compare the fingerprint series, then (opt-in) the digest series.
    // Evidence from both merges into a single replay_divergence
    // violation so aggregated reports keep one (rule, target) pair.
    let mut diverged_details: Vec<String> = vec![];
    // Actual first mismatching frame across both comparisons, so violation
    // frame ranges point consumers at the divergence, not at run start.
    let mut first_divergent_frame: Option<u64> = None;
    if report.status != crate::enums::PlaytestStatus::Crash {
        if let Err(div) = crate::replay::compare(&series_a, &frames_b) {
            diverged_details.push(format!(
                "first divergent frame {}: pass A fingerprint {:#018x}, pass B \
                 fingerprint {:#018x} — hidden nondeterminism (wall clock, \
                 unseeded RNG, or unordered iteration)",
                div.frame, div.pass_a_hash, div.pass_b_hash
            ));
            first_divergent_frame = Some(first_divergent_frame.unwrap_or(u64::MAX).min(div.frame));
        }
        if scenario.record_digest {
            let bytes = std::fs::read(&digest_path)
                .map_err(|err| format!("cannot read replay pass B digest series: {err}"))?;
            // Wire format matches the child writer: Vec<(frame, hash)>.
            let pairs: Vec<(u64, u64)> = serde_json::from_slice(&bytes)
                .map_err(|err| format!("corrupt replay pass B digest series: {err}"))?;
            let digests_b: Vec<crate::digest::StateDigest> = pairs
                .into_iter()
                .map(|(frame, hash)| crate::digest::StateDigest { frame, hash })
                .collect();
            if let Err(div) = crate::digest::compare(&report.state_digest, &digests_b) {
                diverged_details.push(format!(
                    "first state-digest divergence at frame {}: pass A hash \
                     {:#018x}, pass B hash {:#018x} — hidden nondeterminism \
                     in a signal the narrow fingerprint does not cover \
                     (resource values, entity/archetype counts, or UI state)",
                    div.frame, div.hash_a, div.hash_b
                ));
                first_divergent_frame =
                    Some(first_divergent_frame.unwrap_or(u64::MAX).min(div.frame));
            }
        }
    }
    let _ = std::fs::remove_file(&digest_path);
    if !diverged_details.is_empty() {
        let first = first_divergent_frame.unwrap_or(0);
        report.violations.push(crate::state::ViolationEntry {
            rule: crate::rules::Rule::ReplayDivergence.name().to_string(),
            target: "run".to_string(),
            first_frame: first,
            last_frame: first,
            count: 1,
            detail: diverged_details.join("; "),
            last_detail: diverged_details.last().cloned().unwrap_or_default(),
            meta: None,
        });
        report.status = crate::enums::PlaytestStatus::Fail;
    }
    Ok(report)
}

fn series_fingerprints(series: &[crate::replay::FrameFingerprint]) -> Vec<(u64, u64)> {
    series.iter().map(|f| (f.frame, f.hash)).collect()
}

fn digest_series(series: &[crate::digest::StateDigest]) -> Vec<(u64, u64)> {
    series.iter().map(|d| (d.frame, d.hash)).collect()
}

fn load_scenario(path: &str) -> Result<crate::scenario::Scenario, String> {
    let bytes =
        std::fs::read(path).map_err(|err| format!("cannot read scenario '{path}': {err}"))?;
    serde_json::from_slice::<crate::scenario::Scenario>(&bytes)
        .map_err(|err| format!("invalid scenario '{path}': {err}"))
}

fn write_report(path: &str, report: &crate::state::PlaytestReport) -> Result<(), String> {
    let json = serde_json::to_string_pretty(report)
        .map_err(|err| format!("cannot serialize report: {err}"))?;
    std::fs::write(path, json).map_err(|err| format!("cannot write report '{path}': {err}"))
}

/// `DefaultPlugins` equivalent that plays well with [`PlaytestPlugin`].
///
/// Optional sugar: with plain `DefaultPlugins` + [`PlaytestPlugin`]
/// (plugin added after the group) the takeover runner already replaces
/// winit's event loop, so no window appears. This group exists for games
/// that prefer an explicit headless plugin graph.
///
/// - No env vars set: identical to `bevy::DefaultPlugins` (pure pass-through).
/// - [`ENV_SCENARIO`] or [`ENV_HEADLESS`] set: drops
///   `WinitPlugin` and adds `ScheduleRunnerPlugin::run_loop` so the app
///   ticks headless. (bevy_winit 0.20 builds its event loop eagerly in
///   `WinitPlugin::build` — a display-less host panics before any
///   runner swap could intervene, so removing the plugin is the only
///   reliable headless path.)
/// - [`ENV_SCENARIO`] only (a playtest run will drive this app via
///   `run_scenario`): additionally drops `LogPlugin` so the harness's
///   capture subscriber can install as the global tracing default —
///   `LogPlugin` would install its own at plugin-build time and the
///   global default is write-once per process, permanently blinding
///   log capture. The harness's fmt layer replaces LogPlugin's
///   console output (RUST_LOG honored, default INFO). Deliberately
///   NOT applied under [`ENV_HEADLESS`]: those run
///   the game normally and keep the game's own logging.
///
/// Requires the `takeover-winit` feature (adds the `bevy_winit` and
/// `bevy` (DefaultPlugins) dependencies).
#[cfg(feature = "takeover-winit")]
pub struct PlaytestDefaultPlugins;

#[cfg(feature = "takeover-winit")]
impl bevy_app::prelude::PluginGroup for PlaytestDefaultPlugins {
    fn build(self) -> bevy_app::PluginGroupBuilder {
        let builder = bevy::DefaultPlugins.build();
        if std::env::var_os(ENV_SCENARIO).is_some() || std::env::var_os(ENV_HEADLESS).is_some() {
            let builder = builder.disable::<bevy_winit::WinitPlugin>().add(
                bevy_app::ScheduleRunnerPlugin::run_loop(std::time::Duration::from_secs_f64(
                    1.0 / 60.0,
                )),
            );
            // Only a scenario run installs the harness's capture
            // subscriber (run_scenario); headless-assert
            // children never do, so dropping LogPlugin there would
            // leave them with NO logging at all. Note: unconditional
            // `bevy::log::LogPlugin` reference relies on bevy's
            // default features including `bevy_log`.
            if std::env::var_os(ENV_SCENARIO).is_some() {
                builder.disable::<bevy::log::LogPlugin>()
            } else {
                builder
            }
        } else {
            builder
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_env<F: FnOnce() -> T, T>(vars: &[(&str, &str)], f: F) -> T {
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
        struct Guard(Vec<std::ffi::OsString>);
        impl Drop for Guard {
            fn drop(&mut self) {
                for key in &self.0 {
                    std::env::remove_var(key);
                }
            }
        }
        let mut removed = Vec::new();
        for (key, value) in vars {
            if std::env::var_os(key).is_some() {
                std::env::remove_var(key);
                removed.push(std::ffi::OsString::from(key));
            }
            std::env::set_var(key, value);
        }
        let guard = Guard(removed);
        let result = f();
        for (key, _) in vars {
            std::env::remove_var(key);
        }
        drop(guard);
        result
    }

    #[test]
    fn no_scenario_env_means_no_config() {
        with_env(&[], || {
            // No BEVY_PLAYTEST_* vars at all (parallel tests may set none).
            if std::env::var_os(ENV_SCENARIO).is_some() {
                return;
            }
            let cfg = read_env_config().unwrap();
            assert_eq!(cfg, Takeover::Inert);
        });
    }

    #[test]
    fn battery_scenario_literal_accepted() {
        // BEVY_PLAYTEST_SCENARIO=battery routes to the builtin battery.
        with_env(&[(ENV_SCENARIO, BATTERY_LITERAL)], || {
            let cfg = read_env_config().unwrap();
            assert_eq!(
                cfg,
                Takeover::Run(TakeoverConfig {
                    scenario_path: BATTERY_LITERAL.to_string(),
                    report_path: "bevy_playtest-battery.report.json".to_string(),
                })
            );
        });
    }

    #[test]
    fn battery_scenario_validates() {
        battery_scenario().expect("built-in battery validates");
    }

    #[test]
    fn scenario_env_sets_paths() {
        with_env(
            &[(ENV_SCENARIO, "/tmp/s.json"), (ENV_REPORT, "/tmp/r.json")],
            || {
                let Takeover::Run(cfg) = read_env_config().unwrap() else {
                    panic!("expected Run")
                };
                assert_eq!(cfg.scenario_path, "/tmp/s.json");
                assert_eq!(cfg.report_path, "/tmp/r.json");
            },
        );
    }

    #[test]
    fn report_defaults_to_scenario_sibling() {
        with_env(&[(ENV_SCENARIO, "/tmp/s.json")], || {
            let Takeover::Run(cfg) = read_env_config().unwrap() else {
                panic!("expected Run")
            };
            assert_eq!(cfg.report_path, "/tmp/s.json.report.json");
        });
    }

    #[test]
    fn headless_only_env_yields_assert_branch() {
        with_env(&[(ENV_HEADLESS, "1")], || {
            let cfg = read_env_config().unwrap();
            assert_eq!(cfg, Takeover::HeadlessAssert);
        });
    }

    #[test]
    fn empty_scenario_env_rejects() {
        with_env(&[(ENV_SCENARIO, "  ")], || {
            let err = read_env_config().unwrap_err();
            assert!(err.contains("empty"), "{err}");
        });
    }

    #[test]
    fn unknown_env_var_rejects() {
        with_env(
            &[(ENV_SCENARIO, "/tmp/s.json"), ("BEVY_PLAYTEST_TURBO", "1")],
            || {
                let err = read_env_config().unwrap_err();
                assert!(err.contains("BEVY_PLAYTEST_TURBO"), "{err}");
            },
        );
    }

    #[test]
    fn plugin_is_inert_without_scenario() {
        if std::env::var_os(ENV_SCENARIO).is_some() {
            return;
        }
        let mut app = App::new();
        app.add_plugins(PlaytestPlugin);
        // Inert: no runner swap. Prove it by running to a normal exit.
        let exit = app.run();
        assert_eq!(exit, AppExit::Success);
    }

    #[test]
    fn smoke_literal_resolves_to_valid_scenario() {
        let scenario = smoke_scenario().expect("smoke validates");
        assert_eq!(scenario.bot.bot_type, crate::enums::BotType::Chaos);
        assert_eq!(scenario.duration_s, 5.0);
        assert_eq!(scenario.name.as_deref(), Some("smoke"));
    }

    #[test]
    fn smoke_literal_gets_cwd_report_default() {
        with_env(&[(ENV_SCENARIO, SMOKE_LITERAL)], || {
            let Takeover::Run(cfg) = read_env_config().unwrap() else {
                panic!("expected Run")
            };
            assert_eq!(cfg.report_path, "bevy_playtest-smoke.report.json");
        });
    }

    #[test]
    fn scenario_json_round_trips_into_config() {
        let dir = std::env::temp_dir();
        let path = dir.join("bevy_playtest_takeover_unit.json");
        let scenario: crate::scenario::Scenario =
            serde_json::from_str(r#"{"bot":{"type":"chaos","seed":3},"duration_s":0.1,"tps":60}"#)
                .unwrap();
        std::fs::write(&path, serde_json::to_vec(&scenario).unwrap()).unwrap();
        let loaded = load_scenario(path.to_str().unwrap()).unwrap();
        assert_eq!(
            serde_json::to_value(&loaded).unwrap(),
            serde_json::to_value(&scenario).unwrap()
        );
        let _ = std::fs::remove_file(&path);
    }

    // -----------------------------------------------------------------------
    // Stage-2 registry probe (lint_registry_probe)
    // -----------------------------------------------------------------------

    /// Test game: a registered reflected resource, INSERTED; a
    /// registered-but-never-inserted resource (the Absent doctrine);
    /// a registered component with an instance spawned in Startup.
    mod probe_game {
        use bevy_app::prelude::*;
        use bevy_ecs::prelude::*;
        use bevy_transform::prelude::*;

        #[derive(Resource, Debug, bevy_reflect::Reflect)]
        #[reflect(bevy_ecs::reflect::ReflectResource)]
        pub struct Score {
            pub home: f32,
        }

        /// Registered, never inserted — legit for lint (late
        /// arrival), would be vacuity at run time, never a lint
        /// finding.
        #[derive(Resource, Debug, bevy_reflect::Reflect)]
        #[reflect(bevy_ecs::reflect::ReflectResource)]
        pub struct LateArriving {
            pub value: f32,
        }

        #[derive(Component, Debug, bevy_reflect::Reflect)]
        #[reflect(bevy_ecs::reflect::ReflectComponent)]
        pub struct Health {
            pub hp: f32,
        }

        pub fn app() -> App {
            let mut app = App::new();
            app.register_type::<Score>();
            app.register_type::<LateArriving>();
            app.register_type::<Health>();
            app.insert_resource(Score { home: 1.0 });
            // Spawned in the Startup schedule, exactly like a real
            // game: the probe's ONE warm-up update must bring this
            // entity into existence for the instance-level checks.
            app.add_systems(Startup, |mut commands: Commands| {
                commands.spawn((Health { hp: 3.0 }, Transform::default()));
            });
            app
        }
    }

    fn probe_json(invariants: serde_json::Value) -> crate::scenario::Scenario {
        let json = serde_json::json!({
            "bot": { "type": "chaos", "seed": 1 },
            "duration_s": 1.0,
            "invariants": invariants,
        });
        serde_json::from_value(json).expect("parse probe scenario")
    }

    /// A typo'd resource name is a definite error: unknown to the
    /// registry, the run would raise oracle_error at frame 0. Lint
    /// front-loads it.
    #[test]
    fn probe_flags_unknown_resource_name() {
        let scenario = probe_json(serde_json::json!([
            { "name": "score_watch", "rule": "resource_check",
              "resource": "Scroe", "path": "home", "check": "gt", "value": 0.0 }
        ]));
        let mut app = probe_game::app();
        let findings = lint_registry_probe(&mut app, &scenario);
        assert!(
            findings
                .iter()
                .any(|f| f.to_string().contains("unknown") && f.to_string().contains("Scroe")),
            "typo'd resource name must be a finding: {findings:?}"
        );
    }

    /// A registered-but-never-inserted resource (Absent doctrine) is
    /// NEVER a finding: late arrival is legitimate gameplay.
    #[test]
    fn probe_absent_resource_is_not_a_finding() {
        let scenario = probe_json(serde_json::json!([
            { "name": "late_watch", "rule": "resource_check",
              "resource": "LateArriving", "path": "value", "check": "gt", "value": 0.0 }
        ]));
        let mut app = probe_game::app();
        let findings = lint_registry_probe(&mut app, &scenario);
        assert!(
            findings.is_empty(),
            "Absent resource must not be a lint finding: {findings:?}"
        );
    }

    /// A bogus path on an INSERTED resource is a definite error —
    /// the instance exists, the leaf does not.
    #[test]
    fn probe_flags_bogus_path_on_present_resource() {
        let scenario = probe_json(serde_json::json!([
            { "name": "score_watch", "rule": "resource_check",
              "resource": "Score", "path": "hoem", "check": "gt", "value": 0.0 }
        ]));
        let mut app = probe_game::app();
        let findings = lint_registry_probe(&mut app, &scenario);
        assert!(
            findings.iter().any(|f| f.to_string().contains("hoem")),
            "bogus path on present resource must be a finding: {findings:?}"
        );
    }

    /// A valid path on an inserted resource is clean — the happy
    /// path stays silent.
    #[test]
    fn probe_accepts_valid_resource_path() {
        let scenario = probe_json(serde_json::json!([
            { "name": "score_watch", "rule": "resource_check",
              "resource": "Score", "path": "home", "check": "gt", "value": 0.0 }
        ]));
        let mut app = probe_game::app();
        let findings = lint_registry_probe(&mut app, &scenario);
        assert!(
            findings.is_empty(),
            "valid resource check must lint clean: {findings:?}"
        );
    }

    /// A typo'd selector component name is a definite error.
    #[test]
    fn probe_flags_unknown_component_name() {
        let scenario = probe_json(serde_json::json!([
            { "name": "hp_watch", "rule": "component_check",
              "target_selector": { "with_component": { "name": "Heatlh" } },
              "path": "hp", "check": "lt", "value": 0.0 }
        ]));
        let mut app = probe_game::app();
        let findings = lint_registry_probe(&mut app, &scenario);
        assert!(
            findings
                .iter()
                .any(|f| f.to_string().contains("unknown") && f.to_string().contains("Heatlh")),
            "typo'd component name must be a finding: {findings:?}"
        );
    }

    /// A bogus path on an EXISTING entity (spawned in Startup, alive
    /// at the warm-up frame) is a definite error; a valid path
    /// resolving through the selector component is clean. Also the
    /// Transform fallback: `translation.x` resolves on the entity's
    /// Transform even though Health has no such field.
    #[test]
    fn probe_instance_level_component_paths() {
        // Bogus leaf on the selector component, entity exists.
        let scenario = probe_json(serde_json::json!([
            { "name": "hp_watch", "rule": "component_check",
              "target_selector": { "with_component": { "name": "Health" } },
              "path": "ph", "check": "lt", "value": 0.0 }
        ]));
        let mut app = probe_game::app();
        let findings = lint_registry_probe(&mut app, &scenario);
        assert!(
            findings.iter().any(|f| f.to_string().contains("ph")),
            "bogus path on existing entity must be a finding: {findings:?}"
        );

        // Valid leaf on the selector component.
        let scenario = probe_json(serde_json::json!([
            { "name": "hp_watch", "rule": "component_check",
              "target_selector": { "with_component": { "name": "Health" } },
              "path": "hp", "check": "lt", "value": 0.0 }
        ]));
        let mut app = probe_game::app();
        let findings = lint_registry_probe(&mut app, &scenario);
        assert!(
            findings.is_empty(),
            "valid selector path must lint clean: {findings:?}"
        );

        // Transform fallback path: selector has no such field, the
        // entity's Transform does. eval_one's doctrine, mirrored.
        let scenario = probe_json(serde_json::json!([
            { "name": "hp_watch", "rule": "component_check",
              "target_selector": { "with_component": { "name": "Health" } },
              "path": "translation.x", "check": "lt", "value": 10.0 }
        ]));
        let mut app = probe_game::app();
        let findings = lint_registry_probe(&mut app, &scenario);
        assert!(
            findings.is_empty(),
            "Transform-fallback path must lint clean: {findings:?}"
        );
    }

    /// An inserted REFLECTED-as-component-only type named as a
    /// resource: definite data-shape error (the run would raise
    /// oracle_error at frame 0).
    #[test]
    fn probe_flags_component_named_as_resource() {
        let scenario = probe_json(serde_json::json!([
            { "name": "hp_watch", "rule": "resource_check",
              "resource": "Health", "path": "hp", "check": "gt", "value": 0.0 }
        ]));
        let mut app = probe_game::app();
        let findings = lint_registry_probe(&mut app, &scenario);
        assert!(
            findings
                .iter()
                .any(|f| f.to_string().contains("not a reflected Resource")),
            "a component named as a resource must be a finding: {findings:?}"
        );
    }

    /// stale stagnant_resource driver paths (the leaf the windows
    /// budget watches) probe exactly like watched leaves — on the
    /// driver_resource, or the primary resource by default.
    #[test]
    fn probe_stagnant_driver_paths() {
        // Driver on the primary resource, valid leaf: clean.
        let scenario = probe_json(serde_json::json!([
            { "name": "stag", "rule": "stagnant_resource",
              "resource": "Score", "path": "home", "driver_path": "home", "window_s": 0.5 }
        ]));
        let mut app = probe_game::app();
        let findings = lint_registry_probe(&mut app, &scenario);
        assert!(
            findings.is_empty(),
            "valid stagnant_resource must lint clean: {findings:?}"
        );
        // Driver on the primary resource, BOGUS leaf: finding.
        let scenario = probe_json(serde_json::json!([
            { "name": "stag", "rule": "stagnant_resource",
              "resource": "Score", "path": "home", "driver_path": "hoem", "window_s": 0.5 }
        ]));
        let mut app = probe_game::app();
        let findings = lint_registry_probe(&mut app, &scenario);
        assert!(
            findings.iter().any(|f| f.to_string().contains("hoem")),
            "bogus driver_path on primary resource must be a finding: {findings:?}"
        );
    }
}
