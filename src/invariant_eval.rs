//! Invariant evaluation: read scenario invariants, run checks, report violations.
//!
//! Two rules are wired:
//! - `nodes_in_bounds` (scheduled system): AABB check on every entity
//!   with a Transform.
//! - `component_check` (driver-loop RECORDER, not a scheduled system):
//!   a numeric comparison on a reflected component field across the
//!   entities selected by registered type name — the authored-intent
//!   oracle ("holding KeyW must drive the camera forward"). It runs
//!   after `app.update()` in the driver loop (same doctrine as the
//!   replay/digest recorders): `&World` cannot be combined with
//!   `ResMut<RunViolations>` in one system, and no schedule placement
//!   can guarantee ordering after all of the game's own systems.
//!
//! Vacuity is NOT checked here: like the run-level `vacuous_run` rule,
//! it must be judged with any-frame observability (a game whose
//! entities spawn after frame 0 — loading screens — is NOT vacuous),
//! so the driver reports it after the run ends.

use bevy_ecs::prelude::*;
use bevy_ecs::reflect::AppTypeRegistry;
use bevy_reflect::TypeRegistry;
use bevy_transform::prelude::Transform;

use crate::driver::{RunState, RunViolations};
use crate::enums::{InvariantRule, TargetSelector};
use crate::scenario::Invariant;

/// Evaluate the `nodes_in_bounds` invariants each frame. The
/// `component_check` invariants are evaluated by
/// [`record_component_checks`] in the driver loop; this system skips
/// them (they never reached the schedule-only path — validation
/// rejects a component_check without a component selector).
pub fn evaluate_invariants_system(
    state: Res<RunState>,
    mut violations: ResMut<RunViolations>,
    invariants: Res<ScenarioInvariants>,
    q_all: Query<(Entity, &Transform)>,
) {
    for inv in &invariants.0 {
        if inv.rule != InvariantRule::NodesInBounds {
            continue;
        }
        let selector = inv.target_selector.clone().unwrap_or_default();
        match selector {
            TargetSelector::All => {
                evaluate_bounds(inv, state.frame, &mut violations, &mut q_all.iter());
            }
            TargetSelector::WithComponent { .. } => {
                // Validation rejects this combination at load; reaching
                // here means a future code path regressed the schema.
                unreachable!("nodes_in_bounds does not support WithComponent selectors");
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

/// Bookkeeping for `component_check` invariants across frames, kept
/// as a resource so the driver's post-run audit can judge vacuity
/// and deadlines with any-frame observability:
/// - `matched`: the selector matched at least one entity at some
///   frame during the run. An invariant that never matched anything
///   is vacuous — it proves nothing (the vacuous_invariant guard).
/// - `first_held_frame`: the first frame the predicate held for ALL
///   matched entities simultaneously. `eventually_s` invariants are
///   satisfied by this happening within the deadline (frames), not
///   merely before the run ends.
/// - `config_errored`: the invariant hit a resolution/config failure
///   at some frame (already reported as `oracle_error`). The audit
///   must NOT also emit vacuous/deadline entries for such an
///   invariant: those would tell a contradictory story (blaming the
///   game for what is an oracle-evaluability failure).
#[derive(Resource, Default)]
pub struct WatchedInvariants {
    /// Invariant name → matched ≥1 entity at least one frame.
    pub matched: std::collections::HashSet<String>,
    /// Invariant name → frame the predicate first held for every
    /// matched entity (drives `eventually_s` satisfaction; the entry
    /// is inserted once, at the earliest such frame).
    pub first_held_frame: std::collections::HashMap<String, u64>,
    /// Invariant name → the run saw a config/resolution error for
    /// this invariant (drives audit-entry suppression; the
    /// `oracle_error` itself is reported during the run).
    pub config_errored: std::collections::HashSet<String>,
}

/// One frame's evaluation result for a single `component_check`
/// invariant. Computed in the immutable (read-only world) phase; the
/// mutation phase turns it into bookkeeping and violations. Carrying
/// the outcome as owned data is what lets the read phase end all
/// world borrows before violations are written.
struct CcOutcome {
    /// Entities matched by the selector this frame.
    matched: usize,
    /// Whether the predicate held for every matched entity this frame.
    all_held: bool,
    /// First violating (entity, value) witness this frame, if any.
    example_bad: Option<(Entity, f64)>,
    /// Configuration/resolution failure (unknown component, bad
    /// path, non-numeric leaf...): reported instead of the check.
    config_error: Option<String>,
}

/// Driver-loop recorder for `component_check` invariants. Called
/// after `app.update()` every frame (see the module docs for why a
/// recorder and not a scheduled system).
///
/// `frame` is the number of the frame that just ran (matches
/// `RunState::frame` at this point in the loop).
pub fn record_component_checks(world: &mut World, frame: u64) {
    // Clone the component_check invariants out so NO world borrow is
    // held across the phases below (the read phase needs &World, the
    // write phase needs &mut World).
    let cc_invariants: Vec<Invariant> = world
        .get_resource::<ScenarioInvariants>()
        .map(|invariants| {
            invariants
                .0
                .iter()
                .filter(|i| i.rule == InvariantRule::ComponentCheck)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    if cc_invariants.is_empty() {
        return;
    }
    let Some(atr) = world.get_resource::<AppTypeRegistry>().cloned() else {
        // A run with component_checks but no type registry cannot
        // evaluate them: infrastructure failure, not a game bug.
        world.resource_scope(|world, mut watched: Mut<WatchedInvariants>| {
            let mut violations = world.resource_mut::<RunViolations>();
            for inv in &cc_invariants {
                watched.config_errored.insert(inv.name.clone());
                violations.0.report(
                    crate::rules::Rule::OracleError.name(),
                    &inv.name,
                    "the game has no AppTypeRegistry: reflected component checks are \
                     impossible (register types with App::register_type)"
                        .to_string(),
                    frame,
                );
            }
        });
        return;
    };

    // Read phase: everything below borrows the world immutably. The
    // registry read guard stays alive while TypeRegistration refs are
    // used; outcomes are owned so all borrows end before the write
    // phase.
    let registry = atr.0.read();
    let mut outcomes: Vec<CcOutcome> = Vec::new();
    for inv in &cc_invariants {
        outcomes.push(eval_one(world, &registry, inv));
    }
    drop(registry);

    // Write phase: turn outcomes into bookkeeping and violations. All
    // world borrows ended above; only owned data is used here.
    world.resource_scope(|world, mut watched: Mut<WatchedInvariants>| {
        let mut violations = world.resource_mut::<RunViolations>();
        for (inv, outcome) in cc_invariants.iter().zip(outcomes.iter()) {
            apply_outcome(inv, outcome, frame, &mut watched, &mut violations);
        }
    });
}

/// Fold one frame's outcome for one invariant into the watched
/// bookkeeping and (for always-mode violations) the run violations.
fn apply_outcome(
    inv: &Invariant,
    outcome: &CcOutcome,
    frame: u64,
    watched: &mut WatchedInvariants,
    violations: &mut RunViolations,
) {
    if let Some(why) = &outcome.config_error {
        // Remember the failure so the post-run audit stays silent
        // about this invariant: a config error already failed the
        // run; vacuous/deadline entries on top would misattribute
        // an author-side problem to the game (N-B/N-C from review).
        watched.config_errored.insert(inv.name.clone());
        violations.0.report(
            crate::rules::Rule::OracleError.name(),
            &inv.name,
            format!("invariant '{}': {why}", inv.name),
            frame,
        );
        return;
    }
    if outcome.matched == 0 {
        // Not necessarily vacuous: the component may spawn later.
        // The post-run audit decides with any-frame observability.
        return;
    }
    watched.matched.insert(inv.name.clone());
    if outcome.all_held {
        // Earliest-holding frame wins: insert only on the first
        // sighting so late frames cannot "restart the clock".
        watched
            .first_held_frame
            .entry(inv.name.clone())
            .or_insert(frame);
    } else if inv.eventually_s.is_none() {
        // Always-mode: report immediately. Eventually-mode misses
        // are judged only at the deadline (post-run audit).
        if let Some((entity, value)) = outcome.example_bad {
            let (path, check, threshold) =
                cc_fields(inv).unwrap_or(("", crate::enums::CheckOp::Le, 0.0));
            violations.0.report(
                crate::rules::Rule::ComponentCheck.name(),
                &inv.name,
                format!(
                    "{}.{} = {value:.3} on entity {entity:?} violates {} {threshold:.3} \
                     (path '{path}')",
                    selector_component_name(inv).unwrap_or("?"),
                    path,
                    check
                ),
                frame,
            );
        }
    }
}

/// Threshold from the scenario's reserved `value` field. Validated to
/// be a JSON number at load; the Option is defensive only.
fn cc_threshold(inv: &Invariant) -> Option<f64> {
    inv.value.as_ref().and_then(|v| v.as_f64())
}

/// (path, check, threshold) for a component_check invariant, if fully
/// specified (validation guarantees presence).
fn cc_fields(inv: &Invariant) -> Option<(&str, crate::enums::CheckOp, f64)> {
    Some((inv.path.as_deref()?, inv.check?, cc_threshold(inv)?))
}

fn selector_component_name(inv: &Invariant) -> Option<&str> {
    match inv.target_selector.as_ref()? {
        TargetSelector::WithComponent { name } => Some(name),
        TargetSelector::All => None,
    }
}

/// Immutable-phase evaluation of one component_check invariant.
fn eval_one(world: &World, registry: &TypeRegistry, inv: &Invariant) -> CcOutcome {
    let Some(name) = selector_component_name(inv) else {
        return CcOutcome {
            matched: 0,
            all_held: true,
            example_bad: None,
            config_error: Some("component_check requires a with_component target selector".into()),
        };
    };
    let Some((path, check, threshold)) = cc_fields(inv) else {
        return CcOutcome {
            matched: 0,
            all_held: true,
            example_bad: None,
            config_error: Some(
                "component_check requires 'path', 'check' and a numeric 'value'".into(),
            ),
        };
    };
    let registration = match resolve_registration(registry, name) {
        Ok(r) => r,
        Err(why) => {
            return CcOutcome {
                matched: 0,
                all_held: true,
                example_bad: None,
                config_error: Some(why),
            };
        }
    };
    let Some(reflect_component) = registration.data::<bevy_ecs::reflect::ReflectComponent>() else {
        return CcOutcome {
            matched: 0,
            all_held: true,
            example_bad: None,
            config_error: Some(format!(
                "type '{name}' is registered in the game's TypeRegistry but is not a \
                 reflected component (missing ReflectComponent data — is it a Resource?)"
            )),
        };
    };

    let mut outcome = CcOutcome {
        matched: 0,
        all_held: true,
        example_bad: None,
        config_error: None,
    };
    for entity_ref in world.iter_entities() {
        let Some(reflect) = reflect_component.reflect(entity_ref) else {
            continue;
        };
        outcome.matched += 1;
        // The selector component IDENTIFIES the entity; the numeric
        // leaf may live on it (e.g. a game component with a `hp`
        // field) or on the entity's Transform (the universal spatial
        // component). Try the selector component first, then the
        // spatial fallback — this lets identity components like
        // `Camera3d` select while the check reads `translation.z`.
        // The error names BOTH attempted roots so the author can see
        // which one (or both) failed and on WHICH entity.
        let value = reflect_numeric_path(reflect, path).or_else(|selector_err| {
            let tf_result = entity_ref
                .get::<Transform>()
                .map(|t| t as &dyn bevy_reflect::Reflect)
                .ok_or_else(|| format!("entity {:?} has no Transform", entity_ref.id()))
                .and_then(|tf| reflect_numeric_path(tf, path));
            match tf_result {
                Err(tf_err) => Err(format!(
                    "path '{path}' resolves on neither the selector component \
                     ({selector_err}) nor the entity's Transform ({tf_err}) \
                     [entity {:?}]",
                    entity_ref.id()
                )),
                ok => ok,
            }
        });
        match value {
            Ok(current) => {
                if !check.holds(current, threshold) {
                    outcome.all_held = false;
                    if outcome.example_bad.is_none() {
                        outcome.example_bad = Some((entity_ref.id(), current));
                    }
                }
            }
            Err(why) => {
                // Config error: the check as authored cannot be
                // evaluated for this entity. Reported instead of the
                // predicate result — but WITHOUT discarding the
                // witnesses already collected: they are folded into
                // the message so earlier entities' genuine
                // violations stay visible.
                let witness = match outcome.example_bad {
                    Some((entity, value)) => format!(
                        " (among them, entity {entity:?} = {value:.3} violates {} \
                         {threshold:.3})",
                        check
                    ),
                    None => String::new(),
                };
                let others = outcome.matched - 1;
                let other_word = if others == 1 { "entity" } else { "entities" };
                let violations_note = if outcome.example_bad.is_some() {
                    "with at least one violation"
                } else {
                    "with no violations"
                };
                outcome.config_error = Some(format!(
                    "{why}; note: {others} other matched {other_word} evaluated \
                     fine this frame {violations_note}{witness}"
                ));
                outcome.matched = 0;
                outcome.all_held = true;
                return outcome;
            }
        }
    }
    outcome
}

/// Resolve a component type name against the GAME's own registry.
/// Accepts the full registered type path, or the short path (final
/// segment) when it is unambiguous — a short name matching several
/// registered types is rejected loudly instead of silently picking
/// one, because the author's intent would be ambiguous.
fn resolve_registration<'a>(
    registry: &'a TypeRegistry,
    name: &str,
) -> Result<&'a bevy_reflect::TypeRegistration, String> {
    // Full-path lookup first: registry-keyed, unique by construction.
    if let Some(full) = registry.get_with_type_path(name) {
        return Ok(full);
    }
    // Short-path lookup: only when exactly one registered type has
    // this final path segment.
    let mut shorts = registry
        .iter()
        .filter(|r| r.type_info().type_path_table().short_path() == name);
    match (shorts.next(), shorts.next()) {
        (Some(one), None) => Ok(one),
        (None, _) => Err(format!(
            "unknown component '{name}' — not registered in this game's TypeRegistry \
             (components need #[derive(Reflect)] and App::register_type, or the name \
             must be the full registered type path)"
        )),
        (Some(first), Some(_)) => {
            let paths: Vec<String> = registry
                .iter()
                .filter(|r| r.type_info().type_path_table().short_path() == name)
                .map(|r| r.type_info().type_path_table().path().to_string())
                .collect();
            let _ = first;
            Err(format!(
                "component short name '{name}' is ambiguous among {} registered types — \
                 use the full type path (one of: {})",
                paths.len(),
                paths.join(", ")
            ))
        }
    }
}

/// Extract a numeric leaf from a reflected component via a dotted
/// reflect path (`translation.z`, `health`, `velocity.x`). Floats
/// widen to f64; integers convert losslessly; any other leaf type is
/// an error — never silently coerced to a number.
fn reflect_numeric_path(root: &dyn bevy_reflect::Reflect, path: &str) -> Result<f64, String> {
    use bevy_reflect::GetPath;
    let leaf: &dyn bevy_reflect::PartialReflect = root
        .reflect_path(path)
        .map_err(|e| format!("path '{path}' does not resolve on this component: {e}"))?;
    // Integer leaves: any width converts losslessly to f64 for the
    // magnitudes games use (up to 2^53).
    macro_rules! int_leaf {
        ($($t:ty),*) => { $(
            if let Some(v) = leaf.try_downcast_ref::<$t>() {
                return Ok(*v as f64);
            }
        )* };
    }
    if let Some(v) = leaf.try_downcast_ref::<f32>() {
        Ok(*v as f64)
    } else if let Some(v) = leaf.try_downcast_ref::<f64>() {
        Ok(*v)
    } else {
        int_leaf!(u8, u16, u32, u64, usize, i8, i16, i32, i64, isize);
        Err(format!(
            "path '{path}' resolves to non-numeric type '{}' — component_check compares \
             numeric leaves only",
            leaf.reflect_type_path()
        ))
    }
}

/// Post-run audit for `component_check` invariants, called by the
/// driver before the violation snapshot is finalized. Returns the
/// entries to append (already-shaped `ViolationEntry`s, same shape as
/// the driver's own post-run guards). Judges with any-frame
/// observability (mirrors the vacuous_run guard):
///
/// - An invariant that matched zero entities across the WHOLE run is
///   vacuous: it proved nothing (reported per invariant, under the
///   invariant's name, as vacuous_invariant).
/// - An `eventually_s` invariant that never held for all matched
///   entities at once missed its deadline: reported as a
///   component_check violation.
pub fn audit_component_checks(
    invariants: &[Invariant],
    watched: &WatchedInvariants,
    tps: u64,
    last_frame: u64,
) -> Vec<crate::state::ViolationEntry> {
    let mut entries = Vec::new();
    for inv in invariants
        .iter()
        .filter(|i| i.rule == InvariantRule::ComponentCheck)
    {
        // A config/resolution error was already reported during the
        // run (oracle_error, gating). Emitting a vacuous or deadline
        // entry here would blame the game for an author-side or
        // evaluability problem — contradictory report noise.
        if watched.config_errored.contains(&inv.name) {
            continue;
        }
        if !watched.matched.contains(&inv.name) {
            // If the run ALSO never had any observable entity, the
            // driver's run-level guard already reported every
            // invariant as vacuous: report the component_check one
            // anyway (named target), matching that guard's behavior
            // of reporting per invariant name.
            entries.push(vacuous_entry(&inv.name, last_frame));
            continue;
        }
        if let Some(deadline_s) = inv.eventually_s {
            let deadline_frames = (deadline_s * tps as f32).ceil() as u64;
            // The deadline is the temporal bound the author wrote:
            // satisfaction requires the predicate to have first held
            // for all matched entities AT or BEFORE this frame —
            // holding "sometime before the run ends" is NOT enough.
            let held_in_time = watched
                .first_held_frame
                .get(&inv.name)
                .is_some_and(|frame| *frame <= deadline_frames);
            if !held_in_time {
                let held_note = match watched.first_held_frame.get(&inv.name) {
                    Some(frame) => format!(
                        "the predicate first held only at frame {frame}, past the \
                         ~{deadline_frames}-frame deadline"
                    ),
                    None => "the predicate never held".to_string(),
                };
                entries.push(crate::state::ViolationEntry {
                    rule: crate::rules::Rule::ComponentCheck.name().to_string(),
                    target: inv.name.clone(),
                    first_frame: 0,
                    last_frame: deadline_frames.min(last_frame),
                    count: 1,
                    detail: format!(
                        "authored intent missed its {deadline_s}s deadline \
                         (~{deadline_frames} frames): {held_note}. The check was \
                         authored as game intent and the game failed to fulfill it \
                         in time"
                    ),
                    last_detail: format!("{held_note} within the {deadline_s}s deadline"),
                    meta: None,
                });
            }
        }
    }
    entries
}

fn vacuous_entry(name: &str, last_frame: u64) -> crate::state::ViolationEntry {
    crate::state::ViolationEntry {
        rule: crate::rules::Rule::VacuousInvariant.name().to_string(),
        target: name.to_string(),
        first_frame: 0,
        last_frame,
        count: 1,
        detail: format!(
            "invariant '{name}' matched zero entities over the whole run; \
             the check proves nothing until there is something to check"
        ),
        last_detail: format!("invariant '{name}' matched zero entities"),
        meta: None,
    }
}
