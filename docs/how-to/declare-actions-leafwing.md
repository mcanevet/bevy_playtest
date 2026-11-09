# Declare your actions with Leafwing Input Manager

**Problem:** your game uses [Leafwing Input
Manager](https://leafwing-studios.com/projects/leafwing-input-manager/)
(LW) to declare its bindings in `InputMap`s, and you want discovery
to read those declarations — so the report can distinguish "this key
is dead wiring" from "this key is a live binding the sweep happened
to miss", and so declared-but-dead bindings actually alarm.

**Solution:** enable the `lw-reader` feature on `bevy_playtest`. The
declared tier is pure reflection — the harness links **no** Leafwing
code; it walks your game's live `InputMap`s and `ActionState`s at
runtime, exactly like it reads any other reflected resource:

```toml
[dependencies]
bevy_playtest = { version = "…", features = ["lw-reader"] }
```

## What gets read

During a playtest run the reader snapshots every binding from every
`InputMap<A>` it can find through the type registry:

- **Multiple maps and multiple `Actionlike` types** coexist fine —
  each row records which map it came from (`action_map`) plus the
  action's variant label and the binding's input, normalized to the
  same key spellings the sweep uses (e.g. `KeyJ`; mouse buttons join
  the same canonical mouse names — `Left`, `Right`, `Middle`,
  `Back`, `Forward` — the scenario space uses).
- **RON-loaded maps work.** A map parsed from a RON string or asset
  at startup is indistinguishable from a code-built one — the reader
  reads the deserialized value, never the source form.
- **Instances conflate by design.** The `action_map` on each row is
  the reflected TYPE path, not the entity: two components of the
  same `InputMap<A>` type (a per-player clone, a pause-overlay map)
  read as ONE row set. Same-type instances carrying different
  bindings union into that set, and the tap attributes a press to
  the (map type, action) pair — never to one player's instance.
  Per-instance attribution needs distinct action-map TYPES.
- **Chord bindings (`ButtonlikeChord`)** are marked `chord: true` —
  they fire only with all members held, so single-key probes are
  expected misses and never alarm material.
- An `action_state` snapshot: whether each action is enabled or
  disabled.

The declaration snapshot is one-shot: bindings the game builds or
loads after the reader's snapshot frame are not enumerated (same
class of limitation as the sweep's init-time binding read). Mid-run
REBINDS are covered by the same caveat from the other side: a key
rebound after the snapshot keeps reporting its snapshot-time
binding, so the report can disagree with the game's CURRENT keybinds
settings screen — compare against what was bound when the run
started.

Everything rides the playtest report out as `lw_declared` — absent
when the feature is off or your game has no Leafwing state, so
consumers can tell "nothing declared" from "reader not compiled in".

## What the intent-tap observes

The tap is a wiring-liveness check and **only** that: it correlates
the synthetic bot's driver presses with action firings at the
`"manager_pipeline"` stage — did the press reach your input manager
and match a binding? Entries whose tap corroborates the declaration
earn `grade: manager_pipeline_ok` plus `TapObserved` evidence.

A binding that fires with no game effect still grades
`manager_pipeline_ok` — **wiring liveness is not game liveness**.
The report says so via a `context_unverified` finding (wiring fired,
no effect verified outside boot context), never by an alarm.

## Alarms: declared-but-dead only

The one alarm in the declared tier is `declared_but_dead`, and it
requires proof of dead wiring, in exactly one of two arms:

- the action was **disabled** (per-action or component-wide, via
  Leafwing's `ActionState`) **and never fired** across the run — a
  firing overrides the disabled observation (a disabled-then-
  re-enabled action is alive), or
- the driver **pressed THIS binding** and the action **never fired**
  across the whole run. The proof is per binding, not per action:
  an action bound to several keys only alarms for the keys that
  were actually pressed — a healthy sibling binding's press (or
  firing) must never arm an alarm for a key the scenario never
  touched.

It never fires for chord members — including a disabled chord, a
deliberate gap: chord firing is only observable as a group, so no
per-member arm can be proven — nor for bindings the scenario never
pressed, nor for fired-but-ignored actions, nor for a disabled
action that went on to fire. Zero false taps is the channel's
survival condition.

## Reconciling against the sweep

Since D1-d the reconcile is WIRED into the discovery sweep: every
probe child's declared rows and tap stats aggregate into one
run-equivalent evidence, and the resulting section rides the surface
and the draft as an additive `lw` field — no caller-side assembly
required. Games without Leafwing state (and `lw-reader`-off builds)
simply omit it.

The section is a REPORT, not an edit: it joins the declared tier to
the draft sweep's measured surface — declared inputs upgrade to
`manager_pipeline_ok` when the tap corroborates — but the draft's
own entries and findings stay untouched, including the
`discovered_but_undeclared` residue on inputs the reader declared.
Those inputs are listed in the section's `retracted_residue` for a
CONSUMER to apply (the reader's source citation supersedes the raw
finding when you fold the section into your curated surface); the
draft itself keeps its findings ↔ live-inputs bijection intact.
The determinism gate extends unchanged — identical runs produce
byte-identical reconcile output.
