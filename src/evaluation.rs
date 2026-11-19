//! Exploration evaluation: run a game's battery over several seeds with
//! a fixed frame budget and score it against a committed targets file.
//!
//! "Reached" is decided ONLY from the report's independent
//! [`state_trace`](crate::discovery::state_trace) (state schedules that
//! ran, game resources that changed) — never from the bot's own claims.
//! Targets that cannot be measured generically are kept in the file as
//! `unmeasurable` with the reason, so the gap stays visible.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::battery::{run_with_overrides, BatteryConfig, ScenarioOverrides, BUILTIN_BATTERY};
use crate::discovery::state_trace::StateTrace;

/// A committed per-fixture targets file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TargetsFile {
    pub fixture: String,
    /// Game crate directory (relative to the targets file's repo root):
    /// becomes the child's `CARGO_MANIFEST_DIR`, so assets and the
    /// source scan resolve exactly as under `cargo test`.
    pub manifest_dir: String,
    pub targets: Vec<Target>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Target {
    pub id: String,
    pub description: String,
    pub check: Check,
}

/// How a target is decided from one run's [`StateTrace`].
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Check {
    /// Every listed state schedule ran at least once.
    Entered(Vec<String>),
    /// The state schedule ran at least `at_least` times.
    EnteredTimes { label: String, at_least: usize },
    /// At least one listed game resource (type-name suffix) changed
    /// after the startup frame.
    ResourceChangedAny(Vec<String>),
    /// The game resource changed in at least `at_least` frames.
    ResourceChangedTimes { resource: String, at_least: u64 },
    /// Not decidable generically today; the reason is reported.
    Unmeasurable(String),
}

/// Outcome of one target on one run: `Some(frame)` when reached.
fn evaluate(check: &Check, trace: &StateTrace) -> Option<Option<u64>> {
    let first_run = |label: &str| {
        trace
            .schedules_run
            .iter()
            .find(|r| r.label == label)
            .map(|r| r.frame)
    };
    let resource = |suffix: &str| {
        trace
            .resource_changes
            .iter()
            .find(|r| r.name.ends_with(&format!("::{suffix}")) || r.name == suffix)
    };
    Some(match check {
        Check::Entered(labels) => labels
            .iter()
            .map(|l| first_run(l))
            .collect::<Option<Vec<u64>>>()
            .and_then(|frames| frames.into_iter().max()),
        Check::EnteredTimes { label, at_least } => {
            let frames: Vec<u64> = trace
                .schedules_run
                .iter()
                .filter(|r| &r.label == label)
                .map(|r| r.frame)
                .collect();
            (frames.len() >= *at_least && *at_least > 0).then(|| frames[*at_least - 1])
        }
        Check::ResourceChangedAny(names) => names
            .iter()
            .filter_map(|n| resource(n))
            .map(|r| r.first_frame)
            .min(),
        Check::ResourceChangedTimes {
            resource: name,
            at_least,
        } => resource(name)
            .filter(|r| r.frames_changed >= *at_least && *at_least > 0)
            .map(|r| {
                // Exact frame of the Nth change when recorded; beyond the
                // kept window, the last change is the honest upper bound.
                r.change_frames
                    .get(*at_least as usize - 1)
                    .copied()
                    .unwrap_or(r.last_frame)
            }),
        Check::Unmeasurable(_) => return None,
    })
}

/// One target's result across all seeds.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TargetResult {
    pub id: String,
    pub description: String,
    /// `None` when the target is unmeasurable.
    pub hits: Option<usize>,
    pub runs: usize,
    /// Median frame of first reach, over the runs that reached it.
    pub median_frame: Option<u64>,
    pub unmeasurable_reason: Option<String>,
}

/// One run (seed) summary.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunSummary {
    pub seed: u64,
    pub status: String,
    pub wall_s: f64,
    pub violation_rules: Vec<String>,
    /// Infrastructure error instead of a report, if any.
    pub error: Option<String>,
    /// The run's raw state trace (the evidence behind every hit).
    pub trace: Option<StateTrace>,
    /// Each finding as "rule (target): detail", first occurrence, capped.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<String>,
    /// The exploring bot's own summary (`bot.explore` runs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exploration: Option<crate::explorer::ExplorationSummary>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Evaluation {
    pub fixture: String,
    pub frames: u64,
    pub seeds: Vec<u64>,
    pub runs: Vec<RunSummary>,
    pub targets: Vec<TargetResult>,
}

/// Run `binary` once per seed for `frames` frames and score `targets`.
pub fn run_evaluation(
    targets: &TargetsFile,
    repo_root: &Path,
    binary: &str,
    seeds: &[u64],
    frames: u64,
) -> Evaluation {
    let tps = crate::plugin::battery_scenario()
        .map(|s| s.tps)
        .unwrap_or(60)
        .max(1);
    let duration_s = frames as f32 / tps as f32;
    let manifest_dir: PathBuf = repo_root.join(&targets.manifest_dir);
    // The child (and the battery's source scan) resolve the game crate
    // through CARGO_MANIFEST_DIR, and the battery mirrors the CURRENT
    // directory into the child's working directory, so games reading
    // `./assets/...` directly need it to be the game crate, exactly as
    // under `cargo test`.
    //
    // Resolve the binary BEFORE changing directory (callers pass paths
    // relative to the repo root).
    let binary = std::fs::canonicalize(binary)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| binary.to_string());
    let binary = binary.as_str();
    // Both are restored when this function returns OR panics, so callers'
    // relative paths (e.g. `--json-out`) and their own CARGO_MANIFEST_DIR
    // are untouched afterwards.
    struct RestoreProcessState {
        dir: Option<PathBuf>,
        manifest_dir: Option<std::ffi::OsString>,
    }
    impl Drop for RestoreProcessState {
        fn drop(&mut self) {
            if let Some(dir) = &self.dir {
                let _ = std::env::set_current_dir(dir);
            }
            match &self.manifest_dir {
                Some(v) => std::env::set_var("CARGO_MANIFEST_DIR", v),
                None => std::env::remove_var("CARGO_MANIFEST_DIR"),
            }
        }
    }
    let _restore = RestoreProcessState {
        dir: std::env::current_dir().ok(),
        manifest_dir: std::env::var_os("CARGO_MANIFEST_DIR"),
    };
    std::env::set_var("CARGO_MANIFEST_DIR", &manifest_dir);
    if let Err(e) = std::env::set_current_dir(&manifest_dir) {
        eprintln!(
            "bevy_playtest_eval: cannot enter {}: {e}",
            manifest_dir.display()
        );
    }

    let mut traces: Vec<Option<StateTrace>> = Vec::new();
    let mut runs = Vec::new();
    for &seed in seeds {
        let start = std::time::Instant::now();
        let result = run_with_overrides(
            &BatteryConfig {
                binary: binary.to_string(),
                scenario: BUILTIN_BATTERY.to_string(),
                timeout_secs: 900,
                seed_override: None,
            },
            &ScenarioOverrides {
                seed: Some(seed),
                duration_s: Some(duration_s),
            },
        );
        let wall_s = start.elapsed().as_secs_f64();
        match result {
            Ok(report) => {
                runs.push(RunSummary {
                    seed,
                    status: format!("{:?}", report.status),
                    wall_s,
                    violation_rules: report
                        .violation_rules()
                        .iter()
                        .map(|r| r.to_string())
                        .collect(),
                    error: None,
                    findings: report
                        .violations
                        .iter()
                        .take(20)
                        .map(|v| format!("{} ({}): {}", v.rule, v.target, v.detail))
                        .collect(),
                    trace: report.state_trace.clone(),
                    exploration: report.exploration.clone(),
                });
                traces.push(report.state_trace);
            }
            Err(e) => {
                runs.push(RunSummary {
                    seed,
                    status: "error".to_string(),
                    wall_s,
                    violation_rules: vec![],
                    error: Some(e.to_string()),
                    findings: vec![],
                    trace: None,
                    exploration: None,
                });
                traces.push(None);
            }
        }
    }

    let results = targets
        .targets
        .iter()
        .map(|t| {
            if let Check::Unmeasurable(reason) = &t.check {
                return TargetResult {
                    id: t.id.clone(),
                    description: t.description.clone(),
                    hits: None,
                    runs: traces.len(),
                    median_frame: None,
                    unmeasurable_reason: Some(reason.clone()),
                };
            }
            let mut frames: Vec<u64> = traces
                .iter()
                .filter_map(|tr| tr.as_ref().and_then(|tr| evaluate(&t.check, tr).flatten()))
                .collect();
            frames.sort_unstable();
            TargetResult {
                id: t.id.clone(),
                description: t.description.clone(),
                hits: Some(frames.len()),
                runs: traces.len(),
                median_frame: frames.get(frames.len() / 2).copied(),
                unmeasurable_reason: None,
            }
        })
        .collect();

    Evaluation {
        fixture: targets.fixture.clone(),
        frames,
        seeds: seeds.to_vec(),
        runs,
        targets: results,
    }
}

impl Evaluation {
    /// Markdown table for humans (CI step summary).
    pub fn to_markdown(&self) -> String {
        let mut out = format!(
            "### {} — {} frames × {} seeds\n\n| Target | Hits | Median frame | Description |\n|---|---|---|---|\n",
            self.fixture,
            self.frames,
            self.seeds.len()
        );
        for t in &self.targets {
            let hits = match (t.hits, &t.unmeasurable_reason) {
                (Some(h), _) => format!("{h}/{}", t.runs),
                (None, _) => "unmeasurable".to_string(),
            };
            let median = t
                .median_frame
                .map(|f| f.to_string())
                .unwrap_or_else(|| "—".into());
            let desc = match &t.unmeasurable_reason {
                Some(r) => format!("{} ({r})", t.description),
                None => t.description.clone(),
            };
            out.push_str(&format!("| {} | {hits} | {median} | {desc} |\n", t.id));
        }
        let errors: Vec<String> = self
            .runs
            .iter()
            .filter_map(|r| r.error.as_ref().map(|e| format!("seed {}: {e}", r.seed)))
            .collect();
        let wall: f64 = self.runs.iter().map(|r| r.wall_s).sum();
        out.push_str(&format!("\nWall time: {wall:.0}s total.\n"));
        if !errors.is_empty() {
            out.push_str(&format!("\n**Run errors:** {}\n", errors.join("; ")));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::state_trace::{ResourceChange, ScheduleRun};

    fn trace() -> StateTrace {
        StateTrace {
            schedules_run: vec![
                ScheduleRun {
                    frame: 0,
                    label: "OnEnter(Menu)".into(),
                },
                ScheduleRun {
                    frame: 40,
                    label: "OnEnter(Game)".into(),
                },
                ScheduleRun {
                    frame: 90,
                    label: "OnEnter(Menu)".into(),
                },
            ],
            schedules_never_run: vec!["OnEnter(Secret)".into()],
            resource_changes: vec![ResourceChange {
                name: "fixture_game_menu::Volume".into(),
                first_frame: 55,
                last_frame: 70,
                frames_changed: 3,
                change_frames: vec![55, 60, 70],
            }],
            notes: vec![],
        }
    }

    #[test]
    fn checks_decide_from_the_trace() {
        let t = trace();
        let entered = Check::Entered(vec!["OnEnter(Menu)".into(), "OnEnter(Game)".into()]);
        assert_eq!(evaluate(&entered, &t), Some(Some(40)));
        let missing = Check::Entered(vec!["OnEnter(Secret)".into()]);
        assert_eq!(evaluate(&missing, &t), Some(None));
        let twice = Check::EnteredTimes {
            label: "OnEnter(Menu)".into(),
            at_least: 2,
        };
        assert_eq!(evaluate(&twice, &t), Some(Some(90)));
        let changed = Check::ResourceChangedAny(vec!["DisplayQuality".into(), "Volume".into()]);
        assert_eq!(evaluate(&changed, &t), Some(Some(55)));
        let times = Check::ResourceChangedTimes {
            resource: "Volume".into(),
            at_least: 4,
        };
        assert_eq!(evaluate(&times, &t), Some(None));
        assert_eq!(evaluate(&Check::Unmeasurable("x".into()), &t), None);
    }

    #[test]
    fn suffix_match_does_not_match_partial_type_names() {
        let t = trace();
        let partial = Check::ResourceChangedAny(vec!["lume".into()]);
        assert_eq!(evaluate(&partial, &t), Some(None));
    }
}
