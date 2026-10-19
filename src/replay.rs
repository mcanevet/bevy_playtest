//! Replay-divergence verification.
//!
//! `verify_replay: true` runs the scenario twice against freshly built
//! Apps and compares per-frame fingerprints of the observable world.
//!
//! Fingerprint per frame: hash of (choice draws recorded that frame,
//! all Transform translations in sorted order). Query iteration order
//! over archetypes is stable for identical construction on the
//! single-threaded executor, but the fingerprint sorts translations
//! anyway so the guarantee is structural, not assumed.
//!
//! BRP mode rejects `verify_replay` outright: wall-clock pacing makes
//! honest replay verification impossible there, and the BRP report
//! must never claim it.

use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_transform::prelude::*;

use crate::scenario::{Scenario, ScenarioError};

/// Per-frame observable fingerprint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameFingerprint {
    pub frame: u64,
    pub hash: u64,
}

/// A replay divergence: the first frame where fingerprints disagree.
#[derive(Clone, Debug)]
pub struct Divergence {
    pub frame: u64,
    pub pass_a_hash: u64,
    pub pass_b_hash: u64,
}

/// Compute the fingerprint for the current world state at `frame`,
/// plus the choice draws recorded for that frame.
pub fn fingerprint_frame(frame: u64, choices: &[crate::choice::Choice], world: &mut World) -> u64 {
    let mut hasher = fnv_hasher();
    hasher = hash_u64(hasher, frame);
    let draws: Vec<&crate::choice::Choice> = choices.iter().filter(|c| c.frame == frame).collect();
    hasher = hash_usize(hasher, draws.len());
    for d in draws {
        hasher = hash_u64(hasher, d.value);
        hasher = hash_u64(hasher, d.bound);
    }
    // Canonical order: collect and sort all translations, so Query
    // iteration order never leaks into the fingerprint.
    let mut translations: Vec<[f32; 3]> = world
        .query_filtered::<&Transform, ()>()
        .iter(world)
        .map(|t| [t.translation.x, t.translation.y, t.translation.z])
        .collect();
    translations.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    hasher = hash_usize(hasher, translations.len());
    for t in translations {
        for v in t {
            hasher = hash_f32_bits(hasher, v);
        }
    }
    hasher
}

fn fnv_hasher() -> u64 {
    0xcbf29ce484222325
}

fn hash_u64(mut h: u64, v: u64) -> u64 {
    for b in v.to_le_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn hash_usize(h: u64, v: usize) -> u64 {
    hash_u64(h, v as u64)
}

fn hash_f32_bits(h: u64, v: f32) -> u64 {
    hash_u64(h, v.to_bits() as u64)
}

/// Compare two fingerprint series; returns the first divergence.
pub fn compare(a: &[FrameFingerprint], b: &[FrameFingerprint]) -> Result<(), Divergence> {
    for (fa, fb) in a.iter().zip(b.iter()) {
        debug_assert_eq!(fa.frame, fb.frame);
        if fa.hash != fb.hash {
            return Err(Divergence {
                frame: fa.frame,
                pass_a_hash: fa.hash,
                pass_b_hash: fb.hash,
            });
        }
    }
    if a.len() != b.len() {
        let frame = a.len().min(b.len()) as u64;
        return Err(Divergence {
            frame,
            pass_a_hash: a.get(frame as usize).map(|f| f.hash).unwrap_or(0),
            pass_b_hash: b.get(frame as usize).map(|f| f.hash).unwrap_or(0),
        });
    }
    Ok(())
}

/// Run the scenario twice against freshly built Apps and verify the
/// per-frame fingerprints match.
///
/// # Footgun
///
/// Pass A MUTATES its App (driver resources, message registrations,
/// runner state are inserted into `app_a`). The `build` closure must
/// therefore construct a fresh, reconstruction-safe App each call —
/// if it captures or reuses partially-driven state, the two passes
/// compare unequal worlds and divergence noise results. The takeover
/// path avoids this entirely by re-running the game executable as a
/// subprocess per pass; prefer that mechanism (`"verify_replay": true`
/// in a scenario) when the game binary is available.
///
/// The returned report is that of the SECOND pass (both passes'
/// statuses must agree for the run to Pass; disagreement is itself a
/// divergence and reported as such).
pub fn run_scenario_verified(
    build: impl Fn() -> App,
    scenario: &Scenario,
) -> Result<crate::state::PlaytestReport, ScenarioError> {
    crate::scenario::validate_scenario(scenario)?;

    let mut app_a = build();
    let report_a = crate::driver::run_scenario(&mut app_a, scenario)?;
    let series_a = app_a
        .world_mut()
        .remove_resource::<ReplaySeries>()
        .map(|s| s.0)
        .unwrap_or_default();

    let mut app_b = build();
    let report_b = crate::driver::run_scenario(&mut app_b, scenario)?;
    let series_b = app_b
        .world_mut()
        .remove_resource::<ReplaySeries>()
        .map(|s| s.0)
        .unwrap_or_default();

    let status_b = report_b.status;
    let mut report = report_b;

    // A crash exempts the divergence check — it applies only to
    // completed runs.
    if report_a.status != crate::enums::PlaytestStatus::Crash
        && status_b != crate::enums::PlaytestStatus::Crash
    {
        if let Err(div) = compare(&series_a, &series_b) {
            report.violations.push(crate::state::ViolationEntry {
                rule: crate::rules::Rule::ReplayDivergence.name().to_string(),
                target: "run".to_string(),
                first_frame: div.frame,
                last_frame: div.frame,
                count: 1,
                detail: format!(
                    "first divergent frame {}: pass A fingerprint {:#018x}, \
                     pass B fingerprint {:#018x} — hidden nondeterminism \
                     (wall clock, unseeded RNG, or unordered iteration)",
                    div.frame, div.pass_a_hash, div.pass_b_hash
                ),
                last_detail: format!(
                    "divergent frame {} ({:#018x} vs {:#018x})",
                    div.frame, div.pass_a_hash, div.pass_b_hash
                ),
            });
            report.status = crate::enums::PlaytestStatus::Fail;
        }
    }

    // Both runs must agree on status; a Pass/Fail split is itself a
    // divergence signal the fingerprints may have missed (e.g. timing
    // oracle differences).
    if report_a.status != status_b {
        report.violations.push(crate::state::ViolationEntry {
            rule: crate::rules::Rule::ReplayDivergence.name().to_string(),
            target: "run".to_string(),
            first_frame: 0,
            last_frame: report.frame_count,
            count: 1,
            detail: format!(
                "passes report different statuses ({:?} vs {:?}) — \
                 determinism cannot be certified",
                report_a.status, status_b
            ),
            last_detail: "pass statuses differ".to_string(),
        });
        report.status = crate::enums::PlaytestStatus::Fail;
    }

    Ok(report)
}

/// Resource carrying the recorded fingerprint series out of a run.
#[derive(bevy_ecs::prelude::Resource, Default)]
pub struct ReplaySeries(pub Vec<FrameFingerprint>);

/// System run each frame by the driver when verify_replay is set:
/// records the fingerprint of the current world + this frame's draws.
pub fn fingerprint_system(world: &mut World) {
    let frame = world
        .get_resource::<crate::driver::RunState>()
        .map(|s| s.frame)
        .unwrap_or(0);
    let recorded: Vec<crate::choice::Choice> = world
        .get_resource::<crate::driver::RunChoices>()
        .map(|c| c.0.recorded.clone())
        .unwrap_or_default();
    let draws: Vec<crate::choice::Choice> = recorded
        .iter()
        .filter(|c| c.frame == frame)
        .copied()
        .collect();
    let hash = fingerprint_frame(frame, &draws, world);
    if let Some(mut series) = world.get_resource_mut::<ReplaySeries>() {
        series.0.push(FrameFingerprint { frame, hash });
    }
}
