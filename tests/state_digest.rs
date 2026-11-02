//! Integration tests for the broadened state digest.
//!
//! These tests verify that reflected game resources are captured by the digest,
//! which cannot be tested from unit tests in the `bevy_playtest` crate due to
//! the engine-resource filter (`bevy_*` types are excluded, and the test type
//! path `bevy_playtest::...` would collide).
//!
//! This crate (`state_digest`) does NOT start with `bevy_`, so reflection tests
//! run correctly here.

use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;

#[derive(Resource, Debug, Reflect)]
#[reflect(Resource)]
struct GameScore(u32);

#[derive(Resource, Debug, Reflect)]
#[reflect(Resource)]
struct Health(u32);

fn mk_base_world() -> World {
    let mut world = World::new();
    world.init_resource::<bevy_ecs::reflect::AppTypeRegistry>();
    world.insert_resource(GameScore(0));
    world.insert_resource(Health(100));
    {
        let registry = world.resource_mut::<bevy_ecs::reflect::AppTypeRegistry>();
        let mut reg = registry.0.write();
        reg.register::<GameScore>();
        reg.register::<Health>();
    }
    world
}

#[test]
fn reflected_game_resource_value_changes_affect_digest() {
    let mut world = mk_base_world();
    world.resource_mut::<GameScore>().0 = 100;
    let digest_before = bevy_playtest::digest::compute_digest(&mut world);

    // In-place value change: structure constant, reflected value changed.
    world.resource_mut::<GameScore>().0 = 200;

    let digest_after = bevy_playtest::digest::compute_digest(&mut world);
    assert_ne!(
        digest_before, digest_after,
        "in-place reflected game resource value changes must change the digest"
    );
}

#[test]
fn same_resources_same_digest_independent_of_insertion_order() {
    let mut world_a = mk_base_world();
    world_a.resource_mut::<GameScore>().0 = 1;
    world_a.resource_mut::<Health>().0 = 2;

    let mut world_b = mk_base_world();
    world_b.resource_mut::<Health>().0 = 2;
    world_b.resource_mut::<GameScore>().0 = 1;

    let digest_a = bevy_playtest::digest::compute_digest(&mut world_a);
    let digest_b = bevy_playtest::digest::compute_digest(&mut world_b);
    assert_eq!(
        digest_a, digest_b,
        "digest must be independent of resource insertion order (canonical sorting)"
    );
}

#[test]
fn engine_resources_are_excluded_from_digest() {
    let mut world = World::new();
    world.init_resource::<bevy_ecs::reflect::AppTypeRegistry>();
    world.insert_resource(bevy_time::Time::<bevy_time::Virtual>::default());
    // Register the engine resource as reflected: without exclusion, its
    // (non-deterministic) value would leak into the digest.
    {
        let registry = world.resource::<bevy_ecs::reflect::AppTypeRegistry>();
        registry
            .0
            .write()
            .register::<bevy_time::Time<bevy_time::Virtual>>();
    }
    let digest_before = bevy_playtest::digest::compute_digest(&mut world);

    world
        .resource_mut::<bevy_time::Time<bevy_time::Virtual>>()
        .advance_by(std::time::Duration::from_secs(1));

    let digest_after = bevy_playtest::digest::compute_digest(&mut world);
    assert_eq!(
        digest_before, digest_after,
        "engine resource value changes must not leak into the digest"
    );
}

#[test]
fn resource_exclusion_predicate_works() {
    use bevy_playtest::digest::resource_is_excluded;

    // Engine types excluded.
    assert!(resource_is_excluded("bevy_time::Time", "Time"));
    assert!(resource_is_excluded("bevy::time::Time", "Time"));
    assert!(resource_is_excluded("bevy_window::Window", "Window"));

    // Harness types excluded by `bevy_playtest::` full-path prefix.
    assert!(resource_is_excluded(
        "bevy_playtest::driver::RunState",
        "RunState"
    ));

    // Game types NOT excluded — even when sharing a short name with a
    // harness type (the old short-path exclusion would drop these).
    assert!(!resource_is_excluded("game::RunState", "RunState"));
    assert!(!resource_is_excluded(
        "some_crate::RunViolations",
        "RunViolations"
    ));
    assert!(!resource_is_excluded("game::Score", "Score"));
    assert!(!resource_is_excluded(
        "state_digest::GameScore",
        "GameScore"
    ));
}

#[test]
fn snapshot_captures_resource_values() {
    let mut world = mk_base_world();
    world.resource_mut::<GameScore>().0 = 42;

    let snapshot = bevy_playtest::digest::build_snapshot(&mut world);

    // Verify GameScore appears in the snapshot keyed by FULL type path
    // (distinct resources can share a short name).
    let key = "state_digest::GameScore";
    let score_entry = snapshot.resources.get(key);
    assert!(
        score_entry.is_some(),
        "GameScore must be captured in the snapshot; keys: {:?}",
        snapshot.resources.keys().collect::<Vec<_>>()
    );
    assert!(
        score_entry.unwrap().contains("42"),
        "captured GameScore must show updated value; got {:?}",
        snapshot.resources.get(key)
    );
}

/// Component-type counts are an advertised digest signal (snapshot field
/// `marker_counts`). Swapping one reflected marker for a DIFFERENT one
/// keeps entity count AND archetype count constant (still one marker
/// archetype holding one entity) while changing the counted type PATH
/// — this isolates `collect_component_type_counts` from the coarser
/// entity/archetype signals. A regression in the collector (e.g.
/// failing to notice a marker) must flip these assertions.
#[derive(Component, Debug, Reflect)]
#[reflect(Component)]
struct Powerup;

#[derive(Component, Debug, Reflect)]
#[reflect(Component)]
struct Cursed;

#[test]
fn reflected_marker_type_swap_changes_digest_at_constant_structure() {
    let mut world = mk_base_world();
    {
        let registry = world.resource_mut::<bevy_ecs::reflect::AppTypeRegistry>();
        let mut reg = registry.0.write();
        reg.register::<Powerup>();
        reg.register::<Cursed>();
    }
    let e = world.spawn(Powerup).id();

    // Direct signal check: the snapshot's marker_counts must attribute
    // the entity to the `Powerup` type by FULL path.
    let snapshot_before = bevy_playtest::digest::build_snapshot(&mut world);
    assert_eq!(
        snapshot_before.marker_counts.get("state_digest::Powerup"),
        Some(&1),
        "marker_counts must count the reflected marker by full type path; got {:?}",
        snapshot_before.marker_counts
    );

    // Swap: {Powerup} -> {Cursed}; entity count and archetype count are
    // unchanged, only the component-type PATH set changes.
    world.entity_mut(e).remove::<Powerup>();
    world.entity_mut(e).insert(Cursed);

    let snapshot_after = bevy_playtest::digest::build_snapshot(&mut world);
    assert_eq!(
        snapshot_after.marker_counts.get("state_digest::Powerup"),
        Some(&0),
        "removed marker must no longer count entities"
    );
    assert_eq!(
        snapshot_after.marker_counts.get("state_digest::Cursed"),
        Some(&1),
        "inserted marker must be counted by full type path; got {:?}",
        snapshot_after.marker_counts
    );

    // End to end: the whole digest must notice the swap.
    let digest_swap_a = bevy_playtest::digest::compute_digest(&mut world);
    world.entity_mut(e).remove::<Cursed>();
    world.entity_mut(e).insert(Powerup);
    let digest_swap_b = bevy_playtest::digest::compute_digest(&mut world);
    assert_ne!(
        digest_swap_a, digest_swap_b,
        "marker-type swap at constant entity/archetype counts must change the digest"
    );
}

/// Distinct NaN payloads must NOT collapse in the digest: `Debug`
/// formats every NaN identically, but the digest promises bit-exact
/// float hashing. Two worlds whose reflected resource holds different
/// NaN bit patterns (same length, same type) must digests differ.
#[test]
fn nan_payload_divergence_changes_digest() {
    #[derive(Resource, Debug, Reflect)]
    #[reflect(Resource)]
    struct Sensor(f32);

    fn world_with(payload_bits: u32) -> World {
        let mut world = World::new();
        world.init_resource::<bevy_ecs::reflect::AppTypeRegistry>();
        {
            let registry = world.resource::<bevy_ecs::reflect::AppTypeRegistry>();
            registry.0.write().register::<Sensor>();
        }
        world.insert_resource(Sensor(f32::from_bits(payload_bits)));
        world
    }

    let digest_a = bevy_playtest::digest::compute_digest(&mut world_with(0x7FC0_0000));
    let digest_b = bevy_playtest::digest::compute_digest(&mut world_with(0xFFC0_0000));
    assert_ne!(
        digest_a, digest_b,
        "distinct NaN payloads must produce different digests"
    );

    // -0.0 vs 0.0 also collapse under Debug; bit-exactness must catch it.
    let digest_neg = bevy_playtest::digest::compute_digest(&mut world_with(0x8000_0000)); // -0.0
    let digest_pos = bevy_playtest::digest::compute_digest(&mut world_with(0x0000_0000)); // 0.0
    assert_ne!(
        digest_neg, digest_pos,
        "-0.0 vs 0.0 must produce different digests"
    );
}

#[test]
fn ui_transform_changes_affect_digest_when_feature_enabled() {
    #[cfg(feature = "ui-liveness")]
    {
        use bevy_ui::prelude::*;

        let mut world = World::new();
        world.spawn(UiTransform::default());
        let digest_before = bevy_playtest::digest::compute_digest(&mut world);
        let e = world.spawn(UiTransform::default()).id();
        let digest_after = bevy_playtest::digest::compute_digest(&mut world);
        assert_ne!(
            digest_before, digest_after,
            "UI entity count must affect digest"
        );

        // Modify UiTransform translation (Px).
        world.entity_mut(e).insert(UiTransform {
            translation: bevy_ui::Val2::px(100.0, 200.0),
            ..Default::default()
        });
        let digest_changed = bevy_playtest::digest::compute_digest(&mut world);
        assert_ne!(
            digest_after, digest_changed,
            "UiTransform change must affect digest"
        );
    }

    #[cfg(not(feature = "ui-liveness"))]
    {
        // Feature disabled: UiTransform is not part of the digest. A plain
        // world must still produce a stable digest.
        let mut world = World::new();
        let digest_a = bevy_playtest::digest::compute_digest(&mut world);
        let digest_b = bevy_playtest::digest::compute_digest(&mut world);
        assert_eq!(digest_a, digest_b, "digest must remain stable");
    }
}
