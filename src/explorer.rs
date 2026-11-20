//! Exploration policy for the chaos bot (`bot.explore: true`).
//!
//! The chaos systems keep delivering input exactly as before (holds,
//! dual-form events, the recorded choice stream); in explore mode they
//! only change WHAT they pick:
//!
//! - A **screen** is the set of enabled widgets currently on screen
//!   (their labels, else role + index), from the UI-surface enumeration.
//! - On each screen the bot prefers the **least-tried** key and the
//!   least-tried widget; ties are broken by the recorded random draw, so
//!   runs stay seeded and replay-faithful.
//! - **Novelty** is a state schedule that never ran before (from the
//!   state trace) or a screen never seen before. After
//!   [`STUCK_SECONDS`] without novelty the bot presses Escape to back
//!   out of whatever it is stuck in, except on the first screen seen
//!   (often a title screen, where Escape may quit), and the wait doubles
//!   after each back-out on the same screen (a static gameplay HUD must
//!   not turn into a pause-menu toggle loop).
//! - The run ends with an [`ExplorationSummary`] on the report: screens
//!   seen, widgets never clicked, keys never pressed, and whether
//!   discovery had plateaued or was still going when the run ended.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};

use bevy_ecs::prelude::Resource;
use serde::{Deserialize, Serialize};

/// Seconds without novelty before the explorer presses Escape.
pub const STUCK_SECONDS: f32 = 3.0;

/// What the explorer reports at the end of a run.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExplorationSummary {
    /// Distinct screens (sets of enabled widgets) seen.
    pub screens_seen: usize,
    /// State schedules that ran at least once, sorted.
    pub states_entered: Vec<String>,
    /// Widgets seen enabled on some screen but never clicked by the bot,
    /// sorted. A click counts when it is SENT at the widget's last known
    /// center (refreshed every ~0.5 s), not when the game confirms it.
    pub widgets_never_clicked: Vec<String>,
    /// Keys the bot could press but never did, sorted.
    pub keys_never_pressed: Vec<String>,
    /// Times the bot pressed Escape because nothing new was happening.
    pub back_outs: u32,
    /// Frame of the last genuine discovery (new state or new screen).
    pub last_discovery_frame: u64,
    /// Frames the run lasted.
    pub frames: u64,
    /// "plateaued: ..." when nothing new appeared in the last quarter
    /// of the run, else "still discovering when the run ended": in the
    /// second case "no findings" only covers what was reached so far.
    pub verdict: String,
}

/// Explorer memory, shared between the driver loop and the chaos systems.
#[derive(Resource, Default)]
pub(crate) struct Explorer {
    pub(crate) enabled: bool,
    /// Current screen (None until the first observe).
    screen: Option<u64>,
    first_screen: Option<u64>,
    back_outs_per_screen: BTreeMap<u64, u32>,
    tries: BTreeMap<(u64, String), u32>,
    /// Screen signature -> every widget name seen on it.
    screens: BTreeMap<u64, BTreeSet<String>>,
    /// Frame the bot arrived on the current screen.
    arrived: u64,
    states: BTreeSet<String>,
    keys_offered: BTreeSet<String>,
    keys_pressed: BTreeSet<String>,
    clicked: BTreeSet<String>,
    /// Reset by real novelty AND by a back-out (stuck detection).
    last_novelty: u64,
    /// Reset by real novelty only (reporting).
    last_discovery: u64,
    back_outs: u32,
    frames: u64,
}

/// Screen identity: the sorted widget names with all numerals removed,
/// so a ticking countdown or score label does not make every refresh a
/// "new screen". Widget NAMES keep their digits ("Level 1" and "Level
/// 11" stay different buttons); only the screen identity ignores them.
fn signature(ids: &[String]) -> u64 {
    let mut stable: Vec<String> = ids
        .iter()
        .map(|id| id.chars().filter(|c| !c.is_numeric()).collect())
        .collect();
    stable.sort();
    stable.dedup();
    // DefaultHasher::new() uses fixed keys: stable across processes.
    let mut h = DefaultHasher::new();
    stable.hash(&mut h);
    h.finish()
}

impl Explorer {
    pub(crate) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            ..Self::default()
        }
    }

    /// Driver-loop update after each frame: the state schedules that ran
    /// this frame and the current click targets `(name, disabled)`.
    pub(crate) fn observe(
        &mut self,
        frame: u64,
        ran_states: &[String],
        targets: &[(String, bool)],
    ) {
        self.frames = frame + 1;
        let mut novel = false;
        for state in ran_states {
            novel |= self.states.insert(state.clone());
        }
        let mut ids: Vec<String> = targets
            .iter()
            .filter(|(_, disabled)| !disabled)
            .map(|(id, _)| id.clone())
            .collect();
        ids.sort();
        ids.dedup();
        let sig = signature(&ids);
        if !self.screens.contains_key(&sig) {
            novel = true;
        }
        self.screens
            .entry(sig)
            .or_default()
            .extend(ids.iter().cloned());
        if self.screen != Some(sig) {
            self.arrived = frame;
        }
        self.screen = Some(sig);
        // Only a screen WITH widgets can be the exempt first screen: an
        // empty target list (loading frame, widgetless HUD, enumeration
        // off) would otherwise exempt every widgetless screen forever.
        if self.first_screen.is_none() && !ids.is_empty() {
            self.first_screen = Some(sig);
        }
        if novel {
            self.last_novelty = frame;
            self.last_discovery = frame;
        }
    }

    /// True when the bot should back out: not on the first screen with
    /// widgets, and nothing new (and no screen change) for
    /// [`STUCK_SECONDS`] times 2^(back-outs already done on this screen,
    /// capped at 5).
    pub(crate) fn stuck(&self, frame: u64, tps: u64) -> bool {
        let Some(screen) = self.screen else {
            return false;
        };
        if self.first_screen == Some(screen) {
            return false;
        }
        let done = self.back_outs_per_screen.get(&screen).copied().unwrap_or(0);
        let base = (tps as f32 * STUCK_SECONDS).ceil() as u64;
        let window = base.max(1) << done.min(5);
        // Timed from the later of the last novelty and the arrival on this
        // screen: returning to a known screen must not trigger at once.
        let since = self.last_novelty.max(self.arrived);
        frame.saturating_sub(since) >= window
    }

    /// The bot backed out (pressed Escape): restart the stuck timer.
    pub(crate) fn backed_out(&mut self, frame: u64) {
        self.back_outs += 1;
        if let Some(screen) = self.screen {
            *self.back_outs_per_screen.entry(screen).or_default() += 1;
        }
        self.last_novelty = frame;
    }

    /// Note the keys the bot could press (for "keys never pressed").
    pub(crate) fn offer_keys(&mut self, keys: &[&str]) {
        self.keys_offered.extend(keys.iter().map(|k| k.to_string()));
    }

    /// Indices of `names` tried the fewest times on the current screen
    /// (all of them before the first screen is known).
    pub(crate) fn least_tried(&self, kind: &str, names: &[&str]) -> Vec<usize> {
        let Some(screen) = self.screen else {
            return (0..names.len()).collect();
        };
        let count = |n: &str| {
            self.tries
                .get(&(screen, format!("{kind}:{n}")))
                .copied()
                .unwrap_or(0)
        };
        let min = names.iter().map(|n| count(n)).min().unwrap_or(0);
        (0..names.len())
            .filter(|&i| count(names[i]) == min)
            .collect()
    }

    /// Record one try of `name` (a key or a widget) on the current screen.
    pub(crate) fn record(&mut self, kind: &str, name: &str) {
        if let Some(screen) = self.screen {
            *self
                .tries
                .entry((screen, format!("{kind}:{name}")))
                .or_default() += 1;
        }
        match kind {
            "key" => {
                self.keys_pressed.insert(name.to_string());
            }
            "click" => {
                self.clicked.insert(name.to_string());
            }
            _ => {}
        }
    }

    pub(crate) fn summary(&self) -> ExplorationSummary {
        let seen: BTreeSet<&String> = self.screens.values().flatten().collect();
        let tail = self.frames / 4;
        let quiet = self.frames.saturating_sub(self.last_discovery + 1);
        let verdict = if self.frames > 0 && quiet >= tail {
            format!(
                "plateaued: nothing new in the last {quiet} of {} frames",
                self.frames
            )
        } else {
            "still discovering when the run ended".to_string()
        };
        ExplorationSummary {
            screens_seen: self.screens.len(),
            states_entered: self.states.iter().cloned().collect(),
            widgets_never_clicked: seen
                .into_iter()
                .filter(|id| !self.clicked.contains(*id))
                .cloned()
                .collect(),
            keys_never_pressed: self
                .keys_offered
                .iter()
                .filter(|k| !self.keys_pressed.contains(*k))
                .cloned()
                .collect(),
            back_outs: self.back_outs,
            last_discovery_frame: self.last_discovery,
            frames: self.frames,
            verdict,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(names: &[&str]) -> Vec<(String, bool)> {
        names.iter().map(|n| (n.to_string(), false)).collect()
    }

    #[test]
    fn prefers_least_tried_on_the_current_screen() {
        let mut e = Explorer::new(true);
        e.observe(0, &[], &t(&["Play", "Settings"]));
        assert_eq!(e.least_tried("click", &["Play", "Settings"]), vec![0, 1]);
        e.record("click", "Play");
        assert_eq!(e.least_tried("click", &["Play", "Settings"]), vec![1]);
        // A different screen has its own counts.
        e.observe(1, &[], &t(&["Back"]));
        assert_eq!(e.least_tried("click", &["Back"]), vec![0]);
    }

    #[test]
    fn novelty_and_stuck_detection() {
        let mut e = Explorer::new(true);
        e.observe(0, &["OnEnter(Menu)".into()], &t(&["Play"]));
        assert!(!e.stuck(500, 60), "never back out on the first screen");
        e.observe(10, &["OnEnter(Game)".into()], &t(&["Pause"]));
        assert!(!e.stuck(100, 60));
        assert!(e.stuck(190, 60), "3 s at 60 tps without novelty");
        e.backed_out(190);
        assert!(!e.stuck(190 + 300, 60), "second wait on this screen is 6 s");
        assert!(e.stuck(190 + 360, 60));
        e.observe(600, &["OnEnter(Over)".into()], &t(&[]));
        assert_eq!(e.summary().last_discovery_frame, 600);
    }

    #[test]
    fn first_screen_is_the_first_with_widgets() {
        let mut e = Explorer::new(true);
        e.observe(0, &[], &t(&[])); // loading frame: no widgets
        e.observe(5, &["OnEnter(Menu)".into()], &t(&["Play"]));
        assert!(!e.stuck(1000, 60), "the title menu is exempt");
        e.observe(10, &["OnEnter(Game)".into()], &t(&[])); // widgetless HUD
        assert!(
            e.stuck(200, 60),
            "a widgetless gameplay screen is not exempt"
        );
    }

    #[test]
    fn returning_to_a_known_screen_does_not_trigger_at_once() {
        let mut e = Explorer::new(true);
        e.observe(0, &[], &t(&["Play", "Settings"])); // title (exempt)
        e.observe(100, &[], &t(&["Back"])); // settings, novel
        e.observe(110, &[], &t(&["Play", "Settings"])); // back to title
        e.observe(1300, &[], &t(&["Back"])); // settings again, not novel
        assert!(!e.stuck(1301, 60), "timed from arrival, not last novelty");
        assert!(e.stuck(1300 + 180, 60));
    }

    #[test]
    fn ticking_labels_keep_one_screen_but_distinct_buttons() {
        let mut e = Explorer::new(true);
        e.observe(0, &[], &t(&["Start in 3"]));
        e.observe(30, &[], &t(&["Start in 2"]));
        assert_eq!(e.summary().screens_seen, 1, "a countdown is one screen");
        e.observe(60, &[], &t(&["Level 1", "Level 2"]));
        e.record("click", "Level 1");
        assert_eq!(e.least_tried("click", &["Level 1", "Level 2"]), vec![1]);
    }

    #[test]
    fn summary_reports_gaps_and_verdict() {
        let mut e = Explorer::new(true);
        e.observe(0, &["OnEnter(Menu)".into()], &t(&["Play", "Quit"]));
        e.offer_keys(&["Space", "Enter"]);
        e.record("click", "Play");
        e.record("key", "Space");
        for f in 1..100 {
            e.observe(f, &[], &t(&["Play", "Quit"]));
        }
        let s = e.summary();
        assert_eq!(s.screens_seen, 1);
        assert_eq!(s.widgets_never_clicked, vec!["Quit".to_string()]);
        assert_eq!(s.keys_never_pressed, vec!["Enter".to_string()]);
        assert!(s.verdict.starts_with("plateaued"), "{}", s.verdict);
    }
}
