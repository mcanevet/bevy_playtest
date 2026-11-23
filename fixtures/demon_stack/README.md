# demon_stack — spatial-litmus fixture

A tiny hand-authored Bevy game whose entire spatial behavior is a
deliberately planted defect: six demons converge on a jittering
target with no separation term, collapsing into a sustained contact
pile. It is the `pairwise_separation` rule's fixture-tier LITMUS —
the judgement layer's §8 acceptance exhibit ("demon stack, contact
pile = one episode, worst pair, bounded roster, soak-discarded
fly-bys").

Unlike the other `fixtures/` members (fetched showcase games), this
one is fully committed and NOT managed by
`scripts/fetch_bevy_examples.sh` (the fetch script must preserve it
in the workspace members list).

## What is planted

Exactly one defect: the homing-without-separation pile in
`demon_dynamics` (see the `IMPORTANT` comment in `src/main.rs` —
adding a collision-resolution pass would erase the planted defect
this fixture exists to exhibit).

| Plant | Mechanism | Judgment |
|-------|-----------|----------|
| Contact pile | 6 `Demon`s, proportional homing on a jittered `PileTarget`, no separation term | `pairwise_separation` RED: exactly one folded finding, worst-pair detail, `_meta.pairwise_episode` with exactly C(6,2)=15 counted pairs |
| Fly-by control | Same world, same contacts, `window_s: 20.0` (just above the episode's 19.43s span — a boundary discard) | SILENT + genuine `pass` status: soak drift in either direction flips the verdict |

Everything else is deliberately judgeable-clean:

- Deterministic (no wall clock, no RNG, frame-driven jitter), so the
  golden verdict is stable across runs and seeds.
- The pile keeps micro-moving (target jitter) and the player
  breathes (scale oscillation) — `frozen_world` stays silent; a
  camera is spawned so `render_liveness` (which reports camera
  absence as a standing informational finding) stays silent too;
  the litmus pins the pairwise finding ALONE as the run's only
  finding.
- `Demon`/`Player`/`PileTarget` are `Reflect`-derived and
  registered, so `with_component` selectors resolve them by name.

## Running

Standard battery tier (same as every fixture):

```sh
cargo test --manifest-path fixtures/demon_stack/Cargo.toml
```

Plain `cargo build` compiles the game WITHOUT `bevy_playtest`
(feature-gated plugin pair); `cargo test` activates `playtest` via
the self-dev-dependency and runs the golden-verdict litmus against
the compiled binary.
