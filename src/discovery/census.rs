//! Load-time capability census (plug-and-play tier 0.5).
//!
//! Reports what a game's world EXPOSES — reflected gameplay types,
//! named entities, message channels, input maps, schedule
//! systems — as a read-only, in-process, milliseconds-cheap pass that
//! runs on every playtest (default-on) and prints a one-line summary
//! to stderr (full rows ride in the report artifact).
//!
//! Doctrine (mirrors [`super::static_scan`]):
//!
//! - **Census, never promotion.** The census reports what EXISTS.
//!   It never claims a game "is" a tier, never licenses skipping
//!   work, and every INACTIVE row carries a one-line remediation so
//!   the reading is actionable rather than discouraging.
//! - **Correspondence over canned verdicts.** A hardcoded scope
//!   report (the known anti-pattern this replaces) outputs the same
//!   thing regardless of world content; the census derives every row
//!   from live world state, and the negative-control test in
//!   `tests/census.rs` pins rich-vs-minimal correspondence for
//!   every capability.
//! - **Honest degradation.** Component/system names are MASKED in
//!   consumer graphs unless `bevy_ecs`'s `debug` feature is compiled
//!   in (same doctrine as [`super::static_scan`]'s driver filter):
//!   name-dependent rows report `Unknown` there with a one-line
//!   reason, never a possibly-wrong active/inactive verdict. Exact
//!   verdicts whenever names are readable.
//! - **Structural neutrality.** The census is a READ. It may
//!   force-initialize schedules (computing access sets is not
//!   running systems — the [`super::static_scan::capture_static_scan`]
//!   precedent), but must not spawn entities or add archetypes:
//!   state digests hash exactly those counts and a census that
//!   shifted them would diverge control from probe runs at frame 0.
//! - **Reflect precondition stated plainly.** A game that never
//!   calls `register_type` presents a nearly featureless world to
//!   reflection-based machinery (probes, digests); the census says
//!   so in its remediation text rather than implying the game is
//!   broken.
//!
//! Engine-vs-game split: types under `bevy_*`/`bevy::` are ENGINE
//! machinery (the same prefix filter the state digest uses to keep
//! wall-clock and pool data out of digests) and never count as game
//! capability. `App::new()` alone contributes the main schedules,
//! the engine `AppExit` channel and driver systems — an empty game
//! must read "no gameplay capability", not "some".
//!
//! Scope blind spots, stated up front (the census reports, never
//! gates, so a blind spot means UNDERCOUNTING in a row, never a
//! wrong verdict elsewhere):
//!
//! - Systems cached via `world.register_system(..)` but never added
//!   to a schedule are INVISIBLE — the census walks
//!   `Schedules` → `systems_with_access` only, not the
//!   system-registry entities. A game driving logic through cached
//!   function systems will read `game_systems` lower than its true
//!   count; the row's number is "scheduled game systems", and the
//!   remediation text says "add systems to a schedule".
//! - Schedules inside a SubApp's own world (e.g. a render sub-app)
//!   are invisible; see `game_systems`'s scope note.

use bevy_app::App;
use bevy_ecs::prelude::{Name, Schedules};
use bevy_ecs::reflect::AppTypeRegistry;
use bevy_ecs::system::SystemAccess;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The name bevy_ecs's `DebugName` prints when the `debug` feature is
/// off — the sentinel for "names are masked in this build" (see
/// module docs and [`super::static_scan`] for the masking doctrine).
pub(crate) const MASKED_NAME_SENTINEL: &str = "<Enable the debug feature to see the name>";

/// Engine driver systems present in every Bevy 0.20 app. Identified
/// by exact name (never a loose prefix), same doctrine and same list
/// as [`super::static_scan`]; masked names degrade to over-counting
/// (fail-open), which the census row reflects as `Unknown` verdicts
/// rather than a false INACTIVE.
pub(crate) const ENGINE_DRIVER_NAMES: &[&str] = &[
    "bevy_app::main_schedule::Main::run_main",
    "bevy_app::main_schedule::FixedMain::run_fixed_main",
    "bevy_ecs::message::update::message_update_system",
];

/// Type-path prefixes of engine machinery. Types under these never
/// count as game capability (state-digest filter precedent).
pub(crate) const ENGINE_TYPE_PREFIXES: &[&str] = &["bevy_", "bevy::"];

/// A capability the census reports on.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityKind {
    /// Reflected GAMEPLAY types (components/resources registered in
    /// the app's type registry, excluding engine `bevy_*` types).
    ReflectedGameplayTypes,
    /// Entities carrying a `Name` component.
    NamedEntities,
    /// Game-owned message channels (`Messages<T>` resources for
    /// non-engine `T`).
    GameMessageChannels,
    /// Game-owned systems (schedule systems that are not engine
    /// drivers).
    GameSystems,
    /// Declared input maps (Leafwing-style `InputMap<A>` reflected
    /// registrations).
    DeclaredInputMaps,
}

impl CapabilityKind {
    /// Stable, human-facing identifier (used in the report rows and
    /// the one-line summary).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReflectedGameplayTypes => "reflected_gameplay_types",
            Self::NamedEntities => "named_entities",
            Self::GameMessageChannels => "game_message_channels",
            Self::GameSystems => "game_systems",
            Self::DeclaredInputMaps => "declared_input_maps",
        }
    }
}

/// The census verdict for one capability.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStatus {
    /// The capability was detected as present.
    Active,
    /// The capability was detected as absent.
    Inactive,
    /// Name-dependent detection was impossible (masked names in a
    /// consumer graph without `bevy_ecs/debug`). Neither an Active
    /// nor an Inactive verdict would be honest.
    Unknown,
}

/// One census row: capability, verdict, evidence count, and — for
/// non-Active rows — a one-line remediation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilityRow {
    /// Which capability this row describes.
    pub kind: CapabilityKind,
    /// Active / Inactive / Unknown (see [`CapabilityStatus`]).
    pub status: CapabilityStatus,
    /// How many instances were seen (registered types, named
    /// entities, channels, systems, maps). Zero-information for
    /// `Unknown` rows.
    pub count: usize,
    /// One-line remediation for `Inactive` and `Unknown` rows;
    /// `None` on `Active`.
    pub remediation: Option<String>,
}

impl CapabilityRow {
    fn active(kind: CapabilityKind, count: usize) -> Self {
        Self {
            kind,
            status: CapabilityStatus::Active,
            count,
            remediation: None,
        }
    }

    fn inactive(kind: CapabilityKind, count: usize, remediation: impl Into<String>) -> Self {
        Self {
            kind,
            status: CapabilityStatus::Inactive,
            count,
            remediation: Some(remediation.into()),
        }
    }

    fn unknown(kind: CapabilityKind, remediation: impl Into<String>) -> Self {
        Self {
            kind,
            status: CapabilityStatus::Unknown,
            count: 0,
            remediation: Some(remediation.into()),
        }
    }
}

/// The census artifact: one row per tracked capability, plus
/// blind-spot notes (exclusive systems, never-initialized
/// schedules).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilityCensus {
    /// One row per [`CapabilityKind`] (order: enum order — stable).
    pub rows: Vec<CapabilityRow>,
    /// Blind-spot notes: exclusive-system count (attribution blind
    /// spot — reported as a count, never guessed at), schedules
    /// that never initialized (their systems are invisible).
    pub notes: Vec<String>,
}

impl CapabilityCensus {
    /// The row for one capability, if present.
    pub fn row(&self, kind: CapabilityKind) -> Option<&CapabilityRow> {
        self.rows.iter().find(|r| r.kind == kind)
    }

    /// Whether the census read this capability as active. `false`
    /// for `Unknown` (and for a missing row): callers must consult
    /// the row itself for the honest three-way verdict.
    pub fn is_active(&self, kind: CapabilityKind) -> bool {
        self.row(kind)
            .is_some_and(|r| r.status == CapabilityStatus::Active)
    }

    /// All rows (accessor for tests and report consumers).
    pub fn rows(&self) -> &[CapabilityRow] {
        &self.rows
    }

    /// The one-line summary printed on every flagless run (stderr
    /// stays near-silent; full rows ride in the report artifact).
    ///
    /// Format: `N/M capabilities active — kind: count; ... —
    /// inactive kinds and remediations in the report`. `M` counts
    /// only resolvable rows: `Unknown` rows are reported separately
    /// as "k masked" so a consumer-graph census never quietly
    /// undercounts.
    pub fn summary_line(&self) -> String {
        let mut parts = Vec::new();
        let mut resolved_active = 0usize;
        let mut resolved_total = 0usize;
        let mut masked = 0usize;
        for row in &self.rows {
            match row.status {
                CapabilityStatus::Active => {
                    resolved_active += 1;
                    resolved_total += 1;
                    parts.push(format!("{}: {}", row.kind.as_str(), row.count));
                }
                CapabilityStatus::Inactive => {
                    resolved_total += 1;
                }
                CapabilityStatus::Unknown => {
                    masked += 1;
                }
            }
        }
        if parts.is_empty() {
            parts.push("none detected".to_owned());
        }
        let mut line = format!(
            "bevy_playtest: capability census: {resolved_active}/{resolved_total} active ({})",
            parts.join("; ")
        );
        if masked > 0 {
            line.push_str(&format!(
                "; {masked} masked (enable bevy_ecs/debug for verdicts)"
            ));
        }
        line
    }
}

/// `true` for engine type paths (`bevy_*` / `bevy::`) — the digest's
/// established exclusion filter, reused so "gameplay capability"
/// never counts engine machinery. `pub(crate)`: the contract
/// suggestions walk shares the exact same filter so census counts
/// and suggestion targets can never disagree about ownership.
pub(crate) fn is_engine_type(path: &str) -> bool {
    ENGINE_TYPE_PREFIXES.iter().any(|p| path.starts_with(p))
}

/// Count reflected GAMEPLAY registrations in the app's type registry:
/// registered types carrying `ReflectComponent` or `ReflectResource`
/// data (the registration shape probes and state digests consume)
/// whose path is not engine machinery. Raw path counting is WRONG
/// here: `App::new()` with bevy's `reflect_auto_register` feature
/// links hundreds of plain scalar types (`glam::*`, std
/// collections) into the registry — none are `bevy_*`-prefixed,
/// none are gameplay. The component/resource registration filter
/// plus the engine prefix makes the count mean "gameplay types the
/// harness can actually perceive". The registry may legitimately be
/// absent (hand-rolled `World`s); the census then reports zero
/// honestly.
fn reflected_gameplay_types(app: &App) -> usize {
    let Some(registry) = app.world().get_resource::<AppTypeRegistry>() else {
        return 0;
    };
    let registry = registry.0.read();
    let found: Vec<&str> = registry
        .iter()
        .filter(|t| {
            let path = t.type_info().type_path();
            !is_engine_type(path)
                && (t.data::<bevy_ecs::reflect::ReflectComponent>().is_some()
                    || t.data::<bevy_ecs::reflect::ReflectResource>().is_some())
        })
        .map(|t| t.type_info().type_path())
        .collect();
    found.len()
}

/// Count entities carrying a `Name` component. Name is a plain
/// component id lookup, no names needed.
fn named_entity_count(app: &App) -> usize {
    let world = app.world();
    let Some(name_id) = world.components().component_id::<Name>() else {
        return 0;
    };
    world
        .iter_entities()
        .filter(|e| e.contains_id(name_id))
        .count()
}

/// Type-path prefix of the engine's per-message-type storage
/// resource (verified spelling in bevy_ecs 0.20.1:
/// `bevy_ecs::message::messages::Messages<...>`). `pub(crate)`: the
/// contract suggestions walk reuses the exact spelling so channel
/// classification cannot drift between the two modules.
pub(crate) const MESSAGES_RESOURCE_PREFIX: &str = "bevy_ecs::message::messages::Messages<";

/// Count game-owned message channels: `Messages<T>` resources whose
/// `T` is not engine machinery. `MessageRegistry.message_updates`
/// (which would enumerate channels directly) is PRIVATE in 0.20.1,
/// so the census walks all resource entities instead and
/// inspects their [`ComponentInfo`] names — which requires
/// readable names; masked builds get an `Unknown` row.
fn game_message_channels(app: &App) -> Option<usize> {
    let world = app.world();
    let mut count = 0usize;
    // `ResourceEntities::iter` scans the whole component-id array
    // (documented as potentially slow) — bounded by registered
    // component ids, fine for a load-time census.
    for (id, _entity) in world.resource_entities().iter() {
        let Some(info) = world.components().get_info(id) else {
            continue;
        };
        let name: &str = &info.name();
        if name == MASKED_NAME_SENTINEL {
            // Names masked: cannot classify this resource at all.
            return None;
        }
        // The ENGINE PREFIX must be applied to the message type
        // INSIDE the brackets, never to the whole resource name —
        // every `Messages<T>` resource path starts with
        // `bevy_ecs::...` regardless of who owns the channel.
        if let Some(inner) = name.strip_prefix(MESSAGES_RESOURCE_PREFIX) {
            let inner = inner.strip_suffix('>').unwrap_or(inner);
            if !is_engine_type(inner) {
                count += 1;
            }
        }
    }
    Some(count)
}

/// Force-initialize every schedule WITHOUT running it: a lazily
/// built schedule otherwise returns `ScheduleNotInitialized` from
/// `systems_with_access`, hiding its systems from the census.
///
/// Digest-neutral mechanics copied verbatim from
/// [`super::static_scan::capture_static_scan`] (which proved them
/// out in review): the `Schedules` resource stays resident — a
/// REMOVED resource strips its component and lands the entity in
/// a fresh archetype, growing the archetype count this census must
/// not touch. Each schedule is temporarily owned inside the
/// resource's map instead (`remove_temporarily` + `reinsert`), and
/// initialization failures surface as honest notes.
///
/// Timing note: force-initializing a changed graph triggers
/// `On<ScheduleBuilt>` observers (bevy_ecs fires the event during
/// `Schedule::initialize`). Because the census is default-on, a
/// game observer of `ScheduleBuilt` now fires during census setup
/// rather than at the first `app.update()` — earlier, but
/// symmetric across every run of the same scenario, so digests
/// and verdicts are unaffected.
fn force_initialize_schedules(app: &mut App, notes: &mut Vec<String>) {
    let labels: Vec<_> = app
        .world()
        .get_resource::<Schedules>()
        .map(|s| s.iter().map(|(_, sch)| sch.label()).collect())
        .unwrap_or_default();
    // Failure notes join `notes` SORTED: the label walk iterates
    // `Schedules` (a HashMap, undefined order), so an unsorted
    // multi-failure run would emit process-nondeterministic note
    // ordering inside an otherwise byte-stable report.
    let mut failures: Vec<String> = Vec::new();
    for label in &labels {
        let Some(mut schedule) = app
            .world_mut()
            .get_resource_mut::<Schedules>()
            .and_then(|mut s| s.remove_temporarily(*label))
        else {
            continue;
        };
        if let Err(err) = schedule.initialize(app.world_mut()) {
            // Same rich formatting as the scan capture: name the
            // systems/edges that broke the build, not just the
            // bare Display string.
            failures.push(format!(
                "schedule {label:?} failed to initialize: {}",
                err.to_string(schedule.graph(), app.world_mut())
            ));
        }
        if let Some(mut schedules) = app.world_mut().get_resource_mut::<Schedules>() {
            schedules.reinsert(schedule);
        }
    }
    failures.sort();
    notes.extend(failures);
}

/// Count game-owned systems across every schedule, plus the
/// exclusive-system blind-spot count. Needs readable system names
/// (driver filtering); masked builds get an `Unknown` row. Must run
/// AFTER [`force_initialize_schedules`] — schedules never built hide
/// their systems from `systems_with_access`.
///
/// Scope: this walk (like the whole census) reads the MAIN world's
/// `Schedules` resource only — schedules inside a SubApp's own world
/// (e.g. a render sub-app) are invisible. Fine for a census that
/// reports, never gates; noted so future consumers do not read the
/// counts as exhaustive for the whole `App`.
fn game_systems(app: &App) -> Option<(usize, usize)> {
    let world = app.world();
    let Some(schedules) = world.get_resource::<Schedules>() else {
        // No schedules at all: no systems to count (not masked).
        return Some((0, 0));
    };
    let mut count = 0usize;
    let mut exclusive = 0usize;
    for (_label, schedule) in schedules.iter() {
        let Ok(systems) = schedule.systems_with_access() else {
            // Still uninitialized (initialization failure above):
            // its systems are invisible to this read.
            continue;
        };
        for (_key, swa) in systems {
            let name: &str = &swa.system().name();
            if name == MASKED_NAME_SENTINEL {
                return None;
            }
            if ENGINE_DRIVER_NAMES.contains(&name) || is_engine_type(name) {
                // Engine drivers (exact names) and engine-owned
                // systems generally (`bevy_*` prefixes — e.g.
                // `despawn_unused_registered_systems`, which
                // `App::new()` itself adds to `Last`). Same
                // documented trade-off as the digest filter: a game
                // crate whose path starts with `bevy_` would be
                // excluded too.
                continue;
            }
            if matches!(swa.access(), SystemAccess::Exclusive) {
                exclusive += 1;
            }
            count += 1;
        }
    }
    Some((count, exclusive))
}

/// Count declared input maps: `InputMap<A>` registrations in the type
/// registry (the same reflected-component shape the `lw-reader`
/// detects; a registry-prefix read works without the feature).
fn declared_input_maps(app: &App) -> usize {
    const INPUT_MAP_PREFIX: &str = "leafwing_input_manager::input_map::InputMap";
    let Some(registry) = app.world().get_resource::<AppTypeRegistry>() else {
        return 0;
    };
    let registry = registry.0.read();
    registry
        .iter()
        .filter(|t| t.type_info().type_path().starts_with(INPUT_MAP_PREFIX))
        .count()
}

/// Run the capability census against a booted app.
///
/// Pure read (plus schedule force-initialization, the documented
/// [`super::static_scan::capture_static_scan`] precedent — computing
/// access sets is not running systems): no entities spawned, no
/// archetypes added, no resources inserted. Safe to call on any App
/// that has finished plugin building; the census tolerates minimal
/// and hand-rolled worlds.
///
/// ORDERING CONSTRAINT: `[crate::driver::run_scenario]` invokes this
/// BELOW the env-gated scan capture and before bot/oracle
/// registration. Running it ABOVE the capture is FORBIDDEN:
/// force-initialization BUILDS a changed schedule graph, which
/// moves run conditions into executor storage outside `graph()`
/// (`pub(super)` in bevy_ecs 0.20.1) and would empty both of the
/// capture's condition harvests in scan-exporting runs. Seeded
/// harness input plumbing cannot inflate the counts: every seeded
/// type is engine-owned (`bevy_*`) and excluded by the census's
/// ownership filter.
pub fn run_capability_census(app: &mut App) -> CapabilityCensus {
    let mut census = CapabilityCensus::default();

    // Name-independent capabilities first: always exact verdicts.
    let reflected = reflected_gameplay_types(app);
    census.rows.push(if reflected > 0 {
        CapabilityRow::active(CapabilityKind::ReflectedGameplayTypes, reflected)
    } else {
        CapabilityRow::inactive(
            CapabilityKind::ReflectedGameplayTypes,
            0,
            "no reflected gameplay types found — register yours with \
             #[derive(Reflect)] + #[reflect(Component/Resource)] and \
             app.register_type::<T>() to unlock percepts and probes",
        )
    });

    let named = named_entity_count(app);
    census.rows.push(if named > 0 {
        CapabilityRow::active(CapabilityKind::NamedEntities, named)
    } else {
        CapabilityRow::inactive(
            CapabilityKind::NamedEntities,
            0,
            "no named entities found — spawn entities with bevy_ecs::name::Name \
             to make findings easier to attribute",
        )
    });

    let maps = declared_input_maps(app);
    census.rows.push(if maps > 0 {
        CapabilityRow::active(CapabilityKind::DeclaredInputMaps, maps)
    } else {
        CapabilityRow::inactive(
            CapabilityKind::DeclaredInputMaps,
            0,
            "no declared input maps found — this game drives via raw input \
             reads, or uses an unmapped input abstraction",
        )
    });

    // Name-dependent capabilities: force-initialize schedules first
    // so `systems_with_access` can see their systems (the digest-
    // neutral static_scan precedent; see
    // [`force_initialize_schedules`]). Masked builds then get
    // honest `Unknown` rows (see module docs).
    force_initialize_schedules(app, &mut census.notes);

    census.rows.push(match game_message_channels(app) {
        Some(0) => CapabilityRow::inactive(
            CapabilityKind::GameMessageChannels,
            0,
            "no game-owned message channels found — open one with \
             app.add_message::<T>() so writers/readers can meet",
        ),
        Some(channels) => CapabilityRow::active(CapabilityKind::GameMessageChannels, channels),
        None => CapabilityRow::unknown(
            CapabilityKind::GameMessageChannels,
            "component names are masked in this build — enable bevy_ecs's \
             debug feature to classify message channels",
        ),
    });

    census.rows.push(match game_systems(app) {
        Some((systems, exclusive)) => {
            if exclusive > 0 {
                census.notes.push(format!(
                    "{exclusive} exclusive system(s) present: an exclusive \
                     system may touch anything, so per-capability attribution \
                     is a blind spot there (counted, never guessed)"
                ));
            }
            if systems > 0 {
                CapabilityRow::active(CapabilityKind::GameSystems, systems)
            } else {
                CapabilityRow::inactive(
                    CapabilityKind::GameSystems,
                    0,
                    "no game-owned systems found in initialized schedules — \
                     add systems to a schedule so the census can see them",
                )
            }
        }
        None => CapabilityRow::unknown(
            CapabilityKind::GameSystems,
            "system names are masked in this build — enable bevy_ecs's \
             debug feature to separate game systems from engine drivers",
        ),
    });

    census
}

/// Count distinct type paths registered in the app's type registry
/// (including engine types) — used by tests and callers needing the
/// registry's raw size.
#[allow(dead_code)]
pub(crate) fn registered_type_paths(app: &App) -> BTreeSet<String> {
    let mut paths = BTreeSet::new();
    if let Some(registry) = app.world().get_resource::<AppTypeRegistry>() {
        let registry = registry.0.read();
        for t in registry.iter() {
            paths.insert(t.type_info().type_path().to_owned());
        }
    }
    paths
}
