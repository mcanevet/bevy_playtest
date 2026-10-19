//! Minimal oracle suite: bounds, NaN, liveness.
//!
//! Each oracle is a Bevy system reporting into the run's violations
//! resource. The driver registers them; scenario configuration gates
//! which families run (v0.1: bounds/NaN run when any Gameplay entity
//! exists; liveness runs when the scenario arms it).

use bevy_ecs::prelude::*;
use bevy_transform::prelude::Transform;

use crate::driver::{RunState, RunViolations};
use crate::rules::Rule;

/// Play-space bounds in half-extents (v0.1: hard-coded ±1000 on all
/// axes; scenario-configurable bounds arrive with the bounds invariant).
pub const DEFAULT_BOUNDS_EXTENT: f32 = 1000.0;

/// Bounds oracle: Gameplay entities must stay inside the play space.
/// Structural/UI entities (no Gameplay marker) are excluded — they
/// legitimately sit at origin.
pub fn bounds_oracle_system(
    state: Res<RunState>,
    mut violations: ResMut<RunViolations>,
    q: Query<(Entity, &Transform), With<crate::contract::Gameplay>>,
) {
    for (entity, transform) in q.iter() {
        let t = transform.translation;
        if !t.is_finite() {
            continue; // NaN oracle owns this case
        }
        if t.x.abs() > DEFAULT_BOUNDS_EXTENT
            || t.y.abs() > DEFAULT_BOUNDS_EXTENT
            || t.z.abs() > DEFAULT_BOUNDS_EXTENT
        {
            violations.0.report(
                Rule::NodesInBounds.name(),
                &format!("{entity:?}"),
                format!(
                    "entity {entity:?} out of bounds: ({:.2}, {:.2}, {:.2})",
                    t.x, t.y, t.z
                ),
                state.frame,
            );
        }
    }
}

/// NaN oracle: Gameplay transforms must hold finite values.
pub fn nan_oracle_system(
    state: Res<RunState>,
    mut violations: ResMut<RunViolations>,
    q: Query<(Entity, &Transform), With<crate::contract::Gameplay>>,
) {
    for (entity, transform) in q.iter() {
        if !transform.translation.is_finite() {
            violations.0.report(
                Rule::FiniteTransforms.name(),
                &format!("{entity:?}"),
                format!(
                    "entity {entity:?} has non-finite translation: {:?}",
                    transform.translation
                ),
                state.frame,
            );
        }
    }
}

/// Marker resource: games insert/keep this when responsive. Absence for
/// longer than the grace window trips the liveness oracle.
#[derive(Resource, Default)]
pub struct LivenessSignal {
    pub last_signal_frame: u64,
}

/// Liveness oracle: a scenario-armed check that SOME change keeps
/// happening (either a liveness signal ticked or a Transform changed).
#[derive(Resource)]
pub struct LivenessArmed {
    pub grace_frames: u64,
}

impl Default for LivenessArmed {
    fn default() -> Self {
        Self {
            grace_frames: 120, // 2s at 60 TPS
        }
    }
}

pub fn liveness_oracle_system(
    state: Res<RunState>,
    mut violations: ResMut<RunViolations>,
    armed: Option<Res<LivenessArmed>>,
    signal: Option<ResMut<LivenessSignal>>,
    q_changed: Query<(), Changed<Transform>>,
) {
    let Some(armed) = armed else { return };
    // Aliveness evidence: any changed Transform this frame.
    let world_changed = !q_changed.is_empty();
    if let Some(mut signal) = signal {
        if world_changed {
            signal.last_signal_frame = state.frame;
        }
        let silent = state.frame.saturating_sub(signal.last_signal_frame);
        if silent > armed.grace_frames {
            violations.0.report(
                Rule::FrozenWorld.name(),
                "world",
                format!(
                    "no liveness signal for {silent} frames (grace={}) — soft-lock?",
                    armed.grace_frames
                ),
                state.frame,
            );
        }
    } else if !world_changed {
        // No explicit signal resource: fall back to world-change checks,
        // tracked via the driver's frozen_frames counter.
    }
}
