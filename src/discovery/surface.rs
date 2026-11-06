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
}
