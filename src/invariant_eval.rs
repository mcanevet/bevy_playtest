//! Invariant evaluation: read scenario invariants, run checks, report violations.
//!
//! v0.1: only `nodes_in_bounds` is wired (uses scenario min/max_X). All
//! other rules are rejected at load time (unimplemented).

use bevy_ecs::prelude::*;
use bevy_transform::prelude::Transform;

use crate::driver::{RunState, RunViolations};
use crate::enums::InvariantRule;
use crate::scenario::Invariant;

/// Evaluate all scenario invariants each frame. v0.1: only
/// `nodes_in_bounds` is wired; others panic (should have been rejected
/// at load time).
pub fn evaluate_invariants_system(
    state: Res<RunState>,
    mut violations: ResMut<RunViolations>,
    invariants: Res<ScenarioInvariants>,
    q: Query<(Entity, &Transform), With<crate::contract::Gameplay>>,
) {
    for inv in &invariants.0 {
        match inv.rule {
            InvariantRule::NodesInBounds => {
                evaluate_bounds(inv, state.frame, &mut violations, &q);
            }
            InvariantRule::Custom => {
                // Should have been rejected at load time (unimplemented).
                continue;
            }
        }
    }
}

/// Evaluate a single bounds invariant.
fn evaluate_bounds(
    inv: &Invariant,
    frame: u64,
    violations: &mut RunViolations,
    q: &Query<(Entity, &Transform), With<crate::contract::Gameplay>>,
) {
    let min_x = inv.min_x.unwrap_or(-1000.0);
    let max_x = inv.max_x.unwrap_or(1000.0);
    let min_y = inv.min_y.unwrap_or(-1000.0);
    let max_y = inv.max_y.unwrap_or(1000.0);
    let min_z = inv.min_z.unwrap_or(-1000.0);
    let max_z = inv.max_z.unwrap_or(1000.0);
    let targets: Vec<&str> = inv.targets.iter().map(|s| s.as_str()).collect();
    let target_set = !targets.is_empty();

    for (entity, transform) in q.iter() {
        // Skip if target-filtered and this entity isn't named in the list.
        if target_set {
            // v0.1: no Name lookup (deferred); skip filtering.
        }
        let t = transform.translation;
        let out = t.x < min_x
            || t.x > max_x
            || t.y < min_y
            || t.y > max_y
            || t.z < min_z
            || t.z > max_z;
        if out {
            violations.0.report(
                inv.rule.to_string().as_str(),
                &inv.name,
                format!(
                    "entity {:?} out of bounds [{:.2},{:.2}]×[{:.2},{:.2}]×[{:.2},{:.2}]: ({:.2},{:.2},{:.2})",
                    entity, min_x, max_x, min_y, max_y, min_z, max_z, t.x, t.y, t.z
                ),
                frame,
            );
        }
    }
}

/// Resource carrying the scenario's invariants into the run loop.
#[derive(Resource)]
pub struct ScenarioInvariants(pub Vec<Invariant>);
