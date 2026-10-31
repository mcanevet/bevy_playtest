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

Two constraints:

- **Requires `single_threaded: true`** (the default) — multithreaded
  system order is not stable enough to fingerprint.
- **In-process only.** External-BRP mode rejects the flag explicitly:
  wall-clock pacing makes honest replay verification impossible there.

On divergence, the report fails with `replay_divergence` and the first
frame where the two passes disagreed — the signature of hidden
nondeterminism (unseeded game RNG, wall-clock reads, iteration-order
dependencies).

