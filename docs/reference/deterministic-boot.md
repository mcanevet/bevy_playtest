# Deterministic boot

How to make a game's boot deterministic so playtests can trust
their verdicts. The harness can *detect* nondeterminism
(`verify_replay` divergence) but only the game can *remove* it.
This page is the "why + how to fix" target for that message: when a
run degrades, the remediation is in your game, not in the scenario.

## The one-line fix: read your seed from the environment

A deterministic boot needs the game's random state to be identical
across processes. The pattern:

```rust
// One line, somewhere in your plugin setup, before any randomness
// is drawn.
let seed: u64 = std::env::var("MY_GAME_SEED")
    .ok()
    .and_then(|s| s.parse().ok())
    .unwrap_or(0xC0FFEE);
```

The harness does not set this variable for you and does not define a
canonical name — the point is simply that the value comes from the
environment (per-process, inspectable, overridable) rather than from
`from_entropy()`/system entropy sources, which differ across every
process launch and make every run unique.

Concretely, wherever the game creates a PRNG resource at boot:

```rust
use rand::{SeedableRng, rngs::StdRng};

app.insert_resource(MyRng(StdRng::seed_from_u64(seed)));
```

The harness's own bots are seeded the same way (see the scenario
`bot.seed` field) — one seeded stream per process is what makes a
`chaos` scenario replayable at all.

## What unseeded RNG does to your results

Nothing in the harness *fails* on an unseeded game RNG. What happens
is quieter and worse: the run **degrades**.

- `verify_replay: true` compares two freshly built apps running the
  same scenario. Unseeded game RNG diverges the per-frame
  fingerprints; the report fails with `replay_divergence` citing the
  first mismatching frame, with the canonical causes in its message:
  wall clock, unseeded RNG, or unordered iteration.

The remediation for unseeded RNG is the seed line above (plus, for the
other canonical causes: a fixed virtual time instead of wall-clock
reads, and avoiding unordered-iteration-dependent state).

## Common causes beyond the message

When a replay comparison diverges, the harness reports the first
mismatching frame and both hashes, and the violation message names
the canonical causes (wall clock, unseeded RNG, unordered
iteration). It does not classify beyond that — these are frequent
culprits from experience, not harness-detected categories:

- **Persisted external state** — a game that reads or writes state
  outside the process (a save file, a cache) boots differently on the
  second run even with identical code and seed. Delete or isolate the
  state between runs (the thetawave fixture keeps its sqlite storage
  plugin off by default for exactly this reason).
- **Network sockets** — anything opening real sockets at boot pulls
  peer identity, latency, and external responses into the boot
  digest. Make network subsystems opt-in at boot so the playtested
  build does not dial out.
