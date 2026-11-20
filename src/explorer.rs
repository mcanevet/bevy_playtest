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

/// Modifiers the explorer holds under another key for a combo.
pub const COMBO_MODIFIERS: [&str; 3] = ["ControlLeft", "ShiftLeft", "AltLeft"];

/// One key press in this many is a modifier+key combo.
pub const COMBO_EVERY: u64 = 4;

/// Whether `key` (canonical name) is a modifier: never a combo's base.
pub(crate) fn is_modifier(key: &str) -> bool {
    ["Control", "Shift", "Alt", "Super"]
        .iter()
        .any(|m| key.starts_with(m))
}

/// One mouse action in this many is a drag (press, move, release) when
/// a node on screen declares drag handling...
pub const DRAG_EVERY: u64 = 4;

/// ...and one in this many otherwise: a blind probe for drags handled
/// by global observers. A drag costs a click (it is no click, and it
/// holds the mouse for `DRAG_FRAMES`), so it stays rare where nothing
/// says it helps.
pub const BLIND_DRAG_EVERY: u64 = 16;

/// Longest attribution window, in frames: a reaction is credited to a
/// key press or a random click when it comes within the window after
/// it, and no input of the other device happened in the window before
/// or since. The window is one input tick minus a frame, capped
/// here.
///
/// Explore mode alternates input ticks between keyboard and mouse (a
/// device with nothing to do yields its tick), so the other device's
/// last input is at least a tick away: a key's effect lands a frame
/// or two after it, a click's within about four, and both stay
/// attributable. With every tick shared, no window would ever be clean.
pub const HIT_CREDIT_FRAMES: u64 = 10;

/// Frames after a key press or release within which the game shows its
/// effect (the input is read next frame, a state change applies the
/// frame after). Key activity earlier than this before a click cannot
/// explain the click's reaction.
pub const KEY_EFFECT_FRAMES: u64 = 3;

/// Tries on one screen after which a key that was never followed by
/// novelty stops pressing on resting keyboard turns (it still presses on
/// the others).
pub const STALE_AFTER_TRIES: u32 = 3;

/// Shortest turn, in frames: a click's reaction lands up to about four
/// frames after the press (the click fires on release), so a turn and
/// its attribution window must cover that even at high input rates.
pub const MIN_TURN_FRAMES: u64 = 5;

/// Credited clicks on one node archetype before every node of that
/// archetype becomes a click target (two, so one coincidence is not
/// enough). A click followed by a state transition counts for all of
/// them at once: that is rarely a coincidence, and a button whose only
/// effect is leaving the screen (mines' "new game") gets no second
/// chance on the same screen.
pub const LEARN_AFTER: u32 = 2;

/// Crate roots of the engine (every Bevy 0.20 crate in the lockfile), the
/// standard library and this harness: components from these are not the
/// game's own. A game crate may well be named `bevy_something`, so this
/// is a list, not a prefix.
const ENGINE_CRATES: &[&str] = &[
    "bevy",
    "bevy_a11y",
    "bevy_android",
    "bevy_animation",
    "bevy_animation_macros",
    "bevy_anti_alias",
    "bevy_app",
    "bevy_asset",
    "bevy_asset_macros",
    "bevy_audio",
    "bevy_camera",
    "bevy_clipboard",
    "bevy_color",
    "bevy_core_pipeline",
    "bevy_curve",
    "bevy_derive",
    "bevy_dev_tools",
    "bevy_diagnostic",
    "bevy_ecs",
    "bevy_ecs_macro_logic",
    "bevy_ecs_macros",
    "bevy_encase_derive",
    "bevy_extract",
    "bevy_extract_macros",
    "bevy_feathers",
    "bevy_gilrs",
    "bevy_gizmos",
    "bevy_gizmos_macros",
    "bevy_gizmos_render",
    "bevy_gltf",
    "bevy_image",
    "bevy_input",
    "bevy_input_focus",
    "bevy_internal",
    "bevy_light",
    "bevy_log",
    "bevy_macro_utils",
    "bevy_material",
    "bevy_material_macros",
    "bevy_math",
    "bevy_mesh",
    "bevy_pbr",
    "bevy_picking",
    "bevy_platform",
    "bevy_playtest",
    "bevy_post_process",
    "bevy_ptr",
    "bevy_reflect",
    "bevy_reflect_derive",
    "bevy_render",
    "bevy_render_macros",
    "bevy_scene",
    "bevy_scene_macros",
    "bevy_shader",
    "bevy_shape",
    "bevy_sprite",
    "bevy_sprite_render",
    "bevy_state",
    "bevy_state_macros",
    "bevy_tasks",
    "bevy_text",
    "bevy_time",
    "bevy_transform",
    "bevy_ui",
    "bevy_ui_render",
    "bevy_ui_widgets",
    "bevy_utils",
    "bevy_window",
    "bevy_winit",
    "bevy_world_serialization",
    "alloc",
    "core",
    "std",
];

/// Whether a component type path belongs to the engine, the standard
/// library or this harness rather than the game.
pub(crate) fn is_engine_type_path(path: &str) -> bool {
    let root = path.split("::").next().unwrap_or(path);
    let root = root.split('<').next().unwrap_or(root);
    ENGINE_CRATES.contains(&root)
}

/// Name prefix of targets that are only known through learning.
pub(crate) const LEARNED_PREFIX: &str = "~";

/// A UI node archetype: sorted component ids minus hierarchy links
/// (see `discovery::ui_surface::archetype_key`).
pub(crate) type ArchetypeKey = Vec<usize>;

/// Keys tried once on every new screen even when the game's sources do
/// not mention them (bindings from config files, plugin crates, macros):
/// the probe prior minus Escape (reserved for backing out) and Tab
/// (focus cycling derails blind input).
fn prior_keys() -> impl Iterator<Item = &'static str> {
    crate::discovery::probe_scenarios::PROBE_CANDIDATE_KEYS
        .iter()
        .copied()
        .filter(|k| !matches!(*k, "Escape" | "Tab"))
}

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
    /// Keys from the built-in prior (not in the declared key surface)
    /// that were followed within half a second by a new state or screen
    /// with no mouse press in between, sorted. "Followed by", not
    /// proven to cause it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys_found: Vec<String>,
    /// Distinct modifier+key combos pressed (e.g. `ControlLeft+KeyA`).
    #[serde(default)]
    pub combos_tried: usize,
    /// Combos followed within half a second by a new state or screen,
    /// sorted ("followed by", like `keys_found`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub combos_found: Vec<String>,
    /// Node archetypes learned to respond to clicks: nodes with no
    /// click declaration (a grid of cells handled by one global
    /// observer) that the explorer then targets like widgets.
    #[serde(default)]
    pub learned_targets: usize,
    /// Mouse drags made (press, move while held, release elsewhere).
    #[serde(default)]
    pub drags: u64,
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
    /// Declared key surface (source scan or scenario), as last offered.
    declared: BTreeSet<String>,
    /// Keys followed by novelty (kept in rotation).
    productive: BTreeSet<String>,
    /// Combos followed by novelty.
    productive_combos: BTreeSet<String>,
    combos_pressed: BTreeSet<String>,
    /// Key presses so far (singles and combos): paces the combos.
    presses: u64,
    /// Mouse actions so far (clicks and drags): paces the drags.
    mouse_actions: u64,
    drags: u64,
    /// Frame of a random-position press whose hovered nodes are not
    /// read yet (read one frame later, after picking ran).
    probe_press: Option<u64>,
    /// Hovered node archetypes of the last random press, awaiting a
    /// reaction: (press frame, archetypes).
    pending_hit: Option<(u64, Vec<ArchetypeKey>)>,
    hit_credits: BTreeMap<ArchetypeKey, u32>,
    /// Frame of the last mouse press (clicks and drags).
    last_mouse_press: Option<u64>,
    /// Frame of the last key activity of any kind (a press or a release;
    /// declared, prior, combo, Escape back-out).
    last_any_key: Option<u64>,
    /// Frame of the last key press (not release).
    last_key_press: Option<u64>,
    /// Frame the current mouse hold (click or drag) releases.
    mouse_hold_until: Option<u64>,
    /// Frame the last drag releases (a drop's reaction comes after it).
    drag_release: Option<u64>,
    /// Attribution window in frames (see [`HIT_CREDIT_FRAMES`]).
    window: u64,
    pub(crate) learned: BTreeSet<ArchetypeKey>,
    /// Last key press: (canonical name, or `Mod+Key` for a combo; frame).
    last_key: Option<(String, u64)>,
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
            window: HIT_CREDIT_FRAMES,
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
        // Learned targets (see LEARNED_PREFIX) are left out of the screen
        // identity: learning them changes what the bot knows, not the
        // screen it is on.
        let mut ids: Vec<String> = targets
            .iter()
            .filter(|(id, disabled)| !disabled && !id.starts_with(LEARNED_PREFIX))
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
            // Credited to the last key when it came within the window
            // and no mouse press happened from a window before the key
            // until now (a click's effect lands a few frames late), so
            // `keys_found` only lists keys that were the only recent
            // input. The latest press is enough to check: any press
            // after the key would be the latest.
            let window = self.window;
            // A drag holds the mouse for two ticks: its motion and release
            // count as mouse activity until it lets go.
            // A drop fires on release, so a drag's release gets its own
            // window, like a click's press.
            let mouse_quiet = |at: u64| {
                self.last_mouse_press.is_none_or(|m| m + window < at)
                    && self.mouse_hold_until.is_none_or(|r| r < at)
                    && self.drag_release.is_none_or(|r| r + window < at)
            };
            if let Some((key, at)) = &self.last_key {
                if frame.saturating_sub(*at) <= window && mouse_quiet(*at) {
                    if key.contains('+') {
                        self.productive_combos.insert(key.clone());
                    } else {
                        self.productive.insert(key.clone());
                    }
                }
            }
        }
    }

    /// The keys to choose from on the current screen: the declared
    /// surface, plus prior keys not yet tried on this screen (one try
    /// each), plus any key that was followed by novelty before.
    pub(crate) fn key_pool(&mut self, declared: &[&str]) -> Vec<String> {
        // Canonical names, like every other key set here: a surface may
        // spell keys by alias ("w", "space", "esc").
        self.declared = declared
            .iter()
            .map(|k| {
                crate::scenario::normalize_key_name(k)
                    .map(str::to_string)
                    .unwrap_or_else(|| k.to_string())
            })
            .collect();
        let mut pool: BTreeSet<String> = self.declared.clone();
        pool.extend(self.productive.iter().cloned());
        if let Some(screen) = self.screen {
            for key in prior_keys() {
                if !self.tries.contains_key(&(screen, format!("key:{key}"))) {
                    pool.insert(key.to_string());
                }
            }
        }
        pool.into_iter().collect()
    }

    /// A key (or `Mod+Key` combo) was actually pressed at `frame`.
    pub(crate) fn note_key_press(&mut self, key: &str, frame: u64) {
        self.presses += 1;
        self.last_key = Some((key.to_string(), frame));
        self.last_any_key = Some(frame);
        self.last_key_press = Some(frame);
    }

    /// A key was released at `frame`: key activity too (a game may act
    /// on `just_released`), so it makes a nearby click ambiguous.
    pub(crate) fn note_key_release(&mut self, frame: u64) {
        self.last_any_key = Some(frame);
    }

    /// Whether `key` (canonical) is a probe: neither declared nor already
    /// productive. Probes are tapped (shortest hold), so their release
    /// stays out of the mouse's turn.
    pub(crate) fn is_probe_key(&self, key: &str) -> bool {
        !self.declared.contains(key) && !self.productive.contains(key)
    }

    /// Whether `key` (canonical) still earns resting keyboard turns: it
    /// was followed by novelty before, or has been tried fewer than
    /// [`STALE_AFTER_TRIES`] times on this screen.
    pub(crate) fn is_fresh_key(&self, key: &str) -> bool {
        if self.productive.contains(key) {
            return true;
        }
        let Some(screen) = self.screen else {
            return true;
        };
        self.tries
            .get(&(screen, format!("key:{key}")))
            .copied()
            .unwrap_or(0)
            < STALE_AFTER_TRIES
    }

    /// Whether the driver should re-scan the UI now: at the end of the
    /// last key press's window, so a panel the key opened without a state
    /// change is seen while the key can still be credited.
    pub(crate) fn recapture_due(&self, frame: u64) -> bool {
        self.last_key_press
            .is_some_and(|k| k + self.window == frame)
    }

    /// Whether a drag holds the mouse at `frame`: then the keyboard may
    /// use the mouse's turn (declared keys only, see the driver).
    pub(crate) fn mouse_busy(&self, frame: u64) -> bool {
        self.drag_release.is_some_and(|r| r > frame)
    }

    /// Length of one device's turn: a whole number of input ticks, at
    /// least [`MIN_TURN_FRAMES`]. Each device presses once per turn, on
    /// its first tick, so at any rate the other device's input is a
    /// full turn away.
    fn turn(interval: u64) -> u64 {
        let interval = interval.max(1);
        MIN_TURN_FRAMES.div_ceil(interval).max(1) * interval
    }

    /// Input cadence: one tick every `interval` frames. The attribution
    /// window is a turn minus one frame (capped at [`HIT_CREDIT_FRAMES`]),
    /// so it never reaches the other device's next turn.
    pub(crate) fn set_cadence(&mut self, interval: u64) {
        self.window = (Self::turn(interval) - 1).min(HIT_CREDIT_FRAMES);
    }

    /// Whether `frame` is the keyboard's tick (the first tick of an even
    /// turn). A device with nothing to do yields its tick.
    pub(crate) fn keyboard_turn(frame: u64, interval: u64) -> bool {
        frame.is_multiple_of(2 * Self::turn(interval))
    }

    /// The first frame at or after `at` close enough after a keyboard
    /// tick that a release there has played out (within
    /// [`KEY_EFFECT_FRAMES`]) before the mouse's next tick: where an
    /// explore-mode key release goes. Taps released right after their
    /// press already qualify.
    pub(crate) fn release_frame(at: u64, interval: u64) -> u64 {
        let turn = Self::turn(interval);
        let cycle = 2 * turn;
        let max_offset = turn.saturating_sub(KEY_EFFECT_FRAMES + 1);
        let offset = at % cycle;
        if offset <= max_offset {
            at
        } else {
            at + (cycle - offset)
        }
    }

    /// Whether `frame` is the mouse's tick (the first tick of an odd turn).
    pub(crate) fn mouse_turn(frame: u64, interval: u64) -> bool {
        let turn = Self::turn(interval);
        frame % (2 * turn) == turn
    }

    /// Whether `frame` falls in a resting keyboard turn (every other
    /// one), when probes wait on screens with click targets.
    pub(crate) fn probe_rest(frame: u64, interval: u64) -> bool {
        !(frame / (2 * Self::turn(interval))).is_multiple_of(2)
    }

    /// Whether the keyboard pressed something on `frame` (then the
    /// mouse, which runs after it, leaves the tick alone).
    pub(crate) fn keyboard_acted(&self, frame: u64) -> bool {
        self.last_key_press == Some(frame)
    }

    /// The attribution window in frames.
    pub(crate) fn window(&self) -> u64 {
        self.window
    }

    /// Press frame of the click awaiting a reaction, if any.
    pub(crate) fn pending_press(&self) -> Option<u64> {
        self.pending_hit.as_ref().map(|(press, _)| *press)
    }

    /// Whether the next key press should be a combo (every
    /// [`COMBO_EVERY`]th press). Depends only on presses delivered, so
    /// replays make the same decision.
    pub(crate) fn combo_due(&self) -> bool {
        self.presses % COMBO_EVERY == COMBO_EVERY - 1
    }

    /// Whether the next mouse action should be a drag: every
    /// [`DRAG_EVERY`]th when `draggable_on_screen`, else every
    /// [`BLIND_DRAG_EVERY`]th. Depends only on actions delivered and
    /// the enumerated targets, so replays decide the same way.
    pub(crate) fn drag_due(&self, draggable_on_screen: bool) -> bool {
        let every = if draggable_on_screen {
            DRAG_EVERY
        } else {
            BLIND_DRAG_EVERY
        };
        self.mouse_actions % every == every - 1
    }

    /// A mouse action (a drag when `drag`) was delivered.
    pub(crate) fn note_mouse_action(&mut self, drag: bool) {
        if drag {
            self.drags += 1;
        }
        self.mouse_actions += 1;
    }

    /// A mouse press was delivered; `random` when it went to a random
    /// position rather than a known widget. Any press drops the
    /// previous pending hit: a later reaction could be its doing.
    /// `release_at` is the frame the hold ends (later for a drag).
    pub(crate) fn note_press(&mut self, frame: u64, random: bool, release_at: u64, drag: bool) {
        self.last_mouse_press = Some(frame);
        self.mouse_hold_until = Some(release_at);
        if drag {
            self.drag_release = Some(release_at);
        }
        self.probe_press = random.then_some(frame);
        self.pending_hit = None;
    }

    /// Whether the driver should read the hovered nodes now (the frame
    /// after a random press, once picking has seen the cursor).
    pub(crate) fn hover_due(&self, frame: u64) -> bool {
        self.probe_press.is_some_and(|p| frame == p + 1)
    }

    /// The archetypes hovered by the last random press.
    pub(crate) fn set_hit(&mut self, keys: Vec<ArchetypeKey>) {
        if let Some(press) = self.probe_press.take() {
            if !keys.is_empty() {
                self.pending_hit = Some((press, keys));
            }
        }
    }

    /// End of frame: credit the pending hit if the game reacted (a state
    /// transition counts fully, see [`LEARN_AFTER`]), drop it when its
    /// window has passed or a key press makes it ambiguous.
    pub(crate) fn after_reaction(&mut self, frame: u64, reacted: bool, state_changed: bool) {
        let Some((press, keys)) = &self.pending_hit else {
            return;
        };
        // A key press or release whose effect can still land after the
        // click (from KEY_EFFECT_FRAMES before it until now) could explain
        // the reaction: no credit at all then. Earlier key activity has
        // played out before the click's reaction can show.
        let window = self.window;
        if self
            .last_any_key
            .is_some_and(|k| k + KEY_EFFECT_FRAMES >= *press)
        {
            self.pending_hit = None;
            return;
        }
        if (reacted || state_changed) && frame > *press {
            let credit = if state_changed { LEARN_AFTER } else { 1 };
            for key in keys {
                let credits = self.hit_credits.entry(key.clone()).or_default();
                *credits += credit;
                if *credits >= LEARN_AFTER {
                    self.learned.insert(key.clone());
                }
            }
            self.pending_hit = None;
        } else if frame >= press + window {
            self.pending_hit = None;
        }
    }

    /// Keys a combo may hold a modifier over: the declared surface and
    /// the productive keys, minus modifiers and Escape. Prior keys are
    /// left out on purpose: 3 modifiers x 27 keys would swamp the
    /// declared ones.
    pub(crate) fn combo_bases(&self) -> Vec<String> {
        self.declared
            .iter()
            .chain(self.productive.iter())
            .filter(|k| !is_modifier(k) && k.as_str() != "Escape")
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
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
        // What follows is Escape's doing, not the previous key's; and
        // it is a key press as far as click learning is concerned.
        self.last_key = None;
        self.last_any_key = Some(frame);
        self.last_key_press = Some(frame);
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
            "combo" => {
                self.combos_pressed.insert(name.to_string());
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
                .filter(|k| self.declared.contains(*k) && !self.keys_pressed.contains(*k))
                .cloned()
                .collect(),
            keys_found: self
                .productive
                .iter()
                .filter(|k| !self.declared.contains(*k))
                .cloned()
                .collect(),
            combos_tried: self.combos_pressed.len(),
            combos_found: self.productive_combos.iter().cloned().collect(),
            drags: self.drags,
            learned_targets: self.learned.len(),
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
    fn prior_keys_get_one_try_per_screen_and_productive_ones_stay() {
        let mut e = Explorer::new(true);
        e.observe(0, &[], &t(&["Play"]));
        let pool = e.key_pool(&["Space"]);
        assert!(pool.contains(&"Space".to_string()));
        assert!(
            pool.contains(&"Digit3".to_string()),
            "prior offered on a new screen"
        );
        assert!(!pool.iter().any(|k| k == "Escape" || k == "Tab"));
        // Digit3 tried once here, followed by a new state: it stays.
        e.record("key", "KeyQ");
        e.note_key_press("KeyQ", 5);
        e.record("key", "Digit3");
        e.note_key_press("Digit3", 10);
        e.observe(20, &["OnEnter(Level)".into()], &t(&["Play"]));
        let pool = e.key_pool(&["Space"]);
        assert!(pool.contains(&"Digit3".to_string()), "productive key stays");
        // A prior key tried once on this screen without effect drops out.
        e.record("key", "Digit7");
        assert!(!e.key_pool(&["Space"]).contains(&"Digit7".to_string()));
        assert_eq!(e.summary().keys_found, vec!["Digit3".to_string()]);
    }

    #[test]
    fn learning_needs_two_reactions_or_one_state_change() {
        let mut e = Explorer::new(true);
        let cell = vec![1, 2, 3];
        let button = vec![1, 2, 4];
        // Random press at 10, hovered read at 11, resource reaction at 13.
        e.note_press(10, true, 12, false);
        assert!(e.hover_due(11));
        e.set_hit(vec![cell.clone()]);
        e.after_reaction(13, true, false);
        assert!(e.learned.is_empty(), "one coincidence is not enough");
        e.note_press(20, true, 22, false);
        e.set_hit(vec![cell.clone()]);
        e.after_reaction(23, true, false);
        assert!(e.learned.contains(&cell));
        // A state transition after a click learns at once.
        e.note_press(30, true, 32, false);
        e.set_hit(vec![button.clone()]);
        e.after_reaction(33, false, true);
        assert!(e.learned.contains(&button));
        // No reaction within the window: nothing learned.
        let other = vec![9];
        e.note_press(40, true, 42, false);
        e.set_hit(vec![other.clone()]);
        e.after_reaction(40 + HIT_CREDIT_FRAMES, false, false);
        e.after_reaction(60, true, true);
        assert!(!e.learned.contains(&other));
    }

    #[test]
    fn review_fixes_back_out_alias_key_near_click_learned_names() {
        // Escape back-out: the next novelty is not the previous key's.
        let mut e = Explorer::new(true);
        e.observe(0, &[], &t(&["Play"]));
        e.observe(1, &[], &t(&["Resume"]));
        e.note_key_press("Digit7", 1000);
        e.backed_out(1020);
        e.observe(1021, &["OnEnter(Menu)".into()], &t(&["Resume"]));
        assert!(e.summary().keys_found.is_empty(), "{:?}", e.summary());

        // Aliased surface keys are canonical in the reports.
        let mut e = Explorer::new(true);
        e.observe(0, &[], &t(&["Play"]));
        let pool = e.key_pool(&["w", "space"]);
        e.offer_keys(&pool.iter().map(String::as_str).collect::<Vec<_>>());
        e.record("key", "Space");
        e.note_key_press("Space", 10);
        e.observe(15, &["OnEnter(Game)".into()], &t(&["Play"]));
        let summary = e.summary();
        assert!(
            summary.keys_found.is_empty(),
            "Space is declared: {summary:?}"
        );
        assert_eq!(summary.keys_never_pressed, vec!["KeyW".to_string()]);

        // A key pressed around a click: a state change is one credit only.
        let mut e = Explorer::new(true);
        let panel = vec![7, 8];
        e.note_press(100, true, 102, false);
        e.set_hit(vec![panel.clone()]);
        e.note_key_press("Space", 103);
        e.after_reaction(104, true, true);
        assert!(!e.learned.contains(&panel));

        // Learned targets do not change the screen identity.
        let mut e = Explorer::new(true);
        e.observe(0, &[], &t(&["Play"]));
        e.observe(5, &[], &t(&["Play", "~<unknown>", "~<unknown>#2"]));
        assert_eq!(e.summary().screens_seen, 1);
    }

    #[test]
    fn engine_crates_are_a_list_not_a_prefix() {
        assert!(is_engine_type_path("bevy_ui::ui_node::Node"));
        assert!(is_engine_type_path("bevy_playtest::driver::Marker"));
        assert!(is_engine_type_path("core::option::Option<u8>"));
        assert!(!is_engine_type_path("bevy_minesweeper::Cell"));
        assert!(!is_engine_type_path("fixture_mines::TileCell"));
    }

    #[test]
    fn turns_cover_a_click_even_at_high_rates() {
        let mut e = Explorer::new(true);
        // 30 Hz at 60 tps: a turn is 3 ticks (6 frames), one press each.
        e.set_cadence(2);
        assert_eq!(e.window(), 5, "a click's reaction needs ~4 frames");
        assert!(Explorer::keyboard_turn(0, 2) && Explorer::mouse_turn(6, 2));
        assert!(!Explorer::keyboard_turn(2, 2) && !Explorer::keyboard_turn(4, 2));
        assert!(!Explorer::mouse_turn(8, 2) && Explorer::keyboard_turn(12, 2));
        // Every tick at 60 Hz: 5-frame turns.
        e.set_cadence(1);
        assert_eq!(e.window(), 4);
        assert!(Explorer::keyboard_turn(10, 1) && Explorer::mouse_turn(5, 1));
        // Default rate: alternate ticks.
        e.set_cadence(6);
        assert_eq!(e.window(), 5);
        assert!(Explorer::keyboard_turn(0, 6) && Explorer::mouse_turn(6, 6));
        assert!(Explorer::keyboard_turn(12, 6) && !Explorer::mouse_turn(12, 6));
    }

    #[test]
    fn drags_releases_and_rescans_keep_attribution_honest() {
        // A key pressed while a drag still holds the mouse: no credit.
        let mut e = Explorer::new(true);
        e.set_cadence(6);
        e.observe(0, &[], &t(&["Play"]));
        e.note_press(6, false, 18, true);
        assert!(e.mouse_busy(12));
        e.note_key_press("Digit4", 12);
        e.observe(14, &["OnEnter(Inventory)".into()], &t(&["Play"]));
        assert!(e.summary().keys_found.is_empty(), "{:?}", e.summary());

        // A key released around a random click makes it ambiguous.
        let mut e = Explorer::new(true);
        e.set_cadence(6);
        let cell = vec![1, 2];
        e.note_press(30, true, 32, false);
        e.set_hit(vec![cell.clone()]);
        e.note_key_release(31);
        e.after_reaction(33, true, true);
        assert!(!e.learned.contains(&cell));

        // The UI is re-scanned at the end of a key's window.
        let mut e = Explorer::new(true);
        e.set_cadence(6);
        e.note_key_press("KeyI", 60);
        assert!(!e.recapture_due(64) && e.recapture_due(65));
    }

    #[test]
    fn a_tapped_key_released_before_the_click_does_not_spoil_it() {
        // Keyboard tick at 0 taps a probe (released at 2); the mouse
        // clicks at 6 and the game reacts at 9: the release played out.
        let mut e = Explorer::new(true);
        e.set_cadence(6);
        let tile = vec![4, 5];
        e.note_key_press("Digit3", 0);
        e.note_key_release(2);
        e.note_press(6, true, 8, false);
        e.set_hit(vec![tile.clone()]);
        e.after_reaction(9, false, true);
        assert!(e.learned.contains(&tile));
    }

    #[test]
    fn key_releases_land_right_after_keyboard_ticks() {
        // Default rate: turn 6, cycle 12, releases at offsets 0..=2.
        assert_eq!(Explorer::release_frame(2, 6), 2, "a tap stays");
        assert_eq!(Explorer::release_frame(3, 6), 12);
        assert_eq!(Explorer::release_frame(25, 6), 25);
        assert_eq!(Explorer::release_frame(27, 6), 36);
        // Shortest turn (5 frames): offsets 0..=1 only.
        assert_eq!(Explorer::release_frame(2, 1), 10);
        assert_eq!(Explorer::release_frame(11, 1), 11);
    }

    #[test]
    fn a_click_near_a_key_press_takes_away_its_credit() {
        let mut e = Explorer::new(true);
        e.observe(0, &[], &t(&["Play"]));
        // Key at 100, click at 104, new state at 110: ambiguous.
        e.note_key_press("Digit5", 100);
        e.note_press(104, false, 106, false);
        e.observe(110, &["OnEnter(Game)".into()], &t(&["Play"]));
        // A click shortly BEFORE the key lands after it: ambiguous too.
        e.note_press(195, false, 197, false);
        e.note_key_press("KeyF", 200);
        e.observe(205, &["OnEnter(Pause)".into()], &t(&["Play"]));
        assert!(e.summary().keys_found.is_empty(), "{:?}", e.summary());
        // Mouse quiet around the key: credited.
        e.note_key_press("Digit3", 300);
        e.observe(310, &["OnEnter(Level)".into()], &t(&["Play"]));
        assert_eq!(e.summary().keys_found, vec!["Digit3".to_string()]);
    }

    #[test]
    fn summary_reports_gaps_and_verdict() {
        let mut e = Explorer::new(true);
        e.observe(0, &["OnEnter(Menu)".into()], &t(&["Play", "Quit"]));
        e.key_pool(&["Space", "Enter"]);
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
