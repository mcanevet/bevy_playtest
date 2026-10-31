# bevy_playtest

A playtesting harness for Bevy games. Scenarios drive a deterministic bot
through your game while oracles check invariants and produce a machine-readable
report (JSON + JUnit XML).

**Documentation:** [tutorial](docs/tutorial/) · [how-to guides](docs/how-to/) ·
[reference](docs/reference/) · [explanation](docs/explanation/)

## The two modes

bevy_playtest drives your game in one of two ways. Both run as plain
`cargo test` tests, produce the same report formats, and gate the same
way — they differ in **how much they can see**:

|                          | **Mode 1: In-process (takeover)**              | **Mode 2: External BRP**                       |
|--------------------------|-------------------------------------------------|-----------------------------------------------|
| Game source changes      | ~15 lines (feature + cfg'd helpers)             | 3 touch-points (bevy_remote, register_type, port) |
| Harness code in game     | Yes, test builds only                           | None                                          |
| Oracles                  | All (panic, log, liveness, NaN, bounds, replay) | Reduced (NaN, bounds, liveness)  |
| Determinism proof (`verify_replay`) | ✓                                    | ✗ (rejected explicitly)                      |
| Release builds           | Zero playtest code                              | Remote plugin stays in the binary            |

### BRP parity matrix

| Capability               | Mode 1 (in-process) | Mode 2 (BRP) | Mode 2 + PlaytestBrpPlugin     |
|--------------------------|---------------------|--------------|--------------------------------|
| Panic capture            | ✓                   | ✗            | ✓ (`bevy_playtest/health`)     |
| Log capture              | ✓                   | ✗            | ✓                              |
| Time scale control       | N/A                 | ✗            | ✓ (`bevy_playtest/time`)       |

The report's `brp_parity` field distinguishes "full" from "degraded"
modes; never a silent downgrade. (`PlaytestBrpPlugin` restores some
mode-1 capabilities to mode 2 via a small BRP method surface — see the
[docs](docs/how-to/how-to-drive-a-game-you-cannot-modify.md).)

Pick mode 1 unless you can't touch the game code at all or are testing
a third-party binary.

## Mode 1: In-process (takeover)

Integration is ~15 lines across three files (Cargo.toml feature +
self-dev-dependency, two cfg'd helper pairs in main.rs, one small
test) — then plain `cargo test` runs your
playtests, and `cargo build --release` contains zero bevy_playtest code.
The [tutorial](docs/tutorial/) walks through every line; this section is
the same integration as a quick reference.

The built-in battery scenario (`BEVY_PLAYTEST_SCENARIO=battery`) is
zero-config: the chaos bot both types (WASD/arrows or your declared
`raw_surface.keys`) and clicks (all mouse buttons by default), at 10 Hz,
holding each input 2 frames so `ButtonInput`-based games observe it.
No scenario, no contract — declare `mouse_buttons: []` in
`raw_surface` to opt out of clicks.

**Cargo.toml:**
```toml
[dependencies]
bevy_playtest = { version = "0.3", optional = true }

[dev-dependencies]
# Self-dependency: activates OUR OWN `playtest` feature only for
# test builds. Normal & release builds stay clean.
my_game = { path = ".", features = ["playtest"] }

[features]
playtest = ["dep:bevy_playtest", "bevy_playtest/takeover"]
```

**src/main.rs:**
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
is last-set-wins). `PlaytestDefaultPlugins` drops `WinitPlugin` under the takeover env vars
(`BEVY_PLAYTEST_*`, plus `BRP_PORT` for external mode — bevy_winit 0.20 builds its event loop
eagerly, so headless CI hosts need it gone before the runner swap).

**tests/playtest.rs** (commit it — it's yours, per-game assertions live here):
```rust
#[test]
fn playtest_battery() {
    bevy_playtest::battery::run_and_expect_clean(
        env!("CARGO_BIN_EXE_my_game"),
        bevy_playtest::battery::BUILTIN_BATTERY,
    );
}
```

Authored scenarios are data files referenced from the same helper:
`battery::run_and_expect_clean(binary, "playtests/menu.json")`.

Games with known findings can pin them instead of failing — see
[pinning in the reproduce-a-failure guide](docs/how-to/how-to-reproduce-a-failure.md).

`cargo test` compiles the game WITH the feature (the self-dev-dep is
what activates it) and Cargo hands the test the binary path. The
helper spawns it with the scenario, reads the report, and fails the
test with a per-violation findings summary if anything gates.

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

The battery is a smoke/stress verdict — NaN propagation, runaway
transforms, soft-locks, panics, nondeterminism, ERROR spam — not
gameplay-correctness. See "Verification depth" below for exactly what
each oracle catches.

Bounds invariants check every entity with a `Transform` — zero
bevy_playtest-specific code in the game — and **fail loudly (not
vacuously)** when there are no entities to check.

### How mode 1 works

When `BEVY_PLAYTEST_SCENARIO` is set, your binary becomes its own headless
playtest runner (exit codes: 0 Pass, 1 Fail, 2 Crash, 3 config error). Without
the env var the plugin is inert. The runner swap replaces winit's event loop,
so no window appears. Under `BRP_PORT` the same binary serves the mode-2
external-BRP surface instead. In the recommended integration these modes only
exist in a playtest-enabled build (`cargo test`, or `cargo run --features
playtest`); the default and release builds contain zero bevy_playtest code.

Known limitations: rendering is unavailable (software-only) so
visual assertions do not apply.

### Log capture (the `log_error`/`log_warn` oracles)

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

Mode 2 (external BRP) does not capture logs — the game is a separate
process whose logs go to pipes; the parity matrix above reflects this.

Panics are captured independently of tracing: a panic during the run's
loop lands in the run's report; a panic in the setup window (before the
run starts) propagates out of `run_scenario` to the caller — the
harness does not swallow it, but produces no report for it.

### Verification depth (what is proven vs. what is asserted)

One mechanism takes a run from "asserted" to "proven":

1. **Replay divergence** (`"verify_replay": true`) runs the scenario
   twice against fresh Apps and compares per-frame fingerprints
   (choice draws + sorted Transform translations). Divergence is a
   `replay_divergence` Fail identifying the first divergent frame.
   This PROVES determinism for the observable surface — it does not
   prove absence of nondeterminism in unobserved state (components
   without Transform). In-process only; BRP mode rejects the field
   (wall-clock pacing makes honest verification impossible there).
   On the takeover/plugin path, pass B re-runs your game binary as a
   subprocess — whole-process determinism, the strongest form. NOTE:
   `verify_replay` requires `single_threaded: true` (default).

## Mode 2: External BRP (experimental)

No harness imports in the game at all — run the game as a subprocess and
drive it over Bevy's Remote Protocol. Requires the game binary to:

1. Add Bevy's remote plugins (`RemotePlugin::default()` +
   `bevy::remote::http::RemoteHttpPlugin::default()`, with the
   `bevy_remote` cargo feature)
2. Register the types the harness inspects:
    ```rust
    .register_type::<Transform>()
    ```
3. Read the listen port from the `BRP_PORT` env var and pass it to
   `RemoteHttpPlugin::default().with_port(port)` (the harness sets it)

Then from the test side — `cargo test` runs it like any other test.
`brp_runner` is behind the (non-default) `brp` feature, so the test-side
dependency must enable it:

```toml
[dev-dependencies]
bevy_playtest = { version = "0.3", features = ["brp"] }
```

```rust
use bevy_playtest::brp_runner::{run_scenario_brp, BrpRunConfig};

#[test]
fn playtest_brp() -> Result<(), Box<dyn std::error::Error>> {
    let cfg = BrpRunConfig {
        game_binary: env!("CARGO_BIN_EXE_my_game").to_string(),
        port: Some(4570),
        ..Default::default()
    };
    let scenario = serde_json::from_str::<bevy_playtest::scenario::Scenario>(
        r#"{"bot":{"type":"chaos"},"duration_s":10}"#,
    )?;
    let report = run_scenario_brp(&cfg, &scenario)?;
    assert_ne!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
    Ok(())
}
```
Note: unlike mode 1, the game has no playtest feature here — the
`bevy_remote`-enabled binary is the normal build. The report carries
`harness_mode: "external_brp"`. Limits versus mode 1: chaos
bot only and a reduced oracle set — bounds, NaN
transforms, and the frozen-world (liveness) check run over BRP, but panic
and log capture require mode 1 (or `PlaytestBrpPlugin` — see the parity
matrix above).

See the [docs](docs/README.md) for the full guides.
