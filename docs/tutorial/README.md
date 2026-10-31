# Tutorial: your first playtest in 5 minutes

In this tutorial we'll take a Bevy game from nothing to a green
playtest report. Along the way we'll add the harness to the game's
`Cargo.toml`, wire up two small cfg-gated helpers, write a test that
runs the built-in battery, and watch a bot play the game while oracles
check it. By the end you'll have a repeatable playtest you can extend.

The integration touches three files — `Cargo.toml`, `src/main.rs`,
`tests/playtest.rs` — needs no scenario to start, no contract in game
code, and compiles to zero bevy_playtest code in normal and release
builds.

## 1. Add the dependency (self-dev-dependency pattern)

We keep `cargo build --release` free of any bevy_playtest code while
letting plain `cargo test` run our playtests. Open `Cargo.toml` and add
an optional dependency plus the self-dev-dependency that activates it
only for test builds:

```toml
[dependencies]
bevy_playtest = { version = "0.4", optional = true }

[dev-dependencies]
# Self-dependency: activates OUR OWN `playtest` feature only for
# test builds. Normal & release builds stay clean.
my_game = { path = ".", features = ["playtest"] }

[features]
playtest = ["dep:bevy_playtest", "bevy_playtest/takeover"]
```

(use your own package name in place of `my_game`)

The whys of this pattern — and the `cargo package` wrinkle where an
unpacked archive loses the auto-activation — are covered in
[why the self-dev-dependency pattern](../explanation/why-the-self-dev-dependency-pattern.md).

## 2. Wire the plugin (two cfg'd helper pairs)

Open `src/main.rs`. We don't extract a `build_app()` or touch
`WinitPlugin` — instead we add two cfg-gated helper pairs around our
existing plugin setup. Replace your `DefaultPlugins` wiring with:

```rust
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
```

then use `.add_plugins(default_plugins().set(...))` where you had
`DefaultPlugins`, and `.add_plugins(playtest_plugins())` before
`.run()` (AFTER the group — the runner swap is last-set-wins).

`PlaytestDefaultPlugins` is a pass-through to `DefaultPlugins` normally,
and swaps to a headless runner when `BEVY_PLAYTEST_SCENARIO` is set.
Without the feature, plain `cargo build` compiles our original plugin
graph unchanged — zero bevy_playtest code ships.

## 3. Write the test

Create a file at `tests/playtest.rs` — commit this file; per-game
assertions live here. The whole file, no scenario needed:

```rust
// tests/playtest.rs
#[test]
fn playtest_battery() {
    bevy_playtest::battery::run_and_expect_clean(
        env!("CARGO_BIN_EXE_my_game"),
        bevy_playtest::battery::BUILTIN_BATTERY,
    );
}
```

(again, replace `my_game` with your package name)

That's it — no scenario file, no bevy_playtest code in the game logic.
The harness provides a built-in battery scenario (`BUILTIN_BATTERY`)
that drives the chaos bot (keys + mouse) while oracles watch.

## 4. Run it

With the wiring in place, run:

```bash
$ cargo test
running 1 test
test playtest_battery ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

That `ok` is a verdict, not a formality: for the battery's duration a
seeded bot pressed keys in your game while NaN checks, bounds checks,
the liveness oracle, panic capture and log capture all watched — and
none of them fired. Had something gone wrong, the test would print the
violation list instead — NaN transform, out-of-bounds entity, crash, or
ERROR log, each with the tick and entities involved.

The battery catches out of the box:
- **NaN/finite transforms** — any non-finite `Transform` → Fail
- **Runaway geometry** — entities leaving bounds (a hard-coded global
  ±1000 on x/y/z runs even with nothing declared)
- **Frozen world** — no entity movement across the liveness grace
  window → Fail (catches soft-locks)
- **Panic capture** — crash → Crash
- **Log capture** — ERROR → Fail, WARN → informational
- **Vacuity guard** — no entities and no inputs means the run proved
  nothing; it fails loudly instead of passing vacuously

## 5. Make it repeat

Same seed → same key sequence → same result. A failing run will fail
identically on every re-run — that's the feature. So when a failure
appears, we don't roll the dice again: we leave the seed alone, re-run
to reproduce while we debug the game, and the same seed proves the fix
— the run passes again only when the game actually changed.

The chaos bot is seeded internally, so declaring nothing is sufficient
for reproducibility. When you need a specific seed, you can declare one
in an authored scenario (see step 6 below).

## 6. Author your own scenario (optional)

When the battery isn't specific enough, author a scenario file. Create
a directory at `playtests/` (or whatever suits your layout) and drop a
JSON file in it:

```json
// playtests/menu.json
{
  "name": "menu-smoke",
  "bot": { "type": "chaos", "seed": 42 },
  "duration_s": 10.0,
  "invariants": [
    { "name": "bounds", "rule": "nodes_in_bounds", "max_x": 960.0, "max_y": 540.0 }
  ]
}
```

Every invariant needs a unique `name` and a `rule`. Bounds are per-axis
(`min_x`/`max_x`/`min_y`/`max_y`, plus `min_z`/`max_z` if your game moves
in three dimensions), not a single box.

Reference it from the same helper:

```rust
#[test]
fn menu_scenario() {
    bevy_playtest::battery::run_and_expect_clean(
        env!("CARGO_BIN_EXE_my_game"),
        "playtests/menu.json",
    );
}
```

To opt out of mouse clicks, add `"mouse_buttons": []` to the scenario's
`raw_surface` field. To opt out of chaos keys entirely, add
`"keys": []` (mouse-only chaos). See the
[scenario reference](../reference/README.md) for the full JSON schema,
and [script keys instead of chaos](../how-to/how-to-script-keys-instead-of-chaos.md)
for scripted input.

## Wrap-up

We now have a playtest that any contributor can run with plain
`cargo test`, that replays bit-for-bit from its seed, and that fails
loudly the moment the game misbehaves.

When you're ready for the next step — the exact JSON fields, scripting
keys instead of chaos, proving determinism, or the reasoning behind the
design — the [how-to guides](../how-to/), [reference](../reference/)
and [explanation](../explanation/) are the places to go.
