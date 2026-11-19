//! Crash-member child for the seed-sweep gate tests (tests/sweep.rs:
//! crashed members gate the sweep). A real App run under the
//! takeover whose Update system panics on the first frame: the
//! harness catches the unwind, classifies the run Crash, and writes
//! a normal report — the member outcome the sweep must treat as
//! always-red ("a crashed run certifies nothing"), distinct from an
//! infrastructure failure (no report at all).
//!
//! Same plugin graph as the other takeover demos (`PlaytestPlugin`
//! only, headless) so the takeover matches production children.
//!
//! No `#[cfg(not(feature = "takeover"))]` fallback arm: the `[[bin]]`
//! entry carries `required-features = ["takeover"]`, so this binary
//! is only ever BUILT with the feature on.
#[cfg(feature = "takeover")]
fn main() {
    use bevy_app::prelude::*;
    use bevy_ecs::prelude::*;
    use bevy_playtest::PlaytestPlugin;
    use bevy_transform::prelude::*;

    let mut app = App::new();
    app.add_plugins(PlaytestPlugin);
    app.add_systems(Startup, |mut commands: Commands| {
        // Observable world: the vacuity guard stays satisfied up to
        // the first Update tick, so the only finding is the panic.
        commands.spawn(Transform::from_xyz(0.0, 0.0, 0.0));
    });
    app.add_systems(Update, || panic!("demo crash"));
    app.run();
}
