//! Game-side BRP plugin: closes the parity gap between the in-process
//! driver and external-BRP mode by serving harness methods over Bevy's
//! Remote Protocol.
//!
//! Methods (namespace `bevy_playtest/`, discovered via `rpc.discover`):
//! - `bevy_playtest/health`: panic status + recent log excerpts
//! - `bevy_playtest/time`: time scale control (scale only — pause/step
//!   stay in-process-only; see README "time control asymmetry")
//!
//! The external runner (`brp_runner.rs`) detects these methods and
//! records full-vs-degraded parity in the report.

use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_ecs::world::World;
use bevy_remote::{error_codes, BrpError, BrpResult, RemoteMethodSystemId, RemoteMethods};
use serde_json::{json, Value};

/// Number of recent log excerpts kept for `bevy_playtest/health`.
const HEALTH_LOG_TAIL: usize = 32;

/// Recent log/error lines captured game-side for health reporting.
/// The game (or the plugin's companion log tap) pushes here; a plain
/// game that never logs simply reports an empty tail.
#[derive(Resource, Default)]
pub struct HealthLedger {
    /// True once a panic was observed by the game's own handler.
    pub panicked: bool,
    /// Most recent log lines (capped at HEALTH_LOG_TAIL).
    pub tail: Vec<String>,
}

impl HealthLedger {
    pub fn push_line(&mut self, line: String) {
        self.tail.push(line);
        if self.tail.len() > HEALTH_LOG_TAIL {
            self.tail.remove(0);
        }
    }
}

/// Plugin registering the `bevy_playtest/*` BRP methods.
///
/// Must be added AFTER `RemotePlugin` (the methods resource must
/// already exist — same ordering rule as bevy_debugger_bevy).
#[derive(Default)]
pub struct PlaytestBrpPlugin;

impl Plugin for PlaytestBrpPlugin {
    fn build(&self, app: &mut App) {
        if !app.world().contains_resource::<RemoteMethods>() {
            panic!("add RemotePlugin before PlaytestBrpPlugin");
        }
        app.init_resource::<HealthLedger>();
        register_method(app, "bevy_playtest/health", health_handler);
        register_method(app, "bevy_playtest/time", time_handler);
    }
}

fn brp_err(code: i16, message: String) -> BrpError {
    BrpError {
        code,
        message,
        data: None,
    }
}

fn register_method<M>(
    app: &mut App,
    name: &str,
    handler: impl IntoSystem<In<Option<Value>>, BrpResult, M> + 'static,
) {
    let world = app.world_mut();
    let handler_id = world.register_system(handler);
    world
        .resource_mut::<RemoteMethods>()
        .insert(name.to_string(), RemoteMethodSystemId::Instant(handler_id));
}

/// Health: panic status + recent log tail.
fn health_handler(In(_): In<Option<Value>>, world: &mut World) -> BrpResult {
    let ledger = world.resource::<HealthLedger>();
    Ok(json!({
        "panicked": ledger.panicked,
        "recent_logs": ledger.tail,
    }))
}

/// Time: scale control only. Pause/step are in-process-model concepts;
/// see the documented asymmetry.
fn time_handler(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let Some(params) = params else {
        let scale = world
            .resource::<bevy_time::prelude::Time<bevy_time::prelude::Virtual>>()
            .relative_speed();
        return Ok(json!({ "relative_speed": scale }));
    };
    let Value::Object(ref map) = params else {
        return Err(brp_err(
            error_codes::INVALID_PARAMS,
            "params must be an object".into(),
        ));
    };
    let Some(speed) = map.get("relative_speed").and_then(Value::as_f64) else {
        return Err(brp_err(
            error_codes::INVALID_PARAMS,
            "expected finite numeric 'relative_speed'".into(),
        ));
    };
    if !(0.0..=16.0).contains(&speed) {
        return Err(brp_err(
            error_codes::INVALID_PARAMS,
            "'relative_speed' out of range [0, 16]".into(),
        ));
    }
    world
        .resource_mut::<bevy_time::prelude::Time<bevy_time::prelude::Virtual>>()
        .set_relative_speed(speed as f32);
    Ok(json!({ "relative_speed": speed }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_registers_both_methods_into_remote_methods() {
        let mut app = bevy_app::App::new();
        app.add_plugins(bevy_remote::RemotePlugin::default());
        app.add_plugins(PlaytestBrpPlugin);
        let methods = app.world().resource::<RemoteMethods>();
        for m in ["bevy_playtest/health", "bevy_playtest/time"] {
            assert!(methods.get(m).is_some(), "{m} not registered");
        }
    }

    #[test]
    #[should_panic(expected = "add RemotePlugin before PlaytestBrpPlugin")]
    fn plugin_panics_when_remote_plugin_missing() {
        let mut app = bevy_app::App::new();
        app.add_plugins(PlaytestBrpPlugin);
    }

    #[test]
    fn health_ledger_caps_tail() {
        let mut ledger = HealthLedger::default();
        for i in 0..(HEALTH_LOG_TAIL + 10) {
            ledger.push_line(format!("line {i}"));
        }
        assert_eq!(ledger.tail.len(), HEALTH_LOG_TAIL);
        assert_eq!(ledger.tail.last().unwrap(), "line 41");
    }
}
