# Script keys instead of chaos

For deterministic replays, use `synthetic_keyboard` with a frame-indexed
key schedule:

```json
"bot": {
  "type": "synthetic_keyboard",
  "key_presses": [
    { "frame": 0, "key": "left", "hold_frames": 60 },
    { "frame": 120, "key": "space" }
  ]
}
```

Frames are simulation ticks (default 60 tps, so frame 120 ≈ 2 s). Keys
accept `KeyCode` spellings (`ArrowLeft`, `KeyW`) or shortcuts (`left`,
`w`); unknown names are rejected at load with a did-you-mean suggestion.
`hold_frames` defaults to 1.

To script mouse clicks instead (works with any bot type — clicks layer
orthogonally over chaos or synthetic keyboards):

```json
"bot": {
  "type": "chaos",
  "mouse_clicks": [
    { "frame": 60, "button": "left", "x": 640.0, "y": 360.0, "hold_frames": 3 }
  ]
}
```

`x`/`y` are window-space logical pixels (default 0.0), `button` is one
of `left`, `right`, `middle`, `back`, `forward` (default `left`), and
`hold_frames` defaults to 1 — set it to 2+ if the game reads
`ButtonInput` and polls on a frame boundary.

To keep chaos keys but disable chaos clicks (or vice versa), declare an
explicit surface:

```json
"raw_surface": { "keys": ["KeyW"], "mouse_buttons": [] }
```

An omitted field keeps its default (full WASD+arrows key set, three
primary mouse buttons); only an explicitly empty list opts that half
out. Disabling both halves is rejected at load (a chaos bot that can do
nothing is a vacuous run).
