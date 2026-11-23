# Judgement-layer roadmap — dogfooding steering vote, 2026-11-23

Date: 2026-11-23, immediately after the published-0.16.0
validation round (all five dogfooding seats green, zero scenario
edits). Every seat was asked the same question — *as the
dogfooder of your game, what does the harness need next, ranked
by real defect-catching added to YOUR suite?* — and answered in
writing (`docs/roadmap-vote-2026-11-23.md` in each downstream
repo, local-only). This record consolidates the five ballots and
reconciles them with the judgement-layer proposal
(`docs/proposals/judgement-layer.md`), whose §7 sequencing they
were not shown, to fix the build order.

## The ballots

| Seat (genre) | #1 | #2 | #3 |
|---|---|---|---|
| Sports sim | `derived_transform_consistency` | spatial contracts | component-tier held traces |
| Arena shooter | spatial contracts | component-tier held traces | authored-scenario seed sweeps |
| Skate sandbox | spatial contracts | component-tier held traces | serde-grammar lint aggregation |
| RTS (bazel) | spatial contracts | menu/UI-driving tier | component-tier held traces |
| Park/tycoon sim | spatial contracts | component-tier held traces | aggregation contracts |

## What the vote says

1. **Spatial contracts win four of five first-place votes** (the
   design doc's §1 family: `pairwise_separation`,
   `occupancy_cap`, `spawn_safety`, `forbidden_region`). Each
   seat ranked it against preserved, measured bugs it would
   convert: a six-enemy contact-damage stack, a respawn ring
   spawning inside attack range, grounded rail tunneling reading
   as maximal liveness, harvest-path clumping, coaster geometry
   cutting path slabs. The strongest signal a harness feature can
   get: a corpus of known-red acceptance tests waiting for it.

2. **`derived_transform_consistency` takes the fifth first-place
   vote** — and it is the design doc's own §2 rule, promoted on
   the strength of a shipped-in-production miss the voter scored
   10/10: authored `Transform`s advancing while every
   `GlobalTransform` stayed frozen at identity — every sprite
   drawn piled at the center, the entire battery green, caught
   only by a human looking at a screenshot. It is axiomatic
   (bevy's one canonical produced→consumed chain), needs zero game
   knowledge, has near-zero false positives, and rides the same
   fold engine as §1.

3. **Component-tier held traces are unanimous top-3** (the
   proposal's §4). Two seats attached a sequencing requirement
   that outranks their raw preference: spatial rules are
   component-tier, so pass-side evidence should land **with or
   before** them, or every green spatial contract inherits the
   vacuity risk the liveness oracles already taught the seats to
   distrust (a selector matching zero entities passing forever).
   One seat has a staged regression pin that flips the day this
   ships. Implementation begins with the §4 audit the proposal
   already prescribes: verify the component-tier recorders
   compute the held data and drop it — plumbing, not new
   computation, until proven otherwise.

4. **Considered and deselected by the vote** (kept on the
   ledger, not dropped):
   - serde-grammar error aggregation into lint — authoring
     throughput, not catching; ranked last by four seats;
   - consequence-chain diagnostic suppression — legibility, not
     discovery; two seats noted it re-ranks upward once spatial
     findings (which have causal children) exist;
   - menu/UI-driving tier, aggregation contracts (conservation
     laws over component leaves), authored-scenario seed sweeps —
     three strong off-list proposals recorded for the 0.17+ line.

## Build order (vote ∩ §7 sequencing)

1. **Episode-fold engine** (proposal §7) — the shared substrate;
   pure, bevy-free, unit-tested alone.
2. **`pairwise_separation`** — flattest spatial proof of the
   engine (demon-stack litmus: contact pile = one episode, worst
   pair, bounded roster, soak-discarded fly-bys).
3. **`derived_transform_consistency`** (proposal §2) — the
   sports sim's production miss; same engine, axiomatic.
4. **Component-tier held traces** (proposal §4 audit) —
   interleaved BEFORE the remaining spatial rules ship, so
   `occupancy_cap`/`spawn_safety`/`forbidden_region` greens carry
   evidence from their first day.
5. **Remaining spatial family + entity stagnation** (proposal
   §1 remainder, §3 plausibility tier) — in that order, each
   shipping against its preserved litmus bug.

Rationale: steps 1–2 execute four seats' #1; step 3 honors the
strongest single-seat argument on the ballot (the only candidate
with a proven production miss); step 4 honors the unanimous
top-3 + with-or-before sequencing requirement; step 5 completes
the family the vote funded.
