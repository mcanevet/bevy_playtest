# bevy_playtest

A playtesting harness for Bevy games. Scenarios drive a deterministic bot
through your game while oracles check invariants and produce a machine-readable
report (JSON + JUnit XML).

**Documentation:** [tutorial](docs/tutorial.md) · [how-to guides](docs/how-to.md) ·
[reference](docs/reference.md) · [explanation](docs/explanation.md)

## Integrating a game

### Zero-annotation happy path (rivals' bar, met)

No marker types, no extracted `build_app()`, no hooks — one plugin line plus
the CLI:

```rust
app.add_plugins(bevy_playtest::PlaytestDefaultPlugins);
app.add_plugins(bevy_playtest::PlaytestPlugin); // added AFTER DefaultPlugins
```

```console
$ bevy_playtest init                      # writes playtest.scenario.json
$ $EDITOR playtest.scenario.json           # set "target_selector": "all" on invariants
$ bevy_playtest run playtest.scenario.json
```

With `"target_selector": "all"`, bounds invariants check every entity
with a `Transform` — zero bevy_playtest-specific code in the game.
(The default selector `gameplay` checks only `contract::Gameplay`-
marked entities and **fails loudly (not vacuously)** when it matches
zero entities, suggesting the `all` selector.)

Even faster, no file at all: `bevy_playtest run smoke`. Or omit the
argument entirely — the CLI auto-discovers `playtest.scenario.json` or
the first JSON in `playtests/` (alphabetically). Multiple matches
produce an error with a suggestion to specify one explicitly.

Integration cost story vs. rivals: the plugin line is the only required
change (rival parity); markers/hooks/`build_app()` extraction are
opt-in refinements for tighter contracts, never prerequisites.

`bevy_playtest doctor` audits a project's readiness: dependency
present, `takeover` feature enabled, plugin referenced in `src/`
(plain-text grep — renames/re-exports/macros can hide references),
scenario file valid. Exit 0 = ready, exit 4 = hints shown.

### One-line integration (PlaytestPlugin, now default)

Add the plugin; when `BEVY_PLAYTEST_SCENARIO` is set, your binary becomes its
own headless playtest runner (exit codes: 0 Pass, 1 Fail, 2 Crash, 3 config
error). Without the env var the plugin is inert.

```toml
bevy_playtest = "..."
```

```rust
app.add_plugins(DefaultPlugins);      // ordinary windowed setup — takeover still works
app.add_plugins(bevy_playtest::PlaytestPlugin); // MUST come after plugin groups
```

Headless is automatic: with `BEVY_PLAYTEST_SCENARIO` set, the takeover
runner replaces winit's event loop, so no window appears — no
`PlaytestDefaultPlugins` swap needed (it remains available as optional
sugar). Caveat: `PlaytestPlugin` must be added AFTER `DefaultPlugins`,
or winit's runner wins and the takeover is silently inert.

Then, for a zero-config smoke test:

```console
$ BEVY_PLAYTEST_SCENARIO=smoke cargo run
```

Or the full three-command happy path:

```console
$ bevy_playtest init                 # writes playtest.scenario.json
$ $EDITOR playtest.scenario.json     # add invariants (example in --help)
$ bevy_playtest run playtest.scenario.json
```

`PlaytestDefaultPlugins` swaps to a headless runner when a
`BEVY_PLAYTEST_*` env var is set, and is a pure pass-through to
`DefaultPlugins` otherwise. If you forget to swap and keep `DefaultPlugins`,
the takeover detects `WinitPlugin` and exits 3 instead of opening a window.
Known limitations of the automatic headless takeover: games whose first state
is a menu may need a reset hook before the scenario is meaningful; rendering
is unavailable (software-only) so visual assertions do not apply.

### Basic integration (tag gameplay entities)

Tag gameplay-relevant entities with `bevy_playtest::contract::Gameplay`. That's it.

```rust
use bevy::prelude::*;

fn tag_gameplay(mut commands: Commands, ball: Query<Entity, With<Ball>>) {
    for e in ball.iter() {
        commands.entity(e).insert(bevy_playtest::contract::Gameplay);
    }
}
```

**Integration is one dependency, one plugin line, two env vars — no other
ceremony.** No `build_app()` extraction, no `WinitPlugin` disabling, no
plugin-group swap:

```rust
app.add_plugins(DefaultPlugins);
app.add_plugins(bevy_playtest::PlaytestPlugin); // AFTER the group
```

Under takeover (`BEVY_PLAYTEST_SCENARIO` set) the plugin's runner swap
(last-set-wins) replaces winit's event loop, so the game runs headless
with no window. Under `BRP_PORT` the same binary serves the external-BRP
surface. The one ordering rule: add `PlaytestPlugin` AFTER any plugin
group that sets a runner, or the takeover is silently inert.

### Verification depth (what is proven vs. what is asserted)

Three mechanisms take a run from "asserted" to "proven":

1. **Replay divergence** (`"verify_replay": true`) runs the scenario
   twice against fresh Apps and compares per-frame fingerprints
   (choice draws + sorted Transform translations). Divergence is a
   `replay_divergence` Fail identifying the first divergent frame.
   This PROVES determinism for the observable surface — it does not
   prove absence of nondeterminism in unobserved state (components
   without Transform). In-process only; BRP mode rejects the field
   (wall-clock pacing makes honest verification impossible there).
   On the takeover/plugin path, pass B re-runs your game binary as a
   subprocess — whole-process determinism, the strongest form. NOTE:
   `verify_replay` requires `single_threaded: true` (default).

2. **Cheat postcondition contracts** (`expects` on cheat entries):
   v1 is statistical — transform-entity count comparisons
   (`lt`/`gt`/`eq`) against the pre-hook value, evaluated
   immediately after the hook runs. This catches silently no-op hooks
   (a failing contract is a `cheat_contract_violated` Fail carrying
   both counts). Semantic contracts (resource/component VALUES) are
   roadmapped behind reflection wiring — v1 cannot catch a hook that
   mutates state without changing entity counts.

3. **Behavioral coverage** (`observes: [...]` declarations): the
   report records, per declared event, whether any mechanism saw it
   this run. v1 mechanism: log-message matching (digit-normalized,
   INFO included) — observation is opt-in by logging convention
   (`tracing::info!("enemy_killed")`). Unobserved declarations are
   coverage holes in the report; `"strict_coverage": true` makes
   them Fail. Coverage also reports the fraction of the chaos
   `raw_surface` keys actually drawn. Holes without the strict flag
   are visible truth, not silent passes — but they don't gate.

### BRP parity matrix

| Capability               | In-process | External BRP | External BRP + PlaytestBrpPlugin |
|--------------------------|------------|--------------|----------------------------------|
| Panic capture            | ✓          | ✗            | ✓ (`bevy_playtest/health`)       |
| Log capture              | ✓          | ✗            | ✓                                |
| Reset hooks              | ✓          | ✗            | ✓ (`bevy_playtest/reset`)        |
| Cheat hooks              | ✓          | ✗            | ✓ (`bevy_playtest/cheat`)        |
| Time scale control       | N/A        | ✗            | ✓ (`bevy_playtest/time`)         |
| Pause/step               | ✓          | ✗            | ✗ (in-process only)              |

The report's `brp_parity` field distinguishes "full" from "degraded"
modes; never a silent downgrade.

### Zero-contract external-BRP (experimental)

Run the game as a subprocess and drive it over Bevy's Remote Protocol — no
harness imports in the game at all. Requires the game binary to:

1. Add Bevy's remote plugins (`RemotePlugin::default()` +
   `bevy::remote::http::RemoteHttpPlugin::default()`, with the
   `bevy_remote` cargo feature)
2. Register the types the harness inspects:
    ```rust
    .register_type::<Transform>()
    ```
3. Read the listen port from the `BRP_PORT` env var and pass it to
   `RemoteHttpPlugin::default().with_port(port)` (the harness sets it)

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

Reports carry `harness_mode: "external_brp"`. Limits versus in-process:
chaos bot only, no reset/cheat hooks, and a reduced oracle set — bounds
and NaN transforms are checked, but panic, log, and liveness oracles
require in-process mode.

See the [docs](docs/index.md) for the full guides.
