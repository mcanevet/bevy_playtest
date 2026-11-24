//! The shared episode-fold engine (0.16 judgement-layer §7).
//!
//! Both judgement-layer vocabularies (spatial contracts and entity
//! stagnation) SELECT entities, ENUMERATE observations per frame,
//! and FOLD consecutive violating frames into EPISODES with
//! identity rules and bounded evidence. This module is that fold,
//! as a pure state machine over owned, bevy-free data:
//!
//! - the recorder (driver loop) supplies, per frame, the set of
//!   violating observations (a KEY plus a measured VALUE — pairwise
//!   feeds ordered entity-index pairs with the observed distance,
//!   and every vocabulary that measures per-entity severity feeds
//!   the entity index with its deviation) and a driver-clock
//!   sample;
//! - [`EpisodeFolder`] folds consecutive frames into a maximal
//!   stretch, tracks the worst observation and a bounded roster of
//!   distinct keys, and hands back a [`ClosedEpisode`] when the
//!   stretch ends (first violation-free frame, or run end);
//! - the caller judges soak (the episode's driver-unit span vs the
//!   rule's `window_s`) and, if it elapses, reports ONE finding
//!   with the episode's evidence.
//!
//! Identity rules (the fold the vocabularies must NOT re-implement):
//!
//! - An episode is a maximal stretch over TIME, not over key
//!   identity: any violating key on a frame keeps the episode
//!   open. A–B violating frames 1–10 and C–D violating 5–15 is
//!   ONE episode (1–15); splitting by key would let a persistent
//!   offender masquerade as many short findings.
//! - The episode reports the WORST observation (per the folder's
//!   [`WorstDirection`] — pairwise's deepest contact is the
//!   minimum distance; a deviation-family rule's worst is the
//!   maximum — tie-broken by earliest frame) plus a bounded roster
//!   (first + worst + frame-count per distinct key, capped) so a
//!   handoff A–B → A–C cannot hide the persistent offender A.
//!
//! Driver-clock doctrine follows `stagnant_resource` (see
//! `invariant_eval`): soak is measured in DRIVER-LEAF units, and
//! only forward deltas credit the span — a retreating driver
//! (seek/replay) advances no budget. A missing or non-finite driver
//! sample is a GAP: it proves nothing about continuity, so it
//! closes any open episode (the evidence gathered so far is judged
//! on its own accumulated span, never silently extended).

/// Which end of the measured value is the WORST observation. The
/// direction fixes severity semantics for BOTH the worst-observation
/// selection and the roster's worst-first sort — the two can never
/// disagree about what "worst" means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorstDirection {
    /// Smaller values are worse. The pairwise vocabulary: severity
    /// is proximity, and the deepest contact is the minimum
    /// observed distance.
    SmallerIsWorse,
    /// Larger values are worse. The deviation-family vocabularies:
    /// severity is incoherence, and the worst observation is the
    /// maximum observed deviation.
    LargerIsWorse,
}

impl WorstDirection {
    /// Whether `candidate` strictly beats `incumbent` as the worst
    /// observation. Ties are the caller's business (earliest frame).
    fn is_worse(self, candidate: f32, incumbent: f32) -> bool {
        match self {
            Self::SmallerIsWorse => candidate < incumbent,
            Self::LargerIsWorse => candidate > incumbent,
        }
    }

    /// Severity ordering for the roster's worst-first sort: the
    /// WORSE value orders FIRST. The caller chains frame and key
    /// tie-breaks after this.
    fn severity_cmp(self, a: f32, b: f32) -> std::cmp::Ordering {
        match self {
            // Worse (smaller) first: descending severity = ascending
            // value.
            Self::SmallerIsWorse => a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal),
            // Worse (larger) first: descending severity = descending
            // value.
            Self::LargerIsWorse => b.partial_cmp(&a).unwrap_or(std::cmp::Ordering::Equal),
        }
    }
}

/// Ordered pair identity: entity indices, low before high. Ordering
/// makes (a, b) and (b, a) the same pair so per-frame rosters
/// dedupe across traversal order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PairKey {
    pub low: u32,
    pub high: u32,
}

impl PairKey {
    pub fn new(a: u32, b: u32) -> Self {
        if a <= b {
            Self { low: a, high: b }
        } else {
            Self { low: b, high: a }
        }
    }
}

/// A per-frame violating observation: the observed KEY and the
/// measured VALUE seen that frame. Value semantics are fixed by the
/// folder's [`WorstDirection`] — the engine itself only requires
/// finiteness.
#[derive(Clone, Copy, Debug)]
pub struct Observation<K> {
    pub key: K,
    pub value: f32,
}

/// Bounded roster entry for one distinct key inside an episode:
/// first violating frame, worst value with the frame it was seen,
/// and how many frames the key violated. The roster is capped
/// ([`DEFAULT_ROSTER_CAP`]) so a pathological crowd cannot balloon
/// `_meta`; truncation is disclosed, never silent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RosterEntry<K> {
    pub key: K,
    pub first_frame: u64,
    pub worst_value: f32,
    pub worst_frame: u64,
    pub violating_frames: u64,
}

/// Default cap on distinct keys recorded in an episode's roster.
pub const DEFAULT_ROSTER_CAP: usize = 32;

/// The worst observation of an episode: the most severe value
/// observed (per the folder's direction), tie-broken by earliest
/// frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorstObservation<K> {
    pub key: K,
    pub value: f32,
    pub frame: u64,
}

/// A maximal stretch of violating frames, handed back when it ends.
///
/// The caller judges soak: `driver_span` is the episode's forward
/// driver-leaf advance in the driver's OWN unit; compare it against
/// the rule's `window_s` (which the author expressed in the same
/// unit) and discard sub-soak episodes as transient.
#[derive(Clone, Debug, PartialEq)]
pub struct ClosedEpisode<K> {
    /// First frame with a violating key.
    pub start_frame: u64,
    /// Last frame with a violating key (the episode ends AFTER it).
    pub end_frame: u64,
    /// Forward driver-leaf advance accumulated across the episode,
    /// in the driver's OWN unit. Episodes only ever form on frames
    /// with a finite driver sample (gaps close stretches rather
    /// than extend them), so the span is always measured; the
    /// caller compares it against the rule's `window_s` and
    /// discards sub-soak episodes as transients.
    pub driver_span: f64,
    /// Distinct violating keys observed, first + worst + count.
    /// Sorted worst-first (per the folder's direction, then
    /// earliest frame, then key order).
    pub roster: Vec<RosterEntry<K>>,
    /// True when the roster was truncated at the cap — disclosed so
    /// `_meta` never lies by omission.
    pub roster_truncated: bool,
    /// Total DISTINCT violating keys observed in the episode —
    /// `roster.len()` plus the keys the cap dropped. Reported so
    /// `_meta` can disclose the crowd size a truncation hid.
    pub distinct_keys: u64,
    /// The worst observation (per the folder's direction,
    /// tie-broken by earliest frame).
    pub worst: WorstObservation<K>,
}

struct OpenEpisode<K> {
    start_frame: u64,
    end_frame: u64,
    /// Forward driver accumulation (positive deltas only).
    driver_span: f64,
    /// Last observed driver value, for delta computation. None
    /// until the first observable sample inside the episode.
    last_driver: Option<f64>,
    /// The un-seeded worst observation.
    worst: Option<WorstObservation<K>>,
    /// Per-key records, in first-seen order (linear lookup: the
    /// roster is bounded by the cap, which beat a map here).
    roster: Vec<RosterEntry<K>>,
    /// Set at absorb time when a distinct key was dropped by the
    /// cap — truncation is disclosed, never silently subsampled.
    roster_truncated: bool,
    /// Total distinct violating keys observed in the episode,
    /// including the ones the cap dropped — the disclosed crowd
    /// size behind a truncated roster.
    distinct_keys: u64,
    /// Severity semantics for worst-selection and the roster's
    /// worst-first sort.
    direction: WorstDirection,
}

impl<K: Copy + Ord> OpenEpisode<K> {
    fn new(frame: u64, driver: Option<f64>, direction: WorstDirection) -> Self {
        Self {
            start_frame: frame,
            end_frame: frame,
            driver_span: 0.0,
            last_driver: driver.filter(|d| d.is_finite()),
            worst: None,
            roster: Vec::new(),
            roster_truncated: false,
            distinct_keys: 0,
            direction,
        }
    }

    /// Advance the stretch to `frame` and credit forward driver
    /// advance. Retreats advance no budget (same doctrine as
    /// `stagnant_resource`'s window: a retreating driver is a seek,
    /// not progress).
    fn extend(&mut self, frame: u64, driver: Option<f64>) {
        debug_assert!(frame > self.end_frame);
        self.end_frame = frame;
        let now = driver.filter(|d| d.is_finite());
        if let (Some(last), Some(cur)) = (self.last_driver, now) {
            let delta = cur - last;
            if delta > 0.0 {
                self.driver_span += delta;
            }
        }
        if now.is_some() {
            self.last_driver = now;
        }
    }

    fn close(mut self) -> ClosedEpisode<K> {
        let worst = self
            .worst
            .expect("absorb always seeds the worst observation on open");
        self.roster.sort_by(|a, b| {
            self.direction
                .severity_cmp(a.worst_value, b.worst_value)
                .then(a.worst_frame.cmp(&b.worst_frame))
                .then(a.key.cmp(&b.key))
        });
        ClosedEpisode {
            start_frame: self.start_frame,
            end_frame: self.end_frame,
            driver_span: self.driver_span,
            roster: self.roster,
            roster_truncated: self.roster_truncated,
            distinct_keys: self.distinct_keys,
            worst,
        }
    }

    /// Fold one frame's observations into the stretch: seed/update
    /// the worst, and add one frame to every distinct key's record
    /// (first-seen, worst-seen, count), dropping new keys past the
    /// cap while still counting them.
    fn absorb(&mut self, frame: u64, obs: &[Observation<K>], roster_cap: usize) {
        for o in obs {
            // Worst observation: per the folder's direction,
            // tie-broken by earliest frame.
            let better = match self.worst {
                None => true,
                Some(w) => {
                    self.direction.is_worse(o.value, w.value)
                        || (o.value == w.value && frame < w.frame)
                }
            };
            if better {
                self.worst = Some(WorstObservation {
                    key: o.key,
                    value: o.value,
                    frame,
                });
            }
            match self.roster.iter_mut().find(|r| r.key == o.key) {
                Some(r) => {
                    r.violating_frames += 1;
                    let worse_value = self.direction.is_worse(o.value, r.worst_value)
                        || (o.value == r.worst_value && frame < r.worst_frame);
                    if worse_value {
                        r.worst_value = o.value;
                        r.worst_frame = frame;
                    }
                }
                None => {
                    // A distinct key: count it BEFORE the cap
                    // decision so `distinct_keys` is the honest
                    // crowd size even when the roster drops it.
                    self.distinct_keys += 1;
                    let exceeded = self.roster.len() >= roster_cap;
                    if !exceeded {
                        self.roster.push(RosterEntry {
                            key: o.key,
                            first_frame: frame,
                            worst_value: o.value,
                            worst_frame: frame,
                            violating_frames: 1,
                        });
                    } else {
                        self.roster_truncated = true;
                    }
                }
            }
        }
    }
}

/// Per-invariant episode folder. Feed it every observed frame (in
/// frame order); it owns the maximal-stretch identity rules.
///
/// The default shape is the pairwise vocabulary ([`PairKey`] keys,
/// [`WorstDirection::SmallerIsWorse`] — deepest contact first); a
/// deviation-family vocabulary constructs its folder with entity
/// keys and [`WorstDirection::LargerIsWorse`].
pub struct EpisodeFolder<K = PairKey> {
    open: Option<OpenEpisode<K>>,
    roster_cap: usize,
    direction: WorstDirection,
}

impl Default for EpisodeFolder<PairKey> {
    fn default() -> Self {
        Self::new(DEFAULT_ROSTER_CAP, WorstDirection::SmallerIsWorse)
    }
}

impl<K: Copy + Ord> EpisodeFolder<K> {
    pub fn new(roster_cap: usize, direction: WorstDirection) -> Self {
        Self {
            open: None,
            roster_cap: roster_cap.max(1),
            direction,
        }
    }

    /// Observe one frame. `obs` is the frame's violating set (any
    /// order; duplicates allowed). `driver` is the driver-leaf
    /// sample in the driver's own unit (the frame clock in seconds
    /// when the author declared no driver). Returns the just-closed
    /// episode if this frame ends one.
    ///
    /// An empty `obs` ends any open episode. A `None`/non-finite
    /// driver is a GAP: it also closes an open episode rather than
    /// extending it across an unobservable clock. A non-finite
    /// observation value is a gap too — it proves nothing about
    /// the measured quantity this frame.
    pub fn push(
        &mut self,
        frame: u64,
        driver: Option<f64>,
        obs: &[Observation<K>],
    ) -> Option<ClosedEpisode<K>> {
        let clean = obs.iter().all(|o| o.value.is_finite());
        let gap = driver.map(|d| !d.is_finite()).unwrap_or(true);
        if !clean {
            // Non-finite observation: it proves nothing about the
            // world's state this frame — treat like a gap.
            return self.close_gap();
        }
        if obs.is_empty() {
            return self.open.take().map(|ep| ep.close());
        }
        if gap {
            return self.close_gap();
        }
        // A non-consecutive frame (a skipped observation slot)
        // would silently invent continuity, so close and reopen.
        if self
            .open
            .as_ref()
            .is_some_and(|ep| frame != ep.end_frame.saturating_add(1))
        {
            let closed = self.open.take().map(|ep| ep.close());
            self.push(frame, driver, obs);
            return closed;
        }
        match self.open.as_mut() {
            Some(ep) => {
                ep.extend(frame, driver);
                let roster_cap = self.roster_cap;
                ep.absorb(frame, obs, roster_cap);
                None
            }
            None => {
                self.open = Some(OpenEpisode::new(frame, driver, self.direction));
                let roster_cap = self.roster_cap;
                // Absorb runs on `self.open` via reborrow, so the
                // cap read happens first (borrow discipline only).
                if let Some(ep) = self.open.as_mut() {
                    ep.absorb(frame, obs, roster_cap);
                }
                None
            }
        }
    }

    /// Close any open episode at run end. Call exactly once, after
    /// the last observed frame.
    pub fn close_run(&mut self) -> Option<ClosedEpisode<K>> {
        self.open.take().map(|ep| ep.close())
    }

    /// A gap (missing/non-finite driver or observation) ends the
    /// open stretch; the evidence gathered is still a judgeable
    /// episode on its own accumulated span, so hand it back.
    fn close_gap(&mut self) -> Option<ClosedEpisode<K>> {
        self.open.take().map(|ep| ep.close())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(a: u32, b: u32, d: f32) -> Observation<PairKey> {
        Observation {
            key: PairKey::new(a, b),
            value: d,
        }
    }

    fn frame_s(i: u64) -> Option<f64> {
        Some(i as f64 / 60.0)
    }

    #[test]
    fn fly_by_short_episode_is_discardable_by_soak() {
        // Two frames of contact then clean: the episode closes with
        // a sub-soak span; the CALLER discards it (window_s = 30).
        let mut f = EpisodeFolder::default();
        assert!(f.push(0, frame_s(0), &[obs(1, 2, 0.1)]).is_none());
        assert!(f.push(1, frame_s(1), &[obs(1, 2, 0.1)]).is_none());
        let ep = f.push(2, frame_s(2), &[]).expect("clean frame closes");
        assert_eq!(ep.start_frame, 0);
        assert_eq!(ep.end_frame, 1);
        assert!((ep.driver_span - 1.0 / 60.0).abs() < 1e-9);
    }

    #[test]
    fn persistent_stack_reports_one_episode_with_worst_pair() {
        let mut f = EpisodeFolder::default();
        for i in 0..10u64 {
            let pairs = [
                obs(1, 2, 0.4 + 0.01 * (i % 3) as f32),
                obs(2, 3, 0.05), // the demon stack's real overlap
            ];
            assert!(f.push(i, frame_s(i), &pairs).is_none());
        }
        let ep = f.close_run().expect("run end closes");
        assert_eq!(ep.start_frame, 0);
        assert_eq!(ep.end_frame, 9);
        assert_eq!(ep.worst.key, PairKey::new(2, 3));
        assert!((ep.worst.value - 0.05).abs() < 1e-9);
        assert_eq!(ep.worst.frame, 0);
        assert_eq!(ep.roster.len(), 2);
        assert_eq!(ep.roster[0].key, PairKey::new(2, 3));
        assert_eq!(ep.roster[0].violating_frames, 10);
        assert!(!ep.roster_truncated);
    }

    #[test]
    fn handoff_keeps_one_episode_and_exposes_both_pairs() {
        // A–B violates frames 0–4, then A–C 5–9 (handoff): ONE
        // episode, roster shows both, worst is the deepest contact.
        let mut f = EpisodeFolder::default();
        for i in 0..5u64 {
            assert!(f.push(i, frame_s(i), &[obs(1, 2, 0.2)]).is_none());
        }
        for i in 5..10u64 {
            assert!(f.push(i, frame_s(i), &[obs(1, 3, 0.3)]).is_none());
        }
        let ep = f.close_run().expect("run end closes");
        assert_eq!(ep.start_frame, 0);
        assert_eq!(ep.end_frame, 9);
        assert_eq!(ep.roster.len(), 2);
        // Worst-first ordering: the 0.2 contact leads.
        assert_eq!(ep.roster[0].key, PairKey::new(1, 2));
        assert_eq!(ep.roster[0].violating_frames, 5);
        assert_eq!(ep.roster[0].first_frame, 0);
        assert_eq!(ep.roster[1].key, PairKey::new(1, 3));
        assert_eq!(ep.roster[1].first_frame, 5);
        assert_eq!(ep.worst.key, PairKey::new(1, 2));
    }

    #[test]
    fn overlapping_pairs_do_not_split_the_episode() {
        let mut f = EpisodeFolder::default();
        // A–B frames 0–1; C–D frames 2–6: the stretch continues via
        // C–D after A–B ends mid-way: ONE episode 0–6, both pairs.
        for i in 0..2u64 {
            assert!(f.push(i, frame_s(i), &[obs(1, 2, 0.2)]).is_none());
        }
        for i in 2..7u64 {
            assert!(f.push(i, frame_s(i), &[obs(10, 11, 0.3)]).is_none());
        }
        let ep = f.push(7, frame_s(7), &[]).expect("clean frame closes");
        assert_eq!(ep.start_frame, 0);
        assert_eq!(ep.end_frame, 6);
        assert_eq!(ep.roster.len(), 2);
        assert_eq!(ep.roster[0].key, PairKey::new(1, 2));
        assert_eq!(ep.roster[1].key, PairKey::new(10, 11));
    }

    #[test]
    fn retreat_does_not_credit_soak() {
        // Driver jumps back mid-episode (seek): forward-only
        // accumulation must not subtract nor credit.
        let mut f = EpisodeFolder::default();
        f.push(0, Some(0.0), &[obs(1, 2, 0.1)]);
        f.push(1, Some(10.0), &[obs(1, 2, 0.1)]);
        f.push(2, Some(4.0), &[obs(1, 2, 0.1)]); // retreat
        f.push(3, Some(6.0), &[obs(1, 2, 0.1)]); // forward from 4 → +2
        let ep = f.close_run().expect("run end closes");
        // 0→10 credits +10; 10→4 retreats (no credit, no penalty);
        // 4→6 credits +2. Total 12 — NOT the endpoint difference 6.
        assert!((ep.driver_span - 12.0).abs() < 1e-9);
    }

    #[test]
    fn driver_gap_closes_the_episode_rather_than_extending_it() {
        let mut f = EpisodeFolder::default();
        f.push(0, Some(0.0), &[obs(1, 2, 0.1)]);
        f.push(1, Some(1.0), &[obs(1, 2, 0.1)]);
        let ep = f
            .push(2, None, &[obs(1, 2, 0.1)])
            .expect("driver gap closes the stretch");
        assert_eq!(ep.end_frame, 1);
        assert!((ep.driver_span - 1.0).abs() < 1e-9);
        // The stretch re-opens at the next clean sample.
        assert!(f.push(3, Some(3.0), &[obs(1, 2, 0.1)]).is_none());
        let ep2 = f.close_run().expect("run end closes the reopened stretch");
        assert_eq!(ep2.start_frame, 3);
        assert_eq!(ep2.end_frame, 3);
        // The reopened stretch's driver_span seeds at 3.0 and never
        // advances before run end.
        assert!((ep2.driver_span).abs() < 1e-9);
    }

    #[test]
    fn non_finite_observation_is_a_gap() {
        let mut f = EpisodeFolder::default();
        f.push(0, Some(0.0), &[obs(1, 2, 0.1)]);
        let ep = f
            .push(1, Some(1.0), &[obs(1, 2, f32::NAN)])
            .expect("NaN observation closes the stretch");
        assert_eq!(ep.end_frame, 0);
    }

    #[test]
    fn unobserved_driver_yields_no_judgeable_episode() {
        // Doctrine pin: an episode can never OPEN on an
        // unobservable driver frame — a gap closes stretches, it
        // does not start them (continuity across an unseen clock
        // cannot be proven). A run where the driver is never
        // observable therefore accumulates no judgeable episode at
        // all, and the gap between observed pairs splits them
        // rather than gluing them.
        let mut f = EpisodeFolder::default();
        assert!(f.push(0, None, &[obs(1, 2, 0.1)]).is_none());
        assert!(f.push(1, None, &[obs(1, 2, 0.1)]).is_none());
        assert!(f.close_run().is_none());
    }

    #[test]
    fn non_consecutive_frame_splits_rather_than_inventing_continuity() {
        // The caller's frame-order contract's enforcement branch:
        // observing f0, f1, then f3 (f2 skipped) must NOT glue the
        // stretches into one episode — that would silently invent
        // continuity across the unobserved frame. One episode
        // closes (handed back), a new one opens at f3, and closing
        // the run yields the reopened stretch.
        let mut f = EpisodeFolder::default();
        assert!(f.push(0, Some(0.0), &[obs(1, 2, 0.2)]).is_none());
        assert!(f.push(1, Some(1.0), &[obs(1, 2, 0.2)]).is_none());
        // f2 unobserved; f3 violates with a DIFFERENT pair so the
        // two stretches are distinguishable in the evidence.
        let ep = f
            .push(3, Some(3.0), &[obs(5, 6, 0.3)])
            .expect("skipped frame closes the stretch");
        assert_eq!(ep.start_frame, 0);
        assert_eq!(ep.end_frame, 1);
        assert_eq!(ep.worst.key, PairKey::new(1, 2));
        assert!((ep.driver_span - 1.0).abs() < 1e-9);
        // The reopened stretch seeds at the OBSERVING frame (3),
        // not the skipped one (2): the gap proved nothing.
        assert!(f.push(4, Some(4.0), &[obs(5, 6, 0.3)]).is_none());
        let ep2 = f.close_run().expect("run end closes the reopened stretch");
        assert_eq!(ep2.start_frame, 3);
        assert_eq!(ep2.end_frame, 4);
        assert_eq!(ep2.worst.key, PairKey::new(5, 6));
        assert!((ep2.driver_span - 1.0).abs() < 1e-9);
    }

    #[test]
    fn pair_key_orders_ids() {
        assert_eq!(PairKey::new(7, 3), PairKey::new(3, 7));
        assert_eq!(PairKey::new(5, 5).low, 5);
        assert_eq!(PairKey::new(3, 7).low, 3);
        assert_eq!(PairKey::new(3, 7).high, 7);
    }

    #[test]
    fn worst_ties_break_to_earliest_frame() {
        let mut f = EpisodeFolder::default();
        f.push(0, Some(0.0), &[obs(1, 2, 0.1)]);
        f.push(1, Some(1.0), &[obs(3, 4, 0.1)]);
        let ep = f.close_run().expect("run end closes");
        assert_eq!(ep.worst.key, PairKey::new(1, 2));
        assert_eq!(ep.worst.frame, 0);
    }

    #[test]
    fn roster_cap_truncation_is_disclosed() {
        let mut f = EpisodeFolder::new(3, WorstDirection::SmallerIsWorse);
        // Six distinct pairs, cap 3: roster holds the first three
        // seen (first-seen order is deterministic), truncation flag
        // set — never silently subsampled.
        f.push(0, Some(0.0), &[obs(1, 2, 0.1)]);
        f.push(
            1,
            Some(1.0),
            &[
                obs(3, 4, 0.05),
                obs(5, 6, 0.2),
                obs(7, 8, 0.3),
                obs(9, 10, 0.4),
            ],
        );
        let ep = f.close_run().expect("run end closes");
        assert_eq!(ep.roster.len(), 3);
        assert!(ep.roster_truncated);
        // The honest crowd size: all FIVE distinct pairs were
        // counted (one from frame 0, four from frame 1), including
        // the two the cap dropped.
        assert_eq!(ep.distinct_keys, 5);
        // The roster holds the first three pairs SEEN — (1,2),
        // (3,4), (5,6) — and the cap dropped (7,8), (9,10). The
        // worst observation is tracked OUTSIDE the roster, so it
        // still sees pair (3,4) at distance 0.05.
        assert_eq!(ep.worst.key, PairKey::new(3, 4));
        assert_eq!(ep.roster[2].key, PairKey::new(5, 6));
    }

    // ------------------------------------------------------------------
    // The larger-is-worse direction: the deviation-family shape
    // (per-entity keys, worst = MAXIMUM value). Mirrors the pairwise
    // pins that are direction-sensitive so the two directions are
    // provably symmetric.
    // ------------------------------------------------------------------

    fn dev(entity: u32, v: f32) -> Observation<u32> {
        Observation {
            key: entity,
            value: v,
        }
    }

    #[test]
    fn larger_is_worst_selects_the_maximum() {
        let mut f = EpisodeFolder::new(DEFAULT_ROSTER_CAP, WorstDirection::LargerIsWorse);
        for i in 0..5u64 {
            // Entity 7's deviation GROWS frame over frame: the worst
            // is the LAST-seen maximum, not a minimum anywhere.
            assert!(f.push(i, frame_s(i), &[dev(7, 1.0 + i as f32)]).is_none());
        }
        let ep = f.push(5, frame_s(5), &[]).expect("clean frame closes");
        assert_eq!(ep.worst.key, 7);
        assert!((ep.worst.value - 5.0).abs() < 1e-9);
        assert_eq!(ep.worst.frame, 4);
        assert_eq!(ep.roster.len(), 1);
        assert_eq!(ep.roster[0].worst_value, 5.0);
    }

    #[test]
    fn larger_is_worst_roster_sorts_worst_first() {
        let mut f = EpisodeFolder::new(DEFAULT_ROSTER_CAP, WorstDirection::LargerIsWorse);
        f.push(0, Some(0.0), &[dev(1, 0.5), dev(2, 2.5), dev(3, 1.5)]);
        let ep = f.close_run().expect("run end closes");
        // Worst-first under LargerIsWorse: entity 2 (2.5) leads,
        // then 3 (1.5), then 1 (0.5) — the exact mirror of the
        // pairwise roster's deepest-contact-first ordering.
        assert_eq!(ep.roster[0].key, 2);
        assert_eq!(ep.roster[1].key, 3);
        assert_eq!(ep.roster[2].key, 1);
        assert_eq!(ep.worst.key, 2);
    }

    #[test]
    fn larger_is_worst_ties_break_to_earliest_frame() {
        let mut f = EpisodeFolder::new(DEFAULT_ROSTER_CAP, WorstDirection::LargerIsWorse);
        f.push(0, Some(0.0), &[dev(11, 3.0)]);
        f.push(1, Some(1.0), &[dev(12, 3.0)]);
        let ep = f.close_run().expect("run end closes");
        assert_eq!(ep.worst.key, 11);
        assert_eq!(ep.worst.frame, 0);
    }

    #[test]
    fn larger_is_worst_roster_cap_truncation_is_disclosed() {
        let mut f = EpisodeFolder::new(2, WorstDirection::LargerIsWorse);
        f.push(0, Some(0.0), &[dev(1, 0.1), dev(2, 0.2), dev(3, 0.3)]);
        let ep = f.close_run().expect("run end closes");
        assert_eq!(ep.roster.len(), 2);
        assert!(ep.roster_truncated);
        assert_eq!(ep.distinct_keys, 3);
        // The worst is tracked outside the roster: entity 3's 0.3
        // survives even though the cap dropped its roster entry.
        assert_eq!(ep.worst.key, 3);
    }

    #[test]
    fn larger_is_worst_non_finite_observation_is_a_gap() {
        // Direction-symmetry mirror of the pairwise NaN pin: the
        // gap doctrine (a non-finite observation proves nothing,
        // so it ends the stretch rather than feeding it) is
        // shared, direction-blind code — but the symmetry claim
        // the two direction families make deserves the pin on
        // BOTH sides of the axis.
        let mut f = EpisodeFolder::new(DEFAULT_ROSTER_CAP, WorstDirection::LargerIsWorse);
        f.push(0, Some(0.0), &[dev(1, 2.0)]);
        let ep = f
            .push(1, Some(1.0), &[dev(1, f32::NAN)])
            .expect("NaN observation closes the stretch");
        assert_eq!(ep.end_frame, 0);
    }
}
