//! Zero-annotation demo "game" with HIDDEN RESOURCE-ONLY
//! NONDETERMINISM: a reflected game resource (`GameClock`) is driven
//! from the wall clock, while no Transform ever changes. The narrow
//! replay fingerprint (Transform-only) stays blind to it; only the
//! broadened state digest sees it. Used by e2e tests as the
//! takeover-mode digest-handoff negative control.
fn main() {
    use bevy_app::prelude::*;
    use bevy_ecs::prelude::*;
    use bevy_playtest::PlaytestPlugin;
    use bevy_transform::prelude::*;

    #[derive(Resource, Debug, bevy_reflect::Reflect)]
    #[reflect(bevy_ecs::reflect::ReflectResource)]
    struct GameClock(f32);

    let mut app = App::new();
    app.add_plugins(PlaytestPlugin);
    // Register reflection BEFORE inserting the resource, then set up
    // the world state. Static transform: the narrow fingerprint stays
    // constant, so ONLY the digest can flag this game's nondeterminism.
    {
        let world = app.world_mut();
        world
            .get_resource_or_insert_with(bevy_ecs::reflect::AppTypeRegistry::default)
            .0
            .write()
            .register::<GameClock>();
        world.insert_resource(GameClock(0.0));
        world.spawn(Transform::from_xyz(1.0, 2.0, 0.0));
    }
    app.add_systems(Update, |mut clock: ResMut<GameClock>| {
        // Wall-clock nanos: differs between replay pass A and pass B
        // (separate processes, microseconds apart) — hidden
        // nondeterminism the digest exists to expose.
        clock.0 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos() as f32;
    });
    app.run();
}
