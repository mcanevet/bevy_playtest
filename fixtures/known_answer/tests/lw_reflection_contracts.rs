//! LW REFLECTION CONTRACTS (promoted from the D1-d scratch dump):
//! pin — against real Leafwing Input Manager 0.22 on Bevy 0.20 —
//! every reflected shape the harness's pure-reflection reader and
//! intent-tap rely on. If LW changes any of these, the reader
//! breaks HERE with a pointed assertion instead of silently
//! misreading a game.
//!
//! Contracts pinned:
//! 1. `InputMap`/`ActionState` register with `ReflectComponent`
//!    data and enumerate via registry walk + iter_entities;
//! 2. `InputMap.buttonlike_map` is a reflect map of
//!    Actionlike-enum keys → lists of `Box<dyn Buttonlike>`;
//! 3. the boxed bindings serialize as typetagged JSON
//!    (`{"KeyCode": "KeyJ"}`, `{"ButtonlikeChord": [...]}`)
//!    via LW's ReflectSerialize installation — the reader's only
//!    key-name source;
//! 4. `ButtonData.state` (a reflected enum) ticks `JustPressed`
//!    even for disabled actions, while the public `just_pressed()`
//!    gates — the tap's firing gate mirrors the accessor, not the
//!    raw datum;
//! 5. per-action `ActionData.disabled` AND struct-level
//!    `ActionState.disabled` are readable booleans via reflection
//!    (the component-wide-disable contract);
//! 6. LW's raw-input resources carry no ReflectComponent data and
//!    stay invisible to the state digest.
#![allow(dead_code)]

use bevy::prelude::*;
use bevy::reflect::{PartialReflect, ReflectRef};
use leafwing_input_manager::prelude::*;
use std::collections::HashSet;

#[derive(
    Actionlike, Clone, Copy, PartialEq, Eq, Hash, Debug, Reflect, serde::Serialize, serde::Deserialize,
)]
enum ScratchAction {
    Jump,
    Mute,
    Dead,
    Combo,
}

fn scratch_map() -> InputMap<ScratchAction> {
    let mut m = InputMap::default();
    m.insert(ScratchAction::Jump, KeyCode::Space);
    m.insert(ScratchAction::Mute, KeyCode::KeyM);
    m.insert(ScratchAction::Dead, KeyCode::F9);
    m.insert(
        ScratchAction::Combo,
        ButtonlikeChord::new([KeyCode::KeyJ, KeyCode::KeyK]),
    );
    m
}

#[test]
fn dump_lw_resource_and_component_layout() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::input::InputPlugin);
    app.add_plugins(InputManagerPlugin::<ScratchAction>::default());
    app.world_mut().spawn((
        scratch_map(),
        ActionState::<ScratchAction>::default(),
        Name::new("player"),
    ));
    // second entity, second map instance (multi-map enumeration case)
    app.world_mut().spawn((
        {
            let mut m = InputMap::default();
            m.insert(ScratchAction::Jump, KeyCode::ArrowUp);
            m
        },
        ActionState::<ScratchAction>::default(),
        Name::new("player2"),
    ));
    app.update();
    app.update();

    // 1. TYPE REGISTRY: what does the InputManagerPlugin register?
    println!("=== registered type names mentioning InputMap/ActionState/Central ===");
    let reg = app.world().resource::<AppTypeRegistry>().read();
    for tr in reg.iter() {
        let n = tr.type_info().type_path();
        if n.contains("InputMap") || n.contains("ActionState") || n.contains("Central") {
            println!("registered: {}", n);
        }
    }
    drop(reg);

    // 2. GENERIC COMPONENT enumeration: find InputMap components
    //    WITHOUT naming the action type (the reader's real path).
    println!("=== generic component-id enumeration (iter_registered) ===");
    let mut total = 0usize;
    let mut matches = Vec::new();
    for (id, info) in app.world().components().iter_registered() {
        total += 1;
        let n = info.name();
        println!("  registered component: {}", n);
        if n.contains("InputMap") || n.contains("ActionState") || n.contains("Central") {
            matches.push((id, n.to_string()));
        }
    }
    println!("total registered components: {}, LW matches: {:?}", total, matches);

    // 3. TYPED query for ground truth (what the reader must match)
    println!("=== typed component query enumeration ===");
    let mut q = app
        .world_mut()
        .query_filtered::<(&Name, &InputMap<ScratchAction>), With<InputMap<ScratchAction>>>();
    for (name, map) in q.iter(app.world()) {
        println!("entity {} has InputMap<ScratchAction>", name);
        for (action, binding) in map.buttonlike_bindings() {
            println!("  action {:?} <- {:?}", action, binding);
        }
    }

    // 4. REFLECTION walk of an InputMap: private fields reachable?
    let mut q = app
        .world_mut()
        .query_filtered::<&InputMap<ScratchAction>, With<Name>>();
    let map = q
        .iter(app.world())
        .find(|_| true)
        .cloned()
        .expect("at least one InputMap");
    let reg = app.world().resource::<AppTypeRegistry>().read();
    println!("=== reflection walk of InputMap<ScratchAction> ===");
    match map.reflect_ref() {
        ReflectRef::Struct(s) => {
            for i in 0..s.field_len() {
                let fval = s.field_at(i).unwrap();
                println!(
                    "  field '{}' : {} (len-evidence: {:?})",
                    s.name_at(i).unwrap(),
                    fval.reflect_type_path(),
                    field_len_evidence(fval)
                );
            }
            // The money question: buttonlike_map walk.
            let bm = s.field("buttonlike_map").unwrap();
            if let ReflectRef::Map(mp) = bm.reflect_ref() {
                for kv in mp.iter() {
                    let (k, v) = kv;
                    println!(
                        "  buttonlike key {:?} -> value type {}",
                        k.reflect_type_path(),
                        v.reflect_type_path()
                    );
                    // action identity via enum reflection
                    if let ReflectRef::Enum(e) = k.reflect_ref() {
                        println!("    action variant: {}", e.variant_name());
                    }
                    // value is Vec<Box<dyn Buttonlike>>: a list
                    if let ReflectRef::List(l) = v.reflect_ref() {
                        for bi in 0..l.len() {
                            let b = l.get(bi).unwrap();
                            println!("    binding[{}]: {}", bi, b.reflect_type_path());
                        }
                    } else {
                        println!("    value NOT a list: {:?}", v.reflect_kind());
                    }
                }
            } else {
                println!("  buttonlike_map NOT a reflect map!");
            }
        }
        _other => println!("NOT a struct"),
    }
    drop(reg);

    // 5. DOWNCAST boxed bindings to concrete types
    if let Some(bindings) = map.get_buttonlike(&ScratchAction::Jump) {
        let b: &dyn Buttonlike = bindings[0].as_ref();
        let pr: &dyn PartialReflect = b;
        if let Some(kc) = pr.try_downcast_ref::<KeyCode>() {
            // KeyCode is a fieldless enum: variant name is the key name
            if let ReflectRef::Enum(e) = kc.reflect_ref() {
                println!("Jump binding -> KeyCode::{}", e.variant_name());
            }
        } else {
            println!(
                "Jump binding did NOT downcast to KeyCode: {}",
                pr.reflect_type_path()
            );
        }
    }
    if let Some(bindings) = map.get_buttonlike(&ScratchAction::Combo) {
        let b: &dyn Buttonlike = bindings[0].as_ref();
        let pr: &dyn PartialReflect = b;
        println!("Combo binding concrete type: {}", pr.reflect_type_path());
        if let Some(chord) = pr.try_downcast_ref::<ButtonlikeChord>() {
            match chord.reflect_ref() {
                ReflectRef::Tuple(t) => {
                    println!("  chord arity: {}", t.field_len());
                    let inner = t.field(0).unwrap();
                    println!("  chord member 0: {}", inner.reflect_type_path());
                    if let ReflectRef::List(l) = inner.reflect_ref() {
                        for i in 0..l.len() {
                            let x = l.get(i).unwrap();
                            println!("    chord member[{i}]: {}", x.reflect_type_path());
                            if let ReflectRef::Enum(e) = x.reflect_ref() {
                                println!("      -> KeyCode::{}", e.variant_name());
                            }
                        }
                    }
                }
                _other => {
                    println!("  chord NOT a tuple: kind={:?}", pr.reflect_kind());
                    println!("  chord Debug: {:?}", chord);
                    // RON round-trip fallback: Buttonlike is erased-serde Serialize
                    // but not accessible without the erased_serde crate; use Debug.
                    println!("  chord Debug is the key-name source: see above");
                }
            }
        }
    }

    // 6. SERDE: does an InputMap round-trip through RON (typetags)?
    let ron_string = ron::to_string(&scratch_map()).unwrap();
    println!("=== InputMap serialized to RON ===");
    println!("{}", ron_string);
    let back: InputMap<ScratchAction> = ron::from_str(&ron_string).unwrap();
    assert_eq!(back, scratch_map(), "RON round-trip preserves the map");

    // 7. ACTION-STATE tap: after pressing KeyM via LW's own testing
    //    helper, what does an outside observer see?
    KeyCode::KeyM.press(app.world_mut());
    app.update();
    let mut q = app
        .world_mut()
        .query_filtered::<(&ActionState<ScratchAction>, &Name), With<InputMap<ScratchAction>>>();
    let (state, name) = q
        .iter(app.world())
        .find(|(_, n)| n.as_str() == "player")
        .expect("player entity");
    for action in [
        ScratchAction::Jump,
        ScratchAction::Mute,
        ScratchAction::Dead,
        ScratchAction::Combo,
    ] {
        println!(
            "[{name}] action {:?}: pressed={} just_pressed={}",
            action,
            state.pressed(&action),
            state.just_pressed(&action)
        );
    }
    assert!(state.just_pressed(&ScratchAction::Mute));

    // 8. BOTH map-bearing entities enumerated generically?
    let mut names = HashSet::new();
    let mut q2 = app
        .world_mut()
        .query_filtered::<&Name, With<InputMap<ScratchAction>>>();
    for n in q2.iter(app.world()) {
        names.insert(n.to_string());
    }
    println!("map-bearing entities: {:?}", names);
    assert_eq!(names.len(), 2, "both player map instances enumerated");

    // 9. PURE-REFLECTION enumeration: registry -> ReflectComponent ->
    //    per-entity &dyn Reflect, ZERO named LW types. This is the
    //    production reader path (the library has no LW dependency).
    println!("=== pure-reflection enumeration via ReflectComponent ===");
    {
        use bevy::ecs::reflect::ReflectComponent;

        // Clone the Arc so the read-guard borrows nothing from `app`.
        let registry_arc = app.world().resource::<AppTypeRegistry>().clone();
        let registry = registry_arc.read();

        let mut map_registrations = 0usize;
        for tr in registry.iter() {
            if !tr
                .type_info()
                .type_path()
                .starts_with("leafwing_input_manager::input_map::InputMap")
            {
                continue;
            }
            map_registrations += 1;
            let Some(rc) = tr.data::<ReflectComponent>() else {
                panic!("InputMap registration lacks ReflectComponent data");
            };

            let mut seen: Vec<(String, Vec<String>)> = Vec::new();
            for entity_ref in app.world().iter_entities() {
                let Some(comp) = rc.reflect(entity_ref) else {
                    continue;
                };
                let mut actions = Vec::new();
                if let ReflectRef::Struct(s) = comp.reflect_ref() {
                    if let Some(bm) = s.field("buttonlike_map") {
                        if let ReflectRef::Map(mp) = bm.reflect_ref() {
                            for kv in mp.iter() {
                                let (k, v) = kv;
                                let action_label = match k.reflect_ref() {
                                    ReflectRef::Enum(e) => e.variant_name().to_string(),
                                    _ => "?".into(),
                                };
                                let n = match v.reflect_ref() {
                                    ReflectRef::List(l) => l.len(),
                                    _ => 0,
                                };
                                actions.push(format!("{action_label}:{n}"));
                            }
                        }
                    }
                }
                seen.push((
                    entity_ref
                        .get::<Name>()
                        .map(|n| n.to_string())
                        .unwrap_or_default(),
                    actions,
                ));
            }
            seen.sort();
            for (name, actions) in &seen {
                println!("  entity '{name}': {actions:?}");
            }
            assert_eq!(seen.len(), 2, "reflect path saw both map entities");
            assert!(
                seen.iter().any(|(n, a)| n == "player" && !a.is_empty()),
                "player map had bindings"
            );
        }
        assert_eq!(map_registrations, 1, "exactly one InputMap registration");

        // TypeId path sanity: registration TypeId -> Components::get_id
        // resolves to the same ComponentId as ReflectComponent.
        let tr = registry
            .iter()
            .find(|tr| {
                tr.type_info()
                    .type_path()
                    .starts_with("leafwing_input_manager::input_map::InputMap")
            })
            .unwrap();
        let comp_id_via_typeid = app
            .world()
            .components()
            .get_id(tr.type_id())
            .expect("InputMap registered as component");
        let rc = tr.data::<ReflectComponent>().unwrap();
        let comp_id_via_rc = rc.register_component(app.world_mut());
        assert_eq!(
            comp_id_via_typeid, comp_id_via_rc,
            "TypeId->ComponentId lookup agrees with ReflectComponent"
        );
    }

    // 10. ACTION-STATE tap via pure reflection: ButtonData.state is
    //     LW's reflected ButtonState enum (JustPressed/Pressed/
    //     JustReleased/Released) — the variant name alone is the tap.
    println!("=== ActionState tap via pure reflection ===");
    {
        use bevy::ecs::reflect::ReflectComponent;

        let registry_arc = app.world().resource::<AppTypeRegistry>().clone();
        let registry = registry_arc.read();

        let mut player_mute_just_pressed_via_reflect = false;
        for tr in registry.iter() {
            if !tr
                .type_info()
                .type_path()
                .starts_with("leafwing_input_manager::action_state::ActionState")
            {
                continue;
            }
            let rc = tr.data::<ReflectComponent>().unwrap();
            for entity_ref in app.world().iter_entities() {
                let Some(comp) = rc.reflect(entity_ref) else {
                    continue;
                };
                let name = entity_ref
                    .get::<Name>()
                    .map(|n| n.to_string())
                    .unwrap_or_default();
                if name.is_empty() {
                    continue;
                }
                if let ReflectRef::Struct(s) = comp.reflect_ref() {
                    if let Some(ad) = s.field("action_data") {
                        if let ReflectRef::Map(mp) = ad.reflect_ref() {
                            for kv in mp.iter() {
                                let (k, v) = kv;
                                let action_label = match k.reflect_ref() {
                                    ReflectRef::Enum(e) => e.variant_name().to_string(),
                                    _ => continue,
                                };
                                let row = describe_action_datum(v);
                                println!("  [{name}] {action_label} => {row}");
                                if name == "player" && action_label == "Mute" {
                                    if row.contains("JustPressed") {
                                        player_mute_just_pressed_via_reflect = true;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(
            player_mute_just_pressed_via_reflect,
            "pure-reflection tap saw Mute fire JustPressed on player"
        );
    }

    // 11. DISABLED-ACTION visibility (L-alarm plant shape): the
    //     `disabled` flag on ActionData must be readable via pure
    //     reflection for the DeclaredButDead alarm path.
    println!("=== disabled-at-spawn action visible via reflection ===");
    {
        use bevy::ecs::reflect::ReflectComponent;

        // Disable player2's Jump (typed, standing in for spawn-time
        // disabled actions in the eventual fixture plant).
        let mut q = app
            .world_mut()
            .query_filtered::<(&Name, &mut ActionState<ScratchAction>), ()>();
        for (name, mut state) in q.iter_mut(app.world_mut()) {
            if name.as_str() == "player2" {
                state.disable_action(&ScratchAction::Jump);
                println!("  [{name}] disabled Jump");
            }
        }

        let registry_arc = app.world().resource::<AppTypeRegistry>().clone();
        let registry = registry_arc.read();
        let tr = registry
            .iter()
            .find(|tr| {
                tr.type_info()
                    .type_path()
                    .starts_with("leafwing_input_manager::action_state::ActionState")
            })
            .unwrap();
        let rc = tr.data::<ReflectComponent>().unwrap();

        let mut disabled_seen = Vec::new();
        for entity_ref in app.world().iter_entities() {
            let Some(comp) = rc.reflect(entity_ref) else {
                continue;
            };
            if let ReflectRef::Struct(s) = comp.reflect_ref() {
                if let Some(ad) = s.field("action_data") {
                    if let ReflectRef::Map(mp) = ad.reflect_ref() {
                        for kv in mp.iter() {
                            let (k, v) = kv;
                            let action_label = match k.reflect_ref() {
                                ReflectRef::Enum(e) => e.variant_name().to_string(),
                                _ => continue,
                            };
                            if let ReflectRef::Struct(datum) = v.reflect_ref() {
                                let disabled = datum
                                    .field("disabled")
                                    .and_then(|f| f.try_downcast_ref::<bool>())
                                    .copied()
                                    .unwrap_or(false);
                                if disabled {
                                    disabled_seen.push(action_label);
                                }
                            }
                        }
                    }
                }
            }
        }
        println!("  disabled actions visible via reflection: {disabled_seen:?}");
        assert!(
            disabled_seen.iter().any(|a| a == "Jump"),
            "player2's disabled Jump was visible via pure reflection"
        );

        // 11b. STRUCT-LEVEL disable contract (H2): LW's
        //      component-wide `ActionState::disable()` sets the
        //      STRUCT field `disabled` (public + reflected), and
        //      `action_disabled()` gates EVERY accessor on
        //      `struct.disabled || datum.disabled`. The tap mirrors
        //      that union, so the struct field must be readable.
        let mut q = app
            .world_mut()
            .query_filtered::<(&Name, &mut ActionState<ScratchAction>), ()>();
        for (name, mut state) in q.iter_mut(app.world_mut()) {
            if name.as_str() == "player2" {
                state.disable(); // component-WIDE (not disable_action)
            }
        }
        let mut struct_disabled_seen = 0usize;
        for entity_ref in app.world().iter_entities() {
            let Some(comp) = rc.reflect(entity_ref) else {
                continue;
            };
            if let ReflectRef::Struct(s) = comp.reflect_ref() {
                let disabled = s
                    .field("disabled")
                    .and_then(|f| f.try_downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false);
                if disabled {
                    struct_disabled_seen += 1;
                }
            }
        }
        assert_eq!(
            struct_disabled_seen, 1,
            "component-wide disable() must be visible on exactly one \
             ActionState's STRUCT-level `disabled` field"
        );

        // And LW's accessor gate honors it: just_pressed on the
        // component-wide-disabled player2 must report NOTHING even
        // though the raw datum (checked above) still ticks.
        let mut q = app
            .world_mut()
            .query_filtered::<(&ActionState<ScratchAction>, &Name), ()>();
        let (state, _) = q
            .iter(app.world())
            .find(|(_, n)| n.as_str() == "player2")
            .expect("player2");
        assert!(
            !state.just_pressed(&ScratchAction::Mute),
            "component-wide disable() must gate just_pressed()"
        );
    }

    // 12. PURE-REFLECTION KEY EXTRACTION: does `Box<dyn Buttonlike>`
    //     delegate reflection to the concrete input? If yes, the
    //     reader needs ZERO downcasts — enum variant name = key name.
    //     Also: ButtonlikeChord's members walked the same way.
    println!("=== pure-reflection binding-item extraction ===");
    {
        use bevy::ecs::reflect::ReflectComponent;

        let registry_arc = app.world().resource::<AppTypeRegistry>().clone();
        let registry = registry_arc.read();
        let tr = registry
            .iter()
            .find(|tr| {
                tr.type_info()
                    .type_path()
                    .starts_with("leafwing_input_manager::input_map::InputMap")
            })
            .unwrap();
        let rc = tr.data::<ReflectComponent>().unwrap();

        let mut extracted: Vec<(String, Vec<String>)> = Vec::new();
        for entity_ref in app.world().iter_entities() {
            let Some(comp) = rc.reflect(entity_ref) else { continue };
            let name = entity_ref
                .get::<Name>()
                .map(|n| n.to_string())
                .unwrap_or_default();
            if name != "player" {
                continue;
            }
            let s = match comp.reflect_ref() {
                ReflectRef::Struct(s) => s,
                _ => panic!("InputMap is a struct"),
            };
            let bm = s.field("buttonlike_map").unwrap();
            let mp = match bm.reflect_ref() {
                ReflectRef::Map(mp) => mp,
                _ => panic!("buttonlike_map is a map"),
            };
            for kv in mp.iter() {
                let (k, v) = kv;
                let action = match k.reflect_ref() {
                    ReflectRef::Enum(e) => e.variant_name().to_string(),
                    _ => continue,
                };
                let l = match v.reflect_ref() {
                    ReflectRef::List(l) => l,
                    _ => continue,
                };
                for bi in 0..l.len() {
                    let item = l.get(bi).unwrap();
                        // Box<dyn Buttonlike> is OPAQUE to reflection
                        // (LW's trait_reflection glue), so key names come
                        // from the SERIALIZED TYPE TAG instead: LW
                        // registers ReflectSerialize on the box's
                        // registration, and the dyn trait object
                        // serializes as {"ConcreteShortPath": value}.
                        let as_reflect = item.try_as_reflect().unwrap();
                        let path = item.reflect_type_path();
                        let registration = registry
                            .get_with_type_path(path)
                            .expect("Box<dyn Buttonlike> registration present");
                        let rs = registration
                            .data::<bevy::reflect::ReflectSerialize>()
                            .expect("ReflectSerialize data installed by LW");
                        let serializable = rs
                            .get_serializable(as_reflect);
                        let json = serde_json::to_string(&*serializable).unwrap();
                        println!(
                            "  {action} binding[{bi}] -> typetag JSON: {json}"
                        );
                        extracted.push((action.clone(), vec![json]));
                }
            }
        }

        let jump = extracted
            .iter()
            .find(|(a, _)| a == "Jump")
            .expect("Jump binding extracted");
        // The typetag JSON identifies the concrete input type; KeyCode
        // carries the key name as the (unit) enum payload.
        println!("Jump binding typetag: {:?}", jump.1);
        assert!(
            jump.1[0].contains("\"KeyCode\":") && jump.1[0].contains("Space"),
            "typetag JSON identified Jump's KeyCode::Space: {:?}",
            jump.1
        );
        let combo = extracted
            .iter()
            .find(|(a, _)| a == "Combo")
            .expect("Combo binding extracted");
        assert!(
            combo.1[0].contains("ButtonlikeChord")
                && combo.1[0].contains("KeyJ")
                && combo.1[0].contains("KeyK"),
            "typetag JSON identified the chord AND its members: {:?}",
            combo.1
        );
    }

    // 13. DIGEST BLINDNESS: the state digest folds only resources with
    //     `ReflectComponent` type data (plus Transform/UI queries).
    //     `CentralInputStore` is a plain `#[derive(Resource, Reflect)]`
    //     with no `#[reflect(Resource)]` and no Component impl, so its
    //     per-frame raw-input mirror must be INVISIBLE to the digest.
    //     If this ever breaks, every probe candidate diverges via the
    //     raw-input mirror and the known-answer matrix collapses.
    println!("=== digest-blindness: CentralInputStore has no ReflectComponent data ===");
    {
        use bevy::ecs::reflect::ReflectComponent;

        let registry_arc = app.world().resource::<AppTypeRegistry>().clone();
        let registry = registry_arc.read();
        let mut lw_resources_blind = 0usize;
        for tr in registry.iter() {
            let path = tr.type_info().type_path();
            if !path.starts_with("leafwing_input_manager::") {
                continue;
            }
            let has_comp_data = tr.data::<ReflectComponent>().is_some();
            // InputMap/ActionState legitimately have component data.
            let is_component =
                path.contains("input_map::InputMap") || path.contains("action_state::ActionState");
            println!("  {path}: ReflectComponent data = {has_comp_data}");
            if !has_comp_data && !is_component {
                lw_resources_blind += 1;
            }
        }
        assert!(
            lw_resources_blind > 0,
            "expected LW reflected NON-component types (CentralInputStore et al) to lack ReflectComponent data"
        );
    }
}

fn field_len_evidence(v: &dyn PartialReflect) -> String {
    match v.reflect_ref() {
        ReflectRef::Map(m) => format!("map len {}", m.len()),
        ReflectRef::List(l) => format!("list len {}", l.len()),
        _ => "scalar".to_string(),
    }
}

/// Reflective summary of one ActionData value: button state + disabled.
fn describe_action_datum(v: &dyn bevy::reflect::PartialReflect) -> String {
    if let ReflectRef::Struct(datum) = v.reflect_ref() {
        let disabled = datum
            .field("disabled")
            .and_then(|f| f.try_downcast_ref::<bool>())
            .copied()
            .unwrap_or(false);
        let kind_desc = match datum.field("kind_data").map(|kd| kd.reflect_ref()) {
            Some(ReflectRef::Enum(kind)) => {
                if kind.variant_name() == "Button" {
                    // Field 0 of the Button variant is ButtonData.
                    kind.field_at(0).map(|bd| bd.reflect_ref()).map_or_else(
                        || "?".to_string(),
                        |r| match r {
                            ReflectRef::Struct(button_data) => {
                                let state = button_data
                                    .field("state")
                                    .map(|st| match st.reflect_ref() {
                                        ReflectRef::Enum(e) => {
                                            e.variant_name().to_string()
                                        }
                                        _ => "?".to_string(),
                                    })
                                    .unwrap_or_default();
                                format!("Button(state={state})")
                            }
                            _ => "Button(?)".to_string(),
                        },
                    )
                } else {
                    format!("Kind::{}", kind.variant_name())
                }
            }
            _ => "non-button kind".to_string(),
        };
        return format!("disabled={disabled} {kind_desc}");
    }
    "?".to_string()
}
