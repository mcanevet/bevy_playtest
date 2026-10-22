# Tutorial: playtest your first game

Goal: run a scenario against a Bevy game and get a report.

## 1. Add the dependency (self-dev-dependency pattern)

The integration keeps `cargo build --release` free of any bevy_playtest
code while letting plain `cargo test` run your playtests:

```toml
[dependencies]
bevy_playtest = { version = "0.1", optional = true }

[dev-dependencies]
# Self-dependency: activates OUR OWN `playtest` feature only for
# test builds. Normal & release builds stay clean.
my_game = { path = ".", features = ["playtest"] }

[features]
playtest = ["dep:bevy_playtest", "bevy_playtest/takeover"]
```

(replace `my_game` with your package name)

Packaging note: `cargo package` strips `[dev-dependencies]`, so the
self-dev-dependency no longer auto-activates `playtest` in an unpacked
archive. The generated test handles this gracefully: it reports as
ignored with the reason and the fix — `cargo test --features playtest`
restores the full battery (the feature and optional dependency survive
packaging; only the auto-activation is lost).

## 2. Wire the plugin (two cfg'd helper pairs)

No extracted `build_app()`, no `WinitPlugin` disabling — cfg-gated
helpers around your existing plugin setup:

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
Without the feature, plain `cargo build` compiles the ORIGINAL plugin
graph unchanged — zero bevy_playtest code ships.

## 3. Tag gameplay entities (optional but recommended)

Entities the harness should treat as gameplay get one marker:

```rust
use bevy_playtest::contract::Gameplay;

commands.entity(ball).insert(Gameplay);
```

The bounds invariant checks `Gameplay`-tagged entities. The NaN and
liveness oracles watch every entity with a `Transform`, so they work
whether or not you tag anything. There is also a hard-coded global
bounds check (±1000 on all axes) that runs regardless of tagging or
invariant declarations — untagged structural entities at the origin
won't trip it, but stray geometry will.

## 4. Write the scenario

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
(`min_x`/`max_x`/`min_y`/`max_y`), not a single box.

## 5. Run it

From `cargo test` (the recommended integration — see the README's
"Mode 1: In-process (takeover)" section for the Cargo.toml pattern):

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

Known findings can be pinned with plain assertions:
`assert_eq!(report.violation_rules(), ["frozen_world"])` for rule-only,
or `assert_eq!(report.violation_pairs(), [("frozen_world", "world")])` to
pin both rule and target — passes while the known finding is present,
fails when it changes.

Or directly from a test (in-process, no child):

```rust
use bevy_playtest::scenario::Scenario;

#[test]
fn smoke() {
    let scenario: Scenario = serde_json::from_str(SCENARIO_JSON).unwrap();
    let mut app = App::new();
    app.add_plugins(bevy_playtest::PlaytestDefaultPlugins);
    app.add_plugins(bevy_playtest::PlaytestPlugin);
    // ... your game systems ...
    let report = bevy_playtest::run_scenario(&mut app, &scenario).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
}
```

Failures print the violation list — NaN transform, out-of-bounds entity,
crash, or ERROR log, each with the tick and entities involved.

## 6. Make it repeat

Same `seed` → same key sequence → same result. Put the failing seed in the
scenario file and re-run until the invariant passes.

## 7. Go further

- **Replay divergence** (`"verify_replay": true`) proves determinism by
  running the scenario twice and comparing per-frame fingerprints.
- **Cheat postconditions** (`expects` on cheat entries) catch silently
  no-op hooks.
- **Coverage declarations** (`observes: [...]`) report unobserved events.
- **External-BRP mode** (`BRP_PORT` env var) drives a game binary over
  Bevy's Remote Protocol — no harness imports needed.

See the [how-to guides](docs/how-to.md) for recipes and the
[reference](docs/reference.md) for API details.
