# Discover your game's input surface

**Problem:** you want to know which keys your game actually responds
to — for a new project nobody touched in months, a teammate's game,
or before authoring scenarios — without reading the code or guessing
from bindings tables.

**Solution:** run one battery playtest with
`BEVY_PLAYTEST_DISCOVER=1` in the environment. The harness spawns a
pool of probe subprocesses that tap candidate keys one at a time and
diff each run against a control, learning which keys perturb the
game state. The discovered surface rides along on the playtest
report (`discovered_surface`), and any frozen-world finding gains a
`_meta` block with probe statistics next to its verbatim detail.

```console
BEVY_PLAYTEST_DISCOVER=1 cargo test --test playtest -- --nocapture
```

The first discovery costs ~40s (the full candidate matrix runs 6–8
subprocesses in parallel); the result is cached under
`.bevy_playtest/cache/` keyed by the game binary, the harness
version, the active features, and the probe plan, so subsequent runs
pay nothing.

## What you get

The report carries a `discovered_surface` block:

```json
"discovered_surface": {
  "keys": [
    { "key": "Digit1", "effect": { "first_divergence_frame_delta": 1 },
      "confidence": "Exact" },
    { "key": "KeyQ", "effect": null, "confidence": "Unclassified" }
  ],
  "probed_at": [60, 90],
  "confidence": "exact",
  "readers": [["keyboard", 3]],
  "pruned_devices": [],
  "notes": ["pruned_devices withheld: the static scan did not see the full schedule graph, so zero reader counts prove nothing (fail-open; the keyboard class is never pruned)",
            "exclusive system detected: probing the full surface (an exclusive system may read anything)"]
}
```

Reading it:

- **`effect`** is the observed first divergence between the probe
  run and an identical control run that pressed a different key
  (`null` = the key did nothing at the probed points).
- **`confidence` per key** is `Exact` (observed in this key's own
  run), `Inferred` (group completion: a WASD/arrows sibling of an
  observed-effective key, recorded as likely-consumed without
  fabricated evidence), or `Unclassified` — the probe matrix's own
  control key, whose probe run is identical to its baseline by
  construction: no verdict about it is possible either way, and the
  surface says so instead of reporting a trusted negative. The
  default candidate set excludes the control key; only a bespoke
  candidate list (like the sample above) can produce this entry.
- **`confidence` whole-surface** is `exact` (the two identical
  control runs agreed — negatives are trusted) or `statistical`
  (the controls diverged from each other; the game is
  nondeterministic — every conclusion is weaker and the notes say
  so). Discovery never fails on a nondeterministic game; it
  downgrades confidence instead.
- **`probed_at`** lists the frames the taps landed on (readiness —
  ~1s in, covering menu/title listeners — and mid-duration).

A `frozen_world` violation additionally gets `_meta`:

```json
"rule": "frozen_world",
"detail": "no Transform change for 180 frames — soft-lock?",
"_meta": {
  "probed_keys_effective": 1,
  "probed_keys_total": 2,
  "note": "probed 2 keys, 1 effective (confidence: exact)"
}
```

`probed_keys_total` counts keys with an INFORMATIVE raw probe record:
never-probed group-completion siblings and the control key are
visible in `discovered_surface.keys` but excluded from the count, so
the denominator never overstates what the matrix actually tapped.

That is the diagnostic payoff: a frozen world where several keys
provably move things looks very different from one where nothing in
the probed set does.

## Knobs (all optional, env vars)

| Variable | Effect |
|---|---|
| `BEVY_PLAYTEST_DISCOVER=1` | Attach discovery to battery runs |
| `BEVY_PLAYTEST_DISCOVER_KEYS=Digit1,KeyQ` | Probe only these candidates (comma-separated, canonical `KeyCode` spellings). Invalid names fail the run — they never silently probe nothing. |
| `BEVY_PLAYTEST_DISCOVER_CACHE=/path` | Cache dir override (default `.bevy_playtest/cache/`). CI jobs isolated per workspace use this. |

The game subprocess never sees these variables — the battery and the
probe pool strip every `BEVY_PLAYTEST_*` variable from spawned
children (strict takeover contract), so opt-in is purely a
parent-side decision.

## Managing the cache

The cache is a staleness token, not a security primitive: entries
are keyed by the game binary's hash, the bevy_playtest version, the
active feature set, the fixture name, and a fingerprint of the probe
plan (candidate set, control key, tps, duration). Any rebuild, harness
upgrade, feature change, or candidate-list change is an automatic
miss — a slim `BEVY_PLAYTEST_DISCOVER_KEYS` run never answers from a
full-matrix entry or vice versa.

```console
rm -rf .bevy_playtest/cache   # force fresh discovery
```

A corrupt or unreadable entry is a miss, never an error — the cache
can only ever make discovery slower, never wrong or failed.

## Calling it directly (library use)

If you embed the harness in your own test code and want the surface
as a value, skip the env var and call the orchestrator:

```rust
use bevy_playtest::discovery::flow::{discover, DiscoveryConfig};

let mut config = DiscoveryConfig::new(env!("CARGO_BIN_EXE_my_game"));
config.plan.candidate_keys = vec!["Digit1".into(), "KeyQ".into()];
let outcome = discover(&config)?;
println!("from cache: {}", outcome.from_cache);
for finding in &outcome.surface.keys {
    println!("{}: {:?}", finding.key, finding.effect);
}
```

## Limits (MVP, by design)

- **Keyboard only, tap only.** The candidate set is ~26 prior-likely
  keys; mouse/gamepad/wheel are future probe classes. The keyboard
  class is never pruned (fail-open).
- **Two probe points.** Readiness and mid-duration. A key consumed
  only in a state neither point samples reads as inert — group
  completion catches the near-universal movement clusters; anything
  else needs a wider matrix (`BEVY_PLAYTEST_DISCOVER_KEYS`) or
  custom probe points.
- **Hash-only effects.** `Effect` says when the run diverged, not
  which systems reacted (a snapshot vocabulary would be needed —
  see the design doc).
- **Static scan runs in the game child.** The Phase 1 static scan
  (`readers` / `pruned_devices`) runs in the probe child — the pool
  asks its first control run to export the scan
  (`BEVY_PLAYTEST_DISCOVER_SCAN`), and the flow merges it into the
  surface. The scan is CAPTURED inside `run_scenario` at
  harness-setup time — after input-signal seeding but BEFORE the
  harness registers its bot and oracle systems (their input-message
  writes would otherwise count as phantom game readers on every
  device class), then exported after the scenario. The capture only
  happens when the scan export was requested — ordinary playtests
  keep their previous world setup and pay no scan cost. If no export
  arrives (a masked-name build, a child that
  failed to write it), the surface degrades fail-open: empty
  `readers`/`pruned_devices` and a "static scan not run" note. The
  keyboard class is never pruned regardless (fail-open). Likewise, if
  the exported scan could not see the full schedule graph (the
  `Schedules` resource missing, a schedule whose systems were never
  initialized), its reader counts still land in `readers` as
  annotations but pruning is withheld with a
  "pruned_devices withheld" note — zero readers only proves anything
  from a complete scan. In practice pruning never fires today: the
  capture point sits before the world's first update, so every
  exported scan is incomplete by construction and reader counts are
  always annotations (see the design doc's pruning-dormancy note).

See `docs/explanation/input-surface-discovery.md` for the full
design, accounting, and open questions.
