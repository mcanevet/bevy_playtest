# bevy_playtest docs

A playtesting harness for [Bevy](https://bevyengine.org) games: scenarios drive
a bot through your game while oracles check invariants and produce a
machine-readable report.

## Where to start

| You want to... | Go to |
|---|---|
| Try the harness on a game, step by step | [Tutorial](tutorial/) |
| Solve a specific task | [How-to guides](how-to/) |
| Look up exact APIs and config fields | [Reference](reference/) |
| Understand why it works this way — design rationale, or how the harness tests itself | [Explanation](explanation/) |

## What it does

- Runs a Bevy game headless, in-process, or as a subprocess over BRP
  (Bevy's Remote Protocol)
- Drives it with a seeded chaos bot or a scripted key schedule
- Watches for: NaN/finite transforms, entities leaving bounds,
  a frozen world, panics, ERROR logs
- Emits JSON and JUnit XML reports; anything it can't genuinely check
  fails at load instead of passing silently (see
  [why never a vacuous pass](explanation/why-never-a-vacuous-pass.md))

## One-minute mental model

```
Scenario (JSON) ──▶ Bot ──▶ your App ──▶ Oracles ──▶ PlaytestReport
                    │                        │
             key events, ticks        invariants, liveness,
                                        panic/log capture
```

The harness never edits your game logic. In-process mode swaps two
plugin groups in (`PlaytestDefaultPlugins` + `PlaytestPlugin`);
external-BRP mode needs no harness imports in the game, but the game
binary must serve Bevy's Remote Protocol (`bevy_remote` feature,
`register_type`, `BRP_PORT`) and keep ticking without winit — see
[drive a game you can't modify](how-to/how-to-drive-a-game-you-cannot-modify.md).
