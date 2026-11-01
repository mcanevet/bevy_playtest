//! Seeded by scripts/fetch_bevy_examples.sh — COMMITTED, edit freely.
//! Plain `cargo test` runs the default playtest battery against the
//! game binary (compiled with the playtest feature via the
//! self-dev-dependency). Per-game assertions belong here.
#[cfg(feature = "playtest")]
#[test]
fn playtest_battery() {
    let report = bevy_playtest::battery::run(
        env!("CARGO_BIN_EXE_fixture_loading_screen"),
        bevy_playtest::battery::BUILTIN_BATTERY,
    )
    .expect("battery infrastructure failure");
    // Known finding: static loading screen screens trip the liveness
    // oracle. Keyboard delivery is healthy post just_pressed fix —
    // verified 200+ delivered press transitions — but the level-select
    // scene holds no animating transforms, so the static-scene verdict
    // is CORRECT for now. Once the harness emits CursorMoved (pointer
    // path), menu interaction may animate the scene; re-check then.
    assert_eq!(report.violation_pairs(), [("frozen_world", "world")]);
}

// `cargo package` strips `[dev-dependencies]`, so the self-dev-dependency
// that auto-activates `playtest` is gone in an unpacked archive. Rather
// than fail to compile (missing `bevy_playtest`) or silently run zero
// playtests, surface it as an ignored test pointing at the fix.
#[cfg(not(feature = "playtest"))]
#[test]
#[ignore = "playtest feature inactive: self-dev-dependency stripped (e.g. cargo package). Run `cargo test --features playtest`."]
fn playtest_battery() {}
