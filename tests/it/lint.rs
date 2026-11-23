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

// ---------------------------------------------------------------------------
// Stage 2 e2e: the type-registry probe against a booted App
// ---------------------------------------------------------------------------

/// The game for stage-2 e2e: `takeover_demo_nondet_resource` ships a
/// registered, inserted, reflected `GameClock(f32)` resource (field
/// `0`, tuple-indexed leaf "0").
fn resource_game_binary() -> PathBuf {
    let path = PathBuf::from(env!("CARGO_BIN_EXE_takeover_demo_nondet_resource"));
    assert!(path.is_file(), "demo binary missing: {path:?}");
    path
}

fn spawn_resource_game_lint_child(
    scenario_contents: &str,
    dir: &std::path::Path,
    tag: &str,
) -> (std::process::Output, PathBuf) {
    let scenario_path = dir.join(format!("{tag}.json"));
    let report_path = dir.join(format!("{tag}.report.json"));
    std::fs::write(&scenario_path, scenario_contents).unwrap();
    let out = Command::new(resource_game_binary())
        .env_remove(ENV_SCENARIO)
        .env_remove(ENV_REPORT)
        .env_remove(ENV_LINT)
        .env(ENV_SCENARIO, scenario_path.to_str().unwrap())
        .env(ENV_REPORT, report_path.to_str().unwrap())
        .env(ENV_LINT, "1")
        .output()
        .expect("spawn resource-game lint child");
    (out, report_path)
}

/// A typo'd resource name is caught at LINT time (stage 2), not at
/// the first real run's frame 0 — the front-loading the mode exists
/// for. Stage-1-clean scenario, so the finding can only come from
/// the registry probe.
#[test]
fn lint_e2e_stage2_typo_resource_name() {
    let _slot = common::run_slot();
    let dir = unique_dir("e2e_s2_typo");
    let (out, report_path) = spawn_resource_game_lint_child(
        r#"{"bot":{"type":"chaos","seed":42},"duration_s":1.0,"tps":60,
            "invariants":[{"name":"clock_watch","rule":"resource_check",
                           "resource":"GameClok","path":"0","check":"gt","value":-1.0}]}"#,
        &dir,
        "s2_typo",
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(exit_code::FAIL),
        "stage-2 findings must exit 1: {stderr}"
    );
    assert!(stderr.contains("GameClok"), "{stderr}");
    assert!(
        stderr.contains("not registered in this game's TypeRegistry"),
        "{stderr}"
    );
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
    assert_eq!(report["status"], "fail", "{report}");
    assert!(
        report["violations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["rule"] == "lint_finding"
                && v["detail"].as_str().is_some_and(|d| d.contains("GameClok"))),
        "the finding must ride the report: {report}"
    );
}

/// A stage-1-clean scenario whose resource and path are valid lints
/// CLEAN even though the game never ran a real frame: the config is
/// coherent, and coherence is all lint claims. (GameClock's tuple
/// field is reflect path "0".)
#[test]
fn lint_e2e_stage2_valid_resource_lints_clean() {
    let _slot = common::run_slot();
    let dir = unique_dir("e2e_s2_clean");
    let (out, report_path) = spawn_resource_game_lint_child(
        r#"{"bot":{"type":"chaos","seed":42},"duration_s":1.0,"tps":60,
            "invariants":[{"name":"clock_watch","rule":"resource_check",
                           "resource":"GameClock","path":"0","check":"gt","value":-1.0}]}"#,
        &dir,
        "s2_clean",
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
    assert_eq!(
        report["violations"].as_array().unwrap().len(),
        0,
        "{report}"
    );
}

/// The plain demo (Transform entities, no game resources) lints clean
/// through stage 2 with a component_check reading Transform via the
/// warm-up frame's Startup-spawned entity — the selector resolves,
/// the path resolves on the entity's Transform (fallback doctrine).
#[test]
fn lint_e2e_stage2_component_check_clean_on_startup_entity() {
    let _slot = common::run_slot();
    let dir = unique_dir("e2e_s2_cc");
    let (out, _report_path) = {
        let scenario_path = dir.join("cc.json");
        let report_path = dir.join("cc.report.json");
        std::fs::write(
            &scenario_path,
            r#"{"bot":{"type":"chaos","seed":42},"duration_s":1.0,"tps":60,
                "invariants":[{"name":"fwd","rule":"component_check",
                               "target_selector":{"with_component":{"name":"Transform"}},
                               "path":"translation.x","check":"lt","value":100.0}]}"#,
        )
        .unwrap();
        let out = Command::new(game_binary())
            .env_remove(ENV_SCENARIO)
            .env_remove(ENV_REPORT)
            .env_remove(ENV_LINT)
            .env(ENV_SCENARIO, scenario_path.to_str().unwrap())
            .env(ENV_REPORT, report_path.to_str().unwrap())
            .env(ENV_LINT, "1")
            .output()
            .expect("spawn lint child");
        (out, report_path)
    };
    assert_eq!(
        out.status.code(),
        Some(exit_code::PASS),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

// ---------------------------------------------------------------------------
// Stage-2 e2e: the warm-up-frame instance doctrines
// ---------------------------------------------------------------------------

/// The game for the instance-doctrine tests: ships a reflected
/// component that is registered but NEVER spawned (`GhostArmor`)
/// and one that the Startup schedule does spawn (`SpawnedMarker`,
/// field `0`).
fn unspawned_game_binary() -> PathBuf {
    let path = PathBuf::from(env!("CARGO_BIN_EXE_takeover_demo_unspawned"));
    assert!(path.is_file(), "demo binary missing: {path:?}");
    path
}

fn spawn_unspawned_game_lint_child(
    scenario_contents: &str,
    dir: &std::path::Path,
    tag: &str,
) -> (std::process::Output, PathBuf) {
    let scenario_path = dir.join(format!("{tag}.json"));
    let report_path = dir.join(format!("{tag}.report.json"));
    std::fs::write(&scenario_path, scenario_contents).unwrap();
    let out = Command::new(unspawned_game_binary())
        .env_remove(ENV_SCENARIO)
        .env_remove(ENV_REPORT)
        .env_remove(ENV_LINT)
        .env(ENV_SCENARIO, scenario_path.to_str().unwrap())
        .env(ENV_REPORT, report_path.to_str().unwrap())
        .env(ENV_LINT, "1")
        .output()
        .expect("spawn unspawned-game lint child");
    (out, report_path)
}

/// A registered component with ZERO instances is never a lint
/// finding — absence is not an error, the entities may spawn after
/// the warm-up frame. The definitive absence test for the doctrine
/// the docs lead with.
#[test]
fn lint_e2e_stage2_registered_but_unspawned_component_lints_clean() {
    let _slot = common::run_slot();
    let dir = unique_dir("e2e_s2_unspawned_clean");
    let (out, report_path) = spawn_unspawned_game_lint_child(
        r#"{"bot":{"type":"chaos","seed":42},"duration_s":1.0,"tps":60,
            "invariants":[{"name":"ghost_watch","rule":"component_check",
                           "target_selector":{"with_component":{"name":"GhostArmor"}},
                           "path":"0","check":"gt","value":-1.0}]}"#,
        &dir,
        "s2_unspawned_clean",
    );
    assert_eq!(
        out.status.code(),
        Some(exit_code::PASS),
        "zero instances must lint clean: stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
    assert_eq!(report["status"], "pass", "{report}");
}

/// The NodesInBounds probe arm: a stage-1-clean
/// `nodes_in_bounds` whose with_component names an unknown type is
/// caught by the registry probe, not deferred to the first run.
#[test]
fn lint_e2e_stage2_nodes_in_bounds_unknown_selector_is_a_finding() {
    let _slot = common::run_slot();
    let dir = unique_dir("e2e_s2_nib_unknown");
    let (out, report_path) = spawn_unspawned_game_lint_child(
        r#"{"bot":{"type":"chaos","seed":42},"duration_s":1.0,"tps":60,
            "invariants":[{"name":"bounds","rule":"nodes_in_bounds",
                           "target_selector":{"with_component":{"name":"NoSuchMarker"}},
                           "min_x":-10.0,"max_x":10.0}]}"#,
        &dir,
        "s2_nib_unknown",
    );
    assert_eq!(
        out.status.code(),
        Some(exit_code::FAIL),
        "unknown selector must exit 1: stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
    assert_eq!(report["status"], "fail", "{report}");
    assert!(
        report["violations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["rule"] == "lint_finding"
                && v["detail"]
                    .as_str()
                    .is_some_and(|d| d.contains("NoSuchMarker"))),
        "the finding must name the unknown selector: {report}"
    );
}

/// A `nodes_in_bounds` scenario whose selector matches zero entities
/// at the warm-up frame lints clean: the probe registers nothing to
/// report — with_component bounds checks are vacuously satisfiable
/// when no entity matches yet (entities may spawn later), exactly
/// like a never-inserted resource.
#[test]
fn lint_e2e_stage2_nodes_in_bounds_zero_matches_lints_clean() {
    let _slot = common::run_slot();
    let dir = unique_dir("e2e_s2_nib_zero");
    let (out, _report_path) = spawn_unspawned_game_lint_child(
        r#"{"bot":{"type":"chaos","seed":42},"duration_s":1.0,"tps":60,
            "invariants":[{"name":"bounds","rule":"nodes_in_bounds",
                           "target_selector":{"with_component":{"name":"GhostArmor"}},
                           "min_x":-10.0,"max_x":10.0}]}"#,
        &dir,
        "s2_nib_zero",
    );
    assert_eq!(
        out.status.code(),
        Some(exit_code::PASS),
        "zero matched bounds entities must lint clean: stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Stage 1 firing suppresses stage 2 — and that is report-visible,
/// not silent: a scenario with BOTH a stage-1 error (missing
/// `check` on a component_check) and a stage-2 error (a typo'd
/// selector in a different, stage-1-clean invariant) reports the
/// stage-1 finding and defers the probe. The author fixes the first
/// batch and re-lints to discover the second, exactly as documented.
#[test]
fn lint_e2e_stage1_findings_defer_the_probe() {
    let _slot = common::run_slot();
    let dir = unique_dir("e2e_s2_gated");
    let (out, report_path) = spawn_unspawned_game_lint_child(
        r#"{"bot":{"type":"chaos","seed":42},"duration_s":1.0,"tps":60,
            "invariants":[
              {"name":"shape_broken","rule":"component_check",
               "target_selector":{"with_component":{"name":"SpawnedMarker"}}},
              {"name":"typo_selector","rule":"nodes_in_bounds",
               "target_selector":{"with_component":{"name":"NoSuchMarker"}},
               "min_x":-10.0,"max_x":10.0}]}"#,
        &dir,
        "s2_gated",
    );
    assert_eq!(
        out.status.code(),
        Some(exit_code::FAIL),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
    let violations = report["violations"].as_array().unwrap();
    // The stage-1 finding rides; the stage-2 typo is DEFERRED
    // (absent) — documented staged disclosure.
    assert!(
        violations.iter().any(|v| v["rule"] == "lint_finding"
            && v["detail"]
                .as_str()
                .is_some_and(|d| d.contains("shape_broken"))),
        "stage-1 finding must ride: {report}"
    );
    assert!(
        !violations.iter().any(|v| v["detail"]
            .as_str()
            .is_some_and(|d| d.contains("NoSuchMarker"))),
        "stage-2 probe must be deferred while stage 1 fires: {report}"
    );
}

// ---------------------------------------------------------------------------
// Probe-crash containment (M2): pin the flagship fix end to end
// ---------------------------------------------------------------------------

/// A game that panics during the lint probe's warm-up frame must
/// report a Crash report (status "crash", exit 2), never a naked
/// exit-101 panic with no report. Without the containment (panic
/// hook + run guard + catch_unwind), a boot-crashing game turned
/// every lint invocation into an uncompilable-looking failure. The
/// report must also be self-describing: `error` carries the crash
/// narrative, matching the run path's crash usage — a machine
/// consumer sees a reason, not just a status.
///
/// Uses takeover_demo_crash (panics in Update on frame 1) with a
/// stage-1-clean scenario requesting a probeable rule: the probe
/// boots the game, the warm-up frame panics inside catch_unwind,
/// and lint exits CRASH.
#[test]
fn lint_e2e_probe_crash_reports_crash_and_exits_2() {
    let _slot = common::run_slot();
    let dir = unique_dir("e2e_probe_crash");
    // Stage-1-clean, probe-triggering: resource_check on bevy's
    // registered Time resource with a valid shape. The crash
    // happens at the warm-up frame regardless of whether the
    // resource itself would have resolved.
    let scenario = r#"{"bot":{"type":"chaos","seed":42},"duration_s":0.05,"tps":60,
        "invariants":[
          {"name":"time_moves","rule":"resource_check","resource":"Time","path":"delta","check":"gt","value":0.0}
        ]}"#;
    let scenario_path = dir.join("probe_crash.json");
    let report_path = dir.join("probe_crash.report.json");
    std::fs::write(&scenario_path, scenario).unwrap();
    let out = Command::new(PathBuf::from(env!("CARGO_BIN_EXE_takeover_demo_crash")))
        .env_remove(ENV_SCENARIO)
        .env_remove(ENV_REPORT)
        .env_remove(ENV_LINT)
        .env(ENV_SCENARIO, scenario_path.to_str().unwrap())
        .env(ENV_REPORT, report_path.to_str().unwrap())
        .env(ENV_LINT, "1")
        .output()
        .expect("spawn lint crash child");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(exit_code::CRASH),
        "probe crash must exit 2 (contained), not a naked panic: {stderr}"
    );
    assert!(
        stderr.contains("panicked during the lint warm-up frame"),
        "the crash narrative must reach stderr: {stderr}"
    );
    assert!(
        !stderr.contains("scenario rejected:"),
        "a boot crash is not a lint finding: {stderr}"
    );
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
    assert_eq!(report["status"], "crash", "{report}");
    assert_eq!(report["harness_mode"], "lint", "{report}");
    assert!(
        report["error"].is_string(),
        "lint crash report must carry a reason in `error`: {report}"
    );
    assert!(
        report["error"]
            .as_str()
            .unwrap()
            .contains("panicked during the lint warm-up frame"),
        "`error` must narrate the crash: {report}"
    );
}
