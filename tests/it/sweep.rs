#![cfg(feature = "takeover")]

//! Seed-sweep integration: the sweep's end-to-end contract on REAL
//! battery subprocesses, against two demo children whose verdicts
//! differ in exactly the ways the gate distinguishes:
//!
//! - `takeover_demo_input` — the Digit1-only game. Its default
//!   chaos surface (WASD+arrows) never presses Digit1, the world
//!   never awakens, and `frozen_world` fires on EVERY seed: a
//!   DETERMINISTIC failure, the shape a regression-file entry
//!   exists to remember.
//! - `takeover_demo_motion` — the mouse-pan game; mouse motion
//!   (sampled from the default surface) keeps it alive: a CLEAN
//!   child for the healed/verdict paths.
//!
//! Beyond verdicts, the sweep must PROVE the seed override reached
//! the bot: two seeds on the same scenario must produce DIFFERENT
//! recorded draw streams (same draws would mean the override never
//! traveled to the child, silently degenerating the sweep into five
//! copies of the seed-42 run).

use crate::common;

use bevy_app::App;
use bevy_playtest::plugin::{ENV_SCENARIO, ENV_SEED};
use bevy_playtest::run_scenario;
use bevy_playtest::scenario::Scenario;
use bevy_playtest::sweep::{
    run_sweep, run_sweep_and_gate, FindingFingerprint, RegressionDb, SweepConfig, SweepVerdict,
};
use bevy_transform::prelude::Transform;
use std::path::PathBuf;

/// Short scenario (the built-in battery's 20 s/member would dominate
/// CI time; the sweep contract does not depend on run length).
const SHORT_SCENARIO: &str = r#"{"name":"sweep-short","bot":{"type":"chaos","seed":42},
    "duration_s":2.0,"liveness":{"timeout_s":1.0}}"#;

fn temp_dir(tag: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "bevy_playtest_sweep_it_{}_{}_{}_{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        seq
    ))
}

// The draw-digest assertions below assume no stale seed override
// leaks from a sibling test's environment (it would corrupt every
// member identically, masking a broken override as
// "deterministic"). The battery strips `BEVY_PLAYTEST_*` from
// children wholesale at the spawn boundary; the digests pin that
// defense from the OUTSIDE.

#[test]
fn sweep_proves_seed_override_changes_draws() {
    let _slot = common::run_slot();
    let dir = temp_dir("draws");
    std::fs::create_dir_all(&dir).unwrap();
    let scenario_path = dir.join("short.json");
    std::fs::write(&scenario_path, SHORT_SCENARIO).unwrap();

    let config = SweepConfig::new(env!("CARGO_BIN_EXE_takeover_demo_motion"))
        .scenario(scenario_path.to_string_lossy().into_owned())
        .seeds(vec![42, 99])
        .parallel(2);
    let report = run_sweep(&config).expect("sweep must run");
    assert_eq!(report.members.len(), 2, "one member per seed");
    for m in &report.members {
        assert!(
            m.status.is_ok(),
            "clean child must complete: {:?}",
            m.status
        );
    }

    // The override reached the bot INSIDE the sweep — asserted on
    // the sweep's own per-member digests, not on hand-re-run
    // members: distinct seeds must yield distinct draw-stream
    // digests. Equal digests would mean `seed_override` never
    // traveled and the sweep silently degenerated into N copies of
    // the declared-seed run.
    let d42 = report.members[0].draw_digest.expect("member 42 drew");
    let d99 = report.members[1].draw_digest.expect("member 99 drew");
    assert_ne!(d42, d99, "seed override must change the bot's draws");
    assert!(
        d42 != 0 || d99 != 0,
        "degenerate digests: members must draw"
    );

    // Negative control on the same digests: re-running the SAME
    // seed must reproduce the SAME digest, so the inequality above
    // is attributable to the seed, not run-to-run noise.
    let again = run_sweep(&config).expect("second sweep");
    assert_eq!(
        again.members[0].draw_digest.expect("member 42 drew"),
        d42,
        "same seed must redraw identically"
    );
    assert_eq!(
        again.members[1].draw_digest.expect("member 99 drew"),
        d99,
        "same seed must redraw identically"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sweep_gates_on_new_findings_then_accepts_them_once_known() {
    let _slot = common::run_slot();
    let dir = temp_dir("gate");
    std::fs::create_dir_all(&dir).unwrap();
    let scenario_path = dir.join("short.json");
    std::fs::write(&scenario_path, SHORT_SCENARIO).unwrap();
    let regressions = dir.join("regressions.txt");

    // Phase 1 — no regression file: the deterministic frozen_world
    // finding is NEW. The gate must panic.
    let config = || {
        SweepConfig::new(env!("CARGO_BIN_EXE_takeover_demo_input"))
            .scenario(scenario_path.to_string_lossy().into_owned())
            .seeds(vec![42, 43])
            .parallel(2)
            .regressions(regressions.clone())
    };
    let result = std::panic::catch_unwind(|| {
        run_sweep_and_gate(&config());
    });
    assert!(
        result.is_err(),
        "unknown frozen_world fingerprint must gate (panic)"
    );

    // Phase 2 — record the fingerprint (the developer workflow:
    // adjudicate the finding, then add it to the file). A helper
    // does this from the sweep's own findings so the key format can
    // never drift from what the gate compares.
    let first = run_sweep(&config()).expect("sweep for recording");
    assert!(!first.findings.is_empty(), "deterministic failure child");
    let mut db = RegressionDb::load(&regressions).expect("empty db");
    for f in &first.findings {
        // Key must be well-formed: round-trips through the parser.
        assert!(FindingFingerprint::from_key(&f.fingerprint_key).is_some());
        assert!(
            db.insert(
                f.fingerprint_key.clone(),
                "adjudicated: test fixture".into(),
            ),
            "recording a key the sweep itself reported must succeed: {}",
            f.fingerprint_key
        );
    }
    db.store(&regressions).expect("store db");

    // Phase 3 — same finding, now known: the gate PASSES (returns
    // the report; known findings are expected, not blocking).
    let report = run_sweep_and_gate(&config());
    assert_eq!(report.verdict, Some(SweepVerdict::Known));

    // Phase 4 — healing: sweep the clean child against the same
    // regression file. Every known key is now absent — the report
    // must surface them as healed suggestions, and the verdict is
    // Clean (no findings occurred).
    let clean_config = || {
        SweepConfig::new(env!("CARGO_BIN_EXE_takeover_demo_motion"))
            .scenario(scenario_path.to_string_lossy().into_owned())
            .seeds(vec![42, 43])
            .parallel(2)
            .regressions(regressions.clone())
    };
    let healed_report = run_sweep_and_gate(&clean_config());
    assert_eq!(healed_report.verdict, Some(SweepVerdict::Clean));
    assert!(
        !healed_report.healed.is_empty(),
        "known keys absent ⇒ healed"
    );
    for f in &first.findings {
        assert!(
            healed_report.healed.contains(&f.fingerprint_key),
            "recorded key must appear as healed: {}",
            f.fingerprint_key
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Crash gating: a crashed member is always red, with or without a
// regression file (tests/sweep's takeover_demo_crash child).
// ---------------------------------------------------------------------------

/// The crash child under takeover: PlaytestPlugin catches the Update
/// panic, classifies the run Crash, writes the report, and exits 2 —
/// a REPORT-BEARING member outcome (distinct from the infrastructure
/// failure shape: exit 101 with no report at all). The sweep treats
/// it as a crash, not as a fingerprintable finding: a crashed run
/// certifies nothing.
#[test]
fn crashed_member_gates_the_sweep_even_without_a_regression_file() {
    let _slot = common::run_slot();
    let dir = temp_dir("crash");
    std::fs::create_dir_all(&dir).unwrap();
    let scenario_path = dir.join("short.json");
    std::fs::write(&scenario_path, SHORT_SCENARIO).unwrap();

    // A crash is not a findings question: no regression file is
    // configured, yet the sweep must be RED, not the silent
    // report-only pass an ungated sweep otherwise yields.
    let config = SweepConfig::new(env!("CARGO_BIN_EXE_takeover_demo_crash"))
        .scenario(scenario_path.to_string_lossy().into_owned())
        .seeds(vec![7]);
    let report = run_sweep(&config).expect("sweep must run");
    assert_eq!(report.members.len(), 1);
    assert_eq!(
        report.crashed_seeds,
        vec![7u64],
        "crash child must be a crashed member, not a finding: {:?}",
        report.members[0].status
    );
    assert_eq!(
        report.verdict,
        Some(SweepVerdict::NewOrRegressed),
        "crashes gate even when no regression file is configured"
    );
    // The crash may also surface its panic as a fingerprintable
    // finding, but the crash itself is carried on `crashed_seeds` —
    // the crash must gate REGARDLESS of any fingerprint.

    // And the gate entry point panics on it — the contract a test
    // body actually feels.
    let result = std::panic::catch_unwind(|| run_sweep_and_gate(&config));
    assert!(
        result.is_err(),
        "run_sweep_and_gate must panic on a crashed member"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Negative control on the same child: the crash's OWN panic finding
/// (`panic:world` — `Captured::Panic` reports target `world`) is
/// pre-recorded in the regression file, simulating a developer who
/// adjudicated the crash as known. It must not buy the crash a pass:
/// known-finding forgiveness never extends to crashes (they are
/// exempt from the findings memory), so the verdict stays red and
/// the crash stays on `crashed_seeds`.
#[test]
fn crashed_member_gates_even_when_a_regression_file_exists() {
    let _slot = common::run_slot();
    let dir = temp_dir("crash-db");
    std::fs::create_dir_all(&dir).unwrap();
    let scenario_path = dir.join("short.json");
    std::fs::write(&scenario_path, SHORT_SCENARIO).unwrap();
    let regressions = dir.join("regressions.txt");

    let mut db = RegressionDb::load(&regressions).expect("empty db");
    assert!(db.insert("panic:world".into(), "pre-adjudicated".into()));
    db.store(&regressions).expect("store db");

    let config = SweepConfig::new(env!("CARGO_BIN_EXE_takeover_demo_crash"))
        .scenario(scenario_path.to_string_lossy().into_owned())
        .seeds(vec![7])
        .regressions(regressions.clone());
    // The gate must panic (crash gates despite the DB entry)...
    let result = std::panic::catch_unwind(|| run_sweep_and_gate(&config));
    assert!(
        result.is_err(),
        "a crashed member must gate even with a regression file"
    );
    // ...and the underlying sweep reports the crash shape.
    let report = run_sweep(&config).expect("re-run for report");
    assert_eq!(report.crashed_seeds, vec![7u64]);
    assert_eq!(report.verdict, Some(SweepVerdict::NewOrRegressed));

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Real-entity canonicalization: the target bevy_ecs ACTUALLY emits
// must collapse to `entity` in the fingerprint.
// ---------------------------------------------------------------------------

/// The canonicalizer was previously tested only against hand-fed
/// strings shaped like its own matcher — a self-confirming loop that
/// survived a wrong format assumption. This test closes the loop: a
/// REAL `run_scenario` report (entities spawned by an actual Bevy
/// app, targets formatted by bevy_ecs itself) must produce a
/// fingerprint whose target collapsed to `entity`, pinned against
/// whatever format the vendored bevy_ecs actually emits.
#[test]
fn real_run_entity_targets_collapse_in_fingerprints() {
    let _slot = common::run_slot();
    // NaN Transform: the global finite_transforms oracle fires on the
    // offending ENTITY with a per-entity debug target — proven shape
    // (tests/driver_smoke.rs: nan_transform_yields_finite_transforms_
    // violation), here pinned into the FINGERPRINT layer.
    let scenario: Scenario =
        serde_json::from_str(r#"{"bot":{"type":"chaos","seed":7},"duration_s":0.1,"tps":60}"#)
            .unwrap();
    bevy_playtest::sinks::ensure_global_subscriber();
    let mut app = App::new();
    app.add_systems(
        bevy_app::Update,
        |mut commands: bevy_ecs::prelude::Commands| {
            commands.spawn(Transform::from_xyz(f32::NAN, 0.0, 0.0));
        },
    );
    let report = run_scenario(&mut app, &scenario).unwrap();
    let entry = report
        .violations
        .iter()
        .find(|v| v.rule == "finite_transforms")
        .expect("global oracle must fire on the NaN entity");
    let fp =
        FindingFingerprint::of_blocking(entry).expect("finite_transforms is a blocking finding");
    assert_eq!(
        fp.key(),
        "finite_transforms:entity",
        "REAL child-side target '{}' must canonicalize to 'entity'",
        entry.target
    );

    // Negative control on the merge grain: a DIFFERENT entity
    // (distinct index) violating the same rule collapses to the SAME
    // fingerprint — the intended cross-seed merge, exercised against
    // real bevy_ecs formatting rather than hand-fed strings.
    // Fresh `App`s allocate entities deterministically, so the
    // second app spawns DECOY entities first to push its offending
    // entity to a provably different index — otherwise both targets
    // would be the identical string and the merge claim would pass
    // vacuously.
    let scenario_b: Scenario =
        serde_json::from_str(r#"{"bot":{"type":"chaos","seed":99},"duration_s":0.1,"tps":60}"#)
            .unwrap();
    let mut app_b = App::new();
    app_b.add_systems(
        bevy_app::Update,
        |mut commands: bevy_ecs::prelude::Commands| {
            // Decoys: shift the offending entity's index relative to
            // the first app's (no Transform needed — allocation
            // order is what matters for the debug target).
            commands.spawn_empty();
            commands.spawn_empty();
            commands.spawn(Transform::from_xyz(0.0, f32::NAN, 0.0));
        },
    );
    let report_b = run_scenario(&mut app_b, &scenario_b).unwrap();
    let entry_b = report_b
        .violations
        .iter()
        .find(|v| v.rule == "finite_transforms")
        .expect("second seed's oracle must fire too");
    // Non-vacuity guard: the two findings must come from genuinely
    // different entities (distinct debug targets) or the collapse
    // below proves nothing.
    assert_ne!(
        entry.target, entry_b.target,
        "negative control requires two DIFFERENT entity debug targets"
    );
    let fp_b = FindingFingerprint::of_blocking(entry_b).expect("blocking");
    assert_eq!(
        fp.key(),
        fp_b.key(),
        "different entities, same bug, one key ({:?} vs {:?})",
        entry.target,
        entry_b.target
    );
}

// ---------------------------------------------------------------------------
// Non-u64 seed env var: a config error, not a mystery failure.
// ---------------------------------------------------------------------------

/// H2: a malformed `BEVY_PLAYTEST_SEED` must exit CONFIG_ERROR (3)
/// with a diagnostic — so a typo'd sweep seed surfaces as
/// infrastructure, never as an inscrutable per-member game failure.
#[test]
fn non_u64_seed_env_exits_config_error() {
    let _slot = common::run_slot();
    let dir = temp_dir("bad-seed");
    std::fs::create_dir_all(&dir).unwrap();
    let scenario_path = dir.join("scenario.json");
    std::fs::write(
        &scenario_path,
        r#"{"bot":{"type":"chaos","seed":1},"duration_s":0.1,"tps":60}"#,
    )
    .unwrap();
    // Scrub any playtest env leaked into the test-runner's own
    // environment: `Command` inherits it wholesale, the takeover
    // REJECTS unknown BEVY_PLAYTEST_* variables, and a leaked var
    // would CONFIG_ERROR this child for reasons unrelated to the
    // seed parsing under test. Same strip-then-set doctrine as
    // `battery::run_with_config`.
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_takeover_demo"));
    for (key, _) in std::env::vars_os() {
        if let Some(k) = key.to_str() {
            if k.starts_with("BEVY_PLAYTEST") {
                cmd.env_remove(k);
            }
        }
    }
    let out = cmd
        .env(ENV_SCENARIO, scenario_path.to_string_lossy().into_owned())
        .env(ENV_SEED, "not-a-number")
        .output()
        .expect("spawn demo");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(bevy_playtest::plugin::exit_code::CONFIG_ERROR),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("not a u64"),
        "diagnostic must name the parse failure: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
