//! Grandchild-holding-pipe child for the battery's bounded-drain
//! regression, TIMEOUT path (tests/battery.rs:
//! bounded_drain_survives_a_held_pipe): prints a startup line,
//! spawns a GRANDCHILD that inherits its stderr (the battery's tee
//! pipe) and parks, then parks FOREVER itself. The watchdog kills
//! the direct child, but the parked grandchild keeps the stderr
//! pipe's WRITE END open, so the tee thread never observes EOF.
//! The battery's stderr drain must be BOUNDED and detach on grace
//! expiry instead of joining forever (see `StderrTee::drain` in
//! src/battery.rs) — before that bounding, this shape turned the
//! watchdog's time-bounded kill into a permanent whole-job hang.
//! A game that parks a launcher shim / asset compiler / crash
//! handler behind it is exactly this child.
//!
//! The grandchild re-invokes THIS binary with `--park` and sleeps a
//! bounded interval (not forever), so no stray process meaningfully
//! outlives the test run. It deliberately does NOT exit early when
//! its parent dies — the test needs the pipe held THROUGH the drain
//! grace window.
//!
//! The NORMAL-path sibling (direct child exits promptly while the
//! grandchild holds the pipe) is
//! `takeover_demo_stderr_grandchild_exit`.
//!
//! Deliberately NOT an App run (same rationale as the other demo
//! children): the test observes the parent's PIPE/bookkeeping
//! behavior, not game logic.
//!
//! No `#[cfg(not(feature = "takeover"))]` fallback arm: the
//! `[[bin]]` entry carries `required-features = ["takeover"]`, so
//! this binary is only ever BUILT with the feature on — an arm for
//! the off case would be unreachable dead code (and would silently
//! mask a future removal of `required-features`).
fn main() {
    if std::env::args().any(|a| a == "--park") {
        // Grandchild: hold the inherited stderr pipe open by doing
        // nothing with it for a bounded interval, then exit. The
        // interval must DWARF the test's elapsed-time bound (10s)
        // and the drain grace (3s): if it were shorter, the pipe
        // could close mid-run and the regression would pass by
        // racing the grandchild's death instead of pinning the
        // bounded drain. Bounded (not infinite) so a stray
        // grandchild cannot meaningfully outlive the test run.
        std::thread::sleep(std::time::Duration::from_secs(60));
        return;
    }
    eprintln!("grandchild-pipe child: handing stderr to a parked grandchild");
    let exe = std::env::current_exe().expect("current exe");
    // LEAKED DELIBERATELY (the whole point): the grandchild must
    // outlive its parent to keep the tee pipe open through the
    // battery's drain grace window. Never `.wait()`ed — it parks a
    // bounded interval (see `--park`) and exits on its own.
    #[allow(clippy::zombie_processes)]
    let _parked = std::process::Command::new(exe)
        .arg("--park")
        // Inherit stderr = hold THE BATTERY'S tee pipe open; stdout
        // and stdin are irrelevant here.
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .expect("spawn parked grandchild");
    // Park until the watchdog kills us — the parked grandchild
    // keeps the pipe, which is the whole point.
    loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}
