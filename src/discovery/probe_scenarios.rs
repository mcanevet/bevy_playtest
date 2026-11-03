//! Probe scenario generator (discovery Phase 2).
//!
//! Builds the run matrix for input-surface probing out of plain
//! [`Scenario`] values:
//!
//! - **Probe runs:** `synthetic_keyboard` tapping ONE candidate key at
//!   ONE probe point (one point per run — an early press would mutate
//!   state before later points are reached, destroying attribution;
//!   see design doc "One Probe Point Per RUN").
//! - **Control runs:** the normalized pair partner — the SAME
//!   synthetic bot pressing a control key at the SAME frame with the
//!   SAME `hold_frames`. Probe and control differ by exactly one
//!   bit: which key. (There is no idle bot in the schema, so the
//!   control must be a real — but normalized — key press.)
//!
//! Every generated scenario opts into `record_digest: true` (the
//! per-frame opaque u64 series is the diff input) and runs
//! `single_threaded` with simulated time at a fixed `tps`, so probe
//! and control traces are frame-aligned.

use crate::enums::BotType;
use crate::scenario::{validate_scenario, BotConfig, KeyPressInput, Scenario, ScenarioError};

/// Simulated ticks per second for probe runs. Frames in generated
/// scenarios index into this fixed rate (default matches the schema's
/// `tps` default).
pub const PROBE_TPS: u32 = 60;

/// Probe run duration (seconds). Short on purpose: probes only need
/// enough runway to observe the divergence after a tap (the spike
/// validated 3s against the slowest-loading fixture).
pub const PROBE_DURATION_S: f32 = 3.0;

/// Tap variant: hold for exactly 1 frame (the only MVP variant — a
/// tap is the harder case for a game to miss, and hold-only
/// semantics stay covered fail-open; see design doc).
pub const TAP_HOLD_FRAMES: u64 = 1;

/// Frames the game allegedly needs before being ready to accept
/// input; the readiness probe point sits this far past startup.
/// Zero for the MVP: the readiness point is simply "1s in" and
/// moves with `tps`.
pub const PRE_READY_FRAMES: u64 = 0;

/// Default control key. `KeyQ` is inert for the majority of menu /
/// title screens (WASD+arrows games rarely bind Q outside gameplay),
/// but ANY choice can collide with a binding. The double-control
/// determinism gate bounds the damage: a control that DOES affect
/// the game only errs toward keeping keys (fail-open), never toward
/// pruning. The control key is deliberately EXCLUDED from
/// [`PROBE_CANDIDATE_KEYS`]: a probe of the control against itself is
/// byte-identical to its baseline and could only ever yield a
/// vacuous "inert" verdict — if a bespoke matrix probes it anyway,
/// the flow marks the finding `Unclassified`.
pub const DEFAULT_CONTROL_KEY: &str = "KeyQ";

/// The prior-likelihood keyboard candidate set (~26-60 keys, design
/// doc "Candidates"): WASD, arrows, digits, navigation cluster,
/// modifiers, and common action letters. Emitted as canonical
/// `KeyCode` spellings accepted verbatim by `normalize_key_name`.
/// Deliberately does NOT contain [`DEFAULT_CONTROL_KEY`] — the
/// control cannot classify itself (see there).
pub const PROBE_CANDIDATE_KEYS: &[&str] = &[
    // Movement cluster.
    "KeyW",
    "KeyA",
    "KeyS",
    "KeyD",
    "ArrowUp",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
    // Digits (menu slots, debug keys, weapon select).
    "Digit0",
    "Digit1",
    "Digit2",
    "Digit3",
    "Digit4",
    "Digit5",
    "Digit6",
    "Digit7",
    "Digit8",
    "Digit9",
    // Navigation cluster.
    "Space",
    "Enter",
    "Escape",
    "Tab",
    "Backspace",
    // Modifiers.
    "ShiftLeft",
    "ControlLeft",
    "AltLeft",
    // Common action letters. (KeyQ lives here in spirit, but it IS
    // the default control key — a self-probe cannot classify, so it
    // is excluded; see DEFAULT_CONTROL_KEY.)
    "KeyE",
    "KeyF",
    "KeyR",
];

/// A probe point: WHERE in the run the tap happens (one per run).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProbePoint {
    /// Short label for scenario names and reports ("readiness",
    /// "mid").
    pub label: &'static str,
    /// Frame index (0-based ticks since run start) of the tap.
    pub frame: u64,
}

/// The two MVP probe points (Strategy A, third point dropped — see
/// design doc "Accounting"):
///
/// - **Readiness:** `pre_ready_frames + tps` (≈1s after startup;
///   covers menu/title listeners).
/// - **Mid-duration:** `duration_s / 2 * tps` (covers
///   post-transition listeners).
pub fn probe_points(duration_s: f32, tps: u32) -> [ProbePoint; 2] {
    let readiness = PRE_READY_FRAMES + tps as u64;
    let mid = ((duration_s / 2.0) * tps as f32) as u64;
    [
        ProbePoint {
            label: "readiness",
            frame: readiness.max(1),
        },
        ProbePoint {
            label: "mid",
            frame: mid.max(readiness + 1),
        },
    ]
}

/// Total ticks of a run lasting `duration_s` at `tps` (the same
/// ceiling `validate_scenario` applies to authored scenarios).
/// Digests are recorded once per tick at frames `0..=ticks-1` (see
/// the driver's tick loop), so a COMPLETE run's last digest frame
/// is `ticks - 1` — surfaced via [`ProbePlan::expected_last_frame`].
fn total_ticks(duration_s: f32, tps: u32) -> u64 {
    (duration_s * tps as f32).ceil() as u64
}

/// Configuration of the probe run matrix. Cheap to construct;
/// `Default` covers the design-doc MVP.
#[derive(Clone, Debug)]
pub struct ProbePlan {
    /// Candidate keys to probe, in canonical spelling.
    pub candidate_keys: Vec<String>,
    /// Simulated ticks per second of every generated scenario.
    pub tps: u32,
    /// Duration (seconds) of every generated scenario.
    pub duration_s: f32,
    /// Control key pressed by control runs (mirrors probe presses).
    pub control_key: String,
}

impl Default for ProbePlan {
    fn default() -> Self {
        Self {
            candidate_keys: PROBE_CANDIDATE_KEYS.iter().map(|s| s.to_string()).collect(),
            tps: PROBE_TPS,
            duration_s: PROBE_DURATION_S,
            control_key: DEFAULT_CONTROL_KEY.to_string(),
        }
    }
}

impl ProbePlan {
    /// Last digest frame a COMPLETE run of this plan records:
    /// `ceil(duration_s * tps) - 1` (one digest per tick at frames
    /// `0..=ticks-1`). Snapshot once at job-build time — the pool
    /// refuses any report whose trace stops short of it, so a
    /// crashed or truncated run can never be read as an inert key.
    pub fn expected_last_frame(&self) -> u64 {
        total_ticks(self.duration_s, self.tps).saturating_sub(1)
    }

    /// The plan with every key name reduced to its CANONICAL
    /// spelling: control key and candidates alike. The driver
    /// already normalizes aliases at delivery time (`w`, `digit1`,
    /// `Space` all press the same physical key), but everything
    /// DOWNSTREAM of delivery — job identity, plan fingerprint,
    /// aggregation's `key == control_key` self-probe guard —
    /// compares the RAW spellings. An aliased control key (`"q"`)
    /// pressing the same physical key as a canonical candidate
    /// (`"KeyQ"`) would diff a probe run against a baseline built
    /// from the SAME input, find the guaranteed `None`, and emit it
    /// as a trusted `Exact` negative — the vacuous self-probe the
    /// `Unclassified` guard exists to prevent, defeated by spelling.
    ///
    /// Canonicalization happens ONCE, at the flow's front door
    /// (`discover`), so the fingerprint, the job list, the surface
    /// findings, and the self-probe guard all see one vocabulary.
    /// Non-canonical spellings that `normalize_key_name` rejects are
    /// a setup error, not a silently kept raw name: silently keeping
    /// the alias would reintroduce the split vocabulary this method
    /// exists to close. Duplicate canonical forms collapse
    /// (pressing `KeyQ` twice probes nothing the second time).
    pub fn canonicalized(&self) -> Result<Self, ScenarioError> {
        let canonical = |name: &str| {
            crate::scenario::normalize_key_name(name).ok_or_else(|| {
                ScenarioError::Rejected(format!(
                    "unknown key name '{name}' in the probe plan — use a \
                     canonical key name or alias (e.g. KeyW, Digit1, Space)"
                ))
            })
        };
        let control_key = canonical(&self.control_key)?.to_string();
        // The self-probe guard needs the control key PRESENT-or-not
        // spelled exactly like the candidates will be, but the
        // guard itself runs on whatever vocabulary arrives: a plan
        // with control "KeyQ" and NO KeyQ candidate is fine, and a
        // plan with control "q" aliasing INTO a KeyQ candidate is
        // exactly what this method repairs. Canonicalize both sides
        // from the same table, then let the guard see the result.
        let mut candidate_keys: Vec<String> = Vec::with_capacity(self.candidate_keys.len());
        for key in &self.candidate_keys {
            let c = canonical(key)?;
            if !candidate_keys.iter().any(|k| k == c) {
                candidate_keys.push(c.to_string());
            }
        }
        Ok(Self {
            candidate_keys,
            control_key,
            tps: self.tps,
            duration_s: self.duration_s,
        })
    }

    /// The plan's probe points, VALIDATED to land inside the run: a
    /// point past the last tick would produce a tap that never
    /// fires, silently degrading the whole matrix into unattributable
    /// no-ops. A degenerate plan is rejected here, not silently run.
    pub fn points(&self) -> Result<[ProbePoint; 2], ScenarioError> {
        let points = probe_points(self.duration_s, self.tps);
        let ticks = total_ticks(self.duration_s, self.tps);
        for point in points {
            // Tap frame + hold must fit strictly inside the run (a
            // press AT the final tick can never be observed).
            if point.frame + TAP_HOLD_FRAMES >= ticks {
                return Err(ScenarioError::Rejected(format!(
                    "probe point '{}' at frame {} does not fit a {}s run at \
                     {} tps ({} ticks) — shorten readiness or lengthen the \
                     duration",
                    point.label, point.frame, self.duration_s, self.tps, ticks
                )));
            }
        }
        Ok(points)
    }

    fn tap_press(frame: u64, key: &str) -> KeyPressInput {
        KeyPressInput {
            frame,
            key: key.to_string(),
            hold_frames: TAP_HOLD_FRAMES,
        }
    }

    /// Base scenario every probe/control run shares: deterministic
    /// (fixed seed, simulated time, single-threaded), digest
    /// recording ON (the diff input), replay verification OFF
    /// (the double-control run IS the determinism gate).
    fn base_scenario(&self, name: String) -> Scenario {
        Scenario {
            name: Some(name),
            bot: BotConfig {
                bot_type: BotType::SyntheticKeyboard,
                seed: 42,
                input_rate_hz: 10,
                key_presses: vec![],
                mouse_clicks: vec![],
                raw_surface: None,
            },
            tps: self.tps,
            simulated_time: true,
            single_threaded: true,
            strict_warnings: false,
            duration_s: self.duration_s,
            invariants: vec![],
            oracles: None,
            liveness: Default::default(),
            verify_replay: false,
            record_digest: true,
        }
    }

    /// The probe scenario for `key` tapped at `point`.
    pub fn probe_scenario(&self, key: &str, point: ProbePoint) -> Result<Scenario, ScenarioError> {
        let mut scenario = self.base_scenario(format!("probe_{key}_{}", point.label));
        scenario.bot.key_presses = vec![Self::tap_press(point.frame, key)];
        validate_scenario(&scenario)?;
        Ok(scenario)
    }

    /// The normalized control scenario for `point`: identical to the
    /// probe runs except the pressed key is the control key.
    pub fn control_scenario(&self, point: ProbePoint) -> Result<Scenario, ScenarioError> {
        let mut scenario = self.base_scenario(format!("probe_control_{}", point.label));
        scenario.bot.key_presses = vec![Self::tap_press(point.frame, &self.control_key)];
        validate_scenario(&scenario)?;
        Ok(scenario)
    }

    /// The full probe-run list: every candidate key × every probe
    /// point, one tap per run.
    pub fn probe_runs(&self) -> Result<Vec<Scenario>, ScenarioError> {
        let points = self.points()?;
        let mut runs = Vec::with_capacity(self.candidate_keys.len() * points.len());
        for key in &self.candidate_keys {
            for point in points {
                runs.push(self.probe_scenario(key, point)?);
            }
        }
        Ok(runs)
    }

    /// The control runs: one per probe point. These run TWICE each
    /// (double-control determinism gate) — the caller schedules the
    /// duplicates.
    pub fn control_runs(&self) -> Result<Vec<Scenario>, ScenarioError> {
        let points = self.points()?;
        points.iter().map(|p| self.control_scenario(*p)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::{normalize_key_name, validate_scenario};
    // The matrix-builder plumbing of the pool (gated behind
    // `takeover` alongside the module itself — it is a pure function
    // over the plan, but its home does not exist without the
    // feature, and an ungated import would break
    // `--no-default-features` builds).
    #[cfg(feature = "takeover")]
    use crate::discovery::probe_pool::{build_job_list, ProbeError};

    #[test]
    fn probe_points_follow_the_design_doc_formula() {
        // readiness = pre_ready_frames + tps; mid = duration/2 * tps.
        let [readiness, mid] = probe_points(PROBE_DURATION_S, PROBE_TPS);
        assert_eq!(readiness.label, "readiness");
        assert_eq!(readiness.frame, PRE_READY_FRAMES + PROBE_TPS as u64);
        assert_eq!(mid.label, "mid");
        assert_eq!(
            mid.frame,
            (PROBE_DURATION_S / 2.0 * PROBE_TPS as f32) as u64
        );
        // Mid must sit strictly AFTER readiness (distinct regimes).
        assert!(mid.frame > readiness.frame);
    }

    #[test]
    fn tiny_durations_still_produce_ordered_points() {
        // Degenerate configs must not produce mid <= readiness.
        let [r, m] = probe_points(0.5, 10);
        assert_eq!(r.frame, 10);
        assert!(m.frame > r.frame);
        let [r0, _] = probe_points(0.05, 10);
        assert!(r0.frame >= 1, "points must never tap at frame 0");
    }

    #[test]
    fn points_that_do_not_fit_the_run_are_rejected() {
        // The raw formula stays permissive (pure math); the PLAN
        // rejects configurations whose taps would never fire. A
        // 0.15s run at 60 tps = 9 ticks, but readiness sits at
        // frame 60: every tap would land past the run end and every
        // probe would silently read as inert.
        let plan = ProbePlan {
            duration_s: 0.15,
            ..Default::default()
        };
        let err = plan.points().unwrap_err().to_string();
        assert!(
            err.contains("does not fit"),
            "must explain the misfit: {err}"
        );
        assert!(plan.probe_runs().is_err());
        assert!(plan.control_runs().is_err());
        // And the pool's matrix builder surfaces it as a plan error
        // (takeover builds only — the pool spawns subprocesses, so
        // its module lives behind the feature).
        #[cfg(feature = "takeover")]
        assert!(matches!(build_job_list(&plan), Err(ProbeError::Plan(_))));

        // Sanity the other way: the DEFAULT plan fits comfortably.
        let default_plan = ProbePlan::default();
        let ticks = total_ticks(default_plan.duration_s, default_plan.tps);
        for point in default_plan.points().unwrap() {
            assert!(point.frame + TAP_HOLD_FRAMES < ticks);
        }
    }

    #[test]
    fn candidate_keys_are_canonical_and_unique() {
        let mut seen = std::collections::HashSet::new();
        for key in PROBE_CANDIDATE_KEYS {
            // Round-trips through the schema's own normalization: the
            // generator must emit names verbatim (design doc: "the
            // candidate generator should emit canonical names").
            assert_eq!(
                normalize_key_name(key),
                Some(*key),
                "candidate '{key}' is not a canonical key name"
            );
            assert!(seen.insert(*key), "duplicate candidate key {key}");
        }
        // The control key must never be a candidate: probing it
        // against itself is structurally unclassifiable.
        assert!(
            !PROBE_CANDIDATE_KEYS.contains(&DEFAULT_CONTROL_KEY),
            "the control key must not probe itself"
        );
        assert!(PROBE_CANDIDATE_KEYS.len() >= 26);
    }

    #[test]
    fn canonicalized_collapses_aliases_to_one_vocabulary() {
        // Copilot r4468466364: the driver normalizes aliases at
        // delivery, but everything downstream (job identity, plan
        // fingerprint, the aggregation `key == control_key`
        // self-probe guard) compared RAW spellings — a control key
        // "q" and a candidate "KeyQ" press the SAME physical key,
        // so their diff is guaranteed `None` yet the spellings
        // differ and the vacuous negative was emitted as `Exact`.
        // `canonicalized` reduces the whole plan to one vocabulary.
        let plan = ProbePlan {
            control_key: "q".to_string(),
            candidate_keys: vec![
                "KeyQ".to_string(), // alias of the control key
                "w".to_string(),
                "digit1".to_string(),
                "Digit1".to_string(), // duplicate under aliasing
                "Space".to_string(),  // already canonical
            ],
            ..Default::default()
        };
        let canon = plan.canonicalized().expect("aliases must canonicalize");
        assert_eq!(canon.control_key, "KeyQ");
        // Collapsed to canonical, deduplicated, in first-seen order.
        // Note KeyQ STAYS in the list: canonicalization is not
        // allowed to change the probe matrix (that would silently
        // shrink the surface) — the aggregation self-probe guard
        // takes over from here and labels it Unclassified.
        assert_eq!(
            canon.candidate_keys,
            vec!["KeyQ", "KeyW", "Digit1", "Space"]
        );
        // The self-probe guard now sees the collision the raw
        // spellings hid.
        assert!(
            canon.candidate_keys.contains(&canon.control_key),
            "the aliased control key must be visible as a candidate collision"
        );
    }

    #[test]
    fn canonicalized_rejects_unknown_names() {
        // Silently keeping an unknown raw name would reopen the
        // split vocabulary; it is a setup error instead.
        let bad_candidate = ProbePlan {
            candidate_keys: vec!["Nonsense".to_string()],
            ..Default::default()
        };
        assert!(bad_candidate.canonicalized().is_err());

        let bad_control = ProbePlan {
            control_key: "NotAKey".to_string(),
            ..Default::default()
        };
        assert!(bad_control.canonicalized().is_err());
    }

    #[test]
    fn default_plan_is_already_canonical_and_unchanged() {
        // Idempotence guard: the default plan (and any hand-written
        // canonical plan) must pass through byte-for-byte so
        // canonicalization never perturbs healthy setups — and so
        // the cache fingerprint stays stable.
        let plan = ProbePlan::default();
        let canon = plan
            .canonicalized()
            .expect("default plan must canonicalize");
        assert_eq!(canon.candidate_keys, plan.candidate_keys);
        assert_eq!(canon.control_key, plan.control_key);
        assert_eq!(canon.tps, plan.tps);
        assert_eq!(canon.duration_s, plan.duration_s);
    }

    #[test]
    fn probe_scenarios_validate_against_the_real_schema() {
        let plan = ProbePlan::default();
        for run in plan.probe_runs().unwrap() {
            // probe_runs already validates; re-validate to assert the
            // contract explicitly and cover round-tripping.
            validate_scenario(&run).expect("generated probe scenario must validate");
        }
        for run in plan.control_runs().unwrap() {
            validate_scenario(&run).expect("generated control scenario must validate");
        }
    }

    #[test]
    fn probe_and_control_differ_by_exactly_the_key() {
        // The normalized-pair contract: same frame, hold, bot type,
        // seed, rate, tps, duration, determinism knobs — the ONLY
        // difference is which key is pressed.
        let plan = ProbePlan::default();
        let [readiness, _mid] = probe_points(plan.duration_s, plan.tps);
        let probe = plan.probe_scenario("Digit1", readiness).unwrap();
        let control = plan.control_scenario(readiness).unwrap();

        let [p] = &probe.bot.key_presses[..] else {
            panic!("probe run must press exactly one key");
        };
        let [c] = &control.bot.key_presses[..] else {
            panic!("control run must press exactly one key");
        };
        assert_eq!(p.frame, c.frame, "press frame must match");
        assert_eq!(p.hold_frames, c.hold_frames, "hold must match");
        assert_eq!(p.key, "Digit1");
        assert_eq!(c.key, DEFAULT_CONTROL_KEY);

        assert_eq!(probe.bot.bot_type, control.bot.bot_type);
        assert_eq!(probe.bot.seed, control.bot.seed);
        assert_eq!(probe.bot.input_rate_hz, control.bot.input_rate_hz);
        assert_eq!(probe.tps, control.tps);
        assert_eq!(probe.duration_s, control.duration_s);
        assert_eq!(probe.simulated_time, control.simulated_time);
        assert_eq!(probe.single_threaded, control.single_threaded);
        assert_eq!(probe.verify_replay, control.verify_replay);
        assert_eq!(probe.record_digest, control.record_digest);
    }

    #[test]
    fn every_generated_run_records_digests_single_tap() {
        let plan = ProbePlan::default();
        // Digest recording is the diff input; exactly one press per
        // run is the independence requirement.
        for run in plan.probe_runs().unwrap() {
            assert!(run.record_digest, "probe run must record digests");
            assert_eq!(run.bot.key_presses.len(), 1, "one tap per probe run");
            assert_eq!(run.bot.key_presses[0].hold_frames, TAP_HOLD_FRAMES);
            assert!(run.single_threaded);
        }
        for run in plan.control_runs().unwrap() {
            assert!(run.record_digest, "control run must record digests");
            assert_eq!(run.bot.key_presses.len(), 1);
        }
    }

    #[test]
    fn matrix_size_matches_candidate_keys_times_two_points() {
        let plan = ProbePlan::default();
        let probes = plan.probe_runs().unwrap();
        let controls = plan.control_runs().unwrap();
        assert_eq!(probes.len(), PROBE_CANDIDATE_KEYS.len() * 2);
        assert_eq!(controls.len(), 2, "one control per probe point");
        // Scenario names embed the key and point label for attribution.
        let names: Vec<&str> = probes.iter().map(|r| r.name.as_deref().unwrap()).collect();
        assert!(names.contains(&"probe_Digit1_readiness"));
        assert!(names.contains(&"probe_Digit1_mid"));
        let control_names: Vec<&str> = controls
            .iter()
            .map(|r| r.name.as_deref().unwrap())
            .collect();
        assert!(control_names.contains(&"probe_control_readiness"));
        assert!(control_names.contains(&"probe_control_mid"));
    }
}
