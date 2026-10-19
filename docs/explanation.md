# Explanation

Design rationale — the "why" behind the harness.

## Why oracles, not assertions

A unit test knows where to look; a playtester doesn't. Instead of asserting
specific outcomes, oracles watch global health signals — NaN transforms,
entities escaping the play field, a world that stops moving — during normal
bot-driven play. This catches bugs nobody wrote an assertion for, and it
keeps the game's code completely free of test scaffolding.

## Why liveness is time-boxed

A world that stops moving mid-game is usually a soft-lock, but a paused
menu or a loading screen looks identical. The liveness oracle therefore
waits a grace window (default 2 s) before declaring the world frozen, and
the scenario can widen or disable it. It watches `Transform` changes on
any entity, so it fires even in games that never adopt the `Gameplay`
marker.

## Why never a vacuous pass

The worst failure mode of a test harness is a silent "green" that verified
nothing. So the harness rejects anything it can't actually check: unsupported
invariant kinds, unknown hook names, malformed scenarios all fail at load
with explicit errors. A passing report means the oracles genuinely ran.

## Why two driving modes

- **In-process** gives the bot full programmatic control: reset hooks,
  cheat hooks, direct ECS access. Cost: your game must expose an
  `App` constructor and the harness shares the crate graph.
- **External-BRP** trades power for zero coupling: the game runs as a
  stock subprocess driven over Bevy's Remote Protocol. Useful for
  unmodifiable games or other engines' Bevy ports; no harness imports.

## Why the bot is seeded

Chaos without reproducibility is just flakiness. Every bot run carries a
seed; the same seed replays the identical key sequence. A bug report that
includes the seed is a reproducer anyone can run.

## Why `Gameplay` tags instead of tracking everything

Tracking every entity makes bounds checks noisy (UI, cameras, effects).
A single marker component lets the game author declare what "gameplay"
means — the harness stays generic, the game stays in control of
semantics. The NaN and liveness oracles still watch every entity with
a `Transform`, so untagged games aren't given a vacuous pass.

## Why disable winit

Winit owns an OS event loop, which conflicts with test runners driving
ticks programmatically. Disabling only `WinitPlugin` keeps rendering,
assets, and audio alive — so systems touching those still run and can
still break loudly if buggy.
