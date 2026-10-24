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
