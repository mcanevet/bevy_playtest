//! Rule-name registry: all oracle rule names as an enum.
//!
//! Every entry must be emitted by an oracle that exists in this crate,
//! or be reserved for the scenario_error path. Kept as a single declarative
//! list (enum + ALL + name()) so registries cannot drift.

/// Single source of truth: declares the `Rule` enum, the `Rule::ALL`
/// registry slice, and the snake_case `name()` mapping from one list.
macro_rules! declare_rules {
    ($( $(#[$attr:meta])* $variant:ident => $name:literal ),* $(,)?) => {
        /// All oracle rule names as an enum.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Rule {
            $($(#[$attr])* $variant),*
        }

        impl Rule {
            /// All registered rules (derived from the same list as the
            /// enum itself — cannot drift).
            pub const ALL: &[Rule] = &[$($(#[$attr])* Rule::$variant),*];

            /// The string name used in reports (snake_case).
            pub fn name(self) -> &'static str {
                match self {
                    $($(#[$attr])* Rule::$variant => $name),*
                }
            }

            /// Look up a rule by its report name (inverse of `name`).
            pub fn from_name(name: &str) -> Option<Rule> {
                $($(#[$attr])* if name == $name { return Some(Rule::$variant); })*
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
            Self::ResourceCheck => "An authored-intent check: a numeric field on a reflected resource must satisfy the declared comparison (always, or eventually within a deadline) — the resource sibling of component_check: game-wide state like scores, economies and cooldowns.",
            Self::StagnantResource => "Rate-form liveness: a watched numeric leaf on a reflected resource must change at least once per window of game time while a declared driver leaf strictly advances — constant scoreboards on a running clock are livelocks, not gameplay.",
            Self::PairwiseSeparation => "Coincident-bodies spatial oracle: every pair of selected entities (via a with_component selector) must stay at least min_distance apart; violating frames fold into episodes, and only an episode that soaks past window_s (in driver units) becomes a finding — sub-soak overlaps are transient fly-bys, not bugs.",
            Self::DerivedTransformConsistency => "Compositional identity over Bevy's derived Transform relationship: every parented entity's GlobalTransform must equal the parent's GlobalTransform composed with its own Transform, both read at the same settled frame — stale or foreign-written globals (sprites frozen at identity under a moving parent) are state bugs, folded into episodes that must persist past a window_s soak. Promoted to gating (fail-power) on flip-test evidence: the multi-child pile-up fixture comes out RED with the defect present and GREEN with it absent, and the rotating/scaling hierarchy stays coherent — the rule earned trust by demonstrating the discrimination, not by optimism (proposal §2).",
            Self::HierarchyAbsent => "A derived_transform_consistency rule found no parented entities over the whole run: no hierarchy, no identity to check. Informational, not vacuous — a flat world is a coherent scene the rule simply cannot judge.",
            Self::FiniteTransforms => "Every entity's Transform holds finite values (no NaN/inf).",
            #[cfg(feature = "render-liveness")]
            Self::RenderLiveness => "A structurally live render path: a sim-active run with something to draw should have at least one camera that is enabled and written out; informational — a structural rule, it sees the main-world camera components, never the rendered pixels (a live camera rendering black passes).",
            #[cfg(feature = "screen-liveness")]
            Self::ScreenLiveness => "A screen that reflects its simulation: a sim-active run's captured frames should not be bit-identical forever; informational — pixel-hash evidence from a harness-owned capture rig (2D views only; legitimately static screens like menus are a judgment call, not an oracle).",
            Self::VacuousRun => "Nothing was ever observable: no entities to check and no inputs emitted — the run proves nothing about the game.",
            Self::VacuousInvariant => "An invariant matched zero entities; the check proves nothing until there is something to check.",
            Self::WindowUnelapsed => "A stagnant_resource window (in driver-leaf units) never elapsed: the driver never advanced past the window over the whole run, so the stagnation verdict is unjudgeable for THIS run — lengthen the run, shrink the window, or accept the note. Informational: a driver that legitimately rests (frozen world aside) is correct behavior, not a failure.",
            Self::ReplayDivergence => "Same scenario + seed run twice must produce identical per-frame fingerprints; divergence means hidden nondeterminism (wall clock, unseeded RNG, unordered iteration).",
            Self::StalledProgress => "A churning world is not a progressing one: the frame loop keeps ticking with no inputs buffered and no observable game-state surface change over a whole window — the world is spinning without doing anything.",
            Self::ProgressSurfaceEmpty => "No game state was verifiable at all during progress monitoring (no reflected game types and no progress markers/resources in scope); progress cannot be assessed. Informational by default — escalate with strict_warnings or progress.require_surface.",
            Self::OracleError => "An oracle itself errored while evaluating (infrastructure failure, not a game bug).",
            Self::ScenarioError => "The scenario itself failed (rejected config) rather than the game misbehaving.",
            Self::LintFinding => "Lint-mode finding: the scenario config failed static validation, or a definite type-resolution error was caught by the lint registry probe (BEVY_PLAYTEST_LINT). Never a statement about a real run's outcome.",
        }
    }

    /// Whether this rule is informational — reported but
    /// never gating the run status. Derived from the enum so the
    /// registry cannot drift.
    pub fn is_informational(self) -> bool {
        match self {
            Self::LogWarn
            | Self::LogCaptureLost
            | Self::LogBridgeLost
            | Self::LogLevelTampered
            | Self::ProgressSurfaceEmpty
            | Self::WindowUnelapsed
            | Self::HierarchyAbsent => true,
            #[cfg(feature = "render-liveness")]
            Self::RenderLiveness => true,
            #[cfg(feature = "screen-liveness")]
            Self::ScreenLiveness => true,
            _ => false,
        }
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
    ResourceCheck => "resource_check",
    StagnantResource => "stagnant_resource",

    // Coincident-bodies spatial oracle (judgement layer, slice 1):
    // pairs selected via with_component, folded into episodes,
    // judged against a window_s soak in driver units.
    PairwiseSeparation => "pairwise_separation",

    // Compositional identity over the derived Transform
    // relationship (judgement layer, slice 2): parented entities'
    // GlobalTransform vs parent∘Transform, both sides read at
    // the same settled frame, episodes persisted past a
    // window_s soak.
    DerivedTransformConsistency => "derived_transform_consistency",

    // A derived_transform_consistency rule with zero parented
    // entities over the whole run (judgement layer, slice 2).
    // INFORMATIONAL — deliberately not vacuous_invariant: a flat
    // world is a coherent scene the rule cannot judge, not a
    // misauthored invariant, and a gating vacuity would punish
    // every legitimately hierarchy-free game that scopes the rule.
    HierarchyAbsent => "hierarchy_absent",

    // Structural render-path health (informational; render-liveness
    // feature). A visual game with no working camera is soft-broken,
    // but "renders correctly" cannot be judged structurally — see
    // Rule::meaning and docs/how-to/headless-ci-gpu.md. Compiled out
    // entirely when the feature is off (Rule::ALL shrinks with it).
    #[cfg(feature = "render-liveness")]
    RenderLiveness => "render_liveness",

    // Pixel-observed screen health (informational; screen-liveness
    // feature): the successor probe to render_liveness — a
    // harness-owned capture rig reads back what the renderer
    // actually produced and hashes it; a screen that never changed
    // while the simulation claimed activity is surfaced as evidence,
    // never a failure (legitimately static screens exist). Compiled
    // out entirely when the feature is off.
    #[cfg(feature = "screen-liveness")]
    ScreenLiveness => "screen_liveness",

    // Vacuity guard: a run where nothing was ever observable.
    VacuousRun => "vacuous_run",

    // Vacuity guard: a single invariant matched zero entities.
    VacuousInvariant => "vacuous_invariant",

    // Post-run elision audit: a stagnant_resource window that never
    // elapsed under the driver's own advance (informational — the
    // 0.14 doctrine that a present-but-frozen driver legitimately
    // passes silently must NOT flip pass->fail).
    WindowUnelapsed => "window_unelapsed",

    // Verification depth.
    ReplayDivergence => "replay_divergence",

    // Progress-liveness oracle (progress.rs): a churning-but-dead world.
    StalledProgress => "stalled_progress",
    ProgressSurfaceEmpty => "progress_surface_empty",

    // Lint mode (plugin.rs): a config-coherence finding surfaced by
    // `BEVY_PLAYTEST_LINT` — an aggregated scenario validation
    // error or a definite resolution error caught by the registry
    // probe of the booted App. The config is wrong, not the game:
    // lint never says anything about a real run's outcome.
    LintFinding => "lint_finding",

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
