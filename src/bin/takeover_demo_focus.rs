//! Demo "game" for the FOCUS-WALK probe tests.
//!
//! The canonical tab-navigation consumer: adds
//! `bevy::input_focus::tab_navigation::TabNavigationPlugin`
//! (deliberately NOT part of `DefaultPlugins` — games opt in) and
//! spawns a `TabGroup` of `TabIndex`ed widgets, then reacts
//! observably to focus MOVING between them: every frame, the marker
//! `Transform` snaps to the focused widget's `TabIndex` value (or
//! rests at the origin while focus sits on the window, the initial
//! state), so focus state is a digest input EVERY frame. A scripted
//! `Tab` press advances focus window→widget0→widget1→widget2→window
//! and the marker moves — the walker's probe (a single `Tab` tap at
//! the readiness point) diverges from its control (a single
//! control-key tap at the same frame; the substitution pair differs
//! by exactly one bit — the pressed key) from the press frame
//! onward, for the rest of the trace.
//!
//! Why TRANSFORM tracking: the digest hashes observable world state
//! each frame; a moving `Transform` is the plainest digest-visible
//! signal, immune to "does the digest cover component X" questions.
//! The moved distance encodes WHICH widget holds focus, so the
//! finding is attributable, not just "something changed".
//!
//! Same plugin graph as the other discovery fixtures
//! (`PlaytestDefaultPlugins` + `PlaytestPlugin`) so the takeover
//! register matches production probes. `InputFocusPlugin` and
//! `InputDispatchPlugin` come via `DefaultPlugins` (inside
//! `PlaytestDefaultPlugins`); the demo adds only
//! `TabNavigationPlugin` — exactly the one-line opt-in real menu
//! games make, and the one the honest-inert docs describe.
#[cfg(feature = "takeover-winit")]
fn main() {
    use bevy::input_focus::tab_navigation::{TabGroup, TabIndex};
    use bevy::prelude::*;
    use bevy_playtest::{PlaytestDefaultPlugins, PlaytestPlugin};

    #[derive(Component)]
    struct Marker;

    let mut app = App::new();
    app.add_plugins(PlaytestDefaultPlugins);
    app.add_plugins(bevy::input_focus::tab_navigation::TabNavigationPlugin);
    app.add_plugins(PlaytestPlugin);
    app.add_systems(Startup, |mut commands: Commands| {
        commands.spawn(Camera2d);
        // The tab group: three focusable widgets, tab indexes 0..3.
        // Plain entities suffice — `TabIndex` is the only thing the
        // tab navigator walks, and the marker system resolves focus
        // to an index by entity. Widget 0 carries `AutoFocus` so
        // focus starts INSIDE the group (the idiomatic menu-open
        // state) — real menu UIs focus their first widget on open,
        // and navigation then stays inside the group instead of
        // logging the navigator's `NoTabGroupForCurrentFocus`
        // warning on first entry. This demo exercises CLEAN tab
        // cycling, not first-entry edge cases.
        commands.spawn(TabGroup::new(0)).with_children(|p| {
            for i in 0..3 {
                if i == 0 {
                    p.spawn((bevy::input_focus::AutoFocus, TabIndex(i)));
                } else {
                    p.spawn(TabIndex(i));
                }
            }
        });
        // The digest-visible marker: tracks the focused widget.
        commands.spawn((Marker, Transform::from_xyz(0.0, 0.0, 0.0)));
    });
    // Snapshot focus into the marker transform every frame: while
    // focus sits on the window (initial state) the marker rests at
    // the origin (index −1); when a Tab press hands focus to widget
    // i, it hops to (i, i). Divergence from the control run is
    // immediate and permanent — a probe diffing any post-press
    // frame sees it.
    app.add_systems(
        Update,
        |focus: Option<Res<bevy::input_focus::InputFocus>>,
         widgets: Query<(Entity, &TabIndex)>,
         mut markers: Query<&mut Transform, With<Marker>>| {
            let idx = focus
                .as_deref()
                .and_then(|f| f.get())
                .and_then(|focused| {
                    widgets
                        .iter()
                        .find(|(e, _)| *e == focused)
                        .map(|(_, t)| t.0)
                })
                .unwrap_or(-1);
            for mut t in markers.iter_mut() {
                t.translation = Vec3::new(idx as f32, idx as f32, 0.0);
            }
        },
    );
    app.run();
}

#[cfg(not(feature = "takeover-winit"))]
fn main() {}
