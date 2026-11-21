# Scenario (JSON schema)

The full grammar for scenario files, the behavior of the two
scripted-input families, and every class of load-time rejection.

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
    "raw_surface": {              // optional, chaos only
      "keys": ["KeyW", "ArrowLeft"],
      "mouse_buttons": ["Left", "Right"]   // default: all three primary buttons
    },
    "explore": true               // optional, chaos only: exploring bot (see below)
  },
  "tps": number,                  // default 60; range 1..=1000
  "simulated_time": boolean,      // default true
  "single_threaded": boolean,      // default true
  "strict_warnings": boolean,     // default false
  "duration_s": number,           // default 15.0; max 3600
  "verify_replay": boolean,       // default false; needs single_threaded. In-process runs two passes in one process; under the takeover a fresh subprocess is pass B
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
    }
  ],
  "oracles": ["string"],          // rejected: oracle selection not implemented
  "liveness": {
    "mode": "real_time",          // "off" rejected: not implemented
    "timeout_s": number,          // default 2.0; must span at least one frame
    "progress": {                 // progress-heartbeat liveness oracle
      "mode": "auto",             // "auto" | "markers" | "off" (default auto)
      "markers": ["string"],      // markers mode: named reflected resources/components
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
  `eventually_s` (except on `component_check`, where it is supported),
  `oracles` selection,
  `liveness.mode: "off"`. Unknown rule values
  (e.g. `custom`) and unknown bot types (e.g. `pursuit`, `replay`) fail
  deserialization naming the offender.
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
