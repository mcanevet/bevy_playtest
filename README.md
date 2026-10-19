# bevy_playtest

A playtesting harness for Bevy games. Scenarios drive a deterministic bot
through your game while oracles check invariants and produce a machine-readable
report (JSON + JUnit XML).

**Documentation:** [tutorial](docs/tutorial.md) · [how-to guides](docs/how-to.md) ·
[reference](docs/reference.md) · [explanation](docs/explanation.md)

## Integrating a game

### Basic integration (tag gameplay entities)

Tag gameplay-relevant entities with `bevy_playtest::contract::Gameplay`. That's it.

```rust
use bevy::prelude::*;

fn tag_gameplay(mut commands: Commands, ball: Query<Entity, With<Ball>>) {
    for e in ball.iter() {
        commands.entity(e).insert(bevy_playtest::contract::Gameplay);
    }
}
```

**Two practical requirements for in-process runs:**

1. **Extract app construction from `main()`** — Bevy apps must construct an `App`
   somewhere accessible to the harness. If your game wires everything inline in
   `main()`, extract it to a `build_app()` function (standard Bevy testing pattern).

2. **Disable `WinitPlugin`** — the harness drives input itself; winit's event loop
   conflicts with test runners. Use:
   ```rust
   app.add_plugins(DefaultPlugins.build().disable::<bevy::winit::WinitPlugin>());
   ```
   Render/audio/assets still initialize; the game runs headless.

### Zero-contract external-BRP (experimental)

Run the game as a subprocess and drive it over Bevy's Remote Protocol — no
harness imports in the game at all. Requires the game binary to:

1. Add Bevy's remote plugins (`RemotePlugin::default()` +
   `bevy::remote::http::RemoteHttpPlugin::default()`, with the
   `bevy_remote` cargo feature)
2. Register the types the harness inspects:
    ```rust
    .register_type::<Transform>()
    ```
3. Read the listen port from the `BRP_PORT` env var and pass it to
   `RemoteHttpPlugin::default().with_port(port)` (the harness sets it)

Then from the test side:
```rust
use bevy_playtest::brp_runner::{run_scenario_brp, BrpRunConfig};

let cfg = BrpRunConfig {
    game_binary: env!("CARGO_BIN_EXE_my_game").to_string(),
    port: Some(4570),
    ..Default::default()
};
let report = run_scenario_brp(&cfg, &scenario)?;
```

Reports carry `harness_mode: "external_brp"`. Limits versus in-process:
chaos bot only, no reset/cheat hooks, and a reduced oracle set — bounds
and NaN transforms are checked, but panic, log, and liveness oracles
require in-process mode.

See the [docs](docs/index.md) for the full guides.
