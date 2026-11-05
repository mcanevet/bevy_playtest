//! Input-surface discovery: automatically determine which inputs a
//! game actually consumes, without requiring authors to declare them.
//!
//! Shipped so far: the TYPE layer ([`surface`], [`diff`],
//! [`group_completion`]) plus [`static_scan`] — the first PRODUCER
//! of surface annotations: a schedule walk that classifies systems
//! by the input-signal ComponentIds their access sets touch (never
//! by system names) and emits reader counts per device class, with a
//! fail-open completeness contract (zero counts only license
//! pruning from a scan that saw the whole world). The PROBE
//! machinery is now live too: [`probe_scenarios`] (normalized
//! control/probe pair generation, deterministic validated probe
//! points), the takeover-gated [`probe_pool`] (subprocess worker
//! pool on the plugin env contract), and [`cache`] (the on-disk
//! surface cache). Only the orchestration FLOW remains.
//!
//! Feature split: `probe_pool` drives TAKEOVER SUBPROCESSES (it
//! builds on `crate::plugin`'s env contract, which only exists
//! with the `takeover` feature) and is gated behind it, exactly
//! like its crate consumer, `crate::battery`. The pure layers —
//! `cache`, `diff`, `group_completion`, `probe_scenarios`,
//! `static_scan`, `surface` — compile everywhere so report types
//! like `DiscoveredSurface`/`ViolationMeta` stay available to
//! in-process-only builds.

pub mod cache;
pub mod diff;
pub mod group_completion;
#[cfg(feature = "takeover")]
pub mod probe_pool;
pub mod probe_scenarios;
pub mod static_scan;
pub mod surface;
