# Call `run_scenario` directly (in-process, no child)

When you want the `PlaytestReport` object in your own test — to inspect
violations, pin known findings, or compose runs:

```rust
use bevy_playtest::scenario::Scenario;

#[test]
fn smoke() {
    let scenario: Scenario = serde_json::from_str(SCENARIO_JSON).unwrap();
    let mut app = App::new();
    app.add_plugins(bevy::MinimalPlugins);
    // ... input/time plugins and your game systems ...
    app.add_plugins(bevy_playtest::PlaytestPlugin);
    let report = bevy_playtest::run_scenario(&mut app, &scenario).unwrap();
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
}
```

Two traps to avoid:

- **Don't use `PlaytestDefaultPlugins` here.** Outside a takeover run
  (no `BEVY_PLAYTEST_*` env vars set) it is a pure pass-through to
  `bevy::DefaultPlugins` — including `WinitPlugin`, which owns an OS
  event loop that conflicts with a test runner driving ticks, and which
  panics on hosts without a display. Build the app with
  `MinimalPlugins` (plus whatever your systems need) instead, or see
  [why winit is disabled](../explanation/why-disable-winit.md).
- **`run_scenario` is exclusive per process** — a second concurrent
  call is rejected, not queued. Run such tests serially with
  `--test-threads=1`.

The alternative for full-fidelity runs against a real game binary is
[`battery::run`](../reference/README.md#battery), which drives the
compiled game as a subprocess.
