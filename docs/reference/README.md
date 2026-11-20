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
    "key_presses": [              // synthetic_keyboard needs this, mouse_clicks or mouse_motion
      { "frame": number, "key": "string", "hold_frames": number }
    ],
    "mouse_clicks": [             // optional; scripted clicks, any bot type
      { "frame": number, "x": number, "y": number,
        "button": "left" | "right" | "middle" | "back" | "forward",
        "hold_frames": number }   // default 1
    ],
    "mouse_motion": [             // optional; scripted motion, any bot type
      { "frame": number, "dx": number, "dy": number,
        "repeat_frames": number } // default 1; net deltas per frame
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
  "record_digest": boolean,       // default false; record per-frame broadened state digest
  "invariants": [
    {
      "name": "string",
      "rule": "nodes_in_bounds",
      "min_x": number, "max_x": number,
      "min_y": number, "max_y": number,
      "min_z": number, "max_z": number
    },
    {
      "name": "string",
      "rule": "component_check",     // authored-intent oracle
      "target_selector": { "with_component": { "name": "Camera3d" } },
      "path": "translation.z",      // reflect path to a numeric leaf
      "check": "gt",                // lt | le | gt | ge | equals | ne
      "value": number,               // threshold
      "eventually_s": number        // optional: deadline (seconds)
    }
  ],
  "oracles": ["string"],          // rejected: oracle selection not implemented
  "liveness": {
    "mode": "real_time",          // "off" rejected: not implemented
    "timeout_s": number,          // default 2.0; must span at least one frame
    "progress": {                 // progress-heartbeat liveness oracle
      "mode": "auto",             // "auto" | "markers" | "off" (default auto)
      "markers": ["string"],      // markers mode: named reflected resources/components
      "include_paths": ["string"],// auto mode: extra reflected resources to watch;
                                   //  entries ending in "::" are path prefixes matching
                                   //  ALL resources under that module path
      "exclude": ["string"],      // auto mode: paths to ignore
      "window_s": number,         // default 8.0; verdict horizon — at authored
                                   //  configs must be a whole multiple of sample_s
                                   //  and sample_s a whole number of frames
      "sample_s": number,         // default 0.25; sampling cadence (whole frames)
      "require_surface": false    // empty verifiable surface → Fail (see below)
    }
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
releases after 2 frames (fixed, unlike chaos keys — see below). Clicks
reach the game through real `MouseButtonInput` messages, so
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

### Chaos keyboard (multi-slot holds)

Chaos keys are no longer one-at-a-time. On each input tick (still
governed by `input_rate_hz`) the bot presses one surface key whenever a
hold slot is free, holding up to **2 keys concurrently** and releasing
each after a **per-press drawn duration of 2–30 frames**. This lets the
bot form chords (accelerate + steer) and exercise hold-to-charge
mechanics without knowing what any key means. The draw selects among
surface keys not currently held — a physical user cannot press a key
they are already holding — so every key draw delivers a real press.
Expiry follows a last-holder-releases rule as defense-in-depth (see the
`ChaosHoldState::release` doc comment), but the candidate filter makes
same-key double-holds unreachable with today's draw policy: a key can
never occupy two slots, so `ButtonInput` never sees a premature
release.

Both per-press draws — key index and hold duration — are recorded in
`choices` with their frame and bound, and covered by the replay
fingerprint. Duration draws are tagged `kind: "key_hold_frames"`
(the key-index draw stays `kind: "key"`); the action log derives its
`key_press` entries from key draws only, so a duration draw never
masquerades as a press. Note that `choices` streams are not comparable
across harness versions that change the draw sequence — the added
duration draw (and the candidate-set bound) changes per-frame draw
order by design.

### Rejection at load

Two distinct error classes, at different stages:

- **Unknown fields** (any misspelled or never-implemented knob, including
  once-documented-in-planning names like `persona`, `actuator`, `inputs`,
  `agent_target`, or gamepad/wheel surface knobs) fail deserialization
  immediately with a serde unknown-field error naming the offender.
- **Known-but-unimplemented fields** parse, then reject at validation
  with `NotImplemented`: `after_s`, `before_s`,
  `eventually_s` (except on `component_check`, where it is supported),
  `oracles` selection,
  `liveness.mode: "off"`. Unknown rule values
  (e.g. `custom`) and unknown bot types (e.g. `pursuit`, `replay`) fail
  deserialization naming the offender.
- **Invalid values** reject with specific `Rejected` errors:
  `tps` outside 1..=1000; `duration_s` non-finite, ≤ 0, or > 3600;
  total ticks (`duration_s × tps`) > 10,000,000; simulated-time frame
  duration > 0.250 s (i.e. `tps` < 5 — `Time<Virtual>`'s max_delta
  clamp would silently distort dt); `liveness.timeout_s` non-finite,
  ≤ 0, or shorter than one frame at the scenario's `tps`; authored
  `liveness.progress` mis-sizing (window ≥ `duration_s`, sampling slower
  than one frame, sampling ≥ window, non-finite/negative values,
  `markers` mode with an empty marker list — the DEFAULT progress
  config is exempt from the duration-coupled checks so it can ride
  along short smoke runs); scenario
  `name` outside `[A-Za-z0-9_-]` (it is embedded in report file names —
  path safety); broken bound pairs; typo'd or empty key lists.

## Invariants

- `nodes_in_bounds`: per-entity AABB check on every entity with a
  `Transform` (zero-annotation);
  bounds are per-axis (`min_x`/`max_x`/`min_y`/`max_y`/`min_z`/`max_z`),
  each defaulting
  to ±1000 when unset. A `target_selector:
  {"with_component": {"name": "..."}}` narrows the check to entities
  carrying that named registered component (full type path or an
  unambiguous short path — same resolution rules as
  `component_check`, ambiguity and unknown names reported as
  `oracle_error`); an invariant that never matched a checkable entity
  is `vacuous_invariant`. Additionally, a hard-coded global bounds check
  (±1000 on x/y/z) runs on all entities regardless of invariants or
  tagging — stray geometry will trip it even with zero invariants declared.
- `component_check`: an authored-intent oracle. Selects every entity
  carrying a named registered component
  (`target_selector: {"with_component": {"name": "..."}}` — full
  registered type path, or an unambiguous short path like
  `Camera3d`; short names matching several registered types reject),
  reads a numeric leaf via a reflect `path` (e.g. `"translation.z"`
  — tried on the selector component first, then on the entity's
  `Transform` so identity components can select while the check reads
  position), and compares it against a numeric `value` with `check`
  (`lt`/`le`/`gt`/`ge`/`equals`/`ne`). Without `eventually_s` the
  predicate must hold on every frame where the selector matched
  anything; with `eventually_s: N` it must hold for all selected
  entities simultaneously at least once within N seconds of the
  run start (holding only after N seconds elapsed misses the
  deadline and fails; holding at exactly the deadline boundary
  frame counts). `eventually_s` must be positive, finite, and ≤
  `duration_s` — a deadline past the run's end can never elapse
  and is rejected at load. "Simultaneously" is judged per frame
  against the entities matched AT that frame: satisfaction
  recorded before a new entity spawns is not retroactively
  invalidated by the latecomer, even if it never satisfies.
  Zero
  matches over the whole run → `vacuous_invariant`; an unresolvable
  name/path → `oracle_error` naming the invariant (config errors
  suppress the vacuous/deadline audit entries for that invariant,
  so the report tells one story). This is the rule
  that catches bugs the generic oracles cannot see: a player whose
  forward input never accumulates (position oscillates near spawn)
  keeps moving and stays in bounds, so only the authored expectation
  "holding W drives the camera forward" flags it.

## Oracles

- **NaN/finite transforms** — any non-finite `Transform` → Fail
- **Frozen world** — no movement of any entity over the liveness grace
  window (default 2 s at 60 tps) → Fail (catches soft-locks). With
  `record_digest: true`, a silent streak where the state digest kept
  changing reports "stepping but visually silent" (the sim is running
  while nothing visible moved — presenter layers must write
  Transform/UI to be observable) instead of "soft-lock?"; see
  [Why liveness is time-boxed](../explanation/why-liveness-is-time-boxed.md).
  Phantom
  writes — every frame `Changed<Transform>` fires while the transform
  values stay bit-identical across sampled intervals (a world
  rewriting itself without changing) — defeat the change-streak and are
  caught by a value-hash interval check (30-frame intervals) instead,
  reported once per crossing with a "phantom writes" detail. The
  interval length is a fixed property of the detector — it trades
  detection latency for per-sample cost and is deliberately not
  scenario-configurable. The hash
  covers translation AND rotation AND scale, so orientation/scale
  animation is genuine change, never phantom; a softer "mostly
  phantom writes" variant fires when ≥90% of interval frames (not
  necessarily all) rewrite bit-identical values, resetting only on a
  genuine value change.
- **Progress heartbeat** (`liveness.progress`) — the
  churning-but-dead complement to frozen world: the frame loop stays
  hot and transforms jitter, but no game-verifiable state evolves over
  the window (default 8 s). Sampled every `sample_s` (default 0.25 s);
  a window is stalled when it churned at every sampled frame while no
  watched channel showed a non-ticker change. Resources that change in
  ≥90% of sampled transitions are tickers (heartbeats, frame counters)
  and are suppressed — the stalled detail names them. Entity/archetype
  counts are recorded for breadth but never count as progress
  evidence: spawn churn is engine bookkeeping, not state evolution.
  Channels: auto mode watches reflected component types plus
  `include_paths` resources (minus `exclude`); markers mode watches
  only the named reflected types; `off` disables the oracle entirely.
  Authored configs are held to a coherent sampling grid:
  `sample_s·tps` must be a whole frame count and `window_s` a whole
  multiple of it (the default config rides along unvalidated and is
  re-quantized coherently instead). When nothing
  game-verifiable is in scope, the report gains an informational
  `progress_surface_empty` note, escalating to Fail under
  `strict_warnings: true` or `progress.require_surface: true` (the
  built-in battery scenario sets `require_surface`). Verdicts require
  at least one complete window, so runs shorter than `window_s` get
  samples but no progress verdict. Per-run samples are exported in the
  report's `progress_samples` series.
- **Panic capture** — crash → Crash (in-process)
- **Log capture** — ERROR → Fail, WARN → informational (unless
  `strict_warnings: true`). A log→tracing bridge is installed
  alongside the capture subscriber, so events emitted through the
  `log` crate — bevy's B000x hierarchy validators (B0001/B0004...),
  wgpu/naga, asset loaders, any log-crate user — are captured too
  (they are invisible without a bridge: bevy's validators speak
  `log`, not `tracing`). If the bridge loses the write-once `log`
  logger slot to a game-provided backend, the report gains an
  informational `log_bridge_lost` violation.
- **Log level defense** — while the harness bridge owns the `log`
  logger slot, the `log` max level is harness-owned state for the
  duration of a run: the driver re-asserts it at run start and at
  every tick, overriding any lower level the game set (deliberate
  verbosity curation included — a game that silenced a chatty
  dependency's `log::warn!`s will see them surface as
  informational `log_warn` findings). A one-time foreign
  `log::set_max_level(Off)` thus blinds at most the remainder of the
  tick it lands in; an `Off` re-applied every tick cannot be
  repaired (each tick's tail stays lost) and is detected instead,
  reported as an informational `log_level_tampered` violation whose
  count is the number of tampered ticks.
- **Render liveness** (`render-liveness` feature) — informational,
  never gating: a sim-active run whose game HAS a render sub-app
  (the `render_liveness` report field stays absent otherwise — "not
  evaluated" never means "healthy") but whose cameras never existed,
  were all disabled (`is_active == false`), or had no active camera
  writing output (every sampled active camera sat on
  `CameraOutputMode::Skip`, which may be intentional for
  compositing/prepass setups; a disabled camera's mode is
  irrelevant — it renders nothing) gains an informational
  `render_liveness` violation. STRUCTURAL ONLY: the rule reads
  main-world `Camera` components, never the render world or the
  GPU — a live camera that renders black passes. Evidence (tier,
  `cameras_seen`, `samples`, `sim_active_ever`, notes — collected
  at cadence-gated samples only, not continuously) rides the
  report's `render_liveness` field;
  see the [headless-CI how-to](../how-to/headless-ci-gpu.md) for the
  promise boundary.
- **Screen liveness** (`screen-liveness` feature, implies
  `render-liveness`) — informational, never gating: PIXEL
  observation where `render_liveness` is structural. The harness
  spawns its own marker-tagged `Camera2d` (only when the game itself
  has a 2D camera — a purely-3D view cannot be faithfully mirrored)
  and requests a screenshot every `tps/2` ticks; captured frames are
  box-filtered to an 8x8 luma grid and hashed, consecutive hashes
  compared by Hamming distance. A sim-active run whose captured
  screen never changed across EVERY consecutive capture pair gains
  an informational `screen_liveness` violation ("pixels did not
  flow despite a stepping world"; may be intentional — menus,
  dialogue, level-load art). Honest boundaries, fail-open:
  a run with no 2D game camera at any cadence tick is labeled
  `not_evaluated_3d` (never guessed), and fewer than two usable
  captures (ultra-short run, or GPU readback failure) come out
  `insufficient_captures` — the question is refused, not guessed.
  The rig requires the render sub-app AND an `Assets<Image>` store;
  absence of either means the `screen_liveness` report field stays
  absent — "not evaluated", never "healthy". The 8x8 grid has a
  documented under-sensitivity floor: sub-cell motion (one cell is
  ~40x30 px at the 320x240 capture size) is invisible by design;
  live tiers emit nothing. Evidence (tier, requests, captures,
  `max_hamming`, `sim_active_ever`, notes) rides the report's
  `screen_liveness` field.

## Report

`bevy_playtest::state::PlaytestReport` — written as JSON to
`BEVY_PLAYTEST_REPORT` (default: `<scenario>.report.json`).

- `status`: `Pass | Fail | Crash`
- `violations`: `[{ rule, target, first_frame, last_frame, count, detail,
  last_detail }]` — aggregated by `(rule, target)`; `target` semantics
  vary by rule: the invariant's `name` for scenario bounds checks, the
  entity id for the global bounds oracle, the literal `"world"` for
  `frozen_world`
- `metrics`: `{ input_count, frame_count, worst_frame_ms, crash_detected }`
- `schema_version`, `frame_count`, `tps`, `simulated_duration_s`,
  `wall_time_s`, `error`
- `action_log`: `[{ frame, source, action, details }]` — audited bot
  inputs; `details` on a `key_press` entry names the canonical key the
  draw resolved to (the `action` value indexes the draw-time unheld-
  candidate list, which the report does not contain)
- `choices` (the bot's recorded random draws): `{ frame, value, bound,
  kind, label }` — `kind` separates key-index draws (`key`), per-press
  hold-duration draws (`key_hold_frames`), and mouse draws
  (`mouse_position`, `mouse_button`); `label` (present only on
  key-index draws) names the selected key
- `state_digest`: `[{ frame, hash }]` — per-frame broadened state digests,
  present only when the scenario sets `record_digest: true`; omitted
  (empty) otherwise
- `progress_samples`: `[{ frame, surface_hash, churn, channels }]` —
  the progress-oracle evidence series (frame-indexed, deterministic;
  `surface_hash` is the canonical hash of the whole observable
  surface, `churn` says the frame loop was alive, `channels` counts
  the channels in scope). Non-empty whenever
  `liveness.progress.mode` is not `"off"`.
- `harness_mode`: `"in_process"`
- `scenario_source`: `"file"`, `"builtin_smoke"`, or `"builtin_battery"`
- `log_capture_available` (absent when not applicable; `log_capture_available: false` also produces an
  informational `log_capture_lost` violation — see the
  [rejected-scenario recipe](../how-to/how-to-handle-a-rejected-scenario.md);
  when the capture subscriber wins but the log→tracing bridge cannot
  install, an informational `log_bridge_lost` violation is produced
  instead)
- `log_bridge_available` (absent when not applicable; `false` means
  another `log` crate logger owned the write-once slot, so
  `log`-crate events — bevy's B000x hierarchy validators, wgpu/naga —
  are invisible to `log_warn`/`log_error` even when
  `log_capture_available` is `true`; paired with the informational
  `log_bridge_lost` violation, which is emitted when the capture
  subscriber won and not silenced by the
  `BEVY_PLAYTEST_ALLOW_MISSING_LOG_CAPTURE` escape hatch)
- `render_liveness` (present only with the `render-liveness` feature
  AND a render sub-app in the game; carries `{ cameras_seen,
  sim_active_ever, tier, samples, notes }` — the evidence behind the
  informational `render_liveness` violation; absence means "not
  evaluated", never "healthy")
- `screen_liveness` (present only with the `screen-liveness` feature
  AND a render sub-app AND an image-asset store in the game; carries
  `{ requests, captures, max_hamming, sim_active_ever, tier, notes }`
  — the pixel-observed evidence behind the informational
  `screen_liveness` violation; absence means "not evaluated", never
  "healthy")

The `log_capture_available`/`log_bridge_lost` and
`log_capture_lost` violation entries are the HUMAN-facing layer:
silenced by the `BEVY_PLAYTEST_ALLOW_MISSING_LOG_CAPTURE` escape
hatch (and the bridge entry requires the capture subscriber to have
won, since `log_capture_lost` otherwise already tells the story).
The fields themselves are never silenced — the report always
carries the machine-readable truth.

Report helpers on `PlaytestReport`:

- `violation_rules() -> Vec<&str>` — deduplicated, sorted rule names
- `violation_pairs() -> Vec<(&str, &str)>` — deduplicated, sorted
  (rule, target) pairs
- `has_violation(rule, target) -> bool`

Reports from `lw-reader` builds also carry `lw_reconciled`, whose
entries and findings use the IR specified in
[the SurfaceEntry schema page](surface-entry-schema.md).

## Entry points

- `bevy_playtest::run_scenario(&mut App, &Scenario) ->
  Result<PlaytestReport, ScenarioError>` — in-process (feature `takeover`)
- `bevy_playtest::replay::run_scenario_verified(build: impl Fn() -> App,
  &Scenario) -> Result<PlaytestReport, ScenarioError>` — runs the scenario
  twice on freshly built apps and compares per-frame fingerprints
  (and, when `record_digest: true`, the broadened per-frame state digests)
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
- `run_and_expect_findings(binary, scenario, &[(rule, target,
  detail_needle)])` — golden verdict: panics unless the report
  carries EXACTLY the expected findings (same rule+target, detail
  containing the needle) and nothing else; returns the report for
  additional pins
- `run_and_expect_scoped_findings(binary, scenario, &[])` — the
  stricter sibling: declares EXPECTED findings as scoped
  predicates, not an allowlist. Each `ExpectFinding` (rule, mandatory
  justification, optional target, inclusive first-frame window,
  occurrence count defaulting to 1) must consume exactly one matching
  violation entry whose occurrence `count` equals the declared value:
  any uncovered violation still fails, and an expectation that stops
  matching (game fixed, draw stream shifted) fails the run until the
  declaration is updated — expectations cannot silently rot. Scope
  tightly: e.g. a menu-idle `frozen_world` fires at
  onset + liveness-window frames, so a window around that fire-frame
  admits only startup-phase freezes and leaves mid-game wedges red.
  Differences from `run_and_expect_clean`: informational findings
  (warnings, capture notes) also fail unless covered by an
  expectation, and a `crash` status fails the run outright
- `run_with_config(&BatteryConfig)` — explicit config:
  `BatteryConfig { binary, scenario, timeout_secs }`
- `BatteryError` — infrastructure failures only: `Spawn`, `Timeout`,
  `NoReport` (game exited before reporting), `ReportParse`, and
  `GpuMissing` (bevy_render found no GPU adapter — headless-CI
  hosts without a software rasterizer; see
  `docs/how-to/headless-ci-gpu.md`). The enum is
  `#[non_exhaustive]`: new infrastructure failure modes may be
  added in any release, so match with a catch-all arm

## Plugins

- `PlaytestPlugin` — inert unless `BEVY_PLAYTEST_SCENARIO` is set;
  performs the takeover when it is (feature `takeover`)
- `PlaytestDefaultPlugins` — pass-through of `bevy::DefaultPlugins` that
  swaps winit's runner for a schedule loop under the takeover env vars
  (feature `takeover-winit`)

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
| `BEVY_PLAYTEST_ALLOW_MISSING_LOG_CAPTURE` | Acknowledge a lost log subscriber instead of failing |

Unknown `BEVY_PLAYTEST_*` variables abort the game process with exit
code 3 — a typo'd setting must never be mistaken for a configured option.

## Exit codes

The takeover runner exits the game process with:
`0` Pass, `1` Fail, `2` Crash, `3` config error (bad scenario or env).

## Known-answer fixture

[`fixtures/known_answer/`](known-answer-fixture.md) — the planted-truth
fixture: hand-authored input surface, committed ground truth, and the
measurement/mutation harnesses that grade the declared tier against it.

## Thetawave fixture

[`fixtures/thetawave/`](thetawave-fixture.md) — the real-world fixture:
a ported third-party game (Theta Wave, MIT) proving the declared
tier against actual Leafwing usage, with an honest
NOTICE covering attribution and stand-in assets.

## Deterministic boot

[deterministic-boot.md](deterministic-boot.md) — the "why + how to
fix" target for `replay_divergence` and `Statistical`-surface
messages: the read-your-seed-from-the-environment pattern, what
unseeded game RNG does to confidence, and the degradation classes
(persisted external state, network sockets) as reported categories.

## Compatibility

Targets Bevy 0.20.
