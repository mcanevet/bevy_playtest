//! Seeded by scripts/fetch_bevy_examples.sh — COMMITTED, edit freely.
//! Plain `cargo test` runs the default playtest battery against the
//! game binary (compiled with the playtest feature via the
//! self-dev-dependency). Per-game assertions belong here.
#[cfg(feature = "playtest")]
#[test]
fn playtest_battery() {
    // game_menu is pointer-driven (bevy_picking): the harness emits
    // wrapped WindowEvents so picking fires, and the UI liveness watcher
    // (ui-liveness feature) sees Changed<UiTransform> — the menu
    // responds to bot clicks, so the run is healthy: no violations.
    bevy_playtest::battery::run_and_expect_clean(
        env!("CARGO_BIN_EXE_fixture_game_menu"),
        bevy_playtest::battery::BUILTIN_BATTERY,
    );
}

// `cargo package` strips `[dev-dependencies]`, so the self-dev-dependency
// that auto-activates `playtest` is gone in an unpacked archive. Rather
// than fail to compile (missing `bevy_playtest`) or silently run zero
// playtests, surface it as an ignored test pointing at the fix.
#[cfg(not(feature = "playtest"))]
#[test]
#[ignore = "playtest feature inactive: self-dev-dependency stripped (e.g. cargo package). Run `cargo test --features playtest`."]
fn playtest_battery() {}
