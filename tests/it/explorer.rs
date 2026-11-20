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
