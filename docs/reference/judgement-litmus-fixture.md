# Judgement-layer litmus fixture (planted spatial defect)

Reference for `fixtures/demon_stack/`: a hand-authored Bevy game
whose entire spatial behavior is one deliberately planted defect.
It is the `pairwise_separation` rule's fixture-tier LITMUS — the
judgement layer's §8 acceptance exhibit ("demon stack, contact pile
= one episode, worst pair, bounded roster, soak-discarded fly-bys",
per `docs/proposals/judgement-layer.md`).

Same doctrine as the [known-answer fixture](known-answer-fixture.md):
a rule is only trustworthy if it has been checked against a world
whose verdict was derived independently — here, by hand from the
code that was written to produce the pile. The fetched showcase
games are realistic, but none of them contains a sustained spatial
defect, so a run against them can only prove silence, never
correctness. `demon_stack` plugs that hole for the spatial-family
rules: it contains geometry chosen because a naive pairwise
checker gives the wrong answer for it.

## The plant

Exactly one defect: proportional homing with **no separation term**
(`demon_dynamics` in `fixtures/demon_stack/src/main.rs` — see the
`IMPORTANT` comment there; adding a collision-resolution pass would
erase the planted defect this fixture exists to exhibit).

Six `Demon`s spawn on a ring (radius 8) and chase a `PileTarget`
jittered deterministically by frame counter (no wall clock, no RNG).
Proportional homing shrinks the step as the distance shrinks, so the
swarm collapses into sustained near-perfect coincidence around the
player rather than orbiting — the same shape the arena shooter's
contact-damage stack produced in dogfooding.

Construction properties (each pins a specific oracle to SILENT so
the golden verdict can pin the pairwise finding ALONE):

| Property | Mechanism | Pins silent |
|----------|-----------|-------------|
| World never freezes | target jitter + player scale "breathing" | `frozen_world` |
| Transforms keep changing | pile micro-motion + breathing player | value-hash phantom-write check |
| Spatial predicate never misfires elsewhere | no walls, no bounds, no UI overlays | `nodes_in_bounds` |
| Camera exists and renders | `Camera3d` in `setup` — `render_liveness` reports camera absence as a standing informational finding, which would break the "exactly one finding" verdict | `render_liveness` |
| Progress surface irrelevant | no progress markers declared; `liveness.progress.mode: "off"` in the litmus scenario | `stalled_progress` / `progress_surface_empty` |
| Reflector present | `Demon`/`Player`/`PileTarget` are `Reflect`-derived and registered | selector-resolution failures |

## The litmus verdicts

Two golden-verdict tests (`fixtures/demon_stack/tests/playtest.rs`),
run via the battery subprocess path — the full journey: scenario
JSON → child process → takeover plugin → recorder → folded finding
→ report back over the wire.

| Test | Scenario | Verdict pinned |
|------|----------|----------------|
| `pile_is_one_judged_episode_with_roster_evidence` | 20 s passive run, `min_distance: 0.5`, `window_s: 2.0` | Run status `fail` with EXACTLY ONE finding overall (complement pin: any other oracle firing on this deterministic scene is a misfire); detail needles "of each other" + "not a fly-by"; `_meta.pairwise_episode` with `sampled_pairs == 15` (= C(6,2), exact), roster UNtruncated, first violating frame ≤ 60, episode sustaining to the run's end |
| `fly_by_control_stays_silent` | Same world, same contacts, `window_s: 20.0` — just above the episode's 19.43 s span | Genuine `pass` status (a crash is not silence) and zero pairwise findings — a boundary discard: soak drift in either direction flips the verdict |

The fly-by control is the same world with a soak window sitting
JUST above the episode's span: the contacts are identical, only the
judgment horizon differs (~3% margin), so a units bug or arithmetic
drift in the soak — inflating or deflating the effective window —
flips the verdict in one direction or the other. A rule that fired
anyway would be judging single crossings — exactly the
false-positive the ε/soak design exists to prevent. Together the
two tests pin both sides of the rule's contract: red on sustained
piles, silent on the identical-but-sub-soak geometry.

## Determinism

No wall clock, no RNG: the target jitter is a pure function of the
frame counter, and `simulated_time` keeps each `app.update()` at
exactly 1/tps. The verdict is stable across runs and seeds (the
integration-tier analogue is `tests/it/pairwise_separation.rs`'s
slow-clock discrimination suite, which proves the soak never
converts through `tps`).

## Maintenance contract

Committed wholesale and NOT managed by
`scripts/fetch_bevy_examples.sh` (the fetch script must preserve it
in the workspace members list; regeneration must not touch it —
the same contract as `known_answer`). The `playtest` feature is
activated via the self-dev-dependency pattern: plain
`cargo build` compiles the game without `bevy_playtest`;
`cargo test` runs the battery against the compiled binary.
