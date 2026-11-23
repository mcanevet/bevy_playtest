# Reference

Public API surface for integrators.

- [Scenario (JSON schema)](scenario-schema.md) — the full grammar:
  bot types, liveness, invariants, chaos mouse/keyboard behavior,
  path and driver authoring (composite leaf paths, engine clock
  drivers, opaque leaves, window units), and every class of
  load-time rejection
- [Oracles](#oracles) — what watches a run, and each rule's evidence
  and honest boundary
- [Report](report.md) — every `PlaytestReport` JSON field, what each
  carries, and when absence means "not evaluated" rather than
  "healthy"
- [Battery](battery.md) — entry points (`run_scenario`,
  verified replay, JUnit output), the `run`/`run_and_expect_*`
  helpers, the plugins, and the takeover runner's exit codes
- [Environment variables](environment.md) — every `BEVY_PLAYTEST_*`
  var, and which side of a battery run reads it
- [SurfaceEntry schema](surface-entry-schema.md) — the
  Leafwing-reconcile intermediate representation carried by
  `lw_reconciled`
- [Deterministic boot](deterministic-boot.md) — the "why + how to fix"
  target for `replay_divergence` messages: seeding your game's RNG,
  and common causes beyond the canonical three (persisted state,
  network sockets)
- [Known-answer fixture](known-answer-fixture.md) — the planted-truth
  fixture: hand-authored input surface, committed ground truth, and
  the measurement/mutation harnesses that grade the declared tier
- [Judgement-litmus fixture](judgement-litmus-fixture.md) — the planted
  spatial defect: a six-demon contact pile the `pairwise_separation`
  rule must catch as one folded episode, plus the fly-by control
- [Thetawave fixture](thetawave-fixture.md) — the real-world fixture:
  a ported third-party game (Theta Wave, MIT) proving the declared
  tier against actual Leafwing usage

## Oracles

- **NaN/finite translations** — any non-finite `Transform.translation` →
  Fail (rotation/scale are not checked)
- **Runaway geometry** — with nothing declared, an entity outside a
  global ±1000 box on x/y/z that keeps moving away for 0.5 s (e.g. a
  projectile that is never despawned) → Fail; far-but-static level
  geometry and objects coming in from afar are not flagged. Bounds
  declared in an authored scenario are checked strictly, per axis
- **Frozen world** — no movement of any entity (or UI change, under
  `ui-liveness`) over the liveness grace
  window (default 2 s at 60 tps) → Fail (catches soft-locks). With
  `record_digest: true`, a silent streak where the state digest kept
  changing reports "stepping but visually silent" (the sim is running
  while nothing visible moved — presenter layers must write
  Transform/UI to be observable) instead of "soft-lock?"; see
  [Why liveness is time-boxed](../explanation/why-liveness-is-time-boxed.md).
  Phantom
  writes — every frame `Changed<Transform>` fires while the transform
  values stay bit-identical across sampled intervals (a world
  rewriting itself without changing) — defeat the change-streak and are
  caught by a value-hash interval check (30-frame intervals) instead,
  reported once per crossing with a "phantom writes" detail. The
  interval length is a fixed property of the detector — it trades
  detection latency for per-sample cost and is deliberately not
  scenario-configurable. The hash
  covers translation and rotation and scale, so orientation/scale
  animation is genuine change, never phantom; a softer "mostly
  phantom writes" variant fires when ≥90% of interval frames (not
  necessarily all) rewrite bit-identical values, resetting only on a
  genuine value change.
- **Progress heartbeat** (`liveness.progress`) — the
  churning-but-dead complement to frozen world: the frame loop stays
  hot and transforms jitter, but no game-verifiable state evolves over
  the window (default 8 s). Sampled every `sample_s` (default 0.25 s);
  a window is stalled when it churned at every sampled frame while no
  watched channel showed a non-ticker change. Resources that change in
  ≥90% of sampled transitions are tickers (heartbeats, frame counters)
  and are suppressed — the stalled detail names them. Entity/archetype
  counts are recorded for breadth but never count as progress
  evidence: spawn churn is engine bookkeeping, not state evolution.
  Channels: auto mode watches reflected component types plus
  `include_paths` resources (minus `exclude`); markers mode watches
  only the named reflected resources; `off` disables the oracle entirely.
  One design boundary worth reading twice: the zero-config built-in
  battery's auto-mode surface is COUNTS-ONLY — component-type counts,
  not component values. Structural change (spawns, deaths, archetype
  moves) is visible; value mutation on stable structure (an economy
  growing, units repositioning, meters filling) is not. Games whose
  state evolves purely in value-space hit `stalled_progress` on the
  bare battery without anything being wrong — the remedy is an
  authored scenario declaring `include_paths`/`markers` naming the
  reflected resources that carry the live state, which adds VALUE
  channels (each resource's canonical value hash, compared per
  sample), never a change to the game.
  Authored configs are held to a coherent sampling grid:
  `sample_s·tps` must be a whole frame count and `window_s` a whole
  multiple of it (the default config rides along unvalidated and is
  re-quantized coherently instead). When nothing
  game-verifiable is in scope, the report gains an informational
  `progress_surface_empty` note, escalating to Fail under
  `strict_warnings: true` or `progress.require_surface: true` (the
  built-in battery scenario sets `require_surface`). Verdicts require
  at least one complete window, so runs shorter than `window_s` get
  samples but no progress verdict. Per-run samples are exported in the
  report's `progress_samples` series.
- **Panic capture** — crash → Crash (in-process)
- **Vacuity guard** — if no entity with a `Transform` (or, under
  `ui-liveness`, a `UiTransform`) existed at any point during the run,
  a Pass proves nothing; it fails loudly instead (the criterion is
  any-frame, so a game whose entities despawn before the end is not
  flagged)
- **Log capture** — ERROR → Fail, WARN → informational (unless
  `strict_warnings: true`). A log→tracing bridge is installed
  alongside the capture subscriber, so events emitted through the
  `log` crate — bevy's B000x hierarchy validators (B0001/B0004...),
  wgpu/naga, asset loaders, any log-crate user — are captured too
  (they are invisible without a bridge: bevy's validators speak
  `log`, not `tracing`). If the bridge loses the write-once `log`
  logger slot to a game-provided backend, the report gains an
  informational `log_bridge_lost` violation.
- **Log level defense** — while the harness bridge owns the `log`
  logger slot, the `log` max level is harness-owned state for the
  duration of a run: the driver re-asserts it at run start and at
  every tick, overriding any lower level the game set (deliberate
  verbosity curation included — a game that silenced a chatty
  dependency's `log::warn!`s will see them surface as
  informational `log_warn` findings). A one-time foreign
  `log::set_max_level(Off)` thus blinds at most the remainder of the
  tick it lands in; an `Off` re-applied every tick cannot be
  repaired (each tick's tail stays lost) and is detected instead,
  reported as an informational `log_level_tampered` violation whose
  count is the number of tampered ticks.
- **Render liveness** (`render-liveness` feature) — informational,
  never gating: a sim-active run whose game has a render sub-app
  (the `render_liveness` report field stays absent otherwise — "not
  evaluated" never means "healthy") but whose cameras never existed,
  were all disabled (`is_active == false`), or had no active camera
  writing output (every sampled active camera sat on
  `CameraOutputMode::Skip`, which may be intentional for
  compositing/prepass setups; a disabled camera's mode is
  irrelevant — it renders nothing) gains an informational
  `render_liveness` violation. The rule reads only main-world
  `Camera` components, never the render world or the
  GPU — a live camera that renders black passes. Evidence (tier,
  `cameras_seen`, `samples`, `sim_active_ever`, notes — collected
  at cadence-gated samples only, not continuously) rides the
  report's `render_liveness` field;
  see the [headless-CI how-to](../how-to/headless-ci-gpu.md) for the
  promise boundary.
- **Screen liveness** (`screen-liveness` feature, implies
  `render-liveness`) — informational, never gating: pixel
  observation where `render_liveness` is structural. The harness
  spawns its own marker-tagged `Camera2d` (only when the game itself
  has a 2D camera — a purely-3D view cannot be faithfully mirrored)
  and requests a screenshot every `tps/2` ticks; captured frames are
  box-filtered to an 8x8 luma grid and hashed, consecutive hashes
  compared by Hamming distance. A sim-active run whose captured
  screen never changed across every consecutive capture pair gains
  an informational `screen_liveness` violation ("pixels did not
  flow despite a stepping world"; may be intentional — menus,
  dialogue, level-load art). Honest boundaries, fail-open:
  a run with no 2D game camera at any cadence tick is labeled
  `not_evaluated_3d` (never guessed), and fewer than two usable
  captures (ultra-short run, or GPU readback failure) come out
  `insufficient_captures` — the question is refused, not guessed.
  The rig requires the render sub-app and an `Assets<Image>` store;
  absence of either means the `screen_liveness` report field stays
  absent — "not evaluated", never "healthy". The 8x8 grid has a
  documented under-sensitivity floor: sub-cell motion (one cell is
  ~40x30 px at the 320x240 capture size) is invisible by design;
  live tiers emit nothing. Evidence (tier, requests, captures,
  `max_hamming`, `sim_active_ever`, notes) rides the report's
  `screen_liveness` field.

## Compatibility

Targets Bevy 0.20.

## Keeping this reference current

These pages document the shipped grammar, not a plan. Two
disciplines keep them from drifting:

- The scenario reference carries an **applies-to stamp**; update the
  stamp — and the page — in the same PR that changes the behavior it
  documents, the same rule the changelog follows. A released rule
  whose documented example no longer loads is a bug.
- Every strict JSON example in these pages is **parsed and
  validated against the current grammar** by a CI test
  (`doc_examples`), so prose examples cannot rot silently. Mark a
  fence that is illustrative-only with `<!-- doc-lint: skip -->` and
  one that teaches a load rejection with
  `<!-- doc-lint: rejected -->` (renders unchanged in markdown).
  Quoted failure texts are not machine-checked; keep them short and
  grep-distinctive so drift is easy to spot.
