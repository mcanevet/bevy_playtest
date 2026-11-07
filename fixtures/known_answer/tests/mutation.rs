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
//!   AND no finding claiming it live. (TODO D1-d: assert a
//!   `DeclaredButDead` finding once the declared-binding reader lens
//!   exists; today the inert-entry-plus-no-finding pair is the best
//!   observable stand-in for that alarm.)
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
/// TODO(D1-d): once the declared-binding reader lens exists, this
/// must also assert a `DeclaredButDead` finding (source cites the
/// binding; the boot-time probe refutes it). Live in the pristine
/// run: tests/measure.rs.
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
