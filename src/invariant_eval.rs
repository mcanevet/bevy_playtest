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
    // Optional: the driver initializes the accumulator, but an App
    // driven outside `run_scenario` (manual test setups) may lack it.
    // A missing accumulator must not crash the judge — held
    // evidence is additive, never load-bearing.
    mut held: Option<ResMut<crate::held::HeldBookkeeping>>,
    q_all: Query<(Entity, &Transform)>,
) {
    for inv in &invariants.0 {
        if inv.rule != InvariantRule::NodesInBounds {
            continue;
        }
        let selector = inv.target_selector.clone().unwrap_or_default();
        match selector {
            TargetSelector::All => {
                let held = held.as_deref_mut();
                evaluate_bounds(inv, state.frame, &mut violations, held, &mut q_all.iter());
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
    held: Option<&mut crate::held::HeldBookkeeping>,
    transforms: &mut dyn Iterator<Item = (Entity, &Transform)>,
) {
    let min_x = inv.min_x.unwrap_or(-1000.0);
    let max_x = inv.max_x.unwrap_or(1000.0);
    let min_y = inv.min_y.unwrap_or(-1000.0);
    let max_y = inv.max_y.unwrap_or(1000.0);
    let min_z = inv.min_z.unwrap_or(-1000.0);
    let max_z = inv.max_z.unwrap_or(1000.0);
    let mut judged = 0usize;
    for (entity, transform) in transforms {
        judged += 1;
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
    // Held-invariant evidence (the component tier's passing trace):
    // the judge's workload for this frame — counted only when the
    // scope judged something, matching the with_component recorder's
    // existence semantics. Absent accumulator ⇒ no evidence, no
    // crash (additive, never load-bearing).
    if judged > 0 {
        if let Some(held) = held {
            held.observe_component_frame(&inv.name, judged, frame, None);
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
    /// Nearest-margin HOLDING sample this frame: the smallest
    /// |value − threshold| among matched entities whose predicate
    /// held — the "how close did any judged entity come to
    /// violating" evidence a passing run's held record reports.
    /// `None` when any entity violated (a held record only exists
    /// for passing runs, so violating frames contribute no
    /// nearest-margin evidence) or when no entity held.
    nearest_margin: Option<f64>,
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
        world.resource_scope(|world, mut held: Mut<crate::held::HeldBookkeeping>| {
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
                    // Held-invariant evidence (component-tier passing
                    // trace): the judge's workload for this frame. No
                    // margin sample — bounds checks judge containment,
                    // not a thresholded leaf.
                    held.observe_component_frame(&inv.name, checkable, frame, None);
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
        world.resource_scope(|world, mut held: Mut<crate::held::HeldBookkeeping>| {
            let mut violations = world.resource_mut::<RunViolations>();
            for (inv, outcome) in cc_invariants.iter().zip(outcomes.iter()) {
                apply_outcome(
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

/// Fold one frame's outcome for one invariant into the watched
/// bookkeeping and (for always-mode violations) the run violations.
fn apply_outcome(
    inv: &Invariant,
    outcome: &CcOutcome,
    frame: u64,
    watched: &mut WatchedInvariants,
    violations: &mut RunViolations,
    held: &mut crate::held::HeldBookkeeping,
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
    // Held-invariant evidence (the component tier's passing-invariant
    // trace): workload counts plus the nearest-margin sample, folded
    // only for frames where the selector resolved and matched ≥1
    // entity. The margin rides only all-held frames — a held record
    // exists only for passing runs, and the predicate's operator is
    // cached for the collector's direction interpretation.
    // config_error already returned early above, so `all_held`
    // alone gates the margin: a frame with any violation carries
    // no nearest-margin evidence (the held record only exists for
    // passing runs).
    let margin = if outcome.all_held {
        outcome.nearest_margin
    } else {
        None
    };
    held.observe_component_frame(&inv.name, outcome.matched, frame, margin);
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
            nearest_margin: None,
            config_error: Some("component_check requires a with_component target selector".into()),
        };
    };
    let Some((path, check, threshold)) = cc_fields(inv) else {
        return CcOutcome {
            matched: 0,
            all_held: true,
            example_bad: None,
            nearest_margin: None,
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
                nearest_margin: None,
                config_error: Some(why),
            };
        }
    };
    let Some(reflect_component) = registration.data::<bevy_ecs::reflect::ReflectComponent>() else {
        return CcOutcome {
            matched: 0,
            all_held: true,
            example_bad: None,
            nearest_margin: None,
            config_error: Some(not_a_reflected_component_message(name)),
        };
    };

    let mut outcome = CcOutcome {
        matched: 0,
        all_held: true,
        example_bad: None,
        nearest_margin: None,
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
                if check.holds(current, threshold) {
                    // Nearest-margin tracking: among entities that
                    // held, keep the sample closest to the threshold
                    // — the held record's "how close did it get"
                    // evidence. Violating frames contribute no
                    // margin (the held record only exists for
                    // passing runs).
                    let margin = (current - threshold).abs();
                    if outcome.nearest_margin.is_none_or(|m| margin < m) {
                        outcome.nearest_margin = Some(margin);
                    }
                } else {
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

/// Definite resolution of an entity_stagnation watch path against
/// the registry — NO live instance needed. Walks the path's static
/// type information: struct fields via `StructInfo::field`, enum
/// VARIANT names via `EnumInfo::variant` (the variant-qualified
/// path `state.Queued.waited` walks component field `state` →
/// variant `Queued` → field `waited`), and struct-variant fields
/// via `StructVariantInfo::field`. This is leaf_where's safety
/// hinge: a variant typo in either `watch_path` or `leaf_where`
/// is a CONFIG error caught at probe time, never a silent
/// "never true" that masquerades as a passing workload fact. A
/// path that resolves here can STILL fail per-instance at run
/// time (an entity whose enum currently holds a different
/// variant) — that is the leaf_where precondition, not an error.
pub(crate) fn probe_entity_watch_path(
    registry: &TypeRegistry,
    component: &str,
    path: &str,
) -> Result<(), String> {
    use bevy_reflect::TypeInfo;
    let registration = resolve_registration(registry, component, "component")?;
    let mut ty: &TypeInfo = registration.type_info();
    let segments: Vec<&str> = path.split('.').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return Err(format!(
            "watch path '{path}' is empty — a blank path resolves the \
             component root, never a watchable leaf"
        ));
    }
    // Manual index loop: an ENUM segment consumes TWO segments (the
    // variant name and the field inside it), which a for-loop
    // cannot express.
    let mut i = 0;
    while i < segments.len() {
        let segment = segments[i];
        match ty {
            TypeInfo::Struct(info) => {
                let Some(field) = info.field(segment) else {
                    return Err(format!(
                        "watch path '{path}' does not resolve on component \
                         '{component}': struct has no field named '{segment}' \
                         (fields: {})",
                        info.field_names().join(", ")
                    ));
                };
                let Some(next) = field.type_info() else {
                    return Err(format!(
                        "watch path '{path}': field '{segment}' on component \
                         '{component}' carries no static type information — \
                         its leaf cannot be resolved at probe time"
                    ));
                };
                ty = next;
                i += 1;
            }
            TypeInfo::Enum(info) => {
                let Some(variant) = info.variant(segment) else {
                    return Err(format!(
                        "watch path '{path}' does not resolve on component \
                         '{component}': enum '{}' has no variant named \
                         '{segment}' (variants: {})",
                        ty.type_path(),
                        info.iter()
                            .map(|v| v.name().to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                };
                // The variant name itself can be the watch leaf
                // (`watch_path: "state" == leaf_where's variant) —
                // resolved, nothing more to descend.
                if i + 1 == segments.len() {
                    return Ok(());
                }
                // Continue INSIDE the variant: only struct variants
                // carry named fields — the same constraint the live
                // path resolver enforces on enum field access.
                let struct_variant = variant.as_struct_variant().map_err(|_| {
                    format!(
                        "watch path '{path}' resolves into variant \
                         '{segment}', which is not a struct variant — only \
                         struct variants carry named fields to continue the \
                         path"
                    )
                })?;
                let next_segment = segments[i + 1];
                let Some(field) = struct_variant.field(next_segment) else {
                    return Err(format!(
                        "watch path '{path}' does not resolve on component \
                         '{component}': variant '{segment}' has no field named \
                         '{next_segment}' (fields: {})",
                        struct_variant.field_names().join(", ")
                    ));
                };
                let Some(next) = field.type_info() else {
                    return Err(format!(
                        "watch path '{path}': field '{next_segment}' inside \
                         variant '{segment}' carries no static type \
                         information — its leaf cannot be resolved at probe \
                         time"
                    ));
                };
                ty = next;
                i += 2;
            }
            other => {
                return Err(format!(
                    "watch path '{path}' does not resolve on component \
                     '{component}': segment '{segment}' descends into a {}, \
                     which carries no named fields or variants — the path \
                     cannot continue",
                    match other {
                        TypeInfo::TupleStruct(_) => "tuple struct",
                        TypeInfo::Tuple(_) => "tuple",
                        TypeInfo::List(_) => "list",
                        TypeInfo::Array(_) => "array",
                        TypeInfo::Map(_) => "map",
                        TypeInfo::Set(_) => "set",
                        TypeInfo::Opaque(_) => "leaf value",
                        TypeInfo::Struct(_) | TypeInfo::Enum(_) => unreachable!(),
                    }
                ));
            }
        }
    }
    Ok(())
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
    /// Invariant name → the pass-side held statistics (the
    /// passing-invariant trace — see
    /// [`crate::held::EpisodeHeldStats`]). Folds alongside the
    /// engine so sub-soak episodes — the near-misses a passing
    /// run's story is made of — survive the soak judgment that
    /// discards them from the violation list.
    pub episode_stats: std::collections::HashMap<String, crate::held::EpisodeHeldStats>,
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
    /// How many checkable (marker + Transform) entities the
    /// enumeration walked this frame — the workload figure behind
    /// `pair_judgments` (n·(n−1)/2 pairs). Zero when not pairable.
    checkable_count: usize,
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
        checkable_count: 0,
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
            checkable_count: nodes.len(),
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
        checkable_count: nodes.len(),
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
    // Held-trace workload fold: judged frames and pair-judgments.
    // Only pairable frames count (a frame without two checkable
    // entities judged nothing — mirroring the flat tiers'
    // "judged something" semantics).
    if outcome.pairable {
        let n = outcome.checkable_count as u64;
        book.episode_stats
            .entry(inv.name.clone())
            .or_default()
            .observe_frame(Some(n * (n - 1) / 2), None);
    }
    let folder = book.folders.entry(inv.name.clone()).or_default();
    // Frame order is guaranteed by the driver loop; push can close
    // an episode only via this frame (violation ends, gap, or the
    // non-consecutive close-and-reopen).
    let closed = folder.push(frame, outcome.driver, &outcome.pairs);
    if let Some(closed) = closed {
        book.episode_stats
            .entry(inv.name.clone())
            .or_default()
            .observe_episode_span(closed.driver_span, closed.start_frame);
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
                consistency_episode: None,
                forbidden_episode: None,
                stagnation_episode: None,
                occupancy_episode: None,
                unpromoted_rule: None,
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
            let closed = book
                .folders
                .get_mut(&inv.name)
                .and_then(|folder| folder.close_run());
            if let Some(closed) = closed {
                book.episode_stats
                    .entry(inv.name.clone())
                    .or_default()
                    .observe_episode_span(closed.driver_span, closed.start_frame);
                judge_closed_episode(inv, &closed, &mut violations.0);
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

// ---------------------------------------------------------------------------
// forbidden_region: the declarative no-go-zone oracle
// (judgement layer, spatial family, per the reviewer-confirmed
// design: GATING posture — the acceptance-table row carries a
// RED/GREEN litmus pair, not a placeholder — and ONE EpisodeFolder
// PER INVARIANT keyed by ENTITY INDEX, so each region's incursion
// timeline is independent).
// ---------------------------------------------------------------------------

/// Bookkeeping for `forbidden_region` invariants (same doctrine as
/// [`PairwiseBookkeeping`]): per-frame recorder ownership, full-
/// series observability flags for the vacuity audit.
#[derive(Resource, Default)]
pub struct ForbiddenRegionBookkeeping {
    /// Invariant name → the selector resolved AND at least one
    /// checkable entity (marker + Transform) existed in some
    /// observed frame. Otherwise vacuous — a region rule with
    /// nothing to select proves nothing.
    pub matched: std::collections::HashSet<String>,
    /// Invariant name → a resolution/config failure was seen
    /// during the run (already reported as `oracle_error`); the
    /// audit suppresses its vacuity entry (N-B/N-C doctrine).
    pub config_errored: std::collections::HashSet<String>,
    /// (invariant name, error class) pairs already reported as
    /// `oracle_error` this run — the once-per-distinct-error dedup.
    pub oracle_error_seen: std::collections::HashSet<String>,
    /// Invariant name → the fold engine. One folder per invariant:
    /// the grammar allows exactly one region per invariant, so
    /// folder identity IS region identity; episodes in one region
    /// never weld to another's (an author wanting two regions
    /// writes two named invariants).
    pub folders: std::collections::HashMap<String, crate::episode::EpisodeFolder<u32>>,
    /// Invariant name → the pass-side held statistics, folding
    /// alongside the engine so sub-soak incursions survive the
    /// soak judgment as near-miss story.
    pub episode_stats: std::collections::HashMap<String, crate::held::EpisodeHeldStats>,
}

impl ForbiddenRegionBookkeeping {
    /// The folder shape for a forbidden_region invariant: entity
    /// keys, larger-is-worse (the observation value is penetration
    /// depth — DEEPER is WORSE). Constructed explicitly (not via
    /// Default) so the direction choice stays visible at the
    /// type-erased HashMap boundary, same doctrine as
    /// [`ConsistencyBookkeeping::new_folder`].
    fn new_folder() -> crate::episode::EpisodeFolder<u32> {
        crate::episode::EpisodeFolder::new(
            crate::episode::DEFAULT_ROSTER_CAP,
            crate::episode::WorstDirection::LargerIsWorse,
        )
    }
}

/// One frame's forbidden_region outcome for one invariant
/// (owned-data phase-split doctrine).
struct FrOutcome {
    /// The frame's incursive entities (empty = a clean frame,
    /// which ends any open episode).
    incursions: Vec<crate::episode::Observation<u32>>,
    /// The driver-leaf sample; `None` on a gap.
    driver: Option<f64>,
    /// First config error this frame, if any.
    config_error: Option<String>,
    /// The selector resolved AND ≥1 checkable entity existed.
    checkable: bool,
    /// How many checkable entities the enumeration walked this
    /// frame (the workload figure behind the held trace's
    /// per-frame count).
    checkable_count: usize,
}

/// Driver-loop recorder for `forbidden_region` invariants: the
/// declarative no-go-zone oracle. Selects every entity carrying
/// the named registered component; a selected entity whose
/// Transform translation lies INSIDE the author-declared box
/// region (boundaries INCLUDED — the on-edge doctrine, pinned by
/// the battery) is incursive. Incursion frames fold into EPISODES
/// (maximal stretches over time — see [`crate::episode`]) and an
/// episode becomes ONE finding when its `window_s` soak elapses
/// (in the declared driver leaf's own units; the frame clock in
/// seconds when no driver is declared).
///
/// Cost is O(n) over the selector's entities — the author bounds
/// it by choosing the selector (that is why `all` is rejected at
/// load: forbidding every `Transform` in the world — walls,
/// camera, UI — from a region is never what an author means).
///
/// Soak semantics follow `pairwise_separation`'s window doctrine
/// exactly: forward driver deltas credit the span, retreats
/// advance no budget, a gap (missing/non-finite driver sample)
/// CLOSES the open episode, and sub-soak episodes are discarded as
/// transient fly-bys (the ε doctrine: brushing a no-go zone in
/// passing is gameplay, not a bug).
///
/// `allow_if` exempts per entity, per frame: an entity carrying
/// the exemption component is never incursive, and never even
/// opens an episode (so a lifetime-exempt presence inside the
/// region is silence, not vacuity). The exemption is read at the
/// same settled observation point as the position — the flip-test
/// discipline (proposal §1) is the AUTHOR's job: the exemption
/// must name the component marking LEGITIMATE presence, never the
/// state the buggy run is in.
///
/// The EVERY-FRAME PUSH discipline: this recorder pushes to its
/// folder EVERY observed frame, including frames with zero
/// incursions — an empty observation set is what CLOSES an
/// episode when the entity leaves the region, and skipping the
/// push would weld two incursions into one across the gap.
pub fn record_forbidden_region(world: &mut World, frame: u64, tps: u64) {
    let invariants: Vec<Invariant> = world
        .get_resource::<ScenarioInvariants>()
        .map(|invariants| {
            invariants
                .0
                .iter()
                .filter(|i| i.rule == InvariantRule::ForbiddenRegion)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    if invariants.is_empty() {
        return;
    }
    let Some(atr) = world.get_resource::<AppTypeRegistry>().cloned() else {
        world.resource_scope(|world, mut book: Mut<ForbiddenRegionBookkeeping>| {
            let mut violations = world.resource_mut::<RunViolations>();
            for inv in &invariants {
                book.config_errored.insert(inv.name.clone());
                let why = &no_atr_message("forbidden region checks");
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

    let mut outcomes: Vec<(Invariant, FrOutcome)> = Vec::with_capacity(invariants.len());
    for inv in &invariants {
        outcomes.push((
            inv.clone(),
            eval_forbidden_region(world, &registry, inv, frame, tps),
        ));
    }
    drop(registry);

    world.resource_scope(|world, mut book: Mut<ForbiddenRegionBookkeeping>| {
        let mut violations = world.resource_mut::<RunViolations>();
        for (inv, outcome) in outcomes {
            apply_fr_outcome(&inv, &outcome, frame, &mut book, &mut violations.0);
        }
    });
}

/// Enumerate the frame's incursive entities for one invariant
/// (the read phase).
fn eval_forbidden_region(
    world: &World,
    registry: &TypeRegistry,
    inv: &Invariant,
    frame: u64,
    tps: u64,
) -> FrOutcome {
    let err = |why: String| FrOutcome {
        incursions: vec![],
        driver: None,
        config_error: Some(why),
        checkable: false,
        checkable_count: 0,
    };
    // Validation guarantees these; the Options are defensive only.
    let Some(crate::enums::TargetSelector::WithComponent { name }) = inv.target_selector.as_ref()
    else {
        return err("forbidden_region requires a with_component selector (validated)".into());
    };
    let Some(region) = inv.region else {
        return err("forbidden_region requires 'region' (validated)".into());
    };
    let type_reg = match resolve_registration(registry, name, "component") {
        Ok(t) => t,
        Err(why) => return err(why),
    };
    // Resolve the optional exemption component ONCE up front: a
    // bad name is a config error regardless of whether anyone is
    // inside the region this frame (the lint front-loads the
    // same resolution; the recorder still guards defensively).
    // Kept as a bare TypeId: the membership probe below is
    // EntityRef::contains_type_id, no ComponentId round-trip.
    let allow_type_id = match inv.allow_if.as_deref() {
        Some(allow) => match resolve_registration(registry, allow, "component") {
            Ok(t) => Some(t.type_id()),
            Err(why) => return err(why),
        },
        None => None,
    };
    // Collect (entity, translation) for every entity carrying the
    // marker AND a Transform — the bounds recorder's doctrine
    // ("carries the marker but no Transform: not checkable"). The
    // FULL Entity is kept (not just its index) so the exemption
    // check below resolves the live entity, never a reconstructed
    // index with a stale generation.
    let mut nodes: Vec<(Entity, [f32; 3])> = Vec::new();
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
                    entity_ref.id(),
                    [
                        transform.translation.x,
                        transform.translation.y,
                        transform.translation.z,
                    ],
                ));
            }
        }
    }
    // Registered but never instantiated: nothing to select —
    // vacuity is the audit's call, frames are clean (but STILL
    // pushed, so any open episode closes: the every-frame-push
    // discipline).
    if nodes.is_empty() {
        return FrOutcome {
            incursions: vec![],
            driver: frame_clock(frame, tps),
            config_error: None,
            checkable: false,
            checkable_count: 0,
        };
    }
    let mut incursions: Vec<crate::episode::Observation<u32>> = Vec::new();
    let checkable_count = nodes.len();
    for (entity, pos) in nodes {
        // NaN translations are never inside (NaN comparisons are
        // false) — the finite_transforms oracle owns NaN findings;
        // never double-reported.
        if !region.contains(pos) {
            continue;
        }
        // allow_if exempts per entity, per frame, read at the
        // settled observation point. An exempt entity never even
        // opens an episode (silence, not vacuity).
        if let Some(allow_tid) = allow_type_id {
            let exempt = world
                .get_entity(entity)
                .map(|e| e.contains_type_id(allow_tid))
                .unwrap_or(false);
            if exempt {
                continue;
            }
        }
        // The observation value is PENETRATION DEPTH: distance
        // from the translation to the NEAREST face of the box
        // (the MINIMUM remaining margin over the axes — how far
        // the entity must travel to exit through the closest
        // face). DEEPER is WORSE (LargerIsWorse), so the
        // engine's worst-selection and the roster's worst-first
        // sort name the most-embedded observation as evidence.
        // On-face depth is 0 (the inclusive-boundary doctrine:
        // still incursive); at the very center it equals the
        // smallest half_extent.
        let depth = {
            let dx = (pos[0] - region.center[0]).abs();
            let dy = (pos[1] - region.center[1]).abs();
            let dz = (pos[2] - region.center[2]).abs();
            debug_assert!(
                dx <= region.half_extents[0]
                    && dy <= region.half_extents[1]
                    && dz <= region.half_extents[2]
            );
            (region.half_extents[0] - dx)
                .min(region.half_extents[1] - dy)
                .min(region.half_extents[2] - dz)
        };
        incursions.push(crate::episode::Observation {
            key: entity.index().index(),
            value: depth,
        });
    }
    // Driver sample: declared leaf, or the frame clock (seconds).
    let driver = match (inv.driver_resource.as_deref(), inv.driver_path.as_deref()) {
        (Some(res_name), Some(path)) => {
            match sample_declared_driver(world, registry, res_name, path) {
                Ok(Some(v)) => Some(v),
                Ok(None) => None,
                Err(why) => return err(why),
            }
        }
        (None, None) => frame_clock(frame, tps),
        _ => return err("forbidden_region's driver needs both halves (validated)".into()),
    };
    FrOutcome {
        incursions,
        driver,
        config_error: None,
        checkable: true,
        checkable_count,
    }
}

/// Write-phase fold of one frame's forbidden_region outcome: feed
/// the engine (EVERY frame — an empty set closes the open
/// episode), judge any closed episode against the soak, report ONE
/// finding with the episode's evidence.
fn apply_fr_outcome(
    inv: &Invariant,
    outcome: &FrOutcome,
    frame: u64,
    book: &mut ForbiddenRegionBookkeeping,
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
    if outcome.checkable {
        book.matched.insert(inv.name.clone());
        // Held-trace workload fold: judged frames and per-frame
        // selected-entity count (the consistency recorder's
        // entity_count slot — same doctrine).
        book.episode_stats
            .entry(inv.name.clone())
            .or_default()
            .observe_frame(None, Some(outcome.checkable_count as u64));
    }
    let folder = book
        .folders
        .entry(inv.name.clone())
        .or_insert_with(ForbiddenRegionBookkeeping::new_folder);
    let closed = folder.push(frame, outcome.driver, &outcome.incursions);
    if let Some(closed) = closed {
        book.episode_stats
            .entry(inv.name.clone())
            .or_default()
            .observe_episode_span(closed.driver_span, closed.start_frame);
        judge_fr_episode(inv, &closed, violations);
    }
}

/// Soak judgment and finding emission for one closed incursion
/// episode: discard sub-soak episodes as transient fly-bys; report
/// one finding with the worst observation and the roster
/// evidence.
fn judge_fr_episode(
    inv: &Invariant,
    ep: &crate::episode::ClosedEpisode<u32>,
    violations: &mut crate::state::Violations,
) {
    let Some(window_s) = inv.window_s else {
        return; // Validated required; defensive only.
    };
    if ep.driver_span < window_s as f64 {
        // Sub-soak: a transient brush the ε doctrine discards.
        return;
    }
    let Some(region) = inv.region else {
        return; // Validated required; defensive only.
    };
    let (span, span_unit) = match inv.driver_path.as_deref() {
        Some(_) => (ep.driver_span, " driver units"),
        None => (ep.driver_span, "s of frame clock"),
    };
    let worst_depth_frame = ep.worst.frame;
    violations.report(
        crate::rules::Rule::ForbiddenRegion.name(),
        &inv.name,
        format!(
            "entity #{} entered the forbidden region (center {:?}, \
             half_extents {:?}), reaching a penetration depth of {:.3} \
             (worst at frame {}), sustained for {:.3}{} across frames \
             {}–{} ({} incursive entit{}); lingering inside a no-go \
             zone is a state bug, not a fly-by",
            ep.worst.key,
            region.center,
            region.half_extents,
            ep.worst.value,
            worst_depth_frame,
            span,
            span_unit,
            ep.start_frame,
            ep.end_frame,
            ep.distinct_keys,
            if ep.distinct_keys == 1 { "y" } else { "ies" },
        ),
        ep.start_frame,
    );
    // Attach the roster evidence (`_meta`) to the just-reported
    // entry — first-write-wins, the pairwise doctrine: `report`
    // keeps the FIRST episode's detail/first_frame, so the meta
    // must describe the same episode — a later soaked episode
    // extends last_frame (its detail span already told the reader
    // there was more) but never swaps the evidence out from under
    // the first finding.
    if let Some(entry) = violations.entries.get_mut(&(
        crate::rules::Rule::ForbiddenRegion.name().to_string(),
        inv.name.clone(),
    )) {
        entry.last_frame = entry.last_frame.max(ep.end_frame);
        if entry.meta.is_none() {
            entry.meta = Some(crate::discovery::surface::ViolationMeta {
                stepping: None,
                pairwise_episode: None,
                consistency_episode: None,
                forbidden_episode: Some(forbidden_meta_of(ep)),
                stagnation_episode: None,
                occupancy_episode: None,
                unpromoted_rule: None,
            });
        }
    }
}

/// The `_meta` evidence block for one closed incursion episode:
/// the roster maps the generic engine fields into the wire-stable
/// names (entity, worst_depth — the penetration-depth value the
/// recorder observes).
fn forbidden_meta_of(
    ep: &crate::episode::ClosedEpisode<u32>,
) -> crate::discovery::surface::ForbiddenEpisodeMeta {
    crate::discovery::surface::ForbiddenEpisodeMeta {
        sampled_entities: ep.distinct_keys,
        roster: ep
            .roster
            .iter()
            .map(|r| crate::discovery::surface::ForbiddenRosterEntry {
                entity: r.key,
                first_frame: r.first_frame,
                worst_depth: r.worst_value,
                worst_frame: r.worst_frame,
                violating_frames: r.violating_frames,
            })
            .collect(),
        roster_truncated: ep.roster_truncated,
    }
}

/// Run-end drain: an incursion still open at the last frame is a
/// complete stretch on its own — fold it out and judge it like
/// any other closed episode.
pub fn close_forbidden_region_episodes(world: &mut World) {
    let invariants: Vec<Invariant> = world
        .get_resource::<ScenarioInvariants>()
        .map(|invariants| {
            invariants
                .0
                .iter()
                .filter(|i| i.rule == InvariantRule::ForbiddenRegion)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    if invariants.is_empty() {
        return;
    }
    world.resource_scope(|world, mut book: Mut<ForbiddenRegionBookkeeping>| {
        let mut violations = world.resource_mut::<RunViolations>();
        for inv in &invariants {
            let closed = book
                .folders
                .get_mut(&inv.name)
                .and_then(|folder| folder.close_run());
            if let Some(closed) = closed {
                book.episode_stats
                    .entry(inv.name.clone())
                    .or_default()
                    .observe_episode_span(closed.driver_span, closed.start_frame);
                judge_fr_episode(inv, &closed, &mut violations.0);
            }
        }
    });
}

/// Post-run audit for `forbidden_region` invariants: a selector
/// that never had a checkable entity over the whole run proves
/// nothing — reported as `vacuous_invariant`, named per
/// invariant. Config-errored invariants are suppressed (their
/// `oracle_error` already told the story).
pub fn audit_forbidden_region(
    invariants: &[Invariant],
    book: &ForbiddenRegionBookkeeping,
    last_frame: u64,
) -> Vec<crate::state::ViolationEntry> {
    let mut entries = Vec::new();
    for inv in invariants
        .iter()
        .filter(|i| i.rule == InvariantRule::ForbiddenRegion)
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
                    "invariant '{}' never resolved a checkable entity over the \
                     whole run; the region rule proves nothing until there is \
                     something to select",
                    inv.name
                ),
                last_detail: format!("invariant '{}' never resolved a checkable entity", inv.name),
                meta: None,
            });
        }
    }
    entries
}

// ---------------------------------------------------------------------
// derived_transform_consistency: the compositional-identity recorder
// (judgement layer, slice 2, part 3).
//
// Doctrine: observe POST-UPDATE — the recorder runs from the driver
// loop after `app.update()` returns, so both sides of the identity
// are read at the same SETTLED frame (post-PostUpdate,
// post-TransformPropagate). The check asks "is the stored
// (Transform, GlobalTransform, parent's GlobalTransform) triple
// internally consistent", NEVER "did the game call propagate": a
// naive Update-scheduled direct write self-repairs by
// PostUpdate/propagation in the same frame and is invisible here —
// only divergence that SURVIVES propagation is observable, and that
// is precisely the class the rule means to catch (stale globals:
// writes the game performs after propagation, or propagation runs
// that skipped a subtree).
// ---------------------------------------------------------------------

/// Bookkeeping for `derived_transform_consistency` invariants across
/// frames (the pairwise bookkeeping doctrine, one shape per rule):
/// observability for the vacuity audit, once-per-error-class dedup,
/// and the per-invariant fold engine.
#[derive(Resource, Default)]
pub struct ConsistencyBookkeeping {
    /// Invariant name → at least one parented, checkable entity was
    /// observed in at least one judged frame. Otherwise the world
    /// is flat and the audit reports `hierarchy_absent`
    /// (informational — a flat world is a coherent scene the rule
    /// cannot judge, not a misauthored invariant).
    pub matched: std::collections::HashSet<String>,
    /// Invariant name → at least one in-scope parented entity was
    /// seen whose parent was never observable with a
    /// GlobalTransform. When `matched` is ALSO empty the world is
    /// NOT flat — the audit reports `unjudgeable_parent` instead of
    /// the flattering `hierarchy_absent`; when `matched` is set,
    /// the audit still discloses the partial blindness once.
    pub unjudgeable_parent: std::collections::HashSet<String>,
    /// Invariant name → a resolution/config failure was seen during
    /// the run (already reported as `oracle_error`); the audit
    /// suppresses its vacuity entry so the game is not blamed for
    /// an author-side problem.
    pub config_errored: std::collections::HashSet<String>,
    /// (invariant name, error class) pairs already reported as
    /// `oracle_error` this run — the once-per-distinct-error dedup.
    pub oracle_error_seen: std::collections::HashSet<String>,
    /// Invariant name → the fold engine (entity keys, worst = the
    /// MAXIMUM deviation — compositional severity is incoherence).
    pub folders: std::collections::HashMap<String, crate::episode::EpisodeFolder<u32>>,
    /// Invariant name → the pass-side held statistics (the
    /// passing-invariant trace — see
    /// [`crate::held::EpisodeHeldStats`]), the same doctrine as
    /// pairwise's.
    pub episode_stats: std::collections::HashMap<String, crate::held::EpisodeHeldStats>,
}

impl ConsistencyBookkeeping {
    /// The folder shape for a consistency invariant: entity keys,
    /// larger-is-worse. Constructed explicitly (not via Default) so
    /// the direction choice stays visible at the type-erased
    /// HashMap boundary.
    fn new_folder() -> crate::episode::EpisodeFolder<u32> {
        crate::episode::EpisodeFolder::new(
            crate::episode::DEFAULT_ROSTER_CAP,
            crate::episode::WorstDirection::LargerIsWorse,
        )
    }
}

/// One frame's consistency outcome for one invariant (the
/// owned-data phase-split doctrine: computed under `&World`, applied
/// after the borrow ends).
struct DtcOutcome {
    /// The frame's violating observations: (entity index, deviation
    /// magnitude — the largest per-element affine delta between the
    /// observed and parent-composed GlobalTransform). Empty = a
    /// clean frame, which ends any open episode.
    obs: Vec<crate::episode::Observation<u32>>,
    /// The driver-leaf sample in the driver's own unit; `None` on a
    /// gap (driver resource absent / leaf non-finite).
    driver: Option<f64>,
    /// First config error this frame, if any.
    config_error: Option<String>,
    /// At least one parented, checkable entity was observed this
    /// frame (observability for the vacuity audit).
    parented_seen: bool,
    /// At least one in-scope parented entity was seen this frame
    /// whose PARENT was unobservable (no readable GlobalTransform)
    /// — the anti-false-confidence signal that distinguishes a
    /// broken-but-unreadable hierarchy from a flat world.
    unjudgeable_parent: bool,
    /// How many parented, checkable entities the walk judged this
    /// frame — the workload figure behind `entity_judgments`. Zero
    /// when nothing was checkable.
    checkable_count: usize,
}

/// The default ε when the author omits `tolerance`: 1e-4 absolute
/// per element. Healthy f32 composition of sane-magnitude chains
/// reproduces propagation's own arithmetic (same `mul_transform`
/// path) and sits at ~0..1e-5; real misses (frozen globals under a
/// moving parent) diverge by whole units. The default separates
/// noise from signal on both sides; deep hierarchies or
/// large-coordinate scenes loosen it explicitly (a deliberate
/// author decision, not a hidden growth formula).
pub const DEFAULT_CONSISTENCY_TOLERANCE: f32 = 1e-4;

/// The default startup-settle skip when the author omits
/// `grace_frames`: the first 3 frames. Relationship application and
/// the first propagation settle asynchronously during the startup
/// sequence, so the earliest frames can legitimately disagree; three
/// frames covers a warm-up + first-propagation cycle at any sane tps
/// without opening a blind spot an author must remember to close.
pub const DEFAULT_GRACE_FRAMES: u32 = 3;

/// Per-entity opt-out honored by `derived_transform_consistency` in
/// ANY selection mode: attach it once to an entity that legitimately
/// writes its `GlobalTransform` after transform propagation (camera
/// smoothing ghosts, interpolation layers) and the rule ignores that
/// entity, scoped to that rule alone — no other enumeration or
/// oracle consults it. Reflect-registered under takeover so the
/// selector/exclude grammar can coherently co-name it, but never a
/// requirement: the component is plain ECS, usable whether or not
/// this game reflects its own types.
///
/// Lives here — not in the takeover-gated `plugin` module — because
/// this recorder (always compiled, every feature combination,
/// including in-process builds with no `plugin` module) resolves
/// the marker's component ID to honor it. `plugin` re-exports the
/// type under its original crate path, so consumer code is
/// unchanged.
#[derive(
    bevy_ecs::component::Component,
    Debug,
    Default,
    Clone,
    Copy,
    PartialEq,
    Eq,
    bevy_reflect::Reflect,
)]
#[reflect(bevy_ecs::reflect::ReflectComponent)]
pub struct SkipDerivedConsistency;

/// Record one frame of `derived_transform_consistency` evidence.
/// Driver-loop recorder doctrine (see the pairwise recorder): the
/// driver calls this AFTER `app.update()` returns, with the frame
/// index and tps, so both sides of the identity are read at the
/// same settled, post-propagation frame.
pub fn record_derived_transform_consistency(world: &mut World, frame: u64, tps: u64) {
    // Clone the relevant invariants out so no world borrow survives
    // the phase split below. Grace frames are per-invariant (each
    // authoring decides its own startup-settle window): an invariant
    // whose grace has not elapsed is simply not judged this frame.
    // Skipping without observing cannot close an open episode —
    // grace frames always precede an invariant's first judged frame
    // (frames increase monotonically), so none can be open.
    let invariants: Vec<Invariant> = world
        .get_resource::<ScenarioInvariants>()
        .map(|invariants| {
            invariants
                .0
                .iter()
                .filter(|i| i.rule == InvariantRule::DerivedTransformConsistency)
                .filter(|i| frame >= i.grace_frames.unwrap_or(DEFAULT_GRACE_FRAMES) as u64)
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
        world.resource_scope(|world, mut book: Mut<ConsistencyBookkeeping>| {
            let mut violations = world.resource_mut::<RunViolations>();
            for inv in &invariants {
                book.config_errored.insert(inv.name.clone());
                let why = &no_atr_message("derived-transform consistency checks");
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
    let mut outcomes: Vec<(Invariant, DtcOutcome)> = Vec::with_capacity(invariants.len());
    for inv in &invariants {
        outcomes.push((
            inv.clone(),
            eval_consistency(world, &registry, inv, frame, tps),
        ));
    }
    drop(registry);

    // Write phase: fold, judge soak, report one finding per
    // soaked episode.
    world.resource_scope(|world, mut book: Mut<ConsistencyBookkeeping>| {
        let mut violations = world.resource_mut::<RunViolations>();
        for (inv, outcome) in outcomes {
            apply_dtc_outcome(&inv, &outcome, frame, &mut book, &mut violations.0);
        }
    });
}

/// Enumerate the frame's incoherent parent-child triples for one
/// invariant (the read phase). Selection: an optional
/// `with_component` selector (absent or `all` = every parented
/// entity — the whole hierarchy is exactly what this rule is for),
/// minus `exclude_components` prefix matches, minus the per-instance
/// `SkipDerivedConsistency` marker.
fn eval_consistency(
    world: &World,
    registry: &TypeRegistry,
    inv: &Invariant,
    frame: u64,
    tps: u64,
) -> DtcOutcome {
    let err = |why: String| DtcOutcome {
        obs: vec![],
        driver: None,
        config_error: Some(why),
        parented_seen: false,
        unjudgeable_parent: false,
        checkable_count: 0,
    };
    // Driver sample: declared leaf, or the frame clock (seconds) —
    // pairwise's grammar verbatim.
    let driver = match (inv.driver_resource.as_deref(), inv.driver_path.as_deref()) {
        (Some(res_name), Some(path)) => {
            match sample_declared_driver(world, registry, res_name, path) {
                Ok(Some(v)) => Some(v),
                // Absent resource: a gap, not an error (late
                // arrivals, mid-run removal, teardown).
                Ok(None) => None,
                Err(why) => return err(why),
            }
        }
        (None, None) => frame_clock(frame, tps),
        // Lint guarantees both halves or neither; defensive only.
        _ => {
            return err(
                "derived_transform_consistency's driver needs both halves (validated)".into(),
            )
        }
    };
    // The selector, when present, names a component TYPE the entity
    // must carry (validation + lint guarantee resolution when the
    // type exists at all). Absent or `all` = the whole hierarchy.
    let mut selector_id: Option<bevy_ecs::component::ComponentId> = None;
    if let Some(crate::enums::TargetSelector::WithComponent { name }) = inv.target_selector.as_ref()
    {
        let type_reg = match resolve_registration(registry, name, "component") {
            Ok(t) => t,
            Err(why) => return err(why),
        };
        match world.components().get_valid_id(type_reg.type_id()) {
            Some(id) => selector_id = Some(id),
            // Registered but carrying no component role in this
            // app: NO entity can bear it, so no frame is ever
            // checkable — the flat-world vacuity audit's call.
            None => {
                return DtcOutcome {
                    obs: vec![],
                    driver,
                    config_error: None,
                    parented_seen: false,
                    unjudgeable_parent: false,
                    checkable_count: 0,
                };
            }
        }
    }
    // exclude_components: component IDs resolved from the prefixes'
    // matched TYPE PATHS. A prefix can name a whole module, so
    // exclusion is decided per TYPE PATH (any registered type whose
    // path starts with a prefix — the progress-surface exclude
    // semantics), then reduced to the component IDs entities
    // actually carry. Prefixes are deliberately NOT lint-probed:
    // exact resolution is the wrong question for a prefix.
    let exclude_ids: Vec<bevy_ecs::component::ComponentId> = if inv.exclude_components.is_empty() {
        Vec::new()
    } else {
        registry
            .iter()
            .filter(|r| {
                inv.exclude_components.iter().any(|prefix| {
                    !prefix.trim().is_empty()
                        && r.type_info()
                            .type_path()
                            .starts_with(prefix.trim_start_matches("::"))
                })
            })
            .filter_map(|r| world.components().get_valid_id(r.type_id()))
            .collect()
    };
    // The relationship triple's component IDs. Absent IDs mean this
    // app has no hierarchy at all (or no transforms) — a flat scene,
    // the vacuity audit's call; frames are clean.
    let (Some(childof_id), Some(transform_id), Some(global_id)) = (
        world
            .components()
            .component_id::<bevy_ecs::hierarchy::ChildOf>(),
        world
            .components()
            .component_id::<bevy_transform::prelude::Transform>(),
        world
            .components()
            .component_id::<bevy_transform::prelude::GlobalTransform>(),
    ) else {
        return DtcOutcome {
            obs: vec![],
            driver,
            config_error: None,
            parented_seen: false,
            unjudgeable_parent: false,
            checkable_count: 0,
        };
    };
    // The per-instance opt-out marker (registered under takeover;
    // None = no entity has ever carried it — nothing to skip).
    let skip_id = world.components().component_id::<SkipDerivedConsistency>();
    let tolerance = inv.tolerance.unwrap_or(DEFAULT_CONSISTENCY_TOLERANCE);
    let mut obs: Vec<crate::episode::Observation<u32>> = Vec::new();
    let mut parented_seen = false;
    let mut unjudgeable_parent = false;
    // The per-frame judged-entity count — the workload figure for
    // the held record (entities re-walked each frame are
    // re-counted, the component tier's entity_judgments doctrine).
    let mut checkable_count: usize = 0;
    // Walk every archetype bearing the parent link (the pairwise
    // enumerator's doctrine: component-ID set tests at the
    // archetype, typed reads per entity).
    for archetype in world.archetypes().iter() {
        if !archetype.components().contains(&childof_id) {
            continue;
        }
        // The triple must be checkable; the marker opts the whole
        // entity out; the selector (when declared) must match; the
        // exclude prefixes subtract. All archetype-level, decided
        // before any per-entity work.
        if !archetype.components().contains(&transform_id)
            || !archetype.components().contains(&global_id)
        {
            continue;
        }
        if let Some(skip_id) = skip_id {
            if archetype.components().contains(&skip_id) {
                continue;
            }
        }
        if let Some(selector_id) = selector_id {
            if !archetype.components().contains(&selector_id) {
                continue;
            }
        }
        if !exclude_ids.is_empty()
            && archetype
                .iter_components()
                .any(|id| exclude_ids.contains(&id))
        {
            continue;
        }
        for entry in archetype.entities() {
            let Ok(entity_ref) = world.get_entity(entry.id()) else {
                continue;
            };
            let Some(child_of) = entity_ref.get::<bevy_ecs::hierarchy::ChildOf>() else {
                continue;
            };
            // The triple must be readable on the child; only then
            // can the PARENT's observability decide judgeability.
            let Some(transform) = entity_ref.get::<bevy_transform::prelude::Transform>() else {
                continue;
            };
            let Some(global) = entity_ref.get::<bevy_transform::prelude::GlobalTransform>() else {
                continue;
            };
            // The parent must be observable with a GlobalTransform;
            // an orphaned/dead parent link is bevy's own
            // maintenance bug family (relationship validators,
            // staged child disposal) — not a compositional miss.
            // Unobservable THIS frame means no evidence in EITHER
            // direction for that entity.
            let Some(parent_global) =
                world.get::<bevy_transform::prelude::GlobalTransform>(child_of.parent())
            else {
                // Disclosed blindness, not a silent skip: an
                // in-scope parented entity exists but the
                // compositional identity cannot be judged for it.
                unjudgeable_parent = true;
                continue;
            };
            parented_seen = true;
            checkable_count += 1;
            // The identity: observed vs parent-composed — the same
            // `mul_transform` arithmetic propagation itself uses, so
            // a propagated-then-untouched triple agrees exactly and
            // only divergence that survived propagation shows up.
            // The ε contract is ELEMENTWISE on the affine delta:
            // translation and each basis column count separately,
            // and the observation value is the largest of the four
            // (the severity a report should name).
            let expected = parent_global.mul_transform(*transform);
            let obs_aff = global.affine();
            let exp_aff = expected.affine();
            let dev_x = (obs_aff.matrix3.x_axis - exp_aff.matrix3.x_axis).length();
            let dev_y = (obs_aff.matrix3.y_axis - exp_aff.matrix3.y_axis).length();
            let dev_z = (obs_aff.matrix3.z_axis - exp_aff.matrix3.z_axis).length();
            let dev_t = (obs_aff.translation - exp_aff.translation).length();
            let deviation = dev_x.max(dev_y).max(dev_z).max(dev_t);
            // NaN deltas (NaN Transform family) prove nothing — the
            // finite_transforms oracle owns NaN findings; filter
            // rather than double-report.
            if !deviation.is_finite() {
                continue;
            }
            if deviation > tolerance {
                obs.push(crate::episode::Observation {
                    key: entity_ref.id().index().index(),
                    value: deviation,
                });
            }
        }
    }
    DtcOutcome {
        obs,
        driver,
        config_error: None,
        parented_seen,
        unjudgeable_parent,
        checkable_count,
    }
}

/// Write-phase fold of one frame's consistency outcome: feed the
/// engine, judge any closed episode against the soak, report ONE
/// finding with the episode's evidence.
fn apply_dtc_outcome(
    inv: &Invariant,
    outcome: &DtcOutcome,
    frame: u64,
    book: &mut ConsistencyBookkeeping,
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
    if outcome.parented_seen {
        book.matched.insert(inv.name.clone());
    }
    if outcome.unjudgeable_parent {
        book.unjudgeable_parent.insert(inv.name.clone());
    }
    // Held-trace workload fold: judged frames and entity-judgments.
    // Only frames that judged ≥1 checkable parented entity count
    // (the flat tiers' "judged something" semantics).
    if outcome.checkable_count > 0 {
        book.episode_stats
            .entry(inv.name.clone())
            .or_default()
            .observe_frame(None, Some(outcome.checkable_count as u64));
    }
    let closed = {
        let folder = book
            .folders
            .entry(inv.name.clone())
            .or_insert_with(ConsistencyBookkeeping::new_folder);
        // Frame order is guaranteed by the driver loop; push can close
        // an episode only via this frame (violation ends, gap, or the
        // non-consecutive close-and-reopen).
        folder.push(frame, outcome.driver, &outcome.obs)
    };
    if let Some(closed) = closed {
        book.episode_stats
            .entry(inv.name.clone())
            .or_default()
            .observe_episode_span(closed.driver_span, closed.start_frame);
        judge_dtc_episode(inv, &closed, violations);
    }
}

/// Soak judgment and finding emission for one closed consistency
/// episode: discard sub-soak episodes as transients (a frame hitch
/// during relationship churn, a teleport settling); report one
/// finding with the worst deviation and the roster evidence.
fn judge_dtc_episode(
    inv: &Invariant,
    ep: &crate::episode::ClosedEpisode<u32>,
    violations: &mut crate::state::Violations,
) {
    let Some(window_s) = inv.window_s else {
        return; // Validated required; defensive only.
    };
    if ep.driver_span < window_s as f64 {
        // Sub-soak: a transient divergence the persistence doctrine
        // discards.
        return;
    }
    let worst = &ep.worst;
    let tolerance = inv.tolerance.unwrap_or(DEFAULT_CONSISTENCY_TOLERANCE);
    // The soak span unit doctrine: the driver leaf's OWN unit —
    // seconds only for the frame-clock default.
    let (span, span_unit) = match inv.driver_path.as_deref() {
        Some(_) => (ep.driver_span, " driver units"),
        None => (ep.driver_span, "s of frame clock"),
    };
    violations.report(
        crate::rules::Rule::DerivedTransformConsistency.name(),
        &inv.name,
        format!(
            "entity #{}'s GlobalTransform diverged from its parent-composed \
             expectation by up to {:.6} (> {}) for {:.3}{} across frames {}–{} \
             ({} diverging entit{}, worst deviation {:.6}); a divergence this \
             persistent is a stale or foreign-written global, not a settling \
             transient",
            worst.key,
            worst.value,
            tolerance,
            span,
            span_unit,
            ep.start_frame,
            ep.end_frame,
            ep.distinct_keys,
            if ep.distinct_keys == 1 { "y" } else { "ies" },
            worst.value,
        ),
        ep.start_frame,
    );
    // Attach the roster evidence (`_meta`) to the just-reported
    // entry — one finding, full story. First-write-wins, mirroring
    // the pairwise detail doctrine: a second soaked episode extends
    // last_frame but never swaps the evidence out from under the
    // first finding.
    if let Some(entry) = violations.entries.get_mut(&(
        crate::rules::Rule::DerivedTransformConsistency
            .name()
            .to_string(),
        inv.name.clone(),
    )) {
        entry.last_frame = entry.last_frame.max(ep.end_frame);
        if entry.meta.is_none() {
            entry.meta = Some(crate::discovery::surface::ViolationMeta {
                stepping: None,
                pairwise_episode: None,
                consistency_episode: Some(consistency_meta_of(ep)),
                forbidden_episode: None,
                stagnation_episode: None,
                occupancy_episode: None,
                unpromoted_rule: None,
            });
        }
    }
}

/// The `_meta` evidence block for one closed consistency episode:
/// the roster maps the generic engine fields into the wire-stable
/// names (entity, worst_deviation).
fn consistency_meta_of(
    ep: &crate::episode::ClosedEpisode<u32>,
) -> crate::discovery::surface::ConsistencyEpisodeMeta {
    crate::discovery::surface::ConsistencyEpisodeMeta {
        sampled_entities: ep.distinct_keys,
        roster: ep
            .roster
            .iter()
            .map(|r| crate::discovery::surface::ConsistencyRosterEntry {
                entity: r.key,
                first_frame: r.first_frame,
                worst_deviation: r.worst_value,
                worst_frame: r.worst_frame,
                violating_frames: r.violating_frames,
            })
            .collect(),
        roster_truncated: ep.roster_truncated,
    }
}

/// Run-end drain: any consistency episode still open at the last
/// frame is a complete stretch on its own; fold it out and judge it
/// like any other closed episode. Called once, after the driver
/// loop ends and before the bookkeeping is audited.
pub fn close_consistency_episodes(world: &mut World) {
    let invariants: Vec<Invariant> = world
        .get_resource::<ScenarioInvariants>()
        .map(|invariants| {
            invariants
                .0
                .iter()
                .filter(|i| i.rule == InvariantRule::DerivedTransformConsistency)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    if invariants.is_empty() {
        return;
    }
    world.resource_scope(|world, mut book: Mut<ConsistencyBookkeeping>| {
        let mut violations = world.resource_mut::<RunViolations>();
        for inv in &invariants {
            let closed = book
                .folders
                .get_mut(&inv.name)
                .and_then(|folder| folder.close_run());
            if let Some(closed) = closed {
                book.episode_stats
                    .entry(inv.name.clone())
                    .or_default()
                    .observe_episode_span(closed.driver_span, closed.start_frame);
                judge_dtc_episode(inv, &closed, &mut violations.0);
            }
        }
    });
}

/// Post-run audit for `derived_transform_consistency` invariants:
/// a world where no parented, checkable entity was EVER observed is
/// a flat scene the rule cannot judge — reported as the INFORMATIONAL
/// `hierarchy_absent` (never gating `vacuous_invariant`: a flat
/// world is a coherent scene, not a misauthored invariant, and a
/// gating vacuity would punish every legitimately hierarchy-free
/// game that scopes the rule). Config-errored invariants are
/// suppressed (their `oracle_error` already told the story).
pub fn audit_derived_transform_consistency(
    invariants: &[Invariant],
    book: &ConsistencyBookkeeping,
    last_frame: u64,
) -> Vec<crate::state::ViolationEntry> {
    let mut entries = Vec::new();
    for inv in invariants
        .iter()
        .filter(|i| i.rule == InvariantRule::DerivedTransformConsistency)
    {
        if book.config_errored.contains(&inv.name) {
            continue;
        }
        if !book.matched.contains(&inv.name) {
            // Vocabulary split (anti-false-confidence): a world with
            // NO parented entity at all is flat — the coherent-
            // scene `hierarchy_absent` note. A world that DID have
            // an in-scope parented entity whose parent was never
            // observable is NOT flat — the note must say so, or a
            // broken-but-unreadable hierarchy reads as a coherent
            // flat world.
            if book.unjudgeable_parent.contains(&inv.name) {
                entries.push(crate::state::ViolationEntry {
                    rule: crate::rules::Rule::UnjudgeableParent.name().to_string(),
                    target: inv.name.clone(),
                    first_frame: 0,
                    last_frame,
                    count: 1,
                    detail: format!(
                        "invariant '{}' found a parented entity in scope but \
                         its parent was never observable with a GlobalTransform \
                         over the whole run; the compositional identity could not \
                         be judged for that subtree — this is not a flat world. \
                         Give the parent a readable GlobalTransform, or scope the \
                         rule away from the unjudgeable subtree",
                        inv.name
                    ),
                    last_detail: format!(
                        "invariant '{}' never saw a readable parent GlobalTransform",
                        inv.name
                    ),
                    meta: None,
                });
            } else {
                entries.push(crate::state::ViolationEntry {
                    rule: crate::rules::Rule::HierarchyAbsent.name().to_string(),
                    target: inv.name.clone(),
                    first_frame: 0,
                    last_frame,
                    count: 1,
                    detail: format!(
                        "invariant '{}' never found a parented entity to check over \
                         the whole run; a flat world is a coherent scene this rule \
                         cannot judge — scope the rule to a hierarchy-bearing \
                         selector, or drop it from this scenario",
                        inv.name
                    ),
                    last_detail: format!(
                        "invariant '{}' never found a parented entity to check",
                        inv.name
                    ),
                    meta: None,
                });
            }
        } else if book.unjudgeable_parent.contains(&inv.name) {
            // Partial blindness: some parented entities were judged
            // but at least one in-scope subtree stayed unreadable —
            // disclosed once, never allowed to silently vanish into
            // a green run.
            entries.push(crate::state::ViolationEntry {
                rule: crate::rules::Rule::UnjudgeableParent.name().to_string(),
                target: inv.name.clone(),
                first_frame: 0,
                last_frame,
                count: 1,
                detail: format!(
                    "invariant '{}' judged some parented entities, but at least \
                     one in-scope parented entity's parent was never observable \
                     with a GlobalTransform; its compositional identity went \
                     unchecked for the whole run — a partial verdict, not a full \
                     one",
                    inv.name
                ),
                last_detail: format!(
                    "invariant '{}' has an unchecked subtree: an in-scope \
                     parent was never observable with a GlobalTransform",
                    inv.name
                ),
                meta: None,
            });
        }
    }
    entries
}

// ===========================================================================
// entity_stagnation (judgement layer, second vocabulary)
// ===========================================================================
//
// Per-entity rate-form liveness: each selected entity's watched leaf
// must not stay digest-constant while the driver advances. The
// reviewed semantics (proposal §7, spec, review-corrected):
//
// - SCOPED WATCH: `watch_path` names the leaf whose digest is
//   compared across samples; a mutation anywhere outside the path
//   does not clear stagnation.
// - leaf_where PRECONDITION: only entities whose `path == Variant`
//   holds are watched; a definite-resolution failure is an
//   oracle_error (probe-time doctrine), never a silent pass.
// - DESPAWN = ACQUITTAL: a vanished key ends its stint; its accrued
//   span freezes into the roster but the JUDGMENT rides the shared
//   episode (aggregate-first). Turnover never refunds the span —
//   an episode whose last key despawns is judged on the span it
//   already accrued. Equally: CHANGE = the stint ends; a re-stuck
//   key starts a FRESH stint (a new observation with value 0
//   re-enters the set only once it is stuck across two samples
//   again).
// - AGGREGATE-FIRST SHARED TIMELINE: one folder per invariant,
//   K = (index, generation), obs = the still-stuck set; empty obs
//   closes the episode. A RELAY — obs never empties while no
//   individual entity soaks — FIRES once the union span crosses
//   window_s: the authored invariant says "this watch must not
//   stay stagnant continuously for window_s", and in a relay some
//   guest is frozen at every instant. Deliberate doctrine, pinned
//   by the battery (the batched-service-tick false-positive trap).
// - OBSERVATION VALUE = the entity's ACCRUED stuck-span in driver
//   units, LargerIsWorse — the fold's `worst` naturally names the
//   longest-stuck guest and `worst_frame` lands at its stint end.
// - SAMPLED CADENCE: the leaf is read every sampling_s of driver
//   advance (scheduled as a frame stride); two consecutive
//   equal-digest samples credit driver(k) − driver(k−1), and the
//   episode's telescoped span is compared against window_s. The
//   folder is a `with_step` folder — the stride generalizes the
//   consecutiveness check, it does not remove it.

/// Bookkeeping for the entity_stagnation recorders: the fold
/// engines plus the per-invariant SAMPLER state. Sampling is
/// per-invariant (each invariant declares its own cadence), so
/// the state cannot ride the world — it lives beside the fold.
#[derive(bevy_ecs::resource::Resource, Default)]
pub struct StagnationBookkeeping {
    /// Invariant name → the selector resolved AND ≥1 watchable
    /// entity was enumerated in at least one sampled frame
    /// (vacuity evidence — the selector never resolving is
    /// `vacuous_invariant`; the selector resolving but
    /// leaf_where never holding is a PASS with evidence, a
    /// held-trace workload fact, NOT vacuity).
    pub matched: std::collections::HashSet<String>,
    /// Invariant name → the selector resolved at least once
    /// (distinct from `matched`: leaf_where may never hold, the
    /// held trace tells that story).
    pub selector_resolved: std::collections::HashSet<String>,
    /// Invariant name → a resolution/config failure was seen
    /// during the run (already reported as `oracle_error`); the
    /// audit suppresses its vacuity entry (N-B/N-C doctrine).
    pub config_errored: std::collections::HashSet<String>,
    /// (invariant name, error class) pairs already reported as
    /// `oracle_error` this run — the once-per-distinct-error dedup.
    pub oracle_error_seen: std::collections::HashSet<String>,
    /// Invariant name → the sampled-cadence fold engine.
    pub folders:
        std::collections::HashMap<String, crate::episode::EpisodeFolder<crate::episode::StagKey>>,
    /// Invariant name → the pass-side held statistics.
    pub episode_stats: std::collections::HashMap<String, crate::held::EpisodeHeldStats>,
    /// Invariant name → per-key sampler state: the previous
    /// sample's digest (None until the first digest is taken).
    pub last_digest:
        std::collections::HashMap<String, std::collections::HashMap<crate::episode::StagKey, u64>>,
    /// Invariant name → per-key accrued stuck-span in driver
    /// units, and the DRIVER LEAF VALUE at the key's last sample
    /// (the inter-sample delta base). Reset (removed) on change;
    /// a key absent from a sample is REMOVED (its stint ended —
    /// despawn, precondition lapse, or selector exit are
    /// indistinguishable at this layer and all acquit).
    pub stint: std::collections::HashMap<
        String,
        std::collections::HashMap<crate::episode::StagKey, (f64 /* span */, f64 /* driver_base */)>,
    >,
    /// Invariant name → the frame stride the cadence resolves to
    /// (cached at first use; the folder's `step`).
    pub strides: std::collections::HashMap<String, u64>,
}

/// One frame's stagnation outcome for one invariant (the
/// owned-data phase-split doctrine: computed under `&World`,
/// applied after the borrow ends). The digests are the raw
/// &World-derived data; stint accrual (which needs the previous
/// sample) happens in the write phase against the bookkeeping.
struct StagOutcome {
    /// The frame's per-entity leaf digests (raw; every
    /// precondition-satisfying, path-resolvable entity).
    digests: Vec<(crate::episode::StagKey, u64)>,
    /// Whether THIS frame was a sampling frame for the invariant.
    sampled: bool,
    /// The driver-leaf sample (driver units) on a sampled frame.
    driver: Option<f64>,
    /// First config error this frame, if any.
    config_error: Option<String>,
    /// How many selector-component entities the enumeration
    /// walked this sampled frame (precondition or not — the
    /// held-trace workload figure and the selector-resolves
    /// evidence).
    enumerated: usize,
}

/// Driver-loop recorder for `entity_stagnation` invariants: the
/// per-entity rate-form liveness oracle (second vocabulary). The
/// cadence is per-invariant: the watched leaf is digest-read only
/// on SAMPLING frames (a frame stride resolved from sampling_s,
/// the driver's rate, and tps), and the still-stuck set folds
/// into the shared-timeline episode via `with_step` — the stride
/// generalizes consecutiveness, the sampler defines continuity.
pub fn record_entity_stagnation(world: &mut World, frame: u64, tps: u64) {
    let invariants: Vec<Invariant> = world
        .get_resource::<ScenarioInvariants>()
        .map(|invariants| {
            invariants
                .0
                .iter()
                .filter(|i| i.rule == InvariantRule::EntityStagnation)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    if invariants.is_empty() {
        return;
    }
    let Some(atr) = world.get_resource::<AppTypeRegistry>().cloned() else {
        world.resource_scope(|world, mut book: Mut<StagnationBookkeeping>| {
            let mut violations = world.resource_mut::<RunViolations>();
            for inv in &invariants {
                book.config_errored.insert(inv.name.clone());
                let why = &no_atr_message("entity stagnation checks");
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

    // Read phase: every invariant is evaluated for cadence; only
    // sampling frames walk entities and take digests.
    let mut outcomes: Vec<(Invariant, StagOutcome)> = Vec::with_capacity(invariants.len());
    for inv in &invariants {
        outcomes.push((
            inv.clone(),
            eval_stagnation(world, &registry, inv, frame, tps),
        ));
    }
    drop(registry);

    // Write phase: fold, judge soak, report.
    world.resource_scope(|world, mut book: Mut<StagnationBookkeeping>| {
        let mut violations = world.resource_mut::<RunViolations>();
        for (inv, outcome) in outcomes {
            apply_stag_outcome(&inv, outcome, frame, tps, &mut book, &mut violations.0);
        }
    });
}

/// The frame stride a stagnation invariant's cadence resolves to:
/// sampling_s driver units -> wall seconds (via the driver's
/// declared rate) -> frames (via tps), rounded, floored at 1.
/// Validation guarantees rate > 0.
fn stagnation_stride(inv: &Invariant, tps: u64) -> u64 {
    let sampling_s = inv.sampling_s.unwrap_or(1.0) as f64;
    let rate = match inv.driver.as_deref() {
        Some("resource") => inv.driver_units_per_second.unwrap_or(1.0) as f64,
        _ => 1.0,
    };
    let frame_s = 1.0 / tps as f64;
    ((sampling_s / rate) / frame_s).round().max(1.0) as u64
}

/// Resolve the scoped watch leaf on one live component value.
/// bevy_reflect's path grammar has NO variant dispatch — it
/// assumes the variant is known ahead of time — so a
/// variant-qualified authoring path (`state.Queued.waited`)
/// needs its own walk: a segment naming the LIVE variant is a
/// confirmation (the leaf_where precondition in path form), and
/// field segments route to the active variant through the
/// ordinary path machinery. Any failure — a different live
/// variant, a missing field inside it — is a PRECONDITION LAPSE
/// (the entity is not watchable this frame), never an error.
fn scoped_watch_leaf<'a>(
    component: &'a dyn bevy_reflect::Reflect,
    path: &str,
) -> Option<&'a dyn bevy_reflect::PartialReflect> {
    use bevy_reflect::GetPath;
    let mut current: &dyn bevy_reflect::Reflect = component;
    for segment in path.split('.').filter(|s| !s.is_empty()) {
        if let bevy_reflect::ReflectRef::Enum(e) = current.as_partial_reflect().reflect_ref() {
            if e.variant_name() == segment {
                // Variant confirmation: consume the segment and
                // stay on the enum — the next field segment
                // routes to the active variant.
                continue;
            }
        }
        let seg = format!(".{segment}");
        let next = current.reflect_path(seg.as_str()).ok()?.try_as_reflect()?;
        current = next;
    }
    Some(current.as_partial_reflect())
}

/// Read phase: one invariant's frame outcome. Non-sampling frames
/// are cheap no-ops for the watch itself — digest reads are the
/// cost being sampled away. The first sample lands at the first
/// frame divisible by the stride, then every stride-th frame
/// after; the schedule is deterministic from frame 0 and cannot
/// drift.
fn eval_stagnation(
    world: &World,
    registry: &TypeRegistry,
    inv: &Invariant,
    frame: u64,
    tps: u64,
) -> StagOutcome {
    use bevy_reflect::GetPath;
    let err = |why: String, sampled: bool| StagOutcome {
        digests: vec![],
        sampled,
        driver: None,
        config_error: Some(why),
        enumerated: 0,
    };
    // Validation guarantees these; the Options are defensive only.
    let Some(crate::enums::TargetSelector::WithComponent { name }) = inv.target_selector.as_ref()
    else {
        return err(
            "entity_stagnation requires a with_component selector (validated)".into(),
            false,
        );
    };
    let Some(watch_path) = inv.watch_path.as_deref() else {
        return err(
            "entity_stagnation requires 'watch_path' (validated)".into(),
            false,
        );
    };
    let type_reg = match resolve_registration(registry, name, "component") {
        Ok(t) => t,
        Err(why) => return err(why, false),
    };
    let Some(reflect_component) = type_reg.data::<bevy_ecs::reflect::ReflectComponent>() else {
        return err(not_a_reflected_component_message(name), false);
    };
    // Cadence: the stride, cached per-invariant in the bookkeeping
    // (write phase), is recomputed here cheaply and must agree —
    // it is a pure function of the immutable invariant + tps.
    let stride = stagnation_stride(inv, tps);
    let sampled = frame.is_multiple_of(stride);
    if !sampled {
        return StagOutcome {
            digests: vec![],
            sampled: false,
            driver: None,
            config_error: None,
            enumerated: 0,
        };
    }
    // leaf_where parses to (path, variant); validation guaranteed
    // the `path == Variant` shape, the probe guaranteed definite
    // resolution. Variant-equality is checked against the LIVE
    // value each sample — the precondition is a per-frame fact.
    let leaf_where = inv.leaf_where.as_deref().map(|lw| {
        let (p, v) = lw.split_once("==").expect("validated shape");
        (p.trim().to_string(), v.trim().to_string())
    });
    let mut digests: Vec<(crate::episode::StagKey, u64)> = Vec::new();
    let mut enumerated = 0usize;
    if let Some(component_id) = world.components().get_valid_id(type_reg.type_id()) {
        for archetype in world.archetypes().iter() {
            if !archetype.components().contains(&component_id) {
                continue;
            }
            for entry in archetype.entities() {
                let Ok(entity_ref) = world.get_entity(entry.id()) else {
                    continue;
                };
                let Some(reflect) = reflect_component.reflect(entity_ref) else {
                    continue;
                };
                enumerated += 1;
                // leaf_where precondition: `path == Variant` must
                // hold NOW (variant-equality only).
                if let Some((lw_path, lw_variant)) = &leaf_where {
                    let holds = reflect
                        .reflect_path(lw_path.as_str())
                        .ok()
                        .and_then(|leaf| {
                            leaf.try_as_reflect().map(|r| {
                                matches!(
                                    r.as_partial_reflect().reflect_ref(),
                                    bevy_reflect::ReflectRef::Enum(e)
                                        if e.variant_name() == lw_variant
                                )
                            })
                        })
                        .unwrap_or(false);
                    if !holds {
                        continue;
                    }
                }
                // The scoped watch leaf, variant-aware (bevy_reflect's
                // path grammar cannot dispatch variants). An entity
                // in the WRONG enum variant fails resolution
                // legitimately — the precondition lapsed (the
                // leaf_where mechanism when the watch path is
                // variant-qualified), not an error.
                let reflect: &dyn bevy_reflect::Reflect = reflect.as_reflect();
                let Some(leaf) = scoped_watch_leaf(reflect, watch_path) else {
                    continue;
                };
                let mut hasher = crate::digest::fnv_hasher();
                crate::digest::hash_canonical(leaf, &mut hasher);
                digests.push((
                    crate::episode::StagKey::new(
                        entity_ref.id().index().index(),
                        entity_ref.id().generation().to_bits(),
                    ),
                    hasher,
                ));
            }
        }
    }
    // Driver sample: the declared resource leaf, or the frame
    // clock (seconds) — read on sampled frames only.
    let driver = match inv.driver.as_deref() {
        Some("resource") => {
            let (res_name, path) = (
                inv.driver_resource.as_deref().expect("validated"),
                inv.driver_path.as_deref().expect("validated"),
            );
            match sample_declared_driver(world, registry, res_name, path) {
                Ok(Some(v)) => Some(v),
                // Absent resource: a gap, not an error.
                Ok(None) => None,
                Err(why) => return err(why, true),
            }
        }
        _ => frame_clock(frame, tps),
    };
    StagOutcome {
        digests,
        sampled: true,
        driver,
        config_error: None,
        enumerated,
    }
}

/// Write-phase fold of one frame's stagnation outcome: digest
/// diffs against the previous sample accrue per-key stints, the
/// still-stuck set is pushed into the sampled-cadence folder, and
/// any closed episode is judged against the soak.
///
/// A `None` driver on a sampled frame is a GAP: stints do not
/// accrue (the clock is unobservable) and the folder closes its
/// open episode (the engine's own gap doctrine); digests still
/// UPDATE (the sample happened) so the next interval is measured
/// from this frame's value.
fn apply_stag_outcome(
    inv: &Invariant,
    outcome: StagOutcome,
    frame: u64,
    tps: u64,
    book: &mut StagnationBookkeeping,
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
    let stride = stagnation_stride(inv, tps);
    book.strides.entry(inv.name.clone()).or_insert(stride);
    if !outcome.sampled {
        return;
    }
    // Selector-resolution evidence: the enumeration walked ≥1
    // selector-component entity. `matched` (watchability: the
    // rule actually judged digests) requires ≥1 digest — a
    // resolved selector whose entities never satisfy
    // leaf_where is a PASS with evidence: the held entry must
    // still exist (frames_judged counts the sampled frames the
    // selector enumerated, entity_judgments stays 0), so the
    // collector admits on EITHER marker. Distinct from the DTC
    // mirror where zero judged frames means never exercised:
    // here the watch WAS live — the world simply never offered a
    // qualifying leaf.
    if outcome.enumerated > 0 {
        book.selector_resolved.insert(inv.name.clone());
        if !outcome.digests.is_empty() {
            book.matched.insert(inv.name.clone());
        }
        book.episode_stats
            .entry(inv.name.clone())
            .or_default()
            .observe_frame(None, Some(outcome.digests.len() as u64));
    }
    let last_digest = book.last_digest.entry(inv.name.clone()).or_default();
    let stint = book.stint.entry(inv.name.clone()).or_default();
    // Step 1: close stints for keys ABSENT from this sample
    // (despawn, precondition lapse, selector exit — all
    // acquittal; the span is already banked in the roster and
    // the key leaves the stuck set).
    let present: std::collections::HashSet<crate::episode::StagKey> =
        outcome.digests.iter().map(|(k, _)| *k).collect();
    stint.retain(|k, _| present.contains(k));
    // Step 2: accrue or reset stints from the digest diff.
    let mut stuck: Vec<crate::episode::Observation<crate::episode::StagKey>> = Vec::new();
    for (key, digest) in &outcome.digests {
        let prev = last_digest.get(key);
        match prev {
            Some(prev_digest) if prev_digest == digest => {
                // Still stuck: accrue the inter-sample driver
                // delta onto the stint.
                if let Some(driver) = outcome.driver {
                    let entry = stint.entry(*key).or_insert((0.0, driver));
                    // Gap handling: if the stint's base is ahead
                    // of the driver (a retreat or a gap in
                    // between), credit no negative span — forward
                    // deltas only (the episode engine's doctrine).
                    let delta = driver - entry.1;
                    if delta > 0.0 {
                        entry.0 += delta;
                    }
                    entry.1 = driver;
                    stuck.push(crate::episode::Observation {
                        key: *key,
                        value: entry.0 as f32,
                    });
                } else {
                    // A gap this sample: the stint survives (the
                    // digest did not change) but accrues nothing
                    // and the key enters the push with its
                    // CURRENT accrued span so the folder can
                    // close the episode (its gap doctrine).
                    if let Some(entry) = stint.get(key) {
                        stuck.push(crate::episode::Observation {
                            key: *key,
                            value: entry.0 as f32,
                        });
                    }
                }
            }
            _ => {
                // Changed (or first sample): a FRESH stint base.
                // First sample seeds the digest with span 0; the
                // key becomes stuck only when the NEXT sample's
                // digest is equal.
                stint.remove(key);
                if let Some(driver) = outcome.driver {
                    // Seed the base so the NEXT equal sample
                    // accrues from here; span stays 0 until then.
                    stint.insert(*key, (0.0, driver));
                }
            }
        }
    }
    // Step 3: update the digest memory (the sample happened).
    last_digest.clear();
    for (key, digest) in &outcome.digests {
        last_digest.insert(*key, *digest);
    }
    // Step 4: push the still-stuck set into the sampled-cadence
    // folder. Empty stuck set closes any open episode; a gap
    // (driver None) also closes (the engine's gap doctrine).
    let folder = book
        .folders
        .entry(inv.name.clone())
        .or_insert_with(|| StagnationBookkeeping::folder_for(stride));
    let closed = folder.push(frame, outcome.driver, &stuck);
    if let Some(closed) = closed {
        book.episode_stats
            .entry(inv.name.clone())
            .or_default()
            .observe_episode_span(closed.driver_span, closed.start_frame);
        judge_stag_episode(inv, &closed, violations);
    }
}

impl StagnationBookkeeping {
    /// The sampled-cadence folder for one stagnation invariant:
    /// composite entity keys, LargerIsWorse (the accrued span
    /// grows within a stint; the worst observation naturally
    /// names the longest-stuck guest), constructed with the
    /// invariant's stride so pushes weld ON-cadence and an
    /// off-cadence push still closes-and-reopens.
    fn folder_for(stride: u64) -> crate::episode::EpisodeFolder<crate::episode::StagKey> {
        crate::episode::EpisodeFolder::with_step(
            crate::episode::DEFAULT_ROSTER_CAP,
            crate::episode::WorstDirection::LargerIsWorse,
            stride,
        )
    }
}

/// Soak judgment and finding emission for one closed stagnation
/// episode: sub-soak stretches are lulls, discarded; a soaked
/// episode is ONE finding naming the longest-stuck guest, with
/// the roster carrying every participant (a crowd is one
/// incident, many participants — aggregate-first doctrine).
fn judge_stag_episode(
    inv: &Invariant,
    ep: &crate::episode::ClosedEpisode<crate::episode::StagKey>,
    violations: &mut crate::state::Violations,
) {
    let Some(window_s) = inv.window_s else {
        return; // Validated required; defensive only.
    };
    if ep.driver_span < window_s as f64 {
        // Sub-soak: a lull the window doctrine discards.
        return;
    }
    let worst = &ep.worst;
    // The soak span is in the DRIVER LEAF'S OWN UNIT — seconds
    // only for the frame-clock default.
    let (span, span_unit) = match inv.driver.as_deref() {
        Some("resource") => (ep.driver_span, " driver units"),
        _ => (ep.driver_span, "s of frame clock"),
    };
    violations.report(
        crate::rules::Rule::EntityStagnation.name(),
        &inv.name,
        format!(
            "entity #{} (gen {}) stayed digest-constant on its watched leaf for {:.3}{} \
             (worst single-entity stuck-span; the EPISODE's union span across all \
             {} stuck entit{} is {:.3}{}, frames {}-{} ({} frame{})); a stagnation \
             this long while the driver advanced is a per-entity livelock, not a lull",
            worst.key.index,
            worst.key.generation,
            worst.value,
            span_unit,
            ep.distinct_keys,
            if ep.distinct_keys == 1 { "y" } else { "ies" },
            span,
            span_unit,
            ep.start_frame,
            ep.end_frame,
            ep.end_frame - ep.start_frame + 1,
            if ep.end_frame - ep.start_frame == 0 {
                ""
            } else {
                "s"
            },
        ),
        ep.start_frame,
    );
    if let Some(entry) = violations.entries.get_mut(&(
        crate::rules::Rule::EntityStagnation.name().to_string(),
        inv.name.clone(),
    )) {
        entry.last_frame = entry.last_frame.max(ep.end_frame);
        if entry.meta.is_none() {
            entry.meta = Some(crate::discovery::surface::ViolationMeta {
                stepping: None,
                pairwise_episode: None,
                consistency_episode: None,
                forbidden_episode: None,
                stagnation_episode: Some(stagnation_meta_of(ep)),
                occupancy_episode: None,
                unpromoted_rule: None,
            });
        }
    }
}

/// Convert a closed stagnation episode into its `_meta` evidence
/// block. The roster's per-entry span is the accrued stuck-span
/// frozen at the key's stint end (change, despawn, or run end) —
/// a participant that left mid-episode keeps its personal soak
/// measure readable even though the shared timeline continued.
fn stagnation_meta_of(
    ep: &crate::episode::ClosedEpisode<crate::episode::StagKey>,
) -> crate::discovery::surface::StagnationEpisodeMeta {
    crate::discovery::surface::StagnationEpisodeMeta {
        sampled_entities: ep.distinct_keys,
        roster: ep
            .roster
            .iter()
            .map(|r| crate::discovery::surface::StagnationRosterEntry {
                entity: r.key.index,
                generation: r.key.generation,
                first_frame: r.first_frame,
                last_frame: r.worst_frame,
                stuck_span: r.worst_value,
                stuck_frames: r.violating_frames,
            })
            .collect(),
        roster_truncated: ep.roster_truncated,
    }
}

/// Run-end drain: an episode still open at the last frame is a
/// complete stretch on its own — fold it out and judge it (the
/// pairwise/consistency doctrine verbatim).
pub fn close_stagnation_episodes(world: &mut World) {
    let invariants: Vec<Invariant> = world
        .get_resource::<ScenarioInvariants>()
        .map(|invariants| {
            invariants
                .0
                .iter()
                .filter(|i| i.rule == InvariantRule::EntityStagnation)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    if invariants.is_empty() {
        return;
    }
    world.resource_scope(|world, mut book: Mut<StagnationBookkeeping>| {
        let mut violations = world.resource_mut::<RunViolations>();
        for inv in &invariants {
            let closed = book
                .folders
                .get_mut(&inv.name)
                .and_then(|folder| folder.close_run());
            if let Some(closed) = closed {
                book.episode_stats
                    .entry(inv.name.clone())
                    .or_default()
                    .observe_episode_span(closed.driver_span, closed.start_frame);
                judge_stag_episode(inv, &closed, &mut violations.0);
            }
        }
    });
}

/// Post-run audit for `entity_stagnation` invariants: a selector
/// that never RESOLVED proves nothing — `vacuous_invariant`. A
/// selector that resolved but whose entities never satisfied
/// leaf_where (or the watch path) is a PASS with evidence (the
/// held trace carries the story); a rule whose selector resolved
/// and whose entities were watched but nothing was stuck ALSO
/// passes — stagnation is an absence finding, a clean world is
/// the success case.
pub fn audit_entity_stagnation(
    invariants: &[Invariant],
    book: &StagnationBookkeeping,
    last_frame: u64,
) -> Vec<crate::state::ViolationEntry> {
    let mut entries = Vec::new();
    for inv in invariants
        .iter()
        .filter(|i| i.rule == InvariantRule::EntityStagnation)
    {
        if book.config_errored.contains(&inv.name) {
            continue;
        }
        if !book.selector_resolved.contains(&inv.name) {
            entries.push(crate::state::ViolationEntry {
                rule: crate::rules::Rule::VacuousInvariant.name().to_string(),
                target: inv.name.clone(),
                first_frame: 0,
                last_frame,
                count: 1,
                detail: format!(
                    "invariant '{}' matched zero entities over the whole run; \
                     the check proves nothing until there is something to check",
                    inv.name
                ),
                last_detail: format!("invariant '{}' matched zero entities", inv.name),
                meta: None,
            });
        }
    }
    entries
}

// ---------------------------------------------------------------------
// spawn_safety: the archetype-entry spatial recorder (judgement
// layer, remaining-spatial-family part 1).
//
// Doctrine: entries are judged ONCE, at the frame they enter the
// selected archetype — a newborn entity bearing the target
// component at spawn, or a PRE-EXISTING entity first acquiring it
// (component-addition is the judged event by default; `spawn_only`
// filters to brand-new (index, generation) pairs). Anchors are
// snapshotted at the END of the same judging frame (post-update,
// the harness's standard observation point): same-frame anchors
// PARTICIPATE — an anchor spawned alongside the entry is a
// participant in the judgment, not an excuse for it. There is NO
// episode and NO soak: the transition frame is the whole verdict,
// so a single violation gates immediately (the respawn-ring
// incident class — bodies appearing inside the danger zone — is a
// state bug AT the transition, not a persistence bug the window
// doctrine exists to smooth over).
//
// The "newborn" definition is a GENERATION-TRACKED, FRAME-OVER-
// FRAME ARCHETYPE DIFF, never bevy's Added<T>: the recorder keeps
// the previous frame's entity snapshot — every (index, generation)
// pair mapped to whether it bore the target component — and an
// entry is a current member that is either a pair new to the
// snapshot (a spawn) or a known pair newly bearing the component
// (component-addition). Generation tracking makes id reuse sound
// in both directions, and the same two facts seal the tempting
// same-frame corner (a despawn and same-slot recycle landing in
// one update, which would read as "stayed a member"):
// (a) `Commands::spawn` allocates EAGERLY AT QUEUE TIME, before
// any despawn command applies — the same-system
// despawn-then-spawn shape hands out a fresh slot (verified
// empirically: same-tick re-fills never observed at 36v0, always
// a fresh index), and
// (b) the slots that DO recycle come back GENERATION-BUMPED:
// bevy 0.20's World-path despawn marks the index free with
// `mark_free(index, 1)`, bumping the generation BEFORE the free,
// so a recycled pair is by construction a pair the previous
// snapshot never recorded — it arrives as an unknown pair and is
// judged anew (a stale membership record cannot hide an entry).
// The raw `EntityAllocator` recycles freed slots bit-identically
// (no bump at that layer — its own allocator tests assert
// UNIQUENESS of allocation, which bit-identity serves), but games
// and harnesses only despawn through the bumped World path or
// the command queue, so the bump covers every observable
// recycle. Pinned by
// same_frame_death_and_recycle_takes_a_fresh_slot_and_is_judged.
// ---------------------------------------------------------------------

/// Bookkeeping for `spawn_safety` invariants across frames (the
/// pairwise bookkeeping doctrine, one shape per rule):
/// observability for the vacuity audit, once-per-error-class
/// dedup, the generation-tracked existence/membership snapshot,
/// and the pass-side workload counts.
#[derive(Resource, Default)]
pub struct SpawnSafetyBookkeeping {
    /// Invariant name → at least one anchor was resolvable in at
    /// least one judged frame. Otherwise the rule could never
    /// judge an entry (nothing was ever near anything) and the
    /// audit reports `vacuous_invariant` — the anchor side of the
    /// pairwise "never had a pair to check" doctrine.
    pub matched: std::collections::HashSet<String>,
    /// Invariant name → run-total frames with ≥1 resolvable anchor
    /// — the held trace's `frames_matched` (existence evidence: the
    /// frames the rule COULD have judged an entry).
    pub frames_matched: std::collections::HashMap<String, u64>,
    /// Invariant name → a resolution/config failure was seen
    /// during the run (already reported as `oracle_error`); the
    /// audit suppresses its vacuity entry so the game is not
    /// blamed for an author-side problem.
    pub config_errored: std::collections::HashSet<String>,
    /// (invariant name, error class) pairs already reported as
    /// `oracle_error` this run — the once-per-distinct-error dedup.
    pub oracle_error_seen: std::collections::HashSet<String>,
    /// Invariant name → the previous judged frame's entity
    /// snapshot: (index, generation) → bore the target component
    /// last frame. Recorded for EVERY entity (not just members):
    /// distinguishing a spawn from a component-addition requires
    /// knowing the pair existed before, which a members-only
    /// snapshot cannot express.
    pub prev_snapshot:
        std::collections::HashMap<String, std::collections::HashMap<(u32, u32), bool>>,
    /// Invariant name → run-total entries JUDGED (diff entries
    /// surviving the spawn_only filter, when authored) — the
    /// workload figure behind the held trace's entry counts.
    pub entries_judged: std::collections::HashMap<String, u64>,
    /// Invariant name → run-total entries judged CLEAN (entered
    /// at least min_distance from every same-frame anchor) — the
    /// pass-side matched-newborns count the held trace carries so
    /// a clean run distinguishes exercised from never-triggered.
    pub entries_clean: std::collections::HashMap<String, u64>,
    /// Invariant name → run-total entries that VIOLATED (appeared
    /// within min_distance of a same-frame anchor).
    pub entries_violating: std::collections::HashMap<String, u64>,
}

/// One frame's spawn_safety outcome for one invariant (the
/// owned-data phase-split doctrine: computed under `&World`,
/// applied after the borrow ends).
struct SsOutcome {
    /// This frame's entity snapshot: (index, generation) → global
    /// translation for every selector-matching, Transform-carrying
    /// entity (members), plus the existence/membership map for the
    /// ENTIRE world (the spawn-vs-component-addition diff needs
    /// pre-existing non-members too).
    members: Vec<((u32, u32), [f32; 3])>,
    /// The full snapshot for the next frame's diff: pair → was a
    /// member this frame.
    next_snapshot: std::collections::HashMap<(u32, u32), bool>,
    /// The anchor set's (entity, global translation) snapshots,
    /// taken at the end of this frame (same-frame anchors
    /// participate). The entity key lets the judge exclude the
    /// entering entity ITSELF from the anchor set — an `all`
    /// anchor selector (or overlapping target/anchor components)
    /// would otherwise place every entry at distance 0 from
    /// itself and trivially violate.
    anchors: Vec<((u32, u32), [f32; 3])>,
    /// First config error this frame, if any.
    config_error: Option<String>,
    /// At least one anchor resolvable this frame (observability
    /// for the vacuity audit).
    any_anchor: bool,
}

/// The driver-loop post-update recorder for `spawn_safety`: diff
/// the current frame's selected membership against the previous
/// frame's snapshot, judge each entry against the same-frame
/// anchors, report violations immediately (no episode, no soak —
/// the transition frame is the whole verdict).
pub fn record_spawn_safety(world: &mut World, frame: u64) {
    let invariants: Vec<Invariant> = world
        .get_resource::<ScenarioInvariants>()
        .map(|invariants| {
            invariants
                .0
                .iter()
                .filter(|i| i.rule == InvariantRule::SpawnSafety)
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
        world.resource_scope(|world, mut book: Mut<SpawnSafetyBookkeeping>| {
            let mut violations = world.resource_mut::<RunViolations>();
            for inv in &invariants {
                book.config_errored.insert(inv.name.clone());
                let why = &no_atr_message("spawn safety selectors");
                report_config_error_once(
                    &mut violations.0,
                    &mut book.oracle_error_seen,
                    &inv.name,
                    &format!("invariant '{}': {why}", inv.name),
                    frame,
                );
            }
        });
        return;
    };
    let registry = atr.read();
    // Compute phase: everything read under &World, owned data out.
    let outcomes: Vec<(Invariant, SsOutcome)> = invariants
        .iter()
        .filter_map(|inv| compute_ss_outcome(world, &registry, inv).map(|o| (inv.clone(), o)))
        .collect();
    drop(registry);
    // Apply phase: bookkeeping folds + immediate violation
    // reporting. No episode machinery exists here by design.
    world.resource_scope(|world, mut book: Mut<SpawnSafetyBookkeeping>| {
        let mut violations = world.resource_mut::<RunViolations>();
        for (inv, outcome) in &outcomes {
            if let Some(why) = &outcome.config_error {
                book.config_errored.insert(inv.name.clone());
                report_config_error_once(
                    &mut violations.0,
                    &mut book.oracle_error_seen,
                    &inv.name,
                    &format!("invariant '{}': {why}", inv.name),
                    frame,
                );
                continue;
            }
            if outcome.any_anchor {
                book.matched.insert(inv.name.clone());
                *book.frames_matched.entry(inv.name.clone()).or_insert(0) += 1;
            }
            // The diff. A stored previous snapshot means judged
            // transitions; a MISSING one is the invariant's first
            // judged frame — the observed membership is the
            // scenario's initial scene (the frame-0-is-not-a-spawn
            // doctrine: the scenario authored those entities; the
            // run never observed their entry).
            if book.prev_snapshot.contains_key(&inv.name) {
                let spawn_only = inv.spawn_only.unwrap_or(false);
                let min_distance = inv.min_distance.unwrap_or(0.0);
                let prev = book.prev_snapshot[&inv.name].clone();
                for ((index, generation), pos) in &outcome.members {
                    match prev.get(&(*index, *generation)) {
                        // Known member: stayed a member — no
                        // transition, no judgment.
                        Some(true) => {}
                        // Known NON-member: the entity existed
                        // last frame and now bears the component —
                        // a component-addition entry (judged by
                        // default; filtered out under spawn_only).
                        Some(false) => {
                            if spawn_only {
                                continue;
                            }
                            book.judge_one(
                                &mut violations.0,
                                inv,
                                ((*index, *generation), *pos),
                                &outcome.anchors,
                                min_distance,
                                frame,
                            );
                        }
                        // Unknown pair: a brand-new entity — a
                        // spawn entry under both semantics.
                        None => {
                            book.judge_one(
                                &mut violations.0,
                                inv,
                                ((*index, *generation), *pos),
                                &outcome.anchors,
                                min_distance,
                                frame,
                            );
                        }
                    }
                }
            }
            book.prev_snapshot
                .insert(inv.name.clone(), outcome.next_snapshot.clone());
        }
    });
}

impl SpawnSafetyBookkeeping {
    /// Judge one entry against the same-frame anchors: fold the
    /// workload count, then report a finding when the entry
    /// appeared within min_distance of any anchor (nearest
    /// anchor wins the finding's evidence), else fold the clean
    /// count.
    fn judge_one(
        &mut self,
        violations: &mut crate::state::Violations,
        inv: &Invariant,
        entry: ((u32, u32), [f32; 3]),
        anchors: &[((u32, u32), [f32; 3])],
        min_distance: f32,
        frame: u64,
    ) {
        *self.entries_judged.entry(inv.name.clone()).or_default() += 1;
        let ((index, generation), pos) = entry;
        let threshold_sq = min_distance * min_distance;
        // The NEAREST anchor carries the finding: minimum
        // distance over all violating anchors.
        let mut worst: Option<(usize, f32)> = None;
        let entry_pair = (index, generation);
        for (ai, (anchor_pair, anchor)) in anchors.iter().enumerate() {
            // The entry itself is never its own anchor: an `all`
            // anchor selector (or overlapping target/anchor
            // components) would otherwise measure every entry at
            // distance 0 from itself.
            if *anchor_pair == entry_pair {
                continue;
            }
            let dx = pos[0] - anchor[0];
            let dy = pos[1] - anchor[1];
            let dz = pos[2] - anchor[2];
            let dist_sq = dx * dx + dy * dy + dz * dz;
            // NaN translations yield NaN distances — the
            // finite_transforms oracle owns NaN findings; skip
            // rather than double-report.
            if !dist_sq.is_finite() {
                continue;
            }
            if dist_sq < threshold_sq {
                let d = dist_sq.sqrt();
                match worst {
                    Some((_, wd)) if wd <= d => {}
                    _ => worst = Some((ai, d)),
                }
            }
        }
        match worst {
            Some((ai, d)) => {
                let (_, anchor_pos) = anchors[ai];
                *self.entries_violating.entry(inv.name.clone()).or_default() += 1;
                violations.report(
                    crate::rules::Rule::SpawnSafety.name(),
                    &inv.name,
                    format!(
                        "entity #{}v{} entered the '{}' archetype at ({:.3}, {:.3}, {:.3}), \
                         within min_distance ({:.3}) of a same-frame anchor at ({:.3}, \
                         {:.3}, {:.3}) — nearest distance {:.3}; a body appearing inside \
                         the danger zone is a state bug at the transition frame, not a \
                         transient",
                        index,
                        generation,
                        selector_display(inv),
                        pos[0],
                        pos[1],
                        pos[2],
                        min_distance,
                        anchor_pos[0],
                        anchor_pos[1],
                        anchor_pos[2],
                        d,
                    ),
                    frame,
                );
            }
            None => {
                *self.entries_clean.entry(inv.name.clone()).or_default() += 1;
            }
        }
    }
}

/// The compute phase for one invariant, under `&World`: resolve
/// both selectors, walk the world once building the
/// existence/membership snapshot, collect the members' settled
/// positions and the same-frame anchor positions.
fn compute_ss_outcome(
    world: &World,
    registry: &bevy_reflect::TypeRegistry,
    inv: &Invariant,
) -> Option<SsOutcome> {
    let err = |why: String| SsOutcome {
        members: vec![],
        next_snapshot: std::collections::HashMap::new(),
        anchors: vec![],
        config_error: Some(why),
        any_anchor: false,
    };
    // Validation guarantees these; the Options are defensive only.
    let Some(crate::enums::TargetSelector::WithComponent { name }) = inv.target_selector.as_ref()
    else {
        return Some(err(
            "spawn_safety requires a with_component target selector (validated)".into(),
        ));
    };
    if inv.min_distance.is_none() {
        return Some(err(
            "spawn_safety requires 'min_distance' (validated)".into()
        ));
    }
    let type_reg = match resolve_registration(registry, name, "component") {
        Ok(t) => t,
        Err(why) => return Some(err(why)),
    };
    // The anchor selector's component id, when it names one.
    let anchor_id = match inv.min_distance_from.as_ref() {
        Some(crate::enums::TargetSelector::WithComponent { name }) => {
            match resolve_registration(registry, name, "component") {
                Ok(t) => Some((name.clone(), t.type_id())),
                Err(why) => return Some(err(why)),
            }
        }
        Some(crate::enums::TargetSelector::All) => None,
        // Validation requires the field; defensive only.
        None => {
            return Some(err(
                "spawn_safety requires 'min_distance_from' (validated)".into()
            ))
        }
    };
    let member_id = world.components().get_valid_id(type_reg.type_id());
    let anchor_cid = match &anchor_id {
        // Resolution succeeded above; the component SHOULD be
        // valid. A non-valid id means it exists in the registry
        // but not the world's component store yet — treat as
        // zero anchors (same-frame absence, not an error).
        Some((_, tid)) => world.components().get_valid_id(*tid),
        None => None,
    };
    // One walk of the world: members (selector-bearing, spatially
    // observable entities with their settled global positions),
    // the existence/membership snapshot, and the anchor
    // positions. Membership = target component + a settled
    // GlobalTransform (the pairwise "checkable" doctrine: a
    // marker without a position cannot be judged, and its FIRST
    // observable frame is its judged entry — so an entity that
    // gains a global later is judged then, never silently
    // skipped).
    let mut members: Vec<((u32, u32), [f32; 3])> = Vec::new();
    let mut next_snapshot: std::collections::HashMap<(u32, u32), bool> =
        std::collections::HashMap::new();
    let mut anchors: Vec<((u32, u32), [f32; 3])> = Vec::new();
    for entity_ref in world.iter_entities() {
        let id = entity_ref.id();
        let pair = (id.index().index(), id.generation().to_bits());
        // Position read: the settled GlobalTransform (post-update
        // doctrine). Entities without one are not spatial
        // participants this frame.
        let Some(global) = entity_ref.get::<bevy_transform::components::GlobalTransform>() else {
            next_snapshot.insert(pair, false);
            continue;
        };
        let p = global.translation();
        let pos = [p.x, p.y, p.z];
        let is_member = member_id.is_some_and(|cid| entity_ref.contains_id(cid));
        next_snapshot.insert(pair, is_member);
        if is_member {
            members.push((pair, pos));
        }
        let is_anchor = match &anchor_id {
            // with_component anchors: component + GlobalTransform
            // (checked via contains_id; the anchor need not carry
            // a Transform — its settled global IS the position).
            Some((_, _)) => anchor_cid.is_some_and(|cid| entity_ref.contains_id(cid)),
            // all-anchors: every entity with a settled global.
            None => true,
        };
        if is_anchor {
            anchors.push((pair, pos));
        }
    }
    let anchor_count = anchors.len();
    Some(SsOutcome {
        members,
        next_snapshot,
        anchors,
        config_error: None,
        any_anchor: anchor_count > 0,
    })
}

/// Display form of the target selector's component name (the
/// finding's vocabulary for the entered archetype).
fn selector_display(inv: &Invariant) -> String {
    match inv.target_selector.as_ref() {
        Some(crate::enums::TargetSelector::WithComponent { name }) => name.clone(),
        _ => "all".to_string(),
    }
}

/// Run-end vacuity audit for `spawn_safety` (the pairwise audit's
/// doctrine): an invariant whose anchors never resolved could
/// never judge an entry — the rule proved nothing until there was
/// something to be near. Reported as `vacuous_invariant`
/// (author-side blame), suppressed when a config error already
/// spoke.
pub fn audit_spawn_safety(
    invariants: &[Invariant],
    book: &SpawnSafetyBookkeeping,
    last_frame: u64,
) -> Vec<crate::state::ViolationEntry> {
    let mut entries = Vec::new();
    for inv in invariants
        .iter()
        .filter(|i| i.rule == InvariantRule::SpawnSafety)
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
                    "invariant '{}' never resolved an anchor over the whole run; the \
                     check proves nothing until there is something to be near",
                    inv.name
                ),
                last_detail: format!(
                    "invariant '{}' never resolved an anchor to judge entries against",
                    inv.name
                ),
                meta: None,
            });
        }
    }
    entries
}

// ---------------------------------------------------------------------
// occupancy_cap: the local-density crowd recorder (judgement layer,
// spatial family, WARN-ONLY).
//
// Doctrine: more than `cap` selected entities within `radius` of a
// SELECTED ENTITY'S POSITION (self-inclusive counting — the entity
// itself is within its own radius, so the count is always >= 1).
// The enumeration is ENTITY-WITNESSED by design: the ring of
// entities spaced just past radius around an empty center is a
// documented NON-FIRING case (settlement — exact "any point"
// counting is a float-flake factory and can ship at promotion time
// if a real litmus shows the ring case matters). Crowd frames fold
// into a union-timeline episode per invariant keyed by composite
// (index, generation) — StagKey reused verbatim, no third key type
// — and only an episode soaking past window_s (driver-leaf units)
// becomes a finding, roster = WITNESSING entities only.
//
// WARN-ONLY BY CONSTRUCTION: Rule::OccupancyCap sits in
// Rule::is_informational(), so every finding this recorder emits —
// crowd episodes AND the never-a-crowd vacuity note — renders as an
// ordinary violations[] entry that STRUCTURALLY CANNOT gate run
// status (driver.rs filters informational rules out of every
// status flip; sweep fingerprints skip them). This is the first
// warn-only INVARIANT rule, so the vacuity audit deliberately does
// NOT emit `vacuous_invariant` (a gating rule) for a never-populated
// selector: the no-crowd note emits under occupancy_cap's own name.
//
// Enumeration determinism (settlement Q6): entities are collected
// BY ARCHETYPE ORDER into an index-ordered list, a uniform-grid
// spatial hash (cell size = radius, so any entity within radius of
// a point lies in the surrounding 3x3 cell block) answers counts,
// and observations are emitted by walking the SAME index-ordered
// list — NEVER by iterating hash buckets (insertion order there is
// not part of the contract).
// ---------------------------------------------------------------------

/// Bookkeeping for `occupancy_cap` invariants across frames (the
/// pairwise bookkeeping doctrine, one shape per rule):
/// observability for the never-a-crowd audit, once-per-error-class
/// dedup, the fold engines, and the pass-side held statistics.
#[derive(Resource, Default)]
pub struct OccupancyBookkeeping {
    /// Invariant name → the selector resolved AND at least one
    /// checkable entity (marker + Transform) existed in some
    /// observed frame. Otherwise never-a-crowd — an occupancy rule
    /// with nothing to select proves nothing (reported under the
    /// rule's own informational name, never vacuous_invariant).
    pub matched: std::collections::HashSet<String>,
    /// Invariant name → a resolution/config failure was seen
    /// during the run (already reported as `oracle_error`); the
    /// audit suppresses its vacuity entry (N-B/N-C doctrine).
    pub config_errored: std::collections::HashSet<String>,
    /// (invariant name, error class) pairs already reported as
    /// `oracle_error` this run — the once-per-distinct-error dedup.
    pub oracle_error_seen: std::collections::HashSet<String>,
    /// Invariant name → the fold engine (composite keys; one
    /// folder per invariant — folder identity IS invariant
    /// identity, the forbidden_region doctrine).
    pub folders:
        std::collections::HashMap<String, crate::episode::EpisodeFolder<crate::episode::StagKey>>,
    /// Invariant name → the pass-side held statistics, folding
    /// alongside the engine so sub-soak episodes survive the soak
    /// judgment.
    pub episode_stats: std::collections::HashMap<String, crate::held::EpisodeHeldStats>,
}

/// One frame's occupancy outcome for one invariant (owned-data
/// phase-split doctrine: computed under `&World`, applied after
/// the borrow ends).
struct OccOutcome {
    /// The frame's witnessing entities (self-inclusive
    /// neighborhood count > cap) with their counts. Index-ordered
    /// (the enumeration order), never bucket-ordered.
    witnesses: Vec<crate::episode::Observation<crate::episode::StagKey>>,
    /// The driver-leaf sample in the driver's own unit; `None` on
    /// a gap (driver resource absent / leaf non-finite).
    driver: Option<f64>,
    /// First config error this frame, if any.
    config_error: Option<String>,
    /// The selector resolved AND >= 1 checkable entity existed
    /// this frame.
    populated: bool,
    /// How many checkable (marker + Transform) entities the
    /// enumeration walked this frame — the held-trace workload
    /// figure.
    checkable_count: usize,
}

/// Driver-loop recorder for `occupancy_cap` invariants: the
/// local-density crowd oracle, WARN-ONLY (see the block comment for
/// the no-fail-power construction). Every-frame evaluation, the
/// same post-update observation doctrine as the rest of the family.
pub fn record_occupancy_cap(world: &mut World, frame: u64, tps: u64) {
    let invariants: Vec<Invariant> = world
        .get_resource::<ScenarioInvariants>()
        .map(|invariants| {
            invariants
                .0
                .iter()
                .filter(|i| i.rule == InvariantRule::OccupancyCap)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    if invariants.is_empty() {
        return;
    }
    let Some(atr) = world.get_resource::<AppTypeRegistry>().cloned() else {
        world.resource_scope(|world, mut book: Mut<OccupancyBookkeeping>| {
            let mut violations = world.resource_mut::<RunViolations>();
            for inv in &invariants {
                book.config_errored.insert(inv.name.clone());
                let why = &no_atr_message("occupancy cap checks");
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

    // Read phase: every invariant evaluates every frame (no
    // sampling — density reads are the cost being paid; a
    // sampled cadence stays TBD by the cost model).
    let mut outcomes: Vec<(Invariant, OccOutcome)> = Vec::with_capacity(invariants.len());
    for inv in &invariants {
        outcomes.push((
            inv.clone(),
            eval_occupancy(world, &registry, inv, frame, tps),
        ));
    }
    drop(registry);

    // Write phase: fold, judge soak, report.
    world.resource_scope(|world, mut book: Mut<OccupancyBookkeeping>| {
        let mut violations = world.resource_mut::<RunViolations>();
        for (inv, outcome) in outcomes {
            apply_occ_outcome(&inv, &outcome, frame, &mut book, &mut violations.0);
        }
    });
}

/// Read phase: one invariant's frame outcome. Determinism
/// contract: the entity list is collected in archetype iteration
/// order and observation emission walks that SAME list — the
/// spatial hash is queried only for counts, never iterated.
fn eval_occupancy(
    world: &World,
    registry: &TypeRegistry,
    inv: &Invariant,
    frame: u64,
    tps: u64,
) -> OccOutcome {
    let err = |why: String| OccOutcome {
        witnesses: vec![],
        driver: None,
        config_error: Some(why),
        populated: false,
        checkable_count: 0,
    };
    // Validation guarantees these; the Options are defensive only.
    let Some(crate::enums::TargetSelector::WithComponent { name }) = inv.target_selector.as_ref()
    else {
        return err("occupancy_cap requires a with_component selector (validated)".into());
    };
    let Some(radius) = inv.radius else {
        return err("occupancy_cap requires 'radius' (validated)".into());
    };
    let Some(cap) = inv.cap else {
        return err("occupancy_cap requires 'cap' (validated)".into());
    };
    let type_reg = match resolve_registration(registry, name, "component") {
        Ok(t) => t,
        Err(why) => return err(why),
    };
    // Marker membership is decided by archetype (the component_id
    // contains-check below); the ReflectComponent data check is
    // defensive-only (same shape as the family).
    if type_reg
        .data::<bevy_ecs::reflect::ReflectComponent>()
        .is_none()
    {
        return err(not_a_reflected_component_message(name));
    }
    // Collect (key, position) for every marker+Transform entity,
    // archetype order — the family's enumeration doctrine
    // ("carries the marker but no Transform: not checkable").
    let mut nodes: Vec<(crate::episode::StagKey, [f32; 3])> = Vec::new();
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
                let t = transform.translation;
                // NaN translations are never witnesses (NaN
                // comparisons are false) — finite_transforms owns
                // NaN findings, this recorder must not fold a NaN
                // observation into an episode.
                if !t.x.is_finite() || !t.y.is_finite() || !t.z.is_finite() {
                    continue;
                }
                nodes.push((
                    crate::episode::StagKey::new(
                        entity_ref.id().index().index(),
                        entity_ref.id().generation().to_bits(),
                    ),
                    [t.x, t.y, t.z],
                ));
            }
        }
    }
    let checkable_count = nodes.len();
    if nodes.is_empty() {
        // Registered but never instantiated: no crowd to judge —
        // the never-a-crowd audit's call, frames are clean.
        return OccOutcome {
            witnesses: vec![],
            driver: frame_clock(frame, tps),
            config_error: None,
            populated: false,
            checkable_count: 0,
        };
    }
    // Spatial hash: cell size = radius guarantees any entity
    // within radius of a point lies in the 3x3 block around the
    // point's cell. Counts only — never iterated for emission.
    let cell = radius;
    let floor_cell = |v: f32| (v / cell).floor() as i64;
    let mut hash: std::collections::HashMap<(i64, i64, i64), Vec<usize>> =
        std::collections::HashMap::with_capacity(nodes.len());
    for (i, (_, pos)) in nodes.iter().enumerate() {
        hash.entry((floor_cell(pos[0]), floor_cell(pos[1]), floor_cell(pos[2])))
            .or_default()
            .push(i);
    }
    // Witness emission: walk the index-ordered node list (the
    // deterministic stream), count neighbors in the 3x3 block
    // within radius (INCLUSIVE `<=` boundary — pinned by the
    // battery), self-inclusive (distance 0 <= radius always).
    let radius_sq = radius * radius;
    let mut witnesses: Vec<crate::episode::Observation<crate::episode::StagKey>> = Vec::new();
    for (key, pos) in nodes.iter() {
        let mut count: u32 = 0;
        let (cx, cy, cz) = (floor_cell(pos[0]), floor_cell(pos[1]), floor_cell(pos[2]));
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let Some(bucket) = hash.get(&(cx + dx, cy + dy, cz + dz)) else {
                        continue;
                    };
                    for &j in bucket {
                        let (opos, _) = (nodes[j].1, nodes[j].0);
                        let ddx = pos[0] - opos[0];
                        let ddy = pos[1] - opos[1];
                        let ddz = pos[2] - opos[2];
                        // Squared-distance compare: within radius
                        // iff d2 <= radius_sq. f32 squaring is
                        // monotonic for non-negative magnitudes,
                        // so the INCLUSIVE boundary pin (distance
                        // exactly radius counts as within) holds
                        // without a sqrt.
                        if ddx * ddx + ddy * ddy + ddz * ddz <= radius_sq {
                            count += 1;
                        }
                    }
                }
            }
        }
        if count > cap {
            witnesses.push(crate::episode::Observation {
                key: *key,
                value: count as f32,
            });
        }
    }
    // Driver sample: declared leaf, or the frame clock (seconds) —
    // pairwise's grammar (both halves or neither).
    let driver = match (inv.driver_resource.as_deref(), inv.driver_path.as_deref()) {
        (Some(res_name), Some(path)) => {
            match sample_declared_driver(world, registry, res_name, path) {
                Ok(Some(v)) => Some(v),
                // Absent resource: a gap, not an error.
                Ok(None) => None,
                Err(why) => return err(why),
            }
        }
        (None, None) => frame_clock(frame, tps),
        // Lint guarantees both halves or neither; defensive only.
        _ => return err("occupancy_cap's driver needs both halves (validated)".into()),
    };
    OccOutcome {
        witnesses,
        driver,
        config_error: None,
        populated: true,
        checkable_count,
    }
}

/// Write-phase fold of one frame's occupancy outcome: feed the
/// engine, judge any closed episode against the soak, report ONE
/// WARN-ONLY finding with the episode's evidence.
fn apply_occ_outcome(
    inv: &Invariant,
    outcome: &OccOutcome,
    frame: u64,
    book: &mut OccupancyBookkeeping,
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
    if outcome.populated {
        book.matched.insert(inv.name.clone());
    }
    // Held-trace workload fold: judged frames and entity-judgments
    // (the entity_count slot — a density rule judges entities, not
    // pairs; NEVER pair_judgments).
    if outcome.populated {
        book.episode_stats
            .entry(inv.name.clone())
            .or_default()
            .observe_frame(None, Some(outcome.checkable_count as u64));
    }
    let folder = book.folders.entry(inv.name.clone()).or_insert_with(|| {
        // Composite keys, LargerIsWorse (a bigger witnessed
        // crowd is worse), every-frame cadence (step 1) — the
        // family default.
        crate::episode::EpisodeFolder::new(
            crate::episode::DEFAULT_ROSTER_CAP,
            crate::episode::WorstDirection::LargerIsWorse,
        )
    });
    let closed = folder.push(frame, outcome.driver, &outcome.witnesses);
    if let Some(closed) = closed {
        book.episode_stats
            .entry(inv.name.clone())
            .or_default()
            .observe_episode_span(closed.driver_span, closed.start_frame);
        judge_occ_episode(inv, &closed, violations);
    }
}

/// Soak judgment and finding emission for one closed crowd
/// episode: sub-soak gatherings are transient, discarded; a
/// soaked episode is ONE WARN-ONLY finding naming the worst
/// witness, with the roster carrying every witness and the
/// `unpromoted_rule` mark set — the finding says "this fired, but
/// the rule carries no fail-power claim" (proposal §8).
fn judge_occ_episode(
    inv: &Invariant,
    ep: &crate::episode::ClosedEpisode<crate::episode::StagKey>,
    violations: &mut crate::state::Violations,
) {
    let Some(window_s) = inv.window_s else {
        return; // Validated required; defensive only.
    };
    if ep.driver_span < window_s as f64 {
        // Sub-soak: a transient gathering the window doctrine
        // discards.
        return;
    }
    let worst = &ep.worst;
    let cap = inv.cap.unwrap_or(0);
    let radius = inv.radius.unwrap_or(0.0);
    // The soak span is in the DRIVER LEAF'S OWN UNIT — seconds
    // only for the frame-clock default (pairwise's suffix
    // doctrine).
    let (span, span_unit) = match inv.driver_path.as_deref() {
        Some(_) => (ep.driver_span, " driver units"),
        None => (ep.driver_span, "s of frame clock"),
    };
    violations.report(
        crate::rules::Rule::OccupancyCap.name(),
        &inv.name,
        format!(
            "entity #{} (gen {}) saw {} entities (including itself) within \
             radius {:.3} (cap {}), sustained for {:.3}{} across frames {}–{} \
             ({} witnessing entit{}, worst count {}); a crowd this dense this \
             long is congestion, not a gathering — \
             UNPROMOTED RULE: occupancy_cap is warn-only, this finding carries \
             no fail-power claim and does not affect run status (see the \
             acceptance table)",
            worst.key.index,
            worst.key.generation,
            worst.value,
            radius,
            cap,
            span,
            span_unit,
            ep.start_frame,
            ep.end_frame,
            ep.distinct_keys,
            if ep.distinct_keys == 1 { "y" } else { "ies" },
            worst.value,
        ),
        ep.start_frame,
    );
    // Attach the roster evidence (`_meta`) to the just-reported
    // entry — one finding, full story; first-write-wins (the
    // pairwise doctrine: a second soaked episode extends
    // last_frame but never swaps the evidence).
    if let Some(entry) = violations.entries.get_mut(&(
        crate::rules::Rule::OccupancyCap.name().to_string(),
        inv.name.clone(),
    )) {
        entry.last_frame = entry.last_frame.max(ep.end_frame);
        if entry.meta.is_none() {
            entry.meta = Some(crate::discovery::surface::ViolationMeta {
                stepping: None,
                pairwise_episode: None,
                consistency_episode: None,
                forbidden_episode: None,
                stagnation_episode: None,
                occupancy_episode: Some(occupancy_meta_of(inv, ep)),
                unpromoted_rule: Some(true),
            });
        }
    }
}

/// Convert a closed crowd episode into its `_meta` evidence block:
/// witnessing entities only (never synthesized participants), with
/// the episode's radius and cap echoed so the evidence reads
/// standalone.
fn occupancy_meta_of(
    inv: &Invariant,
    ep: &crate::episode::ClosedEpisode<crate::episode::StagKey>,
) -> crate::discovery::surface::OccupancyEpisodeMeta {
    crate::discovery::surface::OccupancyEpisodeMeta {
        sampled_entities: ep.distinct_keys,
        radius: inv.radius.unwrap_or(0.0),
        cap: inv.cap.unwrap_or(0),
        roster: ep
            .roster
            .iter()
            .map(|r| crate::discovery::surface::OccupancyRosterEntry {
                entity: r.key.index,
                generation: r.key.generation,
                first_frame: r.first_frame,
                worst_count: r.worst_value as u32,
                worst_frame: r.worst_frame,
                violating_frames: r.violating_frames,
            })
            .collect(),
        roster_truncated: ep.roster_truncated,
    }
}

/// Run-end drain: an episode still open at the last frame is a
/// complete stretch on its own — fold it out and judge it (the
/// family doctrine verbatim).
pub fn close_occupancy_episodes(world: &mut World) {
    let invariants: Vec<Invariant> = world
        .get_resource::<ScenarioInvariants>()
        .map(|invariants| {
            invariants
                .0
                .iter()
                .filter(|i| i.rule == InvariantRule::OccupancyCap)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    if invariants.is_empty() {
        return;
    }
    world.resource_scope(|world, mut book: Mut<OccupancyBookkeeping>| {
        let mut violations = world.resource_mut::<RunViolations>();
        for inv in &invariants {
            let closed = book
                .folders
                .get_mut(&inv.name)
                .and_then(|folder| folder.close_run());
            if let Some(closed) = closed {
                book.episode_stats
                    .entry(inv.name.clone())
                    .or_default()
                    .observe_episode_span(closed.driver_span, closed.start_frame);
                judge_occ_episode(inv, &closed, &mut violations.0);
            }
        }
    });
}

/// Post-run audit for `occupancy_cap` invariants: a selector that
/// never had a checkable entity over the whole run never had a
/// crowd to bound — the note emits under the rule's OWN name (an
/// informational rule), never `vacuous_invariant` (a GATING rule:
/// emitting it would smuggle fail-power through the audit path —
/// the sole reason occupancy is the first warn-only invariant and
/// this audit departs from pairwise's shape). A population always
/// within cap is the healthy case; the note is an honesty device,
/// not a blame.
pub fn audit_occupancy_cap(
    invariants: &[Invariant],
    book: &OccupancyBookkeeping,
    last_frame: u64,
) -> Vec<crate::state::ViolationEntry> {
    let mut entries = Vec::new();
    for inv in invariants
        .iter()
        .filter(|i| i.rule == InvariantRule::OccupancyCap)
    {
        if book.config_errored.contains(&inv.name) {
            continue;
        }
        if !book.matched.contains(&inv.name) {
            let component = match inv.target_selector.as_ref() {
                Some(crate::enums::TargetSelector::WithComponent { name }) => name.clone(),
                _ => "component".to_string(),
            };
            let cap = inv.cap.unwrap_or(0);
            entries.push(crate::state::ViolationEntry {
                rule: crate::rules::Rule::OccupancyCap.name().to_string(),
                target: inv.name.clone(),
                first_frame: 0,
                last_frame,
                count: 1,
                detail: format!(
                    "invariant '{}' never had a crowd to judge over the whole run \
                     (no frame offered even ONE checkable '{}' \
                     entity); a population \
                     always within cap ({cap}, self-inclusive) is the healthy case — \
                     the census simply never had a bigger crowd to bound. \
                     UNPROMOTED RULE: occupancy_cap is warn-only and this note does \
                     not affect run status (see the acceptance table)",
                    inv.name, component
                ),
                last_detail: format!(
                    "invariant '{}' never had a crowd to judge (population always \
                     within cap {cap})",
                    inv.name
                ),
                meta: Some(crate::discovery::surface::ViolationMeta {
                    stepping: None,
                    pairwise_episode: None,
                    consistency_episode: None,
                    forbidden_episode: None,
                    stagnation_episode: None,
                    occupancy_episode: None,
                    unpromoted_rule: Some(true),
                }),
            });
        }
    }
    entries
}
