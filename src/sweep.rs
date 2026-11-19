//! Seed sweep + fingerprint regression gate.
//!
//! A single-seed battery verdict is LUCK-DEPENDENT (the flagship
//! `mines` fixture famously passes seed 42 and fails 3 of 5 sampled
//! seeds: its menu escapes only via a phase-precise click). The
//! sweep runs the same scenario across many seeds, deduplicates
//! failures by finding FINGERPRINT, remembers known fingerprints in
//! a plain-text regression file, and gates only on NEW or REGRESSED
//! findings — giving CI memory across runs instead of a stateless
//! per-seed coin flip.
//!
//! # Manual repro
//!
//! Every sweep member is reproducible by hand: the sweep drives the
//! child through [`crate::battery::BatteryConfig::seed_override`],
//! which the takeover honors over `scenario.bot.seed` via
//! `BEVY_PLAYTEST_SEED`. A finding at seed 43 is reproduced with
//! `BEVY_PLAYTEST_SCENARIO=battery BEVY_PLAYTEST_SEED=43 cargo run
//! --features playtest --bin <game>` (the seed var alone is inert —
//! the takeover needs its scenario to take over at all).

use crate::battery::{self, BatteryConfig, BatteryError, DEFAULT_TIMEOUT_SECS};
use crate::driver::is_progress_informational_public;
use crate::enums::PlaytestStatus;
use crate::rules::Rule;
use crate::state::{PlaytestReport, ViolationEntry};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

// ---------------------------------------------------------------------------
// Fingerprint
// ---------------------------------------------------------------------------

/// Default seed set: 42 (the default-seed canonical case) plus four
/// spread samples. Deliberately small — the sweep is a CI-budget
/// feature, and the regression gate (not seed count) is what gives
/// CI memory.
pub const DEFAULT_SWEEP_SEEDS: [u64; 5] = [42, 43, 99, 1234, 2718];

/// Upper bound on parallel members. Each member is a full battery
/// subprocess with its own render context; above this, CI hosts
/// thrash.
pub const MAX_PARALLEL: usize = 8;

/// A finding's identity across seeds and runs: rule + target.
/// Frames, counts, and details are stripped — they vary per seed and
/// would fragment fingerprints (the `first_frame` pinning that
/// `ExpectFinding::first_frame_in` exists to handle manually is
/// exactly the variance being dropped here).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FindingFingerprint {
    pub rule: String,
    pub target: String,
}

impl FindingFingerprint {
    /// Fingerprint a blocking violation. Informational rules never
    /// gate and never fingerprint — a `log_warn` storm is noise, not
    /// a known-bug ledger entry.
    pub fn of_blocking(v: &ViolationEntry) -> Option<Self> {
        let rule = Rule::from_name(&v.rule)?;
        if rule.is_informational() {
            return None;
        }
        Some(Self {
            rule: v.rule.clone(),
            target: canonical_target(&v.rule, &v.target),
        })
    }

    /// Stable textual form for the regression file: `rule:target`.
    /// Human-readable beats compact — the file is edited by hand and
    /// reviewed in diffs. Neither field may contain `#` (the file's
    /// comment syntax — see [`RegressionDb`]), and
    /// [`Self::from_key`] enforces it, so a key can never be silently
    /// truncated into a comment.
    ///
    /// Targets containing `:` are safe (module-path targets like
    /// `my_game::physics`): the RULE field is always a colon-free
    /// snake_case rule name, so splitting on the FIRST colon is
    /// unambiguous.
    pub fn key(&self) -> String {
        format!("{}:{}", self.rule, self.target)
    }

    /// Inverse of [`FindingFingerprint::key`]. Rejects keys that
    /// could not round-trip through the file format: empty fields,
    /// `#` (comment syntax), newlines, and whitespace at either
    /// field's edge — a stray space after the colon (`rule: x`)
    /// parses but never equals a canonical key, the worst failure
    /// shape: a recorded finding that still reports NEW forever,
    /// silently. Internal whitespace is fine (`canonical_target`
    /// only trims edges).
    ///
    /// The target is CANONICALIZED for the entity-targeted rules: a
    /// hand-written raw entity target (`nodes_in_bounds:3v1`) is
    /// valid text but can never EQUAL the `entity` fingerprint the
    /// gate compares — normalization here makes it match instead of
    /// silently muting it.
    pub fn from_key(key: &str) -> Option<Self> {
        if key.contains('\n') || key.contains('\r') {
            return None;
        }
        let (rule, target) = key.split_once(':')?;
        if rule.is_empty() || target.is_empty() {
            return None;
        }
        if rule.contains('#') || target.contains('#') {
            return None;
        }
        if rule.trim() != rule || target.trim() != target {
            return None;
        }
        Some(Self {
            rule: rule.to_string(),
            target: canonical_target(rule, target),
        })
    }
}

/// Canonicalize a violation target for fingerprinting: collapse
/// volatile substrings that vary across runs/seeds without changing
/// the finding's identity. The volatile shape TODAY:
///
/// - Bevy entity debug targets — `{index}v{generation}`, e.g. `3v1`
///   (index and generation vary with spawn/despawn order across
///   seeds) — reported by the global `nodes_in_bounds` /
///   `finite_transforms` oracles. They collapse to `entity`. Which
///   entity is preserved in the finding's `detail` (the repro
///   anchor); the finding's IDENTITY is "the rule fired on some
///   entity" — the same escaped entity on two seeds is ONE finding,
///   not two. Note the merge grain: these global oracles query ALL
///   entities, so their per-entity targets carry no identity
///   anyway; two distinct out-of-bounds bugs surface as one
///   `nodes_in_bounds:entity` finding and share an adjudication.
///
/// Extend conservatively: a wrong collapse merges two different
/// findings, which breaks the gate's core promise.
fn canonical_target(rule: &str, target: &str) -> String {
    let trimmed = target.trim();
    // Restrict the collapse to the two GLOBAL oracles that report
    // per-ENTITY targets (`oracles.rs`: `format!("{entity:?}")` for
    // nodes_in_bounds / finite_transforms). Authored invariants
    // NAME a rule but choose free-form targets (the invariant name
    // is the target in `invariant_eval`) — scenario validation
    // REJECTS entity-shaped or `entity`-named nodes_in_bounds
    // invariants precisely so this collapse can never swallow them
    // (see `validate_scenario`): a game-defined target that merely
    // LOOKS entity-like cannot reach this code for those rules.
    if rule != Rule::NodesInBounds.name() && rule != Rule::FiniteTransforms.name() {
        return trimmed.to_string();
    }
    // Bevy's Entity debug format (bevy_ecs 0.20):
    // "{index}v{generation}", e.g. `3v1` (index: u32,
    // generation: u32 — no `#bits` suffix; PLACEHOLDER only in
    // sparse-set debug builds). Both halves must be non-empty
    // ASCII digits. Exact shape match, so near-miss targets never
    // collapse.
    let is_entity_debug = trimmed.split_once('v').is_some_and(|(idx, gen)| {
        !idx.is_empty()
            && !gen.is_empty()
            && idx.bytes().all(|b| b.is_ascii_digit())
            && gen.bytes().all(|b| b.is_ascii_digit())
    });
    if is_entity_debug {
        "entity".to_string()
    } else {
        trimmed.to_string()
    }
}

// ---------------------------------------------------------------------------
// Regression DB
// ---------------------------------------------------------------------------

/// Plain-text, human-editable, git-mergeable known-fingerprints file
/// (proptest-regressions style). One line per known finding:
///
/// ```text
/// # bevy_playtest seed-sweep regression fingerprints
/// # <fingerprint-key>  # <optional human note>
/// frozen_world:world  # mines menu soft-lock (pre-existing, adjudicated)
/// ```
///
/// Blank lines and `#`-prefixed lines are ignored. A malformed line
/// (hand-edit typo) is tolerated on load — it must not brick CI —
/// but is dropped from the known set, so the finding it garbled
/// reports as NEW on the next run (loud, never silent).
#[derive(Default)]
pub struct RegressionDb {
    entries: BTreeMap<String, String>, // key -> note
}

impl RegressionDb {
    /// Load from a path; a missing file is an empty DB (fresh game,
    /// nothing known yet).
    pub fn load(path: &Path) -> std::io::Result<Self> {
        let mut entries = BTreeMap::new();
        if !path.exists() {
            return Ok(Self { entries });
        }
        for line in std::fs::read_to_string(path)?.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            // Trailing comment: `key # note`.
            let (body, note) = match line.split_once('#') {
                Some((b, n)) => (b.trim(), n.trim().to_string()),
                None => (line, String::new()),
            };
            let Some(fp) = FindingFingerprint::from_key(body) else {
                continue; // malformed: tolerated, dropped (see type docs)
            };
            // Store the CANONICAL key: the gate compares against
            // canonicalized fingerprints, so a hand-written raw
            // entity target (`nodes_in_bounds:3v1`) must load as
            // `nodes_in_bounds:entity` or it never matches anything.
            entries.insert(fp.key(), note);
        }
        Ok(Self { entries })
    }

    /// All known fingerprint keys (sorted — BTreeMap).
    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.entries.keys()
    }

    pub fn contains(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    /// Insert a known fingerprint with a note. Keys that could not
    /// round-trip (rejected by [`FindingFingerprint::from_key`] —
    /// `#`, newline, edge-whitespace anomalies) are REFUSED: an
    /// entry that `load` would silently drop is an entry whose
    /// finding re-gates as NEW forever. The key is stored in
    /// CANONICAL form, so hand-written raw entity targets match the
    /// same identity the gate compares. Returns whether the key was
    /// accepted. Do not bypass with field access —
    /// `FindingFingerprint`'s fields are `pub`, but an invalid key
    /// reaching [`Self::store`] self-destructs on the next load.
    pub fn insert(&mut self, key: String, note: String) -> bool {
        let Some(fp) = FindingFingerprint::from_key(&key) else {
            return false;
        };
        self.entries.insert(fp.key(), note);
        true
    }

    /// Write the file format back out.
    pub fn store(&self, path: &Path) -> std::io::Result<()> {
        let mut text = String::from(
            "# bevy_playtest seed-sweep regression fingerprints\n\
             # One line per known finding fingerprint. Blank lines and lines\n\
             # starting with '#' are ignored. Delete a line when its finding is\n\
             # fixed; the sweep then reports it as new on the next run.\n",
        );
        for (key, note) in &self.entries {
            if note.is_empty() {
                text.push_str(key);
            } else {
                text.push_str(&format!("{key}  # {note}"));
            }
            text.push('\n');
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, text)
    }
}

// ---------------------------------------------------------------------------
// Sweep configuration and report
// ---------------------------------------------------------------------------

/// Sweep configuration. NOT `Default` — `binary` has no sensible
/// default (same contract as [`BatteryConfig`]). Builder methods
/// consume and return `Self` for `#[test]` ergonomics.
#[derive(Debug, Clone)]
pub struct SweepConfig {
    /// Feature-enabled game binary (`env!("CARGO_BIN_EXE_<game>")`).
    pub binary: String,
    /// Scenario path or [`battery::BUILTIN_BATTERY`].
    pub scenario: String,
    /// Seeds to run; empty uses [`DEFAULT_SWEEP_SEEDS`].
    pub seeds: Vec<u64>,
    /// Worker count for parallel members (clamped to 1..=[MAX_PARALLEL]).
    pub parallel: usize,
    /// Regression DB path. `None` = ungated sweep (report-only).
    pub regressions_path: Option<PathBuf>,
    /// Per-member wall-clock timeout (seconds).
    pub timeout_secs: u64,
}

impl SweepConfig {
    /// Builtin battery, default seeds, default parallelism (4).
    pub fn new(binary: impl Into<String>) -> Self {
        Self {
            binary: binary.into(),
            scenario: battery::BUILTIN_BATTERY.to_string(),
            seeds: DEFAULT_SWEEP_SEEDS.to_vec(),
            parallel: 4,
            regressions_path: None,
            timeout_secs: DEFAULT_TIMEOUT_SECS,
        }
    }

    pub fn scenario(mut self, scenario: impl Into<String>) -> Self {
        self.scenario = scenario.into();
        self
    }

    pub fn seeds(mut self, seeds: Vec<u64>) -> Self {
        self.seeds = seeds;
        self
    }

    pub fn parallel(mut self, parallel: usize) -> Self {
        self.parallel = parallel;
        self
    }

    pub fn regressions(mut self, path: impl Into<PathBuf>) -> Self {
        self.regressions_path = Some(path.into());
        self
    }

    /// Seeds the sweep actually runs: the defaults when none are
    /// authored, else the authored list SORTED ascending — members
    /// (and the serialized report) are seed-ordered regardless of
    /// authored order, and the first contributor of a fingerprint
    /// carries the lowest-seed detail.
    fn effective_seeds(&self) -> Vec<u64> {
        if self.seeds.is_empty() {
            DEFAULT_SWEEP_SEEDS.to_vec()
        } else {
            let mut seeds = self.seeds.clone();
            seeds.sort_unstable();
            seeds.dedup();
            seeds
        }
    }
}

/// One member run's serialized outcome.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SweepMember {
    pub seed: u64,
    /// Final report status on a completed run (`Pass`/`Fail`/`Crash`),
    /// or an infrastructure-error message (spawn/timeout/no-report —
    /// infrastructure failures do NOT fingerprint; they fail the
    /// sweep outright, like they fail a single battery run).
    pub status: Result<String, String>,
    /// Digest of the recorded bot draw stream on a completed run.
    /// Present so callers (and tests) can verify the seed override
    /// actually reached the bot WITHOUT re-running members by hand:
    /// members sweeping different seeds must show different digests
    /// (equal digests would mean the override never traveled and
    /// the sweep degenerated into N copies of one run). Digests are
    /// comparable WITHIN one process only (std hasher; not stable
    /// across Rust versions — never persist them). `None` on
    /// infrastructure failure.
    pub draw_digest: Option<u64>,
}

/// A blocking finding aggregated across seeds.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SweepFinding {
    pub fingerprint_key: String,
    /// Seeds that produced this finding (ascending).
    pub seeds: Vec<u64>,
    /// Lowest-seed member's finding detail (canonical repro anchor).
    pub detail: String,
}

/// Gate classification against the regression DB.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SweepVerdict {
    /// No blocking findings on any seed.
    Clean,
    /// Every blocking finding's fingerprint is known (expected,
    /// non-blocking). Known findings that did NOT recur are reported
    /// as healed notes.
    Known,
    /// At least one fingerprint is unknown — new or regressed.
    /// This is the blocking verdict.
    NewOrRegressed,
}

/// Result of a sweep.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SweepReport {
    /// Member outcomes, seed-ascending.
    pub members: Vec<SweepMember>,
    /// Blocking findings aggregated by fingerprint.
    pub findings: Vec<SweepFinding>,
    /// Known fingerprints that did NOT recur on any seed — likely
    /// fixed; each suggests deleting its line so it gates again.
    pub healed: Vec<String>,
    /// Seeds whose run FAILED yet produced no fingerprintable
    /// blocking finding — typically an escalated informational rule
    /// (e.g. `progress_surface_empty` under
    /// `progress.require_surface`, `log_warn` under
    /// `strict_warnings`). The gate blocks on these — with or
    /// without a regression file: an unrecordable gating failure is
    /// never a clean verdict. Such findings cannot enter the
    /// regression file (there is no key); fix the game or relax the
    /// scenario instead.
    pub unfingerprinted_failures: Vec<u64>,
    /// Seeds whose run CRASHED. Crashes always gate — a crashed run
    /// certifies nothing, regardless of known fingerprints, and
    /// REGARDLESS of whether a regression file is configured.
    pub crashed_seeds: Vec<u64>,
    /// Gate classification. `None` only when no regression file is
    /// configured AND no member crashed or failed with an
    /// unrecordable gating finding (a fully clean report-only
    /// sweep); red members force `Some(NewOrRegressed)` even
    /// without a DB.
    pub verdict: Option<SweepVerdict>,
    /// Whether the `NewOrRegressed` verdict (if any) was driven by
    /// actually-unknown fingerprints, as opposed to red members
    /// (crashes / unrecordable failures) gating on their own
    /// account. Private, derived: the summary phrases the gate line
    /// from it so the printed cause matches the machine's decision.
    has_new_findings: bool,
}

impl SweepReport {
    /// Human-readable summary for a failing `#[test]` panic.
    pub fn summary(&self, config: &SweepConfig) -> String {
        let mut s = String::from("seed sweep findings:\n");
        for m in &self.members {
            match &m.status {
                Ok(status) => s.push_str(&format!("  seed {}: {status}\n", m.seed)),
                Err(e) => s.push_str(&format!("  seed {}: INFRASTRUCTURE ERROR: {e}\n", m.seed)),
            }
        }
        for f in &self.findings {
            s.push_str(&format!(
                "  finding [{}] on seeds {:?}\n    {}\n",
                f.fingerprint_key, f.seeds, f.detail
            ));
        }
        for h in &self.healed {
            s.push_str(&format!(
                "  healed: {h} no longer reproduces — delete its line from the regression file so it gates again\n"
            ));
        }
        if !self.crashed_seeds.is_empty() {
            s.push_str(&format!(
                "  crashed seeds {:?}: a crashed run certifies nothing and always gates\n",
                self.crashed_seeds
            ));
        }
        if !self.unfingerprinted_failures.is_empty() {
            s.push_str(&format!(
                "  seeds {:?} failed with NO fingerprintable finding — typically an escalated informational rule \
                 (progress_surface_empty under progress.require_surface, log_warn under strict_warnings); \
                 fix the game or relax the scenario\n",
                self.unfingerprinted_failures
            ));
        }
        match &self.verdict {
            Some(SweepVerdict::NewOrRegressed) => {
                // Name the machine's ACTUAL decision: crashes and
                // unrecordable failures gate the sweep on their own
                // account — "unknown fingerprints" would misstate
                // the cause when every recorded finding was known.
                if self.has_new_findings {
                    s.push_str("  gate: NEW OR REGRESSED findings (unknown fingerprints)\n")
                } else if !self.crashed_seeds.is_empty()
                    && !self.unfingerprinted_failures.is_empty()
                {
                    s.push_str(
                        "  gate: crashed members AND unrecordable failures (all recorded findings known)\n",
                    )
                } else if !self.crashed_seeds.is_empty() {
                    s.push_str("  gate: crashed members (all recorded findings known)\n")
                } else {
                    s.push_str(
                        "  gate: unrecordable failed members (all recorded findings known)\n",
                    )
                }
            }
            Some(SweepVerdict::Known) => s.push_str("  gate: all findings known (non-blocking)\n"),
            Some(SweepVerdict::Clean) => s.push_str("  gate: clean\n"),
            None => s.push_str("  gate: none (ungated sweep)\n"),
        }
        // Paste-ready recording block: the developer workflow after
        // adjudicating a finding is copying keys into the regression
        // file, so print them bare (one per line, no prose) — a
        // pasted sentence would be dropped by the loader as a
        // malformed line and the finding would re-gate forever.
        if !self.findings.is_empty() {
            s.push_str(
                "  to record adjudicated findings, add these lines to the regression file:\n",
            );
            for f in &self.findings {
                s.push_str(&format!("  {}  # TODO note\n", f.fingerprint_key));
            }
        }
        s.push_str(&format!(
            "  reproduce any member: BEVY_PLAYTEST_SCENARIO={scenario} BEVY_PLAYTEST_SEED=<seed> {bin}\n",
            scenario = config.scenario,
            // Works for both shapes: a bare game binary name
            // ("cargo run --bin <name>") and an absolute path (the
            // test-built CARGO_BIN_EXE_* form — run the path
            // directly; it is already built).
            bin = if std::path::Path::new(&config.binary).exists() {
                config.binary.clone()
            } else {
                format!("cargo run --features playtest --bin {}", config.binary)
            }
        ));
        s
    }
}

// ---------------------------------------------------------------------------
// Sweep execution
// ---------------------------------------------------------------------------

/// Bounded parallel map: run `f(0..n)` on `workers` scoped threads,
/// returning outcomes in INDEX order (completion order is irrelevant
/// — indices define the report's member order, and the slots write
/// one index each, so the `Mutex` never contends). Workers claim
/// indices via a shared atomic counter.
fn run_pool<T: Send>(n: usize, workers: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    let slots: Vec<Mutex<Option<T>>> = (0..n).map(|_| Mutex::new(None)).collect();
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..workers.min(n) {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if i >= n {
                    break;
                }
                *slots[i].lock().unwrap() = Some(f(i));
            });
        }
    });
    slots
        .into_iter()
        .map(|s| {
            s.into_inner()
                .expect("pool slot filled")
                .expect("worker set slot")
        })
        .collect()
}

/// One member's complete outcome: the lean serialized member plus
/// the full report when the run completed (needed for fingerprint
/// aggregation; dropped from the serialized [`SweepReport`]).
struct MemberOutcome {
    member: SweepMember,
    report: Option<PlaytestReport>,
}

/// Run one sweep member (a full battery run with a seed override).
fn run_member(config: &SweepConfig, seed: u64) -> MemberOutcome {
    let result = battery::run_with_config(&BatteryConfig {
        binary: config.binary.clone(),
        scenario: config.scenario.clone(),
        timeout_secs: config.timeout_secs,
        seed_override: Some(seed),
    });
    match result {
        Ok(report) => {
            let status = report.status.to_string();
            let draw_digest = Some(draw_stream_digest(&report.choices));
            MemberOutcome {
                member: SweepMember {
                    seed,
                    status: Ok(status),
                    draw_digest,
                },
                report: Some(report),
            }
        }
        Err(e) => MemberOutcome {
            member: SweepMember {
                seed,
                status: Err(e.to_string()),
                draw_digest: None,
            },
            report: None,
        },
    }
}

/// Within-process digest of a draw stream (see
/// [`SweepMember::draw_digest`]): the round-tripped JSON array
/// hashed as bytes, so it captures every recorded field without
/// requiring `Hash` on [`crate::choice::Choice`]. Std hasher; no
/// stability guarantees across Rust versions — never persist it.
fn draw_stream_digest(choices: &[crate::choice::Choice]) -> u64 {
    use std::hash::{Hash, Hasher};
    let json = serde_json::to_string(choices).expect("choices serialize");
    let mut h = std::hash::DefaultHasher::new();
    json.hash(&mut h);
    h.finish()
}

/// The escalation knobs of the scenario a sweep member's CHILD will
/// load, re-derived parent-side so the gate replicates the child's
/// contextual gating (`is_progress_informational`) without drift:
/// the built-in battery literal implies `require_surface: true`
/// (see `plugin::battery_scenario`); a scenario FILE is parsed and
/// its authored knobs honored. Scan layering writes VARIANT files
/// derived from the built-in battery, preserving its knobs — so the
/// flags hold there too.
fn sweep_scenario_flags(config: &SweepConfig) -> ScenarioFlags {
    if config.scenario != battery::BUILTIN_BATTERY {
        if let Ok(bytes) = std::fs::read_to_string(&config.scenario) {
            if let Ok(scenario) = serde_json::from_str::<crate::scenario::Scenario>(&bytes) {
                return ScenarioFlags {
                    strict_warnings: scenario.strict_warnings,
                    require_surface: scenario.liveness.progress.require_surface,
                };
            }
        }
        // Unreadable scenario: the child will CONFIG_ERROR and the
        // member surfaces as infrastructure failure. Pass through
        // permissive defaults so the gate's analysis never SPURIOUSLY
        // blocks on flags it could not derive.
        return ScenarioFlags::default();
    }
    // Single source of truth: derive the battery literal's knobs from
    // the same constructor the CHILD parses (`plugin::
    // battery_scenario`) — the parent must replicate the child's
    // contextual gating, so re-typing the knobs here would invite
    // silent drift (a changed battery literal silently
    // misclassifying violations). Failure falls back to permissive
    // defaults, same doctrine as the unreadable-file arm above.
    crate::plugin::battery_scenario()
        .map(|s| ScenarioFlags {
            strict_warnings: s.strict_warnings,
            require_surface: s.liveness.progress.require_surface,
        })
        .unwrap_or_default()
}

/// Contextual escalation knobs the child applies when deciding
/// which violations gate (see `driver::is_progress_informational`).
#[derive(Clone, Copy, Default)]
struct ScenarioFlags {
    strict_warnings: bool,
    require_surface: bool,
}

/// Recover the typed [`PlaytestStatus`] from the member's stringified
/// status (children report lowercase `pass`/`fail`/`crash`; unknown
/// strings are treated as failures, never passes).
fn typed_status(status: &Result<String, String>) -> Option<PlaytestStatus> {
    status.as_ref().ok().and_then(|s| match s.as_str() {
        "pass" => Some(PlaytestStatus::Pass),
        "fail" => Some(PlaytestStatus::Fail),
        "crash" => Some(PlaytestStatus::Crash),
        _ => None,
    })
}

/// Run the sweep: bounded worker pool over seeds (completion order
/// never defines member order — `seeds` order does), each member a
/// full battery subprocess inheriting temp-dir isolation, env
/// stripping and RAII cleanup from [`battery::run_with_config`].
/// Blocks until every member settles (member timeouts enforce the
/// bound; workers cannot hang past them).
pub fn run_sweep(config: &SweepConfig) -> Result<SweepReport, BatteryError> {
    let seeds = config.effective_seeds();
    let parallel = config.parallel.clamp(1, MAX_PARALLEL);
    let outcomes: Vec<MemberOutcome> =
        run_pool(seeds.len(), parallel, |i| run_member(config, seeds[i]));

    // Infrastructure failures fail the sweep outright: a member that
    // could not run says nothing about the game — same contract as a
    // single battery run. Findings aggregate only from completed
    // members.
    let mut infra: Vec<String> = Vec::new();
    for o in &outcomes {
        if let Err(e) = &o.member.status {
            infra.push(format!("seed {}: {e}", o.member.seed));
        }
    }
    if !infra.is_empty() {
        return Err(BatteryError::Sweep(format!(
            "sweep member infrastructure failure(s): [{}]",
            infra.join("; ")
        )));
    }

    // Aggregate blocking findings by fingerprint. Members iterate
    // seed-ascending (`effective_seeds` sorts), so the FIRST report
    // contributing a fingerprint carries the lowest-seed detail.
    // Informational rules are skipped by
    // `FindingFingerprint::of_blocking`.
    let mut by_fp: BTreeMap<String, SweepFinding> = BTreeMap::new();
    for o in &outcomes {
        let report = o.report.as_ref().expect("infra handled above");
        for v in &report.violations {
            let Some(fp) = FindingFingerprint::of_blocking(v) else {
                continue;
            };
            let key = fp.key();
            by_fp
                .entry(key)
                .and_modify(|f| {
                    if !f.seeds.contains(&o.member.seed) {
                        f.seeds.push(o.member.seed);
                    }
                })
                .or_insert_with(|| SweepFinding {
                    fingerprint_key: fp.key(),
                    seeds: vec![o.member.seed],
                    detail: v.detail.clone(),
                });
        }
    }
    let findings: Vec<SweepFinding> = {
        let mut fs: Vec<SweepFinding> = by_fp.into_values().collect();
        for f in &mut fs {
            f.seeds.sort_unstable();
        }
        fs
    };

    // Gate against the regression DB when one is configured.
    //
    // B2 guard: the child's gating is CONTEXTUAL —
    // `is_progress_informational` escalates `log_warn` under
    // `strict_warnings` and `progress_surface_empty` under
    // `progress.require_surface` — but the fingerprint filter
    // (`Rule::is_informational`) is context-free. A failed member
    // whose ONLY violations are escalated informationals
    // fingerprints as nothing and must not slip through as clean.
    //
    // The parent replicates the child's classifier by re-deriving
    // the escalation knobs from the SAME scenario the child loads
    // (battery literal / scenario file), so the two cannot drift.
    // A member is "covered" only when EVERY violation the child
    // treated as gating corresponds to a fingerprintable finding —
    // a `any()`-style check would let a known `frozen_world`
    // silently mask an un-adjudicable escalated `log_warn` on the
    // same member (composite member).
    let scenario_flags = sweep_scenario_flags(config);
    let mut unfingerprinted_failures: Vec<u64> = Vec::new();
    let mut crashed: Vec<u64> = Vec::new();
    for o in &outcomes {
        let status = typed_status(&o.member.status).expect("infra handled above");
        if status == PlaytestStatus::Crash {
            // A crashed run certifies nothing, however the violation
            // bookkeeping shook out (same doctrine as
            // `run_and_expect_scoped_findings`): always gates.
            // Counted ONLY here — a crash with no fingerprintable
            // finding would otherwise be double-listed under
            // unrecordable failures too, and both lists describe
            // the same red member either way.
            crashed.push(o.member.seed);
        } else if status != PlaytestStatus::Pass {
            let gating_uncovered = o.report.as_ref().is_some_and(|r| {
                r.violations.iter().any(|v| {
                    !is_progress_informational_public(
                        &v.rule,
                        scenario_flags.strict_warnings,
                        scenario_flags.require_surface,
                    ) && FindingFingerprint::of_blocking(v).is_none()
                })
            });
            if gating_uncovered {
                unfingerprinted_failures.push(o.member.seed);
            }
        }
    }
    // Verdict. Crashed members and failed members with unrecordable
    // gating findings (escalated informationals) force RED regardless
    // of whether a regression file is configured: neither is a
    // findings-memory question the DB could answer, and a report-only
    // sweep passing silently while every member is red would
    // contradict "a crashed run certifies nothing". With a DB, red
    // members coexist with DB classification (NewOrRegressed wins).
    let red_members = !crashed.is_empty() || !unfingerprinted_failures.is_empty();
    let (verdict, healed, has_new_findings) = match &config.regressions_path {
        None => (
            if red_members {
                Some(SweepVerdict::NewOrRegressed)
            } else {
                None
            },
            Vec::new(),
            false,
        ),
        Some(path) => {
            let db = RegressionDb::load(path)
                .map_err(|e| BatteryError::Sweep(format!("regression db unreadable: {e}")))?;
            let mut new = false;
            for f in &findings {
                if !db.contains(&f.fingerprint_key) {
                    new = true;
                }
            }
            let healed: Vec<String> = db
                .keys()
                .filter(|k| !findings.iter().any(|f| &f.fingerprint_key == *k))
                .cloned()
                .collect();
            let verdict = if red_members || new {
                SweepVerdict::NewOrRegressed
            } else if findings.is_empty() {
                SweepVerdict::Clean
            } else {
                SweepVerdict::Known
            };
            (Some(verdict), healed, new)
        }
    };

    Ok(SweepReport {
        members: outcomes.into_iter().map(|o| o.member).collect(),
        findings,
        healed,
        unfingerprinted_failures,
        crashed_seeds: crashed,
        verdict,
        has_new_findings,
    })
}

/// Gate entry point for `#[test]` bodies: run the sweep, consult the
/// regression DB (when configured), and panic only on NEW or
/// REGRESSED findings — including members that failed with no
/// fingerprintable finding (escalated informational rules) or
/// crashed outright (crashes gate even without a regression file).
/// Known findings pass with a printed note; healed known
/// fingerprints pass with a suggestion to delete their lines
/// (so a future regression of the SAME bug gates again — the
/// regression file is the bug ledger).
///
/// Composes with — never replaces — the other battery entry points:
/// a game with adjudicated per-scenario expectations keeps
/// `run_and_expect_scoped_findings`; this is the fleet-level memory
/// layer on top. Expected/scoped findings can be pre-seeded into the
/// regression file via their rule+target pairs instead.
pub fn run_sweep_and_gate(config: &SweepConfig) -> SweepReport {
    let report = match run_sweep(config) {
        Ok(r) => r,
        Err(e) => panic!("{e}"),
    };
    match &report.verdict {
        Some(SweepVerdict::NewOrRegressed) => panic!("{}", report.summary(config)),
        _ => {
            // Silent only when there is nothing to say: healed keys
            // or unrecordable failed members both warrant a print,
            // even on a passing gate (the developer should know the
            // gate is passing on borrowed evidence).
            if !report.healed.is_empty() || !report.unfingerprinted_failures.is_empty() {
                println!("{}", report.summary(config));
            }
            report
        }
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_key_roundtrip() {
        let fp = FindingFingerprint {
            rule: "frozen_world".into(),
            target: "world".into(),
        };
        assert_eq!(fp.key(), "frozen_world:world");
        assert_eq!(FindingFingerprint::from_key(&fp.key()), Some(fp));
        // Malformed keys are rejected, not guessed at.
        assert_eq!(FindingFingerprint::from_key(""), None);
        assert_eq!(FindingFingerprint::from_key(":target"), None);
        assert_eq!(FindingFingerprint::from_key("rule:"), None);
        assert_eq!(FindingFingerprint::from_key("no-colon"), None);
    }

    #[test]
    fn regression_db_store_load_roundtrip() {
        let dir = std::env::temp_dir().join(format!(
            "bevy_playtest_sweep_db_{}_{}",
            std::process::id(),
            line!()
        ));
        let path = dir.join("regressions.txt");
        let mut db = RegressionDb::default();
        db.insert("frozen_world:world".into(), "menu soft-lock".into());
        db.insert("panic_recovery:physics".into(), String::new());
        db.store(&path).expect("store");
        let loaded = RegressionDb::load(&path).expect("load");
        assert!(loaded.contains("frozen_world:world"));
        assert!(loaded.contains("panic_recovery:physics"));
        let keys: Vec<&String> = loaded.keys().collect();
        assert_eq!(keys.len(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn regression_db_missing_file_is_empty() {
        let dir = std::env::temp_dir().join(format!(
            "bevy_playtest_sweep_absent_{}_{}",
            std::process::id(),
            line!()
        ));
        let db = RegressionDb::load(&dir.join("nope.txt")).expect("missing file = empty db");
        assert_eq!(db.keys().count(), 0);
    }

    #[test]
    fn regression_db_tolerates_hand_edit_typos() {
        let dir = std::env::temp_dir().join(format!(
            "bevy_playtest_sweep_typo_{}_{}",
            std::process::id(),
            line!()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("regressions.txt");
        std::fs::write(
            &path,
            "# header comment\n\
             frozen_world:world  # good line with note\n\
             \n\
             a-typo-with-no-colon\n\
             :empty-rule\n\
             another_good:key\n",
        )
        .unwrap();
        let db = RegressionDb::load(&path).expect("load");
        assert_eq!(db.keys().count(), 2, "malformed lines dropped, good kept");
        assert!(db.contains("frozen_world:world"));
        assert!(db.contains("another_good:key"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn informational_rules_never_fingerprint() {
        let entry = ViolationEntry {
            rule: "log_warn".into(),
            target: "world".into(),
            ..Default::default()
        };
        assert!(FindingFingerprint::of_blocking(&entry).is_none());
        let entry = ViolationEntry {
            rule: "frozen_world".into(),
            target: "world".into(),
            ..Default::default()
        };
        assert_eq!(
            FindingFingerprint::of_blocking(&entry),
            Some(FindingFingerprint {
                rule: "frozen_world".into(),
                target: "world".into()
            })
        );
        // Unknown rule names (future/garbled) never fingerprint either.
        let entry = ViolationEntry {
            rule: "not_a_rule".into(),
            target: "world".into(),
            ..Default::default()
        };
        assert!(FindingFingerprint::of_blocking(&entry).is_none());
    }

    #[test]
    fn entity_debug_targets_collapse_to_stable_fingerprints() {
        // B1: the global nodes_in_bounds/finite_transforms oracles
        // report targets in Bevy's Entity debug format
        // "{index}v{generation}" (e.g. `3v1` — bevy_ecs 0.20 emits
        // no `#bits` suffix). Indices and generations vary across
        // seeds (spawn/despawn order differs with bot input), so the
        // RAW target would fragment one bug into N fingerprints. The
        // identity is "this rule fired on some entity"; the entity
        // itself lives on in `detail`.
        let mk = |target: &str| {
            FindingFingerprint::of_blocking(&ViolationEntry {
                rule: "nodes_in_bounds".into(),
                target: target.into(),
                ..Default::default()
            })
        };
        assert_eq!(
            mk("3v1").unwrap().key(),
            "nodes_in_bounds:entity",
            "entity debug target must collapse"
        );
        assert_eq!(
            mk("0v0").unwrap().key(),
            mk("123v2").unwrap().key(),
            "the same escaped-entity bug on two seeds is ONE finding"
        );
        // Large indices are legal (u32) and still collapse.
        assert_eq!(
            mk("4294967295v1").map(|f| f.target),
            Some("entity".into()),
            "u32-sized index collapses"
        );
        // Whitespace-trimmed targets still canonicalize (the trim
        // precedes the shape check); a non-entity target keeps its
        // trimmed text unchanged.
        assert_eq!(
            mk(" 3v1 ").unwrap().key(),
            "nodes_in_bounds:entity",
            "trim precedes the shape check"
        );
        assert_eq!(
            mk(" physics_joint ").unwrap().target,
            "physics_joint".to_string()
        );
        // Near-miss shapes never collapse: no index part, empty
        // generation, or non-digit fields mean the target is NOT the
        // entity debug format.
        assert_eq!(
            mk("v1").map(|f| f.target),
            Some("v1".into()),
            "no index part — not the entity debug shape, left intact"
        );
        assert_eq!(
            mk("3v").map(|f| f.target),
            Some("3v".into()),
            "empty generation is not the entity debug shape — left intact"
        );
        assert_eq!(
            mk("3vX").map(|f| f.target),
            Some("3vX".into()),
            "non-digit generation is not the entity debug shape"
        );
    }

    #[test]
    fn canonical_target_collapses_only_the_global_entity_oracles() {
        // The collapse is restricted to nodes_in_bounds /
        // finite_transforms: authored invariants may use free-form
        // targets, and an invariant literally NAMED like an entity
        // debug string must never lose its identity.
        let entry = |rule: &str, target: &str| {
            FindingFingerprint::of_blocking(&ViolationEntry {
                rule: rule.into(),
                target: target.into(),
                ..Default::default()
            })
        };
        assert_eq!(
            entry("panic", "3v1").map(|f| f.target),
            Some("3v1".into()),
            "panic targets are not entity-shaped for the canonicalizer"
        );
        // Second collapsing rule.
        assert_eq!(
            entry("finite_transforms", "17v3").map(|f| f.target),
            Some("entity".into())
        );
    }

    #[test]
    fn from_key_rejects_hash_and_comment_truncation() {
        // B1: `#` is the DB file's comment syntax. A key carrying it
        // (e.g. pasted from a report target before canonicalization)
        // would be silently truncated into a comment by the parser —
        // never matching anything. Reject loudly at from_key.
        assert_eq!(FindingFingerprint::from_key("nodes_in_bounds:3v1#17"), None);
        assert_eq!(
            FindingFingerprint::from_key("finite_trans#forms:entity"),
            None
        );
        // Newlines would split the entry across lines on store,
        // mangling the file. Edge whitespace parses but never
        // equals a canonical key (canonical_target trims), so a
        // recorded-yet-still-NEW line is rejected instead of
        // accepted-but-mute.
        assert_eq!(FindingFingerprint::from_key("frozen_world:\nworld"), None);
        assert_eq!(FindingFingerprint::from_key("frozen_world: x"), None);
        assert_eq!(FindingFingerprint::from_key(" frozen_world:x"), None);
        assert_eq!(FindingFingerprint::from_key("frozen_world:x "), None);
        // Colon-containing targets (module paths) remain legal: the
        // rule field is a colon-free snake_case name, so the first
        // colon is unambiguous.
        assert_eq!(
            FindingFingerprint::from_key("panic_recovery:my_game::physics"),
            Some(FindingFingerprint {
                rule: "panic_recovery".into(),
                target: "my_game::physics".into()
            })
        );
        // A HAND-WRITTEN raw entity target is accepted and
        // CANONICALIZED to the identity the gate compares — a valid
        // key that could never match must not load as a mute entry.
        assert_eq!(
            FindingFingerprint::from_key("nodes_in_bounds:3v1"),
            Some(FindingFingerprint {
                rule: "nodes_in_bounds".into(),
                target: "entity".into()
            })
        );
    }

    #[test]
    fn regression_db_insert_rejects_malformed_and_canonicalizes_raw_keys() {
        // H-4: `insert` is the WRITE path; refusing keys `load` would
        // silently drop prevents an entry from self-destructing on
        // the next load (recorded-yet-still-NEW). The round-trip is
        // the exact same predicate `load` applies.
        let mut db = RegressionDb::default();
        assert!(db.insert("frozen_world:world".into(), "ok".into()));
        // Raw-but-VALID entity keys are accepted and stored in
        // canonical form — the identity the gate actually compares.
        assert!(db.insert("nodes_in_bounds:3v1".into(), "raw".into()));
        assert!(db.contains("nodes_in_bounds:entity"));
        for bad in [
            "frozen_world:3v1#17",  // '#' comment syntax
            "bad key:no-newline\n", // embedded newline
            "frozen_world: x",      // edge whitespace (never canonical)
            ":empty-rule",
            "no-colon",
            "",
        ] {
            assert!(
                !db.insert(bad.into(), String::new()),
                "insert must refuse non-roundtripping key: {bad:?}"
            );
        }
        assert_eq!(db.keys().count(), 2, "valid keys stored, malformed refused");
        assert!(db.contains("frozen_world:world"));
    }

    #[test]
    fn effective_seeds_defaults_when_empty() {
        let cfg = SweepConfig::new("unused-binary").seeds(Vec::new());
        assert_eq!(cfg.effective_seeds(), DEFAULT_SWEEP_SEEDS.to_vec());
        let cfg = SweepConfig::new("unused-binary").seeds(vec![7]);
        assert_eq!(cfg.effective_seeds(), vec![7]);
        // Authored order is normalized: members are seed-ascending
        // (serialized report order and lowest-seed detail carrier)
        // regardless of how the author listed them. Duplicates are
        // collapsed — the same seed twice is one member, not a
        // doubled finding seed-list.
        let cfg = SweepConfig::new("unused-binary").seeds(vec![99, 43, 7, 43]);
        assert_eq!(cfg.effective_seeds(), vec![7, 43, 99]);
    }

    #[test]
    fn run_pool_preserves_index_order() {
        // Completion order != index order: simulate staggered work and
        // check outcomes land at their own indices.
        let out = run_pool(8, 4, |i| {
            // Reverse-stagger: later indices finish first.
            std::thread::sleep(std::time::Duration::from_millis((8 - i) as u64));
            i * 10
        });
        assert_eq!(out, vec![0, 10, 20, 30, 40, 50, 60, 70]);
    }
}
