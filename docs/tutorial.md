# Tutorial: playtest your first game

Goal: run a scenario against a Bevy game and get a report.

## 1. Add the dependency

```toml
[dev-dependencies]
bevy_playtest = "0.1"
```

## 2. Wire the plugin (one line)

No extracted `build_app()`, no `WinitPlugin` disabling — just add the
plugin **after** your plugin group:

```rust
app.add_plugins(bevy_playtest::PlaytestDefaultPlugins);
app.add_plugins(bevy_playtest::PlaytestPlugin); // AFTER the group
```

`PlaytestDefaultPlugins` is a pass-through to `DefaultPlugins` normally,
and swaps to a headless runner when `BEVY_PLAYTEST_SCENARIO` is set. The
one ordering rule: add `PlaytestPlugin` after any plugin group that sets
a runner, or the takeover is silently inert (runner swap is last-set-wins).

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

From the CLI:

```console
$ bevy_playtest init                  # writes playtest.scenario.json
$ $EDITOR playtest.scenario.json      # set "target_selector": "all" on invariants
$ bevy_playtest run playtest.scenario.json
```

Or from a test:

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
