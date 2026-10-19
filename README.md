# bevy_playtest

A playtesting harness for Bevy games. Scenarios drive a deterministic bot
through your game while oracles check invariants and produce a machine-readable
report (JSON + JUnit XML).

## v0.1 scope (bare minimum) — FINAL

v0.1 is feature-frozen. The surface below is the release contract; anything
outside it is rejected loudly (never a vacuous pass) and deferred to v0.2+.

**Supported:**
- In-process mode (Bevy 0.20, bevy_ecs/bevy_app/bevy_transform/bevy_input only)
- External-BRP mode (zero-contract, subprocess + Remote Protocol)
- Bots: `chaos` (random keys, scenario-seeded, choices recorded), `synthetic_keyboard` (scripted key schedule)
- Invariant: `nodes_in_bounds` (per-axis min/max on `Gameplay`-tagged entities)
- Oracle: NaN/finite transforms, liveness (`liveness.timeout_s` grace window)
- Panic and `tracing` log capture (panics → Crash, ERROR logs → Fail, WARN → informational)
- Reset hooks / cheat hooks (typed registry, unknown kinds rejected at load)
- Reports: `PlaytestReport` (serde) + JUnit XML

**Explicitly rejected at scenario load (never a vacuous pass):**
- `custom` invariants, `eventually` mode, target filtering, time windows — `NotImplemented` error
- Unknown reset/cheat kinds, malformed scenarios

**Known limitations (accepted for v0.1):**
- Single Bevy version (0.20). Multi-version support was evaluated and deferred —
  see "Version policy" below.
- External-BRP mode: chaos bot only; no reset/cheat hooks (need in-process or a
  game-side BRP method).

### Version policy

v0.1 targets **Bevy 0.20 only**. Multi-version support (0.19/0.20 in one source
tree via build-time cfgs) was prototyped and **deliberately not adopted**:
- No observed demand — every dogfooded game is on 0.20
- Ecosystem convention is one release per Bevy minor (checked: bevy_asset_loader,
  bevy_kira_audio, bevy_common_assets, bevy_inspector_egui, bevy_rapier2d,
  bevy_ecs_tilemap, leafwing-input-manager — none do multi-version-in-one-crate)
- Healthy plugins ship compatible releases within 1–2 days of a Bevy bump
- External-BRP mode already covers unmodifiable/older games without any cfg hacks

If real demand for an older Bevy appears, the plan is branch-per-version (the
ecosystem standard), not a cfg shim. The monotonic-threshold `build.rs` design
is retained as a reference for a future fast-follow.

## Integrating a game

### Tier-2 Observable (v0.1 default)

Tag gameplay-relevant entities with `bevy_playtest::contract::Gameplay`. That's it.

```rust
use bevy::prelude::*;

fn tag_gameplay(mut commands: Commands, ball: Query<Entity, With<Ball>>) {
    for e in ball.iter() {
        commands.entity(e).insert(bevy_playtest::contract::Gameplay);
    }
}
```

**Two practical requirements for in-process runs:**

1. **Extract app construction from `main()`** — Bevy apps must construct an `App`
   somewhere accessible to the harness. If your game wires everything inline in
   `main()`, extract it to a `build_app()` function (standard Bevy testing pattern).

2. **Disable `WinitPlugin`** — the harness drives input itself; winit's event loop
   conflicts with test runners. Use:
   ```rust
   app.add_plugins(DefaultPlugins.build().disable::<bevy::winit::WinitPlugin>());
   ```
   Render/audio/assets still initialize; the game runs headless.

### Zero-contract external-BRP (v0.1, experimental)

Run the game as a subprocess and drive it over Bevy's Remote Protocol — no
harness imports in the game at all. Requires the game binary to:

1. Add `RemotePlugin::default()` + `RemoteHttpPlugin::default().with_port(port)`
2. Register the types the harness inspects:
   ```rust
   .register_type::<Transform>()
   .register_type::<YourGameplayMarker>() // or bevy_playtest::contract::Gameplay
   ```
3. Read the port from `BRP_PORT` (the runner sets it)

Then from the test side:
```rust
use bevy_playtest::brp_runner::{run_scenario_brp, BrpRunConfig};

let cfg = BrpRunConfig {
    game_binary: env!("CARGO_BIN_EXE_my_game").to_string(),
    port: Some(4570),
    ..Default::default()
};
let report = run_scenario_brp(&cfg, &scenario)?;
```

Reports carry `harness_mode: "external_brp"`. Same invariants/oracles as
in-process. v0.1 limits: chaos bot only, no reset/cheat hooks (those need
in-process or a game-side BRP method).

## Examples

- `bevy_playtest_test/examples/pong/` — minimal headless pong, designed for harness
- `bevy_playtest_test/examples/breakout/` — upstream Bevy example, third-party integration
- `bevy_playtest_test/src/bin/pong_brp.rs` — BRP-enabled pong for external mode

v0.2 candidates (not yet scoped): custom invariants via TestApi, replay bot,
fingerprints, multi-Bevy-version support.
