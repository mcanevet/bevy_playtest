//! Zero-annotation demo "game" with HIDDEN NONDETERMINISM: a system
//! consults the wall clock (not the ChoiceStream). Used by e2e tests
//! as the replay-divergence negative control.
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
        let jitter = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos() as f32;
        for mut t in q.iter_mut() {
            t.translation.y = jitter;
        }
    });
    app.run();
}
