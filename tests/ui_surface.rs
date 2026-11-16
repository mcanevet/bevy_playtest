//! Headless tests for the declared UI-surface enumeration
//! (`ui-surface` feature, src/discovery/ui_surface.rs).
//!
//! The test app mirrors `bevy_ui`'s own layout-test bootstrap
//! (crates/`bevy_ui`/src/layout/mod.rs `setup_ui_test_app`): a
//! `Camera2d` with a synthetic `RenderTargetInfo` + explicit
//! `Viewport` provides layout a render target without a GPU, and
//! `UiPlugin` schedules the full layout/stack/clipping pipeline.
//! `bevy_ui_widgets`' plugin adds widget observers so widget-marker
//! enumeration matches a real game's world.

#[path = "common/mod.rs"]
mod common;

#[cfg(feature = "ui-surface")]
mod ui_tests {
    use bevy_app::prelude::*;
    use bevy_camera::{
        visibility::Visibility, Camera, Camera2d, ComputedCameraValues, RenderTargetInfo, Viewport,
    };
    use bevy_ecs::prelude::*;
    use bevy_input::prelude::*;
    use bevy_math::{UVec2, Vec2};
    use bevy_picking::events::PointerClick;
    use bevy_picking::Pickable;
    use bevy_playtest::discovery::ui_surface::{enumerate_ui, InteractableKind, WidgetRole};
    use bevy_transform::prelude::*;
    use bevy_ui::prelude::*;
    use bevy_ui::UiPlugin;
    use bevy_ui_widgets as widgets;
    use bevy_utils::default;

    const TARGET_WIDTH: u32 = 1280;
    const TARGET_HEIGHT: u32 = 720;

    fn setup_app() -> App {
        let mut app = App::new();
        app.add_plugins((
            TaskPoolPlugin::default(),
            bevy_asset::AssetPlugin::default(),
            bevy_window::WindowPlugin::default(),
            TransformPlugin,
            bevy_text::TextPlugin,
            UiPlugin,
            bevy_camera::visibility::VisibilityPlugin,
            bevy_input_focus::InputFocusPlugin,
            // Registers pointer messages + `PointerState` that UiPlugin's
            // text-interaction and widget systems read.
            bevy_picking::InteractionPlugin,
            bevy_picking::PickingPlugin,
            widgets::UiWidgetsPlugins,
        ));
        app.init_resource::<bevy_text::TextPipeline>();
        app.init_resource::<bevy_text::FontCx>();
        app.init_resource::<bevy_text::ScaleCx>();
        // `update_inline_image_boxes` and `update_image_content_size_system`
        // (UiPlugin) read `Assets<Image>` / `Assets<TextureAtlasLayout>`;
        // normally registered by render's `ImagePlugin`, absent headless.
        bevy_asset::AssetApp::init_asset::<bevy_image::Image>(&mut app);
        bevy_asset::AssetApp::init_asset::<bevy_image::TextureAtlasLayout>(&mut app);
        // `VisibilityPlugin`'s auto-AABB systems read `Assets<Mesh>`.
        bevy_asset::AssetApp::init_asset::<bevy_mesh::Mesh>(&mut app);
        bevy_asset::AssetApp::init_asset::<bevy_mesh::skinning::SkinnedMeshInverseBindposes>(
            &mut app,
        );
        // Resources the UiPlugin/widget systems read that are normally
        // registered by render/time/picking plugin groups, absent headless.
        app.init_resource::<bevy_time::Time<bevy_time::Real>>();
        app.init_resource::<UiScale>();
        // Resources that `ui_focus_system` requires; normally provided by
        // the windowing/input plugin group, absent in this headless test.
        app.init_resource::<ButtonInput<MouseButton>>();
        app.init_resource::<ButtonInput<KeyCode>>();
        app.init_resource::<Touches>();
        app.init_resource::<bevy_input_focus::InputFocus>();
        app.init_resource::<bevy_input_focus::InputFocusVisible>();

        let world = app.world_mut();
        // Dummy headless camera (the bevy_ui layout-test pattern).
        world.spawn((
            Camera2d,
            Camera {
                computed: ComputedCameraValues {
                    target_info: Some(RenderTargetInfo {
                        physical_size: UVec2::new(TARGET_WIDTH, TARGET_HEIGHT),
                        scale_factor: 1.,
                    }),
                    ..Default::default()
                },
                viewport: Some(Viewport {
                    physical_size: UVec2::new(TARGET_WIDTH, TARGET_HEIGHT),
                    ..Default::default()
                }),
                ..Default::default()
            },
        ));
        app
    }

    fn run_frames(app: &mut App, n: usize) {
        for _ in 0..n {
            app.update();
        }
    }

    #[test]
    fn buttons_are_enumerated_with_geometry_and_labels() {
        let mut app = setup_app();

        let world = app.world_mut();
        let button = world
            .spawn((
                Node {
                    width: Val::Px(200.),
                    height: Val::Px(80.),
                    position_type: PositionType::Absolute,
                    left: Val::Px(100.),
                    top: Val::Px(50.),
                    ..default()
                },
                widgets::Button,
                Pickable::default(),
            ))
            .with_children(|parent| {
                parent.spawn((Text::new("Resume"),));
            })
            .id();

        run_frames(&mut app, 2);

        let surface = enumerate_ui(app.world());
        let hit = surface
            .interactables
            .iter()
            .find(|i| i.entity == button)
            .expect("button must be enumerated");
        assert!(hit.kinds.contains(&InteractableKind::Widget));
        assert!(hit.kinds.contains(&InteractableKind::Pickable));
        assert_eq!(hit.role, WidgetRole::Button);
        assert_eq!(hit.label.as_deref(), Some("Resume"));

        let rect = hit.rect.expect("geometry must resolve");
        // Absolute (100, 50), 200x80 → window-space rect (100,50)-(300,130).
        assert!((rect.min.x - 100.).abs() < 0.5, "got {rect:?}");
        assert!((rect.min.y - 50.).abs() < 0.5, "got {rect:?}");
        assert!((rect.max.x - 300.).abs() < 0.5, "got {rect:?}");
        assert!((rect.max.y - 130.).abs() < 0.5, "got {rect:?}");
        assert_eq!(hit.center, Some(Vec2::new(200., 90.)));
    }

    #[test]
    fn tab_index_focusable_is_enumerated() {
        let mut app = setup_app();

        app.world_mut().spawn((
            Node {
                width: Val::Px(50.),
                height: Val::Px(50.),
                position_type: PositionType::Absolute,
                left: Val::Px(10.),
                top: Val::Px(10.),
                ..default()
            },
            bevy_input_focus::tab_navigation::TabIndex(0),
        ));

        run_frames(&mut app, 2);

        let surface = enumerate_ui(app.world());
        assert!(
            surface
                .interactables
                .iter()
                .any(|i| i.kinds.contains(&InteractableKind::Focusable)),
            "TabIndex entity must be enumerated"
        );
    }

    #[test]
    fn pointer_observer_declares_interactable() {
        let mut app = setup_app();

        // A bare Node with a click observer: no markers at all. This is
        // the "game wired clicks imperatively" case that marker-based
        // enumeration misses entirely.
        app.world_mut()
            .spawn(Node {
                width: Val::Px(64.),
                height: Val::Px(64.),
                position_type: PositionType::Absolute,
                left: Val::Px(0.),
                top: Val::Px(0.),
                ..default()
            })
            .observe(|_: On<PointerClick>| {});

        run_frames(&mut app, 2);

        let surface = enumerate_ui(app.world());
        assert!(
            surface
                .interactables
                .iter()
                .any(|i| i.kinds.contains(&InteractableKind::Observed)),
            "entity with a click observer must be enumerated"
        );
    }

    #[test]
    fn hidden_nodes_are_excluded() {
        let mut app = setup_app();

        let world = app.world_mut();
        let hidden = world
            .spawn((
                Node {
                    width: Val::Px(50.),
                    height: Val::Px(50.),
                    position_type: PositionType::Absolute,
                    left: Val::Px(10.),
                    top: Val::Px(10.),
                    ..default()
                },
                Visibility::Hidden,
                widgets::Button,
            ))
            .id();

        run_frames(&mut app, 2);

        let surface = enumerate_ui(app.world());
        assert!(
            !surface.interactables.iter().any(|i| i.entity == hidden),
            "hidden button must be excluded"
        );
    }

    #[test]
    fn focused_flag_tracks_input_focus() {
        let mut app = setup_app();

        let world = app.world_mut();
        let target = world
            .spawn((
                Node {
                    width: Val::Px(50.),
                    height: Val::Px(50.),
                    position_type: PositionType::Absolute,
                    ..default()
                },
                widgets::Button,
            ))
            .id();
        world.insert_resource(bevy_input_focus::InputFocus::from_entity(target));

        run_frames(&mut app, 2);

        let surface = enumerate_ui(app.world());
        let hit = surface
            .interactables
            .iter()
            .find(|i| i.entity == target)
            .expect("focused button enumerated");
        assert!(hit.focused);
    }

    // Honesty doctrine: enumeration is deliberately incomplete (global
    // and component-scoped pointer observers, deprecated
    // Interaction/Button, third-party action maps, and anything
    // spawned after the capture-time snapshot), so the result must
    // SAY so — `complete: false` with the corresponding notes.
    #[test]
    fn result_admits_omissions() {
        let mut app = setup_app();
        run_frames(&mut app, 2);

        let surface = enumerate_ui(app.world());
        assert!(!surface.complete, "omissions documented: {:?}", surface);
        assert!(
            surface
                .notes
                .iter()
                .any(|n| n.contains("pointer observers")),
            "observer omission noted: {:?}",
            surface.notes
        );
    }
}

// Bot-seam coverage: the enumerated widget centers must actually be
// CLICKABLE through the full driver pipeline — capture-once →
// UiClickTargets → chaos mouse interleave → wrapped WindowEvent →
// picking's hit test → the widget's own entity. A center in the
// report that picking refuses to target would make the entire
// interleave silently vacuous. Needs the full plugin graph
// (`bevy` meta-crate), hence the dual feature gate.
#[cfg(all(feature = "ui-surface", feature = "takeover"))]
mod bot_seam {
    use super::common;
    use bevy::prelude::*;
    use bevy_camera::visibility::Visibility;
    use bevy_playtest::run_scenario;
    use bevy_playtest::scenario::Scenario;
    use bevy_utils::default;

    #[derive(Resource, Default, Debug)]
    struct ClickProbe {
        clicks_on_widget: u32,
        widget_entity: Option<Entity>,
    }

    fn setup_widget(mut commands: Commands, mut probe: ResMut<ClickProbe>) {
        // SMALL widget on purpose (~2% of the 1280x720 default
        // window): the legacy quantized-random path hits it only
        // ~2% of the time, so sustained targeted clicks prove the
        // enumerated-center interleave — a fullscreen widget would
        // let the legacy path satisfy the assertion on its own.
        let entity = commands
            .spawn((
                Node {
                    width: Val::Px(200.),
                    height: Val::Px(100.),
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.),
                    top: Val::Px(0.),
                    ..default()
                },
                bevy_ui_widgets::Button,
                bevy_picking::Pickable::default(),
                // NOTE: explicit `Visibility::Visible` — `Default` is
                // Hidden (anti-flash), which the enumeration would
                // correctly exclude.
                Visibility::Visible,
            ))
            .id();
        probe.widget_entity = Some(entity);
        // A camera gives the UI layout a target and picking a view
        // (mirrors the input_delivery picking-probe bootstrap).
        commands.spawn(Camera2d);
    }

    #[test]
    fn enumerated_centers_are_clickable_targets() {
        let _slot = common::run_slot();
        // Chaos mouse only, default three buttons, dense rate so the
        // 50% interleave accumulates targeted clicks quickly.
        let scenario: Scenario = serde_json::from_str(
            r#"{"name":"ui-target","bot":{"type":"chaos","seed":42,"raw_surface":{"keys":[],"mouse_buttons":["Left"]},"input_rate_hz":20},"duration_s":3.0,"tps":60,"invariants":[]}"#,
        )
        .expect("scenario parses");

        let mut app = App::new();
        app.add_plugins(
            bevy::DefaultPlugins
                .build()
                .disable::<bevy_winit::WinitPlugin>(),
        );
        app.insert_resource(ClickProbe::default());
        app.add_systems(Startup, setup_widget);
        app.add_observer(
            |trigger: On<bevy_picking::events::PointerClick>, mut probe: ResMut<ClickProbe>| {
                if Some(trigger.entity) == probe.widget_entity {
                    probe.clicks_on_widget += 1;
                }
            },
        );
        // Moving entity so frozen_world cannot mask the probe
        // assertions (we assert on the probe, not the verdict).
        app.add_systems(Startup, |mut commands: Commands| {
            commands.spawn(Transform::from_xyz(0.0, 0.0, 0.0));
        });
        app.add_systems(Update, |mut q: Query<&mut Transform>| {
            for mut t in q.iter_mut() {
                t.translation.x += 0.05;
            }
        });

        let report = run_scenario(&mut app, &scenario).expect("run completes");

        // The enumeration must have run, seen the button, and
        // attached itself to the report.
        let ui_surface = report
            .ui_surface
            .as_ref()
            .expect("ui_surface captured after first frame");
        let widget = probe_widget_entity(&app);
        assert!(
            ui_surface.interactables.iter().any(|i| i.entity == widget),
            "declared Button must be enumerated: {:?}",
            ui_surface.interactables
        );

        // And the interleave must be genuinely driving clicks to the
        // widget: ~60 inputs at 20 Hz over 3 s, ~half routed through
        // the enumerated center (a guaranteed hit), the rest
        // quantized-random over the 1280x720 window (~2% hit rate on
        // the 200x100 widget). Without the interleave the expected
        // count is ~1-2; with it, ~30. Floor 10 separates the two
        // regimes decisively.
        let probe = app.world().resource::<ClickProbe>();
        assert!(
            probe.clicks_on_widget >= 10,
            "chaos mouse delivered {} clicks to the widget across {} total \
             inputs (probe {probe:?}) — the 50% interleave through enumerated \
             centers is not landing on the widget",
            probe.clicks_on_widget,
            report.metrics.input_count
        );
    }

    fn probe_widget_entity(app: &bevy_app::App) -> Entity {
        *app.world()
            .resource::<ClickProbe>()
            .widget_entity
            .as_ref()
            .expect("widget entity recorded at spawn")
    }
}
