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
//! - **Mouse cell runs (additive MVP):** the same `synthetic_keyboard`
//!   bot delivering ONE scripted left click at the center of one
//!   grid cell (`bot.mouse_clicks`), plus a mouse CONTROL run
//!   clicking a canonical position. Cell probe and mouse control
//!   differ by exactly one bit: WHERE the click lands.
//!
//! Every generated scenario opts into `record_digest: true` (the
//! per-frame opaque u64 series is the diff input) and runs
//! `single_threaded` with simulated time at a fixed `tps`, so probe
//! and control traces are frame-aligned.

use crate::enums::BotType;
use crate::scenario::{
    validate_scenario, BotConfig, KeyPressInput, MouseClickInput, Scenario, ScenarioError,
};

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

/// Default mouse probing grid: 5 columns × 4 rows = 20 cells labeled
/// `r{row}c{col}` (0-based), clicked at cell CENTERS. Coarse on
/// purpose: the MVP asks "does the game react to a click AT ALL", not
/// "which screen region" — one hit anywhere in a cell lights the
/// cell, and the normalized coordinates travel with every finding so
/// finer grids can refine without reinterpreting old artifacts.
pub const MOUSE_GRID_COLS: u32 = 5;
pub const MOUSE_GRID_ROWS: u32 = 4;

/// Logical window HEIGHT mouse cell coordinates are scaled against
/// (see [`MOUSE_CLICK_EXTENT_W_PX`]). Per-axis, deliberately: the
/// probe children run headless without winit, so there is no real
/// window to measure, and a square-extent assumption would push
/// bottom-row cells of a 4-row grid to y = 1120 px — off the
/// 1280×720 window entirely, where picking silently drops the click.
pub const MOUSE_CLICK_EXTENT_H_PX: f32 = 720.0;

/// Logical window WIDTH mouse cell coordinates are scaled against at
/// scenario-BUILD time (`scripted_mouse_system` delivers
/// `click.x`/`click.y` verbatim as window-space logical pixels
/// through the same wrapped-event path the picking fixtures read, so
/// the plan must commit to one assumed geometry up front).
/// Deliberately DISTINCT from the chaos bot's
/// `HEADLESS_CLICK_EXTENT` fallback: that constant feeds a different
/// delivery layer (normalized-coordinate sampling in the live
/// takeover), and coupling them would let a change in either
/// silently reshape the other's probe semantics.
pub const MOUSE_CLICK_EXTENT_W_PX: f32 = 1280.0;

/// Mouse cell control position (normalized window coordinates):
/// dead center of the window. Every cell probe diff is taken
/// against a REAL click at this position — the exact mouse analogue
/// of the key control press — so probe and control differ by
/// exactly one bit: WHERE the click lands.
pub const MOUSE_CONTROL_NORM_X: f32 = 0.5;
pub const MOUSE_CONTROL_NORM_Y: f32 = 0.5;

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

/// One mouse probe cell of the candidate grid (see
/// [`MOUSE_GRID_COLS`]/[`MOUSE_GRID_ROWS`]): WHERE the scripted click
/// lands. Cells are identified `r{row}c{col}` (0-based); the
/// normalized coordinates are the CELL CENTER in window space
/// (`(col + 0.5) / cols`, `(row + 0.5) / rows`), so findings are
/// comparable across grid resolutions and window sizes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MouseCell {
    /// Grid row (0-based, top to bottom).
    pub row: u32,
    /// Grid column (0-based, left to right).
    pub col: u32,
    /// Normalized window x of the cell center (0.0..=1.0).
    pub norm_x: f32,
    /// Normalized window y of the cell center (0.0..=1.0).
    pub norm_y: f32,
}

impl MouseCell {
    /// The `r{row}c{col}` identifier (0-based, non-negative: safe for
    /// scenario names, which the schema restricts to
    /// `[A-Za-z0-9_-]`).
    pub fn id(&self) -> String {
        format!("r{}c{}", self.row, self.col)
    }

    /// A cell at the given grid position, with its center computed
    /// against the given grid EXTENT (columns × rows). Division by
    /// zero is impossible through this constructor (extents come
    /// from the plan, which validates non-zero when cells exist).
    pub fn at(row: u32, col: u32, cols: u32, rows: u32) -> Self {
        Self {
            row,
            col,
            norm_x: (col as f32 + 0.5) / cols as f32,
            norm_y: (row as f32 + 0.5) / rows as f32,
        }
    }
}

/// The two MVP probe points (Strategy A, third point dropped — see
/// design doc "Accounting"):
///
/// - **Readiness:** `pre_ready_frames + tps` (≈1s after startup;
///   covers menu/title listeners).
/// - **Mid-duration:** `duration_s / 2 * tps` (covers
///   post-transition listeners).
///
/// The mid point is CLAMPED to `readiness + 1` so the two points can
/// never coincide (ordered points are a structural requirement), but
/// a clamped mid no longer covers "post-transition listeners" at all
/// — it samples roughly the same instant as the readiness point.
/// [`ProbePlan::points`] therefore REJECTS a plan whose mid clamp
/// engages, rather than silently burning half the matrix on a
/// redundant instant.
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

/// Whether the "mid" point's [`probe_points`] clamp relocated it from
/// its nominal `duration_s / 2 * tps` position to `readiness + 1`
/// (true whenever the clamp engaged). Then the mid point sits ~1
/// frame after the readiness point and covers no additional regime;
/// [`ProbePlan::points`] refuses such plans loudly.
pub fn mid_point_is_clamped(duration_s: f32, tps: u32) -> bool {
    let readiness = PRE_READY_FRAMES + tps as u64;
    let nominal_mid = ((duration_s / 2.0) * tps as f32) as u64;
    nominal_mid < readiness + 1
}

/// Total ticks of a run lasting `duration_s` at `tps` (the same
/// ceiling `validate_scenario` applies to authored scenarios).
/// Digests are recorded once per tick at frames `0..=ticks-1` (see
/// the driver's tick loop), so a COMPLETE run's last digest frame
/// is `ticks - 1` — surfaced via [`ProbePlan::expected_last_frame`].
fn total_ticks(duration_s: f32, tps: u32) -> u64 {
    (duration_s * tps as f32).ceil() as u64
}

/// The configuration of the probe run matrix. Cheap to construct;
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
    /// Mouse probing grid: `cols` × `rows` cells clicked at their
    /// centers, plus one doubled control run at the window center.
    /// `(0, 0)` disables mouse probing entirely — no cell jobs and
    /// no mouse control jobs (tests slim the matrix this way; the
    /// keyboard pipeline is unaffected).
    pub mouse_grid_cols: u32,
    /// See [`Self::mouse_grid_cols`].
    pub mouse_grid_rows: u32,
    /// Assumed window WIDTH (logical px) mouse x coordinates are
    /// scaled against at scenario-build time
    /// ([`MOUSE_CLICK_EXTENT_W_PX`] by default).
    pub mouse_click_extent_w_px: f32,
    /// Assumed window HEIGHT (logical px) mouse y coordinates are
    /// scaled against at scenario-build time
    /// ([`MOUSE_CLICK_EXTENT_H_PX`] by default).
    pub mouse_click_extent_h_px: f32,
    /// Normalized window x of the mouse control click
    /// ([`MOUSE_CONTROL_NORM_X`] by default: window dead-center).
    pub mouse_control_norm_x: f32,
    /// Normalized window y of the mouse control click
    /// ([`MOUSE_CONTROL_NORM_Y`] by default).
    pub mouse_control_norm_y: f32,
}

impl Default for ProbePlan {
    fn default() -> Self {
        Self {
            candidate_keys: PROBE_CANDIDATE_KEYS.iter().map(|s| s.to_string()).collect(),
            tps: PROBE_TPS,
            duration_s: PROBE_DURATION_S,
            control_key: DEFAULT_CONTROL_KEY.to_string(),
            mouse_grid_cols: MOUSE_GRID_COLS,
            mouse_grid_rows: MOUSE_GRID_ROWS,
            mouse_click_extent_w_px: MOUSE_CLICK_EXTENT_W_PX,
            mouse_click_extent_h_px: MOUSE_CLICK_EXTENT_H_PX,
            mouse_control_norm_x: MOUSE_CONTROL_NORM_X,
            mouse_control_norm_y: MOUSE_CONTROL_NORM_Y,
        }
    }
}

impl ProbePlan {
    /// The plan's mouse grid cells, row-major (`r0c0, r0c1, ...`), or
    /// empty when the grid is disabled (`0 × 0`). Iteration order is
    /// FIXED (row-major), so job lists, cache fingerprints and
    /// per-cell findings are byte-stable across runs.
    pub fn mouse_cells(&self) -> Vec<MouseCell> {
        let mut cells = Vec::new();
        for row in 0..self.mouse_grid_rows {
            for col in 0..self.mouse_grid_cols {
                cells.push(MouseCell::at(
                    row,
                    col,
                    self.mouse_grid_cols,
                    self.mouse_grid_rows,
                ));
            }
        }
        cells
    }

    /// A degenerate HALF-grid (columns without rows or vice versa)
    /// probes nothing and must die at plan-build time rather than
    /// silently schedule zero cells while claiming a grid.
    fn validate_mouse_grid(&self) -> Result<(), ScenarioError> {
        match (self.mouse_grid_cols, self.mouse_grid_rows) {
            (0, 0) => Ok(()), // disabled by config — legitimate
            (0, _) | (_, 0) => Err(ScenarioError::Rejected(format!(
                "mouse grid {}x{} is degenerate — a zero dimension \
                  disables mouse probing entirely, so both dimensions \
                  must be zero (off) or at least one",
                self.mouse_grid_cols, self.mouse_grid_rows
            ))),
            _ => Ok(()),
        }
    }

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
            mouse_grid_cols: self.mouse_grid_cols,
            mouse_grid_rows: self.mouse_grid_rows,
            mouse_click_extent_w_px: self.mouse_click_extent_w_px,
            mouse_click_extent_h_px: self.mouse_click_extent_h_px,
            mouse_control_norm_x: self.mouse_control_norm_x,
            mouse_control_norm_y: self.mouse_control_norm_y,
        })
    }

    /// The plan's probe points, VALIDATED to land inside the run: a
    /// point past the last tick would produce a tap that never
    /// fires, silently degrading the whole matrix into unattributable
    /// no-ops. A degenerate plan is rejected here, not silently run.
    pub fn points(&self) -> Result<[ProbePoint; 2], ScenarioError> {
        // Raw-math validation first: a point past the last tick would
        // produce a tap that never fires, silently degrading the
        // whole matrix into unattributable no-ops.
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
        // Then the clamp refusal: a clamped mid relocates to
        // `readiness + 1` and no longer covers the post-transition
        // regime its label promises — silently running it would burn
        // half the matrix probing ~one instant. (The raw
        // `probe_points` math stays permissive; plan validation is
        // where configs die.)
        if mid_point_is_clamped(self.duration_s, self.tps) {
            return Err(ScenarioError::Rejected(format!(
                "the 'mid' probe point collapses onto the readiness point \
                 (duration_s/2·tps <= readiness) in a {}s run at {} tps — \
                 the matrix would sample the same instant twice; \
                 lengthen the duration",
                self.duration_s, self.tps
            )));
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

    /// One scripted left-click at `(x, y)` logical px, at the
    /// readiness probe point's frame. The schema does NOT validate
    /// that mouse frames fit the run (it does for key presses), so
    /// every mouse scenario builder calls [`Self::validate_mouse_fit`]
    /// itself — a click whose frame+hold lands past the run's last
    /// tick would silently never fire and degrade the whole cell
    /// matrix into unattributable no-ops, the exact failure mode
    /// `points()` refuses for keys.
    fn click_input(frame: u64, x: f32, y: f32) -> MouseClickInput {
        MouseClickInput {
            frame,
            x,
            y,
            button: "left".to_string(),
            hold_frames: TAP_HOLD_FRAMES,
        }
    }

    /// Frame-safety for mouse scenarios (the schema's mouse-frame
    /// validation is nonexistent, unlike key presses): click frame +
    /// hold must fit strictly inside the run, mirroring the
    /// `points()` tap-fit check.
    fn validate_mouse_fit(&self, frame: u64) -> Result<(), ScenarioError> {
        let ticks = total_ticks(self.duration_s, self.tps);
        if frame + TAP_HOLD_FRAMES >= ticks {
            return Err(ScenarioError::Rejected(format!(
                "mouse click at frame {frame} does not fit a {}s run at {} \
                  tps ({} ticks) — lengthen the duration",
                self.duration_s, self.tps, ticks
            )));
        }
        Ok(())
    }

    /// Whether `cell`'s center coincides with THIS plan's mouse
    /// control position: probing the control position against itself
    /// yields a probe byte-identical to its baseline — the exact
    /// mouse analogue of the key control-key self-probe, and equally
    /// unclassifiable rather than a trusted negative. Compare on
    /// NORMALIZED COORDINATES, not labels: a 1×1 grid's single cell
    /// sits at the control position no matter what it is called.
    /// (Bitwise equality: both sides are computed by the same
    /// float expression, `MouseCell::at`, from the same dimensions.)
    pub fn mouse_cell_is_control_position(&self, cell: &MouseCell) -> bool {
        cell.norm_x == self.mouse_control_norm_x && cell.norm_y == self.mouse_control_norm_y
    }

    /// The mouse cell probe scenario for `cell`: a synthetic bot
    /// delivering ONE left click at the cell's center (px = normalized
    /// × the plan's assumed extent) at the readiness point's frame.
    /// No keys are pressed — validation accepts a `mouse_clicks`-only
    /// bot.
    pub fn mouse_cell_scenario(&self, cell: &MouseCell) -> Result<Scenario, ScenarioError> {
        let readiness = probe_points(self.duration_s, self.tps)[0].frame;
        self.validate_mouse_grid()?;
        self.validate_mouse_fit(readiness)?;
        let mut scenario = self.base_scenario(format!("probe_mouse_{}_readiness", cell.id()));
        scenario.bot.mouse_clicks = vec![Self::click_input(
            readiness,
            cell.norm_x * self.mouse_click_extent_w_px,
            // No flip: `norm_y` is top-to-bottom (row 0 = top of the
            // grid) and window-event y is ALSO top-to-bottom (winit
            // convention, preserved unchanged through CursorMoved,
            // bevy_picking, and bevy_ui's hit-test). Flipping here
            // would mirror every cell vertically.
            cell.norm_y * self.mouse_click_extent_h_px,
        )];
        validate_scenario(&scenario)?;
        Ok(scenario)
    }

    /// The normalized mouse CONTROL scenario: identical to the cell
    /// probes except the click lands at the canonical control
    /// position (window dead-center). Like key controls, `discover`
    /// schedules it TWICE — the determinism gate for the mouse
    /// matrix.
    pub fn mouse_control_scenario(&self) -> Result<Scenario, ScenarioError> {
        let readiness = probe_points(self.duration_s, self.tps)[0].frame;
        self.validate_mouse_grid()?;
        self.validate_mouse_fit(readiness)?;
        let mut scenario = self.base_scenario("probe_mouse_control_readiness".to_string());
        scenario.bot.mouse_clicks = vec![Self::click_input(
            readiness,
            self.mouse_control_norm_x * self.mouse_click_extent_w_px,
            // Same top-down convention as cell probes (see
            // `mouse_cell_scenario`): window-event y is top-to-bottom,
            // matching `norm_y` — no flip needed.
            self.mouse_control_norm_y * self.mouse_click_extent_h_px,
        )];
        validate_scenario(&scenario)?;
        Ok(scenario)
    }

    /// All mouse cell scenarios (row-major) followed by the control
    /// — NOT doubled here; the pool's job builder appends the second
    /// control copy. Empty (with no control) when the grid is
    /// disabled (`0 × 0`).
    pub fn mouse_runs(&self) -> Result<Vec<Scenario>, ScenarioError> {
        self.validate_mouse_grid()?;
        let cells = self.mouse_cells();
        if cells.is_empty() {
            // Grid disabled: no cell probes, no control, no doubling.
            return Ok(Vec::new());
        }
        let mut runs = Vec::with_capacity(cells.len() + 1);
        for cell in &cells {
            runs.push(self.mouse_cell_scenario(cell)?);
        }
        runs.push(self.mouse_control_scenario()?);
        Ok(runs)
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
    fn clamped_mid_point_is_rejected_loudly_not_run_silently() {
        // A run short enough that `duration_s/2·tps` falls at or
        // below `readiness + 1` used to SILENTLY relocate the "mid"
        // point next to the readiness point — two probe points ~1
        // frame apart, half the matrix sampling the same startup
        // instant while the label still claimed post-transition
        // coverage. The plan must refuse such a configuration
        // loudly. Boundary chosen so the readiness point still FITS
        // (the fit rejection would mask the clamp rejection
        // otherwise): 1.5s at 60 tps = 90 ticks > 60+1, but the
        // nominal mid (45) sits well below readiness.
        let plan = ProbePlan {
            duration_s: 1.5,
            ..Default::default()
        };
        let err = plan.points().unwrap_err().to_string();
        assert!(
            err.contains("collapses onto the readiness point"),
            "must explain the mid clamp: {err}"
        );
        assert!(plan.probe_runs().is_err());
        assert!(plan.control_runs().is_err());
        // The default plan (3s) never clamps and must stay valid.
        assert!(ProbePlan::default().points().is_ok());
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
        assert_eq!(canon.mouse_grid_cols, plan.mouse_grid_cols);
        assert_eq!(canon.mouse_grid_rows, plan.mouse_grid_rows);
        assert_eq!(canon.mouse_click_extent_w_px, plan.mouse_click_extent_w_px);
        assert_eq!(canon.mouse_click_extent_h_px, plan.mouse_click_extent_h_px);
        assert_eq!(canon.mouse_control_norm_x, plan.mouse_control_norm_x);
        assert_eq!(canon.mouse_control_norm_y, plan.mouse_control_norm_y);
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

    #[test]
    fn mouse_cells_cover_the_grid_row_major_at_centers() {
        let plan = ProbePlan::default();
        let cells = plan.mouse_cells();
        // 5 cols × 4 rows = 20 cells, row-major order (`r0c0, r0c1, …`).
        assert_eq!(cells.len(), (MOUSE_GRID_COLS * MOUSE_GRID_ROWS) as usize);
        assert_eq!(cells[0].id(), "r0c0");
        assert_eq!(cells[1].id(), "r0c1");
        assert_eq!(cells[MOUSE_GRID_COLS as usize].id(), "r1c0");
        assert_eq!(
            cells.last().unwrap().id(),
            format!("r{}c{}", MOUSE_GRID_ROWS - 1, MOUSE_GRID_COLS - 1)
        );
        // Cell centers are normalized against the GRID extent, so
        // they stay comparable across window sizes.
        assert!((cells[0].norm_x - (0.5 / MOUSE_GRID_COLS as f32)).abs() < f32::EPSILON);
        assert!((cells[0].norm_y - (0.5 / MOUSE_GRID_ROWS as f32)).abs() < f32::EPSILON);
        for cell in &cells {
            assert!((0.0..=1.0).contains(&cell.norm_x));
            assert!((0.0..=1.0).contains(&cell.norm_y));
        }
    }

    #[test]
    fn mouse_scenarios_validate_and_name_themselves_legibly() {
        let plan = ProbePlan::default();
        let runs = plan.mouse_runs().expect("default grid must build");
        // 20 cell probes + ONE control (the pool doubles it).
        assert_eq!(runs.len(), (MOUSE_GRID_COLS * MOUSE_GRID_ROWS) as usize + 1);
        for run in &runs {
            validate_scenario(run).expect("generated mouse scenario must validate");
            let name = run.name.as_deref().unwrap();
            // Scenario names must fit the schema's [A-Za-z0-9_-].
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
                "scenario name '{name}' must be schema-safe"
            );
            // Mouse-only bots: clicks present, no key presses.
            assert_eq!(run.bot.key_presses.len(), 0);
            assert_eq!(run.bot.mouse_clicks.len(), 1);
            let [click] = &run.bot.mouse_clicks[..] else {
                panic!("mouse runs carry exactly one click");
            };
            assert_eq!(click.hold_frames, TAP_HOLD_FRAMES);
            assert_eq!(click.button, "left");
            assert_eq!(
                click.frame,
                probe_points(plan.duration_s, plan.tps)[0].frame
            );
            assert!(run.record_digest, "mouse runs must record digests");
        }
        let names: Vec<&str> = runs.iter().map(|r| r.name.as_deref().unwrap()).collect();
        assert!(names.contains(&"probe_mouse_r0c0_readiness"));
        assert!(names.contains(&"probe_mouse_control_readiness"));
        // Cell probes come before the control (row-major, then control).
        assert_eq!(names.last(), Some(&"probe_mouse_control_readiness"));
    }

    #[test]
    fn mouse_probe_and_control_differ_by_exactly_the_click_position() {
        // The normalized-pair contract for the mouse matrix: same
        // frame, hold, button, bot knobs, determinism knobs — the
        // ONLY difference is WHERE the click lands.
        let plan = ProbePlan::default();
        let cell = &plan.mouse_cells()[0];
        let probe = plan.mouse_cell_scenario(cell).unwrap();
        let control = plan.mouse_control_scenario().unwrap();

        let [p] = &probe.bot.mouse_clicks[..] else {
            panic!("cell probe must deliver exactly one click");
        };
        let [c] = &control.bot.mouse_clicks[..] else {
            panic!("control run must deliver exactly one click");
        };
        assert_eq!(p.frame, c.frame, "click frame must match");
        assert_eq!(p.hold_frames, c.hold_frames, "hold must match");
        assert_eq!(p.button, c.button, "button must match");
        // And the position is the one bit that differs.
        assert_ne!(p.x, c.x);
        assert_ne!(p.y, c.y);

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
    fn mouse_cell_px_comes_from_normalized_times_extent() {
        // Scenario clicks bake normalized × assumed extent (px), so
        // per-cell findings stay meaningful for any real window size.
        // NO y-flip is deliberate and pinned here: `norm_y` is
        // top-to-bottom (row 0 = top), and window-event y (winit
        // convention) is ALSO top-to-bottom — CursorMoved positions
        // flow unchanged through bevy_picking into bevy_ui's
        // hit-test. Flipping would mirror the whole grid vertically.
        let plan = ProbePlan::default();
        let cell = &plan.mouse_cells()[0];
        let probe = plan.mouse_cell_scenario(cell).unwrap();
        let [click] = &probe.bot.mouse_clicks[..] else {
            panic!("cell probe must deliver exactly one click");
        };
        assert_eq!(click.x, cell.norm_x * plan.mouse_click_extent_w_px);
        assert_eq!(click.y, cell.norm_y * plan.mouse_click_extent_h_px);
        // Control: plan fields, not just module constants. Identity
        // either way for the default dead-center position.
        let control = plan.mouse_control_scenario().unwrap();
        let [ctrl] = &control.bot.mouse_clicks[..] else {
            panic!("control run must deliver exactly one click");
        };
        assert_eq!(
            ctrl.x,
            plan.mouse_control_norm_x * plan.mouse_click_extent_w_px
        );
        assert_eq!(
            ctrl.y,
            plan.mouse_control_norm_y * plan.mouse_click_extent_h_px
        );
        assert_eq!(ctrl.x, MOUSE_CONTROL_NORM_X * plan.mouse_click_extent_w_px);
        assert_eq!(ctrl.y, MOUSE_CONTROL_NORM_Y * plan.mouse_click_extent_h_px);
    }

    #[test]
    fn disabled_grid_means_zero_mouse_runs() {
        // (0, 0) is the sanctioned off-switch — slim tests use it to
        // leave the keyboard matrix untouched.
        let plan = ProbePlan {
            mouse_grid_cols: 0,
            mouse_grid_rows: 0,
            ..Default::default()
        };
        assert!(plan.mouse_cells().is_empty());
        assert!(plan.mouse_runs().unwrap().is_empty());
    }

    #[test]
    fn degenerate_half_grid_is_rejected_loudly() {
        // Columns without rows (or vice versa) claim a grid but would
        // schedule zero cells — a silent lie. Both halves must reject.
        for (cols, rows) in [(3, 0), (0, 3)] {
            let plan = ProbePlan {
                mouse_grid_cols: cols,
                mouse_grid_rows: rows,
                ..Default::default()
            };
            let err = plan.mouse_runs().unwrap_err().to_string();
            assert!(
                err.contains("degenerate"),
                "must explain the half-grid ({cols}x{rows}): {err}"
            );
        }
    }

    #[test]
    fn mouse_control_position_cell_is_detected_by_coordinates() {
        // The self-probe guard, mouse edition: a cell whose CENTER
        // coincides with the control position cannot be classified
        // (probe == baseline by construction). Detected on
        // coordinates, not ids — a 1×1 grid's sole cell IS dead
        // center whatever its label.
        let plan = ProbePlan::default();
        let centered = ProbePlan {
            mouse_grid_cols: 1,
            mouse_grid_rows: 1,
            ..Default::default()
        };
        assert!(centered.mouse_cell_is_control_position(&centered.mouse_cells()[0]));
        // The default 5×4 grid's centers (multiples of 0.2 / 0.25
        // plus half-cell offsets) never hit (0.5, 0.5) exactly.
        for cell in plan.mouse_cells() {
            assert!(
                !plan.mouse_cell_is_control_position(&cell),
                "{} must not alias the control position",
                cell.id()
            );
        }
        // Bespoke control positions must be honored, not hardcoded
        // to the module constants.
        let moved = ProbePlan {
            mouse_control_norm_x: 0.3,
            mouse_control_norm_y: 0.7,
            ..Default::default()
        };
        let hit = MouseCell {
            row: 0,
            col: 0,
            norm_x: 0.3,
            norm_y: 0.7,
        };
        assert!(moved.mouse_cell_is_control_position(&hit));
        assert!(!moved.mouse_cell_is_control_position(&plan.mouse_cells()[0]));
    }

    #[test]
    fn mouse_click_frame_fit_is_validated_like_key_points() {
        // The schema does NOT validate mouse frames against the
        // run, so the plan must do it itself (schema gap documented
        // on `click_input`). A 1.0s run at 60 tps = 60 ticks, but the
        // readiness click sits at frame 60 — past the end: it would
        // silently never fire and the whole cell matrix would read
        // as inert. 2.0s = 120 ticks fits comfortably.
        let tight = ProbePlan {
            duration_s: 1.0,
            ..Default::default()
        };
        let err = tight.mouse_runs().unwrap_err().to_string();
        assert!(err.contains("does not fit"), "must explain: {err}");

        let fits = ProbePlan {
            duration_s: 2.0,
            ..Default::default()
        };
        assert!(fits.mouse_runs().is_ok());
    }

    #[test]
    fn mouse_scenarios_share_the_readiness_point_and_record_digests() {
        // Cell probes ride the SAME point as the mouse control (the
        // readiness point) so probe and baseline are frame-aligned;
        // digests are the diff input for every mouse run.
        let plan = ProbePlan::default();
        let readiness = probe_points(plan.duration_s, plan.tps)[0].frame;
        for run in plan.mouse_runs().unwrap() {
            assert!(run.record_digest, "mouse run must record digests");
            assert!(run.single_threaded);
            assert_eq!(run.bot.mouse_clicks[0].frame, readiness);
            assert!(run.bot.key_presses.is_empty());
        }
    }
}
