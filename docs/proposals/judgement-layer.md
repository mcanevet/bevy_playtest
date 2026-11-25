# Design: the 0.16 judgement layer — entity-scoped contracts

Status: REVIEWED, SHIPPED AS PLAN — synthesized 2026-11-23 from
five dogfooding programs (a headless team-sports sim, an FPS-style
arena shooter, a skate-physics sandbox, an RTS with a tick-domain
sim clock, a park-management sim). Survived four adversarial
review passes (2026-11-23): 6 blockers + 9 minors → 1 blocker +
2 gaps + 4 minors → 1 blocker + 2 gaps + 4 minors → SHIP. The §8
acceptance table is the gate; the one declared exception
(`occupancy_cap`, grammar-until-litmus) is spelled out in §1 and
the table. Not yet implemented.

## 0. The thesis

0.15 can judge a game that *documents its contracts in reflected
resources*. Every dogfooding program independently hit the same wall:
the contracts that actually encode gameplay correctness live in
**entities and their derived/spatial relationships** — a GlobalTransform
that forgot its Parent, six demon bodies stacked on one point, a
respawn landing inside attack range, a skater clipping through a rail.
0.16 makes that tier judgeable.

Evidence per pillar (all empirically observed, not hypothetical):

| Pillar | Observed at | The blind spot |
| --- | --- | --- |
| Derived-transform consistency | sports sim (player pile) | a Transform edited without propagation; every oracle green |
| Pairwise separation | sports sim, arena shooter | 6 solids packed into a 1.2 m disk dealing 240 DPS; world churns, in bounds, "progressing" (occupancy — the density formulation of the same class — has NO independently observed litmus; see §1's ship-gate honesty) |
| Spawn safety | arena shooter | respawn ring centered on arena origin, not player — instant damage at spawn frame, no telegraph |
| Forbidden regions | skate sandbox | grounded rail tunneling *looks maximally alive* to the liveness oracle — noclip is indistinguishable from gameplay |
| Held-trace parity | skate sandbox, arena shooter | `held_invariants` collects only ResourceCheck/StagnantResource; every spatial/component contract is invisible in passing runs |

## 1. Rule family: spatial contracts

One grammar, four rules, sharing 0.15's established vocabulary
(`target_selector`, `window_s` in driver-leaf units with optional
`driver_units_per_second`, `after_s`/`before_s`/`eventually_s`, episode
semantics, one finding per episode, `_meta` evidence). Names align with
the arena shooter's design sketch and the sports sim's independent
convergence on a minimum-separation field (`min_distance` here —
matching the arena-shooter sketch's `min_pair_dist` in meaning;
the schema adopts ONE name when the engine lands, chosen then):

### `pairwise_separation`

```json
{ "name": "no_demon_stack", "rule": "pairwise_separation",
  "target_selector": "Demon", "min_distance": 0.9,
  "window_s": 30, "driver_units_per_second": 20.0 }
```

- A selected pair sitting closer than `min_distance` is a
  VIOLATING observation; violating observations must persist for the
  rule's soak (`window_s`, below) before the episode they form is
  reportable — the episode fires, not the instantaneous pair
  (ε semantics: this is "coincident bodies", not "personal space" —
  the threshold must be below gameplay-speed closing distances or
  legitimate fly-bys false-positive; the soak discards the
  fly-bys).
- Episode = a maximal stretch of violating pairs; one finding per
  episode with the worst pair, both entity ids, and the minimum
  distance observed.
- **Episode folding, defined precisely** (the fold the §7 engine
  interface formalizes): violations are recorded per FRAME as
  (ordered id pair, frame, distance). Frames fold into one episode
  when consecutive; an episode ENDS on the first frame with zero
  violating pairs (or run end). Overlapping pairs do NOT split an
  episode — a maximal stretch is over TIME, not over pair identity
  (A–B violating frames 1–10 and C–D violating 5–15 is ONE episode,
  frames 1–15); splitting by pair would let a persistent stack
  masquerade as many short findings — the count must not lie about
  duration. The episode finding reports the WORST pair (minimum
  distance observed, tie-broken by earliest frame; ids of that
  observation) plus, in `_meta`, a bounded roster of the distinct
  violating pairs (first + worst + count per pair, capped) so a
  handoff A–B → A–C cannot hide the persistent offender A — but the
  finding is one.
- O(n²) over the selector; a cap with an explicit `sampled_pairs`
  disclosure in `_meta` when a population exceeds it (never silently
  subsample). The default cap and the sampling stride are TBD by the
  engine's cost model, with one hard requirement fixed now: sampling
  is SEED-DETERMINISTIC (stride derives from the scenario seed, like
  the chaos bot), because the digest/replay machinery compares runs
  pass-to-pass — differently-subsampled passes would be
  nondeterminism the harness itself introduced.
- `window_s` in this family is a SOAK, not a stagnation budget: the
  violation must persist for the window (in driver units, the
  inherited grammar) before the episode is reportable. The DRIVER
  LEAF carries over unchanged: spatial rules accept the same
  optional `driver_resource`/`driver_path` fields as
  `stagnant_resource`, and when absent the default is the driver
  loop's own elapsed time (seconds-denominated — the frame clock
  every run has), so a tick-domain game (the RTS) declares its
  driver explicitly exactly as it does today and `window_s` stays
  meaningful. The maximal-stretch episode therefore has a floor (episodes shorter
  than `window_s` are discarded as transient fly-bys — that is what
  the ε threshold discussion above guards) and no ceiling; this is
  0.15's `window_s`-as-budget reinterpreted, and the schema doc must
  say so in the rule's section. The pairwise ε rule stays as
  written: threshold below closing speed, soak discards the rest.

### `occupancy_cap`

```json
{ "name": "max_three_in_a_room", "rule": "occupancy_cap",
  "target_selector": "Demon", "radius": 2.0, "cap": 3,
  "window_s": 30 }
```

Generalizes the same family: at most `cap` selected entities within
`radius` of any point. The demon-stack litmus expressed directly
("no more than N of kind X within R of each other, sustained").

Ship-gate honesty: the round-1–round-4 evidence contains NO
measured crowding litmus — every stacked-bodies incident on record
routes to `pairwise_separation` — so under §8's acceptance
discipline `occupancy_cap` ships as grammar WITHOUT promotion:
the vocabulary lands with the family — it shares the FOLD and the
schema, but not necessarily the enumeration (pairwise enumerates
pairs; occupancy asks "at most cap within radius of ANY POINT", a
density query that wants a spatial hash, not a shared sweep) — and
its findings are WARN-ONLY by construction, exactly §2's
default-posture pattern: an unpromoted occupancy finding does not
fail the run; it warns, with the finding's `_meta` marking it
`unpromoted_rule`. Promotion follows §2's mechanism — the flip
— with the owner the park sim (crowds queuing at ride entrances
are the natural first litmus): a demonstrated pre-fix-RED /
post-fix-GREEN crowding incident on an owning repo's pin harness
promotes the rule to fail-power FOR THE HARNESS (per-repo evidence
proves the rule discriminates; once proven, it discriminates for
everyone — same as §2's sports-sim pile bug promoting
`derived_transform_consistency`). The acceptance table carries
the placeholder row while no such evidence exists.

### `spawn_safety`

```json
{ "name": "respawn_not_on_player", "rule": "spawn_safety",
  "target_selector": {"with_component": {"name": "Demon"}},
  "min_distance_from": {"with_component": {"name": "Player"}}, "min_distance": 1.5 }
```

(Selector spellings throughout this proposal use the bare-string
shorthand for readability; the SHIPPED wire form is the object
selector shown here, the same grammar as every other rule —
`min_distance_from` also admits `"all"`.)
Judges entities at their spawn moment only: a NEWBORN must satisfy
the distance predicate against a selector-selected anchor set. The
arena-shooter respawn-ring defect fires on the exact frame it
happens, with the distance observed. `forbidden_region` (below)
composes from the same anchor machinery with a shape instead of a
selector.

The "newborn" definition is where the subtlety lives, and it is
designed here (not deferred): the judge observes ARCHETYPE ENTRY —
an entity first acquiring the target component, or a newly-spawned
entity bearing it. Mechanically this is NOT bevy's
`Added<T>` query operator (a scheduled-query mechanism — the judge
sits outside the schedule by design, see below); it is a
GENERATION-TRACKED, FRAME-OVER-FRAME ARCHETYPE DIFF against the
post-update snapshot: the judge keeps the previous frame's
(entity id, generation, archetype/selector-membership) snapshot
and reports an entry when (id, generation) is new, or the same
(id, generation) newly bears the target component. Generation
tracking is what makes `spawn_only: true` survivable under entity
id reuse and what keeps component-additions on pre-existing
entities distinguishable from spawns. (AS BUILT, a precision the
implementation owes to the engine's own source: reuse soundness
is sealed TWICE OVER. The raw `EntityAllocator` does recycle
freed ids bit-identically without a generation bump — but games
and harnesses only despawn through the World/command paths, and
the World-path despawn bumps the generation before freeing
(`mark_free(index, 1)`), so every observable recycle arrives as a
NEW (index, generation) pair. And in the same-frame
despawn-then-spawn shape, `Commands::spawn` allocates eagerly at
QUEUE TIME, before any despawn command applies, so a same-tick
re-fill takes a fresh slot in the first place. Either way the
recycled re-entry is an unknown pair to the previous snapshot
and is judged anew.)
Three semantics follow and are fixed now, because each one
changes what the rule fires on:

- **Component-addition is the judged event, not spawn.** A PRE-EXISTING
  entity gaining the component (faction change converting a
  Villager into a Demon, archetype swaps, teleports) IS judged —
  the same-frame incoherence the rule exists for does not care
  how the entity came to bear the component. Authors who need
  spawn-only semantics opt out per rule
  (`spawn_only: true`, filtering to brand-new (id, generation)
  pairs only — the diff's generation tracking is what makes this
  sound); the default judges every entry, because a converted enemy
  spawning inside the player is the same bug.
- **Same-frame anchors count.** The anchor set is snapshotted at
  the END of the judging frame (post-update, the harness's
  standard observation point): a Player spawned in the same frame
  as the Demon participates in the judgment. Snapshotting the
  anchor set EARLIER would let a same-frame-spawned anchor excuse
  a violation, and the litmus (respawn ring centered on arena
  origin vs. player position) does not hinge on this — but the
  rule must not hinge on schedule luck either.
- **Scheduling: the judge sits in the driver-loop recorder tier**
  (post-update, same doctrine as the invariant recorders — never a
  scheduled system competing for `&World`), evaluating after
  propagation so GlobalTransform is settled. Frame N's component
  additions are visible at frame N's post-update observation —
  judged once, at the transition frame. AS BUILT: the shipped rule
  REJECTS `after_s`/`before_s` (both are informational-window
  semantics from the eventually-era, incompatible with a rule
  whose whole verdict is the single transition frame; a rule that
  can only fire once has no window to bound). (This
  judge-at-transition-frame doctrine
  is the frame-0-double-billing lesson from §8 item 5 applied to
  spawn safety.)

### `forbidden_region`

```json
{ "name": "no_rail_tunneling", "rule": "forbidden_region",
  "target_selector": "Skater",
  "region": { "shape": "box", "center": [0.0, 0.5, 0.0], "half_extents": [0.4, 1.0, 12.0] } }
```

An entity's GlobalTransform translation entering the region fires one
finding per incursion episode, WITH the entry frame and position. The
skate sandbox's rail-tunneling pin (currently a scripted/behavioral
pin) becomes declaratively judgeable.

(AS BUILT, the deviations and precisions the shipped rule adds to
this sketch: the wire selector is the object form,
`target_selector: {"with_component": {"name": "..."}}`, REQUIRED —
`all` rejects at load; the region carries no `shape` key — the box is
the grammar and unknown keys fail `deny_unknown_fields` — and its
half-extents must be finite and strictly positive, with containment
INCLUSIVE on every face (a translation exactly on a face is inside;
NaN translations are never inside, `finite_transforms` keeps owning
NaN findings). `window_s` is REQUIRED and is a SOAK in the driver
leaf's own units — the same both-halves-or-neither driver grammar as
`pairwise_separation`, frame-clock default in seconds, with the
declared-rate fit check at load — an incursion must LINGER, sub-soak
incursions being transient fly-bys, discarded. The "fires one finding
per incursion episode" promise is delivered by the episode-fold
machinery, not the literal sketch above, so the entry frame and
position arrive as the episode's evidence: worst penetration depth +
frame, per-distinct-entity capped roster with truncation disclosed,
`allow_if` exemptions resolved per entity per frame (an exempt
entity's roster participation ends at acquisition; the episode —
per-invariant — closes only when NO incursive entity remains, so a
survivor's soak spans the exempted period).
And the harness checks the entity's `Transform` translation — bevy's
canonical own-space position; a `GlobalTransform` read would import
propagation timing into a rule whose defect class (noclip tunneling)
survives propagation intact.)

The escape-hatch grammar deserves care — this is the lesson the first
draft of this example taught. `allow_if` (optional component/tag
exemption) exists for the "decorative cover you can walk through"
case: the author DECLARES that overlapping this region is legal when
a component is present, and the harness judges intent only where it
was declared. But the escape clause must exempt the LEGITIMATE
presence, not the bug: for rail tunneling the skater enters the
region's geometric interior — the rail runs ALONG its surface — so
the correct `allow_if` targets a component meaning "legitimately
mounted on the rail's outer lane" (`RailLane`), NOT `GroundedOnRail`:
grounded-on-rail is exactly the state the buggy run is IN while
tunneling, and exempting it would suppress the finding on the litmus
frame itself. Designing escape clauses therefore follows the same
flip-test discipline as §8 item 5: author the `allow_if`, then verify
pre-fix runs red and post-fix green — an exemption that flips the
pre-fix run green has exempted the bug, not the feature. Litmus 3
(decorative cover with no collision system) remains a *game* defect
judged only where intent is declared (§6); `allow_if` is the declared
side of that boundary, and its misuse (exempting the bug) is why the
flip-test is mandatory, not advisory.

Anti-goals, stated now: no physics-collision inference (the harness
does not know a collider from a decoration — that is Litmus 3's
lesson), no pathfinding/occupancy-graph reasoning, no
"plausibility of movement" scoring in this family (see §3 for why
liveness plausibility is deliberately NOT a spatial rule).

## 2. Rule: `derived_transform_consistency`

The highest-value single addition (ranked #1 in the sports sim's
cycle; the pile bug proved the class real, silent, and invisible to
every existing oracle):

```json
{ "name": "transforms_coherent", "rule": "derived_transform_consistency",
  "target_selector": "*" }
```

- One compositional identity over bevy's one canonical derived
  relationship: for every entity with a Parent, GlobalTransform ==
  propagate(Parent's GlobalTransform, own Transform).
- **Evaluation point: after `TransformPropagate`-equivalent
  settlement, at the driver-loop post-update observation** (the
  harness's standard). Crucially, the check compares against the
  PARENT'S GlobalTransform AS OBSERVED AT THE SAME FRAME — never
  the recomputed-from-root value — so the rule asks "is the stored
  pair internally consistent", not "did the game call propagate".
- **False positives are NOT near-zero without scoping, and the
  first draft's claim was oversold.** Two real FP sources, each
  with its designed answer:
  - Late-frame direct writes (camera smoothing, aim/look offsets,
    interpolation ghosts that write GlobalTransform after
    propagation) look momentarily incoherent EVERY frame. Answer:
    the default selector is `*`, but `derived_transform_consistency`
    carries the same `exclude` prefix grammar as the progress
    surface, PLUS a fast per-entity opt-out honored by any
    selection mode: a `SkipDerivedConsistency` marker component the
    harness registers (Reflect + register_type) — games tag their
    ghost/camera entities once and the rule ignores them.
  - ε is per-depth but must also be per-SCALE: ε(depth) grows with
    parent depth AND with the magnitude of the transforms involved
    (multiplicative slack: `ε = base × scale_factor^depth`, with
    `scale_factor` tied to the observed transform magnitudes) — a
    constant ε accumulates false positives down deep hierarchies
    AND at large coordinates. The exact growth function is tuned
    against the sports sim's hierarchy in implementation; the
    schema will document the landed formula, not promise a
    universal one here.
- Default posture: WARN-only until the pile-up incident class's
  pin proves the rule discriminates fix-present-green /
  fix-reverted-red on a real hierarchy (the §8 item 5
  flip-test discipline) — then promote to fail. An opt-in rule
  promoted on evidence, not optimism. PROMOTED: the harness's
  own battery demonstrates the flip-test — a multi-child pile
  (foreign-written globals under a moving parent) comes out RED
  with the full per-entity roster, the identical
  defect-free pile GREEN, and rotating/scaling hierarchies
  coherent — so `derived_transform_consistency` findings now
  gate the run.
- Finding carries both sides' values and the frame — the sports sim's
  `src/scratch_pile.rs` diagnostic (reverted, technique documented in
  its round-3 notes) is the working prototype for the evidence format.

## 3. Liveness plausibility tier (answering the skate sandbox's tunneling reward)

Problem: the liveness/progress oracles reward *change*, and noclip
through a rail produces spectacular, healthy-looking change. The
sandbox's interim verdict: "the liveness oracle rewards tunneling."

Design decision: do NOT bolt plausibility onto spatial rules (an
unboundedly game-specific rabbit hole). Instead, a plausibility TIER on
the existing progress oracle:

- A progress channel's changes earn plausibility credit when the
  channel correlates with input activity (verifiable by the harness:
  chaos-bot input presence/windows are known) AND the channel's value
  excursions respect authored bounds when the scenario declares them.
- Tunneling-style degeneracy is then caught by the combination the
  0.16 family enables: `forbidden_region` catches the act;
  progress plausibility merely stops *rewarding* it.

Concretely: `progress.plausible: true` opt-in marks a channel for
input-correlation bookkeeping; a channel that changes wildly while
inputs are idle (or vice versa: static while inputs pour in) is
annotated `implausible_change` / `implausible_stasis` — informational
by default. `implausible_change` escalates under strict_warnings (per
the window-majority rule below). `implausible_stasis` does NOT: the
change-side metric below is fully specified, but the stasis side is
deliberately left unspecifiable-for-now — what fraction of
input-active frames a static channel must span before annotation
is genuinely ambiguous (loading screens, cutscenes, deliberate
pauses are all legal static-under-input), and fabricating a
threshold would manufacture false positives. `implausible_stasis`
therefore ships INFORMATIONAL-ONLY — no strict_warnings escalation
path — with a deliberately extreme, explicitly conservative
trigger: a channel annotated for plausibility whose value is
bit-identical across a window made ENTIRELY of input-active frames
(the zero-change-all-window-under-input case — the only
stasis-under-input observation that no legitimate pause explains,
because loading screens and cutscenes are not wall-to-wall
input-active). Anything short of that extreme is not annotated at
all. Better an asymmetrical half-vocabulary with one honest
extreme than a fabricated threshold or dead vocabulary.

The change-side metric must be specified before this tier can be
costed, so it is specified here (the stasis side is settled by
the extreme-trigger decision above):

- **Input activity is a RUN-FACT, not an inference**: the harness
  knows every input event it injected (scripted or chaos), per
  frame. An input-active frame is one where the bot's injection
  schedule delivered ≥1 event (or a held key was active — held keys
  count per frame while down).
- **Correlation is per WINDOW, with lag allowance**: for each
  progress window (the progress oracle's existing windowing), the
  channel is scored on whether its CHANGES cluster inside
  input-active stretches. Allowance: changes within
  `lag_allowance_s` (default 0.25s, authored override) AFTER an
  input-active stretch count as correlated — physics carryover
  (momentum after key release, delayed sim effects) is the norm, not
  an anomaly, and the window's lag allowance absorbs it. A channel
  whose changes persist BEYOND the lag allowance while inputs stay
  idle is the `implausible_change` candidate. Schema placement:
  `lag_allowance_s` is per-PROGRESS-CONFIG (one field alongside
  the existing `liveness.progress` settings), not per-channel —
  channels within one progress config share an allowance, because
  the allowance models the game's input-to-effect latency, a
  property of the GAME, not of the channel; a per-channel escape
  exists if the shared value proves wrong for one marker
  (added only on demand).
- **Determinism ≠ validity.** Seeded runs reproduce identically, but
  a channel that legitimately lags input longer than the allowance
  (heavy physics settling, slow tweens) will be deterministically
  and FALSELY annotated — which is why the annotations are
  informational by default and the annotation text names the
  allowance ("changed for 1.2s after the last input; allowance
  0.25s — author tune `lag_allowance_s` if this is intended
  behavior"). The deterministic seeds remove measurement noise;
  they cannot remove intent ambiguity — only the author can, by
  tuning the allowance. The sandbox's tunneling case is exactly the
  complement: input went idle LONG ago, and the wild motion
  continued far past any plausible settle — that is the true
  positive the tier exists to name.
- **Per-window annotations, aggregated per-run**: each window
  carries its own verdict; the report's per-run summary counts
  windows annotated, and `strict_warnings` escalates only when
  annotated windows exceed a majority of COMPLETE windows (a single
  noisy window must not fail a run).

## 4. Held-trace parity: component tier enters `held_invariants`

Confirmed empirically on 0.15 (twice: skate sandbox and sports
sim), and now QUANTIFIED: a skate-sandbox scenario with three
passing invariants — one resource_check, one component_check, one
nodes_in_bounds — produced exactly ONE held entry (the
resource_check); the two component-tier passes were invisible. The
sand-box's suite carries five component-tier invariants against one
resource-tier — for spatial games the component tier is the
MAJORITY tier, so held traces cannot serve as a coverage story
until this closes (their verdict: blocks-adoption). Fixes:

- Extend the collector to every invariant tier; per-invariant record
  `{name, first_held_frame, matched_frames, worst/witness values}`
  — the data is already computed by the oracles and dropped for
  the resource tier (verified in `held.rs`); the component-tier
  claim has the same shape but only the resource tier's is
  code-verified today — the component-tier and `nodes_in_bounds`
  recorders need an audit as the implementation's first step,
  with "plumbing, not new computation" as the expected outcome
  rather than the assumed one.
  The NEW spatial/stagnation episode rules (§1, §7) generate
  their held evidence by construction (bounded episode logs
  below), not from oracle leftovers — the "already computed"
  claim applies only to the existing tiers.
- `spawn_safety`/`forbidden_region` add an episode log (bounded:
  first + worst + count) so a clean run still shows the contract was
  exercised — "zero incursions over 400 ticks" is itself evidence.
  For `spawn_safety` there is a second count that makes the
  difference between EXERCISED and NEVER-TRIGGERED visible: the
  held record carries the MATCHED-NEWBORNS COUNT (how many
  archetype entries the judge observed and cleared), because a
  clean run with zero entries cannot be distinguished from a
  never-fired rule by incursions alone — the coverage story of
  held traces depends on naming what was judged, not only what
  violated. AS BUILT (spawn_safety, the transition-only rule with
  no episode machinery to log): the exercise story rides the
  held record's entry counts — `entity_judgments` (ALL entries
  judged, once per entry) plus a new `entries_clean` field (the
  matched-newborns count this section demanded) and
  `frames_matched` (frames with ≥1 resolvable anchor, the
  would-have-judged existence evidence); a zero-entries run
  earns no held entry at all, making exercised-vs-never-triggered
  a structural distinction rather than a count comparison.
  `forbidden_region` (episode-shaped; AS BUILT, shipped) keeps the
  bounded episode-log plan above.
- The sandbox's predicted experiment (a passive run where the
  component-tier contract `skater_starts_centered` is held but absent
  from the trace) becomes the regression test.

## 5. Authoring-experience completions (dogfood-demanded)

Triage first — this is a menu with recommendations, not a
commitment list. [FIXED] items are done on the foundation
branch; [DOC] items are documentation work; [0.16-SHIP] are
recommended for the 0.16 release; [DEFER] are open questions or
larger work deliberately out of the 0.16 line. The "cheap" label
this section carried in earlier drafts was only honest for the
[DOC] rows.

1. **[FIXED] Did-you-mean for full paths** — short-name typos get
   `suggest_similar`; full-path typos (the form the error message
   itself recommends!) got nothing. FIXED on the foundation branch
   post-review: the final path segment routes through the same
   substring/Levenshtein helpers, and full-path queries get the
   candidate's full path back. (Independently reported by the RTS
   round-4 and the skate sandbox round-4 — the sandbox further
   suggests comparing path TOKEN-WISE (segments + generic args)
   rather than only the final segment; the landed fix covers the
   final-segment class, token-wise remains a refinement.)
2. **[DEFER — NOT cheap] Serde-grammar errors fold into lint
   aggregation** — `deny_unknown_fields` rejections occur at
   deserialization, before any struct exists to walk; aggregating
   ALL serde errors requires custom deserializer plumbing (a
   streaming/visitor-based reader) — a project, not a completion.
   The first-error-only serde report (full candidate list,
   line/column) is honest behavior; folding is deferred until
   demand beyond "would be nice" appears. (Sports sim ranked it #4.)
3. **[FIXED] Stage-1 lint stderr double-print** — findings printed eagerly
   after stage 1 and again after stage 2. FIXED on the foundation
   branch post-review: one print loop, after both stages contribute.
   (RTS round-4.)
4. **[0.16-SHIP — schema decision needed] Informational entries
   ride `violations[]`** — naive consumers counting the array
   misread green runs; consider a top-level `informational[]`.
   Schema note: `held_invariants` established the additive
   skipped-when-empty doctrine (an OPTIONAL top-level field does
   NOT bump `schema_version`); MOVING entries out of `violations`
   is the first removing change and DOES bump the schema —
   annotate-and-document-loudly is the additive medicine,
   `informational[]` the bumping one; pick explicitly before
   landing. (RTS round-4.)
5. **[FIXED (doc); open tolerance question] Off-by-one driver budget note** — a `duration × tps` run
   observes `duration × tps − 1` driver deltas, so a window authored
   at exactly `duration × rate` is unjudgeable by one unit. Document
   ("budget windows at `duration × rate − 1` or smaller"); consider
   whether the audit should tolerate the one-unit shortfall for
   declared-rate windows. (RTS round-4, confirmed via
   `held_invariants.driver_advanced`.) The schema doc now carries
   the budget-headroom note; the tolerance question stays open.
6. **[DOC] Manual-run env recipe** — manual lint/run invocation
   without the battery's injected `BEVY_ASSET_ROOT` makes bevy's
   asset server fall back to the executable directory and collect
   a spurious `log_error` ("Path not found") for games with
   relative assets. Document the recipe (battery users unaffected);
   consider a lint convenience that inherits the manifest dir.
   (Skate sandbox round-4.)
7. **[0.16-SHIP, cosmetic] `last_detail` duplication on lint findings** — degenerate 0–0
   frame range duplicates `detail` verbatim; a lint flavor could
   drop the redundant field. (Skate sandbox round-4, cosmetic.)
8. **[DOC for 0.16; DEFER the feature] Driven-motion passive
   mode** — `passive: true` cannot drive an input-triggered bug
   (rail tunneling needs a held accelerate key); document the
   observe-only boundary sharply in the tutorial for 0.16. The
   seeded-chaos observe mode is deferred (workaround-exists: the
   scripted scenario stays available). (Skate sandbox round-4.)
9. **[FIXED] Stage-2 progress-surface probe** — a typo'd
   `liveness.progress.markers` or exact `include_paths` entry
   passed lint clean and only failed a real run, because the
   probe gated on probeable *invariants* and never booted.
   FIXED on the foundation branch post-review: markers and exact
   include entries now trigger and feed the probe (reusing the
   recorder's own resolution flow so wording cannot drift), with
   did-you-mean on the typo; prefix (`foo::`) entries stay the
   run's to grade. (Park-sim round-4, their ranked #1 — the only
   lint-vs-run divergence they found.)
10. **[0.16-SHIP] Suppress consequence-chain diagnostics in lint stage 1** —
   after a root rejection (`tps must be >= 1`), downstream
   dependent checks still fire and render nonsense (`infs` for
   divisions by the invalid tps); gate dependent validations on
   their inputs being sane. (Park-sim round-4.)
11. **[DOC] Where lint aggregation begins** — a malformed file
    (unknown top-level field) dies at deserialization before the
    walker: exit 3, first serde error with the full candidate
    list. Fair behavior, but "all errors at once" starts where
    serde succeeds — say so in the docs. (Park-sim round-4.)
12. **[Release logistics, not harness code] Publish a 0.15.x (or pre-release tag) to untrap `[patch]`**
    — the locked 0.14 dependency cannot be raised by
    `[patch.crates-io]` (semver floor), so dogfooders following
    the "just patch it" instinct get a silent non-override; a
    path dependency is the workaround, and the release checklist
    should call the trap out. (Park-sim round-4, release
    logistics rather than harness code.)
13. **[DEFER] Per-window report artifacts for `stalled_progress`** — a
    "churning-but-dead" finding currently names only the union of
    tickers and the first/last window start frames; carrying each
    window's verdict and the surface entries evaluated per window
    would make triage much faster. (Park-sim round-4, ranked #4.)
14. **[0.16-SHIP] Did-you-mean for `driver_path`/`path` LEAVES** — stage 2
    validates resource names; extending the same treatment to
    path components (a typo'd `months_elapssed` →
    `months_elapsed`) would complete the author-experience story.
    (Park-sim round-4, ranked #5.)

## 6. What 0.16 is NOT

- Not a general assertion language. Rules stay compositional and
  closed; `population_stat_check` (min/max/mean/quartiles over a
  selector's numeric leaf) is deferred until §1–§2 prove the tier.
  "Prove" has a measurable definition, fixed now: the spatial rules
  and `derived_transform_consistency` are proven when (a) each
  §8 litmus they target flips fix-present-green /
  fix-reverted-red on the owning dogfood repo's pin harness, and
  (b) a full battery run on the shipped example games — the
  `fixtures/` corpus: alien_cake_addict, breakout, contributors,
  desk_toy, game_menu, known_answer, loading_screen, mines,
  thetawave — stays violation-clean (no false positives on
  non-defective games). Honest caveat on coverage: the corpus's
  deep-hierarchy exercise is THIN (desk_toy's parenting is the
  only real one) — the sports sim's game hierarchy is the real
  §2 deep-hierarchy negative test, so criterion (b) for §2
  includes the sports sim's battery alongside the fixtures.
  Timing note: for rules in their WARN phase (§2's default
  posture, `occupancy_cap` unpromoted), "violation-clean" means
  WARNING-clean — during the warn phase there are no violations
  by construction, so (b) is only fully meaningful at/after
  promotion; warn-phase runs of the fixtures must show zero
  warnings attributable to harness misjudgment.
  Until both hold, `population_stat_check` stays deferred.
- Not collision-physics. Litmus 3 (decorative cover with no collision
  system) is a *game* defect the harness can only judge once the
  author declares intent (`forbidden_region` + `allow_if`), never by
  inferring solidity.
- Not a movement-plausibility judge. Correlation tier only (§3).
- Not predictive. Everything is evaluated over observed frames;
  no "the entity would have crashed" counterfactuals.

## 7. The convergence on a shared entity-scoped engine

Two independent per-family design reviews (the park sim's
`entity_stagnation` spec and the spatial-family spec) converged on the
same architecture from opposite directions — not on every detail, and
the honest statement of what converged is narrower than "ONE
engine":

> the right architecture is **one entity-scoped evaluation ENGINE —
> selection → enumeration → episode folding → bounded `_meta` —
> hosting two rule vocabularies** (spatial consistency and entity
> stagnation), with the PL/PLAUSIBILITY TIER (§3) and the HELD-TRACE
> COLLECTOR (§4) as adjacent subsystems consuming its output, not
rule families living inside it.

What the convergence buys is a shared FOLD INTERFACE: both
vocabularies select entities, enumerate observations per frame, fold
observations into episodes with identity rules, and report bounded
evidence. That is 0.16's real architectural claim, and it is a
weaker, more useful claim than "one engine for everything" — the
judgement layer of 0.16 is this shared fold engine plus two rule
vocabularies plus the two adjacent subsystems. Calling everything
"one engine" would obscure where work actually lives:

| Piece | New reflect machinery | New vocabulary |
| --- | --- | --- |
| Spatial pair/region rules (§1: separation, occupancy, spawn_safety, forbidden_region) | none | shapes, anchors, `allow_if`, thresholds, opt-outs |
| `derived_transform_consistency` (§2) | none (Transform/GT already registered) | parent-pair enumeration, `SkipDerivedConsistency` opt-out |
| Entity stagnation (below) | variant-qualified paths, enum digests | `leaf_where`, scoped watch paths, flat `driver` discriminator |

Note the second row: §2 is a THIRD vocabulary — a hierarchy-consistency
rule — sharing the fold interface without being either spatial or
stagnation. Sequencing keeps it second-class to §1 only in ordering,
not in honesty about its shape.

The park sim's spec supplies the second vocabulary's full
semantics, review-corrected: episode identity =
(entity id + generation, watch-expression, continuous match), with
**despawn-mid-watch = acquittal** (never punish legal turnover),
`leaf_where` preconditioning (select "entities whose `state` is the
`Queued` variant"), change-clears-stagnation over a SCOPED path
(watch `state`, not the whole `Guest` — mutating needs defeat
whole-component watches), flat `driver` string (`global_time`
default, or `"resource"` reusing the driver fields verbatim),
sampled evaluation with a `window_s ≥ 2 × sampling_s` floor, and
aggregate-first reporting with a capped `_meta` roster. Its honest
novelty accounting: five engine parts (variant paths, `leaf_where`
predicates, enum change digests, per-entity episode windows,
roster `_meta`) — of which the reflection-layer pair is zero-vocabulary
engine work. Its litmus: the queue-on-dead-ride soft-lock (in-transit
arrivals orphaned by demolish; `waited` frozen at 0.0; immortal
guest that every existing rule family is structurally blind to —
world churns, resources flow, no panic, no error log).

Its documented blind spots carry over unchanged and deliberately:
oscillating livelocks (re-decide loops) need cycle detection — a
different engine, deferred; per-entity ledger deltas; population
statistics (deferred with §1-§2 priority). "The author chooses the
blast radius; the engine cannot know that hunger ticking isn't
progress" — the docs must say so plainly.

Sequencing recommendation: implement the shared fold engine
(selector extension points, episode fold interface, roster `_meta`
shape) once, land the spatial rules and `derived_transform_consistency`
first (neither needs new reflect-layer machinery — they prove the
engine on the flattest vocabulary), then entity_stagnation as the
second vocabulary, which brings the variant-path/enum-digest
reflection-layer work with it.

(AS BUILT, the shipped `entity_stagnation` and where it had to
deviate from or go beyond this paragraph: the reflection layer's
variant-qualified watch paths needed a bespoke `scoped_watch_leaf`
reader — bevy 0.20's reflect `GetPath` does not dispatch into enum
variants, so `watch_path: "state.Queued.waited"` is resolved by
walking `path`, variant name, and field manually; the leaf digest is
BITWISE-canonical per entity (f32 `to_bits()` — -0.0 → +0.0 is a
change, distinct NaN payloads distinct values) with despawn-ACQUITTAL
mid-stint (banking
the accrued span, never refunding it), and a RECYCLED slot is a new
(index, generation) episode key — turnover cannot weld a successor
to its predecessor's stint. The sampled cadence (`sampling_s`,
default 1.0) is scheduled in FRAMES via the driver rate —
`window_s >= 2 * sampling_s` is enforced at load, and a sub-frame
cadence rejects (it would collapse to repeated identical samples).
The flat `driver` discriminator is exactly as specced:
`global_time` default, `"resource"` reusing the driver fields
verbatim, both-halves-or-neither. Episode identity is the UNION
TIMELINE over stuck entities — one episode per invariant, a
stretch where ANY selected entity is stuck, spanning sampled gaps
where nothing qualified — so the immortal guest's finding is the
episode's worst-span witness, not per-entity noise; the `_meta`
roster carries per-entity (index, generation, first/last frame,
accrued span) worst-first with disclosed truncation. And one
split the spec did not draw: VACUITY BY NON-RESOLUTION (selector
never resolved a checkable entity → `vacuous_invariant`) is
deliberately separated from a RESOLVED-but-never-qualifying watch
(a `leaf_where` that was never true, or a watch that judged every
sample and saw nothing stuck) — the latter is a PASS WITH
EVIDENCE: the held trace keeps the entry, zero judgments or
per-sample digest workload as the case may be, because a live
watch the world never offered the qualifying state is coverage,
not vacuity. Sampling-schedule and relay-union doctrines are the
review's two veto points; both stand as shipped.)

## 8. Evidence appendix — the measured failures this design must catch

1. Sports sim: players converge to one point; GlobalTransform
   disagrees with Parent composition; possession assigned then frozen
   for five clock-minutes under zero input (the idle-input scenario
   class — deterministically exposes "match never develops" — earns
   its keep on day one; already expressible via `passive: true`).
2. Arena shooter: 6 demons at 40 DPS each stacked on the player
   (~240 DPS contact spiral, all oracles green); respawn ring
   centered on arena origin allowing spawn-in-attack-range;
   negative health reset regression pinned at the fix site (out of
   harness scope by design — it is covered by the same pin-harness
   pattern as item 5's fix-and-flip methodology, not by a harness
   rule; the harness's contribution there is making the pin cheap
   to maintain, not judging it).
3. Skate sandbox: grounded rail tunneling reads as maximal liveness;
   component-tier contracts absent from held traces.
4. RTS: tick-domain windows (fixed by the 0.16-line driver-units
   work this design rides on); full-path typo without suggestion
   (FIXED on the foundation branch); stage-1 stderr duplication
   (FIXED on the foundation branch); the 399-of-400 budget quirk
   (documented in the schema doc; tolerance question open, §5
   item 5).
5. Park sim: the pin methodology itself — fix-and-flip discrimination
   (proven on the frame-0 Calendar double-billing: pre-fix
   `upkeep_total = 208` at frame 0, post-fix 0), meter fields exposed
   as reflected resources exactly where oracles can judge them
   (`stuck_queue_guests`), and the frame-0-double-billing
   time-of-check lesson feeding `spawn_safety`'s
   judge-at-transition-frame semantics. Plus the second litmus class
   from §7: the queue-on-dead-ride soft-lock — guests in transit
   toward a demolished ride arrive, enter `Queued`, and wait
   forever (`waited` frozen at 0.0, patience loop iterates live rides
   only, gate-only despawn) while the park looks perfectly alive.
   Their fix-and-flip pin harness and the regression-contracts doc
   are the working pattern for how games adopt each new rule family.
6. Skate sandbox round-4 quantification: three passing invariants
   (one per tier: resource_check, component_check, nodes_in_bounds)
   → exactly ONE held entry. The component tier — five of their
   seven scenarios — leaves zero pass-side evidence. Their pin
   harness keeps the tunneling finding red; the liveness oracle
   still cannot see it (motion-blind, unchanged on the branch);
   window-semantics migration needed zero scenario edits (seconds-
   denominated driver; `window_unelapsed` made the misfit case
   observable without any edit).
7. Park sim round-4 (foundation-branch validation): 38/38 green,
   ZERO scenario edits across the window-semantics rework — their
   months-denominated `stagnant_resource` window kept its meaning
   because the driver leaf was already authoritative under 0.14;
   `driver_units_per_second` adopted as an optional strengthening
   (declared-rate run stays green; a deliberately mis-declared
   rate rejects with a numerically truthful message in both run
   and lint modes). All three regression pins discriminate
   byte-identically to round-3. Lint stage-1 aggregation verified
   (six findings in one pass); stage-2 did-you-mean verified for
   full-path and module-typo cases — and their one real gap
   (progress-surface names escaping the probe) is fixed on the
   branch (§5 item 9). Plus the standing trap: `[patch.crates-io]`
   cannot raise the locked dependency's semver floor — path
   dependency or published release required.

### §8 acceptance table — every litmus, its catching rule, and the flip expectation

The narrative above is evidence; this table is the acceptance
criteria. Every litmus bug names the rule that catches it and the
fix-and-flip expectation the owning repo's pin harness must
demonstrate before 0.16 ships the rule (M1's lesson: a rule
example that exempts its own litmus must not survive to the plan).

| Litmus (owner) | Catching rule | Pre-fix expectation | Post-fix expectation |
| --- | --- | --- | --- |
| Demon stack, 240 DPS contact spiral (arena shooter) | `pairwise_separation` | RED: one episode, min distance ≈ 0, both ids | GREEN |
| Respawn ring spawns in attack range (arena shooter) | `spawn_safety` (archetype-entry semantics, same-frame anchors) | RED: violation at the spawn frame with distance | GREEN |
| Grounded rail tunneling (skate sandbox) | `forbidden_region`, NO `allow_if` on `GroundedOnRail` (exempting the tunneling state would exempt the bug — §1's flip-test) | RED: incursion episode with entry frame + position | GREEN (legitimate rail-surface presence, if any, exempted by a correctly-authored clause that keeps pre-fix RED) |
| Decorative cover, no collision (arena shooter, Litmus 3) | none — judged only via declared intent (`forbidden_region` + `allow_if`); game defect | n/a (out of harness scope, §6) | n/a |
| Player pile-up; GlobalTransform ≠ Parent composition (sports sim) | `derived_transform_consistency` — PROMOTED on the harness battery's flip-test (multi-child pile RED with full roster, propagated pile GREEN, rotating/scaling hierarchy coherent) | RED: both sides' values + frame | GREEN — demonstrated |
| Possession frozen five clock-minutes under zero input (sports sim) | expressible TODAY: `passive: true` + `stagnant_resource` — ALREADY DEMONSTRABLE, free to verify now, not future work | RED on the frozen resource | GREEN |
| Queue-on-dead-ride soft-lock: immortal queued guests (park sim) | `entity_stagnation` (`leaf_where state==Queued`, scoped `waited` watch, `global_time` driver) — SHIPPED, pinned by the integration battery (17 tests: the litmus fixture RED with per-entity `_meta` roster, relay/id-reuse, cadence, vacuity split) | RED: stagnation episode per guest, `_meta` roster | GREEN (despawn or state change acquires the episode) |
| Crowd congestion (park sim, candidate owner — NO measured litmus yet on record) | `occupancy_cap` — ships as grammar, unpromoted (§1): the park sim's queued-at-entrance crowds are the natural first owner; until a crowding incident demonstrates pre-fix-RED / post-fix-GREEN, the rule carries no fail-power claim and blocks nothing else | PLACEHOLDER: awaiting an owned crowding incident | PLACEHOLDER |
| Camera smoothing / ghost-entity late GT writes (FP guard, all repos) | §2's opt-outs (`exclude` prefixes, `SkipDerivedConsistency`) | n/a — NEGATIVE acceptance: battery games must stay violation-clean (part (b) of §6's "prove" criterion; for WARN-phase rules read WARNING-clean, per §6's timing note) | n/a |

A rule ships only when its row's two expectations are demonstrated
on the owning repo. Rows that cannot be demonstrated block the
rule they name — not the release of the other rules. A
placeholder row (a rule landing as grammar with its expectations
undemonstrated) is the declared exception: the rule ships
WARN-ONLY, unpromoted, its findings carrying the
`unpromoted_rule` mark, and the row's demonstration is the
promotion event that removes the mark.
