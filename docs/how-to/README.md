# How-to guides

Task-focused recipes, one per page. Each assumes the
[tutorial](../tutorial/) basics.

- [Script keys instead of chaos](how-to-script-keys-instead-of-chaos.md) — Deterministic key schedules instead of the chaos bot
- [Get JUnit XML for CI](how-to-get-junit-xml-for-ci.md) — Emit JUnit XML that CI dashboards ingest
- [Call `run_scenario` directly (in-process, no child)](how-to-run-a-scenario-in-process.md) — Use the PlaytestReport object in your own tests
- [Reproduce a failure](how-to-reproduce-a-failure.md) — Turn a failing run into a local reproducer
- [Handle a rejected scenario](how-to-handle-a-rejected-scenario.md) — Decode load-time rejections and fix them
- [Handle lost log capture](how-to-handle-lost-log-capture.md) — Recover when another tracing subscriber wins the process global
- [Run playtests on a GPU-less CI host](headless-ci-gpu.md) — Fix the `Unable to find a GPU!` panic on headless CI runners
- [Prove a scenario is deterministic](how-to-prove-a-scenario-is-deterministic.md) — Verify replay divergence
- [Discover your input surface](how-to-discover-your-input-surface.md) — Learn which keys your game actually consumes, with cached probe results
- [Draft your input surface with one command](discover-input-surface.md) — Authoring-time `surface.draft.json` artifact with live entries, pinned inerts, and honest limitations
- [Declare your actions with Leafwing Input Manager](declare-actions-leafwing.md) — Have discovery read `InputMap` declarations (RON maps included), tap wiring liveness, and alarm on declared-but-dead bindings
