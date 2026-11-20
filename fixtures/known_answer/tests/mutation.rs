//! Known-answer mutation harness — OPT-IN: `--features playtest,measure`.
//!
//! `LW_dead` kills a healthy Leafwing binding mid-run and asserts the
//! declared tier's alarm channel fires for it (and only for it), using
//! the `lw_reconciled` section every report now carries. (The probe-
//! sweep mutations that used to live here went away with input-surface
//! discovery.)
//!
//! Run (single-threaded; run_scenario exclusivity):
//! ```text
//! cargo test --manifest-path fixtures/Cargo.toml -p fixture_known_answer \
//!   --features playtest,measure --test mutation -- --test-threads=1
//! ```

#![cfg(feature = "measure")]

mod support;

use bevy_playtest::discovery::surface::DiscoveryFindingKind;

/// The game's mutation selector: exactly `LW_dead`, or unset/empty
/// (none, enforced by a loud panic in the game on anything else).
const ENV_MUTATION: &str = "KNOWN_ANSWER_MUTATION";

/// `LW_dead`: the HEALTHY Live action (L5, KeyJ) is disabled at frame
/// 35 — after the reader's declared snapshot, before the scenario's
/// frame-40 press. The D1-d declared-tier contract, alarm direction:
///
/// - pristine run: exactly one DeclaredButDead alarm (L1 KeyN, the
///   static disabled plant) and NONE for KeyJ — the healthy binding
///   must not false-alarm;
/// - mutated run: the intent-tap records KeyJ pressed=1, fired=0,
///   disabled=true — reconcile MUST add the pressed-never-fired
///   `DeclaredButDead` alarm for KeyJ (the disabled arm and the
///   pressed-never-fired arm coincide: disabling IS what stops the
///   firing);
/// - blast radius: the OTHER declared rows' reconcile results are
///   unchanged (KeyN still its static alarm; chords never alarm).
///
/// This plus the static L1 plant proves the alarm channel BOTH
/// ways: a binding healthy-then-killed in the SAME run alarms, and
/// the pristine run stays false-tap-free.
#[test]
fn mutation_lw_dead_alarms_on_healthy_binding_turned_dead() {

    // PRISTINE arm (control): healthy Live wiring — NO KeyJ alarm,
    // and exactly the one static L1 alarm (KeyN).
    let pristine = support::run_lw_lens("lw_pristine");
    let rec_pristine = pristine
        .lw_reconciled
        .clone()
        .expect("pristine lw-reader run must stamp lw_reconciled");
    let pristine_alarms: Vec<&str> = rec_pristine
        .findings
        .iter()
        .filter(|f| matches!(f.kind, DiscoveryFindingKind::DeclaredButDead))
        .map(|f| f.input.as_str())
        .collect();
    assert_eq!(
        pristine_alarms,
        vec!["KeyN"],
        "pristine run: exactly the static L1 alarm, none for healthy KeyJ — \
         a false tap on the healthy binding would destroy the alarm channel"
    );

    // MUTATED arm: LW_dead disables Live at frame 35; the scenario's
    // frame-40 press lands on dead wiring. The tap must record
    // pressed=1, fired=0, disabled=true for the Live action…
    struct RestoreEnv(&'static str);
    impl Drop for RestoreEnv {
        fn drop(&mut self) {
            unsafe { std::env::remove_var(self.0) };
        }
    }
    // Everything fallible happens BEFORE the var is set  — a panic cannot leak the
    // mutation into a subsequent run.
    unsafe { std::env::set_var(ENV_MUTATION, "LW_dead") };
    let _restore = RestoreEnv(ENV_MUTATION);
    let mutated =
        std::panic::catch_unwind(|| support::run_lw_lens("lw_mutated"));
    drop(_restore);
    let mutated = match mutated {
        Ok(report) => report,
        Err(_) => panic!(
            "LW-lens run panicked under mutation 'LW_dead'; see the panic \
             printed above — a mutated run must never fail silently"
        ),
    };
    let live_stat = mutated
        .lw_tap_stats
        .iter()
        .find(|s| s.action == "Live")
        .expect("tap stats must carry the Live action");
    assert_eq!(
        (live_stat.pressed_frames, live_stat.fired_frames, live_stat.disabled),
        (1, 0, true),
        "LW_dead: the frame-40 KeyJ press must land as pressed-never-fired \
         with the action disabled"
    );

    // …and reconcile must ALARM on KeyJ: the pressed-never-fired
    // arm, proven on a binding the pristine run grades healthy.
    let rec_mutated = mutated
        .lw_reconciled
        .clone()
        .expect("mutated lw-reader run must stamp lw_reconciled");
    let mutated_alarms: Vec<&str> = rec_mutated
        .findings
        .iter()
        .filter(|f| matches!(f.kind, DiscoveryFindingKind::DeclaredButDead))
        .map(|f| f.input.as_str())
        .collect();
    assert_eq!(
        mutated_alarms,
        vec!["KeyJ", "KeyN"],
        "LW_dead reconcile: the turned-dead KeyJ must alarm (pressed-\
         never-fired arm) alongside the static L1 KeyN alarm — and no \
         others"
    );

    // Blast radius: the rest of the declared tier is unchanged —
    // chords never alarm (Combo fired under the synchronized
    // multi-press in BOTH arms), Ignored/Gated/RonBound keep the same
    // findings and never alarm.
    fn kinds<'a>(
        rec: &'a bevy_playtest::discovery::lw::ReconciledLw,
        key: &str,
    ) -> Vec<&'a DiscoveryFindingKind> {
        rec.findings
            .iter()
            .filter(|f| f.input == key)
            .map(|f| &f.kind)
            .collect()
    }
    for key in ["KeyO", "KeyP", "KeyH"] {
        let pristine_kinds = kinds(&rec_pristine, key);
        let mutated_kinds = kinds(&rec_mutated, key);
        assert_eq!(
            pristine_kinds, mutated_kinds,
            "LW_dead blast radius: {key}'s findings must be unchanged"
        );
        assert!(
            !mutated_kinds
                .iter()
                .any(|k| matches!(k, DiscoveryFindingKind::DeclaredButDead)),
            "LW_dead blast radius: {key} must never alarm"
        );
    }
    assert!(
        kinds(&rec_mutated, "ControlLeft").is_empty()
            && kinds(&rec_mutated, "KeyI").is_empty(),
        "LW_dead blast radius: chord members keep NO findings in the \
         mutated run"
    );
}
