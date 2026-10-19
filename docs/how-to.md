# How-to guides

Task-focused recipes. Each assumes the [tutorial](tutorial.md) basics.

## Script keys instead of chaos

For deterministic replays, use `synthetic_keyboard` with a key schedule:

```json
"bot": {
  "type": "synthetic_keyboard",
  "schedule": [
    { "at_s": 0.0, "key": "ArrowLeft" },
    { "at_s": 0.5, "key": "ArrowLeft", "release_at_s": 1.0 }
  ]
}
```

## Reset or cheat between runs

Register typed hooks so the harness can restore state or apply cheats:

```rust
use bevy_playtest::sinks; // reset/cheat hook registry lives here

// register a reset kind the scenario can name:
// "setup": { "reset": { "kind": "my_reset" } }
```

Unknown kinds are rejected at scenario load — not silently ignored.

## Drive a game you can't modify (external-BRP)

No harness imports needed in the game. It must add (in a thin binary):

1. `RemotePlugin::default()` and `RemoteHttpPlugin::default().with_port(port)`
2. `.register_type::<Transform>()` and your gameplay marker type
3. Read the port from `BRP_PORT`

```rust
use bevy_playtest::brp_runner::{run_scenario_brp, BrpRunConfig};

let cfg = BrpRunConfig {
    game_binary: env!("CARGO_BIN_EXE_my_game").to_string(),
    port: Some(4570),
    ..Default::default()
};
let report = run_scenario_brp(&cfg, &scenario)?;
```

This mode drives the chaos bot only; reset and cheat hooks require
in-process mode or a game-side BRP method.

## Get JUnit XML for CI

Convert the report after a run:

```rust
let xml = bevy_playtest::junit::to_junit_xml(&report);
std::fs::write("playtest-junit.xml", xml)?;
```

Most CI systems (GitHub Actions via adapters, Jenkins, GitLab) ingest
JUnit XML natively.

## Reproduce a failure

- The report's violations include the bot seed — reuse it verbatim.
- Run locally with the same seed; chaos is deterministic per seed.

## Know what's intentionally rejected

Scenario load fails with `NotImplemented` for: `custom` invariants,
`eventually` mode, target filtering, time windows. Better a loud error
than a test that passes vacuously.
