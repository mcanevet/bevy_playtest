# Explanation

Design rationale — the "why" behind the harness. One topic per page.

- [Why oracles, not assertions](why-oracles-not-assertions.md) — Watching global health signals instead of writing per-case assertions
- [Why liveness is time-boxed](why-liveness-is-time-boxed.md) — Grace windows distinguish soft-locks from loading screens and menus
- [Why never a vacuous pass](why-never-a-vacuous-pass.md) — Anything the harness can't check fails at load, loudly
- [Why the self-dev-dependency pattern](why-the-self-dev-dependency-pattern.md) — Test builds get the harness; release builds resolve zero of it
- [Why the bot is seeded](why-the-bot-is-seeded.md) — Chaos without reproducibility is just flakiness
- [Why disable winit](why-disable-winit.md) — Keeping rendering, assets, and audio alive while headless
- [How the harness proves itself](how-the-harness-proves-itself.md) — The test tiers and CI gates behind the harness's own guarantees
