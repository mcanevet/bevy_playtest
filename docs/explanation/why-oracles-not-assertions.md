# Why oracles, not assertions

A unit test knows where to look; a playtester doesn't. Instead of asserting
specific outcomes, oracles watch global health signals — NaN transforms,
entities escaping the play field, a world that stops moving — during normal
bot-driven play. This catches bugs nobody wrote an assertion for, and it
keeps the game free of per-test scaffolding: beyond the two cfg'd
helper pairs from the integration, no test-specific code ships in the
game at all.
