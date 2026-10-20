//! Minimal oracle suite: bounds, NaN.
//!
//! Each oracle is a Bevy system reporting into the run's violations
//! resource. The driver registers them; scenario configuration gates
//! which families run (v0.1: bounds/NaN run when any Gameplay entity

use bevy_ecs::prelude::*;
use bevy_transform::prelude::Transform;

use crate::driver::{RunState, RunViolations};
use crate::rules::Rule;

/// Play-space bounds in half-extents (v0.1: hard-coded ±1000 on all
/// axes; scenario-configurable bounds arrive with the bounds invariant).
pub const DEFAULT_BOUNDS_EXTENT: f32 = 1000.0;

/// Bounds oracle: all entities with `Transform` must stay inside the play space.
/// v0.1 queries ALL entities (no `Gameplay` marker requirement): static
/// UI/structural entities sit at or near the origin, which is within
/// bounds, so no false positives; games that adopt the marker can rely on
/// it later for narrowing. This keeps the oracle non-vacuous on showcase
/// games that never insert `Gameplay`.
pub fn bounds_oracle_system(
    state: Res<RunState>,
    mut violations: ResMut<RunViolations>,
    q: Query<(Entity, &Transform)>,
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

/// NaN oracle: all `Transform`s must hold finite values.
/// v0.1 queries ALL entities (no `Gameplay` marker requirement) so the
/// oracle is never vacuous on games that haven't adopted the contract.
pub fn nan_oracle_system(
    state: Res<RunState>,
    mut violations: ResMut<RunViolations>,
    q: Query<(Entity, &Transform)>,
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
