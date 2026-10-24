# Why two driving modes

- **In-process** gives the bot full programmatic control: reset hooks,
  cheat hooks, direct ECS access. Cost: the game takes the harness as a
  (test-build-only) dependency and the fixture shares the crate graph —
  the binary is re-run under the harness's runner for a takeover run.
- **External-BRP** trades power for zero coupling: the game runs as a
  stock subprocess driven over Bevy's Remote Protocol. Useful for
  binaries you can't rebuild or third-party games; no harness imports.
  BRP is Bevy's own protocol, so this mode reaches any Bevy version that
  serves it — including versions older than the one the harness
  compiles against.
