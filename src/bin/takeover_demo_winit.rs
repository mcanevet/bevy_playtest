//! Winit-backed demo "game" for PlaytestPlugin hard-fail tests.
//!
//! Mounts DefaultPlugins (windowless mode) + PlaytestPlugin: with
//! BEVY_PLAYTEST_SCENARIO set the takeover must refuse (exit 3) rather
//! than silently run a winit loop in CI.
#[cfg(feature = "takeover-winit")]
fn main() {
    use bevy_app::prelude::*;
    use bevy_playtest::PlaytestPlugin;

    let mut app = App::new();
    app.add_plugins(bevy::DefaultPlugins.set(bevy::window::WindowPlugin {
        primary_window: None,
        primary_cursor_options: Default::default(),
        exit_condition: bevy::window::ExitCondition::DontExit,
        close_when_requested: true,
    }));
    app.add_plugins(PlaytestPlugin);
    app.run();
}

#[cfg(not(feature = "takeover-winit"))]
fn main() {}
