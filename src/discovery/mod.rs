//! Input-surface discovery: automatically determine which inputs a
//! game actually consumes, without requiring authors to declare them.
//!
//! This module currently ships the TYPE layer only: [`surface`] (the
//! `DiscoveredSurface` report shape and its key/device annotations),
//! [`diff`] (state-digest differentials between runs), and
//! [`group_completion`] (WASD/arrows sibling inference). Later slices
//! add the static scan, the probe machinery, and the orchestration
//! flow that PRODUCE these types — the shapes land first so later
//! slices stack on a stable wire format.

pub mod diff;
pub mod group_completion;
pub mod surface;
