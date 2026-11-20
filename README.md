# bevy_playtest

A playtesting harness for Bevy games. A deterministic bot plays your
game while oracles watch for NaN propagation, runaway transforms,
soft-locks, panics and error spam — and fail a plain `cargo test` the
moment any of them fires.

**Doctrine: zero friction.** The recommended integration touches three
files — `Cargo.toml`, `src/main.rs`, `tests/playtest.rs` — needs no
scenario to start, no contract in game code, and compiles to
zero bevy_playtest code in normal and release builds.

**Documentation:** [tutorial](docs/tutorial/) · [how-to guides](docs/how-to/) ·
[reference](docs/reference/) · [explanation](docs/explanation/)

## Quick start: a playtest in three files

**1. `Cargo.toml`** — optional dependency plus the self-dev-dependency
that activates it only for test builds:

```toml
[dependencies]
bevy_playtest = { version = "0.8", optional = true }

[dev-dependencies]
# Self-dependency: activates OUR OWN `playtest` feature only for
# test builds. Normal & release builds stay clean.
my_game = { path = ".", features = ["playtest"] }

[features]
playtest = ["dep:bevy_playtest", "bevy_playtest/takeover"]
```

**2. `src/main.rs`** — two cfg'd helper pairs around your existing
plugin setup (use your own package name in place of `my_game`):

```rust
#[cfg(feature = "playtest")]
fn default_plugins() -> bevy_playtest::PlaytestDefaultPlugins {
    bevy_playtest::PlaytestDefaultPlugins
}
#[cfg(not(feature = "playtest"))]
fn default_plugins() -> bevy::DefaultPlugins {
    bevy::DefaultPlugins
}

#[cfg(feature = "playtest")]
fn playtest_plugins() -> (bevy_playtest::PlaytestPlugin,) {
    (bevy_playtest::PlaytestPlugin,)
}
#[cfg(not(feature = "playtest"))]
fn playtest_plugins() -> () {}
```

then use `.add_plugins(default_plugins().set(...))` where you had
`DefaultPlugins`, and `.add_plugins(playtest_plugins())` before `.run()`.
The one ordering rule: add `PlaytestPlugin` **after** any plugin group
that sets a runner, or the takeover is silently inert (the runner swap
is last-set-wins).

**3. `tests/playtest.rs`** — the whole file, no scenario needed:

```rust
#[test]
fn playtest_battery() {
    bevy_playtest::battery::run_and_expect_clean(
        env!("CARGO_BIN_EXE_my_game"),
        bevy_playtest::battery::BUILTIN_BATTERY,
    );
}
```

Run it:

```
$ cargo test
running 1 test
test playtest_battery ... ok

test result: ok. 1 passed; 0 failed
```

That `ok` is a verdict, not a formality: for the battery's duration a
seeded bot typed and clicked through your game while every oracle
watched — and nothing fired. When something does, the test prints the
findings (violation, tick, entities involved) instead.

## What the battery catches out of the box

The built-in battery scenario runs the chaos bot in **explore** mode —
keys (the game's own `KeyCode`s found by the source scan, else
WASD/arrows) and mouse clicks at 10 Hz — while these oracles watch.
Exploring means the bot remembers what it tried on each screen (the
set of enabled widgets it can see) and prefers the least-tried key or
widget there, clicks widgets three times out of four (else a random
spot), and presses Escape to back out after 3 s without anything new
(a new state or a new screen): never on the first screen seen, and
waiting twice as long after each back-out on the same screen. Escape
needs a key surface, so a scenario declaring no keys never backs out. On
every new screen it also tries a built-in prior of 27 common keys
(movement, digits, Space/Enter, modifiers, E/F/R) once each, so keys the
source scan cannot see (bound in config files, plugin crates, macros)
are still found; a key followed by a new state or screen within one
input tick, with no click in between, stays in rotation and is listed
under `keys_found` ("followed by", not proven to cause it). To keep
that attribution clean, explore mode alternates input ticks between
the keyboard and the mouse; a device with nothing to do yields its
tick. Every fourth key press is a combo: Ctrl, Shift
or Alt held under a declared or productive key (Ctrl+S, Shift+L), the
least-tried combo on the screen first; combos followed by something new
are listed under `combos_found`. Chords of two ordinary keys (A+D) are
only tried by chance. Every fourth mouse action is a drag when a
node on screen can be dragged (an entity observer of `DragStart`,
`Drag` or `DragEnd`: kind `draggable`) or accepts drops (an observer of
`DragDrop`: kind `drop_target`), every sixteenth otherwise: press on a
draggable node (else the least-tried widget or a random spot), move
while held for 11 frames, and release three times in four on a drop
target (else another widget), otherwise on a random spot; `drags`
counts them. Clickable nodes that declare nothing (a grid of
cells handled by one global click observer) are learned: when a click
at a random spot lands on a node carrying one of the game's own
components (or under one, like a cell's label) and the game reacts
within one input turn with no key pressed or released around it (a state change,
or a game resource changing that had been quiet before the click),
that node's archetype earns a credit; after two, every
node of that archetype is a click target (`learned_targets` counts the
archetypes). A click followed by a state change learns at once, unless
a key was pressed around it. Learned targets are named with a `~`
prefix and do not change the screen identity. Every choice is still drawn from the
seeded stream, so runs replay exactly. The report then carries an
`exploration` section: screens seen, states entered, widgets never
clicked, declared keys never pressed, prior keys and combos found, back-outs,
and a verdict — "plateaued"
(nothing new in the last quarter of the run) or "still discovering
when the run ended", in which case "no findings" only covers what was
reached. Authored scenarios opt in with `"explore": true` on a chaos
bot. The oracles:

- **Non-finite translations** — any non-finite `Transform.translation` → Fail (rotations/scales are not checked)
- **Runaway geometry** — with nothing declared, an entity outside a
  global ±1000 box on x/y/z that keeps moving away for 0.5 s (e.g. a
  projectile that is never despawned); far-but-static level geometry and
  objects coming in from afar are not flagged. Bounds declared in an
  authored scenario are checked strictly, per axis
- **Frozen world** — no entity movement (or UI change, under
  `ui-liveness`) across the liveness grace window → Fail (catches
  soft-locks; with `record_digest: true` a silent-but-stepping world
  is named "visually silent" instead — see
  [Why liveness is time-boxed](docs/explanation/why-liveness-is-time-boxed.md))
- **Panic capture** — crash → Crash
- **Log capture** — ERROR → Fail, WARN → informational
- **Render liveness** — with the `render-liveness` feature, a
  sim-active run on a game that HAS a render pipeline but never had a
  camera, kept every sampled camera disabled (`is_active == false`),
  or never had an active camera writing output (every sampled
  active camera sat on `CameraOutputMode::Skip`; a disabled
  camera's mode is irrelevant — it renders nothing), gains an
  informational `render_liveness` entry (structural wiring only: it
  reads main-world `Camera` components, never the rendered pixels —
  a camera that renders black still passes; see
  [headless CI without a GPU](docs/how-to/headless-ci-gpu.md))
- **Screen liveness** — with the `screen-liveness` feature, the
  harness captures what the renderer actually produced (a
  harness-owned `Camera2d` taking periodic screenshots) and hashes
  the pixels. A sim-active 2D run whose captured screen never
  changed across every capture pair gains an informational
  `screen_liveness` entry; purely-3D scenes (no 2D camera) are
  honestly labeled `not_evaluated_3d` instead of guessed, and fewer
  than two captures refuse the question (`insufficient_captures`).
  Informational, never gating: a frozen screen may be intentional
  (menus, dialogue, level-load art). Evidence (tier, request/capture
  counts, max Hamming drift, notes) rides the report's
  `screen_liveness` field, which stays absent when the feature is
  off or the game has no render pipeline — absence means "not
  evaluated", never "healthy"
- **Vacuity guard** — if no entity with a `Transform` (or, under
  `ui-liveness`, a `UiTransform`) existed at any point during the run, a
  Pass proves nothing; it fails loudly instead (the criterion is
  any-frame, so a game whose entities despawn before the end is not
  flagged)

Determinism: the seed controls the bot's choices — same seed → same
inputs → same bot behavior. On Linux the battery also makes the game's
OS-seeded randomness repeatable: it preloads a tiny shim (`LD_PRELOAD`)
whose `getrandom()` is seeded from the scenario seed. That covers
callers going through libc's `getrandom`: the `getrandom` crate 0.3+,
hence `rand::rng()` (rand 0.9+) and ahash. Not covered: `getrandom`
0.2 (raw syscall), `/dev/urandom` readers, glibc `arc4random`, and
clock-based seeds. Only the main thread's stream is guaranteed to
repeat (game systems run there under the takeover's single-threaded
executor); other threads share a seeded stream whose order depends on
thread timing. The shim is built with a plain `rustc` call, so cross
builds with a custom target linker may fail to build it; the run then
reports randomness as not intercepted. The report's
`randomness` field says whether that happened, or why not (not Linux,
in-process run, or `BEVY_PLAYTEST_DETRNG_DISABLE=1` set in the test's
environment). The shim also affects anything else in the game process
that asks the OS for randomness, networking crypto included, so it is
test-build-only by construction. Seeds taken from the clock and other
wall-clock reads are not controlled, so an unchanged game can still
produce a different verdict; the battery leaves replay verification
off. To
prove determinism, enable
[replay verification](#verification-depth-what-is-proven-vs-what-is-asserted),
which runs the scenario twice and compares per-frame fingerprints.

Declaring nothing is the point. When you need to target a specific
situation, authored scenarios are data files referenced from the same
helper — `battery::run_and_expect_clean(binary, "playtests/menu.json")`
(see the [scenario reference](docs/reference/README.md) for the full
JSON schema, and [script keys instead of
chaos](docs/how-to/how-to-script-keys-instead-of-chaos.md) for scripted
input).

Opt-outs: `mouse_buttons: []` under a scenario's `bot.raw_surface`
drops clicks; `"keys": []` there drops chaos keys (mouse-only chaos).

## The hand-maintained fixtures

The repository carries two hand-maintained fixtures beyond the
fetched showcase games. The first is hand-authored:
`fixtures/known_answer/` — a deliberately constructed input surface
(numbered "plants": chords, debounces, shape-only KeyCode traps)
with committed ground truth (`ground_truth.json`) that discovery
measurements are graded against, including asserted expected
misses and two runtime code mutations for drift testing. See the
[known-answer fixture reference](docs/reference/known-answer-fixture.md).

The second is a real-world game: `fixtures/thetawave/` — a port of
the MIT-licensed thetawave (Bevy 0.14 → 0.20) using real Leafwing
Input Manager maps loaded from RON. It has no ground truth by
design: its committed tests make only per-run-derivable, honest
claims — headless boot, a clean battery, reader enumeration of the
game's actual `MenuAction` bindings, and driven actions grading
`manager_pipeline_ok` — the reference point for what the declared
tier observes on a production-shaped game.

## Aiming the bot at the game's real input surface

With no flags at all, the battery parent runs a milliseconds-cost
source scan: it collects every `KeyCode::` variant the game's own
sources mention and — when it finds any — REPLACES the default
WASD+arrows chaos surface with that scan-derived key set (poison
keys like `Escape`/`Tab` filtered, capped, all decisions
reported), concentrating the chaos presses on keys the game
actually reads. A game whose bindings sit outside WASD+arrows
therefore plays correctly out of the box; the scan-aimed run
records `source_derived_keys` and `source_derived_notes` on its
report. An empty or failed scan keeps the WASD+arrows fallback.
`BEVY_PLAYTEST_SOURCE_SCAN_DISABLE=1` restores the blind default,
and `BEVY_PLAYTEST_SOURCE_ROOT` points the scan at a non-standard
source root. An authored scenario file always wins (declaration
beats inference).

Games with menus get a UI tier too: with the `ui-surface` feature (on
by default under the takeover) the harness enumerates the declared
UI/interactable surface (`Pickable` markers, `bevy_ui_widgets`
components, bevy_ui `Interaction` buttons, `TabIndex`, accessibility
nodes, pointer-event observers) after the first frame
and re-captures it periodically (~twice a second), so menus and
dialogs opened mid-run join the surface, with geometry matching
`bevy_ui`'s own picking backend. The latest snapshot rides the
playtest report as `ui_surface` (entity-set deltas along the way are
reported as notes), and it is what the explorer (above) navigates by.
Without explore mode, the chaos mouse bot interleaves: half its
clicks target the widget centers from the latest recapture
(pointer-driven games get exercised; centers lag the world by at
most one recapture interval, ~0.5s), half keep sweeping the whole
window (undeclared regions stay probed).

Games using Leafwing Input Manager get a declared tier: with the
`lw-reader` feature the harness reads `InputMap` bindings by pure
reflection (RON-loaded maps included), taps wiring liveness during
the run, and reconciles the two on every run — the report carries
`lw_declared`, `lw_tap_stats` and `lw_reconciled`, where
declared-but-dead wiring raises a `declared_but_dead` finding and
everything unproven stays a note. See [declare your actions with
Leafwing Input Manager](docs/how-to/declare-actions-leafwing.md).

## How in-process takeover works

When `BEVY_PLAYTEST_SCENARIO` is set, your binary becomes its own
headless playtest runner (exit codes: 0 Pass, 1 Fail, 2 Crash, 3 config
error). Without the env var the plugin is inert. The runner swap
replaces winit's event loop, so no window appears. In the
recommended integration this mode only exists in a playtest-enabled
build (`cargo test`, or `cargo run --features playtest`); the default
and release builds contain zero bevy_playtest code.

`PlaytestDefaultPlugins` drops `WinitPlugin` under the takeover env vars
(`BEVY_PLAYTEST_*` — bevy_winit 0.20 builds its event loop eagerly, so
headless CI hosts need it gone before the runner swap).

Known limitations: rendering is unavailable (software-only) so visual
assertions do not apply.

## Oracles


Errors logged via `tracing` from ANY thread — including IoTaskPool
workers (where bevy_asset reports load failures) and multi-threaded
schedule executors — are captured into the run's report. Capturing is
process-global and exclusive: exactly one `run_scenario` may be active
per process; concurrent or nested calls are REJECTED immediately
(`ScenarioError::Rejected`) — never blocked. Parallel callers (e.g. a
test battery) must serialize runs themselves.

Capture depends on the harness's tracing subscriber winning the
write-once global default. Two consequences:

- Use `PlaytestDefaultPlugins` (or any setup where `PlaytestPlugin` is
  added before the game's `LogPlugin`): under `BEVY_PLAYTEST_SCENARIO`
  it drops `LogPlugin` and installs its own subscriber with identical
  console behavior (honors `RUST_LOG`).
- If the game's `LogPlugin` is added first (plain `DefaultPlugins` plus
  `PlaytestPlugin` after it), its subscriber wins the race and log
  capture is OFF. The plugin warns loudly at build time, the report
  says so (`log_capture_available: false` plus an informational,
  non-gating `log_capture_lost` entry), and setting
  `BEVY_PLAYTEST_ALLOW_MISSING_LOG_CAPTURE=1` acknowledges the loss
  deliberately. A green report under a lost race never implies "no
  errors were logged".

Panics are captured independently of tracing: a panic during the run's
loop lands in the run's report; a panic in the setup window (before the
run starts) propagates out of `run_scenario` to the caller — the
harness does not swallow it, but produces no report for it.

## Verification depth (what is proven vs. what is asserted)

One mechanism takes a run from "asserted" to "proven":

1. **Replay divergence** (`"verify_replay": true`) runs the scenario
   twice against fresh Apps and compares per-frame fingerprints
   (choice draws + sorted Transform translations). Divergence is a
   `replay_divergence` Fail identifying the first divergent frame.
   This PROVES determinism for the observable surface — it does not
   prove absence of nondeterminism in unobserved state (components
   without Transform).
   On the takeover/plugin path, pass B re-runs your game binary as a
   subprocess — whole-process determinism, the strongest form. NOTE:
   `verify_replay` requires `single_threaded: true` (default).

## Integration caveats (read when something surprises you)

Release purity caveat: the feature activates for ANY test/example/bench
build — including `cargo test --release` / `cargo build --all-targets`.
Only plain `cargo build` / `cargo build --release` (bins) are
guaranteed playtest-free. To enforce that contract in CI (recommended —
the failure mode is otherwise silent):

```console
$ cargo check
$ cargo tree -e normal | grep -q "bevy_playtest v" && echo "LEAK" && exit 1
```

Bin-only games (no `src/lib.rs` — the common shape for Bevy games):
cargo may print
`ignoring invalid dependency ... which is missing a lib target` when
resolving the self-dev-dependency (notably for workspace members). The
warning is benign for this pattern: the dependency is only used for
feature unification, and the `playtest` feature still activates on
plain `cargo test` — verified in isolation with a bin-only crate, and
pinned by CI (the build job's flagless fixture test).

Packaging note: `cargo package` strips `[dev-dependencies]`, so the
self-dev-dependency no longer auto-activates `playtest` in an unpacked
archive. Handle this the way the tutorial's battery does: detect the
inactive feature in `tests/playtest.rs` and report an ignored test
with the reason and the fix
(`cargo test --features playtest` restores the full battery — the
feature and optional dependency survive packaging; only the
auto-activation is lost).

Games with known findings can pin them instead of failing — see
[pinning in the reproduce-a-failure guide](docs/how-to/how-to-reproduce-a-failure.md).

Headless CI caveat: takeover drops `WinitPlugin`, not `RenderPlugin` —
camera-bearing games still need a GPU adapter, so stock GPU-less
runners panic with `Unable to find a GPU!`. See
[run playtests on a GPU-less CI host](docs/how-to/headless-ci-gpu.md).

See the [docs](docs/README.md) for the full guides.
