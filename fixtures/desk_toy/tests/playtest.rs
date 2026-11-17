//! Seeded by scripts/fetch_bevy_examples.sh — COMMITTED, edit freely.
//! Plain `cargo test` runs the default playtest battery against the
//! game binary (compiled with the playtest feature via the
//! self-dev-dependency). Per-game assertions belong here.
//!
//! Golden verdict — the phantom-write exhibit: this stock demo is a
//! settled physics scene that keeps REWRITING its transforms with
//! bit-identical values every frame. `Changed<Transform>` fires
//! constantly, so the old change-streak liveness oracle is fooled
//! and passes; the value-hash phantom-write check catches it. The
//! demo also exposes no verifiable progress surface (no `Reflect`
//! game types), so the run carries exactly two findings — both
//! pinned present and pinned ALONE below: lose the phantom finding
//! and the detector regressed; gain any other and an oracle
//! misfired.
#[cfg(feature = "playtest")]
#[test]
fn playtest_battery() {
    bevy_playtest::battery::run_and_expect_findings(
        env!("CARGO_BIN_EXE_fixture_desk_toy"),
        bevy_playtest::battery::BUILTIN_BATTERY,
        &[
            (
                "frozen_world",
                "world",
                "phantom writes: Changed<Transform> fired on every frame",
            ),
            (
                "progress_surface_empty",
                "run",
                "no verifiable progress surface",
            ),
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
