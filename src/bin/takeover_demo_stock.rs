//! Zero-annotation demo "game": NO bevy_playtest-specific code except
//! the plugin line — no Gameplay marker, no hooks. Used by e2e tests
//! to prove the `all` target selector works on a stock game.
fn main() {
    use bevy_app::prelude::*;
    use bevy_ecs::prelude::*;
    use bevy_playtest::PlaytestPlugin;
    use bevy_transform::prelude::*;

    let mut app = App::new();
    app.add_plugins(PlaytestPlugin);
    app.add_systems(Startup, |mut commands: Commands| {
        commands.spawn(Transform::from_xyz(1.0, 2.0, 0.0));
    });
    app.add_systems(Update, |mut q: Query<&mut Transform>| {
        for mut t in q.iter_mut() {
            t.translation.x += 0.1;
        }
    });
    app.run();
}
