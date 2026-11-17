//! Rule-name registry: all oracle rule names as an enum.
//!
//! Every entry must be emitted by an oracle that exists in this crate,
//! or be reserved for the scenario_error path. Kept as a single declarative
//! list (enum + ALL + name()) so registries cannot drift.

/// Single source of truth: declares the `Rule` enum, the `Rule::ALL`
/// registry slice, and the snake_case `name()` mapping from one list.
macro_rules! declare_rules {
    ($( $variant:ident => $name:literal ),* $(,)?) => {
        /// All oracle rule names as an enum.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Rule {
            $($variant),*
        }

        impl Rule {
            /// All registered rules (derived from the same list as the
            /// enum itself — cannot drift).
            pub const ALL: &[Rule] = &[$(Rule::$variant),*];

            /// The string name used in reports (snake_case).
            pub fn name(self) -> &'static str {
                match self {
                    $(Rule::$variant => $name),*
                }
            }

            /// Look up a rule by its report name (inverse of `name`).
            pub fn from_name(name: &str) -> Option<Rule> {
                $(if name == $name { return Some(Rule::$variant); })*
                None
            }
        }

        impl std::fmt::Display for Rule {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.name())
            }
        }
    };
}

impl Rule {
    /// One-line meaning (what invariant of a healthy game this rule
    /// protects).
    pub fn meaning(self) -> &'static str {
        match self {
            Self::FrozenWorld => "Entities must keep changing: a world that stops updating while the game is running is a soft-lock.",
            Self::Panic => "A system panicked during the run (caught by the panic hook).",
            Self::LogError => "Game code logged at error level.",
            Self::LogWarn => "Game code logged at warn level; informational by default, escalate with scenario strict_warnings.",
            Self::LogCaptureLost => "Log capture was OFF for this process (another tracing subscriber, typically DefaultPlugins' LogPlugin, installed first): log_error/log_warn findings are under-reported for every run in this process.",
            Self::LogBridgeLost => "The log→tracing bridge could not be installed (another `log` crate logger already set): events emitted through `log` — bevy's B000x hierarchy validators, wgpu/naga, any log-crate user — are invisible to log_error/log_warn for every run in this process.",
            Self::LogLevelTampered => "The `log` crate's max level was lowered mid-run while the harness bridge owned the logger (a foreign `set_max_level`): log_error/log_warn findings are under-reported for the affected ticks. A one-time lowering is repaired within one tick by the harness's re-assert; a persistent one means game code re-blinds every tick.",
            Self::NodesInBounds => "Entity positions stay inside declared play-space bounds.",
            Self::ComponentCheck => "An authored-intent check: a numeric field on a reflected component must satisfy the declared comparison across the selected entities (always, or eventually within a deadline).",
            Self::FiniteTransforms => "Every entity's Transform holds finite values (no NaN/inf).",
            Self::VacuousRun => "Nothing was ever observable: no entities to check and no inputs emitted — the run proves nothing about the game.",
            Self::VacuousInvariant => "An invariant matched zero entities; the check proves nothing until there is something to check.",
            Self::ReplayDivergence => "Same scenario + seed run twice must produce identical per-frame fingerprints; divergence means hidden nondeterminism (wall clock, unseeded RNG, unordered iteration).",
            Self::StalledProgress => "A churning world is not a progressing one: the frame loop keeps ticking with no inputs buffered and no observable game-state surface change over a whole window — the world is spinning without doing anything.",
            Self::ProgressSurfaceEmpty => "No game state was verifiable at all during progress monitoring (no reflected game types and no progress markers/resources in scope); progress cannot be assessed. Informational by default — escalate with strict_warnings or progress.require_surface.",
            Self::OracleError => "An oracle itself errored while evaluating (infrastructure failure, not a game bug).",
            Self::ScenarioError => "The scenario itself failed (rejected config) rather than the game misbehaving.",
        }
    }

    /// Whether this rule is informational — reported but
    /// never gating the run status. Derived from the enum so the
    /// registry cannot drift.
    pub fn is_informational(self) -> bool {
        matches!(
            self,
            Self::LogWarn
                | Self::LogCaptureLost
                | Self::LogBridgeLost
                | Self::LogLevelTampered
                | Self::ProgressSurfaceEmpty
        )
    }
}

declare_rules! {
    // Core oracles (producible, tested)
    FrozenWorld => "frozen_world",
    Panic => "panic",
    LogError => "log_error",
    LogWarn => "log_warn",
    LogCaptureLost => "log_capture_lost",
    LogBridgeLost => "log_bridge_lost",
    LogLevelTampered => "log_level_tampered",
    NodesInBounds => "nodes_in_bounds",
    FiniteTransforms => "finite_transforms",
    ComponentCheck => "component_check",

    // Vacuity guard: a run where nothing was ever observable.
    VacuousRun => "vacuous_run",

    // Vacuity guard: a single invariant matched zero entities.
    VacuousInvariant => "vacuous_invariant",

    // Verification depth.
    ReplayDivergence => "replay_divergence",

    // Progress-liveness oracle (progress.rs): a churning-but-dead world.
    StalledProgress => "stalled_progress",
    ProgressSurfaceEmpty => "progress_surface_empty",

    // Infrastructure errors
    OracleError => "oracle_error",
    ScenarioError => "scenario_error",
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique() {
        let mut sorted: Vec<&str> = Rule::ALL.iter().map(|r| r.name()).collect();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), Rule::ALL.len(), "Duplicate rule names found");
    }

    #[test]
    fn rule_names_valid_format() {
        for &rule in Rule::ALL {
            let name = rule.name();
            assert!(!name.is_empty(), "Empty rule name");
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit()),
                "Invalid rule name format: {name}"
            );
        }
    }

    #[test]
    fn every_rule_has_meaning() {
        for &rule in Rule::ALL {
            assert!(!rule.meaning().is_empty(), "{rule} lacks meaning");
        }
    }
}
