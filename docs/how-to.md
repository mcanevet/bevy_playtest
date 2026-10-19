# How-to guides

Task-focused recipes. Each assumes the [tutorial](tutorial.md) basics.

## Script keys instead of chaos

For deterministic replays, use `synthetic_keyboard` with a frame-indexed
key schedule:

```json
"bot": {
  "type": "synthetic_keyboard",
  "key_presses": [
    { "frame": 0, "key": "left", "hold_frames": 60 },
    { "frame": 120, "key": "space" }
  ]
}
```

Frames are simulation ticks (default 60 tps, so frame 120 ≈ 2 s). Keys
accept `KeyCode` spellings (`ArrowLeft`, `KeyW`) or shortcuts (`left`,
`w`); unknown names are rejected at load with a did-you-mean suggestion.
`hold_frames` defaults to 1.

## Reset or cheat between runs

Games register typed hooks as resources. A reset runs before the scenario;
cheats fire at scheduled frames:

```rust
use bevy_playtest::contract::{CheatHooks, ResetHooks};

app.world_mut().insert_resource(ResetHooks::default());
app.world_mut()
    .resource_mut::<ResetHooks>()
    .register("my_reset", |world| {
        // restore level state...
    });

app.world_mut().insert_resource(CheatHooks::default());
app.world_mut()
    .resource_mut::<CheatHooks>()
    .register("invincible", |world| {
        // apply cheat...
    });
```

Then reference them from the scenario:

```json
"setup": {
  "resets": [ { "kind": "my_reset" } ],
  "cheats": [ { "frame": 300, "kind": "invincible" } ]
}
```

Unknown kinds are rejected at load — not silently ignored.

## Drive a game you can't modify (external-BRP)

No harness imports in the game. Its binary must:

1. Add Bevy's remote plugins (`bevy::remote::RemotePlugin` and
   `bevy::remote::http::RemoteHttpPlugin` — requires the `bevy_remote`
   cargo feature)
2. Register the reflected types the harness inspects:
   `.register_type::<Transform>()` plus your gameplay marker
3. Read the listen port from the `BRP_PORT` env var (the harness sets it
   for the subprocess)

From the test side:

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
are also configurable. This mode drives the chaos bot only; reset and
cheat hooks require in-process mode.

## Get JUnit XML for CI

Collect named reports and convert:

```rust
let xml = bevy_playtest::junit::reports_to_junit(
    &[("smoke".to_string(), report)],
    "playtest-suite",
);
std::fs::write("playtest-junit.xml", xml)?;
```

Most CI systems (Jenkins, GitLab, and GitHub Actions via JUnit adapters)
ingest JUnit XML natively.

## Reproduce a failure

- The report includes the bot seed — reuse it verbatim.
- Run locally with the same seed; chaos is deterministic per seed.

## Know what's intentionally rejected

Scenario load fails with `NotImplemented` for: `custom` invariants,
`eventually` mode, target filtering, time windows. Better a loud error
than a test that passes vacuously. Invalid values (negative durations,
broken bound pairs, typo'd key names) are rejected with specific errors.
