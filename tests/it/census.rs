//! Load-time capability census: the negative-control test suite.
//!
//! The controlling anti-pattern is a HARDCODED scope report: a
//! capability summary that outputs a canned verdict regardless of
//! what the world actually contains. Such a report is worse than no
//! report — it looks authoritative while carrying zero information
//! about the game under test.
//!
//! The negative control here asserts CORRESPONDENCE, not just
//! inequality: for every capability the census reports, a world that
//! demonstrably HAS the capability must read ACTIVE, and a world
//! that demonstrably LACKS it must read INACTIVE. A hardcoded census
//! fails at least one axis of this matrix by construction.
//!
//! Worlds are synthetic rather than fixture binaries on purpose: the
//! census is a pure in-process library function, and synthetic
//! worlds give exact per-capability control (a fixture game has a
//! fixed, broad capability set — it cannot isolate one capability).
//! The fixture battery covers the census on real games at its tier.

use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_ecs::reflect::AppTypeRegistry;
use bevy_playtest::discovery::census::{run_capability_census, CapabilityKind, CapabilityStatus};
use bevy_reflect::Reflect;

/// A gameplay component type the rich world registers.
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct Health {
    hp: f32,
}

/// A gameplay resource type the rich world registers.
#[derive(Resource, Reflect, Default)]
#[reflect(Resource)]
struct Score {
    value: u32,
}

/// A game-owned message channel the rich world opens.
#[derive(Message)]
struct PlayerDied;

fn score_updater(mut score: ResMut<Score>) {
    score.value += 1;
}

/// The MINIMAL world: an `App::new()` with nothing added. It still
/// contains engine machinery (the main schedules, the `AppExit`
/// message channel, engine driver systems) — the census must not
/// mistake any of that for game capability.
///
/// One test-graph adjustment: this binary links bevy's
/// `reflect_auto_register`, which registers every derived `Reflect`
/// type IN THIS TEST BINARY (including the rich world's `Health`
/// and `Score`!) into the registry of ANY `App::new()` here. A
/// consumer graph without that feature gets an empty registry. Swap
/// in an explicitly empty registry so the control mirrors consumer
/// reality instead of test-binary contamination.
fn minimal_world() -> App {
    let mut app = App::new();
    app.world_mut()
        .insert_resource(bevy_ecs::reflect::AppTypeRegistry::default());
    app
}

/// The RICH world: one reflected gameplay component, one reflected
/// gameplay resource, one named entity, one game message channel,
/// and one game system — each chosen to flip exactly one census
/// capability.
fn rich_world() -> App {
    let mut app = App::new();
    // Register gameplay types directly in the AppTypeRegistry: the
    // harness's bevy_app dep runs without the reflect feature in
    // consumer graphs, so `App::register_type` is not portable
    // across feature resolutions — the registry is.
    let registry = app.world_mut().get_resource_or_init::<AppTypeRegistry>();
    registry.write().register::<Health>();
    registry.write().register::<Score>();
    app.add_message::<PlayerDied>();
    app.world_mut()
        .spawn((bevy_ecs::name::Name::new("Player"), Health { hp: 100.0 }));
    app.add_systems(Update, score_updater);
    app
}

/// THE negative control (see module docs). Correspondence matrix:
/// every capability the rich world has must read active there and
/// inactive in the minimal world. Both halves matter — a census that
/// hardcodes "everything active" fails the minimal side; one that
/// hardcodes "nothing detected" fails the rich side; one that
/// hardcodes a canned mix fails whichever cells it guessed wrong.
#[test]
fn census_corresponds_to_actual_world_content() {
    let minimal = run_capability_census(&mut minimal_world());
    let rich = run_capability_census(&mut rich_world());

    // Reflected gameplay types: Health + Score in the rich world;
    // the minimal world has only engine machinery (any `bevy_*` /
    // `bevy::` registrations the engine itself makes are engine
    // types, not gameplay types).
    assert!(
        !minimal.is_active(CapabilityKind::ReflectedGameplayTypes),
        "minimal world must report reflected gameplay types INACTIVE, got: {minimal:#?}"
    );
    assert!(
        rich.is_active(CapabilityKind::ReflectedGameplayTypes),
        "rich world registers Health + Score, must report reflected gameplay types ACTIVE, got: {rich:#?}"
    );

    // Named entities: one spawned entity carries Name in the rich
    // world; the minimal world has none.
    assert!(
        !minimal.is_active(CapabilityKind::NamedEntities),
        "minimal world must report named entities INACTIVE, got: {minimal:#?}"
    );
    assert!(
        rich.is_active(CapabilityKind::NamedEntities),
        "rich world spawns a Name entity, must report named entities ACTIVE, got: {rich:#?}"
    );

    // Game-owned message channels: PlayerDied in the rich world; the
    // minimal world's AppExit channel is ENGINE machinery.
    assert!(
        !minimal.is_active(CapabilityKind::GameMessageChannels),
        "minimal world must report game message channels INACTIVE (AppExit is engine), got: {minimal:#?}"
    );
    assert!(
        rich.is_active(CapabilityKind::GameMessageChannels),
        "rich world opens PlayerDied, must report game message channels ACTIVE, got: {rich:#?}"
    );

    // Game systems: score_updater in the rich world; the minimal
    // world's driver systems are engine machinery.
    assert!(
        !minimal.is_active(CapabilityKind::GameSystems),
        "minimal world must report game systems INACTIVE (drivers are engine), got: {minimal:#?}"
    );
    assert!(
        rich.is_active(CapabilityKind::GameSystems),
        "rich world adds score_updater, must report game systems ACTIVE, got: {rich:#?}"
    );

    // Neither world declares input maps, so this capability must
    // read INACTIVE in BOTH — a hardcoded census that canned this
    // cell "active" would fail here even if it guessed the rows
    // above right.
    assert!(
        !minimal.is_active(CapabilityKind::DeclaredInputMaps),
        "minimal world must report declared input maps INACTIVE, got: {minimal:#?}"
    );
    assert!(
        !rich.is_active(CapabilityKind::DeclaredInputMaps),
        "rich world declares no input maps, must report INACTIVE, got: {rich:#?}"
    );

    // And the summaries must measurably differ — the belt to the
    // correspondence braces.
    assert_ne!(
        minimal.summary_line(),
        rich.summary_line(),
        "census of a minimal world must differ from a rich world"
    );
}

/// Inactive rows must state a one-line remediation — the census
/// never just says "no"; it says how to get to yes. This is what
/// makes an INACTIVE reading actionable instead of discouraging.
#[test]
fn inactive_rows_state_remediation() {
    let minimal = run_capability_census(&mut minimal_world());
    for row in minimal.rows() {
        if row.status != CapabilityStatus::Active {
            assert!(
                row.remediation.as_deref().is_some_and(|r| !r.is_empty()),
                "inactive row {row:?} must state a remediation"
            );
        }
    }
}

/// The one-line summary the driver prints to stderr on every
/// flagless run has a documented shape; pin it so a refactor can't
/// silently drop or mutate the advertised line.
#[test]
fn summary_line_matches_the_documented_stderr_shape() {
    let minimal = run_capability_census(&mut minimal_world());
    let line = minimal.summary_line();
    assert!(
        line.starts_with("bevy_playtest: capability census: "),
        "the stderr line must lead with the harness prefix, got: {line}"
    );
    assert!(
        line.contains(" active"),
        "the stderr line must report the active/total ratio, got: {line}"
    );
}

/// `force_initialize_schedules` walks `Schedules` (a HashMap, whose
/// iteration order is undefined across processes), so a run where
/// MULTIPLE schedules fail to initialize must still attach failure
/// notes in a deterministic (sorted) order — the census's
/// byte-stability story extends to its notes, not just its rows.
#[test]
fn multi_schedule_failure_notes_arrive_sorted() {
    use bevy_ecs::prelude::*;

    #[derive(bevy_ecs::schedule::ScheduleLabel, Hash, PartialEq, Eq, Debug, Clone)]
    struct BadFirst;

    #[derive(bevy_ecs::schedule::ScheduleLabel, Hash, PartialEq, Eq, Debug, Clone)]
    struct BadSecond;

    fn first() {}
    fn second() {}

    // Two custom schedules, each with a dependency cycle: both
    // fail to build, producing one failure note each.
    let mut app = App::new();
    app.add_systems(BadFirst, (first.before(second), second.before(first)));
    app.add_systems(BadSecond, (second.before(first), first.before(second)));
    let census = run_capability_census(&mut app);
    let failures: Vec<&String> = census
        .notes
        .iter()
        .filter(|n| n.contains("failed to initialize"))
        .collect();
    assert!(
        failures.len() >= 2,
        "both cyclic schedules must report failure notes, got: {census:#?}"
    );
    assert!(
        failures.windows(2).all(|w| w[0] <= w[1]),
        "failure notes must be sorted for cross-process determinism, got: {failures:?}"
    );
}

/// Structural neutrality: the census is a READ. It may force-init
/// schedules (computing access sets is not running systems), but it
/// must not spawn entities or add archetypes — per-frame state
/// digests hash exactly those counts, and a census that shifted them
/// would diverge control from probe runs at frame 0.
#[test]
fn census_does_not_touch_world_structure() {
    let mut app = rich_world();
    let spawned_before = app.world().entities().count_spawned();
    let archetypes_before = app.world().archetypes().len();
    let _ = run_capability_census(&mut app);
    assert_eq!(
        app.world().entities().count_spawned(),
        spawned_before,
        "census must not change the spawned-entity count"
    );
    assert_eq!(
        app.world().archetypes().len(),
        archetypes_before,
        "census must not change the archetype count"
    );
}
