//! Enumeration of the declared UI/interactable surface.
//!
//! A pure `fn(&World)` walk — never a registered system — so it
//! stays invisible to `static_scan`'s reader census and the state
//! digest. The driver captures it once after the first frame (see
//! `driver.rs`) and attaches it to the playtest report as
//! `ui_surface`; the chaos mouse bot additionally uses enumerated
//! widget centers as click targets on half its draws.
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
//! - Anything that comes into existence after the first settled
//!   frame: the driver snapshots this enumeration ONCE, so menus,
//!   dialogs, and tooltips spawned in response to input (or after a
//!   loading screen) are absent from the report AND unreachable by
//!   the chaos mouse bot's targeted clicks. The declared surface is
//!   an authoring-time property only for statically-spawned UI.
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
    // because the driver snapshots it once, after the first settled
    // frame — any surface that comes into existence later (menus,
    // dialogs, tooltips spawned in response to input). Absent
    // entries prove nothing (see module docs).
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
        "snapshot-in-time: the driver captures this once after the first \
         settled frame; surfaces spawned later (menus, dialogs) are absent"
            .to_string(),
    );
    result.interactables = interactables;
    result.complete = false;
    result
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
