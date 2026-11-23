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
bevy_playtest = { version = "0.13", optional = true }

[dev-dependencies]
# Self-dependency: activates our own `playtest` feature only for
# test builds. Normal & release builds stay clean.
my_game = { path = ".", features = ["playtest"] }

[features]
playtest = ["dep:bevy_playtest", "bevy_playtest/takeover"]
```

**2. `src/main.rs`** — a cfg'd plugin-picker helper plus a cfg-guarded
call site around your existing plugin setup (use your own package
name in place of `my_game`):

```rust
#[cfg(feature = "playtest")]
fn default_plugins() -> bevy_playtest::PlaytestDefaultPlugins {
    bevy_playtest::PlaytestDefaultPlugins
}
#[cfg(not(feature = "playtest"))]
fn default_plugins() -> bevy::DefaultPlugins {
    bevy::DefaultPlugins
}
```

then use `.add_plugins(default_plugins().set(...))` where you had
`DefaultPlugins`, and — before `.run()` — a cfg-guarded block for
the takeover plugin:

```rust
    #[cfg(feature = "playtest")]
    app.add_plugins((bevy_playtest::PlaytestPlugin,));
```

Why not a symmetric `playtest_plugins()` helper with a
`-> ()` arm for normal builds? Because the unit-returning arm
trips `clippy::unused_unit` on its signature and `clippy::unit_arg`
at the call site — and a clean `cargo clippy` is part of the
zero-friction promise. (`()` is a legal zero-plugin tuple for
bevy's `Plugins` bound, so the old form compiles — the lints are
the entire problem.) The cfg-guarded call site is the idiomatic
escape: normal builds compile the guarded line away to nothing,
test builds get the plugin.

The one ordering rule: add `PlaytestPlugin` **after** any plugin
group that sets a runner, or the takeover is silently inert (the
runner swap is last-set-wins).

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

### Under Bazel instead of cargo

`env!("CARGO_BIN_EXE_…")` is a cargo-ism: it is set for cargo test
builds and does not exist at a `bazel test` runner's runtime. Three
plausible-looking wirings fail before one works — worth knowing so you
don't burn an afternoon on them:

- **`args = ["$(location //:my_game)"]`** — bazel forwards test args
  to the libtest runner, which treats the FIRST positional argument as
  a test-name filter. Every test silently filters out; you get a green
  `PASS` that ran nothing. This is the nastiest failure mode because
  it looks like success.
- **`env = {"MY_GAME": "$(location //:my_game)"}`** — `$(location)`
  expands execroot-relative; the sandboxed test process can't reach it.
  (`$(rootpath)`/`$(rlocationpath)` exist and expand runfiles-friendly,
  but they still land you writing the same runfiles lookup the recipe
  below does — go straight there.)
- **`CARGO_BIN_EXE_*`** seen in `bazel aquery` output — that's the
  compile-action environment only, absent at test runtime.

The reliable recipe: put the binary in the test's `data` and locate it
through the **runfiles** tree. In `BUILD.bazel`:

```python
rust_test(
    name = "playtest_battery",
    srcs = ["tests/playtest.rs"],
    crate_name = "playtest",
    deps = [
        ":my_game_playtest_lib",   # your library, built with the playtest cfg
        "@crates//:bevy_playtest",
    ],
    data = [
        ":my_game_bin",            # the game binary rides in runfiles
        "scenarios/battery.json",  # authored scenarios, if any
    ],
)
```

and in `tests/playtest.rs` replace `env!("CARGO_BIN_EXE_my_game")`
with a runfiles lookup:

```rust
fn binary_path() -> String {
    let name = "my_game_bin";
    let candidates = [
        std::env::var("TEST_SRCDIR").ok().map(|d| {
            std::path::Path::new(&d).join("_main").join(name)
        }),
        std::env::var("RUNFILES_DIR").ok().map(|d| {
            std::path::Path::new(&d).join("_main").join(name)
        }),
        Some(std::path::PathBuf::from(name)),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|p| p.exists())
        .map(|p| p.display().to_string())
        .expect("game binary not found in runfiles; is it in the test's data?")
}
```

`_main` is the workspace entry under bzlmod (`$(WORKSPACE_NAME)` under
legacy mode); the last candidate covers `bazel run` invocations where
runfiles sit next to the binary. Authored scenario files declared in
`data` resolve through the same helpers.

## What the battery does

The built-in battery scenario runs the chaos bot in **explore** mode —
keys (the game's own `KeyCode`s found by the source scan, else
WASD/arrows) and mouse clicks at 10 Hz — while the oracles watch:
non-finite transforms, runaway geometry, a frozen world, panics, ERROR
logs, and a vacuity guard that refuses to pass a run nothing was
observable in. Exploring means the bot remembers what it tried on each
screen and prefers the least-tried key or widget there, presses
Escape to back out of exhausted screens, tries modifier combos and
drags, and learns click targets the declared surface misses — all
drawn from the seeded stream, so runs replay exactly. The full
mechanics, opt-outs, and report fields are in
[the scenario reference](docs/reference/scenario-schema.md#explore-mode-explore-true-on-a-chaos-bot);
the full oracle list with each rule's evidence and honest boundary is
in [the reference](docs/reference/README.md#oracles).

Determinism: the seed controls the bot's choices — same seed → same
inputs → same bot behavior. Game-side randomness is not the seed's to
control: wall-clock reads and OS-seeded RNG can still change a verdict.
On Linux the battery preloads a shim that intercepts `getrandom()`
through libc (covers `rand::rng()` and ahash; not raw-syscall callers);
the full coverage and boundary list lives in
[deterministic boot](docs/reference/deterministic-boot.md). To
prove determinism itself, enable
[replay verification](docs/how-to/how-to-prove-a-scenario-is-deterministic.md),
which runs the scenario twice and compares per-frame fingerprints.

Declaring nothing is the point — with one honest boundary. The
zero-config battery's progress oracle (auto mode) watches *structural*
state: entity, archetype, and registered component-type counts, plus
any `include_paths`/`markers` you declared. It cannot see values
mutating in place: an economy whose balance grows on a fixed
population, an RTS whose units reposition without spawning or dying,
an XP bar filling — all look identical to the counts-only surface.
If your game hits `stalled_progress` while visibly alive, the sim is
almost certainly fine; the surface is counts-only by design. (A game
with NO reflected game types never gets that far — the battery's
`require_surface` fails the run outright with `progress_surface_empty`;
registering state types first, then naming the live resources, is the
path.) The fix
is one authored scenario adding value channels for the live state —
`liveness.progress.include_paths` or `markers` naming the reflected
resources whose values evolve (`my_game::Economy` and friends) —
not debugging your simulation. See
[the progress heartbeat
reference](docs/reference/README.md#oracles) for the full channel
semantics.

To target a specific situation, authored scenarios are data files
referenced from the same helper —
`battery::run_and_expect_clean(binary, "playtests/menu.json")`
(see the [scenario reference](docs/reference/scenario-schema.md) for the
full JSON schema, and [script keys instead of
chaos](docs/how-to/how-to-script-keys-instead-of-chaos.md) for scripted
input).

## Aiming the bot at the game's real input surface

With no flags at all, the battery parent runs a milliseconds-cost
source scan: it collects every `KeyCode::` variant the game's own
sources mention and — when it finds any — replaces the default
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

## The hand-maintained fixtures

The repository carries three hand-maintained fixtures beyond the
fetched showcase games. The first is hand-authored:
`fixtures/known_answer/` — a deliberately constructed input surface
(numbered "plants": chords, debounces, shape-only KeyCode traps)
with committed ground truth (`ground_truth.json`) that measurement
lenses are graded against, including asserted expected misses and
runtime code mutations for drift testing. See the
[known-answer fixture reference](docs/reference/known-answer-fixture.md).

The second is a spatial litmus: `fixtures/demon_stack/` — a
hand-authored game whose only planted defect is a six-demon
contact pile (proportional homing, no separation term) that the
`pairwise_separation` rule must catch as exactly one folded episode
with worst-pair and roster evidence, alongside a fly-by control
proving the same geometry under a boundary soak (window just above
the episode's span) stays silent.
See the
[judgement-litmus fixture reference](docs/reference/judgement-litmus-fixture.md).

The third is a real-world game: `fixtures/thetawave/` — a port of
the MIT-licensed thetawave (Bevy 0.14 → 0.20) using real Leafwing
Input Manager maps loaded from RON. It has no ground truth by
design: its committed tests make only per-run-derivable, honest
claims — headless boot, a clean battery, reader enumeration of the
game's actual `MenuAction` bindings, and driven actions grading
`manager_pipeline_ok` — the reference point for what the declared
tier observes on a production-shaped game.

## How in-process takeover works

When `BEVY_PLAYTEST_SCENARIO` is set, your binary becomes its own
headless playtest runner (exit codes: 0 Pass, 1 Fail, 2 Crash, 3
config error). Without the env var the plugin is inert. The runner
swap replaces winit's event loop, so no window appears. In the
recommended integration this mode only exists in a playtest-enabled
build (`cargo test`, or `cargo run --features playtest`); the default
and release builds contain zero bevy_playtest code.

`PlaytestDefaultPlugins` drops `WinitPlugin` under the takeover env vars
(`BEVY_PLAYTEST_*` — bevy_winit 0.20 builds its event loop eagerly, so
headless CI hosts need it gone before the runner swap).

Known limitations: rendering is unavailable (software-only) so visual
assertions do not apply.

## Log capture

Errors logged via `tracing` from any thread — including IoTaskPool
workers (where bevy_asset reports load failures) and multi-threaded
schedule executors — are captured into the run's report. Capturing is
process-global and exclusive: exactly one `run_scenario` may be active
per process; concurrent or nested calls are rejected immediately
(`ScenarioError::Rejected`), never blocked. Parallel callers (e.g. a
test battery) must serialize runs themselves.

Capture depends on the harness's tracing subscriber winning the
write-once global default. Two consequences:

- Use `PlaytestDefaultPlugins` (or any setup where `PlaytestPlugin` is
  added before the game's `LogPlugin`): under `BEVY_PLAYTEST_SCENARIO`
  it drops `LogPlugin` and installs its own subscriber with identical
  console behavior (honors `RUST_LOG`).
- If the game's `LogPlugin` is added first (plain `DefaultPlugins` plus
  `PlaytestPlugin` after it), its subscriber wins the race and log
  capture is off. The plugin warns loudly at build time, the report
  says so (`log_capture_available: false` plus an informational,
  non-gating `log_capture_lost` entry), and setting
  `BEVY_PLAYTEST_ALLOW_MISSING_LOG_CAPTURE=1` acknowledges the loss
  deliberately. A green report under a lost race never implies "no
  errors were logged".

## Panic capture

Panics are captured independently of tracing: a panic during the run's
loop lands in the run's report; a panic in the setup window (before the
run starts) propagates out of `run_scenario` to the caller — the
harness does not swallow it, but produces no report for it.

## Integration caveats (read when something surprises you)

Release purity caveat: the feature activates for any test/example/bench
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

Curious why the Cargo.toml dance works and what it guarantees?
[Why the self-dev-dependency pattern](docs/explanation/why-the-self-dev-dependency-pattern.md).

See the [docs](docs/README.md) for the full guides.
