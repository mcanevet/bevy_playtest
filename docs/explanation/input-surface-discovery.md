# Input-Surface Discovery via Differential Probing

This page explains the design for **input-surface discovery**, a feature that automatically discovers which input keys/buttons a Bevy game actually consumes—without requiring game authors to declare them upfront.

## Motivation

bevy_playtest's chaos bot randomly presses keys from a declared `raw_surface`. But Tier-0 games (unmodified Bevy showcases) don't declare anything. Manual inspection defeats the zero-annotation promise.

**Discovery answers causally:** input X matters in state S *iff* the per-frame state digest diverges between "press X at frame F" and "do nothing at frame F", starting from the same deterministic prefix.

## Architecture Overview

As shipped, discovery is a **report-annotation step that runs AFTER
the battery's game child completes** — it never touches the run, and
the game child never even sees the opt-in variables (they are stripped
from every spawned child; the contract is parent-side only).

```
┌─────────────────────────────────────────────────────────────────────┐
│  Battery Run Phase (the declared scenario, unchanged)                │
│  ───────────────────────                                             │
│  The game child runs the DECLARED scenario UNCHANGED — the           │
│  discovered surface is never injected into the bot config.          │
│  If scenario.bot.raw_surface was EXPLICITLY DECLARED, that          │
│  declaration is what the bot itself used — discovery still          │
│  annotates the report (declaration wins; discovery describes).      │
└─────────────────────────────────────────────────────────────────────┘
                              │ child completes, report parsed
                              ▼
┌─────────────────────────────────────────────────────────────────────┐
│  Discovery Phase (opt-in: BEVY_PLAYTEST_DISCOVER=1, parent-side)     │
│  ───────────────────────                                             │
│  1. Cache lookup keyed by (game-binary-hash, bevy_playtest-version,  │
│     feature-fingerprint, fixture-identity, probe-plan-fingerprint): │
│     a fresh hit returns the persisted surface without spawning.     │
│  2. Probe matrix: subprocess pool (6-8 parallel game processes)      │
│     - Control run ×2 (determinism gate; same shape as probe minus   │
│       the key). The FIRST control also exports the static device-   │
│       reader scan (child-side capture at harness-setup time).       │
│     - For each candidate key: tap at readiness + mid-duration      │
│       (one probe point per run — independence requirement)         │
│     - Pool completeness gate: refuses reports short of the plan's   │
│       expected final frame (UnusableReport), never a vacuous inert  │
│  3. Aggregation: diff each probe against its point's control        │
│     baseline (hash-only MVP: opaque u64 digests, no changed-        │
│     signals) → Effect (first_divergence_frame_delta); asymmetric   │
│     or truncated coverage downgrades to Unclassified               │
│  4. Group completion (movement clusters) → cache persist            │
│     (best-effort) → surface attached to the report                  │
│     (report.discovered_surface) and frozen-world findings          │
│     annotated with probe statistics (_meta).                        │
└─────────────────────────────────────────────────────────────────────┘
```

An earlier draft of this design depicted discovery as a battery
PRELUDE — a phase that ran BEFORE the game run and injected the
discovered surface into an undeclared scenario. That flow is now
implemented as **discovery-driven scenario injection**
(`BEVY_PLAYTEST_DISCOVER_INJECT=1`) — see the section below.

## The Declared Tier (Leafwing reader, intent-tap, reconcile)

The sweep measures; it cannot cite. For games using Leafwing Input
Manager, the declared tier adds the citation: an **opt-in reader**
(`lw-reader` feature, pure reflection — the harness links no
Leafwing code, it walks the game's live `InputMap`s through the type
registry exactly as it reads any reflected resource) enumerates
every declared binding, including maps loaded from RON and maps of
different `Actionlike` types coexisting in one world. Declarations
ride the playtest report as `lw_declared`.

Three layers, each honest about what it proves:

1. **Reader** — the game code declares this binding. Nothing about
   liveness: a declared binding the probes refute is the
   `declared_but_dead` alarm channel, and a declared binding the
   probes never looked at is coverage debt, not a verdict.
2. **Intent-tap** — the driver's synthetic press reached the input
   manager and fired the action (`stage: "manager_pipeline"`). This
   is **wiring** liveness, and the product's honesty spine lives
   here: **wiring liveness is not game liveness.** An action that
   fires while the game ignores it is a note (`context_unverified`),
   never an alarm — a false tap would destroy the channel, so
   `manager_pipeline_ok` claims exactly what it observed and nothing
   more.
3. **Reconcile** — the join. Declared inputs upgrade to
   `manager_pipeline_ok` when the tap corroborates; their duplicate
   `discovered_but_undeclared` residue from the draft is retracted
   (citation supersedes raw observation); and the alarm fires only
   on **proven-dead wiring**: a disabled action that never fired
   (per-action or component-wide disable — any firing overrides the
   disabled observation), or a pressed binding whose action never
   fired across the run. Chord members — disabled or not —
   never-pressed bindings, and fired-but-ignored actions NEVER
   alarm: the chord gap is deliberate (chord firing is only
   observable as a group, so no per-member arm can be proven), and
   the known-answer fixture plants each of these as a negative
   control, while the mutation harness proves the channel both ways
   (silent for live, loud for killed).

Determinism extends unchanged: reconcile is a pure function of
(declarations, tap stats, draft inputs), and identical runs produce
byte-identical output.

## Discovery-Driven Scenario Injection

The report-annotation flow above describes the run; it cannot save
one. The motivating case: a loading screen whose whole interactive
surface is `Digit1`/`Digit2` level selection. The default chaos bot
samples WASD+arrows, never presses Digit keys, the level never
loads, and the battery correctly pins a `frozen_world` finding —
about a bot that was never playing the game. Discovery knew better
(it had watched Digit1 and Digit2 diverge) but the finding shipped
anyway, because nothing consumed the surface.

`BEVY_PLAYTEST_DISCOVER_INJECT=1` closes that loop, and it is what
the "battery PRELUDE" of an earlier draft of this design turned
into. Semantics:

- **Implies discovery.** The injection needs a surface; the
  annotation rides along free (the same surface feeds the bot and
  the report). Setting both flags is harmless; the inject flag
  alone is enough.
- **Declaration beats inference.** Injection targets ONLY the
  built-in battery literal, whose key surface is undeclared (the
  default WASD+arrows set). An authored scenario file — with or
  without its own `raw_surface` — is never injected into;
  discovery then describes rather than replaces.
- **Injectable keys** (`DiscoveredSurface::chaos_keys`): every key
  with an observed effect, plus group-completed `Inferred`
  siblings (likely-bound movement clusters; pressing them costs
  nothing, omitting them starves the bot of exactly the coverage
  the group phase exists to provide). NEVER `Unclassified` (the
  control key — no differential verdict exists about it) and
  never inert keys (pressing keys the game demonstrably ignores
  recreates the declared-surface hole injection exists to close).
  Sorted and deduplicated — the list becomes a scenario file
  verbatim.
- **Mouse stays default.** The MVP probe matrix is keyboard-only,
  so there is no discovered mouse surface; an explicitly empty
  mouse list would DISABLE mouse chaos, which discovery never
  licensed. The injected scenario keeps the default buttons.
- **Runs discovery exactly once.** The parent discovers BEFORE
  the child (cache-first, ~40s cold at pool=8), writes
  `scenario.injected.json` (the battery literal plus
  `bot.raw_surface.keys`) into the run's temp dir, and drives the
  child with that path. The post-run annotation step reuses the
  pre-run surface — it never re-probes (the injected file path
  would otherwise cold-miss the `"battery"`-keyed cache entry).
- **Honest reporting.** The report carries
  `injected_surface_keys` (absent when no injection happened),
  because the child honestly reports `scenario_source: "file"`
  for the injected scenario — the field is what distinguishes
  "an authored scenario" from "the battery, aimed".

On the motivating fixture the effect is decisive: the bot plays
the game (periodic Digit1/Digit2 level reloads keep
`Changed<Transform>` flowing well within the liveness budget) and
the run goes clean for the full duration where the undiscovered
battery pinned a `frozen_world` finding at frame ~521.

## Static Scan: Annotation + Conservative Pruning Only

### What Works (Verified in Bevy 0.20.1)

`Schedule::systems_with_access()` exists (requires initialized schedule). `SystemAccess` is public. `Access::has_read(ComponentId)` works. `Components::valid_component_id::<T>()` (or the type-erased `get_valid_id(TypeId)`) maps TypeId→ComponentId. Caveat: system NAMES are masked without `bevy_ecs/debug` — classify by ComponentId reads, never by name (see Spike Results).

So we CAN statically detect:
- Systems reading `Res<ButtonInput<KeyCode>>`
- Systems reading `Res<ButtonInput<MouseButton>>`
- Systems reading `AccumulatedMouseMotion`
- Systems reading `AccumulatedMouseScroll`
- Systems reading `Res<Gamepad>`
- **NEW:** Systems reading `Messages<KeyboardInput>` (via `TypeId::of::<Messages<KeyboardInput>>()`)
- **NEW:** Systems reading `Messages<WindowEvent>` (for wrapped input paths; attributed to
  keyboard AND mouse-button AND mouse-motion AND mouse-wheel — the umbrella
  event spans all four, so a `MessageReader<WindowEvent>` observes all of
  those channels)

### Blind Spots (Design Rule: FAIL-OPEN FOR KEYBOARD)

Static scanning **cannot** see:
1. **Run conditions** (`input_just_pressed`, etc. in `bevy_input::common_conditions`) — these are guard systems, not part of the main system's access set.
2. **Observers** — bypass schedules entirely. An observer closure may read input state (`Res<ButtonInput<…>>`, `MessageReader<…>`) beside its `On<E>` trigger param, and bevy 0.20 keeps observer systems private (their access sets are invisible to the schedule walk; `World::observers()` is queryable only at event-type level). Direct observation of input messages (`On<MouseWheel>`) is impossible — bevy 0.20 messages are not Events — but any observer can hide a side-param consumer, so the scan marks its result incomplete (`complete: false`) whenever ANY observer entity exists in the world: the reader counts stay as annotations, but the prune derivation refuses them (same fail-open posture as uninitialized schedules).
3. **Lazy-initialized schedules** — return `ScheduleNotInitialized` until run. The scan marks its result incomplete (`complete: false`) whenever an uninitialized schedule holds any non-engine-driver system: the reader counts stay as annotations, but the prune derivation refuses them (a "pruned_devices withheld" note explains why).
4. **"bevy_\*" prefix filtering misclassification** — `bevy_ui::focus.rs` reads `ButtonInput<MouseButton>` and would be excluded as "engine", incorrectly.

**Design Decision:** NEVER prune the keyboard class. Static scan output becomes:
- **(a)** An annotation/report artifact documenting which game systems read which device classes
- **(b)** At most a prune gate for gamepad/wheel/gestures (fail-open: "no readers found" ⇒ probe everything) — and only from a scan that saw the full schedule graph (`complete`): an uninitialized or missing schedule hides its systems' reads, so zero readers from an incomplete scan proves nothing and never licenses pruning

**Pruning is DORMANT today, by construction.** The only scans the
shipped pipeline exports come from `run_scenario`'s child-side
capture point, which fires at harness-setup time — BEFORE the
world's first `app.update()` — so runtime-added readers (systems
or observers registered from inside a system) are invisible to it
and the scan is stamped `complete: false` by construction (the
pre-update blindness note). The capture DOES force-initialize
schedules (so their systems' access sets are walked), but
force-initialization cannot see what has not been registered yet;
observer entities and run-condition-hidden readers withhold
completeness the same way. Every scan exported from a real
pipeline therefore arrives `complete: false`, the prune derivation
always withholds (`pruned_devices` stays empty with the
"pruned_devices withheld" note), and reader counts serve purely as
annotations. The pruning code path exists and is unit-tested (a
synthetic `complete: true` scan with zero gamepad readers prunes
gamepad), but no real capture point can produce a `complete: true`
scan today. Until a trusted post-update capture point exists (if
ever), reader counts are informative annotations and pruning never
fires. This is deliberate fail-open posture, not a TODO: a capture
point after the first update would count the harness's own bot
systems as readers of every device class.

If "no keyboard consumers detected", we still probe the full prior set AND emit an informational finding: "no input consumers detected — probing conservatively".

### Expanded TypeId List

The scan must check for these additional input signal types:
```rust
// Direct resources (current)
TypeId::of::<ButtonInput<KeyCode>>()
TypeId::of::<ButtonInput<MouseButton>>()
TypeId::of::<AccumulatedMouseMotion>()
TypeId::of::<AccumulatedMouseScroll>()
TypeId::of::<Gamepad>()

// Event messages (additionally checked). Note: Messages<WindowEvent> is
// registered once per device class it covers (keyboard, mouse button,
// mouse motion, mouse wheel) — see the blind-spots section above.
TypeId::of::<Messages<KeyboardInput>>()
TypeId::of::<Messages<WindowEvent>>()
TypeId::of::<Messages<bevy_input::keyboard::KeyboardFocusLost>>()
TypeId::of::<Messages<bevy_input::mouse::MouseButtonInput>>()
TypeId::of::<Messages<bevy_window::CursorMoved>>()
```

## Probe Matrix Design

### Candidates

Per the prior-likelihood set (as shipped in
`PROBE_CANDIDATE_KEYS`: 30 canonical keys — WASD + arrows,
digits 0-9, navigation cluster, modifiers, and common action
letters E/F/R):
- **Keys:** WASD, arrows, digits 0-9, Space, Enter, Escape, Tab, Backspace, Shift/Ctrl/Alt, E/F/R (Q is the default control key — a probe of the control against itself is structurally unclassifiable, so it is excluded from candidates; a bespoke matrix probing it gets an `Unclassified` finding, never a trusted negative)
- **Mouse buttons:** Left, Right, Middle
- **Mouse motion:** delta (10.0, 5.0)
- **Wheel:** dy = 1.0
- **Gamepad:** face buttons (South/East/North/West), DPad, Start/Select, axes (LeftStickX/Y)

### Variants Per Candidate

- **Tap:** hold_frames = 1
- **Hold:** hold_frames = 10

**MVP decision: TAP ONLY.** With independent per-point runs (below), the
run matrix doubles; carrying both variants doubles it again (~2× runs on
top of the 2-point baseline). A tap is the harder case for a game to miss
(a `just_pressed` edge fires; a hold additionally satisfies `pressed`),
and the dominant interaction styles (menu confirm, action trigger)
respond to taps. Hold-only semantics (charging, continuous movement) are
covered fail-open anyway: keys showing no tap effect are NOT pruned into
"dead" — they stay in the surface at `Statistical` confidence. Hold
variants become a follow-up knob once the cache absorbs the cost.

### Probe Points (State-Gating Handling)

Games may listen for inputs only in specific states (e.g., `loading_screen` listens for `Digit1`/`Digit2` only while `LevelReady`).

**Two strategies, evaluate both:**

#### Strategy A: Multi-Point Fixed Probe
Probe at ≥2-3 frames:
1. **Readiness point:** `pre_ready_frames + tps` (≈1s after startup)
2. **+1s later:** `readiness + tps`
3. **Mid-duration:** `duration_s / 2 * tps`

Each candidate is tried at each probe point. Effect reports earliest divergence frame delta.

#### Strategy B: Phase-Segment Baseline
Run baseline first, analyze digest trace for stability windows:
1. Detect digest-stable phases (consecutive frames with identical digests)
2. Probe once per phase segment (entry + mid-point)

**Recommendation:** Start with Strategy A (simpler, no digest-analysis overhead). If coverage is poor (many "no effect" results on known games), migrate to B.

#### Accounting: One Probe Point Per RUN (Independence Requirement)

**Decision: NEVER press at multiple probe points in one run.** An early
effective press mutates game state before later points are reached — a
divergence observed at the mid-duration point would then be attributable
to the earlier press's downstream consequences, not to the input at that
point. Points must be independent for the frame-delta attribution to be
meaningful.

Therefore each probe point gets its OWN run, paired with its own
normalized control (identical except the key — see Synthetic-Control
Normalization below):

- **MVP: 2 points per key**, covering the two distinct listening regimes
  with minimum runs:
  - **Run 1 — readiness probe:** press at `pre_ready_frames + tps` (≈1s
    after startup; covers "menu/title" listeners).
  - **Run 2 — mid-duration probe:** press at `duration_s/2 * tps` (covers
    "post-transition" listeners; if run 1's key changed state, run 2
    probes a different state — which is the point).
- The third Strategy-A point (+1s) is dropped for the MVP: it sits between
  the other two and rarely adds a distinct regime. Revisit after coverage
  data from real fixtures.

**Attribution:** with one point per run,
`Effect.first_divergence_frame_delta` is attributable to THAT point
unambiguously. `probed_at` in the surface report lists the frames actually
probed per key.

**Accounting summary:** 2 probe runs per key (tap only) + 2 control runs
(shared across keys, once per determinism gate — see Cost Model). A key
showing an effect at EITHER point is kept; a key showing no effect at
BOTH points (with passing determinism gates) is reported as
probed-and-unobserved — still never pruned from the keyboard class
(fail-open).

### Subprocess Pool Size

- **Pool size:** 6-8 concurrent game processes
- **Rationale:** Bevy examples run ~5s headless. 8 parallel = ~40s wall-clock for full matrix vs ~6-8 min serial. Memory footprint acceptable on dev machines (each process ~50-150 MB).
- **CI constraint:** Battery runs with `--test-threads=1`. Discovery spawns subprocesses internally, so CI cost = (discovery_wall_clock) × 1 thread.

### Scenario JSON Shape for Probe Runs

Reuse the existing scenario schema — verified against the real
`src/scenario.rs` (see Spike Results). Two shapes that work today:

**Probe run (probe key K, tap variant):**

```json
{
    "name": "probe_d1_tap",
    "bot": {
        "type": "synthetic_keyboard",
        "seed": 42,
        "key_presses": [
            { "frame": 60, "key": "Digit1", "hold_frames": 1 }
        ]
    },
    "duration_s": 3.0,
    "single_threaded": true,
    "verify_replay": false
}
```

- `hold_frames: 1` = tap variant (the only MVP variant).
- One probe point per run: generate a separate scenario per point (see
  Accounting above). Do NOT put multiple `key_presses` entries in one run —
  that confounds attribution.
- Key names go through `normalize_key_name` (accepts `"Digit1"`, `"1"`,
  `"keyq"`, `"space"`, `"up"`, …) — the candidate generator should emit
  canonical names directly. The flow additionally canonicalizes the
  WHOLE plan (control key and candidates) at the front door
  (`ProbePlan::canonicalized`): an alias pair (`"q"` control, `"KeyQ"`
  candidate) presses the same physical key, and downstream spelling
  comparisons (plan fingerprint, job identity, the self-probe guard)
  must see one vocabulary or the vacuous negative would masquerade as
  `Exact`.

**Control run (synthetic-control normalization):**

The control must be a **`synthetic_keyboard` run pressing the control key
at the SAME frame and hold_frames as the probe**. Anything else is
confounded: an earlier spike draft used a chaos bot hammering the control
key at 60 Hz, which differs from the probe in press rate, timing, AND key
— a divergence then proves "the runs differ", not "the key matters".

Normalized pair for probe key K at frame F, tap variant:

```json
{
    "name": "probe_control",
    "bot": {
        "type": "synthetic_keyboard",
        "seed": 42,
        "key_presses": [
            { "frame": 60, "key": "KeyQ", "hold_frames": 1 }
        ]
    },
    "duration_s": 3.0,
    "single_threaded": true,
    "verify_replay": false
}
```

Probe and control then differ by exactly one bit: which key is pressed.
Everything else (bot type, seed, frame, hold duration, rate) is identical.
The same normalization applies per variant and per probe point — the
control always mirrors the probe's press parameters.

There is still NO idle bot in the schema (`synthetic_keyboard` requires
non-empty actions; a chaos bot requires a non-empty surface), so the
control key must be a real key press — but a normalized one. 

Control-key choice needs care: `KeyQ` works for loading_screen (only
Digit1/Digit2 are read) but games may read any key. Mitigations and
their LIMITS: the double-control determinism gate (run C twice;
identical reports) checks REPEATABILITY, not NEUTRALITY — it cannot
detect a consumed control key. A game that consumes the control key
can MASK candidates: a key causing the same digest change as the
control press compares equal to the baseline and reads as falsely
INERT, which is a fail-CLOSED error (the earlier "only adds
conservative error toward keep the key" claim was wrong and is
retracted). Runtime detection is impossible under the hash-only MVP
(opaque digests cannot be decomposed), so every discovered surface
carries a baseline caveat note, and the surface's inert findings
should be read with that caveat. Practical remediation: configure a
control key the game demonstrably ignores (per game), or extend the
scenario schema with a true idle baseline (future work — `synthetic_keyboard`
currently requires non-empty actions). The control key remains
excluded from the default candidate set, and any bespoke matrix
probing it gets an `Unclassified` finding rather than a trusted
negative.

**Why this is inherent to the differential method (limitation, not
a bug):** the control run's trace IS the differential baseline, so
"same digest change as the control press" is precisely
indistinguishable from "no change at all" — a candidate whose
input produces the SAME state change as a (consumed) control key
compares equal against the baseline and necessarily classifies as
inert. This is not fixable within the methodology; only a baseline
that provably does not touch game state escapes it. The fail-open
alternative — a TRUE IDLE BASELINE (a run that presses nothing) —
was considered and REJECTED: `synthetic_keyboard` requires
non-empty `key_presses` and a chaos bot requires a non-empty
surface, but the deeper blocker is that an idle baseline does not
help for the games that need it most: NONDETERMINISTIC games
diverge from their own idle baseline on noise alone, which poisons
every per-candidate verdict (every diff would read "diverged",
degrading the whole surface to `Statistical` with no usable
negatives). The normalized control-key pair plus the explicit
caveat notes — every surface carries the baseline caveat, and the
control key itself is always `Unclassified` and excluded from probe
statistics — is the shipped compromise: deterministic games get
trusted verdicts modulo a documented blind spot, and the blind
spot is surfaced in-band rather than hidden.

## Divergence Detection Algorithm

Using per-frame broad state digests (from `feat/state-digest`, merged at
`e9592ab`):

### Digest Spec (Shipped)

Per-frame **opaque `u64`**. That is the entire contract: the digest is a
hash of whatever the state-digest PR chose to fold in, and the discovery
layer treats it as an opaque equality token — it compares digests, it does
NOT decompose them. There is no `FrameDigest` snapshot struct, no
per-field/per-key percept vocabulary, and no `census.*` /
`bevy_playtest::*` key namespace in this codebase (those are concepts from
an unrelated project, not bevy_playtest). Deterministic across processes;
optional per-frame recording for a run.

**MVP scope: hash-only divergence.** Discovery's signal is "did the digest
sequence diverge, and at which frame" — nothing more. See Amendment 1 note
under Risk 5 for why this is the shipped mode rather than a fallback.

### Comparison Logic

```rust
/// Hash-only divergence detection (MVP, and the shipped mode).
///
/// Traces are per-frame `StateDigest { frame, hash }` records from
/// the state-digest work, compared from the tap frame onward,
/// ALIGNED BY ABSOLUTE FRAME (a frame the other trace skipped is
/// itself divergence). Returns the first frame relative to the probe
/// press where the probe run's digest sequence stops matching the
/// control run's, or None if the sequences never diverge (=> the
/// probe key had no observed effect at this probe point).
fn diff_trace(
    probe_digests: &[StateDigest],
    control_digests: &[StateDigest],
    probe_frame: u64,
) -> Option<Effect> {
    let probe_from = probe_digests.iter().filter(|d| d.frame >= probe_frame);
    let control_from = control_digests.iter().filter(|d| d.frame >= probe_frame);
    for ((pi, p), (ci, c)) in frame_aligned(probe_from).zip(frame_aligned(control_from)) {
        if pi != ci {
            // Frame misalignment: one trace skipped a frame the
            // other recorded — that IS a divergence of the runs.
            return Some(Effect {
                first_divergence_frame_delta: (pi.min(ci) + 1 - probe_frame).max(1) as u32,
            });
        }
        if p != c {
            return Some(Effect {
                first_divergence_frame_delta: (pi + 1 - probe_frame).max(1) as u32,
            });
        }
    }

    None // No divergence observed over the frames compared
}
```

The `Effect` struct is correspondingly minimal:

```rust
pub struct Effect {
    /// Ticks elapsed between the probe press and the first digest
    /// divergence (1 = same frame).
    pub first_divergence_frame_delta: u32,
}
```

There is NO `changed_signals` field. The digest is an opaque hash: the
moment we tried to extract "which signals changed" from it we would need a
snapshot vocabulary that the merged `feat/state-digest` API deliberately
does not expose. If a future state-digest mode records structured
snapshots (e.g., via `DynamicSceneBuilder` / `DynamicWorldBuilder`), a
follow-up can extend `Effect` with `changed_signals: Option<Vec<String>>`
— gated behind that snapshot mode, default `None`, never blocking the
hash-only path.

### First Divergent Frame Extraction

- **Frame delta:** `(divergent_frame - probe_frame + 1).max(1)` — measures "how many ticks after the input did the world change?"
- **Changed signals:** Not available in the MVP — the digest is an opaque u64 (see Digest Spec). Report the delta and the probe point only; a future snapshot mode may add per-signal detail as an opt-in extension.

### Partial-Trace Defense (Aggregation Layer)

`diff_trace` zips only the COMMON PREFIX of the two traces, so a
truncated or gappy pair can produce a vacuous `None` ("never
diverged") that speaks for fewer frames than the run was supposed to
observe. The shipped pipeline guards this at three layers:

1. **Pool completeness gate** (first layer): the pool refuses any
   report whose trace stops short of the plan's expected final
   frame `ceil(duration_s * tps) - 1` (`UnusableReport` — an
   infrastructure error, never a vacuous inert), and requires the
   trace to cover the tap point.
2. **Pairwise symmetry backstop** (`aggregate`'s
   `coverage_mismatch`): for direct callers, an inert verdict is
   trusted only when the probe and control traces are both intact,
   contiguous frame sequences from 0 of the same length — equal
   maxima with shared gaps, empty traces, and asymmetric lengths all
   downgrade the key to `Unclassified` with a "coverage caveat" note.
3. **Expected-span argument** (`aggregate`'s
   `expected_last_frame: Option<u64>`): symmetry alone still permits
   two IDENTICAL truncated traces (both stop at frame 100 of a
   180-frame plan) to read as trusted inert. `None` keeps that
   legacy contract for callers without plan knowledge; `Some(n)`
   demands both traces cover frames `0..=n` or the pair downgrades
   to `Unclassified`. `discover()` always passes
   `Some(plan.expected_last_frame())` — redundant with the pool gate
   for real pipelines, but it closes the public-API flank for
   direct `aggregate` callers (Copilot #58 review finding, fixed in
   the slice-D flow integration).

## Cache Design

### Key Structure

```rust
CacheKey {
    game_binary_hash: fnv1a_hex(binary_path), // internal FNV-1a (staleness
                                              // token, not a security
                                              // primitive — see cache.rs)
    bevy_playtest_version: env!("CARGO_PKG_VERSION"),
    feature_fingerprint: sort(features.join(",")), // e.g., "in-process,takeover,ui-liveness"
    fixture_name: Option<String>, // for fixtures: "loading_screen"
    plan_fingerprint: fnv1a_hex(sort(candidate_keys), control_key, tps, duration_bits),
}
```

The plan fingerprint means a slim `BEVY_PLAYTEST_DISCOVER_KEYS` run and
the full default matrix NEVER share an entry: the cached surface must
match what the current plan can express.

### Staleness Policy

Cache invalidates when:
1. `bevy_playtest` version changes (schema/algorithm changes)
2. Game binary hash changes (code change)
3. Feature flags change (different build)
4. Probe plan changes (candidate set, control key, tps, duration — a
   slim override run and a full-matrix run are different questions)

Staleness check: compare stored key against current. If stale, re-run
discovery. Any read failure (missing, corrupt, unreadable) is a miss,
never an error — the cache can only make discovery slower, not wrong.

### Cache Location

**Decision:** `.bevy_playtest/cache/` alongside reports (user-writable, gitignored).

Structure:
```
.bevy_playtest/
  cache/
    loading_screen_<binary_hash>_<version>_p<plan_fp>_f<feature_fp>.json
    breakout_<binary_hash>_<version>_p<plan_fp>_f<feature_fp>.json
  reports/
    loading_screen.smoke.report.json
    ...
```

Alternative: embed cache in report directory. Chosen `.bevy_playtest/` because:
- Cache is cross-scenario (same game, multiple scenarios reuse same discovered surface)
- Separates transient artifacts from permanent reports
- Easy to `rm -rf .bevy_playtest/cache` for fresh discovery

### Cache Format

Mirrors the shipped `CachedSurface` in `src/discovery/cache.rs`:
exactly `key` + `surface` (there is no `cached_at` timestamp — the
key IS the staleness policy, and a timestamp would suggest time-based
invalidation the cache does not do). Entry shape:

```json
{
  "key": {
    "game_binary_hash": "abc123...",
    "bevy_playtest_version": "0.6.0",
    "feature_fingerprint": "in-process,takeover",
    "fixture_name": "loading_screen",
    "plan_fingerprint": "0d1e2f3a4b5c6d7e"
  },
  "surface": {
    "keys": [{"key": "Digit1",
              "effect": {"first_divergence_frame_delta": 1},
              "confidence": "Exact"},
             {"key": "Digit2", "effect": null, "confidence": "Exact"}],
    "mouse_buttons": [],
    "mouse_motion": null,
    "wheel": null,
    "gamepad_buttons": [],
    "gamepad_axes": [],
    "probed_at": [60, 90],
    "readers": [["keyboard", 3]],
    "key_confidence": [["KeyA", "Inferred"]],
    "confidence": "exact",
    "pruned_devices": [],
    "notes": ["pruned_devices withheld: the static scan did not see the full schedule graph, so zero reader counts prove nothing (fail-open; the keyboard class is never pruned)",
              "..."]
  }
}
```

(`pruned_devices` is `[]` in real entries today: every exported scan
arrives `complete: false`, so the prune derivation always withholds —
see the pruning-dormancy note in the static-scan section.)

## Report Integration

### DiscoveredSurface Structure

As shipped in `src/discovery/surface.rs` (verbatim field shapes —
this is the wire format; `KeyConfidence` lives INSIDE each finding,
there is no tuple-shaped variant, and a `ControlPair` type never
existed):

```rust
pub struct DiscoveredSurface {
    pub keys: Vec<KeyFinding>,          // keyboard candidates
    pub mouse_buttons: Vec<KeyFinding>, // same shape as `keys`
    pub mouse_motion: Option<Effect>,   // motion-probe effect, if probed
    pub wheel: Option<Effect>,          // wheel-probe effect, if probed
    pub gamepad_buttons: Vec<KeyFinding>, // same shape as `keys`
    pub gamepad_axes: Vec<(String, Option<Effect>)>, // axis → effect
    pub probed_at: Vec<u64>,            // probe-point frames actually used
    pub readers: Vec<(String, usize)>, // Device class → reader COUNT
                                       // (system names are masked without
                                       // bevy_ecs/debug — annotation grade)
    /// SPARSE per-key confidence OVERRIDE map, keyed by key name.
    /// The PRIMARY residence of confidence is the `KeyFinding`
    /// itself; this map lets group completion (Phase 6) mark a
    /// sibling as inferred without rewriting its raw probe record.
    /// Lookups check this map first, then fall back to the finding.
    pub key_confidence: Vec<(String, KeyConfidence)>,
    pub confidence: SurfaceConfidence, // Whole-surface: Exact | Statistical
    pub pruned_devices: Vec<String>,   // Skipped via static pruning
                                       // (always empty from real
                                       // pipelines today — see the
                                       // pruning-dormancy note above)
    pub notes: Vec<String>,            // Caveats from the flow and scan
}

/// One effectiveness datum, with its confidence embedded.
pub struct KeyFinding {
    /// Candidate key name (canonical spelling).
    pub key: String,
    /// Observed effect (first-divergence delta), or `None` if the
    /// probe run matched the control trace.
    pub effect: Option<Effect>,
    /// How this entry got into the surface.
    pub confidence: KeyConfidence,
}

/// How a key came to be in the surface.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum KeyConfidence {
    /// Observed divergence in this key's own probe run(s).
    Exact,
    /// Inferred: group-completion sibling of a key that DID diverge
    /// (e.g., KeyW diverges; KeyA/KeyS/KeyD marked as likely-movement).
    /// Heuristic — inferred only, never Exact, or the confidence
    /// field lies. Keys that diverged in their OWN probe run are
    /// `Exact` (modulo the whole-surface degradation rule for
    /// nondeterministic baselines).
    Inferred,
    /// The control key probed against itself: no differential verdict
    /// possible, never a trusted negative (excluded from the default
    /// candidate set; labelled if a bespoke matrix probes it).
    Unclassified,
}
```

### Integration Into PlaytestReport

Add field:
```rust
pub struct PlaytestReport {
    // ... existing fields ...
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discovered_surface: Option<DiscoveredSurface>,
}
```

### Frozen World Findings Annotation

When discovery runs and finds "frozen_world" violations:
```json
{
  "rule": "frozen_world",
  "target": "world",
  "detail": "no Transform change for 180 frames — soft-lock?",
  "_meta": {
    "probed_keys_effective": 3,    // Keys that caused divergence
    "probed_keys_total": 28,       // Total probed (Exact-confidence keys
                                   // only; control key excluded)
    "pruned_devices": [],          // Always [] from real pipelines today:
                                   // pruning is dormant (see the static-scan
                                   // section) — the accompanying surface
                                   // carries the "pruned_devices withheld"
                                   // note instead
    "note": "probed N keys, M effective"
  }
}
```

## Cost Model

### Honest Wall-Clock Estimates

Calibrated against the spike measurement (1.9 s wall-clock per 3 s
probe run on a dev machine, debug build; see Spike Results). The
planning estimates below assume 5 s per run to leave headroom for
heavier games.

Run-count derivation (MVP: tap only, 2 independent probe points per key,
the shipped 30-key candidate set):

- **Controls:** 4 runs — the double-control determinism gate runs C
  twice PER PROBE POINT, and `build_job_list` emits the pair inside
  the per-point loop (2 points × 2 control copies = 4). Shared across
  all keys — not per-key.
- **Probe runs:** 30 keys × 2 points = 60 runs.
- **Total:** 64 runs per fixture.

Wall-clock:
- Serial: 64 × 5s = 320s (~5.3 min)
- Pool of 8: ~320s / 8 ≈ 40s
- Pool of 6: ~320s / 6 ≈ 53s
- **Total (pool=8):** ~40s per fixture

At the spike's measured 1.9s/run (rather than the 5s planning figure),
pool=8 lands near ~15s per fixture — comfortably under a minute.

**Scaling knobs (documented, not MVP):** adding the hold variant doubles
the probe runs (120 probes → ~570s serial, ~72s at pool=8); extending to
the full ~60-key prior set with both variants and 2 points reaches ~240
probe runs (~4 min at pool=8). Still a coffee break, which is why the
subprocess pool
architecture is the right call.

### Full Battery Cost (8 fixtures: 7 fetched showcase games + the committed known_answer fixture)

- Serial: 7 × ~5.3 min = ~37 min
- Parallel discovery (pool=8 per fixture, sequential fixtures): 7 × ~40s ≈ 5 min
- **With fixture-level parallelism (4 fixtures concurrently):** ~1-2 min

### CI Considerations

- **CI runs:** `battery --test-threads=1`
- **Discovery cost absorbed:** Discovery is a prelude, not a separate test
- **Cache hit:** Skip discovery, use cached surface (near-zero cost)
- **CI honesty — no pre-warming:** The cache key includes the game binary
  hash, which changes on EVERY CI build, so a cache warmed in an earlier
  CI job (or committed to the repo) is always stale and never hits. Two
  options, choose at implementation time:
  1. **Accept re-discovery per CI run** (recommended for MVP): CI simply
     pays the ~40s/fixture discovery cost each run. Simple, always
     correct, no stale-cache class of bugs.
  2. **Key CI cache on a source-tree digest** (e.g., hash of the game's
     `src/**` + `Cargo.lock`) instead of the built binary hash, enabling a
     real CI cache hit across builds with unchanged sources. More moving
     parts; defer until discovery cost actually hurts CI.
  Until then, do NOT recommend "pre-warm the cache in CI" — it cannot work
  as designed.

## Risks & Mitigations

### 1. Nondeterminism

**Risk:** Baseline runs produce different digests each time → cannot trust differential probing.

**Mitigation:**
- Run baseline TWICE before probing
- If baselines diverge: degrade to `SurfaceConfidence::Statistical`
- Statistical mode: use `SurfaceConfidence::{Exact, Statistical}` (whole-surface)
  - Report effect *rates* (N out of M probes caused divergence) instead of exact diffs
  - Document: "statistical confidence — baseline was nondeterministic"

### 2. State-Gating

**Risk:** Input only matters in specific states (e.g., `Digit1` only while `LevelReady`). Probing at wrong frame yields false negative.

**Mitigation:**
- Use multi-point probing (MVP: 2 independent points, one per run — readiness + mid-duration)
- Analyze control digest trace for state transitions (sharp digest changes = state change)
- Log informational finding: "probed at frames [60, 90]; consider adding probe_points to scenario"

### 3. Exclusive Systems Disabling Pruning

**Risk:** Any exclusive system (or `has_read_all()`) forces conservative "probe everything" mode.

**Mitigation:**
- This is correct behavior (exclusive systems may read anything)
- Emit informational note: "exclusive system detected — probing full surface"
- Rare in practice (most games are shared-access)

### 4. Lazy-Initialized Schedules

**Risk:** `systems_with_access()` returns `ScheduleNotInitialized` for schedules never run.

**Mitigation:**
- Run one `app.update()` before scanning (initializes schedules)
- Catch panic if game crashes during init → treat as "no surface", fallback to full probe
- Document limitation: "discovery requires game to boot without panic"

### 5. Digest-Insensitivity Residual Risk

**Risk:** Broad digest (opaque u64 hash) may miss subtle input effects (e.g., a counter increments but no entity changes, or the digest algorithm folds in less than the game's full state).

**Mitigation:**
- **Short-term:** Accept risk. The hash-only divergence signal is the shipped MVP mode (merged `feat/state-digest` at e9592ab exposes only an opaque u64 per frame). It catches 80% of meaningful interactions.
- **Long-term:** Future `StateDigest` hook allowing games to opt-in to custom digest algorithms or structured snapshots.
- **Fallback built-in:** Even without any snapshot/changed-signals detail, the hash-only path still discovers *whether* a key matters and *when* it mattered (the divergence frame delta). That is sufficient for the discovery contract; the "what changed" detail is nice-to-have, not required.

Note: `Effect` is correspondingly minimal (`first_divergence_frame_delta: u32`). The `changed_signals` field was removed from the MVP design; if a future state-digest mode records structured snapshots, `Effect` can gain an optional `changed_signals: Option<Vec<String>>` gated behind that snapshot mode, default `None`. This avoids blocking this feature on snapshot instrumentation work.

## Schema Changes Summary

### scenario.rs (Minimal Addition)

No breaking changes. Probe scenarios are generated programmatically as
plain `Scenario` values (or JSON via the verified shapes above) using the
existing `synthetic_keyboard` bot for BOTH probe and normalized control
runs — no schema additions are required for the probe loop itself.

**`raw_surface` UNDECLARED vs defaulted — how detection works.** The
chaos bot's `raw_surface` is declared as
`Option<RawSurface>` with `#[serde(default, skip_serializing_if =
"Option::none")]`, and `RawSurface` itself has defaulted fields
(`keys` → WASD+arrows, `mouse_buttons` → all three primary buttons). So
undeclared-vs-defaulted IS distinguishable post-deserialize, at TWO
levels:

1. **Whole-surface level:** `scenario.bot.raw_surface.is_none()` ⇒ the
   author did NOT declare a surface at all (the game provided nothing;
   discovery's output is the interesting input). `is_some()` ⇒ an
   explicit declaration exists, even if partially defaulted.
2. **Field level:** for `Some(surface)`, the presence of a default cannot
   be detected per-field without serde markers (a defaulted field and an
   explicitly-written default value deserialize identically). If
   per-field attribution ever matters, the fix is a
   `#[serde(skip_serializing_if = "is_default")]` round-trip sentinel or a
   `deny_unknown_fields`-style strict mode — deferred, not needed for
   MVP, because discovery keys off level 1: `raw_surface: None` means
   "author declared nothing — discover it"; `Some(_)` means "author
   declared something — discovery annotates rather than replaces".

Optional future addition (not required for MVP):
```rust
pub struct BotConfig {
    // ... existing fields ...
    /// Discovery overrides: explicit probe points instead of auto-detect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe_points: Option<Vec<u64>>,
}
```

### PlaytestReport (Field Addition)

```rust
pub struct PlaytestReport {
    // ... existing fields ...
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discovered_surface: Option<DiscoveredSurface>,
}
```

Non-breaking (skip-if-none). Existing reports remain valid.

## Spike Results (Empirically Verified)

These facts were established by a validation spike run before
implementation (tests 1–2 auto-ran; tests 3–4 ran against a generated
fixture). They refine the design above.

### Static scan

- `Schedule::systems_with_access()` works as designed, but **system names
  are masked** (`<Enable the debug feature to see the name>`) unless
  `bevy_ecs/debug` (→ `bevy_utils/debug`) is compiled in. Consumer builds
  usually DON'T have it. Consequences for the design:
  - The scanner must classify by `ComponentId` reads, never by name.
  - The "bevy_\* prefix" skip-list heuristic is UNUSABLE in production
    (masked names make prefix matching meaningless anyway — the earlier
    misclassification risk is moot but so is the skip-list itself).
    Instead, engine noise (`bevy_ecs::apply_deferred`,
    `bevy_input::keyboard::keyboard_input_system`, etc.) showed up as
    keyboard-readers in the spike and must be tolerated in annotations:
    report them, mark the report as informational-only, never gate on them.
- Run-condition blind spot **empirically confirmed**: a system guarded by
  `input_just_pressed(KeyCode::Space)` appears in the schedule with
  `SystemAccess::None` (no reads at all). Meanwhile the raw condition closure
  itself never surfaces as a scannable system. FAIL-OPEN for keyboard is
  mandatory, not precautionary.
- Bevy 0.20.1 specifics: `Components::valid_component_id::<T>()` is the
  TypeId→ComponentId entry point (there is no `get_resource_id`); resources
  share the ComponentId namespace with components.

### Subprocess probing mechanics (real constraints hit)

- **Strict env contract bites the prober**: the takeover rejects ANY
  unknown `BEVY_PLAYTEST_*` env var with exit code 3. A probe runner
  inherited from `battery::run_with_config` already sanitizes the child
  env — reuse that code path, don't reinvent it.
- **Exit codes are content, not errors**: exit 1 = findings (loading_screen
  always trips `frozen_world`), exit 3 = contract rejection (fatal).
  Probe-runner success criteria = "report file written AND its digest trace
  is usable — it must cover the job's tap point AND the run's full
  expected frame span" (a trace-less or pre-tap-terminated report
  supports no verdict — every probe would vacuously match its control; a
  trace that stops short of the run's expected final frame
  `ceil(duration_s * tps) - 1` — a crash or truncation mid-run —
  similarly supports only a common-prefix comparison that reads as a
  vacuous inert; the pool refuses all three as infrastructure errors,
  never "no effect"), NOT process exit 0.
- **No idle baseline exists in the scenario schema**: `synthetic_keyboard`
  rejects empty `key_presses`; a chaos bot with empty `raw_surface` is
  "vacuous". Differential probing therefore MUST use **control-key runs**
  rather than a true no-input baseline.
- **Synthetic-control normalization (design correction from this spike)**:
  the spike's divergence demo used a CHAOS bot scoped to KeyQ as the
  control, which is CONFOUNDED — it differs from the probe run in press
  rate, timing pattern, and key, so the observed divergence proves the
  runs differ, not that Digit1 specifically matters. The spike's pass is
  evidence that the *pipeline mechanics* work (subprocess spawning, env
  sanitization, report parsing, violation-set diffing), NOT that the
  control shape was valid. The production design (above) fixes this:
  control = `synthetic_keyboard` pressing the inert key at the SAME frame
  and hold_frames as the probe, differing by exactly one bit (the key).
  The double-control determinism gate (run C twice, compare) carries
  over unchanged.
- **End-to-end pipeline proven on loading_screen**: control run (chaos
  scoped to KeyQ) vs probe run (chaos scoped to Digit1) — identical frame
  counts (180) and action counts (90), but divergent violation sets (the
  Digit1 run gains `log_error` from asset loads). NOTE: this demo used the
  confounded control shape (see synthetic-control normalization above);
  it validates the plumbing, and the production control shape is the
  normalized synthetic_keyboard pair. A violation-set diff plus
  action-log diff remains a viable interim divergence signal even BEFORE
  the per-frame state digest lands. First-divergent-frame extraction
  upgrades gracefully when the digest branch merges.

### Cost model (measured, this machine, debug build)

- 3s scenario ≈ **1.9 s wall-clock per subprocess run** (fixture spends ~1s
  in startup/teardown outside simulated time).
- Shipped matrix (30 keys × 2 probe points, tap only, + 4 per-point
  control runs = 64 runs): ~122 s sequential at the measured 1.9s/run,
  ~15 s at pool=8. At the 5s planning figure: ~320s serial, ~40s at
  pool=8.
- Even the honest worst case (full 60-key prior set, both tap and hold
  variants, 2 probe points each) stays within a coffee-break budget at
  pool=8 (< 10 min). The cost is dominated by run count × per-run overhead,
  NOT digest computation — confirming the fixed-overhead subprocess pool is
  the right architecture (vs. any in-process batching, which
  `run_scenario`'s process-global exclusivity forbids anyway).

## Implementation Checklist

### Phase 1: Static Scan Module (bevy-free where possible)
- [x] Extract `static_device_readers(app: &App) -> (readers, pruned)` into `src/discovery/static_scan.rs`
- [x] Add TypeId checks for `Messages<KeyboardInput>`, `Messages<WindowEvent>`
- [x] Unit test: verify scan detects `ButtonInput<KeyCode>` reader, verifies blind spot with run-condition

### Phase 2: Subprocess Probe Runner
- [x] ~~Extend `battery::run_with_config` to accept scenario JSON inline~~ (superseded: the
  pool writes generated scenario JSON to a per-run workspace and passes paths — same
  capability, cleaner lifecycle; `tests/probe_pool.rs` pins the contract)
- [x] Create probe scenario generator in `src/discovery/probe_scenarios.rs`
- [x] Implement pool executor in `src/discovery/probe_pool.rs` (6-8 parallel subprocesses)

### Phase 3: Digest Diff Integration
- [x] Coordinate with `feat/state-digest` branch for the opaque u64 per-frame API
- [x] Implement hash-only `diff_trace()` in `src/discovery/diff.rs` (MVP, shipped)
- [x] `changed_signals` is NOT available in the MVP — it would require a snapshot
  vocabulary that the merged state-digest does not expose; optionally add
  `changed_signals: Option<Vec<String>>` to `Effect` behind a future snapshot mode

### Phase 4: Cache Layer
- [x] Define `CacheKey` struct, serialization
- [x] Implement cache read/write in `src/discovery/cache.rs`
- [x] Integrate into discovery flow: try cache miss → re-probe → write (`src/discovery/flow.rs`)

### Phase 5: Report Integration
- [x] Add `DiscoveredSurface` to `PlaytestReport`
- [x] Wire `discovered_surface` into battery output (opt-in: `BEVY_PLAYTEST_DISCOVER=1`)
- [x] Annotate frozen_world findings with `_meta`

### Phase 6: Group Completion
- [x] Implement `apply_group_completion()` (WASD/arrows: if one key in a movement cluster diverges, mark its siblings as group-completed)
- [x] Mark inferred siblings `KeyConfidence::Inferred` (statistical) — NEVER `Exact` (see DiscoveredSurface struct)
- [x] Ensure dead-key siblings appear in surface with "group-completion" marker

### Phase 7: CI & Documentation
- [x] Update README with discovery workflow
- [x] ~~Add `--no-discovery` flag for CI override~~ (superseded: discovery is opt-IN via
  `BEVY_PLAYTEST_DISCOVER=1` per Open Question 1's recommendation; no opt-out flag needed)
- [x] ~~Write migration guide for existing scenario authors~~ (superseded: discovery is
  opt-in and additive — zero-contract means existing scenarios need no migration; the
  how-to guide covers the workflow:
  `docs/how-to/how-to-discover-your-input-surface.md`)

## Estimated Implementation Size

- **Lines of code:** ~1200-1500 (including tests)
- **Files modified:** 6-8 (`scenario.rs`, `state.rs`, `battery.rs`, new `discovery/` module)
- **Files created:** 4-5 (`static_scan.rs`, `probe_scenarios.rs`, `probe_pool.rs`, `diff.rs`, `cache.rs`)
- **Estimated time:** 2-3 days focused (assuming `state-digest` branch merges cleanly)

## Open Questions

1. **Should discovery run automatically on first battery, or opt-in?**
   - RESOLVED (Phase 7): Opt-in via `BEVY_PLAYTEST_DISCOVER=1`, per this recommendation.
     A cold cache costs ~40s of subprocess probing — silent surprise costs for every
     battery run outweigh convenience; revisit auto-enable if caching proves invisible
     in practice. Candidate-list and cache-dir overrides:
     `BEVY_PLAYTEST_DISCOVER_KEYS` / `BEVY_PLAYTEST_DISCOVER_CACHE` (parent-side only;
     children strip all `BEVY_PLAYTEST_*` per the strict takeover contract).

2. **How to handle games that crash during static scan init?**
   - Largely moot: the static scan runs in the game CHILD as an export step
     (`BEVY_PLAYTEST_DISCOVER_SCAN` on the first control run, executed after the
     scenario completes — see `flow.rs` docs). It is best-effort and fail-open: a
     failed export costs the reader annotations, never the run or its report (the
     surface falls back to the "static scan not run" note). A probe
     run crashing DURING the scenario surfaces as `DiscoveryError::Probe` — refusal
     over silence, the matrix is never partially interpreted.

3. **Should probe points be configurable per-game?**
   - OPEN (deferred): `ProbePlan` already exposes the knobs (duration, tps) and the
     2-point strategy is the shipped default; per-scenario probe points would ride the
     same plan plumbing when needed.

4. **What if `feat/state-digest` isn't mergeable in time?**
   - RESOLVED: `feat/state-digest` merged at e9592ab (opaque per-frame u64). The MVP is hash-only. If a future snapshot mode lands, `Effect` can gain an optional `changed_signals` field without breaking the hash-only path.

---

*This design doc is versioned with the input-surface discovery feature. Updates should maintain this header and note revision history.*
