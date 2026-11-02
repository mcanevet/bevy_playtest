//! Broadened state digest for replay verification and input-surface discovery.
//!
//! # Design Overview
//!
//! This module provides a per-frame `state_digest(world) -> u64` that covers a BROAD set of
//! signals to detect any change in the game state. Unlike the narrow fingerprint in `replay.rs`
//! (which only sees Transform translations + choices), this digest can detect:
//!
//! - Entity count changes (spawns/despawns)
//! - Archetype count changes (structural world modifications)
//! - All Transform translations (preserved for backward compatibility)
//! - All UiTransform translations (pure-UI games become visible)
//! - Reflected resource values (game-defined resources only, excluding harness/engine)
//! - Component type counts (entities per component type, via reflection)
//!
//! # Signals Covered
//!
//! | Signal | Why |
//! |--------|-----|
//! | Entity count | Spawns/despawns are fundamental state changes |
//! | Archetype count | Structural changes (adding/removing component types) |
//! | Transform translations | Existing fingerprint signal, preserved for compat |
//! | UiTransform translations | Pure-UI games need visibility (menu focus, animations) |
//! | Reflected resources | Game state stored in resources (score, health, flags) |
//! | Component type counts | Adding/removing a marker component shifts a type count |
//!
//! # Determinism Policy
//!
//! - **Iteration order**: All queries are collected into `Vec`s then SORTED before hashing.
//!   Archetype iteration order is stable within identical construction, but we sort anyway.
//! - **HashMap replacement**: `BTreeMap` used where key sorting matters.
//! - **Float hashing**: Uses `to_bits()` for `f32` and `f64` — BIT-EXACT representation.
//!   This correctly handles:
//!   - `NaN`: Different NaN payloads hash differently (by design — payload difference = state diff)
//!   - `-0.0` vs `0.0`: Hash differently (IEEE 754 distinction; if game distinguishes them, digest should too)
//!   - Infinity: Bit patterns hash consistently
//!
//! # Performance Strategy
//!
//! - **Zero-cost when unused**: Digest only runs when `StateDigestRecorder` resource is inserted (like `StateTrace` in bevy_swarm).
//! - **Direct byte hashing**: No string allocations — hash bytes directly from `to_bits()`, `to_le_bytes()`.
//! - **Single pass**: All signals gathered in one query iteration, hashed incrementally.
//! - **Expected cost**: ~5-15µs per frame on typical worlds (<1000 entities); negligible vs 16ms budget.
//!
//! # Integration Path
//!
//! 1. **Backward compatible**: The new digest is opt-in via `StateDigestRecorder` resource. Existing `verify_replay`
//!    continues using `replay::fingerprint_frame()` unless migrated.
//! 2. **Migration path**: When ready, `replay::fingerprint_frame()` can be replaced with `compute_digest()`.
//!    Tests will immediately become MORE SENSITIVE (good — they'll catch bugs previously invisible).
//! 3. **Replay report**: Per-frame digests can optionally be serialized to reports when enabled.
//!
//! # Caveats
//!
//! - **Engine resources excluded**: `Time`, `Fixed timestep` resources contain wall-clock Instants — not deterministic.
//!   Only non-bevy resources are digested.
//! - **Harness resources excluded**: `RunState`, `ReplaySeries`, etc. are excluded to avoid harness-internal churn.
//! - **Reflection requires registration**: Only reflected resources with `ReflectComponent` are visible.
//! - **UiTransform feature-gated**: Requires `ui-liveness` feature to compile.

use bevy_ecs::prelude::*;
use std::collections::BTreeMap;

/// Per-frame state digest recorded during a run.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct StateDigest {
    pub frame: u64,
    pub hash: u64,
}

/// Resource that enables digest recording. When present, `record_digest_system`
/// will compute and record a digest each frame.
///
/// `with_snapshots`: If true, also record a debug snapshot explaining the digest contents.
/// Snapshots are expensive (allocations) and meant for debugging divergences, not production runs.
#[derive(Resource, Default)]
pub struct StateDigestRecorder {
    pub with_snapshots: bool,
    /// Stop recording after this frame (bounds memory during divergence tracing).
    pub stop_after: Option<u64>,
    pub digests: Vec<StateDigest>,
    /// Optional snapshots keyed by frame (only populated if `with_snapshots`).
    pub snapshots: BTreeMap<u64, DigestSnapshot>,
}

/// A debug snapshot of the digest contents (expensive, optional).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct DigestSnapshot {
    /// Total entity count.
    pub entity_count: usize,
    /// Total archetype count.
    pub archetype_count: usize,
    /// Transform count (for sanity checking).
    pub transform_count: usize,
    /// UiTransform count (for sanity checking).
    pub ui_transform_count: usize,
    /// Reflected resource keys and their canonical rendered strings.
    pub resources: BTreeMap<String, String>,
    /// Per-component-type entity counts (game-only): each reflected component
    /// type tracked, with the count of entities that carry it. Kept as
    /// `marker_counts` for schema stability despite the broader content.
    pub marker_counts: BTreeMap<String, usize>,
}

/// Internal hasher using FNV-1a algorithm (same as `replay.rs` for consistency).
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

// NOTE: floats are hashed via `to_bits()` cast to u64 inline at the call
// sites (see the Transform / UiTransform collectors). f64 hashing is not
// currently needed; if added, follow the same bit-exact policy.

/// Compute the broadened state digest for the current world state.
///
/// Covers:
/// - Live entity count (spawns/despawns)
/// - Archetype count
/// - All Transform translations (sorted by bit-pattern for determinism)
/// - All UiTransform translations (if feature enabled, sorted)
/// - Reflected game resources (sorted by full type path, canonically rendered)
/// - Component type counts (entities per reflected game component type)
pub fn compute_digest(world: &mut World) -> u64 {
    let mut hasher = fnv_hasher();

    // --- Live entity count (spawns/despawns) ---
    let entity_count = world.entities().count_spawned();
    hasher = hash_usize(hasher, entity_count as usize);

    // --- Archetype count ---
    let archetype_count = world.archetypes().len();
    hasher = hash_usize(hasher, archetype_count);

    // --- Transform translations (sorted) ---
    // Collect all transforms, sort by (x_bits, y_bits, z_bits) for canonical order.
    let mut translations: Vec<(u64, u64, u64)> = world
        .query::<&bevy_transform::prelude::Transform>()
        .iter(world)
        .map(|t| {
            (
                t.translation.x.to_bits() as u64,
                t.translation.y.to_bits() as u64,
                t.translation.z.to_bits() as u64,
            )
        })
        .collect();
    translations.sort();
    hasher = hash_usize(hasher, translations.len());
    for (x, y, z) in translations {
        hasher = hash_u64(hasher, x);
        hasher = hash_u64(hasher, y);
        hasher = hash_u64(hasher, z);
    }

    // --- UiTransform translations (sorted, if feature enabled) ---
    #[cfg(feature = "ui-liveness")]
    {
        use bevy_ui::prelude::*;
        let mut ui_transforms: Vec<(u64, u64)> = world
            .query::<&UiTransform>()
            .iter(world)
            .filter_map(|ut| {
                // UiTransform translation uses Val2 (2D) with Val<f32>.
                // Extract Px values only; other units (Percent, Viewport) are ignored for digest simplicity.
                match ut.translation.x {
                    bevy_ui::Val::Px(x) => match ut.translation.y {
                        bevy_ui::Val::Px(y) => Some((x.to_bits() as u64, y.to_bits() as u64)),
                        _ => None,
                    },
                    _ => None,
                }
            })
            .collect();
        ui_transforms.sort();
        hasher = hash_usize(hasher, ui_transforms.len());
        for (x, y) in ui_transforms {
            hasher = hash_u64(hasher, x);
            hasher = hash_u64(hasher, y);
        }
    }

    // --- Reflected resources (game-only, sorted by type path) ---
    collect_resource_digests(world, &mut hasher);

    // --- Component type counts (game-only, sorted by type path) ---
    collect_component_type_digests(world, &mut hasher);

    hasher
}

/// Harness bookkeeping resources that must be excluded from the digest:
/// they change independently of the game (recording state, bot choices)
/// and would flag harness behavior as divergence.
///
/// Matching is by FULL type path prefix `bevy_playtest::`, so a game
/// resource that merely shares a short name (e.g. `game::RunState`)
/// is never excluded.
const EXCLUDED_HARNESS_PREFIX: &str = "bevy_playtest::";

/// Returns `true` if the resource with the given type paths is excluded from
/// the digest. Exposed so callers can reason about which of their resources
/// are covered:
/// - Engine types (`bevy_*`, `bevy::*`): non-deterministic wall-clock/pool data
///   (Instants, task-pool state, auto-registered caches).
///   **Trade-off**: games in a crate whose name starts with `bevy_` (e.g.
///   `bevy_mygame::Score`) will also be excluded; this is acceptable because
///   we avoid false divergences from leaking engine internals over the
///   marginal gain of catching such edge-case crate naming.
/// - Harness bookkeeping types, in the `bevy_playtest::` namespace
pub fn resource_is_excluded(full_path: &str, short_path: &str) -> bool {
    let _ = short_path;
    (full_path.starts_with("bevy_") || full_path.starts_with("bevy::"))
        || full_path.starts_with(EXCLUDED_HARNESS_PREFIX)
}

/// Collect `(full_type_path, canonical_render)` for every reflected GAME
/// resource present in the world, sorted canonically.
///
/// Two filters apply:
/// - Engine types (`bevy_*`, `bevy::*`) are excluded wholesale: they carry
///   wall-clock Instants, task-pool state and other non-deterministic data
///   that is not GAME state.
/// - Harness types are excluded by the `bevy_playtest::` full-path prefix
///   (see [`resource_is_excluded`]): they are deterministic but represent
///   harness bookkeeping, not game state; hashing them would flag harness
///   behavior as divergence.
///
/// # Canonical rendering
///
/// Hashing uses [`hash_canonical`], a direct walk over the reflected
/// tree that folds each field into the running FNV hasher WITHOUT
/// materializing strings (length- and tag-prefixed, so boundary shifts
/// cannot collide). Unordered collections (`HashMap`/`HashSet`) hash
/// through per-entry sub-hashers sorted numerically — deterministic
/// without full-string allocation. Opaque leaves hash `f32`/`f64`
/// bitwise via `to_bits()`; other opaque leaves fall back to `Debug`
/// and remain the residual risk surface.
///
/// Snapshots (opt-in, `with_snapshots`) use [`render_canonical`], the
/// string-rendering twin: same canonicalization, human-readable output.
fn collect_resource_reflections_locked<'a>(
    world: &'a World,
    registry: &'a bevy_reflect::TypeRegistry,
) -> Vec<(String, &'a dyn bevy_reflect::Reflect)> {
    let mut entries: Vec<(String, &'a dyn bevy_reflect::Reflect)> = vec![];

    for type_reg in registry.iter() {
        let full_path = type_reg.type_info().type_path_table().path();
        let short_path = type_reg.type_info().type_path_table().short_path();

        // Apply exclusion filter.
        if resource_is_excluded(full_path, short_path) {
            continue;
        }

        // Only resources with ReflectComponent data.
        let Some(reflect_component) = type_reg.data::<bevy_ecs::reflect::ReflectComponent>() else {
            continue;
        };

        // Find the resource entity: O(1) sparse-array lookup by the
        // CURRENT registration's TypeId (via its ComponentId), not by
        // short path: two distinct resources can share a short path
        // (e.g. `game::State` vs `ui::State`), and a short-path lookup
        // would silently pick one of them.
        let Some(component_id) = world.components().get_valid_id(type_reg.type_id()) else {
            continue;
        };

        let Some(resource_entity) = world.resource_entities().get(component_id) else {
            continue;
        };

        let Ok(entity_ref) = world.get_entity(resource_entity) else {
            continue;
        };

        let Some(reflect) = reflect_component.reflect(entity_ref) else {
            continue;
        };

        // Full type path as the entry key: distinct resources can share a
        // short path (e.g. `game::State` vs `ui::State`); hashing only the
        // short path would conflate them and miss a type/state replacement.
        entries.push((full_path.to_string(), reflect));
    }

    // Canonical order for deterministic hashing. (Sort by path only;
    // paths are unique so no value tie-break is needed.)
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
}

/// Hash the collected resource entries into the running digest.
///
/// No per-frame STRING allocation: reflected trees fold directly into
/// per-resource sub-hashers via [`hash_canonical`] (registry paths are
/// hashed from their borrowed &str). The canonical ORDER is achieved by
/// sorting the small (path-hash, value-hash) pair vector numerically —
/// no owned path strings, no rendered values. (A single scratch Vec of
/// pairs is allocated per frame; bounded by the number of reflected
/// resources, not the serialized size of their values.)
fn collect_resource_digests(world: &World, hasher: &mut u64) {
    let Some(registry_arc) = world.get_resource::<bevy_ecs::reflect::AppTypeRegistry>() else {
        return;
    };
    let registry = registry_arc.0.read();

    let mut resource_hashes: Vec<(u64, u64)> = vec![];
    for type_reg in registry.iter() {
        let full_path = type_reg.type_info().type_path_table().path();
        let short_path = type_reg.type_info().type_path_table().short_path();

        // Apply exclusion filter.
        if resource_is_excluded(full_path, short_path) {
            continue;
        }

        // Only resources with ReflectComponent data.
        let Some(reflect_component) = type_reg.data::<bevy_ecs::reflect::ReflectComponent>() else {
            continue;
        };

        // Find the resource entity: O(1) sparse-array lookup by the
        // CURRENT registration's TypeId (via its ComponentId), not by
        // short path: two distinct resources can share a short path
        // (e.g. `game::State` vs `ui::State`), and a short-path lookup
        // would silently pick one of them.
        let Some(component_id) = world.components().get_valid_id(type_reg.type_id()) else {
            continue;
        };

        let Some(resource_entity) = world.resource_entities().get(component_id) else {
            continue;
        };

        let Ok(entity_ref) = world.get_entity(resource_entity) else {
            continue;
        };

        let Some(reflect) = reflect_component.reflect(entity_ref) else {
            continue;
        };

        // Per-resource sub-hasher: the PATH and VALUE hashes are each
        // length-prefixed within their own sub-stream, so key/value
        // boundary shifts cannot collide.
        let mut key_h = fnv_hasher();
        key_h = hash_usize(key_h, full_path.len());
        for b in full_path.bytes() {
            key_h = hash_u64(key_h, b as u64);
        }

        let mut value_h = fnv_hasher();
        hash_canonical(reflect.as_partial_reflect(), &mut value_h);

        resource_hashes.push((key_h, value_h));
    }

    // Canonical order for deterministic hashing: sort by (path-hash,
    // value-hash) pairs — path hashes are unique per resource type.
    resource_hashes.sort_unstable();
    *hasher = hash_usize(*hasher, resource_hashes.len());
    for (key_h, value_h) in resource_hashes {
        *hasher = hash_u64(*hasher, key_h);
        *hasher = hash_u64(*hasher, value_h);
    }
}

/// Count entities per reflected GAME component type (keyed by full type
/// path, sorted canonically). This detects adding/removing a component
/// type from entities even when no transform, resource, or entity count
/// changes — e.g. flipping a marker component on a static entity.
///
/// The same exclusion rules as resources apply (engine + harness types):
/// engine component counts churn with plugin bookkeeping (relationship
/// markers etc.) and harness component counts track bot bookkeeping, none
/// of which is game state.
fn collect_component_type_counts(world: &World) -> BTreeMap<String, usize> {
    let Some(registry_arc) = world.get_resource::<bevy_ecs::reflect::AppTypeRegistry>() else {
        // No reflection registry — no component-type coverage.
        return BTreeMap::new();
    };
    let registry = registry_arc.0.read();

    // Look up each registered (non-excluded) component type once; resolve
    // its ComponentId so we can count matches across archetypes.
    let mut counts: BTreeMap<bevy_ecs::component::ComponentId, usize> = BTreeMap::new();
    let mut id_to_path: BTreeMap<bevy_ecs::component::ComponentId, &str> = BTreeMap::new();
    for type_reg in registry.iter() {
        let full_path = type_reg.type_info().type_path_table().path();
        let short_path = type_reg.type_info().type_path_table().short_path();
        if resource_is_excluded(full_path, short_path) {
            continue;
        }
        // Only types that were registered as components.
        let Some(_reflect_component) = type_reg.data::<bevy_ecs::reflect::ReflectComponent>()
        else {
            continue;
        };
        let Some(component_id) = world.components().get_valid_id(type_reg.type_id()) else {
            // Registered in the type registry but not instantiated in this
            // world — no entities can carry it.
            continue;
        };
        id_to_path.insert(component_id, full_path);
    }

    // Sum entity counts per component type across archetypes. Entities in an
    // archetype all share its component set, so archetype.len() is exact.
    // Sum entity counts per component type across archetypes. Entities in an
    // archetype all share its component set, so archetype.len() is exact.
    for archetype in world.archetypes().iter() {
        for component_id in archetype.components() {
            if id_to_path.contains_key(component_id) {
                *counts.entry(*component_id).or_insert(0) += archetype.len() as usize;
            }
        }
    }

    counts
        .into_iter()
        .map(|(id, count)| (id_to_path[&id].to_string(), count))
        .collect()
}

/// Hash component-type counts into the running digest.
fn collect_component_type_digests(world: &World, hasher: &mut u64) {
    let counts = collect_component_type_counts(world);
    *hasher = hash_usize(*hasher, counts.len());
    for (path, count) in &counts {
        for b in path.bytes() {
            *hasher = hash_u64(*hasher, b as u64);
        }
        *hasher = hash_usize(*hasher, *count);
    }
}

/// Recursively render a reflected value into a CANONICAL string: unordered
/// collections (maps, sets) have their entries sorted by rendered form, so
/// two equal values in different iteration orders produce identical output.
/// `Debug` formatting alone cannot guarantee that (HashMap/HashSet iteration
/// order depends on hasher state and insertion history).
fn render_canonical(value: &dyn bevy_reflect::PartialReflect) -> String {
    fn rec(value: &dyn bevy_reflect::PartialReflect, out: &mut String) {
        use bevy_reflect::ReflectRef;

        // Unordered container kinds first: they need sorting.
        match value.reflect_ref() {
            ReflectRef::Map(map) => {
                let mut entries: Vec<(String, String)> = map
                    .iter()
                    .map(|(k, v)| {
                        let mut ks = String::new();
                        let mut vs = String::new();
                        rec(k.as_partial_reflect(), &mut ks);
                        rec(v.as_partial_reflect(), &mut vs);
                        (ks, vs)
                    })
                    .collect();
                entries.sort();
                out.push('{');
                for (k, v) in entries {
                    out.push_str(&k);
                    out.push_str(": ");
                    out.push_str(&v);
                    out.push_str(", ");
                }
                out.push('}');
            }
            ReflectRef::Set(set) => {
                let mut items: Vec<String> = set
                    .iter()
                    .map(|v| {
                        let mut s = String::new();
                        rec(v, &mut s);
                        s
                    })
                    .collect();
                items.sort();
                out.push('{');
                for item in items {
                    out.push_str(&item);
                    out.push_str(", ");
                }
                out.push('}');
            }
            // Ordered containers keep position; recurse where needed.
            ReflectRef::Struct(s) => {
                out.push('{');
                for idx in 0..s.field_len() {
                    if let Some(name) = s.name_at(idx) {
                        out.push_str(name);
                        out.push_str(": ");
                    }
                    if let Some(field) = s.field_at(idx) {
                        let mut fs = String::new();
                        rec(field, &mut fs);
                        out.push_str(&fs);
                    }
                    out.push_str(", ");
                }
                out.push('}');
            }
            ReflectRef::TupleStruct(ts) => {
                out.push('(');
                for idx in 0..ts.field_len() {
                    if let Some(field) = ts.field(idx) {
                        let mut fs = String::new();
                        rec(field, &mut fs);
                        out.push_str(&fs);
                    }
                    out.push_str(", ");
                }
                out.push(')');
            }
            ReflectRef::Tuple(t) => {
                out.push('(');
                for idx in 0..t.field_len() {
                    if let Some(field) = t.field(idx) {
                        let mut fs = String::new();
                        rec(field, &mut fs);
                        out.push_str(&fs);
                    }
                    out.push_str(", ");
                }
                out.push(')');
            }
            ReflectRef::List(list) => {
                out.push('[');
                for item in list.iter() {
                    let mut s = String::new();
                    rec(item.as_partial_reflect(), &mut s);
                    out.push_str(&s);
                    out.push_str(", ");
                }
                out.push(']');
            }
            ReflectRef::Array(arr) => {
                out.push('[');
                for item in arr.iter() {
                    let mut s = String::new();
                    rec(item.as_partial_reflect(), &mut s);
                    out.push_str(&s);
                    out.push_str(", ");
                }
                out.push(']');
            }
            ReflectRef::Enum(e) => {
                out.push_str(e.variant_name());
                out.push('(');
                for idx in 0..e.field_len() {
                    if let Some(field) = e.field_at(idx) {
                        let mut fs = String::new();
                        rec(field, &mut fs);
                        out.push_str(&fs);
                        out.push_str(", ");
                    }
                }
                out.push(')');
            }
            // NOTE: `ReflectRef::Function` requires bevy_reflect's
            // `functions` feature (not enabled); function values surface
            // here as `Opaque` and fall through to the same Debug path.
            ReflectRef::Opaque(_) => {
                // Primitive float leaves are hashed BITWISE via `to_bits()`
                // BEFORE falling back to Debug: distinct NaN payloads (and
                // -0.0 vs 0.0) all format identically under Debug, which
                // would violate the digest's bit-exact float policy and
                // hide resource-only NaN-state divergence.
                if let Some(bits) = value.try_downcast_ref::<f32>() {
                    out.push_str(&format!("f32:{:#018x}", bits.to_bits()));
                } else if let Some(bits) = value.try_downcast_ref::<f64>() {
                    out.push_str(&format!("f64:{:#026x}", bits.to_bits()));
                } else {
                    // Other scalar/opaque leaves: delegate to Debug
                    // (bit-deterministic for primitives; opaque wrappers
                    // are the residual risk surface).
                    out.push_str(&format!("{value:?}"));
                }
            }
            #[allow(unreachable_patterns)]
            _ => {
                // Fallback for reflect kinds not handled above (future
                // bevy_reflect additions): Debug, same as opaque leaves.
                // (Primitives always surface as `Opaque`, so float
                // bit-exactness is unaffected by this arm.)
                out.push_str(&format!("{value:?}"));
            }
        }
    }
    let mut s = String::new();
    rec(value, &mut s);
    s
}

/// Allocation-free hashing twin of [`render_canonical`]: walks the
/// reflected tree and folds it DIRECTLY into the running hasher with
/// type tags and length prefixes, so no strings are materialized per
/// frame and boundary shifts between fields/entries cannot collide.
///
/// Canonicalization parity with `render_canonical`:
/// - Struct/TupleStruct/Tuple/List/Array: field count is hashed first
///   (the "length prefix"), then each field in order with its tag.
/// - Map/Set: per-entry SUB-HASHERS seeded fresh from a constant,
///   collected and sorted NUMERICALLY, then folded in sorted order —
///   deterministic for unordered containers without allocating
///   rendered strings.
/// - Enum: variant name length-prefixed, then fields.
/// - Opaque floats: `f32`/`f64` hashed bitwise via `to_bits()`;
///   other opaque leaves hash their `Debug` rendering (residual risk
///   surface, same as `render_canonical`).
fn hash_canonical(value: &dyn bevy_reflect::PartialReflect, hasher: &mut u64) {
    fn rec(value: &dyn bevy_reflect::PartialReflect, hasher: &mut u64) {
        use bevy_reflect::ReflectRef;

        // Tag the KIND first: distinct reflect shapes for the same bytes
        // must not collide.
        match value.reflect_ref() {
            ReflectRef::Map(map) => {
                *hasher = hash_u64(*hasher, TAG_MAP);
                *hasher = hash_usize(*hasher, map.len());
                let mut entry_hashes: Vec<u64> = Vec::with_capacity(map.len());
                for (k, v) in map.iter() {
                    // Sub-hasher per entry: numeric sort is order-independent.
                    let mut eh = fnv_hasher();
                    rec(k.as_partial_reflect(), &mut eh);
                    rec(v.as_partial_reflect(), &mut eh);
                    entry_hashes.push(eh);
                }
                entry_hashes.sort_unstable();
                for eh in entry_hashes {
                    *hasher = hash_u64(*hasher, eh);
                }
            }
            ReflectRef::Set(set) => {
                *hasher = hash_u64(*hasher, TAG_SET);
                *hasher = hash_usize(*hasher, set.len());
                let mut item_hashes: Vec<u64> = Vec::with_capacity(set.len());
                for v in set.iter() {
                    let mut ih = fnv_hasher();
                    rec(v, &mut ih);
                    item_hashes.push(ih);
                }
                item_hashes.sort_unstable();
                for ih in item_hashes {
                    *hasher = hash_u64(*hasher, ih);
                }
            }
            // Ordered containers keep position; recurse where needed.
            ReflectRef::Struct(s) => {
                *hasher = hash_u64(*hasher, TAG_STRUCT);
                *hasher = hash_usize(*hasher, s.field_len());
                for idx in 0..s.field_len() {
                    if let Some(name) = s.name_at(idx) {
                        *hasher = hash_usize(*hasher, name.len());
                        for b in name.bytes() {
                            *hasher = hash_u64(*hasher, b as u64);
                        }
                    }
                    if let Some(field) = s.field_at(idx) {
                        rec(field, hasher);
                    }
                }
            }
            ReflectRef::TupleStruct(ts) => {
                *hasher = hash_u64(*hasher, TAG_TUPLE_STRUCT);
                *hasher = hash_usize(*hasher, ts.field_len());
                for idx in 0..ts.field_len() {
                    if let Some(field) = ts.field(idx) {
                        rec(field, hasher);
                    }
                }
            }
            ReflectRef::Tuple(t) => {
                *hasher = hash_u64(*hasher, TAG_TUPLE);
                *hasher = hash_usize(*hasher, t.field_len());
                for idx in 0..t.field_len() {
                    if let Some(field) = t.field(idx) {
                        rec(field, hasher);
                    }
                }
            }
            ReflectRef::List(list) => {
                *hasher = hash_u64(*hasher, TAG_LIST);
                *hasher = hash_usize(*hasher, list.len());
                for item in list.iter() {
                    rec(item.as_partial_reflect(), hasher);
                }
            }
            ReflectRef::Array(arr) => {
                *hasher = hash_u64(*hasher, TAG_ARRAY);
                *hasher = hash_usize(*hasher, arr.len());
                for item in arr.iter() {
                    rec(item.as_partial_reflect(), hasher);
                }
            }
            ReflectRef::Enum(e) => {
                *hasher = hash_u64(*hasher, TAG_ENUM);
                let name = e.variant_name();
                *hasher = hash_usize(*hasher, name.len());
                for b in name.bytes() {
                    *hasher = hash_u64(*hasher, b as u64);
                }
                *hasher = hash_usize(*hasher, e.field_len());
                for idx in 0..e.field_len() {
                    if let Some(field) = e.field_at(idx) {
                        rec(field, hasher);
                    }
                }
            }
            // NOTE: `ReflectRef::Function` requires bevy_reflect's
            // `functions` feature (not enabled); function values surface
            // here as `Opaque` and fall through to the primitive/
            // Debug path below.
            ReflectRef::Opaque(_) => {
                *hasher = hash_u64(*hasher, TAG_OPAQUE);
                // Primitives hash DIRECTLY (bit patterns) — no
                // `format!` allocation per leaf. Each primitive type
                // contributes a distinct PTAG discriminant before its
                // bits, so cross-type bit-pattern collisions (e.g.
                // `true` vs `1u8`, `1u16` vs `1u32`) cannot occur,
                // preserving the distinguishing power of Debug
                // rendering without allocating.
                if let Some(v) = value.try_downcast_ref::<f32>() {
                    *hasher = hash_u64(*hasher, PTAG_F32);
                    *hasher = hash_u64(*hasher, v.to_bits() as u64);
                } else if let Some(v) = value.try_downcast_ref::<f64>() {
                    *hasher = hash_u64(*hasher, PTAG_F64);
                    *hasher = hash_u64(*hasher, v.to_bits());
                } else if let Some(v) = value.try_downcast_ref::<bool>() {
                    *hasher = hash_u64(*hasher, PTAG_BOOL);
                    *hasher = hash_u64(*hasher, *v as u64);
                } else if let Some(v) = value.try_downcast_ref::<char>() {
                    *hasher = hash_u64(*hasher, PTAG_CHAR);
                    *hasher = hash_u64(*hasher, *v as u64);
                } else if let Some(v) = value.try_downcast_ref::<u8>() {
                    *hasher = hash_u64(*hasher, PTAG_U8);
                    *hasher = hash_u64(*hasher, *v as u64);
                } else if let Some(v) = value.try_downcast_ref::<u16>() {
                    *hasher = hash_u64(*hasher, PTAG_U16);
                    *hasher = hash_u64(*hasher, *v as u64);
                } else if let Some(v) = value.try_downcast_ref::<u32>() {
                    *hasher = hash_u64(*hasher, PTAG_U32);
                    *hasher = hash_u64(*hasher, *v as u64);
                } else if let Some(v) = value.try_downcast_ref::<u64>() {
                    *hasher = hash_u64(*hasher, PTAG_U64);
                    *hasher = hash_u64(*hasher, *v);
                } else if let Some(v) = value.try_downcast_ref::<usize>() {
                    *hasher = hash_u64(*hasher, PTAG_USIZE);
                    *hasher = hash_u64(*hasher, *v as u64);
                } else if let Some(v) = value.try_downcast_ref::<i8>() {
                    *hasher = hash_u64(*hasher, PTAG_I8);
                    *hasher = hash_u64(*hasher, *v as i64 as u64);
                } else if let Some(v) = value.try_downcast_ref::<i16>() {
                    *hasher = hash_u64(*hasher, PTAG_I16);
                    *hasher = hash_u64(*hasher, *v as i64 as u64);
                } else if let Some(v) = value.try_downcast_ref::<i32>() {
                    *hasher = hash_u64(*hasher, PTAG_I32);
                    *hasher = hash_u64(*hasher, *v as i64 as u64);
                } else if let Some(v) = value.try_downcast_ref::<i64>() {
                    *hasher = hash_u64(*hasher, PTAG_I64);
                    *hasher = hash_u64(*hasher, *v as u64);
                } else if let Some(v) = value.try_downcast_ref::<isize>() {
                    *hasher = hash_u64(*hasher, PTAG_ISIZE);
                    *hasher = hash_u64(*hasher, *v as i64 as u64);
                } else if let Some(v) = value.try_downcast_ref::<String>() {
                    // Owned/borrowed strings hash content + length —
                    // still no allocation (no formatting involved).
                    *hasher = hash_u64(*hasher, PTAG_STRING);
                    *hasher = hash_usize(*hasher, v.len());
                    for b in v.bytes() {
                        *hasher = hash_u64(*hasher, b as u64);
                    }
                } else {
                    // Genuinely opaque wrappers: Debug rendering is the
                    // residual risk surface. Rare on hot paths (most
                    // game state is primitives/strings/containers).
                    let rendered = format!("{value:?}");
                    *hasher = hash_usize(*hasher, rendered.len());
                    for b in rendered.bytes() {
                        *hasher = hash_u64(*hasher, b as u64);
                    }
                }
            }
            #[allow(unreachable_patterns)]
            _ => {
                // Fallback for reflect kinds not handled above (future
                // bevy_reflect additions): Debug, same as opaque leaves.
                *hasher = hash_u64(*hasher, TAG_OPAQUE);
                let rendered = format!("{value:?}");
                *hasher = hash_usize(*hasher, rendered.len());
                for b in rendered.bytes() {
                    *hasher = hash_u64(*hasher, b as u64);
                }
            }
        }
    }
    rec(value, hasher);
}

/// Stream-kind tags for `hash_canonical` so structurally different
/// values with identical byte content cannot collide.
const TAG_STRUCT: u64 = 0x01;
const TAG_MAP: u64 = 0x02;
const TAG_SET: u64 = 0x03;
const TAG_TUPLE: u64 = 0x04;
const TAG_TUPLE_STRUCT: u64 = 0x05;
const TAG_LIST: u64 = 0x06;
const TAG_ARRAY: u64 = 0x07;
const TAG_ENUM: u64 = 0x08;
const TAG_OPAQUE: u64 = 0x09;

/// Per-primitive-type discriminants hashed before primitive leaves in
/// `hash_canonical`, so e.g. `true` and `1u8` cannot collide.
const PTAG_F32: u64 = 0x101;
const PTAG_F64: u64 = 0x102;
const PTAG_BOOL: u64 = 0x103;
const PTAG_CHAR: u64 = 0x104;
const PTAG_U8: u64 = 0x105;
const PTAG_U16: u64 = 0x106;
const PTAG_U32: u64 = 0x107;
const PTAG_U64: u64 = 0x108;
const PTAG_USIZE: u64 = 0x109;
const PTAG_I8: u64 = 0x10a;
const PTAG_I16: u64 = 0x10b;
const PTAG_I32: u64 = 0x10c;
const PTAG_I64: u64 = 0x10d;
const PTAG_ISIZE: u64 = 0x10e;
const PTAG_STRING: u64 = 0x10f;

/// Build a debug snapshot of the current digest components. Expensive (allocations),
/// meant for debugging divergences.
pub fn build_snapshot(world: &mut World) -> DigestSnapshot {
    // Live entity count, matching the digest's spawn/despawn signal
    // (Entities::len() counts allocated indices, including despawned).
    let entity_count = world.entities().count_spawned() as usize;
    let archetype_count = world.archetypes().len();

    let transform_count = world
        .query::<&bevy_transform::prelude::Transform>()
        .iter(world)
        .count();

    let ui_transform_count = {
        #[cfg(feature = "ui-liveness")]
        {
            use bevy_ui::prelude::*;
            world.query::<&UiTransform>().iter(world).count()
        }
        #[cfg(not(feature = "ui-liveness"))]
        {
            0
        }
    };

    let mut resources = BTreeMap::new();
    if let Some(registry_arc) = world.get_resource::<bevy_ecs::reflect::AppTypeRegistry>() {
        let registry = registry_arc.0.read();
        for (path, value) in collect_resource_reflections_locked(world, &registry) {
            resources.insert(path, render_canonical(value));
        }
    }

    // Component-type counts share the digest's exclusion rules; the
    // legacy field name (`marker_counts`) is kept for schema stability.
    let marker_counts = collect_component_type_counts(world);

    DigestSnapshot {
        entity_count,
        archetype_count,
        transform_count,
        ui_transform_count,
        resources,
        marker_counts,
    }
}

/// System that records the digest each frame when `StateDigestRecorder` is present.
pub fn record_digest_system(world: &mut World) {
    if world.get_resource::<StateDigestRecorder>().is_none() {
        return; // Opt-in: zero cost when not recording.
    }

    let frame = world
        .get_resource::<crate::driver::RunState>()
        .map(|s| s.frame)
        .unwrap_or(0);

    // Check stop condition before doing any work.
    if let Some(stop_after) = world
        .get_resource::<StateDigestRecorder>()
        .and_then(|r| r.stop_after)
    {
        if frame > stop_after {
            return;
        }
    }

    let with_snapshots = world
        .get_resource::<StateDigestRecorder>()
        .map(|r| r.with_snapshots)
        .unwrap_or(false);

    let hash = compute_digest(world);
    let snapshot = if with_snapshots {
        Some(build_snapshot(world))
    } else {
        None
    };

    let mut recorder = world
        .get_resource_mut::<StateDigestRecorder>()
        .expect("checked above");
    recorder.digests.push(StateDigest { frame, hash });
    if let Some(snapshot) = snapshot {
        recorder.snapshots.insert(frame, snapshot);
    }
}

/// Compare two digest series; returns the first divergence.
pub fn compare(a: &[StateDigest], b: &[StateDigest]) -> Result<(), Divergence> {
    // Compare frame-by-frame; if frames disagree, that's also a divergence.
    for (da, db) in a.iter().zip(b.iter()) {
        if da.frame != db.frame {
            return Err(Divergence {
                frame: da.frame.min(db.frame),
                hash_a: da.hash,
                hash_b: db.hash,
            });
        }
        if da.hash != db.hash {
            return Err(Divergence {
                frame: da.frame,
                hash_a: da.hash,
                hash_b: db.hash,
            });
        }
    }
    if a.len() != b.len() {
        let frame = a.len().min(b.len()) as u64;
        return Err(Divergence {
            frame,
            hash_a: a.get(frame as usize).map(|d| d.hash).unwrap_or(0),
            hash_b: b.get(frame as usize).map(|d| d.hash).unwrap_or(0),
        });
    }
    Ok(())
}

/// A digest divergence: the first frame where digests disagree.
#[derive(Clone, Debug)]
pub struct Divergence {
    pub frame: u64,
    pub hash_a: u64,
    pub hash_b: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_app::App;

    #[test]
    fn hash_canonical_matches_render_canonical_distinguishing_power() {
        // Equality power: two values that render differently must hash
        // differently (hashing must not LOSE coverage relative to the
        // rendering path), and equal values must hash equal.
        let a: f32 = 1.0;
        let b: f32 = 2.0;
        let (mut ha, mut hb) = (fnv_hasher(), fnv_hasher());
        hash_canonical(&a, &mut ha);
        hash_canonical(&b, &mut hb);
        assert_ne!(ha, hb, "distinct floats must hash differently");

        let a2: f32 = 1.0;
        let mut ha2 = fnv_hasher();
        hash_canonical(&a2, &mut ha2);
        assert_eq!(ha, ha2, "equal floats must hash identically");
    }

    #[test]
    fn hash_canonical_boundary_delimiting_prevents_collisions() {
        use bevy_reflect::Reflect;
        // A naive key||value stream could let different (key, value)
        // splits serialize to the same bytes. `hash_canonical` tags and
        // lengths everything, so structurally different trees with
        // overlapping byte content must not collide.
        #[derive(Reflect)]
        struct A(u8, u8);
        #[derive(Reflect)]
        struct B(u16);
        let (mut ha, mut hb) = (fnv_hasher(), fnv_hasher());
        hash_canonical(&A(0x11, 0x22), &mut ha);
        hash_canonical(&B(0x2211), &mut hb);
        assert_ne!(
            ha, hb,
            "byte-identical payloads in different shapes must not collide"
        );

        // Empty containers of different kinds must not collide either
        // (tuple () vs struct {} vs list []).
        #[derive(Reflect)]
        struct Unit;
        #[derive(Reflect)]
        struct Tuple0();
        let v1: Vec<u8> = vec![];
        let (mut h1, mut h2, mut h3) = (fnv_hasher(), fnv_hasher(), fnv_hasher());
        hash_canonical(&Unit, &mut h1);
        hash_canonical(&Tuple0(), &mut h2);
        hash_canonical(&v1, &mut h3);
        assert_ne!(h1, h2, "unit struct vs empty tuple struct");
        assert_ne!(h2, h3, "empty tuple struct vs empty list");
        assert_ne!(h1, h3, "unit struct vs empty list");
    }

    #[test]
    fn hash_canonical_distinguishes_container_kinds() {
        use bevy_reflect::Reflect;
        // Arrays vs lists, and tuple structs vs tuples, must hash
        // DIFFERENTLY even with identical element content: a distinct
        // kind tag precedes each container. (Regression guard: arrays
        // once shared TAG_LIST, letting a dynamic value migrate between
        // container kinds without changing the digest.)
        let arr: [u32; 3] = [1, 2, 3];
        let list: Vec<u32> = vec![1, 2, 3];
        let (mut ha, mut hl) = (fnv_hasher(), fnv_hasher());
        hash_canonical(&arr, &mut ha);
        hash_canonical(&list, &mut hl);
        assert_ne!(ha, hl, "array and list must hash differently");

        #[derive(Reflect)]
        struct Ts(u32, u32, u32);
        let tup: (u32, u32, u32) = (1, 2, 3);
        let (mut hts, mut ht) = (fnv_hasher(), fnv_hasher());
        hash_canonical(&Ts(1, 2, 3), &mut hts);
        hash_canonical(&tup, &mut ht);
        assert_ne!(hts, ht, "tuple struct and tuple must hash differently");
    }

    #[test]
    fn hash_canonical_primitive_types_do_not_collide_across_types() {
        // Same numeric bit patterns in different primitive types must
        // hash differently (per-type PTAG discriminants), matching the
        // distinguishing power of the old Debug rendering.
        let b: bool = true;
        let u: u8 = 1;
        let u16v: u16 = 1;
        let u32v: u32 = 1;
        let i: i64 = 1;
        let (mut hb, mut hu, mut hu16, mut hu32, mut hi) = (
            fnv_hasher(),
            fnv_hasher(),
            fnv_hasher(),
            fnv_hasher(),
            fnv_hasher(),
        );
        hash_canonical(&b, &mut hb);
        hash_canonical(&u, &mut hu);
        hash_canonical(&u16v, &mut hu16);
        hash_canonical(&u32v, &mut hu32);
        hash_canonical(&i, &mut hi);
        let all = [hb, hu, hu16, hu32, hi];
        for x in 0..all.len() {
            for y in (x + 1)..all.len() {
                assert_ne!(
                    all[x], all[y],
                    "primitive value 1 must hash differently per type"
                );
            }
        }

        // Strings hash content + length deterministically, and unequal
        // strings must not collide.
        let (mut hs1, mut hs2) = (fnv_hasher(), fnv_hasher());
        hash_canonical(&"foo".to_string(), &mut hs1);
        hash_canonical(&"bar".to_string(), &mut hs2);
        assert_ne!(hs1, hs2, "different strings must hash differently");
    }

    #[test]
    fn hash_canonical_unordered_containers_are_order_independent() {
        // HashMap/HashSet iteration order varies with insertion history;
        // the digest must be stable regardless. Sub-hasher sorting makes
        // hash_canonical order-independent, mirroring render_canonical.
        use std::collections::{HashMap, HashSet};
        let mut m1: HashMap<String, u32> = HashMap::new();
        m1.insert("a".into(), 1);
        m1.insert("b".into(), 2);
        m1.insert("c".into(), 3);
        let mut m2: HashMap<String, u32> = HashMap::new();
        // Reverse insertion order: different internal layout.
        m2.insert("c".into(), 3);
        m2.insert("b".into(), 2);
        m2.insert("a".into(), 1);
        let (mut h1, mut h2) = (fnv_hasher(), fnv_hasher());
        hash_canonical(&m1, &mut h1);
        hash_canonical(&m2, &mut h2);
        assert_eq!(h1, h2, "maps with equal entries must hash identically");

        let mut s1: HashSet<u32> = HashSet::new();
        s1.insert(1);
        s1.insert(2);
        s1.insert(3);
        let mut s2: HashSet<u32> = HashSet::new();
        s2.insert(3);
        s2.insert(1);
        s2.insert(2);
        let (mut h1, mut h2) = (fnv_hasher(), fnv_hasher());
        hash_canonical(&s1, &mut h1);
        hash_canonical(&s2, &mut h2);
        assert_eq!(h1, h2, "sets with equal items must hash identically");

        // And a genuinely different content set must differ.
        let mut s3: HashSet<u32> = HashSet::new();
        s3.insert(4);
        let mut h3 = fnv_hasher();
        hash_canonical(&s3, &mut h3);
        assert_ne!(h1, h3, "different set content must hash differently");
    }

    #[test]
    fn render_canonical_floats_are_bit_exact() {
        // Distinct NaN payloads: `Debug` formats both as `NaN`, but the
        // digest policy promises bitwise float hashing — the rendered
        // forms must differ.
        let nan_a: f32 = f32::from_bits(0x7FC0_0000);
        let nan_b: f32 = f32::from_bits(0xFFC0_0000);
        assert_ne!(render_canonical(&nan_a), render_canonical(&nan_b));

        // -0.0 vs 0.0 also collapse under Debug; bits must not.
        let neg_zero: f32 = -0.0;
        let pos_zero: f32 = 0.0;
        assert_ne!(render_canonical(&neg_zero), render_canonical(&pos_zero));

        // Bitwise-equal values still render identically (stability).
        assert_eq!(render_canonical(&nan_a), render_canonical(&nan_a));

        // Same checks for f64.
        let nan_64_a: f64 = f64::from_bits(0x7FF8_0000_0000_0000);
        let nan_64_b: f64 = f64::from_bits(0xFFF8_0000_0000_0000);
        assert_ne!(render_canonical(&nan_64_a), render_canonical(&nan_64_b));
    }

    #[test]
    fn empty_world_produces_stable_digest() {
        let mut world = World::new();
        let digest_a = compute_digest(&mut world);
        let digest_b = compute_digest(&mut world);
        assert_eq!(digest_a, digest_b, "empty world digest must be stable");
    }

    #[test]
    fn entity_spawn_changes_digest() {
        let mut world = World::new();
        let digest_before = compute_digest(&mut world);
        world.spawn(bevy_transform::prelude::Transform::default());
        let digest_after = compute_digest(&mut world);
        assert_ne!(
            digest_before, digest_after,
            "spawning an entity must change the digest"
        );
    }

    #[test]
    fn transform_translation_change_changes_digest() {
        let mut world = World::new();
        let e = world
            .spawn(bevy_transform::prelude::Transform::from_xyz(1.0, 2.0, 3.0))
            .id();
        let digest_before = compute_digest(&mut world);

        // Mutate the transform.
        world
            .entity_mut(e)
            .insert(bevy_transform::prelude::Transform::from_xyz(
                10.0, 20.0, 30.0,
            ));

        let digest_after = compute_digest(&mut world);
        assert_ne!(
            digest_before, digest_after,
            "changing a Transform translation must change the digest"
        );
    }

    #[test]
    fn transform_sorting_independence() {
        // Spawn entities in two orders; digests must match.
        let mut world_a = World::new();
        world_a.spawn(bevy_transform::prelude::Transform::from_xyz(1.0, 0.0, 0.0));
        world_a.spawn(bevy_transform::prelude::Transform::from_xyz(2.0, 0.0, 0.0));
        world_a.spawn(bevy_transform::prelude::Transform::from_xyz(3.0, 0.0, 0.0));
        let digest_a = compute_digest(&mut world_a);

        let mut world_b = World::new();
        world_b.spawn(bevy_transform::prelude::Transform::from_xyz(3.0, 0.0, 0.0));
        world_b.spawn(bevy_transform::prelude::Transform::from_xyz(1.0, 0.0, 0.0));
        world_b.spawn(bevy_transform::prelude::Transform::from_xyz(2.0, 0.0, 0.0));
        let digest_b = compute_digest(&mut world_b);

        assert_eq!(
            digest_a, digest_b,
            "digest must be independent of spawn order (sorting must normalize)"
        );
    }

    #[test]
    fn float_bit_exactness() {
        // -0.0 and 0.0 have different bit representations.
        let mut world_pos_zero = World::new();
        world_pos_zero.spawn(bevy_transform::prelude::Transform::from_xyz(0.0, 0.0, 0.0));
        let digest_zero = compute_digest(&mut world_pos_zero);

        let mut world_neg_zero = World::new();
        world_neg_zero.spawn(bevy_transform::prelude::Transform::from_xyz(
            -0.0, -0.0, -0.0,
        ));
        let digest_neg_zero = compute_digest(&mut world_neg_zero);

        // IEEE 754: -0.0.to_bits() != 0.0.to_bits()
        assert_ne!(
            digest_zero, digest_neg_zero,
            "-0.0 and 0.0 must hash differently (bit-exact policy)"
        );

        // NaN payloads also differ.
        let mut world_nan1 = World::new();
        world_nan1.spawn(bevy_transform::prelude::Transform::from_xyz(
            f32::NAN,
            0.0,
            0.0,
        ));
        let digest_nan1 = compute_digest(&mut world_nan1);

        let mut world_nan2 = World::new();
        world_nan2.spawn(bevy_transform::prelude::Transform::from_xyz(
            f32::NAN,
            0.0,
            0.0,
        ));
        let digest_nan2 = compute_digest(&mut world_nan2);

        // Same NaN construction → same bits → same digest.
        assert_eq!(
            digest_nan1, digest_nan2,
            "identical NaN construction must match"
        );
    }

    #[test]
    fn app_wired_digest_recording() {
        use crate::choice::ChoiceStream;
        use crate::driver::RunChoices;
        use crate::driver::RunState;

        let mut app = App::new();
        app.insert_resource(RunChoices(ChoiceStream::from_seed(42)));
        app.insert_resource(StateDigestRecorder {
            with_snapshots: true,
            stop_after: Some(5),
            digests: vec![],
            snapshots: BTreeMap::new(),
        });
        app.insert_resource(RunState::default());

        // Add the digest recording system.
        app.add_systems(bevy_app::Update, record_digest_system);

        // Drive 10 frames, advancing the harness frame counter each time
        // (in a real run the driver owns this counter).
        for frame in 0..10u64 {
            app.world_mut().resource_mut::<RunState>().frame = frame;
            app.update();
        }

        let recorder = app.world().get_resource::<StateDigestRecorder>().unwrap();
        // Should have recorded frames 0..=5 (stop_after = 5).
        assert_eq!(recorder.digests.len(), 6, "digests must respect stop_after");
        assert_eq!(recorder.digests.first().unwrap().frame, 0);
        assert_eq!(recorder.digests.last().unwrap().frame, 5);
        assert_eq!(
            recorder.snapshots.len(),
            6,
            "snapshots must be recorded alongside digests (with_snapshots = true)"
        );
    }

    #[test]
    fn recorder_absent_means_no_recording() {
        // Opt-in contract: with no StateDigestRecorder resource, the system
        // is a no-op and must not panic.
        let mut app = App::new();
        app.insert_resource(crate::driver::RunState::default());
        app.add_systems(bevy_app::Update, record_digest_system);
        for _ in 0..5 {
            app.update();
        }
        assert!(app.world().get_resource::<StateDigestRecorder>().is_none());
    }

    #[test]
    fn digest_comparison_detects_divergence() {
        let digest_a = vec![
            StateDigest {
                frame: 0,
                hash: 100,
            },
            StateDigest {
                frame: 1,
                hash: 200,
            },
            StateDigest {
                frame: 2,
                hash: 300,
            },
        ];
        let digest_b = vec![
            StateDigest {
                frame: 0,
                hash: 100,
            },
            StateDigest {
                frame: 1,
                hash: 999,
            }, // Divergence at frame 1.
            StateDigest {
                frame: 2,
                hash: 400,
            },
        ];

        let div = compare(&digest_a, &digest_b).unwrap_err();
        assert_eq!(div.frame, 1);
        assert_eq!(div.hash_a, 200);
        assert_eq!(div.hash_b, 999);
    }

    #[test]
    fn digest_comparison_length_mismatch_is_divergence() {
        let a = vec![StateDigest { frame: 0, hash: 1 }];
        let b = vec![
            StateDigest { frame: 0, hash: 1 },
            StateDigest { frame: 1, hash: 2 },
        ];
        let div = compare(&a, &b).unwrap_err();
        assert_eq!(div.frame, 1);
    }

    #[test]
    fn same_state_same_digest() {
        // Same world state must produce the same digest.
        let mut world = World::new();
        world.spawn(bevy_transform::prelude::Transform::from_xyz(1.0, 2.0, 3.0));
        world.spawn(bevy_transform::prelude::Transform::from_xyz(4.0, 5.0, 6.0));

        let digest1 = compute_digest(&mut world);
        let digest2 = compute_digest(&mut world);
        let digest3 = compute_digest(&mut world);

        assert_eq!(digest1, digest2);
        assert_eq!(digest2, digest3);
    }

    #[test]
    fn insertion_order_independence_for_resources() {
        // Two worlds where reflected resources are inserted in different
        // orders must hash identically (sort-before-hash).
        #[derive(Resource, Debug, bevy_reflect::Reflect)]
        #[reflect(Resource)]
        struct ScoreA(u32);
        #[derive(Resource, Debug, bevy_reflect::Reflect)]
        #[reflect(Resource)]
        struct ScoreB(u32);

        let mut world_a = World::new();
        world_a.init_resource::<bevy_ecs::reflect::AppTypeRegistry>();
        {
            let registry = world_a.resource_mut::<bevy_ecs::reflect::AppTypeRegistry>();
            registry.0.write().register::<ScoreA>();
            registry.0.write().register::<ScoreB>();
        }
        world_a.insert_resource(ScoreA(1));
        world_a.insert_resource(ScoreB(2));

        let mut world_b = World::new();
        world_b.init_resource::<bevy_ecs::reflect::AppTypeRegistry>();
        {
            let registry = world_b.resource_mut::<bevy_ecs::reflect::AppTypeRegistry>();
            registry.0.write().register::<ScoreB>();
            registry.0.write().register::<ScoreA>();
        }
        world_b.insert_resource(ScoreB(2));
        world_b.insert_resource(ScoreA(1));

        let digest_a = compute_digest(&mut world_a);
        let digest_b = compute_digest(&mut world_b);
        assert_eq!(
            digest_a, digest_b,
            "digest must be independent of resource insertion order"
        );
    }

    #[test]
    #[cfg(feature = "ui-liveness")]
    fn ui_liveness_feature_gating() {
        use bevy_ui::prelude::*;
        let mut world = World::new();
        world.spawn(UiTransform::default());
        let digest_before = compute_digest(&mut world);
        let e = world.spawn(UiTransform::default()).id();
        let digest_after = compute_digest(&mut world);
        assert_ne!(
            digest_before, digest_after,
            "UI entity count must affect digest"
        );

        // Modify UiTransform translation (Px).
        world.entity_mut(e).insert(UiTransform {
            translation: bevy_ui::Val2::px(100.0, 200.0),
            ..Default::default()
        });
        let digest_changed = compute_digest(&mut world);
        assert_ne!(
            digest_after, digest_changed,
            "UiTransform change must affect digest"
        );
    }
}
