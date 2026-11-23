# Environment variables

All `BEVY_PLAYTEST_*` variables. In a battery run there are two
sides: the **battery parent** (the `cargo test` process) and the
**game child** (under `BEVY_PLAYTEST_SCENARIO`). The Side column
says who reads the variable.

| Variable | Side | Effect |
|---|---|---|
| `BEVY_PLAYTEST_SCENARIO` | child | Scenario path, or the literals `"smoke"` / `"battery"`. Absent: `PlaytestPlugin` inert |
| `BEVY_PLAYTEST_REPORT` | child | Report output path (default `<scenario>.report.json`) |
| `BEVY_PLAYTEST_HEADLESS` | child | Assert headlessness without running a scenario: no window, no display needed |
| `BEVY_PLAYTEST_REPLAY_PASS_B_SERIES` | child | Internal: replay pass-B fingerprint-series path (set by the parent of a `verify_replay` subprocess pass) |
| `BEVY_PLAYTEST_REPLAY_PASS_B_DIGEST` | child | Internal: replay pass-B state-digest-series path (set on the pass-B child; compared only when the scenario records digests) |
| `BEVY_PLAYTEST_ALLOW_MISSING_LOG_CAPTURE` | child | Acknowledge a lost log subscriber instead of failing |
| `BEVY_PLAYTEST_SEED` | child | Override the loaded scenario's `bot.seed` (after loading — composes with scan-layered variants). Reproduce any single battery or sweep member by hand: pair with `BEVY_PLAYTEST_SCENARIO` (the seed var alone leaves the takeover inert) |
| `BEVY_PLAYTEST_DISCOVER_SCAN` | child (set by parent) | Internal: static input-surface scan export path (set by the probe-pool substrate; also listed as known so an inherited value is never rejected) |
| `BEVY_PLAYTEST_DETRNG_SEED` | child (shim reads it) | Internal: deterministic-randomness shim seed; the battery parent sets it on the child from the scenario seed |
| `BEVY_PLAYTEST_DETRNG_DISABLE` | parent | Internal: the battery parent reads it to skip preloading the shim on Linux |
| `BEVY_PLAYTEST_SOURCE_ROOT` | parent | Point the source scan at a non-standard source root (never passed to the game child) |
| `BEVY_PLAYTEST_SOURCE_SCAN_DISABLE` | parent | Restore the default WASD+arrows chaos surface (see [the README](../../README.md#aiming-the-bot-at-the-games-real-input-surface)) |
| `BEVY_PLAYTEST_FIXTURE` | both | Internal: test-selection var (the acceptance suite picks one fixture); the child inherits it harmlessly |
| `BEVY_PLAYTEST_LINT` | child | Lint the scenario instead of running it: with `BEVY_PLAYTEST_SCENARIO` also set, every config error is collected and reported at once (instead of one per full run), plus a type-registry probe of the booted App (one warm-up frame) catching definite errors like typo'd resource names early; then the process exits. The probe's warm-up frame runs ONLY when the scenario contains a probeable rule (`component_check`, `nodes_in_bounds` with a `with_component` selector, `resource_check`, `stagnant_resource`) AND stage 1 found nothing — a guaranteed-empty probe skips the game boot entirely, and a game that panics on boot reports a Crash (exit 2). Clean lint = "config is coherent", never "the game will pass". Alone (no scenario) it is inert |

Unknown `BEVY_PLAYTEST_*` variables abort the game process with exit
code 3 — a typo'd setting must never be mistaken for a configured option.
