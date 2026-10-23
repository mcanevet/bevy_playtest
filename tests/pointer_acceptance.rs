//! Acceptance tests for the synthetic_pointer bot: the UPSTREAM Bevy
//! `game_menu` showcase, clicked through the real picking chain.
//!
//! The fixture is the fetched showcase (`scripts/fetch_bevy_examples.sh`)
//! with auditable pointer-tier edits (see the script): the main-menu
//! Play and Settings buttons are named, and a world-level observer
//! logs `menu play button activated` on Play activation. The Settings
//! button is deliberately UN-instrumented: upstream it works (switches
//! to the settings screen) but emits no log — the natural negative
//! case, no fault injection needed. The binary is launched as a child
//! process under the in-process takeover (`BEVY_PLAYTEST_SCENARIO`),
//! the pointer bot clicks the button after the 1s splash screen, and
//! the report must show the observation and a clean pass.
//!
//! Prerequisite (same as tests/acceptance.rs):
//! ```text
//! cargo build --manifest-path fixtures/game_menu/Cargo.toml --bin fixture_game_menu
//! ```
#![cfg(all(feature = "takeover", feature = "pointer-input"))]

use std::path::PathBuf;
use std::process::Command;

use bevy_playtest::plugin::ENV_REPORT;
use bevy_playtest::plugin::ENV_SCENARIO;

/// Upstream game_menu shows a splash screen for 1 second (~61 frames
/// at 60 tps) before spawning the main menu buttons; click well after
/// the menu is up (asset loads are async, layout settles a few frames
/// after OnEnter(Menu)).
const CLICK_FRAME: u64 = 120;

const CLICK_SCENARIO: &str = r#"{
    "name": "pointer-acceptance",
    "bot": {
        "type": "synthetic_pointer",
        "seed": 42,
        "pointer_clicks": [
            {"frame": 120, "target": "menu_play_button", "hold_frames": 1}
        ]
    },
    "duration_s": 3.0,
    "tps": 60,
    "liveness": {"timeout_s": 10.0},
    "strict_coverage": true,
    "observes": ["menu play button activated"],
    "invariants": [],
    "setup": {}
}"#;

/// The natural negative case (NO fault injection): upstream Settings
/// works — it transitions to the settings screen — but emits no log.
/// Declaring the observation anyway must fail via the coverage
/// contract, proving unobservable buttons are caught, not silently
/// passed.
const UNINSTRUMENTED_SCENARIO: &str = r#"{
    "name": "pointer-acceptance-negative",
    "bot": {
        "type": "synthetic_pointer",
        "seed": 42,
        "pointer_clicks": [
            {"frame": 120, "target": "menu_settings_button", "hold_frames": 1}
        ]
    },
    "duration_s": 3.0,
    "tps": 60,
    "liveness": {"timeout_s": 10.0},
    "strict_coverage": true,
    "observes": ["settings screen entered"],
    "invariants": [],
    "setup": {}
}"#;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixture_binary() -> PathBuf {
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    // The fixture workspace shares target/ with the repo root (cargo
    // workspace conventions): built via
    // `cargo build --manifest-path fixtures/game_menu/Cargo.toml`.
    let bin = repo_root()
        .join("target")
        .join(profile)
        .join("fixture_game_menu");
    assert!(
        bin.is_file(),
        "fixture binary not found at {}.\n\
         Generate + build it first:\n\
         \x20 ./scripts/fetch_bevy_examples.sh\n\
         \x20 cargo build --manifest-path fixtures/game_menu/Cargo.toml --bin fixture_game_menu",
        bin.display()
    );
    bin
}

fn run_fixture(scenario: &str, bug: Option<&str>) -> (std::process::Output, serde_json::Value) {
    let dir = std::env::temp_dir().join(format!(
        "bevy_playtest_pointer_acc_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();

    let scenario_path = dir.join("scenario.json");
    std::fs::write(&scenario_path, scenario).unwrap();
    let report_path = dir.join("report.json");

    // Run from the fixture dir with BEVY_ASSET_ROOT pointing at its
    // assets/: bevy_asset resolves relative to the executable
    // (target/debug/) by default, which would log ERROR Path-not-found
    // and trip the log_error oracle.
    let mut cmd = Command::new(fixture_binary());
    cmd.current_dir(repo_root().join("fixtures").join("game_menu"))
        .env("BEVY_ASSET_ROOT", ".")
        .env(ENV_SCENARIO, &scenario_path)
        .env(ENV_REPORT, &report_path);
    if let Some(bug) = bug {
        cmd.env("FIXTURE_BUG", bug);
    }
    let output = cmd.output().expect("spawn fixture");

    let report = std::fs::read_to_string(&report_path)
        .map(|s| serde_json::from_str(&s).expect("report is valid JSON"))
        .unwrap_or(serde_json::Value::Null);
    (output, report)
}

/// Happy path: the pointer bot clicks the upstream main-menu Play
/// button through the real picking chain; the observation (`menu play
/// button activated`) is captured, no violations, exit 0 (Pass).
#[test]
fn pointer_click_lands_on_real_button() {
    let (output, report) = run_fixture(CLICK_SCENARIO, None);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "exit {:?}, stderr: {stderr}",
        output.status.code()
    );
    assert_eq!(report["status"], "pass", "report: {report:#?}");
    assert!(
        report["violations"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(false),
        "violations: {:#?}",
        report["violations"]
    );
    // The click must be OBSERVED: `observes: ["menu play button
    // activated"]` would surface a hole if the click never landed.
    let observed = report["coverage"]["observations"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|o| o["observed"].as_bool())
        .unwrap_or(false);
    assert!(
        observed,
        "click was not observed (picking chain broken?): {report:#?}"
    );
}

/// The run must not be vacuous: input_count proves the bot emitted
/// pointer input.
#[test]
fn pointer_bot_emits_input() {
    let (_output, report) = run_fixture(CLICK_SCENARIO, None);
    let input_count = report["metrics"]["input_count"].as_u64().unwrap_or(0);
    assert!(input_count >= 1, "input_count: {input_count}");
}

/// Unknown click target: the run must FAIL loudly (bot_config
/// violation), not pass silently. Exit 1, status fail.
#[test]
fn unknown_target_fails_loudly() {
    let scenario = CLICK_SCENARIO.replace("\"menu_play_button\"", "\"nonexistent\"");
    let (output, report) = run_fixture(&scenario, None);
    let code = output.status.code();
    assert_eq!(code, Some(1), "exit code must be 1 (Fail)");
    assert_eq!(report["status"], "fail");
    let violations = report["violations"].as_array().cloned().unwrap_or_default();
    assert!(
        violations.iter().any(|v| v["rule"] == "bot_config"),
        "expected bot_config violation, got: {violations:#?}"
    );
}

/// Natural negative case (no fault injection): clicking upstream's
/// Settings button is healthy behavior — it switches to the settings
/// screen — but emits no log. The declared observation must go
/// MISSING, failing the run loudly: unobservable-but-working UI must
/// not silently pass.
#[test]
fn uninstrumented_button_fails_via_missing_observation() {
    let (output, report) = run_fixture(UNINSTRUMENTED_SCENARIO, None);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() || output.status.code() == Some(1),
        "exit {:?}, stderr: {stderr}",
        output.status.code()
    );
    let observed = report["coverage"]["observations"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|o| o["observed"].as_bool())
        .unwrap_or(true);
    assert!(
        !observed,
        "uninstrumented Settings button must miss the observation: {report:#?}"
    );
    assert_eq!(
        report["status"], "fail",
        "missing observation must fail the run"
    );
}
