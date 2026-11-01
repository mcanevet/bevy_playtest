//! Demo "game" for the picking-delivery regression test.
//!
//! Mimics the plugin graph of the pointer-driven showcase fixtures
//! (mines, game_menu): full `PlaytestDefaultPlugins` (DefaultPlugins
//! minus WinitPlugin, so bevy_picking's mouse backend is present)
//! plus `PlaytestPlugin` for the takeover.
//!
//! The world moves ONLY in response to `PointerClick` on a UI node —
//! the standard bevy_picking idiom for pointer-driven games, and the
//! exact mechanism the wrapper gap blinded (bevy_picking reads the
//! WRAPPED `bevy_window::WindowEvent` messages that winit writes; a
//! bot emitting only raw `bevy_input` messages is invisible to it). A
//! harness that drops the wrapped forms makes this game look frozen
//! (frozen_world trips); a healthy one keeps it alive.
//!
//! The clickable node is FULLSCREEN so any in-window cursor position
//! hovers it: the verdict isolates message delivery, not aim.
#[cfg(feature = "takeover-winit")]
fn main() {
    use bevy::prelude::*;
    use bevy_playtest::{PlaytestDefaultPlugins, PlaytestPlugin};

    let mut app = App::new();
    app.add_plugins(PlaytestDefaultPlugins);
    app.add_plugins(PlaytestPlugin);
    app.add_systems(Startup, |mut commands: Commands| {
        commands.spawn(Camera2d);
        commands.spawn(Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        });
        commands.spawn(Transform::from_xyz(0.0, 0.0, 0.0));
    });
    // Awaken on the first UI click; keep the world moving afterwards.
    // If wrapped WindowEvents are not delivered, picking never fires,
    // the flag never sets, and frozen_world trips — exactly what
    // pointer-blind delivery looks like from the outside.
    app.add_observer(|_click: On<PointerClick>, mut awakened: ResMut<Awakened>| {
        awakened.0 = true;
    });
    app.insert_resource(Awakened(false));
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

#[derive(bevy_ecs::prelude::Resource)]
struct Awakened(bool);

#[cfg(not(feature = "takeover-winit"))]
fn main() {}
