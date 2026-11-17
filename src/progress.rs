//! Progress-liveness oracle: distinguishes a CHURNING-BUT-DEAD world
//! from one that is actually progressing.
//!
//! The frozen-world oracle catches dead frame loops; it cannot catch
//! the opposite failure — a world that looks perfectly alive (frames
//! ticking, transforms jittering every frame) while no game state
//! actually evolves. This oracle samples an observable game-state
//! SURFACE on a fixed cadence and asks, once per window: did anything
//! verifiably change?
//!
//! Doctrine (same as the replay fingerprint, state digest, and
//! component_check recorders): sampling happens in the DRIVER LOOP
//! after `app.update()` returns — never as a scheduled system.
//! `&World` reflection access cannot coexist with `ResMut<RunViolations>`
//! in one system, and only post-update observation sees the fully
//! settled frame. The per-window VERDICT is a post-run audit: it needs
//! the full sample series, not any single frame.
//!
//! Surface (per [`ProgressMode`]):
//! - `auto`: counts-only breadth — entity count, archetype count,
//!   reflected component-type counts — plus marker values and
//!   `include_paths`-scoped resource values.
//! - `markers`: only the named markers.
//! - `off`: the recorder is never inserted (zero cost, silence).
//!
//! Entity/archetype counts are recorded and exported for breadth, but
//! they are NEVER progress evidence: their changes measure engine and
//! harness bookkeeping (spawns, despawns, archetype shuffles), not
//! game-state evolution. The stalled analysis skips them; only the
//! path-addressable channels (component counts, markers, scoped
//! resources) can acquit a window.
//!
//! Sampling grid: both periods are quantized to whole frames at
//! construction, and the window is a WHOLE number of sample intervals
//! (see [`ProgressRecorder::new`]) — a sample lands exactly on every
//! window boundary, so windows tile the frame axis cleanly from the
//! first sample. Authored configs are held to this grid at validation;
//! the default config rides along unvalidated and is re-quantized
//! coherently instead.
//!
//! Heartbeat/ticker suppression: a quantity whose value changed in
//! ≥90% of a window's sample transitions is a ticker for THAT window
//! and does not count as progress evidence (a score that increments
//! every frame is a clock, not progress). Recomputed per window; no
//! cross-window state.
//!
//! Verdict per window: `stalled_progress` iff the run was churning
//! (`RunState::frozen_frames == 0` — the frozen-world oracle saw
//! activity) at EVERY sampled frame in the window AND no non-ticker
//! surface change occurred. Truly frozen worlds are the frozen-world
//! oracle's finding, not this one; this oracle fires exactly where
//! the streak never builds.

use bevy_ecs::prelude::*;

use serde::{Deserialize, Serialize};

use crate::digest::{
    collect_component_type_counts, collect_resource_reflections_locked, fnv_hasher, hash_canonical,
    hash_u64, hash_usize,
};
use crate::rules::Rule;
use crate::scenario::{ProgressConfig, ProgressMode};
use crate::state::ViolationEntry;

/// A quantity whose value changed in at least this fraction of a
/// window's sample transitions is a ticker (heartbeat) for that window.
const TICKER_FRACTION_NUM: u64 = 9;
const TICKER_FRACTION_DEN: u64 = 10;

/// Sentinel value-hash for a resource channel whose resource is
/// ABSENT this sample. A resource appearing/disappearing between
/// samples must register as a change (toggle-detection), so absence
/// gets its own hashable value instead of a skip.
const ABSENT_RESOURCE_HASH: u64 = 0x4242_AB5E_0000_0001;

/// A tracked observable quantity.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Channel {
    /// Live entity count (auto mode; cannot be path-excluded).
    EntityCount,
    /// Archetype count (auto mode; cannot be path-excluded).
    ArchetypeCount,
    /// Entity count for one reflected component type (full path).
    ComponentCount(String),
    /// Canonical value hash of one reflected resource (full path).
    ResourceValue(String),
}

impl Channel {
    /// Human-readable name for report details.
    fn label(&self) -> &str {
        match self {
            Channel::EntityCount => "entity_count",
            Channel::ArchetypeCount => "archetype_count",
            Channel::ComponentCount(path) => path,
            Channel::ResourceValue(path) => path,
        }
    }

    /// Bookkeeping pseudo-channels (entity/archetype totals). Recorded
    /// and exported for breadth, but never admitted as progress
    /// evidence in the stalled analysis (module doctrine): they
    /// measure harness/engine bookkeeping, not game-state evolution.
    fn is_bookkeeping(&self) -> bool {
        matches!(self, Channel::EntityCount | Channel::ArchetypeCount)
    }
}

/// One recorded surface sample (raw, pre-audit; parallel to
/// `ProgressRecorder::channels`).
struct RawSample {
    frame: u64,
    /// Was the run churning at this frame (frozen-world oracle saw
    /// activity: `frozen_frames == 0`)?
    churn: bool,
    /// Per-channel values, indexed like `channels`.
    values: Vec<u64>,
    /// Canonical hash of the whole surface at this sample.
    surface_hash: u64,
}

/// Serializable per-sample series exported on the report
/// (`progress_samples`): the evidence behind every progress verdict,
/// comparable across runs (determinism checking) and inspectable
/// without re-running the game.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ProgressSample {
    pub frame: u64,
    /// Canonical hash of the whole observable surface.
    pub surface_hash: u64,
    /// Whether the run was churning (frame loop alive) this frame.
    pub churn: bool,
    /// How many channels the surface had (0 = empty surface).
    pub channels: u32,
}

/// Resource enabling progress-liveness recording. Inserted by the
/// driver when `liveness.progress.mode != off`; absent otherwise.
#[derive(Resource, Default)]
pub struct ProgressRecorder {
    config: ProgressConfig,
    /// Sample every N frames (derived from `sample_s`).
    sample_interval: u64,
    /// Window length in frames: a WHOLE number of `sample_interval`s,
    /// so samples land exactly on every window boundary and windows
    /// tile the frame axis with no gaps (see `new`).
    window_frames: u64,
    /// Channels are resolved lazily on the FIRST sample (the type
    /// registry is settled by then; resolving at insertion time would
    /// run before plugin-building guarantees).
    channels: Vec<Channel>,
    initialized: bool,
    /// Marker names that failed to resolve (unknown or ambiguous),
    /// each reported once as an oracle_error.
    unresolved: Vec<String>,
    samples: Vec<RawSample>,
}

impl ProgressRecorder {
    /// Quantize the authored periods onto ONE coherent frame grid:
    /// `sample_interval = round(sample_s · tps)` (≥1), then
    /// `window_frames = round(window_s · tps / sample_interval) ·
    /// sample_interval` (≥1 interval) — the window is a whole number
    /// of sample intervals by construction, whatever the authored
    /// seconds. Independent rounding of the two periods (the naive
    /// `round(window_s · tps)`) can straddle a window boundary with no
    /// sample on it, leaving sliver windows that no verdict can
    /// faithfully cover. Authored configs that do not already sit on
    /// this grid are rejected at validation; this re-quantization is
    /// the safety net for the (unvalidated) default config riding
    /// along every scenario.
    pub fn new(config: ProgressConfig, tps: u64) -> Self {
        let sample_interval = ((config.sample_s * tps as f32).round() as u64).max(1);
        let intervals =
            ((config.window_s * tps as f32 / sample_interval as f32).round() as u64).max(1);
        Self {
            config,
            sample_interval,
            window_frames: intervals * sample_interval,
            channels: Vec::new(),
            initialized: false,
            unresolved: Vec::new(),
            samples: Vec::new(),
        }
    }
}

/// Driver-loop recorder: sample the progress surface. No-op when the
/// recorder is absent (progress mode `off`). Uses `resource_scope` —
/// never remove/reinsert (resources are entities in this Bevy version;
/// despawning the recorder mid-sample would transiently shift the very
/// entity/archetype counts the surface measures).
pub fn record_progress_sample(world: &mut World, frame: u64) {
    if !world.contains_resource::<ProgressRecorder>() {
        return;
    }
    world.resource_scope(|world, mut recorder: Mut<ProgressRecorder>| {
        let churn = world
            .get_resource::<crate::driver::RunState>()
            .is_some_and(|s| s.frozen_frames == 0);
        recorder.sample(world, frame, churn);
    });
}

impl ProgressRecorder {
    fn sample(&mut self, world: &World, frame: u64, churn: bool) {
        if !self.initialized {
            self.initialize(world);
            self.initialized = true;
        }
        if !frame.is_multiple_of(self.sample_interval) {
            return;
        }
        let mut values = Vec::with_capacity(self.channels.len());
        // Lazily-shared per-sample maps: one reflection/registry walk
        // serves every channel of its kind (not one per channel).
        let mut resource_hashes: Option<Vec<(String, u64)>> = None;
        let mut component_counts: Option<std::collections::BTreeMap<String, usize>> = None;
        for channel in &self.channels {
            let value = match channel {
                Channel::EntityCount => world.entities().count_spawned() as u64,
                Channel::ArchetypeCount => world.archetypes().len() as u64,
                Channel::ComponentCount(path) => {
                    let counts = component_counts
                        .get_or_insert_with(|| collect_component_type_counts(world));
                    counts.get(path).copied().unwrap_or(0) as u64
                }
                Channel::ResourceValue(path) => {
                    let hashes =
                        resource_hashes.get_or_insert_with(|| resource_value_hashes(world));
                    hashes
                        .iter()
                        .find(|(p, _)| p == path)
                        .map(|(_, h)| *h)
                        .unwrap_or(ABSENT_RESOURCE_HASH)
                }
            };
            values.push(value);
        }

        // Whole-surface canonical hash (sorted path-hash/value pairs;
        // channel order is not canonical across runs).
        let mut keyed: Vec<(u64, u64)> = self
            .channels
            .iter()
            .zip(&values)
            .map(|(c, v)| (channel_key_hash(c), *v))
            .collect();
        keyed.sort_unstable();
        let mut hasher = fnv_hasher();
        hasher = hash_usize(hasher, keyed.len());
        for (k, v) in keyed {
            hasher = hash_u64(hasher, k);
            hasher = hash_u64(hasher, v);
        }

        self.samples.push(RawSample {
            frame,
            churn,
            values,
            surface_hash: hasher,
        });
    }

    /// Resolve channels once, from the settled first-frame registry.
    fn initialize(&mut self, world: &World) {
        if self.config.mode == ProgressMode::Auto {
            self.channels.push(Channel::EntityCount);
            self.channels.push(Channel::ArchetypeCount);
            for (path, _count) in collect_component_type_counts(world) {
                if path_is_excluded(&self.config, &path) {
                    continue;
                }
                self.channels.push(Channel::ComponentCount(path));
            }
        }
        // Markers count in every non-off mode (auto unions them onto
        // the counts-only core). Resource values additionally need
        // `include_paths` (auto) — never hashed unscoped.
        let wanted = marker_names_to_resolve(&self.config);
        let mut include: Vec<String> = Vec::new();
        if self.config.mode == ProgressMode::Auto {
            include = self
                .config
                .include_paths
                .iter()
                .filter(|p| !p.trim().is_empty())
                .cloned()
                .collect();
        }
        if wanted.is_empty() && include.is_empty() {
            return;
        }
        let Some(registry_arc) = world.get_resource::<bevy_ecs::reflect::AppTypeRegistry>() else {
            self.unresolved
                .extend(wanted.iter().map(|n| format!("{n}: no type registry")));
            return;
        };
        let registry = registry_arc.0.read();
        let pairs = collect_resource_reflections_locked(world, &registry);
        for name in &wanted {
            match resolve_resource(&pairs, name) {
                Ok(path) => {
                    if path_is_excluded(&self.config, &path) {
                        continue;
                    }
                    let channel = Channel::ResourceValue(path);
                    if !self.channels.contains(&channel) {
                        self.channels.push(channel);
                    }
                }
                Err(problem) => self.unresolved.push(format!("{name}: {problem}")),
            }
        }
        // `include_paths` entries ending in `::` are PATH PREFIXES:
        // they scope every reflected resource under that module path
        // into the surface (one entry can watch a whole game
        // namespace). Other entries keep exact/full-path/short-name
        // resolution — an explicit type is usually intentional and
        // should fail loudly if mistyped, not silently broaden.
        for entry in &include {
            if entry.ends_with("::") {
                let prefix = entry.trim_start_matches("::");
                let mut matched = 0usize;
                for (path, _) in &pairs {
                    if !path.starts_with(prefix) || path_is_excluded(&self.config, path) {
                        continue;
                    }
                    matched += 1;
                    let channel = Channel::ResourceValue(path.clone());
                    if !self.channels.contains(&channel) {
                        self.channels.push(channel);
                    }
                }
                // Zero matches is reported, not silently dropped: a
                // prefix that matches nothing means the oracle is
                // watching an empty scope — either the module path is
                // wrong or the game registers no reflected resources
                // under it yet. Both are author-actionable; pass would
                // hide a broken config behind a green verdict.
                if matched == 0 {
                    self.unresolved.push(format!(
                        "{entry}: no reflected resources under this path prefix — \
                         check the module path or register the resource types \
                         (Reflect + register_type)"
                    ));
                }
            } else {
                match resolve_resource(&pairs, entry) {
                    Ok(path) => {
                        if path_is_excluded(&self.config, &path) {
                            continue;
                        }
                        let channel = Channel::ResourceValue(path);
                        if !self.channels.contains(&channel) {
                            self.channels.push(channel);
                        }
                    }
                    Err(problem) => self.unresolved.push(format!("{entry}: {problem}")),
                }
            }
        }
    }
}

/// Marker names needing registry resolution (every non-off mode).
fn marker_names_to_resolve(config: &ProgressConfig) -> Vec<String> {
    config
        .markers
        .iter()
        .filter(|m| !m.trim().is_empty())
        .cloned()
        .collect()
}

/// User `exclude` prefixes subtract from every path-addressable
/// channel (component counts, resource values) in every mode. The
/// auto-mode count pseudo-channels (entity/archetype totals) are
/// global and cannot be path-excluded.
fn path_is_excluded(config: &ProgressConfig, path: &str) -> bool {
    config.exclude.iter().any(|prefix| {
        !prefix.trim().is_empty() && path.starts_with(prefix.trim_start_matches("::"))
    })
}

/// Resolve a resource by full path, or by short path when uniquely
/// matching — ambiguous short names are rejected loudly (same
/// doctrine as invariant target resolution).
fn resolve_resource(
    pairs: &[(String, &dyn bevy_reflect::Reflect)],
    name: &str,
) -> Result<String, String> {
    if let Some((path, _)) = pairs.iter().find(|(p, _)| p == name) {
        return Ok(path.clone());
    }
    let shorts: Vec<&str> = pairs
        .iter()
        .filter(|(p, _)| short_path(p) == name)
        .map(|(p, _)| p.as_str())
        .collect();
    match shorts.len() {
        0 => Err(format!(
            "no reflected resource matches '{name}' — register the \
             resource type (Reflect + register_type) and use its full \
             path or an unambiguous short name"
        )),
        1 => Ok(shorts[0].to_string()),
        _ => Err(format!(
            "short name '{name}' is ambiguous among {} reflected \
             resources — use the full type path (one of: {})",
            shorts.len(),
            shorts.join(", ")
        )),
    }
}

fn short_path(path: &str) -> &str {
    path.rsplit("::").next().unwrap_or(path)
}

fn channel_key_hash(channel: &Channel) -> u64 {
    let mut h = fnv_hasher();
    let label = channel.label();
    h = hash_usize(h, label.len());
    for b in label.bytes() {
        h = hash_u64(h, b as u64);
    }
    h
}

/// Per-sample canonical value hashes of every reflected game resource.
fn resource_value_hashes(world: &World) -> Vec<(String, u64)> {
    let Some(registry_arc) = world.get_resource::<bevy_ecs::reflect::AppTypeRegistry>() else {
        return Vec::new();
    };
    let registry = registry_arc.0.read();
    let pairs = collect_resource_reflections_locked(world, &registry);
    pairs
        .into_iter()
        .map(|(path, reflect)| {
            let mut value_h = fnv_hasher();
            hash_canonical(reflect.as_partial_reflect(), &mut value_h);
            (path, value_h)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Post-run audit
// ---------------------------------------------------------------------------

/// Result of auditing the recorded sample series.
pub struct ProgressAudit {
    pub entries: Vec<ViolationEntry>,
    pub samples: Vec<ProgressSample>,
}

/// Post-run audit: evaluate every complete window and emit
/// `stalled_progress` (gating, churning-but-dead crossings),
/// `progress_surface_empty` (informational), and `oracle_error` for
/// marker names that never resolved.
pub fn audit_progress(recorder: ProgressRecorder) -> ProgressAudit {
    let config = &recorder.config;
    let samples = &recorder.samples;
    // Coherence by construction: the audit judges windows on the SAME
    // grid the recorder sampled on (stored at `new`), never an
    // independently-rounded recomputation. `.max(1)` guards a
    // `Default`-constructed recorder (never used for real audits).
    let window_frames = recorder.window_frames.max(1);
    let channels = &recorder.channels;

    let mut entries: Vec<ViolationEntry> = Vec::new();

    // A verdict (even the informational empty-surface note) requires at
    // least one COMPLETE window: the default config rides along every
    // scenario, including sub-second smoke runs where progress is not
    // the point — issuing empty-surface notes (which escalate under
    // strict_warnings) for runs shorter than one window would gate them
    // on a verdict the oracle never actually reached. The samples
    // themselves are still exported (they are evidence, not verdicts).
    // Boundary-consistent with the per-window rule below: a window
    // [first, first+window_frames) is complete iff the sample series
    // reaches its last frame (first + window_frames - 1).
    let has_complete_window = !samples.is_empty()
        && samples.first().is_some_and(|f| {
            f.frame + window_frames - 1 <= samples.last().expect("checked nonempty").frame
        });

    // Unresolved surface entries (markers, exact include paths, or
    // empty include prefixes): infrastructure-visible authoring error —
    // loud, once per offending name. Reported even without a complete
    // window: the entry never resolved at all, which is an authoring
    // failure visible from the first sample on.
    for problem in &recorder.unresolved {
        entries.push(ViolationEntry {
            rule: Rule::OracleError.name().to_string(),
            target: "progress".to_string(),
            first_frame: 0,
            last_frame: 0,
            count: 1,
            detail: format!("progress surface entry failed to resolve — {problem}"),
            last_detail: format!("surface entry unresolved: {problem}"),
            meta: None,
        });
    }

    // Empty surface: nothing GAME-verifiable was in scope. In auto
    // mode the entity/archetype pseudo-channels always exist, but they
    // count engine and harness bookkeeping too — they are not
    // evidence that THIS game's state is observable. The verifiable
    // surface is the path-addressable channels: reflected component
    // types, markers, include_paths resources. None at all means the
    // oracle's question cannot be judged; say so instead of passing
    // silently — and do NOT also run the stalled analysis on the
    // pseudo-counts (that would gate runs on entity bookkeeping).
    let has_game_channels = channels
        .iter()
        .any(|c| !matches!(c, Channel::EntityCount | Channel::ArchetypeCount));
    if has_complete_window && !has_game_channels {
        entries.push(ViolationEntry {
            rule: Rule::ProgressSurfaceEmpty.name().to_string(),
            target: "run".to_string(),
            first_frame: samples.first().map(|s| s.frame).unwrap_or(0),
            last_frame: samples.last().map(|s| s.frame).unwrap_or(0),
            count: 1,
            detail: format!(
                "no verifiable progress surface: {} matched the progress \
                 configuration (mode {:?}); register game state types \
                 (Reflect + register_type), or declare markers/include_paths \
                 naming the game's observable state",
                channel_summary(channels),
                config.mode
            ),
            last_detail: "progress surface empty — progress not verifiable".to_string(),
            meta: None,
        });
        let samples = samples
            .iter()
            .map(|s| ProgressSample {
                frame: s.frame,
                surface_hash: s.surface_hash,
                churn: s.churn,
                channels: s.values.len() as u32,
            })
            .collect();
        return ProgressAudit { entries, samples };
    }

    // Windows: group samples by floor(frame / window_frames). The
    // first sample of each window is its baseline — no transition is
    // counted across window boundaries (per-window recompute; no
    // cross-window state).
    let mut stalled: Vec<(&RawSample, Vec<String>)> = Vec::new(); // (first sample, tickers) of each stalled window
    for window in group_windows(samples, window_frames) {
        let Some(first) = window.first() else {
            continue;
        };
        // Only judge COMPLETE windows: a window [first.frame,
        // first.frame + window_frames) is complete iff the sample
        // series reaches its LAST frame (first.frame +
        // window_frames - 1) — boundary-consistent with
        // `has_complete_window` above. Judging a window the samples
        // fall short of would shrink the effective window and fire
        // early.
        if first.frame + window_frames - 1 > samples.last().expect("nonempty").frame {
            continue;
        }
        if window.len() < 2 {
            continue; // Degenerate (window shorter than the sampling gap).
        }
        let transitions = window.len() - 1;
        let churn_all = window.iter().all(|s| s.churn);
        if !churn_all {
            continue;
        }
        let mut progress = false;
        let mut tickers: Vec<&str> = Vec::new();
        for (idx, channel) in channels.iter().enumerate() {
            // Bookkeeping pseudo-channels never acquit a window
            // (module doctrine): entity/archetype-count churn is
            // engine/harness bookkeeping, not game-state evolution.
            if channel.is_bookkeeping() {
                continue;
            }
            let changed = window
                .windows(2)
                .filter(|pair| pair[0].values[idx] != pair[1].values[idx])
                .count() as u64;
            if changed == 0 {
                continue;
            }
            if changed * TICKER_FRACTION_DEN >= TICKER_FRACTION_NUM * transitions as u64 {
                tickers.push(channel.label());
            } else {
                progress = true;
            }
        }
        if !progress {
            let tickers: Vec<String> = tickers.iter().map(|s| s.to_string()).collect();
            stalled.push((first, tickers));
        }
    }

    if let Some(&(first_crossing, _)) = stalled.first() {
        let &(last_crossing, _) = stalled.last().expect("nonempty");
        // Ticker detail is the UNION over every stalled window: with
        // multiple disjoint stalled windows, the last window's tickers
        // alone would misdescribe the whole finding.
        let mut all_tickers: Vec<&str> = stalled
            .iter()
            .flat_map(|(_, ts)| ts.iter().map(|s| s.as_str()))
            .collect();
        all_tickers.sort_unstable();
        all_tickers.dedup();
        let ticker_note = if all_tickers.is_empty() {
            "none".to_string()
        } else {
            all_tickers.join(", ")
        };
        let windows_note = if stalled.len() == 1 {
            format!(
                "the whole {:.1}s window from frame {}",
                config.window_s, first_crossing.frame
            )
        } else {
            format!(
                "{} complete windows of {:.1}s, the first starting at frame \
                 {} and the last at frame {}",
                stalled.len(),
                config.window_s,
                first_crossing.frame,
                last_crossing.frame
            )
        };
        entries.push(ViolationEntry {
            rule: Rule::StalledProgress.name().to_string(),
            target: "world".to_string(),
            first_frame: first_crossing.frame,
            last_frame: last_crossing.frame,
            count: stalled.len() as u64,
            detail: format!(
                "churning-but-dead: the frame loop stayed alive (frozen_world \
                 never triggered) across {windows_note}, while no verifiable \
                 game state changed; ticker-suppressed (heartbeat) channels: \
                 {ticker_note}"
            ),
            last_detail: format!(
                "stalled progress in {} window{}; last window starts at frame \
                 {}; tickers suppressed: {ticker_note}",
                stalled.len(),
                if stalled.len() == 1 { "" } else { "s" },
                last_crossing.frame
            ),
            meta: None,
        });
    }

    let samples = samples
        .iter()
        .map(|s| ProgressSample {
            frame: s.frame,
            surface_hash: s.surface_hash,
            churn: s.churn,
            channels: s.values.len() as u32,
        })
        .collect();

    ProgressAudit { entries, samples }
}

/// Group consecutive samples into windows by frame index. Returns
/// slices into the sample vector.
fn group_windows<'a>(samples: &'a [RawSample], window_frames: u64) -> Vec<&'a [RawSample]> {
    if samples.is_empty() || window_frames == 0 {
        return Vec::new();
    }
    let mut windows: Vec<&'a [RawSample]> = Vec::new();
    let mut start = 0;
    let mut current = samples[0].frame / window_frames;
    for (i, s) in samples.iter().enumerate() {
        let w = s.frame / window_frames;
        if w != current {
            windows.push(&samples[start..i]);
            start = i;
            current = w;
        }
    }
    windows.push(&samples[start..]);
    windows
}

fn channel_summary(channels: &[Channel]) -> String {
    match channels.len() {
        0 => "no channels".to_string(),
        1 => format!("only '{}'", channels[0].label()),
        n => format!("{n} channels ('{}', ...)", channels[0].label()),
    }
}
