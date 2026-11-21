# API: entry points, battery, plugins

The public API surface for integrators: how runs are launched, the
subprocess battery used by `cargo test`, the two plugins, and the
takeover runner's exit codes.

## Entry points

- `bevy_playtest::run_scenario(&mut App, &Scenario) ->
  Result<PlaytestReport, ScenarioError>` — in-process (the `in-process`
  feature; `takeover` adds the plugins, battery, and subprocess
  takeover)
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
  carries exactly the expected findings (same rule+target, detail
  containing the needle) and nothing else; returns the report for
  additional pins
- `run_and_expect_scoped_findings(binary, scenario, &[])` — the
  stricter sibling: declares expected findings as scoped
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
  `BatteryConfig { binary, scenario, timeout_secs, seed_override }`
  (`seed_override` reproduces a sweep member exactly; see the
  sweep how-to)
- `BatteryError` — infrastructure failures only: `Spawn`, `Timeout`,
  `NoReport` (game exited before reporting), `ReportParse`,
  `GpuMissing` (bevy_render found no GPU adapter — headless-CI
  hosts without a software rasterizer; see
  [the headless-CI how-to](../how-to/headless-ci-gpu.md)), and
  `Sweep` (sweep/regression-db orchestration failures). The enum is
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

## Exit codes

The takeover runner exits the game process with:
`0` Pass, `1` Fail, `2` Crash, `3` config error (bad scenario or env).
