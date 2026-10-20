//! End-to-end takeover tests for `PlaytestPlugin`.
//!
//! The "game" is `examples/takeover_demo.rs` (a tiny headless Bevy app
//! with PlaytestPlugin and a moving Gameplay entity), spawned as a child
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
    assert!(
        !report["violations"].as_array().unwrap().is_empty(),
        "{report}"
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
fn takeover_hard_fails_on_winit() {
    if !cfg!(feature = "takeover-winit") {
        return;
    }
    let dir = unique_dir("winit");
    let scenario_path = dir.join("scenario.json");
    std::fs::write(
        &scenario_path,
        r#"{"bot":{"type":"chaos","seed":42},"duration_s":0.05,"tps":60}"#,
    )
    .unwrap();
    let path = PathBuf::from(env!("CARGO_BIN_EXE_takeover_demo_winit"));
    let out = Command::new(&path)
        .env(ENV_SCENARIO, &scenario_path)
        .output()
        .expect("spawn winit example");
    assert_eq!(out.status.code(), Some(exit_code::CONFIG_ERROR));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("WinitPlugin"),
        "stderr must name winit: {stderr}"
    );
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
fn headless_env_with_winit_hard_fails() {
    let path = PathBuf::from(env!("CARGO_BIN_EXE_takeover_demo_winit"));
    let out = Command::new(&path)
        .env_remove(ENV_SCENARIO)
        .env("BEVY_PLAYTEST_HEADLESS", "1")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(exit_code::CONFIG_ERROR));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("WinitPlugin"),
        "stderr must name winit: {stderr}"
    );
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

// NOTE: a full e2e `bevy_playtest run` test would need to compile a temp
// cargo project with bevy dependencies; that cost is disproportionate
// for a thin wrapper that forwards env vars. The CLI contract is covered
// by unit tests in src/bin/bevy_playtest.rs; the smoke/file-path boundary
// is covered by the smoke_typo_is_treated_as_a_file_path test above.
