# Why liveness is time-boxed

A world that stops moving mid-game is usually a soft-lock, but a paused
menu or a loading screen looks identical. The liveness oracle therefore
waits a grace window (default 2 s) before declaring the world frozen.
The timeout is configurable via `liveness.timeout_s` in the scenario.
Disabling liveness entirely is not yet implemented. It watches
`Transform` changes on any entity (plus `UiTransform` changes under the
`ui-liveness` feature, on by default with takeover), so it needs no
game-side annotations.
