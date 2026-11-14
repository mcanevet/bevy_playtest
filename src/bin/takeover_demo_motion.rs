//! Demo "game" for the mouse-MOTION probe tests.
//!
//! The canonical motion-API consumer: a camera-pan/look-style system
//! reading `AccumulatedMouseMotion` — Bevy's frame-delta motion API,
//! summed from raw `MouseMotion` by `accumulate_mouse_motion_system`.
//! The world translates by the SIGNED accumulated delta each frame,
//! so a zero-net +d/−d oscillation still displaces the world during
//! the oscillation window (motion effects with NO net cursor drift —
//! exactly the position-confound-free property the probe relies on).
//!
//! Why SIGNED response is load-bearing: a direction-blind consumer
//! (e.g. one that integrates `abs(delta)` or `delta.length()`) would
//! see the oscillation too, but a direction-RESOLVED consumer must
//! move by +d then −d and also diverge — both legitimately read as
//! "reacts to motion". The pan integral over the whole oscillation
//! nets zero, but the world position is a digest input EVERY frame:
//! the probe trace diverges from the frozen control trace from the
//! FIRST +d frame onward, regardless of the eventual cancel. A probe
//! diffing only the FINAL frame would be vacuous here; the digest
//! series diffs every frame.
//!
//! Same plugin graph as the other discovery fixtures
//! (`PlaytestDefaultPlugins` + `PlaytestPlugin`) so the takeover
//! register matches production probes.
#[cfg(feature = "takeover-winit")]
fn main() {
    use bevy::prelude::*;
    use bevy_playtest::{PlaytestDefaultPlugins, PlaytestPlugin};

    let mut app = App::new();
    app.add_plugins(PlaytestDefaultPlugins);
    app.add_plugins(PlaytestPlugin);
    app.add_systems(Startup, |mut commands: Commands| {
        commands.spawn(Camera2d);
        commands.spawn(Transform::from_xyz(0.0, 0.0, 0.0));
    });
    // Pan by the signed per-frame delta: the single-line idiom behind
    // camera controllers and look-controls across the ecosystem.
    app.add_systems(
        Update,
        |motion: Res<bevy_input::mouse::AccumulatedMouseMotion>, mut q: Query<&mut Transform>| {
            for mut t in q.iter_mut() {
                t.translation += motion.delta.extend(0.0);
            }
        },
    );
    app.run();
}

#[cfg(not(feature = "takeover-winit"))]
fn main() {}
