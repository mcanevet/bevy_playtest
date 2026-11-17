//! Seeded by scripts/fetch_bevy_examples.sh — COMMITTED, edit freely.
//! Plain `cargo test` runs the default playtest battery against the
//! game binary (compiled with the playtest feature via the
//! self-dev-dependency). Per-game assertions belong here.
//!
//! This game is the canonical non-WASD fixture: its whole interactive
//! surface is `Digit1`/`Digit2` level selection, which the blind
//! chaos-bot key set (WASD+arrows) never presses — undecorated and
//! undiscovered, every battery run pins a `frozen_world` finding at
//! frame ~521, a false verdict about the GAME. Two tests pin the two
//! tiers that fix this, in precedence order:
//!
//! - `playtest_battery`: ZERO flags. The battery parent's source scan
//!   (milliseconds; this test binary's runtime `CARGO_MANIFEST_DIR`
//!   IS the fixture crate, so the scan walks this game's `src/`)
//!   finds `KeyCode::Digit1`/`Digit2` and REPLACES the WASD/arrows
//!   default with them — the ~200 chaos presses concentrate on the
//!   keys this game actually reads, and the input-layer verdicts go
//!   clean out of the box (the progress-surface verdict is pinned
//!   separately, see below).
//! - `playtest_battery_injected`: `BEVY_PLAYTEST_DISCOVER_INJECT=1`
//!   (implies discovery). The parent probes the input surface first
//!   (cache-first; the cache lives in `.bevy_playtest/cache/`,
//!   gitignored) and feeds the verified keys to the chaos bot. Kept
//!   for the cases the scan cannot solve: keys whose mention is
//!   guarded away (inert in practice) and pointer-only surfaces a
//!   text scan cannot see at all.
//!
//! `--test-threads=1` is mandatory for the fixture battery
//! (`run_scenario` is exclusive per process); `set_var` below relies
//! on it to be race-free.
#[cfg(feature = "playtest")]
mod playtests {
    /// Restore/remove `BEVY_PLAYTEST_DISCOVER_INJECT` on drop so the
    /// injected test cannot contaminate the zero-config one (same
    /// test binary, sequential execution).
    struct InjectVarGuard;
    impl InjectVarGuard {
        fn set() -> Self {
            // Parent-side discovery opt-in (implies
            // `BEVY_PLAYTEST_DISCOVER`; the game child never sees it
            // — the battery strips every `BEVY_PLAYTEST_*` variable
            // from spawned children). Edition 2024: set_var is
            // unsafe; `--test-threads=1` makes it race-free.
            unsafe { std::env::set_var("BEVY_PLAYTEST_DISCOVER_INJECT", "1") };
            InjectVarGuard
        }
    }
    impl Drop for InjectVarGuard {
        fn drop(&mut self) {
            unsafe { std::env::remove_var("BEVY_PLAYTEST_DISCOVER_INJECT") };
        }
    }

    #[test]
    fn playtest_battery() {
        // ZERO config: no env vars, no scenario file. If this test
        // fails with a `frozen_world` finding, the source-derived
        // layering regressed (or someone renamed this game's
        // Digit1/Digit2 bindings). Golden verdict besides that: the
        // stock demo exposes no verifiable progress surface, so the
        // battery's honest finding (`progress_surface_empty`) is
        // pinned present and ALONE — a re-greening is an oracle
        // regression, any extra finding is a misfire.
        let report = bevy_playtest::battery::run_and_expect_findings(
            env!("CARGO_BIN_EXE_fixture_loading_screen"),
            bevy_playtest::battery::BUILTIN_BATTERY,
            &[(
                "progress_surface_empty",
                "run",
                "no verifiable progress surface",
            )],
        );
        // And pin the replacement itself, not just the outcome: the
        // game cites exactly Digit1/Digit2, so the scan-derived
        // surface IS that key set (exact equality — stray extras
        // from a misbehaving scan fail here).
        let layered = report
            .source_derived_keys
            .as_ref()
            .expect("the source scan must stamp source_derived_keys");
        assert_eq!(
            layered,
            &vec!["Digit1".to_string(), "Digit2".to_string()],
            "the scan-derived surface must be exactly the game's bound keys"
        );
    }

    #[test]
    fn playtest_battery_injected() {
        let _inject = InjectVarGuard::set();
        // Same golden verdict as the zero-config run (see there);
        // the discovery layer changes the key surface, not the
        // progress surface.
        bevy_playtest::battery::run_and_expect_findings(
            env!("CARGO_BIN_EXE_fixture_loading_screen"),
            bevy_playtest::battery::BUILTIN_BATTERY,
            &[(
                "progress_surface_empty",
                "run",
                "no verifiable progress surface",
            )],
        );
    }
}

// `cargo package` strips `[dev-dependencies]`, so the self-dev-dependency
// that auto-activates `playtest` is gone in an unpacked archive. Rather
// than fail to compile (missing `bevy_playtest`) or silently run zero
// playtests, surface it as an ignored test pointing at the fix.
#[cfg(not(feature = "playtest"))]
#[test]
#[ignore = "playtest feature inactive: self-dev-dependency stripped (e.g. cargo package). Run `cargo test --features playtest`."]
fn playtest_battery() {}

#[cfg(not(feature = "playtest"))]
#[test]
#[ignore = "playtest feature inactive: self-dev-dependency stripped (e.g. cargo package). Run `cargo test --features playtest`."]
fn playtest_battery_injected() {}
