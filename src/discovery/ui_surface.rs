//! Enumeration of the declared UI/interactable surface.
//!
//! A pure `fn(&World)` walk — never a registered system — so it
//! stays invisible to `static_scan`'s reader census and the state
//! digest. The driver captures it after the first settled frame and
//! RE-CAPTURES it periodically (`UiSurfaceTracker`, a fixed tick
//! interval ~twice a second — see `driver.rs`); the report carries
//! the LATEST snapshot plus bounded delta notes as `ui_surface`,
//! and the chaos mouse bot additionally uses the current snapshot's
//! widget centers as click targets on half its draws — refreshed
//! on every recapture, so it lags the world by at most one
//! interval.
//!
//! Sources of declaration, all unions (an entity may be declared
//! interactable by several independent signals):
//!
//! 1. `Pickable` — the explicit `bevy_picking` opt-in marker.
//! 2. `bevy_ui_widgets` widget components — `Button`, `Checkbox`,
//!    `RadioButton`, `Slider`, `Tab`, `TextInput`, `ListBox`,
//!    `MenuItem`, `MenuButton`, `ScrollArea` (see [`WidgetRole`]).
//! 3. `TabIndex` — keyboard-focusable (`bevy_input_focus` 0.20's
//!    focusable marker; the tab/directional navigation systems key
//!    off it).
//! 4. `AccessibilityNode` / `AccessibleLabel` — a semantic
//!    role/label declaration. Classification beyond "accessible"
//!    would require reading the internal `Arc<ActionRequestWrapper>`
//!    payload, deliberately not done: an a11y node declares
//!    interactivity, not a precise widget role, in this iteration.
//! 5. Pointer-event observers — `world.observers()` exposes the
//!    centralized observer storage; watching `PointerClick`,
//!    `PointerPress`, or `PointerRelease` on a *specific entity* is a
//!    declaration of click-responsiveness, even with no markers.
//!
//! Geometry mirrors `bevy_ui::picking_backend::ui_picking` (verified
//! against 0.20.1): `UiGlobalTransform` is in *viewport space of the
//! node's target camera* — to yield window-space physical pixels we
//! add the camera's `physical_viewport_rect().min` back, the inverse
//! of the correction `ui_picking` applies to cursor positions. Nodes
//! are skipped for visibility exactly when `ui_picking` would skip
//! them (zero size, `InheritedVisibility` false).
//!
//! Deliberately omitted (documented blind spots — [`UiSurfaceResult`]
//! therefore reports `complete: false` and carries the same notes, so
//! absent entries prove nothing):
//!
//! - Global and component-scoped pointer-event observers (their
//!   firing targets are unattributable at scan time without walking
//!   all archetypes; fabricating attribution would invent
//!   declarations).
//! - `Interaction`/`Button` (deprecated in 0.20, scheduled for
//!   removal — new code should not build on it).
//! - Third-party action-map crates (LWIM is served by the separate
//!   `lw-reader` declared tier; other ecosystems need their own
//!   reflection-based readers).
//! - Anything that comes into existence between the driver's
//!   periodic recaptures: the driver re-enumerates roughly twice a
//!   second (first capture after the first settled frame, then a
//!   fixed tick interval), so menus, dialogs, and tooltips spawned
//!   in response to input usually join the surface shortly after
//!   they appear — but a surface that exists for less than one
//!   recapture interval can still slip through unobserved. Absent
//!   entries prove nothing (fail-open).
//!
//! Geometry caveats: `rect` is an axis-aligned bounding box — the
//! CENTER is exact for rotated/scaled nodes (translation is the node
//! center), but the rect approximates the hit region. Neither takes
//! clipping (`CalculatedClip`, e.g. children scrolled out of a
//! `ScrollArea`) or occlusion (e.g. modal overlays) into account: a
//! reported center may fall where the real hit test would miss.
//! Centers are
//! resolved in each widget's target-camera window space while the
//! bot delivers clicks to the PRIMARY window; widgets on secondary
//! windows report valid centers the bot cannot reach (the same
//! primary-window assumption governs all bot delivery).
//!
//! Forward hazard (Bevy 0.21, bevy#25675): `TabIndex` grows a
//! `Focusable` marker via `#[require]`, so worlds built against it
//! can hold focusable entities WITHOUT an explicit `TabIndex` — this
//! enumeration under-reports there. Bump-and-check when the harness
//! moves past 0.20.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};

use bevy_a11y::AccessibilityNode;
use bevy_camera::{visibility::InheritedVisibility, Camera};
use bevy_ecs::entity::Entity;
use bevy_ecs::prelude::*;
use bevy_ecs::query::QueryState;
use bevy_input_focus::tab_navigation::TabIndex;
use bevy_math::Vec2;
use bevy_picking::events::{PointerClick, PointerPress, PointerRelease};
use bevy_picking::Pickable;
use bevy_ui::{
    prelude::AccessibleLabel, widget::Text, ComputedNode, ComputedUiTargetCamera,
    InteractionDisabled as UiInteractionDisabled, UiGlobalTransform,
};
use bevy_ui_widgets::{
    Button, Checkbox, ListBox, MenuButton, MenuItem, RadioButton, ScrollArea, Slider, Tab,
    TextInput,
};
use serde::{Deserialize, Serialize};

/// How an interactable was declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractableKind {
    /// Explicit `Pickable` component (`bevy_picking` opt-in).
    Pickable,
    /// A `bevy_ui_widgets` widget component (Button, Checkbox, ...).
    Widget,
    /// Keyboard-focusable via `TabIndex` (`bevy_input_focus`).
    Focusable,
    /// Carries an accessibility node (role + label): a semantic
    /// declaration of interactivity.
    Accessible,
    /// Has an observer watching at least one pointer event type
    /// (`PointerClick`, `PointerPress`, ...). Declarative by
    /// construction: someone wrote code reacting to clicks here.
    Observed,
}

impl InteractableKind {
    /// Stable snake-case name, useful for serialized consumers.
    pub fn as_str(self) -> &'static str {
        match self {
            InteractableKind::Pickable => "pickable",
            InteractableKind::Widget => "widget",
            InteractableKind::Focusable => "focusable",
            InteractableKind::Accessible => "accessible",
            InteractableKind::Observed => "observed",
        }
    }
}

/// Best-effort semantic classification of an interactable, derived
/// from known widget components. `Unknown` means "some declared
/// surface exists but its role is not standardized".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WidgetRole {
    /// A `bevy_ui_widgets::Button`.
    Button,
    /// A `bevy_ui_widgets::Checkbox`.
    Checkbox,
    /// A `bevy_ui_widgets::RadioButton`.
    Radio,
    /// A `bevy_ui_widgets::Slider`.
    Slider,
    /// A `bevy_ui_widgets::Tab`.
    Tab,
    /// A `bevy_ui_widgets::TextInput`.
    TextInput,
    /// A `bevy_ui_widgets::ListBox`.
    ListBox,
    /// A `bevy_ui_widgets::MenuItem`.
    MenuItem,
    /// A `bevy_ui_widgets::MenuButton`.
    MenuButton,
    /// A `bevy_ui_widgets::ScrollArea`.
    ScrollArea,
    /// Declared surface with no recognized widget component.
    Unknown,
}

impl WidgetRole {
    /// Stable snake-case name, useful for serialized consumers.
    pub fn as_str(self) -> &'static str {
        match self {
            WidgetRole::Button => "button",
            WidgetRole::Checkbox => "checkbox",
            WidgetRole::Radio => "radio",
            WidgetRole::Slider => "slider",
            WidgetRole::Tab => "tab",
            WidgetRole::TextInput => "text_input",
            WidgetRole::ListBox => "list_box",
            WidgetRole::MenuItem => "menu_item",
            WidgetRole::MenuButton => "menu_button",
            WidgetRole::ScrollArea => "scroll_area",
            WidgetRole::Unknown => "unknown",
        }
    }
}

/// One declared interactive surface point.
///
/// Positions are **physical pixels, window-space** (origin top-left,
/// y-down) — the same space a native cursor event reports. Consumers
/// clicking here approximately reproduce picking (the driver's bot
/// converts to logical pixels via the window's `scale_factor`), but
/// the click does not account for clipping (`CalculatedClip`, e.g.
/// scrolled-out children) or occlusion (e.g. modal overlays) — see
/// the module's geometry caveats.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Interactable {
    /// Generational entity id (serialization requires the
    /// `bevy_ecs/serialize` feature, chained by `ui-surface`).
    pub entity: Entity,
    /// Every distinct declaration signal found on this entity,
    /// unioned. An entity can be both `Widget` and `Focusable`.
    pub kinds: Vec<InteractableKind>,
    /// Semantic role if identifiable from a known widget component.
    pub role: WidgetRole,
    /// Screen-space center in physical pixels, if resolvable.
    pub center: Option<Vec2>,
    /// Screen-space axis-aligned bounding rect (min/max corners,
    /// physical pixels), if resolvable. Always in window space.
    pub rect: Option<Rect>,
    /// Disabled flag: `InteractionDisabled` (`bevy_ui`) present.
    pub disabled: bool,
    /// Best-effort label: accessible label > child `Text` text > `Name`.
    pub label: Option<String>,
    /// Currently keyboard-focused (input focus resource points here).
    pub focused: bool,
}

/// Screen-space rectangle, min corner inclusive, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    /// Top-left corner of the rect.
    pub min: Vec2,
    /// Bottom-right corner of the rect.
    pub max: Vec2,
}

/// Result of enumerating the UI/interactable surface.
///
/// Honest-declaration doctrine (the enumeration serves fail-open
/// consumers): it reports what the game *declares*, not what is
/// guaranteed to work — a declared interactable may still be dead
/// code; consumers must verify liveness empirically. Counts and
/// lists are annotations, never licenses to prune probe surfaces.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiSurfaceResult {
    /// Every declared interactable found, ordered deterministically
    /// by entity index.
    pub interactables: Vec<Interactable>,
    /// Whether the enumeration saw every signal source it knows
    /// about: `false` means the result provably missed possible
    /// declarations (fail-open: absent ≠ non-existent).
    pub complete: bool,
    /// Non-fatal caveats discovered during enumeration.
    pub notes: Vec<String>,
}

/// The pointer events whose observers declare click-responsiveness.
///
/// Deliberately only press/click/release: hover/scroll observers mark
/// attention-tracking, not actionability — the distinction matters
/// because the enumeration's purpose is answering "what can I DO".
fn pointer_observers(world: &World) -> HashMap<Entity, usize> {
    let mut result = HashMap::new();
    let pointer_event_keys = [
        world.event_key::<PointerClick>(),
        world.event_key::<PointerPress>(),
        world.event_key::<PointerRelease>(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();

    let observers = world.observers();
    for key in pointer_event_keys {
        let Some(cached) = observers.try_get_observers(key) else {
            continue;
        };
        // Entity-scoped observers fire ONLY for that entity: safe to
        // attribute. Global/component observers are omitted (see
        // module docs) — under-attribution is the honest default.
        for (entity, map) in cached.entity_observers() {
            *result.entry(*entity).or_insert(0) += map.len();
        }
    }
    result
}

/// Semantic role from `bevy_ui_widgets` marker components, resolved by a
/// precedence-ordered probe table.
///
/// Precedence resolves multi-widget entities (a menu button is both
/// `MenuButton` and `Button`): the more specific role wins.
type MarkerProbe = fn(&World, Entity) -> bool;

const ROLE_PROBES: &[(WidgetRole, MarkerProbe)] = &[
    (WidgetRole::MenuItem, |w, e| w.get::<MenuItem>(e).is_some()),
    (WidgetRole::MenuButton, |w, e| {
        w.get::<MenuButton>(e).is_some()
    }),
    (WidgetRole::Checkbox, |w, e| w.get::<Checkbox>(e).is_some()),
    (WidgetRole::Radio, |w, e| w.get::<RadioButton>(e).is_some()),
    (WidgetRole::Slider, |w, e| w.get::<Slider>(e).is_some()),
    (WidgetRole::TextInput, |w, e| {
        w.get::<TextInput>(e).is_some()
    }),
    (WidgetRole::Tab, |w, e| w.get::<Tab>(e).is_some()),
    (WidgetRole::ListBox, |w, e| w.get::<ListBox>(e).is_some()),
    (WidgetRole::ScrollArea, |w, e| {
        w.get::<ScrollArea>(e).is_some()
    }),
    (WidgetRole::Button, |w, e| w.get::<Button>(e).is_some()),
];

/// Resolve the [`WidgetRole`] of an entity from its widget marker
/// components; `Unknown` when no known marker is present.
fn role_for_entity(world: &World, entity: Entity) -> WidgetRole {
    ROLE_PROBES
        .iter()
        .find_map(|&(role, probe)| probe(world, entity).then_some(role))
        .unwrap_or(WidgetRole::Unknown)
}

/// Enumerate every declared interactable in the world.
///
/// Call on a booted app; after at least one `app.update()` so
/// `UiStack` and `ComputedNode` exist (layout-dependent surfaces only
/// appear after layout has run at least once). See
/// [`UiSurfaceResult::complete`] for what the enumeration provably
/// saw — and the module docs for what it provably did not.
pub fn enumerate_ui(world: &World) -> UiSurfaceResult {
    let mut result = UiSurfaceResult::default();
    let observed = pointer_observers(world);

    let focused: Option<Entity> = world
        .get_resource::<bevy_input_focus::InputFocus>()
        .and_then(bevy_input_focus::InputFocus::get);

    // `QueryState::try_new` works from `&World` (immutable, safe). It
    // returns `None` when a queried component is not registered in
    // this world — in that case no entity can possibly carry it, so an
    // empty set is both correct and complete (not an incompleteness).
    let widget_entities: HashSet<Entity> = QueryState::<
        Entity,
        Or<(
            With<Button>,
            With<Checkbox>,
            With<RadioButton>,
            With<Slider>,
            With<Tab>,
            With<TextInput>,
            With<ListBox>,
            With<MenuItem>,
            With<MenuButton>,
            With<ScrollArea>,
        )>,
    >::try_new(world)
    .map(|mut state| state.iter(world).collect())
    .unwrap_or_default();
    let pickable_entities: HashSet<Entity> = QueryState::<Entity, With<Pickable>>::try_new(world)
        .map(|mut state| state.iter(world).collect())
        .unwrap_or_default();
    let tabindex_entities: HashSet<Entity> = QueryState::<Entity, With<TabIndex>>::try_new(world)
        .map(|mut state| state.iter(world).collect())
        .unwrap_or_default();
    let a11y_entities: HashSet<Entity> =
        QueryState::<Entity, Or<(With<AccessibilityNode>, With<AccessibleLabel>)>>::try_new(world)
            .map(|mut state| state.iter(world).collect())
            .unwrap_or_default();

    let mut declared: HashSet<Entity> = HashSet::new();
    declared.extend(widget_entities.iter().copied());
    declared.extend(pickable_entities.iter().copied());
    declared.extend(tabindex_entities.iter().copied());
    declared.extend(a11y_entities.iter().copied());
    declared.extend(observed.keys().copied());

    let mut viewport_min_cache: HashMap<Entity, Option<Vec2>> = HashMap::new();
    let mut interactables: Vec<Interactable> = Vec::with_capacity(declared.len());

    // Deterministic order: ascending entity index.
    let mut ordered: Vec<Entity> = declared.into_iter().collect();
    ordered.sort_unstable_by_key(|e| e.index());

    for entity in ordered {
        // Existence of declaration data is not enough — the entity
        // must also carry a UI node for geometry purposes.
        let Some(computed) = world.get::<ComputedNode>(entity) else {
            // Declared but not a laid-out UI node (e.g. a world-space
            // entity with an observer). Report it as a surface point
            // with unknown geometry.
            let kinds = empty_kinds_for(
                entity,
                &widget_entities,
                &pickable_entities,
                &tabindex_entities,
                &a11y_entities,
                &observed,
            );
            interactables.push(Interactable {
                entity,
                kinds,
                role: role_for_entity(world, entity),
                center: None,
                rect: None,
                disabled: world.get::<UiInteractionDisabled>(entity).is_some(),
                label: best_label(world, entity),
                focused: focused == Some(entity),
            });
            continue;
        };
        let Some(transform) = world.get::<UiGlobalTransform>(entity) else {
            let kinds = empty_kinds_for(
                entity,
                &widget_entities,
                &pickable_entities,
                &tabindex_entities,
                &a11y_entities,
                &observed,
            );
            interactables.push(Interactable {
                entity,
                kinds,
                role: role_for_entity(world, entity),
                center: None,
                rect: None,
                disabled: world.get::<UiInteractionDisabled>(entity).is_some(),
                label: best_label(world, entity),
                focused: focused == Some(entity),
            });
            continue;
        };

        // Visibility doctrine mirrors `ui_picking`: hidden nodes are
        // not actionable and are excluded entirely.
        if world
            .get::<InheritedVisibility>(entity)
            .is_some_and(|vis| !vis.get())
        {
            continue;
        }
        // Zero-size nodes cannot be hit; `ui_picking` skips them.
        if computed.size == Vec2::ZERO {
            continue;
        }

        let role = role_for_entity(world, entity);
        let kinds = empty_kinds_for(
            entity,
            &widget_entities,
            &pickable_entities,
            &tabindex_entities,
            &a11y_entities,
            &observed,
        );

        // Resolve window-space rect: node position is in target
        // camera viewport space; add the viewport offset. Both fields
        // resolve or neither does (unified failure, no half-answers).
        let geometry = world
            .get::<ComputedUiTargetCamera>(entity)
            .and_then(ComputedUiTargetCamera::get)
            .and_then(|camera_entity| {
                // Viewport offsets are camera-invariant per frame;
                // compute each camera's offset once.
                let offset = match viewport_min_cache.entry(camera_entity) {
                    Entry::Occupied(o) => *o.get(),
                    Entry::Vacant(v) => {
                        let value = world
                            .get::<Camera>(camera_entity)
                            .and_then(Camera::physical_viewport_rect)
                            .map(|r| r.min.as_vec2());
                        v.insert(value);
                        value
                    }
                };
                offset.map(|offset| {
                    let center_win = transform.affine().translation + offset;
                    let half = computed.size * 0.5;
                    (
                        center_win,
                        Rect {
                            min: center_win - half,
                            max: center_win + half,
                        },
                    )
                })
            });
        let (center, rect) = match geometry {
            Some((c, r)) => (Some(c), Some(r)),
            None => (None, None),
        };

        interactables.push(Interactable {
            entity,
            kinds,
            role,
            center,
            rect,
            disabled: world.get::<UiInteractionDisabled>(entity).is_some(),
            label: best_label(world, entity),
            focused: focused == Some(entity),
        });
    }

    // Honest completeness: the enumeration provably misses
    // global/component-scoped pointer observers, deprecated
    // `Interaction`/`Button` widgets, third-party action maps, and —
    // because the driver recaptures on a fixed tick interval — any
    // surface that exists for less than one interval between
    // appearances. Absent entries prove nothing (see module docs).
    result.notes.push(
        "global and component-scoped pointer observers are omitted: their \
         firing targets cannot be attributed honestly at scan time"
            .to_string(),
    );
    result.notes.push(
        "deprecated bevy_ui Interaction/Button widgets are excluded; \
         games still on the legacy marker under-report here"
            .to_string(),
    );
    result
        .notes
        .push("third-party action-map crates need their own readers (see lw-reader)".to_string());
    result.notes.push(
        "periodic capture: the driver recaptures this surface on a fixed tick \
         interval; a surface alive for less than one interval may be missed"
            .to_string(),
    );
    result.notes.push(
        "aim freshness: the chaos mouse targets centers from the latest \
         recapture, so between recaptures it can briefly aim at a moved or \
         despawned widget's old position"
            .to_string(),
    );
    result.interactables = interactables;
    result.complete = false;
    result
}

/// Cap on delta notes the tracker keeps per run. Deltas BEYOND the
/// cap are collapsed into one disclosed-suppression note (refusal
/// over silence: the count is always reported, even when individual
/// deltas are dropped). Keeps report memory bounded for
/// UI-churn-heavy games (spawn/despawn storms must not balloon the
/// report).
pub(crate) const MAX_SURFACE_DELTA_NOTES: usize = 32;

/// Driver-loop recorder for PERIODIC re-enumeration of the declared
/// UI surface (E4 part A).
///
/// Doctrine: invoked from the driver loop only, never a scheduled
/// system — same census-neutrality rule as the one-shot capture it
/// replaces (see the module docs and driver.rs). Cadence is a fixed
/// tick interval derived from `tps` (deterministic), and each
/// recapture diffs the interactable ENTITY SET against the previous
/// snapshot:
///
/// - Deltas compare entity ids ONLY. The `focused` flag and geometry
///   drift by design (focus moves, layout animates) — treating either
///   as a "change" would flood the notes with flicker, so they are
///   deliberately NOT delta signals; the latest snapshot carries the
///   current values.
/// - Every added/removed entity becomes a note (refusal over silence),
///   capped at [`MAX_SURFACE_DELTA_NOTES`]; further deltas collapse
///   into one disclosed count.
/// - The tracked `interactables` vec (and the mirrored
///   `UiClickTargets`) always reflect the LATEST snapshot, so the
///   chaos mouse aims at most one recapture interval behind the
///   world: newly opened menus join its targets on the next
///   recapture, and a despawned widget's old position can be aimed
///   at briefly in between. Freshness is eventual, not instant —
///   and that lag is disclosed (see the enumeration notes).
/// - Memory is bounded: one snapshot + at most
///   [`MAX_SURFACE_DELTA_NOTES`] + 1 notes.
#[derive(Debug, Default)]
pub(crate) struct UiSurfaceTracker {
    /// Latest enumeration snapshot (what the report's `ui_surface`
    /// field will carry).
    snapshot: Option<UiSurfaceResult>,
    /// Entity ids of the last snapshot, for diffing.
    last_ids: HashSet<Entity>,
    /// Delta notes accumulated across recaptures (bounded).
    notes: Vec<String>,
    /// How many delta-note LINES were suppressed past the cap (a
    /// line batches one recapture's whole added or removed set, so
    /// the count is a lower bound on individual entity deltas).
    suppressed: usize,
}

impl UiSurfaceTracker {
    /// Tick interval between recaptures: half a second of
    /// simulated time, minimum 1 tick. Twice a second catches
    /// menu/dialog opens without re-walking the UI tree every
    /// frame (enumeration is a full archetype sweep).
    pub(crate) fn recapture_interval(tps: u64) -> u64 {
        (tps / 2).max(1)
    }

    /// Capture on the FIRST settled frame (tick 0), then on every
    /// interval boundary. Mirrors `UiClickTargets` on every capture.
    ///
    /// `interval` must be ≥ 1 (pass [`Self::recapture_interval`]'s
    /// output — the boundary test divides by it).
    pub(crate) fn observe(
        &mut self,
        world: &World,
        tick: u64,
        interval: u64,
        click_targets: &mut Vec<Vec2>,
    ) {
        debug_assert!(interval > 0, "interval must be >= 1");
        let boundary = tick.is_multiple_of(interval);
        // `is_first` is deliberately explicit even though tick 0 is a
        // multiple of every interval (so production runs always enter
        // the capture block there anyway): it states the first-capture
        // contract independently of the interval math, and unit tests
        // observe from arbitrary ticks — without the guard, a test
        // observing from tick 1 with interval 2 would silently skip
        // its first capture. Not a bug-fix for a live path; intent.
        let is_first = self.snapshot.is_none();
        if !boundary && !is_first {
            return;
        }
        let result = enumerate_ui(world);
        let ids: HashSet<Entity> = result.interactables.iter().map(|e| e.entity).collect();
        if let Some(_prev) = &self.snapshot {
            // Diff against the previous snapshot's entity set.
            let prev = &self.last_ids;
            let added: Vec<Entity> = ids.iter().filter(|e| !prev.contains(e)).copied().collect();
            let removed: Vec<Entity> = prev.iter().filter(|e| !ids.contains(e)).copied().collect();
            if !added.is_empty() || !removed.is_empty() {
                self.record_delta(tick, &added, &removed);
            }
        }
        click_targets.clear();
        click_targets.extend(result.interactables.iter().filter_map(|e| e.center));
        self.last_ids = ids;
        self.snapshot = Some(result);
    }

    /// Turn one recapture diff into notes (bounded, disclosed).
    /// Entities render by their full generational id (`Entity`'s
    /// `Display`, e.g. `42v3`): a bare index would conflate
    /// different generations of the same slot.
    fn record_delta(&mut self, tick: u64, added: &[Entity], removed: &[Entity]) {
        let mut lines: Vec<String> = Vec::new();
        if !added.is_empty() {
            let mut list = added.to_vec();
            list.sort_unstable_by_key(|e| e.index());
            let rendered: Vec<String> = list.iter().map(|e| e.to_string()).collect();
            lines.push(format!(
                "ui surface delta at recapture (tick {tick}): added interactables {}",
                rendered.join(", ")
            ));
        }
        if !removed.is_empty() {
            let mut list = removed.to_vec();
            list.sort_unstable_by_key(|e| e.index());
            let rendered: Vec<String> = list.iter().map(|e| e.to_string()).collect();
            lines.push(format!(
                "ui surface delta at recapture (tick {tick}): removed interactables {}",
                rendered.join(", ")
            ));
        }
        for line in lines {
            if self.notes.len() >= MAX_SURFACE_DELTA_NOTES {
                self.suppressed += 1;
            } else {
                self.notes.push(line);
            }
        }
    }

    /// Final result for the report: the LATEST snapshot with the
    /// delta history first, then the snapshot's own enumeration
    /// notes, and — if any deltas were suppressed — the disclosed
    /// count LAST. Ordering is deterministic. `None` if never
    /// captured: unreachable from a healthy driver loop (which
    /// captures at tick 0) but real when the game panics inside
    /// the very first `app.update()` — the honest shape either
    /// way.
    pub(crate) fn into_result(mut self) -> Option<UiSurfaceResult> {
        let suppressed = self.suppressed;
        let deltas = std::mem::take(&mut self.notes);
        self.snapshot.map(|mut snap| {
            if suppressed > 0 {
                snap.notes.push(format!(
                    "ui surface deltas: {suppressed} more delta notes \
                     suppressed past the cap ({MAX_SURFACE_DELTA_NOTES})"
                ));
            }
            let mut notes = deltas;
            notes.append(&mut snap.notes);
            snap.notes = notes;
            snap
        })
    }
}

/// Kinds for an entity given the pre-collected signal sets.
fn empty_kinds_for(
    entity: Entity,
    widget_entities: &HashSet<Entity>,
    pickable_entities: &HashSet<Entity>,
    tabindex_entities: &HashSet<Entity>,
    a11y_entities: &HashSet<Entity>,
    observed: &HashMap<Entity, usize>,
) -> Vec<InteractableKind> {
    let mut kinds = Vec::new();
    if pickable_entities.contains(&entity) {
        kinds.push(InteractableKind::Pickable);
    }
    if widget_entities.contains(&entity) {
        kinds.push(InteractableKind::Widget);
    }
    if tabindex_entities.contains(&entity) {
        kinds.push(InteractableKind::Focusable);
    }
    if a11y_entities.contains(&entity) {
        kinds.push(InteractableKind::Accessible);
    }
    if observed.contains_key(&entity) {
        kinds.push(InteractableKind::Observed);
    }
    kinds
}

/// Best-effort label: `AccessibleLabel` > descendant `Text` (one
/// level) > `Name` component.
fn best_label(world: &World, entity: Entity) -> Option<String> {
    if let Some(label) = world.get::<AccessibleLabel>(entity) {
        return Some(label.0.clone());
    }
    // Direct child text.
    if let Some(children) = world.get::<Children>(entity) {
        for child in children {
            if let Some(text) = world.get::<Text>(*child) {
                return Some(text.0.clone());
            }
        }
    }
    world.get::<Name>(entity).map(|n| n.as_str().to_owned())
}

#[cfg(test)]
mod tracker_tests {
    //! The tracker is the E4 periodic-recapture contract; these
    //! tests pin cadence, first-capture, delta notes, the note cap
    //! with disclosed suppression, and click-target mirroring.
    //! They drive `observe` with a real Bevy `World` (widget spawns
    //! mid-"run") because the contract is about entity sets, not
    //! mockable inputs.
    use super::*;

    fn test_world() -> World {
        // Minimal resources `enumerate_ui` tolerates absent (all its
        // queries are try_new-based; InputFocus missing = nobody
        // focused), so an empty world enumerates zero interactables.
        World::new()
    }

    fn spawn_pickable(world: &mut World) -> Entity {
        // Any entity registering as "declared" works; Pickable is
        // the cheapest unambiguous signal. ComputedNode absent =>
        // reported as a surface point without geometry (fine: the
        // tracker only diffs entity ids).
        world.spawn(Pickable::default()).id()
    }

    #[test]
    fn first_capture_on_any_tick_before_interval_boundary() {
        // The first observe() must capture regardless of tick
        // position (the driver relies on capture-after-first-frame
        // semantics: tick 0 in practice, but the tracker itself only
        // needs is_first precedence over the cadence gate).
        let world = test_world();
        let mut tracker = UiSurfaceTracker::default();
        let mut targets: Vec<Vec2> = Vec::new();
        tracker.observe(&world, 7, 30, &mut targets);
        assert!(tracker.snapshot.is_some(), "first observe must capture");
    }

    #[test]
    fn recaptures_only_on_interval_boundaries_after_first() {
        let world = test_world();
        let mut tracker = UiSurfaceTracker::default();
        let mut targets: Vec<Vec2> = Vec::new();
        tracker.observe(&world, 0, 30, &mut targets);
        // Mutate the world BETWEEN captures: the tracker must not
        // notice it off-boundary (that's the whole point of the
        // cadence — a full archetype sweep every frame would be
        // needlessly expensive).
        let mut world = world;
        let spawned = spawn_pickable(&mut world);
        tracker.observe(&world, 1, 30, &mut targets);
        assert!(
            tracker.last_ids.is_empty(),
            "off-boundary observe must not recapture"
        );
        tracker.observe(&world, 30, 30, &mut targets);
        assert_eq!(
            tracker.last_ids,
            HashSet::from([spawned]),
            "boundary observe must recapture and diff"
        );
    }

    #[test]
    fn added_and_removed_entities_become_notes() {
        let mut world = test_world();
        let a = spawn_pickable(&mut world);
        let mut tracker = UiSurfaceTracker::default();
        let mut targets: Vec<Vec2> = Vec::new();
        tracker.observe(&world, 0, 30, &mut targets);
        // Remove `a`, add `b`.
        world.despawn(a);
        let b = spawn_pickable(&mut world);
        tracker.observe(&world, 30, 30, &mut targets);
        assert!(tracker
            .notes
            .iter()
            .any(|n| n.contains(&format!("added interactables {b}"))));
        assert!(tracker
            .notes
            .iter()
            .any(|n| n.contains(&format!("removed interactables {a}"))));

        // And the final result carries the delta notes plus the
        // latest snapshot's interactables.
        let result = tracker.into_result().expect("captured");
        assert_eq!(
            result
                .interactables
                .iter()
                .map(|e| e.entity)
                .collect::<Vec<_>>(),
            vec![b],
            "the latest snapshot must reflect the post-delta world"
        );
        assert!(result
            .notes
            .iter()
            .any(|n| n.starts_with("ui surface delta at recapture (tick 30): added")));
    }

    #[test]
    fn delta_notes_past_the_cap_are_suppressed_but_counted() {
        // The suppression must be DISCLOSED in the final result.
        let mut world = test_world();
        let mut tracker = UiSurfaceTracker::default();
        let mut targets: Vec<Vec2> = Vec::new();
        tracker.observe(&world, 0, 1, &mut targets);
        // Interval 1 => every tick is a boundary; each pair of
        // mutations generates 2 delta notes (added + removed).
        let mut pairs_spawned: Vec<Entity> = Vec::new();
        for tick in 1..=(MAX_SURFACE_DELTA_NOTES as u64 + 10) * 2 {
            let e = spawn_pickable(&mut world);
            pairs_spawned.push(e);
            tracker.observe(&world, tick, 1, &mut targets);
        }
        // More than MAX_SURFACE_DELTA_NOTES boundaries fired with
        // deltas: individual notes cap out, suppressed > 0.
        assert!(
            tracker.notes.len() <= MAX_SURFACE_DELTA_NOTES,
            "note history must stay bounded"
        );
        assert!(tracker.suppressed > 0);
        let result = tracker.into_result().expect("captured");
        assert!(
            result
                .notes
                .iter()
                .any(|n| n.contains("delta notes suppressed past the cap")),
            "suppression must be disclosed: {:?}",
            result.notes
        );
        assert!(result.notes.len() <= MAX_SURFACE_DELTA_NOTES + 1 + 10);
    }

    #[test]
    fn click_targets_carry_geometry_from_the_latest_snapshot() {
        // Findings 3+4 from review: the Some(center) → click-targets
        // path had ZERO unit coverage (bare-World tests can never
        // resolve geometry — ComputedUiTargetCamera is only written
        // by UiPlugin's prepare systems). Bootstrap the same
        // headless layout app as tests/it/ui_surface.rs (dev-deps make
        // the text/asset/image/mesh stacks legal in lib tests), so
        // the mirroring contract is pinned with a real center —
        // and the despawn path visibly EMPTIES the target vec
        // (mutation: "keep stale centers" must fail here).
        use bevy_app::prelude::*;
        use bevy_camera::{Camera, Camera2d, ComputedCameraValues, RenderTargetInfo, Viewport};
        use bevy_input::prelude::*;
        use bevy_math::UVec2;
        use bevy_transform::prelude::*;
        use bevy_ui::{prelude::UiScale, UiPlugin};
        use bevy_ui_widgets as widgets;
        use bevy_window::WindowPlugin;

        let mut app = App::new();
        app.add_plugins((
            TaskPoolPlugin::default(),
            bevy_asset::AssetPlugin::default(),
            WindowPlugin::default(),
            TransformPlugin,
            bevy_text::TextPlugin,
            UiPlugin,
            bevy_camera::visibility::VisibilityPlugin,
            bevy_input_focus::InputFocusPlugin,
            bevy_picking::InteractionPlugin,
            bevy_picking::PickingPlugin,
            widgets::UiWidgetsPlugins,
        ));
        bevy_asset::AssetApp::init_asset::<bevy_image::Image>(&mut app);
        bevy_asset::AssetApp::init_asset::<bevy_image::TextureAtlasLayout>(&mut app);
        bevy_asset::AssetApp::init_asset::<bevy_mesh::Mesh>(&mut app);
        bevy_asset::AssetApp::init_asset::<bevy_mesh::skinning::SkinnedMeshInverseBindposes>(
            &mut app,
        );
        app.init_resource::<bevy_time::Time<bevy_time::Real>>();
        app.init_resource::<UiScale>();
        app.init_resource::<ButtonInput<MouseButton>>();
        app.init_resource::<ButtonInput<KeyCode>>();
        app.init_resource::<Touches>();
        app.init_resource::<bevy_input_focus::InputFocus>();
        app.init_resource::<bevy_input_focus::InputFocusVisible>();

        let width = 1280u32;
        let height = 720u32;
        app.world_mut().spawn((
            Camera2d,
            Camera {
                computed: ComputedCameraValues {
                    target_info: Some(RenderTargetInfo {
                        physical_size: UVec2::new(width, height),
                        scale_factor: 1.,
                    }),
                    ..Default::default()
                },
                viewport: Some(Viewport {
                    physical_size: UVec2::new(width, height),
                    ..Default::default()
                }),
                ..Default::default()
            },
        ));

        // A Pickable node at (300, 300), 200x100: its window-space
        // center must resolve to (400, 350) (y-down, offset (0,0)).
        let node = app
            .world_mut()
            .spawn((
                bevy_ui::Node {
                    width: bevy_ui::Val::Px(200.),
                    height: bevy_ui::Val::Px(100.),
                    position_type: bevy_ui::PositionType::Absolute,
                    left: bevy_ui::Val::Px(300.),
                    top: bevy_ui::Val::Px(300.),
                    ..bevy_utils::default()
                },
                Pickable::default(),
            ))
            .id();
        // Let layout settle (ComputedNode/UiGlobalTransform/
        // ComputedUiTargetCamera are written by UiPlugin's systems).
        for _ in 0..2 {
            app.update();
        }

        let mut tracker = UiSurfaceTracker::default();
        let mut targets: Vec<Vec2> = Vec::new();
        {
            let world = app.world();
            tracker.observe(world, 0, 2, &mut targets);
        }
        assert_eq!(
            targets.len(),
            1,
            "the laid-out node's center must land in the click targets"
        );
        let center = targets[0];
        assert!(
            (center.x - 400.).abs() < 0.5 && (center.y - 350.).abs() < 0.5,
            "expected center (400, 350), got {center:?}"
        );

        // Off-boundary observe must leave targets untouched.
        {
            let world = app.world();
            tracker.observe(world, 1, 2, &mut targets);
        }
        assert_eq!(
            targets.len(),
            1,
            "off-boundary observe must not touch targets"
        );

        // Despawn between boundaries: the NEXT boundary recapture
        // must visibly empty the vec — stale centers must not
        // survive (the load-bearing mirroring regression).
        app.world_mut().despawn(node);
        {
            let world = app.world();
            tracker.observe(world, 2, 2, &mut targets);
        }
        assert!(
            targets.is_empty(),
            "despawned widget's center must leave the click targets: {targets:?}"
        );
        assert!(tracker.last_ids.is_empty());
        // And the removal was REPORTED (refusal over silence).
        assert!(
            tracker
                .notes
                .iter()
                .any(|n| n.contains(&format!("removed interactables {node}"))),
            "the despawn must be a delta note: {:?}",
            tracker.notes
        );
    }
}
