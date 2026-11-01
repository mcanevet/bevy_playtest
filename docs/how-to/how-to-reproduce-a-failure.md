# Reproduce a failure

A failing playtest is a reproducer by construction — the seed makes chaos
deterministic.

1. **Re-run the same scenario verbatim.** The bot seed lives in the scenario
   file; the same seed replays the identical key sequence, so the failure
   recurs unless it depends on unseeded state (wall clock, unseeded game RNG,
   unordered iteration — which the replay-divergence check exists to expose).
2. **Run locally** rather than in CI to iterate quickly:
   `BEVY_PLAYTEST_SCENARIO=<scenario.json> cargo run --features playtest`
   inside the game crate, or run the failing test directly. The
   report lands next to the scenario at `<scenario>.report.json`.
3. **Read the report before the code.** Each violation names its rule
   (`finite_transforms`, `nodes_in_bounds`, `frozen_world`…), a target
   (whose meaning depends on the rule — the invariant `name`, the entity
   id, or `"world"` for frozen-world; see the
   [reference](../reference/README.md#report)), and the first/last frame it
   was observed — start the debugger at `first_frame`, not at the end.
4. **If the failure does NOT reproduce with the same seed** — suspect hidden
   nondeterminism in the game, not harness flakiness. Turn on
   `"verify_replay": true` to get the first divergent frame.
5. **Pin it once understood.** If the finding is a known issue you're
   tracking (rather than fixing right away), pin it with plain assertions
   instead of letting the test fail forever. Get the report with
   `battery::run(...)`, then:
   `assert_eq!(report.violation_rules(), ["frozen_world"])` for rule-only,
   or `assert_eq!(report.violation_pairs(), [("frozen_world", "world")])`
   to pin both rule and target. The test passes while the known finding is
   present and fails when it changes (fixed → empty set, regression → a new
   rule/target appears).

See [handle a rejected scenario](how-to-handle-a-rejected-scenario.md) if the failure
is at scenario load instead.
