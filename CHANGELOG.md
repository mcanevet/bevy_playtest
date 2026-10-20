# Changelog

## [Unreleased]

### Changed (round 6)
- PlaytestPlugin now auto-enables external-BRP mode when `BRP_PORT` is
  set (adds RemotePlugin + type registrations); invalid port exits 3,
  and a build without the `brp-plugin` feature fails loudly with a
  rebuild hint.
- Fixture generation collapsed to a one-line insertion transform
  (`.add_plugins(PlaytestPlugin)` before `.run()`); `fixture-runner`
  deleted — the in-process tier is now `BEVY_PLAYTEST_SCENARIO=…
  cargo run --bin fixture_X`.
- Removed the phantom headless watchdog and stale winit hard-fail docs;
  the actual mechanism (runner last-set-wins) and the plugin-ordering
  responsibility are now documented honestly.

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

