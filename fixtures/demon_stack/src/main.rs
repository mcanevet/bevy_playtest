//! Demon-stack fixture game (planted spatial defect).
//!
//! A minimal deterministic arena: six demons converge on a
//! stationary player and pile into a sustained contact spiral —
//! the arena-shooter defect whose real-game shape (a six-enemy
//! contact-damage stack) the `pairwise_separation` rule
//! was built to judge. This fixture is the rule's fixture-tier
//! LITMUS: the pile is the planted truth, and the golden-verdict
//! test in `tests/playtest.rs` pins the rule catching it RED —
//! one episode, worst pair, bounded roster — plus the fly-by
//! control that must stay silent.
//!
//! Design constraints (mirrors the known_answer doctrine):
//! - Deterministic: no wall clock, no RNG, frame-driven only.
//! - The planted defect is VISIBLE to the judgement layer by
//!   construction: `Demon` is `Reflect`-derived and registered, so
//!   a `pairwise_separation` selector can name it.
//! - Movement keeps every transform genuinely changing (the pile
//!   chases a jittering target) — a frozen pile would trip
//!   `frozen_world`, which is ANOTHER oracle's story, and the
//!   litmus pins exactly the pairwise finding.
//!
//! IMPORTANT (do not "fix" silently): the spiral convergence in
//! `demon_dynamics` has NO separation term. A collision-resolution
//! pass would erase the planted defect this fixture exists to
//! exhibit; the pairwise rule must be able to go red against it.

use bevy::prelude::*;

/// Marker for the selector: `#[derive(Reflect)]` + `register_type`
/// puts the name `Demon` on the game's reflection surface so an
/// authored `pairwise_separation` invariant can target it.
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct Demon;

/// Player marker — the pile target. Stationary but breathing, so
/// the world never looks frozen to the liveness oracles.
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct Player;

/// Shared chase target for the pile: recomputed deterministically
/// from the frame counter every frame (no wall clock, no RNG) and
/// WRITTEN into this registered resource, so the run's state digest
/// sees a reflected game resource genuinely evolving — the eval
/// tier's state trace can measure it. Demons steer toward it with
/// proportional velocity, so six bodies converge from their spawn
/// ring into a shrinking spiral that ends in near-perfect
/// coincidence around the player — a sustained overlap, not a
/// fly-by.
#[derive(Resource, Reflect, Default)]
#[reflect(Resource)]
struct PileTarget {
    x: f32,
    z: f32,
}

/// Frame counter driving the deterministic jitter.
#[derive(Resource, Default)]
struct Frame(u32);

fn setup(mut commands: Commands) {
    // Camera: a real game always has one, and `render_liveness`
    // reports the absence as a standing (informational) finding —
    // without it, the litmus report would carry a second, planted-
    // by-omission finding, breaking the golden verdict's "exactly
    // the pairwise finding and nothing else" claim. The demons are
    // selected by `Demon`, not proximity to a camera, so this
    // changes no pair geometry.
    commands.spawn(Camera3d::default());

    // Player parked at the origin.
    commands.spawn((Player, Transform::from_xyz(0.0, 0.0, 0.0)));

    // Six demons on a spawn ring, radius 8 — the same shape the
    // arena shooter's contact damage produced: every demon heads
    // for the same point, and nothing pushes back.
    for i in 0..6 {
        let angle = i as f32 * std::f32::consts::TAU / 6.0;
        commands.spawn((
            Demon,
            Transform::from_xyz(angle.cos() * 8.0, 0.0, angle.sin() * 8.0),
        ));
    }
}

/// The planted defect: proportional homing with NO separation term.
fn demon_dynamics(
    mut demons: Query<&mut Transform, With<Demon>>,
    mut target: ResMut<PileTarget>,
    frame: Res<Frame>,
) {
    // Deterministic jitter keeps the pile target genuinely moving
    // (liveness) while staying inside a ~0.5 unit box (the pile
    // cannot drift out of coincidence).
    let jx = (frame.0 as f32 * 0.37).sin() * 0.25;
    let jz = (frame.0 as f32 * 0.61).cos() * 0.25;
    target.x = jx;
    target.z = jz;
    for mut t in demons.iter_mut() {
        let dx = target.x - t.translation.x;
        let dz = target.z - t.translation.z;
        // Proportional homing: as distance shrinks, so does the
        // step — the spiral tightens into sustained coincidence
        // rather than an oscillator that orbits the target forever.
        t.translation.x += dx * 0.08;
        t.translation.z += dz * 0.08;
    }
}

/// The breathing player: scale oscillates so the world's transform
/// set keeps changing even at terminal pile density — the pile
/// itself also keeps micro-moving (target jitter), so no phantom
/// -write shape arises.
fn player_breathing(frame: Res<Frame>, mut player: Query<&mut Transform, With<Player>>) {
    if let Ok(mut t) = player.single_mut() {
        let s = 1.0 + (frame.0 as f32 * 0.13).sin() * 0.1;
        t.scale = Vec3::splat(s);
    }
}

fn tick_frame(mut frame: ResMut<Frame>) {
    frame.0 += 1;
}

fn main() {
    App::new()
        .add_plugins(default_plugins())
        .add_plugins(playtest_plugins())
        .init_resource::<PileTarget>()
        .init_resource::<Frame>()
        .register_type::<Demon>()
        .register_type::<Player>()
        .register_type::<PileTarget>()
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (demon_dynamics, player_breathing, tick_frame).chain(),
        )
        .run();
}

/// Feature-gated plugin pair — mirrors the fetch script's mechanical
/// edit on fetched fixtures: plain builds get stock `DefaultPlugins`
/// (compiling this file with zero bevy_playtest involvement), test
/// builds get the harness plugins so the battery can drive the
/// child process.
#[cfg(feature = "playtest")]
fn default_plugins() -> bevy_playtest::PlaytestDefaultPlugins {
    bevy_playtest::PlaytestDefaultPlugins
}
#[cfg(not(feature = "playtest"))]
fn default_plugins() -> bevy::DefaultPlugins {
    bevy::DefaultPlugins
}

#[cfg(feature = "playtest")]
fn playtest_plugins() -> (bevy_playtest::PlaytestPlugin,) {
    (bevy_playtest::PlaytestPlugin,)
}
#[cfg(not(feature = "playtest"))]
fn playtest_plugins() -> () {}
