# SurfaceEntry schema (discovery IR)

The canonical intermediate representation for input-surface
discovery results: `bevy_playtest::discovery::surface::SurfaceEntry`,
plus the provenance grades, context witnesses, evidence kinds and
finding vocabulary that surround it. Written against the code as of
this slice: the schema is **forward-complete** (grades and witnesses
exist that no producer emits yet — D1+ producers are filling them in),
so every name below is contract, none of it provisional.

The two-sentence honest-claim discipline this page follows: a datum
says what was observed and nothing more; silence about a claim is
never an upgrade of it. Every grade below carries exactly what it
asserts and what it never asserts.

## SurfaceEntry

One input, one context, one observed effect:

```rust
pub struct SurfaceEntry {
    pub input: String,                       // normalized name (same space as KeyFinding.key)
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

Entries live on `DiscoveredSurface.entries` — additive and
default-empty. The legacy `keys` / `mouse_buttons` lists are untouched
and remain the D0 wire format; `DiscoveredSurface::to_entries()`
projects them mechanically (effective findings become
`ProbeVerified` entries in an unfingerprinted `Boot` context).
The projection is lossy by design — entries are richer; there is no
reverse mapping. Negatives (inert, inferred-no-effect keys) stay
legacy-only *in the D0 projection*, and per-key probe frames are not
retained by the legacy shape — so every projected entry carries a
`notes` caveat saying its `ProbeRun` evidence frame is the surface's
first probe point, not this input's. The disclosure travels with the
artifact.

The D1-c sweep draft extends this: a probed-and-inert candidate ALSO
becomes an entry, because a drafted surface that omits inert keys
hides the residue problem and makes curation impossible. Such an
entry keeps `grade: probe_verified` (the probe method vouched for
the observation) but says so honestly: `effect_signature: null`,
evidence `probe_run_inert` (ran to completion, no divergence), and a
note "probed inert under Exact controls". Nothing was verified live;
the entry records a ruled-out candidate, not a binding.

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

How the entry earned its place. The wire names are the roadmap
vocabulary, snake_case:

| Grade | Asserts | Never asserts |
|---|---|---|
| `declared` | the game code declares this binding (Leafwing `InputMap`, bespoke resource) | that the binding is live — declared + refuted is the `declared_but_dead` alarm |
| `source_cited` | a file:line source location references this input | that the referenced code runs (run conditions, dead code) |
| `probe_verified` | pressing the input changed game state at the probed point, under the probe build profile and context; an inert entry with `probe_run_inert` evidence asserts the opposite fact — the probe RAN and observed no divergence, i.e. the input was ruled out at that context, never that it is dead elsewhere | *what* changed (hash-only MVP), or persistence in other contexts |
| `suspect_shape` | nothing mechanical — a shape-detected candidate, not yet scan-linked | anything: reportable, **never alarm-grade** |
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

POSITIVE-ONLY witness predicates (R3-C): a witness records when an
entry **is** live, and never encodes "dead when not". Focus present
does not imply gameplay-dead when absent — radial menus, gameplay
without focusable entities, one global `InputFocus`.

| Witness | Meaning |
|---|---|
| `Boot` | observed at boot context, **without** a recorded fingerprint — genuinely uncharacterized beyond "ordinary startup state" (legacy shape; what `to_entries` emits) |
| `Focused { widget_kind }` | live when a focusable entity of `widget_kind` is focused (bevy_input_focus witness channel) |
| `Fingerprinted { snapshot }` | observed under a described state: (resource → rendered value) pairs captured **at probe time**, serialized as a plain JSON map (sorted). "Boot context" without a fingerprint is a euphemism; this is the requirement it satisfies |
| `Scanned { condition_hint }` | source-scan-linked run-condition description (D2+ producer; forward-complete now) |

`ContextWitness::describe()` renders a non-empty human string for
reports — which context, or "uncharacterized", never silence.

## Evidence

Structured and extensible; an entry's evidence list is how it earned
its grade:

- `ProbeRun { frame, effect }` — differential divergence against the
  control baseline (frame is absolute, matching `probed_at`).
- `ProbeRunInert { frame }` — the probe ran to completion at `frame`
  and observed NO divergence: evidence for a negative. The inert
  counterpart of `ProbeRun` (D1-c sweep); backs inert entries'
  `probe_verified` grade so ruled-out candidates can appear as
  entries without ever claiming liveness.
- `TapObserved { stage }` — intent-tap observed at a named pipeline
  stage. Example: the Leafwing reader's post-update tap records
  `TapObserved { stage: "manager_pipeline" }` on a binding whose
  driver press fired the action — **wiring** liveness, never game
  liveness (see `manager_pipeline_ok` above). Reconcile: the
  declared tier is joined against the draft sweep — declared inputs
  upgrade to `manager_pipeline_ok` when the tap corroborates and
  their `discovered_but_undeclared` residue is retracted, while
  proven-dead wiring (disabled-without-a-firing, or
  pressed-never-fired) alarms
  `declared_but_dead` and nothing else ever does.
- `SourceRef { file, line, enclosing }` — the mechanically-checkable
  citation format.
- `DeclaredIn { source_kind }` — e.g. `"leafwing_input_map"`.

## Discovery findings

The disagreement channel between methods — findings annotate the
surface (`DiscoveredSurface.findings`, additive, skip-serialized
when empty), they never mutate the entries they discuss:

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
  and the report must not silently call it `verified`).

Each finding is `{ kind, input, detail }` — the structured fact is
the kind + the entry it discusses; `detail` is the human one-liner.
