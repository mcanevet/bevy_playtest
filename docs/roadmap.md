# Deferred items — reasons and trigger conditions

Consolidated ledger of accumulated deferrals. Each entry: what, why
deferred, and the trigger that would justify picking it up.

## named(name) invariant target selector

- **What**: `target_selector: "named(PlayerTag)"` restricting
  invariant evaluation to entities carrying a specific component.
- **Why deferred**: requires Bevy TypeRegistry registration of the
  target type at harness level; the registration ceremony is exactly
  the per-game setup the zero-annotation `all` selector eliminates.
- **Trigger**: user demand for selective invariants on entity-heavy
  worlds where `all` is too coarse, paired with a low-friction
  registration story (e.g. a `register_targets!` macro in the
  PlaytestPlugin builder).

## Semantic postcondition contracts for cheat hooks

- **What**: `expects: {"resource_absent": "MyState"}`,
  component-value assertions after hook invocation.
- **Why deferred**: reflection wiring is not in place; current
  contracts are statistical (transform-entity counts, `lt`/`gt`/`eq`)
  and provable without game cooperation. Value-level checks would
  need reflected access and a type-mapping DSL — same bucket as
  `named()`.
- **Trigger**: same as `named()` — TypeRegistry integration point.

## Shared oracle core across in-process driver and BRP runner

- **What**: one evaluation core serving both `driver.rs` (typed
  queries) and `brp_runner.rs` (wire-shaped HashMap rows).
- **Why deferred**: the two operate on different data shapes; a
  shared core would need an intermediate representation. Assessed
  during round 3 — the abstraction cost exceeded the duplication
  cost at the current oracle count.
- **Trigger**: a third driver surface appears (e.g. replay-file
  playback), or the oracle count doubles past ~12 rules.

## BRP replay verification

- **What**: `verify_replay` in external-BRP mode.
- **Why deferred**: BRP mode paces inputs over real wall-clock time
  against a separate process; honest replay verification is
  impossible without deterministic clock injection across the wire.
  The BRP report correctly rejects the field today rather than
  claiming an unverifiable pass.
- **Trigger**: the harness controls game time scale end-to-end
  (pause/step parity), making two BRP passes comparable.

## Pursuit/persona bots

- **What**: goal-directed bot policies beyond chaos
  (pursue-nearest-target, scripted personas).
- **Why deferred**: chaos + synthetic_keyboard cover the current
  verification-depth goals; persona bots are an authoring surface,
  not an oracle surface.
- **Trigger**: user studies showing chaos misses the failure modes
  users actually care about (e.g. navigation bugs requiring
  goal-seeking to expose).

## Scheduled cheats

- **What**: cheats firing at arbitrary frames mid-run, not just
  frame 0 pre-loop.
- **Why deferred**: current cheats all apply pre-run (frame 0);
  mid-run application interacts with the replay contract (cheats
  participate in both passes and must be re-applied at the right
  frame in pass B).
- **Trigger**: replay verification stabilizes (shipped this round);
  mid-run cheats then slot into the same two-pass contract.

## "Eventually" mode

- **What**: scenario-level temporal operators — "eventually the
  player dies" rather than frame-scoped invariants.
- **Why deferred**: requires a semantics for
  satisfaction/violation windows in the report format (when did the
  event fire, was it in-window), a report schema bump.
- **Trigger**: the `observes` coverage surface (shipped this round)
  proves insufficient — users needing temporal logic on top of
  observation matching.

## Upstream BRP swap

Already documented in `src/brp/`: the vendored client speaks a
subset of the BRP protocol; the plan is to swap to an upstream
maintained client when one covers `rpc.discover`, method
registration, and reconnect semantics. Keep the seam in
`src/brp/` narrow to make that swap mechanical.
