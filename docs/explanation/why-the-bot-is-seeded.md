# Why the bot is seeded

Chaos without reproducibility is just flakiness. Every bot run carries a
seed; the same seed replays the identical key sequence. A bug report that
includes the seed is a reproducer anyone can run.

Every random decision the bot makes is a recorded draw in the report's
`choices` — which key, how long it is held, where the cursor went, which
button was clicked — each stamped with its frame and draw bound. The
chaos keyboard presses up to two keys concurrently with per-press drawn
hold durations (2–30 frames), and every one of those draws flows through
the same seeded stream: same seed, same chords, same holds, byte for
byte. (Draw sequences are stable per harness version; a version that
adds or reorders draws produces a different `choices` stream by design.)
