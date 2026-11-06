//! Discovery-flow end-to-end tests: the full Phase 7 orchestrator
//! (cache → probe pool → determinism gate → diff aggregation → group
//! completion → cache write) against `takeover_demo_input` — the
//! input-delivery demo game whose world moves only on
//! `just_pressed(Digit1)` — as REAL takeover subprocesses.
//!
//! Two contracts pinned here:
//!
//! - `discover()` learns the right surface: Digit1 effective with
//!   `Exact` confidence, the control key KeyQ (which the slim
//!   matrix deliberately probes) marked `Unclassified` — never a
//!   trusted negative — and a second run answers from the CACHE with
//!   the identical surface.
//! - The battery opt-in (`BEVY_PLAYTEST_DISCOVER=1`) attaches the
//!   surface to the battery report and annotates frozen-world
//!   findings with `_meta` probe statistics — using the env-var
//!   candidate/cache overrides so the matrix stays slim.
//!
//! Env mutation is serialized (run_slot) and restored via a drop
//! guard: the opt-in variables are battery-parent-side only — both
//! the battery and the pool strip every `BEVY_PLAYTEST_*` variable
//! from spawned children, so no takeover child can reject them.
//!
//! Takeover-gated like every other `CARGO_BIN_EXE_takeover_demo_input`
//! consumer: without the `takeover` feature the demo binary is not
//! built (`required-features`), and this `env!` would fail to compile
//! under `cargo test --no-default-features`.

#![cfg(feature = "takeover")]

mod common;

use std::path::PathBuf;

use bevy_playtest::battery::{run_with_config, BatteryConfig, BUILTIN_BATTERY};
use bevy_playtest::discovery::flow::{
    discover, discover_from_env, DiscoveryConfig, DiscoveryError, ENV_DISCOVER, ENV_DISCOVER_CACHE,
    ENV_DISCOVER_INJECT, ENV_DISCOVER_KEYS,
};
use bevy_playtest::discovery::surface::{KeyConfidence, SurfaceConfidence};

fn demo_binary() -> &'static str {
    env!("CARGO_BIN_EXE_takeover_demo_input")
}

/// Unique temp dir (pid + nanos + counter — the counter because
/// sibling tests can start within the same coarse clock tick).
fn temp_dir() -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "bevy_playtest_flow_test_{}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        seq,
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Restores the previous values of the discovery env vars on drop
/// (panic-safe: a failed assertion must not leak opt-in state into
/// the other tests of this binary).
struct EnvGuard {
    saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl EnvGuard {
    fn set(vars: &[(&'static str, &str)]) -> Self {
        let saved = vars
            .iter()
            .map(|(k, _)| (*k, std::env::var_os(k)))
            .collect();
        for (k, v) in vars {
            std::env::set_var(k, v);
        }
        Self { saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (k, v) in self.saved.drain(..) {
            match v {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
    }
}

/// Slim two-key matrix: 2 keys × 2 points + 2 points × 2 controls =
/// 8 subprocesses (~5s at pool=8). Candidates cover the interesting
/// cases (bound Digit1, plus the control key KeyQ — probing the
/// control against itself is structurally unclassifiable, and the
/// surface must SAY so, not report a trusted negative).
fn slim_config() -> DiscoveryConfig {
    let mut config = DiscoveryConfig::new(demo_binary());
    config.plan.candidate_keys = vec!["Digit1".into(), "KeyQ".into()];
    config.cache_dir = temp_dir();
    config
}

#[test]
fn discovers_bound_key_and_answers_second_run_from_cache() {
    let _slot = common::run_slot();
    let config = slim_config();

    // Cold run: full matrix, real subprocesses.
    let cold = discover(&config).expect("cold discovery");
    assert!(!cold.from_cache, "fresh cache dir must probe");

    // The bound key is effective with an observed effect.
    let digit1 = cold
        .surface
        .keys
        .iter()
        .find(|k| k.key == "Digit1")
        .expect("Digit1 was probed");
    assert!(
        digit1.effect.is_some(),
        "Digit1 moves the world — its probe must diverge from the control"
    );
    assert_eq!(
        cold.surface.confidence_for("Digit1"),
        Some(KeyConfidence::Exact)
    );

    // The control key stays visible but NEVER as a trusted negative:
    // its probe run equals its control baseline by construction.
    let keyq = cold
        .surface
        .keys
        .iter()
        .find(|k| k.key == "KeyQ")
        .expect("KeyQ was probed");
    assert_eq!(keyq.effect, None, "KeyQ must not perturb the demo game");
    assert_eq!(
        cold.surface.confidence_for("KeyQ"),
        Some(KeyConfidence::Unclassified),
        "the control key cannot classify itself"
    );

    // The determinism gate passed (deterministic demo, clean controls).
    assert_eq!(cold.surface.confidence, SurfaceConfidence::Exact);

    // Both probe points were learned.
    assert_eq!(cold.surface.probed_at.len(), 2);

    // Warm run: same surface, zero subprocesses (cache hit).
    let warm = discover(&config).expect("warm discovery");
    assert!(warm.from_cache, "second run must answer from the cache");
    assert_eq!(warm.surface, cold.surface, "cached surface is identical");

    std::fs::remove_dir_all(&config.cache_dir).unwrap();
}

#[test]
fn cold_discovery_carries_child_exported_scan_data() {
    let _slot = common::run_slot();
    // Copilot r4466455064: a surface produced by a COLD discovery
    // run must deliver the advertised static-scan annotations — the
    // probe child runs the scanner on its live App (via
    // ENV_DISCOVER_SCAN), and the flow merges the export into the
    // surface. Before the fix every surface shipped empty
    // `readers`/`pruned_devices` and only the hard-coded
    // "static scan not run" note.
    let config = slim_config();
    let cold = discover(&config).expect("cold discovery");
    assert!(!cold.from_cache, "fresh cache dir must probe");

    // Reader data arrived: the demo reads ButtonInput<KeyCode> and
    // keyboard_input_system writes it, so the keyboard class must
    // show at least one reader.
    let keyboard = cold
        .surface
        .readers
        .iter()
        .find(|(class, _)| class == "keyboard")
        .map(|(_, n)| *n)
        .expect("the child scan must deliver reader counts");
    assert!(
        keyboard >= 1,
        "keyboard must have readers: {:?}",
        cold.surface.readers
    );

    // No reader anywhere ⇒ gamepad/wheel derived as pruned. The
    // demo never touches either, and its engine drivers are the
    // schedule runner + message drain (neither is exclusive-visible
    // as input readers... the scan decides; we only pin the DERIVED
    // consequence of whatever the scan reported for gamepad).
    let gamepad_readers = cold
        .surface
        .readers
        .iter()
        .find(|(class, _)| class == "gamepad")
        .map(|(_, n)| *n);
    if gamepad_readers == Some(0) {
        assert!(
            cold.surface.pruned_devices.iter().any(|d| d == "gamepad"),
            "a readerless gamepad class must be derived as pruned: {:?}",
            cold.surface.pruned_devices
        );
    } else {
        assert!(
            !cold.surface.pruned_devices.iter().any(|d| d == "gamepad"),
            "gamepad with readers (or absent from the scan) must not be pruned: {:?}",
            cold.surface.pruned_devices
        );
    }
    // Keyboard is structurally exempt from pruning — regardless of
    // the scan's keyboard entry (fail-open rule).
    assert!(
        !cold.surface.pruned_devices.iter().any(|d| d == "keyboard"),
        "keyboard must never be pruned: {:?}",
        cold.surface.pruned_devices
    );

    // The not-run note is gone — the run delivered real scan data.
    assert!(
        !cold
            .surface
            .notes
            .iter()
            .any(|n| n.contains("static scan not run")),
        "a delivered scan must replace the not-run note: {:?}",
        cold.surface.notes
    );

    std::fs::remove_dir_all(&config.cache_dir).unwrap();
}

#[test]
fn fixture_identity_participates_in_the_cache_key() {
    let _slot = common::run_slot();
    // Copilot r4464792287: the fixture battery compiles ONE binary
    // for all of its `tests/playtest.rs` cases; the discovery cache
    // key must tell the scenarios apart or the FIRST scenario's
    // surface gets handed to every subsequent one. `discover_from_env`
    // threads the caller's fixture identity into the key, so two
    // discoveries of the same binary under DIFFERENT identities
    // must never share a cache entry, while a repeat under the SAME
    // identity must hit.
    let cache_root = temp_dir();
    let _env = EnvGuard::set(&[
        (ENV_DISCOVER, "1"),
        (ENV_DISCOVER_CACHE, cache_root.to_str().unwrap()),
        (ENV_DISCOVER_KEYS, "Digit1"),
    ]);

    // Cold under "breakout": full matrix, real subprocesses.
    let breakout_cold = discover_from_env(demo_binary().as_ref(), Some("breakout"))
        .expect("discovery under breakout identity");
    assert!(!breakout_cold.from_cache, "fresh cache must probe");

    // Same binary, same identity: cache hit, identical surface.
    let breakout_warm = discover_from_env(demo_binary().as_ref(), Some("breakout"))
        .expect("warm discovery under breakout identity");
    assert!(
        breakout_warm.from_cache,
        "same fixture identity must answer from the cache"
    );
    assert_eq!(breakout_warm.surface, breakout_cold.surface);

    // Same binary, DIFFERENT identity: distinct cache entry — must
    // re-probe, never hand breakout's surface to mines.
    let mines_cold = discover_from_env(demo_binary().as_ref(), Some("mines"))
        .expect("discovery under mines identity");
    assert!(
        !mines_cold.from_cache,
        "a different fixture identity must NOT reuse the other fixture's entry"
    );
    assert_eq!(
        mines_cold.surface, breakout_cold.surface,
        "same binary ⇒ same surface content; only the cache entries differ"
    );

    std::fs::remove_dir_all(&cache_root).unwrap();
}

#[test]
fn alias_override_normalizes_to_canonical_names() {
    let _slot = common::run_slot();
    // `BEVY_PLAYTEST_DISCOVER_KEYS=digit1, keyq` must probe the
    // SAME matrix as `Digit1,KeyQ`: aliases normalize to canonical
    // spellings at plan-build time, so the surface vocabulary and
    // the cache plan fingerprint stay canonical. Raw alias strings
    // leaking into `KeyFinding.key` would fragment the surface
    // vocabulary and multiply cache entries.
    let dir = temp_dir();
    let _env = EnvGuard::set(&[
        (ENV_DISCOVER_KEYS, "digit1, keyq"),
        (ENV_DISCOVER_CACHE, dir.join("cache").to_str().unwrap()),
    ]);

    let outcome =
        discover_from_env(demo_binary().as_ref(), None).expect("alias override must discover");
    assert!(!outcome.from_cache);
    assert!(
        outcome
            .surface
            .keys
            .iter()
            .all(|k| k.key == "Digit1" || k.key == "KeyQ"),
        "aliases must normalize to canonical names: {:?}",
        outcome.surface.keys
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn empty_keys_override_is_an_error_not_default_matrix() {
    let _slot = common::run_slot();
    // `BEVY_PLAYTEST_DISCOVER_KEYS=""` (or whitespace-only) is
    // explicit user intent to probe NOTHING — falling back to the
    // full ~35s default matrix would silently do the opposite of
    // what was asked. Must surface as a Setup error.
    let _env = EnvGuard::set(&[(ENV_DISCOVER_KEYS, "  ")]);

    let err = discover_from_env(demo_binary().as_ref(), None)
        .expect_err("explicit empty override must be an error");
    assert!(
        matches!(err, DiscoveryError::Setup(_)),
        "empty override is a setup error, got: {err:?}"
    );
    assert!(
        err.to_string().contains(ENV_DISCOVER_KEYS),
        "error names the variable: {err}"
    );
}

#[test]
fn unknown_keys_override_is_an_error_not_a_silent_drop() {
    let _slot = common::run_slot();
    // A typo'd key must never be silently dropped from the matrix
    // (the user asked to probe it; a "not found" answer would be a
    // lie) — nor silently accept garbage into the plan.
    let _env = EnvGuard::set(&[(ENV_DISCOVER_KEYS, "Digit1,NoSuchKey")]);

    let err =
        discover_from_env(demo_binary().as_ref(), None).expect_err("unknown key must be an error");
    assert!(
        matches!(err, DiscoveryError::Setup(_)),
        "unknown key is a setup error, got: {err:?}"
    );
    assert!(
        err.to_string().contains("NoSuchKey"),
        "error names the offending key: {err}"
    );
}

#[test]
fn cache_disabled_after_dir_removal() {
    // rm -rf .bevy_playtest/cache must force re-discovery: the cache
    // dir (not just the entry) is fair game to delete.
    let _slot = common::run_slot();
    let config = slim_config();
    discover(&config).expect("cold discovery");
    std::fs::remove_dir_all(&config.cache_dir).unwrap();

    let again = discover(&config).expect("re-discovery");
    assert!(!again.from_cache, "a deleted cache dir must re-probe");
    assert!(
        again
            .surface
            .keys
            .iter()
            .find(|k| k.key == "Digit1")
            .unwrap()
            .effect
            .is_some(),
        "re-discovery learns the same bound key"
    );
    std::fs::remove_dir_all(&config.cache_dir).unwrap();
}

#[test]
fn battery_opt_in_attaches_surface_and_annotates_findings() {
    let _slot = common::run_slot();

    // A quiet scenario: the bot taps ONLY KeyQ — the demo world never
    // awakens (no Digit1 press), so frozen_world fires and the
    // battery report has the finding that discovery must annotate.
    let dir = temp_dir();
    let scenario_path = dir.join("quiet.json");
    std::fs::write(
        &scenario_path,
        r#"{"name":"quiet","bot":{"type":"synthetic_keyboard","seed":1,
             "key_presses":[{"frame":30,"key":"KeyQ","hold_frames":1}]},
             "duration_s":3.0,"liveness":{"timeout_s":1.0}}"#,
    )
    .unwrap();

    let _env = EnvGuard::set(&[
        (ENV_DISCOVER, "1"),
        (ENV_DISCOVER_KEYS, "Digit1,KeyQ"),
        (ENV_DISCOVER_CACHE, dir.join("cache").to_str().unwrap()),
    ]);

    let report = run_with_config(&BatteryConfig {
        binary: demo_binary().to_string(),
        scenario: scenario_path.to_string_lossy().into_owned(),
        timeout_secs: 300,
    })
    .expect("battery run with discovery");

    // The surface rode along on the report.
    let surface = report
        .discovered_surface
        .as_ref()
        .expect("opt-in battery run carries the discovered surface");
    assert_eq!(surface.confidence, SurfaceConfidence::Exact);
    assert_eq!(surface.confidence_for("Digit1"), Some(KeyConfidence::Exact));
    // The slim matrix includes the control key: visible, but excluded
    // from the probed count (no differential verdict about it exists).
    assert_eq!(
        surface.confidence_for("KeyQ"),
        Some(KeyConfidence::Unclassified)
    );
    assert_eq!(
        surface.probed_key_count(),
        1,
        "only Digit1 was informatively probed"
    );

    // The frozen-world finding is annotated with `_meta` probe stats —
    // next to its verbatim detail, never inside it.
    let frozen = report
        .violations
        .iter()
        .find(|v| v.rule == "frozen_world")
        .expect("quiet scenario freezes the demo world");
    let meta = frozen
        .meta
        .as_ref()
        .expect("frozen_world must be annotated when discovery ran");
    assert_eq!(meta.probed_keys_total, Some(1));
    assert_eq!(meta.probed_keys_effective, Some(1));
    assert!(!frozen.detail.contains("probed"), "detail stays verbatim");

    // The battery's scenario identity threaded into the cache key
    // (Copilot r4464792287): this run's cache entry must live under
    // the scenario file's name, never under the default "game" slug
    // — otherwise same-binary fixtures would share entries.
    let entries: Vec<std::ffi::OsString> = std::fs::read_dir(dir.join("cache"))
        .expect("cache dir exists after a cold discovery")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name())
        .collect();
    assert!(
        entries
            .iter()
            .any(|n| n.to_string_lossy().contains("quiet")),
        "battery cache entry must carry the scenario identity: {:?}",
        entries
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn injection_feeds_discovered_keys_to_the_builtin_battery() {
    let _slot = common::run_slot();

    // The e2e of discovery-driven injection, on the game shaped
    // exactly like the finding that motivated it: with
    // DISCOVER_INJECT on and the BUILT-IN battery literal (an
    // undeclared surface), the parent must discover first, feed the
    // effective keys to the chaos bot via an injected scenario, and
    // say so on the report.
    let dir = temp_dir();
    let _env = EnvGuard::set(&[
        (ENV_DISCOVER_INJECT, "1"),
        (ENV_DISCOVER_KEYS, "Digit1,KeyQ"),
        (ENV_DISCOVER_CACHE, dir.join("cache").to_str().unwrap()),
    ]);

    let report = run_with_config(&BatteryConfig {
        binary: demo_binary().to_string(),
        scenario: BUILTIN_BATTERY.to_string(),
        timeout_secs: 300,
    })
    .expect("battery run with injection");

    // Injected exactly the discovered surface: Digit1 (the one bound,
    // effective key) — and NOT KeyQ (control key, never injectable)
    // nor any inert default (WASD+arrows would have been the blind
    // set this feature exists to replace).
    assert_eq!(
        report.injected_surface_keys,
        Some(vec!["Digit1".to_string()]),
        "injection must record the discovered surface on the report"
    );
    // The surface rides along (injection implies discovery — the
    // same surface feeds the bot and annotates the report).
    let surface = report
        .discovered_surface
        .as_ref()
        .expect("injection implies discovery annotation");
    assert_eq!(surface.confidence_for("Digit1"), Some(KeyConfidence::Exact));
    // The injected run played the game instead of ignoring it: the
    // chaos bot drew key presses during the run. Action-log entries
    // record the SURFACE INDEX (`key_press:<idx>`) — with the
    // injected surface being exactly ["Digit1"] (asserted above),
    // every key draw is a Digit1 press.
    assert!(
        !report.action_log.is_empty(),
        "the injected chaos bot must actually press keys during the run"
    );
    assert!(
        report.action_log.iter().all(|a| a.action == "key_press:0"),
        "every key draw must index into the injected surface: {:?}",
        report
            .action_log
            .iter()
            .map(|a| a.action.as_str())
            .collect::<Vec<_>>()
    );
    // Second run on the same cache: warm — proves the discovery
    // before injection uses the cache-first path.
    let report = run_with_config(&BatteryConfig {
        binary: demo_binary().to_string(),
        scenario: BUILTIN_BATTERY.to_string(),
        timeout_secs: 300,
    })
    .expect("warm battery run with injection");
    assert_eq!(
        report.injected_surface_keys,
        Some(vec!["Digit1".to_string()])
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn injection_never_overrides_an_authored_scenario() {
    let _slot = common::run_slot();

    // Declaration beats inference: an authored scenario file wins
    // even with DISCOVER_INJECT set — injection targets ONLY the
    // built-in battery literal, whose surface is undeclared. (This
    // is also the report-honesty negative: injected_surface_keys
    // must be None, never a stale list.)
    let dir = temp_dir();
    let scenario_path = dir.join("authored.json");
    std::fs::write(
        &scenario_path,
        r#"{"name":"authored","bot":{"type":"synthetic_keyboard","seed":1,
             "key_presses":[{"frame":30,"key":"KeyQ","hold_frames":1}]},
             "duration_s":3.0,"liveness":{"timeout_s":1.0}}"#,
    )
    .unwrap();

    let _env = EnvGuard::set(&[
        (ENV_DISCOVER_INJECT, "1"),
        (ENV_DISCOVER_KEYS, "Digit1,KeyQ"),
        (ENV_DISCOVER_CACHE, dir.join("cache").to_str().unwrap()),
    ]);

    let report = run_with_config(&BatteryConfig {
        binary: demo_binary().to_string(),
        scenario: scenario_path.to_string_lossy().into_owned(),
        timeout_secs: 300,
    })
    .expect("battery run of an authored scenario with inject set");

    // No injection: the authored file ran verbatim...
    assert_eq!(
        report.injected_surface_keys, None,
        "an authored scenario file must never be injected into"
    );
    // ...but the IMPLIED annotation still happened (INJECT alone is
    // set here, DISCOVER deliberately unset — "implies discovery"
    // must survive injection refusing the authored file).
    assert!(
        report.discovered_surface.is_some(),
        "BEVY_PLAYTEST_DISCOVER_INJECT implies discovery: the annotation \
         must not vanish when injection refuses an authored scenario"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}
