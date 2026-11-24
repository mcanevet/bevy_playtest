//! Held-invariant evidence: the per-invariant pass-side companion to
//! the violation list (the "passing-invariant trace").
//!
//! A passing run previously carried ZERO per-invariant evidence: a
//! deadline met at frame 299 of 300 was indistinguishable from one met
//! at frame 3, and a comfortable `ge 0` margin showed nowhere. The
//! violation list only speaks when something broke. This module adds
//! the missing half: one compact object per PASSING invariant in a
//! new `held_invariants` report section, default-on, constant-size.
//!
//! ## Why a separate resource
//!
//! The accumulators live in their own [`HeldBookkeeping`] ECS
//! resource, deliberately SEPARATE from
//! [`crate::invariant_eval::WatchedResources`]/[`crate::invariant_eval::WatchedInvariants`]:
//! the existing watched state encodes verdict logic (episode firing,
//! budget resets, earliest-held doctrine), while this module owns
//! only additive, per-invariant descriptive statistics. Keeping them
//! in a self-contained resource means the two concerns can later be
//! merged or reconciled without disturbing either's invariants.
//!
//! ## Semantics (chosen and documented)
//!
//! - `frames_matched`: RESOLVED frames — the count of frames on which
//!   the resource resolved and the leaf was read (existence
//!   evidence, exactly the documented meaning). For eventually-mode
//!   checks this counts RESOLUTIONS, not predicate-held frames, so a
//!   late-arriving resource shows `frames_matched < frame_count`
//!   honestly. (Amendment note: the semantics is deliberately
//!   "resolved", never conflated with "held".)
//! - `observed_min`/`observed_max`: extrema of the sampled leaf
//!   across matched frames — the "how close did it get" number for
//!   always-mode predicates.
//! - `worst_frame`: the frame of the extremum CLOSEST TO VIOLATING
//!   (the margin frame). For `lt`/`le` that is the max sample; for
//!   `gt`/`ge` the min sample; earliest frame wins ties. Omitted for
//!   `equals`/`ne`, where "closest to violating" is not totally
//!   ordered (both sides of the threshold violate in different
//!   ways) — the extrema already carry the distance information.
//! - `driver_advanced`: run-total driver-leaf advance — the sum of
//!   POSITIVE driver deltas across fully-observed frames. Distinct
//!   from the per-episode `driver_elapsed` budget, which resets
//!   every episode: the run-total answers "how much time did the
//!   clock actually spend moving this run". A parallel run-total
//!   accumulator may appear in a future window-semantics rework
//!   inside the watched state; integration will reconcile the two,
//!   which is exactly why this lives in its own module.
//! - `met_at_frame`/`deadline_frame`: eventually-mode only, mirroring
//!   the audited deadline formula `(eventually_s * tps).ceil()`.
//!
//! Cost: O(1) per invariant per frame (a few map lookups and float
//! compares), constant report size — one object per invariant, ~10
//! fields, no per-frame arrays. 20 invariants ≈ 3 KB (sized by test).

use bevy_ecs::prelude::*;

use crate::enums::{CheckOp, InvariantRule};
use crate::scenario::Invariant;

/// Report object: evidence that one invariant HELD — the pass-side
/// mirror of a violation entry. Serialized additively
/// (`skip_serializing_if = "Vec::is_empty"` on the section), so the
/// report's `schema_version` stays at 1: additive skipped-when-empty
/// fields never bump the schema (older readers deserialize them as
/// defaults; older writers simply omit them).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct HeldInvariant {
    /// Author's invariant name (same key violations use as target).
    pub name: String,
    /// Rule kind (`resource_check`, `stagnant_resource`, ...) — the
    /// same identifier a violation entry's `rule` field uses.
    pub rule: String,
    /// What was judged, human-readable: the reflect target path
    /// (`Resource.leaf`).
    pub target: String,
    /// Predicate in author syntax, e.g. `ge 0` (operator + threshold).
    /// Absent for rule kinds without a numeric predicate.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub predicate: Option<String>,
    /// Frames on which the resource resolved and the leaf was read
    /// (existence evidence; see module docs for the semantics).
    pub frames_matched: u64,
    /// Extrema of the sampled leaf across matched frames. Absent for
    /// rule kinds without a numeric observable leaf.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub observed_min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub observed_max: Option<f64>,
    /// Frame of the extremum closest to violating (earliest wins).
    /// Omitted for `equals`/`ne` predicates (see module docs).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub worst_frame: Option<u64>,
    /// Eventually-mode only: first frame the predicate held
    /// (earliest-holding doctrine, from `first_held_frame`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub met_at_frame: Option<u64>,
    /// Eventually-mode only: `eventually_s × tps`, ceil'ed — the
    /// same formula the post-run deadline audit uses.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub deadline_frame: Option<u64>,
    /// Stagnant-mode only: stagnation episodes that fired this run
    /// (0 on a passing run — a fired episode means the run FAILED).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub episodes_fired: Option<u64>,
    /// Stagnant-mode only: run-total driver advance — the sum of
    /// positive driver deltas across fully-observed frames.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub driver_advanced: Option<f64>,
    /// Stagnant-mode only: the driver leaf's reflect path
    /// (`DriverResource.driver_path`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub driver: Option<String>,
    /// Eventually-mode parameter, when authored.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub eventually_s: Option<f32>,
    /// Stagnant-mode window parameter, when authored.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub window_s: Option<f32>,
    /// Component-tier only (component_check / nodes_in_bounds):
    /// total entity-judgments — the sum of per-frame matched-entity
    /// counts over observed frames. A workload figure ("N
    /// entity-judgments were made"), not a census: entities
    /// surviving many frames are counted once per observed frame.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub entity_judgments: Option<u64>,
    /// Component-tier only: peak per-frame entity count the judge
    /// worked over — the largest scope any single frame presented.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub peak_entities: Option<u64>,
    /// Component-tier only: frame of that peak scope, earliest
    /// frame winning ties.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub peak_entities_frame: Option<u64>,
    /// component_check only: the smallest margin by which any judged
    /// entity's value cleared the threshold on a frame where EVERY
    /// judged entity held — the run's "how close did it get" story.
    /// Absent on runs that never had an all-held frame (those carry
    /// a violation entry, not a held record).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub nearest_margin: Option<f64>,
    /// component_check only: frame of that nearest-margin sample,
    /// earliest frame winning ties. Absent with its margin.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub nearest_margin_frame: Option<u64>,
}

/// An observed leaf extremum: the value and the FIRST frame it was
/// seen at.
type Extremum = (f64, u64);

/// Per-invariant accumulators feeding the `held_invariants` report
/// section. One entry per invariant NAME (scenario validation
/// guarantees uniqueness).
///
/// Kept deliberately flat — this resource owns only descriptive
/// statistics, no verdict logic (that stays in the watched state).
///
/// The component-tier maps hold the "naming what was judged"
/// evidence for `component_check` / `nodes_in_bounds`: entity and
/// frame counts plus a nearest-margin witness, mirroring the
/// resource tier's extrema doctrine (a passing run's "how close did
/// it get" story). Values fold in only for FULLY observed frames —
/// frames where the selector resolved and matched at least one
/// entity — matching the `frames_matched` semantics of the
/// resource tier (existence evidence).
#[derive(Resource, Default)]
pub struct HeldBookkeeping {
    /// Invariant name → count of frames the resource resolved and
    /// the leaf was read.
    pub frames_matched: std::collections::HashMap<String, u64>,
    /// Invariant name → leaf extrema with the frame each extremum
    /// was FIRST seen at: ((min, min_frame), (max, max_frame)).
    pub extrema: std::collections::HashMap<String, (Extremum, Extremum)>,
    /// Invariant name → run-total positive driver advance.
    pub driver_advanced: std::collections::HashMap<String, f64>,
    /// component_check/nodes_in_bounds: invariant name → total
    /// entity-matches summed over observed frames. A per-frame
    /// entity count double-counts spawned/despawned entities across
    /// frames; the run-total is the judge's workload figure ("N
    /// entity-judgments were made").
    pub entity_judgments: std::collections::HashMap<String, u64>,
    /// component_check/nodes_in_bounds: invariant name → peak
    /// per-frame entity count and the frame it was FIRST seen at:
    /// (count, frame).
    pub peak_entities: std::collections::HashMap<String, (u64, u64)>,
    /// component_check/nodes_in_bounds: invariant name → total
    /// frames with ≥1 entity matched (selector resolved AND judged
    /// something).
    pub frames_judged: std::collections::HashMap<String, u64>,
    /// component_check only: invariant name → nearest-margin sample
    /// so far and the frame FIRST seen at: (margin, frame). Lower
    /// margin = closer to violating; strictly better samples
    /// replace, ties keep the earliest frame (the resource tier's
    /// extrema doctrine).
    pub nearest_margin: std::collections::HashMap<String, (f64, u64)>,
}

impl HeldBookkeeping {
    /// Fold one resolved frame (resource + leaf read) of a
    /// `resource_check` invariant: bump the resolution count and
    /// track leaf extrema with first-seen frames. O(1) per fold.
    pub fn observe_resource_leaf(&mut self, name: &str, value: f64, frame: u64) {
        *self.frames_matched.entry(name.to_string()).or_insert(0) += 1;
        match self.extrema.entry(name.to_string()) {
            std::collections::hash_map::Entry::Occupied(mut e) => {
                let ((lo, lo_f), (hi, hi_f)) = e.get_mut();
                if value < *lo {
                    *lo = value;
                    *lo_f = frame;
                }
                if value > *hi {
                    *hi = value;
                    *hi_f = frame;
                }
            }
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(((value, frame), (value, frame)));
            }
        }
    }

    /// Fold one fully-observed frame of a `stagnant_resource`
    /// invariant: bump the resolution count (both leaves read) and
    /// accumulate the run-total driver advance. `driver_delta` is
    /// this frame's signed driver-leaf delta versus the previous
    /// observed frame — `None` on frames with no attributable delta
    /// (first observed frame, gap-resume re-seed: continuity is
    /// broken, so advance across it cannot be credited). Only
    /// POSITIVE deltas advance the total (rewinds are not time
    /// flowing; a zero delta is a paused clock).
    pub fn observe_stagnant_frame(&mut self, name: &str, driver_delta: Option<f64>) {
        *self.frames_matched.entry(name.to_string()).or_insert(0) += 1;
        if let Some(delta) = driver_delta {
            if delta > 0.0 {
                *self.driver_advanced.entry(name.to_string()).or_insert(0.0) += delta;
            }
        }
    }

    /// Fold one observed frame of a `component_check` /
    /// `nodes_in_bounds` invariant: the selector resolved and matched
    /// `matched_entities` entities this frame. Counts the judge's
    /// workload (run-total entity-judgments, peak per-frame scope
    /// with the frame it was FIRST seen at) and bumps the
    /// frames-judged tally. Called only for frames where
    /// `matched_entities > 0` — a zero-match frame is
    /// existence-absence, not a judged frame.
    ///
    /// `margin` (component_check only): this frame's nearest-margin
    /// sample — the smallest |value − threshold| among matched
    /// entities that HELD, i.e. the closest any judged entity came
    /// to violating. Strictly smaller samples replace, earliest
    /// frame wins ties (the extrema doctrine). `None` on frames
    /// where some entity violated (the held record only exists for
    /// passing runs, so a violating frame contributes no
    /// nearest-margin evidence) or where no per-entity numeric
    /// margin is computable.
    pub fn observe_component_frame(
        &mut self,
        name: &str,
        matched_entities: usize,
        frame: u64,
        margin: Option<f64>,
    ) {
        let tallied = matched_entities as u64;
        *self.frames_judged.entry(name.to_string()).or_insert(0) += 1;
        *self.entity_judgments.entry(name.to_string()).or_insert(0) += tallied;
        match self.peak_entities.entry(name.to_string()) {
            std::collections::hash_map::Entry::Occupied(mut e) => {
                let peak = e.get_mut();
                if tallied > peak.0 {
                    *peak = (tallied, frame);
                }
            }
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert((tallied, frame));
            }
        }
        if let Some(margin) = margin {
            match self.nearest_margin.entry(name.to_string()) {
                std::collections::hash_map::Entry::Occupied(mut e) => {
                    let best = e.get_mut();
                    if margin < best.0 {
                        *best = (margin, frame);
                    }
                }
                std::collections::hash_map::Entry::Vacant(e) => {
                    e.insert((margin, frame));
                }
            }
        }
    }

    /// The frame of the extremum closest to violating, for predicates
    /// where that extremum is well-defined (`lt`/`le` → the max
    /// sample, `gt`/`ge` → the min sample; earliest frame wins ties).
    /// `None` for `equals`/`ne` — "closest to violating" is not
    /// totally ordered there (both sides of the threshold violate in
    /// different ways), and the extrema already carry the distance.
    pub fn worst_frame_for(&self, name: &str, op: CheckOp) -> Option<u64> {
        let ((_lo, lo_f), (_hi, hi_f)) = self.extrema.get(name)?;
        match op {
            CheckOp::Lt | CheckOp::Le => Some(*hi_f),
            CheckOp::Gt | CheckOp::Ge => Some(*lo_f),
            CheckOp::Equals | CheckOp::Ne => None,
        }
    }

    /// Build the `held_invariants` section: one entry per PASSING
    /// invariant, in scenario order. `failed` is the caller's test
    /// for "this invariant produced a violation entry" (checked
    /// against the post-audit violation snapshot; entry rule varies
    /// — resource_check, stagnant_resource, vacuous_invariant, ... —
    /// so the caller judges by TARGET name, the invariant's unique
    /// key).
    ///
    /// `watched` carries the resource-tier bookkeeping
    /// (`WatchedResources`), `watched_inv` the component-tier one
    /// (`WatchedInvariants`); the tiers' exclusion evidence lives in
    /// their respective bookkeeping.
    ///
    /// Exclusions, each deliberate:
    /// - `failed(name)` — a violated invariant belongs in the
    ///   violations section, not here;
    /// - config-errored invariants — the run already failed with an
    ///   `oracle_error`; a held entry would be evidence for a check
    ///   that never judged anything;
    /// - vacuous invariants (resource tier: `!watched.matched`;
    ///   component tier: `!watched_inv.matched` — except the
    ///   all-selector `nodes_in_bounds`, which no recorder audits
    ///   into `matched`, so zero judged frames is its sole vacuity
    ///   test) — vacuity carries a `vacuous_invariant` finding for
    ///   the audited shapes (which fails the run); an all-selector
    ///   bounds scope that judged nothing simply proves nothing
    ///   and gets no entry.
    ///
    /// Entries are built from the SCENARIO invariants (authoritative
    /// names, rule, authored fields) cross-referenced with the
    /// accumulated statistics and the watched verdict bookkeeping.
    pub fn collect(
        &self,
        invariants: &[Invariant],
        watched: &crate::invariant_eval::WatchedResources,
        watched_inv: &crate::invariant_eval::WatchedInvariants,
        tps: u64,
        failed: &dyn Fn(&str) -> bool,
    ) -> Vec<HeldInvariant> {
        let mut out = Vec::new();
        for inv in invariants.iter().filter(|i| {
            matches!(
                i.rule,
                InvariantRule::ResourceCheck
                    | InvariantRule::StagnantResource
                    | InvariantRule::ComponentCheck
                    | InvariantRule::NodesInBounds
            )
        }) {
            if failed(&inv.name) {
                continue;
            }
            let is_component_tier = matches!(
                inv.rule,
                InvariantRule::ComponentCheck | InvariantRule::NodesInBounds
            );
            // Tier-appropriate exclusion evidence. The component tier
            // additionally treats zero judged frames as vacuous: a
            // held record for a scope that never judged anything
            // would be evidence for nothing.
            //
            // The all-selector `nodes_in_bounds` path never touches
            // `WatchedInvariants` (the scheduled system has no
            // access to it; only the driver-loop recorders insert
            // into `matched`), so for that shape — and ONLY that
            // shape — `frames_judged > 0` is the sole vacuity test:
            // no frames judged ⇒ no run-level audit fires (the
            // run-level guard reports vacuity only when NOTHING
            // was ever observable), so the collector must not emit
            // an entry a passing-but-empty run never earned.
            let all_selector_bounds = inv.rule == InvariantRule::NodesInBounds
                && !matches!(
                    inv.target_selector,
                    Some(crate::enums::TargetSelector::WithComponent { .. })
                );
            if is_component_tier {
                if watched_inv.config_errored.contains(&inv.name) {
                    continue;
                }
                if all_selector_bounds {
                    if self.frames_judged.get(&inv.name).is_none_or(|f| *f == 0) {
                        continue;
                    }
                } else if !watched_inv.matched.contains(&inv.name)
                    || self.frames_judged.get(&inv.name).is_none_or(|f| *f == 0)
                {
                    continue;
                }
            } else if watched.config_errored.contains(&inv.name)
                || !watched.matched.contains(&inv.name)
            {
                continue;
            }
            // `frames_matched`: frames the check resolved and judged
            // something — for the resource tier the resource/leaf
            // resolved; for the component tier the selector matched
            // ≥1 entity (the fold only runs on such frames).
            let frames_matched = if is_component_tier {
                self.frames_judged.get(&inv.name).copied().unwrap_or(0)
            } else {
                self.frames_matched.get(&inv.name).copied().unwrap_or(0)
            };
            // Extrema and the margin frame, tracked only for
            // resource_check (single leaf per frame). Stagnant
            // invariants judge RATE, not value margins — their
            // watched-leaf extrema carry no margin meaning.
            let (observed_min, observed_max, worst_frame) =
                if inv.rule == InvariantRule::ResourceCheck {
                    let extrema = self.extrema.get(&inv.name);
                    let worst = inv.check.and_then(|op| self.worst_frame_for(&inv.name, op));
                    (extrema.map(|e| (e.0).0), extrema.map(|e| (e.1).0), worst)
                } else {
                    (None, None, None)
                };
            let predicate = inv.check.map(|op| {
                // Wire-name syntax ("ge 0"), matching scenario
                // authoring, not the symbol form (">=").
                let wire = match op {
                    CheckOp::Lt => "lt",
                    CheckOp::Le => "le",
                    CheckOp::Gt => "gt",
                    CheckOp::Ge => "ge",
                    CheckOp::Equals => "equals",
                    CheckOp::Ne => "ne",
                };
                let threshold = crate::invariant_eval::cc_threshold(inv)
                    .map(|t| t.to_string())
                    .unwrap_or_else(|| "?".to_string());
                format!("{wire} {threshold}")
            });
            // Component tiers: `first_held_frame` lives in the
            // component-tier bookkeeping; eventual-mode deadlines use
            // the same formula either way.
            let first_held = if is_component_tier {
                watched_inv.first_held_frame.get(&inv.name).copied()
            } else {
                watched.first_held_frame.get(&inv.name).copied()
            };
            let (met_at_frame, deadline_frame) = match inv.eventually_s {
                Some(secs) => {
                    // Same formula as the post-run deadline audit.
                    let deadline = (secs * tps as f32).ceil() as u64;
                    (first_held, Some(deadline))
                }
                None => (None, None),
            };
            let (episodes_fired, driver_advanced, driver) =
                if inv.rule == InvariantRule::StagnantResource {
                    let fired = watched.fired.contains(&inv.name) as u64;
                    let advanced = self.driver_advanced.get(&inv.name).copied().unwrap_or(0.0);
                    let driver = inv
                        .driver_resource
                        .clone()
                        .unwrap_or_else(|| inv.resource.clone().unwrap_or_default());
                    let driver_path = inv.driver_path.clone().unwrap_or_default();
                    (
                        Some(fired),
                        Some(advanced),
                        Some(format!("{driver}.{driver_path}")),
                    )
                } else {
                    (None, None, None)
                };
            // Component-tier workload and margin evidence: the
            // "naming what was judged" story. Margins ride
            // component_check only — bounds judge containment, not a
            // thresholded leaf.
            let (entity_judgments, peak_entities, peak_entities_frame, nearest, nearest_frame) =
                if is_component_tier {
                    let judgments = self.entity_judgments.get(&inv.name).copied().unwrap_or(0);
                    let (peak, peak_frame) = self
                        .peak_entities
                        .get(&inv.name)
                        .map(|(p, f)| (*p, *f))
                        .unwrap_or((0, 0));
                    let (nearest, nearest_frame) = if inv.rule == InvariantRule::ComponentCheck {
                        self.nearest_margin
                            .get(&inv.name)
                            .map(|(m, f)| (*m, *f))
                            .unzip()
                    } else {
                        (None, None)
                    };
                    (
                        Some(judgments),
                        Some(peak),
                        (peak > 0).then_some(peak_frame),
                        nearest,
                        nearest_frame,
                    )
                } else {
                    (None, None, None, None, None)
                };
            out.push(HeldInvariant {
                name: inv.name.clone(),
                rule: inv.rule.to_string(),
                target: leaf_target(inv),
                predicate,
                frames_matched,
                observed_min,
                observed_max,
                worst_frame,
                met_at_frame,
                deadline_frame,
                episodes_fired,
                driver_advanced,
                driver,
                eventually_s: inv.eventually_s,
                window_s: inv.window_s,
                entity_judgments,
                peak_entities,
                peak_entities_frame,
                nearest_margin: nearest,
                nearest_margin_frame: nearest_frame,
            });
        }
        out
    }
}

/// Human-readable "what was judged" for the entry. Resource tier:
/// `Resource.path`. Component tier: `Component.path` for
/// `component_check` (the selector names the entity, the path the
/// judged leaf); for `nodes_in_bounds` the selector component or
/// `all` (containment in the authored box, no leaf).
fn leaf_target(inv: &Invariant) -> String {
    if inv.rule == InvariantRule::NodesInBounds {
        return match inv.target_selector.as_ref() {
            Some(crate::enums::TargetSelector::WithComponent { name }) => name.clone(),
            _ => "all".to_string(),
        };
    }
    if inv.rule == InvariantRule::ComponentCheck {
        let name = match inv.target_selector.as_ref() {
            Some(crate::enums::TargetSelector::WithComponent { name }) => name.clone(),
            _ => "?".to_string(),
        };
        return match &inv.path {
            Some(path) => format!("{name}.{path}"),
            None => name,
        };
    }
    match (&inv.resource, &inv.path) {
        (Some(res), Some(path)) => format!("{res}.{path}"),
        (Some(res), None) => res.clone(),
        (None, Some(path)) => path.clone(),
        (None, None) => "?".to_string(),
    }
}
