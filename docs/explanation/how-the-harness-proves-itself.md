# Explanation: how the harness proves itself

bevy_playtest's core promise is a verdict you can gate CI on. That promise
would be worthless if the harness itself were only ever exercised against
healthy, self-authored demo apps. This page maps every suite in this repo and
what each one is *for* — the doctrine is that **every detection the harness
advertises is paired with a test that plants the fault it claims to catch**,
and every "green" outcome has a test proving it cannot happen by accident.

The suites live in `tests/` (integration tests) and inside `src/` modules
(unit tests). Everything is runnable as ordinary cargo commands; some tiers
are feature-gated and the fixture battery additionally needs the
fixtures generated, see below.

## The tiers at a glance

| Tier | Suite(s) | Count | What it proves |
|---|---|---|---|
| Unit | in-module `#[test]`s + doc-tests | ~39 | leaf logic: report XML, defaults, key mapping |
| Schema | `scenario_schema` | 25 | the scenario JSON grammar parses as documented |
| Negative validation | `negative_validation`, `vacuous_invariant_rule` | 8 | invalid/ignored config fails loudly |
| Oracle negative controls | `fault_injection` | 10 | every oracle detects the fault it names |
| Verification depth | `verification_depth`, `report_integrity` | 18 | verdicts and reports reflect reality |
| Process robustness | `brp_robustness`, `isolation`, `log_capture_race` | 17 | hostile games, poisoned processes, races |
| End-to-end | `takeover_e2e`, `driver_smoke`, `plugin_brp`, `mode_parity` | 42 | the whole loop, including subprocesses |
| Real games | fixture battery + external-BRP acceptance | 7 games × 2 modes | it works on unmodified third-party games |

Counts are `#[test]` functions per file at the time of writing; they drift as
suites grow, the tiers do not.

## Unit tests (in `src/`)

Fast, no `App` needed: JUnit XML generation and its hardening (control-char
replacement, `CDATA` splitting, U+FFFE/U+FFFF substitution), plugin
internals, BRP auto-enable logic, battery defaults, choice/RNG draw logic,
rule classification, report sinks, and run-state accounting. Doc-tests cover
the battery's public examples.

## Schema tests (`scenario_schema`)

The scenario grammar behaves exactly as the reference documents: defaults
(`tps` = 60), canonical key-name normalization (`"w"` vs `"KeyW"`),
constraints, and round-trips. Pure parsing — no game runs.

## Negative validation — "never a vacuous pass" at the boundary

The harness refuses configurations it cannot honor:

- Unknown fields fail at parse time (`deny_unknown_fields`), so a typo'd knob
  can't silently do nothing.
- Planned-but-unimplemented semantics fail at validation time with an
  explicit `NotImplemented` error instead of being accepted and ignored.
- An invariant that matches zero entities produces a **violation**
  (`vacuous_invariant`), not a green run — an oracle that watched nothing
  asserted nothing.

`negative_validation` encodes the rejection paths; `vacuous_invariant_rule`
encodes the zero-entity case and checks the rule classifies as gating (it
fails the run), not informational.

## Fault injection — the oracle negative controls

`fault_injection` plants a deliberate fault in a minimal app per test and
asserts the run **reports the expected violation rule**:

- NaN translation → `finite_transforms`
- entity out of bounds → `nodes_in_bounds`
- world stops updating → liveness/frozen-world oracle
- panic in a system → captured and surfaced, not swallowed
- ERROR/WARN logs → captured under the log oracle

This is the epistemological core of the project: a detection that has never
seen its own fault is a rumor. Every rule in the taxonomy has at least one
planted-fault proof.

## Verification depth and report integrity

- `verification_depth`: replay-divergence actually fires when hidden
  nondeterminism exists (same scenario + seed producing different
  fingerprints).
- `report_integrity`: what the `PlaytestReport` says matches what happened —
  provenance recorded, no phantom passes, findings not dropped on the floor
  between detection and report.

## Process robustness

The harness runs games as processes; hostile processes must not defeat it
(`brp_robustness`, against mock game binaries compiled on demand):

- stdout/stderr flooding → no deadlock
- crash at startup → clear error, no hang, no leaked process
- never binding the BRP port → startup timeout honored, bounded runtime
- death mid-run → surfaced as a failure, never a silent Pass

`isolation` proves runs don't contaminate each other: the panic/log capture
machinery is process-global (one exclusively-held active-run buffer, one
global panic hook), so cross-run leakage is a realistic failure mode. The
design answer is structural — a second concurrent `run_scenario` in the same
process is **rejected, not queued** — and the tests attack it from multiple
threads, which is also why fixture battery runs serialize
(`--test-threads=1`).

`log_capture_race` owns its own test binary: it installs a competing
tracing subscriber before the harness's capture subscriber, reproducing the
game-`LogPlugin`-wins race. The run must admit `log_capture_available=false`
plus an informational `log_capture_lost` finding — a green report must never
imply "no errors were logged" when it was deaf.

## End-to-end and parity

- `takeover_e2e` spawns real demo game **subprocesses** (the `takeover_demo*`
  bins) and verifies the exit-code contract end to end: 0 = pass with report
  written, 1 = invariant violation, 3 = configuration/diagnostics error
  (missing scenario file, invalid JSON, schema rejection, unknown env var,
  winit-present-with-takeover hard fail). Also: no env var → the game runs
  normally and untouched (inert takeover), smoke-literal UX, report
  placement, and the stock-game `all` selector.
- `driver_smoke` runs `run_scenario` against a minimal headless app in the
  test process.
- `plugin_brp` checks BRP auto-enable for `PlaytestPlugin` — including the
  rejection path when the feature is off.
- `mode_parity` asserts the same seed drives the same behavior in-process
  and over BRP: external BRP is advertised as driving the same chaos bot
  as in-process mode,
  and the determinism claim depends on that equality. The choice logic is
  compared fixture-independently; the full comparison is fixture-gated.

## The fixture battery — real, unmodified games

The strongest evidence tier: seven Bevy showcase games (`alien_cake_addict`,
`breakout`, `contributors`, `desk_toy`, `game_menu`, `loading_screen`,
`mines`) generated from a pinned Bevy revision by
`scripts/fetch_bevy_examples.sh` (not committed; only `tests/playtest.rs`
and `Cargo.lock` are).

Each fixture runs the default battery **against its own unmodified game
binary** in both modes:

- **Mode 1 (in-process)** — via a self-dev-dependency that activates the
  `playtest` feature, each game's committed `tests/playtest.rs` runs
  `bevy_playtest::battery::run(BUILTIN_BATTERY)` and asserts per-game known
  findings (e.g. `mines` expects exactly `[("frozen_world", "world")]` — it
  waits for pointer input the keyboard bot cannot give; the assertion pins
  that behavior until the synthetic-pointer bot lands).
- **Mode 2 (external BRP)** — `acceptance.rs` drives each fixture's stock
  binary over BRP; CI fans out one matrix job per fixture via
  `BEVY_PLAYTEST_FIXTURE`.

Two guards around the battery:

- The self-dev-dependency is stripped by `cargo package`, so the suite ships
  an *ignored test with instructions* rather than failing to compile or
  silently running zero playtests — refusal over silence, again.
- Runs serialize per process (`--test-threads=1`) because `run_scenario` is
  exclusive per process (by design, see isolation).

## Non-test CI gates

Three gates complement the suites:

1. **UX pin** — flagless `cargo test` on a fixture must not report ignored
   tests (the zero-config experience stays honest).
2. **Release purity** — `cargo tree -e normal` on a plain bin build must not
   resolve bevy_playtest (a game that doesn't opt in must not ship it).
3. Standard hygiene: formatting, lint and doc gates
   `--all-features`, rustdoc `-D warnings`.

## The doctrine, compressed

- Detection is proven by planting the fault, never presumed.
- Refusal over silence: unknown config, stripped dev-deps, lost log capture,
  dead games, concurrent runs — everything either fails loudly or marks the
  report as degraded.
- Reality anchoring: demo apps prove the loop; only unmodified third-party
  games prove the product.
