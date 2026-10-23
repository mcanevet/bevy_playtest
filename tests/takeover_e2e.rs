//! End-to-end takeover tests for `PlaytestPlugin`.
//!
//! The "game" is `examples/takeover_demo.rs` (a tiny headless Bevy app
//! with PlaytestPlugin and a moving entity), spawned as a child
//! process per case.
//!
//! Cases:
//! - happy path: scenario env set -> exit 0 (Pass), report written
//! - fail path: invariant violation -> exit 1
//! - missing scenario file / invalid JSON / schema-rejected /
//!   unknown env var -> exit 3 with stderr diagnostics
//! - inert: no env var -> normal exit 0, no report written
//! - hard-fail: winit present alongside takeover -> exit 3 (covered by
//!   `takeover_demo_winit` when the takeover-winit feature is on)
#![cfg(feature = "takeover")]

use bevy_playtest::plugin::{exit_code, ENV_REPORT, ENV_SCENARIO};
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

fn unique_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "bevy_playtest_takeover_e2e_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn game_binary() -> PathBuf {
    let path = PathBuf::from(env!("CARGO_BIN_EXE_takeover_demo"));
    assert!(path.is_file(), "example binary missing: {path:?}");
    path
}

fn spawn_game(
    scenario: Option<&str>,
    report: Option<&str>,
    extra_env: &[(&str, &str)],
) -> std::process::Output {
    let mut cmd = Command::new(game_binary());
    cmd.env_remove(ENV_SCENARIO)
        .env_remove(ENV_REPORT)
        .env_remove("BEVY_PLAYTEST_HEADLESS");

    if let Some(scenario) = scenario {
        cmd.env(ENV_SCENARIO, scenario);
    }
    if let Some(report) = report {
        cmd.env(ENV_REPORT, report);
    }
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    cmd.output().expect("spawn example")
}

fn assert_stderr_contains(out: &std::process::Output, needle: &str) {
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(needle), "stderr: {stderr}");
}

#[test]
fn takeover_end_to_end_pass_and_fail() {
    let dir = unique_dir("outcome");

    let scenario_path = dir.join("scenario.json");
    let report_path = dir.join("report.json");
    std::fs::write(
        &scenario_path,
        r#"{"bot":{"type":"chaos","seed":42},"duration_s":0.05,"tps":60}"#,
    )
    .unwrap();
    let out = spawn_game(
        Some(scenario_path.to_str().unwrap()),
        Some(report_path.to_str().unwrap()),
        &[],
    );
    assert_eq!(
        out.status.code(),
        Some(exit_code::PASS),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("report written to"),
        "child must print the report path: {stdout}"
    );
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
    assert_eq!(report["status"], "pass", "{report}");
    assert!(report["frame_count"].as_u64().unwrap() >= 1, "{report}");

    let scenario_path = dir.join("fail.json");
    let report_path = dir.join("fail.report.json");
    std::fs::write(
        &scenario_path,
        r#"{"bot":{"type":"chaos","seed":42},"duration_s":0.05,"tps":60,
            "invariants":[{"name":"bounds","rule":"nodes_in_bounds",
                          "min_x":-1.0,"max_x":1.0}]}"#,
    )
    .unwrap();
    let out = spawn_game(
        Some(scenario_path.to_str().unwrap()),
        Some(report_path.to_str().unwrap()),
        &[],
    );
    assert_eq!(out.status.code(), Some(exit_code::FAIL));
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
    assert_eq!(report["status"], "fail", "{report}");
    // The fail must come from THE declared invariant (bounds), not from
    // an unrelated violation (e.g. a stray error log) — otherwise the
    // test proves nothing about invariant evaluation itself.
    let rules = all_rules(&report);
    assert!(
        rules.contains(&"nodes_in_bounds"),
        "expected nodes_in_bounds among {rules:?}: {report}"
    );
}

#[test]
fn takeover_config_errors() {
    let dir = unique_dir("errors");

    let out = spawn_game(Some("/nonexistent/scenario.json"), None, &[]);
    assert_eq!(out.status.code(), Some(exit_code::CONFIG_ERROR));
    assert_stderr_contains(&out, "cannot read scenario");

    let bad = dir.join("bad.json");
    std::fs::write(&bad, "{not json").unwrap();
    let out = spawn_game(Some(bad.to_str().unwrap()), None, &[]);
    assert_eq!(out.status.code(), Some(exit_code::CONFIG_ERROR));
    assert_stderr_contains(&out, "invalid scenario");

    let bad = dir.join("rejected.json");
    let mut f = std::fs::File::create(&bad).unwrap();
    writeln!(
        f,
        r#"{{"bot":{{"type":"chaos","seed":1}},"duration_s":-1.0,"tps":60}}"#
    )
    .unwrap();
    let out = spawn_game(Some(bad.to_str().unwrap()), None, &[]);
    assert_eq!(out.status.code(), Some(exit_code::CONFIG_ERROR));
    assert_stderr_contains(&out, "scenario");

    let out = spawn_game(
        Some("/nonexistent/scenario.json"),
        None,
        &[("BEVY_PLAYTEST_TURBO", "1")],
    );
    assert_eq!(out.status.code(), Some(exit_code::CONFIG_ERROR));
    assert_stderr_contains(&out, "BEVY_PLAYTEST_TURBO");
}

#[test]
fn takeover_inert_without_env() {
    let dir = unique_dir("inert");
    let sentinel = dir.join("sentinel");
    std::fs::write(&sentinel, b"x").unwrap();
    let out = spawn_game(None, Some(sentinel.to_str().unwrap()), &[]);
    assert_eq!(out.status.code(), Some(0), "inert run must exit normally");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("report written to"),
        "inert run must not write a report: {stdout}"
    );
}

#[test]
fn takeover_default_plugins_headless_succeeds() {
    if !cfg!(feature = "takeover-winit") {
        return;
    }
    let dir = unique_dir("winit_ok");
    let report_path = dir.join("report.json");
    let path = PathBuf::from(env!("CARGO_BIN_EXE_takeover_demo_winit"));
    let out = Command::new(&path)
        .env(ENV_SCENARIO, "smoke")
        .env(ENV_REPORT, report_path.to_str().unwrap())
        .output()
        .expect("spawn winit demo");
    assert_eq!(
        out.status.code(),
        Some(exit_code::PASS),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
    assert_eq!(report["status"], "pass", "{report}");
}

#[test]
fn headless_env_with_winit_hard_fails() {
    // Doc contract: BEVY_PLAYTEST_HEADLESS alone (no scenario, so no
    // runner swap) must refuse loudly if WinitPlugin is present —
    // a window would open otherwise. Uses the RAW-winit demo (stock
    // DefaultPlugins), not the PlaytestDefaultPlugins demos, which
    // drop WinitPlugin themselves and thus never hit this assert.
    //
    // Two loud outcomes are acceptable depending on host:
    //  - display-ful host (macOS dev machines): our assert_headless
    //    fires at PlaytestPlugin::build -> exit 3 with an explanatory
    //    message, before winit opens a window.
    //  - display-less host (Linux CI): bevy_winit 0.20 builds its
    //    event loop eagerly at DefaultPlugins-build time, BEFORE
    //    PlaytestPlugin builds, and panics (exit 101) with "Failed to
    //    build event loop". We never reach assert_headless. That is
    //    still a loud, non-window failure, so we accept it.
    if !cfg!(feature = "takeover-winit") {
        return;
    }
    let path = PathBuf::from(env!("CARGO_BIN_EXE_takeover_demo_raw_winit"));
    let out = Command::new(&path)
        .env_remove(ENV_SCENARIO)
        .env_remove(ENV_REPORT)
        .env("BEVY_PLAYTEST_HEADLESS", "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    let code = out.status.code();
    match code {
        Some(exit_code::CONFIG_ERROR) => assert!(
            stderr.contains("WinitPlugin") || stderr.contains("window"),
            "exit-3 stderr must explain the headless refusal: {stderr}"
        ),
        // bevy_winit's eager event-loop panic on display-less hosts
        // (see comment above); happens before our assert can fire.
        Some(101) => assert!(
            stderr.contains("Failed to build event loop"),
            "exit-101 must be the winit event-loop panic: {stderr}"
        ),
        other => panic!(
            "expected exit 3 (assert_headless) or 101 (winit event-loop \
             panic on display-less hosts), got {other:?}; \
             stdout={} stderr={}",
            String::from_utf8_lossy(&out.stdout),
            stderr
        ),
    }
}

#[test]
fn headless_env_alone_asserts_and_exits_normally() {
    let dir = unique_dir("headless");
    let out = Command::new(game_binary())
        .env_remove(ENV_SCENARIO)
        .env_remove(ENV_REPORT)
        .env("BEVY_PLAYTEST_HEADLESS", "1")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("report written to"),
        "headless-assert must not run a scenario: {stdout}"
    );
    let _ = dir;
}

#[test]
fn headless_env_with_winit_works() {
    let path = PathBuf::from(env!("CARGO_BIN_EXE_takeover_demo_winit"));
    let dir = unique_dir("headless_winit");
    let report_path = dir.join("report.json");
    let out = Command::new(&path)
        .env(ENV_SCENARIO, "smoke")
        .env(ENV_REPORT, report_path.to_str().unwrap())
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(exit_code::PASS),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
    assert_eq!(report["status"], "pass", "{report}");
}

#[test]
fn smoke_literal_runs_builtin_and_marks_provenance() {
    let dir = unique_dir("smoke");
    let report_path = dir.join("smoke.report.json");
    let out = Command::new(game_binary())
        .env(ENV_SCENARIO, "smoke")
        .env(ENV_REPORT, report_path.to_str().unwrap())
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(exit_code::PASS),
        "stdout: {}
stderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
    assert_eq!(report["status"], "pass", "{report}");
    assert_eq!(report["scenario_source"], "builtin_smoke", "{report}");
    assert_eq!(report["frame_count"].as_u64().unwrap(), 300, "{report}");
}

#[test]
fn smoke_report_defaults_to_cwd() {
    let dir = unique_dir("smoke_cwd");
    let out = Command::new(game_binary())
        .env(ENV_SCENARIO, "smoke")
        .current_dir(&dir)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(exit_code::PASS));
    let default_report = dir.join("bevy_playtest-smoke.report.json");
    assert!(default_report.is_file(), "smoke default report missing");
}

#[test]
fn smoke_typo_is_treated_as_a_file_path() {
    for wrong in ["SMOKE", "Smoke"] {
        let out = Command::new(game_binary())
            .env(ENV_SCENARIO, wrong)
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(exit_code::CONFIG_ERROR),
            "'{wrong}' must be file-path treated"
        );
        assert_stderr_contains(&out, "cannot read scenario");
    }
}

#[test]
fn all_selector_runs_on_stock_game_without_annotations() {
    let stock = PathBuf::from(env!("CARGO_BIN_EXE_takeover_demo_stock"));
    let dir = unique_dir("stock");

    let scenario_path = dir.join("scenario.json");
    let report_path = dir.join("report.json");
    std::fs::write(
        &scenario_path,
        r#"{"bot":{"type":"chaos","seed":42},"duration_s":0.05,"tps":60,
            "invariants":[{"name":"bounds","rule":"nodes_in_bounds",
                          "target_selector":"all",
                          "min_x":-5.0,"max_x":5.0}]}"#,
    )
    .unwrap();
    let out = Command::new(&stock)
        .env(ENV_SCENARIO, &scenario_path)
        .env(ENV_REPORT, &report_path)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(exit_code::PASS),
        "stock game with all-selector must pass: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
    assert_eq!(report["status"], "pass", "{report}");

    let tight = dir.join("tight.json");
    let tight_report = dir.join("tight.report.json");
    std::fs::write(
        &tight,
        r#"{"bot":{"type":"chaos","seed":42},"duration_s":0.05,"tps":60,
            "invariants":[{"name":"bounds","rule":"nodes_in_bounds",
                          "target_selector":"all",
                          "min_x":-0.5,"max_x":0.5}]}"#,
    )
    .unwrap();
    let out = Command::new(&stock)
        .env(ENV_SCENARIO, &tight)
        .env(ENV_REPORT, &tight_report)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(exit_code::FAIL));
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&tight_report).unwrap()).unwrap();
    assert_eq!(report["status"], "fail", "{report}");
}

#[test]
fn invariant_with_zero_entities_fires_vacuous_invariant() {
    let empty = PathBuf::from(env!("CARGO_BIN_EXE_takeover_demo_empty"));
    let dir = unique_dir("empty_vacuous");

    let scenario_path = dir.join("scenario.json");
    let report_path = dir.join("report.json");
    std::fs::write(
        &scenario_path,
        r#"{"bot":{"type":"chaos","seed":42},"duration_s":0.05,"tps":60,
            "invariants":[{"name":"bounds","rule":"nodes_in_bounds"}]}"#,
    )
    .unwrap();
    let out = Command::new(&empty)
        .env(ENV_SCENARIO, &scenario_path)
        .env(ENV_REPORT, &report_path)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(exit_code::FAIL),
        "invariant matching zero entities must Fail loudly"
    );
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
    let violations = report["violations"].as_array().unwrap();
    assert!(
        violations.iter().any(|v| v["rule"] == "vacuous_invariant"),
        "must flag vacuous_invariant: {report}"
    );
}

#[test]
fn replay_verification_passes_on_deterministic_stock_game() {
    let stock = PathBuf::from(env!("CARGO_BIN_EXE_takeover_demo_stock"));
    let dir = unique_dir("replay_ok");
    let scenario_path = dir.join("scenario.json");
    let report_path = dir.join("report.json");
    std::fs::write(
        &scenario_path,
        r#"{"bot":{"type":"chaos","seed":42},"duration_s":0.05,"tps":60,
            "verify_replay":true}"#,
    )
    .unwrap();
    let out = Command::new(&stock)
        .env(ENV_SCENARIO, &scenario_path)
        .env(ENV_REPORT, &report_path)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(exit_code::PASS),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
    assert_eq!(report["status"], "pass", "{report}");
}

#[test]
fn replay_divergence_fails_on_nondeterministic_stock_game() {
    let nondet = PathBuf::from(env!("CARGO_BIN_EXE_takeover_demo_nondet"));
    let dir = unique_dir("replay_div");
    let scenario_path = dir.join("scenario.json");
    let report_path = dir.join("report.json");
    std::fs::write(
        &scenario_path,
        r#"{"bot":{"type":"chaos","seed":42},"duration_s":0.05,"tps":60,
            "verify_replay":true}"#,
    )
    .unwrap();
    let out = Command::new(&nondet)
        .env(ENV_SCENARIO, &scenario_path)
        .env(ENV_REPORT, &report_path)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(exit_code::FAIL),
        "wall-clock nondeterminism must fail: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
    assert_eq!(report["status"], "fail", "{report}");
    let rules: Vec<&str> = report["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["rule"].as_str().unwrap())
        .collect();
    assert!(
        rules.contains(&"replay_divergence"),
        "expected replay_divergence among {rules:?}"
    );
}

/// A game whose only entity despawns just before the run ends is NOT
/// vacuous: the oracles observed it during the run. Regression test
/// for the post-hoc world-query vacuity guard (false positive).
#[test]
fn transient_entities_are_not_vacuous() {
    let dir = unique_dir("transient");
    let report_path = dir.join("report.json");
    let out = Command::new(PathBuf::from(env!("CARGO_BIN_EXE_takeover_demo_transient")))
        .env_remove(ENV_SCENARIO)
        .env_remove(ENV_REPORT)
        .env(ENV_SCENARIO, "smoke")
        .env(ENV_REPORT, &report_path)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(exit_code::PASS),
        "transient-entity game must pass: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
    assert_eq!(report["status"], "pass", "{report}");
    let rules = all_rules(&report);
    assert!(
        !rules.contains(&"vacuous_run"),
        "entities observed during the run — must not be vacuous: {report}"
    );
    assert!(
        !rules.contains(&"frozen_world"),
        "entity moved while alive — frozen_world must not fire: {report}"
    );
}

/// A game with NO entities at all is vacuous: a Pass on an empty world
/// proves nothing. The vacuity guard must turn it into a loud failure.
#[test]
fn empty_world_game_is_flagged_vacuous() {
    let dir = unique_dir("empty");
    let report_path = dir.join("report.json");
    let out = Command::new(PathBuf::from(env!("CARGO_BIN_EXE_takeover_demo_empty")))
        .env_remove(ENV_SCENARIO)
        .env_remove(ENV_REPORT)
        .env(ENV_SCENARIO, "smoke")
        .env(ENV_REPORT, &report_path)
        .output()
        .unwrap();
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
    let rules = all_rules(&report);
    assert!(
        rules.contains(&"vacuous_run") || rules.contains(&"frozen_world"),
        "empty game must fail loudly (vacuous_run or frozen_world): {report}"
    );
    assert_ne!(report["status"], "pass", "{report}");
    assert_eq!(out.status.code(), Some(exit_code::FAIL));
}

fn all_rules(report: &serde_json::Value) -> Vec<&str> {
    report["violations"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|v| v["rule"].as_str().unwrap_or("?"))
                .collect()
        })
        .unwrap_or_default()
}
