# Report

`bevy_playtest::state::PlaytestReport` — written as JSON to
`BEVY_PLAYTEST_REPORT` (default: `<scenario>.report.json`).

- `status`: `Pass | Fail | Crash`
- `violations`: `[{ rule, target, first_frame, last_frame, count, detail,
  last_detail }]` — aggregated by `(rule, target)`; `target` semantics
  vary by rule: the invariant's `name` for scenario bounds checks, the
  entity id for the global bounds oracle, the literal `"world"` for
  `frozen_world`. Informational entries ride the same list: a
  WARN-ONLY rule (`occupancy_cap` today — see the proposal §1/§8
  ship-gate honesty) renders its findings as ordinary entries here
  on a status `"Pass"` report; there is no separate
  informational section, and `_meta.unpromoted_rule` plus the
  detail's warn-only sentence mark the posture. Notes ride it
  too: `derived_transform_consistency`'s `hierarchy_absent`
  (truly flat world) and `unjudgeable_parent` (hierarchy in
  scope, parent never readable — full or partial blindness,
  disclosed) are informational `(rule, target)` entries on a
  passing report
- `finding_summary`: top-level skim-guard block (absent on crash
  stubs and pre-field reports; additive). `gating` /
  `warn_only` split the `violations` rows by verdict class under
  the SAME escalation context the verdict used:
  `strict_warnings` escalates `log_warn` /
  `progress_surface_empty` to gating and
  `liveness.progress.require_surface` escalates
  `progress_surface_empty` — the summary can never disagree with
  the status line. `warn_only` covers informational notes and
  warn-only rules' findings (`occupancy_cap` today), plus
  `log_warn`/`progress_surface_empty` on runs that did not
  escalate them. `near_misses` rows
  carry the pass-side near-miss story for passing episode-tier
  invariants that discarded at least one sub-soak episode:
  `invariant`, `rule`, `discarded_subsoak_episodes`, the worst
  discarded episode's `worst_span` / `worst_span_frame` against
  its authored `soak` (`window_s`, same driver units), and
  `span_vs_soak` — how close this pass came to failing. Worst
  first, capped at 8 rows; the per-invariant detail stays in
  `held_invariants`. Guidance: pair any soak-family finding on a
  failing run with the passing side's `discarded_subsoak_episodes`
  check — episodes that ALMOST soaked are the calibration signal,
  never noise
- `metrics`: `{ input_count, frame_count, worst_frame_ms, crash_detected }`
- `schema_version`, `frame_count`, `tps`, `simulated_duration_s`,
  `wall_time_s`, `error`
- `action_log`: `[{ frame, source, action, details }]` — audited bot
  inputs; `details` on a `key_press` entry names the canonical key the
  draw resolved to (the `action` value indexes the draw-time unheld-
  candidate list, which the report does not contain)
- `choices` (the bot's recorded random draws): `{ frame, value, bound,
  kind, label }` — `kind` separates key-index draws (`key`), explore-mode
  combo draws (`key_combo`), per-press
  hold-duration draws (`key_hold_frames`), and mouse draws
  (`mouse_position`, `mouse_button`); `label` (present on key-index and
  combo draws) names the selected key
- `state_digest`: `[{ frame, hash }]` — per-frame broadened state digests,
  present only when the scenario sets `record_digest: true`; omitted
  (empty) otherwise
- `progress_samples`: `[{ frame, surface_hash, churn, channels }]` —
  the progress-oracle evidence series (frame-indexed, deterministic;
  `surface_hash` is the canonical hash of the whole observable
  surface, `churn` says the frame loop was alive, `channels` counts the
  channels in scope). Non-empty whenever
  `liveness.progress.mode` is not `"off"`.
- `held_invariants` (absent when no invariant passed):
  the pass-side evidence trace — one compact object per PASSING
  `resource_check` / `stagnant_resource` / `component_check` /
  `nodes_in_bounds` / `pairwise_separation` /
  `derived_transform_consistency` / `spawn_safety` / `forbidden_region` /
  `entity_stagnation` / `occupancy_cap` invariant,
  the mirror of the
  violations list for invariants that held. A passing run previously
  carried zero per-invariant evidence (a deadline met at frame 299 of
  300 was indistinguishable from one met at frame 3):
  - `name`, `rule`, `target` — the invariant's identity, the rule
    kind, and the reflect target (`Resource.leaf` for the resource
    tier; `Component.leaf` for `component_check`, the selector
    component for `nodes_in_bounds`, `all` for the all-selector;
    for the episode rules the selector component, or `all` for
    DTC's whole-hierarchy scope; `forbidden_region` names its
    selector component; for `spawn_safety` the selector
    component; for `entity_stagnation` the selector component; for
    `occupancy_cap` the selector component)
  - `frames_matched` — frames on which the check resolved and
    judged something (existence evidence; `frames_matched: 0` plus a
    `vacuous_invariant` finding is the wrong-path signal, since
    vacuous invariants get no entry; for the component tier, the
    frames the selector matched ≥1 entity; for the episode rules,
    the frames the rule had something to judge — pairwise: ≥2
    checkable entities co-existed, DTC: ≥1 checkable parented
    entity past the grace window, forbidden_region: ≥1 checkable
    selected entity, entity_stagnation: ≥1 sampled frame the
    selector enumerated entities, digests or not; `occupancy_cap`:
    ≥1 checkable entity offered — a density read runs every
    frame; for `spawn_safety`, the frames
    with ≥1 resolvable anchor — the frames an incoming entry
    would have been judged on had one arrived)
  - `predicate` (`resource_check` and `component_check`: the
    authored predicate in wire syntax, e.g. `ge 0`; episode rules:
    the rule's threshold rendered the same way — `ge <min_distance>`
    for pairwise, `le <tolerance>` for DTC with the documented
    default when the author omitted it, `inside at most <window_s>
    driver units` for forbidden_region, `window <window_s>` for
    entity_stagnation (the soak threshold the episode judge
    compares against); `occupancy_cap` renders
    `count le <cap>` (the neighborhood bound, self-inclusive);
    `spawn_safety` renders
    `ge <min_distance>`, pairwise-shaped)
  - `observed_min` / `observed_max` (`resource_check` only) —
    extrema of the sampled leaf across matched frames
  - `worst_frame` (`resource_check`, ordered predicates only) — the
    frame of the extremum closest to violating (`lt`/`le` → the max,
    `gt`/`ge` → the min; earliest frame wins ties); omitted for
    `equals`/`ne`, where "closest to violating" is not well-ordered
  - `met_at_frame` / `deadline_frame` (eventually-mode only) — the
    first frame the predicate held and the audited deadline
    (`eventually_s × tps`, ceiled)
  - `episodes_fired` / `driver_advanced` / `driver`
    (`stagnant_resource` only — `entity_stagnation` keeps
    `episodes_fired` at zero on passing runs too, its findings
    arriving as soaked episodes instead; the per-run episode
    story lives in `episodes_observed`) — stagnation episodes that fired (0 on
    a passing run) and the run-total driver-leaf advance (sum of
    positive deltas across fully-observed frames; gaps and rewinds
    are never credited), plus the driver reflect path
  - `eventually_s` / `window_s` — the authored parameters, when
    present; `window_s` rides the episode rules' entries too (the
    soak the worst-span witness is compared against)
  - `entity_judgments` (component tier: `component_check` /
    `nodes_in_bounds`; also `derived_transform_consistency`;
    `spawn_safety` reuses the field for run-total ENTRIES judged —
    once per entry, not per frame, the transition-frame doctrine;
    `forbidden_region` sums the selected entities per judged frame;
    `occupancy_cap` sums the checkable entities per judged frame —
    a density rule judges entities, never pairs, so it rides this
    slot, never `pair_judgments`)
    —
    run-total entity-judgments: the sum of
    per-frame matched-entity counts over observed frames. A
    workload figure ("N entity-judgments were made"), not a census —
    an entity surviving many frames is counted once per observed
    frame
  - `peak_entities` / `peak_entities_frame` (component tier) — the
    largest scope any single frame presented and the frame it was
    first seen at (earliest frame wins ties)
  - `nearest_margin` / `nearest_margin_frame` (`component_check`
    only) — the smallest margin by which any judged entity's value
    cleared the threshold on a frame where every judged entity held,
    and the frame of that sample: the component tier's "how close did
    it get" evidence. Bounds judge containment, not a thresholded
    leaf, so `nodes_in_bounds` carries workload fields only
  - `episodes_observed` (episode rules: `pairwise_separation`,
    `derived_transform_consistency`, `forbidden_region`,
    `entity_stagnation`, `occupancy_cap`) — total
    episodes observed this
    run INCLUDING the sub-soak ones the soak judgment discarded as
    transients: a passing run's episodes are precisely its
    near-misses, so this is the rule's exercise story
  - `worst_episode_span` / `worst_episode_frame` (episode rules) —
    the worst sub-soak episode's driver span (the largest over every
    episode this run) and the frame it started at: the "how close did
    it get to soaking" witness. Compare against the entry's
    `window_s`. The span is in the driver leaf's own unit — seconds
    only for the frame-clock default (a declared game clock's unit,
    never converted through tps). The soak judgment discards
    strictly-sub-window spans only, so a held record's span is
    always strictly below `window_s`; an at-window episode fails
    the run and earns no entry (warn-only rules excepted: a
    soaked occupancy crowd WARNs — the run still passes, the
    fired rule earning no held entry either way)
  - `pair_judgments` (`pairwise_separation` only) — run-total
    pair-judgments: `n·(n−1)/2` per frame where the selector
    resolved and ≥2 checkable entities co-existed, summed over the
    run. A workload figure, not a census (pairs re-enumerated each
    frame are re-counted) — the component tier's
    `entity_judgments` doctrine, which `derived_transform_consistency`
    reuses directly: its `entity_judgments` sums the checkable
    parented entities per judged frame; `forbidden_region` the
    same way — its `entity_judgments` sums the selected entities
    per judged frame (a workload figure, not a census);
    `entity_stagnation` per SAMPLED frame — one digest read per
    selected entity at every cadence tick the selector enumerated,
    so a watch that judged at every sample reports its per-sample
    workload even on a clean run (a lull, judged, is still
    evidence the watch was live)
  - `entries_clean` (`spawn_safety` only) — run-total entries judged
    CLEAN (entered at least `min_distance` from every same-frame
    anchor): together with its `entity_judgments` (ALL entries
    judged — once per entry, not per frame, the transition-frame
    doctrine) it separates the rule that exercised N entries from
    the never-exercised twin; a zero-entries run earns no held
    entry at all (an archetype that never gained a member proved
    nothing about entry safety)
  - `closest_approach` / `closest_approach_frame`
    (`pairwise_separation` only) — the smallest pair distance seen
    over the whole run and the frame of that sample: the
    HEADROOM story, predicate-independent. How close the closest
    pair came — including distances BELOW `min_distance` (a
    transient the episode fold handled as a sub-soak near-miss
    while the run still passed). Compare against the entry's
    predicate; a passing run can legally carry a value below it,
    and that is precisely the disclosed near-miss. Only frames
    where the selector resolved and ≥2 checkable entities
    co-existed contribute, and finite distances only — a judged
    frame whose every pair distance came back non-finite
    contributes nothing, so an entry can carry
    `frames_matched > 0` with both fields absent. The earliest
    frame wins ties
  - `peak_witness_count` / `peak_witness_frame` (`occupancy_cap`
    only) — the largest self-inclusive neighborhood count any
    entity presented over the run and the frame of that sample:
    the CROWD story, predicate-independent (the witness predicate
    is `count > cap`). How big the biggest neighborhood got —
    including legal counts a hair under the cap, and ABOVE-cap
    transients on a passing run whose discarded sub-soak episodes
    show alongside `episodes_observed`. Compare against the
    entry's `cap` authoring; a soaked above-cap crowd earns no
    held entry — the rule is warn-only, so the run itself still
    passes — and the warning shows in `violations[]` instead.
    Only populated frames contribute; the earliest frame wins
    ties
- the held-trace ADMISSION split for `entity_stagnation`: an entry
  appears when the selector RESOLVED checkable entities OR the
  watch was watchable (its component registered with a resolvable
  watch path — both markers exist in the bookkeeping for exactly
  this story). A live watch the world never offered a qualifying
  `leaf_where` state keeps its entry with ZERO judgments — a PASS
  WITH EVIDENCE, never vacuity; only a selector that never
  resolved at all is `vacuous_invariant` (the run already failed
  on it), and a zero-instance watch earns no held entry via the
  resolution marker alone. `occupancy_cap` partitions the same
  way a warn-only rule must: a run whose census judged crowds —
  including a run WARNED for a soaked one — keeps no held entry
  for the FIRED rule (the violations[] warning and the held
  trace partition by verdict even on a passing report), while a
  never-a-crowd selector (informational note emitted, no
  census workload) claims no held evidence either
- the violation-entry `_meta` block (wire key `_meta`): rule-owned
  structured evidence appended to findings. `pairwise_episode`
  (`sampled_pairs`, capped `roster` of per-pair worst-first
  frames + distances, `roster_truncated`),
  `consistency_episode` (same shape for divergent entities), and
  `forbidden_region`'s `forbidden_episode`:
  `sampled_entities` (distinct incursive entities this episode),
  `roster` — per-entity `entity` (index), `first_frame`,
  `worst_depth` (penetration depth to the nearest face; deeper is
  worse), `worst_frame`, `violating_frames` — capped at 32 with
  `roster_truncated` disclosing the drop, so a crowd cannot hide
  behind the cap. `stagnation_episode` for `entity_stagnation`
  findings: `sampled_entities` (distinct stuck entities this
  episode), `roster` — per-entity `entity` (index) AND
  `generation` (slot recycling must never weld a successor to
  its predecessor's stint), `first_frame`, `last_frame` (the
  stint's end: change, despawn, or run end), `stuck_span` (the
  entity's accrued stuck-span in driver-leaf units, frozen at
  stint end), `stuck_frames` (sampled frames stuck) — worst-first
  (longest span, then earliest frame), capped at 32 with
  `roster_truncated` disclosing the drop. `occupancy_episode` for
  `occupancy_cap` findings: `sampled_entities` (distinct
  WITNESSING entities this episode — each saw a self-inclusive
  neighborhood count over cap), `radius` and `cap` echoed (the
  evidence is self-describing), `roster` — per-entity `entity`
  (index) AND `generation` (slot recycling must never weld a
  successor to its predecessor's crowd), `first_frame`,
  `worst_frame`, `worst_count` (the highest self-inclusive
  crowd count the entity witnessed), `violating_frames` —
  worst-first (highest count, then earliest frame), capped at 32
  with `roster_truncated` disclosing the drop. The occupancy
  finding also carries `unpromoted_rule: true` in its `_meta`
  (see below). `stepping` (step-capture
  runs) predates the
  episode blocks. All fields are wire-stable names
- `harness_mode`: `"in_process"` for a real run, or `"lint"` for a
  lint-mode artifact (see below)
- `scenario_source`: `"file"`, `"builtin_smoke"`, or `"builtin_battery"`
- `log_capture_available` (absent when not applicable; `log_capture_available: false` also produces an
  informational `log_capture_lost` violation — see the
  [lost-log-capture how-to](../how-to/how-to-handle-lost-log-capture.md);
  when the capture subscriber wins but the log→tracing bridge cannot
  install, an informational `log_bridge_lost` violation is produced
  instead)
- `log_bridge_available` (absent when not applicable; `false` means
  another `log` crate logger owned the write-once slot, so
  `log`-crate events — bevy's B000x hierarchy validators, wgpu/naga —
  are invisible to `log_warn`/`log_error` even when
  `log_capture_available` is `true`; paired with the informational
  `log_bridge_lost` violation, which is emitted when the capture
  subscriber won and not silenced by the
  `BEVY_PLAYTEST_ALLOW_MISSING_LOG_CAPTURE` escape hatch)
- `game_build_profile` (absent on stubs and old reports): the
  reporting game binary's own debug/release profile — dev-profile
  behavior differs from release (`#[cfg(debug_assertions)]` keys)
- `source_derived_keys` / `source_derived_notes` (present exactly
  when the battery's source scan replaced the default chaos keys):
  the poison-filtered key set that replaced WASD+arrows, plus
  provenance notes (cap drops, unreadable files). `None` means no
  replacement happened, never "the scan found nothing"
- `lw_declared`, `lw_tap_stats` (`lw-reader` builds): the game's
  declared Leafwing bindings as read from its `InputMap`s, and the
  per-run intent-tap press/fire statistics
- `state_trace` (absent on crash stubs): state schedules that ran
  (and never ran) and game resources that changed — the evidence
  behind the committed exploration targets
- `randomness` (stamped by the battery parent): whether the
  deterministic-randomness shim intercepted OS-seeded randomness,
  and how or why not
- `exploration` (`bot.explore` runs only): screens seen, widgets
  never clicked, keys never pressed, and a saturation verdict
- `capability_census` (absent on crash stubs): load-time capability
  rows captured before bot/oracle registration — absence never
  means "no census"
- `ui_surface` (`ui-surface` feature): the latest snapshot of the
  declared UI/interactable surface plus bounded delta notes —
  absence never means "no UI"
- `render_liveness` (present only with the `render-liveness` feature
  and a render sub-app in the game; carries `{ cameras_seen,
  sim_active_ever, tier, samples, notes }` — the evidence behind the
  informational `render_liveness` violation; absence means "not
  evaluated", never "healthy")
- `screen_liveness` (present only with the `screen-liveness` feature
  and a render sub-app and an image-asset store in the game; carries
  `{ requests, captures, max_hamming, sim_active_ever, tier, notes }`
  — the pixel-observed evidence behind the informational
  `screen_liveness` violation; absence means "not evaluated", never
  "healthy")

The `log_capture_available`/`log_bridge_lost` and
`log_capture_lost` violation entries are the human-facing layer:
silenced by the `BEVY_PLAYTEST_ALLOW_MISSING_LOG_CAPTURE` escape
hatch (and the bridge entry requires the capture subscriber to have
won, since `log_capture_lost` otherwise already tells the story).
The fields themselves are never silenced — the report always
carries the machine-readable truth.

Report helpers on `PlaytestReport`:

- `violation_rules() -> Vec<&str>` — deduplicated, sorted rule names
- `violation_pairs() -> Vec<(&str, &str)>` — deduplicated, sorted
  (rule, target) pairs
- `has_violation(rule, target) -> bool`

Reports from `lw-reader` builds also carry `lw_reconciled`, whose
entries and findings use the IR specified in
[the SurfaceEntry schema page](surface-entry-schema.md).

## Lint-mode reports

Setting `BEVY_PLAYTEST_LINT` alongside `BEVY_PLAYTEST_SCENARIO`
makes the takeover lint the scenario instead of running it (see
the environment-variable reference). The report a lint produces
uses this same schema, with three distinctions a consumer must
know:

- `harness_mode` is `"lint"`, not `"in_process"` — the game's frame
  loop never executed (but plugins build and ONE warm-up frame
  runs, so the registry probe sees a booted world).
- Every finding rides `violations` as an entry whose `rule` is
  `lint_finding` — one per aggregated scenario validation error,
  plus one per definite resolution error the type-registry probe
  caught (typo'd resource/component names, numeric paths that
  cannot resolve on the booted App, and typo'd progress-surface
  names — `liveness.progress.markers` and exact `include_paths`
  entries, which resolve at the run's first sampled frame and so
  fail just as certainly at run time). The probe is silent about
  what it cannot confirm (a component with no instances yet, a
  `include_paths` PREFIX whose zero-match judgment needs the whole
  run); its findings mirror the run's resolution machinery and message
  builders (`resolve_registration`, `reflect_resource`,
  `reflect_numeric_path`, `resolve_entity_numeric_path`, and the
  progress recorder's own surface resolution), so
  lint clean-ness transfers: a
  scenario that lints clean stays clean for the resolution errors
  the probe checks.
- `status: "fail"` in a lint report says the *config* is incoherent.
  It is never a statement about the game: lint passes mean "the
  scenario is coherent", not "the game will pass". Exit codes
  follow the normal contract (0 coherent / 1 findings / 2 the game
  panicked during the probe's warm-up frame — a boot crash, not a
  lint verdict / 3 infrastructure failure).
