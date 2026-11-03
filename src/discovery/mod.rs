//! Input-surface discovery: automatically determine which inputs a
//! game actually consumes, without requiring authors to declare them.
//!
//! See `docs/explanation/input-surface-discovery.md` for the design.
//! Phases land incrementally; the static device-reader scan (Phase 1)
//! and the orchestrating flow (Phase 7) are both live — see
//! [`flow::discover`] for the one-call entry point.
//!
//! Feature split: `flow` and `probe_pool` drive TAKEOVER SUBPROCESSES
//! (they build on `crate::plugin`'s env contract, which only exists
//! with the `takeover` feature) and are gated behind it, exactly like
//! their only crate consumer, `crate::battery`. The pure layers —
//! `cache`, `diff`, `group_completion`, `probe_scenarios`,
//! `static_scan`, `surface` — compile everywhere so report types
//! like `DiscoveredSurface`/`ViolationMeta` stay available to
//! in-process-only builds.

pub mod cache;
pub mod diff;
#[cfg(feature = "takeover")]
pub mod flow;
pub mod group_completion;
#[cfg(feature = "takeover")]
pub mod probe_pool;
pub mod probe_scenarios;
pub mod static_scan;
pub mod surface;
