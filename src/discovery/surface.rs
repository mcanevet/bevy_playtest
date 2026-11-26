//! Report-facing IR shared by the declared tier and the oracles:
//! `SurfaceEntry`, `ContextWitness`, `Evidence`, `DiscoveryFinding`
//! (used by the Leafwing reconcile, reported as `lw_reconciled`),
//! `BuildProfile` (stamped on every report), and `ViolationMeta`
//! (structured `_meta` evidence next to a violation's verbatim
//! detail, never inside it).

use serde::{Deserialize, Serialize};

use super::diff::Effect;
use super::provenance::ProvenanceGrade;

/// Build profile a surface was learned under. `cfg`-gated keys
/// (e.g. `#[cfg(debug_assertions)]` debug trackers) make dev-profile
/// surfaces LARGER than release ones — a surface recorded without
/// its profile invites misleading cross-profile comparisons, so
/// `surface.json` carries this alongside the entries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BuildProfile {
    Debug,
    Release,
}

impl BuildProfile {
    /// The profile this library was compiled with — the honest
    /// default a producer stamps when it cannot ask the game
    /// binary. A probe parent runs the GAME's binary, so the D1-c
    /// producer must read the profile from the run, not from this
    /// helper; this is the fallback for in-proces producers and
    /// tests.
    pub fn current() -> Self {
        if cfg!(debug_assertions) {
            BuildProfile::Debug
        } else {
            BuildProfile::Release
        }
    }
}

/// The context an entry was observed (or declared) in, as a
/// POSITIVE-ONLY witness predicate (R3-C ruling): it records when
/// an entry IS live, and NEVER encodes "dead when not" — a focus
/// witness tells you when focus was PRESENT, and presence does not
/// imply gameplay-death in its absence (radial menus, gameplay
/// without focusable entities, one global `InputFocus`).
///
/// "Boot context" is not a euphemism: it is [`ContextWitness::Fingerprinted`]
/// state, described at probe time, so drift alarms compare
/// fingerprints rather than vibes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContextWitness {
    /// Entry observed at boot context, WITHOUT a recorded
    /// fingerprint (what the Leafwing reconcile emits). The context is genuinely
    /// uncharacterized beyond "the game's ordinary startup state";
    /// `describe` says so rather than guessing.
    Boot,
    /// Live when a focusable entity of this kind is focused
    /// (bevy_input_focus witness channel). POSITIVE ONLY: focused
    /// means live-when-focused; it never means dead-when-not.
    Focused {
        /// Focusable widget kind, when the witness can name one
        /// (e.g. a component name). `None` when the scan saw an
        /// `InputFocus` carrier without finer classification.
        widget_kind: Option<String>,
    },
    /// Entry observed under a described state: (resource name →
    /// rendered value) pairs captured AT PROBE TIME —
    /// state-resource values, focused-widget presence, frame count.
    /// Serialized as a plain JSON map (a `BTreeMap`, so iteration
    /// order is sorted and stable). This is the "boot context is a
    /// described state" requirement: an alarm about context drift
    /// compares these fingerprints, not an unnamed vibe.
    Fingerprinted {
        /// Resource name → rendered value, sorted by name.
        snapshot: std::collections::BTreeMap<String, String>,
    },
    /// Source-scan-linked run-condition description (D2+
    /// producer; forward-complete now): the scan connected this
    /// entry to a run condition it can describe but not evaluate.
    Scanned {
        /// Human-oriented description of the run condition, as
        /// extracted from source (verbatim fragment, not our
        /// paraphrase).
        condition_hint: String,
    },
}

impl ContextWitness {
    /// Human-facing rendering for reports (refusal-over-silence:
    /// reports say WHICH context an entry was seen in, or say
    /// "uncharacterized" — never blank).
    pub fn describe(&self) -> String {
        match self {
            ContextWitness::Boot => "boot context (unfingerprinted)".to_string(),
            ContextWitness::Focused { widget_kind } => match widget_kind {
                Some(kind) => format!("focused: {kind}"),
                None => "focused (widget kind unclassified)".to_string(),
            },
            ContextWitness::Fingerprinted { snapshot } => {
                let pairs: Vec<String> = snapshot
                    .iter()
                    .map(|(name, value)| format!("{name}={value}"))
                    .collect();
                format!("fingerprinted context ({})", pairs.join(", "))
            }
            ContextWitness::Scanned { condition_hint } => {
                format!("scanned run condition ({condition_hint})")
            }
        }
    }
}

/// Structured, extensible evidence: how an entry earned its
/// [`ProvenanceGrade`]. An entry's evidence list is the grade's
/// backing; [`SurfaceEntry::grade_is_consistent`] checks the pairing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Evidence {
    /// Differential probe run diverged from its control baseline
    /// at `frame` (absolute frame index).
    ProbeRun { frame: u64, effect: Effect },
    /// Intent-tap observed at `stage` (stage names come from the
    /// `crate::discovery::provenance` constants, starting with
    /// [`TAP_STAGE_MANAGER_PIPELINE`](crate::discovery::provenance::TAP_STAGE_MANAGER_PIPELINE);
    /// more arrive with their producers).
    TapObserved { stage: String },
    /// file:line provenance: the mechanically-checkable citation
    /// format. `enclosing` names the enclosing item (function,
    /// system, impl block) when the extractor knows it.
    SourceRef {
        file: String,
        line: u32,
        enclosing: Option<String>,
    },
    /// Declared in a binding source of this kind (e.g.
    /// `"leafwing_input_map"`, `"bespoke_resource:<Name>"`).
    DeclaredIn { source_kind: String },
    /// Differential probe ran to completion at `frame` and observed
    /// NO divergence from the control baseline: the input was probed
    /// under the run's controls and found inert. This is evidence
    /// FOR a negative — it earns an entry's place the same way
    /// [`Evidence::ProbeRun`] earns a live one, letting inert keys
    /// appear as entries (grade [`ProvenanceGrade::ProbeVerified`],
    /// `effect_signature: None`) so curators see what was ruled out
    /// and why, instead of the draft silently omitting the residue.
    /// Introduced by the D1-c sweep; additive to the D1-a shape, no
    /// existing variant changed.
    ProbeRunInert { frame: u64 },
}

/// The canonical IR unit (roadmap §6): one input, the context it
/// was observed in, the declared intent it maps to (if any), the
/// observed effect signature, the evidence that earned its grade,
/// and the qualifiers that alarm semantics depend on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceEntry {
    /// Normalized input name — the scenario schema's key spelling
    /// (e.g. `KeyJ`).
    pub input: String,
    /// Grade this entry earned. Backing evidence lives in
    /// `evidence`; see [`Self::grade_is_consistent`].
    pub grade: ProvenanceGrade,
    /// Declared intent name when known (Leafwing action name,
    /// bespoke field name); `None` for raw discoveries.
    pub action_label: Option<String>,
    /// Context the entry was observed or declared in
    /// (positive-only witness predicate).
    pub context: ContextWitness,
    /// Observed effect signature, reusing the probe tier's
    /// [`Effect`] verbatim — there is exactly one effect type.
    pub effect_signature: Option<Effect>,
    /// The evidence that earned `grade`.
    pub evidence: Vec<Evidence>,
    /// `true` when the declared source is runtime-mutable (rebind
    /// menus): a mutable binding's probe-vs-snapshot divergence is
    /// PROVENANCE, not necessarily drift. Producers cannot detect
    /// this yet; the field exists because alarm semantics depend
    /// on it. Default `false`.
    pub mutable: bool,
    /// Build profile that produced this entry (`cfg`-gated keys
    /// make dev surfaces larger than release ones).
    pub profile: Option<BuildProfile>,
    /// Scan-caveat style qualifiers.
    pub notes: Vec<String>,
}

impl SurfaceEntry {
    /// Is `grade` backed by the evidence it claims? A grade whose
    /// evidence is missing is a lying report — e.g. `ProbeVerified`
    /// with no [`Evidence::ProbeRun`]/[`Evidence::ProbeRunInert`]
    /// observation, or `Declared` with no [`Evidence::DeclaredIn`]
    /// source. Pure function, no I/O; used by tests now and
    /// report-rendering warnings later.
    ///
    /// Deliberately a MINIMUM-consistency check, not a full proof:
    /// `true` means the grade's REQUIRED evidence shape is present,
    /// never that the evidence is truthful (that is the mutation
    /// harness's job). `SuspectShape` has no mechanical evidence
    /// shape at all — the two-grade scheme makes it reportable
    /// without ever making it alarm-grade, so it is consistent
    /// with an empty evidence list.
    pub fn grade_is_consistent(&self) -> bool {
        match self.grade {
            ProvenanceGrade::Declared => self
                .evidence
                .iter()
                .any(|e| matches!(e, Evidence::DeclaredIn { .. })),
            ProvenanceGrade::SourceCited => self
                .evidence
                .iter()
                .any(|e| matches!(e, Evidence::SourceRef { .. })),
            ProvenanceGrade::ProbeVerified => self.evidence.iter().any(|e| {
                matches!(
                    e,
                    Evidence::ProbeRun { .. } | Evidence::ProbeRunInert { .. }
                )
            }),
            ProvenanceGrade::SuspectShape => true,
            ProvenanceGrade::LlmClaimed => self
                .evidence
                .iter()
                .any(|e| matches!(e, Evidence::SourceRef { .. })),
            ProvenanceGrade::ManagerPipelineOk => self
                .evidence
                .iter()
                .any(|e| matches!(e, Evidence::TapObserved { .. })),
        }
    }
}

/// Kind of discovery finding — the disagreement channel between
/// methods. Findings live in a separate list (e.g.
/// `ReconciledLw::findings`); they annotate, they never mutate the
/// entries they discuss.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryFindingKind {
    /// ALARM GRADE: the input is declared (Leafwing map, bespoke
    /// resource) but no live evidence exists at the characterized
    /// context. The one alarm-grade finding; the mutation harness
    /// negative-controls it (flip a plant binding to dead code,
    /// assert this fires).
    DeclaredButDead,
    /// RESIDUE: probe/source evidence exists but no declared
    /// source names the input — the game consumes an input nobody
    /// declared (raw `ButtonState` reads, ad-hoc chords).
    DiscoveredButUndeclared,
    /// The probe saw NOTHING and upstream consumption is
    /// UNEXPLAINED (gui-focus class): possibly consumed by an
    /// unknown pipeline — distinct from inert, because the
    /// upstream `FocusedInput`-bubbling question may eventually
    /// get an official consumption answer. Track it; good news
    /// either way.
    PossiblyConsumedByUnknownPipeline,
    /// Declared and tap-ok but never verified OUTSIDE the boot
    /// context: the honest coverage grade for context-gated
    /// actions — a context-gated action cannot false-alarm the
    /// frozen-world oracle, and the report must say which claims
    /// carry this reduced coverage rather than silently calling
    /// them `verified`.
    ContextUnverified,
}

/// One discovery finding: a method disagreement worth reporting.
/// `{ kind, input, detail }`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveryFinding {
    /// Which disagreement this is.
    pub kind: DiscoveryFindingKind,
    /// The input the finding is about (same normalized name space
    /// as [`SurfaceEntry::input`]).
    pub input: String,
    /// Human-oriented one-liner (scan-caveat tone; the structured
    /// facts live in the kind + the entry it discusses).
    pub detail: String,
}

/// Machine-readable annotation block attached to a violation
/// (serialized as `_meta`), next to the verbatim human `detail`.
/// `PartialEq` only — the `pairwise_separation` episode block
/// carries `f32` distances, which have no total `Eq`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ViolationMeta {
    /// `frozen_world` only: whether the broadened state digest kept
    /// changing through the silent streak (`Some(true)` — the sim was
    /// demonstrably stepping but nothing visual changed; `Some(false)`
    /// — the digest was flat too, corroborating a true freeze;
    /// `None` — no digest was recorded, so stepping is UNKNOWN and
    /// the detail keeps the legacy "soft-lock?" wording).
    #[serde(default)]
    pub stepping: Option<bool>,
    /// `pairwise_separation` only: the episode's evidence — the
    /// distinct violating pairs (first + worst + frame count per
    /// pair, capped, truncation disclosed via `roster_truncated`)
    /// plus how many pairs the frame enumerator sampled vs. saw.
    /// Populated only by episode findings; the detail text names
    /// the worst pair, the roster holds the full story so a
    /// handoff A–B → A–C cannot hide the persistent offender A.
    #[serde(default)]
    pub pairwise_episode: Option<PairwiseEpisodeMeta>,
    /// `derived_transform_consistency` only: the episode's
    /// evidence — the distinct diverging entities (first/worst
    /// frames and a frame count per entity, capped, truncation
    /// disclosed via `roster_truncated`). Same doctrine as the
    /// pairwise block: the detail text names the worst entity,
    /// the roster holds the full story.
    #[serde(default)]
    pub consistency_episode: Option<ConsistencyEpisodeMeta>,
    /// `forbidden_region` only: the incursion episode's evidence —
    /// the distinct incursive entities (first/worst frames and a
    /// frame count per entity, capped, truncation disclosed via
    /// `roster_truncated`). Same doctrine as the pairwise block:
    /// the detail text names the deepest incursion, the roster
    /// holds the full story.
    #[serde(default)]
    pub forbidden_episode: Option<ForbiddenEpisodeMeta>,
    /// `entity_stagnation` only: the episode's evidence — the
    /// distinct stuck entities (first/worst frames, per-entity
    /// accrued stuck-span, capped, truncation disclosed via
    /// `roster_truncated`). Roster entries carry BOTH the entity
    /// index and its GENERATION (slot recycling must never weld
    /// a successor to its predecessor's stint).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stagnation_episode: Option<StagnationEpisodeMeta>,
    /// `occupancy_cap` only: the crowd episode's evidence — the
    /// distinct WITNESSING entities (those whose self-inclusive
    /// neighborhood count exceeded cap; first/worst frames and a
    /// frame count per entity, index + generation, capped,
    /// truncation disclosed via `roster_truncated`), plus the
    /// episode's radius and cap echoed for self-describing
    /// evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occupancy_episode: Option<OccupancyEpisodeMeta>,
    /// Unpromoted-rule posture mark (generic by design — future
    /// §2 postures reuse it): `Some(true)` marks a finding whose
    /// rule carries no fail-power claim on record (see the
    /// proposal §8 acceptance table). `occupancy_cap` sets it on
    /// every finding; all other rules leave it absent. The mark
    /// is the honesty device: the finding renders as an ordinary
    /// `violations[]` entry on a status:"Pass" report, and the
    /// juxtaposition plus this flag tells the reader the rule is
    /// warn-only pending promotion evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unpromoted_rule: Option<bool>,
}

/// The `pairwise_separation` episode evidence block (`_meta`).
/// Roster entries are `(low, high)` entity indices, ordered worst
/// pair first (minimum distance, then earliest frame). `PartialEq`
/// only — the roster carries `f32` distances, which have no total
/// `Eq`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PairwiseEpisodeMeta {
    /// How many DISTINCT violating pairs the episode saw in total —
    /// `roster.len()` plus the pairs the cap dropped. Disclosed so
    /// `_meta` never lies by omission when a crowd exceeds the cap
    /// (`roster_truncated`). (Enumeration itself is uncapped in
    /// this slice: the author bounds O(n²) by choosing the selector;
    /// a sampling stride stays TBD by the cost model per the design
    /// doc.)
    pub sampled_pairs: u64,
    /// Distinct violating pairs with first/worst/count evidence,
    /// worst-first. Capped at the roster cap; `roster_truncated`
    /// discloses when the episode had more.
    pub roster: Vec<PairwiseRosterEntry>,
    /// True when the episode saw more distinct pairs than the
    /// roster cap admits — disclosed truncation, never silent
    /// subsampling.
    pub roster_truncated: bool,
}

/// One roster entry: the evidence for one distinct violating pair
/// inside an episode. Serialized field names are wire-stable
/// (`first_frame`, `worst_frame`, `worst_distance`,
/// `violating_frames`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PairwiseRosterEntry {
    /// The ordered pair: lower entity index first.
    pub low: u32,
    pub high: u32,
    /// First violating frame this pair was seen in the episode.
    pub first_frame: u64,
    /// The minimum distance observed for this pair.
    pub worst_distance: f32,
    /// The frame the worst (minimum) distance was seen at.
    pub worst_frame: u64,
    /// How many frames this pair violated within the episode.
    pub violating_frames: u64,
}

/// The `derived_transform_consistency` episode evidence block
/// (`_meta`). Roster entries are single entities, ordered worst
/// first (maximum deviation, then earliest frame). `PartialEq` only
/// — the block carries `f32` deviations, which have no total `Eq`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ConsistencyEpisodeMeta {
    /// How many DISTINCT diverging entities the episode saw in
    /// total — `roster.len()` plus the entities the cap dropped.
    /// Disclosed so `_meta` never lies by omission when a crowd
    /// exceeds the cap (`roster_truncated`).
    pub sampled_entities: u64,
    /// Distinct diverging entities with first/worst/count
    /// evidence, worst-first. Capped at the roster cap;
    /// `roster_truncated` discloses when the episode had more.
    pub roster: Vec<ConsistencyRosterEntry>,
    /// True when the episode saw more distinct entities than the
    /// roster cap admits — disclosed truncation, never silent
    /// subsampling.
    pub roster_truncated: bool,
}

/// One roster entry: the evidence for one distinct diverging
/// entity inside an episode. Serialized field names are
/// wire-stable (`first_frame`, `worst_deviation`, `worst_frame`,
/// `violating_frames`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConsistencyRosterEntry {
    /// The diverging entity's index.
    pub entity: u32,
    /// First violating frame this entity was seen in the episode.
    pub first_frame: u64,
    /// The maximum deviation observed for this entity.
    pub worst_deviation: f32,
    /// The frame the worst (maximum) deviation was seen at.
    pub worst_frame: u64,
    /// How many frames this entity diverged within the episode.
    pub violating_frames: u64,
}

/// The `forbidden_region` episode evidence block (`_meta`). Roster
/// entries are single entities, ordered worst first (maximum
/// penetration depth, then earliest frame). `PartialEq` only — the
/// block carries `f32` depths, which have no total `Eq`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ForbiddenEpisodeMeta {
    /// How many DISTINCT incursive entities the episode saw in
    /// total — `roster.len()` plus the entities the cap dropped.
    /// Disclosed so `_meta` never lies by omission when a crowd
    /// exceeds the cap (`roster_truncated`).
    pub sampled_entities: u64,
    /// Distinct incursive entities with first/worst/count
    /// evidence, worst-first. Capped at the roster cap;
    /// `roster_truncated` discloses when the episode had more.
    pub roster: Vec<ForbiddenRosterEntry>,
    /// True when the episode saw more distinct entities than the
    /// roster cap admits — disclosed truncation, never silent
    /// subsampling.
    pub roster_truncated: bool,
}

/// The `entity_stagnation` episode evidence block (`_meta`).
/// Roster entries are per-entity (index + generation), ordered
/// worst first (maximum accrued stuck-span in driver units, then
/// earliest frame). `PartialEq` only — the roster carries `f32`
/// spans, which have no total `Eq`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StagnationEpisodeMeta {
    /// How many DISTINCT stuck entities the episode saw in total —
    /// `roster.len()` plus the entities the cap dropped. Disclosed
    /// so `_meta` never lies by omission when a crowd exceeds the
    /// cap (`roster_truncated`).
    pub sampled_entities: u64,
    /// Distinct stuck entities with first/worst/span evidence,
    /// worst-first (longest stuck). Capped at the roster cap;
    /// `roster_truncated` discloses when the episode had more.
    pub roster: Vec<StagnationRosterEntry>,
    /// True when the episode saw more distinct entities than the
    /// roster cap admits — disclosed truncation, never silent
    /// subsampling.
    pub roster_truncated: bool,
}

/// One roster entry: the evidence for one distinct incursive
/// entity inside an episode. Serialized field names are
/// wire-stable (`entity`, `first_frame`, `worst_frame`,
/// `worst_depth`, `violating_frames`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ForbiddenRosterEntry {
    /// The incursive entity's index.
    pub entity: u32,
    /// First violating frame this entity was seen in the episode.
    pub first_frame: u64,
    /// The maximum penetration depth observed for this entity
    /// (distance to the nearest region face; deeper is worse).
    pub worst_depth: f32,
    /// The frame the worst (maximum) depth was seen at.
    pub worst_frame: u64,
    /// How many frames this entity stayed incursive within the
    /// episode.
    pub violating_frames: u64,
}

/// One roster entry: the evidence for one distinct stuck entity
/// inside an episode. `stuck_span` is the entity's accrued
/// stuck-span in DRIVER-LEAF UNITS (its personal soak measure,
/// frozen when its stint ended by change or despawn). Serialized
/// field names are wire-stable.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StagnationRosterEntry {
    /// The stuck entity's index.
    pub entity: u32,
    /// The entity's generation — slot recycling must never weld
    /// a successor to its predecessor's stint.
    pub generation: u32,
    /// First sampled frame this entity was stuck in the episode.
    pub first_frame: u64,
    /// Last sampled frame this entity remained stuck in the
    /// episode (its stint's end: change, despawn, or run end).
    pub last_frame: u64,
    /// The entity's accrued stuck-span in driver-leaf units.
    pub stuck_span: f32,
    /// How many sampled frames this entity was stuck.
    pub stuck_frames: u64,
}

/// The `occupancy_cap` episode evidence block (`_meta`). Roster
/// entries are the WITNESSING entities only (those whose
/// self-inclusive neighborhood count exceeded cap), per-entity
/// with index + generation, ordered worst first (maximum
/// witnessed count, then earliest frame). `PartialEq` only —
/// the block carries `f32` radii, which have no total `Eq`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OccupancyEpisodeMeta {
    /// How many DISTINCT witnessing entities the episode saw in
    /// total — `roster.len()` plus the witnesses the cap
    /// dropped. Disclosed so `_meta` never lies by omission when
    /// a crowd exceeds the roster cap (`roster_truncated`).
    pub sampled_entities: u64,
    /// The crowd radius the episode was judged within (echoed so
    /// the evidence is self-describing without the scenario at
    /// hand).
    pub radius: f32,
    /// The cap the witnessed counts were compared against
    /// (self-inclusive counting).
    pub cap: u32,
    /// Distinct witnessing entities with first/worst/count
    /// evidence, worst-first (highest witnessed crowd count).
    /// Capped at the roster cap; `roster_truncated` discloses
    /// when the episode had more.
    pub roster: Vec<OccupancyRosterEntry>,
    /// True when the episode saw more distinct witnesses than the
    /// roster cap admits — disclosed truncation, never silent
    /// subsampling.
    pub roster_truncated: bool,
}

/// One roster entry: the evidence for one distinct WITNESSING
/// entity inside an episode — an entity whose self-inclusive
/// neighborhood count exceeded cap. Serialized field names are
/// wire-stable (`entity`, `generation`, `first_frame`,
/// `worst_frame`, `worst_count`, `violating_frames`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OccupancyRosterEntry {
    /// The witnessing entity's index.
    pub entity: u32,
    /// The entity's generation — slot recycling must never weld
    /// a successor to its predecessor's stint.
    pub generation: u32,
    /// First violating frame this entity witnessed a crowd.
    pub first_frame: u64,
    /// The maximum self-inclusive neighborhood count observed
    /// for this entity within the episode (the worst crowd it
    /// witnessed; larger is worse).
    pub worst_count: u32,
    /// The frame the worst (maximum) count was seen at.
    pub worst_frame: u64,
    /// How many frames this entity witnessed an over-cap crowd
    /// within the episode.
    pub violating_frames: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn violation_meta_serializes_as_underscore_meta() {
        // The design doc pins the JSON key as `_meta`; the field name
        // in Rust is `meta`. Verify the wire format.
        let entry = crate::state::ViolationEntry {
            rule: "frozen_world".to_string(),
            target: "world".to_string(),
            first_frame: 12,
            last_frame: 180,
            count: 1,
            detail: "no Transform change for 180 frames — soft-lock?".to_string(),
            last_detail: "no Transform change".to_string(),
            meta: Some(ViolationMeta {
                stepping: Some(true),
                pairwise_episode: None,
                consistency_episode: None,
                forbidden_episode: None,
                stagnation_episode: None,
                occupancy_episode: None,
                unpromoted_rule: None,
            }),
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"_meta\""), "wire key must be _meta: {json}");
        let back: crate::state::ViolationEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back, entry);
    }

    #[test]
    fn violation_without_meta_stays_verbatim_and_back_compatible() {
        // Old reports carry no `meta`: deserializing them must keep working
        // (serde `default`), and serialization must not add keys.
        let old_json = r#"{
            "rule": "frozen_world",
            "target": "world",
            "first_frame": 12,
            "last_frame": 180,
            "count": 1,
            "detail": "no Transform change for 180 frames",
            "last_detail": "no Transform change"
        }"#;
        let entry: crate::state::ViolationEntry = serde_json::from_str(old_json).unwrap();
        assert!(entry.meta.is_none());
        let reserialized = serde_json::to_string(&entry).unwrap();
        assert!(!reserialized.contains("_meta"));
    }

    // ------------------------------------------------------------------
    // IR layer (SurfaceEntry / ContextWitness / Evidence / grades /
    // findings). Types + serialization only: no producer exists yet.
    // ------------------------------------------------------------------

    fn effect(delta: u32) -> Effect {
        Effect {
            first_divergence_frame_delta: delta,
        }
    }

    fn probe_run_evidence() -> Evidence {
        Evidence::ProbeRun {
            frame: 60,
            effect: effect(2),
        }
    }

    fn probe_run_inert_evidence() -> Evidence {
        Evidence::ProbeRunInert { frame: 60 }
    }

    fn entry(grade: ProvenanceGrade, evidence: Vec<Evidence>) -> SurfaceEntry {
        SurfaceEntry {
            input: "KeyW".to_string(),
            grade,
            action_label: None,
            context: ContextWitness::Boot,
            effect_signature: Some(effect(2)),
            evidence,
            mutable: false,
            profile: Some(BuildProfile::Debug),
            notes: vec![],
        }
    }

    #[test]
    fn surface_entry_round_trips_through_json() {
        let e = SurfaceEntry {
            input: "KeyW".to_string(),
            grade: ProvenanceGrade::ProbeVerified,
            action_label: Some("MoveUp".to_string()),
            context: ContextWitness::Fingerprinted {
                snapshot: std::collections::BTreeMap::from([
                    ("frame_count".to_string(), "60".to_string()),
                    ("settings.paused".to_string(), "false".to_string()),
                ]),
            },
            effect_signature: Some(effect(2)),
            evidence: vec![
                probe_run_evidence(),
                Evidence::SourceRef {
                    file: "src/player.rs".to_string(),
                    line: 42,
                    enclosing: Some("fn movement_system".to_string()),
                },
            ],
            mutable: false,
            profile: Some(BuildProfile::Debug),
            notes: vec!["probed at one point only".to_string()],
        };
        let json = serde_json::to_string(&e).unwrap();
        let back: SurfaceEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back, e);
    }

    #[test]
    fn context_witness_round_trips_every_variant() {
        let witnesses = vec![
            ContextWitness::Boot,
            ContextWitness::Focused {
                widget_kind: Some("Button".to_string()),
            },
            ContextWitness::Focused { widget_kind: None },
            ContextWitness::Fingerprinted {
                snapshot: std::collections::BTreeMap::from([
                    ("paused".to_string(), "false".to_string()),
                    ("frame_count".to_string(), "60".to_string()),
                ]),
            },
            ContextWitness::Scanned {
                condition_hint: "if ui_state.mode == Menu".to_string(),
            },
        ];
        for w in witnesses {
            let json = serde_json::to_string(&w).unwrap();
            let back: ContextWitness = serde_json::from_str(&json).unwrap();
            assert_eq!(back, w);
        }
        // WIRE PIN (brief: "serde as a plain map"): the fingerprint
        // is `"snapshot":{"k":"v"}` — never an array of pairs.
        // Round-trip alone cannot see the difference; the bytes are
        // the contract for downstream tooling.
        let w = ContextWitness::Fingerprinted {
            snapshot: std::collections::BTreeMap::from([(
                "paused".to_string(),
                "false".to_string(),
            )]),
        };
        let json = serde_json::to_string(&w).unwrap();
        assert!(
            json.contains("\"snapshot\":{\"paused\":\"false\"}"),
            "fingerprint must be a plain map on the wire: {json}"
        );
        assert!(
            json.contains("\"kind\":\"fingerprinted\""),
            "tag must be kind/fingerprinted: {json}"
        );
    }

    #[test]
    fn evidence_round_trips_every_variant() {
        let evidences = vec![
            probe_run_evidence(),
            Evidence::TapObserved {
                stage: crate::discovery::provenance::TAP_STAGE_MANAGER_PIPELINE.to_string(),
            },
            Evidence::SourceRef {
                file: "src/input.rs".to_string(),
                line: 7,
                enclosing: None,
            },
            Evidence::DeclaredIn {
                source_kind: "leafwing_input_map".to_string(),
            },
            probe_run_inert_evidence(),
        ];
        for e in evidences {
            let json = serde_json::to_string(&e).unwrap();
            let back: Evidence = serde_json::from_str(&json).unwrap();
            assert_eq!(back, e);
        }
        // WIRE PIN (D1-c additive variant): the inert-probe
        // evidence serializes as probe_run_inert with its frame —
        // the bytes are the contract for downstream tooling.
        let json = serde_json::to_string(&probe_run_inert_evidence()).unwrap();
        assert_eq!(
            json, "{\"kind\":\"probe_run_inert\",\"frame\":60}",
            "inert-evidence wire name is probe_run_inert: {json}"
        );
    }

    #[test]
    fn build_profile_records_current_and_round_trips() {
        // `current()` is cfg-derived: whatever this test binary runs
        // as, the value must round-trip and both wire names are
        // pinned.
        assert_eq!(
            serde_json::to_string(&BuildProfile::Debug).unwrap(),
            "\"debug\""
        );
        assert_eq!(
            serde_json::to_string(&BuildProfile::Release).unwrap(),
            "\"release\""
        );
        let cur = BuildProfile::current();
        let back: BuildProfile =
            serde_json::from_str(&serde_json::to_string(&cur).unwrap()).unwrap();
        assert_eq!(back, cur);
    }

    #[test]
    fn grade_is_consistent_requires_matching_evidence_shape() {
        // FALSE table: each grade with evidence of the WRONG shape
        // (or none at all) is inconsistent — the grade is lying
        // about how it was earned.
        let wrong_shape = vec![
            (ProvenanceGrade::Declared, Vec::new()),
            (
                ProvenanceGrade::Declared,
                // A source citation is not a declaration.
                vec![Evidence::SourceRef {
                    file: "src/input.rs".to_string(),
                    line: 1,
                    enclosing: None,
                }],
            ),
            (ProvenanceGrade::SourceCited, Vec::new()),
            (
                ProvenanceGrade::SourceCited,
                // A probe run is not a citation.
                vec![probe_run_evidence()],
            ),
            (ProvenanceGrade::ProbeVerified, Vec::new()),
            (
                ProvenanceGrade::ProbeVerified,
                // A declaration is not a probe observation.
                vec![Evidence::DeclaredIn {
                    source_kind: "leafwing_input_map".to_string(),
                }],
            ),
            (
                ProvenanceGrade::ManagerPipelineOk,
                // An inert probe run is a probe observation, not an
                // intent-tap.
                vec![probe_run_inert_evidence()],
            ),
            (ProvenanceGrade::ManagerPipelineOk, Vec::new()),
            (
                ProvenanceGrade::ManagerPipelineOk,
                // A probe divergence is not an intent-tap.
                vec![probe_run_evidence()],
            ),
            (ProvenanceGrade::LlmClaimed, Vec::new()),
        ];
        for (grade, evidence) in wrong_shape {
            let e = entry(grade, evidence);
            assert!(
                !e.grade_is_consistent(),
                "{grade:?} with mismatched evidence must be inconsistent"
            );
        }
        // TRUE table: each grade WITH its required evidence shape.
        let right_shape = vec![
            (
                ProvenanceGrade::Declared,
                vec![Evidence::DeclaredIn {
                    source_kind: "leafwing_input_map".to_string(),
                }],
            ),
            (
                ProvenanceGrade::SourceCited,
                vec![Evidence::SourceRef {
                    file: "src/input.rs".to_string(),
                    line: 42,
                    enclosing: Some("fn setup".to_string()),
                }],
            ),
            (ProvenanceGrade::ProbeVerified, vec![probe_run_evidence()]),
            // The D1-c inert representation: ProbeVerified with an
            // inert probe run (ran to completion, never diverged) is
            // a CONSISTENT grade — the backstop now distinguishes
            // ran-and-diverged (ProbeRun) from ran-and-didn't
            // (ProbeRunInert), so inert entries pass it without
            // ever upgrading their claim.
            (
                ProvenanceGrade::ProbeVerified,
                vec![probe_run_inert_evidence()],
            ),
            // SuspectShape has NO mechanical evidence shape: the
            // two-grade scheme makes it reportable without any
            // backing (and never alarm-grade) — even an EMPTY
            // evidence list is consistent.
            (ProvenanceGrade::SuspectShape, Vec::new()),
            // LLM claims are citation-grounded: the grade exists for
            // schema completeness, and a claim without a mechanical
            // citation to check is precisely the inconsistency
            // `grade_is_consistent` guards.
            (
                ProvenanceGrade::LlmClaimed,
                vec![Evidence::SourceRef {
                    file: "src/input.rs".to_string(),
                    line: 42,
                    enclosing: None,
                }],
            ),
            (
                ProvenanceGrade::ManagerPipelineOk,
                vec![Evidence::TapObserved {
                    stage: crate::discovery::provenance::TAP_STAGE_MANAGER_PIPELINE.to_string(),
                }],
            ),
        ];
        for (grade, evidence) in right_shape {
            let e = entry(grade, evidence);
            assert!(
                e.grade_is_consistent(),
                "{grade:?} with its required evidence must be consistent"
            );
        }
        // Extra evidence never breaks consistency (a probe-verified
        // entry that ALSO carries a citation is richer, not lying).
        let e = entry(
            ProvenanceGrade::ProbeVerified,
            vec![
                probe_run_evidence(),
                Evidence::SourceRef {
                    file: "src/input.rs".to_string(),
                    line: 1,
                    enclosing: None,
                },
            ],
        );
        assert!(e.grade_is_consistent());
    }

    #[test]
    fn context_witness_describe_never_empty() {
        let witnesses = vec![
            ContextWitness::Boot,
            ContextWitness::Focused {
                widget_kind: Some("RadialMenu".to_string()),
            },
            ContextWitness::Focused { widget_kind: None },
            ContextWitness::Fingerprinted {
                snapshot: std::collections::BTreeMap::new(),
            },
            ContextWitness::Fingerprinted {
                snapshot: std::collections::BTreeMap::from([("a".to_string(), "1".to_string())]),
            },
            ContextWitness::Scanned {
                condition_hint: String::new(),
            },
        ];
        for w in witnesses {
            let d = w.describe();
            assert!(
                !d.trim().is_empty(),
                "refusal-over-silence: describe({w:?}) must never be blank"
            );
        }
    }

    #[test]
    fn discovery_finding_round_trips_and_kinds_are_snake_case() {
        let finding = DiscoveryFinding {
            kind: DiscoveryFindingKind::DeclaredButDead,
            input: "KeyF".to_string(),
            detail: "declared in InputMap but no probe or tap evidence at boot".to_string(),
        };
        let json = serde_json::to_string(&finding).unwrap();
        assert!(json.contains("\"declared_but_dead\""), "{json}");
        let back: DiscoveryFinding = serde_json::from_str(&json).unwrap();
        assert_eq!(back, finding);
        // All four kinds pin their wire names (the disagreement
        // channel's vocabulary is contract).
        for (kind, wire) in [
            (DiscoveryFindingKind::DeclaredButDead, "declared_but_dead"),
            (
                DiscoveryFindingKind::DiscoveredButUndeclared,
                "discovered_but_undeclared",
            ),
            (
                DiscoveryFindingKind::PossiblyConsumedByUnknownPipeline,
                "possibly_consumed_by_unknown_pipeline",
            ),
            (
                DiscoveryFindingKind::ContextUnverified,
                "context_unverified",
            ),
        ] {
            let kj = serde_json::to_string(&kind).unwrap();
            assert_eq!(kj, format!("\"{wire}\""));
            let back: DiscoveryFindingKind = serde_json::from_str(&kj).unwrap();
            assert_eq!(back, kind);
        }
    }
}
