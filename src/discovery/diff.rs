//! Hash-only digest-trace diff (discovery Phase 3).
//!
//! The discovery layer's signal is minimal on purpose: "did the probe
//! run's per-frame digest sequence stop matching the control run's,
//! and how many ticks after the tap?" The digest is an OPAQUE u64
//! equality token (the state-digest work folds whatever it folds in;
//! discovery never decomposes it). Comparing from the probe frame
//! onward also ignores any pre-tap incidental differences (there
//! should be none — probe and control are normalized pairs — but a
//! digest that legitimately varies before the probe point must not
//! mask an effect).
//!
//! SHORT TRACES, CALLER CONTRACT: `diff_trace` compares only the
//! common prefix of the two traces, so a `None` ("never diverged")
//! from asymmetric coverage speaks only for the frames BOTH runs
//! recorded. Consumers must therefore refuse incomplete traces
//! BEFORE trusting a negative: the probe pool's completeness gate
//! rejects reports that stop short of the run's expected final
//! frame, and `aggregate` downgrades any asymmetric-coverage inert
//! to `Unclassified` as a backstop. This function stays
//! prefix-bounded — it reports what was observed, never invents
//! divergence, and leaves trust policy to the callers.
//!
//! `changed_signals` is deliberately absent: extracting "which
//! signals changed" from an opaque hash would require a snapshot
//! vocabulary the merged state-digest API does not expose. If a
//! future snapshot mode records structured state, `Effect` can grow
//! an opt-in `changed_signals: Option<Vec<String>>` — default
//! `None`, never blocking this hash-only path.

use crate::digest::StateDigest;

/// The observed effect of a probe key at one probe point. Minimal by
/// design (MVP: hash-only divergence).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Effect {
    /// Ticks elapsed between the probe press and the first digest
    /// divergence (1 = same frame as the tap).
    pub first_divergence_frame_delta: u32,
}

/// Hash-only divergence detection (the MVP and the shipped mode).
///
/// `probe_digests` / `control_digests` are the opaque per-frame u64
/// series from the state-digest work; `probe_frame` is the frame the
/// probe (control) tapped. Returns the first frame relative to the
/// press where the probe sequence stops matching the control's, or
/// `None` if they never diverge (=> no observed effect at this point).
///
/// Traces are compared from `probe_frame` onward, aligned by absolute
/// frame. A trace that ends before the other simply stops
/// contributing frames — the shorter of the two bounds the scan, so
/// a `None` from asymmetric coverage is NOT a full-run inert; see
/// the module docs for the caller contract (completeness refusal
/// upstream, `Unclassified` downgrade in aggregation).
pub fn diff_trace(
    probe_digests: &[StateDigest],
    control_digests: &[StateDigest],
    probe_frame: u64,
) -> Option<Effect> {
    let probe_from = probe_digests.iter().filter(|d| d.frame >= probe_frame);
    let control_from = control_digests.iter().filter(|d| d.frame >= probe_frame);
    for ((pi, p), (ci, c)) in frame_aligned(probe_from).zip(frame_aligned(control_from)) {
        if pi != ci {
            // Frame misalignment: one trace skipped a frame the other
            // recorded — that IS a divergence of the runs.
            return Some(Effect {
                first_divergence_frame_delta: (pi.min(ci) + 1 - probe_frame).max(1) as u32,
            });
        }
        if p != c {
            return Some(Effect {
                first_divergence_frame_delta: (pi + 1 - probe_frame).max(1) as u32,
            });
        }
    }
    None
}

/// Pair each digest with its absolute frame so zip stays aligned even
/// if a trace skips frames.
fn frame_aligned<'a>(
    digests: impl Iterator<Item = &'a StateDigest> + 'a,
) -> impl Iterator<Item = (u64, u64)> + 'a {
    digests.map(|d| (d.frame, d.hash))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(frame: u64, hash: u64) -> StateDigest {
        StateDigest { frame, hash }
    }

    fn identical(n: u64) -> Vec<StateDigest> {
        (0..n).map(|i| digest(i, 1000 + i)).collect()
    }

    #[test]
    fn no_divergence_is_no_effect() {
        let trace = identical(10);
        assert_eq!(diff_trace(&trace, &trace, 3), None);
        // Same length, different values BEFORE the probe frame only:
        // comparison starts at the probe frame, so no effect.
        let mut early_noise = identical(10);
        early_noise[1].hash = 999999;
        assert_eq!(diff_trace(&early_noise, &trace, 3), None);
    }

    #[test]
    fn same_frame_divergence_has_delta_one() {
        let mut probe = identical(10);
        probe[5].hash += 1;
        let control = identical(10);
        let effect = diff_trace(&probe, &control, 5).expect("diverges at frame 5");
        assert_eq!(effect.first_divergence_frame_delta, 1);
    }

    #[test]
    fn later_divergence_counts_ticks_after_press() {
        let mut probe = identical(10);
        probe[7].hash += 1;
        let control = identical(10);
        // Press at 5, first mismatch at frame 7 -> delta = 7+1-5 = 3.
        let effect = diff_trace(&probe, &control, 5).expect("diverges at frame 7");
        assert_eq!(effect.first_divergence_frame_delta, 3);
    }

    #[test]
    fn probe_frame_zero_clamps_delta_to_one() {
        // press at frame 0, divergence at frame 0: 0+1-0 = 1.
        let mut probe = identical(5);
        probe[0].hash += 1;
        let control = identical(5);
        let effect = diff_trace(&probe, &control, 0).expect("diverges at frame 0");
        assert_eq!(effect.first_divergence_frame_delta, 1);
    }

    #[test]
    fn divergence_exactly_at_first_compared_frame_counts() {
        // The frame OF the probe press is compared too (effects can
        // land the same tick).
        let mut probe = identical(10);
        probe[3].hash ^= 1; // frame 3 == probe frame
        let control = identical(10);
        assert_eq!(
            diff_trace(&probe, &control, 3)
                .expect("divergence at the probe frame itself")
                .first_divergence_frame_delta,
            1
        );
    }

    #[test]
    fn shorter_probe_trace_bounds_the_scan() {
        // Probe run ends at frame 5 while the control runs to frame
        // 9: only the common prefix (through frame 5) is compared
        // and no divergence appears there, so the verdict is `None`.
        // NOTE: `None` here is an observation about the COMPARED
        // span only — callers must refuse or distrust such
        // asymmetric-coverage pairs before treating it as an inert
        // verdict (the pool's completeness gate and aggregate's
        // backstop do exactly that; pinned end-to-end by
        // `post_tap_but_incomplete_trace_is_refused_too` and
        // `short_probe_trace_never_becomes_a_trusted_negative`).
        let probe = identical(5);
        let control = identical(10);
        assert_eq!(diff_trace(&probe, &control, 3), None);
        // Control shorter than probe: same story.
        assert_eq!(diff_trace(&control, &probe, 3), None);
    }

    #[test]
    fn skipped_frame_in_one_trace_is_a_divergence() {
        // One trace missing a recorded frame is a real divergence of
        // the runs (frame misalignment).
        let mut probe = identical(10);
        probe.remove(6); // gap at frame 6
        let control = identical(10);
        // Frame 6 exists in control but not probe: zip pairs
        // probe's frame 7 against control's frame 6 -> misalignment
        // detected -> divergence with the EARLIER frame as anchor.
        let effect = diff_trace(&probe, &control, 0).expect("misaligned frames diverge");
        assert_eq!(effect.first_divergence_frame_delta, 7);
    }

    #[test]
    fn traces_shorter_than_probe_frame_yield_none() {
        // Nothing to compare at-or-after the press: no observed
        // effect (callers treat with the surrounding pool semantics).
        let probe = identical(3);
        let control = identical(3);
        assert_eq!(diff_trace(&probe, &control, 10), None);
    }

    #[test]
    fn effect_serializes_as_plain_json() {
        // The report layer will embed Effects; pin the JSON shape.
        let e = Effect {
            first_divergence_frame_delta: 3,
        };
        let json = serde_json::to_string(&e).unwrap();
        assert_eq!(json, r#"{"first_divergence_frame_delta":3}"#);
    }
}
