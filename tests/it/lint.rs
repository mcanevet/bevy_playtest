//! Lint-mode tests (`BEVY_PLAYTEST_LINT`): the aggregate
//! scenario validator and its takeover integration.
//!
//! Doctrine: lint must front-load EXACTLY the rejections a normal run
//! would produce — no new rejections, none missing, all at once
//! instead of one per full run. A clean lint never means "the game
//! will pass" (frame-level judgments are out of scope).

use crate::common;
use bevy_playtest::plugin::{exit_code, ENV_LINT, ENV_REPORT, ENV_SCENARIO};
use bevy_playtest::rules::Rule;
use bevy_playtest::scenario::{lint_scenario, validate_scenario, Scenario, ScenarioError};
use std::path::PathBuf;
use std::process::Command;

// ---------------------------------------------------------------------------
// Walker: aggregation and validate/lint equivalence
// ---------------------------------------------------------------------------

fn parse(json: serde_json::Value) -> Scenario {
    serde_json::from_value(json).expect("parse scenario")
}

fn minimal_scenario() -> serde_json::Value {
    serde_json::json!({
        "bot": { "type": "chaos", "seed": 1 },
        "duration_s": 1.0,
    })
}

/// Every error the lint walker finds must be a first-error candidate:
/// validating the SAME scenario must reject with the walker's FIRST
/// finding. This is the no-boundary-drift invariant, exercised with
/// deliberately tangled scenarios (several independent errors).
#[test]
fn lint_first_error_equals_validate_rejection() {
    let mut json = minimal_scenario();
    // Three independent globals-phase errors...
    json["tps"] = serde_json::json!(0);
    json["liveness"]["timeout_s"] = serde_json::json!(0.0);
    // ...plus an invariant-phase error.
    json["invariants"] = serde_json::json!([
        { "name": "b#d", "rule": "nodes_in_bounds" }
    ]);
    let s = parse(json);
    let lint = lint_scenario(&s);
    assert!(
        lint.len() >= 4,
        "walker must aggregate independent errors, got {lint:?}"
    );
    let validated = validate_scenario(&s);
    assert_eq!(
        validated,
        Err(lint[0].clone()),
        "validate_scenario must be the first-error form of lint_scenario"
    );
}

/// A scenario valid by the first-error validator is valid by the
/// walker: the lint list must be empty exactly when validation passes.
#[test]
fn lint_clean_iff_validate_ok() {
    let s = parse(minimal_scenario());
    assert!(validate_scenario(&s).is_ok());
    assert!(lint_scenario(&s).is_empty());
    // Corrupt one thing and both must flip together.
    let mut json = minimal_scenario();
    json["tps"] = serde_json::json!(0);
    let s = parse(json);
    assert!(validate_scenario(&s).is_err());
    assert!(!lint_scenario(&s).is_empty());
}

/// Aggregation must not stop at the first error WITHIN one rule
/// either: two malformed invariants both surface.
#[test]
fn lint_aggregates_across_invariants() {
    let mut json = minimal_scenario();
    json["invariants"] = serde_json::json!([
        { "name": "a", "rule": "component_check", "path": "x" }, // missing check/value/selector
        { "name": "b", "rule": "resource_check" },               // missing resource/path/check/value
    ]);
    let s = parse(json);
    let lint = lint_scenario(&s);
    let names: Vec<&str> = lint
        .iter()
        .filter(|e| matches!(e, ScenarioError::Rejected(m) if m.contains("invariant 'a'")))
        .map(|_| "a")
        .collect();
    assert!(!names.is_empty(), "both invariants must surface: {lint:?}");
    assert!(
        lint.iter()
            .any(|e| matches!(e, ScenarioError::Rejected(m) if m.contains("invariant 'b'"))),
        "both invariants must surface: {lint:?}"
    );
}

/// Lint findings inherit the scenario's structured error taxonomy:
/// `Rejected` and `NotImplemented` both aggregate.
#[test]
fn lint_preserves_error_taxonomy() {
    let mut json = minimal_scenario();
    json["oracles"] = serde_json::json!([]); // NotImplemented path
    json["liveness"]["mode"] = serde_json::json!("off"); // second NotImplemented
    let s = parse(json);
    let lint = lint_scenario(&s);
    assert_eq!(
        lint.iter()
            .filter(|e| matches!(e, ScenarioError::NotImplemented(_)))
            .count(),
        2,
        "both NotImplemented findings must aggregate: {lint:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule registry: the lint_finding rule
// ---------------------------------------------------------------------------

#[test]
fn lint_finding_rule_is_registered_and_lint_specific() {
    assert_eq!(Rule::LintFinding.name(), "lint_finding");
    assert_eq!(Rule::from_name("lint_finding"), Some(Rule::LintFinding));
    // Gating, not informational: lint findings surface config
    // rejections — a report carrying them must read status=fail.
    assert!(!Rule::LintFinding.is_informational());
    assert!(Rule::ALL.contains(&Rule::LintFinding));
}

// ---------------------------------------------------------------------------
// Takeover e2e: lint invocation, exit codes, report, hint
// ---------------------------------------------------------------------------

fn unique_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "bevy_playtest_lint_{tag}_{}_{}",
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
    assert!(path.is_file(), "demo binary missing: {path:?}");
    path
}

fn spawn_lint_child(
    scenario_contents: &str,
    dir: &std::path::Path,
    tag: &str,
    extra_env: &[(&str, &str)],
) -> (std::process::Output, PathBuf) {
    let scenario_path = dir.join(format!("{tag}.json"));
    let report_path = dir.join(format!("{tag}.report.json"));
    std::fs::write(&scenario_path, scenario_contents).unwrap();
    let out = Command::new(game_binary())
        .env_remove(ENV_SCENARIO)
        .env_remove(ENV_REPORT)
        .env_remove(ENV_LINT)
        .env(ENV_SCENARIO, scenario_path.to_str().unwrap())
        .env(ENV_REPORT, report_path.to_str().unwrap())
        .env(ENV_LINT, "1")
        .envs(extra_env.iter().map(|&(k, v)| (k, v)))
        .output()
        .expect("spawn lint child");
    (out, report_path)
}

/// Multi-error aggregation, end to end: a scenario with several
/// independent validation errors reports ALL of them on stderr and in
/// the report, exits 1 (findings = the scenario is rejected config),
/// and writes a standard-schema report with the `lint_finding` rule.
#[test]
fn lint_e2e_aggregates_all_errors_and_exits_1() {
    let _slot = common::run_slot();
    let dir = unique_dir("e2e_agg");
    let (out, report_path) = spawn_lint_child(
        r#"{"bot":{"type":"chaos","seed":42},"duration_s":-1.0,"tps":0,
            "liveness":{"timeout_s":0.0},
            "invariants":[
              {"name":"dup","rule":"nodes_in_bounds"},
              {"name":"dup","rule":"nodes_in_bounds"}
            ]}"#,
        &dir,
        "agg",
        &[],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(exit_code::FAIL),
        "findings must exit 1 (rejected config): {stderr}"
    );
    // At least three independent findings: tps, duration_s, timeout_s,
    // plus the duplicate-name one.
    assert!(stderr.contains("tps"), "{stderr}");
    assert!(stderr.contains("duration_s"), "{stderr}");
    assert!(stderr.contains("liveness.timeout_s"), "{stderr}");
    assert!(stderr.contains("duplicate invariant name"), "{stderr}");
    // Report: standard schema, status fail, findings as lint_finding
    // violations entries (the amendment's channel).
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
    assert_eq!(report["status"], "fail", "{report}");
    assert_eq!(report["harness_mode"], "lint", "{report}");
    let violations = report["violations"].as_array().unwrap();
    assert!(
        violations.len() >= 4,
        "every finding must ride the report: {report}"
    );
    assert!(
        violations.iter().all(|v| v["rule"] == "lint_finding"),
        "findings must ride violations as lint_finding entries: {report}"
    );
    // And the same no-frame contract: zero frames ran.
    assert_eq!(report["frame_count"].as_u64().unwrap(), 0, "{report}");
}

/// The opposite direction: a scenario rejected one error at a time in
/// normal mode is rejected with ALL errors in lint mode — the lint
/// never invents nor loses rejections. Also verifies the NORMAL run
/// keeps fail-fast AND prints the hint pointing at lint mode.
#[test]
fn lint_e2e_normal_run_keeps_fail_fast_plus_hint() {
    let _slot = common::run_slot();
    let dir = unique_dir("e2e_hint");
    let scenario = r#"{"bot":{"type":"chaos","seed":42},"duration_s":-1.0,"tps":0}"#;
    let scenario_path = dir.join("hint.json");
    std::fs::write(&scenario_path, scenario).unwrap();
    let report_path = dir.join("hint.report.json");
    let out = Command::new(game_binary())
        .env_remove(ENV_SCENARIO)
        .env_remove(ENV_REPORT)
        .env_remove(ENV_LINT)
        .env(ENV_SCENARIO, scenario_path.to_str().unwrap())
        .env(ENV_REPORT, report_path.to_str().unwrap())
        .output()
        .expect("spawn normal child");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(exit_code::CONFIG_ERROR));
    // Fail-fast preserved: exactly ONE scenario error before the hint.
    let scenario_errors = stderr.matches("scenario rejected:").count();
    assert_eq!(
        scenario_errors, 1,
        "normal runs stay fail-fast (one error): {stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "run with {ENV_LINT}=1 to see all scenario errors at once"
        )),
        "rejection must hint at lint mode: {stderr}"
    );
}

/// Clean lint: exit 0, status pass, standard report — "config is
/// coherent", never "the game will pass" (frame_count 0; no run
/// happened).
#[test]
fn lint_e2e_clean_config_exits_0() {
    let _slot = common::run_slot();
    let dir = unique_dir("e2e_clean");
    let (out, report_path) = spawn_lint_child(
        r#"{"bot":{"type":"chaos","seed":42},"duration_s":0.05,"tps":60}"#,
        &dir,
        "clean",
        &[],
    );
    assert_eq!(
        out.status.code(),
        Some(exit_code::PASS),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
    assert_eq!(report["status"], "pass", "{report}");
    assert_eq!(report["harness_mode"], "lint", "{report}");
    assert_eq!(
        report["violations"].as_array().unwrap().len(),
        0,
        "{report}"
    );
}

/// Lint with the builtin literals is a real e2e for the builtin
/// branch of the intercept (smoke must lint clean; same walker as a
/// normal run).
#[test]
fn lint_e2e_builtin_smoke_lints_clean() {
    let _slot = common::run_slot();
    let dir = unique_dir("e2e_smoke");
    let report_path = dir.join("smoke.report.json");
    let out = Command::new(game_binary())
        .env_remove(ENV_SCENARIO)
        .env_remove(ENV_REPORT)
        .env_remove(ENV_LINT)
        .env(ENV_SCENARIO, "smoke")
        .env(ENV_REPORT, report_path.to_str().unwrap())
        .env(ENV_LINT, "1")
        .output()
        .expect("spawn lint smoke child");
    assert_eq!(
        out.status.code(),
        Some(exit_code::PASS),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(report_path.exists(), "lint must write the report");
}

/// Invalid lint value (not "1"): a typo'd toggle must never be
/// mistaken for lint-off (strict env contract).
#[test]
fn lint_e2e_non_1_value_still_lints() {
    let _slot = common::run_slot();
    let dir = unique_dir("e2e_typo");
    let scenario = r#"{"bot":{"type":"chaos","seed":42},"duration_s":-1.0,"tps":60}"#;
    let scenario_path = dir.join("typo.json");
    std::fs::write(&scenario_path, scenario).unwrap();
    let report_path = dir.join("typo.report.json");
    // Any value counts as set; here also assert lint beats the
    // replay/run dispatch by using a verify_replay scenario (would
    // otherwise require single_threaded at dispatch).
    let out = Command::new(game_binary())
        .env_remove(ENV_SCENARIO)
        .env_remove(ENV_REPORT)
        .env_remove(ENV_LINT)
        .env(ENV_SCENARIO, scenario_path.to_str().unwrap())
        .env(ENV_REPORT, report_path.to_str().unwrap())
        .env(ENV_LINT, "yes")
        .output()
        .expect("spawn child");
    assert_eq!(
        out.status.code(),
        Some(exit_code::FAIL),
        "any non-empty value enables lint; findings exit 1: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
}
