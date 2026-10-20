# Changelog

## [Unreleased]

### Added
- Default features now include `takeover` for zero-config integration.
- Replay-divergence verification (`verify_replay: true`) proves determinism
  via two-pass fingerprint comparison.
- Cheat postcondition contracts (`expects`) catch silently no-op hooks.
- Behavioral coverage (`observes` + `strict_coverage`) reports unobserved
  events and unexercised chaos-surface keys.
- `docs/roadmap.md` consolidates deferrals with reasons and triggers.

### Changed
- Window suppression spike succeeded: `DefaultPlugins` + `PlaytestPlugin`
  runs headless without `PlaytestDefaultPlugins` (plugin order matters).
- `bevy_playtest run` now auto-discovers `playtest.scenario.json` or
  `playtests/*.json` when no positional argument is given.

### Fixed
- N/A

