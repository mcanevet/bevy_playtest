# known_answer — planted-truth fixture

A tiny hand-authored Bevy game whose ENTIRE input surface is
deliberately constructed (every binding is a numbered "plant").
It is the known-answer test for the discovery stack: ground truth is
committed (`ground_truth.json`), measurement asserts against it,
and the two are cross-checked so neither can drift.

Unlike the other `fixtures/` members (fetched showcase games), this
one is fully committed and NOT managed by `scripts/fetch_bevy_examples.sh`
(the fetch script must preserve it in the workspace members list).

## What is planted

See `docs/reference/known-answer-fixture.md` (repo root docs) for the
full plant catalog and the per-lens commitments. Summary:

| Plant | Key(s) | Point |
|-------|--------|-------|
| P1 | KeyU | unmute toggle, live in menu AND gameplay, mutable binding |
| P2 | W/A/S/D | movement through a bespoke `Keymap` resource |
| P3 | KeyR | reset with a reflected witness counter |
| P4 | KeyL+ShiftLeft | CHORD — naive single-key sweep MUST miss |
| P5 | KeyT | DEBOUNCE (20-frame) — single taps only arm; MUST miss |
| P6 | KeyG | `cfg(debug_assertions)`-gated (dev-profile surface) |
| P7 | KeyV | behind an unreachable state — never discovered at boot |
| N1 | Digit1–Digit0 | hotbar array: 2 live of 10 (shape vs behavior) |
| N2 | Enter (preview-map value KeyS is display data too, but its movement row is P2's) | rebind-preview map + settings row: display data only |
| N3 | * | `last_pressed` logger: KeyCode-shaped, must be invisible |
| N4 | KeyX | raw read, log-only: raw-read shape earns no grade |

Each `// PLANT Pn` / `// PLANT Nn` marker in `src/main.rs` corresponds
to at least one row in `ground_truth.json`; a test enforces both
directions (markers↔rows, full catalog present).

## Running

Standard battery tier (same as every fixture):

```sh
cargo test --manifest-path fixtures/Cargo.toml \
  -p fixture_known_answer --features playtest --test playtest \
  -- --test-threads=1
```

Measurement and mutation harnesses are OPT-IN via the fixture-local
`measure` feature — the plain battery tier never pays for them:

```sh
# D0 sweep lens, graded against ground truth (TP/FP/FN/TN printed)
cargo test --manifest-path fixtures/Cargo.toml -p fixture_known_answer \
  --features playtest,measure --test measure -- --test-threads=1

# Mutated-code drift (P1_dead, N1_hotbar_full)
cargo test --manifest-path fixtures/Cargo.toml -p fixture_known_answer \
  --features playtest,measure --test mutation -- --test-threads=1
```

Single-threading is required: `run_scenario` is exclusive per process.

## Mutations: `KNOWN_ANSWER_MUTATION`

The game supports two runtime code mutations, selected by env var:

- `P1_dead` — P1's unmute system early-outs (declared-but-dead).
- `N1_hotbar_full` — all ten hotbar slots go live (negative→positive).

Unset/empty means pristine. Any other value PANICS the game loudly.

### Why the mutation var is NOT `BEVY_PLAYTEST_`-prefixed

Intentional, do not "fix" it. The harness polices and strips its own
namespace:

- The takeover plugin rejects unknown `BEVY_PLAYTEST_*` vars
  (exit 3) — a prefixed mutation var would be treated as an unknown
  harness knob.
- The discovery probe pool strips `BEVY_PLAYTEST*` from spawned
  game processes — a prefixed var would never reach the game.

`KNOWN_ANSWER_MUTATION` is FIXTURE-OWNED configuration (part of this
game's code, read in `src/main.rs`), not harness configuration.
Its handling never touches library code. The mutation tests
(`tests/mutation.rs`) rely on it passing verbatim from the test
process through the probe pool to the spawned game.
