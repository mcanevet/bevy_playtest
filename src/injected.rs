//! Harness-injected entity marking: census-neutrality for driver-
//! spawned probe entities.
//!
//! The screen-liveness capture rig (`screen-liveness` feature) is
//! the first harness component that spawns MAIN-WORLD entities
//! DURING a run: a persistent probe camera (aimed at an offscreen
//! render target) and one short-lived `Screenshot` request entity
//! per capture. Those entities are harness bookkeeping, not game
//! state — and two enumerations would misread them:
//!
//! - the state digest / replay fingerprint / progress breadth
//!   channels count entities, archetypes, and `Transform`s; probe
//!   churn there would masquerade as simulation activity and mask a
//!   genuine soft-lock;
//! - `render_liveness` judges "the GAME has a camera" — the probe
//!   camera must not satisfy it.
//!
//! The fix is one marker, [`HarnessInjected`], attached to every
//! entity the driver spawns into the game world mid-run, plus
//! [`is_injected`] (per-entity test) and the enumeration-side
//! helpers below. The marker module is
//! ALWAYS compiled (the enumerations are), so the exclusion holds
//! even in builds without the `screen-liveness` feature: no entity
//! carries the marker there, the predicate answers `false` for all,
//! and behavior is unchanged (the screens-liveness rig is absent).
//!
//! Bevy prunes empty archetypes lazily, so spawned/despawned probe
//! entities shift the ARCHETYPE COUNT too — the predicate therefore
//! also powers an archetype-count adjustment (see
//! [`archetype_excess`]) that subtracts probe-only archetypes from
//! raw breadth counts.

use bevy_ecs::prelude::*;

/// Marker for entities the harness spawns into the game's world
/// DURING a run (probe camera, capture-request entities). Carries no
/// data and reflects nothing; its entire job is to keep harness
/// bookkeeping out of every game-state enumeration.
#[derive(Component, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct HarnessInjected;

/// Whether `entity` is harness-injected (and therefore must not
/// count as game state in any enumeration).
pub fn is_injected(entity: Entity, world: &World) -> bool {
    world.get::<HarnessInjected>(entity).is_some()
}

/// How many entities in `world` carry [`HarnessInjected`] — useful
/// for asserting probe hygiene in tests (and a building block for
/// count adjustments). O(entities); called rarely.
///
/// Registration-safe: `QueryState::try_new` returns `None` when the
/// marker is not registered in this world — in which case NO entity
/// can carry it and the count is honestly 0.
pub fn injected_count(world: &World) -> usize {
    QueryState::<(), With<HarnessInjected>>::try_new(world)
        .map(|mut state| state.iter(world).count())
        .unwrap_or(0)
}

/// How many DISTINCT archetypes are probe-owned: their layout
/// contains the [`HarnessInjected`] marker component. Breadth
/// counters that hash or export a raw `world.archetypes().len()` add
/// this count's negation so probe entities cannot shift it
/// (spawn/despawn churn of probe archetypes would otherwise flag
/// as divergence or fake progress).
///
/// MEMBERSHIP, NOT OCCUPANCY: bevy never removes an archetype once
/// created, but a probe archetype goes EMPTY between capture
/// requests (the screenshot wire despawns each request entity; the
/// next request re-occupies the same layout). An occupancy-based
/// count would flap: during the empty window the raw total counts
/// the probe archetype but the subtraction drops it — and because
/// the window's width depends on ASYNC GPU readback timing, two
/// identical runs can disagree on a mid-run digest frame. Counting
/// by layout membership instead makes the subtraction exact and
/// stable: a probe archetype counts as excess for the REST of the
/// run, empty or occupied.
///
/// An archetype holding BOTH game and probe entities cannot arise:
/// layout identity is component-set identity, and no game entity
/// ever carries the marker — so every marker-containing archetype
/// is probe-created and probe-exclusive, and subtracting it can
/// never under-count game state.
pub fn archetype_excess(world: &World) -> usize {
    // Registration-safe: the marker not registered ⇒ no entity can
    // carry it ⇒ no archetype can contain it ⇒ excess is honestly 0.
    let Some(marker_id) = world.component_id::<HarnessInjected>() else {
        return 0;
    };
    world
        .archetypes()
        .iter()
        .filter(|archetype| archetype.contains(marker_id))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_transform::prelude::Transform;

    #[test]
    fn marker_detects_injected_entities() {
        let mut world = World::new();
        let game = world.spawn(Transform::default()).id();
        let probe = world.spawn((Transform::default(), HarnessInjected)).id();
        assert!(!is_injected(game, &world));
        assert!(is_injected(probe, &world));
        assert_eq!(injected_count(&world), 1);
    }

    #[test]
    fn archetype_excess_counts_marker_layouts_forever() {
        let mut world = World::new();
        // Game archetype (Transform only) + probe archetype
        // (Transform + marker) = one probe-owned layout.
        world.spawn(Transform::default());
        world.spawn((Transform::default(), HarnessInjected));
        assert_eq!(archetype_excess(&world), 1);
        // A SECOND injected entity of the same layout changes
        // nothing (same archetype, still one marker layout).
        world.spawn((Transform::default(), HarnessInjected));
        assert_eq!(archetype_excess(&world), 1);
        // MEMBERSHIP, NOT OCCUPANCY: despawn every probe entity and
        // the excess STAYS 1 — bevy keeps the (now empty) archetype,
        // so the raw count keeps including it and the subtraction
        // must keep subtracting it. An occupancy-based count would
        // flap here (empty window = under-subtraction = the async
        // readback race the digests must never see).
        let probes: Vec<Entity> = QueryState::<Entity, With<HarnessInjected>>::try_new(&world)
            .map(|mut state| state.iter(&world).collect())
            .unwrap_or_default();
        for e in probes {
            world.despawn(e);
        }
        assert_eq!(archetype_excess(&world), 1);
        // A marker-only layout (different component set) is a
        // second probe-owned archetype.
        world.spawn(HarnessInjected);
        assert_eq!(archetype_excess(&world), 2);
    }

    #[test]
    fn empty_world_has_no_excess() {
        let world = World::new();
        assert_eq!(archetype_excess(&world), 0);
    }
}
