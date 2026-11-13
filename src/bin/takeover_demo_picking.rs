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
//! hovers it: any delivered click awakens the world, keeping the
//! delivery verdict isolated from aim. The ONGOING MOTION, however,
//! is a function of WHERE the click landed (`PointerClick` carries
//! the window-space pointer position): the mouse-matrix differential
//! (probe cell vs. dead-center control) needs traces that differ by
//! click position — a fullscreen node with a position-independent
//! response would make control and every cell trace identical, and
//! divergence impossible by construction. With the default 5×4 grid
//! no cell center shares BOTH coordinates with the dead-center
//! control, so every cell's trace is distinct.
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
    // Awaken on the first UI click and record WHERE it landed; the
    // world keeps moving afterwards at a click-position-derived
    // velocity. If wrapped WindowEvents are not delivered, picking
    // never fires, the flag never sets, and frozen_world trips —
    // exactly what pointer-blind delivery looks like from the
    // outside. `pointer.position` is the window-space logical pixel
    // position bevy_picking maintains (winit convention: y grows
    // downward); only its DISTINCTNESS matters here, not its sign
    // convention.
    app.add_observer(|click: On<PointerClick>, mut awakened: ResMut<Awakened>| {
        awakened.0 = true;
        awakened.1 = click.pointer.position * 0.001;
    });
    app.insert_resource(Awakened(false, Vec2::ZERO));
    app.add_systems(
        Update,
        |awakened: Res<Awakened>, mut q: Query<&mut Transform>| {
            if !awakened.0 {
                return;
            }
            for mut t in q.iter_mut() {
                t.translation += awakened.1.extend(0.0);
            }
        },
    );
    app.run();
}

#[derive(bevy_ecs::prelude::Resource)]
struct Awakened(bool, bevy_math::Vec2);

#[cfg(not(feature = "takeover-winit"))]
fn main() {}
