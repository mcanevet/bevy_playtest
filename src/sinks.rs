//! Error/log/panic sinks — route failures from the run into Violations.
//!
//! A process-global, exclusively-held "active run" buffer; the driver
//! acquires it for the run's duration and drains it into the report
//! after the loop.
//!
//! - Panics: std panic hook chained to the previous hook; captured with
//!   location + current frame. Panics outside an active run (before
//!   acquisition or after release) are DROPPED — there is no honest
//!   owner to attribute them to. A setup-phase panic unwinds OUT of
//!   `run_scenario` to the caller (setup is not catch_unwind-guarded),
//!   so it is never silently lost — the caller observes it directly.
//! - Logs: a tracing Subscriber layer capturing ERROR/WARN events.
//! - Storage is process-global (NOT thread-local) so events emitted on
//!   worker threads (IoTaskPool asset tasks, compute systems) reach the
//!   run's report. Attribution safety comes from exclusivity: exactly
//!   one `run_scenario` may be active per process — concurrent or
//!   nested calls are REJECTED immediately (never blocked: a blocked
//!   nested call on a worker thread would deadlock the outer run, and
//!   a blocked concurrent caller would misattribute its setup-phase
//!   events into the in-flight run's buffer). Parallel callers must
//!   serialize themselves (test batteries do so with a local mutex).

use crate::rules::Rule;
use std::sync::Mutex;

/// A captured failure event.
#[derive(Debug, Clone)]
pub enum Captured {
    Panic {
        message: String,
        location: Option<String>,
        frame: u64,
    },
    Log {
        level: &'static str,
        target: String,
        message: String,
        frame: u64,
    },
}

/// The exclusively-held active run: all captured events + observation
/// fuel + the current frame stamp. Existence of this state IS the
/// capture-active signal — events observed while no run is held are
/// dropped (bounded memory, no cross-run contamination possible).
#[derive(Default)]
struct ActiveRun {
    events: Vec<Captured>,
    observed_info: Vec<String>,
    frame: u64,
}

/// Process-global sink state. `None` = no run active (events dropped).
static ACTIVE_RUN: Mutex<Option<ActiveRun>> = Mutex::new(None);

/// Run exclusivity: held for a run's ENTIRE duration (acquisition
/// through drain). Acquisition is NON-BLOCKING (`try_lock`): a second
/// concurrent `run_scenario` is rejected immediately. This is what
/// makes the design sound:
///
/// - No attribution leak: the second caller is rejected BEFORE its
///   setup phase emits any events, so nothing of run B can land in
///   run A's buffer.
/// - No deadlock: a nested `run_scenario` called from a worker thread
///   of the running app (e.g. a multithreaded-schedule system) is
///   rejected instead of blocking on the held mutex while the outer
///   run waits for that very worker.
/// - Parallel CALLERS must serialize themselves (test batteries do
///   so with a local mutex); the library refuses to guess instead.
static RUN_SERIAL: Mutex<()> = Mutex::new(());

/// Acquire the active run for a new `run_scenario`. Concurrent or
/// nested calls are REJECTED (never blocked — see [`RUN_SERIAL`]).
/// Must be the FIRST thing `run_scenario` does: everything afterwards
/// (setup included) is then attributable to this run.
pub fn try_acquire_run() -> Result<RunGuard, String> {
    // Non-blocking: rejection over serialization. Poison recovery is
    // REQUIRED here, not optional: a setup-phase panic unwinds through
    // RunGuard, which drops the MutexGuard mid-panic and poisons
    // RUN_SERIAL. Treating poison like "busy" would reject every later
    // run in the process FOREVER — the state is provably free once
    // recovered (Drop cleared ACTIVE_RUN before the guard dropped).
    let serial = match RUN_SERIAL.try_lock() {
        Ok(guard) => guard,
        Err(std::sync::TryLockError::Poisoned(poison)) => poison.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => {
            return Err(
                "a playtest run is already active in this process (concurrent \
                 or nested run_scenario calls are not supported)"
                    .to_string(),
            )
        }
    };
    let mut guard = lock_active_run();
    // No in-flight run can exist while we hold RUN_SERIAL: every run
    // holds it for its whole duration and clears ACTIVE_RUN before
    // releasing. Stale events here would mean stragglers from a prior
    // run arrived before this acquisition — impossible by ordering;
    // drop them defensively anyway.
    *guard = Some(ActiveRun::default());
    drop(guard);
    Ok(RunGuard(Some(serial)))
}

/// RAII release: dropping the guard ends the run. Events observed
/// afterwards are DROPPED, not reattributed: with no run active there
/// is nowhere honest to put them. Known limitation: a worker-thread
/// straggler from a COMPLETED run that fires after the NEXT run
/// acquired cannot be told apart from that run's own event — workers
/// carry no run identity. Immediate back-to-back runs absorb a window
/// of stragglers; document, don't pretend.
pub struct RunGuard(Option<std::sync::MutexGuard<'static, ()>>);

impl Drop for RunGuard {
    fn drop(&mut self) {
        *lock_active_run() = None;
        self.0.take(); // release RUN_SERIAL (state cleared first)
    }
}

/// Poison-tolerant lock of the global run state. A panic inside a
/// captured run must not brick every later run in the process.
fn lock_active_run() -> std::sync::MutexGuard<'static, Option<ActiveRun>> {
    ACTIVE_RUN
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

/// Record one captured event (if a run is active). Called from any
/// thread.
fn push_event(event: Captured) {
    if let Some(run) = lock_active_run().as_mut() {
        run.events.push(event);
    }
}

/// Set the current frame stamp (called by the driver each tick).
pub fn set_current_frame(frame: u64) {
    if let Some(run) = lock_active_run().as_mut() {
        run.frame = frame;
    }
}

/// Current frame stamp (read by the panic hook; any thread).
pub fn current_frame() -> u64 {
    lock_active_run().as_ref().map_or(0, |run| run.frame)
}

/// Drain captured events + observation fuel of the (active) run,
/// and clear them. Called by the driver alongside draining the
/// run's report data.
pub fn drain() -> Vec<Captured> {
    let mut guard = lock_active_run();
    let Some(run) = guard.as_mut() else {
        return Vec::new();
    };
    std::mem::take(&mut run.events)
}

/// Drain captured INFO messages (observation fuel). Called by the
/// driver alongside `drain`.
pub fn drain_observed_info() -> Vec<String> {
    let mut guard = lock_active_run();
    let Some(run) = guard.as_mut() else {
        return Vec::new();
    };
    std::mem::take(&mut run.observed_info)
}

/// Process-global, write-once record of whether the capture subscriber
/// actually became the global default. See [`ensure_global_subscriber`].
static SUBSCRIBER_WON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Whether the harness's capture subscriber won the (write-once) global
/// default installation. `false` means log-based findings
/// (log_error/log_warn) are under-reported for EVERY run in this
/// process — the report surfaces this as `log_capture_lost`.
pub fn subscriber_won() -> bool {
    SUBSCRIBER_WON.load(std::sync::atomic::Ordering::Acquire)
}

/// Escape hatch acknowledging log-capture loss: when the game's own
/// tracing subscriber (typically DefaultPlugins' LogPlugin) wins the
/// write-once global default, log_error/log_warn findings are
/// under-reported. Setting this silences the loud warning and the
/// report's informational `log_capture_lost` entry. The install-time
/// WARNING is decided once (the first subscriber install); report
/// entries consult the env var per run. Meaningless in BRP mode (no
/// log capture there by design).
///
/// Lives here (not plugin.rs) because sinks is compiled under every
/// feature combination.
pub const ENV_ALLOW_MISSING_LOG_CAPTURE: &str = "BEVY_PLAYTEST_ALLOW_MISSING_LOG_CAPTURE";

/// Whether capture loss is acknowledged (escape hatch env var).
fn loss_acknowledged() -> bool {
    std::env::var_os(ENV_ALLOW_MISSING_LOG_CAPTURE).is_some()
}

/// Ensure a global tracing subscriber is installed exactly once per
/// process, with the capture layer first in the stack. If another
/// subscriber was already set, this is a no-op (emit a loud warning —
/// log capture silently off under-reports errors).
///
/// The subscriber also chains a fmt layer so console output is preserved
/// when it wins the race against the game's LogPlugin (LogPlugin then
/// logs a harmless "already set" error). Call this BEFORE the game's
/// plugins are added for capture to engage.
pub fn ensure_global_subscriber() {
    static INSTALLED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    INSTALLED.get_or_init(|| {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::Layer;
        // Console passthrough honoring RUST_LOG (default INFO, so an
        // unset RUST_LOG does not demote output to ERROR-only). This
        // replaces LogPlugin's console output on the capture path.
        // The filter wraps the FMT layer only — the capture layer
        // below must stay unfiltered so RUST_LOG can never blind the
        // oracle.
        let fmt = tracing_subscriber::fmt::layer().with_filter(
            tracing_subscriber::filter::EnvFilter::builder()
                .with_default_directive(tracing_subscriber::filter::LevelFilter::INFO.into())
                .from_env_lossy(),
        );
        let subscriber = tracing_subscriber::registry().with(fmt);
        let subscriber = subscriber.with(CaptureLayer);
        let won = tracing::subscriber::set_global_default(subscriber).is_ok();
        SUBSCRIBER_WON.store(won, std::sync::atomic::Ordering::Release);
        if !won && !loss_acknowledged() {
            eprintln!(
                "bevy_playtest: log capture OFF — another tracing subscriber is already \
                 installed (game-provided LogPlugin?). Errors logged via tracing will \
                 not appear in the playtest report for this process. Use \
                 PlaytestDefaultPlugins (drops LogPlugin under a scenario run) or set \
                 {}=1 to acknowledge.",
                ENV_ALLOW_MISSING_LOG_CAPTURE
            );
        }
    });
}

/// Tracing layer capturing WARN/ERROR events (plus INFO as observation
/// fuel) into the process-global sink — only while a run is active
/// (anywhere in the process; events from worker threads included).
pub struct CaptureLayer;

impl<S> tracing_subscriber::Layer<S> for CaptureLayer
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let level = *event.metadata().level();
        // Cheap level check BEFORE touching the shared lock: TRACE/
        // DEBUG never reach storage, and capture-active (a run being
        // held) is re-checked under the lock itself.
        if !matches!(
            level,
            tracing::Level::INFO | tracing::Level::WARN | tracing::Level::ERROR
        ) {
            return;
        }
        // Coverage observation needs INFO messages too (declared
        // observations often surface as info logs). They never become
        // violations — only observation fuel.
        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);
        if visitor.0.is_empty() {
            return;
        }
        let frame = current_frame();
        if level == tracing::Level::INFO {
            if let Some(run) = lock_active_run().as_mut() {
                run.observed_info.push(visitor.0);
            }
            return;
        }
        let target = event.metadata().target().to_string();
        let message = visitor.0;
        push_event(Captured::Log {
            level: if level == tracing::Level::ERROR {
                "ERROR"
            } else {
                "WARN"
            },
            target,
            message,
            frame,
        });
    }
}

/// Records the `message` field of a tracing event.
struct MessageVisitor(String);

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{:?}", value);
        }
    }
}

/// Install the panic hook (once per process). Chains to the previous
/// hook after capturing. Safe under concurrent first-install.
pub fn install_panic_hook() {
    static INSTALLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if INSTALLED
        .compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
        )
        .is_ok()
    {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let message = info
                .payload()
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| info.payload().downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "opaque panic payload".to_string());
            let location = info
                .location()
                .map(|l| format!("{}:{}", l.file(), l.line()));
            // Capture the panic into the active run's buffer — WITHOUT
            // ever blocking on a lock: a panic can fire while this
            // thread already holds ACTIVE_RUN or RUN_SERIAL (e.g. an
            // oracle panicking mid-capture), and a blocking lock there
            // is a self-deadlock. If the run buffer cannot be taken
            // (contention or no active run), the panic is DROPPED —
            // there is no honest owner to attribute it to. (The run
            // loop unwinds into catch_unwind which reports the crash
            // regardless, so a main-loop panic is never lost.)
            if let Ok(mut guard) = ACTIVE_RUN.try_lock() {
                if let Some(run) = guard.as_mut() {
                    run.events.push(Captured::Panic {
                        message,
                        location,
                        frame: run.frame,
                    });
                }
            }
            prev(info);
        }));
    }
}

/// Convert captured events into violation entries, grouped by
/// (rule, target, normalized message). Numbers in messages collapse to
/// '#' so "entity 5 failed" and "entity 6 failed" group together.
/// `strict_warnings` is reserved for callers that gate log_warn;
/// gating is decided downstream via `Rule::is_informational`.
pub fn to_violation_entries(
    captured: Vec<Captured>,
    #[allow(unused_variables)] strict_warnings: bool,
) -> Vec<crate::state::ViolationEntry> {
    use std::collections::BTreeMap;
    // (rule, target, normalized message) -> (raw message, first, last, count)
    type Groups = BTreeMap<(String, String, String), (String, u64, u64, u64)>;
    let mut groups: Groups = BTreeMap::new();
    for event in captured {
        let (rule, target, message, frame) = match event {
            Captured::Panic {
                message,
                location,
                frame,
            } => {
                let target = "world".to_string();
                let detail = match location {
                    Some(loc) => format!("{message} at {loc}"),
                    None => message.clone(),
                };
                (Rule::Panic.name().to_string(), target, detail, frame)
            }
            Captured::Log {
                level,
                target,
                message,
                frame,
            } => {
                // WARN is captured under log_warn; gating is decided by
                // Rule::is_informational + the driver's strict_warnings.
                let rule = if level == "ERROR" {
                    Rule::LogError.name().to_string()
                } else {
                    Rule::LogWarn.name().to_string()
                };
                (rule, target, message, frame)
            }
        };
        let key = (rule, target.clone(), normalize_sink_message(&message));
        let entry = groups.entry(key).or_insert((message, frame, frame, 0));
        entry.1 = entry.1.min(frame);
        entry.2 = entry.2.max(frame);
        entry.3 += 1;
    }
    groups
        .into_iter()
        .map(
            |((rule, target, _), (message, first, last, count))| crate::state::ViolationEntry {
                rule,
                target,
                first_frame: first,
                last_frame: last,
                count,
                detail: format!("{message} (x{count})"),
                last_detail: message,
            },
        )
        .collect()
}

/// Collapse consecutive digits to a single '#' for dedup grouping.
fn normalize_sink_message(msg: &str) -> String {
    let mut out = String::with_capacity(msg.len());
    let mut prev_digit = false;
    for c in msg.chars() {
        if c.is_ascii_digit() {
            if !prev_digit {
                out.push('#');
            }
            prev_digit = true;
        } else {
            out.push(c);
            prev_digit = false;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_collapse() {
        assert_eq!(normalize_sink_message("entity 5 failed"), "entity # failed");
        assert_eq!(
            normalize_sink_message("entity 5 failed"),
            normalize_sink_message("entity 50 failed")
        );
    }

    #[test]
    fn groups_panic_events_by_signature() {
        let entries = to_violation_entries(
            vec![
                Captured::Panic {
                    message: "entity 5 died".into(),
                    location: Some("game.rs:12".into()),
                    frame: 10,
                },
                Captured::Panic {
                    message: "entity 8 died".into(),
                    location: Some("game.rs:12".into()),
                    frame: 20,
                },
            ],
            false,
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].count, 2);
        assert_eq!(entries[0].first_frame, 10);
        assert_eq!(entries[0].last_frame, 20);
        assert!(entries[0].rule == "panic");
    }

    #[test]
    fn log_levels_map_to_rules() {
        let entries = to_violation_entries(
            vec![Captured::Log {
                level: "ERROR",
                target: "my_game::physics".into(),
                message: "bad state".into(),
                frame: 5,
            }],
            false,
        );
        assert_eq!(entries[0].rule, "log_error");
        assert_eq!(entries[0].target, "my_game::physics");
    }
}
