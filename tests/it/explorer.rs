//! Explorer end-to-end: on a small menu game (Menu → Settings / Game,
//! each with a way back), `bot.explore` must reach every screen and
//! activate every button, and say so in its `exploration` summary.

#![cfg(all(feature = "takeover", feature = "ui-surface"))]

use crate::common;

use bevy::prelude::*;
use bevy_playtest::run_scenario;
use bevy_playtest::scenario::Scenario;

#[derive(States, Default, Debug, Clone, PartialEq, Eq, Hash)]
enum Screen {
    #[default]
    Menu,
    Settings,
    Game,
}

fn button(parent: &mut ChildSpawnerCommands, label: &str, target: Screen) {
    parent
        .spawn((
            Node {
                width: Val::Px(240.0),
                height: Val::Px(60.0),
                margin: UiRect::all(Val::Px(10.0)),
                ..default()
            },
            children![Text::new(label)],
        ))
        .observe(
            move |_: On<PointerClick>, mut next: ResMut<NextState<Screen>>| {
                next.set(target.clone());
            },
        );
}

fn spawn_screen(commands: &mut Commands, screen: Screen, buttons: &[(&str, Screen)]) {
    let buttons: Vec<(String, Screen)> = buttons
        .iter()
        .map(|(l, s)| (l.to_string(), s.clone()))
        .collect();
    commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            DespawnOnExit(screen),
        ))
        .with_children(|parent| {
            for (label, target) in buttons {
                button(parent, &label, target);
            }
        });
}

fn menu_game() -> App {
    let mut app = App::new();
    app.add_plugins(
        bevy::DefaultPlugins
            .build()
            .disable::<bevy_winit::WinitPlugin>(),
    )
    .init_state::<Screen>()
    .add_systems(Startup, |mut commands: Commands| {
        commands.spawn(Camera2d);
    })
    .add_systems(OnEnter(Screen::Menu), |mut commands: Commands| {
        spawn_screen(
            &mut commands,
            Screen::Menu,
            &[("Play", Screen::Game), ("Settings", Screen::Settings)],
        );
    })
    .add_systems(OnEnter(Screen::Settings), |mut commands: Commands| {
        spawn_screen(&mut commands, Screen::Settings, &[("Back", Screen::Menu)]);
    })
    .add_systems(OnEnter(Screen::Game), |mut commands: Commands| {
        spawn_screen(
            &mut commands,
            Screen::Game,
            &[("Quit to menu", Screen::Menu)],
        );
    });
    app
}

fn scenario(explore: bool) -> Scenario {
    serde_json::from_str(&format!(
        r#"{{"bot":{{"type":"chaos","seed":3,"explore":{explore},
            "raw_surface":{{"keys":[],"mouse_buttons":["Left"]}}}},
            "duration_s":10.0,"tps":60}}"#
    ))
    .expect("scenario parses")
}

#[test]
fn explorer_reaches_every_screen_and_reports_it() {
    let _slot = common::run_slot();
    let mut app = menu_game();
    let report = run_scenario(&mut app, &scenario(true)).expect("run completes");
    let summary = report
        .exploration
        .expect("explore runs carry an exploration summary");

    for state in ["OnEnter(Menu)", "OnEnter(Settings)", "OnEnter(Game)"] {
        assert!(
            summary.states_entered.iter().any(|s| s == state),
            "explorer must reach {state}: {summary:?}"
        );
    }
    assert!(summary.screens_seen >= 3, "{summary:?}");
    assert!(
        summary.widgets_never_clicked.is_empty(),
        "every button on this small game gets clicked: {summary:?}"
    );
    assert!(!summary.verdict.is_empty(), "{summary:?}");
}

#[test]
fn plain_chaos_runs_carry_no_exploration_summary() {
    let _slot = common::run_slot();
    let mut app = menu_game();
    let report = run_scenario(&mut app, &scenario(false)).expect("run completes");
    assert!(report.exploration.is_none());
}

#[derive(States, Default, Debug, Clone, PartialEq, Eq, Hash)]
enum Phase {
    #[default]
    Idle,
    Level,
}

/// A key the declared surface does not mention (here: only KeyW) is
/// still found: the explorer tries its built-in prior once per screen,
/// and reports keys followed by something new under `keys_found`.
#[test]
fn explorer_finds_an_undeclared_key_through_the_prior() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.add_plugins(
        bevy::DefaultPlugins
            .build()
            .disable::<bevy_winit::WinitPlugin>(),
    )
    .init_state::<Phase>()
    .add_systems(OnEnter(Phase::Level), || {})
    .add_systems(
        Update,
        |keys: Res<ButtonInput<KeyCode>>, mut next: ResMut<NextState<Phase>>| {
            if keys.just_pressed(KeyCode::Digit3) {
                next.set(Phase::Level);
            }
        },
    );
    let scenario: Scenario = serde_json::from_str(
        r#"{"bot":{"type":"chaos","seed":3,"explore":true,
            "raw_surface":{"keys":["KeyW"],"mouse_buttons":["Left"]}},
            "duration_s":6.0,"tps":60}"#,
    )
    .expect("scenario parses");
    let report = run_scenario(&mut app, &scenario).expect("run completes");
    let summary = report.exploration.expect("exploration summary");
    assert!(
        summary.states_entered.iter().any(|s| s == "OnEnter(Level)"),
        "{summary:?}"
    );
    assert!(
        summary.keys_found.iter().any(|k| k == "Digit3"),
        "Digit3 must be credited: {summary:?}"
    );
}

/// Ctrl+S is found as a combo: the explorer holds a modifier under a
/// declared key every few presses, and credits the combo, not the key.
#[test]
fn explorer_finds_a_modifier_combo() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.add_plugins(
        bevy::DefaultPlugins
            .build()
            .disable::<bevy_winit::WinitPlugin>(),
    )
    .init_state::<Phase>()
    .add_systems(OnEnter(Phase::Level), || {})
    .add_systems(
        Update,
        |keys: Res<ButtonInput<KeyCode>>, mut next: ResMut<NextState<Phase>>| {
            if keys.pressed(KeyCode::ControlLeft) && keys.just_pressed(KeyCode::KeyS) {
                next.set(Phase::Level);
            }
        },
    );
    let scenario: Scenario = serde_json::from_str(
        r#"{"bot":{"type":"chaos","seed":3,"explore":true,
            "raw_surface":{"keys":["KeyS"],"mouse_buttons":["Left"]}},
            "duration_s":6.0,"tps":60}"#,
    )
    .expect("scenario parses");
    let report = run_scenario(&mut app, &scenario).expect("run completes");
    let summary = report.exploration.expect("exploration summary");
    assert!(
        summary.states_entered.iter().any(|s| s == "OnEnter(Level)"),
        "{summary:?}"
    );
    assert_eq!(summary.combos_found, ["ControlLeft+KeyS"], "{summary:?}");
    // Combos have their own action-log entries; key presses keep
    // canonical single-key labels.
    assert!(report
        .action_log
        .iter()
        .any(|a| a.action.starts_with("key_combo:")
            && a.details.as_deref() == Some("ControlLeft+KeyS")));
    assert!(report
        .action_log
        .iter()
        .filter(|a| a.action.starts_with("key_press:"))
        .all(|a| a.details.as_deref().is_some_and(|d| !d.contains('+'))));
    assert!(summary.combos_tried >= 1, "{summary:?}");
}

/// A drag is pressed on one widget, moved while held, and released on
/// another: the target's `DragDrop` fires. A click never fires it.
#[test]
fn explorer_drags_one_widget_onto_another() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.add_plugins(
        bevy::DefaultPlugins
            .build()
            .disable::<bevy_winit::WinitPlugin>(),
    )
    .init_state::<Phase>()
    .add_systems(OnEnter(Phase::Level), || {})
    .add_systems(Startup, |mut commands: Commands| {
        commands.spawn(Camera2d);
        commands
            .spawn(Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::SpaceAround,
                align_items: AlignItems::Center,
                ..default()
            })
            .with_children(|parent| {
                parent
                    .spawn((
                        Node {
                            width: Val::Px(120.0),
                            height: Val::Px(120.0),
                            ..default()
                        },
                        children![Text::new("Card")],
                    ))
                    .observe(|_: On<PointerDragStart>| {});
                parent
                    .spawn((
                        Node {
                            width: Val::Px(120.0),
                            height: Val::Px(120.0),
                            ..default()
                        },
                        children![Text::new("Slot")],
                    ))
                    .observe(
                        |_: On<PointerDragDrop>, mut next: ResMut<NextState<Phase>>| {
                            next.set(Phase::Level);
                        },
                    );
            });
    });
    let scenario: Scenario = serde_json::from_str(
        r#"{"bot":{"type":"chaos","seed":3,"explore":true,
            "raw_surface":{"keys":[],"mouse_buttons":["Left"]}},
            "duration_s":10.0,"tps":60}"#,
    )
    .expect("scenario parses");
    let report = run_scenario(&mut app, &scenario).expect("run completes");
    let summary = report.exploration.expect("exploration summary");
    assert!(summary.drags >= 1, "{summary:?}");
    assert!(
        summary.states_entered.iter().any(|s| s == "OnEnter(Level)"),
        "a drag from Card onto Slot fires DragDrop: {summary:?}"
    );
}

#[derive(Component)]
struct Cell;

#[derive(Component)]
struct Revealed;

#[derive(Resource, Default)]
struct RevealedCount(u32);

/// Cells with no click declaration, handled by ONE global observer (the
/// mines pattern): random clicks that land on a cell make the game
/// react, the explorer learns the cell archetype, then targets every
/// cell like a widget.
#[test]
fn explorer_learns_undeclared_click_targets() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.add_plugins(
        bevy::DefaultPlugins
            .build()
            .disable::<bevy_winit::WinitPlugin>(),
    )
    .init_resource::<RevealedCount>()
    .add_systems(Startup, |mut commands: Commands| {
        commands.spawn(Camera2d);
        commands
            .spawn(Node {
                display: Display::Grid,
                grid_template_columns: RepeatedGridTrack::px(5, 60.0),
                grid_template_rows: RepeatedGridTrack::px(5, 60.0),
                margin: UiRect::all(Val::Auto),
                ..default()
            })
            .with_children(|grid| {
                // A label inside each cell: clicks land on the label,
                // and the credit must go to the cell around it.
                for _ in 0..25 {
                    grid.spawn((Cell, Node::default(), children![Text::new("?")]));
                }
            });
    })
    .add_observer(
        |click: On<PointerClick>,
         cells: Query<(), (With<Cell>, Without<Revealed>)>,
         mut commands: Commands,
         mut count: ResMut<RevealedCount>| {
            // The click bubbles from the label up through the cell.
            let target = click.event_target();
            if cells.get(target).is_ok() {
                commands.entity(target).insert(Revealed);
                count.0 += 1;
            }
        },
    );
    let scenario: Scenario = serde_json::from_str(
        r#"{"bot":{"type":"chaos","seed":3,"explore":true,
            "raw_surface":{"keys":[],"mouse_buttons":["Left"]}},
            "duration_s":10.0,"tps":60}"#,
    )
    .expect("scenario parses");
    let report = run_scenario(&mut app, &scenario).expect("run completes");
    let summary = report.exploration.expect("exploration summary");
    let revealed = app.world().resource::<RevealedCount>().0;
    assert!(
        summary.learned_targets >= 1,
        "{revealed}/25 revealed; {summary:?}"
    );
    // Measured on this seed: 25/25 with learning, 6/25 with learning
    // disabled (random clicks only reach the cells by chance).
    assert!(
        revealed >= 22,
        "learned cells are targeted: {revealed}/25 revealed; {summary:?}"
    );
}
