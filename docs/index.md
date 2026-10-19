# bevy_playtest docs

A playtesting harness for [Bevy](https://bevyengine.org) games: scenarios drive
a bot through your game while oracles check invariants and produce a
machine-readable report.

## Where to start

| You want to... | Go to |
|---|---|
| Try the harness on a game, step by step | [Tutorial](tutorial.md) |
| Solve a specific task | [How-to guides](how-to.md) |
| Look up exact APIs and config fields | [Reference](reference.md) |
| Understand why it works this way | [Explanation](explanation.md) |

## What it does

- Runs a Bevy game headless, in-process, or as a subprocess over BRP
- Drives it with a seeded chaos bot or a scripted key schedule
- Watches for: NaN/finite transforms, gameplay entities leaving bounds,
  a frozen world, panics, ERROR logs
- Emits JSON and JUnit XML reports
- Fails loudly on anything it doesn't support — never a vacuous pass

## One-minute mental model

```
Scenario (JSON) ──▶ Bot ──▶ your App ──▶ Oracles ──▶ PlaytestReport
                    │                        │
             key events, ticks        invariants, liveness,
                                        panic/log capture
```

The harness never edits your game logic. In-process mode needs a
`build_app()` function and no `WinitPlugin`; external-BRP mode needs
nothing but a listening port.