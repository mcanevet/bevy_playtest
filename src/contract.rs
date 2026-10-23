//! Game-side hooks for the harness.
//!
//! Games register typed reset/cheat hooks here. Oracles and invariants
//! are zero-annotation: they apply to every entity with a `Transform`,
//! with no game-side markers required (name-based targeting of specific
//! entities is roadmap, not yet implemented).

use bevy_ecs::prelude::*;
use std::collections::HashMap;

/// Hook closure type shared by ResetHooks and CheatHooks.
pub type WorldHook = Box<dyn Fn(&mut World) + Send + Sync>;

/// Typed resets — no string method dispatch.
#[derive(Resource, Default)]
pub struct ResetHooks(pub HashMap<String, WorldHook>);

impl ResetHooks {
    pub fn register<F: Fn(&mut World) + Send + Sync + 'static>(&mut self, name: &str, f: F) {
        self.0.insert(name.to_string(), Box::new(f));
    }

    pub fn known_kinds(&self) -> Vec<&str> {
        self.0.keys().map(|s| s.as_str()).collect()
    }
}

/// Typed cheats (scenario-declared manipulations).
#[derive(Resource, Default)]
pub struct CheatHooks(pub HashMap<String, WorldHook>);

impl CheatHooks {
    pub fn register<F: Fn(&mut World) + Send + Sync + 'static>(&mut self, name: &str, f: F) {
        self.0.insert(name.to_string(), Box::new(f));
    }

    pub fn known_kinds(&self) -> Vec<&str> {
        self.0.keys().map(|s| s.as_str()).collect()
    }
}
