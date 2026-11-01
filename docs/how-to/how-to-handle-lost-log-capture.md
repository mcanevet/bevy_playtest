# Handle lost log capture

If a takeover run logs a warning like
`LogPlugin was added before PlaytestPlugin — its tracing subscriber
wins the global default and log capture will be OFF for this process`,
another tracing subscriber (almost always `DefaultPlugins`' `LogPlugin`)
claimed the process-global subscriber before the harness could. Log
capture is write-once: whoever sets it first wins, and the harness's
ERROR/WARN findings would be silently under-reported for the whole run —
which is why the harness refuses to be quiet about it.

To fix it:

1. **Prefer `PlaytestDefaultPlugins`.** Under a scenario run it drops
   `LogPlugin` so the harness's subscriber wins. The usual cause is
   building the app with plain `bevy::DefaultPlugins` (or adding
   `LogPlugin` manually) alongside the harness.
2. **Check ordering.** `PlaytestPlugin` must be added **after** any
   plugin group that sets a runner — the runner swap is last-set-wins,
   and a wrong order silently disables the takeover entirely.
3. **Deliberate loss is acknowledged, not silent.** If losing log
   capture is acceptable for this run, set
   `BEVY_PLAYTEST_ALLOW_MISSING_LOG_CAPTURE=1` for the game process.
   The report then records `log_capture_available: false` plus a
   non-gating `log_capture_lost` violation — visible truth instead of
   a silent hole.

The run is never failed on lost log capture alone; acknowledgment only
suppresses the warning.
