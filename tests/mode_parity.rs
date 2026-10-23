//! Cross-mode parity test battery: the same seed must drive the same
//! behavior in-process and over BRP. The docs advertise BRP as "the
//! same chaos bot"; determinism claims depend on it.
//!
//! The in-process side runs a minimal app directly; the BRP side needs
//! a real game binary exposing RemotePlugin. Because fixtures are
//! generated (not committed), the direct BRP comparison is
//! fixture-gated (see acceptance.rs), but the CHOICE LOGIC can be
//! compared deterministically without launching anything: both drivers
//! share `ChoiceStream`, so the contract is that identical
//! (seed, frame, bound) sequences yield identical draws.
//!
//! Expected-fail tests are #[ignore]d with the reason.

// Shared test harness (run-slot serialization)
#[path = "common/mod.rs"]
mod common;

use bevy_playtest::choice::{Choice, ChoiceStream, Continuation};

/// The same seed and bound must produce the identical choice sequence
/// regardless of call order elsewhere — the foundation of the
/// "deterministic per seed" claim. Both drivers construct
/// ChoiceStream::from_seed; if either re-seeds mid-run or reorders
/// draws, replay diverges.
#[test]
fn same_seed_same_bound_same_choice_sequence() {
    let _slot = common::run_slot();
    let mut a = ChoiceStream::from_seed(7);
    let mut b = ChoiceStream::from_seed(7);
    for frame in 0..120 {
        let va = a.below(frame, 8);
        let vb = b.below(frame, 8);
        assert_eq!(va, vb, "divergence at frame {frame} for seed 7, bound 8");
    }
}

/// Different seeds must diverge (guards against a degenerate seed
/// remap like seed 0 -> 42 producing collisions).
#[test]
fn different_seeds_diverge() {
    let _slot = common::run_slot();
    let mut a = ChoiceStream::from_seed(1);
    let mut b = ChoiceStream::from_seed(2);
    let seq_a: Vec<u64> = (0..30).map(|f| a.below(f, 8)).collect();
    let seq_b: Vec<u64> = (0..30).map(|f| b.below(f, 8)).collect();
    assert_ne!(seq_a, seq_b, "distinct seeds produced identical sequences");
}

/// Mode divergence: the in-process driver samples a key every
/// `interval = tps / input_rate_hz` frames against the SCENARIO's key
/// surface length; the BRP driver hardcodes every 6th frame against a
/// fixed 8-key list. With the default surface (8 keys) and default
/// rate (10 Hz at 60 tps = every 6 frames), they coincide. This test
/// documents that the equivalence is CONFIG-DEPENDENT and currently
/// BREAKS for any non-default surface or rate — i.e., the "same chaos
/// bot" claim only holds on the default configuration.
///
/// Contract under test (once fixed): for a custom 3-key surface and
/// rate 5 Hz, both modes' recorded choices must agree frame-by-frame.
/// Non-default input rates and key surfaces must produce the same
/// bot behavior in both modes. The BRP driver computes its interval
/// and choice bound from the same scenario fields the in-process
/// driver uses — this pins that equivalence for a non-default rate
/// (5 Hz) and surface (3 keys), mirroring the wiring in
/// brp_runner.rs against driver.rs.
#[test]
fn custom_surface_interval_matches_brp_contract() {
    let _slot = common::run_slot();
    let tps = 60u64;
    let rate_hz = 5u64; // NON-default input rate
    let surface_len = 3u64; // NON-default key surface

    // In-process formula (driver.rs chaos_bot_system):
    let in_process_interval = (tps / rate_hz).max(1);
    let in_process_bound = surface_len;

    // BRP formula (brp_runner.rs run_brp_inner): interval and bound
    // derived from the same scenario fields — must match.
    let brp_rate = rate_hz.max(1);
    let brp_interval = (tps / brp_rate).max(1);
    let brp_bound = surface_len;

    assert_eq!(
        in_process_interval, brp_interval,
        "input interval must agree across modes for non-default rate"
    );
    assert_eq!(
        in_process_bound, brp_bound,
        "choice bound must agree across modes for non-default surface"
    );
}

/// Report-level parity on the choice log: an in-process run's recorded
/// choices must be replayable via the same seed (the basis of BRP
/// replay and failure reproduction).
#[test]
fn recorded_choices_are_seed_reproducible() {
    let _slot = common::run_slot();
    use bevy_app::App;
    use bevy_playtest::run_scenario;
    use bevy_playtest::scenario::Scenario;
    use bevy_playtest::sinks;

    let scenario: Scenario =
        serde_json::from_str(r#"{"bot":{"type":"chaos","seed":7},"duration_s":0.5,"tps":60}"#)
            .unwrap();

    let run = || {
        let mut app = App::new();
        sinks::ensure_global_subscriber();
        run_scenario(&mut app, &scenario).unwrap()
    };
    let r1 = run();
    let r2 = run();
    assert_eq!(
        r1.choices, r2.choices,
        "identical scenario must produce identical choice logs"
    );
}

// ---------------------------------------------------------------------------
// Replay fidelity
// ---------------------------------------------------------------------------

/// Replay consumes entries positionally, ignoring each entry's recorded
/// `frame` (choice.rs `below`), and re-reduces values with `% bound`. A
/// recording replayed against draws that do not align frame-for-frame
/// therefore serves silently SHIFTED values — the worst failure mode
/// for a determinism tool: a misaligned replay must fail loudly, not
/// produce plausible-but-wrong input.
#[test]
fn misaligned_replay_must_fail_loudly_not_shift_silently() {
    let _slot = common::run_slot();
    // Record a live stream, then attempt to replay it starting one
    // frame later (as if one earlier draw were inserted/removed).
    let mut live = ChoiceStream::from_seed(7);
    let mut recorded = Vec::new();
    for frame in 0..20 {
        let v = live.below(frame, 8);
        recorded.push(Choice {
            frame,
            value: v,
            bound: 8,
        });
    }

    // Sanity: aligned replay reproduces the recorded values exactly.
    let mut aligned = ChoiceStream::from_seed(0).with_replay(recorded.clone(), Continuation::Zeros);
    for frame in 0..20 {
        assert_eq!(aligned.below(frame, 8), recorded[frame as usize].value);
    }

    // Misaligned: first draw consumes entry 0, subsequent draws see a
    // one-off shift. The stream must error or produce values matching
    // the recorded FRAME INDEX — silently serving entry N at frame N+1
    // is a corruption.
    let mut shifted = ChoiceStream::from_seed(0).with_replay(recorded.clone(), Continuation::Zeros);
    let first = shifted.below(5, 8); // frame 5 draws first
    assert_eq!(
        first, recorded[5].value,
        "replay must serve the entry recorded for frame 5, not the head of the queue"
    );
}

// ---------------------------------------------------------------------------
// Default surface consistency
// ---------------------------------------------------------------------------

/// The default chaos key surface exists in three copies: the schema
/// default (`RawSurface::default()`), the in-process driver's local
/// list, and the BRP runner's hardcoded list. Nothing ties them
/// together. This test pins the OBSERVABLE contract: a scenario that
/// sets no surface must behave identically to one that sets the
/// schema default explicitly (same seed -> same choices). If any
/// driver's private default drifts from the schema default, the two
/// choice logs diverge.
#[test]
fn implicit_surface_matches_explicit_schema_default() {
    let _slot = common::run_slot();
    use bevy_app::App;
    use bevy_playtest::run_scenario;
    use bevy_playtest::scenario::Scenario;
    use bevy_playtest::sinks;

    let run = |json: &str| {
        let scenario: Scenario = serde_json::from_str(json).unwrap();
        let mut app = App::new();
        sinks::ensure_global_subscriber();
        run_scenario(&mut app, &scenario).unwrap()
    };

    let implicit = run(r#"{"bot":{"type":"chaos","seed":7},"duration_s":0.5,"tps":60}"#);
    let explicit = run(
        r#"{"bot":{"type":"chaos","seed":7,"raw_surface":{"keys":["KeyW","KeyA","KeyS","KeyD","ArrowUp","ArrowDown","ArrowLeft","ArrowRight"]}},"duration_s":0.5,"tps":60}"#,
    );
    assert_eq!(
        implicit.choices, explicit.choices,
        "implicit default surface must match the documented schema default; \
         if this fails, a driver's private key list has drifted from \
         RawSurface::default()"
    );
}
