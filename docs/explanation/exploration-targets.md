# Exploration targets: how "explores better" is measured

The bot's exploration is judged against **committed, falsifiable
targets** on the fixture games, never against the bot's own claims. A
change to the bot only counts as an improvement if it moves these
numbers. (The input-surface discovery feature was removed partly
because it never had such a yardstick.)

## How a target is decided

- Every playtest report carries a **state trace**
  (`report.state_trace`, see `src/discovery/state_trace.rs`), recorded
  independently of the bot:
  - the **state schedules that ran** — every `OnEnter(..)`,
    `OnExit(..)` and `OnTransition { .. }` the game registered gets a
    recorder system, logged by its `Debug` label (e.g.
    `OnEnter(SettingsSound)`), plus the ones that **never ran**;
  - the **game resources that changed**, from change ticks — no
    `Reflect` needed, so it works for plain `#[derive(Resource)]`
    types. It says *when* a resource was written, never its value.
- Each fixture has a targets file, `eval/targets/<fixture>.json`. A
  target's `check` is one of: `entered` (all listed schedules ran),
  `entered_times`, `resource_changed_any`, `resource_changed_times`,
  or `unmeasurable` with the reason it cannot be decided generically
  today. Unmeasurable targets stay in the file so the gap stays
  visible.
- `bevy_playtest_eval` runs the fixture's battery once per seed with a
  fixed frame budget and reports, per target, how many seeds reached it
  and the median frame of first reach.

## Protocol

- **Evaluation:** 5,000 frames (~83 s of game time at 60 tps) × 5
  seeds per fixture, in the `Exploration evaluation` workflow (nightly
  and on demand; it measures, it never gates a PR).
- **Hit:** a target counts as reached when ≥ 4 of 5 seeds reach it.
- **Locally:** build the fixtures with `--features playtest`, then
  `bevy_playtest_eval --targets eval/targets/<fixture>.json
  --binary target/debug/fixture_<fixture> [--seeds N] [--frames N]`.

## Success criteria for the explorer (phase 1)

- G1–G4 (game_menu), M1, M3, M4 (mines), L1 (loading_screen) and T1
  (thetawave) reached on ≥ 4/5 seeds, and better than the baseline on
  every target the baseline misses; never worse anywhere.
- Honesty, on every fixture: nothing reported as reached that the
  state trace does not confirm; an "untested" section (states never
  entered, affordances never activated, input kinds never emitted,
  oracles switched off); a saturation verdict ("plateaued" vs "budget
  ran out while still discovering").
- Cost: the per-PR smoke adds ≤ 5 minutes to CI.

T2 (thetawave in-game) and K4 (chord/debounce plants) are stretch
goals.

## Caveats

- Names come from Bevy's `debug` feature, which is off by default;
  `bevy_playtest` enables it for test builds (`bevy_ecs/debug`), so
  release builds are unaffected.
- Only states with at least one enter/exit system have a schedule;
  other states are invisible to the trace (opt-in state tracking is a
  later step).
- A resource written every frame (timers) changes every frame; targets
  only use resources that are written on events.
