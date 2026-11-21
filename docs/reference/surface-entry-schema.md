# SurfaceEntry schema (report IR)

The canonical intermediate representation for input-surface
observations: `bevy_playtest::discovery::surface::SurfaceEntry`,
plus the provenance grades, context witnesses, evidence kinds and
finding vocabulary that surround it. The schema is
**forward-complete**: grades and witnesses exist that no producer
emits yet, so every name below is contract, none of it provisional.
Today the sole producer is the Leafwing reconcile (reported as
`lw_reconciled` on `lw-reader` runs), which emits `declared` /
`manager_pipeline_ok` grades in a `Boot` context; the other grades
and evidence kinds await their producers.

The honest-claim rule for this page: a datum says what was observed
and nothing more; silence about a claim is never an upgrade of it.

## SurfaceEntry

One input, one context, one observed effect:

```rust
pub struct SurfaceEntry {
    pub input: String,                       // normalized name (the scenario schema's key spelling)
    pub grade: ProvenanceGrade,
    pub action_label: Option<String>,         // declared intent name, None for raw discoveries
    pub context: ContextWitness,
    pub effect_signature: Option<Effect>,     // discovery::diff::Effect, reused verbatim
    pub evidence: Vec<Evidence>,             // how the grade was earned
    pub mutable: bool,                       // declared source is runtime-mutable (rebind menus)
    pub profile: Option<BuildProfile>,       // "debug" | "release"
    pub notes: Vec<String>,                  // scan-caveat style qualifiers
}
```

Entries live on `ReconciledLw.entries` — inside the report's
`lw_reconciled` field, additive and default-absent. The reconcile
builds one entry per declared binding: grade `declared`
(`Evidence::DeclaredIn` only) or `manager_pipeline_ok` (the tap
corroborated), `context: Boot`, `effect_signature: None` (no effect
signature observation exists yet). Negatives — ruled-out candidates —
are not produced today: the `ProbeRun`/`ProbeRunInert` evidence kinds
that would back them await their producer.

### `mutable` and `profile`

- `mutable: true` marks a declared binding whose source is
  runtime-mutable (rebind menus). Its purpose is alarm semantics: a
  mutable binding's probe-vs-snapshot divergence is **provenance**
  (the snapshot was taken at init), not necessarily drift. No
  producer detects mutability yet; the field defaults to `false`.
- `profile` records which build profile produced the surface:
  `#[cfg(debug_assertions)]`-gated keys make dev-profile surfaces
  larger than release ones. Compare surfaces across profiles and
  you will see "bindings" that only exist in debug builds.

## ProvenanceGrade

How the entry earned its place. Wire names are snake_case:

| Grade | Asserts | Never asserts |
|---|---|---|
| `declared` | the game code declares this binding (Leafwing `InputMap`, bespoke resource) — what the reconcile emits when no tap corroborates the wiring | that the binding is live — declared + refuted is the `declared_but_dead` alarm |
| `source_cited` | a file:line source location references this input | that the referenced code runs (run conditions, dead code) — no producer emits it yet |
| `probe_verified` | pressing the input changed game state at the probed point, under the probe build profile and context; an inert entry with `probe_run_inert` evidence asserts the opposite fact — the probe RAN and observed no divergence, i.e. the input was ruled out at that context, never that it is dead elsewhere | *what* changed (hash-only MVP), or persistence in other contexts — no producer emits it yet |
| `suspect_shape` | nothing mechanical — a shape-detected candidate, not yet scan-linked | anything: reportable, **never alarm-grade** — no producer emits it yet |
| `llm_claimed` | nothing about the game — a model's claim, grounded in `SourceRef` citations a mechanical checker verifies | authority of any kind; no producer exists (opt-in prep step, deferred) |
| `manager_pipeline_ok` | the driver's input reached the input manager and matched a binding (**wiring** liveness) | game liveness: a wiring-forgotten binding fires the action with zero world effect |

`grade_is_consistent()` checks the minimum pairing — grade vs.
evidence shape (`declared` needs `DeclaredIn`, `source_cited` and
`llm_claimed` need `SourceRef`, `probe_verified` needs `ProbeRun`
(live) or `ProbeRunInert` (ruled-out), `manager_pipeline_ok` needs
`TapObserved`; `suspect_shape` needs nothing by design). Consistent
means "the required evidence is present", never "the evidence is
truthful" — that is the mutation harness's job.

## ContextWitness

A witness records when an entry **is** live, and never encodes
"dead when not" — focus present does not imply gameplay-dead when
absent (radial menus, gameplay without focusable entities, one
global `InputFocus`).

| Witness | Meaning |
|---|---|
| `Boot` | observed at boot context, **without** a recorded fingerprint — genuinely uncharacterized beyond "ordinary startup state" (what the Leafwing reconcile emits) |
| `Focused { widget_kind }` | live when a focusable entity of `widget_kind` is focused (bevy_input_focus witness channel) — no producer emits it yet |
| `Fingerprinted { snapshot }` | observed under a described state: (resource → rendered value) pairs captured **at probe time**, serialized as a plain JSON map (sorted) — what a bare `Boot` witness cannot honestly claim; no producer emits it yet |
| `Scanned { condition_hint }` | source-scan-linked run-condition description — no producer emits it yet |

`ContextWitness::describe()` renders a non-empty human string for
reports — which context, or "uncharacterized", never silence.

## Evidence

Structured and extensible; an entry's evidence list is how it earned
its grade. Produced today: `DeclaredIn` and `TapObserved` (the
Leafwing reconcile). The others await their producers:

- `ProbeRun { frame, effect }` — differential divergence against a
  control baseline (frame is absolute).
- `ProbeRunInert { frame }` — the probe ran to completion at `frame`
  and observed no divergence: evidence for a negative. The inert
  counterpart of `ProbeRun`; backs inert entries' `probe_verified`
  grade so ruled-out candidates can appear as entries without ever
  claiming liveness.
- `TapObserved { stage }` — intent-tap observed at a named pipeline
  stage. The Leafwing reader's post-update tap records
  `TapObserved { stage: "manager_pipeline" }` on a binding whose
  driver press fired the action — **wiring** liveness, never game
  liveness (see `manager_pipeline_ok` above). This is the evidence
  behind every `manager_pipeline_ok` grade.
- `SourceRef { file, line, enclosing }` — a mechanically-checkable
  citation. Reserved for the source-derived tier
  (`discovery::source_scan`): the scan already collects its hits in
  this shape for the chaos-key layering, but no producer emits
  `SourceRef`-backed entries yet.
- `DeclaredIn { source_kind }` — e.g. `"leafwing_input_map"`.

## Discovery findings

The disagreement channel between methods — findings annotate
(`ReconciledLw.findings`, inside `lw_reconciled`), they never mutate
the entries they discuss:

- `declared_but_dead` — **alarm grade**: declared, no live evidence at
  the characterized context. The one alarm-grade finding; the
  mutation-harness negative control exists to prove this channel
  fires.
- `discovered_but_undeclared` — residue: probe/source evidence with
  no declared source naming the input.
- `possibly_consumed_by_unknown_pipeline` — probe saw nothing AND
  upstream consumption is unexplained (gui-focus class). Distinct
  from inert: tracked separately because an official consumption
  answer may arrive (FocusedInput bubbling); either outcome is news.
- `context_unverified` — declared and tap-ok but never verified
  outside `Boot`: the honest coverage grade for context-gated
  actions (a context-gated action cannot false-alarm frozen-world,
  and the report must not silently call it `verified`). Emitted only
  by `reconcile` when independent liveness evidence exists; the
  per-run path (`reconcile_without_liveness`) never emits it.

Each finding is `{ kind, input, detail }` — the structured fact is
the kind + the entry it discusses; `detail` is the human one-liner.
