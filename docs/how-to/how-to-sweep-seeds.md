# Sweep your game across seeds (and keep CI memory)

A single-seed battery verdict is luck-dependent: the stock `mines`
example famously passes its default battery run on seed 42 and fails
on 3 of 5 sampled seeds — its static menu escapes only through a
phase-precise click that most seeds never happen to land. A verdict
that flips on the seed is not a verdict.

The seed sweep fixes this in two layers:

1. **Sweep**: run the same scenario across several seeds in a bounded
   worker pool, and aggregate the findings by fingerprint
   (`rule:target` — frames, counts, and details are deliberately
   stripped, so the same bug on two seeds is ONE finding, not two).
2. **Regression file**: a plain-text, human-editable, git-mergeable
   ledger of known fingerprints. The gate fails only on NEW or
   REGRESSED findings; known ones pass (you adjudicated them), and a
   known fingerprint that stops reproducing is reported as *healed* —
   delete its line so a comeback of the same bug gates again.

## Run the sweep

```rust
// In your game's playtest test:
use bevy_playtest::sweep::{run_sweep_and_gate, SweepConfig};

#[test]
fn seed_sweep_stays_within_known_findings() {
    run_sweep_and_gate(
        &SweepConfig::new(env!("CARGO_BIN_EXE_my_game"))
            .regressions("tests/seed_regressions.txt"),
    );
}
```

`SweepConfig::new` uses the built-in battery scenario, the default
seed set (`42, 43, 99, 1234, 2718`), and 4 parallel workers. Chain
`.scenario(...)`, `.seeds(...)`, `.parallel(...)`, and
`.regressions(...)` to customize.

## The first failure: recording a finding

The first sweep on any imperfect game fails with a summary listing
each new finding:

```text
finding [frozen_world:world] on seeds [42, 43]
  ...
gate: NEW OR REGRESSED findings (unknown fingerprints)
```

Adjudicate the finding (is it a real bug? a pre-existing limitation
you're willing to declare?), then record it by copying the
`rule:target` key into your regression file:

```text
# tests/seed_regressions.txt
frozen_world:world  # menu soft-lock; adjudicated — see PLAYTESTS.md
```

The next sweep passes: `gate: all findings known (non-blocking)`.

Hand-written raw entity keys are accepted: the global
`nodes_in_bounds` / `finite_transforms` oracles report per-entity
debug targets (`nodes_in_bounds:3v1`) whose volatile index varies
across seeds, so the gate fingerprints them in canonical form. If
you copy a raw key straight from a report's detail line — or simply
guess the entity shape — the loader accepts it and stores the
canonical identity (`nodes_in_bounds:entity`) that the gate
actually compares; `healed:` notes likewise print the canonical
key, not what you wrote.

Every sweep member is an ordinary battery run with a seed override,
so you reproduce one exactly with a single battery run: the same
binary and scenario with `seed_override: Some(43)`. The battery then
applies the same source-scan key layering and seeds the deterministic
OS randomness the same way.

For a quick look by hand, the override also travels to the game via
`BEVY_PLAYTEST_SEED`. This is only an approximation: it runs without
the battery's key layering and with real OS randomness, so a finding
that depends on either may not show up. The seed var needs its
scenario alongside (on its own it leaves `PlaytestPlugin` inert):

```console
$ BEVY_PLAYTEST_SCENARIO=battery BEVY_PLAYTEST_SEED=43 cargo run --features playtest --bin my_game
```

The override applies after scenario loading, so it composes with
scan-layered scenario variants. A non-`u64`
value aborts with the config-error exit code (3) — a typo'd seed
that silently ran the declared seed instead would be the worst
failure mode for a repro knob.

Panics are never recorded: every panic is reported against the target
`world`, so one recorded line would mute every later, different panic.
A member that panics always gates. Likewise, a recorded
`log_error:<module>` covers every error logged by that module.

## Healing

When a fix lands and a recorded finding no longer reproduces on any
swept seed, the sweep prints a healed note. Delete that line from the
regression file — leaving it in means a regression of the SAME bug
would arrive as a known (passing) finding instead of gating.

**Healed over your seed set is weak evidence.** A flaky finding can
miss a handful of seeds by luck; a deleted entry no longer gates.
Before deleting on a sweep-wide heal, re-sweep with a wider seed set
(or the game's full battery duration) and confirm the fix story.

## Notes

- Blocking findings are fingerprinted as `(rule, canonicalized
  target)`. Informational rules (e.g. `log_warn`) never
  fingerprint — BUT a member that fails outright with no
  fingerprintable finding (typically an informational escalated to
  gating by scenario config, like `progress.require_surface` or
  `strict_warnings`) still blocks the gate: there is no key to
  record, so the fix is on the game/scenario side. Crashed members
  always block — a crashed run certifies nothing.
- Infrastructure failures (spawn errors, timeouts, a child that dies
  without writing a report) fail the sweep outright; they say nothing
  about the game and never become regression-file entries.
- One `#[test]` per game, one regression file per game. The gate
  composes with `run_and_expect_scoped_findings` — use the scoped
  expectations for per-scenario adjudication and the sweep as the
  fleet-level memory layer on top.
