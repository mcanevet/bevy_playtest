//! Runtime state resources: violations, metrics, coverage.
//!
//! v0.1: the minimal set needed to run oracles and emit reports.
//! Fingerprints, system coverage, state traces, and advanced coverage
//! are deferred to v0.2.

use crate::choice::Choice;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

// ---------------------------------------------------------------------------
// Violations — dedup by (rule, target), ordered by first_frame
// ---------------------------------------------------------------------------

/// A single violation occurrence. v0.1 omits fingerprints (deferred);
/// the field is present but always `None` so the report shape stays
/// stable for v0.2.
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
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
    /// v0.1: always None (fingerprints deferred to v0.2).
    pub fingerprint: Option<Fingerprint>,
    /// Scheme version (v0.1: 1; documented migration bumps this).
    pub fingerprint_scheme: u32,
}

/// Stable failure identity (T1). v0.1: placeholder (always None).
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Fingerprint {
    pub kind: String,
    pub rule: String,
    pub detail_hash: u64,
}

/// Runtime violation tracker.
#[derive(Clone, Debug, Default)]
pub struct Violations {
    pub entries: BTreeMap<(String, String), ViolationEntry>,
    /// Latest bot intent in flight (set by bot systems each fire).
    context: String,
}

impl Violations {
    pub fn set_context(&mut self, ctx: impl Into<String>) {
        self.context = ctx.into();
    }

    pub fn report(&mut self, rule: &str, target: &str, detail: String, frame: u64) {
        let key = (rule.to_string(), target.to_string());
        let prefixed = format!("[intent: {}] {}", self.context, detail);
        let entry = self.entries.entry(key).or_insert_with(|| ViolationEntry {
            rule: rule.to_string(),
            target: target.to_string(),
            first_frame: frame,
            last_frame: frame,
            count: 0,
            detail: prefixed.clone(),
            last_detail: prefixed.clone(),
            fingerprint: None,
            fingerprint_scheme: 1,
        });
        entry.count += 1;
        entry.last_frame = frame;
        entry.last_detail = prefixed;
    }

    /// Snapshot sorted by first_frame. v0.1: no fingerprinting.
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

/// Execution-time state (reduced for v0.1).
#[derive(Default)]
pub struct PlaytestState {
    pub frame: u64,
    pub tps: u64,
    pub metrics: Metrics,
    #[allow(dead_code)] // oracle paths land in v0.2 with the typed oracle family
    pub(crate) warned_paths: HashSet<String>,
    /// Liveness frame history (I4).
    pub last_alive_frame: u64,
    /// Turn-based stuck streak (I4).
    pub stuck_after_intent_quanta: u64,
    pub coverage: Coverage,
    /// Frozen-world oracle counter.
    pub frozen_frames: u64,
    /// Pre-ready frames (readiness gate).
    pub pre_ready_frames: u64,
    /// Action log (audit trail).
    pub action_log: Vec<ActionEntry>,
    /// Action-effect rates (Z1 vacuity guard).
    pub action_effect_rate: HashMap<String, EffectRate>,
    /// Choices (J0).
    pub choices: Vec<Choice>,
    /// Robustness margins (J1).
    pub robustness: RobustnessSummary,
}

/// Metrics exported in reports.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Metrics {
    pub input_count: u64,
    pub frame_count: u64,
    pub frame_ms_p99: f64,
    pub worst_frame_ms: f64,
    pub crash_detected: bool,
}

/// Coverage metrics.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Coverage {
    pub intents_emitted: HashMap<String, u64>,
    pub test_api_paths_read: HashSet<String>,
    pub resets_invoked: HashSet<String>,
    pub chaos_surface_indices: HashSet<u64>,
}

/// Per-action effect rate (Z1).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EffectRate {
    /// Effective actions / total actions for this key.
    pub effective: u64,
    pub total: u64,
}

/// J1: robustness summary.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RobustnessSummary {
    /// Per-invariant signed margins.
    pub per_invariant: HashMap<String, f64>,
    /// Overall normalized margin.
    pub overall: f64,
}

/// Audit-trail action entry.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActionEntry {
    pub frame: u64,
    pub source: String,
    pub action: String,
    pub details: Option<String>,
}

/// Playtest report (v0.1).
#[derive(Clone, Debug, Serialize)]
pub struct PlaytestReport {
    pub schema_version: u32,
    pub status: crate::enums::PlaytestStatus,
    pub violations: Vec<ViolationEntry>,
    pub metrics: Metrics,
    pub coverage: Coverage,
    pub frame_count: u64,
    pub tps: u64,
    pub simulated_duration_s: f64,
    pub wall_time_s: f64,
    pub pre_ready_frames: u64,
    pub error: Option<String>,
    pub action_log: Vec<ActionEntry>,
    pub game_version: Option<String>,
    pub cheat_count: usize,
    pub unreplayable_actions: usize,
    pub warnings: Vec<String>,
    pub action_effect_rate: HashMap<String, EffectRate>,
    pub choices: Vec<Choice>,
    pub robustness: RobustnessSummary,
    /// Tier + what ran (Z1).
    pub scope: CheckedScope,
    /// Mode that produced this report.
    pub harness_mode: String,
}

/// Z1: tier scope.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CheckedScope {
    /// "zero" | "observable" | "semantic".
    pub depth: String,
    /// Which oracle families ran.
    pub oracles: Vec<String>,
    /// Actuators used.
    pub actuators: Vec<String>,
}

impl PlaytestReport {
    pub fn crashed_empty() -> Self {
        Self {
            schema_version: 1,
            status: crate::enums::PlaytestStatus::Crash,
            violations: vec![],
            metrics: Default::default(),
            coverage: Default::default(),
            frame_count: 0,
            wall_time_s: 0.0,
            tps: 0,
            simulated_duration_s: 0.0,
            pre_ready_frames: 0,
            error: None,
            action_log: vec![],
            game_version: None,
            cheat_count: 0,
            unreplayable_actions: 0,
            warnings: vec![],
            action_effect_rate: Default::default(),
            choices: vec![],
            robustness: Default::default(),
            scope: CheckedScope::default(),
            harness_mode: "in_process".to_string(),
        }
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
            fingerprint: None,
            fingerprint_scheme: 1,
        }];
        rep
    }
}
