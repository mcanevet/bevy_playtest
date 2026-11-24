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
//! - `resource_check` (driver-loop RECORDER): the resource sibling
//!   of `component_check` — the same predicate vocabulary, applied to
//!   a numeric leaf of a named reflected RESOURCE. Resources are
//!   game-wide singletons (scores, economies, clocks), so there is
//!   exactly one value per frame, never a per-entity sweep.
//! - `stagnant_resource` (driver-loop RECORDER): the rate-form
//!   liveness invariant — a watched leaf on a reflected resource must
//!   change at least once per `window_s` DRIVER-LEAF units while the
//!   driver leaf strictly advances. A scoreboard frozen on a running
//!   match clock is a livelock (a 0–0 scoreboard), and no
//!   change/state oracle can see it: the world churns, the clock
//!   advances, only the VALUE under judgment stands still. The window
//!   is measured in the DRIVER LEAF'S OWN UNITS (a match clock in
//!   seconds makes `window_s` literal seconds of game time) — the
//!   harness never converts through `tps`, so FixedUpdate-heavy or
//!   time-scale-warped games measure honestly in their own clock.
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

/// Report a static authoring failure (`oracle_error`) once per
/// distinct error class, folding recurrence into the entry instead
/// of inflating `count` (observed: one typo'd path produced
/// `count: 300` on a 5 s run — the mechanism behind a 1200-count
/// report on a 20 s run elsewhere). A config error is a property of
/// the SCENARIO, not of time: `count` stays 1 and the entry's
/// `last_frame` advances so the report still shows the failure's
/// full span.
///
/// `error_class` is the message with dynamic values elided (digits,
/// entity ids, frames — the interpolations that differ between
/// frames of the SAME mistake) via [`error_class_of`]. Words, path
/// strings, type names and punctuation carry the mistake's
/// identity, so two genuinely different mistakes on one invariant
/// stay two findings, each reported once.
fn report_config_error_once(
    violations: &mut crate::state::Violations,
    seen: &mut std::collections::HashSet<String>,
    inv_name: &str,
    detail: &str,
    frame: u64,
) {
    let key = format!("{inv_name}\u{1}{}", error_class_of(detail));
    if !seen.insert(key) {
        // Recurrence of the same error class: fold. Push last_frame
        // forward so the entry spans the failure; leave count and
        // first_detail alone — they already tell the story once.
        if let Some(entry) = violations.entries.get_mut(&(
            crate::rules::Rule::OracleError.name().to_string(),
            inv_name.to_string(),
        )) {
            entry.last_frame = entry.last_frame.max(frame);
        }
        return;
    }
    violations.report(
        crate::rules::Rule::OracleError.name(),
        inv_name,
        detail.to_string(),
        frame,
    );
}

/// Elide the dynamic interpolations from an `oracle_error` detail so
/// the SAME authoring mistake produces ONE error class across the
/// frames it recurs on (the dedup key for
/// [`report_config_error_once`]). Digits (numbers, thresholds,
/// offsets), entity ids and frame indices all vary between frames
/// of one mistake; words, path strings, type names and punctuation
/// carry the mistake's identity. Alphabetic tokens are preserved
/// exactly; every maximal run of digits becomes a single `#`
/// placeholder (so `entity 3v1` and `entity 12v4` are one class,
/// while any word difference yields a different class).
fn error_class_of(detail: &str) -> String {
    let mut class = String::with_capacity(detail.len());
    let mut in_digits = false;
    for c in detail.chars() {
        if c.is_ascii_digit() {
            if !in_digits {
                class.push('#');
                in_digits = true;
            }
        } else {
            in_digits = false;
            class.push(c);
        }
    }
    class
}

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
                // Handled by the driver-loop recorder
                // [`record_bounds_with_component`]: resolving a
                // component TYPE NAME dynamically requires `&World`
                // registry access, which cannot coexist with
                // `ResMut<RunViolations>` in one system (same doctrine
                // as the component_check recorder).
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
    /// (invariant name, error class) pairs already reported as
    /// `oracle_error` this run — the once-per-distinct-error dedup
    /// that keeps static authoring failures from inflating `count`
    /// to the frame count. Recorded via the shared report-once
    /// helper for `oracle_error` entries.
    pub oracle_error_seen: std::collections::HashSet<String>,
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

/// Driver-loop recorder for `nodes_in_bounds` invariants with a
/// `with_component` selector: same doctrine as
/// [`record_component_checks`] — the selector names a component TYPE
/// that must be resolved dynamically against the game's registry,
/// which needs `&World` and cannot coexist with
/// `ResMut<RunViolations>` in a scheduled system.
///
/// Frames where the named component is absent or unresolved do not
/// count; vacuity is judged by the post-run audit
/// ([`audit_component_checks`]) with any-frame observability.
pub fn record_bounds_with_component(world: &mut World, frame: u64) {
    // Clone the relevant invariants out so no world borrow survives
    // the phase split below (read phase: &World; write phase: &mut).
    let invariants: Vec<Invariant> = world
        .get_resource::<ScenarioInvariants>()
        .map(|invariants| {
            invariants
                .0
                .iter()
                .filter(|i| {
                    i.rule == InvariantRule::NodesInBounds
                        && matches!(
                            i.target_selector,
                            Some(crate::enums::TargetSelector::WithComponent { .. })
                        )
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    if invariants.is_empty() {
        return;
    }
    let Some(atr) = world.get_resource::<AppTypeRegistry>().cloned() else {
        world.resource_scope(|world, mut watched: Mut<WatchedInvariants>| {
            let mut violations = world.resource_mut::<RunViolations>();
            for inv in &invariants {
                watched.config_errored.insert(inv.name.clone());
                let why = &no_atr_message("with_component bounds checks");
                report_config_error_once(
                    &mut violations.0,
                    &mut watched.oracle_error_seen,
                    &inv.name,
                    why,
                    frame,
                );
            }
        });
        return;
    };

    // Read phase (all world access immutable; registry guard lives
    // only as long as the outcome data below is owned).
    let registry = atr.0.read();
    // Per invariant: (config error, checkable-entity count, out-of-
    // bounds witnesses). `checkable` — entities carrying the named
    // component AND a Transform — drives vacuity judgment in the
    // audit; it is tracked INDEPENDENTLY of witnesses so an invariant
    // whose scope held the whole run is not misjudged as vacuous.
    type BoundsFinding<'a> = (
        &'a Invariant,
        Option<String>,
        usize,
        Vec<(Entity, [f32; 3])>,
    );
    let mut findings: Vec<BoundsFinding> = Vec::with_capacity(invariants.len());
    for inv in &invariants {
        let Some(crate::enums::TargetSelector::WithComponent { name }) =
            inv.target_selector.as_ref()
        else {
            continue;
        };
        let bounds = bounds_of(inv);
        match resolve_registration(&registry, name, "component") {
            Ok(type_reg) => {
                // Iterate matching entities: the type's ComponentId
                // must exist in this world (only registered-component
                // types get one), then every archetype row carrying it
                // is a match (exact, like the digest's per-type counts).
                let Some(component_id) = world.components().get_valid_id(type_reg.type_id()) else {
                    findings.push((inv, None, 0, vec![])); // Not instantiated: vacuous.
                    continue;
                };
                let mut checkable = 0usize;
                let mut witnesses: Vec<(Entity, [f32; 3])> = Vec::new();
                for archetype in world.archetypes().iter() {
                    if !archetype.components().contains(&component_id) {
                        continue;
                    }
                    for entry in archetype.entities() {
                        let entity = entry.id();
                        let Ok(entity_ref) = world.get_entity(entity) else {
                            continue;
                        };
                        let Some(transform) = entity_ref.get::<Transform>() else {
                            continue; // Carries the marker but no Transform: not checkable.
                        };
                        checkable += 1;
                        let t = transform.translation;
                        let [mn, mx] = bounds;
                        let out = t.x < mn[0]
                            || t.x > mx[0]
                            || t.y < mn[1]
                            || t.y > mx[1]
                            || t.z < mn[2]
                            || t.z > mx[2];
                        if out {
                            witnesses.push((entity, [t.x, t.y, t.z]));
                        }
                    }
                }
                findings.push((inv, None, checkable, witnesses));
            }
            Err(problem) => findings.push((inv, Some(problem), 0, vec![])),
        }
    }
    drop(registry);

    // Write phase: report out-of-bounds entities + track vacuity.
    world.resource_scope(|world, mut watched: Mut<WatchedInvariants>| {
        let mut violations = world.resource_mut::<RunViolations>();
        for (inv, config_error, checkable, witnesses) in findings {
            if let Some(why) = config_error {
                watched.config_errored.insert(inv.name.clone());
                report_config_error_once(
                    &mut violations.0,
                    &mut watched.oracle_error_seen,
                    &inv.name,
                    &format!("invariant '{}': {why}", inv.name),
                    frame,
                );
                continue;
            }
            // Selector observability: at least one CHECKABLE entity
            // (named component + Transform) existed at some frame.
            // Independent of witnesses — an invariant whose scope held
            // the whole run is not vacuous, exactly like
            // component_check's `matched`.
            if checkable > 0 {
                watched.matched.insert(inv.name.clone());
            }
            for (entity, [x, y, z]) in witnesses {
                let [mn, mx] = bounds_of(inv);
                violations.0.report(
                    crate::rules::Rule::NodesInBounds.name(),
                    &inv.name,
                    format!(
                        "entity {entity:?} out of bounds [{:.2},{:.2}]x[{:.2},{:.2}]x[{:.2},{:.2}] \
                         via with_component '{}': ({x:.2},{y:.2},{z:.2})",
                        mn[0], mx[0], mn[1], mx[1], mn[2], mx[2],
                        selector_component_name(inv).unwrap_or("?")
                    ),
                    frame,
                );
            }
        }
    });
}

/// Min/max bounds per axis from the invariant (mirrors
/// [`evaluate_bounds`]'s defaults).
fn bounds_of(inv: &Invariant) -> [[f32; 3]; 2] {
    [
        [
            inv.min_x.unwrap_or(-1000.0),
            inv.min_y.unwrap_or(-1000.0),
            inv.min_z.unwrap_or(-1000.0),
        ],
        [
            inv.max_x.unwrap_or(1000.0),
            inv.max_y.unwrap_or(1000.0),
            inv.max_z.unwrap_or(1000.0),
        ],
    ]
}

// ---------------------------------------------------------------------------
// Reflected-resource access (resource_check / stagnant_resource)
// ---------------------------------------------------------------------------

/// Resolve a reflected resource by registered type name and return a
/// `&dyn Reflect` view of its current value.
///
/// Resources are found via the same Bevy 0.20 unified lookup the
/// digest uses (`collect_resource_reflections_locked`): the
/// registration's `ComponentId` → the world's `resource_entities()`
/// map → the resource entity → `ReflectComponent::reflect` on that
/// entity. In Bevy 0.20 a reflected resource is DEFINED by its
/// reflected-component registration (`#[derive(Reflect)]` +
/// `#[reflect(Resource)]`), so this path is the only truthful one.
/// Name resolution follows the component rules (full registered path,
/// or an unambiguous short path) via [`resolve_registration`].
///
/// Errors are author-side CONFIG failures reported by the callers as
/// `oracle_error` — an unevaluable authored check is an author-side
/// failure, never a game bug. A resource that is properly reflected
/// but simply ABSENT this frame yields [`ResourceView::Absent`]:
/// the harness defers judgment to the post-run vacuity audit (same
/// doctrine as component_check's zero-matched-entities frames).
#[derive(Debug)]
pub(crate) enum ResourceView<'a> {
    /// The resource's current value.
    Value(&'a dyn bevy_reflect::Reflect),
    /// Properly reflected as a Resource, but not inserted (yet):
    /// nothing to observe this frame, not an error.
    Absent,
}

pub(crate) fn reflect_resource<'a>(
    world: &'a World,
    registry: &TypeRegistry,
    name: &str,
) -> Result<ResourceView<'a>, String> {
    let registration = resolve_registration(registry, name, "resource")?;
    // ReflectResource is only a MARKER in Bevy 0.20 (reading goes
    // through ReflectComponent on the resource entity), but it is
    // exactly the datum that separates "the author named a resource
    // that has not arrived yet" from "the author named something that
    // is not a resource at all" (e.g. a plain Component).
    if registration
        .data::<bevy_ecs::reflect::ReflectResource>()
        .is_none()
    {
        return Err(format!(
            "type '{name}' is registered but is not a reflected Resource — a resource \
             needs #[derive(Reflect)] with #[reflect(Resource)] (named types that are \
             Components or plain data cannot be checked as resources)"
        ));
    }
    let reflect_component = registration
        .data::<bevy_ecs::reflect::ReflectComponent>()
        .ok_or_else(|| {
            format!(
                "type '{name}' is registered as a Resource but has no ReflectComponent \
                 data (Bevy 0.20 reads reflected resources through the component \
                 registration); register the type with app.register_type"
            )
        })?;
    let Some(component_id) = world.components().get_valid_id(registration.type_id()) else {
        // Registered in the game's TypeRegistry, but never inserted
        // as a resource in this world (or inserted and removed). A
        // late-arriving resource is legitimate; the audit decides
        // vacuity if it NEVER arrives.
        return Ok(ResourceView::Absent);
    };
    let Some(resource_entity) = world.resource_entities().get(component_id) else {
        return Ok(ResourceView::Absent);
    };
    let Ok(entity_ref) = world.get_entity(resource_entity) else {
        return Ok(ResourceView::Absent);
    };
    match reflect_component.reflect(entity_ref) {
        Some(value) => Ok(ResourceView::Value(value)),
        // The resource entity exists but no longer holds the
        // component: the game REMOVED the resource mid-run (a legal
        // runtime event — level teardown, returning to menu; Bevy
        // leaves the resource_entities entry behind after
        // remove_resource). Same doctrine as a late arrival: not an
        // author error, not a game finding — the post-run audit
        // decides vacuity if it never comes back.
        None => Ok(ResourceView::Absent),
    }
}

/// Bookkeeping for `resource_check` / `stagnant_resource` invariants
/// across frames (same doctrine as [`WatchedInvariants`]: post-run
/// audits need any-frame observability and full-series history).
#[derive(Resource, Default)]
pub struct WatchedResources {
    /// Invariant name → the resource resolved and the leaf was read
    /// at least once during the run. Otherwise the invariant is
    /// vacuous — it proved nothing.
    pub matched: std::collections::HashSet<String>,
    /// resource_check: name → first frame the predicate held (drives
    /// `eventually_s` satisfaction; the earliest frame wins).
    pub first_held_frame: std::collections::HashMap<String, u64>,
    /// stagnant_resource: name → last observed (watched value, driver
    /// value) pair. Updated every observed frame; the delta of the
    /// driver column feeds the window budget.
    pub last_values: std::collections::HashMap<String, (f64, f64)>,
    /// stagnant_resource: name → driver-leaf advance accumulated
    /// since the watched leaf last changed (the budget consumed by
    /// the current stagnation episode).
    pub driver_elapsed: std::collections::HashMap<String, f64>,
    /// stagnant_resource: name → TOTAL driver-leaf advance observed
    /// over the WHOLE run — a running SUM of every consumed forward
    /// delta, deliberately not last-minus-first (gap re-seeding
    /// resets episodes, so endpoint subtraction undercounts). Drives
    /// the post-run elision audit (`window_unelapsed`): a window the
    /// driver never passed was unjudgeable for this run.
    pub driver_total_advance: std::collections::HashMap<String, f64>,
    /// stagnant_resource: name → the current stagnation episode has
    /// already been reported (cleared when the watched leaf changes;
    /// one finding per episode, never per frame).
    pub fired: std::collections::HashSet<String>,
    /// stagnant_resource: name → the watched leaf resolved at least
    /// once even though a full (watched, driver) pair never did (the
    /// driver resource never arrived). Distinguishes "driver never
    /// observed" from "nothing was observed" in the vacuity audit.
    pub watched_resolved: std::collections::HashSet<String>,
    /// stagnant_resource: name → the last frame was not fully
    /// observed (watched and/or driver resource absent). The window
    /// math cannot span the gap: the next fully-observed frame must
    /// re-seed (fresh window) instead of folding a cross-gap delta.
    pub driver_gaps: std::collections::HashSet<String>,
    /// Invariant name → a resolution/config failure was seen during
    /// the run (already reported as `oracle_error`); the audit must
    /// not also emit vacuous/deadline entries for it (N-B/N-C
    /// doctrine: never blame the game for an author-side problem).
    pub config_errored: std::collections::HashSet<String>,
    /// (invariant name, error class) pairs already reported as
    /// `oracle_error` this run — the once-per-distinct-error dedup
    /// that keeps static authoring failures from inflating `count`
    /// to the frame count. Recorded via the shared report-once
    /// helper for `oracle_error` entries.
    pub oracle_error_seen: std::collections::HashSet<String>,
}

/// One frame's outcome for one `resource_check` invariant. Owned data
/// ends all world borrows before violations are written (same
/// phase-split doctrine as [`CcOutcome`]).
struct RcOutcome {
    /// The resource resolved and the leaf was read.
    matched: bool,
    /// Predicate verdict this frame (meaningful only when matched).
    held: bool,
    /// Current leaf value (report evidence).
    value: Option<f64>,
    /// Resolution/config failure, reported instead of the check.
    config_error: Option<String>,
}

/// Driver-loop recorder for `resource_check` invariants: the
/// authored-predicate tier over reflected resources (always, or
/// eventually within a deadline) — the resource sibling of
/// [`record_component_checks`]. Resources are game-wide singletons
/// (scores, economies, cooldowns), so there is exactly one value per
/// frame, never a per-entity sweep.
pub fn record_resource_checks(world: &mut World, frame: u64) {
    // Clone the invariants out so no world borrow survives the phase
    // split below (read phase: &World; write phase: resource_scope).
    let invariants: Vec<Invariant> = world
        .get_resource::<ScenarioInvariants>()
        .map(|invariants| {
            invariants
                .0
                .iter()
                .filter(|i| i.rule == InvariantRule::ResourceCheck)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    if invariants.is_empty() {
        return;
    }
    let Some(atr) = world.get_resource::<AppTypeRegistry>().cloned() else {
        world.resource_scope(|world, mut watched: Mut<WatchedResources>| {
            let mut violations = world.resource_mut::<RunViolations>();
            for inv in &invariants {
                watched.config_errored.insert(inv.name.clone());
                let why = &no_atr_message("reflected resource checks");
                report_config_error_once(
                    &mut violations.0,
                    &mut watched.oracle_error_seen,
                    &inv.name,
                    why,
                    frame,
                );
            }
        });
        return;
    };

    // Read phase: all world access immutable; outcomes are owned.
    let registry = atr.0.read();
    let mut outcomes: Vec<RcOutcome> = Vec::with_capacity(invariants.len());
    for inv in &invariants {
        outcomes.push(eval_resource_check(world, &registry, inv));
    }
    drop(registry);

    // Write phase: fold outcomes into bookkeeping and violations.
    world.resource_scope(|world, mut watched: Mut<WatchedResources>| {
        world.resource_scope(|world, mut held: Mut<crate::held::HeldBookkeeping>| {
            let mut violations = world.resource_mut::<RunViolations>();
            for (inv, outcome) in invariants.iter().zip(outcomes.iter()) {
                apply_rc_outcome(
                    inv,
                    outcome,
                    frame,
                    &mut watched,
                    &mut violations,
                    &mut held,
                );
            }
        });
    });
}

/// Read-phase evaluation of one `resource_check` invariant.
fn eval_resource_check(world: &World, registry: &TypeRegistry, inv: &Invariant) -> RcOutcome {
    let err = |why: String| RcOutcome {
        matched: false,
        held: true,
        value: None,
        config_error: Some(why),
    };
    // Validation guarantees these; the Options are defensive only.
    let Some(name) = inv.resource.as_deref() else {
        return err("resource_check requires 'resource' (validated)".into());
    };
    let Some(path) = inv.path.as_deref() else {
        return err("resource_check requires 'path' (validated)".into());
    };
    let Some(check) = inv.check else {
        return err("resource_check requires 'check' (validated)".into());
    };
    let Some(threshold) = cc_threshold(inv) else {
        return err("resource_check requires a numeric 'value' (validated)".into());
    };
    let resource = match reflect_resource(world, registry, name) {
        Ok(ResourceView::Value(r)) => r,
        Ok(ResourceView::Absent) => {
            // Late-arriving resource: nothing to observe this frame;
            // the post-run audit decides vacuity with any-frame
            // observability.
            return RcOutcome {
                matched: false,
                held: true,
                value: None,
                config_error: None,
            };
        }
        Err(why) => return err(why),
    };
    let value = match reflect_numeric_path(resource, path, "resource") {
        Ok(v) => v,
        Err(why) => return err(why),
    };
    RcOutcome {
        matched: true,
        held: check.holds(value, threshold),
        value: Some(value),
        config_error: None,
    }
}

/// Write-phase fold of one frame's outcome (mirrors `apply_outcome`).
fn apply_rc_outcome(
    inv: &Invariant,
    outcome: &RcOutcome,
    frame: u64,
    watched: &mut WatchedResources,
    violations: &mut RunViolations,
    held: &mut crate::held::HeldBookkeeping,
) {
    if let Some(why) = &outcome.config_error {
        watched.config_errored.insert(inv.name.clone());
        // Scenario-property doctrine: once per distinct error class,
        // never per frame (the dogfooded finding — one typo'd
        // resource_check path produced `count: 300` on a 5 s run;
        // the same mechanism scaled to 1200 on a 20 s run).
        report_config_error_once(
            &mut violations.0,
            &mut watched.oracle_error_seen,
            &inv.name,
            &format!("invariant '{}': {why}", inv.name),
            frame,
        );
        return;
    }
    if !outcome.matched {
        // Not necessarily vacuous: the resource may appear later
        // (inserted after a loading state). The post-run audit
        // decides with any-frame observability.
        return;
    }
    watched.matched.insert(inv.name.clone());
    // Held-invariant evidence (passing-invariant trace): a pure read of this
    // frame's observation — no verdict logic, nothing consumed.
    // Config-errored and unresolved frames never reach here, so the
    // extrema and resolution count describe exactly the frames the
    // audit would call observable.
    if let Some(value) = outcome.value {
        held.observe_resource_leaf(&inv.name, value, frame);
    }
    if outcome.held {
        // Earliest-holding frame wins: late frames cannot restart the
        // clock (same doctrine as component_check).
        watched
            .first_held_frame
            .entry(inv.name.clone())
            .or_insert(frame);
    } else if inv.eventually_s.is_none() {
        // Always-mode: report immediately. Eventually-mode misses are
        // judged only at the deadline (post-run audit).
        let path = inv.path.as_deref().unwrap_or("?");
        let check = inv.check.unwrap_or(crate::enums::CheckOp::Le);
        let threshold = cc_threshold(inv).unwrap_or(f64::NAN);
        let value = outcome.value.unwrap_or(f64::NAN);
        violations.0.report(
            crate::rules::Rule::ResourceCheck.name(),
            &inv.name,
            format!(
                "{}.{path} = {value:.3} violates {check} {threshold:.3}",
                inv.resource.as_deref().unwrap_or("?"),
            ),
            frame,
        );
    }
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
                let why = &no_atr_message("reflected component checks");
                report_config_error_once(
                    &mut violations.0,
                    &mut watched.oracle_error_seen,
                    &inv.name,
                    why,
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
        // A config error is a property of the scenario, not of
        // time: report the first occurrence per distinct error
        // class, never once per frame (finding: per-frame reporting
        // inflated `count` to the frame count — 300+ per typo).
        report_config_error_once(
            &mut violations.0,
            &mut watched.oracle_error_seen,
            &inv.name,
            &format!("invariant '{}': {why}", inv.name),
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
pub(crate) fn cc_threshold(inv: &Invariant) -> Option<f64> {
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
    let registration = match resolve_registration(registry, name, "component") {
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
            config_error: Some(not_a_reflected_component_message(name)),
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
        let value = resolve_entity_numeric_path(reflect, &entity_ref, path);
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
///
/// `noun` names WHAT the author was resolving — "component" for
/// per-entity selectors (component_check / nodes_in_bounds),
/// "resource" for reflected resources (resource_check /
/// stagnant_resource). The author of a typo'd resource name must
/// not be told to register a component; the caller owns the noun
/// for the same reason `reflect_numeric_path` takes one.
pub(crate) fn resolve_registration<'a>(
    registry: &'a TypeRegistry,
    name: &str,
    noun: &str,
) -> Result<&'a bevy_reflect::TypeRegistration, String> {
    // Full-path lookup first: registry-keyed, unique by construction.
    if let Some(full) = registry.get_with_type_path(name) {
        return Ok(full);
    }
    // Short-path lookup: only when exactly one registered type has
    // this final path segment.
    let paths: Vec<&bevy_reflect::TypeRegistration> = registry
        .iter()
        .filter(|r| r.type_info().type_path_table().short_path() == name)
        .collect();
    match paths.as_slice() {
        [one] => Ok(*one),
        [] => {
            let mut msg = format!(
                "unknown {noun} '{name}' — not registered in this game's TypeRegistry \
                 ({noun}s need #[derive(Reflect)] and App::register_type, or the name \
                 must be the full registered type path)"
            );
            // Did-you-mean: the short-name scan above is exact-match
            // only, so "Time" is technically "unknown" (not
            // "ambiguous") even though Time<()>, Time<Real>,
            // Time<Virtual> and Time<Fixed> are all sitting right
            // there. Typos ("Kils" for "Kills", "Playre" for
            // "Player") get a Levenshtein ≤ 2 suggestion — the same
            // capped-distance helper the config validator uses for
            // key/button names — with a substring fallback for
            // prefixes and truncations ("Kill" for "Kills"). Purely
            // advisory: resolution semantics never change.
            let shorts: Vec<String> = registry
                .iter()
                .map(|r| r.type_info().type_path_table().short_path().to_string())
                .collect();
            // A full path that failed lookup may be a full-path TYPO
            // (`my_game::Reserve` for `my_game::Reservoir`) — the
            // error message itself recommends full paths, so leaving
            // them suggestion-less is worse than odd. Compare the
            // FINAL SEGMENT against the registered short names the
            // same way; a module typo (`my_gane::Reservoir`) matches
            // by segment and still suggests. When the queried name is
            // a full path, a correct full-path suggestion beats a
            // bare short name — offer the full path of the best
            // candidate. Generic ARGS are stripped before segment
            // extraction (`Time<…Virtuall>` probes as `Time`, not
            // `Virtuall>`) so the suggestion is the CONTAINER — the
            // container's spelling is the only string an author can
            // paste back into the field verbatim; the argument alone
            // is not a valid resource name.
            let base = match name.find('<') {
                Some(open) => &name[..open],
                None => name,
            };
            let (probe, full_probe) = match base.rsplit_once("::") {
                Some((_, last)) => (last, true),
                None => (base, false),
            };
            let full_for = |short: &str| {
                registry
                    .iter()
                    .find(|r| r.type_info().type_path_table().short_path() == short)
                    .map(|r| r.type_info().type_path_table().path().to_string())
                    .unwrap_or_else(|| short.to_string())
            };
            // Prefix/truncation first: a name that is a PREFIX of
            // several registered types ("Time" for Time<()>,
            // Time<Real>, …) is best served by listing those exact
            // generic spellings — an unrelated Levenshtein near-match
            // ("Timer") would shadow the author's real target. Rank
            // candidates that START with the probe ahead of mere
            // containment ("Time" lists the Time<...> containers
            // before Timer/TimedAnimationEvent), so the take-cap
            // spends its slots on the closest kin.
            let mut similar: Vec<String> = shorts
                .iter()
                .filter(|short| short.contains(probe))
                .cloned()
                .collect();
            // Rank: candidates whose OWN name segment (container name
            // — after the last `::`, before any `<args>`) matches the
            // probe exactly (`Time<...>` for probe "Time") outrank
            // longer names that merely extend it ("Timer",
            // "TimedAnimationEvent") — the author who typed `Time`
            // means a Time, and the generic spellings are the exact
            // strings to paste. Registry order breaks ties (stable
            // sort).
            fn own_segment(s: &str) -> &str {
                let container = s.split('<').next().unwrap_or(s);
                container.rsplit("::").next().unwrap_or(container)
            }
            similar.sort_by_key(|short| own_segment(short) != probe);
            similar.truncate(4);
            if !similar.is_empty() {
                let shown: Vec<String> = if full_probe {
                    similar.iter().map(|s| full_for(s)).collect()
                } else {
                    similar
                };
                msg.push_str(&format!(
                    "; known similar registered types: {}",
                    shown.join(", ")
                ));
            } else if let Some(best) = crate::scenario::suggest_similar(probe, &shorts) {
                // Pure typo ("Kils" for "Kills"): no substring hit,
                // but the capped-distance helper the config
                // validator uses for keys/buttons catches it.
                let best = if full_probe { full_for(&best) } else { best };
                msg.push_str(&format!("; did you mean '{best}'?"));
            }
            Err(msg)
        }
        many => {
            let full: Vec<String> = many
                .iter()
                .map(|r| r.type_info().type_path_table().path().to_string())
                .collect();
            Err(format!(
                "{noun} short name '{name}' is ambiguous among {} registered types — \
                 use the full type path (one of: {})",
                full.len(),
                full.join(", ")
            ))
        }
    }
}

/// Extract a numeric leaf from a reflected value via a dotted
/// reflect path (`translation.z`, `health`, `velocity.x`). Floats
/// widen to f64; integers convert losslessly; any other leaf type is
/// an error — never silently coerced to a number.
///
/// `noun` names WHAT the path resolves against in the error message —
/// "component" for per-entity component roots (component_check and
/// the with_component bounds recorder), "resource" for reflected
/// resources (resource_check / stagnant_resource). Dogfood finding:
/// resource_check errors used to say "does not resolve on this
/// component" — true for two of the three call sites, misleading for
/// the resource ones. The caller owns the noun; there is no
/// blanket default to drift again.
/// The message the run's oracles emit when the game has no
/// AppTypeRegistry, parameterized by the tier of checks it makes
/// impossible. One builder shared by every emitter (resource oracle,
/// component oracle, bounds recorder, lint's registry probe) so the
/// wording cannot drift between them. `tier` phrases to complete
/// "the game has no AppTypeRegistry: {tier} are impossible …" —
/// e.g. "reflected resource checks", "with_component bounds checks".
pub(crate) fn no_atr_message(tier: &str) -> String {
    format!(
        "the game has no AppTypeRegistry: {tier} are \
         impossible (register types with App::register_type)"
    )
}

/// The message for a type that IS registered but lacks component
/// reflection data. Shared by the component oracle and lint's
/// registry probe so the wording cannot drift.
pub(crate) fn not_a_reflected_component_message(name: &str) -> String {
    format!(
        "type '{name}' is registered in the game's TypeRegistry but is not a \
         reflected component (missing ReflectComponent data — is it a Resource?)"
    )
}

/// Resolve a numeric path on a matched entity: selector component
/// first, Transform fallback — the doctrine BOTH the run's
/// component oracle and lint's registry probe follow, extracted so
/// the message (which names both attempted roots AND the entity)
/// cannot drift between them.
pub(crate) fn resolve_entity_numeric_path(
    reflect: &dyn bevy_reflect::Reflect,
    entity_ref: &bevy_ecs::world::EntityRef,
    path: &str,
) -> Result<f64, String> {
    reflect_numeric_path(reflect, path, "component").or_else(|selector_err| {
        let tf_result = entity_ref
            .get::<Transform>()
            .map(|t| t as &dyn bevy_reflect::Reflect)
            .ok_or_else(|| format!("entity {:?} has no Transform", entity_ref.id()))
            .and_then(|tf| reflect_numeric_path(tf, path, "component"));
        match tf_result {
            Err(tf_err) => Err(format!(
                "path '{path}' resolves on neither the selector component \
                 ({selector_err}) nor the entity's Transform ({tf_err}) \
                 [entity {:?}]",
                entity_ref.id()
            )),
            ok => ok,
        }
    })
}

pub(crate) fn reflect_numeric_path(
    root: &dyn bevy_reflect::Reflect,
    path: &str,
    noun: &str,
) -> Result<f64, String> {
    use bevy_reflect::GetPath;
    let leaf: &dyn bevy_reflect::PartialReflect = root
        .reflect_path(path)
        .map_err(|e| format!("path '{path}' does not resolve on this {noun}: {e}"))?;
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
            "path '{path}' resolves to non-numeric type '{}' — checks over \
             this {noun} compare numeric leaves only",
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
    for inv in invariants.iter().filter(|i| {
        i.rule == InvariantRule::ComponentCheck
            || (i.rule == InvariantRule::NodesInBounds
                && matches!(
                    i.target_selector,
                    Some(crate::enums::TargetSelector::WithComponent { .. })
                ))
    }) {
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

// ---------------------------------------------------------------------------
// stagnant_resource: the rate-form livelock invariant
// ---------------------------------------------------------------------------

/// One frame's outcome for one `stagnant_resource` invariant (same
/// owned-data phase-split doctrine).
struct SrOutcome {
    /// The watched leaf's numeric value (present when both the
    /// resource and the watched path resolved).
    watched: Option<f64>,
    /// The driver leaf's numeric value (present when the driver
    /// resource+path resolved).
    driver: Option<f64>,
    /// First config error this frame, if any.
    config_error: Option<String>,
}

/// Driver-loop recorder for `stagnant_resource` invariants: the
/// rate-form livelock catcher. A watched numeric leaf on a reflected
/// resource must CHANGE at least once per `window_s`-worth of driver
/// leaf advance. A scoreboard frozen on a running match clock is a
/// livelock — the world churns, the clock advances, only the value
/// under judgment stands still — and no motion/state oracle can see
/// it.
///
/// Window mechanics (all in the DRIVER LEAF'S OWN UNITS — the
/// harness never converts through `tps`, so the author's clock
/// domain is authoritative):
/// - Each observed frame records the (watched, driver) pair. The
///   driver DELTA since the previous observed frame is added to the
///   stagnation budget when the watched leaf is UNCHANGED, and
///   RESET to zero when it changes (budget reset, episode ends).
/// - When the budget exceeds `window_s`, the violation is reported
///   ONCE per episode (one finding per stagnation episode, never
///   per frame — same dedup-by-target discipline as the oracles).
///   The watched leaf changing later clears the episode and starts
///   a fresh window.
/// - A NON-ADVANCING driver freezes the window: time stopped, so
///   stagnation is no longer provable (pause screens, victory
///   states) and the check holds its peace until time resumes.
///   A paused clock may legitimately hold a frozen scoreboard.
/// - A RETREATING driver (delta < 0) resets the budget: clock
///   rewinds (seek, replay) rather than time flowing forward.
pub fn record_stagnant_resources(world: &mut World, frame: u64) {
    // Clone the invariants out so no world borrow survives the phase
    // split below.
    let invariants: Vec<Invariant> = world
        .get_resource::<ScenarioInvariants>()
        .map(|invariants| {
            invariants
                .0
                .iter()
                .filter(|i| i.rule == InvariantRule::StagnantResource)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    if invariants.is_empty() {
        return;
    }
    let Some(atr) = world.get_resource::<AppTypeRegistry>().cloned() else {
        world.resource_scope(|world, mut watched: Mut<WatchedResources>| {
            let mut violations = world.resource_mut::<RunViolations>();
            for inv in &invariants {
                watched.config_errored.insert(inv.name.clone());
                let why = &no_atr_message("reflected resource checks");
                report_config_error_once(
                    &mut violations.0,
                    &mut watched.oracle_error_seen,
                    &inv.name,
                    why,
                    frame,
                );
            }
        });
        return;
    };

    // Read phase: resolve both leaves for every invariant.
    let registry = atr.0.read();
    let mut outcomes: Vec<SrOutcome> = Vec::with_capacity(invariants.len());
    for inv in &invariants {
        outcomes.push(eval_stagnant_pair(world, &registry, inv));
    }
    drop(registry);

    // Write phase: fold into bookkeeping and violations.
    world.resource_scope(|world, mut watched: Mut<WatchedResources>| {
        world.resource_scope(|world, mut held: Mut<crate::held::HeldBookkeeping>| {
            let mut violations = world.resource_mut::<RunViolations>();
            for (inv, outcome) in invariants.iter().zip(outcomes.iter()) {
                apply_sr_outcome(
                    inv,
                    outcome,
                    frame,
                    &mut watched,
                    &mut violations,
                    &mut held,
                );
            }
        });
    });
}

/// Read-phase evaluation of one `stagnant_resource` invariant:
/// resolve the watched leaf (on `resource`) and the driver leaf (on
/// `driver_resource`, defaulting to `resource`).
fn eval_stagnant_pair(world: &World, registry: &TypeRegistry, inv: &Invariant) -> SrOutcome {
    let err = |watched: Option<f64>, driver: Option<f64>, why: Option<String>| SrOutcome {
        watched,
        driver,
        config_error: why,
    };
    // Validation guarantees these; the Options are defensive only.
    let Some(resource_name) = inv.resource.as_deref() else {
        return err(
            None,
            None,
            Some("stagnant_resource requires 'resource' (validated)".into()),
        );
    };
    let Some(watched_path) = inv.path.as_deref() else {
        return err(
            None,
            None,
            Some("stagnant_resource requires 'path' (validated)".into()),
        );
    };
    let Some(driver_path) = inv.driver_path.as_deref() else {
        return err(
            None,
            None,
            Some("stagnant_resource requires 'driver_path' (validated)".into()),
        );
    };
    let driver_resource_name = inv.driver_resource.as_deref().unwrap_or(resource_name);
    let resource = match reflect_resource(world, registry, resource_name) {
        Ok(ResourceView::Value(r)) => r,
        Ok(ResourceView::Absent) => {
            // Late-arriving resource: no observation this frame; the
            // post-run audit decides vacuity.
            return SrOutcome {
                watched: None,
                driver: None,
                config_error: None,
            };
        }
        Err(why) => return err(None, None, Some(why)),
    };
    let watched = match reflect_numeric_path(resource, watched_path, "resource") {
        Ok(v) => v,
        Err(why) => return err(None, None, Some(why)),
    };
    let driver = if driver_resource_name == resource_name {
        // Same resource: already resolved.
        match reflect_numeric_path(resource, driver_path, "resource") {
            Ok(v) => v,
            Err(why) => return err(Some(watched), None, Some(why)),
        }
    } else {
        let driver_resource = match reflect_resource(world, registry, driver_resource_name) {
            Ok(ResourceView::Value(r)) => r,
            Ok(ResourceView::Absent) => {
                // The driver resource is absent this frame (late
                // arrival, mid-run removal, teardown): the watched
                // sample CANNOT be judged — no driver means no
                // provable window math. Return it UNconsumed so the
                // fold marks a gap; the next fully-observed frame
                // starts a fresh window instead of folding a
                // cross-gap delta (finding: gaps corrupted budgets and
                // hid changes).
                return SrOutcome {
                    watched: Some(watched),
                    driver: None,
                    config_error: None,
                };
            }
            Err(why) => return err(Some(watched), None, Some(why)),
        };
        match reflect_numeric_path(driver_resource, driver_path, "resource") {
            Ok(v) => v,
            Err(why) => return err(Some(watched), None, Some(why)),
        }
    };
    SrOutcome {
        watched: Some(watched),
        driver: Some(driver),
        config_error: None,
    }
}

/// Write-phase fold of one frame's stagnation outcome.
fn apply_sr_outcome(
    inv: &Invariant,
    outcome: &SrOutcome,
    frame: u64,
    watched: &mut WatchedResources,
    violations: &mut RunViolations,
    held: &mut crate::held::HeldBookkeeping,
) {
    if let Some(why) = &outcome.config_error {
        watched.config_errored.insert(inv.name.clone());
        report_config_error_once(
            &mut violations.0,
            &mut watched.oracle_error_seen,
            &inv.name,
            &format!("invariant '{}': {why}", inv.name),
            frame,
        );
        return;
    }
    let (Some(watched_value), Some(driver_value)) = (outcome.watched, outcome.driver) else {
        // Partial observation (driver absent, watched present): mark
        // a GAP — the stagnation window math cannot span it. Also
        // record that the watched leaf DID resolve, so the vacuity
        // audit can distinguish "driver never observed" from
        // "nothing was observed". Fully absent (watched None): a
        // possibly late-arriving resource, also a gap.
        if outcome.watched.is_some() {
            watched.watched_resolved.insert(inv.name.clone());
        }
        watched.driver_gaps.insert(inv.name.clone());
        return;
    };
    watched.matched.insert(inv.name.clone());
    // Held-invariant evidence (passing-invariant trace): resolution count
    // plus run-total driver advance. The delta fed here mirrors the
    // window state machine's own attribution exactly — computed
    // BEFORE it consumes the observed values, so gaps and re-seeds
    // are excluded from the total by the same rules.
    let held_delta: Option<f64> = if watched.driver_gaps.contains(&inv.name) {
        // Gap-resume frame: continuity broken, no attributable
        // advance (the state machine re-seeds below).
        None
    } else {
        watched
            .last_values
            .get(&inv.name)
            .map(|(_, last_driver)| driver_value - last_driver)
    };
    held.observe_stagnant_frame(&inv.name, held_delta);
    let window_s = inv.window_s.map(|w| w as f64).unwrap_or(f64::INFINITY);
    if !watched_value.is_finite() || !driver_value.is_finite() {
        // A non-finite sample — NaN or infinity in EITHER leaf — is
        // an arithmetic fault in the game and an unattributable
        // observation. Letting it through poisons the state machine
        // from both sides: a NaN watched leaf reads as a "change"
        // every frame (NaN != NaN) and permanently disarms the
        // window; a NaN driver seeds a NaN baseline so every later
        // delta is NaN and the window freezes forever in silence.
        // Folding across the frame would also credit driver advance
        // that was never witnessed. Judge it loudly (once) and treat
        // the frame as a GAP: the next clean sample re-seeds a fresh
        // window (same doctrine as a resource-absence gap).
        watched.driver_gaps.insert(inv.name.clone());
        if !watched.config_errored.contains(&inv.name) {
            watched.config_errored.insert(inv.name.clone());
            let (owner, path) = if !watched_value.is_finite() {
                (
                    inv.resource.as_deref().unwrap_or("?"),
                    inv.path.as_deref().unwrap_or("?"),
                )
            } else {
                (
                    inv.driver_resource
                        .as_deref()
                        .unwrap_or_else(|| inv.resource.as_deref().unwrap_or("?")),
                    inv.driver_path.as_deref().unwrap_or("?"),
                )
            };
            violations.0.report(
                crate::rules::Rule::OracleError.name(),
                &inv.name,
                format!(
                    "invariant '{}': {} leaf {}.{} went non-finite — the \
                     stagnation check cannot judge it (investigate the game's \
                     arithmetic: division by zero, uninitialized accumulator). \
                     Later non-finite samples on either leaf are covered by \
                     this one finding.",
                    inv.name,
                    if !watched_value.is_finite() {
                        "watched"
                    } else {
                        "driver"
                    },
                    owner,
                    path
                ),
                frame,
            );
        }
        return;
    }
    if watched.driver_gaps.remove(&inv.name) {
        // A prior frame was only partially observed: continuity is
        // broken. Whatever happened during the gap is unknowable —
        // changes may have been missed, driver advance is not
        // attributable. Start a FRESH window from this frame instead
        // of folding a cross-gap delta (a falsely large budget, a
        // falsely claimed "stayed X for Y" episode).
        watched
            .last_values
            .insert(inv.name.clone(), (watched_value, driver_value));
        watched.driver_elapsed.remove(&inv.name);
        watched.fired.remove(&inv.name);
        return;
    }
    match watched.last_values.get_mut(&inv.name) {
        Some((last_watched, last_driver)) => {
            let driver_delta = driver_value - *last_driver;
            if watched_value != *last_watched {
                // Change observed: episode over, budget reset, a
                // fresh window begins this frame. Forward driver
                // motion still counts toward the run total (the
                // elision audit compares the run's NET forward
                // advance against the window — forward deltas
                // accumulate across episodes and retreats do not
                // debit the total, so an oscillating driver can
                // reach the window on paper without ever passing it
                // within one continuous stretch; the stagnation
                // oracle itself resets on retreat, so a value that
                // keeps moving never fires regardless).
                if driver_delta > 0.0 {
                    *watched
                        .driver_total_advance
                        .entry(inv.name.clone())
                        .or_insert(0.0) += driver_delta;
                }
                *last_watched = watched_value;
                *last_driver = driver_value;
                watched.driver_elapsed.remove(&inv.name);
                watched.fired.remove(&inv.name);
            } else {
                match driver_delta.partial_cmp(&0.0) {
                    Some(std::cmp::Ordering::Greater) => {
                        // Forward time: consume budget. Overshoot past
                        // window_s is irrelevant once the finding is
                        // out — one report per episode. The delta
                        // also feeds the run-total advance (the
                        // elision-audit accumulator — an honest sum,
                        // never endpoint subtraction).
                        let budget = watched
                            .driver_elapsed
                            .entry(inv.name.clone())
                            .or_insert(0.0);
                        *budget += driver_delta;
                        *watched
                            .driver_total_advance
                            .entry(inv.name.clone())
                            .or_insert(0.0) += driver_delta;
                        // Consume the observed driver value: the next
                        // frame's delta must be measured from HERE.
                        *last_driver = driver_value;
                        if *budget > window_s && !watched.fired.contains(&inv.name) {
                            watched.fired.insert(inv.name.clone());
                            violations.0.report(
                                crate::rules::Rule::StagnantResource.name(),
                                &inv.name,
                                format!(
                                    "{}.{path} stayed {value:.3} for {elapsed:.3} of \
                                     driver advance on {driver_res}.{driver_path}",
                                    inv.resource.as_deref().unwrap_or("?"),
                                    path = inv.path.as_deref().unwrap_or("?"),
                                    value = *last_watched,
                                    elapsed = *budget,
                                    driver_res = inv
                                        .driver_resource
                                        .as_deref()
                                        .unwrap_or_else(|| inv.resource.as_deref().unwrap_or("?")),
                                    driver_path = inv.driver_path.as_deref().unwrap_or("?"),
                                ),
                                frame,
                            );
                        }
                    }
                    Some(std::cmp::Ordering::Less) => {
                        // Retreating driver (seek/replay): rewind
                        // implies time is not flowing forward; treat
                        // as a fresh window.
                        *last_watched = watched_value;
                        *last_driver = driver_value;
                        watched.driver_elapsed.remove(&inv.name);
                        watched.fired.remove(&inv.name);
                    }
                    _ => {
                        // Non-advancing driver (delta exactly 0):
                        // time stopped, window frozen. (A non-finite
                        // sample never reaches the state machine —
                        // guarded above as an unattributable gap.) A
                        // paused clock may hold a frozen scoreboard
                        // (the deliberate distinction from
                        // frozen-world).
                    }
                }
            }
        }
        None => {
            // First observed frame of the window.
            watched
                .last_values
                .insert(inv.name.clone(), (watched_value, driver_value));
            watched.driver_elapsed.remove(&inv.name);
            watched.fired.remove(&inv.name);
        }
    }
}

/// Post-run audit for `resource_check` and `stagnant_resource`
/// invariants (mirrors [`audit_component_checks`]): full-run-history
/// judgments the per-frame recorders cannot make.
///
/// - An invariant whose resource/leaf never resolved across the
///   whole run is vacuous (reported as `vacuous_invariant`, per
///   invariant name).
/// - A `resource_check` with `eventually_s` that never held within
///   the deadline: `resource_check` violation, deadline-miss shape
///   (same message doctrine as component_check).
///
/// `stagnant_resource` needs no deadline audit: its verdict is
/// complete per episode (fired episodes were reported during the
/// run; episodes still under budget at run end simply never reached
/// the bar — which is the property holding, not a miss).
pub fn audit_resource_checks(
    invariants: &[Invariant],
    watched: &WatchedResources,
    tps: u64,
    last_frame: u64,
) -> Vec<crate::state::ViolationEntry> {
    let mut entries = Vec::new();
    for inv in invariants.iter().filter(|i| {
        i.rule == InvariantRule::ResourceCheck || i.rule == InvariantRule::StagnantResource
    }) {
        // Config errors were already reported during the run; emitting
        // vacuous/deadline entries on top would blame the game for an
        // author-side/evaluability problem (N-B/N-C doctrine).
        if watched.config_errored.contains(&inv.name) {
            continue;
        }
        if !watched.matched.contains(&inv.name) {
            if inv.rule == InvariantRule::StagnantResource
                && watched.watched_resolved.contains(&inv.name)
            {
                // The watched leaf resolved; the DRIVER never did.
                // Factually distinct from full vacuity — and
                // deliberately different in verdict from a PRESENT
                // but non-advancing driver (freeze semantics: a
                // paused clock holds the invariant's peace). An
                // absent driver proves nothing either way, so the
                // run still fails as unproven, but the message must
                // name the real culprit.
                let driver = inv
                    .driver_resource
                    .as_deref()
                    .unwrap_or_else(|| inv.resource.as_deref().unwrap_or("?"));
                let driver_path = inv.driver_path.as_deref().unwrap_or("?");
                entries.push(crate::state::ViolationEntry {
                    rule: crate::rules::Rule::VacuousInvariant.name().to_string(),
                    target: inv.name.clone(),
                    first_frame: 0,
                    last_frame,
                    count: 1,
                    detail: format!(
                        "invariant '{}' watched its leaf resolve but its driver \
                         {driver}.{driver_path} never did — no window could elapse, \
                         so stagnation is unprovable (a driver that is PRESENT but \
                         frozen legitimately holds the invariant's peace; one that \
                         never arrives proves nothing)",
                        inv.name
                    ),
                    last_detail: format!(
                        "invariant '{}': driver {driver}.{driver_path} never resolved",
                        inv.name
                    ),
                    meta: None,
                });
            } else {
                entries.push(vacuous_resource_entry(&inv.name, last_frame));
            }
            continue;
        }
        if inv.rule == InvariantRule::ResourceCheck {
            if let Some(deadline_s) = inv.eventually_s {
                let deadline_frames = (deadline_s * tps as f32).ceil() as u64;
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
                        rule: crate::rules::Rule::ResourceCheck.name().to_string(),
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
        if inv.rule == InvariantRule::StagnantResource {
            // Post-run elision audit (the fallback when no rate is
            // declared): the window is consumed in
            // DRIVER-LEAF units, so it is judgeable only if the
            // driver's TOTAL advance over the run reached the
            // window. It never did → the stagnation verdict is
            // unjudgeable FOR THIS RUN — an INFORMATIONAL note
            // (window_unelapsed), never a failure: a driver that
            // legitimately rests or runs slower than assumed is
            // game behavior, not a bug, and the frozen-driver
            // doctrine keeps such passes silent (a deliberate
            // choice: do NOT
            // reuse the gating vacuous_invariant rule here).
            // Not reached for config-errored invariants (suppressed
            // above) nor for fired ones (a fired episode means the
            // window DID elapse).
            let total_advance = watched
                .driver_total_advance
                .get(&inv.name)
                .copied()
                .unwrap_or(0.0);
            let window = inv.window_s.unwrap_or(0.0) as f64;
            if !watched.fired.contains(&inv.name) && total_advance < window && total_advance >= 0.0
            {
                let driver = inv
                    .driver_resource
                    .as_deref()
                    .unwrap_or_else(|| inv.resource.as_deref().unwrap_or("?"));
                let driver_path = inv.driver_path.as_deref().unwrap_or("?");
                entries.push(crate::state::ViolationEntry {
                    rule: crate::rules::Rule::WindowUnelapsed.name().to_string(),
                    target: inv.name.clone(),
                    first_frame: 0,
                    last_frame,
                    count: 1,
                    detail: if total_advance == 0.0 {
                        format!(
                            "invariant '{}': window ({window} driver units) never elapsed — \
                             the driver '{driver}.{driver_path}' never advanced over the \
                             whole run (frozen or paused the entire time; if that is \
                             expected game behavior this note can be ignored) — the \
                             check was unjudgeable (otherwise: lengthen the run or \
                             shrink the window)",
                            inv.name
                        )
                    } else {
                        format!(
                            "invariant '{}': window ({window} driver units) never elapsed — \
                             the driver '{driver}.{driver_path}' advanced only \
                             {total_advance:.3} units over the whole run; the check was \
                             unjudgeable (lengthen the run or shrink the window)",
                            inv.name
                        )
                    },
                    last_detail: format!(
                        "invariant '{}': driver advanced {total_advance:.3} of the \
                         {window}-unit window",
                        inv.name
                    ),
                    meta: None,
                });
            }
        }
    }
    entries
}

/// Vacuity entry for a resource invariant whose leaf never resolved.
fn vacuous_resource_entry(name: &str, last_frame: u64) -> crate::state::ViolationEntry {
    crate::state::ViolationEntry {
        rule: crate::rules::Rule::VacuousInvariant.name().to_string(),
        target: name.to_string(),
        first_frame: 0,
        last_frame,
        count: 1,
        detail: format!(
            "invariant '{name}' never resolved its resource/leaf over the whole run; \
             the check proves nothing until there is something to check"
        ),
        last_detail: format!("invariant '{name}' never resolved its resource/leaf"),
        meta: None,
    }
}

// ---------------------------------------------------------------------------
// pairwise_separation: the coincident-bodies spatial oracle
// ---------------------------------------------------------------------------

/// Bookkeeping for `pairwise_separation` invariants (same doctrine
/// as [`WatchedInvariants`]/[`WatchedResources`]): the per-frame
/// recorder owns one [`crate::episode::EpisodeFolder`] per invariant, and
/// the post-run vacuity audit needs the full-series observability
/// flags.
#[derive(Resource, Default)]
pub struct PairwiseBookkeeping {
    /// Invariant name → the selector resolved AND a pair was
    /// enumerable in at least one observed frame (≥2 checkable
    /// entities: fewer can never violate). Otherwise the invariant
    /// is vacuous — a pairwise rule with nothing to pairwise
    /// proves nothing.
    pub matched: std::collections::HashSet<String>,
    /// Invariant name → a resolution/config failure was seen during
    /// the run (already reported as `oracle_error`); the audit
    /// suppresses its vacuity entry so the game is not blamed for
    /// an author-side problem (N-B/N-C doctrine).
    pub config_errored: std::collections::HashSet<String>,
    /// (invariant name, error class) pairs already reported as
    /// `oracle_error` this run — the once-per-distinct-error dedup
    /// (same doctrine as [`WatchedResources::oracle_error_seen`]).
    pub oracle_error_seen: std::collections::HashSet<String>,
    /// Invariant name → the fold engine.
    pub folders: std::collections::HashMap<String, crate::episode::EpisodeFolder>,
}

/// One frame's pairwise outcome for one invariant (owned-data
/// phase-split doctrine: computed under `&World`, applied after
/// the borrow ends).
struct PwOutcome {
    /// The frame's violating pairs (empty = a clean frame, which
    /// ends any open episode).
    pairs: Vec<crate::episode::Observation<crate::episode::PairKey>>,
    /// The driver-leaf sample in the driver's own unit; `None` on
    /// a gap (driver resource absent / leaf non-finite).
    driver: Option<f64>,
    /// First config error this frame, if any.
    config_error: Option<String>,
    /// The selector resolved AND ≥2 checkable entities co-existed
    /// this frame.
    pairable: bool,
}

/// Driver-loop recorder for `pairwise_separation` invariants: the
/// coincident-bodies spatial oracle. Selects every entity carrying
/// the named registered component and requires every PAIR of
/// selected entities to stay at least `min_distance` apart; frames
/// with violating pairs fold into EPISODES (maximal stretches over
/// time — see [`crate::episode`]) and an episode becomes ONE finding when
/// its `window_s` soak elapses (measured in the declared driver
/// leaf's own units; the frame clock in seconds when no driver is
/// declared).
///
/// Enumeration is `O(n²)` over the selector's entities — the author
/// bounds the cost by choosing the selector (that is why "all" is
/// rejected at load: pairwising every `Transform` in the world —
/// walls, camera, UI — is never what an author means). A sampling
/// stride stays TBD per the design doc's cost-model carve-out;
/// this recorder enumerates exhaustively.
///
/// Soak semantics follow `stagnant_resource`'s window doctrine
/// exactly (never converted through `tps`): forward driver deltas
/// credit the span, retreats advance no budget, and a gap
/// (missing/non-finite driver sample) CLOSES the open episode
/// rather than extending it across an unobservable clock.
/// Sub-soak episodes are discarded as transient fly-bys — the ε
/// doctrine: `min_distance` is "coincident bodies", not "personal
/// space", so the soak filters legitimate fly-bys and only
/// sustained overlap becomes a finding.
pub fn record_pairwise_separation(world: &mut World, frame: u64, tps: u64) {
    // Clone the relevant invariants out so no world borrow survives
    // the phase split below.
    let invariants: Vec<Invariant> = world
        .get_resource::<ScenarioInvariants>()
        .map(|invariants| {
            invariants
                .0
                .iter()
                .filter(|i| i.rule == InvariantRule::PairwiseSeparation)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    if invariants.is_empty() {
        return;
    }
    let Some(atr) = world.get_resource::<AppTypeRegistry>().cloned() else {
        // No registry at all: the same definite infra error the
        // other reflected oracles front-load — one `oracle_error`
        // per invariant, once per distinct error class.
        world.resource_scope(|world, mut book: Mut<PairwiseBookkeeping>| {
            let mut violations = world.resource_mut::<RunViolations>();
            for inv in &invariants {
                book.config_errored.insert(inv.name.clone());
                let why = &no_atr_message("pairwise separation checks");
                report_config_error_once(
                    &mut violations.0,
                    &mut book.oracle_error_seen,
                    &inv.name,
                    why,
                    frame,
                );
            }
        });
        return;
    };
    let registry = atr.read();

    // Read phase: enumerate under &World, no mutable borrows.
    let mut outcomes: Vec<(Invariant, PwOutcome)> = Vec::with_capacity(invariants.len());
    for inv in &invariants {
        outcomes.push((
            inv.clone(),
            eval_pairwise(world, &registry, inv, frame, tps),
        ));
    }
    drop(registry);

    // Write phase: fold, judge soak, report one finding per
    // soaked episode.
    world.resource_scope(|world, mut book: Mut<PairwiseBookkeeping>| {
        let mut violations = world.resource_mut::<RunViolations>();
        for (inv, outcome) in outcomes {
            apply_pw_outcome(&inv, &outcome, frame, &mut book, &mut violations.0);
        }
    });
}

/// Enumerate the frame's violating pairs for one invariant (the
/// read phase).
fn eval_pairwise(
    world: &World,
    registry: &TypeRegistry,
    inv: &Invariant,
    frame: u64,
    tps: u64,
) -> PwOutcome {
    let err = |why: String| PwOutcome {
        pairs: vec![],
        driver: None,
        config_error: Some(why),
        pairable: false,
    };
    // Validation guarantees these; the Options are defensive only.
    let Some(crate::enums::TargetSelector::WithComponent { name }) = inv.target_selector.as_ref()
    else {
        return err("pairwise_separation requires a with_component selector (validated)".into());
    };
    let Some(min_distance) = inv.min_distance else {
        return err("pairwise_separation requires 'min_distance' (validated)".into());
    };
    let type_reg = match resolve_registration(registry, name, "component") {
        Ok(t) => t,
        Err(why) => return err(why),
    };
    // Collect (entity index, translation) for every entity carrying
    // the marker AND a Transform — the bounds recorder's doctrine
    // ("carries the marker but no Transform: not checkable").
    let mut nodes: Vec<(u32, [f32; 3])> = Vec::new();
    if let Some(component_id) = world.components().get_valid_id(type_reg.type_id()) {
        for archetype in world.archetypes().iter() {
            if !archetype.components().contains(&component_id) {
                continue;
            }
            for entry in archetype.entities() {
                let Ok(entity_ref) = world.get_entity(entry.id()) else {
                    continue;
                };
                let Some(transform) = entity_ref.get::<Transform>() else {
                    continue;
                };
                nodes.push((
                    entity_ref.id().index().index(),
                    [
                        transform.translation.x,
                        transform.translation.y,
                        transform.translation.z,
                    ],
                ));
            }
        }
    }
    // Registered but never instantiated, or a lone survivor: zero
    // pairs to check — vacuity is the audit's call, frames are
    // clean.
    if nodes.len() < 2 {
        return PwOutcome {
            pairs: vec![],
            driver: frame_clock(frame, tps),
            config_error: None,
            pairable: false,
        };
    }
    let mut pairs: Vec<crate::episode::Observation<crate::episode::PairKey>> = Vec::new();
    for i in 0..nodes.len() {
        for j in (i + 1)..nodes.len() {
            let (a_index, a_pos) = nodes[i];
            let (b_index, b_pos) = nodes[j];
            let dx = a_pos[0] - b_pos[0];
            let dy = a_pos[1] - b_pos[1];
            let dz = a_pos[2] - b_pos[2];
            let distance = (dx * dx + dy * dy + dz * dz).sqrt();
            // NaN translations yield NaN distances — the
            // finite_transforms oracle owns NaN findings; this
            // recorder must not fold a NaN observation into an
            // episode. Filter rather than report (the NaN family is
            // already gating elsewhere, never double-reported).
            if distance.is_finite() && distance < min_distance {
                pairs.push(crate::episode::Observation {
                    key: crate::episode::PairKey::new(a_index, b_index),
                    value: distance,
                });
            }
        }
    }
    // Driver sample: declared leaf, or the frame clock (seconds).
    let driver = match (inv.driver_resource.as_deref(), inv.driver_path.as_deref()) {
        (Some(res_name), Some(path)) => {
            match sample_declared_driver(world, registry, res_name, path) {
                Ok(Some(v)) => Some(v),
                // Absent resource: a gap, not an error (same
                // doctrine as stagnant_resource's late arrivals).
                Ok(None) => None,
                Err(why) => return err(why),
            }
        }
        (None, None) => frame_clock(frame, tps),
        // Lint guarantees both halves or neither; defensive only.
        _ => return err("pairwise_separation's driver needs both halves (validated)".into()),
    };
    PwOutcome {
        pairs,
        driver,
        config_error: None,
        pairable: true,
    }
}

/// The default driver: the frame clock, in seconds.
fn frame_clock(frame: u64, tps: u64) -> Option<f64> {
    Some(frame as f64 / tps as f64)
}

/// Resolve and read the declared driver leaf. `Ok(None)` = the
/// resource is absent this frame (a gap, not an error — late
/// arrival, mid-run removal, teardown); `Err` = a resolution
/// failure (a config error). Non-finite samples become `None` — a
/// gap — so the fold closes the episode rather than crediting a
/// bogus budget.
fn sample_declared_driver(
    world: &World,
    registry: &TypeRegistry,
    resource_name: &str,
    path: &str,
) -> Result<Option<f64>, String> {
    let resource = match reflect_resource(world, registry, resource_name) {
        Ok(ResourceView::Value(r)) => r,
        Ok(ResourceView::Absent) => return Ok(None),
        Err(why) => return Err(why),
    };
    match reflect_numeric_path(resource, path, "resource") {
        Ok(v) => Ok(v.is_finite().then_some(v)),
        Err(why) => Err(why),
    }
}

/// Write-phase fold of one frame's pairwise outcome: feed the
/// engine, judge any closed episode against the soak, report ONE
/// finding with the episode's evidence.
fn apply_pw_outcome(
    inv: &Invariant,
    outcome: &PwOutcome,
    frame: u64,
    book: &mut PairwiseBookkeeping,
    violations: &mut crate::state::Violations,
) {
    if let Some(why) = &outcome.config_error {
        book.config_errored.insert(inv.name.clone());
        report_config_error_once(
            violations,
            &mut book.oracle_error_seen,
            &inv.name,
            &format!("invariant '{}': {why}", inv.name),
            frame,
        );
        return;
    }
    if outcome.pairable {
        book.matched.insert(inv.name.clone());
    }
    let folder = book.folders.entry(inv.name.clone()).or_default();
    // Frame order is guaranteed by the driver loop; push can close
    // an episode only via this frame (violation ends, gap, or the
    // non-consecutive close-and-reopen).
    if let Some(closed) = folder.push(frame, outcome.driver, &outcome.pairs) {
        judge_closed_episode(inv, &closed, violations);
    }
}

/// Soak judgment and finding emission for one closed episode:
/// discard sub-soak episodes as transient fly-bys; report one
/// finding with the worst observation and the roster evidence.
fn judge_closed_episode(
    inv: &Invariant,
    ep: &crate::episode::ClosedEpisode<crate::episode::PairKey>,
    violations: &mut crate::state::Violations,
) {
    let Some(window_s) = inv.window_s else {
        return; // Validated required; defensive only.
    };
    if ep.driver_span < window_s as f64 {
        // Sub-soak: a transient fly-by the ε doctrine discards.
        return;
    }
    let worst = &ep.worst;
    let min_distance = inv.min_distance.unwrap_or(0.0);
    // The soak span is in the DRIVER LEAF'S OWN UNIT — seconds only
    // for the frame-clock default. Suffixing a game-clock soak with
    // "s" would lie about the unit (the tests drive minutes).
    let (span, span_unit) = match inv.driver_path.as_deref() {
        Some(_) => (ep.driver_span, " driver units"),
        None => (ep.driver_span, "s of frame clock"),
    };
    violations.report(
        crate::rules::Rule::PairwiseSeparation.name(),
        &inv.name,
        format!(
            "entities #{} and #{} came within {:.3} (< {}) of each other, sustained \
             for {:.3}{} across frames {}–{} ({} violating pair{}, \
             worst distance {:.3}); a coincidence this long is bodies occupying the \
             same space, not a fly-by",
            worst.key.low,
            worst.key.high,
            worst.value,
            min_distance,
            span,
            span_unit,
            ep.start_frame,
            ep.end_frame,
            ep.distinct_keys,
            if ep.distinct_keys == 1 { "" } else { "s" },
            worst.value,
        ),
        ep.start_frame,
    );
    // Attach the roster evidence (`_meta`) to the just-reported
    // entry — one finding, full story (a handoff A–B → A–C cannot
    // hide the persistent offender A). First-write-wins, mirroring
    // the detail doctrine: `report` keeps the FIRST episode's
    // detail/first_frame, so the meta must describe the same
    // episode — a second soaked episode extends last_frame (its
    // detail span already told the reader there was more) but
    // never swaps the evidence out from under the first finding.
    if let Some(entry) = violations.entries.get_mut(&(
        crate::rules::Rule::PairwiseSeparation.name().to_string(),
        inv.name.clone(),
    )) {
        entry.last_frame = entry.last_frame.max(ep.end_frame);
        if entry.meta.is_none() {
            entry.meta = Some(crate::discovery::surface::ViolationMeta {
                stepping: None,
                pairwise_episode: Some(pairwise_meta_of(ep)),
            });
        }
    }
}

/// Convert a closed episode into its `_meta` evidence block.
fn pairwise_meta_of(
    ep: &crate::episode::ClosedEpisode<crate::episode::PairKey>,
) -> crate::discovery::surface::PairwiseEpisodeMeta {
    crate::discovery::surface::PairwiseEpisodeMeta {
        sampled_pairs: ep.distinct_keys,
        roster: ep
            .roster
            .iter()
            .map(|r| crate::discovery::surface::PairwiseRosterEntry {
                low: r.key.low,
                high: r.key.high,
                first_frame: r.first_frame,
                worst_distance: r.worst_value,
                worst_frame: r.worst_frame,
                violating_frames: r.violating_frames,
            })
            .collect(),
        roster_truncated: ep.roster_truncated,
    }
}

/// Run-end drain: any episode still open at the last frame is a
/// complete stretch on its own (frames `start..=end`); the
/// non-consecutive doctrine never spans the unobserved beyond, so
/// fold it out and judge it like any other closed episode. Called
/// once, after the driver loop ends and before the bookkeeping is
/// audited.
pub fn close_pairwise_episodes(world: &mut World) {
    let invariants: Vec<Invariant> = world
        .get_resource::<ScenarioInvariants>()
        .map(|invariants| {
            invariants
                .0
                .iter()
                .filter(|i| i.rule == InvariantRule::PairwiseSeparation)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    if invariants.is_empty() {
        return;
    }
    world.resource_scope(|world, mut book: Mut<PairwiseBookkeeping>| {
        // RunState is already drained at this point (the driver
        // removes it before the audits); no frame count is needed —
        // the episode's own end_frame is the authoritative end.
        let mut violations = world.resource_mut::<RunViolations>();
        for inv in &invariants {
            if let Some(folder) = book.folders.get_mut(&inv.name) {
                if let Some(closed) = folder.close_run() {
                    judge_closed_episode(inv, &closed, &mut violations.0);
                }
            }
        }
    });
}

/// Post-run audit for `pairwise_separation` invariants (mirrors
/// [`audit_component_checks`]): a selector that never had a pair to
/// check proves nothing — reported as `vacuous_invariant`, named
/// per invariant. Config-errored invariants are suppressed (their
/// `oracle_error` already told the story).
pub fn audit_pairwise_separation(
    invariants: &[Invariant],
    book: &PairwiseBookkeeping,
    last_frame: u64,
) -> Vec<crate::state::ViolationEntry> {
    let mut entries = Vec::new();
    for inv in invariants
        .iter()
        .filter(|i| i.rule == InvariantRule::PairwiseSeparation)
    {
        if book.config_errored.contains(&inv.name) {
            continue;
        }
        if !book.matched.contains(&inv.name) {
            entries.push(crate::state::ViolationEntry {
                rule: crate::rules::Rule::VacuousInvariant.name().to_string(),
                target: inv.name.clone(),
                first_frame: 0,
                last_frame,
                count: 1,
                detail: format!(
                    "invariant '{}' never had a pair to check over the whole run; \
                     the check proves nothing until there is something to check",
                    inv.name
                ),
                last_detail: format!("invariant '{}' never had a pair to check", inv.name),
                meta: None,
            });
        }
    }
    entries
}
