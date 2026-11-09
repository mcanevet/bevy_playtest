//! Declared-tier reader for Leafwing Input Manager 0.22 (Bevy ^0.20)
//! — the D1-d "declared tier".
//!
//! Three cooperating capabilities, one module:
//!
//! 1. **Reader** ([`read_declared`]): enumerate EVERY `InputMap` in a
//!    running game — RON-loaded and code-built alike — through pure
//!    reflection (registry walk + `ReflectComponent`), WITHOUT a
//!    Leafwing dependency in this crate. The concrete binding type
//!    behind `Box<dyn Buttonlike>` is opaque to reflection, so key
//!    names are recovered from the box's `ReflectSerialize` data:
//!    Leafwing serializes dyn user inputs as typetagged JSON
//!    (`{"KeyCode":"KeyJ"}`, `{"ButtonlikeChord":[...]}`), and the
//!    typetag + payload normalize into the same canonical key-name
//!    space the probe sweep uses ([`normalize_key_name`]).
//! 2. **Intent-tap** ([`tap_manager_pipeline`]): correlate, per
//!    frame, the keys the driver actually pressed with the actions
//!    that fired in every `ActionState`. A binding whose press
//!    correlated with a firing earns `manager_pipeline_ok` — WIRING
//!    liveness ONLY, never game liveness (the game may ignore a
//!    fired action; that distinction is the product's honesty spine
//!    and is never blurred here).
//! 3. **Reconciliation** ([`reconcile`]): cross-grade the declared
//!    set against the measured draft surface. Exactly one
//!    alarm-grade outcome exists — `DeclaredButDead` — and it fires
//!    only for bindings with NO mitigating evidence (see
//!    [`reconcile`] for the full discipline).
//!
//! The library never links Leafwing: the module matches registrations
//! by type-path prefix
//! (`leafwing_input_manager::input_map::InputMap<`,
//! `...action_state::ActionState<`) and walks reflected values. All
//! LW knowledge is empirical, pinned by the reflection-contract
//! test in the known_answer fixture
//! (`tests/lw_reflection_contracts.rs`).

use bevy_ecs::reflect::ReflectComponent;
use bevy_ecs::world::World;
use bevy_reflect::{PartialReflect, ReflectRef, ReflectSerialize};

use crate::scenario::normalize_key_name;

/// Type-path prefix of Leafwing's `InputMap<A>` registrations.
const INPUT_MAP_PREFIX: &str = "leafwing_input_manager::input_map::InputMap";
/// Type-path prefix of Leafwing's `ActionState<A>` registrations.
const ACTION_STATE_PREFIX: &str = "leafwing_input_manager::action_state::ActionState";

/// Normalize an `ActionState<A>` component type path to the
/// `InputMap<A>` spelling. Declaration rows are keyed by the map
/// they came from (`input_map::InputMap<A>`), but the tap walks
/// `ActionState<A>` components — the SAME `A` in a
/// [`Buttonlike`](https://docs.rs/leafwing-input-manager) setup,
/// enforced by LW's `#[require(...)]` — so translating the path
/// gives both sides one join key. Without this the join never
/// matches and every `pressed_frames` stays 0 (pure-reflection
/// symmetry: no Leafwing types are linked into this crate).
fn action_state_path_as_map(path: &str) -> String {
    path.replacen("action_state::ActionState", "input_map::InputMap", 1)
}

/// One declared (action, input) binding read from the game's
/// `InputMap`s.
///
/// Serializable so it can ride a `PlaytestReport` field out of
/// probe-child processes (the parent reconciles later, without the
/// child's world).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LwDeclaration {
    /// Label of the `Actionlike` variant (the enum variant name,
    /// e.g. `"Jump"`).
    pub action: String,
    /// NORMALIZED key name — same string space as the sweep's
    /// surface entries, so reconciliation joins on equality.
    pub input: String,
    /// Concrete input kind's short type name (typetag), e.g.
    /// `"KeyCode"`, `"MouseButton"`, `"ButtonlikeChord"`.
    pub input_kind: String,
    /// This binding participates in an LW chord
    /// (`ButtonlikeChord`): firing requires ALL members held.
    /// Chord members are expected single-key probe misses — the
    /// reconcile never alarms on them.
    pub chord: bool,
    /// The `Actionlike` type's full type path (which map this row
    /// came from — multiple Actionlike types may coexist).
    pub action_map: String,
}

/// Observed runtime stats for one (action-map, action) PAIR — not
/// per binding — accumulated across a whole run by the tap pass.
/// Multiple declared bindings of the same action share one row;
/// which specific bindings saw driver presses is retained in
/// [`LwTapStats::pressed_inputs`] so the reconcile can attribute
/// alarm evidence to the binding that was actually pressed.
///
/// `pressed_frames` counts frames the driver pressed ANY declared
/// binding of the action (any `ButtonInput` transition to pressed
/// at that frame, post-update); `fired` counts frames the action
/// fired (`ButtonState::JustPressed` in ANY `ActionState` of that
/// action-map) — mirroring Leafwing's public `just_pressed()`
/// semantics, which report NOTHING for disabled actions even
/// though the raw state datum still ticks.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LwTapStats {
    pub action: String,
    pub action_map: String,
    /// Frames the driver pressed this input. Inflated per
    /// `ActionState` instance (per-player map clones each yield
    /// one increment) — diagnostic only; alarm gates never read
    /// this counter (they read `pressed_inputs` and `disabled`).
    pub pressed_frames: u64,
    /// The specific declared bindings (canonical input names) the
    /// driver was OBSERVED pressing on at least one frame. Splits
    /// the action-scoped `pressed_frames` evidence down to per-
    /// binding proof: a binding never pressed earns no alarm arm,
    /// even when a sibling binding was (multi-binding false-positive
    /// guard, HIGH-1). Empty iff `pressed_frames == 0`.
    #[serde(default)]
    pub pressed_inputs: std::collections::BTreeSet<String>,
    /// Frames the action fired under tap observation.
    pub fired_frames: u64,
    /// `true` when ANY observed `ActionState` of this action-map
    /// carried this action's `disabled` flag at ANY tapped frame —
    /// sticky for the whole run (a later re-enable never clears it).
    /// The reconcile treats the flag as evidence, never verdict: an
    /// action that FIRED despite the sticky flag overrides it (see
    /// [`reconcile`]'s alarm discipline).
    pub disabled: bool,
}

/// Read every declared binding from every `InputMap` in the world.
///
/// RON-loaded and code-built maps are indistinguishable by design:
/// both are the same reflected component. Returns rows sorted by
/// (action_map, action, input) for deterministic serialization.
/// Empty when the game does not use (or has not yet spawned) Leafwing
/// state.
pub fn read_declared(world: &World) -> Vec<LwDeclaration> {
    let mut rows: Vec<LwDeclaration> = Vec::new();
    let Some(registry_arc) = world.get_resource::<bevy_ecs::reflect::AppTypeRegistry>() else {
        return rows;
    };
    let registry = registry_arc.0.read();

    for type_reg in registry.iter() {
        if !type_reg
            .type_info()
            .type_path()
            .starts_with(INPUT_MAP_PREFIX)
        {
            continue;
        }
        let Some(reflect_component) = type_reg.data::<ReflectComponent>() else {
            continue;
        };

        for entity_ref in world.iter_entities() {
            let Some(map) = reflect_component.reflect(entity_ref) else {
                continue;
            };
            let action_map = type_reg.type_info().type_path().to_string();
            collect_map(
                world,
                &registry,
                map as &dyn PartialReflect,
                &action_map,
                &mut rows,
            );
        }
    }

    rows.sort_by(|a, b| {
        (&a.action_map, &a.action, &a.input, &a.input_kind).cmp(&(
            &b.action_map,
            &b.action,
            &b.input,
            &b.input_kind,
        ))
    });
    rows.dedup();
    rows
}

/// Walk one reflected `InputMap`'s `buttonlike_map` into declaration
/// rows.
fn collect_map(
    world: &World,
    registry: &bevy_reflect::TypeRegistry,
    map: &dyn PartialReflect,
    action_map: &str,
    rows: &mut Vec<LwDeclaration>,
) {
    let ReflectRef::Struct(s) = map.reflect_ref() else {
        return;
    };
    let Some(binding_map) = s.field("buttonlike_map") else {
        return;
    };
    let ReflectRef::Map(binding_map) = binding_map.reflect_ref() else {
        return;
    };
    for kv in binding_map.iter() {
        let (key, value) = kv;
        let ReflectRef::Enum(action_enum) = key.reflect_ref() else {
            continue;
        };
        let action = action_enum.variant_name().to_string();
        let ReflectRef::List(bindings) = value.reflect_ref() else {
            continue;
        };
        for i in 0..bindings.len() {
            let Some(item) = bindings.get(i) else {
                continue;
            };
            extract_binding(world, registry, item, &action, action_map, rows);
        }
    }
}

/// Recover (input, chord) from one opaque `Box<dyn Buttonlike>` via
/// its serialized typetag.
fn extract_binding(
    world: &World,
    registry: &bevy_reflect::TypeRegistry,
    item: &dyn PartialReflect,
    action: &str,
    action_map: &str,
    rows: &mut Vec<LwDeclaration>,
) {
    // Re-serialize the single opaque box: ReflectSerialize data is
    // installed on the `Box<dyn Buttonlike>` REGISTRATION by LW's
    // trait_serde glue; the value serializes as
    // {"ConcreteType": payload}.
    let Some(as_reflect) = item.try_as_reflect() else {
        return;
    };
    let Some(registration) = registry.get_with_type_path(item.reflect_type_path()) else {
        return;
    };
    let Some(serial) = registration.data::<ReflectSerialize>() else {
        return;
    };
    let serializable = serial.get_serializable(as_reflect);
    let Ok(json) = serde_json::to_string(&*serializable) else {
        return;
    };
    // (The dependency is dev-only in spirit; serde_json is a real
    // dependency of the harness — it serializes reports — so reuse.)

    parse_typetag(&json, action, action_map, rows);

    let _ = world; // reserved: gamepad-associated bindings (D2+)
}

/// Parse typetagged JSON `{"<Kind>": payload}` into declaration
/// rows. Chords fan out to one row per member, flagged `chord`.
fn parse_typetag(json: &str, action: &str, action_map: &str, rows: &mut Vec<LwDeclaration>) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return;
    };
    let Some(obj) = value.as_object() else { return };
    // Typetag is a single-key map: {"ConcreteShortPath": payload}.
    let Some((kind, payload)) = obj.iter().next() else {
        return;
    };
    let input_kind = kind.clone();

    match input_kind.as_str() {
        "ButtonlikeChord" => {
            // Payload: array of member typetags.
            if let Some(members) = payload.as_array() {
                for member in members {
                    if let Some(member_obj) = member.as_object() {
                        if let Some((member_kind, member_payload)) = member_obj.iter().next() {
                            if let Some(input) = key_name_from_payload(member_kind, member_payload)
                            {
                                rows.push(LwDeclaration {
                                    action: action.to_string(),
                                    input,
                                    input_kind: member_kind.clone(),
                                    chord: true,
                                    action_map: action_map.to_string(),
                                });
                            }
                        }
                    }
                }
            }
        }
        kind => {
            if let Some(input) = key_name_from_payload(kind, payload) {
                rows.push(LwDeclaration {
                    action: action.to_string(),
                    input,
                    input_kind: kind.to_string(),
                    chord: false,
                    action_map: action_map.to_string(),
                });
            }
        }
    }
}

/// Normalize a serialised input payload into the sweep's canonical
/// key-name space. Returns `None` for kinds outside D1 scope
/// (gamepad, virtual axes, unrecognized kinds).
fn key_name_from_payload(kind: &str, payload: &serde_json::Value) -> Option<String> {
    let raw = match payload {
        serde_json::Value::String(s) => s.as_str(),
        _ => return None,
    };
    // KeyCode serializes as the bare variant name ("KeyJ") and
    // joins the canonical KEY space via normalize_key_name.
    // MouseButton also serializes as the bare variant name
    // ("Left"/"Right"/"Middle"/"Back"/"Forward") but must join the
    // sweep's MOUSE-button name space instead — never
    // normalize_key_name, whose "left" spelling collides with
    // ArrowLeft. Mouse payloads branch by kind here precisely so
    // they never enter the key-normalization path.
    match kind {
        "KeyCode" => normalize_key_name(raw).map(str::to_string),
        "MouseButton" => crate::scenario::normalize_mouse_button_name(raw).map(str::to_string),
        _ => None,
    }
}

/// One frame's intent-tap observation pass: run AFTER `app.update()`
/// in the driver loop (outside all schedules, like the digest
/// recorder, so it never appears in the game's schedule or the
/// static scan's reader census).
///
/// For every `ActionState` component in the world, this records:
/// - whether the action FIRED this frame (`ButtonState::JustPressed`);
/// - whether any of the action's DECLARED keyboard bindings was in
///   the driver's pressed-this-frame set (press correlation);
/// - the effective disabled flag — Leafwing's struct-level
///   `disabled` OR the per-action datum's — observed every frame
///   and sticky for the run (a firing overrides it downstream).
///
/// Stats accumulate per (action-map, action) in `stats`. `declared`
/// seeds the binding join: only bindings the reader declared
/// participate (D1 scope: keyboard keys, non-chord).
///
/// WIRING liveness ONLY: a fired action with no world effect is a
/// `manager_pipeline_ok` binding the game ignores — a note, never
/// an alarm (see [`reconcile`]).
pub fn tap_manager_pipeline(
    world: &World,
    pressed_this_frame: &std::collections::BTreeSet<String>,
    declared: &[LwDeclaration],
    stats: &mut Vec<LwTapStats>,
) {
    let Some(registry_arc) = world.get_resource::<bevy_ecs::reflect::AppTypeRegistry>() else {
        return;
    };
    let registry = registry_arc.0.read();

    // Keyboard bindings per (action_map, action), non-chord only —
    // a lone member press cannot fire a chord, so counting it as a
    // "driver press" would fabricate alarm fuel.
    let mut bindings: std::collections::BTreeMap<
        (String, String),
        std::collections::BTreeSet<String>,
    > = Default::default();
    for decl in declared {
        if decl.chord {
            continue;
        }
        bindings
            .entry((decl.action_map.clone(), decl.action.clone()))
            .or_default()
            .insert(decl.input.clone());
    }

    for type_reg in registry.iter() {
        if !type_reg
            .type_info()
            .type_path()
            .starts_with(ACTION_STATE_PREFIX)
        {
            continue;
        }
        let Some(reflect_component) = type_reg.data::<ReflectComponent>() else {
            continue;
        };
        // Join key: declarations are keyed by their InputMap path,
        // the tap walks ActionState components — translate so
        // `bindings` (built from declarations) matches. The raw path
        // is kept ONLY for the translated join; rows store the
        // InputMap spelling to stay joinable with declarations.
        let action_map = action_state_path_as_map(type_reg.type_info().type_path());

        for entity_ref in world.iter_entities() {
            let Some(state) = reflect_component.reflect(entity_ref) else {
                continue;
            };
            let ReflectRef::Struct(s) = state.reflect_ref() else {
                continue;
            };
            // Component-wide disable (`ActionState::disable()`) is a
            // STRUCT-level `disabled: bool` field — reflected, public
            // — distinct from the per-action `ActionData.disabled`.
            // Leafwing's `action_disabled()` gates EVERY accessor on
            // `struct.disabled || datum.disabled`, so the tap must
            // mirror the same union or a component-wide disable
            // would masquerade as live wiring.
            let struct_disabled = s
                .field("disabled")
                .and_then(|f| f.try_downcast_ref::<bool>())
                .copied()
                .unwrap_or(false);
            let Some(action_data) = s.field("action_data") else {
                continue;
            };
            let ReflectRef::Map(action_data) = action_data.reflect_ref() else {
                continue;
            };
            for kv in action_data.iter() {
                let (key, value) = kv;
                let ReflectRef::Enum(action_enum) = key.reflect_ref() else {
                    continue;
                };
                let action = action_enum.variant_name().to_string();
                let row = tap_row_for(stats, &action, &action_map);
                let action_bindings = bindings.get(&(action_map.clone(), action.clone()));
                observe_action_datum(
                    value,
                    struct_disabled,
                    pressed_this_frame,
                    action_bindings,
                    row,
                );
            }
        }
    }
}

/// Find-or-create the accumulating stats row for (action, map).
fn tap_row_for<'a>(
    stats: &'a mut Vec<LwTapStats>,
    action: &str,
    action_map: &str,
) -> &'a mut LwTapStats {
    if let Some(idx) = stats
        .iter()
        .position(|r| r.action == action && r.action_map == action_map)
    {
        &mut stats[idx]
    } else {
        stats.push(LwTapStats {
            action: action.to_string(),
            action_map: action_map.to_string(),
            ..Default::default()
        });
        stats.last_mut().expect("just pushed")
    }
}

/// Observe one `ActionData`: fire state vs the driver's press set,
/// plus the disabled flag (per-action OR component-wide).
///
/// The row's `input` stays empty until first correlated press: the
/// driver pressed SOMETHING bound to this action. (Multi-binding
/// ambiguity: the correlation assigns ALL of the action's declared
/// bindings that were pressed this frame — recorded on the ACTION
/// row; per-binding split is the reconcile's join with
/// `read_declared`.)
fn observe_action_datum(
    datum: &dyn PartialReflect,
    struct_disabled: bool,
    pressed_this_frame: &std::collections::BTreeSet<String>,
    action_bindings: Option<&std::collections::BTreeSet<String>>,
    row: &mut LwTapStats,
) {
    let ReflectRef::Struct(datum) = datum.reflect_ref() else {
        return;
    };
    // Leafwing's effective disabled gate: struct-level
    // (`ActionState::disable()`) OR per-action datum — exactly
    // `action_disabled()`'s definition in LW 0.22.
    let disabled = struct_disabled
        || datum
            .field("disabled")
            .and_then(|f| f.try_downcast_ref::<bool>())
            .copied()
            .unwrap_or(false);
    if disabled {
        row.disabled = true;
    }

    // Press correlation: did the driver press one of this action's
    // declared keyboard bindings this frame? No declared bindings
    // (gamepad/axis actions, or the reader saw none) = nothing to
    // correlate — absence of a press proves nothing. Record WHICH
    // bindings were pressed (per-binding evidence for the
    // reconcile's alarm arms — HIGH-1) while keeping the counter
    // semantics untouched.
    if let Some(bindings) = action_bindings {
        let mut any_pressed = false;
        for input in bindings {
            if pressed_this_frame.contains(input) {
                any_pressed = true;
                row.pressed_inputs.insert(input.clone());
            }
        }
        if any_pressed {
            // Frames ANY declared binding was pressed. NOTE: the
            // counter inflates per `ActionState` instance (a
            // per-player clone of the map yields one increment
            // each) — it is diagnostic only. The alarm gates read
            // `pressed_inputs` (per-binding set) and the sticky
            // `disabled` flag, never this counter, so the
            // inflation cannot arm a false alarm.
            row.pressed_frames += 1;
        }
    }

    let Some(kind_data) = datum.field("kind_data") else {
        return;
    };
    let ReflectRef::Enum(kind) = kind_data.reflect_ref() else {
        return;
    };
    if kind.variant_name() != "Button" {
        return; // axis/dual-axis actions: out of D1 tap scope
    }
    let Some(button_data) = kind.field_at(0) else {
        return;
    };
    let ReflectRef::Struct(button_data) = button_data.reflect_ref() else {
        return;
    };
    let Some(state) = button_data.field("state") else {
        return;
    };
    let ReflectRef::Enum(state_enum) = state.reflect_ref() else {
        return;
    };
    // Raw `ButtonData.state` transitions to `JustPressed` even for
    // DISABLED actions (empirically confirmed against LW 0.22): the
    // public `just_pressed()` accessor gates on `action_disabled`,
    // and the tap must mirror that gate or a disabled binding would
    // masquerade as live wiring. The `disabled` flag itself is
    // recorded above — the alarm consumes it there.
    if state_enum.variant_name() == "JustPressed" && !disabled {
        row.fired_frames += 1;
    }
}

/// The keys the driver effectively pressed for THIS frame's tap
/// correlation, read post-update: the bot writes `KeyboardInput`
/// events during frame N's `Update`, `InputPlugin` translates them
/// into `ButtonInput<KeyCode>` at frame N+1's `PreUpdate`, and
/// Leafwing fires the action in the same N+1 window — so the
/// post-update `just_pressed()` set and this frame's `JustPressed`
/// actions describe the same causal press. Names go through
/// [`normalize_key_name`] so the set joins the declaration rows on
/// identical strings; keys outside the canonical space (F-keys,
/// media) drop out on both sides alike.
#[cfg(feature = "lw-reader")]
pub fn pressed_keys_this_frame(world: &World) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    let Some(input) =
        world.get_resource::<bevy_input::ButtonInput<bevy_input::keyboard::KeyCode>>()
    else {
        return out;
    };
    for code in input.get_just_pressed() {
        let raw = format!("{code:?}");
        if let Some(name) = normalize_key_name(&raw) {
            out.insert(name.to_string());
        }
    }
    out
}

/// The runtime evidence gathered across one run by the tap pass:
/// declared rows + per-action press/fire/disabled stats.
///
/// Serialized onto the `PlaytestReport` (additive fields) so the
/// discovery parent can reconcile without re-reading the (gone)
/// child world.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LwRunEvidence {
    /// Declaration rows (from [`read_declared`], taken at the
    /// first post-update frame where Leafwing state exists).
    pub declared: Vec<LwDeclaration>,
    /// Per (action-map, action) tap statistics across the run.
    pub stats: Vec<LwTapStats>,
}

/// The result of reconciling declared intent against measured
/// reality: entries to ADD to the surface (or upgrade), findings to
/// add, and findings from the draft to RETRACT (the draft emitted
/// `DiscoveredButUndeclared` for an input the reader has now found
/// declared — reconciliation keeps no duplicates).
///
/// Pure function: given the same (declared, stats, draft) it always
/// produces the same output — the determinism gate's byte-stability
/// requirement extends here unchanged.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReconciledLw {
    /// New/updated entries (fully-formed `SurfaceEntry`s with grade
    /// `Declared` or `ManagerPipelineOk`).
    pub entries: Vec<super::surface::SurfaceEntry>,
    /// New findings (`DeclaredButDead` alarms, `ContextUnverified`
    /// coverage notes-as-findings).
    pub findings: Vec<super::surface::DiscoveryFinding>,
    /// Every input the reader declared, sorted and deduped — a
    /// SUPERSET, by construction, of the draft
    /// `DiscoveredButUndeclared` findings it stands to retract:
    /// `reconcile` never sees the draft findings, so it cannot say
    /// which declared inputs actually carried one. The semantics
    /// are applied by the consumer: any draft residue finding on
    /// an input in this list is duplication and must be treated
    /// as retracted.
    pub retracted_residue: Vec<String>,
}

/// Aggregate the Leafwing declared-tier evidence scattered across a
/// discovery sweep's child-run reports into ONE [`LwRunEvidence`].
///
/// Every probe child runs the same game — each child's reader sees
/// the same `InputMap`s (declared rows UNION and dedupe by full
/// row identity), while each child's tap only saw the presses ITS
/// scenario drove: stats rows for the same (action-map, action)
/// MERGE — counters sum, `pressed_inputs` union, `disabled`
/// sticks. The merged evidence is exactly what one holistic run
/// would have recorded, so [`reconcile`] treats it unchanged.
///
/// `None` when no child declared anything (game without Leafwing
/// state, or the `lw-reader` feature compiled out of the children)
/// — the surface then honestly carries no lw section at all.
pub fn aggregate_lw_evidence(reports: &[&crate::state::PlaytestReport]) -> Option<LwRunEvidence> {
    let mut declared: Vec<LwDeclaration> = Vec::new();
    // (action_map, action) → merged row; BTreeMap for deterministic
    // output order regardless of report order.
    let mut stats: std::collections::BTreeMap<(String, String), LwTapStats> = Default::default();
    for report in reports {
        for row in &report.lw_declared {
            if !declared.contains(row) {
                declared.push(row.clone());
            }
        }
        for row in &report.lw_tap_stats {
            let k = (row.action_map.clone(), row.action.clone());
            let merged = stats.entry(k).or_insert_with(|| LwTapStats {
                action: row.action.clone(),
                action_map: row.action_map.clone(),
                ..Default::default()
            });
            merged.pressed_frames += row.pressed_frames;
            merged.fired_frames += row.fired_frames;
            merged.disabled |= row.disabled;
            merged
                .pressed_inputs
                .extend(row.pressed_inputs.iter().cloned());
        }
    }
    if declared.is_empty() {
        return None;
    }
    // Deterministic declared order: (action_map, action, input),
    // same key as `read_declared`'s per-child sort.
    declared.sort_by(|a, b| {
        (&a.action_map, &a.action, &a.input).cmp(&(&b.action_map, &b.action, &b.input))
    });
    Some(LwRunEvidence {
        declared,
        stats: stats.into_values().collect(),
    })
}

/// Reconcile the declared tier against the draft sweep.
///
/// Alarm discipline (THE point of D1-d — a false alarm destroys the
/// channel, so the rules are exhaustive):
///
/// - **`DeclaredButDead` (alarm)**: fires for a non-chord binding
///   only when the wiring itself is provably dead, in exactly one
///   of two arms:
///   - **disabled arm**: the action was DISABLED (Leafwing
///     `action_disabled`: per-action OR component-wide) at some
///     tapped frame AND never fired — the sticky `disabled` flag is
///     EVIDENCE, overridden by any firing (an action disabled then
///     re-enabled is alive, not dead);
///   - **pressed-never-fired arm**: the driver PRESSED the binding
///     and the action NEVER fired across the whole run.
/// - **NEVER fires for**: chord members (firing needs all members;
///   a single-key probe press is an expected miss, not death —
///   INCLUDING a disabled chord, a deliberate gap: chord firing is
///   unobservable per-member, so no per-member arm can be proven);
///   bindings the driver never pressed (absence of presses proves
///   nothing); tap-ok bindings with no game effect (WIRING live —
///   the game ignoring a fired action is a note, never an alarm);
///   a disabled-then-re-enabled action that FIRED (the firing
///   overrides the sticky disabled evidence).
/// - **`ContextUnverified`** finding: declared ∧ tap-ok (wiring
///   live) ∧ never verified outside the boot context — the honest
///   coverage grade for context-gated actions; NOT alarm-grade.
/// - **Retraction**: draft `DiscoveredButUndeclared` findings on
///   inputs the reader declared are duplicated residue — listed in
///   `retracted_residue` for the caller to remove.
///
/// `draft_inputs_live`: inputs the sweep measured LIVE (effect
/// signature observed). Chord awareness comes from the declaration
/// rows themselves (`LwDeclaration::chord`).
pub fn reconcile(
    evidence: &LwRunEvidence,
    draft_inputs_live: &std::collections::BTreeSet<String>,
) -> ReconciledLw {
    use super::provenance::ProvenanceGrade;
    use super::surface::{DiscoveryFinding, DiscoveryFindingKind, Evidence};

    // Per (action_map, action) aggregate from the stats.
    let mut fired: std::collections::BTreeMap<(String, String), u64> = Default::default();
    let mut pressed: std::collections::BTreeMap<(String, String), u64> = Default::default();
    let mut pressed_inputs: std::collections::BTreeMap<
        (String, String),
        std::collections::BTreeSet<String>,
    > = Default::default();
    for row in &evidence.stats {
        let k = (row.action_map.clone(), row.action.clone());
        *fired.entry(k.clone()).or_default() += row.fired_frames;
        *pressed.entry(k.clone()).or_default() += row.pressed_frames;
        pressed_inputs
            .entry(k.clone())
            .or_default()
            .extend(row.pressed_inputs.iter().cloned());
        // disabled: row-level, folded below per action.
    }
    let mut disabled_actions: std::collections::BTreeSet<(String, String)> = Default::default();
    for row in &evidence.stats {
        if row.disabled {
            disabled_actions.insert((row.action_map.clone(), row.action.clone()));
        }
    }

    let mut entries = Vec::new();
    let mut findings = Vec::new();
    let mut retracted_residue = Vec::new();

    for decl in &evidence.declared {
        let k = (decl.action_map.clone(), decl.action.clone());
        let fired_frames = fired.get(&k).copied().unwrap_or(0);
        let pressed_frames = pressed.get(&k).copied().unwrap_or(0);
        let is_disabled = disabled_actions.contains(&k);

        // Retract draft residue on this input: it IS declared.
        retracted_residue.push(decl.input.clone());

        let wiring_live = fired_frames > 0;
        let grade = if wiring_live {
            ProvenanceGrade::ManagerPipelineOk
        } else {
            ProvenanceGrade::Declared
        };

        let mut ev = vec![Evidence::DeclaredIn {
            source_kind: "leafwing_input_map".to_string(),
        }];
        if wiring_live {
            ev.push(Evidence::TapObserved {
                stage: super::provenance::TAP_STAGE_MANAGER_PIPELINE.to_string(),
            });
        }

        entries.push(super::surface::SurfaceEntry {
            input: decl.input.clone(),
            grade,
            action_label: Some(decl.action.clone()),
            context: super::surface::ContextWitness::Boot,
            evidence: ev,
            effect_signature: None,
            notes: reconcile_notes(decl, wiring_live, is_disabled, pressed_frames),
            mutable: false,
            profile: None,
        });

        // THE alarm — only with proof of dead wiring. A firing
        // overrides the sticky disabled flag (H1: disabled-then-
        // re-enabled is alive); pressed-never-fired needs proof THIS
        // binding was pressed (HIGH-1: a sibling binding's press
        // must not arm the alarm for an action's other bindings —
        // `fired_frames` is action-scoped, so any firing silences
        // every binding's pressed-arm, which is correct: wiring
        // proven alive cannot also be proven dead).
        let binding_was_pressed = pressed_inputs
            .get(&k)
            .is_some_and(|inputs| inputs.contains(&decl.input));
        let alarm = !decl.chord && fired_frames == 0 && (is_disabled || binding_was_pressed);
        if alarm {
            // Per-arm detail (M2): the two arms say different
            // things, and a lying message poisons the channel like
            // a false tap would.
            let detail = if is_disabled {
                format!(
                    "declared action '{}' (map {}) was disabled \
                     (ActionState) and never fired across the run",
                    decl.action, decl.action_map
                )
            } else {
                format!(
                    "declared action '{}' (map {}) never fired \
                     across {} driver-pressed frame(s)",
                    decl.action, decl.action_map, pressed_frames
                )
            };
            findings.push(DiscoveryFinding {
                kind: DiscoveryFindingKind::DeclaredButDead,
                input: decl.input.clone(),
                detail,
            });
        } else if wiring_live && !decl.chord && !draft_inputs_live.contains(&decl.input) {
            // Wiring live but the sweep never verified game liveness
            // outside boot context: the honest coverage finding.
            // NEVER for chord members: a satisfied chord under a
            // driver-synchronized multi-press is still an
            // acknowledged probe limitation (single-key taps miss
            // chords by design), so a note — not a finding — is the
            // alarm-discipline-correct channel (brief Part 4).
            findings.push(DiscoveryFinding {
                kind: DiscoveryFindingKind::ContextUnverified,
                input: decl.input.clone(),
                detail: format!(
                    "action '{}' fired under tap but no game effect was verified \
                     outside the boot context (context-gated or ignored by game code)",
                    decl.action
                ),
            });
        }
    }

    retracted_residue.sort();
    retracted_residue.dedup();
    entries.sort_by(|a, b| a.input.cmp(&b.input));
    findings.sort_by(|a, b| a.input.cmp(&b.input));

    ReconciledLw {
        entries,
        findings,
        retracted_residue,
    }
}

/// The per-binding notes, honest by construction:
/// - chord member: expected single-key miss, never an alarm;
/// - disabled: the alarm detail lives on the finding, the note says
///   why;
/// - tap-ok, no verified effect: wiring-only liveness spelled out;
/// - tap-ok + live: full pipeline confirmed.
fn reconcile_notes(
    decl: &LwDeclaration,
    wiring_live: bool,
    is_disabled: bool,
    pressed_frames: u64,
) -> Vec<String> {
    if decl.chord {
        return vec![format!(
            "LW chord member (action '{}'): fires only with all members held; \
             single-key probes are expected misses",
            decl.action
        )];
    }
    if wiring_live {
        if is_disabled {
            return vec![format!(
                "LW action '{}' fired despite an observed disabled interval \
                 (disabled then re-enabled): wiring live (manager_pipeline_ok)",
                decl.action
            )];
        }
        if pressed_frames == 0 {
            return vec![format!(
                "LW action '{}' fired without a recorded driver press (game-side \
                 triggered or unobserved source)",
                decl.action
            )];
        }
        return vec![format!(
            "LW action '{}' fired under driver press: wiring live (manager_pipeline_ok)",
            decl.action
        )];
    }
    if is_disabled {
        return vec![format!(
            "LW action '{}' is disabled (ActionState::disabled) and never fired: dead wiring",
            decl.action
        )];
    }
    if pressed_frames > 0 {
        return vec![format!(
            "LW action '{}' never fired across {} driver-pressed frames",
            decl.action, pressed_frames
        )];
    }
    vec![format!(
        "LW action '{}' declared; driver never pressed this binding during the run",
        decl.action
    )]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tap's join key: an `ActionState<A>` component path must
    /// normalize to the `InputMap<A>` spelling declarations carry,
    /// or the per-press correlation (and reconcile) never joins.
    #[test]
    fn action_state_path_translates_to_map_join_key() {
        assert_eq!(
            action_state_path_as_map(
                "leafwing_input_manager::action_state::ActionState<game::LwPlantAction>"
            ),
            "leafwing_input_manager::input_map::InputMap<game::LwPlantAction>"
        );
        // Idempotent-ish safety: a path that's already map-shaped
        // (future-proofing against reorderings) doesn't corrupt.
        let map_path = "leafwing_input_manager::input_map::InputMap<game::A>";
        assert_eq!(action_state_path_as_map(map_path), map_path);
    }

    /// Raw `ButtonData.state` reaches `JustPressed` even for DISABLED
    /// actions (proven empirically against LW 0.22; the public
    /// `just_pressed()` gates on `action_disabled`). The tap must
    /// mirror that gate: a disabled firing is NOT pipeline liveness.
    #[test]
    fn tap_does_not_count_firings_of_disabled_actions() {
        use bevy::reflect::Reflect;

        #[derive(Reflect)]
        enum FakeButtonState {
            JustPressed,
            Pressed,
            JustReleased,
            Released,
        }

        #[derive(Reflect)]
        struct FakeButtonData {
            state: FakeButtonState,
        }

        #[derive(Reflect)]
        enum FakeKind {
            Button(FakeButtonData),
            Axis(f32),
        }

        /// Mirrors LW's `ActionData` shape (struct: disabled +
        /// kind_data enum holding `Button(ButtonData)`).
        #[derive(Reflect)]
        struct FakeActionData {
            disabled: bool,
            kind_data: FakeKind,
        }

        let empty_set = std::collections::BTreeSet::new();
        let no_bindings: Option<&std::collections::BTreeSet<String>> = None;

        let mut fired_and_disabled = FakeActionData {
            disabled: true,
            kind_data: FakeKind::Button(FakeButtonData {
                state: FakeButtonState::JustPressed,
            }),
        };
        let mut row = LwTapStats::default();
        observe_action_datum(
            &fired_and_disabled,
            false,
            &empty_set,
            no_bindings,
            &mut row,
        );
        assert_eq!(
            row.fired_frames, 0,
            "disabled action's raw JustPressed must not count as a firing"
        );
        assert!(row.disabled, "the disabled flag itself IS recorded");

        let healthy = FakeActionData {
            disabled: false,
            kind_data: FakeKind::Button(FakeButtonData {
                state: FakeButtonState::JustPressed,
            }),
        };
        let mut row = LwTapStats::default();
        observe_action_datum(&healthy, false, &empty_set, no_bindings, &mut row);
        assert_eq!(row.fired_frames, 1, "enabled action fires once");

        fired_and_disabled.disabled = false;
        let mut row = LwTapStats::default();
        observe_action_datum(
            &fired_and_disabled,
            false,
            &empty_set,
            no_bindings,
            &mut row,
        );
        assert_eq!(row.fired_frames, 1, "the same datum, enabled, fires");
    }

    /// H2: Leafwing's component-wide `ActionState::disable()` sets
    /// the STRUCT-level `disabled: bool` field (public + reflected in
    /// LW 0.22) and gates EVERY accessor via `action_disabled() =
    /// struct.disabled || datum.disabled`. The tap must OR the
    /// struct flag into each row and gate firings on the union, or a
    /// component-wide disable would count raw ticks as firings.
    #[test]
    fn tap_counts_struct_level_disable_like_lw_action_disabled() {
        use bevy::reflect::Reflect;

        #[derive(Reflect)]
        enum FakeButtonState {
            JustPressed,
            Pressed,
            JustReleased,
            Released,
        }

        #[derive(Reflect)]
        struct FakeButtonData {
            state: FakeButtonState,
        }

        #[derive(Reflect)]
        enum FakeKind {
            Button(FakeButtonData),
            Axis(f32),
        }

        #[derive(Reflect)]
        struct FakeActionData {
            disabled: bool,
            kind_data: FakeKind,
        }

        // Drive the same union logic at the datum level with both
        let empty_set = std::collections::BTreeSet::new();
        let no_bindings: Option<&std::collections::BTreeSet<String>> = None;

        // Datum says enabled, struct says disabled: struct wins
        // (mirrors `action_disabled()`'s short-circuit).
        let datum_enabled = FakeActionData {
            disabled: false,
            kind_data: FakeKind::Button(FakeButtonData {
                state: FakeButtonState::JustPressed,
            }),
        };
        let mut row = LwTapStats::default();
        observe_action_datum(&datum_enabled, true, &empty_set, no_bindings, &mut row);
        assert_eq!(
            row.fired_frames, 0,
            "component-wide disable must suppress firings of enabled-looking data"
        );
        assert!(row.disabled, "the struct-level disabled flag IS recorded");

        // Struct re-enabled later, datum disabled at the time of the
        // raw tick: the datum's flag still suppresses the firing
        // (both flags contribute to the union).
        let datum_disabled = FakeActionData {
            disabled: true,
            kind_data: FakeKind::Button(FakeButtonData {
                state: FakeButtonState::JustPressed,
            }),
        };
        let mut row = LwTapStats::default();
        observe_action_datum(&datum_disabled, false, &empty_set, no_bindings, &mut row);
        assert_eq!(row.fired_frames, 0);
        assert!(row.disabled);

        // Neither disabled: fires.
        let mut row = LwTapStats::default();
        observe_action_datum(&datum_enabled, false, &empty_set, no_bindings, &mut row);
        assert_eq!(row.fired_frames, 1);
    }

    fn decl(action: &str, input: &str, chord: bool) -> LwDeclaration {
        LwDeclaration {
            action: action.to_string(),
            input: input.to_string(),
            input_kind: "KeyCode".to_string(),
            chord,
            action_map: "test::GameAction".to_string(),
        }
    }

    fn stat(action: &str, input: &str, pressed: u64, fired: u64, disabled: bool) -> LwTapStats {
        LwTapStats {
            action: action.to_string(),
            action_map: "test::GameAction".to_string(),
            pressed_frames: pressed,
            pressed_inputs: if pressed > 0 {
                std::iter::once(input.to_string()).collect()
            } else {
                Default::default()
            },
            fired_frames: fired,
            disabled,
        }
    }

    /// parse_typetag is crate-private; exercised through JSON
    /// fixtures mirroring exactly what the LW reflection-contract
    /// test (known_answer fixture) proved LW emits.
    #[test]
    fn typetag_single_key_and_chord_parse() {
        let mut rows = Vec::new();
        parse_typetag(r#"{"KeyCode":"KeyJ"}"#, "Jump", "m::A", &mut rows);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].input, "KeyJ");
        assert!(!rows[0].chord);
        assert_eq!(rows[0].action, "Jump");

        parse_typetag(
            r#"{"ButtonlikeChord":[{"KeyCode":"KeyJ"},{"KeyCode":"KeyK"}]}"#,
            "Combo",
            "m::A",
            &mut rows,
        );
        let chord_rows: Vec<_> = rows.iter().filter(|r| r.chord).collect();
        assert_eq!(chord_rows.len(), 2, "chord fans out per member");
        assert!(chord_rows
            .iter()
            .all(|r| r.input == "KeyJ" || r.input == "KeyK"));

        // Space normalizes; out-of-scope kinds produce nothing.
        parse_typetag(r#"{"KeyCode":"Space"}"#, "Use", "m::A", &mut rows);
        parse_typetag(
            r#"{"GamepadButtonType":{"kind":"South"}}"#,
            "Pad",
            "m::A",
            &mut rows,
        );
        assert!(
            rows.iter().all(|r| r.action != "Pad"),
            "gamepad dropped (D2)"
        );
        let use_row = rows.iter().find(|r| r.action == "Use").unwrap();
        assert_eq!(use_row.input, "Space");

        // Malformed JSON: silently nothing (reader never panics on
        // game data).
        let before = rows.len();
        parse_typetag("{not json", "X", "m::A", &mut rows);
        assert_eq!(rows.len(), before);
    }

    #[test]
    fn reconcile_alarm_matrix() {
        let declared = vec![
            decl("Alive", "KeyJ", false),              // live: fires under press
            decl("DisabledPlant", "KeyN", false),      // disabled: ALARM
            decl("PressedNeverFired", "KeyO", false),  // pressed, never fired: ALARM
            decl("NeverPressed", "KeyP", false),       // no info: NO alarm
            decl("ChordMember", "KeyI", true),         // chord: NEVER alarms
            decl("ReEnabledThenFired", "KeyQ", false), // disabled once, fired anyway
        ];
        let stats = vec![
            stat("Alive", "KeyJ", 2, 1, false),
            stat("DisabledPlant", "KeyN", 1, 0, true),
            stat("PressedNeverFired", "KeyO", 3, 0, false),
            stat("NeverPressed", "KeyP", 0, 0, false),
            stat("ChordMember", "KeyI", 1, 0, false),
            stat("ReEnabledThenFired", "KeyQ", 1, 1, true),
        ];
        let evidence = LwRunEvidence { declared, stats };
        let live = ["KeyJ"]
            .into_iter()
            .map(String::from)
            .collect::<std::collections::BTreeSet<_>>();
        let out = reconcile(&evidence, &live);

        // Alarm discipline: exactly the two dead wirings fire, and
        // only one finding kind is alarm-grade.
        let dead: Vec<_> = out
            .findings
            .iter()
            .filter(|f| {
                matches!(
                    f.kind,
                    super::super::surface::DiscoveryFindingKind::DeclaredButDead
                )
            })
            .collect();
        let inputs: Vec<&str> = dead.iter().map(|f| f.input.as_str()).collect();
        assert_eq!(
            inputs,
            ["KeyN", "KeyO"],
            "alarms: disabled + pressed-never-fired; a firing must \
             override the sticky disabled flag"
        );

        // H1: disabled-then-re-enabled-then-fired: the firing
        // overrides the sticky disabled evidence — grade ok, NO
        // alarm, and the per-arm detail must NOT be claimed.
        let reenabled = out.entries.iter().find(|e| e.input == "KeyQ").unwrap();
        assert!(matches!(
            reenabled.grade,
            super::super::provenance::ProvenanceGrade::ManagerPipelineOk
        ));
        assert!(reenabled.grade_is_consistent());
        assert!(!out.findings.iter().any(|f| f.input == "KeyQ"
            && matches!(
                f.kind,
                super::super::surface::DiscoveryFindingKind::DeclaredButDead
            )));

        // Per-arm alarm detail (M2): the disabled arm and the
        // pressed-never-fired arm must say what they proved.
        let disabled_alarm = dead.iter().find(|f| f.input == "KeyN").unwrap();
        assert!(disabled_alarm.detail.contains("disabled"));
        let pressed_alarm = dead.iter().find(|f| f.input == "KeyO").unwrap();
        assert!(
            pressed_alarm.detail.contains("never fired")
                && !pressed_alarm.detail.contains("disabled"),
            "the pressed-never-fired arm must not claim a disable"
        );

        // Alive: upgraded grade, TapObserved evidence at the
        // manager_pipeline stage.
        let alive = out.entries.iter().find(|e| e.input == "KeyJ").unwrap();
        assert!(matches!(
            alive.grade,
            super::super::provenance::ProvenanceGrade::ManagerPipelineOk
        ));
        assert!(alive
            .evidence
            .iter()
            .any(|e| matches!(e, super::super::surface::Evidence::TapObserved { stage } if stage == "manager_pipeline")));
        assert!(alive.grade_is_consistent());

        // Declared-not-tapped entries stay `Declared` and consistent.
        for input in ["KeyN", "KeyO", "KeyP", "KeyI"] {
            let e = out.entries.iter().find(|e| e.input == input).unwrap();
            assert!(
                matches!(e.grade, super::super::provenance::ProvenanceGrade::Declared),
                "{input} stays Declared"
            );
            assert!(e.grade_is_consistent());
        }

        // Chord member: note present, no alarm, no ContextUnverified.
        let chord = out.entries.iter().find(|e| e.input == "KeyI").unwrap();
        assert!(chord.notes.iter().any(|n| n.contains("chord")));
        assert!(!out.findings.iter().any(|f| f.input == "KeyI"));
    }

    /// HIGH-1: `fired_frames` is action-scoped while `pressed_frames`
    /// used to arm the pressed-never-fired alarm for EVERY binding of
    /// the action — a multi-bound action whose ALIVE binding was
    /// pressed would false-alarm its never-pressed siblings. The
    /// per-binding `pressed_inputs` evidence must attribute the arm.
    #[test]
    fn reconcile_multi_binding_alarm_attributes_the_pressed_binding_only() {
        // One action, two bindings: only KeyJ (the dead wiring) is
        // ever pressed; KeyK never sees a press. The action never
        // fires. Exactly KeyJ alarms — KeyK has no pressed-input
        // evidence and absence of a press proves nothing.
        {
            let declared = vec![decl("Dual", "KeyJ", false), decl("Dual", "KeyK", false)];
            let stats = vec![
                stat("Dual", "KeyJ", 2, 0, false),
                // Same (map, action) row observed twice (two
                // entities' ActionStates): the pressed-input union
                // is what feeds the alarm arm.
                stat("Dual", "KeyJ", 2, 0, false),
            ];
            let evidence = LwRunEvidence { declared, stats };
            let out = reconcile(&evidence, &Default::default());
            let dead: Vec<&str> = out
                .findings
                .iter()
                .filter(|f| {
                    matches!(
                        f.kind,
                        super::super::surface::DiscoveryFindingKind::DeclaredButDead
                    )
                })
                .map(|f| f.input.as_str())
                .collect();
            assert_eq!(
                dead,
                ["KeyJ"],
                "the pressed binding alarms; the never-pressed sibling must not \
                 (multi-binding false-positive, HIGH-1)"
            );
        }

        // Firing silences BOTH bindings' pressed-arms: `fired_frames`
        // is action-scoped, so wiring proven alive cannot also be
        // proven dead — even for the binding that never fired.
        {
            let declared = vec![decl("Dual", "KeyJ", false), decl("Dual", "KeyK", false)];
            let stats = vec![stat("Dual", "KeyJ", 2, 1, false)];
            let evidence = LwRunEvidence { declared, stats };
            let out = reconcile(&evidence, &Default::default());
            assert!(
                !out.findings.iter().any(|f| matches!(
                    f.kind,
                    super::super::surface::DiscoveryFindingKind::DeclaredButDead
                )),
                "an action that fired (via ANY binding) is wiring-alive: no \
                 binding of it may alarm"
            );
            // The fired action upgrades BOTH bindings —
            // ManagerPipelineOk is action-scoped truth.
            assert!(out.entries.iter().all(|e| matches!(
                e.grade,
                super::super::provenance::ProvenanceGrade::ManagerPipelineOk
            )));
        }
    }

    #[test]
    fn tap_ok_but_game_ignores_is_context_unverified_not_alarm() {
        // The honesty-spine case: wiring fires, sweep never saw game
        // liveness. NOTE, never alarm.
        let evidence = LwRunEvidence {
            declared: vec![decl("Ignored", "KeyH", false)],
            stats: vec![stat("Ignored", "KeyH", 1, 1, false)],
        };
        let live: std::collections::BTreeSet<String> = Default::default();
        let out = reconcile(&evidence, &live);
        assert!(out.findings.iter().any(|f| matches!(
            f.kind,
            super::super::surface::DiscoveryFindingKind::ContextUnverified
        )));
        assert!(!out.findings.iter().any(|f| matches!(
            f.kind,
            super::super::surface::DiscoveryFindingKind::DeclaredButDead
        )));
    }

    #[test]
    fn retraction_covers_all_declared_inputs() {
        let evidence = LwRunEvidence {
            declared: vec![decl("A", "KeyJ", false), decl("B", "KeyK", true)],
            stats: vec![],
        };
        let out = reconcile(&evidence, &Default::default());
        assert_eq!(out.retracted_residue, ["KeyJ", "KeyK"]);
    }

    /// The sweep aggregation: many child reports, one run-equivalent
    /// evidence — declared rows dedupe by full identity, stats rows
    /// merge per (action_map, action) with summed counters, OR'd
    /// disabled flag, and unioned `pressed_inputs` (so the HIGH-1
    /// per-binding gate survives aggregation). `None` when no
    /// child declared anything.
    #[test]
    fn aggregate_lw_evidence_merges_child_reports() {
        use crate::state::PlaytestReport;

        let mut a = PlaytestReport::crashed_empty();
        a.lw_declared = vec![decl("Live", "KeyJ", false), decl("Ignored", "KeyO", false)];
        a.lw_tap_stats = vec![stat("Live", "KeyJ", 1, 1, false)];

        // Second child: same declared rows (deduped), plus its OWN
        // tap of the same action from a different press frame and a
        // disabled sibling row.
        let mut b = PlaytestReport::crashed_empty();
        b.lw_declared = vec![decl("Ignored", "KeyO", false), decl("Live", "KeyJ", false)];
        b.lw_tap_stats = vec![stat("Live", "KeyJ", 1, 0, false)];

        let evidence =
            aggregate_lw_evidence(&[&a, &b]).expect("declared rows exist: evidence expected");
        assert_eq!(
            evidence.declared.len(),
            2,
            "identical declared rows across children dedupe to one each"
        );
        // Deterministic (action_map, action, input) order.
        assert_eq!(
            evidence
                .declared
                .iter()
                .map(|d| d.action.as_str())
                .collect::<Vec<_>>(),
            ["Ignored", "Live"]
        );
        assert_eq!(evidence.stats.len(), 1, "same (map, action) merges");
        let row = &evidence.stats[0];
        assert_eq!(
            (row.pressed_frames, row.fired_frames, row.disabled),
            (2, 1, false)
        );
        assert_eq!(
            row.pressed_inputs,
            std::iter::once("KeyJ".to_string()).collect::<std::collections::BTreeSet<_>>(),
            "per-binding evidence unions (still just KeyJ — only KeyJ was pressed)"
        );

        // A child with no Leafwing state contributes nothing; all
        // empty → None (honest absence).
        let empty = PlaytestReport::crashed_empty();
        assert!(aggregate_lw_evidence(&[&empty, &empty]).is_none());
    }

    /// M1: LW serializes `MouseButton` as the bare variant name
    /// ("Left"/"Back"/…) and the reader must map it into the
    /// scenario-canonical mouse space via
    /// `normalize_mouse_button_name` — never through
    /// `normalize_key_name` (whose "left" spelling means ArrowLeft).
    /// All five canonical buttons must join; keys must never become
    /// mouse names and vice versa.
    #[test]
    fn mouse_button_payloads_join_canonical_mouse_names() {
        for (raw, canonical) in [
            ("Left", "Left"),
            ("Right", "Right"),
            ("Middle", "Middle"),
            ("Back", "Back"),
            ("Forward", "Forward"),
        ] {
            let input =
                key_name_from_payload("MouseButton", &serde_json::Value::String(raw.into()));
            assert_eq!(
                input.as_deref(),
                Some(canonical),
                "MouseButton::{raw} must join the canonical mouse space"
            );
        }
        // Unknown mouse variants are dropped, not guessed.
        assert!(
            key_name_from_payload("MouseButton", &serde_json::Value::String("Other".into()))
                .is_none()
        );
        // The key path is untouched: "Left" as a KEY payload would
        // mean nothing (KeyCode has no "Left" variant), and the
        // canonical mouse names never leak into key normalization.
        assert_eq!(
            key_name_from_payload("KeyCode", &serde_json::Value::String("KeyJ".into())).as_deref(),
            Some("KeyJ")
        );
        // Chord member mouse buttons fan out with the same mapping.
        let mut rows = Vec::new();
        parse_typetag(
            r#"{"ButtonlikeChord":[{"MouseButton":"Left"},{"KeyCode":"KeyJ"}]}"#,
            "Combo",
            "m::A",
            &mut rows,
        );
        assert!(rows
            .iter()
            .any(|r| r.chord && r.input == "Left" && r.input_kind == "MouseButton"));
        assert!(rows
            .iter()
            .any(|r| r.chord && r.input == "KeyJ" && r.input_kind == "KeyCode"));
    }
}
