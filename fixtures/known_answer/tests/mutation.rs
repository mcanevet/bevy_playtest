//! Mutation harness — OPT-IN: `--features playtest,measure`.
//!
//! Two deliberate code mutations, selected via the game's own
//! `KNOWN_ANSWER_MUTATION` env var (deliberately NOT prefixed with
//! `BEVY_PLAYTEST_` — see the fixture README for why; the harness
//! strips/pols its own namespace only, so unprefixed vars pass
//! through to the spawned game processes):
//!
//! - `P1_dead`: P1's unmute system early-outs. Ground truth declares
//!   KeyU LIVE, the sweep must now find it INERT — the
//!   declared-but-dead drift direction. (TODO D1-d: assert a
//!   `DeclaredButDead` finding once the declared-binding reader
//!   lens exists; today only the sweep-side inertness is
//!   observable, so the assertion is framed as sweep-vs-truth.)
//! - `N1_hotbar_full`: all ten hotbar slots go live. Ground truth
//!   declares Digit3..Digit0 INERT, the sweep must now find all
//!   ten LIVE — the inert-but-live drift direction.
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

use bevy_playtest::discovery::flow::{DiscoveryConfig, discover};
use bevy_playtest::discovery::probe_scenarios::ProbePlan;
use bevy_playtest::discovery::surface::KeyConfidence;

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

/// Discover the fixture's surface with a mutation active. Sets the
/// env var for the CURRENT process (spawners inherit it — the probe
/// pool strips only `BEVY_PLAYTEST*`), then removes it on EVERY
/// exit path via a drop guard. Tests in this binary MUST run
/// single-threaded (`--test-threads=1`) so the env mutation never
/// cross-contaminates a concurrent run.
///
/// Everything fallible (cache-dir creation, config construction)
/// happens BEFORE the var is set, so a panic there cannot leak the
/// mutation into a subsequent run; the drop guard covers panics
/// inside `discover` itself.
fn discover_with_mutation(
    tag: &str,
    mutation: &str,
) -> bevy_playtest::discovery::surface::DiscoveredSurface {
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
    let outcome = std::panic::catch_unwind(|| discover(&config));
    drop(_restore);
    match outcome {
        Ok(result) => result.expect("discovery completes under mutation").surface,
        // Resume with context instead of `.expect` on the opaque
        // Box<Any> payload (which would swallow the original panic
        // message): the underlying discovery panic is already
        // printed by the default hook; re-raise ours so the failure
        // attributes cleanly.
        Err(payload) => {
            let _ = payload; // opaque Box<dyn Any>; original message
            // was already printed by the panic hook
            panic!(
                "discovery panicked under mutation '{mutation}' (tag \
                 '{tag}'); see the panic printed above — a mutated run \
                 must never fail silently"
            );
        }
    }
}

fn live(surface: &bevy_playtest::discovery::surface::DiscoveredSurface, key: &str) -> bool {
    surface
        .keys
        .iter()
        .find(|k| k.key == key)
        .map(|f| f.effect.is_some() && f.confidence != KeyConfidence::Unclassified)
        .unwrap_or(false)
}

/// `P1_dead`: the unmute system early-outs while ground truth still
/// declares KeyU live — the DECLARED-BUT-DEAD drift direction. The
/// sweep must now report KeyU inert.
///
/// TODO(D1-d): once the declared-binding reader lens exists, this
/// must also assert a `DeclaredButDead` finding (source cites the
/// binding; the boot-time probe refutes it). Today only the
/// sweep-side half is observable, so the assertion is: KeyU inert
/// here, live in the pristine run (tests/measure.rs).
#[test]
fn mutation_p1_dead_makes_keyu_inert_where_truth_says_live() {
    let surface = discover_with_mutation("p1_dead", "P1_dead");

    // The mutated binding itself: declared-live in ground truth,
    // now inert under the sweep.
    assert!(
        !live(&surface, "KeyU"),
        "P1_dead: KeyU must be reported inert (declared-but-dead drift)"
    );

    // Blast radius: the rest of the surface is untouched — the
    // mutation must not leak into unrelated plants.
    for key in ["KeyW", "KeyA", "KeyS", "KeyD", "KeyR", "Digit1", "Digit2"] {
        assert!(
            live(&surface, key),
            "P1_dead blast radius: {key} must remain live"
        );
    }
    for key in ["KeyL", "ShiftLeft", "KeyT"] {
        assert!(
            !live(&surface, key),
            "P1_dead blast radius: {key} must remain an expected miss"
        );
    }
    assert!(!live(&surface, "KeyX"), "P1_dead blast radius: KeyX inert");
}

/// `N1_hotbar_full`: all ten hotbar slots go live while ground truth
/// declares only Digit1/Digit2 live — a negative flipped positive
/// (the inert-but-live drift direction). The sweep must find all
/// ten digits live.
#[test]
fn mutation_n1_hotbar_full_flips_negative_to_positive() {
    let surface = discover_with_mutation("n1_hotbar_full", "N1_hotbar_full");

    // All ten slots now live — the eight adversarial-negative rows
    // (Digit3..Digit9, Digit0) flip to discovered-live.
    for key in [
        "Digit1", "Digit2", "Digit3", "Digit4", "Digit5", "Digit6", "Digit7", "Digit8", "Digit9",
        "Digit0",
    ] {
        assert!(
            live(&surface, key),
            "N1_hotbar_full: {key} must flip to live (all slots active)"
        );
    }

    // Blast radius: everything else unchanged.
    assert!(
        live(&surface, "KeyU"),
        "N1_hotbar_full blast radius: KeyU must remain live"
    );
    for key in ["KeyX", "KeyM", "Enter"] {
        assert!(
            !live(&surface, key),
            "N1_hotbar_full blast radius: {key} must remain inert"
        );
    }
}
