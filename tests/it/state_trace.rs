//! State trace: the report records which state schedules ran (and which
//! never did) and which game resources changed — for a game with no
//! `Reflect` and no state registration.

#![cfg(feature = "takeover")]

use crate::common;

use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use bevy_playtest::driver::run_scenario;

#[derive(States, Default, Debug, Clone, PartialEq, Eq, Hash)]
enum Phase {
    #[default]
    Menu,
    Playing,
    Secret,
}

/// Written every frame (a timer-like resource).
#[derive(Resource, Default)]
struct Ticks(u32);

/// Written exactly once, on the frame the game starts playing.
#[derive(Resource, Default)]
struct Score(u32);

const START_AT: u32 = 10;

fn advance(mut ticks: ResMut<Ticks>, mut score: ResMut<Score>, mut next: ResMut<NextState<Phase>>) {
    ticks.0 += 1;
    if ticks.0 == START_AT {
        score.0 += 1;
        next.set(Phase::Playing);
    }
}

fn noop() {}

fn game() -> App {
    let mut app = App::new();
    app.add_plugins(StatesPlugin)
        .init_state::<Phase>()
        .init_resource::<Ticks>()
        .init_resource::<Score>()
        .add_systems(Update, advance)
        .add_systems(OnEnter(Phase::Menu), noop)
        .add_systems(OnEnter(Phase::Playing), noop)
        .add_systems(OnEnter(Phase::Secret), noop);
    app
}

fn scenario() -> bevy_playtest::scenario::Scenario {
    serde_json::from_str(r#"{"bot":{"type":"chaos","seed":1},"duration_s":0.5,"tps":60}"#).unwrap()
}

#[test]
fn trace_records_state_schedules_and_resource_changes() {
    let _slot = common::run_slot();
    let mut app = game();
    let report = run_scenario(&mut app, &scenario()).expect("run completes");
    let trace = report
        .state_trace
        .expect("every completed run carries a state trace");

    let labels: Vec<(&str, u64)> = trace
        .schedules_run
        .iter()
        .map(|r| (r.label.as_str(), r.frame))
        .collect();
    assert_eq!(
        labels.first(),
        Some(&("OnEnter(Menu)", 0)),
        "the initial state is entered on the first frame: {labels:?}"
    );
    let playing = labels
        .iter()
        .find(|(l, _)| *l == "OnEnter(Playing)")
        .unwrap_or_else(|| panic!("Playing must be entered: {labels:?}"));
    assert!(
        (START_AT as u64 - 1..=START_AT as u64 + 1).contains(&playing.1),
        "Playing is entered right after the transition frame: {labels:?}"
    );

    assert_eq!(
        trace.schedules_never_run,
        vec!["OnEnter(Secret)".to_string()],
        "a state with an enter schedule that never ran is reported as never entered"
    );

    let score = trace
        .resource_changes
        .iter()
        .find(|r| r.name.ends_with("::Score"))
        .unwrap_or_else(|| panic!("Score changed once: {:?}", trace.resource_changes));
    assert_eq!(score.frames_changed, 1, "{score:?}");
    let ticks = trace
        .resource_changes
        .iter()
        .find(|r| r.name.ends_with("::Ticks"))
        .expect("Ticks changes every frame");
    assert!(ticks.frames_changed >= 25, "{ticks:?}");
    assert!(
        trace
            .resource_changes
            .iter()
            .all(|r| !r.name.starts_with("bevy")),
        "engine resources are excluded: {:?}",
        trace.resource_changes
    );
}

#[test]
fn stateless_game_gets_an_honest_note() {
    let _slot = common::run_slot();
    let mut app = App::new();
    let report = run_scenario(&mut app, &scenario()).expect("run completes");
    let trace = report.state_trace.expect("state trace present");
    assert!(trace.schedules_run.is_empty() && trace.schedules_never_run.is_empty());
    assert!(
        trace
            .notes
            .iter()
            .any(|n| n.contains("no OnEnter/OnExit/OnTransition")),
        "{:?}",
        trace.notes
    );
}
