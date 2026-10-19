//! Demo "game" for PlaytestPlugin end-to-end tests.
//!
//! Runs headless: no windowed plugins at all, so the takeover has
//! nothing to fight with. A moving Gameplay entity keeps the run
//! observable; the `bounds` invariant in the scenario trips Fail.
fn main() {
    use bevy_app::prelude::*;
    use bevy_ecs::prelude::*;
    use bevy_playtest::contract::Gameplay;
    use bevy_playtest::PlaytestPlugin;
    use bevy_transform::prelude::*;

    let mut app = App::new();
    app.add_plugins(PlaytestPlugin);
    app.add_systems(Startup, |mut commands: Commands| {
        commands.spawn((Gameplay, Transform::from_xyz(1.0, 2.0, 0.0)));
    });
    app.add_systems(Update, |mut q: Query<&mut Transform, With<Gameplay>>| {
        for mut t in q.iter_mut() {
            t.translation.x += 0.1;
        }
    });
    app.run();
}
