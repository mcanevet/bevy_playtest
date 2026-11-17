# Why liveness is time-boxed

A world that stops moving mid-game is usually a soft-lock, but a paused
menu or a loading screen looks identical. The liveness oracle therefore
waits a grace window (default 2 s) before declaring the world frozen.
The timeout is configurable via `liveness.timeout_s` in the scenario.
Disabling liveness entirely is not yet implemented. It watches
`Transform` changes on any entity (plus `UiTransform` changes under the
`ui-liveness` feature, on by default with takeover), so it needs no
game-side annotations.

## "Frozen" is not always "deadlocked"

The detail string does not always say "soft-lock?". When the scenario
records the broadened state digest (`record_digest: true`) and the
digest kept changing through the silent streak — a reflected game
resource mutated, or entities spawned/despawned — the sim was
demonstrably running while nothing visible moved. The detail then says
"stepping but visually silent" and tells you the remedy (presenter
layers must write `Transform`/UI every frame to be observable), because
a "soft-lock?" there sends you hunting for a deadlock that does not
exist. The machine-readable verdict rides in `_meta.stepping`
(`true`/`false`); without a recorded digest the detail keeps the
legacy "soft-lock?" wording and `_meta` carries no `stepping` value
(the oracle attaches no `_meta` at all — though a discovery-annotated
report may still show `"stepping": null`, the key present with a null
value, when the battery's annotation added its own `_meta` block) —
the oracle cannot know, and says so honestly.

A flat digest does NOT rewrite the detail: the digest sees Transforms,
UI transforms, spawns/despawns and reflected resources, but is blind to
e.g. sprite-color or material mutations, so "digest flat too" only
corroborates (`_meta.stepping: false`) — it cannot prove a deadlock.

