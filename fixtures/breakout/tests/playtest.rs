//! Seeded by scripts/fetch_bevy_examples.sh — COMMITTED, edit freely.
//! Plain `cargo test` runs the default playtest battery against the
//! game binary (compiled with the playtest feature via the
//! self-dev-dependency). Per-game assertions belong here.
//!
//! Golden verdict: this stock demo exposes no verifiable progress
//! surface (its state types derive no `Reflect`, and engine types
//! are excluded from the surface by doctrine), so the battery's
//! HONEST verdict is a `progress_surface_empty` finding. Pin the
//! verdict exactly: the finding must stay present (a regression of
//! the oracle would silently re-green this game) and must stay
//! ALONE (any new finding is a real regression or an oracle
//! misfire).
#[cfg(feature = "playtest")]
#[test]
fn playtest_battery() {
    bevy_playtest::battery::run_and_expect_findings(
        env!("CARGO_BIN_EXE_fixture_breakout"),
        bevy_playtest::battery::BUILTIN_BATTERY,
        &[(
            "progress_surface_empty",
            "run",
            "no verifiable progress surface",
        )],
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
