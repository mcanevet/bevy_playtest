# Draft your input surface with one command

**Problem:** you want a standing artifact of which inputs your game
actually responds to — not attached to a battery run, but a
standalone, authoring-time document you can read, diff, and hand to
curation and to teammates.

**Solution:** run the `bevy_playtest_discover` CLI. It drives the
tier-0 sweep — a pool of probe subprocesses taps each candidate key
and diffs the state trace against an identical control run — and
writes `surface.draft.json`:

```console
cargo run --features takeover --bin bevy_playtest_discover -- \
  --binary path/to/your/game \
  --cache-dir .bevy_playtest/cache \
  --out surface.draft.json
```

Flags: `--binary` is required; `--keys KeyW,Digit1,Space` narrows
the matrix (aliases normalize to canonical spellings; an empty list
is refused, never silently widened to the full default set);
`--cache-dir` defaults to `.bevy_playtest/cache`; `--out` defaults to
`surface.draft.json`; `--fixture-name` labels the cache entry so
multi-scenario fixtures never share surfaces. Repeated flags are
last-wins: the final occurrence supplies the value. The first run costs
seconds-to-minutes depending on the matrix; repeat runs answer from
the cache and cost nothing.

An **all-inert surface is exit 0** — "this game ignores every
candidate key at boot" is an answer, not an error. Non-zero exits
mean the sweep itself failed (probe crash, unreadable binary) or the
artifact could not be written; usage errors additionally print the
one-line help.

## Reading the draft

The artifact header says `"draft": true` — a machine-drafted surface
awaiting curation, never the committed `surface.json` contract (that
lifecycle is a later step). It has no timestamps: two sweeps of the
same binary produce byte-identical files, so drafts diff cleanly in
review.

- **Live entries** carry `grade: probe_verified` and an
  `effect_signature` — pressing that input measurably changed game
  state at the probed point. Each also gets a
  `discoveredButUndeclared` finding: the sweep found it live, and no
  binding source declared it. That finding does **not** say the
  binding is a bug or unknown to the code — only that the draft
  knows an effect exists and not where it is declared. Curation's
  job is attaching the declaration; the sweep's job stops at the
  observation.
- **Inert entries** carry the same `probe_verified` grade — the probe
  method vouched for the *observation* — but `effect_signature:
  null` and `probe_run_inert` evidence: the probe ran to completion
  and saw no divergence, so this input was **ruled out** at boot
  under this build profile. Ruled out is a claim about one context,
  never about others; see "missed by design" below.
- **Coverage** partitions the probed candidates (`verified` live,
  `inert` ruled out) and, crucially, lists the **missed by design**
  limitation classes by name:
  - *chorded bindings* — a chord's member keys probe inert because
    each half alone may move nothing;
  - *debounced/cooldown bindings* — a 1-frame tap inside a cooldown
    arms nothing the probe sees;
  - *context-gated bindings* — probes run at boot, so bindings live
    only in menus, focus, or later game states read as inert.

  An inert entry is therefore not a death certificate — it is a
  ruled-out-at-boot observation, and the limitations name what such
  a verdict cannot cover.

The control key never appears as an entry: its probe run equals its
own baseline by construction, so no verdict about it is possible
either way, and the draft says nothing rather than something
unfounded.

## What to do with it

Hand the draft to curation: verify the live entries against your
bindings (each one is residue until declared), treat the inert
entries as your tested negative space, and read the
missed-by-design list as the checklist of places a real binding
could hide from this method. The build profile rides along
(`build_profile`) because `#[cfg(debug_assertions)]` bindings make
dev-profile surfaces strictly larger — compare drafts across
profiles knowingly, never blindly.

For the battery-time variant (surface attached to the playtest
report, discovery-driven bot injection), see
[discover your input surface](how-to-discover-your-input-surface.md);
for the schema of each entry, see the
[surface entry schema](../reference/surface-entry-schema.md).
