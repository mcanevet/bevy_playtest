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

use crate::common;

use std::path::PathBuf;

use bevy_playtest::battery::{run_with_config, BatteryConfig, BUILTIN_BATTERY};
use bevy_playtest::discovery::draft::{draft_surface, DraftOutcome};
use bevy_playtest::discovery::flow::{
    discover, discover_from_env, DiscoveryConfig, DiscoveryError, ENV_DISCOVER, ENV_DISCOVER_CACHE,
    ENV_DISCOVER_INJECT, ENV_DISCOVER_KEYS,
};
use bevy_playtest::discovery::provenance::ProvenanceGrade;
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
/// surface must SAY so, not report a trusted negative). The mouse
/// grid is switched OFF (`0 × 0`, the sanctioned slim mode) so
/// keyboard-layer tests pay no mouse jobs.
fn slim_config() -> DiscoveryConfig {
    let mut config = DiscoveryConfig::new(demo_binary());
    config.plan.candidate_keys = vec!["Digit1".into(), "KeyQ".into()];
    config.plan.mouse_grid_cols = 0;
    config.plan.mouse_grid_rows = 0;
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

/// The draft sweep on the slim matrix: the e2e of the D1-c layer —
/// `draft_surface` over the demo game produces live AND inert
/// entries, the coverage partition, findings in bijection with live
/// entries, and `from_cache` reflecting reality.
#[test]
fn draft_sweep_derives_live_and_inert_entries_with_honest_coverage() {
    let _slot = common::run_slot();
    let config = slim_config();

    let cold = draft_surface(&config).expect("cold draft sweep");
    assert!(!cold.from_cache, "fresh cache dir must probe");

    // Slim matrix: Digit1 (bound, live) + KeyQ (control, no entry).
    assert_eq!(cold.entries.len(), 1, "Digit1 live, KeyQ entry-less");
    let digit1 = &cold.entries[0];
    assert_eq!(digit1.input, "Digit1");
    assert!(digit1.effect_signature.is_some(), "Digit1 moves the world");
    assert!(
        digit1.evidence.iter().any(|e| matches!(
            e,
            bevy_playtest::discovery::surface::Evidence::ProbeRun { .. }
        )),
        "the live entry carries a probe_run observation"
    );
    assert!(cold.draft, "the outcome wears its draft marker");
    assert_eq!(
        cold.surface_confidence,
        SurfaceConfidence::Exact,
        "deterministic demo, clean controls"
    );

    // Coverage: one informatively-probed candidate, partitioned 1/0.
    assert_eq!(cold.coverage.candidates_probed, 1);
    assert_eq!(cold.coverage.verified, 1);
    assert_eq!(cold.coverage.inert, 0);

    // Findings in bijection with live entries, conservative kind.
    assert_eq!(cold.findings.len(), 1);
    assert!(matches!(
        cold.findings[0].kind,
        bevy_playtest::discovery::surface::DiscoveryFindingKind::DiscoveredButUndeclared
    ));
    assert_eq!(cold.findings[0].input, "Digit1");

    // The named limitation classes are acknowledged verbatim.
    for class in bevy_playtest::discovery::draft::MISSED_BY_DESIGN {
        assert!(
            cold.coverage.missed_by_design.iter().any(|m| m == class),
            "coverage must acknowledge: {class}"
        );
    }

    std::fs::remove_dir_all(&config.cache_dir).unwrap();
}

/// Acceptance bar item 3+4: the CLI drafts a real game binary in
/// one command, the artifact round-trips, and two sweeps of the
/// same binary (isolated caches) produce BYTE-IDENTICAL artifacts —
/// the artifact carries no timestamps, so determinism is asserted
/// on full byte equality.
#[test]
fn cli_draft_artifact_round_trips_and_is_byte_deterministic() {
    let _slot = common::run_slot();
    let dir = temp_dir();
    let out_first = dir.join("first").join("surface.draft.json");
    let out_second = dir.join("second").join("surface.draft.json");
    let keys = "Digit1,KeyQ";

    // One command, real subprocesses, twice — distinct cache dirs so
    // both are cold and the outputs can only agree if the derivation
    // is deterministic (acceptance bar: no timestamp exclusion
    // needed — the artifact has no timestamps to exclude).
    let cli = env!("CARGO_BIN_EXE_bevy_playtest_discover");
    for (cache, out) in [
        (dir.join("cache1"), &out_first),
        (dir.join("cache2"), &out_second),
    ] {
        let status = std::process::Command::new(cli)
            .arg("--binary")
            .arg(demo_binary())
            .arg("--keys")
            .arg(keys)
            .arg("--cache-dir")
            .arg(&cache)
            .arg("--out")
            .arg(out)
            .status()
            .expect("the discover CLI binary must run");
        assert!(
            status.success(),
            "the CLI must exit 0 (inert answers included) — got {status}"
        );
        assert!(out.exists(), "the CLI must write the artifact at --out");
    }

    let bytes_first = std::fs::read(&out_first).unwrap();
    let bytes_second = std::fs::read(&out_second).unwrap();
    assert_eq!(
        bytes_first, bytes_second,
        "two cold sweeps of the same binary must produce byte-identical drafts"
    );

    // Round-trip through the typed shape, wearing the header.
    let json = String::from_utf8(bytes_first).unwrap();
    assert!(
        json.contains("\"draft\": true"),
        "the artifact's header must carry the draft marker"
    );
    let back: DraftOutcome =
        serde_json::from_str(&json).expect("artifact parses as a DraftOutcome");
    assert!(back.draft);
    assert_eq!(
        back.schema_version,
        bevy_playtest::discovery::draft::ARTIFACT_SCHEMA_VERSION,
        "the header's schema-version promise must survive the round trip"
    );

    // The CLI-driven artifact agrees with the library-driven draft
    // of the same matrix (from_cache differs run-to-run by design:
    // both CLI runs were cold, so it is false here).
    let config = slim_config();
    let lib = draft_surface(&config).expect("library draft of the same matrix");
    assert_eq!(back.entries, lib.entries, "CLI and library drafts agree");
    assert_eq!(back.coverage, lib.coverage);
    assert_eq!(back.findings, lib.findings);

    std::fs::remove_dir_all(&dir).unwrap();
    std::fs::remove_dir_all(&config.cache_dir).unwrap();
}

/// The sweep-lens negative control: an INERT key probed under Exact
/// controls becomes a pinned entry — `probe_run_inert` evidence,
/// null effect signature — so an all-inert draft is an answer with
/// something to show, not an empty one. (KeyQ is deliberately NOT
/// this test: the control key supports no entry of any kind.)
#[test]
fn draft_pins_inert_keys_as_observations_not_silence() {
    let _slot = common::run_slot();
    let mut config = slim_config();
    // Digit1 stays; Swap keys: probe Digit1 (live) plus a key the
    // demo demonstrably ignores (KeyW) so the draft carries BOTH
    // entry kinds.
    config.plan.candidate_keys = vec!["Digit1".into(), "KeyW".into(), "KeyQ".into()];

    let draft = draft_surface(&config).expect("draft sweep with an inert candidate");

    let by_input = |key: &str| {
        draft
            .entries
            .iter()
            .find(|e| e.input == key)
            .unwrap_or_else(|| panic!("{key} must have an entry"))
    };
    let digit1 = by_input("Digit1");
    assert!(digit1.effect_signature.is_some());

    // The inert key: pinned with probe_run_inert evidence and the
    // committed note.
    let keyw = by_input("KeyW");
    assert!(keyw.effect_signature.is_none(), "KeyW moves nothing");
    assert!(
        keyw.evidence.iter().any(|e| matches!(
            e,
            bevy_playtest::discovery::surface::Evidence::ProbeRunInert { .. }
        )),
        "inert entries carry the probe_run_inert observation"
    );
    assert!(
        keyw.notes.iter().any(|n| n.contains("probed inert")),
        "the committed inert note travels with the entry"
    );

    // Partition holds with both kinds present: 2 probed candidates
    // (KeyQ never counts), 1 live, 1 inert.
    assert_eq!(draft.coverage.candidates_probed, 2);
    assert_eq!(draft.coverage.verified, 1);
    assert_eq!(draft.coverage.inert, 1);
    // No finding claims the inert key.
    assert!(
        !draft.findings.iter().any(|f| f.input == "KeyW"),
        "inert keys carry no live-residue finding"
    );

    std::fs::remove_dir_all(&config.cache_dir).unwrap();
}

/// Positive mouse fixture: `takeover_demo_picking` moves its world
/// only after a UI `PointerClick` on a FULLSCREEN node — every
/// in-window click position is a bound click. Discovery's mouse
/// grid must therefore report EVERY cell with an observed effect
/// against the mouse control (which leaves the world frozen).
///
/// Slim matrix (3 × 2 grid, keyboard candidates narrowed to the
/// control key only — cells are what is under test): 1 key probe
/// pair × 2 points + 4 key controls + 6 cells + 2 mouse controls
/// = 14 subprocesses.
#[test]
fn mouse_grid_finds_effects_on_pointer_driven_game() {
    let _slot = common::run_slot();
    let picking = env!("CARGO_BIN_EXE_takeover_demo_picking");
    let mut config = DiscoveryConfig::new(picking);
    // Keyboard candidates: the control key alone — it renders as an
    // Unclassified keyboard finding, and the mouse layer (the
    // subject here) stays cheap.
    config.plan.candidate_keys = vec!["KeyQ".into()];
    config.plan.mouse_grid_cols = 3;
    config.plan.mouse_grid_rows = 2;
    config.cache_dir = temp_dir();

    let cold = discover(&config).expect("cold discovery of the picking game");
    assert!(!cold.from_cache);

    assert_eq!(cold.surface.mouse_cells.len(), 6, "3 × 2 grid");
    for cell in &cold.surface.mouse_cells {
        assert!(
            cell.effect.is_some(),
            "cell {} is a bound click for the fullscreen-node game — \
             its probe must diverge from the mouse control",
            cell.id
        );
        assert_eq!(cell.confidence, KeyConfidence::Exact);
    }
    // Row-major order, stable ids.
    let ids: Vec<&str> = cold
        .surface
        .mouse_cells
        .iter()
        .map(|c| c.id.as_str())
        .collect();
    assert_eq!(ids, ["r0c0", "r0c1", "r0c2", "r1c0", "r1c1", "r1c2"]);

    // Deterministic game: the mouse gate passed, whole surface Exact.
    assert_eq!(cold.surface.confidence, SurfaceConfidence::Exact);

    // Warm run: identical surface from the cache (mouse cells ride
    // the same entry).
    let warm = discover(&config).expect("warm discovery");
    assert!(warm.from_cache);
    assert_eq!(warm.surface, cold.surface, "cached surface is identical");

    std::fs::remove_dir_all(&config.cache_dir).unwrap();
}

/// Negative control: `takeover_demo_input` moves ONLY on
/// `just_pressed(Digit1)` — it binds NO pointer input, so a mouse
/// click anywhere leaves the world unchanged and every cell reads
/// as a TRUSTED inert (Exact, effect: null) against the mouse
/// control. The correspondence the mouse grid must draw: a
/// pointer-driven game HAS cell effects, a keyboard-only game
/// LACKS them.
#[test]
fn mouse_grid_finds_no_effects_on_keyboard_only_game() {
    let _slot = common::run_slot();
    let mut config = DiscoveryConfig::new(demo_binary());
    config.plan.candidate_keys = vec!["Digit1".into()];
    config.plan.mouse_grid_cols = 2;
    config.plan.mouse_grid_rows = 2;
    config.cache_dir = temp_dir();

    let cold = discover(&config).expect("cold discovery of the keyboard game");
    assert!(!cold.from_cache);

    assert_eq!(cold.surface.mouse_cells.len(), 4, "2 × 2 grid");
    for cell in &cold.surface.mouse_cells {
        assert!(
            cell.effect.is_none(),
            "cell {} must be inert — the demo binds no pointer input",
            cell.id
        );
        // Trusted negative: deterministic game, symmetric coverage.
        assert_eq!(cell.confidence, KeyConfidence::Exact);
    }
    assert_eq!(cold.surface.confidence, SurfaceConfidence::Exact);

    // Contrast: Digit1 is still effective on the keyboard layer,
    // so the game is demonstrably responsive — the inert mouse
    // cells mean "no pointer input bound", not "dead game".
    let digit1 = cold
        .surface
        .keys
        .iter()
        .find(|k| k.key == "Digit1")
        .expect("Digit1 was probed");
    assert!(digit1.effect.is_some());

    std::fs::remove_dir_all(&config.cache_dir).unwrap();
}

/// Position discrimination, the strongest control of the mouse
/// grid: `takeover_demo_mouse_quadrant` binds clicks ONLY on a UI
/// node covering the top-left ~45% of the window. With the
/// DEFAULT 5×4 grid, exactly four cell centers land inside the
/// quadrant (x ∈ {0.1, 0.3}, y ∈ {0.125, 0.375} → r0c0, r0c1,
/// r1c0, r1c1); every other cell's click misses the node and must
/// read inert. The mouse control (window center, (0.5, 0.5)) also
/// misses — the baseline is a genuinely inert click, so the bound
/// cells' divergence is attributable to POSITION alone.
///
/// This is the fixture that fails a grid which delivers clicks
/// but cannot resolve WHERE (e.g. a fullscreen-node game passes
/// position-blind too). Job count: 1 key pair × 2 + 4 key
/// controls + 20 cells + 2 mouse controls = 28 subprocesses.
#[test]
fn mouse_grid_discriminates_cell_positions() {
    let _slot = common::run_slot();
    let quadrant = env!("CARGO_BIN_EXE_takeover_demo_mouse_quadrant");
    let mut config = DiscoveryConfig::new(quadrant);
    config.plan.candidate_keys = vec!["KeyQ".into()];
    // DEFAULT 5×4 grid — the geometry assertions below are spelled
    // against its exact cell centers.
    config.cache_dir = temp_dir();

    let cold = discover(&config).expect("cold discovery of the quadrant game");
    assert!(!cold.from_cache);

    assert_eq!(cold.surface.mouse_cells.len(), 20, "default 5 × 4 grid");
    let bound = |id: &str| {
        cold.surface
            .mouse_cells
            .iter()
            .find(|c| c.id == id)
            .unwrap_or_else(|| panic!("cell {id} must be in the surface"))
    };
    // Quadrant cells diverge.
    for id in ["r0c0", "r0c1", "r1c0", "r1c1"] {
        let cell = bound(id);
        assert!(
            cell.effect.is_some(),
            "cell {id} overlaps the clickable quadrant — its probe must diverge"
        );
        assert_eq!(cell.confidence, KeyConfidence::Exact);
    }
    // Every other cell is inert AND a trusted negative — the whole
    // point of this fixture: the grid says WHERE, not just THAT.
    for cell in &cold.surface.mouse_cells {
        if ["r0c0", "r0c1", "r1c0", "r1c1"].contains(&cell.id.as_str()) {
            continue;
        }
        assert!(
            cell.effect.is_none(),
            "cell {} lies outside the quadrant — its click misses the node and \
             must match the mouse control trace",
            cell.id
        );
        assert_eq!(
            cell.confidence,
            KeyConfidence::Exact,
            "deterministic game, full coverage: the inert verdict is trusted"
        );
    }

    std::fs::remove_dir_all(&config.cache_dir).unwrap();
}

/// Self-coincidence negative control: a 1×1 grid's sole cell sits at
/// the window dead center — exactly the mouse control position — so
/// its probe is byte-identical to its own baseline BY CONSTRUCTION.
/// Whatever the game binds there (this game binds Digit1-likes only,
/// making the click inert, but the property is structural), the
/// cell must come back `Unclassified` with the coincidence note,
/// never a trusted negative. The mouse analogue of the control-key
/// self-probe guard. Job count: 1 key pair × 2 + 4 key controls + 1
/// cell + 2 mouse controls = 9 subprocesses.
#[test]
fn mouse_cell_coinciding_with_control_position_is_unclassified() {
    let _slot = common::run_slot();
    let mut config = DiscoveryConfig::new(demo_binary());
    config.plan.candidate_keys = vec!["Digit1".into()];
    config.plan.mouse_grid_cols = 1;
    config.plan.mouse_grid_rows = 1;
    config.cache_dir = temp_dir();

    let cold = discover(&config).expect("cold discovery");
    assert!(!cold.from_cache);

    assert_eq!(cold.surface.mouse_cells.len(), 1);
    let cell = &cold.surface.mouse_cells[0];
    assert_eq!(cell.id, "r0c0");
    assert_eq!(cell.confidence, KeyConfidence::Unclassified);
    assert!(cell.effect.is_none());
    assert!(
        cold.surface
            .notes
            .iter()
            .any(|n| n.contains("coincides with the mouse control position")),
        "the coincidence must be surfaced loudly: {:?}",
        cold.surface.notes
    );

    std::fs::remove_dir_all(&config.cache_dir).unwrap();
}

/// Positive motion fixture: `takeover_demo_motion` pans its world by
/// the SIGNED `AccumulatedMouseMotion` delta every frame — the
/// camera-pan idiom. The motion probe's zero-net +d/−d oscillation
/// displaces the world DURING the oscillation window (the digests
/// record every frame, so the trace diverges from the frozen control
/// from the first +d frame), and discovery must report a
/// `mouse_motion` finding with an observed effect and Exact
/// confidence. Job count: 1 key pair × 2 + 4 key controls + the
/// disabled grid + 1 motion probe + 2 motion controls = 9
/// subprocesses.
#[test]
fn mouse_motion_finds_effects_on_motion_driven_game() {
    let _slot = common::run_slot();
    let motion = env!("CARGO_BIN_EXE_takeover_demo_motion");
    let mut config = DiscoveryConfig::new(motion);
    // Keyboard candidates: the control key alone — the motion layer
    // (the subject here) stays cheap.
    config.plan.candidate_keys = vec!["KeyQ".into()];
    // Click grid OFF: cells are not under test here, and the motion
    // demo binds no clicks — the cell matrix would only burn runtime
    // producing inert verdicts.
    config.plan.mouse_grid_cols = 0;
    config.plan.mouse_grid_rows = 0;
    config.cache_dir = temp_dir();

    let cold = discover(&config).expect("cold discovery of the motion game");
    assert!(!cold.from_cache);

    let finding = cold
        .surface
        .mouse_motion
        .as_ref()
        .expect("the motion probe ran; the surface must carry a motion finding");
    assert!(
        finding.effect.is_some(),
        "the motion-driven game must diverge from its no-motion control"
    );
    assert_eq!(finding.confidence, KeyConfidence::Exact);

    // Deterministic game: all three gates passed, whole surface Exact.
    assert_eq!(cold.surface.confidence, SurfaceConfidence::Exact);

    // Warm run: identical surface from the cache.
    let warm = discover(&config).expect("warm discovery");
    assert!(warm.from_cache);
    assert_eq!(warm.surface, cold.surface, "cached surface is identical");

    std::fs::remove_dir_all(&config.cache_dir).unwrap();
}

/// Negative motion control: `takeover_demo_input` moves ONLY on
/// `just_pressed(Digit1)` — it binds no motion reader, so the
/// oscillation leaves the world byte-identical to the motion
/// control and the finding must read as a TRUSTED inert (Exact,
/// effect: null). The correspondence the motion probe must draw: a
/// motion-driven game HAS a motion effect, a keyboard-only game
/// does NOT — this is the anti-vacuity pair (without the D0 raw
/// MouseMotion delivery fix, BOTH sides would read inert and this
/// test's positive twin would fail while the old machinery looked
/// green).
#[test]
fn mouse_motion_finds_no_effects_on_keyboard_only_game() {
    let _slot = common::run_slot();
    let mut config = DiscoveryConfig::new(demo_binary());
    config.plan.candidate_keys = vec!["Digit1".into()];
    config.plan.mouse_grid_cols = 0;
    config.plan.mouse_grid_rows = 0;
    config.cache_dir = temp_dir();

    let cold = discover(&config).expect("cold discovery of the keyboard game");
    assert!(!cold.from_cache);

    let finding = cold
        .surface
        .mouse_motion
        .as_ref()
        .expect("the motion probe ran; the surface must carry a motion finding");
    assert!(
        finding.effect.is_none(),
        "the keyboard-only game must not react to the oscillation"
    );
    // Trusted negative: deterministic game, symmetric coverage.
    assert_eq!(finding.confidence, KeyConfidence::Exact);
    assert_eq!(cold.surface.confidence, SurfaceConfidence::Exact);

    // Contrast: Digit1 is still effective on the keyboard layer —
    // the inert motion verdict means "no motion input bound", not
    // "dead game".
    let digit1 = cold
        .surface
        .keys
        .iter()
        .find(|k| k.key == "Digit1")
        .expect("Digit1 was probed");
    assert!(digit1.effect.is_some());

    std::fs::remove_dir_all(&config.cache_dir).unwrap();
}

/// Positive focus-walk fixture: `takeover_demo_focus` adds
/// `TabNavigationPlugin` (the opt-in NOT in DefaultPlugins) and
/// cycles focus across a `TabGroup` of `TabIndex`ed widgets with a
/// digest-visible marker tracking the focused widget. The walker's
/// probe (a single scripted `Tab` tap) must diverge from its
/// control pair (a single control-key tap at the same frame — the
/// substitution pair differs by exactly one bit, the pressed key)
/// — discovery reports a `focus_walk` finding with an
/// observed effect and Exact confidence.
#[test]
fn focus_walk_finds_effects_on_tab_navigation_game() {
    let _slot = common::run_slot();
    let focus = env!("CARGO_BIN_EXE_takeover_demo_focus");
    let mut config = DiscoveryConfig::new(focus);
    // Keyboard candidates: the control key alone — the walker is
    // the subject; the grid is off to keep the run lean.
    config.plan.candidate_keys = vec!["KeyQ".into()];
    config.plan.mouse_grid_cols = 0;
    config.plan.mouse_grid_rows = 0;
    config.cache_dir = temp_dir();

    let cold = discover(&config).expect("cold discovery of the focus game");
    assert!(!cold.from_cache);

    let finding = cold
        .surface
        .focus_walk
        .as_ref()
        .expect("the walker ran; the surface must carry a focus-walk finding");
    assert!(
        finding.effect.is_some(),
        "the tab-navigation game must diverge from its no-Tab control"
    );
    assert_eq!(finding.confidence, KeyConfidence::Exact);
    assert_eq!(finding.provenance, ProvenanceGrade::ProbeVerified);

    // Deterministic game: all four gates passed, whole surface Exact.
    assert_eq!(cold.surface.confidence, SurfaceConfidence::Exact);

    // Warm run: identical surface from the cache (the fingerprint
    // threads focus_walk_enable, so the entry must hit).
    let warm = discover(&config).expect("warm discovery");
    assert!(warm.from_cache);
    assert_eq!(warm.surface, cold.surface, "cached surface is identical");

    std::fs::remove_dir_all(&config.cache_dir).unwrap();
}

/// Negative focus-walk control: `takeover_demo_input` never adds
/// `TabNavigationPlugin` (nothing listens to Tab) — the honest-inert
/// verdict. The probe must read as a TRUSTED inert (Exact,
/// effect: null): "a Tab press produced no observable change in this
/// run", which is exactly what this game demonstrates. It does NOT
/// license the claim "the game has no focus system" — that reading
/// is forbidden by the finding's own docs.
#[test]
fn focus_walk_finds_no_effects_on_keyboard_only_game() {
    let _slot = common::run_slot();
    let mut config = DiscoveryConfig::new(demo_binary());
    config.plan.candidate_keys = vec!["Digit1".into()];
    config.plan.mouse_grid_cols = 0;
    config.plan.mouse_grid_rows = 0;
    config.cache_dir = temp_dir();

    let cold = discover(&config).expect("cold discovery of the keyboard game");
    assert!(!cold.from_cache);

    let finding = cold
        .surface
        .focus_walk
        .as_ref()
        .expect("the walker ran; the surface must carry a focus-walk finding");
    assert!(
        finding.effect.is_none(),
        "the keyboard-only game must not react to a Tab press"
    );
    // Trusted negative: deterministic game, symmetric coverage.
    assert_eq!(finding.confidence, KeyConfidence::Exact);
    assert_eq!(cold.surface.confidence, SurfaceConfidence::Exact);

    // Contrast: Digit1 is still effective on the keyboard layer —
    // the inert focus-walk verdict means "no observable Tab effect
    // in this run", not "dead game".
    let digit1 = cold
        .surface
        .keys
        .iter()
        .find(|k| k.key == "Digit1")
        .expect("Digit1 was probed");
    assert!(digit1.effect.is_some());

    std::fs::remove_dir_all(&config.cache_dir).unwrap();
}

// ----------------------------------------------------------------------
// Source-derived surface (D1.5 slice 5): the parent-side source scan
// layering game-referenced keys onto the default chaos surface —
// no discovery, no probing, no authored scenario. These tests pin
// the precedence (authored file > injection > source scan > blind
// default), the report honesty field, and the out-of-the-box claim
// itself: the loading_screen-shaped demo goes CLEAN undecorated
// because the scan found KeyCode::Digit1 in its source.
// ----------------------------------------------------------------------

use bevy_playtest::discovery::source_scan::{
    layered_chaos_keys, scan_from_root, scan_root_from_env, ENV_SOURCE_ROOT,
    ENV_SOURCE_SCAN_DISABLE, MAX_SCAN_KEYS,
};
use bevy_playtest::scenario::BACKBONE_KEYS;

#[test]
fn source_scan_finds_the_demos_bound_key() {
    let _slot = common::run_slot();

    // The harness's own source tree is a fine scan subject (pure
    // function; no game needed): `takeover_demo_input.rs` mentions
    // KeyCode::Digit1, so the scan must find it with file:line
    // evidence. Point the scan at OUR src/bin to prove root
    // plumbing + extraction + canonicalization.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let result = scan_from_root(&root);
    assert!(
        result.hits.iter().any(|h| h.key == "Digit1"),
        "the input demo's KeyCode::Digit1 must be found; hits: {:?}",
        result
            .hits
            .iter()
            .map(|h| format!("{}:{}:{}", h.file, h.line, h.key))
            .collect::<Vec<_>>()
    );

    // And the layering contract on the real scan output: the
    // scan-derived surface (arm note present, no poison keys,
    // capped, notes non-empty — refusal over silence).
    let layered = layered_chaos_keys(&result);
    assert!(!layered.keys.contains(&"Escape".to_string()));
    assert!(!layered.keys.contains(&"Tab".to_string()));
    assert!(!layered.keys.is_empty());
    assert!(!layered.notes.is_empty());
    assert!(layered.keys.len() <= MAX_SCAN_KEYS);
    // The real harness src/ certainly cites ≥1 key, so the
    // scan-hit arm must fire and say so (a silent regression to
    // the old layering contract would otherwise slip through).
    assert!(
        layered
            .notes
            .iter()
            .any(|n| n.contains("scan-derived surface replaced the WASD/arrows default")),
        "the scan-hit arm note must be present: {:?}",
        layered.notes
    );
    // Deterministic emit order: alphabetical.
    let mut sorted = layered.keys.clone();
    sorted.sort();
    assert_eq!(layered.keys, sorted);

    // Directly parameterized layering sanity: a synthetic scan of
    // ONLY Digit1 must produce a one-key replacement surface — the
    // scan-hit arm REPLACES the backbone (the loading_screen fix,
    // with the press budget fully concentrated).
    let synthetic = bevy_playtest::discovery::source_scan::SourceScanResult {
        hits: vec![bevy_playtest::discovery::source_scan::SourceScanHit {
            key: "Digit1".into(),
            file: "src/main.rs".into(),
            line: 1,
            enclosing: None,
        }],
        ..Default::default()
    };
    let layered = layered_chaos_keys(&synthetic);
    assert_eq!(layered.keys, vec!["Digit1".to_string()]);
    // The arm must be recorded in the notes.
    assert!(layered
        .notes
        .iter()
        .any(|n| n.contains("scan-derived surface replaced the WASD/arrows default")));
}

#[test]
fn source_scan_root_resolution_follows_env_and_self_skips() {
    let _slot = common::run_slot();

    // Explicit root override wins...
    let custom = temp_dir();
    let _env = EnvGuard::set(&[(ENV_SOURCE_ROOT, custom.to_str().unwrap())]);
    assert_eq!(scan_root_from_env(), Some(custom.clone()));
    // ...an explicitly EMPTY override is a deliberate disable...
    let _env = EnvGuard::set(&[(ENV_SOURCE_ROOT, "")]);
    assert_eq!(scan_root_from_env(), None);
    // ...and with no override, a harness-self parent skips the
    // scan (CARGO_MANIFEST_DIR == the harness's baked-in dir), so
    // self-test batteries never scan their own demo sources.
    let _env = EnvGuard::set(&[]);
    assert_eq!(scan_root_from_env(), None);
    // Restore a non-harness manifest dir: the game's runtime
    // CARGO_MANIFEST_DIR is honored (this test binary IS in that
    // situation).
    //
    // NOTE: EnvGuard::set(&[]) clears SOURCE_ROOT; the harness-self
    // skip below covers the remaining path.

    std::fs::remove_dir_all(&custom).ok();
}

#[test]
fn source_scan_of_a_fresh_game_dir_yields_no_extras() {
    let _slot = common::run_slot();

    // A game crate with no KeyCode mentions at all: the layering
    // must keep the backbone unchanged and NOT stamp the report
    // field (no replacement = no record: None means "unchanged").
    let dir = temp_dir();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/main.rs"), "fn main() {} // no inputs here").unwrap();
    let result = scan_from_root(&dir);
    assert_eq!(result.hits.len(), 0);
    let layered = layered_chaos_keys(&result);
    let backbone: Vec<String> = BACKBONE_KEYS.iter().map(|s| s.to_string()).collect();
    assert_eq!(layered.keys, backbone);

    // And the battery honors that: BUILTIN_BATTERY against a
    // custom source root pointing at the fresh dir leaves the
    // report un-stamped (source_derived_keys stays None — the run
    // used the plain default surface).
    let _env = EnvGuard::set(&[(ENV_SOURCE_ROOT, dir.to_str().unwrap())]);
    let report = run_with_config(&BatteryConfig {
        binary: demo_binary().to_string(),
        scenario: BUILTIN_BATTERY.to_string(),
        timeout_secs: 300,
    })
    .expect("battery run with a fresh source root");
    assert_eq!(
        report.source_derived_keys, None,
        "no scan extras => the report must not pretend a replacement happened"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn source_scan_of_exactly_backbone_keys_leaves_report_unstamped() {
    let _slot = common::run_slot();

    // The set-equality gate's positive corner, e2e: a game citing
    // EXACTLY the backbone 8 triggers the scan-hit arm (the
    // layered surface is those 8 keys, alphabetized — a different
    // ORDER from the declared backbone), yet the battery must
    // treat it as no change: no scenario rewrite, no stamp.
    // source_derived_keys stays None — key order is load-bearing
    // in the driver, so refusing the rewrite IS the "nothing
    // changed" guarantee.
    let dir = temp_dir();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    // Citations must live in CODE, not comments: the scanner's
    // noise stripper blanks comments before token extraction.
    // (The file never compiles — the scan is purely textual.)
    let cites = BACKBONE_KEYS
        .iter()
        .map(|k| format!("let _ = KeyCode::{k};"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(
        dir.join("src/main.rs"),
        format!("fn main() {{\n{cites}\n}}\n"),
    )
    .unwrap();
    let result = scan_from_root(&dir);
    let layered = layered_chaos_keys(&result);
    assert!(layered
        .notes
        .iter()
        .any(|n| n.contains("scan-derived surface replaced the WASD/arrows default")));
    let mut scan_sorted = layered.keys.clone();
    scan_sorted.sort();
    let mut backbone_sorted: Vec<String> = BACKBONE_KEYS.iter().map(|s| s.to_string()).collect();
    backbone_sorted.sort();
    assert_eq!(scan_sorted, backbone_sorted);

    // And the battery honors the gate: set-equal surface => no
    // stamp, no rewrite, `None` honesty preserved.
    let _env = EnvGuard::set(&[(ENV_SOURCE_ROOT, dir.to_str().unwrap())]);
    let report = run_with_config(&BatteryConfig {
        binary: demo_binary().to_string(),
        scenario: BUILTIN_BATTERY.to_string(),
        timeout_secs: 300,
    })
    .expect("battery run with a backbone-only source root");
    assert_eq!(
        report.source_derived_keys, None,
        "a scan citing exactly the backbone 8 is set-equal to the default: \
         no rewrite, no stamp"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn source_scan_replacement_goes_clean_on_the_loading_screen_demo() {
    let _slot = common::run_slot();

    // THE out-of-the-box claim, e2e: the loading_screen-shaped demo
    // (world moves only on Digit1) runs the BUILT-IN battery with NO
    // discovery variables set. Undecorated, the WASD-only default
    // never presses Digit1 and the run pins frozen_world. With the
    // source scan applied, the scan-derived surface REPLACES the
    // blind default with exactly the key the game's own code reads,
    // the bot presses Digit1 every chaos interval (toggle
    // semantics), and the same battery goes CLEAN — zero config,
    // zero scenarios, zero probes.
    //
    // The scan root replicates what a REAL game crate looks like
    // to the parent: only the game's own source (the harness tree
    // would feed the scan every demo binary's keys).
    let root = temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let demo_src =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/bin/takeover_demo_input.rs");
    std::fs::copy(&demo_src, root.join("main.rs")).unwrap();
    let _env = EnvGuard::set(&[(ENV_SOURCE_ROOT, root.to_str().unwrap())]);

    let report = run_with_config(&BatteryConfig {
        binary: demo_binary().to_string(),
        scenario: BUILTIN_BATTERY.to_string(),
        timeout_secs: 300,
    })
    .expect("battery run with source-scan layering");

    // Honesty: the scan-derived surface is recorded...
    assert_eq!(
        report.source_derived_keys,
        Some(vec!["Digit1".to_string()]),
        "the scan must replace the default with the demo's bound key"
    );
    // ...with its provenance notes (refusal over silence: the
    // stamping must carry the scan's decisions, not just the
    // list)...
    assert!(
        report
            .source_derived_notes
            .as_ref()
            .is_some_and(|notes| notes
                .iter()
                .any(|n| n.contains("scan-derived surface replaced the WASD/arrows default"))),
        "notes must record the arm that fired: {:?}",
        report.source_derived_notes
    );
    // ...and the run went clean: no frozen_world (the bot pressed
    // Digit1 early enough to awaken the world before the oracle's
    // grace window closed).
    assert!(
        !report.violations.iter().any(|v| v.rule == "frozen_world"),
        "the source-layered battery must awaken the demo world: violations: {:?}",
        report
            .violations
            .iter()
            .map(|v| v.rule.clone())
            .collect::<Vec<_>>()
    );
}

#[test]
fn injection_does_not_double_stamp_when_scan_adds_nothing_new() {
    let _slot = common::run_slot();

    // Provenance hygiene, e2e: DISCOVER_INJECT implies discovery,
    // and the scan runs as the UNION partner of the probe surface
    // (tier-order fix: the probe prior must not CROWD OUT
    // scan-visible keys — see the sibling union test). Here the
    // scan root mentions only the SAME key the probe verified
    // (Digit1), so the union adds nothing: the report records the
    // injected keys and NOT a source-derived addition — the scan
    // must not double-stamp keys the probe already carried.
    let dir = temp_dir();
    // Scan root replicating a game crate whose only binding is the
    // one the probe matrix verified (the harness's own src/ would
    // feed the scan every demo binary's keys).
    let root = temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let demo_src =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/bin/takeover_demo_input.rs");
    std::fs::copy(&demo_src, root.join("main.rs")).unwrap();
    let _env = EnvGuard::set(&[
        (ENV_SOURCE_ROOT, root.to_str().unwrap()),
        (ENV_DISCOVER_INJECT, "1"),
        (ENV_DISCOVER_KEYS, "Digit1,KeyQ"),
        (ENV_DISCOVER_CACHE, dir.join("cache").to_str().unwrap()),
    ]);

    let report = run_with_config(&BatteryConfig {
        binary: demo_binary().to_string(),
        scenario: BUILTIN_BATTERY.to_string(),
        timeout_secs: 300,
    })
    .expect("battery run with injection over source scan");

    assert_eq!(
        report.injected_surface_keys,
        Some(vec!["Digit1".to_string()])
    );
    assert_eq!(
        report.source_derived_keys, None,
        "the scan added nothing the probe did not have; no double-stamping"
    );
    // F4 (refusal over silence): the scan RAN and added nothing —
    // that is a decision, and it is reported on the notes channel,
    // even though `source_derived_keys` stays absent.
    assert!(
        report
            .source_derived_notes
            .as_ref()
            .is_some_and(|notes| notes
                .iter()
                .any(|n| n.contains("source scan ran but added no keys"))),
        "the scan's no-op decision must be reported: {:?}",
        report.source_derived_notes
    );

    std::fs::remove_dir_all(&dir).unwrap();
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn injection_unions_scan_only_keys_beyond_the_probe_prior() {
    let _slot = common::run_slot();

    // THE tier-inversion fix, e2e: a key textually present in the
    // game's sources but OUTSIDE the probe matrix's candidate
    // prior (thetawave's KeyV is the concrete motivating case) used
    // to be silently LOST when DISCOVER_INJECT was enabled — the
    // injected (probe-bounded) surface replaced the scenario before
    // the scan guard ran, so the LOWER-recall tier crowded out the
    // HIGHER-recall one. Now the injected surface is the UNION:
    // probe-verified keys plus scan extras (same poison/cap
    // discipline as the default-battery layering). KeyV joins
    // Digit1 in the bot's surface, with provenance split across
    // the two report channels.
    let dir = temp_dir();
    // A "game crate" whose sources mention Digit1 (probe-verified)
    // AND KeyV (invisible to the 2-key slim matrix).
    let root = temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let demo_src =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/bin/takeover_demo_input.rs");
    std::fs::copy(&demo_src, root.join("main.rs")).unwrap();
    std::fs::write(
        root.join("more_bindings.rs"),
        "// a menu toggle the probe matrix never probes\n\
         if keys.just_pressed(KeyCode::KeyV) { menu_open ^= true; }\n",
    )
    .unwrap();
    let _env = EnvGuard::set(&[
        (ENV_SOURCE_ROOT, root.to_str().unwrap()),
        (ENV_DISCOVER_INJECT, "1"),
        (ENV_DISCOVER_KEYS, "Digit1,KeyQ"),
        (ENV_DISCOVER_CACHE, dir.join("cache").to_str().unwrap()),
    ]);

    let report = run_with_config(&BatteryConfig {
        binary: demo_binary().to_string(),
        scenario: BUILTIN_BATTERY.to_string(),
        timeout_secs: 300,
    })
    .expect("battery run with injection and scan union");

    // The full union surface, canonical order: Digit1 (probe-
    // verified) + KeyV (scan-only, beyond the probe prior).
    assert_eq!(
        report.injected_surface_keys,
        Some(vec!["Digit1".to_string(), "KeyV".to_string()]),
        "injection must UNION scan-visible keys in, never lose them"
    );
    // The scan-derived contribution is stamped SEPARATELY (exactly
    // the keys the scan added beyond the probe surface — the
    // channels stay distinct so provenance is never coalesced)...
    assert_eq!(
        report.source_derived_keys,
        Some(vec!["KeyV".to_string()]),
        "the scan's contribution must be recorded on its own channel"
    );
    // ...with a note saying so (refusal over silence).
    assert!(
        report
            .source_derived_notes
            .as_ref()
            .is_some_and(|notes| notes.iter().any(|n| n.contains("unioned 1 scan key"))),
        "notes must record the union decision: {:?}",
        report.source_derived_notes
    );

    std::fs::remove_dir_all(&dir).unwrap();
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn injection_with_scan_disabled_degenerates_to_the_pure_probe_surface() {
    let _slot = common::run_slot();

    // Off-arm pin (adversarial-review F6): inject ×
    // BEVY_PLAYTEST_SOURCE_SCAN_DISABLE=1 must degenerate cleanly to
    // the PURE probe surface — no union (the scan is the union
    // partner; with it disabled there is nothing to union), no
    // source_derived stamps, and the injected surface remains
    // exactly the probe-verified keys. The "never loses a
    // scan-derived key" guarantee holds ONLY when a scan applies:
    // here the user explicitly opted out, and the report must not
    // pretend otherwise (no notes, no keys).
    let dir = temp_dir();
    // A scanable root is IN REACH — disabling must still win.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let _env = EnvGuard::set(&[
        (ENV_SOURCE_ROOT, root.to_str().unwrap()),
        (ENV_SOURCE_SCAN_DISABLE, "1"),
        (ENV_DISCOVER_INJECT, "1"),
        (ENV_DISCOVER_KEYS, "Digit1,KeyQ"),
        (ENV_DISCOVER_CACHE, dir.join("cache").to_str().unwrap()),
    ]);

    let report = run_with_config(&BatteryConfig {
        binary: demo_binary().to_string(),
        scenario: BUILTIN_BATTERY.to_string(),
        timeout_secs: 300,
    })
    .expect("battery run with injection and scan disabled");

    // The pure probe surface — nothing unioned in.
    assert_eq!(
        report.injected_surface_keys,
        Some(vec!["Digit1".to_string()]),
        "scan disabled: injection degenerates to the probe surface alone"
    );
    assert_eq!(
        report.source_derived_keys, None,
        "no scan keys entered the surface — none may be claimed"
    );
    assert_eq!(
        report.source_derived_notes, None,
        "no scan ran: no decisions to report (a withhold would lie)"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn scan_disable_leaves_the_blind_default_in_place() {
    let _slot = common::run_slot();

    // The off switch: ENV_SOURCE_SCAN_DISABLE=1 must leave the
    // plain WASD+arrows default untouched even with a scanable root
    // in reach — the escape hatch for games where layering is
    // unwanted (e.g. reproducibility pinning of the old behavior).
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let _env = EnvGuard::set(&[
        (ENV_SOURCE_ROOT, root.to_str().unwrap()),
        (ENV_SOURCE_SCAN_DISABLE, "1"),
    ]);

    let report = run_with_config(&BatteryConfig {
        binary: demo_binary().to_string(),
        scenario: BUILTIN_BATTERY.to_string(),
        timeout_secs: 300,
    })
    .expect("battery run with the scan disabled");

    assert_eq!(
        report.source_derived_keys, None,
        "disabled scan must not layer or stamp anything"
    );
    // And the run behaves exactly like the old default: the demo
    // world never awakens (WASD only), so frozen_world pins.
    assert!(
        report.violations.iter().any(|v| v.rule == "frozen_world"),
        "the disabled scan must reproduce the pre-scan behavior (frozen_world pins)"
    );
}
