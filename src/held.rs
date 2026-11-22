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
//! [`crate::invariant_eval::WatchedResources`]/[`WatchedInvariants`]:
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
//!   evidence, exactly the spec's meaning). For eventually-mode
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
//!   accumulator may appear in the window rework (spec §1) inside
//!   the watched state; integration will reconcile the two, which is
//!   exactly why this lives in its own module.
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
#[serde(deny_unknown_fields)]
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
    /// Exclusions, each deliberate:
    /// - `failed(name)` — a violated invariant belongs in the
    ///   violations section, not here;
    /// - config-errored invariants — the run already failed with an
    ///   `oracle_error`; a held entry would be evidence for a check
    ///   that never judged anything;
    /// - vacuous invariants (`!watched.matched`) — they carry a
    ///   `vacuous_invariant` finding (which fails the run) and
    ///   proved nothing.
    ///
    /// Entries are built from the SCENARIO invariants (authoritative
    /// names, rule, authored fields) cross-referenced with the
    /// accumulated statistics and the watched verdict bookkeeping.
    pub fn collect(
        &self,
        invariants: &[Invariant],
        watched: &crate::invariant_eval::WatchedResources,
        tps: u64,
        failed: &dyn Fn(&str) -> bool,
    ) -> Vec<HeldInvariant> {
        let mut out = Vec::new();
        for inv in invariants.iter().filter(|i| {
            matches!(
                i.rule,
                InvariantRule::ResourceCheck | InvariantRule::StagnantResource
            )
        }) {
            if watched.config_errored.contains(&inv.name)
                || !watched.matched.contains(&inv.name)
                || failed(&inv.name)
            {
                continue;
            }
            let frames_matched = self.frames_matched.get(&inv.name).copied().unwrap_or(0);
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
            let (met_at_frame, deadline_frame) = match inv.eventually_s {
                Some(secs) => {
                    // Same formula as the post-run deadline audit.
                    let deadline = (secs * tps as f32).ceil() as u64;
                    (
                        watched.first_held_frame.get(&inv.name).copied(),
                        Some(deadline),
                    )
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
            });
        }
        out
    }
}

/// `Resource.path` reflect target, falling back to whichever piece
/// exists (validation guarantees both for these rules).
fn leaf_target(inv: &Invariant) -> String {
    match (&inv.resource, &inv.path) {
        (Some(res), Some(path)) => format!("{res}.{path}"),
        (Some(res), None) => res.clone(),
        (None, Some(path)) => path.clone(),
        (None, None) => "?".to_string(),
    }
}
