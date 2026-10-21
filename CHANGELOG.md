# Changelog

## [Unreleased]

## [0.1.1](https://github.com/mcanevet/bevy_playtest/compare/v0.1.0...v0.1.1) - 2026-10-21

### Other

- fix release-plz workflow (add Linux deps + pin toolchain)
- minimize wall-clock via parallel jobs + binary artifacts
- ci Update actions/checkout action to v7
- add release-plz workflow and config

### Added
- In-process playtest takeover: `BEVY_PLAYTEST_SCENARIO` turns the game
  binary into its own headless runner (exit codes 0 Pass / 1 Fail / 2
  Crash / 3 config error).
- `PlaytestDefaultPlugins`: pass-through `DefaultPlugins` that swaps to a
  headless runner under `BEVY_PLAYTEST_*` env vars.
- Deterministic scenario schema with strict validation
  (`deny_unknown_fields`, did-you-mean suggestions, loud rejection of
  unsupported fields).
- Oracles: frozen-world liveness, NaN transforms, bounds invariants,
  panic/log capture, frame-timing anomaly counting.
- Replay-divergence verification (`verify_replay: true`) proves determinism
  via two-pass fingerprint comparison.
- Cheat postcondition contracts (`expects`) catch silently no-op hooks.
- Behavioral coverage (`observes` + `strict_coverage`) reports unobserved
  events and unexercised chaos-surface keys.
- External-BRP tier: drive a game binary over Bevy Remote Protocol with a
  reduced, parity-labeled oracle set; `BRP_PORT` auto-enables it.
- CLI: `init`, `run` (with auto-discovery), `doctor`, JUnit XML output.
- `docs/` (Diátaxis): tutorial, how-to, reference, explanation, roadmap.
