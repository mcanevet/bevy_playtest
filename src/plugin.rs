//! One-line game integration: `PlaytestPlugin`.
//!
//! Add the plugin to your `App`. When `BEVY_PLAYTEST_SCENARIO=<path>` is
//! set at process start, the plugin replaces the app's runner before the
//! windowed event loop can ever start: the scenario JSON is loaded,
//! `run_scenario` drives the (already-built) app to completion, the
//! report is written to `BEVY_PLAYTEST_REPORT` (default: scenario path
//! + `.report.json`), the path is printed, and the process exits with:
//!
//! - 0: Pass
//! - 1: Fail (gating violations)
//! - 2: Crash
//! - 3: harness/config error (unreadable scenario, validation failure,
//!   headless requested but winit is present)
//!
//! When the env var is absent the plugin is inert: no runner swap, no
//! resources, no systems, no env reads past the one `var_os` check.
//!
//! Because a `Plugin` cannot uninstall a `WinitPlugin` that a plugin
//! group already mounted, headless takeover of a windowed app is
//! detected (via `bevy_winit::WinitPlugin`) and refused loudly with
//! exit code 3 instead of silently opening a window in CI. Use
//! [`PlaytestDefaultPlugins`] as the drop-in replacement for
//! `DefaultPlugins` that handles the swap.
//!
//! Ordering: `PlaytestPlugin` must be added AFTER
//! `PlaytestDefaultPlugins` (e.g. `.add_plugins(PlaytestDefaultPlugins)
//! .add_plugins(PlaytestPlugin)`). `PlaytestPlugin` is the only side
//! that swaps the runner via `set_runner`, so a group added after it
//! does not overwrite the takeover; but plugin-group internals add
//! plugins in order, and a takeover installed before the group may be
//! built against an incomplete `App`. Bevy also refuses duplicate
//! unique plugins, so an accidental double-add fails loudly on its own.
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
/// Set: assert headlessness even without a scenario — exit 3 if
/// `bevy_winit::WinitPlugin` is present (contract must not degrade to a
/// silent windowed run).
pub const ENV_HEADLESS: &str = "BEVY_PLAYTEST_HEADLESS";

const KNOWN_ENV_VARS: [&str; 3] = [ENV_SCENARIO, ENV_REPORT, ENV_HEADLESS];

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
        match read_env_config() {
            Ok(Takeover::Inert) => {}
            Ok(Takeover::HeadlessAssert) => {
                assert_headless_possible(app);
            }
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
    assert_headless_possible(app);
    let mut report = match crate::driver::run_scenario(app, &scenario) {
        Ok(report) => report,
        Err(err) => {
            eprintln!("bevy_playtest: {err}");
            std::process::exit(exit_code::CONFIG_ERROR);
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

#[cfg(not(feature = "takeover-winit"))]
fn assert_headless_possible(_app: &App) {}

#[cfg(feature = "takeover-winit")]
fn assert_headless_possible(app: &App) {
    if app.is_plugin_added::<bevy_winit::WinitPlugin>() {
        eprintln!(
            "bevy_playtest: {} is set but the app contains bevy_winit::WinitPlugin; \
             a plugin cannot remove it. Replace DefaultPlugins with \
             bevy_playtest::plugin::PlaytestDefaultPlugins (or disable \
             WinitPlugin yourself) so the takeover can run headless.",
            ENV_SCENARIO
        );
        std::process::exit(exit_code::CONFIG_ERROR);
    }
}

/// `DefaultPlugins` equivalent that plays well with [`PlaytestPlugin`].
///
/// - No env vars set: identical to [`DefaultPlugins`] (pure pass-through).
/// - [`ENV_SCENARIO`] or [`ENV_HEADLESS`] set: drops `WinitPlugin` and
///   adds `ScheduleRunnerPlugin::run_loop` so the app ticks headless.
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
