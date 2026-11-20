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
  seeds per fixture, run on demand with the `Exploration evaluation`
  workflow (Actions → Run workflow) before claiming an improvement.
  PRs that touch the harness, targets or dependencies get a quick run
  automatically (2 seeds × 5,000 frames): its median frames compare with
  the baseline, its hit counts out of 2 do not (use the full run for the
  ≥ 4/5 rule). It never gates a PR and has no schedule; because results
  can also drift with the environment (see "Determinism"), compare
  against a freshly measured baseline when it matters.
- **Hit:** a target counts as reached when ≥ 4 of 5 seeds reach it.
- **Locally:** build the fixtures with `--features playtest`, then
  `bevy_playtest_eval --targets eval/targets/<fixture>.json
  --binary target/debug/fixture_<fixture> [--seeds N] [--frames N]`.

## Baseline (today's chaos bot)

Measured by the `Exploration evaluation` workflow on GitHub runners
(run 44752095130, commit `0722110`; 5,000 frames × 5 seeds, all 45 runs
completed); results in
`eval/baseline/*.json` (raw traces omitted). Compare explorer runs
against it on the same kind of machine: see "Determinism" below.

| Fixture | Target | Hits | Median frame |
|---|---|---|---|
| game_menu | G1 both settings sub-menus | 3/5 | 3,115 |
| | G2 change a setting | 4/5 | 1,371 |
| | G3 start the game | 5/5 | 259 |
| | G4 every screen in one run | 3/5 | 3,115 |
| mines | M1 start a game | 5/5 | 489 |
| | M2 reveal ~10 tiles | 3/5 | 1,581 |
| | M3 reveal a tile | 5/5 | 495 |
| | M4 back to the menu after playing | 2/5 | 4,521 |
| | M5 start three games | 0/5 | — |
| thetawave | T0 main menu (passive) | 5/5 | 7 |
| | T1 character selection | 0/5 | — |
| | T2 in game (stretch) | 0/5 | — |
| | T3 Victory (expected never) | 0/5 | — |
| loading_screen | L1 load levels (passive) | 5/5 | 144 |
| alien_cake_addict | A2 game over (passive) | 5/5 | 901 |
| | A3 restart with Space | 5/5 | 920 |
| breakout | B1 score (passive) | 5/5 | 112 |

Passive targets are reached without meaningful input (a timer, the
ball, or the source scan already aiming the bot); they are regression
checks, not exploration goals. On every run thetawave's trace shows the
bot never leaving the main menu: `CharacterSelection`, `InitializeRun`,
`Game`, `GameOver`, `Paused` and `Victory` are all never entered.

## Success criteria for the explorer (phase 1)

Two measures per target, both compared against the committed baseline
(same seeds and budget):

- **Reach:** G1–G4 (game_menu), M1–M4 (mines) and T1 (thetawave)
  reached on ≥ 4/5 seeds. Every target the baseline misses must be
  reached more often than in the baseline (M5 and T2 are tracked but
  not required); no target may be reached less often, passive ones
  included.
- **Speed:** on every target the baseline already reaches on ≥ 4/5
  seeds (G2, G3, M1, M3), the median frame of first reach must be at
  most **half** the baseline's. Random input already clears much of a
  simple menu given enough frames, so reach alone is too low a bar;
  frames not spent wandering menus are frames spent deeper in the game.
- **Honesty**, on every fixture: nothing reported as reached that the
  state trace does not confirm; an "untested" section (states never
  entered, affordances never activated, input kinds never emitted,
  oracles switched off); a saturation verdict ("plateaued" vs "budget
  ran out while still discovering").
- **Cost:** the per-PR smoke adds ≤ 5 minutes to CI.

T2 (thetawave in-game) and K4 (chord/debounce plants) are stretch
goals.

## Determinism

The same evaluation run locally and on GitHub runners matched exactly on
7 of 9 fixtures. The two exceptions show the two ways a run can depend
on more than its seed:

- **OS-seeded randomness** (mines): the minefield is shuffled with
  `rand::rng()`, seeded from the operating system, so every process
  gets a different board. mines targets M2 and M4 therefore vary from
  run to run; judge them with a margin or more seeds.
  (alien_cake_addict only seeds its RNG when `GITHUB_ACTIONS=true`;
  its targets are passive, so it matched anyway.)
- **Background loading** (loading_screen): a level is ready once its
  assets and GPU pipelines finish loading, which takes wall-clock time.
  On CI's software renderer that spans more frames (median 144 vs 91
  locally), so frame-based timings shift with machine speed.

Both must be addressed before replay-based exploration can work:
deterministic randomness in test builds (e.g. a custom `getrandom`
backend fed from the scenario seed) and a settle step that does not
advance frames while assets or pipelines are still loading.

## Caveats

- Names come from Bevy's `debug` feature, which is off by default;
  `bevy_playtest` enables it for test builds (`bevy_ecs/debug`), so
  release builds are unaffected.
- Only states with at least one enter/exit system have a schedule;
  other states are invisible to the trace (opt-in state tracking is a
  later step).
- A resource written every frame (timers) changes every frame; targets
  only use resources that are written on events.
