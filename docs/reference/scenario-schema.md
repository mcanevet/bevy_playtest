# Scenario (JSON schema)

> Applies to: bevy_playtest 0.14.x on Bevy 0.20. Update this
> stamp — and the page — in the same PR that changes the behavior
> it documents.

The full grammar for scenario files, the behavior of the two
scripted-input families, and every class of load-time rejection.

<!-- doc-lint: skip — the annotated grammar sketch below is
     illustrative: field alternatives make it non-JSON on purpose -->
```json
{
  "name": "string",              // [A-Za-z0-9_-] only (used in report file names)
  "bot": {
    "type": "chaos" | "synthetic_keyboard",
    "seed": number,               // default 42
    "input_rate_hz": number,      // default 10
    "key_presses": [              // synthetic_keyboard needs this, mouse_clicks or mouse_motion
      { "frame": number, "key": "string", "hold_frames": number }
    ],
    "mouse_clicks": [             // optional; scripted clicks, any bot type
      { "frame": number, "x": number, "y": number,
        "button": "left" | "right" | "middle" | "back" | "forward",
        "hold_frames": number }   // default 1
    ],
    "mouse_motion": [             // optional; scripted motion, any bot type
      { "frame": number, "dx": number, "dy": number,
        "repeat_frames": number } // default 1; net deltas per frame
    ],
    "raw_surface": {              // optional; read by chaos (ignored for
                                  // synthetic_keyboard — declaring it there
                                  // is a contradiction "passive" would catch)
      "keys": ["KeyW", "ArrowLeft"],
      "mouse_buttons": ["Left", "Right"]   // default: all three primary buttons
    },
    "explore": true               // optional, chaos only: exploring bot (see below)
    "passive": true               // optional, any bot: emit NO inputs — intentional
                                   // observe-only run; permits the empty input lists
                                   // (which otherwise reject) and rejects any declared
                                   // inputs alongside it
  },
  "tps": number,                  // default 60; range 1..=1000
  "simulated_time": boolean,      // default true
  "single_threaded": boolean,      // default true
  "strict_warnings": boolean,     // default false
  "duration_s": number,           // default 15.0; max 3600
  "verify_replay": boolean,       // default false; needs single_threaded. In-process runs two passes in one process; under the takeover a fresh subprocess is pass B. The report discloses whether it actually ran: see `replay_verified` in the report reference
  "record_digest": boolean,       // default false; record per-frame broadened state digest
  "invariants": [
    {
      "name": "string",
      "rule": "nodes_in_bounds",
      "min_x": number, "max_x": number,
      "min_y": number, "max_y": number,
      "min_z": number, "max_z": number
    },
    {
      "name": "string",
      "rule": "component_check",     // authored-intent oracle
      "target_selector": { "with_component": { "name": "Camera3d" } },
      "path": "translation.z",      // reflect path to a numeric leaf
      "check": "gt",                // lt | le | gt | ge | equals | ne
      "value": number,               // threshold
      "eventually_s": number        // optional: deadline (seconds)
    },
    {
      "name": "string",
      "rule": "resource_check",      // authored intent on a game-wide resource
      "resource": "Scoreboard",      // registered reflected resource type
      "path": "home",                // reflect path to a numeric leaf
      "check": "ge",                 // lt | le | gt | ge | equals | ne
      "value": 0,                    // threshold
      "eventually_s": 3.0            // optional: deadline (seconds)
    },
    {
      "name": "string",
      "rule": "stagnant_resource",   // rate-form livelock catcher
      "resource": "Scoreboard",      // watched resource type
      "path": "home",                // watched leaf: must CHANGE within window_s
      "driver_resource": "MatchClock", // optional: defaults to resource
      "driver_path": "elapsed_s",     // driver leaf: must strictly advance
      "window_s": 300.0,              // max DRIVER-LEAF units the watched leaf may
                                      //  stay constant (NOT seconds unless the driver
                                      //  counts seconds — the budget is consumed in
                                      //  the driver's own unit)
      "driver_units_per_second": 20.0 // optional: driver rate, enables load-time
                                      //  fit validation (window/rate must fit the run)
    }
  ],
  "oracles": ["string"],          // rejected: oracle selection not implemented
  "liveness": {
    "mode": "real_time",          // "off" rejected: not implemented
    "timeout_s": number,          // default 2.0; must span at least one frame
    "progress": {                 // progress-heartbeat liveness oracle
      "mode": "auto",             // "auto" | "markers" | "off" (default auto)
      "markers": ["string"],      // markers: named reflected resources (watched by hashed value)
      "include_paths": ["string"],// auto mode: extra reflected resources to watch;
                                   //  entries ending in "::" are path prefixes matching
                                   //  all resources under that module path
      "exclude": ["string"],      // auto mode: paths to ignore
      "window_s": number,         // default 8.0; verdict horizon — at authored
                                   //  configs must be a whole multiple of sample_s
                                   //  and sample_s a whole number of frames
      "sample_s": number,         // default 0.25; sampling cadence (whole frames)
      "require_surface": false    // empty verifiable surface → Fail (see below)
    }
  }
}
```

Key names accept `KeyCode` spellings (`ArrowLeft`, `KeyW`) or lowercase
shortcuts (`left`, `w`); unknown names are rejected at load with a
did-you-mean suggestion. Mouse buttons accept the same tolerance
(`Left`, `left`); valid buttons are `left`, `right`, `middle`, `back`,
`forward`.

## Paths to numeric leaves

`component_check` and `resource_check` read a numeric leaf through
`path`; `stagnant_resource` reads two — the watched `path` and the
driver `driver_path`. A path is a `bevy_reflect` path: field names,
nested fields, and index accesses, composed freely.

| Construct | Spelling | Example |
|---|---|---|
| Named field | `coins` | `"path": "coins"` |
| Nested field | `wallet.coins` | `"path": "wallet.coins"` |
| Array / list index | `score[0]` | `"path": "score[0]"` |
| Tuple-struct index | `0` or `.0` | `"path": "0"` |

Arrays and lists use bracket syntax. Scoreboards are the classic
case — `[u32; 2]` or `Vec<u32>`, not scalar fields — so the
home-score leaf of a `score` array is `score[0]`, never `score.0`:

```json
{
  "name": "score-home-nonneg",
  "rule": "resource_check",
  "resource": "MatchClock",
  "path": "score[0]",
  "check": "ge",
  "value": 0
}
```

Dot-indexing an array fails at evaluation — the tuple-access
grammar is the wrong tool for an array, and bevy_reflect says so in
its own words: `path 'score.0' does not resolve on this component:
Error accessing element with '.0' access(offset 6): Expected index
access to access a tuple, found a array instead.` Bracket-indexing
past the end fails equally loudly (`[5]` on a two-element array:
`The array accessed doesn't have index '5'`).

Newtype-style resources (`pub struct Coins(u64)`) are tuple structs
and take a bare index — `"path": "0"`, with the leading dot optional
(`".0"` is the same path).

Two limits of the bevy_reflect 0.20 path grammar: there is **no map
key access** (an `Access` is a field name or an index — HashMap
leaves are unreachable), and resolution errors always say
"component" even for a `resource` or `driver_resource` field (the
message formatter is shared by all three). An unknown or ambiguous
name reads the same either way:

```
unknown component 'Time' — not registered in this game's TypeRegistry
(components need #[derive(Reflect)] and App::register_type, or the name must be
the full registered type path)
```

Resolution accepts a full registered type path, or a final-segment
short name when it is unambiguous; a short name matching several
registered types rejects listing the candidates. One consequence
worth knowing in advance: with the engine clocks registered (any
app running `TimePlugin`), the bare name `Time` does **not** match
anything — use the full paths in [Engine clock drivers](#engine-clock-drivers)
below.

## Engine clock drivers

The rate-form `stagnant_resource` rule is usually driven by the
game's own clock — a match clock, turn counter, or day counter in
the game's own units (see the `stagnant_resource` bullet under
[Invariants](#invariants)). When the game has no such clock, the
engine's clocks work: every app running `TimePlugin` has
`Time<Virtual>`, `Time<Real>`, and `Time<Fixed>` inserted and
registered. Their registered paths spell the **definition modules**
of bevy_time, not the crate-root re-exports (`bevy_time::Time<...>`
fails as unknown):

| Registered path | Driver leaf | Units |
|---|---|---|
| `bevy_time::time::Time<bevy_time::virt::Virtual>` | `elapsed_secs_f64` | seconds of virtual time; pauses with `Time::<Virtual>::pause()`, honors `set_relative_speed` |
| `bevy_time::time::Time<bevy_time::real::Real>` | `elapsed_secs_f64` | seconds of wall time; never pauses |
| `bevy_time::time::Time<bevy_time::fixed::Fixed>` | `elapsed_secs_f64` | seconds of fixed-timestep sim time; advances in timestep quanta |

Driving on the engine clock looks like:

```json
{
  "name": "score-not-stagnant",
  "rule": "stagnant_resource",
  "resource": "MatchClock",
  "path": "score[0]",
  "driver_resource": "bevy_time::time::Time<bevy_time::virt::Virtual>",
  "driver_path": "elapsed_secs_f64",
  "window_s": 10.0
}
```

There is deliberately **no shorthand** for these paths. A
`"driver": "virtual_clock"` alias would hide *which* clock is
driven, and the rate is the semantics: Virtual pauses, Real does
not, Fixed steps in quanta. Games that *do* have their own clock
should drive on it — it encodes the game's time domain, and the
driver vocabulary stays uniform.

Paths that cannot reach a numeric leaf at all are covered next: some
std types reflect as opaque values.

## Opaque leaves: the Duration trap

Not every numeric-looking field is reachable. `core::time::Duration`
reflects as an **opaque** value — no fields, no subpaths — so
`Time.elapsed`, `Stopwatch.elapsed`, `Timer.stopwatch.elapsed` and
`Timer.duration` all fail with:

```
path 'elapsed' resolves to non-numeric type 'core::time::Duration'
— checks over this resource compare numeric leaves only
```

(the noun in the quoted message reads `resource` or `component`
depending on which kind of target the invariant watches)

The workaround is built into bevy_time: `Time` carries a redundant
float twin of its elapsed time (`elapsed_secs_f64` — the field
behind the driver leaf in the table above). `Stopwatch`-based types
have no such twin; if a timer must be watched or driven, watch a
cumulative float elsewhere (the game's own clock), or add a
reflected `elapsed_s: f32` twin.

A related hazard when choosing a driver leaf: a **repeating**
timer's remaining time sawtooths — down to zero, snap back to
duration, again. A sawtooth is not a strictly-advancing value; the
window machinery treats a retreating driver as a window reset every
cycle (by design — seek/replay must not accumulate budget). What
makes a good driver leaf:

1. **Numeric** — f32/f64 or any integer width (widened to f64).
2. **Strictly advancing while the game runs** — cumulative
   elapsed, turn counters, tick counts. Never a wrapped, repeating
   or clamped value (`elapsed_wrapped`, repeating-timer remaining).
3. **Freezing when the game freezes** — pausing the clock pauses
   the stagnation budget; a paused game must not be called
   stagnant. This is the load-bearing liveness property.
4. **In the game's own time domain if the game has one** — the
   author budgets `window_s` in the driver's units (next section).

## Window units: `window_s` is measured in the driver's own units

The stagnation budget is consumed by the driver leaf's own advance,
in the driver's own units — never converted through `tps`. The
authoring question is always "how much **driver advance** may pass
without the watched leaf changing?", and the wall-clock duration of
the window is an emergent property:
`wall_seconds ≈ window_s / driver_advance_per_wall_second`.

Load validation checks `window_s` as a positive, finite number.
When the scenario declares the driver's rate
(`driver_units_per_second` — the scale-annotating field), the
loader additionally checks the window's *wall cost*: a window
larger than what the run can deliver (`window_s /
driver_units_per_second > duration_s`) rejects at load with the
rate quoted in the error, so the author learns the mismatch
before paying for a run. Without a declared rate the default
frame-clock driver is seconds by definition (1 unit/s), so the
same fit check applies directly: a `window_s` at or above the
run's `duration_s` (checked strictly greater, so a window equal
to the full run stays accepted — fly-by controls pin this shape:
a judged episode approaches but, by the grace and frame budget,
never exactly reaches a full-run span) rejects at load for the
soak-family rules (`pairwise_separation`,
`derived_transform_consistency`, `forbidden_region` (when
`window_s` is declared — the instant-fire form has no soak to
fit), `entity_stagnation`). Otherwise the recorder would
silently discard every episode as sub-soak — a run that proves
nothing, reported as Pass. For `derived_transform_consistency`
the check compounds with `grace_frames`: episodes accrue span
only on judged (post-grace) frames, so a window longer than the
post-grace remainder of the run rejects too — the grace would eat
the only frames that could have accrued the span.

Three scales, worked:

**1:1 clock.** Driving on `Time<Virtual>.elapsed_secs_f64` at
default settings, driver units are seconds. `window_s: 10.0` means
"no change for 10 seconds of virtual time", costing ~10 wall
seconds per full window at relative speed 1.0.

**Accelerated clock.** A match clock advanced 18× faster than wall
time, with a driver leaf `minutes` counting in-game minutes:

```json
{
  "name": "home-score-not-stagnant",
  "rule": "stagnant_resource",
  "resource": "MatchClock",
  "path": "score[0]",
  "driver_path": "minutes",
  "window_s": 5.0
}
```

`window_s: 5.0` here means "no score change for 5 **in-game
minutes**" — roughly 16.7 wall seconds at 18×. The firing report
quotes the budget in driver units:
`MatchClock.score[0] stayed 0.000 for 5.000 of driver advance on
MatchClock.minutes`. Written the other way — thinking seconds —
`window_s: 30.0` would grant 30 in-game minutes, six times the
grace the author meant.

**Slow tick counter.** A tick resource advancing once per 1/20 s of
game time:

```json
{
  "name": "economy-not-stagnant",
  "rule": "stagnant_resource",
  "resource": "Economy",
  "path": "gold",
  "driver_resource": "TickCount",
  "driver_path": "ticks",
  "window_s": 200.0
}
```

`window_s: 200.0` means "no gold change for 200 **ticks**" — 10
seconds of game time at 20 ticks per second. Ticks-per-second never
appears in the JSON, so the conversion is the author's: "~10 seconds
of grace" → 10 / 0.05 = 200 ticks. Written the other way — thinking
seconds, writing `10.0` — the window would grant half a second and
fire constantly. Converting intent into driver units is the whole
exercise; the harness never converts for you. Declare
`driver_units_per_second: 20` on this invariant and the loader does
the arithmetic for you: 200 ticks at 20/s costs ~10 wall seconds,
and a 4-second run rejects the window at load citing the declared
rate instead of silently starving the window mid-run. An undeclared
rate is audited at run time instead: if the run ends with the window
never fully elapsing, the report carries a `window_unelapsed`
informational note ("the check was unjudgeable") rather than
guessing a verdict.

## Chaos mouse (default on)

The chaos bot clicks as well as types — no scenario changes required.
On each input tick it picks a random surface button, moves a harness
owned cursor to a random quantized position (a single draw, 16 bits
per axis, scaled into a 1280×1280 theoretical viewport), presses, and
releases after 2 frames (fixed, unlike chaos keys — see below). A
click reaches the game through three forms, mirroring winit's
pipeline: the raw `MouseButtonInput` message (feeds
`ButtonInput<MouseButton>`), the wrapped `WindowEvent` form (what
bevy_picking's mouse backend reads), and the primary window's own
cursor position (what bevy_ui's `ui_focus_system` reads to compute
`Interaction`) — so button state, pointer handlers and UI hover/press
all see the click where the bot placed it; scenario seeds replay
identically.

Opt out with an explicit empty surface:
`"raw_surface": { "keys": [...], "mouse_buttons": [] }`. Symmetrically,
`"keys": []` opts out of chaos keys (mouse-only chaos); omitting a
field keeps its default. A fully-empty surface rejects at load.

## Chaos keyboard (multi-slot holds)

Chaos keys are no longer one-at-a-time. On each input tick (still
governed by `input_rate_hz`) the bot presses one surface key whenever a
hold slot is free, holding up to **2 keys concurrently** and releasing
each after a **per-press drawn duration of 2–30 frames**. This lets the
bot form chords (accelerate + steer) and exercise hold-to-charge
mechanics without knowing what any key means. The draw selects among
surface keys not currently held — a physical user cannot press a key
they are already holding — so every key draw delivers a real press.
Expiry follows a last-holder-releases rule as defense-in-depth (see the
`ChaosHoldState::release` doc comment), but the candidate filter makes
same-key double-holds unreachable with today's draw policy: a key can
never occupy two slots, so `ButtonInput` never sees a premature
release.

Both per-press draws — key index and hold duration — are recorded in
`choices` with their frame and bound, and covered by the replay
fingerprint. Duration draws are tagged `kind: "key_hold_frames"`
(the key-index draw stays `kind: "key"`); the action log derives its
`key_press` entries from key draws only, so a duration draw never
masquerades as a press. Note that `choices` streams are not comparable
across harness versions that change the draw sequence — the added
duration draw (and the candidate-set bound) changes per-frame draw
order by design.

## Explore mode (`"explore": true` on a chaos bot)

Opt-in for authored scenarios; on by default in the built-in battery.
The bot remembers what it tried on each screen (the set of enabled
widgets it can see) and prefers the least-tried key or widget there,
clicks widgets three times out of four (else a random spot), and
presses Escape to back out after 3 s without anything new (a new
state or a new screen): never on the first screen seen, and waiting
twice as long after each back-out on the same screen. Escape needs a
key surface, so a scenario declaring no keys never backs out. On
every new screen it also tries a built-in prior of 27 common keys
(movement, digits, Space/Enter, modifiers, E/F/R) once each, so keys
the source scan cannot see (bound in config files, plugin crates,
macros) are still found; a key followed by a new state or screen
within one input tick, with no click in between, stays in rotation
and is listed under `keys_found` ("followed by", not proven to cause
it). To keep that attribution clean, explore mode alternates input
ticks between the keyboard and the mouse; a device with nothing to
do yields its tick. Every fourth key press is a combo: Ctrl, Shift
or Alt held under a declared or productive key (Ctrl+S, Shift+L), the
least-tried combo on the screen first; combos followed by something
new are listed under `combos_found`. Chords of two ordinary keys
(A+D) are only tried by chance. Every fourth mouse action is a drag
when a node on screen can be dragged (an entity observer of
`DragStart`, `Drag` or `DragEnd`: kind `draggable`) or accepts drops
(an observer of `DragDrop`: kind `drop_target`), every sixteenth
otherwise: press on a draggable node (else the least-tried widget or
a random spot), move while held for 11 frames, and release three
times in four on a drop target (else another widget), otherwise on a
random spot; `drags` counts them. Clickable nodes that declare
nothing (a grid of cells handled by one global click observer) are
learned: when a click at a random spot lands on a node carrying one
of the game's own components (or under one, like a cell's label) and
the game reacts within one input turn with no key pressed or released
around it (a state change, or a game resource changing that had been
quiet before the click), that node's archetype earns a credit; after
two, every node of that archetype is a click target
(`learned_targets` counts the archetypes). A click followed by a
state change learns at once, unless a key was pressed around it.
Learned targets are named with a `~` prefix and do not change the
screen identity. Every choice is still drawn from the seeded stream,
so runs replay exactly. The report then carries an `exploration`
section: screens seen, states entered, widgets never clicked,
declared keys never pressed, prior keys and combos found, back-outs,
and a verdict — "plateaued" (nothing new in the last quarter of the
run) or "still discovering when the run ended", in which case "no
findings" only covers what was reached.

## Rejection at load

Two distinct error classes, at different stages:

- **Unknown fields** (any misspelled or never-implemented knob, including
  once-documented-in-planning names like `persona`, `actuator`, `inputs`,
  `agent_target`, or gamepad/wheel surface knobs) fail deserialization
  immediately with a serde unknown-field error naming the offender.
- **Known-but-unimplemented fields** parse, then reject at validation
  with `NotImplemented`: `after_s`, `before_s`,
  `eventually_s` (except on `component_check` and `resource_check`,
  where it is supported),
  `oracles` selection,
  `liveness.mode: "off"`. Unknown rule values
  (e.g. `custom`) and unknown bot types (e.g. `pursuit`, `replay`) fail
  deserialization naming the offender. Rule-scoped reserved fields
  reject with `Rejected` naming the invariant: the resource vocabulary
  (`resource`, `window_s`, `driver_resource`, `driver_path`) is
  meaningful only on `resource_check` / `stagnant_resource`
  (`window_s`/`driver_*` also on `pairwise_separation`,
  `derived_transform_consistency`, and `forbidden_region`, and —
  as the `driver`/`resource` discriminator — `entity_stagnation`),
  and
  binding any of it to `component_check` or `nodes_in_bounds` fails
  at load — a silently-ignored `window_s` on a component check
  would masquerade as a temporal bound that never existed. The
  spawn vocabulary extends the same doctrine:
  `min_distance_from`/`spawn_only` are meaningful only on
  `spawn_safety`, `min_distance` it shares with
  `pairwise_separation` (required there, required here), and
  `window_s`/`driver_*` never on it — an `all` target_selector
  there rejects too (the judged
  archetype must be named; `min_distance_from` is the side that
  may go `all`). Blank
  string fields (`path`, `driver_path`, `driver_resource`,
  `resource`) reject at load too: a blank reflect path resolves the
  resource root, never a numeric leaf.
- **Invalid values** reject with specific `Rejected` errors:
  `tps` outside 1..=1000; `duration_s` non-finite, ≤ 0, or > 3600;
  total ticks (`duration_s × tps`) > 10,000,000; simulated-time frame
  duration > 0.250 s (i.e. `tps` < 5 — `Time<Virtual>`'s max_delta
  clamp would silently distort dt); `liveness.timeout_s` non-finite,
  ≤ 0, or shorter than one frame at the scenario's `tps`; authored
  `liveness.progress` mis-sizing (window ≥ `duration_s`, sampling slower
  than one frame, sampling ≥ window, non-finite/negative values,
  `markers` mode with an empty marker list — the default progress
  config is exempt from the duration-coupled checks so it can ride
  along short smoke runs); scenario
  `name` outside `[A-Za-z0-9_-]` (it is embedded in report file names —
  path safety); broken bound pairs; typo'd or empty key lists.

## Invariants

- `nodes_in_bounds`: per-entity AABB check on every entity with a
  `Transform` (zero-annotation);
  bounds are per-axis (`min_x`/`max_x`/`min_y`/`max_y`/`min_z`/`max_z`),
  each defaulting
  to ±1000 when unset. The invariant's `name` must not look like a
  bevy entity debug string (`3v1`) or the literal `entity` — the name
  doubles as the finding's target, and such a name would collide
  with the seed-sweep fingerprint of the global bounds oracle (which
  canonicalizes per-entity targets). A `target_selector:
  {"with_component": {"name": "..."}}` narrows the check to entities
  carrying that named registered component (full type path or an
  unambiguous short path — same resolution rules as
  `component_check`, ambiguity and unknown names reported as
  `oracle_error`); an invariant that never matched a checkable entity
  is `vacuous_invariant`. Additionally, a hard-coded global bounds check
  (±1000 on x/y/z) runs on all entities regardless of invariants or
  tagging — stray geometry will trip it even with zero invariants declared.
- `component_check`: an authored-intent oracle. Selects every entity
  carrying a named registered component
  (`target_selector: {"with_component": {"name": "..."}}` — full
  registered type path, or an unambiguous short path like
  `Camera3d`; short names matching several registered types reject),
  reads a numeric leaf via a reflect `path` (e.g. `"translation.z"`
  — tried on the selector component first, then on the entity's
  `Transform` so identity components can select while the check reads
  position), and compares it against a numeric `value` with `check`
  (`lt`/`le`/`gt`/`ge`/`equals`/`ne`). Without `eventually_s` the
  predicate must hold on every frame where the selector matched
  anything; with `eventually_s: N` it must hold for all selected
  entities simultaneously at least once within N seconds of the
  run start (holding only after N seconds elapsed misses the
  deadline and fails; holding at exactly the deadline boundary
  frame counts). `eventually_s` must be positive, finite, and ≤
  `duration_s` — a deadline past the run's end can never elapse
  and is rejected at load. "Simultaneously" is judged per frame
  against the entities matched at that frame: satisfaction
  recorded before a new entity spawns is not retroactively
  invalidated by the latecomer, even if it never satisfies.
  Zero
  matches over the whole run → `vacuous_invariant`; an unresolvable
  name/path → `oracle_error` naming the invariant (config errors
  suppress the vacuous/deadline audit entries for that invariant,
  so the report tells one story). This is the rule
  that catches bugs the generic oracles cannot see: a player whose
  forward input never accumulates (position oscillates near spawn)
  keeps moving and stays in bounds, so only the authored expectation
  "holding W drives the camera forward" flags it.
- `resource_check`: the resource sibling of `component_check` — an
  authored-predicate oracle over a game-wide singleton. Names a
  registered reflected resource (`resource`: full registered type
  path, or an unambiguous short path — same resolution rules as
  `component_check`), reads a numeric leaf via a reflect `path`
  (e.g. `"coins"`, `"score[0]"` — see [Paths to numeric
  leaves](#paths-to-numeric-leaves)), and compares it against a
  numeric `value` with `check` (`lt`/`le`/`gt`/`ge`/`equals`/`ne`).
  NOTE on spelling when grepping reports: the schema accepts the
  word forms (`le`, `ge`, `lt`, `gt`), but violation details RENDER
  the comparison with its symbol (`<=`, `>=`, `<`, `>` — `equals`
  and `ne` render as `==`/`!=`) — a `resource_check` miss reads
  `Scoreboard.home = 1.000 violates <= 2.000`, never
  `violates le 2.000`. Authors writing test needles (or greps)
  against report detail strings must use the rendered symbol
  spelling, not the schema spelling.
  Without `eventually_s` the predicate must hold every frame where
  least once within N seconds of the run start (the deadline must
  be positive, finite, and ≤ `duration_s`). Resources are the
  natural home for economy/score invariants: a coin total that goes
  negative (`check: "ge", value: 0`), a health pool that must
  eventually recover. `target_selector`, bounds fields, and the
  driver fields (`driver_resource`/`driver_path`) are rejected for
  this rule at load. A resource that is registered and reflected but
  never inserted over the whole run → `vacuous_invariant`; an
  unknown/ambiguous name, a non-Resource type, or an unresolvable
  path → `oracle_error` naming the invariant (config errors suppress
  the vacuity/deadline audit for that invariant, so the report
  tells one story).
- `stagnant_resource`: the rate-form livelock oracle. Watches a
  numeric leaf (`path` on `resource`) and requires it to CHANGE at
  least once per `window_s`-worth of advance of a declared driver
  leaf (`driver_path` on `driver_resource`, which defaults to the
  same resource; the engine clocks are documented under [Engine
  clock drivers](#engine-clock-drivers)). The window budget is
  consumed by the DRIVER LEAF'S OWN advance — in the driver's own
  units, never converted through `tps` — so the author's clock
  domain is authoritative (a match clock counting literal in-game
  seconds is the canonical driver; see [Window units](#window-units-windows_s-is-measured-in-the-drivers-own-units)).
  `window_s` is measured in the DRIVER'S UNITS, not seconds: a tick counter
  advancing +1 per FixedUpdate tick at 20 ticks/s consumes a
  `window_s: 300` budget in 300 TICKS (= 15 wall seconds), and a
  300-second window on such a driver would be written `window_s:
  6000`. A watched leaf constant for more than `window_s` of driver
  advance while the driver strictly advances is a livelock finding,
  reported once per episode. Judgeability: with
  `driver_units_per_second` declared, `window_s / rate` must fit
  inside `duration_s` (rejected at load otherwise); with it
  omitted there is no duration-coupled rejection, and a window the
  driver's TOTAL observed advance never reached is reported as an
  informational `window_unelapsed` note at run end (naming the
  observed advance) — never a failure. Budget windows with headroom:
  a run of `duration_s` at `tps` observes `ceil(duration_s × tps)`
  FRAMES and one fewer POSITIVE DELTAS, so a window authored at
  exactly `duration_s × driver_units_per_second` (e.g. 400 units
  for a 20 s run at 20 units/s) is unjudgeable by that one unit and
  lands in the `window_unelapsed` note — author windows at
  `duration_s × rate − 1` or smaller to guarantee the full budget
  elapses. Two invariants sharing one
  driver leaf must declare the same rate or the scenario rejects
  (a full-path and a short-name declaration of the same type count as
  one driver; two distinct full paths are always distinct drivers,
  even when their final segments coincide).
  (`stagnant_resource` rejects `eventually_s` at load, so the
  elision audit never interacts with deadlines.) Note:
  `strict_warnings` does NOT escalate `window_unelapsed` — a
  driver that legitimately pauses is game behavior, so there is no
  knob to make an unjudgeable window fail the run; shrink the
  window or lengthen the run instead. The finding is
  reported once per stagnation episode; the value changing later clears the episode
  and starts a fresh window — an oscillating value is "changing",
  not stagnating. A non-advancing driver FREEZES the window: time
  stopped (pause screens, victory states), so stagnation is not
  provable and a paused clock may legitimately hold a frozen
  scoreboard; a retreating driver (seek/replay) resets the window.
  An absent driver (removed mid-run, never inserted) breaks
  observation continuity: the first fully-observed frame afterwards
  starts a FRESH window — driver advance across the gap is not
  witnessable, so it may not be credited toward the budget. Likewise
  the game removing the WATCHED resource mid-run (teardown, back to
  menu) reads as absence — a legal runtime event, never an
  `oracle_error` — and a non-finite sample (NaN or infinite) in
  EITHER leaf is reported loudly (`oracle_error` naming which leaf,
  watched or driver): a poisoned sample would otherwise disarm or
  freeze the window forever (NaN never equals itself), and the
  frame is treated as an observation gap so driver advance across
  it is never credited. Later non-finite samples on either leaf are
  covered by this one finding.
  This is the rule that catches the bugs every generic oracle is
  blind to: a match whose scoreboard stays 0-0 the whole game
  while the match clock ticks — the world churns, the clock runs,
  motion/liveness/progress oracles all stay green, only the value
  under judgment stands still. Predicate fields
  (`check`/`value`), `eventually_s`, `target_selector`, and bounds
  fields are rejected at load; a redundant
  `driver_resource` equal to `resource` is rejected as likely
  authoring confusion — the full message is
  `driver_resource is redundant when it names the same resource as
  'resource' — omit it (the driver leaf defaults to the same
  resource)` — and the same-resource driver is authored by setting
  only `driver_path` (e.g. `"driver_path": "minutes"` on a match
  clock watched at `score[0]`, driven by its own `minutes` leaf).
  Honesty doctrine matches `resource_check`:
  never-resolved resource/leaf → `vacuous_invariant`;
  a watched leaf that resolved while the DRIVER never did →
  `vacuous_invariant` naming the driver (stagnation was unprovable,
  not proven);
  unknown/ambiguous names → `oracle_error`.
- `pairwise_separation`: the coincident-bodies spatial oracle.
  Selects every entity carrying a named registered component
  (`target_selector: {"with_component": {"name": "..."}}` —
  required; "all" is rejected at load because pairwising every
  `Transform` in the world — walls, camera, UI — at O(n²) is
  never what an author means) and requires every PAIR of
  selected entities to stay at least `min_distance` apart.
  `min_distance` has ε semantics: it is "coincident bodies",
  NOT "personal space" — author it BELOW gameplay-speed
  closing distances or legitimate fly-bys false-positive.
  Violating frames are folded into EPISODES: a maximal stretch
  over TIME (not over pair identity — overlapping or handed-off
  pairs never split an episode, so a persistent offender cannot
  masquerade as many short findings), and an episode only
  becomes a finding when its `window_s` soak elapses (forward
  driver-leaf advance only; a retreating driver is a seek, not
  progress). Episodes shorter than the soak are discarded as
  transient fly-bys. The finding reports the WORST observation
  (minimum distance, tie-broken by earliest frame) plus, in
  `_meta`, a bounded roster of distinct pairs (first + worst +
  frame count per pair, capped, truncation-disclosed) so a
  handoff A–B → A–C cannot hide the persistent offender A.
  `window_s` is required on `pairwise_separation` (a windowless
  form is rejected at load — unlike
  `forbidden_region`, pairwise has no instant-fire variant) and
  is measured in DRIVER UNITS like
  `stagnant_resource`'s window: declare
  `driver_path`/`driver_resource` for a game-clock soak (both
  halves or neither — there is no `resource` to default the
  host to), or omit them to soak against the frame clock in
  seconds (which has no declarable rate:
  `driver_units_per_second` without a declared driver is
  rejected). Soak gaps — a missing, removed, or non-finite
  driver sample — CLOSE the open episode rather than extending
  it across an unobservable clock; the evidence gathered so
  far is judged on its own accumulated span. A selector that
  never has a pair to check over the whole run →
  `vacuous_invariant`. `check`/`value`/`eventually_s`/`path`/
  `resource` and the bounds fields are rejected at load.
- `derived_transform_consistency`: the compositional identity over
  Bevy's one canonical derived relationship — every entity with a
  `Parent` must observe
  `GlobalTransform == parent's GlobalTransform ∘ own Transform`,
  with both sides read at the SAME settled frame (the check asks
  "is the stored pair internally consistent", never "did the game
  call propagate"). `target_selector` is OPTIONAL: unlike
  `pairwise_separation` (whose O(n²) over "all" is never what an
  author means), the whole hierarchy is exactly what this rule is
  for, so absence means every parented entity. Optional knobs:
  `tolerance` (absolute per-element slack between observed and
  composed GlobalTransform; healthy f32 composition sits at
  ~1e-6..1e-5, real misses diverge by whole units — loosen for
  deep hierarchies, a deliberate author decision rather than a
  hidden growth formula), `grace_frames` (startup-settle skip;
  relationship application and first propagation land
  asynchronously in the startup sequence), and
  `exclude_components` (type-path PREFIX list — the
  progress-surface exclude grammar — for entities that
  legitimately write `GlobalTransform` after propagation:
  camera-smoothing ghosts, interpolation layers; prefer the
  in-game `SkipDerivedConsistency` marker component for
  per-instance opt-outs). `window_s` is REQUIRED and is the
  PERSISTENCE soak, not a fly-by discard: consistency defects
  are state bugs (frozen/stale globals), so violating frames
  fold into episodes and only an episode that soaks past
  `window_s` (in driver units — the same
  both-halves-or-neither driver grammar as
  `pairwise_separation`, frame-clock default in seconds) counts
  as a finding. NOTE: the rule is PROMOTED to gating — flip-test
  evidence demonstrated the pile-up discrimination (defect-present
  RED, defect-absent GREEN, rotating/scaling hierarchies
  coherent), so a soaked divergence finding FAILS the run.
  `check`/`value`/`eventually_s`/`path`/
  `resource`/`min_distance` and the bounds fields are rejected
  at load.
- `derived_transform_consistency` note vocabulary (both
  INFORMATIONAL, never gating): a rule that never found a
  parented entity reports `hierarchy_absent` — a flat world is a
  coherent scene, and the note certifies it as TRULY flat. A
  rule that found a parented entity in scope whose parent was
  never observable with a `GlobalTransform` reports
  `unjudgeable_parent` instead — a hierarchy the rule could not
  read is not a flat world, and the note says so rather than
  letting a broken-but-unreadable subtree read as coherence.
  The same note also fires once when SOME subtrees were judged
  and at least one stayed unreadable (partial blindness: the
  verdict covers what was checked, and the unchecked subtree is
  disclosed, never silently dropped).
- `forbidden_region`: the declarative no-go-zone oracle — the
  author draws an axis-aligned box an entity's translation must
  not LINGER inside (the rail-tunneling incident class, made
  declarative). `target_selector` is REQUIRED (`with_component`
  only; "all" rejects at load — a region forbidden to every
  `Transform` in the world, walls and camera included, is never
  what an author means) and selects Transform-bearing entities.
  `region` is required and is an inclusive box:
  `{"center": [x,y,z], "half_extents": [hx,hy,hz]}` — a
  translation lying EXACTLY on a face counts as inside (the
  boundary belongs to the region), half-extents must be finite
  and strictly positive (a zero-thickness region is a plane —
  vacuous by construction), and NaN translations are never
  inside (comparisons are false — `finite_transforms` keeps
  owning NaN findings, no double-reporting). `allow_if`
  (optional) names a REGISTERED component that marks
  LEGITIMATE presence (a mounted rider on the rail-surface may
  legally overlap): the exemption is resolved per entity, per
  frame — an exempt entity stops counting as incursive at the
  acquisition frame, so its roster participation ends there and
  loss re-opens its judgment; the EPISODE is per-invariant, so
  an acquisition closes the open episode only when no incursive
  entity remains (a survivor's soak spans the exempted
  period — the roster still shows the exempted entity's
  `violating_frames` stopping at acquisition). AUTHOR IT AS A
  FLIP TEST: name
  the component whose presence is the LEGITIMATE state, never
  the buggy one, and keep a RED/GREEN pair in the acceptance
  table. Incursion frames fold into EPISODES keyed by entity
  (one region per invariant — folder identity IS region
  identity; two zones means two named invariants), with the
  every-frame-push discipline: frames with zero incursions still
  push, so an excursion out and back cannot weld two visits into
  one long stretch. `window_s` is OPTIONAL and picks the form:
  PRESENT = the soak — an episode only becomes a finding when
  its soak elapses (the same both-halves-or-neither driver
  grammar, frame-clock default in seconds, declared-rate fit at
  load — a window the run can never elapse rejects; sub-soak
  incursions are transient fly-bys, discarded); ABSENT = the
  INSTANT-FIRE form — every closed incursion episode is a
  finding, no minimum dwell, the one-frame clip counts exactly
  as much as a full soak (a tunnel-through caught in the act of
  crossing, not only when it lingers; for "no presence is ever
  legitimate" zones — pits that are supposed to be sealed —
  instant is the designed default, since any presence is the
  bug). Because the driver fields measure a soak, they REQUIRE
  `window_s` — driver fields on a windowless forbidden_region
  reject at load. The observation value is PENETRATION DEPTH
  (distance to the nearest face; deeper is worse), so the worst
  evidence names the most-embedded frame, and the finding's
  `_meta` carries a capped roster of distinct incursive entities
  (worst depth + frame per entity, truncation-disclosed) — a
  crowd cannot hide behind the cap. A selector that never
  resolves a checkable entity over the whole run →
  `vacuous_invariant` (gating). `check`/`value`/`eventually_s`/
  `path`/`resource`/`min_distance`/`tolerance` and the bounds
  fields are rejected at load.
- `spawn_safety`: the archetype-entry spatial oracle — the rule
  for the pile-up incident class where a respawned body APPEARS
  inside the danger zone. Judged ONCE, at the frame an entity
  enters the selected archetype: a newborn bearing the target
  component at spawn, or a PRE-EXISTING entity first acquiring
  it (the component-addition transition; `spawn_only: true`
  filters the verdict to brand-new entities when the
  conversion shape is legitimate). `target_selector` is
  REQUIRED and must name a component (the same resolution
  rules as `component_check`; an explicit `all` is rejected —
  the judged archetype must be named) and
  `min_distance_from` selects the ANCHOR set (the full
  target-selector grammar including `all`: everything with a
  settled `GlobalTransform`), against which every entry must
  clear `min_distance` (a positive finite radius). Anchors are
  snapshotted at the END of the same judging frame, so
  same-frame anchors PARTICIPATE — an anchor arriving alongside
  the entry is a participant in the judgment, not an excuse.
  The entering entity itself is never its own anchor (an `all`
  anchor selector would otherwise measure every entry at
  distance 0 from itself). There is NO episode and NO soak:
  the transition frame is the whole verdict, and the rule
  GATES — a body appearing inside the zone is a state bug AT
  the transition, not a transient fly-by the window doctrine
  exists to smooth over (the pairwise rule owns the
  persisted-overlap shape; the entry verdict and the soak
  verdict are different questions). The frame-0 scene is never
  an entry: the recorder stores its first snapshot at frame 0
  and judges nothing — entities the scenario authored upfront
  are the initial scene, not spawn events, and planted
  mid-run transitions are exactly what the rule exists for.
  Id-reuse soundness is generation-tracked: the diff keys on
  (index, generation) pairs, and a despawned pair leaving the
  previous snapshot forces every re-entry to be judged anew —
  and observable recycles cannot forge continuity anyway: the
  World-path despawn bumps the generation before freeing
  (`mark_free(index, 1)`), so a recycled slot returns as a NEW
  (index, generation) pair unknown to the previous snapshot,
  while same-tick re-fills take a fresh slot outright
  (`Commands::spawn` allocates eagerly at queue time, before
  any despawn command applies). An invariant whose anchors never
  resolve over the whole run → `vacuous_invariant` (the rule
  could never judge anything, the pairwise never-a-pair
  doctrine, anchor-shaped). `window_s`/`driver_resource`/
  `driver_path`/`driver_units_per_second`/`check`/`value`/
  `path`/`resource`/`eventually_s` and the bounds fields are
  rejected at load; `min_distance_from`/`spawn_only` on other
  rules are rejected too (foreign fields).
- `entity_stagnation`: the per-entity rate-form liveness oracle
  — the rule for the immortal-guest incident class (guests
  arrive at a demolished ride, enter `Queued`, and wait
  forever: no panic, no error log, the world looks perfectly
  alive; only the per-entity soak can name the stuck one).
  `target_selector` is REQUIRED (`with_component` only; an
  explicit `all` rejects at load — stagnation-watching every
  `Transform` in the world, walls and cameras included, would
  judge legitimately inert scenery) and selects the watched
  entities. `watch_path` is required and names the REFLECT
  PATH of the leaf whose value is digest-compared across
  samples — e.g. `hunger` (a plain field) or
  `state.Queued.waited` (a variant-qualified path reaching
  into an enum's struct variant). The watch is SCOPED: a
  mutation anywhere OUTSIDE the path does not clear stagnation
  (watching `state` does not care that `hunger` mutated), and
  the digest comparison is BITWISE on the leaf's value — f32
  bit patterns via `to_bits()` — so -0.0 → +0.0 counts as a
  change and distinct NaN payloads are distinct values; the
  leaf must actually change bits. `leaf_where` (optional) is a
  precondition with a deliberately narrow grammar,
  `path == VariantName` — only entities whose reflected `path`
  currently holds `VariantName` are watched (e.g.
  `state == Queued`), so a guest who walks past a demolition
  site healthy is never judged for standing still. The path
  and the variant are checked at PROBE time: a typo there
  surfaces as an `oracle_error`, never a silent never-true. A
  variant-qualified `watch_path` implies the corresponding
  `leaf_where`; both present is legal and idempotent. The
  leaf is read on a SAMPLED CADENCE: every `sampling_s` of
  driver-leaf advance (default 1.0 driver-leaf unit; the
  cadence converts
  through a declared `driver_units_per_second` the same way
  `window_s` does), with fit checks at load — a sub-frame
  cadence rejects (repeated identical samples) and
  `window_s >= 2 * sampling_s` is enforced so a soak always
  spans at least two samples. `driver` picks the clock the
  budget is consumed against: `"global_time"` (the default —
  the frame clock) or `"resource"` (soak against a game leaf
  via `driver_resource` + `driver_path` + optional
  `driver_units_per_second`), the same both-halves-or-neither
  grammar as `pairwise_separation` (the driver must strictly
  advance; a retreating driver is a seek, not progress).
  `window_s` is REQUIRED and is the SOAK: a stretch where an
  entity's leaf is digest-constant across consecutive samples
  accrues span in driver-leaf units (sample-to-sample driver
  advance, summed — RELAY SAFE: a stint ending by change or
  despawn banks its span, a sampled gap where no qualifying
  entity was readable contributes nothing, and a successor
  entity at a recycled slot is a NEW (index, generation) pair
  whose stint starts fresh — slot recycling can never weld a
  successor to its predecessor's stint, and despawn is
  ACQUITTAL: turnover never refunds the span already served).
  An episode — a maximal connected stretch of stuck entities —
  only becomes a finding when its worst entity's span soaks
  past `window_s`; sub-soak stints are discarded as lulls. The
  finding's `_meta` carries a capped roster of distinct stuck
  entities (index AND generation, first/worst frames, accrued
  stuck-span in driver units per entity, truncation
  disclosed) — the immortal guest cannot hide behind a crowd.
  Vacuity is SPLIT by what the world offered: a selector that
  never RESOLVED a checkable entity over the whole run →
  `vacuous_invariant`; a resolved watch whose `leaf_where` was
  never satisfied is a PASS WITH EVIDENCE (the held trace
  keeps its entry with zero judgments — the world simply never
  offered the qualifying state); likewise a watch that ran and
  judged every sample but saw no stuck leaf is a pass whose
  per-sample digest workload still lands in the held trace.
  `check`/`value`/`eventually_s`/`path`/`resource`/
  `min_distance`/`region`/`tolerance` and the bounds fields
  are rejected at load; `watch_path`/`leaf_where`/`sampling_s`
  on other rules are rejected too (foreign fields).
- `occupancy_cap`: the local-density crowd oracle — the rule for
  the congestion incident class, and the first WARN-ONLY
  invariant (see the proposal §1/§8 ship-gate honesty: the
  round-1–round-4 record contains no measured crowding
  litmus, so the vocabulary ships WITHOUT fail-power; the
  park sim's queued-at-entrance crowds are the natural first
  owner). `target_selector` is REQUIRED (`with_component`
  only; an explicit `all` rejects at load) and names the
  counted crowd — marker plus `Transform` (a marked entity
  without a `Transform` is not checkable, the family
  doctrine). `radius` is required, finite and > 0; `cap` is
  required, an INTEGER ≥ 1 (`cap: 0` rejects — self-inclusive
  counting means the count is always ≥ 1, so a zero cap
  degenerates to "fire whenever any entity is selected").
  The QUESTION is per-entity density: at most `cap` selected
  entities (SELF-INCLUSIVE — everyone counts themselves) within
  `radius` of a selected entity's position, INCLUSIVE on the
  boundary (distance exactly radius counts as within, the
  squared-distance compare `d² ≤ r²` with no sqrt).
  Enumeration is ENTITY-WITNESSED, not point-sampled: the
  count runs over entities (a spatial hash buckets positions
  for counting only), so a wide-spaced ring around an empty
  center — adjacent members within radius, diagonals outside —
  is a documented NON-FIRING case when each member's
  self-inclusive count stays at cap. `window_s` is REQUIRED
  and is the SOAK: a crowd — a maximal connected stretch of
  over-cap witnesses, UNION TIMELINE (witness-set changes
  never close an episode; only a frame with no witnesses, a
  non-finite driver sample, or a driver gap does) — only
  becomes a finding when its span soaks past `window_s`;
  sub-soak gatherings are discarded as transients. `driver`
  picks the clock the soak is measured against: the frame
  clock (default) or a game leaf via `driver_resource` +
  `driver_path` + optional `driver_units_per_second`, the
  same both-halves-or-neither grammar and fit checks as
  `pairwise_separation` (rate without a declared leaf, a
  window exceeding the run, sub-frame budgets). Evaluation
  is EVERY FRAME (no sampling cadence — density reads are
  the cost being paid). The finding is WARN-ONLY: it renders
  as an ordinary `violations[]` entry on a status `"Pass"`
  report (there is no informational section), with
  `_meta.unpromoted_rule: true` marking the posture, the
  detail sentence carrying the warn-only disclosure, and an
  `occupancy_episode` block (`sampled_entities`, `radius`,
  `cap`, roster of distinct WITNESSING entities — index AND
  generation, first/worst frames, worst self-inclusive count,
  violating frames — worst-first, capped at 32 with
  `roster_truncated` disclosed). NOTHING about the rule can
  flip run status: `strict_warnings` does not touch it, and
  the rule is excluded from every gating path by
  construction. Vacuity is split: a selector that never
  offered a checkable entity over the whole run emits a
  never-a-crowd NOTE under `occupancy_cap`'s own
  (informational) name — NEVER `vacuous_invariant`, which
  gates — while a within-cap population is simply silent.
  `check`/`value`/`eventually_s`/`path`/`resource`/
  `min_distance`/`region`/`tolerance`/`watch_path`/
  `sampling_s`/`spawn_only` and the bounds fields are
  rejected at load; `radius`/`cap`/`window_s` on other rules
  are rejected too (foreign fields, both directions).

### What a `with_component` selector requires of the target type

Every `with_component` selector (and `allow_if` exemptions,
`min_distance_from` anchors) resolves the SAME way: full
registered type path, or an unambiguous final path segment
(with a did-you-mean suggestion for close typos; an ambiguous
short name instead rejects listing the candidates). Resolution succeeds
once the type carries `#[derive(Reflect)]` and has been passed
to `App::register_type` — but what the RULE then does with
the resolved type differs, and that difference decides the
second prerequisite:

| Rule | Membership decided by | `#[reflect(Component)]` needed? |
|---|---|---|
| `pairwise_separation` | archetype (`ComponentId` contains-check) | **No** — registration alone |
| `forbidden_region` (selector and `allow_if`) | archetype / `TypeId` | **No** — registration alone |
| `derived_transform_consistency` | archetype (child pairs, parents) | **No** — registration alone |
| `nodes_in_bounds` | archetype | **No** — registration alone |
| `spawn_safety` (member and anchor) | archetype | **No** — registration alone |
| `occupancy_cap` | archetype | **In practice, yes** — membership is archetype-decided, but the walker additionally rejects a registration carrying no ReflectComponent data (supplied by `#[reflect(Component)]` — or automatically for `#[reflect(Resource)]` types, which Bevy 0.20 registers the data for), so a plain component type is effectively required to carry the attribute |
| `component_check` | archetype | **Yes — required**: the rule READS component values through reflected-data access; a registered type without ReflectComponent data is an `oracle_error` naming `#[reflect(Component)]` |
| `entity_stagnation` | archetype | **Yes — required**: the digest reads reflected field values the same way |
| `resource_check` / `stagnant_resource` | n/a (`resource` field) | n/a — resources need `#[derive(Reflect)]` + `#[reflect(Resource)]`, which auto-registers the ReflectComponent data the read goes through (Bevy 0.20 reads reflected resources via the component registration); the `#[reflect(Component)]` attribute itself is not the resource-side requirement |

Consequences worth knowing before the first run:

- A marker component (e.g. `#[derive(Component, Reflect)]` with
  `#[reflect(Component)]`) satisfies EVERY row: derive +
  attribute + `App::register_type` is the universally safe
  form. The registration-without-attribute corner exists only
  for types derived and registered without the component role.
- Naming the wrong KIND of type behaves differently by the
  type's role, and neither path is what you might expect:
  - Plain reflected data (no ECS role attribute) carries no
    ReflectComponent data and no entity ever bears it: the
    marker rules come out vacuous, and the value-reading
    rules reject with the not-a-reflected-component config
    error naming the attribute to add.
  - A `#[reflect(Resource)]` type is a genuine trap: Bevy 0.20
    auto-registers ReflectComponent data for it (reflected
    resource reads go through the component registration),
    and resources are entity-backed — exactly one entity
    bears the type, visible to every archetype walk. The
    spatial marker rules still end vacuous, but only because
    the resource entity carries no `Transform` (no frame ever
    offers a checkable entity). The value-reading rules are
    worse: they READ and judge the resource's fields through
    the auto-supplied data, so a `component_check` or
    `entity_stagnation` pointing a `with_component` selector
    at a resource silently judges it instead of erroring. To
    check a resource, use the `resource` field
    (`resource_check` / `stagnant_resource`), never a
    `with_component` selector.
- Selector resolution runs at first evaluation (frame 0 of a
  run; lint mode resolves once up front and rejects unknown
  names there) and again at every frame — a
  type registered late (after `App::register_type` was called
  mid-run) is picked up on the frames after its registration.

### Scripted-input coordinates and authoring notes

The cursor the bots and scripts move is the harness-owned
`CursorInputResource` position, expressed in **window-space
pixels with the origin at the top-left, +y down** — the winit
convention, not bevy's world-space +y up. Two different
coordinate stories share this space, and conflating them is the
classic first-run mistake:

- `mouse_clicks`/`mouse_motion` coordinates are used **verbatim**
  as window-space positions and deltas — no scaling against
  the window, no headless fallback extent. Authored (x, y) lands
  at exactly that window position (off-window values go where
  they say and picking will ignore them, exactly as a real
  click past the screen edge would). They are **window pixels,
  never world units**: the harness does not own your camera, so
  it cannot convert pixels to your world plane — pin where a
  widget actually renders on screen (capturing a screenshot of
  the running game is the reliable way) before scripting a click at it.
- The CHAOS bot's random cursor draws are quantized and scaled
  into the real window's logical size when one exists, else a
  theoretical 1280×1280 viewport — so chaos clicks stay
  plausible-on-screen headless or not.

Two timing facts worth encoding in the first scenario draft:

- **Frame-0 parking**: the cursor sits at the window origin
  `(0, 0)` — the top-left corner, not a centered position —
  until the first click or motion script applies, and
  `mouse_motion` deltas accumulate from that origin when no
  click teleported first. Any UI depending on hover state sees
  "no hover" at frame 0; script an explicit first-frame click
  or motion if a centered or moved starting point matters (a
  game camera that pans toward the cursor at the window corner
  from frame 0 will drift diagonally — observed in practice,
  and worth a stabilizing first-frame input when authoring).
- **Held-click interplay**: scripted `hold_frames` is a minimum
  of 1, and scripted holds refcount against the chaos bot's on
  the same button — `Released` fires only when the last holder
  lets go, so a scripted press overlapping a chaos hold
  releases at the later of the two expiry frames.
