//! Input-surface discovery: automatically determine which inputs a
//! game actually consumes, without requiring authors to declare them.
//!
//! Shipped so far: the TYPE layer ([`surface`], [`diff`],
//! [`group_completion`]) plus [`static_scan`] — the first PRODUCER
//! of surface annotations: a schedule walk that classifies systems
//! by the input-signal ComponentIds their access sets touch (never
//! by system names) and emits reader counts per device class, with a
//! fail-open completeness contract (zero counts only license
//! pruning from a scan that saw the whole world). Later slices add
//! the probe machinery and the orchestration flow that consume
//! these layers.

pub mod diff;
pub mod group_completion;
pub mod static_scan;
pub mod surface;
