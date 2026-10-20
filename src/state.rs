//! Runtime state resources: violations and metrics.

use crate::choice::Choice;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Violations — dedup by (rule, target), ordered by first_frame
// ---------------------------------------------------------------------------

/// A single violation occurrence.
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
    /// Frozen-world oracle counter.
    pub frozen_frames: u64,
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
    pub frame_count: u64,
    pub tps: u64,
    pub simulated_duration_s: f64,
    pub wall_time_s: f64,
    pub error: Option<String>,
    pub action_log: Vec<ActionEntry>,
    pub cheat_count: usize,
    pub choices: Vec<Choice>,
    /// Mode that produced this report.
    pub harness_mode: String,
    /// Provenance of the executed scenario: "file" or "builtin_smoke".
    /// Absent for non-takeover producers that never had a source path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scenario_source: Option<String>,
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
            cheat_count: 0,
            choices: vec![],
            harness_mode: "in_process".to_string(),
            scenario_source: None,
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
        }];
        rep
    }
}
