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
pub const ENV_SCENARIO: &str = "BEVY_PLAYTEST_SCENARIO";
/// Optional report output path (default: `<scenario>.report.json`).
pub const ENV_REPORT: &str = "BEVY_PLAYTEST_REPORT";
/// Set: assert headlessness even without a scenario. When set, the
/// plugin (added after DefaultPlugins) replaces winit's runner with a
/// schedule loop so no window opens and no display is needed.
pub const ENV_HEADLESS: &str = "BEVY_PLAYTEST_HEADLESS";
/// External-BRP port. When set, PlaytestPlugin adds RemotePlugin +
/// RemoteHttpPlugin and registers types for reflection queries.
pub const ENV_BRP_PORT: &str = "BRP_PORT";

const KNOWN_ENV_VARS: [&str; 6] = [
    ENV_SCENARIO,
    ENV_REPORT,
    ENV_HEADLESS,
    ENV_REPLAY_PASS_B,
    ENV_BRP_PORT,
    // Test-selection var (set by the acceptance suite / CI matrix to
    // pick one fixture); inherited by the game process harmlessly.
    "BEVY_PLAYTEST_FIXTURE",
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
        // BRP auto-enable: if BRP_PORT is set, add remote plugins and
        // register types for reflection queries. Invalid port → exit 3.
        if let Some(port_str) = std::env::var_os(ENV_BRP_PORT) {
            let port_str = port_str.to_string_lossy();
            let port: u16 = port_str.parse().unwrap_or_else(|_| {
                eprintln!(
                    "bevy_playtest: {} must be a valid port number (got '{}')",
                    ENV_BRP_PORT, port_str
                );
                std::process::exit(exit_code::CONFIG_ERROR);
            });

            #[cfg(not(feature = "brp-plugin"))]
            {
                let _ = port; // validated above; feature is the deciding factor
                eprintln!(
                    "bevy_playtest: {} is set but the brp-plugin feature is not enabled. \
                     Rebuild with --features brp-plugin to enable external-BRP mode.",
                    ENV_BRP_PORT
                );
                std::process::exit(exit_code::CONFIG_ERROR);
            }

            #[cfg(feature = "brp-plugin")]
            {
                // RemotePlugin requires the async task pools; a stock
                // game without DefaultPlugins hasn't initialized them.
                bevy_tasks::IoTaskPool::get_or_init(|| {
                    bevy_tasks::TaskPoolBuilder::default().build()
                });
                app.add_plugins(bevy_remote::RemotePlugin::default());
                app.add_plugins(bevy_remote::http::RemoteHttpPlugin::default().with_port(port));
                // Register types for reflection queries (mirrors the fixture shim)
                app.register_type::<bevy_transform::prelude::Transform>();
                app.register_type::<bevy_input::keyboard::KeyboardInput>();
                // Also add PlaytestBrpPlugin for method registration
                app.add_plugins(crate::brp_plugin::PlaytestBrpPlugin);
                // External-BRP mode should run headless (no window for
                // an automation-driven game). PlaytestDefaultPlugins
                // already installs the schedule runner; for plain
                // DefaultPlugins games add one here so no window opens
                // on display-ful hosts (display-less hosts need the
                // PlaytestDefaultPlugins swap — WinitPlugin builds its
                // event loop eagerly at plugin-build time).
                if !app.is_plugin_added::<bevy_app::ScheduleRunnerPlugin>() {
                    app.add_plugins(bevy_app::ScheduleRunnerPlugin::run_loop(
                        std::time::Duration::from_secs_f64(1.0 / 60.0),
                    ));
                }
            }
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
    app.set_runner(move |mut app: App| -> AppExit {
        run_takeover(&mut app, &config);
    });
}

fn run_takeover(app: &mut App, config: &TakeoverConfig) -> ! {
    let (scenario, source) = if config.scenario_path == SMOKE_LITERAL {
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
    // Replay verification needs TWO freshly built Apps. The takeover
    // already owns process lifetime and `set_runner` hands us the App
    // after plugin-build; a true rebuild requires re-running the
    // game's construction, which only the game's own entry point can
    // do. Mechanism chosen: for verify_replay scenarios the takeover
    // clones the App (Bevy 0.20 App: Clone) for pass B and runs the
    // scenario against clone and original — an approximate rebuild
    // that shares no mutable runtime state (resources/entities are
    // deep-copied), sufficient for the same-process determinism proof.
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
    println!("bevy_playtest: report written to {}", config.report_path);
    let code = match report.status {
        crate::enums::PlaytestStatus::Pass => exit_code::PASS,
        crate::enums::PlaytestStatus::Fail => exit_code::FAIL,
        crate::enums::PlaytestStatus::Crash => exit_code::CRASH,
    };
    std::process::exit(code)
}

/// Case-sensitive magic literal for the built-in smoke scenario.
pub const SMOKE_LITERAL: &str = "smoke";

/// The built-in smoke scenario: chaos bot, no authored invariants (the
/// global oracles still run), ~5s at default tps. Constructed in code
/// through the same `validate_scenario` path as a parsed file so it can
/// never drift from the schema contract.
pub fn smoke_scenario() -> Result<crate::scenario::Scenario, String> {
    let scenario: crate::scenario::Scenario = serde_json::from_str(
        r#"{"name":"smoke","bot":{"type":"chaos","seed":42},"duration_s":5.0}"#,
    )
    .map_err(|err| format!("built-in smoke scenario failed to parse: {err}"))?;
    crate::scenario::validate_scenario(&scenario)
        .map_err(|err| format!("built-in smoke scenario failed validation: {err}"))?;
    Ok(scenario)
}

/// Env var marking a replay-verification PASS-B child process: the
/// takeover skips verification (avoiding recursion), runs one pass,
/// writes the fingerprint series to the path in the value, and exits.
const ENV_REPLAY_PASS_B: &str = "BEVY_PLAYTEST_REPLAY_PASS_B_SERIES";

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
        std::process::exit(0);
    }

    // Pass B: re-run the game binary as a child with the same env.
    let exe = std::env::current_exe()
        .map_err(|err| format!("cannot locate current executable for replay pass B: {err}"))?;
    let series_path = format!("{report_path}.replay-series-b.json");
    let output = std::process::Command::new(&exe)
        .env_remove("BEVY_PLAYTEST_REPORT")
        .env(ENV_REPLAY_PASS_B, &series_path)
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
    if report.status != crate::enums::PlaytestStatus::Crash {
        if let Err(div) = crate::replay::compare(&series_a, &frames_b) {
            report.violations.push(crate::state::ViolationEntry {
                rule: crate::rules::Rule::ReplayDivergence.name().to_string(),
                target: "run".to_string(),
                first_frame: div.frame,
                last_frame: div.frame,
                count: 1,
                detail: format!(
                    "first divergent frame {}: pass A fingerprint {:#018x}, pass B \
                     fingerprint {:#018x} — hidden nondeterminism (wall clock, \
                     unseeded RNG, or unordered iteration)",
                    div.frame, div.pass_a_hash, div.pass_b_hash
                ),
                last_detail: format!(
                    "divergent frame {} ({:#018x} vs {:#018x})",
                    div.frame, div.pass_a_hash, div.pass_b_hash
                ),
            });
            report.status = crate::enums::PlaytestStatus::Fail;
        }
    }
    Ok(report)
}

fn series_fingerprints(series: &[crate::replay::FrameFingerprint]) -> Vec<(u64, u64)> {
    series.iter().map(|f| (f.frame, f.hash)).collect()
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
/// - [`ENV_SCENARIO`], [`ENV_HEADLESS`], or [`ENV_BRP_PORT`] set: drops
///   `WinitPlugin` and adds `ScheduleRunnerPlugin::run_loop` so the app
///   ticks headless. (bevy_winit 0.20 builds its event loop eagerly in
///   `WinitPlugin::build` — a display-less host panics before any
///   runner swap could intervene, so removing the plugin is the only
///   reliable headless path.)
///
/// Requires the `takeover-winit` feature (adds the `bevy_winit` and
/// `bevy` (DefaultPlugins) dependencies).
#[cfg(feature = "takeover-winit")]
pub struct PlaytestDefaultPlugins;

#[cfg(feature = "takeover-winit")]
impl bevy_app::prelude::PluginGroup for PlaytestDefaultPlugins {
    fn build(self) -> bevy_app::PluginGroupBuilder {
        let builder = bevy::DefaultPlugins.build();
        if std::env::var_os(ENV_SCENARIO).is_some()
            || std::env::var_os(ENV_HEADLESS).is_some()
            || std::env::var_os(ENV_BRP_PORT).is_some()
        {
            builder.disable::<bevy_winit::WinitPlugin>().add(
                bevy_app::ScheduleRunnerPlugin::run_loop(std::time::Duration::from_secs_f64(
                    1.0 / 60.0,
                )),
            )
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
}
