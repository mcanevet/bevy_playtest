//! Orchestrating discovery flow (Phase 7): cache → probe pool →
//! determinism gate → diff aggregation → group completion → cache.
//!
//! [`discover`] is the one-call entry point the battery composes:
//!
//! 1. **Cache lookup** ([`CacheKey::for_binary`] over the game binary
//!    plus active harness features): a fresh hit returns the
//!    persisted surface without spawning anything.
//! 2. **Probe matrix**: the plan's jobs run through the pool
//!    ([`run_jobs`]); the FIRST infrastructure error wins (refusal
//!    over silence — a torn matrix is never interpreted).
//! 3. **Double-control determinism gate**: per probe point, the two
//!    identical control runs are compared with [`digest::compare`].
//!    Divergence does NOT abort discovery — the surface is still
//!    built, but stamped [`SurfaceConfidence::Statistical`] with a
//!    note, so no report reader trusts a negative (fail-open, never
//!    fail-closed on nondeterminism).
//! 4. **Aggregation** ([`aggregate`]): each probe run is diffed
//!    against its point's control baseline ([`diff_trace`]); a key
//!    effective at ANY point is effective, with the first divergent
//!    point's effect. Probed-but-inert keys stay in the surface as
//!    trusted negatives (when the gate passed).
//! 5. **Group completion** runs inside [`aggregate`], so cached and
//!    fresh surfaces are always identically completed.
//! 6. **Cache write** is best-effort: a failure costs the NEXT run
//!    ~35s, never this one (a cache must not fail discovery).
//!
//! The static scan (Phase 1) runs IN THE GAME CHILD: the probe pool
//! asks its first control run to export the scan
//! ([`crate::plugin::ENV_DISCOVER_SCAN`]); `run_scenario` CAPTURES
//! the scan at harness-setup time — after input-signal seeding but
//! BEFORE the harness registers its bot and oracle systems, whose
//! input-message writes would otherwise count as phantom game
//! readers — only when that export was requested (ordinary
//! playtests pay no scan cost); the takeover exports the captured
//! scan (parked OUTSIDE the world — a resource entity would shift
//! the digest counts the diff compares) after the
//! scenario, and [`aggregate`] merges the child's result into the
//! surface's
//! `readers`/`pruned_devices`/`notes`. A missing or unparsable
//! export degrades to the fail-open note — the run and its verdicts
//! are unaffected, and the keyboard class is never pruned
//! regardless (run-condition blind spot). An export that could not
//! see the full schedule graph (incomplete scan) keeps its reader
//! counts as annotations but withholds pruning (zero readers prove
//! nothing from an incomplete scan).
//!
//! Opt-in contract (read by the battery wiring, and stable for
//! script/CI use). All four are stripped from spawned children by
//! both the battery and the pool (strict takeover env contract), so
//! the game never sees them:
//!
//! - `BEVY_PLAYTEST_DISCOVER=1` — attach discovery to battery runs.
//! - `BEVY_PLAYTEST_DISCOVER_INJECT=1` — discovery-driven scenario
//!   injection: the battery parent discovers FIRST (cache-first),
//!   then — when the scenario declares no surface of its own (the
//!   built-in battery literal) — drives the game child with an
//!   injected scenario whose chaos bot samples exactly the
//!   discovered effective keys, and records them as
//!   `report.injected_surface_keys`. Implies discovery (the
//!   annotation rides along). An authored scenario file always
//!   wins: declaration beats inference.
//! - `BEVY_PLAYTEST_DISCOVER_KEYS=Comma,List` — override the probe
//!   candidates (slim targeted probes; must be canonical spellings,
//!   invalid ones fail the run rather than probe garbage).
//! - `BEVY_PLAYTEST_DISCOVER_CACHE=path` — cache dir override
//!   (defaults to `.bevy_playtest/cache/`; CI isolates per job with
//!   this).

use std::path::{Path, PathBuf};

use crate::digest;
use crate::state::PlaytestReport;

use super::cache::{
    default_cache_dir, plan_fingerprint, read_cached, write_cached, CacheKey, CachedSurface,
};
use super::diff::diff_trace;
use super::group_completion::apply_group_completion;
use super::probe_pool::{
    build_job_list, run_jobs, ProbeError, ProbeJobKind, ProbeOutcome, ProbePoolConfig,
    DEFAULT_POOL_SIZE, DEFAULT_TIMEOUT_SECS,
};
use super::probe_scenarios::{ProbePlan, ProbePoint};
use super::surface::{DiscoveredSurface, KeyConfidence, KeyFinding, SurfaceConfidence};

/// Opt-in switch for battery-run discovery (value `"1"`).
pub const ENV_DISCOVER: &str = "BEVY_PLAYTEST_DISCOVER";
/// Comma-separated candidate-key override for discovery runs.
pub const ENV_DISCOVER_KEYS: &str = "BEVY_PLAYTEST_DISCOVER_KEYS";
/// Discovery cache-dir override.
pub const ENV_DISCOVER_CACHE: &str = "BEVY_PLAYTEST_DISCOVER_CACHE";
/// Opt-in to discovery-driven scenario injection: the battery
/// parent discovers the surface FIRST (cache-first), then — when
/// the scenario declares no surface of its own (the built-in
/// battery literal) — writes a scenario variant whose chaos bot
/// samples exactly the discovered effective keys and drives the
/// game child with it. Implies `BEVY_PLAYTEST_DISCOVER=1` (the
/// annotation rides along free); an authored scenario file always
/// wins (declaration beats inference).
pub const ENV_DISCOVER_INJECT: &str = "BEVY_PLAYTEST_DISCOVER_INJECT";

/// Active harness features, for the cache key: two builds of
/// bevy_playtest with different feature sets are different harnesses
/// and must not share cache entries.
fn active_features() -> Vec<&'static str> {
    let all = [
        ("in-process", cfg!(feature = "in-process")),
        ("takeover", cfg!(feature = "takeover")),
        ("takeover-winit", cfg!(feature = "takeover-winit")),
        ("ui-liveness", cfg!(feature = "ui-liveness")),
        // The surface embeds the lw reconcile section when on, so
        // lw-on and lw-off harness builds must not share entries.
        ("lw-reader", cfg!(feature = "lw-reader")),
    ];
    all.iter()
        .filter(|(_, on)| *on)
        .map(|(name, _)| *name)
        .collect()
}

/// Configuration for [`discover`]. `binary` is the only field with no
/// default — every other field covers the design-doc MVP via
/// [`DiscoveryConfig::new`].
#[derive(Clone, Debug)]
pub struct DiscoveryConfig {
    /// Feature-enabled game binary
    /// (`env!("CARGO_BIN_EXE_<name>")` from a test).
    pub binary: PathBuf,
    /// The probe matrix (candidates, tps, duration, control key).
    pub plan: ProbePlan,
    /// Concurrent subprocess cap (design doc: 6–8).
    pub pool_size: usize,
    /// Wall-clock kill switch per probe subprocess (seconds).
    pub timeout_secs: u64,
    /// Cache root. Default: `.bevy_playtest/cache/` under the CWD.
    pub cache_dir: PathBuf,
    /// Fixture/game identifier; `None` for a plain game binary.
    pub fixture_name: Option<String>,
}

impl DiscoveryConfig {
    /// Design-doc MVP defaults for everything except the binary.
    pub fn new(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
            plan: ProbePlan::default(),
            pool_size: DEFAULT_POOL_SIZE,
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            cache_dir: default_cache_dir(),
            fixture_name: None,
        }
    }
}

/// Result of [`discover`]: the surface, and whether it came from the
/// cache (report consumers and tests distinguish "learned" from
/// "recalled").
#[derive(Clone, Debug)]
pub struct DiscoveryOutcome {
    pub surface: DiscoveredSurface,
    /// `true` when a fresh cache entry answered without probing.
    pub from_cache: bool,
}

/// Discovery failure modes. All are infrastructure problems (probe
/// crashes, bad candidates, unreadable game binary); the surface
/// itself never fails — a nondeterministic game yields a
/// `Statistical` surface, not an error.
#[derive(Debug)]
pub enum DiscoveryError {
    /// The probe matrix failed (a run produced no report, or plan
    /// generation rejected the candidates).
    Probe(ProbeError),
    /// Setup failed before any probe ran (hashing the binary,
    /// malformed matrix).
    Setup(String),
}

impl std::fmt::Display for DiscoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Probe(e) => write!(f, "probe matrix failed: {e}"),
            Self::Setup(msg) => write!(f, "discovery setup failed: {msg}"),
        }
    }
}

impl std::error::Error for DiscoveryError {}

/// Is discovery requested via the environment? (Battery wiring.)
pub fn env_requested() -> bool {
    std::env::var(ENV_DISCOVER).ok().as_deref() == Some("1")
}

/// Is discovery-driven scenario injection requested? (Battery
/// wiring — see [`ENV_DISCOVER_INJECT`]. Implies discovery: a
/// parent injecting a surface it never discovered is a logic bug,
/// so the battery treats this as requesting discovery too.)
pub fn inject_requested() -> bool {
    std::env::var(ENV_DISCOVER_INJECT).ok().as_deref() == Some("1")
}

/// Full discovery driven by the opt-in env contract: candidate and
/// cache-dir overrides from `BEVY_PLAYTEST_DISCOVER_KEYS` /
/// `BEVY_PLAYTEST_DISCOVER_CACHE` on top of [`DiscoveryConfig::new`]
/// defaults. `fixture_name` is the caller's fixture/scenario identity
/// and joins the cache key (see [`DiscoveryConfig::fixture_name`] and
/// the cache docs): a game binary driven by several scenarios — the
/// fixture battery compiles one binary for all its
/// `tests/playtest.rs` cases — must get a distinct cache entry per
/// scenario, or the first scenario's surface would be handed to the
/// rest. Pass `None` for a plain game binary with a single identity.
///
/// The battery calls this when [`env_requested`] is true.
pub fn discover_from_env(
    binary: &Path,
    fixture_name: Option<&str>,
) -> Result<DiscoveryOutcome, DiscoveryError> {
    let mut config = DiscoveryConfig::new(binary);
    config.fixture_name = fixture_name.map(|f| f.to_string());
    if let Ok(list) = std::env::var(ENV_DISCOVER_KEYS) {
        // The override is explicit user intent: an EMPTY list must
        // NOT silently fall back to the ~35s default matrix (that
        // would directly contradict "probe exactly these keys"), and
        // every accepted name is normalized to its canonical
        // spelling via `normalize_key_name` BEFORE it enters the
        // plan — aliases (`digit1`, `w`, `Space`) must never leak
        // raw into `KeyFinding.key` or the plan fingerprint, where
        // they would multiply cache entries and fragment the
        // surface vocabulary. Unknown names are a setup error, not
        // a silently dropped key.
        let mut keys: Vec<String> = Vec::new();
        for candidate in list.split(',') {
            let trimmed = candidate.trim();
            if trimmed.is_empty() {
                continue;
            }
            match crate::scenario::normalize_key_name(trimmed) {
                Some(canonical) => {
                    if !keys.iter().any(|k| k == canonical) {
                        keys.push(canonical.to_string());
                    }
                }
                None => {
                    return Err(DiscoveryError::Setup(format!(
                        "unknown key '{trimmed}' in {} — use a canonical key \
                         name or alias (e.g. KeyW, Digit1, Space)",
                        ENV_DISCOVER_KEYS
                    )))
                }
            }
        }
        if keys.is_empty() {
            return Err(DiscoveryError::Setup(format!(
                "{ENV_DISCOVER_KEYS} is set but empty (or whitespace-only) — \
                 refusing to silently fall back to the full default matrix; \
                 unset the variable to use the default matrix"
            )));
        }
        config.plan.candidate_keys = keys;
    }
    if let Ok(dir) = std::env::var(ENV_DISCOVER_CACHE) {
        if !dir.is_empty() {
            config.cache_dir = PathBuf::from(dir);
        }
    }
    discover(&config)
}

/// The orchestrator (see the module docs for the phase-by-phase
/// walk-through). Cache hits skip all probing.
pub fn discover(config: &DiscoveryConfig) -> Result<DiscoveryOutcome, DiscoveryError> {
    // Front-door canonicalization (Copilot r4468466364): reduce every
    // key name — control key AND candidates — to one vocabulary
    // BEFORE anything downstream sees it. Aliases (`q` vs `KeyQ`,
    // `digit1` vs `Digit1`) press the same physical key, so a raw
    // alias in the control key would diff a candidate's probe run
    // against a baseline built from the SAME physical input — the
    // guaranteed `None` would then masquerade as a trusted `Exact`
    // negative, defeating the `Unclassified` self-probe guard (which
    // compares spellings). Canonicalizing here also means the
    // fingerprint, job identity, surface findings, and self-probe
    // guard all agree, and alias-vs-canonical plans can no longer
    // fragment cache entries. Unknown names are a setup error:
    // silently keeping a raw unknown would reopen the split
    // vocabulary this step exists to close.
    let plan = config
        .plan
        .canonicalized()
        .map_err(|e| DiscoveryError::Setup(e.to_string()))?;
    // The key covers everything that can change the answer — binary,
    // harness version, features, fixture, and the probe plan itself
    // (a slim `DISCOVER_KEYS` run must never inherit a full-matrix
    // entry or vice versa). An unreadable binary is a setup failure,
    // not a miss.
    let key = CacheKey::for_binary(
        &config.binary,
        active_features(),
        config.fixture_name.clone(),
        plan_fingerprint(
            &plan.candidate_keys,
            &plan.control_key,
            plan.tps,
            plan.duration_s,
        ),
    )
    .map_err(|e| {
        DiscoveryError::Setup(format!(
            "cannot hash game binary '{}': {e}",
            config.binary.display()
        ))
    })?;

    // Cache lookup: a cache must never fail discovery — any read
    // error is treated as a miss and re-probes.
    if let Ok(Some(cached)) = read_cached(&config.cache_dir, &key) {
        return Ok(DiscoveryOutcome {
            surface: cached.surface,
            from_cache: true,
        });
    }

    let jobs = build_job_list(&plan).map_err(DiscoveryError::Probe)?;
    let pool = ProbePoolConfig {
        binary: config.binary.clone(),
        pool_size: config.pool_size,
        timeout_secs: config.timeout_secs,
    };
    let results = run_jobs(&pool, &jobs);
    // First error wins; the matrix is never partially interpreted.
    if let Some(err) = results.iter().find_map(|r| r.as_ref().err()) {
        return Err(DiscoveryError::Probe(err.clone()));
    }
    let outcomes: Vec<ProbeOutcome> = results
        .into_iter()
        .map(|r| r.expect("first-error check above"))
        .collect();

    // Matrix sanity: diffing needs a control baseline per probe
    // point. `build_job_list` guarantees it; guard anyway so a future
    // plan shape can't silently misclassify probes as inert.
    for outcome in &outcomes {
        if let ProbeJobKind::Probe { point, .. } = &outcome.kind {
            let has_baseline = outcomes.iter().any(
                |c| matches!(&c.kind, ProbeJobKind::Control { point: p } if p.label == point.label),
            );
            if !has_baseline {
                return Err(DiscoveryError::Setup(format!(
                    "probe matrix has no control run for point '{}'",
                    point.label
                )));
            }
        }
    }

    // The first control run carried the static-scan export request
    // (`build_job_list`); its sidecar (if delivered) is the matrix's
    // scan. `find_map` (not `[0]`) so direct `run_jobs_in` callers
    // that deviate from the first-control convention still work.
    let scan = outcomes.iter().find_map(|o| o.scan.clone());

    // Expected span from the plan (see `aggregate`'s docs): the
    // pool's completeness gate already refuses reports short of
    // this frame, so for real pipelines the argument is a
    // redundancy check — it exists so DIRECT callers of the
    // public `aggregate` can thread the same contract.
    let surface = aggregate(
        &plan.control_key,
        &outcomes,
        scan.as_ref(),
        Some(plan.expected_last_frame()),
    );

    // Promote the game child's build profile onto the surface (the
    // first control run's report carries it; probe children carry
    // the same stamp). The parent harness's own profile is NOT the
    // game's — this is the only honest channel (roadmap §4 item 5).
    let promoted = outcomes.iter().find_map(|o| o.report.game_build_profile);
    let mut surface = surface;
    surface.build_profile = promoted;

    // Work item 2: promote the contract suggestions the
    // scan-exporting first control run captured onto the surface
    // (the scan piggybacks the same sidecar as `readers`). Absent
    // scan → `None`, honestly: the section describes what a
    // structural walk SAW, never what absence would imply.
    surface.suggestions = scan.as_ref().and_then(|scan| scan.suggestions.clone());

    // D1-d wiring: aggregate the Leafwing declared-tier evidence the
    // child runs recorded (every child spawns the game's `InputMap`s;
    // each child's tap saw only ITS scenario's presses) into one
    // run-equivalent evidence and reconcile it against THIS
    // surface's live inputs. Reported as an additive `lw` section —
    // the D1-c sweep fields stay untouched (the retraction stays
    // consumer-side). Games without Leafwing state (or lw-off
    // builds) leave the section absent, honestly.
    #[cfg(feature = "lw-reader")]
    {
        let reports: Vec<&PlaytestReport> = outcomes.iter().map(|o| &o.report).collect();
        if let Some(evidence) = crate::discovery::lw::aggregate_lw_evidence(&reports) {
            let live: std::collections::BTreeSet<String> = surface
                .keys
                .iter()
                .chain(surface.mouse_buttons.iter())
                .filter(|k| k.effect.is_some() && k.confidence != KeyConfidence::Unclassified)
                .map(|k| k.key.clone())
                .collect();
            surface.lw = Some(crate::discovery::lw::reconcile(&evidence, &live));
        }
    }

    // Best-effort persistence: a failure costs the next run its
    // probe time, never this one its result.
    let entry = CachedSurface {
        key,
        surface: surface.clone(),
    };
    let _ = write_cached(&config.cache_dir, &entry);

    Ok(DiscoveryOutcome {
        surface,
        from_cache: false,
    })
}

/// Turn executed probe outcomes into a [`DiscoveredSurface`]: the
/// double-control gate decides whole-surface confidence, each probe
/// is diffed against its point's baseline, and group completion runs
/// (so cached and fresh surfaces are identically completed).
///
/// `control_key` is the key the control runs pressed. If it also
/// appears among the probed candidates, its finding is marked
/// [`KeyConfidence::Unclassified`]: its probe run is identical to its
/// control baseline BY CONSTRUCTION, so "no divergence" carries zero
/// information about it whatever the game binds — it is never a
/// trusted negative. (The default candidate set excludes the control
/// key; the flow accepts it for bespoke matrices and labels it.)
///
/// Pure (no IO, no subprocesses) — unit-testable on fabricated
/// reports. Expects a well-formed matrix: control runs present for
/// every probed point ([`discover`] enforces this; see
/// [`build_job_list`] for the producer). `control_key` and the
/// probe keys must already be in CANONICAL spelling:
/// [`ProbePlan::canonicalized`] reduces the flow's plan at the
/// front door, because the self-probe guard compares spellings —
/// an alias pair (`"q"` control, `"KeyQ"` candidate) presses the
/// same physical key and would smuggle a vacuous `Exact` negative
/// past the guard.
///
/// # Expected span
///
/// `expected_last_frame` closes the last coverage hole for DIRECT
/// callers (Copilot #58 round 13, "reject matching but incomplete
/// traces in public aggregation"). The pairwise backstop below
/// proves the probe and control traces are SYMMETRIC — intact,
/// contiguous, equal-length — but symmetry says nothing about
/// whether they reached the run's intended END: two identical
/// traces stopping at frame 100 of a 180-frame plan compare as
/// fully covered and yield a trusted `Exact` inert over 79 frames
/// neither run recorded. `None` keeps that legacy contract, for
/// callers with no plan knowledge. `Some(n)` demands both traces
/// actually cover frames `0..=n`: a matching-but-short pair
/// downgrades to [`KeyConfidence::Unclassified`], exactly like an
/// asymmetric pair. [`discover`] always passes
/// `Some(plan.expected_last_frame())` — its pipelines were ALWAYS
/// safe on this axis via the pool's completeness gate (the pool
/// refuses any report short of the plan's expected final frame as
/// `UnusableReport` before aggregation), so the parameter exists
/// to close the PUBLIC-API flank for direct callers, not to fix
/// the flow.
///
/// Whether a probe/control pair's traces cover the SAME frame span.
///
/// Defense-in-depth behind the pool's completeness gate: the pool
/// refuses reports that stop short of the run's expected final
/// frame, so real pipelines never feed `aggregate` asymmetric
/// pairs — but `aggregate` is also callable directly (unit tests,
/// future callers), and `diff_trace` compares only the COMMON
/// prefix of the two traces. If either trace ends early, the `None`
/// ("never diverged") verdict only speaks for the frames BOTH
/// runs recorded: a trusted negative from it would repeat the
/// partial-trace hole at the aggregation layer. The backstop
/// Therefore requires both traces to be intact, contiguous
/// frame sequences of the same length — see the body for why
/// maxima equality is not enough. The EXPECTED span (when the
/// caller supplies one) is checked separately in `aggregate`,
/// after this pairwise gate: a pair can be symmetric yet short.
fn coverage_mismatch(probe: &PlaytestReport, baseline: &PlaytestReport) -> bool {
    // Sequences must be IDENTICAL and individually intact — same
    // length, frames 0,1,2,... stepwise, no gaps, no duplicates —
    // not merely the same maximum frame. Equal maxima with SHARED
    // interior gaps leave the pair positionally identical to each
    // other while the omitted frames were never compared at all: a
    // `None` verdict over the recorded islands says nothing about
    // the holes, so such keys must stay unclassified. When the
    // caller does not supply an expected span, the check anchors
    // on the recorder's contract instead
    // (`record_digest_system` emits one digest per tick over
    // frames 0..=ticks-1): any trace that is not a stepwise
    // sequence from frame 0 is damaged. An EMPTY trace still
    // supports no verdict at all.
    let contiguous_from_zero =
        |t: &[crate::digest::StateDigest]| t.iter().enumerate().all(|(i, d)| d.frame == i as u64);
    probe.state_digest.is_empty()
        || baseline.state_digest.is_empty()
        || !contiguous_from_zero(&probe.state_digest)
        || !contiguous_from_zero(&baseline.state_digest)
        || probe.state_digest.len() != baseline.state_digest.len()
}

/// Whether a symmetric (already `coverage_mismatch`-passed) pair of
/// traces also reaches the run's EXPECTED final frame. `None`
/// (no plan knowledge — legacy direct callers) accepts any
/// symmetric pair. `Some(n)` demands the shared span cover
/// frames `0..=n`: a matching-but-truncated pair compares as
/// "never diverged" over its common prefix, and trusting that
/// `None` for the frames past the truncation point would repeat
/// the partial-trace hole the pool's completeness gate already
/// closes for real pipelines. Coverage of the expected span is
/// proven by LENGTH (`coverage_mismatch` already ruled out gaps,
/// so a contiguous-from-zero trace of `n + 1` frames IS
/// `0..=n`); the probe and baseline agree in length there.
fn expected_span_covered(
    probe: &PlaytestReport,
    baseline: &PlaytestReport,
    expected_last_frame: Option<u64>,
) -> bool {
    let Some(last) = expected_last_frame else {
        return true;
    };
    // A run whose expected span itself is degenerate (zero ticks
    // planned) can never license verdicts: length 0 fails
    // `coverage_mismatch` already, and here `n + 1 <= len` demands
    // at least one recorded frame. The cast is CHECKED: on a
    // 32-bit target a span of `usize::MAX` or wider cannot be
    // represented, and a truncating `as` cast could wrap into a
    // small bound and license a short trace — overflow must
    // fail OPEN (uncovered), never silently trust.
    let last = match usize::try_from(last) {
        Ok(n) => n,
        Err(_) => return false,
    };
    probe.state_digest.len() > last && baseline.state_digest.len() > last
}

pub fn aggregate(
    control_key: &str,
    outcomes: &[ProbeOutcome],
    scan: Option<&crate::discovery::static_scan::StaticScanResult>,
    expected_last_frame: Option<u64>,
) -> DiscoveredSurface {
    // Per point: the FIRST control report is the diff baseline; the
    // SECOND is the determinism gate. Later copies (none today)
    // re-check the gate against the baseline — harmless.
    let mut baselines: Vec<(ProbePoint, &PlaytestReport)> = Vec::new();
    let mut gate_diverged_at: Option<u64> = None;
    for outcome in outcomes {
        let ProbeJobKind::Control { point } = &outcome.kind else {
            continue;
        };
        match baselines.iter_mut().find(|(p, _)| p.label == point.label) {
            None => baselines.push((*point, &outcome.report)),
            Some((_, baseline)) => {
                if let Err(divergence) =
                    digest::compare(&baseline.state_digest, &outcome.report.state_digest)
                {
                    if gate_diverged_at.is_none() {
                        gate_diverged_at = Some(divergence.frame);
                    }
                }
            }
        }
    }

    // One finding per candidate key, in first-seen (job) order. A key
    // probed at several points is effective if ANY point diverged;
    // the first divergent point's effect is the finding (later points
    // only refine if earlier ones were inert).
    //
    // `coverage_trusted` records whether the diff behind this key's
    // verdict compared FULL-span traces: the pool's completeness gate
    // guarantees that for real pipelines; the backstop below
    // preserves the guarantee for direct callers whose probe and
    // control reports have asymmetric frame coverage (an inert
    // verdict from a common-prefix-only comparison would be a
    // trusted FALSE negative).
    let mut probes: Vec<(String, Option<super::diff::Effect>, bool)> = Vec::new();
    // Direct-caller backstop (discover()'s matrix always matches
    // every probe point to a control pair): a probe outcome whose
    // point has no control baseline can support NO verdict — but
    // dropping it silently would hide the malformed matrix. The
    // surface keeps a note naming the victims (refusal over
    // silence, same audience as the coverage caveat).
    let mut unmatched_point_probes: Vec<String> = Vec::new();
    for outcome in outcomes {
        let ProbeJobKind::Probe { key, point } = &outcome.kind else {
            continue;
        };
        let Some((_, baseline)) = baselines.iter().find(|(p, _)| p.label == point.label) else {
            if !unmatched_point_probes.iter().any(|k| k == key) {
                unmatched_point_probes.push(key.clone());
            }
            continue;
        };
        let coverage_trusted = !coverage_mismatch(&outcome.report, baseline)
            && expected_span_covered(&outcome.report, baseline, expected_last_frame);
        let effect = diff_trace(
            &outcome.report.state_digest,
            &baseline.state_digest,
            point.frame,
        );
        match probes.iter_mut().find(|(k, _, _)| k == key) {
            None => probes.push((key.clone(), effect, coverage_trusted)),
            Some((_, slot, trusted)) => {
                if slot.is_none() {
                    *slot = effect;
                    // Once an inert verdict lands, the trust flag is
                    // MONOTONICALLY accumulated across points: `&=`,
                    // not assignment. An inert finding is
                    // trustworthy only if EVERY point that produced
                    // (or refined) it had complete coverage — one
                    // truncated probe/control pair at ANY point
                    // taints the verdict, because its `None` spoke
                    // for fewer frames than the run was supposed to
                    // observe, and a later complete point does not
                    // retroactively cover those missing frames.
                    *trusted &= coverage_trusted;
                }
            }
        }
    }

    let mut untrusted_inert: Vec<String> = Vec::new();
    let mut surface = DiscoveredSurface {
        keys: probes
            .into_iter()
            .map(|(key, effect, coverage_trusted)| {
                // The control key cannot classify itself: its probe
                // run and its baseline are the same scenario. Mark it
                // instead of trusting a vacuous negative.
                let confidence = if key == control_key {
                    KeyConfidence::Unclassified
                } else if effect.is_none() && !coverage_trusted {
                    // Asymmetric probe/control coverage: `diff_trace`
                    // compared only the frames BOTH runs recorded, so
                    // "never diverged" speaks for less than the full
                    // run. Record the finding honestly as
                    // unclassified rather than a trusted negative
                    // (the pool normally refuses such reports before
                    // aggregation; this is the backstop).
                    untrusted_inert.push(key.clone());
                    KeyConfidence::Unclassified
                } else {
                    KeyConfidence::Exact
                };
                KeyFinding {
                    key,
                    effect,
                    confidence,
                }
            })
            .collect(),
        probed_at: baselines.iter().map(|(p, _)| p.frame).collect(),
        confidence: if gate_diverged_at.is_none() {
            SurfaceConfidence::Exact
        } else {
            SurfaceConfidence::Statistical
        },
        ..Default::default()
    };

    if let Some(frame) = gate_diverged_at {
        surface.notes.push(format!(
            "determinism gate FAILED at frame {frame}: identical control runs \
             diverged — effects are statistical, inert keys are NOT trusted \
             negatives"
        ));
    }
    // Backstop companion to the pool's completeness gate: an inert
    // verdict whose probe/control pair covered asymmetric frames —
    // or symmetric frames short of the caller's expected span — is
    // untrustworthy and was recorded as Unclassified above. Surface
    // it loudly — real pipelines never emit this note (the pool
    // refuses incomplete reports before aggregation); seeing it
    // means a caller bypassed the pool.
    if !untrusted_inert.is_empty() {
        surface.notes.push(format!(
            "coverage caveat: {} probe run(s) ({}) ended early relative to \
              their control baseline or the run's expected span — inert \
              verdicts for them are UNTRUSTED and recorded as unclassified",
            untrusted_inert.len(),
            untrusted_inert.join(", ")
        ));
    }
    // Companion to the coverage caveat for the OTHER direct-caller
    // malformation: a probe whose point has NO control baseline at
    // all can support no verdict and was dropped above. Real
    // pipelines never emit this note (build_job_list always pairs
    // every probe point with controls); seeing it means a caller
    // bypassed discover().
    if !unmatched_point_probes.is_empty() {
        surface.notes.push(format!(
            "unmatched probe point(s): {} probe run(s) ({}) reference a probe \
              point with no control run — no verdict is possible for them and \
              they were omitted from the surface",
            unmatched_point_probes.len(),
            unmatched_point_probes.join(", ")
        ));
    }
    // The control-key caveat is a property of the METHOD, not of
    // any particular matrix: the baseline is itself a key-press run,
    // so a game that CONSUMES the control key biases every candidate
    // diff — worst case a candidate causing the same digest change
    // as the control press compares equal and reads as falsely
    // inert (the double-control gate checks repeatability, not
    // neutrality, and cannot catch this). Hash-only MVP cannot
    // detect it at runtime; the practical mitigations (choosing an
    // inert control key, an idle baseline) are documented in the
    // design doc. Surface the caveat on EVERY surface so consumers
    // see the limitation alongside the results.
    surface.notes.push(format!(
        "baseline caveat: the control run presses '{control_key}' — a game \
          that consumes it can mask candidates with identical digest \
          effects (fail-open toward INERT here is NOT guaranteed)"
    ));
    // Separate note when the matrix actually probed the control key:
    // its finding is vacuous BY CONSTRUCTION (probe == baseline).
    if surface.keys.iter().any(|k| k.key == control_key) {
        surface.notes.push(format!(
            "'{control_key}' is the control key: its probe run equals its \
             control baseline by construction, so it is unclassified — \
             neither effective nor a trusted negative"
        ));
    }
    // Static input-surface scan data (child-exported; see the module
    // docs). When present it fills the surface's annotation-grade
    // `readers` and derives `pruned_devices`; when absent the flow
    // stays fail-open and says so. Either way it is PURE DATA for
    // this function — no scanning happens here.
    match scan {
        Some(scan) => {
            surface.readers = scan
                .readers
                .iter()
                .map(|(class, count)| (class.as_str().to_string(), *count))
                .collect();
            // Prune derivation, conservative by design: only classes
            // the scan PROVED readerless may be pruned — and only
            // from a scan that saw the whole schedule graph
            // (`complete`): with schedules missing or uninitialized,
            // zero readers proves nothing (the systems that read
            // them were invisible to the scan), so pruning would
            // violate the fail-open contract and could make future
            // probe stages skip inputs that were never inspected.
            // Keyboard is structurally exempt (fail-open rule;
            // run-condition blind spot) by not appearing in this
            // list at all. An exclusive or read-all system counts
            // as a reader of every tracked class, so surfaces
            // carrying those notes never prune anything either.
            // Observer entities likewise force `complete: false` at
            // capture time (their param access is invisible to the
            // schedule walk), so observer-hidden input consumers can
            // never license pruning through this gate.
            if scan.complete {
                for class in [
                    crate::discovery::static_scan::DeviceClass::Gamepad,
                    crate::discovery::static_scan::DeviceClass::MouseWheel,
                ] {
                    if scan.reader_count(class) == 0 {
                        surface.pruned_devices.push(class.as_str().to_string());
                    }
                }
            } else {
                surface.notes.push(
                    "pruned_devices withheld: the static scan did not see \
                     the full schedule graph, so zero reader counts prove \
                     nothing (fail-open; the keyboard class is never pruned)"
                        .to_string(),
                );
            }
            for note in &scan.notes {
                surface.notes.push(note.clone());
            }
        }
        None => {
            surface.notes.push(
                "static scan not run (no scan export arrived from the probe \
                 child; the keyboard class is never pruned)"
                    .to_string(),
            );
        }
    }

    // Cached and fresh surfaces go through completion identically.
    apply_group_completion(&mut surface);
    surface
}

#[cfg(test)]
mod tests {
    use super::super::probe_scenarios::DEFAULT_CONTROL_KEY;
    use super::*;
    use crate::digest::StateDigest;
    use crate::discovery::probe_pool::ProbeOutcome;

    /// A report carrying an explicit per-frame digest trace — the
    /// only part of `PlaytestReport` aggregation reads.
    fn report_with_trace(trace: &[(u64, u64)]) -> PlaytestReport {
        let mut report = PlaytestReport::crashed_empty();
        report.state_digest = trace
            .iter()
            .map(|&(frame, hash)| StateDigest { frame, hash })
            .collect();
        report
    }

    /// Flat trace: identical hashes up to `len`, starting at frame 0.
    fn flat(len: u64) -> Vec<(u64, u64)> {
        (0..len).map(|frame| (frame, 1000)).collect()
    }

    fn probe_outcome(key: &str, point: ProbePoint, trace: &[(u64, u64)]) -> ProbeOutcome {
        ProbeOutcome {
            kind: ProbeJobKind::Probe {
                key: key.to_string(),
                point,
            },
            report: report_with_trace(trace),
            scan: None,
        }
    }

    fn control_outcome(point: ProbePoint, trace: &[(u64, u64)]) -> ProbeOutcome {
        ProbeOutcome {
            kind: ProbeJobKind::Control { point },
            report: report_with_trace(trace),
            scan: None,
        }
    }

    fn point(label: &'static str, frame: u64) -> ProbePoint {
        ProbePoint { label, frame }
    }

    /// A well-formed 1-point matrix (one probe + two controls).
    fn one_point_matrix() -> Vec<ProbeOutcome> {
        let readiness = point("readiness", 60);
        let control = flat(180);
        vec![
            probe_outcome("Digit1", readiness, &control),
            control_outcome(readiness, &control),
            control_outcome(readiness, &control),
        ]
    }

    #[test]
    fn scan_data_populates_readers_and_derives_pruned_devices() {
        // Copilot r4466455064: the flow never ran the static scan, so
        // `readers`/`pruned_devices` shipped empty with only the
        // "static scan not run" note — the advertised scan annotation
        // was undelivered. When a child-exported scan arrives, it
        // must populate `readers` verbatim, derive `pruned_devices`
        // (gamepad/wheel only when readerless; keyboard NEVER), and
        // forward the scan's own notes.
        let mut scan = crate::discovery::static_scan::StaticScanResult {
            readers: vec![
                (crate::discovery::static_scan::DeviceClass::Keyboard, 3),
                (crate::discovery::static_scan::DeviceClass::MouseWheel, 1),
            ],
            complete: true,
            ..Default::default()
        };
        scan.notes
            .push("exclusive system detected: probing the full surface (an exclusive system may read anything)".to_string());
        let surface = aggregate(DEFAULT_CONTROL_KEY, &one_point_matrix(), Some(&scan), None);
        assert_eq!(
            surface.readers,
            vec![("keyboard".to_string(), 3), ("mouse_wheel".to_string(), 1)]
        );
        // Gamepad had NO readers entry → pruned; wheel had a reader →
        // kept; keyboard structurally exempt.
        assert_eq!(surface.pruned_devices, vec!["gamepad".to_string()]);
        assert!(
            !surface
                .notes
                .iter()
                .any(|n| n.contains("static scan not run")),
            "a delivered scan must replace the not-run note: {:?}",
            surface.notes
        );
        assert!(
            surface
                .notes
                .iter()
                .any(|n| n.contains("exclusive system detected")),
            "the scan's notes must be forwarded: {:?}",
            surface.notes
        );
    }

    #[test]
    fn untrusted_inert_from_any_point_stays_unclassified() {
        // Copilot r4468466307: the per-key trust flag used to be
        // OVERWRITTEN by the latest inert-producing point, so a key
        // whose first point had truncated probe/control coverage
        // (untrusted `None`) but whose second point was complete
        // (trusted `None`) was emitted as `Exact` — a trusted
        // negative derived partly from a comparison that observed
        // fewer frames than the run was supposed to record. Trust
        // must accumulate MONOTONICALLY across points: one
        // truncated pair anywhere taints the inert verdict forever,
        // because the later complete point does not retroactively
        // cover the frames the earlier run never recorded.
        let readiness = point("readiness", 60);
        let midrun = point("midrun", 120);
        let control_full = flat(180);
        let truncated = &control_full[..100]; // stops at frame 99
        let outcomes = vec![
            // Point 1: inert from a TRUNCATED probe trace against its
            // point's full-span baseline — `coverage_mismatch` flags
            // it, so this point's `None` is untrusted.
            probe_outcome("KeyZ", readiness, truncated),
            control_outcome(readiness, &control_full),
            control_outcome(readiness, &control_full),
            // Point 2: the SAME key, now a complete, genuinely
            // inert run.
            probe_outcome("KeyZ", midrun, &control_full),
            control_outcome(midrun, &control_full),
            control_outcome(midrun, &control_full),
        ];
        let surface = aggregate(DEFAULT_CONTROL_KEY, &outcomes, None, None);
        let finding = surface
            .keys
            .iter()
            .find(|k| k.key == "KeyZ")
            .expect("KeyZ must produce a finding");
        assert_eq!(
            finding.confidence,
            KeyConfidence::Unclassified,
            "an inert verdict touched by ANY truncated point must never upgrade to Exact: {:?}",
            finding
        );
    }

    #[test]
    fn absent_scan_keeps_the_fail_open_note() {
        // No export (masked names aside, a child may legitimately not
        // deliver): the fail-open note must stay, verbatim contract
        // for consumers diffing surfaces across harness versions.
        let surface = aggregate(DEFAULT_CONTROL_KEY, &one_point_matrix(), None, None);
        assert!(surface.readers.is_empty());
        assert!(surface.pruned_devices.is_empty());
        assert!(
            surface
                .notes
                .iter()
                .any(|n| n.contains("static scan not run")),
            "the not-run note must surface: {:?}",
            surface.notes
        );
    }

    #[test]
    fn scan_with_every_class_read_prunes_nothing() {
        // An exclusive/read-all scan counts every tracked class as
        // read — the surface must not prune gamepad/wheel on it.
        let scan = crate::discovery::static_scan::StaticScanResult {
            readers: [
                crate::discovery::static_scan::DeviceClass::Keyboard,
                crate::discovery::static_scan::DeviceClass::MouseButton,
                crate::discovery::static_scan::DeviceClass::MouseMotion,
                crate::discovery::static_scan::DeviceClass::MouseWheel,
                crate::discovery::static_scan::DeviceClass::Gamepad,
            ]
            .into_iter()
            .map(|class| (class, 1))
            .collect(),
            complete: true,
            ..Default::default()
        };
        let surface = aggregate(DEFAULT_CONTROL_KEY, &one_point_matrix(), Some(&scan), None);
        assert!(surface.pruned_devices.is_empty());
    }

    #[test]
    fn incomplete_scan_never_prunes_even_with_zero_readers() {
        // Copilot r4467758892: a scan that could not see the whole
        // schedule graph (no Schedules resource, or a schedule whose
        // systems were not initialized) reports zero readers for
        // classes it simply never INSPECTED — that is not proof the
        // game doesn't use gamepad/wheel. Pruning on it would violate
        // the fail-open contract: future probe stages could skip
        // inputs that were never looked at. The reader counts that
        // WERE collected stay as annotations; only the prune
        // derivation refuses them (with an explanatory note).
        let scan = crate::discovery::static_scan::StaticScanResult {
            readers: vec![(crate::discovery::static_scan::DeviceClass::Keyboard, 2)],
            notes: vec!["at least one schedule was not initialized at scan time".to_string()],
            complete: false,
            ..Default::default()
        };
        let surface = aggregate(DEFAULT_CONTROL_KEY, &one_point_matrix(), Some(&scan), None);
        assert_eq!(
            surface.readers,
            vec![("keyboard".to_string(), 2)],
            "collected reader counts remain valid annotations"
        );
        assert!(
            surface.pruned_devices.is_empty(),
            "an incomplete scan must never license pruning: {:?}",
            surface.pruned_devices
        );
        assert!(
            surface
                .notes
                .iter()
                .any(|n| n.contains("pruned_devices withheld")),
            "the withheld-pruning caveat must be surfaced: {:?}",
            surface.notes
        );
        assert!(
            surface
                .notes
                .iter()
                .any(|n| n.contains("schedule was not initialized")),
            "the scan's own caveats must still be forwarded: {:?}",
            surface.notes
        );
    }

    #[test]
    fn old_scan_exports_without_the_flag_deserialize_incomplete() {
        // Wire-format backstop for the `complete` flag: a sidecar
        // written before the field existed has no `complete` key —
        // serde-default must read it as `false` (fail-open: stale
        // exports never license pruning), never as a complete scan.
        let json = serde_json::json!({
            "readers": [["gamepad", 0]],
            "notes": [],
        });
        let scan: crate::discovery::static_scan::StaticScanResult =
            serde_json::from_value(json).expect("old-format scan must deserialize");
        assert!(!scan.complete);
    }

    #[test]
    fn bound_key_is_effective_inert_key_is_a_negative() {
        let readiness = point("readiness", 60);
        let control = flat(180);
        // Digit1 diverges AT the tap frame (delta 1); KeyZ (not the
        // control key) matches — a genuine trusted negative.
        let mut digit1_probe = flat(180);
        digit1_probe[60].1 += 1;
        let outcomes = vec![
            probe_outcome("Digit1", readiness, &digit1_probe),
            probe_outcome("KeyZ", readiness, &flat(180)),
            control_outcome(readiness, &control),
            control_outcome(readiness, &control),
        ];

        let surface = aggregate(DEFAULT_CONTROL_KEY, &outcomes, None, None);

        assert_eq!(surface.confidence, SurfaceConfidence::Exact);
        assert_eq!(surface.probed_at, vec![60]);
        assert_eq!(surface.keys.len(), 2, "every probed key stays visible");
        let digit1 = surface.keys.iter().find(|k| k.key == "Digit1").unwrap();
        assert_eq!(
            digit1.effect,
            Some(super::super::diff::Effect {
                first_divergence_frame_delta: 1
            })
        );
        let keyz = surface.keys.iter().find(|k| k.key == "KeyZ").unwrap();
        assert_eq!(keyz.effect, None, "inert key = trusted negative");
        assert_eq!(keyz.confidence, KeyConfidence::Exact);
    }

    #[test]
    fn control_key_among_candidates_is_unclassified_never_a_negative() {
        // A bespoke matrix that probes the control key itself: its
        // probe scenario IS the control scenario, so a null effect
        // says NOTHING about the game's binding of it. The surface
        // must say so instead of recording a trusted negative.
        let readiness = point("readiness", 60);
        let control = flat(180);
        let outcomes = vec![
            probe_outcome("KeyQ", readiness, &flat(180)),
            control_outcome(readiness, &control),
            control_outcome(readiness, &control),
        ];

        let surface = aggregate(DEFAULT_CONTROL_KEY, &outcomes, None, None);

        let keyq = surface.keys.iter().find(|k| k.key == "KeyQ").unwrap();
        assert_eq!(keyq.effect, None);
        assert_eq!(
            keyq.confidence,
            KeyConfidence::Unclassified,
            "the control key cannot classify itself"
        );
        assert!(surface
            .notes
            .iter()
            .any(|n| n.contains("'KeyQ' is the control key")));
        // And the uninformative record is excluded from probe stats.
        assert_eq!(surface.probed_key_count(), 0);
    }

    #[test]
    fn short_probe_trace_never_becomes_a_trusted_negative() {
        // Aggregation-layer backstop for the partial-trace hole:
        // `diff_trace` zips only the common prefix, so a probe trace
        // ending at frame 100 against a 180-frame control "never
        // diverges" over frames 0..=100 and reads `None` — a trusted
        // FALSE negative for the 79 unobserved frames. Real
        // pipelines never produce this pair (the pool's completeness
        // gate refuses the short report before aggregation), but
        // `aggregate` is also called directly; it must not turn a
        // coverage mismatch into an `Exact` inert verdict.
        let readiness = point("readiness", 60);
        let control = flat(180);
        let outcomes = vec![
            // Probe trace covers frames 0..=100: post-tap frames
            // exist, but the run is short of the control's span.
            probe_outcome("KeyZ", readiness, &flat(101)),
            control_outcome(readiness, &control),
            control_outcome(readiness, &control),
        ];

        let surface = aggregate(DEFAULT_CONTROL_KEY, &outcomes, None, None);

        let keyz = surface.keys.iter().find(|k| k.key == "KeyZ").unwrap();
        assert_eq!(keyz.effect, None, "no divergence was observed");
        assert_eq!(
            keyz.confidence,
            KeyConfidence::Unclassified,
            "a common-prefix-only inert is NOT a trusted negative"
        );
        assert!(
            surface.notes.iter().any(|n| n.contains("coverage caveat")),
            "the coverage caveat must be surfaced: {:?}",
            surface.notes
        );
    }

    #[test]
    fn short_control_trace_never_becomes_a_trusted_negative() {
        // Mirror case: the CONTROL run is the short one. Same
        // untrusted inert for the probe key, and the determinism
        // gate itself stays meaningful (the two control copies are
        // identical short traces — matching is still observed).
        let readiness = point("readiness", 60);
        let short_control = flat(101);
        let outcomes = vec![
            probe_outcome("KeyZ", readiness, &flat(180)),
            control_outcome(readiness, &short_control),
            control_outcome(readiness, &short_control),
        ];

        let surface = aggregate(DEFAULT_CONTROL_KEY, &outcomes, None, None);

        let keyz = surface.keys.iter().find(|k| k.key == "KeyZ").unwrap();
        assert_eq!(keyz.confidence, KeyConfidence::Unclassified);
        assert!(surface.notes.iter().any(|n| n.contains("coverage caveat")));
    }

    #[test]
    fn shared_gap_between_equal_maxima_never_becomes_a_trusted_negative() {
        // Copilot r4464792370: EQUAL MAXIMA ARE NOT EQUAL COVERAGE.
        // Both traces end at frame 179, but both OMIT frame 150: the
        // probe and its control agree everywhere they were recorded,
        // `diff_trace` returns `None` — yet frame 150 was never
        // compared, so the inert is untrusted. The old maxima-only
        // check called this pair covered; the sequence check must
        // not.
        let readiness = point("readiness", 60);
        let gap_at = 150usize;
        let control = {
            let mut t = flat(180);
            t.remove(gap_at);
            t
        };
        // Same omission on the probe side: identical trace (frames
        // agree positionally), equal maximum (179), shared hole.
        let probe = control.clone();
        let outcomes = vec![
            probe_outcome("KeyZ", readiness, &probe),
            control_outcome(readiness, &control),
            control_outcome(readiness, &control),
        ];

        let surface = aggregate(DEFAULT_CONTROL_KEY, &outcomes, None, None);

        let keyz = surface.keys.iter().find(|k| k.key == "KeyZ").unwrap();
        assert_eq!(keyz.effect, None, "no divergence was observed");
        assert_eq!(
            keyz.confidence,
            KeyConfidence::Unclassified,
            "equal maxima with a shared interior gap is NOT trusted coverage"
        );
        assert!(surface.notes.iter().any(|n| n.contains("coverage caveat")));
    }

    #[test]
    fn frame_shift_with_equal_maxima_yields_divergence_not_vacuous_inert() {
        // Positional complement of the shared-gap case, pinning the
        // fail-safe direction: the probe skips frame 149, the
        // control skips 151 — equal maxima, sequences aligned
        // nowhere past 148. `diff_trace` treats frame
        // MISALIGNMENT as divergence, so the key classifies as
        // EFFECTIVE (a possible false positive — but never a
        // trusted negative over unobserved frames). Either way,
        // the pair can never yield an `Exact` inert.
        let readiness = point("readiness", 60);
        let control = {
            let mut t = flat(180);
            t.remove(151);
            t
        };
        let probe = {
            let mut t = flat(180);
            t.remove(149);
            t
        };
        let outcomes = vec![
            probe_outcome("KeyZ", readiness, &probe),
            control_outcome(readiness, &control),
            control_outcome(readiness, &control),
        ];

        let surface = aggregate(DEFAULT_CONTROL_KEY, &outcomes, None, None);

        let keyz = surface.keys.iter().find(|k| k.key == "KeyZ").unwrap();
        assert_ne!(
            (keyz.effect.is_none(), keyz.confidence),
            (true, KeyConfidence::Exact),
            "a misaligned pair must never yield an Exact inert: {:?}",
            keyz
        );
    }

    #[test]
    fn short_trace_with_a_divergence_stays_effective() {
        // The backstop must not eat POSITIVE evidence: a divergence
        // observed inside the compared span is valid regardless of
        // coverage symmetry (the frames after the short trace end
        // can only have ADDED evidence, not removed it).
        let readiness = point("readiness", 60);
        let control = flat(180);
        let mut short_probe = flat(101);
        short_probe[70].1 += 1; // diverges at frame 70
        let outcomes = vec![
            probe_outcome("KeyB", readiness, &short_probe),
            control_outcome(readiness, &control),
            control_outcome(readiness, &control),
        ];

        let surface = aggregate(DEFAULT_CONTROL_KEY, &outcomes, None, None);

        let keyb = surface.keys.iter().find(|k| k.key == "KeyB").unwrap();
        assert_eq!(
            keyb.effect,
            Some(super::super::diff::Effect {
                first_divergence_frame_delta: 11
            })
        );
        assert_eq!(keyb.confidence, KeyConfidence::Exact);
        assert!(!surface.notes.iter().any(|n| n.contains("coverage caveat")));
    }

    #[test]
    fn empty_traces_never_support_any_verdict() {
        // The old maxima check refused empty traces via the `None`
        // match arm; the sequence rewrite keeps the refusal explicit:
        // an empty probe OR control trace supports no verdict at all
        // — an all-empty pair (which a set/length comparison could
        // call "identical") must stay UNTRUSTED.
        let readiness = point("readiness", 60);
        let outcomes = vec![
            probe_outcome("KeyZ", readiness, &[]),
            control_outcome(readiness, &[]),
            control_outcome(readiness, &[]),
        ];

        let surface = aggregate(DEFAULT_CONTROL_KEY, &outcomes, None, None);

        let keyz = surface.keys.iter().find(|k| k.key == "KeyZ").unwrap();
        assert_eq!(
            keyz.confidence,
            KeyConfidence::Unclassified,
            "an empty pair must never yield a trusted verdict"
        );
    }

    #[test]
    fn every_surface_carries_the_baseline_caveat_note() {
        // The control-key MASKING risk is a property of the method
        // (the baseline is itself a key-press run), not of any
        // particular matrix — so the caveat must be surfaced even
        // when the control key is NOT among the probed candidates
        // and the determinism gate PASSED. Without it, a consumer
        // would read the inert findings as unconditionally trusted
        // negatives, which the method cannot guarantee.
        let readiness = point("readiness", 60);
        let control = flat(180);
        let mut digit1_probe = flat(180);
        digit1_probe[60].1 += 1;
        let outcomes = vec![
            probe_outcome("Digit1", readiness, &digit1_probe),
            probe_outcome("KeyZ", readiness, &flat(180)),
            control_outcome(readiness, &control),
            control_outcome(readiness, &control),
        ];

        let surface = aggregate(DEFAULT_CONTROL_KEY, &outcomes, None, None);

        assert_eq!(surface.confidence, SurfaceConfidence::Exact);
        let caveat = surface
            .notes
            .iter()
            .find(|n| n.starts_with("baseline caveat:"))
            .expect("the masking caveat must be surfaced on every surface");
        assert!(
            caveat.contains(&format!("'{DEFAULT_CONTROL_KEY}'")),
            "caveat names the control key: {caveat}"
        );
        // This matrix never probed KeyQ, so the separate
        // probed-control-key note must NOT fire.
        assert!(!surface
            .notes
            .iter()
            .any(|n| n.contains("is the control key")));
    }

    #[test]
    fn a_key_effective_only_at_the_later_point_stays_effective() {
        // State-gated key: inert at readiness (matches control),
        // divergent at mid. A first-point `None` must not stick.
        let readiness = point("readiness", 60);
        let mid = point("mid", 90);
        let control = flat(180);
        let mut mid_probe = flat(180);
        mid_probe[95].1 += 1;
        let outcomes = vec![
            probe_outcome("KeyE", readiness, &control),
            probe_outcome("KeyE", mid, &mid_probe),
            control_outcome(readiness, &control),
            control_outcome(readiness, &control),
            control_outcome(mid, &control),
            control_outcome(mid, &control),
        ];

        let surface = aggregate(DEFAULT_CONTROL_KEY, &outcomes, None, None);

        let key_e = surface.keys.iter().find(|k| k.key == "KeyE").unwrap();
        // Press at 90, first mismatch at 95 → delta 6.
        assert_eq!(
            key_e.effect,
            Some(super::super::diff::Effect {
                first_divergence_frame_delta: 6
            })
        );
        assert_eq!(surface.probed_at, vec![60, 90]);
    }

    #[test]
    fn control_divergence_flags_statistical_but_does_not_abort() {
        let readiness = point("readiness", 60);
        let mut control_b = flat(180);
        control_b[100].1 += 7;
        let mut digit1_probe = flat(180);
        digit1_probe[60].1 += 1;
        let outcomes = vec![
            probe_outcome("Digit1", readiness, &digit1_probe),
            control_outcome(readiness, &flat(180)), // baseline
            control_outcome(readiness, &control_b), // gate FAILS
        ];

        let surface = aggregate(DEFAULT_CONTROL_KEY, &outcomes, None, None);

        assert_eq!(surface.confidence, SurfaceConfidence::Statistical);
        let note = surface
            .notes
            .iter()
            .find(|n| n.starts_with("determinism gate FAILED"))
            .expect("gate failure must be surfaced in notes");
        assert!(note.contains("frame 100"), "note carries the frame: {note}");
        // The effect is still reported (vs the baseline control), just
        // marked statistical — never aborted, never silently trusted.
        let digit1 = surface.keys.iter().find(|k| k.key == "Digit1").unwrap();
        assert!(digit1.effect.is_some());
    }

    #[test]
    fn group_completion_applies_over_aggregated_findings() {
        let readiness = point("readiness", 60);
        let control = flat(180);
        let mut keyw_probe = flat(180);
        keyw_probe[61].1 += 1;
        let outcomes = vec![
            probe_outcome("KeyW", readiness, &keyw_probe),
            probe_outcome("KeyA", readiness, &control),
            probe_outcome("KeyS", readiness, &control),
            probe_outcome("KeyD", readiness, &control),
            control_outcome(readiness, &control),
            control_outcome(readiness, &control),
        ];

        let surface = aggregate(DEFAULT_CONTROL_KEY, &outcomes, None, None);

        // Cluster siblings: raw records preserved, upgraded via the
        // interpretation layer.
        assert_eq!(surface.confidence_for("KeyW"), Some(KeyConfidence::Exact));
        for sibling in ["KeyA", "KeyS", "KeyD"] {
            assert_eq!(
                surface.confidence_for(sibling),
                Some(KeyConfidence::Inferred),
                "{sibling} must be group-completed"
            );
            let finding = surface.keys.iter().find(|k| k.key == sibling).unwrap();
            assert_eq!(finding.effect, None, "no effect invented");
        }
        assert_eq!(surface.effective_key_count(), 1);
    }

    #[test]
    fn controls_only_matrix_yields_empty_surface() {
        // Degenerate but legal: no candidates → the gate still runs.
        let readiness = point("readiness", 60);
        let control = flat(180);
        let outcomes = vec![
            control_outcome(readiness, &control),
            control_outcome(readiness, &control),
        ];

        let surface = aggregate(DEFAULT_CONTROL_KEY, &outcomes, None, None);

        assert!(surface.keys.is_empty());
        assert_eq!(surface.confidence, SurfaceConfidence::Exact);
        assert_eq!(surface.probed_at, vec![60]);
    }

    #[test]
    fn duplicate_control_copies_cannot_shadow_the_baseline() {
        // The FIRST control of a point is the baseline; a divergent
        // second copy must flag statistical, not flip the baseline.
        let readiness = point("readiness", 60);
        let mut control_b = flat(180);
        control_b[10].1 += 3;
        let outcomes = vec![
            control_outcome(readiness, &control_b), // baseline (odd one out)
            control_outcome(readiness, &flat(180)),
            control_outcome(readiness, &flat(180)),
        ];

        let surface = aggregate(DEFAULT_CONTROL_KEY, &outcomes, None, None);

        assert_eq!(
            surface.confidence,
            SurfaceConfidence::Statistical,
            "2-of-3 disagree: gate must fire"
        );
    }

    #[test]
    fn matching_truncated_pair_with_expected_span_is_unclassified() {
        // Copilot #58 round 13, "reject matching but incomplete
        // traces in public aggregation": the pairwise backstop
        // (`coverage_mismatch`) proves SYMMETRY, not completeness
        // — probe and control both intact, contiguous, equal
        // length — so a matching-but-truncated pair sailed through
        // as a trusted `Exact` inert even though the run was
        // supposed to record frames 0..=179 and both stopped at
        // 100. Passing the plan's expected span must catch it:
        // the shared span falls short, the inert downgrades to
        // Unclassified, and the coverage caveat names the key.
        let readiness = point("readiness", 60);
        // Both sides stop at frame 100 of a 180-frame plan.
        let truncated = flat(101);
        let outcomes = vec![
            probe_outcome("KeyZ", readiness, &truncated),
            control_outcome(readiness, &truncated),
            control_outcome(readiness, &truncated),
        ];

        let surface = aggregate(DEFAULT_CONTROL_KEY, &outcomes, None, Some(179));

        let keyz = surface.keys.iter().find(|k| k.key == "KeyZ").unwrap();
        assert_eq!(keyz.effect, None, "no divergence was observed");
        assert_eq!(
            keyz.confidence,
            KeyConfidence::Unclassified,
            "a symmetric pair short of the expected span is NOT a trusted negative: {:?}",
            keyz
        );
        assert!(
            surface.notes.iter().any(|n| n.contains("coverage caveat")),
            "the coverage caveat must be surfaced: {:?}",
            surface.notes
        );
        assert!(
            surface.notes.iter().any(|n| n.contains("expected span")),
            "the caveat must say the pair fell short of the expected span: {:?}",
            surface.notes
        );
    }

    #[test]
    fn matching_truncated_pair_with_legacy_none_keeps_exact() {
        // The `None` contract is unchanged: direct callers with no
        // plan knowledge get the pre-parameter behavior — a
        // SYMMETRIC pair (intact, contiguous, equal length) is
        // trusted, symmetric-but-short included. The upgrade path
        // is opt-in per call; existing behavior must not flip.
        let readiness = point("readiness", 60);
        let truncated = flat(101);
        let outcomes = vec![
            probe_outcome("KeyZ", readiness, &truncated),
            control_outcome(readiness, &truncated),
            control_outcome(readiness, &truncated),
        ];

        let surface = aggregate(DEFAULT_CONTROL_KEY, &outcomes, None, None);

        let keyz = surface.keys.iter().find(|k| k.key == "KeyZ").unwrap();
        assert_eq!(keyz.effect, None);
        assert_eq!(
            keyz.confidence,
            KeyConfidence::Exact,
            "None = legacy contract: symmetric pairs stay trusted"
        );
    }

    #[test]
    fn full_pair_with_expected_span_stays_exact() {
        // Positive control for the new parameter: a pair that
        // actually reaches the plan's expected final frame is
        // unaffected — inert stays `Exact`, effective stays
        // `Exact`, and no coverage caveat appears. Passing
        // `Some(span)` must not accidentally tighten every verdict
        // into unclassified.
        let readiness = point("readiness", 60);
        let control = flat(180);
        let mut digit1_probe = flat(180);
        digit1_probe[60].1 += 1;
        let outcomes = vec![
            probe_outcome("Digit1", readiness, &digit1_probe),
            probe_outcome("KeyZ", readiness, &control),
            control_outcome(readiness, &control),
            control_outcome(readiness, &control),
        ];

        let surface = aggregate(DEFAULT_CONTROL_KEY, &outcomes, None, Some(179));

        let digit1 = surface.keys.iter().find(|k| k.key == "Digit1").unwrap();
        assert_eq!(digit1.confidence, KeyConfidence::Exact);
        let keyz = surface.keys.iter().find(|k| k.key == "KeyZ").unwrap();
        assert_eq!(keyz.effect, None, "genuinely inert");
        assert_eq!(
            keyz.confidence,
            KeyConfidence::Exact,
            "full-span inert stays trusted with the expected span supplied"
        );
        assert!(
            !surface.notes.iter().any(|n| n.contains("coverage caveat")),
            "no caveat for full-coverage pairs: {:?}",
            surface.notes
        );
    }

    #[test]
    fn expected_span_boundary_is_inclusive_and_caller_supplied() {
        // Boundary pins for the span check, both directions:
        //
        // - INCLUSIVE: a trace of exactly `n + 1` frames (last
        //   frame == expected span) is FULL coverage — the plan's
        //   own runs are exactly this long, so an off-by-one here
        //   would unclassify every legitimate inert verdict.
        // - CALLER-SUPPLIED: the span must come from the ARGUMENT.
        //   A check that ignored the parameter (e.g. a hardcoded
        //   length) would flip this test's verdicts: the traces
        //   here cover 0..=59, so they satisfy `Some(59)` but are
        //   short of `Some(179)`.
        let readiness = point("readiness", 30);
        let short = flat(60); // frames 0..=59
        let outcomes = vec![
            probe_outcome("KeyZ", readiness, &short),
            control_outcome(readiness, &short),
            control_outcome(readiness, &short),
        ];

        // Satisfied span: inert stays trusted.
        let exact = aggregate(DEFAULT_CONTROL_KEY, &outcomes, None, Some(59));
        let keyz = exact.keys.iter().find(|k| k.key == "KeyZ").unwrap();
        assert_eq!(
            keyz.confidence,
            KeyConfidence::Exact,
            "last frame == expected span is full coverage (inclusive)"
        );

        // Unsatisfied span, same traces: inert downgrades.
        let short_of = aggregate(DEFAULT_CONTROL_KEY, &outcomes, None, Some(179));
        let keyz = short_of.keys.iter().find(|k| k.key == "KeyZ").unwrap();
        assert_eq!(
            keyz.confidence,
            KeyConfidence::Unclassified,
            "the span is the CALLER's argument — these 0..=59 traces are \
             short of 0..=179 and must not be trusted"
        );
    }

    #[test]
    fn span_beyond_usize_width_fails_open_not_closed() {
        // Overflow pin (defensive on 64-bit, load-bearing on
        // 32-bit): an expected span that cannot be represented as
        // a usize must make the pair UNCOVERED (fail open — no
        // verdict), never wrap into a small bound that licenses a
        // short trace as full coverage. With a truncating `as`
        // cast, a span like 2^32 + 59 on a 32-bit target wraps to
        // 59 and `len(60) > 59` would WRONGLY accept these
        // 0..=59 traces; the checked cast refuses before comparing.
        let readiness = point("readiness", 30);
        let sixty = flat(60); // frames 0..=59
        let outcomes = vec![
            probe_outcome("KeyZ", readiness, &sixty),
            control_outcome(readiness, &sixty),
            control_outcome(readiness, &sixty),
        ];

        let surface = aggregate(
            DEFAULT_CONTROL_KEY,
            &outcomes,
            None,
            Some(u64::MAX), // saturates the comparison on 64-bit, hits
                            // the overflow branch on 32-bit
        );

        let keyz = surface.keys.iter().find(|k| k.key == "KeyZ").unwrap();
        assert_eq!(
            keyz.confidence,
            KeyConfidence::Unclassified,
            "an unrepresentable expected span must fail OPEN: the pair \
             cannot be proven to cover it"
        );
    }

    #[test]
    fn probe_without_matching_control_is_dropped_with_a_loud_note() {
        // Direct-caller backstop: a probe outcome referencing a
        // point with NO control run can support no verdict. It
        // must not appear as a silent omission — the surface
        // names the victim keys in a note so a malformed matrix
        // is visible (refusal over silence). discover() can never
        // produce this shape; this pins the public-API contract.
        let orphan_point = point("orphan", 60);
        let orphan_probe = flat(180);
        // Well-formed half: readiness point HAS its controls.
        let readiness = point("readiness", 60);
        let control = flat(180);
        let outcomes = vec![
            probe_outcome("KeyZ", readiness, &control),
            probe_outcome("KeyQ2", orphan_point, &orphan_probe),
            control_outcome(readiness, &control),
            control_outcome(readiness, &control),
        ];

        let surface = aggregate(DEFAULT_CONTROL_KEY, &outcomes, None, None);

        // The orphaned probe contributed no finding...
        assert!(
            !surface.keys.iter().any(|k| k.key == "KeyQ2"),
            "an unmatched-point probe must not enter the surface: {:?}",
            surface.keys
        );
        // ...but the omission is LOUD, not silent.
        let note = surface
            .notes
            .iter()
            .find(|n| n.starts_with("unmatched probe point"))
            .expect("the unmatched-point note must be surfaced");
        assert!(note.contains("KeyQ2"), "note names the key: {note}");
        assert!(
            note.contains("no verdict is possible"),
            "note explains the consequence: {note}"
        );
        // The well-formed half is unaffected.
        let keyz = surface.keys.iter().find(|k| k.key == "KeyZ").unwrap();
        assert_eq!(keyz.confidence, KeyConfidence::Exact);
    }
}
