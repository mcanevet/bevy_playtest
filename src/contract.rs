//! Game-side hooks and markers for the harness.
//!
//! Games register typed reset/cheat hooks here; the `Gameplay` marker
//! scopes oracle queries.

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

/// Entities that can leave the play area — bounds/NaN invariants check
/// these; structural/UI entities (legitimately at origin) are excluded.
///
/// `Reflect` so external-BRP mode can query it (`world.query`).
#[derive(Component, bevy_reflect::Reflect)]
#[reflect(Component)]
pub struct Gameplay;
