//! Spike demo: DefaultPlugins + PlaytestPlugin with ENV_SCENARIO set —
//! does the takeover runner replace winit's without a window appearing?
fn main() {
    use bevy_app::prelude::*;
    use bevy_playtest::PlaytestPlugin;
    use bevy_transform::prelude::*;

    let mut app = App::new();
    app.add_plugins(bevy::DefaultPlugins);
    app.add_plugins(PlaytestPlugin);
    app.add_systems(Startup, |mut commands: bevy_ecs::prelude::Commands| {
        commands.spawn(bevy_transform::prelude::Transform::from_xyz(1.0, 2.0, 0.0));
    });
    app.add_systems(Update, |mut q: bevy_ecs::prelude::Query<&mut Transform>| {
        for mut t in q.iter_mut() {
            t.translation.x += 0.1;
        }
    });
    app.run();
}
