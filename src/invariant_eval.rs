//! Invariant evaluation: read scenario invariants, run checks, report violations.
//!
//! Only `nodes_in_bounds` is wired; other rules are rejected at load time.

use bevy_ecs::prelude::*;
use bevy_transform::prelude::Transform;

use crate::driver::{RunState, RunViolations};
use crate::scenario::Invariant;

/// Evaluate all scenario invariants each frame (only `nodes_in_bounds`
/// is wired; others panic — they should have been rejected at load time).
///
/// Vacuity is NOT checked here: like the run-level `vacuous_run` rule,
/// it must be judged with any-frame observability (a game whose
/// entities spawn after frame 0 — loading screens — is NOT vacuous),
/// so the driver reports it after the run ends.
pub fn evaluate_invariants_system(
    state: Res<RunState>,
    mut violations: ResMut<RunViolations>,
    invariants: Res<ScenarioInvariants>,
    q_all: Query<(Entity, &Transform)>,
) {
    for inv in &invariants.0 {
        let selector = inv.target_selector.clone().unwrap_or_default();
        match selector {
            crate::enums::TargetSelector::All => {
                evaluate_bounds(inv, state.frame, &mut violations, &mut q_all.iter());
            }
        }
    }
}

/// Evaluate a single bounds invariant.
fn evaluate_bounds(
    inv: &Invariant,
    frame: u64,
    violations: &mut RunViolations,
    transforms: &mut dyn Iterator<Item = (Entity, &Transform)>,
) {
    let min_x = inv.min_x.unwrap_or(-1000.0);
    let max_x = inv.max_x.unwrap_or(1000.0);
    let min_y = inv.min_y.unwrap_or(-1000.0);
    let max_y = inv.max_y.unwrap_or(1000.0);
    let min_z = inv.min_z.unwrap_or(-1000.0);
    let max_z = inv.max_z.unwrap_or(1000.0);
    for (entity, transform) in transforms {
        let t = transform.translation;
        let out =
            t.x < min_x || t.x > max_x || t.y < min_y || t.y > max_y || t.z < min_z || t.z > max_z;
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
