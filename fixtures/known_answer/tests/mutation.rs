//! Mutation harness — OPT-IN: `--features playtest,measure`.
//!
//! Two deliberate code mutations, selected via the game's own
//! `KNOWN_ANSWER_MUTATION` env var (deliberately NOT prefixed with
//! `BEVY_PLAYTEST_` — see the fixture README for why; the harness
//! strips/polishes its own namespace only, so unprefixed vars pass
//! through to the spawned game processes):
//!
//! - `P1_dead`: P1's unmute system early-outs. Ground truth declares
//!   KeyU LIVE, the sweep must now find it INERT — the
//!   declared-but-dead drift direction. At the D1-c draft level that
//!   means BOTH halves: a PINNED inert entry (probe ran, ruled out)
//!   AND no finding claiming it live. (The D1-d declared-binding
//!   reader lens exists now; its alarm contracts are asserted in
//!   tests/measure.rs and in the `LW_dead` test below.)
//! - `N1_hotbar_full`: all ten hotbar slots go live. Ground truth
//!   declares Digit3..Digit0 INERT, the sweep must now find all ten
//!   LIVE with `DiscoveredButUndeclared` findings — the
//!   inert-but-live drift direction.
//!
//! Both runs use DISTINCT `fixture_name` tags and fresh temp caches:
//! `CacheKey::for_binary` hashes binary + features + fixture_name +
//! plan fingerprint but NOT env vars, so a mutated run sharing the
//! pristine tag would read a poisoned cache entry.
//!
//! Run (single-threaded; both probes and env-var mutation require
//! process-level exclusivity):
//! ```text
//! cargo test --manifest-path fixtures/Cargo.toml -p fixture_known_answer \
//!   --features playtest,measure --test mutation -- --test-threads=1
//! ```

#![cfg(feature = "measure")]

mod support;

use bevy_playtest::discovery::draft::{draft_surface, DraftOutcome};
use bevy_playtest::discovery::flow::DiscoveryConfig;
use bevy_playtest::discovery::probe_scenarios::ProbePlan;
use bevy_playtest::discovery::surface::{DiscoveryFindingKind, Evidence};

/// The game's mutation selector (see module docs). Values: exactly
/// `P1_dead`, `N1_hotbar_full`, or unset/empty (none, enforced by a
/// loud panic in the game on anything else).
const ENV_MUTATION: &str = "KNOWN_ANSWER_MUTATION";

/// Candidate list: defaults union every plant-catalog key (shared
/// constant in `tests/support` — the measure harness probes the
/// same matrix; the mutations need KeyU probed for P1_dead and the
/// digits are already in the default set).
fn candidate_keys() -> Vec<String> {
    let mut keys: Vec<String> = bevy_playtest::discovery::probe_scenarios::PROBE_CANDIDATE_KEYS
        .iter()
        .map(|s| s.to_string())
        .collect();
    for extra in support::EXTRA_CANDIDATE_KEYS {
        if !keys.iter().any(|k| k == extra) {
            keys.push(extra.to_string());
        }
    }
    keys
}

/// Fresh per-run temp cache (never the repo's `.bevy_playtest/`).
fn temp_cache(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "bevy_playtest_known_answer_mutation_{}_{}_{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("temp cache dir creatable");
    dir
}

/// Run the tier-0 draft sweep on the fixture with a mutation
/// active. Sets the env var for the CURRENT process (spawners
/// inherit it — the probe pool strips only `BEVY_PLAYTEST*`), then
/// removes it on EVERY exit path via a drop guard. Tests in this
/// binary MUST run single-threaded (`--test-threads=1`) so the env
/// mutation never cross-contaminates a concurrent run.
///
/// Everything fallible (cache-dir creation, config construction)
/// happens BEFORE the var is set, so a panic there cannot leak the
/// mutation into a subsequent run; the drop guard covers panics
/// inside the sweep itself.
fn draft_with_mutation(tag: &str, mutation: &str) -> DraftOutcome {
    let cache_dir = temp_cache(tag);
    let config = DiscoveryConfig {
        plan: ProbePlan {
            candidate_keys: candidate_keys(),
            // Mouse grid OFF: this fixture measures the KEYBOARD
            // draft sweep's alarms (the `0 × 0` sanctioned slim
            // mode — no mouse jobs in the battery).
            mouse_grid_cols: 0,
            mouse_grid_rows: 0,
            ..ProbePlan::default()
        },
        cache_dir,
        fixture_name: Some(format!("known_answer_{tag}")),
        ..DiscoveryConfig::new(env!("CARGO_BIN_EXE_fixture_known_answer"))
    };

    /// Drop guard: the process-global env var must not outlive the
    /// mutated discovery run, even on panic.
    struct RestoreEnv(&'static str);
    impl Drop for RestoreEnv {
        fn drop(&mut self) {
            unsafe { std::env::remove_var(self.0) };
        }
    }

    // Edition 2024: env mutation is unsafe (process-global).
    unsafe { std::env::set_var(ENV_MUTATION, mutation) };
    let _restore = RestoreEnv(ENV_MUTATION);
    let outcome = std::panic::catch_unwind(|| draft_surface(&config));
    drop(_restore);
    match outcome {
        Ok(result) => {
            let draft = result.expect("draft sweep completes under mutation");
            assert!(
                !draft.from_cache,
                "fresh temp cache must probe, never recall a poisoned entry"
            );
            draft
        }
        // Resume with context instead of `.expect` on the opaque
        // Box<Any> payload (which would swallow the original panic
        // message): the underlying discovery panic is already
        // printed by the default hook; re-raise ours so the failure
        // attributes cleanly.
        Err(payload) => {
            let _ = payload; // opaque Box<dyn Any>; original message
            // was already printed by the panic hook
            panic!(
                "draft sweep panicked under mutation '{mutation}' (tag \
                 '{tag}'); see the panic printed above — a mutated run \
                 must never fail silently"
            );
        }
    }
}

/// Sweep-lens liveness on the DRAFT: an entry with an observed
/// effect signature.
fn live(draft: &DraftOutcome, key: &str) -> bool {
    draft
        .entries
        .iter()
        .find(|e| e.input == key)
        .map(|e| e.effect_signature.is_some())
        .unwrap_or(false)
}

/// Sweep-lens inertness on the DRAFT: an entry backed by
/// `probe_run_inert` evidence (the probe RAN and ruled the key
/// out).
fn inert(draft: &DraftOutcome, key: &str) -> bool {
    draft
        .entries
        .iter()
        .find(|e| e.input == key)
        .map(|e| {
            e.effect_signature.is_none()
                && e
                    .evidence
                    .iter()
                    .any(|ev| matches!(ev, Evidence::ProbeRunInert { .. }))
        })
        .unwrap_or(false)
}

/// Does the findings channel claim `key` live (any
/// `DiscoveredButUndeclared` finding naming it)?
fn claimed_live(draft: &DraftOutcome, key: &str) -> bool {
    draft.findings.iter().any(|f| {
        f.input == key && matches!(f.kind, DiscoveryFindingKind::DiscoveredButUndeclared)
    })
}

/// `P1_dead`: the unmute system early-outs while ground truth still
/// declares KeyU live — the DECLARED-BUT-DEAD drift direction. The
/// draft must BOTH pin KeyU as an inert entry (probed, ruled out)
/// AND leave the findings channel silent about it — the residue
/// report must never claim a dead binding live. This pair is
/// today's best observable stand-in for the declared-but-dead
/// alarm.
///
/// D1-d update: the declared-binding reader lens now exists — the
/// pressed-never-fired alarm arm is asserted LIVE in
/// `mutation_lw_dead_alarms_on_healthy_binding_turned_dead` below,
/// and the static dead plant's alarm in the pristine run is
/// asserted in tests/measure.rs
/// (`lw_reconcile_lens_known_answer_commitments`).
#[test]
fn mutation_p1_dead_makes_keyu_inert_where_truth_says_live() {
    let draft = draft_with_mutation("p1_dead", "P1_dead");

    // The mutated binding itself: declared-live in ground truth,
    // now a PINNED inert entry under the sweep — and no finding
    // claims it live (the inert half AND the no-false-claim half).
    assert!(
        !live(&draft, "KeyU"),
        "P1_dead: KeyU must be reported inert (declared-but-dead drift)"
    );
    assert!(
        inert(&draft, "KeyU"),
        "P1_dead: KeyU must be a PINNED inert entry — probed and ruled out, \
         visible in the draft as the negative it now is"
    );
    assert!(
        !claimed_live(&draft, "KeyU"),
        "P1_dead: no finding may claim KeyU live — a dead binding must not \
         enter the residue report as live"
    );

    // Blast radius: the rest of the surface is untouched — the
    // mutation must not leak into unrelated plants.
    for key in ["KeyW", "KeyA", "KeyS", "KeyD", "KeyR", "Digit1", "Digit2"] {
        assert!(
            live(&draft, key),
            "P1_dead blast radius: {key} must remain live"
        );
        assert!(
            claimed_live(&draft, key),
            "P1_dead blast radius: {key} must keep its live finding"
        );
    }
    for key in ["KeyL", "ShiftLeft", "KeyT"] {
        assert!(
            !live(&draft, key),
            "P1_dead blast radius: {key} must remain an expected miss"
        );
    }
    assert!(
        inert(&draft, "KeyX"),
        "P1_dead blast radius: KeyX stays a pinned inert entry"
    );
}

/// `N1_hotbar_full`: all ten hotbar slots go live while ground truth
/// declares only Digit1/Digit2 live — a negative flipped positive
/// (the inert-but-live drift direction). The sweep must find all
/// ten digits live, each carrying a `DiscoveredButUndeclared`
/// finding.
#[test]
fn mutation_n1_hotbar_full_flips_negative_to_positive() {
    let draft = draft_with_mutation("n1_hotbar_full", "N1_hotbar_full");

    // All ten slots now live WITH findings — the eight
    // adversarial-negative rows (Digit3..Digit9, Digit0) flip to
    // discovered-live, and the residue report must say so.
    for key in [
        "Digit1", "Digit2", "Digit3", "Digit4", "Digit5", "Digit6", "Digit7", "Digit8", "Digit9",
        "Digit0",
    ] {
        assert!(
            live(&draft, key),
            "N1_hotbar_full: {key} must flip to live (all slots active)"
        );
        assert!(
            claimed_live(&draft, key),
            "N1_hotbar_full: {key} must carry a discovered_but_undeclared finding"
        );
    }

    // Blast radius: everything else unchanged.
    assert!(
        live(&draft, "KeyU"),
        "N1_hotbar_full blast radius: KeyU must remain live"
    );
    for key in ["KeyX", "KeyM", "Enter"] {
        assert!(
            !live(&draft, key),
            "N1_hotbar_full blast radius: {key} must remain inert"
        );
        assert!(
            inert(&draft, key),
            "N1_hotbar_full blast radius: {key} must remain a pinned inert entry"
        );
    }
}

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
    // DiscoveryFindingKind already imported at the top of this file.

    // Draft live input set for reconcile (the same for both arms:
    // LW_dead only disables the Live action at frame 35, long
    // after the boot-time probe sweep sampled the surface — the
    // DRAFT is mutation-blind here, so ONE pristine draft serves
    // both reconcile calls).
    let draft = draft_pristine("lw_dead");
    let draft_live: std::collections::BTreeSet<String> = draft
        .entries
        .iter()
        .filter(|e| e.effect_signature.is_some())
        .map(|e| e.input.clone())
        .collect();

    // PRISTINE arm (control): healthy Live wiring — NO KeyJ alarm,
    // and exactly the one static L1 alarm (KeyN).
    let pristine = support::run_lw_lens("lw_pristine");
    let rec_pristine = support::reconcile_report(&pristine, &draft_live);
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
    // Everything fallible happens BEFORE the var is set (same
    // discipline as draft_with_mutation) — a panic cannot leak the
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
    let rec_mutated = support::reconcile_report(&mutated, &draft_live);
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
    // multi-press in BOTH arms), Ignored/Gated/RonBound stay
    // ContextUnverified, never alarms.
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

/// A pristine (unmutated) draft sweep with a fresh temp cache — the
/// LW_dead test's reconcile sets derive from it (the mutation is
/// frame-35 runtime state, invisible to the boot-time probes).
fn draft_pristine(tag: &str) -> DraftOutcome {
    let cache_dir = temp_cache(tag);
    let config = DiscoveryConfig {
        plan: ProbePlan {
            candidate_keys: candidate_keys(),
            mouse_grid_cols: 0,
            mouse_grid_rows: 0,
            ..ProbePlan::default()
        },
        cache_dir,
        fixture_name: Some(format!("known_answer_{tag}")),
        ..DiscoveryConfig::new(env!("CARGO_BIN_EXE_fixture_known_answer"))
    };
    let draft = draft_surface(&config)
        .expect("pristine draft sweep completes (the LW_dead test only \
                mutates the tap run, never the sweep)");
    assert!(
        !draft.from_cache,
        "fresh temp cache must probe, never recall a stale entry"
    );
    draft
}
