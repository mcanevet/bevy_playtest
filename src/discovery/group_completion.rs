//! Group completion (Phase 6): heuristic sibling inference over
//! movement clusters.
//!
//! Differential probing taps one key per run at one probe point, so
//! a key that only matters in an unprobed state reads as inert — a
//! false negative the design doc calls state-gating. Movement
//! clusters (WASD, arrow keys) are near-universal in games: if ANY
//! cluster member demonstrably diverges, its siblings are likely
//! consumed too. Group completion records that belief WITHOUT
//! fabricating evidence:
//!
//! - Siblings are marked [`KeyConfidence::Inferred`] — never
//!   `Exact`, or the confidence field lies about what was OBSERVED.
//! - No `Effect` is invented: inferred siblings carry `None`.
//! - Only an OBSERVED, INFORMATIVE divergence triggers completion:
//!   `effect.is_some()` AND confidence ≠ `Unclassified`. The control
//!   key probed against itself is unclassifiable by construction, so
//!   a nondeterministic control run handing it a spurious "effect"
//!   must never seed inference. Inferred members never chain further
//!   inference.
//!
//! Confidence lives in two layers (see [`DiscoveredSurface`]):
//! `keys[].confidence` is the RAW probe record; the
//! `key_confidence` override map is the interpretation layer.
//! Probed-but-inert siblings keep their raw record (probed, trusted
//! negative at the time) and are upgraded via the map;
//! never-probed siblings enter `keys` directly as `Inferred` (they
//! have no probe record to preserve). `confidence_for` is the read
//! API and resolves both.

use super::surface::{DiscoveredSurface, KeyConfidence, KeyFinding};

/// The movement clusters group completion reasons over. Near
/// keyboard-layout-universal directional bindings; deliberately a
/// short, high-prior list (adding clusters multiplies false
/// "inferred" marks).
pub const MOVEMENT_CLUSTERS: &[&[&str]] = &[
    &["KeyW", "KeyA", "KeyS", "KeyD"],
    &["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight"],
];

/// Apply group completion to a discovered surface (in place).
///
/// For each movement cluster where at least one member was observed
/// to cause an INFORMATIVE divergence (an effect with confidence
/// other than `Unclassified`):
/// - probed-but-inert siblings are marked `Inferred` via the
///   `key_confidence` override map (their raw probe record is
///   preserved);
/// - never-probed siblings are appended to `keys` as
///   `Inferred` with no effect;
/// - a note records the completion for report readers.
///
/// The control key never triggers: its self-probe is vacuous by
/// construction (probe run == control baseline), so treating its
/// "effect" as evidence would let a nondeterministic control
/// run bootstrap inference across the whole cluster (e.g. a
/// `KeyW` control seeding A/S/D). Its finding can still be a
/// sibling beneficiary: if another cluster member genuinely
/// diverged, an inert control-key probe is upgraded like any
/// other probed-but-inert key.
///
/// Idempotent, deterministic (cluster order), and never downgrades
/// an observed `Exact` finding.
pub fn apply_group_completion(surface: &mut DiscoveredSurface) {
    for cluster in MOVEMENT_CLUSTERS {
        // Only an OBSERVED, INFORMATIVE divergence triggers
        // completion. `Unclassified` findings (the control key
        // probed against itself) carry no usable evidence — a
        // nondeterministic control run can hand the self-probe a
        // spurious effect, and that must not seed inference.
        // Inferred members (effect None) never chain further
        // inference either.
        let trigger = cluster.iter().find_map(|key| {
            surface
                .keys
                .iter()
                .find(|f| f.key == *key)
                .filter(|f| f.effect.is_some() && f.confidence != KeyConfidence::Unclassified)
                .map(|f| f.key.clone())
        });
        let Some(triggered_by) = trigger else {
            continue;
        };

        let mut inferred: Vec<&str> = vec![];
        for key in *cluster {
            match surface.keys.iter().position(|f| f.key == *key) {
                // Observed divergence in its own run: untouched.
                Some(i) if surface.keys[i].effect.is_some() => {}
                // Probed but inert: upgrade via the interpretation
                // layer, preserving the raw probe record. Skip if
                // already overridden (idempotence).
                Some(i) => {
                    if surface.keys[i].confidence == KeyConfidence::Exact
                        && !surface.key_confidence.iter().any(|(k, _)| k == *key)
                    {
                        surface
                            .key_confidence
                            .push((key.to_string(), KeyConfidence::Inferred));
                        inferred.push(key);
                    }
                }
                // Never probed: appears in the surface by inference
                // alone, with no effect.
                None => {
                    surface.keys.push(KeyFinding {
                        key: key.to_string(),
                        effect: None,
                        confidence: KeyConfidence::Inferred,
                    });
                    inferred.push(key);
                }
            }
        }

        if !inferred.is_empty() {
            surface.notes.push(format!(
                "group completion: {} inferred from observed divergence at {}",
                inferred.join(", "),
                triggered_by,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::diff::Effect;
    use crate::discovery::surface::{
        DiscoveredSurface, KeyConfidence, KeyFinding, SurfaceConfidence,
    };

    fn finding(key: &str, effect: Option<Effect>) -> KeyFinding {
        KeyFinding {
            key: key.to_string(),
            effect,
            confidence: KeyConfidence::Exact,
        }
    }

    fn effect(delta: u32) -> Option<Effect> {
        Some(Effect {
            first_divergence_frame_delta: delta,
        })
    }

    fn surface_with(keys: &[KeyFinding]) -> DiscoveredSurface {
        DiscoveredSurface {
            keys: keys.to_vec(),
            confidence: SurfaceConfidence::Exact,
            ..Default::default()
        }
    }

    fn wasd_probed() -> DiscoveredSurface {
        surface_with(&[
            finding("KeyW", effect(2)),
            finding("KeyA", None),
            finding("KeyS", None),
            finding("KeyD", None),
            finding("Digit1", None),
        ])
    }

    #[test]
    fn diverging_was_member_infers_siblings() {
        let mut s = wasd_probed();
        apply_group_completion(&mut s);
        for key in ["KeyA", "KeyS", "KeyD"] {
            assert_eq!(s.confidence_for(key), Some(KeyConfidence::Inferred));
            // No effect is invented for inferred siblings.
            let f = s.keys.iter().find(|f| f.key == key).unwrap();
            assert!(f.effect.is_none());
            // Raw probe record preserved: the finding itself still
            // says "probed, Exact-negative"; the upgrade lives in
            // the interpretation layer.
            assert_eq!(f.confidence, KeyConfidence::Exact);
            assert!(s
                .key_confidence
                .iter()
                .any(|(k, c)| k == key && *c == KeyConfidence::Inferred));
        }
        // The observed diverger keeps its Exact finding.
        assert_eq!(s.confidence_for("KeyW"), Some(KeyConfidence::Exact));
        // Non-cluster keys are untouched: probed, trusted negative.
        assert_eq!(s.confidence_for("Digit1"), Some(KeyConfidence::Exact));
    }

    #[test]
    fn quiescent_cluster_stays_untouched() {
        let mut s = surface_with(&[
            finding("KeyW", None),
            finding("KeyA", None),
            finding("KeyS", None),
            finding("KeyD", None),
        ]);
        apply_group_completion(&mut s);
        // No divergence → no inference, no map entries, no notes.
        for key in ["KeyW", "KeyA", "KeyS", "KeyD"] {
            assert_eq!(s.confidence_for(key), Some(KeyConfidence::Exact));
        }
        assert!(s.key_confidence.is_empty());
        assert!(s.notes.is_empty());
    }

    #[test]
    fn diverging_non_cluster_key_triggers_nothing() {
        let mut s = wasd_probed();
        s.keys[0].effect = None; // KeyW inert now...
        s.keys[4].effect = effect(3); // ...Digit1 diverges instead.
        apply_group_completion(&mut s);
        for key in ["KeyW", "KeyA", "KeyS", "KeyD"] {
            assert_eq!(s.confidence_for(key), Some(KeyConfidence::Exact));
        }
        assert!(s.key_confidence.is_empty());
        assert!(s.notes.is_empty());
    }

    #[test]
    fn never_probed_siblings_appear_in_surface() {
        // A trimmed candidate list: only KeyW was probed.
        let mut s = surface_with(&[finding("KeyW", effect(1))]);
        apply_group_completion(&mut s);
        for key in ["KeyA", "KeyS", "KeyD"] {
            let f = s.keys.iter().find(|f| f.key == key);
            assert!(f.is_some(), "{key} must appear in the surface");
            let f = f.unwrap();
            assert_eq!(f.confidence, KeyConfidence::Inferred);
            assert!(f.effect.is_none());
        }
        assert_eq!(s.confidence_for("KeyS"), Some(KeyConfidence::Inferred));
    }

    #[test]
    fn unclassified_control_key_never_seeds_inference() {
        // The control key probed against itself can carry an effect
        // when the control runs are nondeterministic (gate failure
        // leaks into the self-probe diff). That effect is vacuous
        // evidence — the probe run IS the baseline run — and must
        // NOT seed group completion, or a `KeyW` control would
        // "infer" A/S/D from nothing.
        let mut s = surface_with(&[
            finding("KeyW", effect(5)), // spurious self-probe effect
            finding("KeyA", None),
            finding("KeyS", None),
            finding("KeyD", None),
        ]);
        s.keys[0].confidence = KeyConfidence::Unclassified;
        apply_group_completion(&mut s);
        assert!(s.key_confidence.is_empty(), "no sibling may be inferred");
        assert!(
            s.keys
                .iter()
                .all(|f| f.effect.is_none() || f.confidence == KeyConfidence::Unclassified),
            "no sibling gained an effect"
        );
        assert!(s.notes.is_empty(), "no completion note: {:?}", s.notes);
    }

    #[test]
    fn unclassified_control_key_can_still_be_inferred_from_others() {
        // Mirror case: excluding Unclassified findings from the
        // TRIGGER must not exclude them from the BENEFICIARY side.
        // Another cluster member genuinely diverges → normal
        // siblings still get inferred; the control key's raw
        // Unclassified record is left untouched (its own label is
        // already honest — vacuous evidence, no override needed).
        let mut s = surface_with(&[
            finding("KeyW", None), // control key, inert self-probe
            finding("KeyA", None),
            finding("KeyS", None),
        ]);
        s.keys[0].confidence = KeyConfidence::Unclassified;
        s.keys[1].effect = effect(3); // KeyA genuinely diverges
        apply_group_completion(&mut s);
        assert_eq!(s.confidence_for("KeyW"), Some(KeyConfidence::Unclassified));
        assert_eq!(s.confidence_for("KeyS"), Some(KeyConfidence::Inferred));
        assert_eq!(s.confidence_for("KeyA"), Some(KeyConfidence::Exact));
        assert!(s.keys.iter().any(|f| f.key == "KeyD"));
    }

    #[test]
    fn inferred_members_do_not_chain() {
        // A surface where KeyA is Inferred (as if seeded by a prior
        // pass) but NO member was observed to diverge: nothing may
        // trigger.
        let mut s = surface_with(&[finding("KeyA", None)]);
        s.keys[0].confidence = KeyConfidence::Inferred;
        apply_group_completion(&mut s);
        // No sibling was appended, no note was added.
        assert_eq!(s.keys.len(), 1);
        assert!(s.notes.is_empty());
        assert!(s.key_confidence.is_empty());
    }

    #[test]
    fn both_clusters_complete_independently() {
        let mut s = surface_with(&[
            finding("KeyW", effect(1)),
            finding("ArrowUp", effect(2)),
            finding("ArrowLeft", None),
        ]);
        apply_group_completion(&mut s);
        // WASD: A/S/D never probed → appended.
        assert!(s.keys.iter().any(|f| f.key == "KeyS"));
        // Arrows: Down/Right appended, Left upgraded via map.
        assert_eq!(s.confidence_for("ArrowLeft"), Some(KeyConfidence::Inferred));
        assert!(s.keys.iter().any(|f| f.key == "ArrowDown"));
        assert_eq!(s.notes.len(), 2, "one note per completed cluster");
    }

    #[test]
    fn application_is_idempotent() {
        let mut s = wasd_probed();
        apply_group_completion(&mut s);
        let once = s.clone();
        apply_group_completion(&mut s);
        assert_eq!(s, once, "second application changes nothing");
    }

    #[test]
    fn notes_carry_the_group_completion_marker() {
        let mut s = wasd_probed();
        apply_group_completion(&mut s);
        assert_eq!(s.notes.len(), 1);
        let note = &s.notes[0];
        assert!(note.contains("group completion"), "marker missing: {note}");
        assert!(note.contains("KeyA, KeyS, KeyD"));
        assert!(note.contains("KeyW"));
    }
}
