# Known-answer fixture (planted truth)

Reference for `fixtures/known_answer/`: a hand-authored Bevy game
whose entire input surface is deliberately constructed so
measurements against it have known answers. Every binding is a numbered
**plant** — a `// PLANT Pn` / `// PLANT Nn` marker in
`fixtures/known_answer/src/main.rs` — and the committed
`fixtures/known_answer/ground_truth.json` states what each lens must
say. A test cross-checks markers and rows so neither the catalog nor
the truth can drift.

This is the fixture counterpart of a known-answer test in crypto:
measurements are only trustworthy if the measuring instrument has
been checked against inputs whose answers were derived
independently — here, by hand from the code that was written to
produce them.

## Why plants, not showcase games

The fetched showcase fixtures are realistic but their input surfaces
are incidental: nobody wrote down what the "right" answer is, so a
measurement against them can only check internal consistency
(determinism controls), never correctness. The known-answer fixture
plugs that hole: it contains inputs chosen because a naive
instrument gives the wrong answer for them — chords, debounces,
shape-only KeyCode data.

## Plant catalog

Positive plants (real bindings) — grading lens per row is in the
`expected_grade.methods` object of each row. The `probe_visible`
values record what the probe sweep (removed with the discovery
feature) observed, and which live bindings it provably could not
see. They stay as catalog data: the yardstick for any future lens
that measures liveness.

| Plant | Key(s) | Construction | What it pins |
|-------|--------|--------------|--------------|
| P1 | `KeyU` | unmute toggle routed through a mutable `UnmuteKey` resource; live in menu AND gameplay | mutable-binding testbed (runtime rebind to KeyM is wired but dormant — see P1's `mutable: true` row) |
| P2 | `KeyW`/`KeyA`/`KeyS`/`KeyD` | movement through a bespoke `Keymap` resource | bindings flowing through a non-enum resource shape (four separate divergences, no synthetic grouping) |
| P3 | `KeyR` | reset with a reflected `ResetCount` witness | delayed-observable-consequence shape: bare probe taps must still diverge via the witness |
| P4 | `KeyL`+`ShiftLeft` | chord: just-pressed KeyL under held ShiftLeft, toggling reflected `LampOn` | catalog expected miss — single-key 1-frame probes can never satisfy both halves |
| P5 | `KeyT` | debounce: first tap arms an unreflected `ArmedFrame`; reflected `TrickCount` bumps only on a later, out-of-cooldown press | catalog expected miss — an isolated tap leaves the digest trace identical to control |
| P6 | `KeyG` | `#[cfg(debug_assertions)]` debug-overlay toggle | dev-profile surface is strictly larger than release; a dev-profile measurement must find it |
| P7 | `KeyV` | real effect (reflected `VhsFilter` toggle) behind `in_state(GameState::Secret)` — a state no transition enters | context-gated not-discovered: graded inert at boot without drift |

Negative plants (shape-positive, behaviorally-negative — graded
inert across the board; a source-shape method grading them live
would be exactly the false-positive this fixture exists to catch):

| Plant | Key(s) | Construction | Trap |
|-------|--------|--------------|------|
| N1 | `Digit1`..`Digit0` | `[KeyCode; 10]` hotbar where only the first two slots act (reflected `HotbarUse` counter) | array SHAPE reads as ten bindings; truth: two live, eight inert |
| N2 | `Enter` (row; `KeyS` also appears as a preview-map value, but its row is P2's) | `HashMap<MenuAction, KeyCode>` rebind-preview plus a `SettingsRow` component carrying a KeyCode payload | display data, never an input route |
| N3 | wildcard (`"*"` row) | `last_pressed: KeyCode` logger mirroring the last press | structurally a KeyCode field, behaviorally a logger; deliberately NOT reflected — if it were, every probe would diverge from its KeyQ control and the matrix would collapse into false positives |
| N4 | `KeyX` | raw `just_pressed(KeyX)` read writing a log-only unreflected resource | a raw read must never earn a binding grade from shape alone |

Declared-tier plants (real Leafwing Input Manager 0.22 subsystem —
per-binding expectations in the additive `lw_rows` key, graded by
the reader/intent-tap/reconcile lens):

| Plant | Binding | Construction | What it pins |
|-------|---------|---------------|--------------|
| L1 | `DisabledAtSpawn` ← `KeyN` | LW action disabled at spawn | the static `declared_but_dead` alarm: pressed-never-fired with `disabled: true` — the pristine run's ONLY true alarm |
| L2 | `Ignored` ← `KeyO` | declared + enabled, no game handler | wiring fires while the game ignores it → `context_unverified`, never an alarm (wiring liveness ≠ game liveness) |
| L3 | `Combo` ← `ControlLeft`+`KeyI` | Leafwing's own `ButtonlikeChord` | chord members get notes, never findings, never alarms — even when the chord fires under a synchronized multi-press |
| L4 | `Gated` ← `KeyP` | real handler + real reflected `GatedUse` witness, but `run_if(in_state(GameState::Menu))` (menu ends frame 30; later presses act nowhere) | context-gated wiring: fires under tap, unverifiable beyond the boot context → `context_unverified`; behaviorally distinct from L2 (handler exists, state unreachable) |
| L5 | `Live` ← `KeyJ` | unconditional handler bumping reflected `LwUse` | the healthy binding: the reader enumerates it, the tap records pressed-and-fired, reconcile grades `manager_pipeline_ok`. Mutation `LW_dead` disables it at frame 35 — the pressed-never-fired alarm arm, proven on a binding the pristine run grades healthy |

A RON-loaded `InputMap<MenuAction2>` (`RonBound` ← `KeyH`) hosts no
plant of its own but pins reader enumeration: a map parsed from a
RON string at startup must be indistinguishable from a code-built
one. Two `Actionlike` types coexist (multi-map enumeration), and no
handler consumes `MenuAction2`, so `KeyH` shares L2's
`context_unverified` grade.

The `context_unverified` grades above are the ground truth for a
reconcile that HAS independent liveness evidence (a future lens).
The per-run reconcile has none, so it emits only the L1 alarm.

Supporting testbeds (kept for future lenses): a bespoke `Keymap`
resource routing the REAL WASD bindings
(shape and behavior), a `MenuFocus` witness, and a
`RebindMenuActive`-gated runtime rebind that flips P1's binding to
`KeyM` (never active in a run; ground truth carries a KeyM row pinned
inert so the rebind contract is explicit from day one).

## Grading semantics

`expected_grade` in each row is either:

- the literal string `"negative"` — adversarial negative: every
  measurement method must report it inert / not-a-binding; or
- a methods object, e.g.

  ```json
  { "probe_visible": false, "source_declared": false, "boot_live": true }
  ```

  - `probe_visible`: whether the probe sweep (removed) observed a
    divergence when tapping this key at boot context.
  - `source_declared`: whether a declared-binding reader
    (Leafwing-style actionlike) cites a source. `false` for the P/N
    plants (raw reads plus bespoke resources); `true` for the L
    plants, whose bindings live in real `InputMap`s.
  - `boot_live`: the binding genuinely fires in the boot context.
    P4/P5 are `boot_live: true, probe_visible: false` — live
    bindings single-key 1-frame probes are known to miss. They
    stay in the catalog as documented misses, not quietly
    relabeled inert.

The wildcard `"*"` row (N3) makes no per-key claim; graders must
skip it when scoring per-key predictions.

`context` is a narrative label, not a measured axis. Labels like
`menu_focused` or `boot` record where the binding is meant to live
per the plant's construction; they become measurable commitments
once a multi-context lens exists. What is asserted today:
`boot_live` and the negative grades hold against the game's actual
behavior, and the per-binding expectations in `lw_rows` are graded
row by row by the committed reconcile lens (see Measurement below).

## Measurement (`tests/measure.rs`)

The probe-sweep lens that graded liveness against the plant catalog
went away with the discovery feature. What remains asserted:

- the Leafwing declared tier (`lw_reconcile_lens_known_answer_commitments`):
  reader completeness across both maps (RON-loaded binding included),
  intent-tap press/fire correlation row by row, exactly one pristine
  `DeclaredButDead` alarm (L1 KeyN) and no other finding (the per-run
  reconcile has no liveness evidence, so it emits no
  `ContextUnverified`), chord members carrying notes only, and
  byte-identical reconcile output across two runs — all read from the
  report's `lw_reconciled`;
- the ground truth's closed vocabularies.

The catalog's per-row probe expectations stay in `ground_truth.json`:
they describe the game's real behavior and are the yardstick a future
exploration lens will be measured against.

## Mutations (`tests/mutation.rs`)

The game accepts `KNOWN_ANSWER_MUTATION` (see the fixture README
for why the var is deliberately not `BEVY_PLAYTEST_`-prefixed).
`P1_dead` and `N1_hotbar_full` remain available in the game but have
no harness test today (they were graded by the removed sweep).

- `LW_dead` — the healthy Live action (L5, KeyJ) is disabled at
  frame 35, after the reader's declared snapshot but before the
  scenario's frame-40 press: the declared-tier **alarm** direction.
  The intent-tap records KeyJ pressed=1, fired=0, disabled=true —
  the reconcile must add the pressed-never-fired `DeclaredButDead`
  alarm for KeyJ (disabling is what stops the firing, so the
  disabled arm and the pressed-never-fired arm coincide). Together
  with the static L1 plant this proves the alarm channel both ways:
  a binding healthy-then-killed in the same run alarms, and the
  pristine run stays false-tap-free.

## Harness tiers

| Tier | Enabled by | Contents |
|------|-----------|----------|
| Battery | `--features playtest` | standard playtest battery + ground-truth round-trip + marker↔row cross-check (fast, no probes) |
| Measure | `--features playtest,measure` | everything above plus the Leafwing reconcile lens and the `LW_dead` mutation (opt-in only, never in the plain battery) |

Both tiers require `--test-threads=1` (`run_scenario` is exclusive
per process).

## Maintenance rules

- **Never tune the truth to hide a miss.** If a measurement
  surprises, fix the GAME (or accept and assert the miss as an
  expected miss with a `findings_expected` entry) — ground truth
  changes only when the game's actual behavior changes.
- Exactly ONE `// PLANT` marker per plant, at the implementing
  system or its section header; the marker grammar is a line
  beginning `// PLANT ` whose next token is the id. Prose may
  mention ids freely.
- New plants: add the marker, the row(s), and update
  `REQUIRED_MARKERS` in `tests/support/mod.rs` — the cross-check
  test enforces all three stay in lockstep.
