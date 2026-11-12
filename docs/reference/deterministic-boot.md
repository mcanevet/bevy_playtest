# Deterministic boot

How to make a game's boot deterministic so playtests can trust
their verdicts. The harness can *detect* nondeterminism
(`verify_replay` divergence, `Statistical` surface confidence) but
only the game can *remove* it. This page is the "why + how to fix"
target for those messages: when a run degrades, the remediation is
in your game, not in the scenario.

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
- Input-surface discovery runs its determinism gate on doubled
  control runs. Controls that disagree stamp the whole surface
  `Statistical` — and a `Statistical` surface cannot certify trusted
  negatives: a key that probed inert is reported as "probed inert
  under Statistical controls — the negative is untrusted", never as
  "the game does not use this key".

That degradation is existing behavior, unchanged by this page. The
remediation is the seed line above (plus, for the other canonical
causes: a fixed virtual time instead of wall-clock reads, and
avoiding unordered-iteration-dependent state).

## Degradation classes, as reported categories

When a control run diverges, the harness reports what it *saw* —
categories, not guesses about your intent:

- **Unseeded RNG / wall clock / unordered iteration** — the
  `replay_divergence` and `Statistical`-surface message trinity.
  Fix: the seed line; virtual time; deterministic iteration.
- **Persisted external state** — a game that reads or writes state
  outside the process (a save file, a cache) boots differently on
  the second run even with identical code and seed. The harness
  reports what the digests observed; deleting or isolating the state
  between runs is a game-side fix. Known-answer fixtures document
  their own persistence choices (e.g. the thetawave fixture keeps
  its sqlite storage plugin off by default for exactly this reason).
- **Network sockets** — anything opening real sockets at boot
  introduces peer identity, latency, and external responses into
  the boot digest. The report shows the resulting divergence; the
  fix is to make network subsystems opt-in at boot so the playtested
  build does not dial out.

These are REPORTED categories — observations attached to a degraded
verdict, never hard failures on their own. The harness does not (and
should not) infer intent from them; the message tells you where to
look, and the fixes above are in the game's hands.
