//! The shared episode-fold engine (0.16 judgement-layer §7).
//!
//! Both judgement-layer vocabularies (spatial contracts and entity
//! stagnation) SELECT entities, ENUMERATE observations per frame,
//! and FOLD consecutive violating frames into EPISODES with
//! identity rules and bounded evidence. This module is that fold,
//! as a pure state machine over owned, bevy-free data:
//!
//! - the recorder (driver loop) supplies, per frame, the set of
//!   violating observations (here: ordered entity-index pairs with
//!   the observed distance) and a-driver-clock sample;
//! - [`EpisodeFolder`] folds consecutive frames into a maximal
//!   stretch, tracks the worst observation and a bounded roster of
//!   distinct pairs, and hands back a [`ClosedEpisode`] when the
//!   stretch ends (first violation-free frame, or run end);
//! - the caller judges soak (the episode's driver-unit span vs the
//!   rule's `window_s`) and, if it elapses, reports ONE finding
//!   with the episode's evidence.
//!
//! Identity rules (the fold the vocabularies must NOT re-implement):
//!
//! - An episode is a maximal stretch over TIME, not over pair
//!   identity: any violating pair on a frame keeps the episode
//!   open. A–B violating frames 1–10 and C–D violating 5–15 is
//!   ONE episode (1–15); splitting by pair would let a persistent
//!   offender masquerade as many short findings.
//! - The episode reports the WORST observation (minimum distance,
//!   tie-broken by earliest frame) plus a bounded roster (first +
//!   worst + frame-count per distinct pair, capped) so a handoff
//!   A–B → A–C cannot hide the persistent offender A.
//!
//! Driver-clock doctrine follows `stagnant_resource` (see
//! `invariant_eval`): soak is measured in DRIVER-LEAF units, and
//! only forward deltas credit the span — a retreating driver
//! (seek/replay) advances no budget. A missing or non-finite driver
//! sample is a GAP: it proves nothing about continuity, so it
//! closes any open episode (the evidence gathered so far is judged
//! on its own accumulated span, never silently extended).

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

/// A per-frame violating observation: the pair and the distance
/// seen that frame.
#[derive(Clone, Copy, Debug)]
pub struct PairObservation {
    pub pair: PairKey,
    pub distance: f32,
}

/// Bounded roster entry for one distinct pair inside an episode:
/// first violating frame, worst (minimum) distance with the frame it
/// was seen, and how many frames the pair violated. The roster is
/// capped ([`DEFAULT_ROSTER_CAP`]) so a pathological crowd cannot
/// balloon `_meta`; truncation is disclosed, never silent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RosterEntry {
    pub pair: PairKey,
    pub first_frame: u64,
    pub worst_distance: f32,
    pub worst_frame: u64,
    pub violating_frames: u64,
}

/// Default cap on distinct pairs recorded in an episode's roster.
pub const DEFAULT_ROSTER_CAP: usize = 32;

/// The worst observation of an episode: minimum distance observed,
/// tie-broken by earliest frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorstObservation {
    pub pair: PairKey,
    pub distance: f32,
    pub frame: u64,
}

/// A maximal stretch of violating frames, handed back when it ends.
///
/// The caller judges soak: `driver_span` is the episode's forward
/// driver-leaf advance in the driver's OWN unit; compare it against
/// the rule's `window_s` (which the author expressed in the same
/// unit) and discard sub-soak episodes as transient fly-bys.
#[derive(Clone, Debug, PartialEq)]
pub struct ClosedEpisode {
    /// First frame with a violating pair.
    pub start_frame: u64,
    /// Last frame with a violating pair (the episode ends AFTER it).
    pub end_frame: u64,
    /// Forward driver-leaf advance accumulated across the episode,
    /// in the driver's OWN unit. Episodes only ever form on frames
    /// with a finite driver sample (gaps close stretches rather
    /// than extend them), so the span is always measured; the
    /// caller compares it against the rule's `window_s` and
    /// discards sub-soak episodes as transient fly-bys.
    pub driver_span: f64,
    /// Distinct violating pairs observed, first + worst + count.
    /// Sorted worst-first (minimum distance, then earliest frame).
    pub roster: Vec<RosterEntry>,
    /// True when the roster was truncated at the cap — disclosed so
    /// `_meta` never lies by omission.
    pub roster_truncated: bool,
    /// The worst observation (minimum distance, earliest frame).
    pub worst: WorstObservation,
}

struct OpenEpisode {
    start_frame: u64,
    end_frame: u64,
    /// Forward driver accumulation (positive deltas only).
    driver_span: f64,
    /// Last observed driver value, for delta computation. None
    /// until the first observable sample inside the episode.
    last_driver: Option<f64>,
    /// The un-seeded worst observation.
    worst: Option<WorstObservation>,
    /// Per-pair records, in first-seen order (linear lookup: the
    /// roster is bounded by the cap, which beat a map here).
    roster: Vec<RosterEntry>,
    /// Set at absorb time when a distinct pair was dropped by the
    /// cap — truncation is disclosed, never silently subsampled.
    roster_truncated: bool,
}

impl OpenEpisode {
    fn new(frame: u64, driver: Option<f64>) -> Self {
        Self {
            start_frame: frame,
            end_frame: frame,
            driver_span: 0.0,
            last_driver: driver.filter(|d| d.is_finite()),
            worst: None,
            roster: Vec::new(),
            roster_truncated: false,
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

    fn close(self) -> ClosedEpisode {
        let worst =
            self.worst.expect("absorb always seeds the worst observation on open");
        let mut roster = self.roster;
        roster.sort_by(|a, b| {
            a.worst_distance
                .partial_cmp(&b.worst_distance)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.worst_frame.cmp(&b.worst_frame))
                .then(a.pair.cmp(&b.pair))
        });
        ClosedEpisode {
            start_frame: self.start_frame,
            end_frame: self.end_frame,
            driver_span: self.driver_span,
            roster,
            roster_truncated: self.roster_truncated,
            worst,
        }
    }
}

/// Per-invariant episode folder. Feed it every observed frame (in
/// frame order); it owns the maximal-stretch identity rules.
pub struct EpisodeFolder {
    open: Option<OpenEpisode>,
    roster_cap: usize,
}

impl Default for EpisodeFolder {
    fn default() -> Self {
        Self::new(DEFAULT_ROSTER_CAP)
    }
}

impl EpisodeFolder {
    pub fn new(roster_cap: usize) -> Self {
        Self {
            open: None,
            roster_cap: roster_cap.max(1),
        }
    }

    /// Observe one frame. `pairs` is the frame's violating set (any
    /// order; duplicates allowed). `driver` is the driver-leaf
    /// sample in the driver's own unit (the frame clock in seconds
    /// when the author declared no driver). Returns the just-closed
    /// episode if this frame ends one.
    ///
    /// An empty `pairs` ends any open episode. A `None`/non-finite
    /// driver is a GAP: it also closes an open episode rather than
    /// extending it across an unobservable clock.
    pub fn push(
        &mut self,
        frame: u64,
        driver: Option<f64>,
        pairs: &[PairObservation],
    ) -> Option<ClosedEpisode> {
        let clean = pairs.iter().all(|o| o.distance.is_finite());
        let gap = driver.map(|d| !d.is_finite()).unwrap_or(true);
        if !clean {
            // Non-finite observation: it proves nothing about the
            // world's spatial state this frame — treat like a gap.
            return self.close_gap();
        }
        if pairs.is_empty() {
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
            self.push(frame, driver, pairs);
            return closed;
        }
        match self.open.as_mut() {
            Some(ep) => {
                ep.extend(frame, driver);
                self.absorb(frame, pairs);
                None
            }
            None => {
                self.open = Some(OpenEpisode::new(frame, driver));
                self.absorb(frame, pairs);
                None
            }
        }
    }

    /// Close any open episode at run end. Call exactly once, after
    /// the last observed frame.
    pub fn close_run(&mut self) -> Option<ClosedEpisode> {
        self.open.take().map(|ep| ep.close())
    }

    /// A gap (missing/non-finite driver or observation) ends the
    /// open stretch; the evidence gathered is still a judgeable
    /// episode on its own accumulated span, so hand it back.
    fn close_gap(&mut self) -> Option<ClosedEpisode> {
        self.open.take().map(|ep| ep.close())
    }

    fn absorb(&mut self, frame: u64, pairs: &[PairObservation]) {
        let Some(ep) = self.open.as_mut() else { return };
        for obs in pairs {
            // Worst observation: minimum distance, tie-broken by
            // earliest frame.
            let better = match ep.worst {
                None => true,
                Some(w) => {
                    obs.distance < w.distance
                        || (obs.distance == w.distance && frame < w.frame)
                }
            };
            if better {
                ep.worst = Some(WorstObservation {
                    pair: obs.pair,
                    distance: obs.distance,
                    frame,
                });
            }
            match ep.roster.iter_mut().find(|r| r.pair == obs.pair) {
                Some(r) => {
                    r.violating_frames += 1;
                    if obs.distance < r.worst_distance
                        || (obs.distance == r.worst_distance && frame < r.worst_frame)
                    {
                        r.worst_distance = obs.distance;
                        r.worst_frame = frame;
                    }
                }
                None => {
                    let exceeded = ep.roster.len() >= self.roster_cap;
                    if !exceeded {
                        ep.roster.push(RosterEntry {
                            pair: obs.pair,
                            first_frame: frame,
                            worst_distance: obs.distance,
                            worst_frame: frame,
                            violating_frames: 1,
                        });
                    } else {
                        ep.roster_truncated = true;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(a: u32, b: u32, d: f32) -> PairObservation {
        PairObservation {
            pair: PairKey::new(a, b),
            distance: d,
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
        assert_eq!(ep.worst.pair, PairKey::new(2, 3));
        assert!((ep.worst.distance - 0.05).abs() < 1e-9);
        assert_eq!(ep.worst.frame, 0);
        assert_eq!(ep.roster.len(), 2);
        assert_eq!(ep.roster[0].pair, PairKey::new(2, 3));
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
        assert_eq!(ep.roster[0].pair, PairKey::new(1, 2));
        assert_eq!(ep.roster[0].violating_frames, 5);
        assert_eq!(ep.roster[0].first_frame, 0);
        assert_eq!(ep.roster[1].pair, PairKey::new(1, 3));
        assert_eq!(ep.roster[1].first_frame, 5);
        assert_eq!(ep.worst.pair, PairKey::new(1, 2));
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
        assert_eq!(ep.roster[0].pair, PairKey::new(1, 2));
        assert_eq!(ep.roster[1].pair, PairKey::new(10, 11));
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
        let ep = f.push(2, None, &[obs(1, 2, 0.1)])
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
        assert_eq!(ep.worst.pair, PairKey::new(1, 2));
        assert!((ep.driver_span - 1.0).abs() < 1e-9);
        // The reopened stretch seeds at the OBSERVING frame (3),
        // not the skipped one (2): the gap proved nothing.
        assert!(f.push(4, Some(4.0), &[obs(5, 6, 0.3)]).is_none());
        let ep2 = f.close_run().expect("run end closes the reopened stretch");
        assert_eq!(ep2.start_frame, 3);
        assert_eq!(ep2.end_frame, 4);
        assert_eq!(ep2.worst.pair, PairKey::new(5, 6));
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
        assert_eq!(ep.worst.pair, PairKey::new(1, 2));
        assert_eq!(ep.worst.frame, 0);
    }

    #[test]
    fn roster_cap_truncation_is_disclosed() {
        let mut f = EpisodeFolder::new(3);
        // Six distinct pairs, cap 3: roster holds the first three
        // seen (first-seen order is deterministic), truncation flag
        // set — never silently subsampled.
        f.push(0, Some(0.0), &[obs(1, 2, 0.1)]);
        f.push(
            1,
            Some(1.0),
            &[obs(3, 4, 0.05), obs(5, 6, 0.2), obs(7, 8, 0.3), obs(9, 10, 0.4)],
        );
        let ep = f.close_run().expect("run end closes");
        assert_eq!(ep.roster.len(), 3);
        assert!(ep.roster_truncated);
        // The roster holds the first three pairs SEEN — (1,2),
        // (3,4), (5,6) — and the cap dropped (7,8), (9,10). The
        // worst observation is tracked OUTSIDE the roster, so it
        // still sees pair (3,4) at distance 0.05.
        assert_eq!(ep.worst.pair, PairKey::new(3, 4));
        assert_eq!(ep.roster[2].pair, PairKey::new(5, 6));
    }
}
