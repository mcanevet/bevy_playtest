//! Tier-0 discover sweep: the authoring-time DRAFT of a game's
//! input surface (D1 headline — roadmap §4 item 1).
//!
//! [`draft_surface`] is the one-call entry point the CLI and the
//! measurement harness compose. It wraps [`super::flow::discover`]
//! (it does NOT re-implement probe machinery: cache lookup → probe
//! pool → determinism gate → diff aggregation → group completion →
//! cache persist all happen underneath) and adds the D1-c layer:
//!
//! - **Entries, live AND inert.**
//!   [`DiscoveredSurface::to_entries`](super::surface::DiscoveredSurface::to_entries)
//!   projects effective findings as `ProbeVerified` entries; the
//!   draft adds the NEGATIVE COMPLEMENT — every probed-but-inert
//!   candidate also becomes an entry (`probe_run_inert` evidence,
//!   `effect_signature: None`), because a drafted surface that
//!   omits inert keys hides the residue problem and makes curation
//!   impossible. Curators must see what was ruled out, and why.
//! - **Findings, populated conservatively.** Every live entry is
//!   residue by definition for a sweep that declares nothing, so
//!   each gets a [`DiscoveryFindingKind::DiscoveredButUndeclared`]
//!   finding. [`DiscoveryFindingKind::PossiblyConsumedByUnknownPipeline`]
//!   is deliberately NOT emitted (it needs the receipt-vs-
//!   consumption tap, D2), and no `ContextUnverified` findings are
//!   emitted either: today's static scan notes are whole-surface
//!   caveats with no per-input evidence, and fabricating per-input
//!   context linkage is exactly the dishonesty this module refuses.
//!   TODO(D2): emit `ContextUnverified` once the scan can attribute
//!   inputs to run-conditioned systems.
//! - **Honest coverage.** [`SweepCoverage`] counts what was probed,
//!   what was verified, what was inert, and NAMES the limitation
//!   classes (chord, debounce, context) — refusal-over-silence: the
//!   draft says what the method cannot see instead of hiding it.
//! - **The grade backstop.** Every entry passes
//!   [`SurfaceEntry::grade_is_consistent`] before artifact write;
//!   an inconsistent entry is a bug — panic in debug (the
//!   test-only path), drop-with-note in release.
//!
//! The artifact is `surface.draft.json` — `draft: true` in the
//! header, a stable field order, and NO timestamps or run-dependent
//! data, so two sweeps of the same binary (isolated caches) produce
//! byte-identical files. The committed contract (`surface.json`,
//! human-curated) is a LATER lifecycle step; the draft never
//! masquerades as it.
//!
//! Context fingerprinting: the probe children export per-frame
//! state digests but no resource-snapshot channel, so all entries
//! carry [`ContextWitness::Boot`] — genuinely uncharacterized
//! beyond "ordinary startup state", and the schema says so.
//! TODO(D2 attribution): fingerprint boot context at probe time and
//! upgrade witnesses to `Fingerprinted`.

use super::flow::{discover, DiscoveryConfig, DiscoveryError};
use super::provenance::ProvenanceGrade;
use super::surface::{
    BuildProfile, ContextWitness, DiscoveryFinding, DiscoveryFindingKind, Evidence, KeyConfidence,
    KeyFinding, SurfaceConfidence, SurfaceEntry,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Artifact schema version for `surface.draft.json`.
pub const ARTIFACT_SCHEMA_VERSION: u32 = 1;

/// Named limitation class: chorded bindings. Members of a chord
/// (e.g. `L` while holding `Shift`) probe as inert under single-key
/// taps — each half alone may move nothing the digest sees.
pub const LIMITATION_CHORD: &str =
    "chorded bindings are not probed individually — a chord's member keys probe inert unless each half alone moves state";

/// Named limitation class: debounced or cooldown bindings. A 1-frame
/// tap inside a cooldown arms nothing the digest observes; the
/// reflected counter trips only on a later out-of-cooldown press.
pub const LIMITATION_DEBOUNCE: &str =
    "debounced/cooldown bindings may not trip at the probe cadence — a 1-frame tap inside a cooldown arms nothing the digest sees";

/// Named limitation class: context-gated bindings. Probes run at
/// boot; a binding live only in a menu, a focus, or a game state
/// the probe never enters reads as inert.
pub const LIMITATION_CONTEXT: &str =
    "context-gated bindings are only visible in their context — probes run at boot, so bindings gated on menus, focus, or game states read as inert";

/// The full missed-by-design list: every limitation class the naive
/// sweep acknowledges. These strings are constants on purpose —
/// the measurement harness asserts the known misses (chord/debounce
/// plants) are COVERED by them, which is the honest-coverage
/// commitment.
pub const MISSED_BY_DESIGN: &[&str] = &[LIMITATION_CHORD, LIMITATION_DEBOUNCE, LIMITATION_CONTEXT];

/// The honest-coverage report: what the sweep probed, what it
/// found, and what it structurally cannot see.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SweepCoverage {
    /// Candidate keys the matrix actually tapped (the control key
    /// and never-probed group-completion additions are excluded by
    /// construction — they carry no differential verdict).
    pub candidates_probed: usize,
    /// Candidates with an observed effect (live entries).
    pub verified: usize,
    /// Candidates probed with no observed effect (inert entries).
    pub inert: usize,
    /// The named limitation classes ([`MISSED_BY_DESIGN`]) — what
    /// the sweep cannot see, stated rather than hidden.
    pub missed_by_design: Vec<String>,
}

impl SweepCoverage {
    /// Verified share of probed candidates, as a percentage. `NaN`
    /// is impossible: a degenerate plan (zero candidates) is
    /// rejected upstream by `ProbePlan` validation.
    pub fn verified_pct(&self) -> f64 {
        pct(self.verified, self.candidates_probed)
    }

    /// Inert share of probed candidates, as a percentage.
    pub fn inert_pct(&self) -> f64 {
        pct(self.inert, self.candidates_probed)
    }
}

fn pct(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        return 0.0;
    }
    part as f64 * 100.0 / whole as f64
}

/// The inert-candidate predicate, single-sourced: a candidate with
/// a raw probe verdict of "ran, saw no divergence" (`Exact` with no
/// observed effect). The pre-frame-guard disclosure check, the
/// inert-entry emission filter, and the `coverage.inert` count MUST
/// all agree on who counts as inert — a drift here would let the
/// artifact say one thing in coverage and another in entries.
fn is_inert_candidate(k: &KeyFinding) -> bool {
    k.effect.is_none() && k.confidence == KeyConfidence::Exact
}

/// The drafted surface — what `surface.draft.json` serializes.
///
/// Header discipline: `draft` is always `true` and `schema_version`
/// is [`ARTIFACT_SCHEMA_VERSION`]; a consumer that mistakes the
/// draft for the committed contract has the fields to notice.
/// Promoted fields (`surface_confidence`, `from_cache`,
/// `build_profile`, `probed_at`, `readers`, `pruned_devices`,
/// `notes`) mirror [`super::surface::DiscoveredSurface`] so the
/// artifact is self-describing without embedding the legacy
/// `keys`/`mouse_buttons` lists (entries supersede them).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DraftOutcome {
    /// Always `true`: this is a machine-drafted surface awaiting
    /// curation, never the committed contract.
    pub draft: bool,
    /// Artifact schema version ([`ARTIFACT_SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// Live AND inert entries, sorted by input name (one entry per
    /// input at one context today). Deterministic: no timestamps,
    /// no wall-clock data — two sweeps of the same binary with
    /// isolated caches produce byte-identical artifacts.
    pub entries: Vec<SurfaceEntry>,
    /// Conservatively-populated findings (see the module docs for
    /// what is deliberately absent).
    pub findings: Vec<DiscoveryFinding>,
    /// The honest-coverage report.
    pub coverage: SweepCoverage,
    /// Whole-surface confidence from the double-control
    /// determinism gate (`Exact` = controls agreed; `Statistical`
    /// = trusted negatives are off the table).
    pub surface_confidence: SurfaceConfidence,
    /// Whether the underlying [`discover`] run answered from the
    /// surface cache (`true` = recalled, not re-probed). Recorded
    /// in the artifact so the draft never claims to have probed
    /// when it recalled.
    pub from_cache: bool,
    /// Build profile of the game binary the surface was learned
    /// against, when a probe child reported it.
    pub build_profile: Option<BuildProfile>,
    /// Probe points (frames) the sweep tapped.
    pub probed_at: Vec<u64>,
    /// Device class → system-reader counts (annotation-grade).
    pub readers: Vec<(String, usize)>,
    /// Devices skipped via static pruning (keyboard never pruned).
    pub pruned_devices: Vec<String>,
    /// Caveats qualifying the surface (scan notes, group-completion
    /// disclosures, backstop drops).
    pub notes: Vec<String>,
}

impl DraftOutcome {
    /// Assemble the draft from a discovered surface — the pure
    /// derivation [`draft_surface`] applies on top of [`discover`]
    /// (split so unit tests run without subprocesses).
    pub fn from_surface(surface: &super::surface::DiscoveredSurface, from_cache: bool) -> Self {
        let mut notes = surface.notes.clone();

        // Live entries: the D0 projection (grade ProbeVerified,
        // ProbeRun evidence, Boot context, lossiness note), stamped
        // with the game's build profile when a probe child
        // reported it.
        let mut entries: Vec<SurfaceEntry> = surface
            .to_entries()
            .into_iter()
            .map(|mut e| {
                e.profile = surface.build_profile;
                e
            })
            .collect();

        // The negative complement: every probed-but-inert
        // candidate ALSO becomes an entry. The control key
        // (`Unclassified` — no differential verdict exists about
        // it) and never-probed group-completion additions
        // (`Inferred` with no raw probe record) are excluded:
        // an inert entry must be backed by a probe that RAN — and a
        // probe that ran left a frame behind. A surface with no
        // recorded probe points cannot honestly back ANY inert
        // entry: `unwrap_or(0)` here would fabricate evidence for a
        // probe at frame 0 that the surface says never happened.
        // (Only hand-constructed or corrupted surfaces reach this
        // guard — `discover()` records a frame whenever a probe
        // runs.) Such candidates get no entry and a disclosure
        // note instead of invented evidence.
        let probe_frame = surface.probed_at.first().copied();
        if probe_frame.is_none()
            && surface
                .keys
                .iter()
                .chain(surface.mouse_buttons.iter())
                .any(is_inert_candidate)
        {
            notes.push(
                "surface records inert candidates but no probe frames \
                 — no inert entries were drafted (an inert entry must \
                 cite a probe that verifiably ran)"
                    .to_string(),
            );
        }
        for k in surface.keys.iter().chain(surface.mouse_buttons.iter()) {
            let Some(frame) = probe_frame else {
                // No probe point exists to cite: fabricating frame 0
                // would claim a probe ran that never did.
                break;
            };
            if !is_inert_candidate(k) {
                continue;
            }
            let mut entry_notes = vec![match surface.confidence {
                // The brief's committed wording — earned only when
                // the determinism gate passed.
                SurfaceConfidence::Exact => "probed inert under Exact controls".to_string(),
                // Fail-open honesty: under nondeterminism an inert
                // verdict is NOT a trusted negative.
                SurfaceConfidence::Statistical => {
                    "probed inert under Statistical (nondeterministic) \
                     controls — the negative is untrusted"
                        .to_string()
                }
            }];
            // A group-completed sibling: probed inert, but the
            // interpretation layer infers liveness from cluster
            // divergence. Both facts travel with the entry.
            if surface.confidence_for(&k.key) == Some(KeyConfidence::Inferred) {
                entry_notes.push(
                    "group completion infers this key live from a \
                     sibling's observed divergence despite the inert \
                     probe"
                        .to_string(),
                );
            }
            entries.push(SurfaceEntry {
                input: k.key.clone(),
                // The probe method vouched for the OBSERVATION;
                // `effect_signature: None` says nothing was verified
                // live. See the schema page's inert-entry paragraph.
                grade: ProvenanceGrade::ProbeVerified,
                action_label: None,
                context: ContextWitness::Boot,
                effect_signature: None,
                evidence: vec![Evidence::ProbeRunInert { frame }],
                mutable: false,
                profile: surface.build_profile,
                notes: entry_notes,
            });
        }

        // Canonical order: sorted by input so two sweeps of the
        // same plan produce the same artifact regardless of job
        // completion order.
        entries.sort_by(|a, b| a.input.cmp(&b.input));

        // Grade backstop: an inconsistent entry is a bug. Panic in
        // debug (tests catch it); drop with a note in release so a
        // shipping harness degrades loudly, never silently.
        let before = entries.len();
        entries.retain(|e| {
            if e.grade_is_consistent() {
                true
            } else {
                debug_assert!(
                    false,
                    "inconsistent draft entry for {}: grade {:?} lacks \
                     its backing evidence",
                    e.input, e.grade
                );
                notes.push(format!(
                    "dropped inconsistent entry for '{}' (grade {:?} \
                     lacked its backing evidence)",
                    e.input, e.grade
                ));
                false
            }
        });
        debug_assert_eq!(
            before,
            entries.len(),
            "draft_surface must never construct an inconsistent entry"
        );

        // Findings, conservatively: every LIVE entry is residue by
        // definition — the sweep declares nothing, so nothing it
        // finds was declared. Sorted by input for determinism.
        let mut findings: Vec<DiscoveryFinding> = entries
            .iter()
            .filter(|e| {
                matches!(e.grade, ProvenanceGrade::ProbeVerified) && e.effect_signature.is_some()
            })
            .map(|e| DiscoveryFinding {
                kind: DiscoveryFindingKind::DiscoveredButUndeclared,
                input: e.input.clone(),
                detail: "sweep-discovered; no declared source".to_string(),
            })
            .collect();
        findings.sort_by(|a, b| a.input.cmp(&b.input));

        // Coverage counts. `candidates_probed` counts keys with an
        // informative RAW probe record (the control key and
        // inferred additions excluded — the same discipline as
        // `probed_key_count`); `verified`/`inert` partition it.
        //
        // PARTITION INVARIANT, cross-referenced to the inert-emission
        // loop above: `candidates_probed` comes from
        // [`DiscoveredSurface::probed_key_count`], which counts
        // KEYBOARD keys only, while the inert-entry loop iterates
        // `keys` chained with `mouse_buttons`. The partition
        // `verified + inert == candidates_probed` therefore holds
        // only while MOUSE-BUTTON PROBING STAYS DORMANT (today the
        // matrix plans never produce inert mouse findings, so both
        // sides count keys only). The day mouse-button probing
        // activates, revisit BOTH: extend `probed_key_count` (or the
        // coverage denominators here) to include probed mouse
        // buttons, or the coverage will under-report candidates
        // while the entries include them — a silent partition break.
        let candidates_probed = surface.probed_key_count();
        let verified = surface.effective_key_count();
        let inert = surface
            .keys
            .iter()
            .chain(surface.mouse_buttons.iter())
            .filter(|k| is_inert_candidate(k))
            .count();

        Self {
            draft: true,
            schema_version: ARTIFACT_SCHEMA_VERSION,
            entries,
            findings,
            coverage: SweepCoverage {
                candidates_probed,
                verified,
                inert,
                missed_by_design: MISSED_BY_DESIGN.iter().map(|s| s.to_string()).collect(),
            },
            surface_confidence: surface.confidence,
            from_cache,
            build_profile: surface.build_profile,
            probed_at: surface.probed_at.clone(),
            readers: surface.readers.clone(),
            pruned_devices: surface.pruned_devices.clone(),
            notes,
        }
    }

    /// A one-line human summary, quotable: counts, percentages,
    /// limitations, findings. No run-dependent data.
    pub fn summary_lines(&self) -> Vec<String> {
        let mut lines = vec![format!(
            "probed {} candidate keys at frames [{}]: {} verified live ({:.1}%), {} inert ({:.1}%)",
            self.coverage.candidates_probed,
            self.probed_at
                .iter()
                .map(|f| f.to_string())
                .collect::<Vec<_>>()
                .join(", "),
            self.coverage.verified,
            self.coverage.verified_pct(),
            self.coverage.inert,
            self.coverage.inert_pct(),
        )];
        lines.push(format!(
            "surface confidence: {} (from cache: {}, build profile: {})",
            self.surface_confidence.as_str(),
            if self.from_cache { "yes" } else { "no" },
            match self.build_profile {
                Some(BuildProfile::Debug) => "debug",
                Some(BuildProfile::Release) => "release",
                None => "unknown",
            }
        ));
        lines.push(format!(
            "findings: {} discovered_but_undeclared (live inputs with no declared source)",
            self.findings.len()
        ));
        lines.push("missed by design:".to_string());
        for limitation in &self.coverage.missed_by_design {
            lines.push(format!("  - {limitation}"));
        }
        for note in &self.notes {
            lines.push(format!("note: {note}"));
        }
        lines
    }
}

/// Options for the authoring-time draft run. Thin by design: the
/// probe knobs live on [`DiscoveryConfig`] (the draft adds no
/// tuning of its own); the only draft-specific option is where the
/// artifact goes.
#[derive(Clone, Debug, Default)]
pub struct DraftOptions {
    /// Artifact destination; `None` = caller writes it themselves.
    out: Option<PathBuf>,
}

impl DraftOptions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Write the resulting draft to `path` after discovery.
    pub fn out(mut self, path: impl Into<PathBuf>) -> Self {
        self.out = Some(path.into());
        self
    }

    /// Run discovery and draft the surface. With [`Self::out`] set,
    /// the artifact is written before returning (a write failure is
    /// reported as a [`DiscoveryError::Setup`] — the run succeeded,
    /// the artifact did not reach disk, and silence would be the
    /// lie).
    pub fn run(&self, config: &DiscoveryConfig) -> Result<DraftOutcome, DiscoveryError> {
        let outcome = draft_surface(config)?;
        if let Some(path) = &self.out {
            write_artifact(&outcome, path).map_err(|e| {
                DiscoveryError::Setup(format!(
                    "cannot write draft artifact '{}': {e}",
                    path.display()
                ))
            })?;
        }
        Ok(outcome)
    }
}

/// The one-call sweep: run the D0 discovery (cache → probes →
/// gate → aggregation → group completion → cache persist, all
/// underneath) and derive the D1-c draft on top. The draft layer
/// adds NO cache of its own — it is transparent over [`discover`]'s.
pub fn draft_surface(config: &DiscoveryConfig) -> Result<DraftOutcome, DiscoveryError> {
    let outcome = discover(config)?;
    Ok(DraftOutcome::from_surface(
        &outcome.surface,
        outcome.from_cache,
    ))
}

/// Serialize the draft to `surface.draft.json`-style output: pretty
/// JSON with a stable field order and no timestamps (byte-identical
/// across two sweeps of the same binary with isolated caches).
pub fn write_artifact(outcome: &DraftOutcome, path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let json = serde_json::to_string_pretty(outcome)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::diff::Effect;
    use crate::discovery::surface::{
        DiscoveredSurface, KeyConfidence, KeyFinding, SurfaceConfidence,
    };

    fn finding(key: &str, effect: Option<Effect>, confidence: KeyConfidence) -> KeyFinding {
        KeyFinding {
            key: key.to_string(),
            effect,
            confidence,
        }
    }

    fn effect(delta: u32) -> Option<Effect> {
        Some(Effect {
            first_divergence_frame_delta: delta,
        })
    }

    fn surface(keys: Vec<KeyFinding>) -> DiscoveredSurface {
        DiscoveredSurface {
            keys,
            probed_at: vec![60, 90],
            confidence: SurfaceConfidence::Exact,
            ..DiscoveredSurface::default()
        }
    }

    #[test]
    fn inert_candidates_become_probe_verified_null_effect_entries() {
        let s = surface(vec![
            finding("KeyW", effect(2), KeyConfidence::Exact),
            finding("KeyX", None, KeyConfidence::Exact),
        ]);
        let draft = DraftOutcome::from_surface(&s, false);

        let live = draft.entries.iter().find(|e| e.input == "KeyW").unwrap();
        assert!(matches!(live.grade, ProvenanceGrade::ProbeVerified));
        assert!(live.effect_signature.is_some());
        assert_eq!(
            draft.entries.iter().find(|e| e.input == "KeyX").unwrap(),
            &SurfaceEntry {
                input: "KeyX".to_string(),
                grade: ProvenanceGrade::ProbeVerified,
                action_label: None,
                context: ContextWitness::Boot,
                effect_signature: None,
                evidence: vec![Evidence::ProbeRunInert { frame: 60 }],
                mutable: false,
                profile: None,
                notes: vec!["probed inert under Exact controls".to_string()],
            },
            "inert entry: probed, ruled out, never claiming liveness"
        );
        // Entries are sorted by input.
        let names: Vec<&str> = draft.entries.iter().map(|e| e.input.as_str()).collect();
        assert_eq!(names, ["KeyW", "KeyX"]);
    }

    #[test]
    fn control_key_and_never_probed_additions_get_no_inert_entry() {
        let s = surface(vec![
            // The control key self-probe: no differential verdict.
            finding("KeyQ", None, KeyConfidence::Unclassified),
            // Never probed, added by group completion alone: no
            // probe ran, so no inert observation exists to record.
            finding("ArrowLeft", None, KeyConfidence::Inferred),
        ]);
        let draft = DraftOutcome::from_surface(&s, false);
        assert!(
            draft.entries.is_empty(),
            "no probe observation, no entry — silence about keys \
             that were never differentially probed"
        );
        assert_eq!(draft.coverage.candidates_probed, 0);
    }

    #[test]
    fn group_completed_sibling_keeps_its_inert_record_plus_inference_note() {
        // A cluster member that WAS probed inert but whose verdict
        // the override map upgraded to Inferred: both facts travel.
        let mut s = surface(vec![
            finding("KeyW", effect(2), KeyConfidence::Exact),
            finding("KeyA", None, KeyConfidence::Exact),
        ]);
        s.key_confidence
            .push(("KeyA".to_string(), KeyConfidence::Inferred));
        let draft = DraftOutcome::from_surface(&s, false);
        let keya = draft.entries.iter().find(|e| e.input == "KeyA").unwrap();
        assert_eq!(
            keya.evidence,
            vec![Evidence::ProbeRunInert { frame: 60 }],
            "the raw probe record stays an inert observation"
        );
        assert!(
            keya.notes.iter().any(|n| n.contains("group completion")),
            "the inference disclosure travels with the entry"
        );
        // Inferred-live siblings are NOT live entries: the draft
        // does not upgrade group-completion guesses to ProbeVerified.
        assert!(keya.effect_signature.is_none());
    }

    #[test]
    fn every_live_entry_gets_one_discovered_but_undeclared_finding() {
        let s = surface(vec![
            finding("KeyW", effect(2), KeyConfidence::Exact),
            finding("Digit1", effect(1), KeyConfidence::Exact),
            finding("KeyX", None, KeyConfidence::Exact),
        ]);
        let draft = DraftOutcome::from_surface(&s, false);
        let inputs: Vec<&str> = draft.findings.iter().map(|f| f.input.as_str()).collect();
        assert_eq!(inputs, ["Digit1", "KeyW"], "sorted, live-only");
        for f in &draft.findings {
            assert!(matches!(
                f.kind,
                DiscoveryFindingKind::DiscoveredButUndeclared
            ));
            assert_eq!(f.detail, "sweep-discovered; no declared source");
        }
        // No PossiblyConsumedByUnknownPipeline, no ContextUnverified:
        // conservative population, constant vocabulary.
        assert_eq!(draft.findings.len(), 2);
    }

    #[test]
    fn coverage_counts_partition_the_probed_candidates() {
        let s = surface(vec![
            finding("KeyW", effect(2), KeyConfidence::Exact),
            finding("KeyA", effect(2), KeyConfidence::Exact),
            finding("KeyX", None, KeyConfidence::Exact),
            finding("KeyZ", None, KeyConfidence::Exact),
            finding("KeyQ", None, KeyConfidence::Unclassified),
        ]);
        let draft = DraftOutcome::from_surface(&s, false);
        assert_eq!(draft.coverage.candidates_probed, 4);
        assert_eq!(draft.coverage.verified, 2);
        assert_eq!(draft.coverage.inert, 2);
        assert!((draft.coverage.verified_pct() - 50.0).abs() < 1e-9);
        assert!((draft.coverage.inert_pct() - 50.0).abs() < 1e-9);
        // The named limitation classes are all present.
        for class in [LIMITATION_CHORD, LIMITATION_DEBOUNCE, LIMITATION_CONTEXT] {
            assert!(
                draft.coverage.missed_by_design.iter().any(|m| m == class),
                "limitation class must be acknowledged: {class}"
            );
        }
        // Greppable words for the measurement harness's
        // miss-acknowledgement asserts.
        let joined = draft.coverage.missed_by_design.join(" ");
        assert!(joined.contains("chord"));
        assert!(joined.contains("cooldown"));
        assert!(joined.contains("context"));
    }

    #[test]
    fn statistical_surfaces_never_claim_exact_controls() {
        let s = DiscoveredSurface {
            keys: vec![finding("KeyX", None, KeyConfidence::Exact)],
            probed_at: vec![60],
            confidence: SurfaceConfidence::Statistical,
            ..DiscoveredSurface::default()
        };
        let draft = DraftOutcome::from_surface(&s, false);
        let keyx = draft.entries.iter().find(|e| e.input == "KeyX").unwrap();
        assert!(
            keyx.notes[0].contains("Statistical"),
            "an inert verdict under nondeterminism is untrusted and says so"
        );
    }

    #[test]
    fn no_probe_frames_means_no_fabricated_inert_entries() {
        // A hand-constructed or corrupted surface claiming inert
        // candidates with NO recorded probe points must not yield
        // `probe_run_inert { frame: 0 }` evidence — that would cite
        // a probe that never ran. Disclosure beats invention.
        let mut s = surface(vec![finding("KeyX", None, KeyConfidence::Exact)]);
        s.probed_at = Vec::new();
        let draft = DraftOutcome::from_surface(&s, false);
        assert!(
            draft.entries.iter().all(|e| e.input != "KeyX"),
            "no probe frame on record ⇒ no inert entry for KeyX"
        );
        assert!(
            draft
                .notes
                .iter()
                .any(|n| n.contains("no inert entries were drafted")),
            "the omission is disclosed, not silent"
        );
        // Deliberate tension, pinned: coverage still counts the inert
        // CANDIDATES (the surface claims they were probed inert)
        // while entries correctly omit them (no citable frame). If
        // this drifts either way, decide anew — do not let it slide
        // unnoticed.
        assert_eq!(draft.coverage.inert, 1);
        assert_eq!(draft.coverage.candidates_probed, 1);
    }

    #[test]
    fn all_inert_is_an_answer_not_an_error() {
        let s = surface(vec![finding("KeyX", None, KeyConfidence::Exact)]);
        let draft = DraftOutcome::from_surface(&s, false);
        assert_eq!(draft.entries.len(), 1);
        assert!(draft.findings.is_empty());
        assert_eq!(draft.coverage.verified, 0);
        assert_eq!(draft.coverage.inert, 1);
        assert!(!draft.summary_lines().is_empty());
    }

    #[test]
    fn grade_backstop_passes_on_every_draft_entry() {
        // Belt-and-braces on top of the in-construction backstop:
        // every entry the derivation can produce is consistent.
        let s = surface(vec![
            finding("KeyW", effect(2), KeyConfidence::Exact),
            finding("KeyX", None, KeyConfidence::Exact),
            finding("KeyQ", None, KeyConfidence::Unclassified),
        ]);
        let draft = DraftOutcome::from_surface(&s, false);
        assert!(!draft.entries.is_empty());
        for e in &draft.entries {
            assert!(
                e.grade_is_consistent(),
                "entry for {} must pass the backstop",
                e.input
            );
        }
        // The backstop itself now distinguishes the two probe
        // observations: ran-and-diverged vs ran-and-didn't.
        let mut lying = SurfaceEntry {
            input: "KeyX".to_string(),
            grade: ProvenanceGrade::ProbeVerified,
            action_label: None,
            context: ContextWitness::Boot,
            effect_signature: None,
            evidence: vec![],
            mutable: false,
            profile: None,
            notes: vec![],
        };
        assert!(!lying.grade_is_consistent());
        lying.evidence = vec![Evidence::ProbeRunInert { frame: 60 }];
        assert!(lying.grade_is_consistent());
    }

    #[test]
    fn summary_lines_are_clean_and_quotable() {
        let s = surface(vec![
            finding("KeyW", effect(2), KeyConfidence::Exact),
            finding("KeyX", None, KeyConfidence::Exact),
        ]);
        let draft = DraftOutcome::from_surface(&s, false);
        let lines = draft.summary_lines();
        assert!(lines[0].starts_with("probed 2 candidate keys at frames [60, 90]"));
        assert!(lines[0].contains("1 verified live (50.0%), 1 inert (50.0%)"));
        assert!(lines[1].contains("surface confidence: exact"));
        assert!(lines[2].contains("1 discovered_but_undeclared"));
        assert!(lines[3] == "missed by design:");
    }
}
