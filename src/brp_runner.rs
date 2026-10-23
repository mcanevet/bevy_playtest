//! External-BRP runner: drives an out-of-process game over Bevy's
//! Remote Protocol — the zero-contract integration mode.
//!
//! Spawns a game binary exposing `RemotePlugin`, connects,
//! drive chaos-bot keyboard input via `world.write_message`, poll
//! transforms via `world.query`, and evaluate the same invariants
//! (bounds/NaN) as the in-process driver. Reports share the
//! `PlaytestReport` shape.
//!
//! The game must be launched headless (no WinitPlugin) with
//! `RemotePlugin::default()` (BRP on localhost:2567).

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::brp::client::BrpClient;
use crate::choice::ChoiceStream;
use crate::enums::{BotType, PlaytestStatus};
use crate::rules;
use crate::scenario::{validate_scenario, Scenario, ScenarioError};
use crate::state::{PlaytestReport, Violations};

/// Configuration for an external-BRP run.
#[derive(Debug, Clone)]
pub struct BrpRunConfig {
    /// Path to the game binary (must expose RemotePlugin).
    pub game_binary: String,
    /// Args passed to the game (e.g. ["--headless"]).
    pub game_args: Vec<String>,
    /// BRP port (game must listen here; picked free port if None).
    pub port: Option<u16>,
    /// How long to wait for the game to come up before failing.
    pub startup_timeout: Duration,
}

impl Default for BrpRunConfig {
    fn default() -> Self {
        Self {
            game_binary: String::new(),
            game_args: vec![],
            port: None,
            startup_timeout: Duration::from_secs(30),
        }
    }
}

/// Negotiate the BRP port: an explicitly configured port wins; otherwise
/// bind-then-drop a TCP listener to pick a free ephemeral port (avoids
/// collisions between parallel test runs).
fn negotiate_port(cfg: &BrpRunConfig) -> Result<u16, ScenarioError> {
    if let Some(port) = cfg.port {
        return Ok(port);
    }
    std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| ScenarioError::Rejected(format!("failed to negotiate a free BRP port: {e}")))?
        .local_addr()
        .map_err(|e| ScenarioError::Rejected(format!("failed to negotiate a free BRP port: {e}")))
        .map(|a| a.port())
}

/// Kills the child process on drop so a failed run never leaks the game
/// process (or its port) into subsequent runs.
struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Wait until the BRP endpoint answers `rpc.discover` (or the game exits).
async fn wait_for_brp(
    client: &BrpClient,
    proc: &mut Child,
    timeout: Duration,
) -> Result<(), String> {
    let start = Instant::now();
    loop {
        // Bail early if the process died. Include the game's stderr
        // tail so windowed-startup panics (e.g. missing display) are
        // diagnosable instead of a bare exit code.
        match proc.try_wait() {
            Ok(Some(status)) => {
                let stderr = proc.stderr.take().map(drain_pipe).unwrap_or_default();
                let tail = truncate_chars(&stderr, 2000);
                let detail = if tail.is_empty() {
                    String::new()
                } else {
                    format!("; stderr tail: {tail}")
                };
                return Err(format!("game exited during startup with {status}{detail}"));
            }
            Ok(None) => {}
            Err(e) => return Err(format!("failed to poll game process: {e}")),
        }
        if client.call("rpc.discover", None).await.is_ok() {
            return Ok(());
        }
        if start.elapsed() > timeout {
            return Err(format!(
                "game did not expose BRP within {}s",
                timeout.as_secs()
            ));
        }
        // Small sleep between polls (async-friendly; we run a dedicated
        // executor thread, so std sleep is fine here).
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Read all of a pipe into a String (for post-mortem on failure).
fn drain_pipe<R: Read>(mut r: R) -> String {
    let mut s = String::new();
    let _ = r.read_to_string(&mut s);
    s
}

/// Run one scenario against an external game over BRP.
///
/// Blocking convenience wrapper: spins a small async runtime internally.
pub fn run_scenario_brp(
    cfg: &BrpRunConfig,
    scenario: &Scenario,
) -> Result<PlaytestReport, ScenarioError> {
    validate_scenario(scenario)?;

    if scenario.bot.bot_type != BotType::Chaos {
        return Err(ScenarioError::NotImplemented(
            "external-BRP mode supports only the chaos bot".to_string(),
        ));
    }
    // Wall-clock pacing across processes makes honest replay verification
    // impossible in BRP mode — the report must never claim it.
    if scenario.verify_replay {
        return Err(ScenarioError::NotImplemented(
            "verify_replay is not supported in external-BRP mode: wall-clock \
             pacing between processes makes honest replay verification \
             impossible; use the in-process driver for determinism proof"
                .to_string(),
        ));
    }

    // Run the async core on a dedicated executor thread.
    let cfg_owned = cfg.clone();
    let scenario_owned = scenario.clone();
    let handle = std::thread::spawn(move || run_brp_async(cfg_owned, scenario_owned));
    handle
        .join()
        .map_err(|_| ScenarioError::Rejected("BRP runner thread panicked".to_string()))?
}

fn run_brp_async(cfg: BrpRunConfig, scenario: Scenario) -> Result<PlaytestReport, ScenarioError> {
    // smol-style block-on: we only use the client's async API.
    futures_lite::future::block_on(async { run_brp_inner(cfg.clone(), scenario.clone()).await })
}

async fn run_brp_inner(
    cfg: BrpRunConfig,
    scenario: Scenario,
) -> Result<PlaytestReport, ScenarioError> {
    let port = negotiate_port(&cfg)?;

    let child = Command::new(&cfg.game_binary)
        .args(&cfg.game_args)
        .env("BRP_PORT", port.to_string())
        // Headless game process: fixtures skip winit unless explicitly
        // overridden (e.g. local interactive debugging).
        .env("BEVY_PLAYTEST_HEADLESS", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            ScenarioError::Rejected(format!(
                "failed to launch game binary '{}': {e}",
                cfg.game_binary
            ))
        })?;

    let mut guard = ChildGuard(child);
    let client = BrpClient::localhost(port);
    wait_for_brp(&client, &mut guard.0, cfg.startup_timeout)
        .await
        .map_err(ScenarioError::Rejected)?;

    // Parity detection: the game-side PlaytestBrpPlugin serves
    // bevy_playtest/health; absence means today's degraded mode (no
    // panic/log visibility, no hooks, no time control). The report
    // must distinguish the two — never a silent downgrade.
    let brp_parity = match client.call("rpc.discover", None).await {
        Ok(Value::Object(methods)) if methods.keys().any(|k| k == "bevy_playtest/health") => "full",
        _ => "degraded",
    }
    .to_string();

    // Wire-level: pass the port to the game via env only; if the game
    // binds a fixed port, cfg.port must match. Document this contract.

    let tps = scenario.tps.max(1);
    let total_frames = (scenario.duration_s * tps as f32).ceil() as u64;
    let mut choices = ChoiceStream::from_seed(scenario.bot.seed);
    let mut violations = Violations::default();
    let mut recorded_choices = Vec::new();
    let mut input_count = 0u64;
    let mut remote_rows_seen: u64 = 0;
    // Frame index of the last observed world change (for liveness).
    let mut last_change_frame: u64 = 0;
    let mut last_positions: std::collections::HashMap<String, (f64, f64)> =
        std::collections::HashMap::new();
    let wall_start = Instant::now();

    let frame_dt = Duration::from_secs_f64(1.0 / tps as f64);
    // External-mode key hold: mirror the in-process chaos bot (2 frames)
    // so ButtonInput-based games observe the press.
    let hold_frames = 2u64;
    // (key name, frame at which to release)
    let mut held: Vec<(&'static str, u64)> = Vec::new();

    // Chaos surface + input rate mirror the in-process driver: same
    // default key list, interval derived from tps/input_rate_hz —
    // never hardcoded, so both modes stay behaviorally identical.
    let keys: Vec<&'static str> = {
        let declared = scenario.bot.raw_surface.as_ref().map(|s| s.keys.clone());
        match declared {
            Some(keys) if !keys.is_empty() => keys
                .iter()
                .filter_map(|k| {
                    // Surface strings are canonical names; CHAOS_KEYS is
                    // the name->payload table the wire path can send.
                    CHAOS_KEYS
                        .iter()
                        .find(|c| c.eq_ignore_ascii_case(k))
                        .copied()
                })
                .collect(),
            _ => CHAOS_KEYS.to_vec(),
        }
    };
    let rate = scenario.bot.input_rate_hz.max(1);
    let interval = (tps as u64 / rate as u64).max(1);

    for frame in 0..total_frames {
        // --- Release keys whose hold duration expired.
        held.retain(|(key, release_at)| {
            if frame >= *release_at {
                input_count += 1;
                // Fire-and-forget release; errors surface as violations.
                let fut = send_key(&client, key, "Released", frame, &mut violations);
                futures_lite::future::block_on(fut);
                false
            } else {
                true
            }
        });

        // --- Drive input: chaos bot decides a raw key, map to a BRP
        // keyboard message.
        if frame % interval == 0 && held.is_empty() {
            let bound = keys.len() as u64;
            let idx = choices.below(frame, bound);
            recorded_choices.push(crate::choice::Choice {
                frame,
                value: idx,
                bound,
            });
            let Some(&key) = keys.get(idx as usize) else {
                continue;
            };
            input_count += 1;
            send_key(&client, key, "Pressed", frame, &mut violations).await;
            held.push((key, frame + hold_frames));
        }

        // --- Poll transforms and evaluate bounds/NaN (same semantics
        // as in-process oracles). Query Transform only: matching the oracles'
        // zero-annotation semantics (every entity with a Transform).
        match client
            .call(
                "world.query",
                Some(json!({
                    "data": {
                        "components": [
                            "bevy_transform::components::transform::Transform"
                        ]
                    }
                })),
            )
            .await
        {
            Ok(result) => {
                // Vacuity guard: a successful query with zero rows
                // means we are not observing ANY entity — treat as an
                // oracle error rather than trivially passing. All
                // showcase fixtures spawn transform-bearing entities.
                let empty = result
                    .as_array()
                    .map(|rows| rows.is_empty())
                    .unwrap_or(true);
                if empty {
                    violations.report(
                        rules::Rule::OracleError.name(),
                        "brp_query",
                        "world.query returned zero entities: oracle is vacuous \
                         (fixture broken or query misconfigured)"
                            .to_string(),
                        frame,
                    );
                }
                let rows = count_rows(&result);
                if rows > 0 {
                    remote_rows_seen = remote_rows_seen.max(rows);
                }
                evaluate_remote_transforms(
                    frame,
                    &result,
                    &scenario,
                    &mut violations,
                    &mut last_positions,
                    &mut last_change_frame,
                );
            }
            Err(e) => {
                violations.report(
                    rules::Rule::OracleError.name(),
                    "brp_query",
                    format!("world.query failed: {e}"),
                    frame,
                );
            }
        }

        // Liveness parity with in-process mode: a world whose
        // transforms never change is a soft-lock. Report once when the
        // silence exceeds the scenario's grace window.
        if remote_rows_seen > 0 && frame > last_change_frame {
            let silent_frames = frame - last_change_frame;
            let grace = (scenario.liveness.timeout_s * tps as f32).max(1.0) as u64;
            if silent_frames == grace + 1 {
                violations.report(
                    rules::Rule::FrozenWorld.name(),
                    "world",
                    format!(
                        "transforms unchanged for {silent_frames} frames over BRP — soft-lock?"
                    ),
                    frame,
                );
            }
        }

        // Advance one frame: external mode has no direct tick control,
        // so we pace wall-clock. Simulated-time runs need a slower pace.
        std::thread::sleep(frame_dt);
    }

    let snap = violations.snapshot();
    let gating = snap
        .iter()
        .filter(|v| {
            !rules::Rule::from_name(&v.rule)
                .unwrap_or(rules::Rule::ScenarioError)
                .is_informational()
        })
        .count();
    let status = if gating == 0 {
        PlaytestStatus::Pass
    } else {
        PlaytestStatus::Fail
    };

    // Shut down and collect game stderr for diagnostics.
    let _ = guard.0.kill();
    let stderr = guard.0.stderr.take().map(drain_pipe).unwrap_or_default();

    let report = PlaytestReport {
        schema_version: 1,
        status,
        violations: violations.snapshot(),
        metrics: crate::state::Metrics {
            input_count,
            frame_count: total_frames,
            ..Default::default()
        },
        frame_count: total_frames,
        tps: tps as u64,
        simulated_duration_s: total_frames as f64 / tps as f64,
        wall_time_s: wall_start.elapsed().as_secs_f64(),
        // `error` means "why this run failed", not "the game chats on
        // stderr" — healthy Bevy games print asset/adapter noise. Attach
        // the stderr tail only when the run actually failed, as a
        // diagnostic.
        error: (status != PlaytestStatus::Pass).then(|| truncate_chars(&stderr, 2000)),
        action_log: vec![],
        cheat_count: 0,
        choices: recorded_choices,
        harness_mode: "external_brp".to_string(),
        scenario_source: None,
        reset_hooks_registered: None,
        brp_parity: Some(brp_parity),
        // Log capture does not exist in BRP mode by design (the game is
        // a subprocess whose logs go to pipes) — the field's concept
        // does not apply there, so it stays absent.
        log_capture_available: None,
        coverage: None,
    };
    Ok(report)
}

/// Send one synthetic KeyboardInput event to the game via BRP
/// `world.write_message`. Payload shape mirrors bevy_remote's
/// BrpWriteMessageParams { message, value } and bevy_input's KeyboardInput
/// serde (field `state`; window 1 = Entity::PLACEHOLDER bits). Errors are
/// propagated as violations so a dropped input never yields a silent
/// vacuous pass.
async fn send_key(
    client: &BrpClient,
    key: &str,
    state: &str,
    frame: u64,
    violations: &mut Violations,
) {
    if let Err(e) = client
        .call(
            "world.write_message",
            Some(json!({
                "message": "bevy_input::keyboard::KeyboardInput",
                "value": {
                    "key_code": key,
                    "logical_key": { "Unidentified": "Unidentified" },
                    "state": state,
                    "text": null,
                    "repeat": false,
                    "window": 1
                }
            })),
        )
        .await
    {
        violations.report(
            rules::Rule::OracleError.name(),
            "brp_write_message",
            format!("world.write_message failed: {e}"),
            frame,
        );
    }
}

/// Keys the chaos bot cycles through in external mode (mirrors the
/// in-process chaos bot's surface).
const CHAOS_KEYS: [&str; 8] = [
    "KeyW",
    "KeyA",
    "KeyS",
    "KeyD",
    "ArrowUp",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
];

/// Truncate a string to at most `max_chars` characters, never cutting a
/// multi-byte UTF-8 character in half.
fn truncate_chars(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

/// Evaluate bounds/NaN over a BRP query result. Mirrors in-process
/// oracles; entities are identified by their BRP entity id (stringified).
fn evaluate_remote_transforms(
    frame: u64,
    result: &Value,
    scenario: &Scenario,
    violations: &mut Violations,
    last_positions: &mut std::collections::HashMap<String, (f64, f64)>,
    last_change_frame: &mut u64,
) {
    // BRP `world.query` returns an array of
    // `{ "entity": <id>, "components": { "<type>": {...} } }`.
    let Some(rows) = result.as_array() else {
        return;
    };
    if rows.is_empty() {
        return;
    }
    for row in rows {
        let entity_id = row
            .get("entity")
            .map(|e| e.to_string())
            .unwrap_or_else(|| "?".to_string());
        let Some(components) = row.get("components") else {
            continue;
        };
        let Some(t) = components.get("bevy_transform::components::transform::Transform") else {
            continue;
        };
        // Transform serializes translation as a Vec3 array [x, y, z].
        // A missing/non-numeric component is DATA CORRUPTION, not a
        // skippable row: flag it instead of silently ignoring the entity.
        let coord = |i: usize| {
            t.get("translation")
                .and_then(|v| v.get(i))
                .and_then(|v| v.as_f64())
        };
        let Some(x) = coord(0) else {
            violations.report(
                rules::Rule::FiniteTransforms.name(),
                &entity_id,
                format!("entity {entity_id} transform x is not a number"),
                frame,
            );
            continue;
        };
        let Some(y) = coord(1) else {
            violations.report(
                rules::Rule::FiniteTransforms.name(),
                &entity_id,
                format!("entity {entity_id} transform y is not a number"),
                frame,
            );
            continue;
        };
        let Some(z) = coord(2) else {
            violations.report(
                rules::Rule::FiniteTransforms.name(),
                &entity_id,
                format!("entity {entity_id} transform z is not a number"),
                frame,
            );
            continue;
        };

        // Movement tracking for the liveness oracle.
        if last_positions.get(&entity_id) != Some(&(x, y)) {
            last_positions.insert(entity_id.clone(), (x, y));
            *last_change_frame = frame;
        }

        // NaN/non-finite check (mirrors finite_transforms oracle).
        if !x.is_finite() || !y.is_finite() || !z.is_finite() {
            violations.report(
                rules::Rule::FiniteTransforms.name(),
                &entity_id,
                format!("entity {entity_id} has non-finite transform"),
                frame,
            );
            continue;
        }

        // Bounds (mirrors nodes_in_bounds invariant evaluation,
        // all three axes — matching the standalone in-process oracle).
        for inv in &scenario.invariants {
            if inv.rule != crate::enums::InvariantRule::NodesInBounds {
                continue;
            }
            let min_x = inv.min_x.unwrap_or(-1000.0) as f64;
            let max_x = inv.max_x.unwrap_or(1000.0) as f64;
            let min_y = inv.min_y.unwrap_or(-1000.0) as f64;
            let max_y = inv.max_y.unwrap_or(1000.0) as f64;
            let min_z = inv.min_z.unwrap_or(-1000.0) as f64;
            let max_z = inv.max_z.unwrap_or(1000.0) as f64;
            if x < min_x || x > max_x || y < min_y || y > max_y || z < min_z || z > max_z {
                violations.report(
                    rules::Rule::NodesInBounds.name(),
                    &inv.name,
                    format!(
                        "entity {entity_id} out of bounds [{min_x},{max_x}]×[{min_y},{max_y}]×[{min_z},{max_z}]: ({x:.2},{y:.2},{z:.2})",
                    ),
                    frame,
                );
            }
        }
    }
}

fn count_rows(result: &Value) -> u64 {
    result.as_array().map(|r| r.len() as u64).unwrap_or(0)
}
