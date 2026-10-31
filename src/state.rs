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
    /// non-takeover producers that never had a source path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scenario_source: Option<String>,
    /// External-BRP parity: "full" when the game serves the
    /// bevy_playtest/* BRP methods, "degraded" otherwise. Absent for
    /// in-process runs (inherently full parity).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brp_parity: Option<String>,
    /// Whether log capture was active for the producing process.
    /// `Some(false)` = another tracing subscriber won the write-once
    /// global default (typically DefaultPlugins' LogPlugin), so
    /// log_error/log_warn findings are under-reported. The run also
    /// carries an informational `log_capture_lost` violation. Absent
    /// for producers where the concept does not apply (BRP children
    /// never capture logs by design).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_capture_available: Option<bool>,
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
            brp_parity: None,
            log_capture_available: None,
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
