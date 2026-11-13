//! Demo "game" for the mouse-GRID position-discrimination test.
//!
//! Same plugin graph as `takeover_demo_picking` (full
//! `PlaytestDefaultPlugins` — bevy_picking's mouse backend included —
//! plus `PlaytestPlugin` for the takeover), but the clickable node
//! covers only the TOP-LEFT ~45% of the window instead of the full
//! screen. The world moves ONLY after a `PointerClick` LANDS ON that
//! quarter node.
//!
//! Why this game exists: a fullscreen click target (`picking`) proves
//! the mouse matrix DELIVERS clicks (every cell diverges) but cannot
//! prove the grid resolves WHERE — every cell is bound, so
//! position-blind probing passes too. A quarter-screen target
//! separates the two: cells overlapping the quarter diverge, the rest
//! match the mouse control (a click at the window center, outside
//! the quarter). A grid that mislabels which cells bind fails HERE,
//! not in the wild.
//!
//! Geometry (normalized window space, origin top-left — the same
//! top-to-bottom y convention as the grid's `norm_y`): the clickable
//! node spans x in [0.0, 0.45], y in [0.0, 0.45]. The default 5×4
//! grid's cell centers are at odd multiples of 0.5/5 (x: 0.1, 0.3,
//! 0.5, 0.7, 0.9) and 0.5/4 (y: 0.125, 0.375, 0.625, 0.875). Centers
//! on the node: x ∈ {0.1, 0.3}, y ∈ {0.125, 0.375} → cells `r0c0`,
//! `r0c1`, `r1c0`, `r1c1`. The control position (0.5, 0.5) sits
//! clearly outside — which is why the node is sized slightly UNDER a
//! quarter (45%) instead of exactly 50%: the boundary center must
//! miss the node unambiguously, clear of layout rounding.
#[cfg(feature = "takeover-winit")]
fn main() {
    use bevy::prelude::*;
    use bevy_playtest::{PlaytestDefaultPlugins, PlaytestPlugin};

    let mut app = App::new();
    app.add_plugins(PlaytestDefaultPlugins);
    app.add_plugins(PlaytestPlugin);
    app.add_systems(
        Startup,
        |mut commands: Commands, mut quarter: ResMut<QuarterNode>| {
            commands.spawn(Camera2d);
            quarter.0 = Some(
                commands
                    .spawn((
                        Node {
                            width: Val::Percent(45.0),
                            height: Val::Percent(45.0),
                            ..default()
                        },
                        ClickableQuarter,
                    ))
                    .id(),
            );
            commands.spawn(Transform::from_xyz(0.0, 0.0, 0.0));
        },
    );
    // Awaken on the first UI click ON THE QUARTER NODE; keep the
    // world moving afterwards. Clicks outside it (including at the
    // mouse control position, the window center) never awaken the
    // world: those cells match the control trace — the inert
    // verdicts the grid must produce for the outside region.
    //
    // The entity comparison is LOAD-BEARING: bevy_picking fires
    // `PointerClick` targeting the WINDOW ENTITY when the pointer is
    // over empty space, and an unfiltered observer catches those too
    // — every in-window click would awaken the world and the grid
    // could never discriminate position. Comparing the event's
    // target entity against the quarter node's (captured at spawn)
    // keeps empty-space clicks inert.
    app.add_observer(
        |click: On<PointerClick>, quarter: Res<QuarterNode>, mut awakened: ResMut<Awakened>| {
            if quarter.0 == Some(click.entity) {
                awakened.0 = true;
            }
        },
    );
    app.insert_resource(Awakened(false));
    app.insert_resource(QuarterNode(None));
    app.add_systems(
        Update,
        |awakened: Res<Awakened>, mut q: Query<&mut Transform>| {
            if !awakened.0 {
                return;
            }
            for mut t in q.iter_mut() {
                t.translation.x += 0.05;
            }
        },
    );
    app.run();
}

#[derive(bevy_ecs::prelude::Component)]
struct ClickableQuarter;

#[derive(bevy_ecs::prelude::Resource)]
struct QuarterNode(Option<bevy_ecs::prelude::Entity>);

#[derive(bevy_ecs::prelude::Resource)]
struct Awakened(bool);

#[cfg(not(feature = "takeover-winit"))]
fn main() {}
