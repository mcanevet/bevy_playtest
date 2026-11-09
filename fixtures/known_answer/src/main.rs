//! Known-answer fixture game (planted truth).
//!
//! A tiny deterministic menu + gameplay state machine whose entire
//! input surface is AUTHORED: every `// PLANT Pn` / `// PLANT Nn`
//! marker corresponds to exactly one row in `ground_truth.json`, and
//! a test cross-checks the two (no drift between catalog and truth).
//! See `docs/reference/known-answer-fixture.md` for the full plant
//! catalog and what each measurement lens asserts today.
//!
//! Design constraints the discovery stack relies on:
//! - Deterministic: no wall clock, no RNG, frame-driven only.
//! - Effects are DELIBERATELY visible to the state digest:
//!   reflected-and-registered resources (everything NOT starting
//!   with `bevy_`/`bevy_playtest::` counts, once registered).
//! - Negative plants are digest-INvisible by construction (see the
//!   N3/N4 blocks below).
//!
//! IMPORTANT (do not "fix" silently): the mutation selector env var
//! `KNOWN_ANSWER_MUTATION` is deliberately NOT prefixed with
//! `BEVY_PLAYTEST_`. The harness polices and strips its own
//! namespace (unknown `BEVY_PLAYTEST_*` vars are rejected with exit
//! 3 by the takeover plugin, and the probe pool strips the prefix
//! from spawned children), so a prefixed var could never reach the
//! game. This var is fixture-owned configuration, not harness
//! configuration — see README.md ("Why the mutation var is not
//! BEVY_PLAYTEST_-prefixed").

use bevy::prelude::*;
use leafwing_input_manager::prelude::*;

/// Mutation selector, read ONCE at startup (see module docs for why
/// the name is deliberately unprefixed). Values: `P1_dead`,
/// `N1_hotbar_full`, or unset/empty (no mutation). Unknown values
/// PANIC: a typo'd selector must fail loudly, never produce a green
/// but meaningless measurement.
const ENV_MUTATION: &str = "KNOWN_ANSWER_MUTATION";

/// Selected mutation (`None` = pristine). Read before the App is
/// built so every system sees one consistent value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mutation {
    None,
    /// P1's system early-outs: the `KeyU` binding becomes dead code
    /// at runtime (declared in ground truth as live).
    P1Dead,
    /// All 10 hotbar slots become live (N1's "two live of ten"
    /// adversarial shape flips to fully live).
    N1HotbarFull,
    /// L5's healthy LW action is disabled at runtime shortly after
    /// boot: a LIVE declared binding flips to dead wiring mid-run
    /// (see `disable_l5_if_mutated`). The intent-tap then records
    /// pressed-but-never-fired — the `DeclaredButDead` alarm's
    /// distinguishing control (the STATIC dead plant L1 already
    /// proves the alarm fires; this proves it doesn't fire on the
    /// pristine live twin).
    LwDead,
}

fn read_mutation() -> Mutation {
    let value = std::env::var(ENV_MUTATION).unwrap_or_default();
    match value.trim() {
        "" => Mutation::None,
        "P1_dead" => Mutation::P1Dead,
        "N1_hotbar_full" => Mutation::N1HotbarFull,
        "LW_dead" => Mutation::LwDead,
        other => panic!(
            "known_answer fixture: unknown {} value '{other}' \
             (expected 'P1_dead', 'N1_hotbar_full', 'LW_dead', or empty)",
            ENV_MUTATION
        ),
    }
}

/// Game-wide state machine. Boots into `Menu`; `Secret` is reachable
/// by NO transition (P7's run-condition-gated binding lives behind
/// `in_state(GameState::Secret)` — declared in source, dead at
/// boot by construction).
#[derive(Clone, Copy, Default, Eq, PartialEq, Debug, Hash, States)]
enum GameState {
    #[default]
    Menu,
    Playing,
    /// Unreachable: no `NextState` transition ever targets it.
    Secret,
}

// ---------------------------------------------------------------------------
// PLANT P1 — Menu context: KeyU unmute toggle.
//
// Audio-only effect made digest-visible WITHOUT real audio: toggle a
// REFLECTED, REGISTERED `Muted` resource. R2 witness testbed: the
// toggle is ALSO gated on a focused menu widget — but a GLOBAL
// shortcut path keeps it live when unfocused ("focus-present ≠
// gameplay-dead"). R3 testbed: a runtime rebind can flip this
// binding to KeyM (`rebind_unmute_to_keym` below, `mutable: true`).
// ---------------------------------------------------------------------------

/// P1/P3/R2 digest-visible effect carrier. Reflected + registered so
/// the state digest folds it every frame.
#[derive(Resource, Debug, Reflect, PartialEq)]
#[reflect(Resource)]
struct Muted(bool);

/// R2: minimal focusable-widget witness. When `true` the P1 handler
/// takes its "focused menu row" path; the global-shortcut path keeps
/// KeyU live when `false`. Only the focused FLAG itself is
/// digest-irrelevant (NOT reflected) — the EFFECT (Muted) is what
/// the probe sees. Deliberately never written in D1 (boots
/// unfocused): the witness channel is structure-only until its
/// producer lands (D2+); dead-code lint allows mark the intent.
#[derive(Resource, Debug, Default)]
#[expect(
    dead_code,
    reason = "R2 witness testbed: read for the focus path; never written in D1"
)]
struct MenuFocus {
    focused: bool,
}

/// R3: runtime-mutable binding for P1. Bespoke-resource SHAPE
/// (named KeyCode field — the D2+ shape-(b) testbed), but the BEHAVIOR
/// flows through it too, so P1 stays probe-live after a rebind to
/// KeyM. NOT reflected (shape data only; the digest watches `Muted`).
///
/// D1 scope: bespoke-shape measurement is D2+. Structure only.
#[derive(Resource, Debug)]
struct UnmuteKey(KeyCode);

/// R1: bespoke `Keymap`-shaped resource routing the REAL gameplay
/// bindings (WASD). Named KeyCode fields — shape-(b) testbed for
/// D2+ bespoke-tier measurement; the movement system READS this
/// resource, so the bindings genuinely flow through it.
/// NOT reflected (see `Muted` for the digest-visible counterpart).
#[derive(Resource, Debug)]
struct Keymap {
    up: KeyCode,
    left: KeyCode,
    down: KeyCode,
    right: KeyCode,
}

// ---------------------------------------------------------------------------
// PLANT P2 — Gameplay: WASD movement cluster → transform deltas.
// The classic group-completion case, routed through R1's `Keymap`.
// ---------------------------------------------------------------------------

/// P2's moved entity. `Transform` is engine-reflected and folded by
/// the digest already, so movement is digest-visible for free.
#[derive(Component)]
struct Player;

// ---------------------------------------------------------------------------
// PLANT P3 — Gameplay: KeyR reset (zeroes state).
//
// R1-poisoning testbed: the reset's observable consequence is
// DELAYED — the run stays clean until a P2 movement has moved the
// player, and `KeyR` only then has anything to zero. The 20-frame
// post-tap coverage window of a probe catches the divergence.
// ---------------------------------------------------------------------------

/// P3 reset-counter: a digest-visible witness that reset happened
/// even when the player hasn't moved (probe runs never move first,
/// so the counter is what a bare KeyR tap flips).
#[derive(Resource, Debug, Reflect, PartialEq)]
#[reflect(Resource)]
struct ResetCount(u32);

// ---------------------------------------------------------------------------
// PLANT P4 — Chord: KeyL while holding ShiftLeft.
//
// Naive single-key probing CANNOT fire this by construction (a probe
// taps ONE key for 1 frame; ShiftLeft alone routes here and is a
// modifier, not the chord trigger). Digest-visible effect: toggling
// a reflected `LampOn` resource.
// ---------------------------------------------------------------------------

#[derive(Resource, Debug, Reflect, PartialEq)]
#[reflect(Resource)]
struct LampOn(bool);

// ---------------------------------------------------------------------------
// PLANT P5 — Debounce: KeyT with a 20-frame cooldown.
//
// Miss-by-design commitment: the FIRST press only ARMS (writes the
// unreflected `ArmedFrame` resource — invisible to the digest);
// the digest-visible `TrickCount` bumps only on a LATER press ≥ 20
// frames after arming. A probe's isolated 1-frame tap therefore
// leaves the digest trace identical to the control's: the naive
// sweep MUST miss this at boot.
// ---------------------------------------------------------------------------

/// P5 arming record. NOT reflected: arming alone must be invisible
/// to the digest (the visible effect fires only on the second,
/// out-of-cooldown tap — something single-tap probes never do).
#[derive(Resource, Debug, Default)]
struct ArmedFrame(Option<u32>);

/// P5's digest-visible counter: incremented only on a second KeyT
/// press ≥ 20 frames after arming.
#[derive(Resource, Debug, Reflect, PartialEq)]
#[reflect(Resource)]
struct TrickCount(u32);

// ---------------------------------------------------------------------------
// PLANT P6 — #[cfg(debug_assertions)]-gated KeyG: debug overlay
// toggle. Dev-profile surface is larger than release — the
// profile-recording case. Digest-visible effect: reflected
// `DebugOverlay` resource (dev builds only).
// ---------------------------------------------------------------------------

#[cfg(debug_assertions)]
#[derive(Resource, Debug, Reflect, PartialEq)]
#[reflect(Resource)]
struct DebugOverlay(bool);

// ---------------------------------------------------------------------------
// PLANT P7 — Scan decoy: KeyV behind `in_state(GameState::Secret)`,
// reachable by NO transition in the shipped state machine. REAL
// effect (toggles reflected `VhsFilter`), dead at boot by
// construction: the naive sweep probes the boot context and MUST
// find KeyV inert there — the commitment is a context-gated
// NOT-DISCOVERED at boot, never a declared-but-live grade.
// ---------------------------------------------------------------------------

#[derive(Resource, Debug, Reflect, PartialEq)]
#[reflect(Resource)]
struct VhsFilter(bool);

// ---------------------------------------------------------------------------
// NEGATIVE PLANTS — shape positives, behavioral negatives. Naive
// shape detection MUST NOT grade these as bindings; the probe sweep
// MUST find them inert. Each carries a ground-truth row with
// `expected_grade: "negative"`.
// ---------------------------------------------------------------------------

// PLANT N1 — Hotbar: a `[KeyCode; 10]` resource the hotbar system
// indexes into. Only TWO slots are live (Digit1/Digit2); the array
// SHAPE must never read as ten bindings. NOT reflected (digest
// sees only the live slots' effects).
#[derive(Resource, Debug)]
struct Hotbar([KeyCode; 10]);

/// N1's digest-visible effect witness: whichever hotbar slot is live,
// pressing it bumps this reflected counter (so Digit1/Digit2 diverge;
/// Digit0/Digit3..9 stay inert despite sharing the array).
#[derive(Resource, Debug, Reflect, PartialEq)]
#[reflect(Resource)]
struct HotbarUse(u32);

// PLANT N2 — Rebind-preview map + settings-UI row: display data for
// a settings screen, never bindings. A `HashMap<MenuAction,
// KeyCode>` plus a UI row struct carrying a KeyCode payload.
/// Menu actions for the preview map (display-only).
#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
enum MenuAction {
    Start,
    Settings,
}

#[derive(Resource, Debug, Default)]
#[expect(
    dead_code,
    reason = "N2 negative plant: display-data map, never an input route"
)]
struct RebindPreview(std::collections::HashMap<MenuAction, KeyCode>);

/// Settings-UI row: displays "Start: KeyEnter". Carries a KeyCode
/// PAYLOAD but no system reads it for input — pure display shape
/// (N2's second half). Dead-code lint allows mark the intent.
#[derive(Component, Debug)]
#[expect(
    dead_code,
    reason = "N2 negative plant: display shape, never an input route"
)]
struct SettingsRow {
    action: MenuAction,
    key: KeyCode,
}

// PLANT N3 — `last_pressed: KeyCode` debug tracker: structurally a
// KeyCode field, behaviorally a LOGGER. It mirrors whatever key was
// pressed last. CRITICAL: NOT reflected — if it were, every probe
// candidate would diverge from the control run (which presses KeyQ)
// and the whole matrix would collapse into false positives. The
// digest must not see it; the commitment is "logged, not bound".
#[derive(Resource, Debug, Default)]
struct LastPressed(Option<KeyCode>);

// PLANT N4 — Raw read with NO gameplay effect: a system reads
// `just_pressed(KeyX)` and writes a LOG-ONLY resource. The log
// resource is deliberately NOT reflected, so the probe classifies
// KeyX inert. Inert-by-design — graded `negative`.
/// Log-only sink for N4. Deliberately NOT reflected: the digest must
/// classify KeyX inert. The value is never read — reading it would
/// defeat the plant's whole point (log-only, not a binding).
#[derive(Resource, Debug, Default)]
struct KeyXLog(u32);

// ---------------------------------------------------------------------------
// App assembly
// ---------------------------------------------------------------------------

#[cfg(feature = "playtest")]
fn default_plugins() -> bevy_playtest::PlaytestDefaultPlugins {
    bevy_playtest::PlaytestDefaultPlugins
}
#[cfg(not(feature = "playtest"))]
fn default_plugins() -> bevy::DefaultPlugins {
    bevy::DefaultPlugins
}

#[cfg(feature = "playtest")]
fn playtest_plugins() -> (bevy_playtest::PlaytestPlugin,) {
    (bevy_playtest::PlaytestPlugin,)
}
#[cfg(not(feature = "playtest"))]
fn playtest_plugins() -> () {}

// The selected mutation as a process-wide read-only fact (set once
// in `main` before the App runs; every system sees one value).
static MUTATION: std::sync::OnceLock<Mutation> = std::sync::OnceLock::new();

fn active_mutation() -> Mutation {
    *MUTATION.get().unwrap_or(&Mutation::None)
}

/// The boot context: `Menu` for the first `MENU_FRAMES` ticks, then
/// the game auto-starts `Playing` (no input drives the transition —
/// deterministic, so probe and control runs share it exactly). The
// probe points (frames 60 and 90 at the default plan) sample the
/// GAMEPLAY context; the menu is the boot prefix.
const MENU_FRAMES: u32 = 30;

// ---------------------------------------------------------------------------
// LEAFWING DECLARED-TIER PLANTS (L1..L5, D1-d).
//
// A real Leafwing InputManager 0.22 subsystem: the declared-tier
// reader enumerates these `InputMap`s by pure reflection (no
// Leafwing dependency in the harness), the intent-tap correlates
// driver presses with action firings, and reconcile grades each
// binding. Plants:
//
// PLANT L1 — Static dead wiring: DisabledAtSpawn(KeyN), LW-disabled at
//            spawn; pressed but never fires → DeclaredButDead alarm.
// PLANT L2 — Honesty spine: Ignored(KeyO) declared + enabled, NO game
//            handler — wiring fires, game ignores → ContextUnverified,
//            never an alarm (wiring liveness ≠ game liveness).
// PLANT L3 — Chord: Combo = ControlLeft+KeyI via LW's own chord —
//            chord members get notes, never alarms.
// PLANT L4 — Context-gated: Gated(KeyP) has a REAL handler with a
//            REAL digest-visible effect, but it runs only inside
//            GameState::Menu (auto-left at frame 30, before probe
//            points sample gameplay) — wiring fires, the sweep can
//            never verify → ContextUnverified, never an alarm.
// PLANT L5 — Healthy live: Live(KeyJ) bumps reflected LwUse —
//            probe-visible, boot-live, manager_pipeline_ok under the
//            deterministic press scenario; the sweep's
//            DiscoveredButUndeclared for KeyJ is RETRACTED by
//            reconcile. Mutation `LW_dead` disables it mid-run.
//
// Two maps exercise multi-map, multi-Actionlike-type enumeration:
// a code-built `InputMap<LwPlantAction>` and a RON-loaded
// `InputMap<MenuAction2>` (parsed at startup — RON maps must be
// indistinguishable from code-built ones to the reflection reader).
// ---------------------------------------------------------------------------

/// The LW plant `Actionlike` enum (code-built map host).
#[derive(
    Actionlike, Clone, Copy, PartialEq, Eq, Hash, Debug, Reflect, serde::Serialize, serde::Deserialize,
)]
enum LwPlantAction {
    /// L5: healthy live action (KeyJ), digest-visible witness bump.
    Live,
    /// L1: disabled at spawn — the static dead-wiring alarm plant.
    DisabledAtSpawn,
    /// L2: declared + enabled, no game handler — wiring fires, the
    /// game ignores it (honesty-spine plant).
    Ignored,
    /// L3: chord with ControlLeft — chord-member semantics plant.
    Combo,
    /// L4: context-gated — fires at wiring level, invisible to the
    /// boot-context sweep.
    Gated,
}

/// The RON-loaded map's `Actionlike` enum. A SECOND Actionlike type
/// coexisting with [`LwPlantAction`] proves the reader enumerates
/// every map generically, not just the first one it meets.
#[derive(
    Actionlike, Clone, Copy, PartialEq, Eq, Hash, Debug, Reflect, serde::Serialize, serde::Deserialize,
)]
enum MenuAction2 {
    /// A binding that exists ONLY via the RON-loaded map.
    RonBound,
}

/// L5's digest-visible witness: bumped by the `Live` action handler.
/// Reflected + registered (the sweep's effect-signature probe sees
/// the bump on KeyJ; the retraction case rides on it).
#[derive(Resource, Debug, Reflect, PartialEq)]
#[reflect(Resource)]
struct LwUse(u32);

/// L4's digest-visible witness: bumped by the menu-gated `Gated`
/// handler. REAL wiring, REAL effect — but the handler runs only in
/// `GameState::Menu`, which the fixture auto-leaves at frame 30
/// (before any gameplay-context probe point), so the sweep can never
/// verify the effect and must NOT claim liveness it cannot see.
#[derive(Resource, Debug, Reflect, PartialEq)]
#[reflect(Resource)]
struct GatedUse(u32);

/// RON source for the second map, hand-written as a real game's
/// asset file would be (shape captured from LW 0.22's own
/// serialization: each binding is a typetagged single-entry map —
/// `{"KeyCode":KeyH}`). The reader must recover `RonBound`'s binding
/// from a map that never existed as a code-built `InputMap` value.
const MENU_MAP_RON: &str = r#"(
    buttonlike_map: {
        RonBound: [{"KeyCode": KeyH}],
    },
    axislike_map: {},
    dual_axislike_map: {},
    triple_axislike_map: {},
    associated_gamepad: None,
)"#;

/// Builds the code-built `InputMap<LwPlantAction>`: L1, L2, L3
/// (chord), L4, L5's KeyJ binding. The RON map (below) is a distinct
/// construction route, exercising BOTH paths the reader must cover.
fn lw_plant_map() -> InputMap<LwPlantAction> {
    let mut map = InputMap::default();
    map.insert(LwPlantAction::Live, KeyCode::KeyJ);
    map.insert(LwPlantAction::DisabledAtSpawn, KeyCode::KeyN);
    map.insert(LwPlantAction::Ignored, KeyCode::KeyO);
    map.insert(
        LwPlantAction::Combo,
        ButtonlikeChord::new([KeyCode::ControlLeft, KeyCode::KeyI]),
    );
    map.insert(LwPlantAction::Gated, KeyCode::KeyP);
    map
}

/// L4's handler: context-gated by DESIGN. The `GatedUse` bump is a
/// real, registered, digest-visible effect — but this system only
/// runs inside `GameState::Menu`, which lasts frames 0..30. The
/// probe plan's gameplay probe points (~frames 60/90) can therefore
/// never observe the effect: the binding is honestly unverifiable by
/// the sweep, not dead. This is what separates L4 from L2 — L2 has
/// NO handler anywhere; L4's handler exists but is state-gated.
fn handle_lw_gated(
    mut actions: Query<&mut ActionState<LwPlantAction>>,
    mut gated_use: ResMut<GatedUse>,
) {
    for actions in &mut actions {
        if actions.just_pressed(&LwPlantAction::Gated) {
            gated_use.0 += 1;
        }
    }
}

/// L5's game handler: the ONLY unconditionally-live effect among the
/// LW plants. The digest-visible `LwUse` bump is what makes KeyJ
/// probe-visible to the sweep — the crown-jewel retraction case needs
/// the sweep to FIND KeyJ live and reconcile to supersede that raw
/// finding.
fn handle_lw_live(
    mut actions: Query<&mut ActionState<LwPlantAction>>,
    mut lw_use: ResMut<LwUse>,
) {
    for actions in &mut actions {
        if actions.just_pressed(&LwPlantAction::Live) {
            lw_use.0 += 1;
        }
    }
}

/// Mutation `LW_dead`: disable the HEALTHY action shortly after boot
/// (frame 35 — after the declared snapshot, before the deterministic
/// KeyJ press at frame 40). The intent-tap then records a pressed
/// key whose action never fires: the pressed-never-fired alarm arm.
/// The action stays enabled in the pristine run — the control arm.
fn disable_l5_if_mutated(mut actions: Query<&mut ActionState<LwPlantAction>>, frame: Res<FrameCount>) {
    if active_mutation() != Mutation::LwDead || frame.0 != 35 {
        return;
    }
    for mut actions in &mut actions {
        actions.disable_action(&LwPlantAction::Live);
    }
}

fn main() {
    let mutation = read_mutation();
    let _ = MUTATION.set(mutation);

    let mut app = App::new();
    app.add_plugins(default_plugins())
        .add_plugins(playtest_plugins())
        .init_state::<GameState>()
        .insert_resource(Muted(false))
        .insert_resource(MenuFocus { focused: false })
        .insert_resource(UnmuteKey(KeyCode::KeyU))
        .insert_resource(Keymap {
            up: KeyCode::KeyW,
            left: KeyCode::KeyA,
            down: KeyCode::KeyS,
            right: KeyCode::KeyD,
        })
        .insert_resource(ResetCount(0))
        .insert_resource(LampOn(false))
        .insert_resource(ArmedFrame(None))
        .insert_resource(TrickCount(0))
        .insert_resource(Hotbar([
            // N1: all ten digit slots LOOK like bindings to a shape
            // scan; only the first two are live (`handle_hotbar`).
            KeyCode::Digit1,
            KeyCode::Digit2,
            KeyCode::Digit3,
            KeyCode::Digit4,
            KeyCode::Digit5,
            KeyCode::Digit6,
            KeyCode::Digit7,
            KeyCode::Digit8,
            KeyCode::Digit9,
            KeyCode::Digit0,
        ]))
        .insert_resource(HotbarUse(0))
        .insert_resource(RebindPreview(std::collections::HashMap::from([
            (MenuAction::Start, KeyCode::Enter),
            (MenuAction::Settings, KeyCode::KeyS),
        ])))
        .insert_resource(LastPressed(None))
        .insert_resource(KeyXLog(0))
        .insert_resource(VhsFilter(false))
        .insert_resource(RebindMenuActive(false))
        .insert_resource(FrameCount(0))
        .insert_resource(LwUse(0))
        .insert_resource(GatedUse(0))
        .register_type::<Muted>()
        .register_type::<ResetCount>()
        .register_type::<LampOn>()
        .register_type::<TrickCount>()
        .register_type::<HotbarUse>()
        .register_type::<VhsFilter>()
        .register_type::<LwUse>()
        .register_type::<GatedUse>();
    // P6: dev-profile surface member — registered (and the system
    // compiled) under `debug_assertions` only, so a release-profile
    // surface is strictly smaller.
    #[cfg(debug_assertions)]
    {
        app.insert_resource(DebugOverlay(false));
        app.register_type::<DebugOverlay>();
        app.add_systems(Update, handle_debug_overlay);
    }

    app.add_systems(Startup, setup)
        .add_systems(
            Update,
            (
                count_frames,
                auto_start,
                // P1 (R2): focused-row path AND global shortcut — live
                // in both contexts. R3: see `rebind_unmute_to_keym`.
                handle_unmute,
                rebind_unmute_to_keym,
            ),
        )
        .add_systems(
            Update,
            (
                handle_movement,
                handle_reset,
                handle_chord,
                handle_debounce,
                handle_hotbar,
                track_last_pressed,
                log_keyx,
            )
                .run_if(in_gameplay),
        )
        .add_systems(
            Update,
            handle_vhs_filter.run_if(in_state(GameState::Secret)),
        );

    // ----- Leafwing declared-tier plants (L1..L5, D1-d) -----
    // Plugin order note: InputManagerPlugin is added AFTER
    // PlaytestPlugin (takeover must own the runner — last-set-wins);
    // LW's own systems run inside the game's normal Update schedule,
    // so the takeover's post-update intent-tap (a driver-loop
    // recorder) still observes their firings after app.update().
    app.add_plugins(InputManagerPlugin::<LwPlantAction>::default());
    app.add_plugins(InputManagerPlugin::<MenuAction2>::default());
    // RON-loaded map (startup-parse: exactly like an asset load):
    // a binding route that never exists as a code-built InputMap.
    let menu_map: InputMap<MenuAction2> = ron::from_str(MENU_MAP_RON)
        .expect("MENU_MAP_RON must parse (LW InputMap serde layout)");
    app.add_systems(Startup, move |mut commands: Commands| {
        // Code-built plant map: L1..L5's bindings.
        let mut actions = ActionState::<LwPlantAction>::default();
        // L1: the static dead-wiring plant — disabled BEFORE the
        // first frame so the reader's ActionState observation sees
        // `disabled: true` from the very first tap frame.
        actions.disable_action(&LwPlantAction::DisabledAtSpawn);
        commands.spawn((lw_plant_map(), actions));
        // RON-loaded map: RonBound(KeyH). MenuAction2 has NO game
        // handler — a declared binding that fires when pressed but
        // is ignored by game code (honesty-spine, ContextUnverified
        // if probed). The deterministic scenario DOES press KeyH at
        // frame 65: the tap records pressed-and-fired, wiring live,
        // game effect unverified — the never-pressed arm is pinned
        // by unit tests instead.
        commands.spawn((menu_map.clone(), ActionState::<MenuAction2>::default()));
    })
    .add_systems(
        Update,
        (
            // L5's digest-visible effect.
            handle_lw_live,
            // L4's menu-gated effect: real handler + real witness,
            // only observable inside GameState::Menu.
            handle_lw_gated.run_if(in_state(GameState::Menu)),
            // Mutation LW_dead's disable pass (no-op pristine).
            disable_l5_if_mutated,
        ),
    );
    app.run();
}

/// Spawn the persistent scene: the P2 player and one N2 settings-UI
/// row (a component with a KeyCode payload — display shape, never
/// an input route).
fn setup(mut commands: Commands) {
    commands.spawn((Player, Transform::default()));
    commands.spawn(SettingsRow {
        action: MenuAction::Start,
        key: KeyCode::Enter,
    });
}

/// Deterministic boot prefix: menu → playing at frame
/// `MENU_FRAMES`, no input involved (probe/control runs share the
/// transition identically, so it never reads as a divergence).
fn auto_start(
    mut frame: Local<u32>,
    state: Res<State<GameState>>,
    mut next: ResMut<NextState<GameState>>,
) {
    *frame += 1;
    if *frame == MENU_FRAMES && *state.get() == GameState::Menu {
        next.set(GameState::Playing);
    }
}

fn in_gameplay(state: Res<State<GameState>>) -> bool {
    matches!(state.get(), GameState::Playing)
}

/// KeyU unmute toggle. R2: the focused-row path AND the global
/// shortcut path reach the same digest-visible effect — KeyU is live
/// regardless of `MenuFocus` (the fixture boots unfocused). R3: the
/// binding key comes from the mutable `UnmuteKey` resource, so a
/// runtime rebind to KeyM keeps it live through the same system.
/// Mutation `P1_dead`: early-out — the binding becomes dead code at
/// runtime while ground truth still declares it live (the
/// declared-but-dead alarm testbed).
fn handle_unmute(
    keys: Res<ButtonInput<KeyCode>>,
    focus: Res<MenuFocus>,
    unmute_key: Res<UnmuteKey>,
    mut muted: ResMut<Muted>,
) {
    if active_mutation() == Mutation::P1Dead {
        return; // P1_dead: declared-but-live flipped to dead.
    }
    let pressed = keys.just_pressed(unmute_key.0);
    if !pressed {
        return;
    }
    // R2 witness shape: focused path vs global shortcut — both toggle.
    let _ = focus; // (read for the witness; effect is unconditional)
    muted.0 = !muted.0;
}

// R3 — structure only, harmless now.
/// Rebind-menu testbed: while a rebind menu is active (never true in
/// this fixture — no D1 input opens it), P1's binding flips to KeyM
/// at runtime (`mutable: true` in ground truth).
fn rebind_unmute_to_keym(rebind_menu: Res<RebindMenuActive>, mut unmute_key: ResMut<UnmuteKey>) {
    if rebind_menu.0 && unmute_key.0 != KeyCode::KeyM {
        unmute_key.0 = KeyCode::KeyM;
    }
}

/// Rebind-menu flag. Always false in D1 (no system sets it); exists
/// so R3's rebind testbed has a live wiring point.
#[derive(Resource, Debug, Default)]
struct RebindMenuActive(bool);

/// WASD movement via the R1 `Keymap` bespoke resource (the bindings
/// genuinely flow through it — shape AND behavior, D2+ testbed).
/// Transform deltas are digest-visible without extra plumbing.
fn handle_movement(
    keys: Res<ButtonInput<KeyCode>>,
    keymap: Res<Keymap>,
    time: Res<Time<Virtual>>,
    mut players: Query<&mut Transform, With<Player>>,
) {
    const SPEED: f32 = 32.0;
    let mut delta = Vec2::ZERO;
    if keys.pressed(keymap.up) {
        delta.y += 1.0;
    }
    if keys.pressed(keymap.down) {
        delta.y -= 1.0;
    }
    if keys.pressed(keymap.left) {
        delta.x -= 1.0;
    }
    if keys.pressed(keymap.right) {
        delta.x += 1.0;
    }
    if delta != Vec2::ZERO {
        for mut transform in &mut players {
            transform.translation.x += delta.x * SPEED * time.delta().as_secs_f32();
            transform.translation.y += delta.y * SPEED * time.delta().as_secs_f32();
        }
    }
}

/// KeyR reset: zeroes the accumulated plants and bumps the
/// digest-visible `ResetCount` witness (the probe never moves the
/// player first, so the counter is what its bare tap flips — the
/// R1-poisoning "delayed observable consequence" shape lives here as
/// the origin snap-back that only matters post-movement).
fn handle_reset(
    keys: Res<ButtonInput<KeyCode>>,
    mut reset_count: ResMut<ResetCount>,
    mut lamp: ResMut<LampOn>,
    mut trick: ResMut<TrickCount>,
    mut players: Query<&mut Transform, With<Player>>,
) {
    if !keys.just_pressed(KeyCode::KeyR) {
        return;
    }
    reset_count.0 += 1;
    lamp.0 = false;
    trick.0 = 0;
    for mut transform in &mut players {
        transform.translation.x = 0.0;
        transform.translation.y = 0.0;
    }
}

/// Chord: KeyL pressed WHILE ShiftLeft is held. A single-key probe
/// (one key, one frame) can never satisfy both halves — the naive
/// sweep MUST miss this at boot, by design.
fn handle_chord(keys: Res<ButtonInput<KeyCode>>, mut lamp: ResMut<LampOn>) {
    if keys.just_pressed(KeyCode::KeyL) && keys.pressed(KeyCode::ShiftLeft) {
        lamp.0 = !lamp.0;
    }
}

/// Debounced KeyT, 20-frame cooldown. FIRST tap only ARMS (writes
/// the unreflected `ArmedFrame` — invisible to the digest); the
/// digest-visible `TrickCount` bumps only on a LATER press ≥ 20
/// frames after arming. A probe's isolated 1-frame tap therefore
/// leaves the digest trace identical to the control's: the naive
/// sweep MUST miss this at boot, by design.
fn handle_debounce(
    keys: Res<ButtonInput<KeyCode>>,
    frame: Res<FrameCount>,
    mut armed: ResMut<ArmedFrame>,
    mut tricks: ResMut<TrickCount>,
) {
    const COOLDOWN_FRAMES: u32 = 20;
    if !keys.just_pressed(KeyCode::KeyT) {
        return;
    }
    match armed.0 {
        Some(armed_at) if frame.0.saturating_sub(armed_at) >= COOLDOWN_FRAMES => {
            tricks.0 += 1;
            armed.0 = None;
        }
        _ => armed.0 = Some(frame.0),
    }
}

/// Game frame counter (inserted at Startup, incremented every tick
/// via `count_frames`). NOT reflected: bookkeeping, not game state.
#[derive(Resource, Debug, Default)]
struct FrameCount(u32);

/// Increments [`FrameCount`] every frame — the fixture's
/// deterministic clock for cooldown logic.
fn count_frames(mut frame: ResMut<FrameCount>) {
    frame.0 += 1;
}

/// Hotbar: only slots 0 (Digit1) and 1 (Digit2) act. The other
/// eight slots hold plausible digit keys that a SHAPE scan would
/// happily grade as bindings — the sweep proves them inert.
/// Mutation `N1_hotbar_full`: all ten slots go live, flipping this
/// negative to positive (two directions of drift under one knob).
fn handle_hotbar(
    keys: Res<ButtonInput<KeyCode>>,
    hotbar: Res<Hotbar>,
    mut uses: ResMut<HotbarUse>,
) {
    let live_slots = if active_mutation() == Mutation::N1HotbarFull {
        hotbar.0.len()
    } else {
        2
    };
    for slot in hotbar.0.iter().take(live_slots) {
        if keys.just_pressed(*slot) {
            uses.0 += 1;
        }
    }
}

/// `last_pressed` logger: mirrors the most recent key press.
/// NOT reflected — structurally a KeyCode field, behaviorally a
/// logger; if it were digest-visible, every candidate would diverge
/// from the KeyQ control and the whole matrix would false-positive.
fn track_last_pressed(keys: Res<ButtonInput<KeyCode>>, mut last: ResMut<LastPressed>) {
    if let Some(code) = keys.get_just_pressed().next().copied() {
        last.0 = Some(code);
    }
}

/// Raw read with no gameplay effect: consumes `just_pressed(KeyX)`
/// into a log-only resource. NOT reflected, so the digest classifies
/// KeyX inert — the shape (a raw ButtonInput read) must never earn a
/// binding grade from the sweep alone.
fn log_keyx(keys: Res<ButtonInput<KeyCode>>, mut log: ResMut<KeyXLog>) {
    if keys.just_pressed(KeyCode::KeyX) {
        log.0 += 1;
    }
}

/// Debug-overlay toggle, present in DEV builds only
/// (`cfg(debug_assertions)`): dev-profile surfaces are larger than
/// release ones. Live in BOTH contexts (ungated), so the dev-profile
/// sweep finds KeyG at boot.
#[cfg(debug_assertions)]
fn handle_debug_overlay(keys: Res<ButtonInput<KeyCode>>, mut overlay: ResMut<DebugOverlay>) {
    if keys.just_pressed(KeyCode::KeyG) {
        overlay.0 = !overlay.0;
    }
}

#[cfg(not(debug_assertions))]
fn handle_debug_overlay() {}

/// Real effect behind an unreachable state: no transition in this
/// fixture ever enters `GameState::Secret`, so KeyV never fires at
/// boot — context-gated NOT-DISCOVERED is the commitment (a
/// source-level shape would call it a binding; the sweep refutes
/// that at boot and must not cry drift either).
fn handle_vhs_filter(keys: Res<ButtonInput<KeyCode>>, mut vhs: ResMut<VhsFilter>) {
    if keys.just_pressed(KeyCode::KeyV) {
        vhs.0 = !vhs.0;
    }
}
