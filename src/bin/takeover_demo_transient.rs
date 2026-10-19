//! Demo "game" with TRANSIENT entities: an entity with a Transform is
//! spawned early and despawned just before the scenario ends. Exercises
//! the vacuity guard: entities were observable DURING the run, so the
//! run must NOT be flagged VacuousRun even though zero Transform
//! entities remain at the end. Smoke scenario: 5s @ 60TPS = 300 frames.
#[cfg(feature = "takeover")]
mod game {
    use bevy_app::prelude::*;
    use bevy_ecs::prelude::*;
    use bevy_playtest::PlaytestPlugin;
    use bevy_transform::prelude::*;

    /// Frame on which the transient entity spawns.
    const SPAWN_AT: u32 = 10;
    /// Frame on which the transient entity despawns (near run end).
    const DESPAWN_AT: u32 = 280;

    #[derive(Resource)]
    struct TransientClock {
        frame: u32,
        despawned: bool,
    }

    #[derive(Component)]
    struct TransientMarker;

    fn transient_entity_system(
        mut clock: ResMut<TransientClock>,
        mut commands: Commands,
        q: Query<Entity, With<TransientMarker>>,
        mut q_transform: Query<&mut Transform, With<TransientMarker>>,
    ) {
        if clock.despawned {
            return;
        }
        let f = clock.frame;
        clock.frame += 1;
        if f == SPAWN_AT {
            commands.spawn((TransientMarker, Transform::from_xyz(1.0, 2.0, 0.0)));
        } else if f == DESPAWN_AT {
            // Despawn near the end — entities were observable during
            // the run, but zero remain at the end.
            for e in q.iter() {
                commands.entity(e).despawn();
            }
            clock.despawned = true;
        } else if f > SPAWN_AT {
            // Keep it moving so the frozen-world liveness oracle stays
            // quiet while the entity lives.
            for mut t in q_transform.iter_mut() {
                t.translation.x += 0.1;
            }
        }
    }

    pub fn main() {
        let mut app = App::new();
        app.add_plugins(PlaytestPlugin);
        app.insert_resource(TransientClock {
            frame: 0,
            despawned: false,
        });
        app.add_systems(Update, transient_entity_system);
        app.run();
    }
}

#[cfg(feature = "takeover")]
fn main() {
    game::main()
}

#[cfg(not(feature = "takeover"))]
fn main() {}
