# Changelog

## [Unreleased]

## [0.6.0](https://github.com/mcanevet/bevy_playtest/compare/v0.5.0...v0.6.0) - 2026-11-01

### Added

- emit wrapped KeyboardInput events with populated logical_key/text
- teach frozen_world oracle about UI activity (ui-liveness)
- [**breaking**] remove external-BRP mode
- emit wrapped WindowEvent messages for pointer-driven games

### Fixed

- deliver just_pressed under DefaultPlugins takeover
- *(ci)* derive Bevy fixture rev from Cargo.lock

### Other

- refresh fixtures Cargo.lock to Bevy 0.20.1
- key target cache on fetch-script hash

## [0.5.0](https://github.com/mcanevet/bevy_playtest/compare/v0.4.0...v0.5.0) - 2026-10-31

### Other

- fix review findings on battery-first restructure
- lead with the zero-config battery in README and tutorial
- ci Update rust dependencies to 0.20.1
- expand runner disk-space cleanup
- [**breaking**] remove reset/cheat hooks, observes, and strict_coverage ([#44](https://github.com/mcanevet/bevy_playtest/pull/44))
- ci Update release-plz/action action to v0.5.147

## [0.4.0](https://github.com/mcanevet/bevy_playtest/compare/v0.3.0...v0.4.0) - 2026-10-26

### Added

- chaos mouse bot + scripted mouse_clicks

### Other

- ci Update release-plz/action action to v0.5.146
- restructure docs/ into diátaxis framework

## [0.3.0](https://github.com/mcanevet/bevy_playtest/compare/v0.2.0...v0.3.0) - 2026-10-23

### Added

- close the log_error oracle gap — cross-thread capture with honest loss reporting

### Fixed

- *(fetch)* fetch fixture runtime assets to silence bevy_asset Path-not-found
- avoid rustdoc private-intra-doc-link warning in try_acquire_run docs
- mock-game source write raced rustc in brp_robustness

### Other

- single-job PR gate + nightly, mirror workaround, build-once-test-many
- ci Update release-plz/action action to v0.5.145
- [**breaking**] remove contract::Gameplay marker — zero-annotation invariants
- drop --locked from fixtures workspace commands

## [0.2.0](https://github.com/mcanevet/bevy_playtest/compare/v0.1.0...v0.2.0) - 2026-10-23

### Added

- [**breaking**] run playtests via plain `cargo test` (self-dev-dependency pattern)

### Other

- ci Update taiki-e/install-action digest to a4d3f3f
- fix unit job — skip acceptance test (needs fixture binaries) ([#21](https://github.com/mcanevet/bevy_playtest/pull/21))
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
