//! Demo "game" proving PlaytestDefaultPlugins + PlaytestPlugin runs headless
//! when BEVY_PLAYTEST_SCENARIO is set — the plugin group drops WinitPlugin
//! (bevy_winit 0.20 builds its event loop eagerly), so no window ever opens.
#[cfg(feature = "takeover-winit")]
fn main() {
    use bevy_app::prelude::*;
    use bevy_playtest::{PlaytestDefaultPlugins, PlaytestPlugin};
    use bevy_transform::prelude::*;

    let mut app = App::new();
    app.add_plugins(PlaytestDefaultPlugins.set(bevy::window::WindowPlugin {
        primary_window: None,
        primary_cursor_options: Default::default(),
        exit_condition: bevy::window::ExitCondition::DontExit,
        close_when_requested: true,
    }));
    app.add_plugins(PlaytestPlugin);
    // A moving Transform so the frozen-world liveness oracle stays happy.
    app.add_systems(Startup, |mut commands: bevy_ecs::prelude::Commands| {
        commands.spawn(Transform::from_xyz(0.0, 0.0, 0.0));
    });
    app.add_systems(Update, |mut q: bevy_ecs::prelude::Query<&mut Transform>| {
        for mut t in q.iter_mut() {
            t.translation.x += 0.05;
        }
    });
    app.run();
}

#[cfg(not(feature = "takeover-winit"))]
fn main() {}
