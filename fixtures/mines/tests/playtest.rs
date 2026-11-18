//! Seeded by scripts/fetch_bevy_examples.sh — COMMITTED, edit freely.
//! Plain `cargo test` runs the default playtest battery against the
//! game binary (compiled with the playtest feature via the
//! self-dev-dependency). Per-game assertions belong here.
//!
//! Golden verdict: this stock demo exposes no verifiable progress
//! surface (its state types derive no `Reflect`, and engine types
//! are excluded from the surface by doctrine), so the battery's
//! HONEST verdict includes a `progress_surface_empty` finding —
//! declared present. And mines boots into a static menu whose only
//! escape is a primary click precisely on the New Game button —
//! from the harness's seat that is indistinguishable from a
//! soft-lock, and `frozen_world` fires from onset + the liveness
//! window (~frame 901 in the 20 s battery at 60 tps). Adjudicated
//! as a genuine, pre-existing finding (the single-slot bot fails
//! it on 3/5 sampled seeds too) — NOT a harness regression. Both
//! findings are declared as scoped, justified expectations via
//! `run_and_expect_scoped_findings`: the tight frozen_world frame
//! window admits only freezes beginning at startup (the menu
//! phase); a mid-game wedge fires at onset + 901 and stays red, and
//! if either finding ever stops occurring (game fix, stream shift)
//! this test goes red until the declaration is removed.
#[cfg(feature = "playtest")]
#[test]
fn playtest_battery() {
    bevy_playtest::battery::run_and_expect_scoped_findings(
        env!("CARGO_BIN_EXE_fixture_mines"),
        bevy_playtest::battery::BUILTIN_BATTERY,
        &[
            bevy_playtest::battery::ExpectFinding::new(
                bevy_playtest::rules::Rule::FrozenWorld,
                "menu is an idle world; the only escape is a lucky New Game \
                 click (adjudicated pre-existing finding, see the harness repo)",
            )
            .target("world")
            .first_frame_in(880..=920),
            bevy_playtest::battery::ExpectFinding::new(
                bevy_playtest::rules::Rule::ProgressSurfaceEmpty,
                "stock demo state types derive no Reflect, so no verifiable \
                 progress surface exists (honest verdict, pinned present)",
            )
            .target("run"),
        ],
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
