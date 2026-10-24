# Reset or cheat between runs

Games register typed hooks as resources. A reset runs before the scenario;
all cheats fire before the run loop starts:

```rust
use bevy_playtest::contract::{CheatHooks, ResetHooks};

app.world_mut().insert_resource(ResetHooks::default());
app.world_mut()
    .resource_mut::<ResetHooks>()
    .register("my_reset", |world| {
        // restore level state...
    });

app.world_mut().insert_resource(CheatHooks::default());
app.world_mut()
    .resource_mut::<CheatHooks>()
    .register("invincible", |world| {
        // apply cheat...
    });
```

Then reference them from the scenario:

```json
"setup": {
  "resets": [ { "kind": "my_reset" } ],
  "cheats": [ { "frame": 0, "kind": "invincible" } ]
}
```

All cheats are applied before the run loop starts (the `frame` field
is validated but not yet used for scheduling). Unknown kinds are
rejected at load — not silently ignored.
