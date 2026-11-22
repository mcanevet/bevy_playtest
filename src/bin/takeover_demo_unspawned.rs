//! Demo "game" for lint stage-2 e2e tests: a reflected component
//! that is REGISTERED but NEVER SPAWNED, plus a plain (non-resource)
//! reflected component that IS spawned.
//!
//! Doctrine under test: the type-registry probe stays silent about a
//! registered component with zero instances (absence is never a
//! finding — entities may spawn after the warm-up frame), while the
//! same registered-but-unspawned name DOES fail loudly at run time
//! when the probe has no instance to check against and the run never
//! produces one either.
fn main() {
    use bevy_app::prelude::*;
    use bevy_ecs::prelude::*;
    use bevy_playtest::PlaytestPlugin;
    use bevy_transform::prelude::*;

    #[derive(Component, Debug, bevy_reflect::Reflect)]
    #[reflect(bevy_ecs::reflect::ReflectComponent)]
    struct GhostArmor(f32);

    #[derive(Component, Debug, bevy_reflect::Reflect)]
    #[reflect(bevy_ecs::reflect::ReflectComponent)]
    struct SpawnedMarker(f32);

    let mut app = App::new();
    app.add_plugins(PlaytestPlugin);
    // register_type (not manual AppTypeRegistry writes) so the types
    // carry their ReflectComponent data — the probe reads it.
    app.register_type::<GhostArmor>();
    app.register_type::<SpawnedMarker>();
    // `SpawnedMarker` exists at the warm-up frame; `GhostArmor` never
    // gets an instance in this game.
    app.add_systems(Startup, |mut commands: Commands| {
        commands.spawn((SpawnedMarker(3.0), Transform::from_xyz(1.0, 2.0, 0.0)));
    });
    app.run();
}
