//! Type-safe enums replacing stringly-typed fields.
//!
//! All enums use `#[serde(rename_all = "...")]` so JSON serialization
//! is stable across releases.

use serde::{Deserialize, Serialize};

/// Bot types (chaos, synthetic_keyboard, synthetic_pointer).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotType {
    Chaos,
    SyntheticKeyboard,
    SyntheticPointer,
}

impl std::fmt::Display for BotType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Chaos => "chaos",
            Self::SyntheticKeyboard => "synthetic_keyboard",
            Self::SyntheticPointer => "synthetic_pointer",
        };
        f.write_str(s)
    }
}

/// Invariant rule types (nodes_in_bounds only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvariantRule {
    NodesInBounds,
}

impl std::fmt::Display for InvariantRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("nodes_in_bounds")
    }
}

/// Comparison operators with ONE semantics everywhere.
/// Wire format: `lt`, `le`, `gt`, `ge`, `equals`, `ne` (plus the legacy
/// aliases `below` = `le`, `above` = `ge`, `eq` = `equals`; all inclusive
/// boundaries).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckOp {
    Lt,
    #[serde(alias = "below")]
    Le,
    Gt,
    #[serde(alias = "above")]
    Ge,
    #[serde(alias = "eq")]
    Equals,
    Ne,
}

/// Relative+absolute tolerance so integer-valued counters compare
/// exactly and large floats don't need bit-equality (`overall<0 iff
/// fail` must hold at exact boundaries).
fn approx_eq(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
}

impl CheckOp {
    /// Single source of truth for numeric comparison.
    pub fn holds(self, cur: f64, thr: f64) -> bool {
        match self {
            Self::Lt => cur < thr,
            Self::Le => cur <= thr,
            Self::Gt => cur > thr,
            Self::Ge => cur >= thr,
            Self::Equals => approx_eq(cur, thr),
            Self::Ne => !approx_eq(cur, thr),
        }
    }
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
            Self::Equals => "==",
            Self::Ne => "!=",
        }
    }
}

impl std::fmt::Display for CheckOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.symbol())
    }
}

/// Playtest report status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaytestStatus {
    Pass,
    Fail,
    Crash,
}

impl std::fmt::Display for PlaytestStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Crash => "crash",
        };
        f.write_str(s)
    }
}

impl From<PlaytestStatus> for String {
    fn from(s: PlaytestStatus) -> Self {
        s.to_string()
    }
}

/// Mouse button for synthetic_pointer clicks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PointerButton {
    #[default]
    Primary,
    Secondary,
    Middle,
}

impl std::fmt::Display for PointerButton {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Primary => "primary",
            Self::Secondary => "secondary",
            Self::Middle => "middle",
        };
        f.write_str(s)
    }
}

/// Target selector for invariants: which entities the rule applies to.
/// Kept as an enum for wire-format forward compatibility.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetSelector {
    /// Apply to all entities with a Transform component (zero-annotation
    /// path: no bevy_playtest-specific code in the game needed).
    #[default]
    All,
}
