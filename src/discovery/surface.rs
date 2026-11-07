//! Discovered input surface (Phase 5): the report-facing shape of
//! discovery results.
//!
//! `DiscoveredSurface` is what the probe/diff/cache layers produce
//! and what `PlaytestReport` carries when discovery ran. Consumers
//! (JUnit renderers, dashboards) read it as structured data; it is
//! deliberately SEPARATE from `ViolationEntry.detail` so the
//! verbatim-detail contract of violations is untouched. The one
//! overlap is the frozen-world annotation: a `frozen_world` finding
//! annotated with discovery context gets a `ViolationMeta` block
//! (serialized as `_meta`) — extra structured data NEXT TO the
//! verbatim detail, never inside it.
//!
//! The IR layer (`SurfaceEntry`, `ContextWitness`, `Evidence`,
//! `BuildProfile`, `DiscoveryFinding`) extends the legacy
//! `keys`/`mouse_buttons` lists — nothing existing is removed or
//! renamed; every existing consumer, cache entry and test keeps
//! working. The new fields are additive, default-empty and
//! skip-serialized when empty, so old JSON never grows new keys and
//! new JSON stays readable by old consumers. `to_entries` projects
//! the legacy shape into entries for D1-c consumers; the projection
//! is lossy by design (entries are richer; a reverse mapping would
//! only ever be approximate).
//!
//! Field-reality adjustments vs the design doc, made visible here:
//!
//! - `readers` maps a device class to a reader COUNT (not system
//!   names): system names are masked without `bevy_ecs/debug`, and
//!   the crate does not depend on that feature. Counts remain
//!   annotation-grade information.
//! - `probed_keys_effective` / `probed_keys_total` in the
//!   frozen-world annotation derive from `keys`, but "probed" means
//!   an INFORMATIVE raw probe record: never-probed group-completion
//!   siblings ([`KeyConfidence::Inferred`]) and the control key
//!   ([`KeyConfidence::Unclassified`]) stay visible in `keys` yet
//!   are excluded from the count.

use serde::{Deserialize, Serialize};

use super::diff::Effect;
use super::provenance::ProvenanceGrade;

/// How a key came to be in the surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyConfidence {
    /// Observed divergence in this key's own probe run(s).
    Exact,
    /// Inferred: group-completion sibling of a key that DID diverge
    /// (e.g., KeyW diverges; KeyA/KeyS/KeyD marked as
    /// likely-movement). Heuristic — inferred only, never `Exact`,
    /// or the confidence field lies. (Phase 6 populates this.)
    Inferred,
    /// Unclassifiable: this key IS the probe matrix's control key, so
    /// its probe run is identical to its own control baseline BY
    /// CONSTRUCTION — no differential verdict about it is possible,
    /// whatever the game binds. Never a trusted negative, and never
    /// counted as an informative probe (the flow layer assigns
    /// this when the control key appears among the candidates; by
    /// default it is excluded from the candidate set instead).
    Unclassified,
}

/// Whole-surface confidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SurfaceConfidence {
    /// Controls agreed (determinism gate passed): every `None`
    /// effect is a trusted negative.
    #[default]
    Exact,
    /// The two control runs diverged from each other: effects are
    /// statistical. The report must say so, not silently trust
    /// negatives (refusal-over-silence applies to confidence too).
    Statistical,
}

impl SurfaceConfidence {
    pub fn as_str(self) -> &'static str {
        match self {
            SurfaceConfidence::Exact => "exact",
            SurfaceConfidence::Statistical => "statistical",
        }
    }
}

/// One effectiveness datum: key → effect, plus the length
/// of the probe that established it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyFinding {
    /// Candidate key name (normalized control-pair string).
    pub key: String,
    /// Observed effect (first-divergence delta), or `None` if the
    /// probe run matched the control trace.
    pub effect: Option<Effect>,
    /// How this entry got into the surface.
    pub confidence: KeyConfidence,
}

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
    /// fingerprint (the legacy shape — [`DiscoveredSurface::to_entries`]
    /// emits this for D0 findings). The context is genuinely
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
    /// at `frame` (frames are absolute, matching
    /// [`DiscoveredSurface::probed_at`]).
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
///
/// Entries are additive to [`DiscoveredSurface`]'s legacy
/// `keys`/`mouse_buttons` lists (see [`DiscoveredSurface::entries`]);
/// D0 surfaces stay valid, and [`DiscoveredSurface::to_entries`]
/// projects them mechanically so D1-c consumers need not maintain
/// both formats by hand.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceEntry {
    /// Normalized input name — SAME string space as
    /// [`KeyFinding::key`] (normalized control-pair keys), so sweep
    /// output and entries interoperate without remapping.
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
    /// Scan-caveat style qualifiers, same tone as
    /// [`DiscoveredSurface::notes`].
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
/// methods. Findings live in a separate list on [`DiscoveredSurface`]
/// (`findings`); they annotate the surface, they never mutate the
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

/// The discovered input surface of one game (per
/// `docs/explanation/input-surface-discovery.md`, Report
/// Integration section).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredSurface {
    /// Keyboard keys probed (all candidates; `effect: None` = probed
    /// but inert at the probed points). Group-completed siblings
    /// (Phase 6) appear here with `KeyConfidence::Inferred`.
    pub keys: Vec<KeyFinding>,
    /// Mouse buttons probed, same shape as `keys`.
    pub mouse_buttons: Vec<KeyFinding>,
    /// Motion-probe effect, if mouse motion was probed.
    pub mouse_motion: Option<Effect>,
    /// Wheel-probe effect, if the wheel was probed.
    pub wheel: Option<Effect>,
    /// Gamepad buttons probed, same shape as `keys`.
    pub gamepad_buttons: Vec<KeyFinding>,
    /// Gamepad axes probed (axis name → effect).
    pub gamepad_axes: Vec<(String, Option<Effect>)>,
    /// Probe points (frames) the surface was learned at.
    pub probed_at: Vec<u64>,
    /// Device class → number of game systems whose access set
    /// includes a read of that class's signals (annotation-grade;
    /// system names are not extractable without bevy_ecs/debug).
    pub readers: Vec<(String, usize)>,
    /// Per-key confidence override map, keyed by key name. Sparse:
    /// keys absent from this map use their `KeyFinding.confidence`.
    /// Populated by group completion (Phase 6).
    pub key_confidence: Vec<(String, KeyConfidence)>,
    /// Whole-surface confidence.
    pub confidence: SurfaceConfidence,
    /// Devices skipped via static pruning (e.g. `["gamepad",
    /// "wheel"]`). The keyboard class is never in this list
    /// (fail-open rule).
    pub pruned_devices: Vec<String>,
    /// Caveats surfaced by the static scan (exclusive systems,
    /// uninitialized schedules, ...) that qualify the surface.
    pub notes: Vec<String>,
    /// Build profile of the GAME binary the surface was learned
    /// against (promoted from the probe child's report by
    /// [`crate::discovery::flow::discover`]). `cfg`-gated keys make
    /// dev-profile surfaces strictly larger than release ones, so a
    /// surface without its profile invites misleading cross-profile
    /// comparisons. `None` for D0-era cached surfaces and direct
    /// `aggregate` callers. Additive, skip-serialized when empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_profile: Option<BuildProfile>,
    /// Canonical IR entries (D1+): richer than `keys`/`mouse_buttons`
    /// — one entry per (input × context) with grade, evidence,
    /// action label and qualifiers. ADDITIVE and default-empty:
    /// D0 surfaces (and the legacy renderers that read them) are
    /// unaffected; D1-c producers fill this so later tooling can
    /// read one format instead of both.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<SurfaceEntry>,
    /// Method disagreements ([`DiscoveryFinding`]): declared-but-dead
    /// alarms, discovered-but-undeclared residue, unexplained
    /// upstream consumption, unverified-context coverage. ADDITIVE
    /// and default-empty (old JSON never grows the key; old
    /// consumers never see it).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<DiscoveryFinding>,
}

impl DiscoveredSurface {
    /// The number of keyboard keys that caused divergence (used by
    /// the frozen-world annotation as `probed_keys_effective`).
    /// EXCLUDES the control key ([`KeyConfidence::Unclassified`]):
    /// under nondeterminism its probe can diverge from its own
    /// baseline, and that artifact must not count as an effective
    /// key (the same exclusion `probed_key_count` applies).
    pub fn effective_key_count(&self) -> usize {
        self.keys
            .iter()
            .filter(|k| k.effect.is_some() && k.confidence != KeyConfidence::Unclassified)
            .count()
    }

    /// Number of keyboard keys with an informative raw probe record
    /// (the denominator of the frozen-world annotation). EXCLUDES
    /// group-completion siblings that were never probed
    /// ([`KeyConfidence::Inferred`] entries added by Phase 6) and the
    /// control key ([`KeyConfidence::Unclassified`], which carries no
    /// differential verdict): counting those would overstate how many
    /// keys the matrix actually tapped.
    pub fn probed_key_count(&self) -> usize {
        self.keys
            .iter()
            .filter(|k| k.confidence == KeyConfidence::Exact)
            .count()
    }

    /// Look up the effective confidence for a key, falling back to
    /// the `KeyFinding`'s own value when the sparse override map has
    /// no entry.
    pub fn confidence_for(&self, key: &str) -> Option<KeyConfidence> {
        if let Some((_, conf)) = self.key_confidence.iter().find(|(k, _)| k == key) {
            return Some(*conf);
        }
        self.keys
            .iter()
            .find(|k| k.key == key)
            .map(|k| k.confidence)
    }

    /// Keys worth pressing for discovery-driven scenario injection
    /// (`BEVY_PLAYTEST_DISCOVER_INJECT=1`): every key with an
    /// observed effect PLUS group-completed siblings
    /// ([`KeyConfidence::Inferred`] — heuristically likely-bound, and
    /// pressing them is cheap; leaving them out would starve the
    /// bot of exactly the movement-cluster coverage the group phase
    /// exists to provide). NEVER the control key
    /// ([`KeyConfidence::Unclassified`] — no differential verdict
    /// exists about it) and never inert keys (pressing keys the game
    /// demonstrably ignores recreates the declared-surface hole
    /// injection exists to close). Sorted and deduplicated: this
    /// list becomes `bot.raw_surface.keys` verbatim, and a canonical
    /// stable order keeps reports and cache entries comparable.
    pub fn chaos_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self
            .keys
            .iter()
            .filter(|k| {
                k.confidence != KeyConfidence::Unclassified
                    && (k.effect.is_some() || k.confidence == KeyConfidence::Inferred)
            })
            .map(|k| k.key.clone())
            .collect();
        keys.sort();
        keys.dedup();
        keys
    }

    /// Convenience accessor for the frozen-world annotation.
    pub fn frozen_world_meta(&self) -> ViolationMeta {
        ViolationMeta {
            probed_keys_effective: Some(self.effective_key_count()),
            probed_keys_total: Some(self.probed_key_count()),
            pruned_devices: if self.pruned_devices.is_empty() {
                None
            } else {
                Some(self.pruned_devices.clone())
            },
            note: Some(format!(
                "probed {} keys, {} effective (confidence: {})",
                self.probed_key_count(),
                self.effective_key_count(),
                self.confidence.as_str()
            )),
        }
    }

    /// Mechanical projection of today's `keys`/`mouse_buttons`
    /// findings into IR entries, for D1-c consumers that read
    /// entries (avoiding dual-format maintenance). Lossy BY DESIGN:
    /// entries are richer than the legacy shape, and a reverse
    /// mapping would only ever be approximate.
    ///
    /// - Only EFFECTIVE findings project (observed effect, excluding
    ///   the control key — the same exclusion discipline as
    ///   [`Self::effective_key_count`] and [`Self::chaos_keys`]);
    ///   inert and inferred-no-effect keys stay legacy-only. The
    ///   entry list is the "what is live" view; the negatives are
    ///   the `keys` list's job.
    /// - Grade is `ProbeVerified` for every projected entry (each
    ///   carries a `ProbeRun` evidence, so `grade_is_consistent`
    ///   holds); context is the unfingerprinted `Boot` witness (the
    ///   D0 flow never recorded fingerprints).
    /// - The probe FRAME is not retained per-key by
    ///   [`KeyFinding`]; the first entry of `probed_at` stands in
    ///   for all — a documented lossiness, and each projected
    ///   entry's `notes` say so, so the caveat travels with the
    ///   artifact instead of living only in this doc.
    pub fn to_entries(&self) -> Vec<SurfaceEntry> {
        let probe_frame = self.probed_at.first().copied().unwrap_or(0);
        self.keys
            .iter()
            .chain(self.mouse_buttons.iter())
            .filter(|k| k.effect.is_some() && k.confidence != KeyConfidence::Unclassified)
            .map(|k| {
                let effect = k.effect.expect("filtered: effect.is_some()");
                SurfaceEntry {
                    input: k.key.clone(),
                    grade: ProvenanceGrade::ProbeVerified,
                    action_label: None,
                    context: ContextWitness::Boot,
                    effect_signature: Some(effect),
                    evidence: vec![Evidence::ProbeRun {
                        frame: probe_frame,
                        effect,
                    }],
                    mutable: false,
                    profile: None,
                    notes: vec![format!(
                        "projected from a D0 surface: per-key probe \
                         frames are not retained; the evidence frame \
                         ({probe_frame}) is the surface's first probe \
                         point, not this input's"
                    )],
                }
            })
            .collect()
    }
}

/// Machine-readable annotation block attached to a violation
/// (serialized as `_meta`). Carries discovery context next to the
/// verbatim human `detail`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViolationMeta {
    /// Keys that caused divergence, out of all probed.
    pub probed_keys_effective: Option<usize>,
    /// Total candidates probed.
    pub probed_keys_total: Option<usize>,
    /// Devices skipped via static pruning.
    pub pruned_devices: Option<Vec<String>>,
    /// One-line context for humans skimming JSON.
    pub note: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(key: &str, effect: Option<Effect>, confidence: KeyConfidence) -> KeyFinding {
        KeyFinding {
            key: key.to_string(),
            effect,
            confidence,
        }
    }

    fn sample_surface() -> DiscoveredSurface {
        DiscoveredSurface {
            keys: vec![
                finding(
                    "KeyW",
                    Some(Effect {
                        first_divergence_frame_delta: 2,
                    }),
                    KeyConfidence::Exact,
                ),
                finding("KeyA", None, KeyConfidence::Exact),
                // Never-probed sibling (group completion): visible in
                // `keys` but NOT counted as probed.
                finding("KeyS", None, KeyConfidence::Inferred),
                // The control key itself: no differential verdict.
                finding("KeyQ", None, KeyConfidence::Unclassified),
            ],
            probed_at: vec![60, 90],
            readers: vec![("keyboard".to_string(), 3)],
            confidence: SurfaceConfidence::Exact,
            ..Default::default()
        }
    }

    #[test]
    fn effective_and_probed_counts_split_inert_from_live() {
        let s = sample_surface();
        assert_eq!(s.effective_key_count(), 1);
        // 4 entries in `keys`, but only 2 carry informative raw
        // probe records (KeyW probed-effective, KeyA probed-inert);
        // the Inferred sibling and the Unclassified control key are
        // visible in the surface but never overstated as probed.
        assert_eq!(s.probed_key_count(), 2);
    }

    #[test]
    fn chaos_keys_pick_effective_and_inferred_drop_the_rest() {
        // The injection derivation in miniature: what the battery
        // parent puts into the injected chaos bot's raw_surface.
        let s = sample_surface();
        // KeyW: observed effect (Exact). KeyS: Inferred sibling —
        // included; discovery believes group completion, and the
        // bot pressing it costs nothing. KeyA: demonstrably inert
        // (probed, no effect) — excluded, pressing it recreates the
        // declared-surface hole injection exists to close. KeyQ:
        // the control key (Unclassified) — never.
        assert_eq!(s.chaos_keys(), vec!["KeyS".to_string(), "KeyW".to_string()]);
        // Order stability matters: the list becomes a scenario file
        // verbatim, and dedup must survive duplicated findings.
        let mut duped = sample_surface();
        duped
            .keys
            .push(finding("KeyW", None, KeyConfidence::Inferred));
        assert_eq!(
            duped.chaos_keys(),
            vec!["KeyS".to_string(), "KeyW".to_string()],
            "duplicates collapse and the sort order is canonical"
        );
        // An Unclassified key is excluded even if it spuriously
        // carries an effect (nondeterministic control-run artifact)
        // — mirroring the effective-count exclusion.
        let s = DiscoveredSurface {
            keys: vec![finding(
                "Space",
                Some(Effect {
                    first_divergence_frame_delta: 1,
                }),
                KeyConfidence::Unclassified,
            )],
            ..sample_surface()
        };
        assert!(
            !s.chaos_keys().contains(&"Space".to_string()),
            "the control key never enters the injected surface"
        );
        // Empty surface (a game that consumes no keys): injection
        // yields an empty key list — keyboard-opt-out, mouse chaos
        // unaffected (the battery keeps default mouse buttons).
        let empty = DiscoveredSurface {
            keys: vec![],
            ..sample_surface()
        };
        assert_eq!(empty.chaos_keys(), Vec::<String>::new());
    }

    #[test]
    fn spurious_control_key_effect_does_not_count_as_effective() {
        // Under nondeterminism the control key's probe CAN diverge
        // from its own baseline — carrying `effect: Some(..)` — but
        // it is `Unclassified` by design and must stay OUT of the
        // effective count just as it is out of the probed count.
        let s = DiscoveredSurface {
            keys: vec![
                finding(
                    "KeyW",
                    Some(Effect {
                        first_divergence_frame_delta: 3,
                    }),
                    KeyConfidence::Exact,
                ),
                finding(
                    "KeyQ",
                    Some(Effect {
                        first_divergence_frame_delta: 1,
                    }),
                    KeyConfidence::Unclassified,
                ),
            ],
            probed_at: vec![60, 90],
            ..sample_surface()
        };
        assert_eq!(s.effective_key_count(), 1, "only KeyW is effective");
        assert_eq!(s.probed_key_count(), 1);
        // And the frozen-world annotation inherits the exclusion.
        let meta = s.frozen_world_meta();
        assert_eq!(meta.probed_keys_effective, Some(1));
    }

    #[test]
    fn frozen_world_meta_reports_probe_stats() {
        let s = sample_surface();
        let meta = s.frozen_world_meta();
        assert_eq!(meta.probed_keys_effective, Some(1));
        assert_eq!(meta.probed_keys_total, Some(2));
        assert_eq!(meta.pruned_devices, None);
        let note = meta.note.unwrap();
        assert!(note.contains("probed 2 keys, 1 effective"));
        assert!(note.contains("exact"));
    }

    #[test]
    fn frozen_world_meta_lists_pruned_devices_when_present() {
        let mut s = sample_surface();
        s.pruned_devices = vec!["gamepad".to_string(), "wheel".to_string()];
        assert_eq!(
            s.frozen_world_meta().pruned_devices,
            Some(vec!["gamepad".to_string(), "wheel".to_string()])
        );
    }

    #[test]
    fn confidence_lookup_prefers_override_map() {
        let mut s = sample_surface();
        // KeyA has no override → its own finding confidence.
        assert_eq!(s.confidence_for("KeyA"), Some(KeyConfidence::Exact));
        // KeyS has no override → Inferred from the finding.
        assert_eq!(s.confidence_for("KeyS"), Some(KeyConfidence::Inferred));
        // Override flips the answer for KeyS.
        s.key_confidence = vec![("KeyS".to_string(), KeyConfidence::Exact)];
        assert_eq!(s.confidence_for("KeyS"), Some(KeyConfidence::Exact));
        // Unknown key → None.
        assert_eq!(s.confidence_for("KeyX"), None);
    }

    #[test]
    fn surface_confidence_serializes_lowercase_key_confidence_title_case() {
        // The documented wire contract (design doc + how-to): the
        // WHOLE-SURFACE confidence is lowercase; the per-key
        // confidence is title-cased. Pin the wire bytes.
        let s = DiscoveredSurface {
            keys: vec![
                finding("Digit1", None, KeyConfidence::Exact),
                finding("KeyW", None, KeyConfidence::Inferred),
                finding("KeyQ", None, KeyConfidence::Unclassified),
            ],
            confidence: SurfaceConfidence::Statistical,
            ..Default::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"confidence\":\"statistical\""), "{json}");
        assert!(json.contains("\"confidence\":\"Exact\""), "{json}");
        assert!(json.contains("\"confidence\":\"Inferred\""), "{json}");
        assert!(json.contains("\"confidence\":\"Unclassified\""), "{json}");
        let back: DiscoveredSurface = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn surface_round_trips_through_json() {
        let s = sample_surface();
        let json = serde_json::to_string(&s).unwrap();
        let back: DiscoveredSurface = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }

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
                probed_keys_effective: Some(3),
                probed_keys_total: Some(54),
                pruned_devices: Some(vec!["gamepad".to_string()]),
                note: Some("probed 54 keys, 3 effective".to_string()),
            }),
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"_meta\""), "wire key must be _meta: {json}");
        let back: crate::state::ViolationEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back, entry);
    }

    #[test]
    fn violation_without_meta_stays_verbatim_and_back_compatible() {
        // Old reports (pre-discovery) carry neither `meta` nor
        // `discovered_surface`: deserializing them must keep working
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

    #[test]
    fn report_annotation_targets_frozen_world_only() {
        let mut report = crate::state::PlaytestReport::crashed_empty();
        report.violations = vec![
            crate::state::ViolationEntry {
                rule: "frozen_world".to_string(),
                target: "world".to_string(),
                first_frame: 12,
                last_frame: 180,
                count: 1,
                detail: "no Transform change for 180 frames".to_string(),
                last_detail: "no Transform change".to_string(),
                meta: None,
            },
            crate::state::ViolationEntry {
                rule: "log_error".to_string(),
                target: "server".to_string(),
                first_frame: 5,
                last_frame: 5,
                count: 1,
                detail: "connection refused".to_string(),
                last_detail: "connection refused".to_string(),
                meta: None,
            },
        ];
        report.discovered_surface = Some(sample_surface());
        report.annotate_frozen_world_with_surface();
        assert!(
            report.violations[0].meta.is_some(),
            "frozen_world gets annotated"
        );
        // And with the SURFACE'S stats — not just any meta block.
        assert_eq!(
            report.violations[0].meta.as_ref().unwrap(),
            &sample_surface().frozen_world_meta()
        );
        assert!(
            report.violations[1].meta.is_none(),
            "other rules stay untouched"
        );
        // The annotation sits next to the detail, never in it.
        assert!(!report.violations[0].detail.contains("probed"));
    }

    #[test]
    fn report_annotation_is_noop_without_surface() {
        let mut report = crate::state::PlaytestReport::crashed_empty();
        report.violations = vec![crate::state::ViolationEntry {
            rule: "frozen_world".to_string(),
            target: "world".to_string(),
            first_frame: 12,
            last_frame: 180,
            count: 1,
            detail: "no Transform change".to_string(),
            last_detail: "no Transform change".to_string(),
            meta: None,
        }];
        report.annotate_frozen_world_with_surface();
        assert!(report.violations[0].meta.is_none());
    }

    #[test]
    fn report_annotation_is_idempotent() {
        let mut report = crate::state::PlaytestReport::crashed_empty();
        report.violations = vec![crate::state::ViolationEntry {
            rule: "frozen_world".to_string(),
            target: "world".to_string(),
            first_frame: 12,
            last_frame: 180,
            count: 1,
            detail: "no Transform change".to_string(),
            last_detail: "no Transform change".to_string(),
            meta: Some(ViolationMeta {
                probed_keys_effective: Some(99),
                probed_keys_total: Some(99),
                pruned_devices: None,
                note: Some("hand-written".to_string()),
            }),
        }];
        report.discovered_surface = Some(sample_surface());
        report.annotate_frozen_world_with_surface();
        // Existing meta is never overwritten.
        assert_eq!(
            report.violations[0]
                .meta
                .as_ref()
                .unwrap()
                .probed_keys_total,
            Some(99)
        );
    }

    #[test]
    fn old_report_json_deserializes_without_surface_fields() {
        // An "old report" is a current report MINUS the two fields
        // Phase 5 added — constructed by round-tripping through
        // serde_json::Value and deleting them, so the fixture never
        // rots as other report fields evolve.
        let mut report = crate::state::PlaytestReport::crashed_empty();
        report.status = crate::enums::PlaytestStatus::Pass;
        report.frame_count = 180;
        report.violations.push(crate::state::ViolationEntry {
            rule: "frozen_world".to_string(),
            target: "world".to_string(),
            first_frame: 12,
            last_frame: 180,
            count: 1,
            detail: "no Transform change".to_string(),
            last_detail: "no Transform change".to_string(),
            meta: Some(ViolationMeta::default()),
        });
        report.discovered_surface = Some(sample_surface());
        let mut value = serde_json::to_value(&report).unwrap();
        let obj = value.as_object_mut().unwrap();
        assert!(obj.remove("discovered_surface").is_some());
        for v in obj.get_mut("violations").unwrap().as_array_mut().unwrap() {
            // Reports with violations too: entries predate `meta`.
            v.as_object_mut().unwrap().remove("meta");
            v.as_object_mut().unwrap().remove("_meta");
        }
        let old_json = serde_json::to_string(&value).unwrap();
        assert!(!old_json.contains("discovered_surface"));
        assert!(!old_json.contains("_meta"));

        let back: crate::state::PlaytestReport = serde_json::from_str(&old_json).unwrap();
        assert!(back.discovered_surface.is_none());
        // Round trip keeps the field absent.
        let out = serde_json::to_string(&back).unwrap();
        assert!(!out.contains("discovered_surface"));
    }

    #[test]
    fn report_with_surface_round_trips() {
        let mut report = crate::state::PlaytestReport::crashed_empty();
        report.discovered_surface = Some(sample_surface());
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("discovered_surface"));
        let back: crate::state::PlaytestReport = serde_json::from_str(&json).unwrap();
        assert_eq!(back.discovered_surface, report.discovered_surface);
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
    fn to_entries_projects_effective_findings_as_probe_verified() {
        let s = sample_surface();
        let entries = s.to_entries();
        // Only KeyW is effective (the Inferred sibling KeyS carries
        // no effect; KeyA is inert; KeyQ is the control key —
        // excluded even if it spuriously carried one, per the
        // effective-count discipline).
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.input, "KeyW");
        assert_eq!(e.grade, ProvenanceGrade::ProbeVerified);
        assert_eq!(e.effect_signature, Some(effect(2)));
        assert_eq!(e.context, ContextWitness::Boot);
        assert!(e.grade_is_consistent(), "projection must be consistent");
        // The ProbeRun evidence carries the first probe frame —
        // documented lossiness (per-key frames are not retained).
        assert!(
            matches!(e.evidence[0], Evidence::ProbeRun { frame: 60, .. }),
            "first probed_at frame stands in for the lost per-key frame"
        );
        // And the entry SAYS so (refusal-over-silence): the lossiness
        // disclosure travels with the artifact, not just the docs.
        assert_eq!(e.notes.len(), 1, "projection carries its caveat");
        assert!(e.notes[0].contains("not retained"));
        // Control keys and effectless findings never project, even
        // with a spurious effect on the control key.
        let s = DiscoveredSurface {
            keys: vec![
                finding("KeyQ", Some(effect(1)), KeyConfidence::Unclassified),
                finding("KeyA", None, KeyConfidence::Exact),
                finding("KeyW", Some(effect(3)), KeyConfidence::Exact),
            ],
            ..sample_surface()
        };
        let entries = s.to_entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].input, "KeyW");
        // Mouse buttons project too (effective ones, same rules);
        // the sample base already carries an effective KeyW, so the
        // mouse entry ADDS to it rather than replacing it.
        let s = DiscoveredSurface {
            mouse_buttons: vec![finding("Left", Some(effect(1)), KeyConfidence::Exact)],
            ..sample_surface()
        };
        let entries = s.to_entries();
        assert_eq!(entries.len(), 2);
        let inputs: Vec<&str> = entries.iter().map(|e| e.input.as_str()).collect();
        assert!(inputs.contains(&"Left"));
        assert!(inputs.contains(&"KeyW"));
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

    #[test]
    fn empty_entries_and_findings_skip_serialization_old_json_stays_old() {
        // Back-compat both directions: a surface with no IR fields
        // serializes EXACTLY as before (no new keys appear), and a
        // legacy JSON document deserializes with empty IR lists.
        let legacy = sample_surface();
        let json = serde_json::to_string(&legacy).unwrap();
        assert!(!json.contains("\"entries\""), "{json}");
        assert!(!json.contains("\"findings\""), "{json}");
        let old_doc = r#"{
            "keys": [],
            "mouse_buttons": [],
            "mouse_motion": null,
            "wheel": null,
            "gamepad_buttons": [],
            "gamepad_axes": [],
            "probed_at": [60],
            "readers": [["keyboard", 1]],
            "key_confidence": [],
            "confidence": "exact",
            "pruned_devices": [],
            "notes": []
        }"#;
        let back: DiscoveredSurface = serde_json::from_str(old_doc).unwrap();
        assert!(back.entries.is_empty());
        assert!(back.findings.is_empty());
        assert_eq!(back.probed_at, vec![60]);
    }

    #[test]
    fn populated_entries_and_findings_round_trip_on_the_surface() {
        let mut s = sample_surface();
        s.entries = vec![entry(
            ProvenanceGrade::ProbeVerified,
            vec![probe_run_evidence()],
        )];
        s.findings = vec![DiscoveryFinding {
            kind: DiscoveryFindingKind::DiscoveredButUndeclared,
            input: "KeyW".to_string(),
            detail: "probe-verified but no declared source names it".to_string(),
        }];
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"entries\""), "{json}");
        assert!(json.contains("\"findings\""), "{json}");
        let back: DiscoveredSurface = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }
}
