//! Input-surface discovery: automatically determine which inputs a
//! game actually consumes, without requiring authors to declare them.
//!
//! The four-slice implementation is COMPLETE, each layer feeding the
//! next: the TYPE layer ([`surface`], [`diff`],
//! [`group_completion`]) plus [`static_scan`] (the schedule walk
//! that classifies systems by input-signal ComponentIds, never
//! names); [`probe_scenarios`] (normalized control/probe pair
//! generation, deterministic validated probe points) and [`cache`]
//! (the on-disk surface cache); the takeover-gated [`probe_pool`]
//! (subprocess worker pool on the plugin env contract); and
//! [`flow`] — the orchestrator that ties them together
//! (cache lookup → probe pool → determinism gate → diff aggregation
//! → group completion → cache persist). See
//! `docs/explanation/input-surface-discovery.md` for the design and
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
