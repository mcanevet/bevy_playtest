//! Shared test helpers: serialize `run_scenario` calls within a test
//! binary.
//!
//! `run_scenario` holds process-global exclusive capture state and
//! REJECTS concurrent calls (misattribution/deadlock otherwise; see
//! `bevy_playtest::sinks`). Cargo runs `#[test]`s on parallel threads
//! within one binary, so any battery that performs multiple runs must
//! serialize them itself.

use std::sync::{Mutex, MutexGuard, OnceLock};

/// Block until no other test in this binary holds the run slot.
pub fn run_slot() -> MutexGuard<'static, ()> {
    static SLOT: OnceLock<Mutex<()>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}
