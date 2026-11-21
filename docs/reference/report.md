# Report

`bevy_playtest::state::PlaytestReport` — written as JSON to
`BEVY_PLAYTEST_REPORT` (default: `<scenario>.report.json`).

- `status`: `Pass | Fail | Crash`
- `violations`: `[{ rule, target, first_frame, last_frame, count, detail,
  last_detail }]` — aggregated by `(rule, target)`; `target` semantics
  vary by rule: the invariant's `name` for scenario bounds checks, the
  entity id for the global bounds oracle, the literal `"world"` for
  `frozen_world`
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
- `harness_mode`: `"in_process"`
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
