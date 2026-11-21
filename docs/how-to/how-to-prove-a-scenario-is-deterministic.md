# Prove a scenario is deterministic

The scenario's seed makes a single run reproducible; it does not prove
the *game* is deterministic. `"verify_replay": true` does: it runs the
scenario twice against freshly built apps and compares per-frame
fingerprints (world state + the bot's recorded choices).

```json
{
  "bot": { "type": "chaos", "seed": 42 },
  "duration_s": 10.0,
  "single_threaded": true,
  "verify_replay": true
}
```

One constraint:

- **Requires `single_threaded: true`** (the default) — multithreaded
  system order is not stable enough to fingerprint.

On divergence, the report fails with `replay_divergence` and the first
frame where the two passes disagreed — the signature of hidden
nondeterminism (unseeded game RNG, wall-clock reads, iteration-order
dependencies). The
[deterministic boot reference](../reference/deterministic-boot.md)
explains the usual causes and the game-side fixes.

On the takeover/plugin path, pass B re-runs your game binary as a
fresh subprocess — whole-process determinism, the strongest form.
An infrastructure failure is never a silent skip: on the takeover
path the game exits with config-error code 3 (the battery surfaces
that as an infrastructure failure), and in-process
`replay::run_scenario_verified` returns a `ScenarioError`. The
divergence comparison itself produces the usual `replay_divergence`
Fail. Under the battery, enabling `verify_replay` on the scenario is
all it takes.

