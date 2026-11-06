//! Seeded by scripts/fetch_bevy_examples.sh — COMMITTED, edit freely.
//! Plain `cargo test` runs the default playtest battery against the
//! game binary (compiled with the playtest feature via the
//! self-dev-dependency). Per-game assertions belong here.
//!
//! This game is the reason discovery-driven injection exists: its
//! whole interactive surface is `Digit1`/`Digit2` level selection,
//! which the default chaos-bot key set (WASD+arrows) never presses —
//! undecorated, every battery run pins a `frozen_world` finding at
//! frame ~521, a false verdict about the GAME. With
//! `BEVY_PLAYTEST_DISCOVER_INJECT=1` (implies discovery), the
//! battery parent probes the input surface first (cache-first,
//! ~40s cold; the cache lives in `.bevy_playtest/cache/`, gitignored)
//! and feeds exactly those keys to the chaos bot — the same battery
//! run goes clean for its full duration.
#[cfg(feature = "playtest")]
#[test]
fn playtest_battery() {
    // Parent-side discovery opt-in (implies `BEVY_PLAYTEST_DISCOVER`;
    // the game child never sees it — the battery strips every
    // `BEVY_PLAYTEST_*` variable from spawned children). Edition
    // 2024: set_var is unsafe; `--test-threads=1` is mandated for
    // the fixture battery anyway, so there is no cross-test race.
    unsafe { std::env::set_var("BEVY_PLAYTEST_DISCOVER_INJECT", "1") };
    bevy_playtest::battery::run_and_expect_clean(
        env!("CARGO_BIN_EXE_fixture_loading_screen"),
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
