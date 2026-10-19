# Reference

Public API surface for integrators.

## Scenario (JSON schema)

```json
{
  "name": "string",
  "bot": {
    "type": "chaos" | "synthetic_keyboard",
    "seed": number,               // required for chaos
    "schedule": [                 // required for synthetic_keyboard
      { "at_s": number, "key": "string", "release_at_s": number? }
    ]
  },
  "duration_s": number,
  "invariants": [
    { "kind": "nodes_in_bounds", "max": [number, number] }
  ],
  "setup": {
    "reset": { "kind": "string" },
    "cheat": { "kind": "string" }
  }
}
```

Rejected at load: `custom` invariants, `eventually` mode, target filters,
time windows — `NotImplemented` error.

## Invariants

- `nodes_in_bounds`: per-entity AABB check on `Gameplay`-tagged entities;
  max is `[width, height]` in game-space coordinates.

## Oracles

- **NaN/finite transforms** — any non-finite `Transform` → Fail
- **Frozen world** — no movement of tagged entities over the liveness
  grace window
- **Panic capture** — crash → Crash
- **Log capture** — ERROR → Fail, WARN → informational

## Report types

- `PlaytestReport`: `status: PlaytestStatus`, `violations: Vec<Violation>`
- `PlaytestStatus`: `Pass | Fail | Crash`
- `Violation`: `kind`, `tick`, `entities: Vec<Entity>`, optional `message`

## Entry points

- `bevy_playtest::run_scenario(&mut App, &Scenario) -> Result<PlaytestReport, Error>`
- `bevy_playtest::brp_runner::run_scenario_brp(&BrpRunConfig, &Scenario) -> Result<PlaytestReport, Error>`
- `bevy_playtest::junit::to_junit_xml(&PlaytestReport) -> String`

## Hooks

Reset/cheat hooks are registered via the harness's typed registry; unknown
kinds are rejected at scenario load.

## Compatibility

Targets Bevy 0.20. Older Bevy versions are reached through external-BRP
mode rather than multi-version compilation.
