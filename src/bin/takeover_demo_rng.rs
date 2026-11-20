//! Demo "game" whose world depends on OS randomness: at startup it asks
//! the C library's `getrandom()` for a value (what `rand::rng()` and the
//! `getrandom` crate do underneath) and places an entity there. Without
//! the battery's deterministic-randomness shim, two runs with the same
//! seed produce different worlds; with it, the same world. See
//! tests/it/battery.rs: os_randomness_is_deterministic_per_seed.
//!
//! Linux only (libc `getrandom`); elsewhere it is a stub so builds with
//! the `takeover` feature still link.
#[cfg(not(target_os = "linux"))]
fn main() {}

#[cfg(target_os = "linux")]
fn main() {
    use bevy_app::prelude::*;
    use bevy_ecs::prelude::*;
    use bevy_playtest::PlaytestPlugin;
    use bevy_transform::prelude::*;

    unsafe extern "C" {
        fn getrandom(buf: *mut u8, len: usize, flags: u32) -> isize;
    }

    let mut app = App::new();
    app.add_plugins(PlaytestPlugin);
    app.add_systems(Startup, |mut commands: Commands| {
        let mut bytes = [0u8; 8];
        // SAFETY: buffer of 8 writable bytes.
        let n = unsafe { getrandom(bytes.as_mut_ptr(), bytes.len(), 0) };
        assert_eq!(n, 8, "getrandom failed");
        let x = (u64::from_le_bytes(bytes) % 997) as f32;
        commands.spawn(Transform::from_xyz(x, 0.0, 0.0));
    });
    app.add_systems(Update, |mut q: Query<&mut Transform>| {
        for mut t in q.iter_mut() {
            t.translation.y += 0.1;
        }
    });
    app.run();
}
