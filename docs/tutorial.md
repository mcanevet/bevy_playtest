# Tutorial: playtest your first game

Goal: run a scenario against a Bevy game and get a report.

## 1. Add the dependency

```toml
[dev-dependencies]
bevy_playtest = "0.1"
```

## 2. Expose an app constructor

If your `main()` wires everything inline, extract it — the standard Bevy
testing pattern:

```rust
pub fn build_app() -> App {
    App::new()
        .add_plugins(DefaultPlugins.build().disable::<bevy::winit::WinitPlugin>())
        .add_systems(Startup, spawn_world)
}
```

Keep `main()` calling `build_app().run()`. Only winit is disabled in the
harness path; render, assets, and audio still initialize.

## 3. Tag gameplay entities (optional but recommended)

Anything the harness should track gets one marker:

```rust
use bevy_playtest::contract::Gameplay;

commands.entity(ball).insert(Gameplay);
```

This enables the `nodes_in_bounds` invariant and the frozen-world oracle.

## 4. Write the scenario

```json
{
  "name": "smoke",
  "bot": { "type": "chaos", "seed": 42 },
  "duration_s": 10.0,
  "invariants": [{ "kind": "nodes_in_bounds", "max": [1920.0, 1080.0] }],
  "setup": {}
}
```

## 5. Run it

```rust
#[test]
fn smoke() {
    let scenario: Scenario = serde_json::from_str(SCENARIO_JSON).unwrap();
    let mut app = build_app();
    let report = bevy_playtest::run_scenario(&mut app, &scenario).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
}
```

Failures print the violation list — NaN transform, out-of-bounds entity,
crash, or ERROR log, each with the tick and entities involved.

## 6. Make it repeat

Same `seed` → same key sequence → same result. Put the failing seed in the
bug report; anyone reproducing it sees the same violations.

---

That's the whole loop. Next: the [How-to guides](how-to.md) cover scripting
keys, CI setup, and external-BRP mode for games you can't modify.
