//! Minimal oracle suite: bounds, NaN.
//!
//! Each oracle is a Bevy system reporting into the run's violations
//! resource. The driver registers them; scenario configuration gates
//! which families run (bounds/NaN run over all entities with a
//! zero-annotation query, so they stay meaningful on games with no
//! bevy_playtest-specific code). The other rule families — frozen-world
//! (soft-lock), panic capture, and log findings — live in
//! [`crate::driver`] / [`crate::sinks`]; see [`crate::rules`] for the
//! full registry and each rule's meaning.

use bevy_ecs::prelude::*;
use bevy_transform::prelude::Transform;

use crate::driver::{RunState, RunViolations};
use crate::rules::Rule;

/// Default play-space half-extent (±1000 on all axes), used when the
/// scenario declares no bounds. Declared bounds (the `nodes_in_bounds`
/// invariant) are checked strictly by `invariant_eval` instead.
pub const DEFAULT_BOUNDS_EXTENT: f32 = 1000.0;

/// How long (seconds of game time) an entity must keep moving AWAY
/// while outside the default bounds before it counts as a runaway.
pub const RUNAWAY_SECONDS: f32 = 0.5;

/// Default bounds oracle: flags RUNAWAY geometry. An entity is reported
/// when it is outside the default ±[`DEFAULT_BOUNDS_EXTENT`] box AND its
/// distance (largest absolute coordinate) grew on every frame of the
/// last [`RUNAWAY_SECONDS`].
///
/// Being far away is not enough: games routinely place level geometry
/// beyond ±1000 (thetawave's arena barrier sits at y = 2250), and a
/// default box cannot know a game's world size. What is always a bug is
/// something that keeps flying off and is never despawned, like a
/// leaked projectile. A shmup enemy that exits the screen and is
/// despawned shortly after is not flagged. A one-off teleport to a
/// fixed far position is not flagged either; declare bounds in the
/// scenario to check positions strictly.
pub fn bounds_oracle_system(
    state: Res<RunState>,
    mut violations: ResMut<RunViolations>,
    q: Query<(Entity, &Transform)>,
    // Per out-of-bounds entity: (last distance, consecutive outward frames).
    mut outward: Local<std::collections::HashMap<Entity, (f32, u32)>>,
) {
    let needed = ((state.tps.max(1) as f32) * RUNAWAY_SECONDS)
        .ceil()
        .max(1.0) as u32;
    let mut next = std::collections::HashMap::with_capacity(outward.len());
    for (entity, transform) in q.iter() {
        let t = transform.translation;
        if !t.is_finite() {
            continue; // NaN oracle owns this case
        }
        let distance = t.x.abs().max(t.y.abs()).max(t.z.abs());
        if distance <= DEFAULT_BOUNDS_EXTENT {
            continue;
        }
        let streak = match outward.get(&entity) {
            Some(&(last, streak)) if distance > last => streak + 1,
            _ => 0,
        };
        next.insert(entity, (distance, streak));
        if streak >= needed {
            violations.0.report(
                Rule::NodesInBounds.name(),
                &format!("{entity:?}"),
                format!(
                    "entity {entity:?} out of bounds and still moving away after \
                     {needed} frames: ({:.2}, {:.2}, {:.2})",
                    t.x, t.y, t.z
                ),
                state.frame,
            );
        }
    }
    *outward = next;
}

/// NaN oracle: all `Transform`s must hold finite values.
/// Queries ALL entities (zero-annotation) so the oracle is never
/// vacuous on games that use no bevy_playtest-specific code.
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
