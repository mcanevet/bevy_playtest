# Tutorial: playtest your first game

In this tutorial we'll take a Bevy game from nothing to a green playtest
report. Along the way we'll add the harness to the game's Cargo.toml, wire
up two small cfg-gated helpers, write a scenario file, and watch a bot
play the game while oracles check it. By the end you'll have a repeatable
playtest you can extend.

## 1. Add the dependency (self-dev-dependency pattern)

We keep `cargo build --release` free of any bevy_playtest code while
letting plain `cargo test` run our playtests:

```toml
[dependencies]
bevy_playtest = { version = "0.3", optional = true }

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

We don't extract a `build_app()` or touch `WinitPlugin` — instead we add
two cfg-gated helper pairs around our existing plugin setup:

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

then use `.add_plugins(default_plugins().set(...))` where we had
`DefaultPlugins`, and `.add_plugins(playtest_plugins())` before
`.run()` (AFTER the group — the runner swap is last-set-wins).

`PlaytestDefaultPlugins` is a pass-through to `DefaultPlugins` normally,
and swaps to a headless runner when `BEVY_PLAYTEST_SCENARIO` is set.
Without the feature, plain `cargo build` compiles our original plugin
graph unchanged — zero bevy_playtest code ships.

## 3. Write the scenario

Our scenario needs no per-test annotations: the oracles and bounds
invariants watch every entity with a `Transform`, so beyond the cfg'd
helpers from step 2 we don't add any bevy_playtest-specific code to the
game. Structural entities at the origin won't trip them, but stray
geometry will.

```json
{
  "name": "smoke",
  "bot": { "type": "chaos", "seed": 42 },
  "duration_s": 10.0,
  "invariants": [
    { "name": "bounds", "rule": "nodes_in_bounds", "max_x": 960.0, "max_y": 540.0 }
  ]
}
```

Every invariant needs a unique `name` and a `rule`. Bounds are per-axis
(`min_x`/`max_x`/`min_y`/`max_y`, plus `min_z`/`max_z` if your game
moves in three dimensions), not a single box.

## 4. Run it

With the wiring in place, we write the test:

```rust
// tests/playtest.rs — commit this file; per-game assertions live here
#[test]
fn menu_scenario() {
    bevy_playtest::battery::run_and_expect_clean(
        env!("CARGO_BIN_EXE_my_game"),
        "playtests/menu.json",
    );
}
```

Now we run it:

```
$ cargo test
running 1 test
test menu_scenario ... ok

test result: ok. 1 passed; 0 failed
```

That `ok` means the oracles genuinely ran: for the scenario's duration a
bot pressed keys in our game while NaN checks, bounds checks, the
liveness oracle, panic capture and log capture all watched — and none of
them fired. Had something gone wrong, the test would print the violation
list instead — NaN transform, out-of-bounds entity, crash, or ERROR log,
each with the tick and entities involved.

## 5. Make it repeat

Same `seed` → same key sequence → same result. A failing run with seed
42 will fail identically on every re-run — that's the feature. So when a
failure appears, we don't roll the dice again: we leave the seed alone,
re-run to reproduce while we debug the game, and the same seed proves
the fix — the run passes again only when the game actually changed.

## Wrap-up

We now have a playtest that any contributor can run with plain
`cargo test`, that replays bit-for-bit from its seed, and that fails
loudly the moment the game misbehaves.

When you're ready for
the next step — a specific task, the exact JSON fields, or the reasoning
behind the design — the [how-to guides](../how-to/),
[reference](../reference/) and [explanation](../explanation/) are the
places to go.
