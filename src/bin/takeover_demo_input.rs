//! Demo "game" for the input-delivery regression test.
//!
//! Mimics the plugin graph of every showcase fixture: full
//! `PlaytestDefaultPlugins` (DefaultPlugins minus WinitPlugin, so
//! InputPlugin — with its own `keyboard_input_system` — is present)
//! plus `PlaytestPlugin` for the takeover.
//!
//! The world moves ONLY in response to `ButtonInput::just_pressed` —
//! the standard Bevy idiom for discrete input, and the exact mechanism
//! the delivery bug blinded (a second harness-registered copy of
//! `keyboard_input_system` wiped the first copy's fresh `just_pressed`
//! state in PreUpdate, so Update systems never saw a transition). A
//! harness that drops or corrupts discrete-input delivery makes this
//! game look frozen (frozen_world trips); a healthy one keeps it
//! alive.
//!
//! This binary is intentionally shaped like loading_screen's
//! `level_selection`: read `just_pressed(Digit1)`, run a system that
//! spawns/moves entities with Transforms.
#[cfg(feature = "takeover-winit")]
fn main() {
    use bevy_app::prelude::*;
    use bevy_ecs::prelude::*;
    use bevy_input::prelude::*;
    use bevy_playtest::{PlaytestDefaultPlugins, PlaytestPlugin};
    use bevy_transform::prelude::Transform;

    let mut app = App::new();
    app.add_plugins(PlaytestDefaultPlugins);
    app.add_plugins(PlaytestPlugin);
    // Stand-in for level_selection: just_pressed-driven world motion.
    // The world moves ONLY once a just_pressed(Digit1) transition has
    // landed (the awakened flag stays false otherwise, so no Transform
    // ever changes and frozen_world trips) — this is exactly what
    // blind delivery looks like from the outside.
    app.add_systems(
        Update,
        move |keys: Res<ButtonInput<KeyCode>>,
              mut awakened: Local<bool>,
              mut q: Query<&mut Transform>| {
            if keys.just_pressed(KeyCode::Digit1) {
                *awakened = true;
            }
            if !*awakened {
                return;
            }
            for mut t in q.iter_mut() {
                t.translation.x += 0.05;
            }
        },
    );
    app.add_systems(Startup, |mut commands: Commands| {
        commands.spawn(Transform::from_xyz(0.0, 0.0, 0.0));
    });
    app.run();
}

#[cfg(not(feature = "takeover-winit"))]
fn main() {}
