# Why the self-dev-dependency pattern

The tutorial's Cargo.toml dance (optional dependency + a dev-dependency on
your own crate that activates the `playtest` feature) buys two properties
at once: plain `cargo test` runs the full battery with zero extra flags,
while a plain `cargo build` — and every downstream consumer's build —
resolves **zero bevy_playtest code** (release purity, enforced by CI's
`cargo tree` guard). The known wrinkle: `cargo package` strips
dev-dependencies, so an unpacked archive loses the auto-activation; the
generated test surfaces this as an ignored test with the fix instead of
silently running nothing.
