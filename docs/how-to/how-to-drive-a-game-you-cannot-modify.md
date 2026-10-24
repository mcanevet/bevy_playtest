# Drive a game you can't modify (external-BRP)

No harness imports in the game. Its binary must:

1. Add Bevy's remote plugins (`bevy::remote::RemotePlugin` and
   `bevy::remote::http::RemoteHttpPlugin` — requires the `bevy_remote`
   cargo feature)
2. Register the reflected types the harness inspects:
   `.register_type::<Transform>()`
3. Read the listen port from the `BRP_PORT` env var (the harness sets it
   for the subprocess)
4. Wrap the plugins in `ScheduleRunnerPlugin::run_loop(...)`: without
   winit's event loop, the default runner returns after one frame and
   the subprocess exits before BRP can connect (the fixtures do this)

From the test side, `brp_runner` is behind the (non-default) `brp`
feature — enable it in the test-side dependency:

```toml
[dev-dependencies]
bevy_playtest = { version = "0.3", features = ["brp"] }
```

```rust
use bevy_playtest::brp_runner::{run_scenario_brp, BrpRunConfig};

let cfg = BrpRunConfig {
    game_binary: env!("CARGO_BIN_EXE_my_game").to_string(),
    port: None, // auto-pick a free port
    ..Default::default()
};
let report = run_scenario_brp(&cfg, &scenario)?;
```

`game_args` (passed to the subprocess) and `startup_timeout` (default 30s)
are also configurable. This mode drives the chaos bot only. The oracle
set is reduced but not as far as you might expect: bounds, NaN transforms,
and the frozen-world (liveness) check all run over BRP queries.

To restore the remaining mode-1 capabilities (panic/log capture,
reset/cheat hooks, time scale control), add `PlaytestBrpPlugin` to the
game and enable the `brp-plugin` feature on the harness side: it serves
the `bevy_playtest/health`, `/reset`, `/cheat`, and `/time` BRP methods,
and the report's `brp_parity` field flips from "degraded" to "full". The
README's BRP parity matrix lists exactly which capabilities restore.

Ports: if `port: Some(p)` is set, the harness passes `p` to the game via
`BRP_PORT` and connects to it; with `port: None` the harness picks a free
port and passes it the same way.
