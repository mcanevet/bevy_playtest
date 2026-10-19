//! TestApi contract — typed game-state resolution for invariants.
//!
//! The harness stays type-blind: games register a resolver for
//! `TestApi.<field>` paths, and invariants (bounds, custom, eventually)
//! resolve values through it. Absent resolver = scenarios referencing
//! `TestApi.*` paths fail at load time (loud, never vacuous).

use bevy_app::App;
use bevy_ecs::prelude::*;
use bevy_ecs::world::World;
use std::collections::HashMap;

/// A resolved TestApi field value — numeric or textual.
#[derive(Debug, Clone, PartialEq)]
pub enum TestFieldValue {
    Numeric(f64),
    Text(String),
}

impl TestFieldValue {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            TestFieldValue::Numeric(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            TestFieldValue::Text(s) => Some(s.as_str()),
            _ => None,
        }
    }
}

/// Trait a game's TestApi resource implements so the harness can resolve
/// `TestApi.<field>` paths without knowing the concrete type.
pub trait TestApiResolve {
    fn resolve(&self, path: &str) -> Option<TestFieldValue>;
}

/// Type-erased resolution function.
pub type ResolveFn = fn(&World, &str) -> Option<TestFieldValue>;

/// The optional type-erased resolver resource.
#[derive(Resource, Clone, Copy)]
pub struct TestApiResolver(pub ResolveFn);

/// Extension for registering a custom TestApi resolver on an App.
pub trait RegisterTestApi {
    fn register_test_api<A: Resource + TestApiResolve>(&mut self) -> &mut Self;
}

impl RegisterTestApi for App {
    fn register_test_api<A: Resource + TestApiResolve>(&mut self) -> &mut Self {
        self.insert_resource(TestApiResolver(|world: &World, path: &str| {
            world.get_resource::<A>()?.resolve(path)
        }));
        self
    }
}

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

/// Genre markers.
#[derive(Component)]
pub struct RealTime;
#[derive(Component)]
pub struct TurnBased;
#[derive(Component)]
pub struct AnalogControls;

/// Entities that can leave the play area — bounds/NaN invariants check
/// these; structural/UI entities (legitimately at origin) are excluded.
///
/// `Reflect` so external-BRP mode can query it (`world.query`).
#[derive(Component, bevy_reflect::Reflect)]
#[reflect(Component)]
pub struct Gameplay;
