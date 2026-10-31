# Reference

Public API surface for integrators.

## Scenario (JSON schema)

```json
{
  "name": "string",              // [A-Za-z0-9_-] only (used in report file names)
  "bot": {
    "type": "chaos" | "synthetic_keyboard",
    "seed": number,               // default 42
    "input_rate_hz": number,      // default 10
    "key_presses": [              // synthetic_keyboard needs this or mouse_clicks
      { "frame": number, "key": "string", "hold_frames": number }
    ],
    "mouse_clicks": [             // optional; scripted clicks, any bot type
      { "frame": number, "x": number, "y": number,
        "button": "left" | "right" | "middle" | "back" | "forward",
        "hold_frames": number }   // default 1
    ],
    "raw_surface": {              // optional, chaos only
      "keys": ["KeyW", "ArrowLeft"],
      "mouse_buttons": ["Left", "Right"]   // default: all three primary buttons
    }
  },
  "tps": number,                  // default 60; range 1..=1000
  "simulated_time": boolean,      // default true
  "single_threaded": boolean,     // default true
  "strict_warnings": boolean,     // default false
  "duration_s": number,           // default 15.0; max 3600
  "verify_replay": boolean,       // default false; in-process only, needs single_threaded
  "invariants": [
    {
      "name": "string",
      "rule": "nodes_in_bounds",
      "min_x": number, "max_x": number,
      "min_y": number, "max_y": number,
      "min_z": number, "max_z": number,
      "target_selector": "all"   // optional; only "all" exists
    }
  ],
  "oracles": ["string"],          // rejected: oracle selection not implemented
  "liveness": {
    "mode": "real_time",          // "off" rejected: not implemented
    "timeout_s": number           // default 2.0; must span at least one frame
  }
}
```

Key names accept `KeyCode` spellings (`ArrowLeft`, `KeyW`) or lowercase
shortcuts (`left`, `w`); unknown names are rejected at load with a
did-you-mean suggestion. Mouse buttons accept the same tolerance
(`Left`, `left`); valid buttons are `left`, `right`, `middle`, `back`,
`forward`.

### Chaos mouse (default on)

The chaos bot clicks as well as types — no scenario changes required.
On each input tick it picks a random surface button, moves a harness
owned cursor to a random quantized position (a single draw, 16 bits
per axis, scaled into a 1280×1280 theoretical viewport), presses, and
releases after 2 frames (same scheduling as chaos keys). Clicks reach
the game through real `MouseButtonInput` messages, so
`ButtonInput<MouseButton>` and message-based input handlers both see
them; scenario seeds replay identically.

Opt out with an explicit empty surface:
`"raw_surface": { "keys": [...], "mouse_buttons": [] }`. Symmetrically,
`"keys": []` opts out of chaos keys (mouse-only chaos); omitting a
field keeps its default. A fully-empty surface rejects at load.

Known limitation (honesty first): the click position lives in a
harness-owned resource (`driver::CursorInputResource`), not the game's
`Window.cursor_position` — UI systems that hit-test against the window
cursor (e.g. `bevy_ui` hover) will not see chaos clicks land where the
bot placed them. Button state itself is fully exercised.

### Rejection at load

Two distinct error classes, at different stages:

- **Unknown fields** (any misspelled or never-implemented knob, including
  once-documented-in-planning names like `persona`, `actuator`, `inputs`,
  `agent_target`, or gamepad/wheel surface knobs) fail deserialization
  immediately with a serde unknown-field error naming the offender.
- **Known-but-unimplemented fields** parse, then reject at validation
  with `NotImplemented`: `eventually_s`, `after_s`, `before_s`,
  invariant `value`/`path`/`check` selectors, `oracles` selection,
  `liveness.mode: "off"`. Unknown rule values
  (e.g. `custom`) and unknown bot types (e.g. `pursuit`, `replay`) fail
  deserialization naming the offender.
- **Invalid values** reject with specific `Rejected` errors:
  `tps` outside 1..=1000; `duration_s` non-finite, ≤ 0, or > 3600;
  total ticks (`duration_s × tps`) > 10,000,000; simulated-time frame
  duration > 0.250 s (i.e. `tps` < 5 — `Time<Virtual>`'s max_delta
  clamp would silently distort dt); `liveness.timeout_s` non-finite,
  ≤ 0, or shorter than one frame at the scenario's `tps`; scenario
  `name` outside `[A-Za-z0-9_-]` (it is embedded in report file names —
  path safety); broken bound pairs; typo'd or empty key lists.

## Invariants

- `nodes_in_bounds`: per-entity AABB check on every entity with a
  `Transform` (zero-annotation);
  bounds are per-axis (`min_x`/`max_x`/`min_y`/`max_y`/`min_z`/`max_z`),
  each defaulting
  to ±1000 when unset. Additionally, a hard-coded global bounds check
  (±1000 on x/y/z) runs on all entities regardless of invariants or
  tagging — stray geometry will trip it even with zero invariants declared.

## Oracles

- **NaN/finite transforms** — any non-finite `Transform` → Fail
- **Frozen world** — no movement of any entity over the liveness grace
  window (default 2 s at 60 tps) → Fail. Also runs in external-BRP mode
  (transform positions polled over BRP); panic/log capture there require
  `PlaytestBrpPlugin` or mode 1.
- **Panic capture** — crash → Crash (in-process, or via
  `bevy_playtest/health` under `PlaytestBrpPlugin`)
- **Log capture** — ERROR → Fail, WARN → informational (unless
  `strict_warnings: true`)

## Report

`bevy_playtest::state::PlaytestReport` — written as JSON to
`BEVY_PLAYTEST_REPORT` (default: `<scenario>.report.json`).

- `status`: `Pass | Fail | Crash`
- `violations`: `[{ rule, target, first_frame, last_frame, count, detail,
  last_detail }]` — aggregated by `(rule, target)`; `target` semantics
  vary by rule: the invariant's `name` for scenario bounds checks, the
  entity id for the global bounds oracle, the literal `"world"` for
  `frozen_world`, `"brp_query"` for BRP transport errors
- `metrics`: `{ input_count, frame_count, worst_frame_ms, crash_detected }`
- `schema_version`, `frame_count`, `tps`, `simulated_duration_s`,
  `wall_time_s`, `error`
- `action_log`: `[{ frame, source, action, details }]` — audited bot
  inputs
- `choices` (the bot's recorded random draws)
- `harness_mode`: `"takeover"` or `"external_brp"`
- `scenario_source`: `"file"`, `"builtin_smoke"`, or `"builtin_battery"`
- `log_capture_available` (absent when not applicable; `log_capture_available: false` also produces an
  informational `log_capture_lost` violation — see the
  [rejected-scenario recipe](../how-to/how-to-handle-a-rejected-scenario.md))
- `brp_parity`: `"full"` | `"degraded"` (external-BRP runs only)

Report helpers on `PlaytestReport`:

- `violation_rules() -> Vec<&str>` — deduplicated, sorted rule names
- `violation_pairs() -> Vec<(&str, &str)>` — deduplicated, sorted
  (rule, target) pairs
- `has_violation(rule, target) -> bool`

## Entry points

- `bevy_playtest::run_scenario(&mut App, &Scenario) ->
  Result<PlaytestReport, ScenarioError>` — in-process (feature `takeover`)
- `bevy_playtest::replay::run_scenario_verified(build: impl Fn() -> App,
  &Scenario) -> Result<PlaytestReport, ScenarioError>` — runs the scenario
  twice on freshly built apps and compares per-frame fingerprints
- `bevy_playtest::brp_runner::run_scenario_brp(&BrpRunConfig, &Scenario)
  -> Result<PlaytestReport, ScenarioError>` — external-BRP (feature `brp`;
  not a default feature)
- `bevy_playtest::junit::reports_to_junit(&[(String, PlaytestReport)],
  &str) -> String`

## Battery

`bevy_playtest::battery` (feature `takeover`) — subprocess orchestration
for `cargo test` integration:

- `run(binary, scenario) -> Result<PlaytestReport, BatteryError>` —
  returns the report even for failing runs (findings are content, not
  infrastructure errors); wall-clock timeout 300 s
- `run_and_expect_clean(binary, scenario)` — panics with a
  per-violation findings summary if anything gates; `scenario` is a path
  or the literal `BUILTIN_BATTERY` (`"battery"`, the zero-config verdict
  run) — `"smoke"` is the equivalent built-in minimal scenario
- `run_with_config(&BatteryConfig)` — explicit config:
  `BatteryConfig { binary, scenario, timeout_secs }`
- `BatteryError` — infrastructure failures only: `Spawn`, `Timeout`,
  `NoReport` (game exited before reporting), `ReportParse`

## Plugins

- `PlaytestPlugin` — inert unless `BEVY_PLAYTEST_SCENARIO` is set;
  performs the takeover when it is (feature `takeover`)
- `PlaytestDefaultPlugins` — pass-through of `bevy::DefaultPlugins` that
  swaps winit's runner for a schedule loop under the takeover env vars
  (feature `takeover-winit`)
- `PlaytestBrpPlugin` — restores mode-1 capabilities to external-BRP
  mode via `bevy_playtest/health` and `/time` BRP methods (feature `brp-plugin`)

Plugin order: add `PlaytestPlugin` **after** any plugin group that sets
a runner — the runner swap is last-set-wins, and a wrong order silently
disables the takeover.

## Environment variables

| Variable | Effect |
|---|---|
| `BEVY_PLAYTEST_SCENARIO` | Scenario path, or the literals `"smoke"` / `"battery"`. Absent: `PlaytestPlugin` inert |
| `BEVY_PLAYTEST_REPORT` | Report output path (default `<scenario>.report.json`) |
| `BEVY_PLAYTEST_HEADLESS` | Assert headlessness without running a scenario: no window, no display needed |
| `BEVY_PLAYTEST_REPLAY_PASS_B` | Internal: replay pass-B marker |
| `BRP_PORT` | External-BRP port; set for the game subprocess by the harness |
| `BEVY_PLAYTEST_ALLOW_MISSING_LOG_CAPTURE` | Acknowledge a lost log subscriber instead of failing |

Unknown `BEVY_PLAYTEST_*` variables abort the game process with exit
code 3 — a typo'd setting must never be mistaken for a configured option.

## Exit codes

The takeover runner exits the game process with:
`0` Pass, `1` Fail, `2` Crash, `3` config error (bad scenario or env).

## Compatibility

Targets Bevy 0.20. Older Bevy versions are reached through external-BRP
mode rather than multi-version compilation.
