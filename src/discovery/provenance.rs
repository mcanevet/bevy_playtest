//! Provenance grades for the discovery IR ([`crate::discovery::surface::SurfaceEntry`]).
//!
//! A grade records HOW an entry earned its place in the surface —
//! which method vouched for it. It is deliberately orthogonal to
//! confidence: grades are the disagreement channel between methods
//! (a `Declared` entry the probes refute is a declared-but-dead
//! FINDING, not a silently-dropped entry; see
//! `crate::discovery::surface::DiscoveryFindingKind`).
//!
//! Vocabulary is fixed by the discovery roadmap (§6) and shared by
//! every producing method (declared readers, source scans, probe
//! sweeps, shape detection, intent taps). Producers arriving in later
//! slices must use these names, never invent synonyms — serialized
//! artifacts are the contract.
//!
//! Honest-claim discipline, grade by grade: each variant's docs say
//! what the grade ASSERTS and what it NEVER asserts. A grade that
//! gets quietly upgraded ("probe verified it, so it's live") is a
//! lying report.

use serde::{Deserialize, Serialize};

/// Canonical intent-tap stage names. The intent-tap producer (D1-d)
/// records WHERE along the input pipeline a tap was observed;
/// these constants are the closed vocabulary so serialized evidence
/// stays stable as more stages appear. More constants arrive with
/// their producers — [`Evidence::TapObserved`](crate::discovery::surface::Evidence::TapObserved)
/// carries the string verbatim.
pub const TAP_STAGE_MANAGER_PIPELINE: &str = "manager_pipeline";

/// How a [`SurfaceEntry`](crate::discovery::surface::SurfaceEntry)
/// earned its place in the discovered surface.
///
/// Wire format: snake_case, exactly the roadmap §6 names
/// (`declared`, `source_cited`, `probe_verified`, `suspect_shape`,
/// `llm_claimed`, `manager_pipeline_ok`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvenanceGrade {
    /// Read from a declared binding source (Leafwing `InputMap`,
    /// bespoke resource). ASSERTS: the game code declares this
    /// binding. NEVER asserts: that the binding is live — a
    /// declared entry the probes refute is the declared-but-dead
    /// alarm channel, and mutation-harness negative controls test
    /// exactly that alarm.
    ///
    /// The declared-tier reader does not exist yet; the grade exists
    /// so the schema is forward-complete.
    Declared,
    /// Static scan candidate with file:line provenance. ASSERTS: a
    /// source location references this input. NEVER asserts: that
    /// the referenced code runs — run conditions, focus gating and
    /// dead code all sit between a citation and a live binding.
    SourceCited,
    /// Naive-sweep differential probe observed an effect: the probe
    /// run's state digests diverged from the control run's after the
    /// tap. ASSERTS: pressing this input changed game state at the
    /// probed point, under the probe run's build profile and
    /// context. NEVER asserts WHAT changed (hash-only MVP) or that
    /// the effect persists in other contexts/builds.
    ProbeVerified,
    /// Shape-detected candidate NOT yet scan-linked (reflect-shape
    /// heuristic, D2+). Two-grade scheme: REPORTABLE, never
    /// alarm-grade — a suspect shape feeds the residue net and the
    /// to-be-scan-linked pipeline, but no finding (alarm or
    /// otherwise) may hang on it alone. Exists so the schema is
    /// forward-complete.
    SuspectShape,
    /// Prep-step LLM output. NEVER authoritative: an `llm_claimed`
    /// entry asserts nothing about the game; it records a claim by
    /// a model, grounded in [`Evidence::SourceRef`](crate::discovery::surface::Evidence::SourceRef)
    /// citations that mechanical checkers verify. No producer
    /// exists (LLM work is a deferred, opt-in prep step; CI stays
    /// hermetic); the grade exists so the schema is
    /// forward-complete.
    LlmClaimed,
    /// Intent-tap confirmed the manager pipeline fired (probe →
    /// `ActionState` action observed). ASSERTS: the driver's input
    /// reached the input manager and matched a binding — WIRING
    /// liveness, a localization grade. NEVER asserts game liveness:
    /// a wiring-forgotten binding fires the action with zero world
    /// effect. Effect-signature observation (attribution core, D2+)
    /// completes the triple-check; until then a tap-only verdict is
    /// reported with this grade, not promoted.
    ManagerPipelineOk,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grades_serialize_snake_case_as_the_roadmap_names_them() {
        // Wire vocabulary is contract: these exact strings are the
        // roadmap §6 names, and downstream tooling must never see
        // synonyms. Pin the bytes both ways for all six grades.
        let expected = [
            (ProvenanceGrade::Declared, "declared"),
            (ProvenanceGrade::SourceCited, "source_cited"),
            (ProvenanceGrade::ProbeVerified, "probe_verified"),
            (ProvenanceGrade::SuspectShape, "suspect_shape"),
            (ProvenanceGrade::LlmClaimed, "llm_claimed"),
            (ProvenanceGrade::ManagerPipelineOk, "manager_pipeline_ok"),
        ];
        for (grade, wire) in expected {
            let json = serde_json::to_string(&grade).unwrap();
            assert_eq!(json, format!("\"{wire}\""), "{json}");
            let back: ProvenanceGrade = serde_json::from_str(&json).unwrap();
            assert_eq!(back, grade);
        }
    }

    #[test]
    fn tap_stage_constant_is_the_wire_name() {
        // The stage constant and the grade share the "manager
        // pipeline" vocabulary; the constant is what an
        // Evidence::TapObserved { stage } will carry once the D1-d
        // producer exists.
        assert_eq!(TAP_STAGE_MANAGER_PIPELINE, "manager_pipeline");
    }
}
