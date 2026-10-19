//! Type-safe enums replacing stringly-typed fields.
//!
//! All enums use `#[serde(rename_all = "...")]` so JSON serialization
//! is identical to the PoC wire format (bevy_playtest_poc) for the
//! fields that carried over.

use serde::{Deserialize, Serialize};

/// Bot types. v0.1 implements `chaos`, `replay`, and `synthetic_keyboard`;
/// the others are accepted on the wire but rejected by `validate_scenario`
/// with a "not yet implemented in this version" message so scenarios
/// never silently degrade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotType {
    Chaos,
    Pursuit,
    Replay,
    SyntheticPointer,
    SyntheticKeyboard,
    Planner,
    Custom,
}

impl BotType {
    /// Whether the in-process driver implements this bot type in v0.1.
    pub fn implemented(self) -> bool {
        matches!(self, Self::Chaos | Self::SyntheticKeyboard)
    }
}

impl std::fmt::Display for BotType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Chaos => "chaos",
            Self::Pursuit => "pursuit",
            Self::Replay => "replay",
            Self::SyntheticPointer => "synthetic_pointer",
            Self::SyntheticKeyboard => "synthetic_keyboard",
            Self::Planner => "planner",
            Self::Custom => "custom",
        };
        f.write_str(s)
    }
}

/// Chaos bot persona.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Persona {
    /// Default: uniform sampling over the declared surface.
    #[default]
    Uniform,
    /// 2x rate, never waits.
    Aggressive,
    /// Mostly waits, occasional jabs.
    Idle,
}

impl std::fmt::Display for Persona {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Uniform => "uniform",
            Self::Aggressive => "aggressive",
            Self::Idle => "idle",
        };
        f.write_str(s)
    }
}

/// Invariant rule types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvariantRule {
    NodesInBounds,
    Custom,
}

impl std::fmt::Display for InvariantRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::NodesInBounds => "nodes_in_bounds",
            Self::Custom => "custom",
        };
        f.write_str(s)
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
/// exactly and large floats don't need bit-equality. Matches the PoC
/// (le3.34 / RF44: `overall<0 iff fail` must hold at exact boundaries).
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
    /// Text comparison: only Equals/Ne are meaningful; others -> None
    /// (validate_scenario rejects them at load time).
    pub fn holds_text(self, cur: &str, expect: &str) -> Option<bool> {
        match self {
            Self::Equals => Some(cur == expect),
            Self::Ne => Some(cur != expect),
            _ => None,
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

/// Mouse buttons for raw input (wire-compatible with bevy's PointerButton).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
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
