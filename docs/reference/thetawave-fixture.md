# Thetawave fixture (real-world port)

Reference for `fixtures/thetawave/`: a REAL third-party game —
Theta Wave — vendored into the fixture battery to prove the
declared-tier discovery machinery (Leafwing reader, intent-tap,
reconciliation) against code nobody involved in bevy_playtest wrote.
Where the [known-answer fixture](known-answer-fixture.md) checks the
instrument against hand-derived truth, this fixture checks it against
reality: RON-loaded `InputMap`s, multiple `Actionlike` types, real
gameplay actions, and zero knowledge of the harness.

It is deliberately NOT a known-answer oracle: there is no ground-truth
file, no truth rows, and no accuracy claim. The battery proves the
game boots headless under takeover and reacts to a real declared
action; the demo-level assertions prove the reader enumerates the
game's actual `InputMap`s and that a driven action reaches
`manager_pipeline_ok`. Honest claims only.

## NOTICE — upstream attribution

- **Upstream project**: Theta Wave —
  <https://github.com/thetawavegame/thetawave>
  (formerly `mdrummies/thetawave`)
- **Commit ported from**: `94fad04fb913bb50245ef33d96abc95ca8cd2355`
  (upstream `main`, 2024-10-06)
- **Upstream license**: MIT — kept verbatim as `LICENSE.upstream`
  inside the fixture directory (Copyright (c) 2022 Theta Wave)
- **Statement of modifications**: ported to Bevy 0.20 /
  leafwing-input-manager 0.22 (crates.io release; upstream pinned a
  git revision); companion crates bumped to their
  Bevy-0.20-compatible releases; assets reduced to the
  headless-minimum set (see below); playback-only/steam/wasm
  scaffolding, the serial-port cabinet-hardware crate, and the
  optional `cli` feature (argh) removed; local
  sqlite persistence (thetawave_storage) kept as an off-by-default
  `storage` feature — upstream parity, excluded from the playtest
  battery; gameplay unchanged. The port itself lives in the
  [mcanevet/thetawave](https://github.com/mcanevet/thetawave) fork
  (branch `bevy-0.20`) and is cloned into `fixtures/thetawave/` by
  `scripts/fetch_bevy_examples.sh` at the pinned commit SHA — same
  pattern as the showcase games, except the remote is our fork
  because the port is ours.

### Stand-in assets (honesty statement)

The upstream repository does not ship a runnable-by-default asset
set: ~55 art files are served from a private S3 bucket (absent even
from a pristine upstream checkout), the ~13 MB of real sound files
are likewise not in the repo, and one required font is not in it
either. The port substitutes, without touching gameplay code:

- **Placeholder art** — ~55 1×1 PNGs standing in for the
  premium S3-only textures, at the paths the game's asset lists name.
- **Silent audio** — one tiny silent WAV standing in for every real
  sound file, with all 30 dynamic-asset keys repointed at it.
- **Added font** — `fonts/Lunchds.ttf` ("Lunchtime Doubly So" by
  codeman38, <https://zone38.net/>, a 100%-free dafont.com font whose
  license asks for credit; credit hereby given). Absent from the
  upstream repo but required by its asset lists; added from an
  equivalent public copy.

None of these substitutions affect the input surface, which is what
this fixture exercises.

## What the committed tests assert

`fixtures/thetawave/tests/playtest.rs` (committed, path-neutral,
surviving fetch regeneration):

- **Battery** — the standard zero-config verdict run via
  `battery::run_and_expect_clean`: the game boots headless under
  takeover, all oracles stay clean, zero gating findings.
- **Reader demo** — a deterministic `MENU_DRIVE_SCENARIO`
  (Enter/arrows/WASD/Escape script) driving the real game binary,
  asserting from report data only:
  - the reader enumerates the game's actual RON-loaded
    `InputMap<MenuAction>` as `Declared` entries;
  - the intent-tap observes the driven action fire;
  - reconcile grades at least one binding
    `manager_pipeline_ok` when driven.

Known honest limitation, asserted as fact rather than papered over:
the reader snapshot freezes `lw_declared` at the first non-empty
frame-0 read, so only the `MenuAction` map appears there —
`PlayerAction` taps surface through the tap/reconcile evidence
instead, not through the frozen declaration list. And because the
game's background picker uses `rand::thread_rng`, the fixture makes
no determinism or byte-stability assertions.

## Running

Standard battery tier (same as every fixture):

```sh
cargo test --manifest-path fixtures/Cargo.toml \
  -p fixture_thetawave --features playtest --test playtest \
  -- --test-threads=1
```

`--test-threads=1` is required: `run_scenario` is exclusive per
process. Debug a scenario directly with
`BEVY_PLAYTEST_SCENARIO=<scenario> cargo run --features playtest`.

## Maintenance rules

- The vendored source is fetch-script-managed: never hand-edit
  `fixtures/thetawave/` outside the fork — change the fork, cut a
  new pinned SHA, update the script.
- Committed `tests/playtest.rs` is the exception: it is the
  canonical, hand-maintainable assertion file. The fork tree also
  carries a copy (so fresh checkouts get a working test file), and
  regeneration checks the two byte-for-byte: divergence fails loudly
  — the fetch script restores the committed copy and errors out
  (`FETCH_THETAWAVE_ALLOW_TEST_DIVERGENCE=1` is the
  experimentation-only escape hatch). After editing the committed
  copy, sync the same bytes to the fork and re-pin; do the same in
  reverse when changing the fork copy. No silent overwrites in
  either direction.
- Assertions stay honest: no oracle/truth file may ever be added for
  this fixture, and claims must remain scoped to what the report
  data actually shows.
