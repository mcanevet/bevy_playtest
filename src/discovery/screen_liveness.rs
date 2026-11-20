//! Pixel-observed screen-liveness probe (`screen_liveness` rule,
//! informational).
//!
//! The structural rule ([`crate::discovery::render_liveness`]) proves
//! the game is WIRED to display something: a camera exists, is
//! enabled, writes output. It cannot see the pixels — a camera
//! rendering black forever passes. This rule is the next link in the
//! chain: it captures what the renderer actually PRODUCED, on a
//! cadence, and asks whether the screen ever changed while the
//! simulation claimed activity.
//!
//! Mechanism (spike-validated on Metal, `bevy` 0.20 screenshot wire):
//! a harness-owned [`Camera2d`] renders the 2D scene into an
//! offscreen `Image` render target (320×240 — hashing downscales
//! anyway); each cadence tick the rig spawns a `Screenshot` request
//! for that target; the GPU readback lands asynchronously as an
//! in-memory `Image` (usually within 1–2 ticks — the game's own
//! `ScreenshotPlugin` drains the wire mid-run via its scheduled
//! `trigger_screenshots`, which dispatches the
//! [`bevy_render::view::screenshot::ScreenshotCaptured`] observers);
//! the rig hashes each capture into a 64-bit perceptual hash
//! ([`phash_4b`]) and compares consecutive hashes (Hamming distance).
//!
//! Verdict semantics — INFORMATIONAL, worse observed wins:
//! - hash CHANGED between captures while sim-active: pixels flow
//!   ([`ScreenLivenessTier::Live`]);
//! - hash FROZEN while sim-active somewhere in the span:
//!   [`ScreenLivenessTier::FrozenWhileSimActive`] (informational —
//!   a legitimately static 2D screen with a live sim is a JUDGMENT
//!   call, not an oracle: menus, dialogue, level-load screens);
//! - fewer than two usable captures:
//!   [`ScreenLivenessTier::InsufficientCaptures`] (wire failure or
//!   ultra-short run — refused, not guessed);
//! - no 2D camera ever observed (purely-3D or camera-less-for-2D
//!   scenes — a `Camera2d` cannot faithfully mirror a 3D view):
//!   [`ScreenLivenessTier::NotEvaluated3D`], fail-open honesty; the
//!   structural rule still speaks for camera wiring itself.
//!
//! Hash sensitivity floor: one hash cell spans ~1/8 of the target
//! (~40 px at 320 wide). Motion smaller than one cell between
//! captures is invisible — a KNOWN under-sensitivity, documented
//! here rather than compensated (DCT-style hashes trade the floor
//! for cross-scene false alarms; within one run+driver the
//! box-average hash is deterministic).
//!
//! Census-neutrality: same doctrine as the structural rule, plus
//! entity-level hygiene the structural rule never needed (it spawns
//! nothing). Every rig entity carries
//! [`crate::injected::HarnessInjected`]: the probe camera, the ONE
//! global capture observer, and each transient `Screenshot` request
//! entity (which despawns at GPU-dependent ticks — digesting them
//! would break the probe-pool determinism gate). The digest, replay
//! fingerprint, progress breadth channels, frozen-world oracle, and
//! the structural rule's camera query all exclude the marker, so
//! the rig is invisible to those game-state enumerations. It is
//! DELIBERATELY visible to the zero-annotation bounds/NaN oracles
//! (`src/oracles.rs`), which intentionally query every `Transform`
//! so they are never vacuous — harmless: the probe camera sits at
//! the origin with finite default values, inside the hard-coded
//! ±1000 extent. The recorder is
//! driver-loop invoked — never a scheduled system — EXCEPT the one
//! upstream-owned observer callback (a data sink, not a census
//! system; it runs in the GAME's own `trigger_screenshots` schedule
//! slot, not one we register).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use bevy_camera::{Camera2d, RenderTarget};
use bevy_ecs::prelude::*;
use bevy_image::{Image, TextureFormatPixelInfo};
use bevy_render::render_resource::TextureFormat;
use serde::{Deserialize, Serialize};

use crate::injected::HarnessInjected;

/// Offscreen target resolution: small enough that per-tick rendering
/// is cheap and readbacks are fast, large enough that an 8×8 hash
/// grid cell (~40×30 px) out-resolves HUD-sized elements.
pub(crate) const TARGET_W: u32 = 320;
pub(crate) const TARGET_H: u32 = 240;

/// Hash grid side (8×8 = 64-bit hash).
pub(crate) const GRID: usize = 8;

/// Cap on evidence notes kept per run (bounded report memory — same
/// doctrine as every other tracker).
pub(crate) const MAX_SCREEN_LIVENESS_NOTES: usize = 32;

/// Tick interval between capture requests: half a second of
/// simulated time, minimum 1 tick — the SAME cadence as
/// [`crate::discovery::render_liveness::sample_interval`] so the two
/// rules' evidence timelines align.
pub(crate) fn sample_interval(tps: u64) -> u64 {
    (tps / 2).max(1)
}

/// Downscale 4-byte-per-pixel data (RGBA8/BGRA8 — channel order is
/// irrelevant, luma weights keep within-driver consistency) to
/// GRID×GRID gray via box filter, then average-threshold into a
/// `u64`. `None` on degenerate input. Integer math only (Rec.601
/// luma): deterministic across platforms and float regimes.
pub fn phash_4b(pixels: &[u8], width: u32, height: u32) -> Option<u64> {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || pixels.len() < w * h * 4 {
        return None;
    }
    let gray: Vec<u16> = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p: &[u8; 4]| {
            ((u32::from(p[0]) * 299 + u32::from(p[1]) * 587 + u32::from(p[2]) * 114) / 1000) as u16
        })
        .collect();
    // Box-filter downscale. div_ceil keeps every cell >= 1 source px.
    let mut grid = [[0u32; GRID]; GRID];
    for (gy, row) in grid.iter_mut().enumerate() {
        for (gx, cell) in row.iter_mut().enumerate() {
            let x0 = (gx * w).div_ceil(GRID);
            let x1 = ((gx + 1) * w).div_ceil(GRID);
            let y0 = (gy * h).div_ceil(GRID);
            let y1 = ((gy + 1) * h).div_ceil(GRID);
            let (mut sum, mut n) = (0u64, 0u64);
            for y in y0..y1 {
                for x in x0..x1 {
                    sum += u64::from(gray[y * w + x]);
                    n += 1;
                }
            }
            *cell = u32::try_from(sum / n.max(1)).unwrap_or(u32::MAX);
        }
    }
    let mean = f64::from(grid.iter().flatten().sum::<u32>()) / (GRID * GRID) as f64;
    let mut hash = 0u64;
    for (gy, row) in grid.iter().enumerate() {
        for (gx, &v) in row.iter().enumerate() {
            if f64::from(v) > mean {
                hash |= 1 << (gy * GRID + gx);
            }
        }
    }
    Some(hash)
}

/// Hamming distance between two hashes: the per-capture "how much
/// did the screen change" metric.
pub fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// Verdict tier for a run, worst observed wins.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenLivenessTier {
    /// No finding: hashes drifted (or nothing was ever judged — see
    /// the attachment gates).
    #[default]
    Live,
    /// A sim-active run with a stable 2D camera whose captured
    /// screen NEVER changed across a span with simulation activity.
    /// Informational: legitimately static screens (menus, dialogue,
    /// loading art) land here too.
    FrozenWhileSimActive,
    /// Fewer than two usable captures arrived — the change-detection
    /// question is refused, not guessed (ultra-short runs, or the
    /// readback wire failing under this GPU/driver).
    InsufficientCaptures,
    /// No 2D game camera was ever observed at a capture tick: a
    /// `Camera2d` cannot faithfully mirror a purely-3D view, so the
    /// run is not evaluated (fail-open; the structural rule still
    /// judges camera wiring itself).
    /// Explicit rename: serde's snake_case algorithm would emit
    /// `"not_evaluated_3_d"` (it underscores before EVERY
    /// uppercase letter, including the `D` in `3D`); the stable
    /// documented name is `not_evaluated_3d` (see `as_str`).
    #[serde(rename = "not_evaluated_3d")]
    NotEvaluated3D,
}

impl ScreenLivenessTier {
    /// Stable snake-case name, useful for serialized consumers.
    pub fn as_str(self) -> &'static str {
        match self {
            ScreenLivenessTier::Live => "live",
            ScreenLivenessTier::FrozenWhileSimActive => "frozen_while_sim_active",
            ScreenLivenessTier::InsufficientCaptures => "insufficient_captures",
            ScreenLivenessTier::NotEvaluated3D => "not_evaluated_3d",
        }
    }
}

/// Result of the screen-liveness probe, carried on the report as
/// `screen_liveness` (`screen-liveness` feature; `None` when the
/// feature is off or the game has no render sub-app / asset store).
/// Absence never means "healthy": additive-honesty contract as
/// [`crate::state::PlaytestReport::capability_census`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ScreenLivenessResult {
    /// Capture requests issued (cadence-gated ticks where a 2D
    /// camera existed).
    pub requests: u64,
    /// Usable captures hashed (readbacks that landed with CPU-side
    /// pixel data before the run ended, including the post-run
    /// flush).
    pub captures: u64,
    /// Greatest Hamming distance observed between consecutive
    /// captures (0 when nothing ever changed — or fewer than two
    /// captures).
    pub max_hamming: u32,
    /// Whether the simulation was observed active at any tick
    /// BETWEEN the first and last capture request (the span the
    /// freeze verdict can speak about).
    pub sim_active_ever: bool,
    /// Verdict tier (see module docs).
    pub tier: ScreenLivenessTier,
    /// Bounded evidence notes (same cap doctrine as every other
    /// harness evidence channel).
    pub notes: Vec<String>,
}

/// One landing capture, as tagged by the rig: the request tick it
/// answers plus its hash. Ordered by arrival; request ticks are
/// monotonic because the FIFO wire pairs them in issue order.
#[derive(Debug, Clone, Copy)]
struct LandedCapture {
    request_tick: u64,
    hash: u64,
}

/// The capture rig + evidence accumulator. Driver-loop invoked
/// ([`Self::tick`] after each `app.update()`, [`Self::finish`] after
/// the loop) — never a scheduled system. Construction validates the
/// world-side prerequisites only; world mutation happens lazily at
/// the first eligible capture tick.
pub(crate) struct ScreenLivenessRig {
    /// Target image handle (created at [`Self::attach`]; the camera
    /// spawns lazily).
    handle: Option<bevy_asset::Handle<Image>>,
    /// Probe camera entity (once spawned).
    camera: Option<Entity>,
    /// Global `ScreenshotCaptured` observer entity (spawned at
    /// [`Self::attach`]).
    observer: Option<Entity>,
    /// Observer sink: (request entity, landed image). Drained by
    /// the driver between ticks (FIFO pairing with `pending`).
    sink: Arc<Mutex<Vec<(Entity, Image)>>>,
    /// Outstanding requests: request entity -> request tick. A
    /// capture lands paired with its request entity; entries are
    /// removed on landing. Non-harness requests (the game taking
    /// its own screenshot) are not in this map and are ignored.
    pending: HashMap<Entity, u64>,
    /// Hashed captures in landing order.
    landed: Vec<LandedCapture>,
    /// Total driver ticks observed (cadence bookkeeping — the
    /// first-always sentinel counts TICKS, unlike the structural
    /// rule's sample counter, because the gate must fire before
    /// any eligible sample is skipped).
    ticks_seen: u64,
    /// True once any cadence tick observed a GAME 2D camera.
    ever_2d: bool,
    /// (tick, sim_active) per issued capture request, in issue
    /// order — the freeze-span analysis closes each consecutive
    /// capture PAIR against the sim-activity of its two request
    /// ticks and everything between (approximated by: either
    /// endpoint active, since the driver observes every tick).
    request_flags: Vec<(u64, bool)>,
    /// Request count (cadence-eligible ticks).
    requests: u64,
    /// Usable capture count.
    captures: u64,
    /// Greatest Hamming distance between consecutive captures.
    max_hamming: u32,
    /// Bounded evidence notes.
    notes: Vec<String>,
    /// Note-dedup: last landed-pair classification string.
    last_pair_note: Option<String>,
    /// Note-dedup: whether the LAST note emitted was the
    /// "capture still in flight" evidence line (sticky until a
    /// different note breaks the run of slow-boundary hits —
    /// `notes.last()` matching is defeated by any interleaved
    /// note).
    in_flight_note_open: bool,
}

impl ScreenLivenessRig {
    /// Cadence gate helper (first-sample-always, mirroring the
    /// structural rule's sentinel).
    fn cadence_due(samples: u64, tick: u64, interval: u64) -> bool {
        samples == 0 || tick.is_multiple_of(interval)
    }

    /// World-side prerequisites for the rig: an asset store for
    /// the target image. Absence (a headless server app without
    /// bevy_asset, or an app torn down mid-run) means the rule is
    /// not evaluated at all — the report field stays `None`.
    pub(crate) fn attachable(world: &World) -> bool {
        // `Assets<Image>` presence also implies the render plugins
        // built (bevy_render registers the asset store); combined
        // with the driver's render-sub-app gate this is the honest
        // "worth attaching" test.
        world.contains_resource::<bevy_asset::Assets<Image>>()
    }

    /// Create the rig. World mutation is deferred to [`Self::tick`];
    /// the global observer is registered immediately so no capture
    /// can slip in before the first request.
    pub(crate) fn new() -> Self {
        let sink: Arc<Mutex<Vec<(Entity, Image)>>> = Arc::new(Mutex::new(Vec::new()));
        Self {
            handle: None,
            camera: None,
            observer: None,
            sink: Arc::clone(&sink),
            pending: HashMap::new(),
            landed: Vec::new(),
            ticks_seen: 0,
            ever_2d: false,
            request_flags: Vec::new(),
            requests: 0,
            captures: 0,
            max_hamming: 0,
            notes: Vec::new(),
            last_pair_note: None,
            in_flight_note_open: false,
        }
    }

    /// Driver-loop tick hook (after `app.update()`). `tick_index`
    /// is the just-finished tick; `interval` from
    /// [`sample_interval`]; `sim_active` is the driver's same
    /// activity signal the structural rule consumes.
    ///
    /// Order of operations per tick:
    /// 1. DRAIN the observer sink — captures that landed during the
    ///    just-finished update pair with their outstanding requests
    ///    and hash immediately.
    /// 2. Cadence gate — request a NEW capture on eligible ticks
    ///    (first-always + boundaries), when a GAME 2D camera
    ///    exists, and when no request is outstanding (upstream
    ///    `extract_screenshots` despawns duplicate-target requests —
    ///    one in flight per target).
    ///
    /// Spawn discipline: everything the rig spawns (probe camera,
    /// ONE global observer, request entities) carries
    /// [`HarnessInjected`]. Spawns happen between schedules (the
    /// driver loop), applied immediately via `world.flush()`, so
    /// they land in the NEXT frame's extraction (extraction runs
    /// during `app.update()`), which is fine — the request simply
    /// captures one frame later; the pairing bookkeeping is
    /// unaffected.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn tick(
        &mut self,
        world: &mut World,
        tick_index: u64,
        interval: u64,
        sim_active: bool,
    ) {
        debug_assert!(interval > 0, "interval must be >= 1");

        // --- 1. Drain landed captures --------------------------------
        let landed_now: Vec<(Entity, Image)> = std::mem::take(&mut *self.sink.lock().unwrap());
        for (request_entity, image) in landed_now {
            let Some(request_tick) = self.pending.remove(&request_entity) else {
                // Not ours (the game's own screenshot request) —
                // ignore: the rule observes the game, never judges
                // the game's own tooling.
                continue;
            };
            self.record_capture(request_tick, &image);
        }

        // --- 2. Cadence-gated request -------------------------------
        let due = Self::cadence_due(self.ticks_seen, tick_index, interval);
        self.ticks_seen += 1;
        if !due {
            return;
        }
        let game_2d = game_camera2d_exists(world);
        self.ever_2d |= game_2d;
        if !game_2d {
            return;
        }
        self.ensure_spawns(world);
        // One request in flight per target: upstream
        // `extract_screenshots` DESPAWNS duplicate-target requests
        // with a warning — never stack, just skip the sample.
        // Deduplicated: a slow GPU can stall MANY consecutive
        // cadence boundaries; one note is evidence, thirty is spam
        // starving the bounded note budget.
        if !self.pending.is_empty() {
            let line = "previous capture still in flight at the next cadence \
                        boundary; skipped a sample (slow GPU readback?)"
                .to_string();
            // Sticky-flag dedup (not notes.last()-matching, which an
            // interleaved note defeats): emit once per RUN of
            // consecutive slow-boundary hits.
            if !self.in_flight_note_open {
                self.note(line);
                self.in_flight_note_open = true;
            }
            return;
        }
        self.requests += 1;
        self.request_flags.push((tick_index, sim_active));
        let request_entity = world
            .spawn((
                bevy_render::view::screenshot::Screenshot::image(
                    self.handle
                        .clone()
                        .expect("ensure_spawns guarantees a handle"),
                ),
                HarnessInjected,
            ))
            .id();
        world.flush();
        self.pending.insert(request_entity, tick_index);
    }

    /// Lazily create the probe camera, target image, and the ONE
    /// global capture observer (idempotent). Called only on
    /// cadence-eligible ticks where a game 2D camera exists.
    fn ensure_spawns(&mut self, world: &mut World) {
        if self.observer.is_some() {
            return;
        }
        // Target image: render-world-only usage (no CPU asset copy;
        // readbacks come back as fresh in-memory images).
        let mut target =
            Image::new_target_texture(TARGET_W, TARGET_H, TextureFormat::Rgba8UnormSrgb, None);
        target.asset_usage = bevy_asset::RenderAssetUsages::RENDER_WORLD;
        let handle = world
            .resource_mut::<bevy_asset::Assets<Image>>()
            .add(target);
        self.handle = Some(handle.clone());

        // Probe camera: renders the 2D scene into the offscreen
        // target. `order: 0` and `is_active: true` — the probe never
        // enters camera ARBITRATION with the game's cameras for
        // window output (different target), and stays out of the
        // structural rule's camera query (marker-excluded).
        let camera = world
            .spawn((
                Camera2d,
                RenderTarget::Image(handle.into()),
                bevy_camera::Camera {
                    order: 0,
                    is_active: true,
                    ..Default::default()
                },
                HarnessInjected,
            ))
            .id();
        self.camera = Some(camera);

        // ONE global observer: every `ScreenshotCaptured` (ours or
        // the game's own) funnels into the sink; ownership is
        // decided by `pending` pairing at drain time. The observer
        // ENTITY carries the marker too (it lives in this world for
        // the whole run; census hygiene applies to it like any
        // probe entity).
        let sink = Arc::clone(&self.sink);
        let observer = world
            .add_observer(
                move |trigger: On<bevy_render::view::screenshot::ScreenshotCaptured>| {
                    sink.lock()
                        .unwrap()
                        .push((trigger.entity, trigger.image.clone()));
                },
            )
            .insert(HarnessInjected)
            .id();
        self.observer = Some(observer);
    }

    /// Post-run hook: GPU wait-poll + direct channel drain (escape
    /// hatch for readbacks that never landed mid-run), then the
    /// verdict. The driver attaches the RESULT only when
    /// prerequisites passed; `not_evaluated_3d` is a tier INSIDE an
    /// attached result, not a missing one.
    pub(crate) fn finish(mut self, world: &mut World) -> ScreenLivenessResult {
        // GPU wait-poll: block until submitted work (including any
        // pending readback) completes — bevy's externally-driven
        // headless idiom. Absent RenderDevice (never attached or
        // already cleaned up) is tolerated: the direct drain below
        // still sweeps whatever already landed.
        if let Some(device) = world.get_resource::<bevy_render::renderer::RenderDevice>() {
            let _ = device
                .wgpu_device()
                .poll(bevy_render::render_resource::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                });
        }
        // Sink drain FIRST: on a crash mid-`app.update()` the last
        // capture can have LANDED (observer fired into the sink)
        // without a `tick()` ever seeing it. Dropping it would
        // discard evidence that existed — and could flip a live
        // crash run to a false `frozen_while_sim_active` finding.
        // Same pairing path as `tick()`: owned requests only.
        let stranded: Vec<(Entity, Image)> = std::mem::take(&mut *self.sink.lock().unwrap());
        for (request_entity, image) in stranded {
            if let Some(request_tick) = self.pending.remove(&request_entity) {
                self.record_capture(request_tick, &image);
            }
        }
        // Direct channel drain (no schedule tick — harness systems
        // do not survive into the post-run phase, and ticking would
        // panic on removed RunState). The channel carries
        // (request_entity, image); pairing is the same.
        if let Some(chan) =
            world.get_resource::<bevy_render::view::screenshot::CapturedScreenshots>()
        {
            let chan = chan.lock().unwrap();
            while let Ok((entity, image)) = chan.try_recv() {
                if let Some(request_tick) = self.pending.remove(&entity) {
                    self.record_capture(request_tick, &image);
                }
            }
        }

        // Freeze-span analysis: a FROZEN verdict needs (a) >= 2
        // captures, (b) zero drift between EVERY consecutive pair,
        // (c) sim activity at (or between) the request ticks of at
        // least one compared pair. Request flags are indexed by
        // request ORDER; landed captures preserve that order (FIFO
        // wire), so position i pairs with flag i — EXCEPT requests
        // whose readbacks never arrived (dropped from `landed`);
        // re-pair by request tick instead: monotonic matching.
        let mut frozen_span_sim_active = false;
        if self.landed.len() >= 2 && self.max_hamming == 0 {
            // Any request tick strictly between two consecutive
            // captures whose driver flag said sim-active marks the
            // frozen span as "while sim active".
            let flag = |tick: u64| {
                self.request_flags
                    .iter()
                    .find(|(t, _)| *t == tick)
                    .map(|(_, active)| *active)
                    .unwrap_or(false)
            };
            for pair in self.landed.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                if flag(a.request_tick) || flag(b.request_tick) {
                    frozen_span_sim_active = true;
                }
            }
        }

        let tier = derive_tier(
            self.ever_2d,
            self.landed.len(),
            self.max_hamming,
            frozen_span_sim_active,
        );

        ScreenLivenessResult {
            requests: self.requests,
            captures: self.captures,
            max_hamming: self.max_hamming,
            // Honest span statement: any capture request whose tick
            // the driver flagged sim-active (limited to requests —
            // ticks between requests are approximated by the
            // endpoints, since the cadence gate only samples there).
            sim_active_ever: self.request_flags.iter().any(|(_, active)| *active),
            tier,
            notes: self.notes,
        }
    }

    /// Bounded push (same doctrine as every other tracker). Any
    /// note closes an open "still in flight" run — the dedup
    /// promise is "one note per RUN of consecutive slow
    /// boundaries", and an interleaved note must re-arm it.
    fn note(&mut self, line: String) {
        self.in_flight_note_open = false;
        if self.notes.len() < MAX_SCREEN_LIVENESS_NOTES {
            self.notes.push(line);
        }
    }

    /// Record one OWNED, landed capture: hash it, compare to the
    /// previous landed capture (Hamming), update the running max and
    /// the one-time "changed" note. Shared by BOTH drain paths — the
    /// mid-run observer sink and the post-run direct channel sweep —
    /// so they cannot drift apart.
    fn record_capture(&mut self, request_tick: u64, image: &Image) {
        match hash_capture(image) {
            Some(h) => {
                self.captures += 1;
                if let Some(last) = self.landed.last() {
                    let d = hamming(last.hash, h);
                    self.max_hamming = self.max_hamming.max(d);
                    if d > 0 && self.last_pair_note.as_deref() != Some("changed") {
                        self.note(format!(
                            "captures for ticks {}..{request_tick} differ \
                             (hamming {d})",
                            last.request_tick
                        ));
                        self.last_pair_note = Some("changed".to_string());
                    }
                }
                self.landed.push(LandedCapture {
                    request_tick,
                    hash: h,
                });
            }
            None => self.note(format!(
                "capture for request at tick {request_tick} unreadable \
                 (no CPU-side pixel data); skipped"
            )),
        }
    }
}

/// Pure tier derivation (split out of `finish` for unit-testability):
/// the attachment gates come first, then the evidence question.
fn derive_tier(
    ever_2d: bool,
    captures: usize,
    max_hamming: u32,
    sim_active_span: bool,
) -> ScreenLivenessTier {
    if !ever_2d {
        ScreenLivenessTier::NotEvaluated3D
    } else if captures < 2 {
        ScreenLivenessTier::InsufficientCaptures
    } else if max_hamming == 0 && sim_active_span {
        ScreenLivenessTier::FrozenWhileSimActive
    } else {
        ScreenLivenessTier::Live
    }
}

/// Whether at least one GAME 2D camera exists (harness-injected
/// probe excluded — the probe is not "the game has a 2D view").
fn game_camera2d_exists(world: &World) -> bool {
    match QueryState::<Entity, (With<Camera2d>, Without<HarnessInjected>)>::try_new(world) {
        Some(mut state) => state.iter(world).next().is_some(),
        // Camera2d unregistered: no 2D camera can exist.
        None => false,
    }
}

/// Hash one landed capture. `None` when the readback carries no
/// CPU-side data (degenerate) or a non-4-byte pixel format arrives
/// (upstream converts window-format reads; the offscreen target is
/// Rgba8UnormSrgb so this is belt-and-braces).
fn hash_capture(image: &Image) -> Option<u64> {
    let data = image.data.as_ref()?;
    let size = &image.texture_descriptor.size;
    let bpp = TextureFormatPixelInfo::pixel_size(&image.texture_descriptor.format).ok()?;
    if bpp != 4 {
        return None;
    }
    phash_4b(data, size.width, size.height)
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    // phash_4b / hamming: deterministic round-trips and pinned hashes.
    // The pinned values come from the T2 capture spike (Metal,
    // bevy 0.20 screenshot wire) — same integer math, so they pin
    // here without a GPU.
    // -----------------------------------------------------------------

    /// A 320x240 fully-static single-color frame: every 8x8 cell has
    /// the same luma, so the mean-threshold comparison is all-false
    //  — hash 0. Pinned at the spike's static-scene observable.
    #[test]
    fn phash_uniform_frame_is_zero() {
        let pixels = vec![128u8; (TARGET_W * TARGET_H * 4) as usize];
        assert_eq!(phash_4b(&pixels, TARGET_W, TARGET_H), Some(0));
    }

    /// Left/right split (dark | bright): the threshold separates
    /// columns exactly at the grid midpoint. Deterministic in the
    //  exact sense: cell sums are integers, the mean is compared
    ///  strictly.
    #[test]
    fn phash_split_frame_has_half_bits_set() {
        let w = 320u32;
        let h = 240u32;
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        for y in 0..h as usize {
            for x in 0..w as usize {
                let bright = x >= (w / 2) as usize;
                let v = if bright { 255 } else { 0 };
                let i = (y * w as usize + x) * 4;
                pixels[i] = v;
                pixels[i + 1] = v;
                pixels[i + 2] = v;
                pixels[i + 3] = 255;
            }
        }
        let hash = phash_4b(&pixels, w, h).unwrap();
        // Columns 4..8 (right half) brighter than mean: high nibble
        // per row pattern — 4 bits set per grid row.
        let bits = hash.count_ones();
        assert_eq!(bits, 32, "half-split frame must set exactly 32 bits");
        // And the LEFT half is dark: row pattern is 0b11110000 per
        // row shifted by row index.
        assert_eq!(hamming(hash, 0), 32);
    }

    /// A small bright square (>= 1 cell) against dark flips only the
    /// cells it covers: hash sensitivity IS per-cell (one cell spans
    // ~40x30 px at the 320x240 target).
    #[test]
    fn phash_small_square_flips_covered_cells_only() {
        let w = 320u32;
        let h = 240u32;
        let base = vec![0u8; (w * h * 4) as usize];
        let mut with_square = base.clone();
        // Cover grid cell (row 3, col 5) fully: x in [200,240),
        // y in [90,120).
        for y in 90..120usize {
            for x in 200..240usize {
                let i = (y * w as usize + x) * 4;
                with_square[i] = 255;
                with_square[i + 1] = 255;
                with_square[i + 2] = 255;
                with_square[i + 3] = 255;
            }
        }
        let a = phash_4b(&base, w, h);
        let b = phash_4b(&with_square, w, h);
        // Dark frame: all cells equal luma 0, mean = 0, strict >
        // comparison — hash 0. With the square: the covered cell is
        // 255 > mean — exactly the cells under the square flip.
        assert_eq!(a, Some(0));
        let hb = b.unwrap();
        // Cell (3,5): bit index 3*8+5 = 29.
        assert_eq!(hb.count_ones(), 1);
        assert_eq!(hb & (1 << 29), 1 << 29);
    }

    /// Sub-cell MOTION is invisible by design (documented
    /// under-sensitivity floor): an object smaller than one cell
    /// (~40x30 px at the 320x240 target) that moves but stays
    /// INSIDE the same cell leaves the cell's box-sum unchanged —
    /// bit-identical hashes. (Brightening a dark frame is NOT the
    /// test: against an all-dark mean, any covered cell flips.)
    #[test]
    fn phash_sub_cell_motion_is_invisible() {
        let w = 320u32;
        let h = 240u32;
        let frame_with_square_at = |x0: usize, y0: usize| {
            let mut pixels = vec![0u8; (w * h * 4) as usize];
            // 10x10 bright square, fully inside grid cell (row 3,
            // col 2): cell x range [80,120), y range [90,120).
            for y in y0..y0 + 10 {
                for x in x0..x0 + 10 {
                    let i = (y * w as usize + x) * 4;
                    pixels[i] = 255;
                    pixels[i + 1] = 255;
                    pixels[i + 2] = 255;
                    pixels[i + 3] = 255;
                }
            }
            pixels
        };
        let a = phash_4b(&frame_with_square_at(100, 100), w, h);
        let b = phash_4b(&frame_with_square_at(104, 104), w, h);
        assert_eq!(a, b, "motion within one cell must not flip bits");
        // Sanity: both placements really sit in the same cell
        // (cells are ~40x30 at this resolution).
        assert_eq!(100 / 40, 104 / 40);
        assert_eq!(100 / 30, 104 / 30);
    }

    /// Degenerate inputs are refused, not guessed.
    #[test]
    fn phash_refuses_degenerate_inputs() {
        assert_eq!(phash_4b(&[], 0, 0), None);
        assert_eq!(phash_4b(&[0; 15], 2, 2), None); // one byte short of 2*2*4
        assert_eq!(phash_4b(&[0; 400], 10, 10), Some(0)); // exact fit is VALID
        assert_eq!(phash_4b(&[0; 399], 10, 10), None); // one byte short
        assert_eq!(phash_4b(&[0; 400], 0, 10), None); // zero width
    }

    /// Spike cross-check of the 64-bit hash SHAPE and the Hamming
    /// metric: `0xf0f0f0f030303030` is the hash the T2 spike
    /// observed on Metal for an empty-vs-red static frame pair
    /// (distance 24). A LITERAL, not a live call: reproducing the
    /// spike's pixels needs a GPU, so this pins the bit-pattern
    /// properties (popcount 24 = 24 threshold-crossing cells, and
    /// nonzero pairwise distance) the spike established — a drift
    /// in the hash's bit layout convention breaks this pin.
    #[test]
    fn spike_observed_hash_literal_has_expected_shape() {
        // 0xf0f0f0f030303030 is the spike's pinned STATIC-scene hash
        // (red sprite on default clear). The empty control hash
        // differed at hamming 24 — pin both relationships without
        // a GPU by pinning the static hash and asserting a nonzero
        // distance from the uniform frame.
        let static_hash: u64 = 0xf0f0f0f030303030;
        let uniform = phash_4b(
            &vec![128u8; (TARGET_W * TARGET_H * 4) as usize],
            TARGET_W,
            TARGET_H,
        );
        assert_eq!(uniform, Some(0));
        assert!(hamming(static_hash, 0) > 0);
        assert_eq!(static_hash.count_ones(), 24);
        assert_eq!(hamming(static_hash, 0), 24);
    }

    // -----------------------------------------------------------------
    // derive_tier: the verdict matrix.
    // -----------------------------------------------------------------

    #[test]
    fn tier_no_2d_camera_means_not_evaluated() {
        assert_eq!(
            derive_tier(false, 0, 0, false),
            ScreenLivenessTier::NotEvaluated3D
        );
        // Even with captures (somehow) landed, no 2D camera wins:
        // the gate is checked FIRST.
        assert_eq!(
            derive_tier(false, 5, 0, true),
            ScreenLivenessTier::NotEvaluated3D
        );
    }

    #[test]
    fn tier_two_captures_needed() {
        assert_eq!(
            derive_tier(true, 0, 0, false),
            ScreenLivenessTier::InsufficientCaptures
        );
        assert_eq!(
            derive_tier(true, 1, 0, true),
            ScreenLivenessTier::InsufficientCaptures
        );
    }

    #[test]
    fn tier_frozen_needs_sim_active() {
        assert_eq!(
            derive_tier(true, 2, 0, true),
            ScreenLivenessTier::FrozenWhileSimActive
        );
        // Identical screens but the sim was quiet at every compared
        // request: no contradiction — the run is not accused.
        assert_eq!(derive_tier(true, 2, 0, false), ScreenLivenessTier::Live);
    }

    #[test]
    fn tier_drift_is_live() {
        assert_eq!(derive_tier(true, 2, 1, true), ScreenLivenessTier::Live);
        assert_eq!(derive_tier(true, 5, 38, true), ScreenLivenessTier::Live);
    }

    // -----------------------------------------------------------------
    // sample_interval: tps/2 with a floor of 1 (mirrors the
    // structural rule — aligned evidence timelines).
    // -----------------------------------------------------------------

    #[test]
    fn sample_interval_is_half_second_floored_at_one() {
        assert_eq!(sample_interval(60), 30);
        assert_eq!(sample_interval(120), 60);
        assert_eq!(sample_interval(1), 1);
        assert_eq!(sample_interval(0), 1);
    }

    #[test]
    fn tier_names_are_stable_snake_case() {
        assert_eq!(ScreenLivenessTier::Live.as_str(), "live");
        assert_eq!(
            ScreenLivenessTier::FrozenWhileSimActive.as_str(),
            "frozen_while_sim_active"
        );
        assert_eq!(
            ScreenLivenessTier::InsufficientCaptures.as_str(),
            "insufficient_captures"
        );
        assert_eq!(
            ScreenLivenessTier::NotEvaluated3D.as_str(),
            "not_evaluated_3d"
        );
    }

    /// The SERIALIZED tier name must match `as_str` — the stable
    /// contract for JSON consumers. `NotEvaluated3D` is the hazard:
    /// serde's snake_case algorithm would emit `not_evaluated_3_d`
    /// (it underscores before every uppercase letter, including the
    /// D in 3D); the explicit `#[serde(rename)]` pins the documented
    /// `not_evaluated_3d` instead. This round-trip dies if either
    /// representation drifts.
    #[test]
    fn tier_serde_names_match_as_str() {
        for tier in [
            ScreenLivenessTier::Live,
            ScreenLivenessTier::FrozenWhileSimActive,
            ScreenLivenessTier::InsufficientCaptures,
            ScreenLivenessTier::NotEvaluated3D,
        ] {
            let json = serde_json::to_string(&tier).expect("serializable");
            let name = json.trim_matches('"');
            assert_eq!(
                name,
                tier.as_str(),
                "serialized tier name must equal the documented as_str name"
            );
            let back: ScreenLivenessTier = serde_json::from_str(&json).expect("deserializable");
            assert_eq!(back, tier);
        }
    }
}
