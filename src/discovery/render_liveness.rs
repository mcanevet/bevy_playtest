//! Structural render-liveness probe (`render_liveness` rule,
//! informational).
//!
//! The frozen_world oracle proves the SIMULATION keeps moving; this
//! rule answers the next question down the causal chain: is the game
//! even WIRED to show anything? A project whose camera was never
//! spawned, disabled with `is_active: false`, or parked on
//! `CameraOutputMode::Skip` ticks perfectly, passes every other
//! oracle, and shows a black screen — a structural blindness no
//! amount of simulation-side checking can catch.
//!
//! Doctrine — STRUCTURAL, not pixels: everything inspected here is a
//! MAIN-WORLD `bevy_camera::Camera` component, plus one lookup in the
//! app's sub-app table. The rule can certify "there is a camera, it
//! is enabled, and its output is written somewhere"; it CANNOT see
//! the render world or the GPU, so a camera that renders black still
//! passes. Pixel-level liveness is reserved for a future oracle (see
//! docs/how-to/headless-ci-gpu.md for the promise boundary).
//!
//! Census-neutrality: same doctrine as the UI-surface enumeration —
//! a pure `fn(&World)` / `fn(&App)` walk driven from the driver
//! loop, NEVER a registered system. Nothing here appears in the
//! game's schedule, `static_scan`'s reader census, or the state
//! digest.
//!
//! Attachment gate — the rule is "not evaluated" unless the game has
//! a render sub-app: `render_sub_app_present` probes for
//! `bevy_render`'s `RenderApp` sub-app, which only exists when a
//! renderer was actually initialized (a backend was available at
//! plugin-build time). A headless server or unit-test app with no
//! `bevy_render` in its plugin graph is not "broken for not
//! rendering" — the rule says nothing about it (mirrors the
//! vacuity guard's refusal to judge a game with nothing to check).
//! The pipelined-rendering caveat on the probe itself is documented
//! there: a FINISHED app no longer lists `RenderApp` (the sub-app
//! ships to the render thread), so the surviving
//! `RenderExtractApp` label acts as the witness.
//! This is why the feature carries a `dep:bevy_render` edge: the
//! presence check names the real label type instead of probing the
//! sub-app table by hash/TypeId shape. Under takeover (and every
//! fixture) the full `bevy` meta-crate already unifies
//! `bevy_render`, so the edge costs zero new crates there;
//! in-process-only consumers who opt in gain the renderer crate but
//! no GPU init (that is the game's own plugin graph's business).
//!
//! Cadence mirrors `UiSurfaceTracker`: first sample on the first
//! settled frame (tick 0, always), then every `sample_interval`
//! ticks (~twice a second, min 1). Verdicts accumulate across
//! samples — one camera alive at ANY sampled tick clears a finding
//! — so cameras that spawn late, die mid-run, or flicker `is_active`
//! for a frame while shaders compile under-claim rather than
//! false-positive (refusal over silence cuts both ways).
//!
//! Verdict tiers (see [`RenderLivenessTier`]; all informational —
//! the rule never gates the run status):
//!
//! 1. No render sub-app (see above) → result not attached at all.
//! 2. Sim-active run, zero cameras on every sample → `no_cameras`.
//! 3. Cameras existed, but every sampled camera was
//!    `is_active == false` → `all_cameras_inactive` (a disabled-for-
//!    debugging camera left in is the classic case).
//! 4. Cameras existed and were active, but no sampled active
//!    camera's `output_mode` was a write mode (i.e. every ACTIVE
//!    camera was `CameraOutputMode::Skip`; a disabled camera's
//!    mode is irrelevant — it renders nothing) →
//!    `all_cameras_skip`. Deliberately softer than tier 3: Skip CAN
//!    be intentional (multi-camera compositing keeps intermediate
//!    cameras on Skip forever; a sole-camera Skip may still be a
//!    deliberate prepass-only setup, e.g. depth-only renderers), so
//!    the finding's detail says "may be intentional".
//! 5. Persistently unresolved `computed.target_info` → NOT a
//!    finding: headless runs (the harness drops `WinitPlugin`) may
//!    never resolve window-backed targets, so judging here would
//!    false-positive on every healthy headless run. Reported as an
//!    evidence note only, never a verdict.
//!
//! Sampling honesty (docs must not lie): all evidence below is
//! gathered ONLY at cadence-gated samples — the first settled
//! frame, then roughly twice a second (`sample_interval`, tps/2
//! ticks). `cameras_seen` is the union across SAMPLES, not across
//! frames: a camera that exists exclusively on non-sampled ticks is
//! invisible to every accumulator. Likewise `sim_active_ever`
//! reflects only sampled ticks — between-sample animation does not
//! reach it. The emitted details always quote the real sample
//! count, so the wording stays honest about its own evidence. The
//! tier-3/4 EVERY-sample requirement deliberately under-claims: a
//! camera active on a single sampled tick vetoes the finding.

use std::collections::HashSet;

use bevy_app::App;
use bevy_camera::{Camera, CameraOutputMode};
use bevy_ecs::entity::Entity;
use bevy_ecs::prelude::*;
use bevy_ecs::query::QueryState;
use serde::{Deserialize, Serialize};

/// Whether the game has a render sub-app at all
/// (`bevy_render::RenderApp` registered in the sub-app table — only
/// true when `RenderPlugin` built with an available backend). The
/// render-liveness result is attached only when this holds; absence
/// of the report field therefore means "not evaluated", never "no
/// problems".
///
/// Pipelined-rendering caveat (bevy 0.20): `PipelinedRenderingPlugin`'s
/// `cleanup` REMOVES `RenderApp` from the sub-app table and ships the
/// sub-app to the dedicated render thread. This runs during
/// `app.finish()/cleanup()` — i.e. before `run_scenario` observes the
/// app — so a finished app no longer lists `RenderApp`. The plugin's
/// `build`, however, only inserts `RenderExtractApp` when `RenderApp`
/// existed at build time (the reverse direction is guarded too:
/// `PipelinedRenderingPlugin::cleanup` bails when `RenderExtractApp`
/// is absent), making the surviving `RenderExtractApp` label the
/// faithful post-cleanup witness that a renderer was initialized.
/// Both labels are therefore accepted: either one means a render
/// sub-app was created. A backend-less app has neither (the
/// ExtractPlugin is never added when `RenderCreation` fails), staying
/// honestly "not evaluated".
pub(crate) fn render_sub_app_present(app: &App) -> bool {
    if app.get_sub_app(bevy_render::RenderApp).is_some() {
        return true;
    }
    // `pipelined_rendering` is non-wasm upstream (`pub mod` carries
    // `#[cfg(not(target_arch = "wasm32"))]`); on wasm the RenderApp is
    // never shipped away, so the primary lookup alone is the whole
    // truth there.
    #[cfg(not(target_arch = "wasm32"))]
    {
        app.get_sub_app(bevy_render::pipelined_rendering::RenderExtractApp)
            .is_some()
    }
    #[cfg(target_arch = "wasm32")]
    {
        false
    }
}

/// Verdict tier for a run, worst observed wins.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderLivenessTier {
    /// No finding: at least one sampled camera was active with a
    /// written output mode (or nothing was ever judged — see the
    /// attachment gate).
    #[default]
    Live,
    /// Sim-active run, zero cameras on every sample.
    NoCameras,
    /// Cameras existed; every sampled camera was `is_active ==
    /// false`.
    AllCamerasInactive,
    /// Active cameras existed; no sampled ACTIVE camera had a
    /// non-`Skip` `CameraOutputMode` (may be intentional —
    /// compositing or prepass-only setups park cameras on Skip
    /// legitimately).
    AllCamerasSkip,
}

impl RenderLivenessTier {
    /// Stable snake-case name, useful for serialized consumers.
    pub fn as_str(self) -> &'static str {
        match self {
            RenderLivenessTier::Live => "live",
            RenderLivenessTier::NoCameras => "no_cameras",
            RenderLivenessTier::AllCamerasInactive => "all_cameras_inactive",
            RenderLivenessTier::AllCamerasSkip => "all_cameras_skip",
        }
    }
}

/// Result of the render-liveness probe, carried on the report as
/// `render_liveness` (`render-liveness` feature; `None` when the
/// feature is off or — the important case — the game has no
/// render sub-app). Absence never
/// means "healthy": additive-honesty contract as `capability_census`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RenderLivenessResult {
    /// Distinct camera entities seen across all SAMPLES (union —
    /// but only sampled ticks are inspected, so a camera alive
    /// solely between samples is invisible to this count).
    pub cameras_seen: usize,
    /// Whether the simulation was observed active at any SAMPLED
    /// tick (feeds the tier-2 guard so boot-quiet runs are not
    /// excused, and never-active sampled runs are not accused;
    /// between-sample activity does not reach this flag).
    pub sim_active_ever: bool,
    /// Verdict tier (see module docs).
    pub tier: RenderLivenessTier,
    /// Samples actually taken (cadence-gated observations, not
    /// ticks) — grounds the detail wording ("({} samples)").
    pub samples: u64,
    /// Bounded evidence notes (see the internal
    /// `MAX_RENDER_LIVENESS_NOTES` cap).
    pub notes: Vec<String>,
}

/// Cap on evidence notes kept per run (bounded report memory — same
/// doctrine as the UI-surface tracker's delta-note cap).
pub(crate) const MAX_RENDER_LIVENESS_NOTES: usize = 32;

/// Tick interval between samples: half a second of simulated time,
/// minimum 1 tick (mirrors `UiSurfaceTracker::recapture_interval`).
pub(crate) fn sample_interval(tps: u64) -> u64 {
    (tps / 2).max(1)
}

/// Driver-loop recorder for the structural render-liveness rule.
///
/// Census-neutral (never a scheduled system; see module docs).
/// Accumulates one [`Sample`] per observation; the verdict is
/// derived once at [`Self::into_result`]:
///
/// - tier 2 fires when `sim_active_ever` and no sample ever saw a
///   camera;
/// - tier 3 fires when a camera existed but no sample ever saw one
///   active;
/// - tier 4 fires when an active camera existed but no sample ever
///   saw an ACTIVE camera with a write-mode output (an inactive
///   camera's output mode does not count — disabled cameras
///   render nothing);
/// - otherwise `Live`.
///
/// `sim_active` is passed in by the driver each observe call (the
/// driver owns the digest/changed-tracking state; the tracker never
/// reaches into harness resources).
#[derive(Debug, Default)]
pub(crate) struct RenderLivenessTracker {
    /// Union of camera entity ids across all samples.
    seen: HashSet<Entity>,
    /// Sim activity observed at any SAMPLED tick.
    sim_active_ever: bool,
    /// False until any sample saw a camera.
    ever_camera: bool,
    /// True once any sampled camera was `is_active == true`.
    ever_active: bool,
    /// True once any sampled ACTIVE camera had a non-`Skip`
    /// `output_mode` (disabled cameras render nothing, so their
    /// output mode is irrelevant to liveness).
    ever_write: bool,
    /// Total samples taken (passed the cadence gate). Grounds the
    /// "({} samples)" wording in the no_cameras detail —
    /// a count that is neither estimated nor fabricated.
    samples: u64,
    /// True once any sample resolved ANY camera's `target_info`.
    ever_target_resolved: bool,
    /// Camera count of the last sample (note-dedup so count
    /// flapping does not spam).
    last_count: Option<usize>,
    /// Evidence notes (bounded).
    notes: Vec<String>,
}

impl RenderLivenessTracker {
    /// Sample on the FIRST settled frame (tick 0, always — the
    /// driver's contract is first-sample-always) and on every
    /// `interval` boundary thereafter. `interval` must be ≥ 1
    /// (pass `sample_interval`'s output).
    pub(crate) fn observe(&mut self, world: &World, tick: u64, interval: u64, sim_active: bool) {
        debug_assert!(interval > 0, "interval must be >= 1");
        // First-sample-always: `samples == 0` distinguishes "never
        // sampled" from "sampled at tick 0", which a plain
        // `is_multiple_of` test cannot (tick 0 is a multiple of
        // every interval, but unit tests may observe from tick 1).
        // Decoupled from the note-dedup `last_count` bookkeeping so
        // future note refactors cannot break the cadence sentinel.
        let first = self.samples == 0;
        if !first && !tick.is_multiple_of(interval) {
            return;
        }
        self.sim_active_ever |= sim_active;
        self.samples += 1;

        // `QueryState::try_new` works from `&World` (immutable,
        // safe). `None` means `Camera` is not registered in this
        // world, so no camera can exist: empty AND complete (same
        // doctrine as the UI-surface enumeration).
        let cameras: Vec<Entity> = QueryState::<Entity, With<Camera>>::try_new(world)
            .map(|mut state| state.iter(world).collect())
            .unwrap_or_default();
        let ordered = {
            // Deterministic order: ascending entity index.
            let mut v = cameras;
            v.sort_unstable_by_key(|e| e.index());
            v
        };
        self.seen.extend(ordered.iter().copied());

        let mut active = 0usize;
        let mut write = 0usize;
        let mut resolved = 0usize;
        for entity in &ordered {
            let Some(cam) = world.get::<Camera>(*entity) else {
                continue;
            };
            active += usize::from(cam.is_active);
            // Skip CAN be legitimate (compositing, prepass-only), but
            // an ACTIVE camera that never writes anywhere is exactly
            // what this rule exists to surface. `write` therefore
            // counts the conjunction — an inactive camera renders
            // nothing, so its output mode is irrelevant to liveness
            // (a disabled Write camera must not mask a sole active
            // Skip camera).
            write +=
                usize::from(cam.is_active && !matches!(cam.output_mode, CameraOutputMode::Skip));
            resolved += usize::from(cam.computed.target_info.is_some());
        }

        if !ordered.is_empty() {
            self.ever_camera = true;
            self.ever_active |= active > 0;
            self.ever_write |= write > 0;
            self.ever_target_resolved |= resolved > 0;
        }

        // Low-frequency evidence notes: first sample and camera-
        // count changes only (bounded, deduplicated).
        if self.last_count != Some(ordered.len()) {
            self.note(format!(
                "sample at tick {}: {} camera(s), {} active, {} writing",
                tick,
                ordered.len(),
                active,
                write
            ));
            self.last_count = Some(ordered.len());
        }
    }

    /// Derive the verdict tier from the accumulated samples.
    /// Ordered by severity; the first matching tier wins.
    pub(crate) fn into_result(self) -> RenderLivenessResult {
        let tier = if self.ever_camera {
            if !self.ever_active {
                RenderLivenessTier::AllCamerasInactive
            } else if !self.ever_write {
                RenderLivenessTier::AllCamerasSkip
            } else {
                RenderLivenessTier::Live
            }
        } else if self.sim_active_ever {
            RenderLivenessTier::NoCameras
        } else {
            // Never a camera, but never sim-active either: a dormant
            // or boot-quiet run is not this rule's business.
            // Under-claiming (mirroring the vacuity guard's
            // never-accuse-without-evidence stance; note our
            // evidence is sample-gated, not any-frame).
            RenderLivenessTier::Live
        };

        let mut notes = self.notes;
        // Tier-5 evidence without judgment (see module docs): cameras
        // existed and NEVER resolved a target across any sample — a
        // late-resolving run (swapchain spin-up) stays note-free.
        // Cap-aware: refused when the evidence notes already fill
        // the cap (bounded memory outranks this soft note).
        if self.ever_camera && !self.ever_target_resolved && notes.len() < MAX_RENDER_LIVENESS_NOTES
        {
            notes.push(
                "cameras present but computed.target_info stayed unresolved; \
                 normal for headless runs (window-backed targets may never \
                 resolve without a window) — evidence only, not a finding"
                    .to_string(),
            );
        }

        RenderLivenessResult {
            cameras_seen: self.seen.len(),
            sim_active_ever: self.sim_active_ever,
            tier,
            samples: self.samples,
            notes,
        }
    }

    /// Bounded push (same doctrine as the UI-surface tracker:
    /// refuse to grow past the cap rather than truncate silently).
    fn note(&mut self, line: String) {
        if self.notes.len() < MAX_RENDER_LIVENESS_NOTES {
            self.notes.push(line);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_app::SubApp;

    /// Minimal world helper: spawn cameras with the given
    /// (is_active, output_mode) configuration.
    fn world_with(cameras: &[(bool, CameraOutputMode)]) -> World {
        let mut world = World::new();
        for &(is_active, output_mode) in cameras {
            world.spawn(Camera {
                is_active,
                output_mode,
                ..Default::default()
            });
        }
        world
    }

    #[test]
    fn tier_no_cameras_when_sim_active() {
        let mut tracker = RenderLivenessTracker::default();
        let world = World::new();
        tracker.observe(&world, 0, 2, true);
        tracker.observe(&world, 10, 2, true);
        let result = tracker.into_result();
        assert_eq!(result.tier, RenderLivenessTier::NoCameras);
        assert!(result.sim_active_ever);
        assert_eq!(result.cameras_seen, 0);
        // Real sample count (grounds the "({} samples)" detail),
        // not a note-count stand-in: both calls sampled, regardless
        // of camera-count notes.
        assert_eq!(result.samples, 2);
    }

    #[test]
    fn dormant_run_is_not_accused() {
        // Never sim-active, never a camera: not this rule's business.
        let mut tracker = RenderLivenessTracker::default();
        tracker.observe(&World::new(), 0, 2, false);
        let result = tracker.into_result();
        assert_eq!(result.tier, RenderLivenessTier::Live);
    }

    #[test]
    fn all_cameras_inactive() {
        let mut tracker = RenderLivenessTracker::default();
        let world = world_with(&[(false, CameraOutputMode::default())]);
        tracker.observe(&world, 0, 2, true);
        let result = tracker.into_result();
        assert_eq!(result.tier, RenderLivenessTier::AllCamerasInactive);
        assert_eq!(result.cameras_seen, 1);
    }

    #[test]
    fn sole_camera_skip_is_all_cameras_skip() {
        let mut tracker = RenderLivenessTracker::default();
        let world = world_with(&[(true, CameraOutputMode::Skip)]);
        tracker.observe(&world, 0, 2, true);
        let result = tracker.into_result();
        assert_eq!(result.tier, RenderLivenessTier::AllCamerasSkip);
    }

    #[test]
    fn active_writing_camera_is_live() {
        let mut tracker = RenderLivenessTracker::default();
        let world = world_with(&[(true, CameraOutputMode::default())]);
        tracker.observe(&world, 0, 2, true);
        let result = tracker.into_result();
        assert_eq!(result.tier, RenderLivenessTier::Live);
    }

    #[test]
    fn one_live_camera_at_any_tick_vetoes_findings() {
        // Under-claiming doctrine: a camera alive for a single
        // sampled tick clears both tier 3 and tier 4.
        let mut tracker = RenderLivenessTracker::default();
        let dead = world_with(&[]);
        let alive = world_with(&[(true, CameraOutputMode::default())]);
        tracker.observe(&dead, 0, 1, true);
        tracker.observe(&alive, 1, 1, true);
        tracker.observe(&dead, 2, 1, true);
        let result = tracker.into_result();
        assert_eq!(result.tier, RenderLivenessTier::Live);
        // Union across samples counts the camera.
        assert_eq!(result.cameras_seen, 1);
    }

    #[test]
    fn late_spawned_camera_is_counted() {
        // First-sample-always + periodic refresh: a camera that
        // appears after tick 0 is still seen.
        let mut tracker = RenderLivenessTracker::default();
        let empty = world_with(&[]);
        let with_cam = world_with(&[(true, CameraOutputMode::default())]);
        tracker.observe(&empty, 0, 2, true);
        tracker.observe(&with_cam, 2, 2, true);
        let result = tracker.into_result();
        assert_eq!(result.tier, RenderLivenessTier::Live);
        assert_eq!(result.cameras_seen, 1);
    }

    #[test]
    fn first_sample_always_happens_even_off_boundary() {
        // Unit-test contract: observing from tick 1 with interval 2
        // still samples (see the `first` comment in observe()).
        let mut tracker = RenderLivenessTracker::default();
        let world = world_with(&[(true, CameraOutputMode::default())]);
        tracker.observe(&world, 1, 2, true);
        let result = tracker.into_result();
        assert_eq!(result.tier, RenderLivenessTier::Live);
        assert_eq!(result.cameras_seen, 1);
    }

    #[test]
    fn inactive_write_camera_does_not_mask_active_skip_camera() {
        // Regression pin for the conjunction contract: `write` must
        // require is_active. A disabled Write-mode camera renders
        // nothing, so pairing it with a sole ACTIVE Skip camera
        // must NOT reach the Live tier — the run has no active
        // camera writing anywhere (finding: all_cameras_skip).
        let mut tracker = RenderLivenessTracker::default();
        let world = world_with(&[
            (false, CameraOutputMode::default()), // Write, disabled
            (true, CameraOutputMode::Skip),       // active, Skip
        ]);
        tracker.observe(&world, 0, 2, true);
        let result = tracker.into_result();
        assert_eq!(result.tier, RenderLivenessTier::AllCamerasSkip);
        assert_eq!(result.cameras_seen, 2);
        // The per-sample evidence note reports the same
        // conjunction, not the unconstrained mode count.
        assert!(
            result
                .notes
                .iter()
                .any(|n| n.contains("1 active, 0 writing")),
            "expected the honest note, got {:?}",
            result.notes
        );
    }

    #[test]
    fn samples_count_ignores_non_boundary_ticks() {
        // Cadence gate integrity: ticks between boundaries and
        // repeats of the same boundary do not inflate `samples`
        // (the number quoted in the no_cameras detail text).
        let mut tracker = RenderLivenessTracker::default();
        let world = World::new();
        tracker.observe(&world, 0, 2, true); // first-sample-always
        tracker.observe(&world, 1, 2, true); // off boundary: ignored
        tracker.observe(&world, 3, 2, true); // off boundary: ignored
        tracker.observe(&world, 4, 2, true); // on boundary
        let result = tracker.into_result();
        assert_eq!(result.samples, 2);
    }

    #[test]
    fn unresolved_target_info_is_note_only_never_a_verdict() {
        // Tier 5: cameras exist, none resolve target_info — the note
        // appears, but the tier stays whatever the structural
        // evidence says (Skip here, since output_mode is Skip).
        let mut tracker = RenderLivenessTracker::default();
        let world = world_with(&[(true, CameraOutputMode::Skip)]);
        tracker.observe(&world, 0, 2, true);
        let result = tracker.into_result();
        assert_eq!(result.tier, RenderLivenessTier::AllCamerasSkip);
        assert!(
            result
                .notes
                .iter()
                .any(|n| n.contains("target_info stayed unresolved")),
            "expected the tier-5 evidence note, got {:?}",
            result.notes
        );
    }

    #[test]
    fn resolved_target_info_yields_no_unresolved_note() {
        // A camera with target_info resolved suppresses the note.
        let mut world = world_with(&[(true, CameraOutputMode::default())]);
        let mut entity = world.query::<&mut Camera>().single_mut(&mut world).unwrap();
        entity.computed.target_info = Some(Default::default());
        let mut tracker = RenderLivenessTracker::default();
        tracker.observe(&world, 0, 2, true);
        let result = tracker.into_result();
        assert_eq!(result.tier, RenderLivenessTier::Live);
        assert!(!result
            .notes
            .iter()
            .any(|n| n.contains("target_info stayed unresolved")));
    }

    #[test]
    fn camera_without_camera_component_type_registered_is_complete() {
        // `QueryState::try_new` returns None for an unregistered
        // component: the tracker treats that as "no cameras exist"
        // (empty AND complete), which for a bare World with no
        // cameras is exactly the no_cameras tier when sim-active.
        let mut tracker = RenderLivenessTracker::default();
        let world = World::new();
        tracker.observe(&world, 0, 2, true);
        let result = tracker.into_result();
        assert_eq!(result.tier, RenderLivenessTier::NoCameras);
    }

    #[test]
    fn sample_respects_interval_boundaries() {
        // Off-boundary ticks are skipped after the first sample.
        let mut tracker = RenderLivenessTracker::default();
        let world = world_with(&[(true, CameraOutputMode::default())]);
        tracker.observe(&world, 0, 2, true);
        tracker.observe(&world, 1, 2, false); // off-boundary: ignored
        let result = tracker.into_result();
        // sim_active_ever stays true from tick 0; tick 1's `false`
        // must not have been recorded (it wasn't sampled).
        assert!(result.sim_active_ever);
    }

    #[test]
    fn notes_are_capped() {
        // Count-change notes: alternating 0↔1 cameras every boundary
        // tick fills the notes to the cap and refuses to exceed it.
        let mut tracker = RenderLivenessTracker::default();
        let empty = world_with(&[]);
        let one = world_with(&[(true, CameraOutputMode::default())]);
        for tick in 0..100 {
            let world = if tick % 2 == 0 { &empty } else { &one };
            tracker.observe(world, tick, 1, true);
        }
        let result = tracker.into_result();
        assert!(result.notes.len() <= MAX_RENDER_LIVENESS_NOTES);
    }

    #[test]
    fn tier_names_are_stable_snake_case() {
        assert_eq!(RenderLivenessTier::Live.as_str(), "live");
        assert_eq!(RenderLivenessTier::NoCameras.as_str(), "no_cameras");
        assert_eq!(
            RenderLivenessTier::AllCamerasInactive.as_str(),
            "all_cameras_inactive"
        );
        assert_eq!(
            RenderLivenessTier::AllCamerasSkip.as_str(),
            "all_cameras_skip"
        );
    }

    // ---- presence gate -------------------------------------------------

    /// The `RenderApp` label ALONE (pre-pipelined-cleanup state, or a
    /// plugin graph without PipelinedRenderingPlugin) attaches.
    #[test]
    fn render_app_label_alone_attaches() {
        let mut app = App::new();
        app.insert_sub_app(bevy_render::RenderApp, SubApp::new());
        assert!(render_sub_app_present(&app));
    }

    /// PipelinedRenderingPlugin::cleanup ships `RenderApp` off to the
    /// render thread and REMOVES it from the sub-app table — every
    /// finished DefaultPlugins app looks like this. The surviving
    /// `RenderExtractApp` (only inserted when `RenderApp` existed at
    /// build time) must keep the rule attached.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn render_extract_app_witness_after_pipelined_cleanup() {
        let mut app = App::new();
        app.insert_sub_app(
            bevy_render::pipelined_rendering::RenderExtractApp,
            SubApp::new(),
        );
        assert!(render_sub_app_present(&app));
    }

    /// Backend-less app (headless server, MinimalPlugins test app):
    /// neither label — honestly "not evaluated".
    #[test]
    fn backendless_app_does_not_attach() {
        let app = App::new();
        assert!(!render_sub_app_present(&app));
    }
}
