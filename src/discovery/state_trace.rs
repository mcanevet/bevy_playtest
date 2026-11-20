//! State trace: what the game DID during a run, observed generically.
//!
//! Two signals, both available for unmodified games (no `Reflect`, no
//! state registration, no hooks):
//!
//! - **State schedules that ran.** Every `OnEnter(..)` / `OnExit(..)` /
//!   `OnTransition { .. }` schedule the game registered gets a tiny
//!   recorder system. When bevy_state runs the schedule, the recorder
//!   logs its label (its `Debug` form, e.g. `OnEnter(Settings)`). The
//!   labels that exist but never ran are reported too: those are the
//!   states the run never entered (only states with at least one
//!   enter/exit system have a schedule — the rest are invisible here).
//! - **Game resources that changed.** After every frame, each resource
//!   whose type path is not an engine (`bevy*`) path is checked against
//!   its change ticks. Change ticks need no reflection, so this works
//!   for plain `#[derive(Resource)]` types; it says WHEN a resource was
//!   written, never its value. A resource written every frame (timers,
//!   per-frame scratch) is visible as such via `frames_changed`.
//!
//! The recorder systems hold no world state (they push into an
//! `Arc<Mutex<..>>` the driver drains between frames), so they add no
//! entities, resources or components: the digest and census see
//! nothing new.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use bevy_app::App;
use bevy_ecs::change_detection::Tick;
use bevy_ecs::schedule::Schedules;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

/// One state schedule run, stamped with the driver frame it ran in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleRun {
    pub frame: u64,
    /// The schedule label's `Debug` form, e.g. `OnEnter(Settings)`.
    pub label: String,
}

/// Change history of one game resource (startup frame excluded).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceChange {
    /// Full type path, e.g. `fixture_game_menu::DisplayQuality`.
    pub name: String,
    pub first_frame: u64,
    pub last_frame: u64,
    /// Frames (after frame 0) in which the resource was written.
    pub frames_changed: u64,
    /// The first [`MAX_CHANGE_FRAMES`] frames in which it was written.
    #[serde(default)]
    pub change_frames: Vec<u64>,
}

/// How many individual change frames are kept per resource.
pub const MAX_CHANGE_FRAMES: usize = 64;

/// The run's state trace, attached to the report as `state_trace`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateTrace {
    /// Every state schedule run, in order.
    pub schedules_run: Vec<ScheduleRun>,
    /// State schedules the game registered that never ran, sorted.
    pub schedules_never_run: Vec<String>,
    /// Game resources written after the startup frame, sorted by name.
    pub resource_changes: Vec<ResourceChange>,
    /// Honest caveats (masked names, schedules absent, ...).
    pub notes: Vec<String>,
}

/// Is this schedule label a bevy_state transition schedule?
fn is_state_schedule(label: &str) -> bool {
    label.starts_with("OnEnter(")
        || label.starts_with("OnExit(")
        || label.starts_with("OnTransition")
}

/// Is this resource type path engine-owned (excluded from the trace)?
fn is_engine_path(name: &str) -> bool {
    name.starts_with("bevy") || name.starts_with("core::") || name.starts_with("alloc::")
}

/// Records a [`StateTrace`] from the driver loop.
pub struct StateTraceRecorder {
    pending: Arc<Mutex<Vec<String>>>,
    labels: Vec<String>,
    runs: Vec<ScheduleRun>,
    resources: BTreeMap<String, ResourceChange>,
    last_tick: Tick,
    names_masked: bool,
}

impl StateTraceRecorder {
    /// Install recorder systems into every state schedule of `app`.
    /// Must be called after the game built its app and before the
    /// first `update()`, so the initial `OnEnter` is captured.
    pub fn install(app: &mut App) -> Self {
        let pending: Arc<Mutex<Vec<String>>> = Arc::default();
        let mut labels = Vec::new();
        if let Some(mut schedules) = app.world_mut().get_resource_mut::<Schedules>() {
            for (label, schedule) in schedules.iter_mut() {
                let label = format!("{label:?}");
                if !is_state_schedule(&label) {
                    continue;
                }
                let sink = Arc::clone(&pending);
                let own = label.clone();
                schedule.add_systems(move || {
                    sink.lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .push(own.clone());
                });
                labels.push(label);
            }
        }
        labels.sort();
        labels.dedup();
        let last_tick = app.world().read_change_tick();
        Self {
            pending,
            labels,
            runs: Vec::new(),
            resources: BTreeMap::new(),
            last_tick,
            names_masked: false,
        }
    }

    /// Record what happened during `frame` (call right after `update()`).
    pub fn after_frame(&mut self, world: &World, frame: u64) {
        let ran: Vec<String> = std::mem::take(
            &mut *self
                .pending
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()),
        );
        self.runs
            .extend(ran.into_iter().map(|label| ScheduleRun { frame, label }));

        let now = world.read_change_tick();
        if frame > 0 {
            for (id, info, _) in world.iter_resources() {
                let name = info.name().to_string();
                if name.starts_with('<') {
                    // DebugName placeholder: bevy's `debug` feature is off.
                    self.names_masked = true;
                    continue;
                }
                if is_engine_path(&name) {
                    continue;
                }
                let Some(ticks) = world.get_resource_change_ticks_by_id(id) else {
                    continue;
                };
                if !ticks.is_changed(self.last_tick, now) {
                    continue;
                }
                self.resources
                    .entry(name.clone())
                    .and_modify(|r| {
                        r.last_frame = frame;
                        r.frames_changed += 1;
                        if r.change_frames.len() < MAX_CHANGE_FRAMES {
                            r.change_frames.push(frame);
                        }
                    })
                    .or_insert(ResourceChange {
                        name,
                        first_frame: frame,
                        last_frame: frame,
                        frames_changed: 1,
                        change_frames: vec![frame],
                    });
            }
        }
        self.last_tick = now;
    }

    /// Finish the trace.
    pub fn into_result(self) -> StateTrace {
        let ran: std::collections::BTreeSet<&str> =
            self.runs.iter().map(|r| r.label.as_str()).collect();
        let schedules_never_run = self
            .labels
            .iter()
            .filter(|l| !ran.contains(l.as_str()))
            .cloned()
            .collect();
        let mut notes = Vec::new();
        if self.labels.is_empty() {
            notes.push(
                "no OnEnter/OnExit/OnTransition schedules found: the game has no \
                 bevy_state states with enter/exit systems, so state progress is \
                 only visible through resource changes"
                    .to_string(),
            );
        }
        if self.names_masked {
            notes.push(
                "some resource names are masked (bevy `debug` feature off): those \
                 resources are not traced"
                    .to_string(),
            );
        }
        StateTrace {
            schedules_run: self.runs,
            schedules_never_run,
            resource_changes: self.resources.into_values().collect(),
            notes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_schedule_labels_are_recognized() {
        assert!(is_state_schedule("OnEnter(Settings)"));
        assert!(is_state_schedule("OnExit(Main)"));
        assert!(is_state_schedule("OnTransition { exited: A, entered: B }"));
        assert!(!is_state_schedule("Update"));
        assert!(!is_state_schedule("FixedUpdate"));
    }

    #[test]
    fn engine_resource_paths_are_excluded() {
        assert!(is_engine_path("bevy_time::time::Time"));
        assert!(is_engine_path("bevy_playtest::driver::RunViolations"));
        assert!(!is_engine_path("fixture_game_menu::DisplayQuality"));
    }
}
