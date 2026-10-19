//! Demo "game" with RAW DefaultPlugins + PlaytestPlugin — deliberately
//! NOT using PlaytestDefaultPlugins. Under BEVY_PLAYTEST_HEADLESS=1 with
//! no scenario, PlaytestPlugin's assert_headless must refuse (exit 3):
//! a winit-backed window cannot open headless (bevy_winit 0.20 builds
//! its event loop eagerly at plugin-build time).
#[cfg(feature = "takeover-winit")]
fn main() {
    use bevy_app::prelude::*;
    use bevy_playtest::PlaytestPlugin;
    use bevy_transform::prelude::*;

    let mut app = App::new();
    app.add_plugins(bevy::DefaultPlugins);
    app.add_plugins(PlaytestPlugin);
    // A moving Transform so the frozen-world liveness oracle stays happy.
    app.add_systems(Startup, |mut commands: bevy_ecs::prelude::Commands| {
        commands.spawn(Transform::from_xyz(0.0, 0.0, 0.0));
    });
    app.run();
}

#[cfg(not(feature = "takeover-winit"))]
fn main() {}
