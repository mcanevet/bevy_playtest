//! Runtime state resources: violations and metrics.

use crate::choice::Choice;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Violations — dedup by (rule, target), ordered by first_frame
// ---------------------------------------------------------------------------

/// A single violation occurrence.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ViolationEntry {
    pub rule: String,
    pub target: String,
    pub first_frame: u64,
    pub last_frame: u64,
    pub count: u64,
    /// Detail + intent context at FIRST occurrence (reproduction anchor).
    pub detail: String,
    /// Detail at the most recent occurrence (trend/debug).
    pub last_detail: String,
    /// Structured annotation (serialized as `_meta`), present when
    /// the oracle has machine-readable evidence for this finding (e.g.
    /// frozen-world `stepping`). Sits NEXT TO the verbatim
    /// `detail` — never inside it — so consumers parsing details as
    /// raw text are unaffected.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none", default)]
    pub meta: Option<crate::discovery::surface::ViolationMeta>,
}

/// Runtime violation tracker.
#[derive(Clone, Debug, Default)]
pub struct Violations {
    pub entries: BTreeMap<(String, String), ViolationEntry>,
}

impl Violations {
    pub fn report(&mut self, rule: &str, target: &str, detail: String, frame: u64) {
        let key = (rule.to_string(), target.to_string());
        // Details are reported verbatim: consumers (JUnit renderers,
        // dashboards) parse them as raw text.
        let prefixed = detail;
        let entry = self.entries.entry(key).or_insert_with(|| ViolationEntry {
            rule: rule.to_string(),
            target: target.to_string(),
            first_frame: frame,
            last_frame: frame,
            count: 0,
            detail: prefixed.clone(),
            last_detail: prefixed.clone(),
            meta: None,
        });
        entry.count += 1;
        entry.last_frame = frame;
        entry.last_detail = prefixed;
    }

    /// Snapshot sorted by first_frame.
    pub fn snapshot(&self) -> Vec<ViolationEntry> {
        let mut v: Vec<ViolationEntry> = self.entries.values().cloned().collect();
        v.sort_by(|a, b| {
            (a.first_frame, &a.rule, &a.target).cmp(&(b.first_frame, &b.rule, &b.target))
        });
        v
    }
}

// ---------------------------------------------------------------------------
// Runtime state resources
// ---------------------------------------------------------------------------

/// Metrics exported in reports.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Metrics {
    pub input_count: u64,
    pub frame_count: u64,
    pub worst_frame_ms: f64,
    pub crash_detected: bool,
}

/// Audit-trail action entry.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActionEntry {
    pub frame: u64,
    pub source: String,
    pub action: String,
    pub details: Option<String>,
}

/// Playtest report.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlaytestReport {
    pub schema_version: u32,
    pub status: crate::enums::PlaytestStatus,
    pub violations: Vec<ViolationEntry>,
    pub metrics: Metrics,
    pub frame_count: u64,
    pub tps: u64,
    pub simulated_duration_s: f64,
    pub wall_time_s: f64,
    pub error: Option<String>,
    pub action_log: Vec<ActionEntry>,
    pub choices: Vec<Choice>,
    /// Mode that produced this report.
    pub harness_mode: String,
    /// Provenance of the executed scenario: `"file"`, `"builtin_smoke"`, or
    /// `"builtin_battery"` (the zero-config battery literal). Absent for
    /// report stubs that never executed a sourced scenario.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub scenario_source: Option<String>,
    /// Whether log capture was active for the producing process.
    /// `Some(false)` = another tracing subscriber won the write-once
    /// global default (typically DefaultPlugins' LogPlugin), so
    /// log_error/log_warn findings are under-reported. The run also
    /// carries an informational `log_capture_lost` violation.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub log_capture_available: Option<bool>,
    /// Whether the log→tracing bridge (log crate → global tracing
    /// subscriber) was active for the producing process. `Some(false)`
    /// = another `log` crate logger owned the write-once slot, so
    /// `log`-crate events (bevy's B000x hierarchy validators,
    /// wgpu/naga, any log-crate user) are invisible to
    /// log_error/log_warn even though `log_capture_available` may be
    /// `true`. Distinguishes the compound-lost case from
    /// subscriber-loss alone. Absent on stubs and pre-field reports.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub log_bridge_available: Option<bool>,
    /// Per-frame broadened state digests (opt-in via `record_digest: true`).
    /// Covers entity/archetype counts, Transform/UiTransform translations,
    /// and reflected game resources. Skipped when empty to keep reports
    /// compact for runs that didn't record.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub state_digest: Vec<crate::digest::StateDigest>,
    /// Per-sample progress-liveness series (frame, whole-surface hash,
    /// churn, channel count) recorded when the scenario's
    /// liveness.progress mode is not `off`. Evidence behind every
    /// progress verdict; comparable across runs.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub progress_samples: Vec<crate::progress::ProgressSample>,
    /// Build profile of the GAME binary that produced this report.
    /// In-process and takeover runs alike stamp the reporting
    /// process's own `cfg!(debug_assertions)` — the report-writing
    /// code lives in the game's binary in both modes, so this is
    /// the game's profile, not the harness library's (cfg-gated
    /// code makes dev-profile behavior differ from release).
    /// Additive, default `None` (absent on stubs and pre-field
    /// reports).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub game_build_profile: Option<crate::discovery::surface::BuildProfile>,
    /// Source-derived chaos keys the battery parent put into this
    /// run's surface via the source scan (`BEVY_PLAYTEST_SOURCE_ROOT`,
    /// or the game crate's runtime `CARGO_MANIFEST_DIR`): the scan's
    /// poison-filtered unique-key set that REPLACED the WASD/arrows
    /// default. `None` means no replacement happened (no scan, a
    /// failed or empty scan, or a backbone-equal scan), not "the scan
    /// found nothing".
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source_derived_keys: Option<Vec<String>>,
    /// Provenance notes from the parent's source scan (poison
    /// filtering, cap drops, unreadable files, unsupported variants).
    /// Present exactly when [`Self::source_derived_keys`] is.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source_derived_notes: Option<Vec<String>>,
    /// Leafwing declared bindings read from the game's `InputMap`s
    /// during this run (`lw-reader` feature; `None` when the
    /// feature is off or the game spawns no Leafwing state).
    /// Additive honesty field: lets a report consumer distinguish
    /// "nothing declared" from "reader not compiled in".
    #[cfg(feature = "lw-reader")]
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub lw_declared: Vec<crate::discovery::lw::LwDeclaration>,
    /// Leafwing intent-tap statistics accumulated across this run
    /// (`lw-reader` feature). Same additive contract as
    /// [`Self::lw_declared`].
    #[cfg(feature = "lw-reader")]
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub lw_tap_stats: Vec<crate::discovery::lw::LwTapStats>,
    /// Leafwing reconcile of [`Self::lw_declared`] against
    /// [`Self::lw_tap_stats`] (`lw-reader` feature): declared entries
    /// graded `Declared` / `ManagerPipelineOk`, plus `declared_but_dead`
    /// alarms for proven-dead wiring and `context_unverified` notes.
    /// `None` when the game has no Leafwing state or the feature is off.
    #[cfg(feature = "lw-reader")]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub lw_reconciled: Option<crate::discovery::lw::ReconciledLw>,
    /// What the game did during the run, observed generically: state
    /// schedules that ran (and those that never did) and game
    /// resources that changed. `None` on crash stubs and pre-field
    /// reports. See [`crate::discovery::state_trace`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub state_trace: Option<crate::discovery::state_trace::StateTrace>,
    /// Whether the battery made OS-seeded randomness deterministic for
    /// this run, and how or why not (e.g. "intercepted getrandom()
    /// (LD_PRELOAD), seed 42" / "not intercepted: not Linux"). Stamped
    /// by the battery parent; `None` for in-process runs (the process
    /// already started, so nothing could be intercepted) and old reports.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub randomness: Option<String>,
    /// Load-time capability census rows captured during
    /// `run_scenario` startup, before bot/oracle registration
    /// (present whenever
    /// [`crate::driver::run_scenario`] executed). Additive
    /// default-`None` field: absent on crash stubs and pre-field
    /// reports; older reports without the field deserialize as
    /// `None`, so consumers must treat absence as "no census",
    /// never "empty census".
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub capability_census: Option<crate::discovery::census::CapabilityCensus>,
    /// Declared UI/interactable surface: captured after the first
    /// simulated frame and RE-CAPTURED periodically (fixed tick
    /// interval, ~twice a second) so mid-run surfaces (menus,
    /// dialogs) join it; the field carries the LATEST snapshot plus
    /// bounded delta notes (`ui-surface` feature; `None` when the
    /// feature is off or the run ended before capture). Same
    /// additive-honesty contract as
    /// [`Self::capability_census`]: absence never means "no UI".
    /// See [`crate::discovery::ui_surface`].
    #[cfg(feature = "ui-surface")]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ui_surface: Option<crate::discovery::ui_surface::UiSurfaceResult>,
    /// Structural render-liveness verdict: whether the game had a
    /// camera, whether it was active, whether its output was
    /// written anywhere (`render-liveness` feature; `None` when the
    /// feature is off or the game has no render sub-app — "not
    /// evaluated", never "healthy"). Same additive-honesty
    /// contract as
    /// [`Self::capability_census`]: absence never means "fine".
    /// The finding ALSO rides `violations` as an informational
    /// `render_liveness` entry; this field carries the evidence
    /// (tier, camera counts, sample count, notes). See
    /// [`crate::discovery::render_liveness`].
    #[cfg(feature = "render-liveness")]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub render_liveness: Option<crate::discovery::render_liveness::RenderLivenessResult>,

    /// Pixel-observed screen-health evidence from the harness-owned
    /// capture rig: hashes of what the renderer actually produced,
    /// greatest inter-capture Hamming drift, and the verdict tier
    /// (`screen-liveness` feature; `None` when the feature is off,
    /// the game has no render sub-app, or the asset store is
    /// missing — "not evaluated", never "healthy"). Same
    /// additive-honesty contract as
    /// [`Self::render_liveness`]: absence never means "fine". The
    /// finding ALSO rides `violations` as an informational
    /// `screen_liveness` entry; this field carries the evidence
    /// (tier, request/capture counts, Hamming drift, notes). See
    /// [`crate::discovery::screen_liveness`].
    #[cfg(feature = "screen-liveness")]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub screen_liveness: Option<crate::discovery::screen_liveness::ScreenLivenessResult>,
}

impl PlaytestReport {
    pub fn crashed_empty() -> Self {
        Self {
            schema_version: 1,
            status: crate::enums::PlaytestStatus::Crash,
            violations: vec![],
            metrics: Default::default(),
            frame_count: 0,
            wall_time_s: 0.0,
            tps: 0,
            simulated_duration_s: 0.0,
            error: None,
            action_log: vec![],
            choices: vec![],
            harness_mode: "in_process".to_string(),
            scenario_source: None,
            log_capture_available: None,
            log_bridge_available: None,
            // Crashed-before-reporting stub: no game run ever
            // completed, so no profile is claimable.
            game_build_profile: None,
            state_digest: vec![],
            progress_samples: vec![],
            source_derived_keys: None,
            source_derived_notes: None,
            #[cfg(feature = "lw-reader")]
            lw_declared: vec![],
            #[cfg(feature = "lw-reader")]
            lw_tap_stats: vec![],
            #[cfg(feature = "lw-reader")]
            lw_reconciled: None,
            state_trace: None,
            randomness: None,
            // Crash stub: the run never reached the census attach
            // point, so no census exists.
            capability_census: None,
            // Crash stub: the run never reached the capture-after-
            // first-frame point, so no UI surface exists.
            #[cfg(feature = "ui-surface")]
            ui_surface: None,
            // Crash stub: same reasoning — no render-liveness
            // verdict exists, and absence never means "healthy".
            #[cfg(feature = "render-liveness")]
            render_liveness: None,
            #[cfg(feature = "screen-liveness")]
            screen_liveness: None,
        }
    }

    /// Sorted, deduplicated list of violation rule names — for
    /// `assert_eq!(report.violation_rules(), &["frozen_world"])` style
    /// assertions in fixture tests.
    pub fn violation_rules(&self) -> Vec<&str> {
        let mut rules: Vec<&str> = self.violations.iter().map(|v| v.rule.as_str()).collect();
        rules.sort_unstable();
        rules.dedup();
        rules
    }

    /// Sorted, deduplicated list of `(rule, target)` pairs — for
    /// assertions like `assert!(report.has_violation("frozen_world", "world"))`.
    pub fn violation_pairs(&self) -> Vec<(&str, &str)> {
        let mut pairs: Vec<(&str, &str)> = self
            .violations
            .iter()
            .map(|v| (v.rule.as_str(), v.target.as_str()))
            .collect();
        pairs.sort_unstable();
        pairs.dedup();
        pairs
    }

    /// True if the report contains at least one violation with this
    /// exact `(rule, target)` pair.
    pub fn has_violation(&self, rule: &str, target: &str) -> bool {
        self.violations
            .iter()
            .any(|v| v.rule == rule && v.target == target)
    }

    pub fn scenario_error(msg: &str) -> Self {
        let mut rep = Self::crashed_empty();
        rep.error = Some(msg.to_string());
        rep.violations = vec![ViolationEntry {
            rule: "scenario_error".to_string(),
            target: "scenario".to_string(),
            first_frame: 0,
            last_frame: 0,
            count: 1,
            detail: msg.to_string(),
            last_detail: msg.to_string(),
            meta: None,
        }];
        rep
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report_with(entries: &[(&str, &str)]) -> PlaytestReport {
        let mut rep = PlaytestReport::crashed_empty();
        rep.violations = entries
            .iter()
            .map(|&(rule, target)| ViolationEntry {
                rule: rule.to_string(),
                target: target.to_string(),
                first_frame: 0,
                last_frame: 0,
                count: 1,
                detail: String::new(),
                last_detail: String::new(),
                meta: None,
            })
            .collect();
        rep
    }

    #[test]
    fn violation_pairs_is_exact_no_silent_extras() {
        // A test asserting [("frozen_world", "world")] on a report that
        // ALSO contains another violation must FAIL — extras never pass
        // silently. Prove it by building both sides of the comparison.
        let rep = report_with(&[("frozen_world", "world"), ("log_error", "server")]);
        assert_ne!(rep.violation_pairs(), vec![("frozen_world", "world")]);
        assert_eq!(
            rep.violation_pairs(),
            vec![("frozen_world", "world"), ("log_error", "server")]
        );
    }

    #[test]
    fn violation_pairs_sorted_and_deduped() {
        let rep = report_with(&[
            ("frozen_world", "world"),
            ("panic", "game_system"),
            ("frozen_world", "world"),
        ]);
        assert_eq!(
            rep.violation_pairs(),
            vec![("frozen_world", "world"), ("panic", "game_system")]
        );
    }

    #[test]
    fn has_violation_matches_rule_and_target() {
        let rep = report_with(&[("frozen_world", "world")]);
        assert!(rep.has_violation("frozen_world", "world"));
        assert!(!rep.has_violation("frozen_world", "other_target"));
        assert!(!rep.has_violation("other_rule", "world"));
    }
}
