//! BRP robustness test battery: exercise `run_scenario_brp` against
//! mock game binaries (compiled on demand from the mock source below)
//! covering hostile/adversarial process behavior:
//!
//! - stdout/stderr flood (does the run deadlock or survive?) — H2
//! - crash-at-startup (clear error, no hang, no leaked process) — H4-adjacent
//! - never-binding game (startup timeout honored, bounded runtime)
//! - exit mid-run (runner must surface the death, never a silent Pass)
//! - NaN-poisoned entity (finite_transforms oracle fires over the wire)
//!
//! Mocks are compiled with rustc directly (std only, no deps) into
//! the target dir, so no fixture infrastructure is needed. Each test
//! picks its own free port (argv + cfg.port) so parallel runs never
//! collide.
#![cfg(feature = "brp")]

use bevy_playtest::brp_runner::{run_scenario_brp, BrpRunConfig};
use bevy_playtest::scenario::Scenario;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

const MOCK_SRC: &str = r#"
// Minimal mock BRP game. Usage: <binary> <mode> <port>
// Modes: flood | instant-exit | never-bind | brp-ok | brp-crash-mid
use std::io::{Read, Write};
use std::net::TcpListener;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).cloned().unwrap_or("brp-ok".into());
    let port: u16 = args.get(2).cloned().and_then(|p| p.parse().ok()).expect("port arg");

    match mode.as_str() {
        "flood" => {
            // Answer BRP correctly, but ALSO spew on both pipes fast.
            let listener = TcpListener::bind(("127.0.0.1", port)).expect("bind");
            std::thread::spawn(|| loop {
                println!("{}", "x".repeat(1000));
                eprintln!("{}", "y".repeat(1000));
                std::thread::sleep(std::time::Duration::from_millis(1));
            });
            serve_brp(listener, "");
        }
        "instant-exit" => {
            std::process::exit(3);
        }
        "never-bind" => {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        }
        "brp-ok" | "brp-crash-mid" => {
            let listener = TcpListener::bind(("127.0.0.1", port)).expect("bind");
            serve_brp(listener, &mode);
        }
        _ => unreachable!(),
    }
}

fn serve_brp(listener: TcpListener, mode: &str) {
    let mut served = 0usize;
    for stream in listener.incoming() {
        let mut stream = stream.expect("stream");
        let mut buf = [0u8; 65536];
        // Naive: one request per connection per read; the BRP client
        // opens one connection per call, so this suffices.
        let n = match stream.read(&mut buf) {
            Ok(0) | Err(_) => continue,
            Ok(n) => n,
        };
        let req = String::from_utf8_lossy(&buf[..n]).into_owned();
        let is_query = req.contains("\"world.query\"");
        let is_discover = req.contains("\"rpc.discover\"");
        let body = if is_discover {
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}".to_string()
        } else if is_query {
            // Two entities: one sane, one poisoned. `null` deserializes
            // as a missing f32 -> the harness must treat it as
            // non-finite/missing and flag it, not silently skip.
            "[{\"entity\":1,\"components\":{\"bevy_transform::components::transform::Transform\":{\"translation\":[0.0,0.0,0.0]}}},{\"entity\":2,\"components\":{\"bevy_transform::components::transform::Transform\":{\"translation\":[0.0,null,0.0]}}}]".to_string()
        } else {
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":null}".to_string()
        };
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = stream.write_all(resp.as_bytes());
        let _ = stream.flush();
        served += 1;
        if mode == "brp-crash-mid" && served > 20 {
            std::process::exit(9);
        }
    }
}
"#;

fn compile_mock(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join(format!("{name}.rs"));
    std::fs::write(&src, MOCK_SRC).unwrap();
    let bin = dir.join(name);
    // Lock file: tests run in parallel within one binary, and rustc
    // linking to the same output path races (errno 17 File exists).
    // flock serializes the compile-once, reuse-after logic.
    let lock = dir.join(format!("{name}.lock"));
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock)
        .unwrap();
    use std::os::unix::io::AsRawFd;
    let ret = unsafe { libc_flock(lock_file.as_raw_fd()) };
    assert_eq!(ret, 0, "flock on mock lock failed");
    if bin.exists()
        && bin.metadata().unwrap().modified().unwrap()
            >= src.metadata().unwrap().modified().unwrap()
    {
        return bin;
    }
    let out = Command::new(std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into()))
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("rustc runs");
    assert!(
        out.status.success(),
        "mock compile failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    bin
}

/// Minimal flock binding without a libc dependency (LOCK_EX = 2).
unsafe fn libc_flock(fd: i32) -> i32 {
    unsafe { syscall_flock(fd, 2) }
}
#[cfg(unix)]
unsafe fn syscall_flock(fd: i32, op: i32) -> i32 {
    unsafe extern "C" {
        fn flock(fd: i32, operation: i32) -> i32;
    }
    flock(fd, op)
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn quick_scenario() -> Scenario {
    serde_json::from_str(r#"{"bot":{"type":"chaos","seed":7},"duration_s":0.5,"tps":60}"#).unwrap()
}

fn cfg(binary: &std::path::Path, mode: &str, port: u16, startup_timeout: Duration) -> BrpRunConfig {
    BrpRunConfig {
        game_binary: binary.to_string_lossy().into_owned(),
        game_args: vec![mode.into(), port.to_string()],
        port: Some(port),
        startup_timeout,
    }
}

/// H2: a game flooding stdout+stderr while correctly answering BRP
/// must not deadlock the run (pipes are drained only after kill
/// today). The run must terminate within a generous but bounded wall
/// time. NOTE: stdin is NOT piped by the runner, but the OS pipe
/// buffer is finite (~64KB) — an undrained flood fills it and the
/// game blocks on write, which is what this test detects.
#[test]
fn stdout_flood_does_not_deadlock_the_run() {
    let bin = compile_mock("brp_mock_game");
    let port = free_port();
    let start = Instant::now();
    let report = run_scenario_brp(
        &cfg(&bin, "flood", port, Duration::from_secs(10)),
        &quick_scenario(),
    )
    .expect("run completes despite flood");
    let elapsed = start.elapsed();
    // Scenario is 0.5s; allow generous overhead for pipe contention,
    // but a real deadlock hangs until the test timeout.
    assert!(
        elapsed < Duration::from_secs(20),
        "run took {elapsed:?} — pipe-induced deadlock suspected"
    );
    // Flood stderr must not be mistaken for a game error that flips
    // status... but the runner stores stderr tail as `error` on
    // non-empty; status itself is driven by violations only.
    let _ = report;
}

/// A game exiting instantly must fail fast with a diagnosable error,
/// never hang until startup_timeout.
#[test]
fn crashed_at_startup_game_fails_fast_with_detail() {
    let bin = compile_mock("brp_mock_game");
    let port = free_port();
    let start = Instant::now();
    let err = run_scenario_brp(
        &cfg(&bin, "instant-exit", port, Duration::from_secs(30)),
        &quick_scenario(),
    )
    .expect_err("game that exits instantly must fail the run");
    let msg = err.to_string();
    assert!(msg.contains("exited"), "error should mention exit: {msg}");
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "must fail fast, not burn the startup timeout"
    );
}

/// A game that never opens its BRP port must time out within the
/// configured startup_timeout, not hang forever.
#[test]
fn never_binding_game_times_out_bounded() {
    let bin = compile_mock("brp_mock_game");
    let port = free_port();
    let start = Instant::now();
    let err = run_scenario_brp(
        &cfg(&bin, "never-bind", port, Duration::from_secs(3)),
        &quick_scenario(),
    )
    .expect_err("never-binding game must fail the run");
    assert!(err.to_string().contains("BRP"), "{}", err.to_string());
    let elapsed = start.elapsed();
    assert!(
        elapsed >= Duration::from_secs(3) && elapsed < Duration::from_secs(10),
        "timeout honored loosely: {elapsed:?}"
    );
}

/// A game that dies MID-RUN (after serving BRP fine) must surface as
/// failure (violations from failed queries), never a silent Pass.
#[test]
fn game_death_mid_run_is_not_a_silent_pass() {
    let bin = compile_mock("brp_mock_game");
    let port = free_port();
    let report = run_scenario_brp(
        &cfg(&bin, "brp-crash-mid", port, Duration::from_secs(10)),
        &quick_scenario(),
    )
    .expect("run structurally completes");
    assert_ne!(
        report.status,
        bevy_playtest::enums::PlaytestStatus::Pass,
        "game died mid-run but report passed; violations={:?}",
        report.violations
    );
}

/// Oracle over the wire: a NaN/null-poisoned translation must trip
/// finite_transforms (or an oracle error), not be silently skipped —
/// the query-response parsing path must reject what it can't read.
#[test]
fn poisoned_translation_is_flagged_over_the_wire() {
    let bin = compile_mock("brp_mock_game");
    let port = free_port();
    let report = run_scenario_brp(
        &cfg(&bin, "brp-ok", port, Duration::from_secs(10)),
        &quick_scenario(),
    )
    .expect("run completes");
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.rule.contains("finite") || v.rule.contains("oracle")),
        "mock serves a poisoned translation; expected finite/oracle violation, got {:?}",
        report.violations
    );
    assert_ne!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
}
