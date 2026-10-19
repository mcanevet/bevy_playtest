# Reference

Public API surface for integrators.

## Scenario (JSON schema)

```json
{
  "name": "string",
  "bot": {
    "type": "chaos" | "synthetic_keyboard",
    "seed": number,               // default 42
    "input_rate_hz": number,      // default 10
    "key_presses": [              // required for synthetic_keyboard
      { "frame": number, "key": "string", "hold_frames": number }
    ]
  },
  "tps": number,                  // default 60
  "simulated_time": boolean,      // default true
  "single_threaded": boolean,     // default true
  "strict_warnings": boolean,     // default false
  "duration_s": number,           // default 15.0
  "invariants": [
    {
      "name": "string",
      "rule": "nodes_in_bounds" | "custom",
      "min_x": number,
      "max_x": number,
      "min_y": number,
      "max_y": number,
      "targets": ["string"]       // empty = all Gameplay entities
    }
  ],
  "setup": {
    "resets": [{ "kind": "string" }],
    "cheats": [{ "frame": number, "kind": "string" }]
  },
  "oracles": ["string"],          // rejected: oracle selection not implemented
  "liveness": {
    "mode": "real_time",          // "off" rejected: not implemented
    "timeout_s": number           // default 2.0
  }
}
```

Rejected at load: `custom` invariants, `eventually` mode, target filters,
time windows — `NotImplemented` error. Invalid values (non-finite
durations, broken bounds, typo'd key names) are rejected with specific
errors.

## Invariants

- `nodes_in_bounds`: per-entity AABB check on `Gameplay`-tagged entities;
  bounds are per-axis (`min_x`/`max_x`/`min_y`/`max_y`), each defaulting
  to ±1000 when unset. Additionally, a hard-coded global bounds check
  (±1000 on x/y/z) runs on all entities regardless of invariants or
  tagging — stray geometry will trip it even with zero invariants declared.

## Oracles

- **NaN/finite transforms** — any non-finite `Transform` → Fail
- **Frozen world** — no movement of any entity over the liveness grace
  window (default 2 s at 60 tps) → Fail
- **Panic capture** — crash → Crash
- **Log capture** — ERROR → Fail, WARN → informational (unless
  `strict_warnings: true`)

## Report types

- `PlaytestReport`: `status: PlaytestStatus`, `violations: Vec<ViolationEntry>`
- `PlaytestStatus`: `Pass | Fail | Crash`
- `ViolationEntry`: `rule`, `target`, `first_frame`, `last_frame`, `count`,
  `detail`

## Entry points

- `bevy_playtest::run_scenario(&mut App, &Scenario) -> Result<PlaytestReport, ScenarioError>`
- `bevy_playtest::brp_runner::run_scenario_brp(&BrpRunConfig, &Scenario) -> Result<PlaytestReport, ScenarioError>`
- `bevy_playtest::junit::reports_to_junit(&[(String, PlaytestReport)], &str) -> String`

## Hooks

Reset/cheat hooks are registered via the harness's typed registry in
`bevy_playtest::contract`:

```rust
use bevy_playtest::contract::ResetHooks;

app.world_mut().insert_resource(ResetHooks::default());
app.world_mut()
    .resource_mut::<ResetHooks>()
    .register("name", |world| { /* ... */ });
```

Unknown kinds are rejected at scenario load.

## Compatibility

Targets Bevy 0.20. Older Bevy versions are reached through external-BRP
mode rather than multi-version compilation.
