//! Error/log/panic sinks — route failures from the run into Violations.
//!
//! A thread-local sink buffer keyed by the current run; the driver
//! drains it into the report after the loop.
//!
//! - Panics: std panic hook chained to the previous hook; captured with
//!   location + current frame.
//! - Logs: a tracing Subscriber layer capturing ERROR/WARN events.

use crate::rules::Rule;
use std::cell::RefCell;

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

thread_local! {
    /// INFO-level messages seen while capture is active: observation
    /// fuel for declared coverage. Kept separate from the violation
    /// sink — INFO never gates a run.
    static OBSERVED_INFO: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

thread_local! {
    static SINK: RefCell<Vec<Captured>> = const { RefCell::new(Vec::new()) };
    static CAPTURE_ACTIVE: RefCell<bool> = const { RefCell::new(false) };
    static CURRENT_FRAME: RefCell<u64> = const { RefCell::new(0) };
}

/// Activate/deactivate log capture on this thread. When inactive, the
/// tracing layer drops events (ordinary game logging outside a run).
pub fn set_capture_active(active: bool) {
    CAPTURE_ACTIVE.with(|a| *a.borrow_mut() = active);
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
        // Console passthrough filtered to INFO+ (bevy's LogPlugin default
        // surface); the capture layer sees everything but keeps only
        // WARN/ERROR.
        let fmt = tracing_subscriber::fmt::layer()
            .with_filter(tracing_subscriber::filter::LevelFilter::INFO);
        let subscriber = tracing_subscriber::registry().with(fmt);
        let subscriber = subscriber.with(CaptureLayer);
        if tracing::subscriber::set_global_default(subscriber).is_err() {
            eprintln!(
                "bevy_playtest: log capture OFF — another tracing subscriber is already \
                 installed (game-provided LogPlugin?). Errors logged via tracing will \
                 not appear in the playtest report."
            );
        }
    });
}

/// Tracing layer capturing WARN/ERROR events into the thread-local sink
/// (only while a run is active on this thread).
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
        if !CAPTURE_ACTIVE.with(|a| *a.borrow()) {
            return;
        }
        let level = *event.metadata().level();
        // Coverage observation needs INFO messages too (declared
        // observations often surface as info logs). They never become
        // violations — only observation fuel.
        if level == tracing::Level::INFO {
            let mut visitor = MessageVisitor(String::new());
            event.record(&mut visitor);
            if !visitor.0.is_empty() {
                OBSERVED_INFO.with(|s| s.borrow_mut().push(visitor.0));
            }
            return;
        }
        if !matches!(level, tracing::Level::WARN | tracing::Level::ERROR) {
            return;
        }
        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);
        let message = visitor.0;
        if message.is_empty() {
            return;
        }
        let target = event.metadata().target().to_string();
        let frame = current_frame();
        SINK.with(|s| {
            s.borrow_mut().push(Captured::Log {
                level: if level == tracing::Level::ERROR {
                    "ERROR"
                } else {
                    "WARN"
                },
                target,
                message,
                frame,
            })
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
            let frame = current_frame();
            SINK.with(|s| {
                s.borrow_mut().push(Captured::Panic {
                    message,
                    location,
                    frame,
                })
            });
            prev(info);
        }));
    }
}

/// Expose the current frame to the panic hook (called by the driver each tick).
pub fn set_current_frame(frame: u64) {
    CURRENT_FRAME.with(|f| *f.borrow_mut() = frame);
}

pub fn current_frame() -> u64 {
    CURRENT_FRAME.with(|f| *f.borrow())
}

/// Drain captured INFO messages (observation fuel). Called by the
/// driver alongside `drain`.
pub fn drain_observed_info() -> Vec<String> {
    OBSERVED_INFO.with(|s| std::mem::take(&mut *s.borrow_mut()))
}

/// Drain all captured events (consumes them).
pub fn drain() -> Vec<Captured> {
    SINK.with(|s| std::mem::take(&mut *s.borrow_mut()))
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
