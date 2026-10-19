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

use bevy_playtest::choice::ChoiceStream;

/// The same seed and bound must produce the identical choice sequence
/// regardless of call order elsewhere — the foundation of the
/// "deterministic per seed" claim. Both drivers construct
/// ChoiceStream::from_seed; if either re-seeds mid-run or reorders
/// draws, replay diverges.
#[test]
fn same_seed_same_bound_same_choice_sequence() {
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
    let mut a = ChoiceStream::from_seed(1);
    let mut b = ChoiceStream::from_seed(2);
    let seq_a: Vec<u64> = (0..30).map(|f| a.below(f, 8)).collect();
    let seq_b: Vec<u64> = (0..30).map(|f| b.below(f, 8)).collect();
    assert_ne!(seq_a, seq_b, "distinct seeds produced identical sequences");
}

/// Mode divergence (C7): the in-process driver samples a key every
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
/// Since the BRP driver cannot be driven without a game binary here,
/// we encode the in-process half and the interval math the BRP driver
/// SHOULD use; equality of the two formulas is what the fix must
/// restore.
#[test]
#[ignore = "BRP driver hardcodes interval 6 and bound 8 (C7): custom surface/rate diverges"]
fn custom_surface_interval_matches_brp_contract() {
    // In-process formula (driver.rs):
    let tps = 60u64;
    let rate_hz = 5u64; // NON-default input rate
    let surface_len = 3u64; // NON-default key surface
    let in_process_interval = (tps / rate_hz).max(1);
    let in_process_bound = surface_len;

    // BRP formula as implemented (brp_runner.rs: hardcoded):
    let brp_interval = 6u64;
    let brp_bound = 8u64;

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
