//! BRP auto-enable tests for PlaytestPlugin.
//!
//! The positive-path tests run under `takeover + brp-plugin`; the
//! featureless-rejection test runs under bare `takeover` (where the
//! demo bins are built WITHOUT brp-plugin — same cfg the user gets).
#![cfg(feature = "takeover")]

use std::path::PathBuf;
use std::process::Command;

use bevy_playtest::plugin::{exit_code, ENV_BRP_PORT};

#[cfg(feature = "brp-plugin")]
use bevy_playtest::plugin::{ENV_REPORT, ENV_SCENARIO};

#[cfg(feature = "brp-plugin")]
fn unique_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "bevy_playtest_plugin_brp_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[cfg(feature = "brp-plugin")]
#[test]
fn brp_port_invalid_exits_3() {
    let stock = PathBuf::from(env!("CARGO_BIN_EXE_takeover_demo_stock"));
    let out = Command::new(&stock)
        .env(ENV_BRP_PORT, "not-a-port")
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(exit_code::CONFIG_ERROR),
        "invalid port must exit 3"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("must be a valid port number"),
        "stderr: {stderr}"
    );
}

#[cfg(feature = "brp-plugin")]
#[test]
fn brp_port_valid_with_scenario_takes_over_cleanly() {
    let stock = PathBuf::from(env!("CARGO_BIN_EXE_takeover_demo_stock"));
    let dir = unique_dir("brp_ok");
    let report_path = dir.join("report.json");
    // Valid BRP_PORT + scenario: plugin adds remote plugins AND swaps
    // the runner. The BRP server binds; the takeover drives the app.
    // The smoke scenario passes regardless of BRP activity.
    let out = Command::new(&stock)
        .env(ENV_BRP_PORT, "0")
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

#[cfg(feature = "brp-plugin")]
#[test]
fn brp_port_is_known_env_not_unknown() {
    // Sanity: BRP_PORT (non-BEVY_PLAYTEST_ prefix) must never trip the
    // unknown-BEVY_PLAYTEST_-prefixed-var check.
    let stock = PathBuf::from(env!("CARGO_BIN_EXE_takeover_demo_stock"));
    let out = Command::new(&stock)
        .env(ENV_BRP_PORT, "9999")
        .env(ENV_SCENARIO, "smoke")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("unknown environment variable"),
        "BRP_PORT must be known: {stderr}"
    );
}

/// Runs under `--features takeover` WITHOUT brp-plugin: the demo bin
/// is built the same way, so BRP_PORT must fail loudly (exit 3).
#[cfg(all(feature = "takeover", not(feature = "brp-plugin")))]
#[test]
fn brp_port_without_feature_fails_loudly() {
    let stock = PathBuf::from(env!("CARGO_BIN_EXE_takeover_demo_stock"));
    let out = Command::new(&stock)
        .env(ENV_BRP_PORT, "9999")
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(exit_code::CONFIG_ERROR),
        "featureless BRP_PORT must exit 3"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--features brp-plugin"),
        "actionable hint required: {stderr}"
    );
}
