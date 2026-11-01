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
bevy_playtest = { version = "0.4", optional = true }

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

The built-in battery scenario runs the chaos bot — keys (WASD/arrows
by default) and mouse clicks (all primary buttons) at 10 Hz, holding
each input 2 frames so `ButtonInput`-based games observe it — while
these oracles watch:

- **Non-finite translations** — any non-finite `Transform.translation` → Fail (rotations/scales are not checked)
- **Runaway geometry** — entities leaving bounds (a hard-coded global
  ±1000 on x/y/z runs even with nothing declared; authored scenarios
  tighten per-axis)
- **Frozen world** — no entity movement (or UI change, under
  `ui-liveness`) across the liveness grace window → Fail (catches
  soft-locks)
- **Panic capture** — crash → Crash
- **Log capture** — ERROR → Fail, WARN → informational
- **Vacuity guard** — if no entity with a `Transform` (or, under
  `ui-liveness`, a `UiTransform`) existed at any point during the run, a
  Pass proves nothing; it fails loudly instead (the criterion is
  any-frame, so a game whose entities despawn before the end is not
  flagged)

Determinism: the seed controls the bot's choices — same seed → same
inputs → same bot behavior. It does not control game-side randomness
or wall-clock reads, so an unchanged game can still produce a
different verdict; the battery leaves replay verification off. To
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

See the [docs](docs/README.md) for the full guides.
