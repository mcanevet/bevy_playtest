//! Static device-reader scan (discovery Phase 1).
//!
//! Walks every initialized schedule of a booted game and reports which
//! device classes (keyboard, mouse button, mouse motion, mouse wheel,
//! gamepad) have systems reading them, by resolving the input
//! resource/message types to `ComponentId`s and consulting each
//! system's [`Access`] set.
//!
//! Hard rules baked in from the design doc and the spike results:
//!
//! - Classify by `ComponentId` reads, NEVER by system name: consumer
//!   builds mask system names unless `bevy_ecs/debug` is compiled in,
//!   making name-based heuristics meaningless. One deliberate
//!   `run_fixed_main`, and `message_update_system`, present in every
//!   App) are identified by their exact names and skipped; masked
//!   names simply degrade to including them (over-counting,
//!   fail-open).
//! - The keyboard class is NEVER pruned. Run conditions
//!   (`input_just_pressed`, ...) hold the input access, not the
//!   guarded system body, so "no keyboard readers detected" is a weak
//!   signal — probing stays conservative (fail-open). Condition-held
//!   reads are additionally DETECTED at capture time
//!   ([`capture_static_scan`]'s census, the `condition_hidden_classes`
//!   helper): a condition observing a class the
//!   body does not surfaces a note and withholds `complete`, so a
//!   condition-guarded gamepad consumer can no longer scan as
//!   provably gamepad-free. Post-build the conditions are drained
//!   into executor storage bevy 0.20.1 does not expose, so the census
//!   exists only on the capture path; a bare
//!   [`static_device_readers`] walk keeps the legacy caveat.
//! - At most gamepad / wheel / gestures MAY be pruned when the scan
//!   finds no readers — and only from a COMPLETE scan
//!   ([`StaticScanResult::complete`]): an uninitialized or missing
//!   schedule hides its systems' reads, so zero readers from an
//!   incomplete scan proves nothing. Even then pruning is only a
//!   probe-surface hint: "no readers found" means "don't bother
//!   probing them", never "they don't work".

use bevy_app::App;
use bevy_ecs::component::ComponentId;
use bevy_ecs::message::Messages;
use bevy_ecs::prelude::Schedules;
use bevy_ecs::query::Access;
use bevy_ecs::system::SystemAccess;
use bevy_input::keyboard::{Key, KeyboardFocusLost, KeyboardInput};
use bevy_input::mouse::{
    AccumulatedMouseMotion, AccumulatedMouseScroll, MouseButtonInput, MouseMotion, MouseWheel,
};
use bevy_input::prelude::ButtonInput;
use bevy_window::{CursorMoved, WindowEvent};
use serde::{Deserialize, Serialize};
use std::any::TypeId;

use bevy_input::gamepad::{
    Gamepad, GamepadAxis, GamepadAxisChangedEvent, GamepadButton, GamepadButtonChangedEvent,
    GamepadButtonStateChangedEvent, GamepadConnectionEvent, GamepadEvent, GamepadRumbleRequest,
    RawGamepadEvent,
};
use bevy_input::Axis;

/// Exact names of the engine's Exclusive driver systems (verified in
/// bevy 0.20.1): every App has `Main::run_main`,
/// `FixedMain::run_fixed_main`, and `message_update_system` in its
/// schedule graph. They are skipped by the Exclusive arm — see the
/// comment at the scan loop — and exempted from the
/// uninitialized-schedule completeness check (FixedMain never
/// initializes in apps without fixed-timestep work). Masked names
/// (debug feature off) or renamed drivers degrade to over-counting
/// (fail-open), never under-counting.
const ENGINE_DRIVER_NAMES: &[&str] = &[
    "bevy_app::main_schedule::Main::run_main",
    "bevy_app::main_schedule::FixedMain::run_fixed_main",
    "bevy_ecs::message::update::message_update_system",
];

/// A device class the static scan tracks.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceClass {
    Keyboard,
    MouseButton,
    MouseMotion,
    MouseWheel,
    Gamepad,
}

impl DeviceClass {
    pub fn as_str(self) -> &'static str {
        match self {
            DeviceClass::Keyboard => "keyboard",
            DeviceClass::MouseButton => "mouse_button",
            DeviceClass::MouseMotion => "mouse_motion",
            DeviceClass::MouseWheel => "mouse_wheel",
            DeviceClass::Gamepad => "gamepad",
        }
    }

    /// Every tracked class. Fail-open arms (exclusive systems,
    /// read-all access) bump ALL of these — a `&mut World` system
    /// can read or initialize ANY signal type at runtime, including
    /// classes whose signal resources are not yet registered in
    /// the world (e.g. a game that never initialized gamepad input
    /// — the exclusive system may still touch it dynamically), so
    /// iterating the registered signal ids alone would
    /// under-count those classes and license the flow to prune
    /// inputs that were never inspected.
    pub const ALL: [Self; 5] = [
        Self::Keyboard,
        Self::MouseButton,
        Self::MouseMotion,
        Self::MouseWheel,
        Self::Gamepad,
    ];
}

/// Outcome of the static scan.
///
/// `readers` is annotation data (device class -> number of systems
/// whose access set includes a read OR WRITE of one of that class's
/// signal types: input-state writers observe the input pipeline as
/// much as readers do — menu key-clearing, rebind layers,
/// input-recording): informational only, never a gating signal.
/// `notes` carries caveats discovered during the scan (exclusive
/// systems, uninitialized schedules) that the report should surface.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct StaticScanResult {
    /// Device class -> count of systems whose access set includes a
    /// read OR WRITE of one of that class's signal types, ordered
    /// by first sighting.
    pub readers: Vec<(DeviceClass, usize)>,
    /// Caveats to surface in the report (exclusive systems present,
    /// schedules not initialized at scan time, ...).
    pub notes: Vec<String>,
    /// Whether the scan actually saw the schedule graph: `false`
    /// when the `Schedules` resource was absent or a schedule's
    /// systems were not initialized, meaning `readers` is provably
    /// INCOMPLETE (a zero count then proves nothing — it must not
    /// license pruning). `complete: false` is not an error: the
    /// reader counts that WERE collected stay valid as annotations;
    /// only prune DERIVATION must refuse them. Serde-defaulted so
    /// pre-flag exports deserialize as complete=false (fail-open —
    /// old sidecars never license pruning).
    #[serde(default)]
    pub complete: bool,
}

impl StaticScanResult {
    /// How many systems the scan saw reading this device class.
    pub fn reader_count(&self, class: DeviceClass) -> usize {
        self.readers
            .iter()
            .find(|(c, _)| *c == class)
            .map(|(_, n)| *n)
            .unwrap_or(0)
    }

    /// Whether the scan found at least one reader for this class.
    /// Informational only — callers must keep the keyboard class
    /// fail-open regardless (run-condition blind spot, design doc).
    pub fn has_readers(&self, class: DeviceClass) -> bool {
        self.reader_count(class) > 0
    }
}

/// The input signal resource/message `TypeId`s per device class.
///
/// Resources and messages share the `ComponentId` namespace
/// (`Components::valid_component_id` resolves both), so one lookup
/// covers `Res<ButtonInput<KeyCode>>`-style readers and
/// `MessageReader<KeyboardInput>`-style readers alike.
fn device_signal_types(world: &bevy_ecs::world::World) -> Vec<(DeviceClass, ComponentId)> {
    use bevy_input::keyboard::KeyCode;
    use bevy_input::mouse::MouseButton;

    let mut types: Vec<(DeviceClass, TypeId)> = vec![
        (DeviceClass::Keyboard, TypeId::of::<ButtonInput<KeyCode>>()),
        // The enhanced-input-era logical-key channel: the driver seeds
        // `ButtonInput<Key>` (bevy 0.20's logical key state, distinct
        // from physical `KeyCode`), so games reading
        // `Res<ButtonInput<Key>>` — e.g. checking `just_pressed(Key::Escape)`
        // without caring WHICH physical key is bound — are keyboard
        // readers too. Without this entry such a game scanned as
        // zero keyboard readers via the logical channel (physical
        // stays covered separately) and could not earn keyboard
        // probes it answers daily. Verified against the seed site in
        // our own driver (keyboard/arm region).
        (DeviceClass::Keyboard, TypeId::of::<ButtonInput<Key>>()),
        (
            DeviceClass::Keyboard,
            TypeId::of::<Messages<KeyboardInput>>(),
        ),
        (
            DeviceClass::Keyboard,
            TypeId::of::<Messages<KeyboardFocusLost>>(),
        ),
        // `WindowEvent` is the umbrella window backend event: its
        // variants span `KeyboardInput`, `MouseButtonInput`,
        // `MouseMotion`, and `MouseWheel` (verified in
        // bevy_window::event). A `MessageReader<WindowEvent>`
        // observes ALL of those channels, so the TypeId is
        // attributed to each applicable class — keyboard-only
        // attribution would let a mouse-only WindowEvent reader
        // masquerade as a keyboard game and earn keyboard probes
        // it can never answer.
        (DeviceClass::Keyboard, TypeId::of::<Messages<WindowEvent>>()),
        (
            DeviceClass::MouseButton,
            TypeId::of::<Messages<WindowEvent>>(),
        ),
        (
            DeviceClass::MouseMotion,
            TypeId::of::<Messages<WindowEvent>>(),
        ),
        (
            DeviceClass::MouseWheel,
            TypeId::of::<Messages<WindowEvent>>(),
        ),
        (
            DeviceClass::MouseButton,
            TypeId::of::<ButtonInput<MouseButton>>(),
        ),
        (
            DeviceClass::MouseButton,
            TypeId::of::<Messages<MouseButtonInput>>(),
        ),
        (
            DeviceClass::MouseMotion,
            TypeId::of::<AccumulatedMouseMotion>(),
        ),
        (
            DeviceClass::MouseWheel,
            TypeId::of::<AccumulatedMouseScroll>(),
        ),
        (
            DeviceClass::MouseMotion,
            TypeId::of::<Messages<CursorMoved>>(),
        ),
        // Mouse MESSAGE channels, mirroring the keyboard arm's
        // `Messages<KeyboardInput>` (review r4474375275): bevy_input's
        // mouse plumbing moves raw motion/wheel deltas through
        // `Messages<T>` — a `MessageReader<MouseMotion>` (raw per-frame
        // motion packets) or `MessageReader<MouseWheel>` (raw scroll
        // packets) reads NEITHER the accumulated resource NOR
        // WindowEvent, so tracking only those would read such a system
        // as ZERO wheel/motion readers and wrongly license pruning
        // (fail-closed hole). Both are `Message` types in
        // bevy_input::mouse 0.20.1, sibling to the accumulated resources.
        (
            DeviceClass::MouseMotion,
            TypeId::of::<Messages<MouseMotion>>(),
        ),
        (
            DeviceClass::MouseWheel,
            TypeId::of::<Messages<MouseWheel>>(),
        ),
    ];

    // Gamepad signals (resource + button state + stick axes).
    types.extend([
        (DeviceClass::Gamepad, TypeId::of::<Gamepad>()),
        (
            DeviceClass::Gamepad,
            TypeId::of::<ButtonInput<GamepadButton>>(),
        ),
        (DeviceClass::Gamepad, TypeId::of::<Axis<GamepadAxis>>()),
    ]);

    // Gamepad MESSAGE channels, mirroring the keyboard arm's
    // `Messages<KeyboardInput>`: bevy_input's gamepad plumbing moves
    // both raw and processed events through `Messages<T>` — a
    // `MessageReader<GamepadEvent>` (the umbrella, `From`-converted
    // from every processed event) or `MessageReader<RawGamepadEvent>`
    // (the backend feed) is a gamepad reader even though it touches
    // no Gamepad resource/state. Tracking only the resource/state
    // TypeIds would read such a system as ZERO and wrongly license
    // gamepad pruning (fail-closed hole). Includes the per-event
    // channels and the rumble REQUEST channel (a game that sends
    // rumble still plays on the gamepad surface), all of which are
    // `Message` types in bevy_input 0.20.1.
    types.extend([
        (DeviceClass::Gamepad, TypeId::of::<Messages<GamepadEvent>>()),
        (
            DeviceClass::Gamepad,
            TypeId::of::<Messages<RawGamepadEvent>>(),
        ),
        (
            DeviceClass::Gamepad,
            TypeId::of::<Messages<GamepadConnectionEvent>>(),
        ),
        (
            DeviceClass::Gamepad,
            TypeId::of::<Messages<GamepadButtonStateChangedEvent>>(),
        ),
        (
            DeviceClass::Gamepad,
            TypeId::of::<Messages<GamepadButtonChangedEvent>>(),
        ),
        (
            DeviceClass::Gamepad,
            TypeId::of::<Messages<GamepadAxisChangedEvent>>(),
        ),
        (
            DeviceClass::Gamepad,
            TypeId::of::<Messages<GamepadRumbleRequest>>(),
        ),
    ]);

    types
        .into_iter()
        .filter_map(|(class, ty)| {
            // Type-erased variant of valid_component_id::<T>: an unregistered
            // type yields None and is simply not tracked (the game never
            // touched that input plugin).
            world.components().get_valid_id(ty).map(|cid| (class, cid))
        })
        .collect()
}

/// Whether a system's access touches the given `ComponentId`.
/// Both bits are checked. Verified against bevy_ecs 0.20.1:
/// `Access::add_write` (and therefore `ResMut`) inserts the READ
/// bit as well, so writers are already visible to `has_read` — the
/// explicit `has_write` term is belt-and-braces documentation of
/// intent (input-state writers MUST count as observers: menu
/// key-clearing, rebind layers, input-recording) and protection if
/// a future Bevy stops implying reads from writes.
fn access_observes(access: &Access, cid: ComponentId) -> bool {
    access.has_read(cid) || access.has_write(cid)
}

/// The device classes a `SystemAccess` may observe among the
/// REGISTERED signal ids. Exclusive and read-all access map to every
/// tracked class (ALL, not just registered — the same doctrine as
/// the scan loop's fail-open arms: an `&mut World` or `Query<EntityRef>`
/// condition may touch any signal type, including unregistered ones).
fn access_observed_classes(
    access: &SystemAccess,
    signal_ids: &[(DeviceClass, ComponentId)],
) -> Vec<DeviceClass> {
    match access {
        SystemAccess::None => Vec::new(),
        SystemAccess::Exclusive => DeviceClass::ALL.to_vec(),
        SystemAccess::Shared(filtered) => {
            let access = filtered.combined_access();
            if access.has_read_all() || access.has_write_all() {
                DeviceClass::ALL.to_vec()
            } else {
                let mut classes = Vec::new();
                for (class, cid) in signal_ids {
                    if !classes.contains(class) && access_observes(access, *cid) {
                        classes.push(*class);
                    }
                }
                classes
            }
        }
    }
}

/// CENSUS of run-condition-held input reads (review r4474375349).
///
/// `Schedule::initialize` builds the executable and `build_schedule`
/// MOVES per-system conditions out of the graph into executor storage
/// that is `pub(super)` in bevy 0.20.1 — after the first build their
/// access is publicly unreadable. The post-pass scan walk
/// (`systems_with_access`) therefore sees only each guarded system's
/// OWN access, never its conditions': a body reading nothing with
/// `run_if(input_just_pressed(GamepadButton::South))` scans as ZERO
/// gamepad readers, and a complete-looking scan would license
/// gamepad pruning on evidence that proves nothing.
///
/// Conditions are publicly visible only BEFORE the build, so the
/// census must run at CAPTURE time (this routine's only caller is
/// [`capture_static_scan`], per schedule, before its
/// `Schedule::initialize`). Mechanism, verified empirically against
/// bevy_ecs 0.20.1: `graph_mut().initialize(world)` populates system
/// AND condition access sets without draining (`Systems::initialize`
/// drains only the not-yet-initialized key list; the condition drain
/// happens later, in `build_schedule`), and a second call inside
/// `Schedule::initialize` is a no-op — so this changes nothing about
/// how the schedule later behaves.
///
/// Blindness is decided at CLASS granularity, because that is the
/// only granularity pruning consumes: a condition observing a class
/// the system's own access already observes adds nothing (the
/// post-pass counts that system for the class either way); a
/// condition observing a class the body does NOT observe is a
/// potential hidden reader for exactly that class. Returned classes
/// are annotated + force `complete: false` upstream — readers are NOT
/// fabricated, keeping `readers` honest about what was actually seen.
///
/// Engine drivers are exempt via the same EXACT-name filter as the
/// scan loop: every default SubApp seeds
/// `message_update_system.run_if(message_update_condition)`, which
/// would otherwise blindfold every capture. Masked names (consumer
/// builds without `bevy_ecs/debug`) degrade to including the driver
/// as a blind system — over-withholding, fail-open.
///
/// RESIDUAL HOLE (documented, not fixable in 0.20.1): conditions
/// attached to SYSTEM SETS are not censused — the graph exposes no
/// system-key-to-owning-sets mapping, so a set's conditions cannot be
/// attributed to member systems. Bare Apps carry zero conditioned
/// sets (probed), and a set condition that gates an input-reading
/// system does NOT hide that system's own read (still counted);
/// only the condition-held part of a set-gated system stays
/// invisible, and it degrades the other way: a set-conditioned
/// hidden read can still license pruning. Revisit when Bevy exposes
/// the mapping.
fn condition_hidden_classes(
    schedule: &mut bevy_ecs::schedule::Schedule,
    world: &mut bevy_ecs::world::World,
) -> Vec<DeviceClass> {
    // Populate access WITHOUT draining conditions (see doc comment);
    // Systems/conditions' System::initialize also auto-registers any
    // Res dependencies (ButtonInput<GamepadButton>, ...), so the
    // signal resolution below sees condition-held resources too.
    schedule.graph_mut().initialize(world);
    let signal_ids = device_signal_types(world);

    let mut hidden: Vec<DeviceClass> = Vec::new();
    for (key, system, conds) in schedule.graph().systems.iter() {
        if conds.is_empty() {
            continue;
        }
        let name = system.name().to_string();
        if ENGINE_DRIVER_NAMES.contains(&name.as_str()) {
            continue;
        }
        // Classes the system's OWN body may already observe: the
        // post-pass counts those, so conditions add nothing there.
        let own = schedule
            .graph()
            .systems
            .get(key)
            .map(|swa| access_observed_classes(swa.access(), &signal_ids))
            .unwrap_or_default();
        for cond in conds {
            for class in access_observed_classes(&cond.access, &signal_ids) {
                if !own.contains(&class) && !hidden.contains(&class) {
                    hidden.push(class);
                }
            }
        }
    }
    hidden
}

/// Scan every initialized schedule of `app` for systems reading input
/// signal types.
///
/// The caller must have run at least one `app.update()` so schedules
/// are initialized (lazy schedules that have never run return
/// `ScheduleNotInitialized` and are skipped with a note — design doc
/// Risk 4).
///
/// Names are deliberately NOT collected (masked in consumer builds).
pub fn static_device_readers(app: &App) -> StaticScanResult {
    let world = app.world();
    let signal_ids = device_signal_types(world);

    let mut result = StaticScanResult::default();
    // Per-system dedup: one system reading 3 keyboard signal types
    // counts once for keyboard.
    let mut exclusive_seen = false;
    // Any read-all/write-all system forces every tracked class on
    // (surfaced as a note, mirroring the exclusive case).
    let mut read_all_seen = false;

    let Some(schedules) = world.get_resource::<Schedules>() else {
        result
            .notes
            .push("no Schedules resource found at scan time".to_owned());
        // `complete` stays false: without the schedule graph the
        // zero reader counts prove nothing (prune derivation must
        // refuse them; the collected counts would be empty anyway).
        return result;
    };

    // Complete only if EVERY schedule yielded its systems: one
    // uninitialized schedule hides its systems' reads, so a zero
    // count in that world proves nothing.
    let mut complete = true;
    for (_label, schedule) in schedules.iter() {
        let Ok(systems) = schedule.systems_with_access() else {
            // Uninitialized schedule: its systems' access sets were
            // never computed (they read as `None`), so whatever the
            // GRAPH holds is invisible to the scan. Only systems
            // beyond the known engine drivers make this a blindness:
            // an uninitialized schedule with no systems (or only
            // drivers — `FixedMain` holds exactly
            // `FixedMain::run_fixed_main` and stays uninitialized in
            // apps with no fixed-timestep work, because
            // `RunFixedMainLoop` skips it every frame) hides nothing.
            // Names are readable pre-initialization (unlike access),
            // and the driver filter stays name-EXACT for the same
            // reasons as the scan loop; a renamed/new driver degrades
            // to refusing completeness — fail-open, prune-proof.
            if schedule
                .graph()
                .systems
                .iter()
                .any(|(_key, system, _conds)| {
                    !ENGINE_DRIVER_NAMES.contains(&system.name().to_string().as_str())
                })
            {
                result.notes.push(
                    "at least one nonempty schedule was not initialized at scan time".to_owned(),
                );
                complete = false;
            }
            continue;
        };

        for (_key, swa) in systems {
            // Engine framework drivers (`bevy_app::main_schedule::Main::run_main`,
            // `bevy_ecs::message::update::message_update_system`) are
            // Exclusive systems present in EVERY Bevy 0.20 App; they
            // would inflate every class with phantom readers and fire
            // the exclusive note on every scan. Identify them by their
            // EXACT names (not a loose prefix: a prefix filter would
            // also swallow user crates named `bevy_*` and the
            // harness's own `bevy_playtest::*` systems).
            //
            // Fail-open note: consumer builds MASK system names unless
            // `bevy_ecs/debug` is compiled in, and future Bevy
            // versions may add drivers; both degrade to OVER-counting
            // (drivers included), never to missing a real game
            // system. Names are used only to REMOVE known engine
            // machinery, never to classify game systems.
            let is_engine_driver = {
                let name = swa.system().name();
                let name: &str = &name;
                ENGINE_DRIVER_NAMES.contains(&name)
            };
            match swa.access() {
                // &mut World systems may read anything: fail-open for
                // every class, counted as a reader of each (consistent
                // with the read-all arm — `readers` is annotation-grade
                // and must never imply "no one reads the keyboard"
                // when an exclusive system exists), surfaced as a note.
                SystemAccess::Exclusive if !is_engine_driver => {
                    exclusive_seen = true;
                    // ALL tracked classes, not just REGISTERED signal
                    // ids: a `&mut World` system can read or even
                    // initialize any signal type at runtime —
                    // including classes whose resources are not yet
                    // registered in the world. Iterating
                    // `signal_ids` alone would leave those classes at
                    // zero readers and license the flow to prune
                    // inputs the scan never inspected.
                    let mut counted: Vec<DeviceClass> = Vec::new();
                    for class in DeviceClass::ALL {
                        if !counted.contains(&class) {
                            counted.push(class);
                            bump_reader(&mut result, class);
                        }
                    }
                }
                SystemAccess::Shared(filtered) => {
                    let access = filtered.combined_access();
                    // Read-ALL access (e.g. `Query<EntityRef>`, `&World`):
                    // the system may observe every signal type, so every
                    // TRACKED class gets a reader — fail-open. Verified
                    // against bevy_ecs 0.20.1: `read_all()` flips the
                    // reads set to an unbounded `InvertibleComponentIdSet`,
                    // so `has_read(cid)` already returns true for any
                    // `cid`; the explicit branch is belt-and-braces (same
                    // posture as the `has_write` term in
                    // [`access_observes`]): it documents the intent and
                    // protects the guarantee if a future Bevy stops
                    // answering per-id queries from the read-all set
                    // (pre-0.16 `FixedBitSet` did NOT — a flagged
                    // `contains` would silently under-count here).
                    if access.has_read_all() || access.has_write_all() {
                        read_all_seen = true;
                        // One reader per DISTINCT class — ALL tracked
                        // classes, not just REGISTERED signal ids: a
                        // read-all system observes any signal type it
                        // pleases at runtime, including classes whose
                        // resources are not yet registered (same
                        // under-count trap as the Exclusive arm).
                        let mut counted: Vec<DeviceClass> = Vec::new();
                        for class in DeviceClass::ALL {
                            if !counted.contains(&class) {
                                counted.push(class);
                                bump_reader(&mut result, class);
                            }
                        }
                    } else {
                        let mut counted: Vec<DeviceClass> = Vec::new();
                        for (class, cid) in &signal_ids {
                            if !counted.contains(class) && access_observes(access, *cid) {
                                counted.push(*class);
                            }
                        }
                        for class in counted {
                            bump_reader(&mut result, class);
                        }
                    }
                }
                SystemAccess::None => {}
                // Engine driver Exclusive systems (fell through the
                // guarded arm above): not a game system, contributes
                // neither readers nor the exclusive note.
                SystemAccess::Exclusive => {}
            }
        }
    }

    if exclusive_seen {
        result.notes.push(
            "exclusive system detected: probing the full surface (an exclusive system may read anything)".to_owned(),
        );
    }
    if read_all_seen {
        result.notes.push(
            "read-all system detected: every device class marked as read (a read-all system may observe any input)".to_owned(),
        );
    }

    // Observer-based input consumers: an observer closure may read
    // any input state (`Res<ButtonInput<GamepadButton>>`, a
    // `MessageReader`, ...) alongside its `On<E>` trigger param, and
    // observer internals are private in bevy 0.20 — the schedule walk
    // cannot see those reads. Direct `On<MouseWheel>`-style
    // observation is impossible (bevy 0.20 messages are not Events),
    // but the side-param route means ANY observer in the world can be
    // a hidden input consumer. Zero reader counts then prove nothing,
    // so the scan refuses completeness whenever one exists — the same
    // fail-open posture as the exclusive/read-all arms, and pruning
    // must stay withheld until Bevy exposes observer param access.
    let observers_seen = world_has_observers(world);
    if observers_seen {
        result.notes.push(
            "observer detected: hidden input consumers cannot be \
             ruled out (observer closures may read input state \
             beside their trigger param; bevy 0.20 does not expose \
             observer access sets)"
                .to_owned(),
        );
    }

    result.complete = complete && !observers_seen;
    result
}

/// Whether the world carries ANY observer entity.
///
/// Bare `App::new()` + `MinimalPlugins` worlds hold ZERO observers
/// (verified against bevy 0.20.1), so this fires only for games that
/// actually registered one. Observer bodies may read input state via
/// arbitrary system params (`Res<ButtonInput<…>>`, `MessageReader<…>`)
/// beside their `On<E>` trigger, and bevy 0.20 keeps observer systems
/// `pub(crate)` — their access sets are invisible to the schedule
/// walk — so observer presence alone must withhold prune authority.
///
/// Read-only and structurally neutral: `contains::<Observer>()` never
/// registers components, spawns, or moves entities between archetypes,
/// so this cannot disturb the digests the discovery diff relies on.
fn world_has_observers(world: &bevy_ecs::world::World) -> bool {
    world
        .iter_entities()
        .any(|entity| entity.contains::<bevy_ecs::observer::Observer>())
}

/// Parking slot for the scan captured at harness-setup time.
///
/// NOT a world resource: every resource is an ENTITY in bevy 0.20,
/// so planting `+1 resource` shifts the run's entity/archetype
/// digest counts. The scan-export control run must stay
/// byte-identical to the non-exporting runs — the discovery diff
/// compares their digests — so the capture must not touch the
/// world beyond initializing schedules. A process-global slot is
/// safe here: [`crate::run_scenario`] rejects concurrent runs per
/// process, capture and export both happen inside one child, and
/// each capture overwrites (and each export drains) whatever came
/// before.
static CAPTURED_SCAN: std::sync::Mutex<Option<StaticScanResult>> = std::sync::Mutex::new(None);

/// Drain the scan parked by the last [`capture_static_scan`].
///
/// Callers exporting it later MUST treat absence as "no scan" (fall
/// back or annotate), never as "zero readers".
pub fn take_captured_scan() -> Option<StaticScanResult> {
    CAPTURED_SCAN
        .lock()
        .expect("captured-scan mutex poisoned")
        .take()
}

/// Capture the game's static input-surface scan BEFORE the harness
/// installs its own systems.
///
/// Must be called at a point in harness setup where:
///
/// - every input signal type the harness seeds is REGISTERED
///   (`add_message::<KeyboardInput>` etc.) — a MinimalPlugins game
///   never registers `ButtonInput<KeyCode>` itself, so seeding must
///   precede the capture or the game's real readers would be
///   under-counted (fail-closed, strictly worse than the over-count
///   this routine replaces);
/// - the `!InputPlugin` fallback translation systems are registered
///   (parity with InputPlugin graphs, where the engine's own
///   `keyboard_input_system` is equally visible to the scan);
/// - NO bot or oracle system has been added yet (that is the entire
///   point — `BotKeyboardWriters` writes `KeyboardInput` +
///   `WindowEvent`, `BotMouseWriters` writes `MouseButtonInput` +
///   `WindowEvent`, and `WindowEvent` attributes to the keyboard,
///   button, motion AND wheel classes, so counting them would pollute
///   every class at once and block wheel pruning forever).
///
/// Schedules are FORCE-INITIALIZED here (`Schedule::initialize` is
/// public API in bevy_ecs 0.20.1) rather than via `app.update()`:
/// initializing is not running, so the game is not advanced a frame
/// and no side effects fire. `Schedule::run` re-initializes whenever
/// the graph changed, so the harness's LATER `add_systems` calls stay
/// effective — this is the same hot-add path Bevy itself uses every
/// time a schedule gains systems between frames.
///
/// The result is parked in a process-global slot (see
/// [`take_captured_scan`]), not in the world: inserting a resource
/// would shift the run's entity/archetype digest counts and break
/// the byte-identity the discovery diff relies on between the
/// scan-exporting control run and every other run. The slot is
/// idempotent-per-run by construction: it is overwritten exactly
/// once, from this routine, before the run loop.
pub fn capture_static_scan(app: &mut App) -> StaticScanResult {
    let scan = capture_scan_without_parking(app);
    *CAPTURED_SCAN.lock().expect("captured-scan mutex poisoned") = Some(scan.clone());
    scan
}

/// The capture pipeline minus the slot park: same blindness notes,
/// same force-initialization, same merged verdict. Unit tests that
/// exercise capture SEMANTICS (notes, initialization, completeness)
/// but not slot PARKING use this directly — `capture_static_scan`
/// parks into a process-global slot, and parallel unit tests would
/// otherwise race on it (only the parking-semantics test needs the
/// real slot, under a lock).
fn capture_scan_without_parking(app: &mut App) -> StaticScanResult {
    // Blindness notes collected at CAPTURE time (missing or
    // unbuildable schedules): these must override the post-pass's
    // own completeness verdict, because the post-pass sees only the
    // schedules this capture managed to initialize — it cannot
    // recover blindness the capture itself hit.
    let mut capture_blind_notes: Vec<String> = Vec::new();
    // PRE-UPDATE BLINDNESS (Copilot r4472054577): the harness
    // captures BEFORE the first `app.update()`, so this is the
    // last chance to answer "could the schedule graph or observer
    // set GROW after this scan?" If the world has never been run,
    // the answer is yes: systems may add systems at runtime
    // (`Commands::add_systems`), and observers may be spawned from
    // inside systems — all invisible at capture time and
    // indistinguishable from a bare world. A dynamic registrant
    // would then hide input reads the scan swore were absent.
    // `World::last_change_tick` is the reliable clock for "has
    // this world ever been updated": it starts at 0 and is bumped
    // exactly once per `App::update()` (via `World::clear_trackers`)
    // — bevy's own change-detection machinery relies on the same
    // invariant. Any nonzero value therefore means at least one
    // completed update cycle: the capture-time snapshot can then
    // be trusted to the extent the graph allows.
    if app.world().last_change_tick().get() == 0 {
        capture_blind_notes.push(
            "scan was taken before the world's first update: runtime-added readers \
             (systems/observers registered from inside a system) are invisible"
                .to_owned(),
        );
    }
    // Initialize every schedule WITHOUT running it: a lazily built
    // schedule otherwise returns `ScheduleNotInitialized` from
    // `systems_with_access`, hiding its systems' reads from the
    // scan.
    //
    // STRUCTURAL NEUTRALITY: the `Schedules` resource must stay IN
    // the world the whole time. An earlier design removed and
    // re-inserted the whole resource around the loop — but removing
    // a resource STRIPS the component from its entity, and the
    // stripped entity lands in a fresh (empty, permanent) archetype:
    // the world's archetype count grows by one on the scan-exporting
    // control run ONLY, shifting its digest and poisoning the
    // double-control determinism gate and every probe differential.
    // Instead each schedule is pulled out of the still-resident
    // resource one at a time (`remove_temporarily` + `reinsert`):
    // `Schedule::initialize` then reads the REAL `Schedules`
    // resource for its ambiguity configuration — no stand-in, no
    // resource churn, zero structural footprint.
    let labels: Vec<_> = app
        .world()
        .get_resource::<Schedules>()
        .map(|schedules| schedules.iter().map(|(_, s)| s.label()).collect())
        .unwrap_or_default();
    if app.world().get_resource::<Schedules>().is_none() {
        capture_blind_notes.push("no Schedules resource found at capture time".to_owned());
    }
    for label in &labels {
        // Temporary ownership without touching the resource entity:
        // the Schedules resource stays in the world (minus this one
        // schedule inside its map) while the schedule initializes.
        let Some(mut schedule) = app
            .world_mut()
            .get_resource_mut::<Schedules>()
            .and_then(|mut schedules| schedules.remove_temporarily(*label))
        else {
            continue;
        };
        // RUN-CONDITION CENSUS (review r4474375349): must run BEFORE
        // `Schedule::initialize` — building moves per-system
        // conditions into executor storage that is `pub(super)` in
        // bevy 0.20.1, after which their access is publicly
        // unreadable. The census is also why this loop is the only
        // place completeness-grade blindness can be detected: the
        // post-pass below reads initialized graphs only.
        let hidden = condition_hidden_classes(&mut schedule, app.world_mut());
        if !hidden.is_empty() {
            let classes: Vec<&str> = hidden.iter().map(|c| c.as_str()).collect();
            capture_blind_notes.push(format!(
                "run-condition-held input reads may be hidden for [{}] \
                 (run conditions live outside the system body; the scan \
                 cannot vouch for zero readers there)",
                classes.join(", ")
            ));
        }
        if let Err(err) = schedule.initialize(app.world_mut()) {
            // Refuse completeness rather than lie: an unbuildable
            // schedule hides its systems' reads, so the scan may
            // not claim to have seen them.
            capture_blind_notes.push(format!(
                "schedule failed to initialize during capture: {}",
                err.to_string(schedule.graph(), app.world_mut())
            ));
        }
        // Back into the resident resource, same map key: iteration
        // order and semantics are unchanged for the scan below.
        if let Some(mut schedules) = app.world_mut().get_resource_mut::<Schedules>() {
            schedules.reinsert(schedule);
        }
    }
    let raw = static_device_readers(app);
    merged_from(capture_blind_notes, raw)
}

/// Fold capture-time blindness notes into the post-capture scan's
/// result: any capture-time blindness forces `complete: false` (the
/// post-pass sees only what the capture initialized, so it cannot
/// vouch for the parts the capture missed).
fn merged_from(capture_blind_notes: Vec<String>, mut scan: StaticScanResult) -> StaticScanResult {
    if !capture_blind_notes.is_empty() {
        scan.complete = false;
        scan.notes.extend(capture_blind_notes);
    }
    scan
}

fn bump_reader(result: &mut StaticScanResult, class: DeviceClass) {
    match result.readers.iter_mut().find(|(c, _)| *c == class) {
        Some((_, n)) => *n += 1,
        None => result.readers.push((class, 1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_app::prelude::*;
    use bevy_ecs::prelude::*;

    #[derive(Resource, Default)]
    struct PlayerState {
        x: f32,
    }

    /// A system whose keyboard read the scan CAN see.
    fn keyboard_reader(
        keys: Res<bevy_input::prelude::ButtonInput<bevy_input::keyboard::KeyCode>>,
        mut player: ResMut<PlayerState>,
    ) {
        if keys.pressed(bevy_input::keyboard::KeyCode::KeyW) {
            player.x += 1.0;
        }
    }

    /// A system reading accumulated mouse motion directly.
    fn motion_reader(motion: Res<AccumulatedMouseMotion>) {
        let _ = motion.delta;
    }

    /// A system reading the umbrella `WindowEvent` message stream —
    /// the game consumes RAW window events (keyboard AND mouse alike).
    fn window_event_reader(mut events: MessageReader<WindowEvent>) {
        for _event in events.read() {}
    }

    /// A system WRITING the keyboard state (e.g. a menu that clears
    /// pressed keys on focus loss): writes must count as observing.
    fn keyboard_writer(
        mut keys: ResMut<bevy_input::prelude::ButtonInput<bevy_input::keyboard::KeyCode>>,
    ) {
        keys.reset_all();
    }

    /// Body declares NO params: the input read lives entirely in the
    /// run condition — the documented blind spot.
    fn guarded_system() {
        println!("guarded system ran!");
    }

    fn scan_result(mut app: App) -> StaticScanResult {
        app.update(); // initialize schedules
        static_device_readers(&app)
    }

    #[test]
    fn scan_detects_direct_keyboard_reader() {
        let mut app = App::new();
        app.init_resource::<bevy_input::prelude::ButtonInput<bevy_input::keyboard::KeyCode>>()
            .init_resource::<PlayerState>()
            .add_systems(Update, keyboard_reader);
        let result = scan_result(app);
        assert_eq!(result.reader_count(DeviceClass::Keyboard), 1);
    }

    #[test]
    fn scan_detects_message_reader() {
        let mut app = App::new();
        app.init_resource::<Messages<KeyboardInput>>().add_systems(
            Update,
            |mut reader: MessageReader<KeyboardInput>| {
                reader.clear();
            },
        );
        let result = scan_result(app);
        assert_eq!(result.reader_count(DeviceClass::Keyboard), 1);
    }

    #[test]
    fn scan_detects_gamepad_message_reader() {
        // Copilot r4472055214-style fix: a `MessageReader<GamepadEvent>`
        // (the umbrella the engine converts every processed gamepad
        // event into) reads NO Gamepad resource/state — the old
        // signal-id list saw it as zero gamepad readers and wrongly
        // licensed gamepad pruning. The raw message channels must be
        // tracked like `Messages<KeyboardInput>` is for keyboards.
        let mut app = App::new();
        app.add_message::<GamepadEvent>().add_systems(
            Update,
            |mut reader: MessageReader<GamepadEvent>| {
                reader.clear();
            },
        );
        let result = scan_result(app);
        assert_eq!(
            result.reader_count(DeviceClass::Gamepad),
            1,
            "a MessageReader<GamepadEvent> must count as a gamepad reader"
        );
        // Cross-class sanity: the gamepad message channel must not
        // bleed into other classes.
        assert_eq!(result.reader_count(DeviceClass::Keyboard), 0);
        assert_eq!(result.reader_count(DeviceClass::MouseWheel), 0);
    }

    #[test]
    fn scan_detects_raw_mouse_motion_and_wheel_message_readers() {
        // Review r4474375275: `MessageReader<MouseMotion>` (raw
        // per-frame motion packets) and `MessageReader<MouseWheel>`
        // (raw scroll packets) read NEITHER the accumulated
        // resources NOR the umbrella `WindowEvent`, so the old
        // signal table scanned them as zero motion/wheel readers and
        // wrongly licensed pruning (fail-closed hole).
        let mut app = App::new();
        app.add_message::<bevy_input::mouse::MouseMotion>()
            .add_message::<bevy_input::mouse::MouseWheel>()
            .add_systems(
                Update,
                (
                    |mut motion: MessageReader<bevy_input::mouse::MouseMotion>| {
                        motion.clear();
                    },
                    |mut wheel: MessageReader<bevy_input::mouse::MouseWheel>| {
                        wheel.clear();
                    },
                ),
            );
        let result = scan_result(app);
        assert_eq!(
            result.reader_count(DeviceClass::MouseMotion),
            1,
            "a MessageReader<MouseMotion> must count as a mouse-motion reader"
        );
        assert_eq!(
            result.reader_count(DeviceClass::MouseWheel),
            1,
            "a MessageReader<MouseWheel> must count as a mouse-wheel reader"
        );
        // Cross-class sanity: neither channel bleeds elsewhere.
        assert_eq!(result.reader_count(DeviceClass::Keyboard), 0);
        assert_eq!(result.reader_count(DeviceClass::MouseButton), 0);
        assert_eq!(result.reader_count(DeviceClass::Gamepad), 0);
    }

    #[test]
    fn scan_detects_logical_key_state_reader() {
        // Review r4474375394: `Res<ButtonInput<Key>>` — bevy 0.20's
        // LOGICAL key state, seeded by the driver alongside the
        // physical `ButtonInput<KeyCode>` — is a keyboard reader
        // too. A game checking `just_pressed(Key::Escape)` through
        // the logical channel scanned as zero keyboard readers and
        // earned no keyboard probes despite answering them daily.
        let mut app = App::new();
        app.init_resource::<bevy_input::prelude::ButtonInput<bevy_input::keyboard::Key>>()
            .add_systems(
                Update,
                |keys: Res<bevy_input::prelude::ButtonInput<bevy_input::keyboard::Key>>| {
                    let _ = keys.pressed(bevy_input::keyboard::Key::Escape);
                },
            );
        let result = scan_result(app);
        assert_eq!(
            result.reader_count(DeviceClass::Keyboard),
            1,
            "a Res<ButtonInput<Key>> reader must count as a keyboard reader"
        );
        // Cross-class sanity: the logical channel is keyboard-only.
        assert_eq!(result.reader_count(DeviceClass::Gamepad), 0);
        assert_eq!(result.reader_count(DeviceClass::MouseWheel), 0);
    }

    #[test]
    fn scan_detects_raw_gamepad_event_reader() {
        // The BACKEND feed channel: games listening to hardware
        // directly (`MessageReader<RawGamepadEvent>`, pre-filtering)
        // are gamepad readers too — and so is a consumer of any of
        // the per-event channels or the rumble REQUEST channel (a
        // game sending rumble still plays on the gamepad surface).
        let mut app = App::new();
        app.add_message::<RawGamepadEvent>()
            .add_message::<GamepadConnectionEvent>()
            .add_message::<GamepadButtonStateChangedEvent>()
            .add_message::<GamepadButtonChangedEvent>()
            .add_message::<GamepadAxisChangedEvent>()
            .add_message::<GamepadRumbleRequest>()
            .add_systems(
                Update,
                (
                    |mut raw: MessageReader<RawGamepadEvent>| {
                        raw.clear();
                    },
                    |mut conn: MessageReader<GamepadConnectionEvent>| {
                        conn.clear();
                    },
                    |mut digital: MessageReader<GamepadButtonStateChangedEvent>| {
                        digital.clear();
                    },
                    |mut analog: MessageReader<GamepadButtonChangedEvent>| {
                        analog.clear();
                    },
                    |mut axis: MessageReader<GamepadAxisChangedEvent>| {
                        axis.clear();
                    },
                    |mut rumble: MessageReader<GamepadRumbleRequest>| {
                        rumble.clear();
                    },
                ),
            );
        let result = scan_result(app);
        // Six DISTINCT readers, one per channel-consuming system
        // (per-system dedup: one system reading several gamepad
        // signal types still counts once).
        assert_eq!(
            result.reader_count(DeviceClass::Gamepad),
            6,
            "every gamepad message channel must track readers: {:?}",
            result.readers
        );
    }

    #[test]
    fn scan_detects_mouse_motion_reader() {
        let mut app = App::new();
        app.init_resource::<AccumulatedMouseMotion>()
            .add_systems(Update, motion_reader);
        let result = scan_result(app);
        assert_eq!(result.reader_count(DeviceClass::MouseMotion), 1);
    }

    #[test]
    fn window_event_reader_counts_for_every_covered_class() {
        // `WindowEvent` is bevy_window's umbrella backend event: its
        // variants carry KeyboardInput, MouseButtonInput, MouseMotion
        // AND MouseWheel, so a `MessageReader<WindowEvent>` observes
        // all four channels. Keyboard-only attribution would let a
        // mouse-only WindowEvent reader masquerade as a keyboard game
        // — earning keyboard probes it can never answer — while the
        // game's real mouse surface went undetected.
        let mut app = App::new();
        app.add_message::<WindowEvent>()
            .add_systems(Update, window_event_reader);
        let result = scan_result(app);
        assert_eq!(result.reader_count(DeviceClass::Keyboard), 1);
        assert_eq!(result.reader_count(DeviceClass::MouseButton), 1);
        assert_eq!(result.reader_count(DeviceClass::MouseMotion), 1);
        assert_eq!(result.reader_count(DeviceClass::MouseWheel), 1);
        // Sanity: the umbrella did NOT bleed into unrelated classes.
        assert_eq!(result.reader_count(DeviceClass::Gamepad), 0);
    }

    #[test]
    fn scan_counts_input_writers_as_observers() {
        // Writers of input state (ResMut<ButtonInput<KeyCode>>) must
        // count as keyboard-touching systems: has_write implies the
        // game manipulates that device's state (menu key-clearing,
        // rebind layers, input-recording). End-to-end through a real
        // App: NOTE that current Bevy also sets the READ bit for
        // `ResMut`, so this test alone cannot distinguish a
        // read-only predicate from a read-or-write one — the
        // write-only-pin lives in `write_only_access_observes`.
        let mut app = App::new();
        app.init_resource::<bevy_input::prelude::ButtonInput<bevy_input::keyboard::KeyCode>>()
            .add_systems(Update, keyboard_writer);
        let result = scan_result(app);
        assert_eq!(result.reader_count(DeviceClass::Keyboard), 1);
    }

    #[test]
    fn scan_stays_complete_when_no_observers_exist() {
        // Baseline: with no observers, completeness is untouched (the
        // bare-app case, mirroring `scan_reports_no_readers_for_engine_noise_only_app`).
        let mut app = App::new();
        app.init_resource::<bevy_input::prelude::ButtonInput<bevy_input::keyboard::KeyCode>>()
            .init_resource::<PlayerState>()
            .add_systems(Update, keyboard_reader);
        let result = scan_result(app);
        assert!(result.complete);
        assert_eq!(result.reader_count(DeviceClass::Keyboard), 1);
    }

    #[test]
    fn scan_withholds_completeness_when_an_observer_exists() {
        // An observer's params are invisible to the schedule walk, so
        // ANY observer (even one with no input params at all) makes
        // zero reader counts unprovable: `complete` must flip false
        // and the scan must say WHY, or prune derivation would treat
        // hidden consumers as absent devices (review finding: an
        // observer consuming MouseWheel/Gamepad input via side params
        // earned a zero reader count and got pruned).
        let mut app = App::new();
        app.init_resource::<bevy_input::prelude::ButtonInput<bevy_input::keyboard::KeyCode>>()
            .init_resource::<PlayerState>()
            .add_systems(Update, keyboard_reader);
        #[derive(Event)]
        struct UnrelatedPing;
        app.add_observer(|_: On<UnrelatedPing>| {});
        let result = scan_result(app);
        // Counts are still annotation-grade truth: the keyboard
        // reader the scan DID see still counts.
        assert_eq!(result.reader_count(DeviceClass::Keyboard), 1);
        assert!(!result.complete);
        assert!(
            result
                .notes
                .iter()
                .any(|note| note.contains("observer detected")),
            "the observer blindness must be surfaced as a note, got {:?}",
            result.notes
        );
    }

    #[test]
    fn scan_marks_input_reading_observer_side_params_unprovable() {
        // The exact hole the review found: an observer whose TRIGGER
        // is unrelated but whose side params read gamepad state. The
        // schedule walk counts the visible keyboard reader only; the
        // gamepad read lives in observer internals bevy 0.20 keeps
        // private. The scan must NOT claim the gamepad class
        // readerless — completeness is withheld, prune derivation
        // refuses the zero count, the surface stays fail-open.
        let mut app = App::new();
        app.init_resource::<bevy_input::prelude::ButtonInput<bevy_input::keyboard::KeyCode>>()
            .init_resource::<PlayerState>()
            .add_systems(Update, keyboard_reader);
        #[derive(Event)]
        struct UnrelatedPing;
        app.add_observer(
            |_: On<UnrelatedPing>,
             _buttons: Res<
                bevy_input::prelude::ButtonInput<bevy_input::gamepad::GamepadButton>,
            >| {},
        );
        let result = scan_result(app);
        assert_eq!(result.reader_count(DeviceClass::Keyboard), 1);
        // The gamepad read is hidden — the count stays 0, which is
        // precisely why `complete` must be false.
        assert_eq!(result.reader_count(DeviceClass::Gamepad), 0);
        assert!(!result.complete);
    }

    #[test]
    fn capture_reads_the_game_before_running_it() {
        // The capture exists to take the scan WITHOUT an
        // `app.update()` first: a cold-built game's schedules are
        // uninitialized, so `systems_with_access` would refuse and
        // the scan would be incomplete. Force-initialization makes
        // a cold app's systems visible.
        let mut app = App::new();
        app.init_resource::<bevy_input::prelude::ButtonInput<bevy_input::keyboard::KeyCode>>()
            .init_resource::<PlayerState>()
            .add_systems(Update, keyboard_reader);
        // Slot-free variant: this test checks capture SEMANTICS
        // (pre-update blindness), not slot parking — writing the
        // process-global slot would race with the parking test
        // running in parallel (review r4474375436).
        let scan = capture_scan_without_parking(&mut app);
        assert_eq!(scan.reader_count(DeviceClass::Keyboard), 1);
        // Pre-update honesty (Copilot r4472054577): the capture is
        // taken BEFORE the first `app.update()`, so systems or
        // observers registered AT RUNTIME (from inside a system —
        // e.g. an observer spawned on player spawn) cannot have
        // happened yet and are indistinguishable from a bare world
        // at capture time. Dynamic registration cannot be ruled
        // out, so the captured scan may never license pruning.
        assert!(!scan.complete);
        assert!(
            scan.notes
                .iter()
                .any(|n| n.contains("scan was taken before the world's first update")),
            "pre-update blindness must be surfaced as a note, got {:?}",
            scan.notes
        );
    }

    #[test]
    fn capture_on_ran_app_can_claim_completeness() {
        // Positive control for the pre-update gate: after a real
        // `app.update()` the capture's blindness note is gone and a
        // healthy app's scan may again claim completeness (the
        // takeover's post-scenario scans rely on this staying true).
        let mut app = App::new();
        app.init_resource::<bevy_input::prelude::ButtonInput<bevy_input::keyboard::KeyCode>>()
            .init_resource::<PlayerState>()
            .add_systems(Update, keyboard_reader);
        app.update();
        let scan = capture_scan_without_parking(&mut app);
        assert_eq!(scan.reader_count(DeviceClass::Keyboard), 1);
        assert!(
            scan.notes
                .iter()
                .all(|n| !n.contains("scan was taken before the world's first update")),
            "a post-update capture must not carry the pre-update note, got {:?}",
            scan.notes
        );
        assert!(scan.complete);
    }

    #[test]
    fn capture_flags_condition_hidden_gamepad_read() {
        // Review r4474375349: a body declaring NO params gated by
        // `run_if(input_just_pressed(GamepadButton::South))` holds its
        // input read ENTIRELY in the condition. The post-pass walk
        // (initialized access sets) sees an empty body — zero gamepad
        // readers — so a complete-looking capture would license
        // gamepad pruning on evidence that proves nothing. The
        // capture-time condition census must surface the class as
        // condition-hidden: a note + `complete: false`, so pruning
        // stays withheld. Readers are NOT fabricated (the census
        // annotates blindness; `readers` stays honest about what was
        // actually counted) — the gamepad count stays 0 and that
        // zero must now be unusable for pruning.
        let mut app = App::new();
        app.init_resource::<bevy_input::prelude::ButtonInput<GamepadButton>>();
        app.add_systems(
            Update,
            guarded_system.run_if(bevy_input::common_conditions::input_just_pressed(
                GamepadButton::South,
            )),
        );
        let scan = capture_scan_without_parking(&mut app);
        assert_eq!(
            scan.reader_count(DeviceClass::Gamepad),
            0,
            "readers must stay honest: the census notes blindness, it does \
             not fabricate counts — got {:?}",
            scan.readers
        );
        assert!(
            !scan.complete,
            "a condition-hidden gamepad read must withhold completeness"
        );
        let note = scan
            .notes
            .iter()
            .find(|n| n.contains("run-condition-held"))
            .expect("the census must surface a run-condition blindness note");
        assert!(
            note.contains("gamepad"),
            "the note must name the hidden class, got: {note}"
        );
    }

    #[test]
    fn capture_does_not_flag_condition_matching_visible_body_read() {
        // Complementary precision control: a condition reading a class
        // the system's OWN body already observes adds nothing — the
        // post-pass counts that system for the class either way. The
        // census must stay silent (no run-condition note), or every
        // legitimately-conditioned input consumer would permanently
        // withhold pruning for noise.
        let mut app = App::new();
        app.init_resource::<bevy_input::prelude::ButtonInput<bevy_input::keyboard::KeyCode>>();
        app.add_systems(
            Update,
            keyboard_reader.run_if(bevy_input::common_conditions::input_just_pressed(
                bevy_input::keyboard::KeyCode::Space,
            )),
        );
        let scan = capture_scan_without_parking(&mut app);
        assert_eq!(scan.reader_count(DeviceClass::Keyboard), 1);
        assert!(
            scan.notes.iter().all(|n| !n.contains("run-condition-held")),
            "a condition matching a visible body read is not blindness, got {:?}",
            scan.notes
        );
    }

    #[test]
    fn capture_exempts_the_baseline_conditioned_driver() {
        // Every default SubApp seeds
        // `message_update_system.run_if(message_update_condition)` —
        // the one conditioned BASELINE system in every App. Without
        // the ENGINE_DRIVER_NAMES exemption in the census, every
        // capture would cry blindness and completeness would be
        // unclaimable anywhere (breaking
        // `capture_on_ran_app_can_claim_completeness` and the
        // takeover's post-scenario scans). A bare App must therefore
        // produce NO run-condition note even though a conditioned
        // driver exists in its schedules.
        let mut app = App::new();
        let scan = capture_scan_without_parking(&mut app);
        assert!(
            scan.notes.iter().all(|n| !n.contains("run-condition-held")),
            "the seeded driver condition must be exempt, got {:?}",
            scan.notes
        );
    }

    #[test]
    fn pre_update_blindness_is_real_runtime_registration() {
        // The blindness the pre-update gate guards against is not
        // hypothetical: a system CAN add another system at runtime —
        // `Schedules` is a plain resource, so a system holding
        // `ResMut<Schedules>` calls `add_systems` on it mid-run. On
        // a cold world the to-be-added system is invisible to any
        // scan; it only enters the schedule graph once its
        // registering system RUNS — i.e. after the first update.
        // Reproduce the mechanism end-to-end so the gate's threat
        // model stays anchored to real API, not folklore.
        let mut app = App::new();
        app.init_resource::<AccumulatedMouseMotion>().add_systems(
            Startup,
            |mut schedules: ResMut<Schedules>| {
                schedules.add_systems(Update, |motion: Res<AccumulatedMouseMotion>| {
                    let _ = motion.delta;
                });
            },
        );
        // Pre-update scan (the capture point): the motion reader is
        // not in any schedule graph yet. Run the app and compare
        // the LIVE world before/after: the reader can only appear
        // once the registering Startup system has actually RUN.
        let cold = static_device_readers(&app);
        app.update();
        let warm = static_device_readers(&app);
        assert_eq!(
            warm.reader_count(DeviceClass::MouseMotion),
            cold.reader_count(DeviceClass::MouseMotion) + 1,
            "the runtime-registered motion reader must appear only after \
             the first update: the pre-update scan could not have seen it"
        );
    }

    #[test]
    fn capture_pins_capture_time_semantics_before_later_systems() {
        // THE purity pin (Copilot r4469158382): the scan riding out
        // of `run_scenario` must reflect the WORLD AT CAPTURE TIME.
        // Systems added AFTER the capture — including the harness's
        // own bot writers — must NOT appear in the stored scan. This
        // is what keeps harness instrumentation invisible in exported
        // reader counts (and preserves wheel pruning, which the
        // bot's umbrella `WindowEvent` writes would otherwise block
        // in every game).
        //
        // The ONLY unit test allowed to touch the process-global
        // slot (review r4474375436): parking-semantics coverage
        // needs the real `capture_static_scan`. Held under a static
        // test-suite-wide lock so it cannot interleave with the
        // integration suite's slot usage (and with any future
        // slot-touching test) — same pattern as
        // `tests/common/mod.rs::run_slot`.
        static SLOT: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _slot = SLOT.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut app = App::new();
        app.init_resource::<bevy_input::prelude::ButtonInput<bevy_input::keyboard::KeyCode>>()
            .init_resource::<PlayerState>()
            .add_systems(Update, keyboard_reader);
        capture_static_scan(&mut app);
        // Post-capture addition: mimic the harness's bots — a writer
        // of the umbrella `WindowEvent`, which attributes to the
        // keyboard, button, motion AND wheel classes.
        app.add_message::<WindowEvent>()
            .add_systems(Update, window_event_reader);
        let scan = take_captured_scan().expect("capture must park the scan");
        assert_eq!(
            scan.reader_count(DeviceClass::Keyboard),
            1,
            "only the pre-capture keyboard reader may count"
        );
        assert_eq!(
            scan.reader_count(DeviceClass::MouseWheel),
            0,
            "post-capture WindowEvent writers must stay invisible to the captured scan"
        );
        assert_eq!(scan.reader_count(DeviceClass::MouseButton), 0);
        assert_eq!(scan.reader_count(DeviceClass::MouseMotion), 0);
        // Contrast with a LIVE post-run scan, which does see the
        // late additions: proves the parked scan pins capture-time
        // semantics, not just some other property of the scan.
        app.update(); // initialize the late additions' access sets
        let live = static_device_readers(&app);
        assert_eq!(live.reader_count(DeviceClass::MouseWheel), 1);
        assert_eq!(live.reader_count(DeviceClass::Keyboard), 2);
    }

    #[test]
    fn capture_survives_without_schedules() {
        // A stripped/minimal world must not panic the capture. Note
        // bevy 0.20's `App::empty()` still carries an EMPTY
        // `Schedules` resource (World::new seeds it), so the
        // interesting pin is resilience + a sane verdict: an empty
        // schedule set truly hides nothing, so the scan may claim
        // completeness — a schedule-less game has no input readers.
        let mut app = bevy_app::App::empty();
        let scan = capture_scan_without_parking(&mut app);
        assert_eq!(
            scan.readers,
            Vec::new(),
            "no schedules means no readers to report"
        );
        assert!(
            scan.notes
                .iter()
                .all(|n| !n.contains("no Schedules resource")),
            "an empty-but-present Schedules resource is not blindness: {:?}",
            scan.notes
        );
        // Pre-update honesty (Copilot r4472054577): the capture runs
        // before the world's first update, so runtime registration
        // cannot be ruled out — completeness must be refused even
        // though the (empty) schedule set itself hides nothing.
        assert!(
            !scan.complete,
            "pre-update captures may never claim completeness (runtime-added \
             readers/observers are invisible)"
        );
        assert!(
            scan.notes
                .iter()
                .any(|n| n.contains("scan was taken before the world's first update")),
            "pre-update blindness must be surfaced as a note, got {:?}",
            scan.notes
        );
    }

    /// A system with read-ALL component access: it may observe any
    /// input signal through generic traversal, so EVERY tracked class
    /// must register a reader (fail-open). `Query<EntityRef>` is the
    /// canonical construction — current Bevy marks its combined
    /// access read-all.
    fn read_all_system(query: Query<bevy_ecs::world::EntityRef>) {
        for entity in query.iter() {
            // Generic traversal: which components are inspected is
            // unknowable statically.
            let _ = entity.id();
        }
    }

    #[test]
    fn read_all_system_observes_every_device_class() {
        // Copilot r4465680069: a read-all system matched no individual
        // signal ComponentId in the old branch, so the scan could
        // report ZERO readers while the system actually reads
        // keyboard/gamepad state via &World-style access. Pin the
        // correct behavior end-to-end: every TRACKED class counts a
        // reader, and the read-all note surfaces for consumers.
        let mut app = App::new();
        app.init_resource::<bevy_input::prelude::ButtonInput<bevy_input::keyboard::KeyCode>>()
            .init_resource::<bevy_input::prelude::ButtonInput<bevy_input::mouse::MouseButton>>()
            .init_resource::<bevy_input::prelude::ButtonInput<GamepadButton>>()
            .init_resource::<bevy_input::prelude::Axis<GamepadAxis>>()
            .init_resource::<AccumulatedMouseMotion>()
            .init_resource::<AccumulatedMouseScroll>()
            .add_systems(Update, read_all_system);
        let result = scan_result(app);
        for class in [
            DeviceClass::Keyboard,
            DeviceClass::MouseButton,
            DeviceClass::MouseMotion,
            DeviceClass::MouseWheel,
            DeviceClass::Gamepad,
        ] {
            assert_eq!(
                result.reader_count(class),
                1,
                "read-all system must count as a reader of {class:?}"
            );
        }
        assert!(
            result
                .notes
                .iter()
                .any(|n| n.contains("read-all system detected")),
            "the read-all note must surface for consumers: {:?}",
            result.notes
        );
    }

    #[test]
    fn engine_drivers_are_invisible_to_the_scan() {
        // Every Bevy 0.20 App carries Exclusive engine drivers
        // (`Main::run_main`, `message_update_system`). Before the
        // driver filter they inflated every class with phantom
        // readers (+2 per class in EVERY scan) and made the
        // "exclusive system detected" note fire constantly — pure
        // noise for consumers. After the filter, an App with ONLY
        // the drivers reports zero readers and no exclusive note;
        // scanning a bare App must stay silent.
        let mut app = App::new();
        app.init_resource::<bevy_input::prelude::ButtonInput<bevy_input::keyboard::KeyCode>>()
            .init_resource::<bevy_input::prelude::ButtonInput<bevy_input::mouse::MouseButton>>()
            .init_resource::<bevy_input::prelude::ButtonInput<GamepadButton>>()
            .init_resource::<bevy_input::prelude::Axis<GamepadAxis>>()
            .init_resource::<AccumulatedMouseMotion>()
            .init_resource::<AccumulatedMouseScroll>();
        let result = scan_result(app);
        for class in [
            DeviceClass::Keyboard,
            DeviceClass::MouseButton,
            DeviceClass::MouseMotion,
            DeviceClass::MouseWheel,
            DeviceClass::Gamepad,
        ] {
            assert_eq!(
                result.reader_count(class),
                0,
                "engine drivers must not count as readers of {class:?}"
            );
        }
        assert!(
            !result
                .notes
                .iter()
                .any(|n| n.contains("exclusive system detected")),
            "bare App must not trigger the exclusive note: {:?}",
            result.notes
        );
    }

    #[test]
    fn exclusive_system_observes_every_device_class() {
        // Copilot r4466005661: an exclusive `&mut World` system may
        // read any input signal, but the old arm only set a note flag
        // — `StaticScanResult` reported ZERO readers for every class,
        // contradicting the fail-open contract the note claims.
        // Exclusive systems must count as a reader of each TRACKED
        // class, exactly like the read-all arm (consistent
        // annotation-grade data + the exclusive note).
        let mut app = App::new();
        app.init_resource::<bevy_input::prelude::ButtonInput<bevy_input::keyboard::KeyCode>>()
            .init_resource::<bevy_input::prelude::ButtonInput<bevy_input::mouse::MouseButton>>()
            .init_resource::<bevy_input::prelude::ButtonInput<GamepadButton>>()
            .init_resource::<bevy_input::prelude::Axis<GamepadAxis>>()
            .init_resource::<AccumulatedMouseMotion>()
            .init_resource::<AccumulatedMouseScroll>()
            .add_systems(Update, |world: &mut World| {
                // &mut World traversal: which components are inspected
                // is unknowable statically.
                let _ = world.entities().len();
            });
        let result = scan_result(app);
        for class in [
            DeviceClass::Keyboard,
            DeviceClass::MouseButton,
            DeviceClass::MouseMotion,
            DeviceClass::MouseWheel,
            DeviceClass::Gamepad,
        ] {
            assert_eq!(
                result.reader_count(class),
                1,
                "exclusive system must count as a reader of {class:?}"
            );
        }
        assert!(
            result
                .notes
                .iter()
                .any(|n| n.contains("exclusive system detected")),
            "the exclusive note must still surface: {:?}",
            result.notes
        );
    }

    #[test]
    fn exclusive_system_counts_unregistered_classes_too() {
        // Copilot r4467758979: the fail-open arms used to iterate the
        // REGISTERED signal ids — an exclusive system in a world that
        // never initialized (say) gamepad or wheel input left those
        // classes at zero readers, and the flow's prune derivation
        // then licensed pruning inputs the scan never inspected. A
        // `&mut World` system can read or initialize any signal type
        // at runtime, so the exclusive arm must bump EVERY tracked
        // class regardless of registration.
        let mut app = App::new();
        // Keyboard ONLY: mouse-button/motion/wheel/gamepad resources
        // and messages are deliberately NOT registered — the
        // registered-signal-id iteration this test pins produced
        // zeros for them.
        app.init_resource::<bevy_input::prelude::ButtonInput<bevy_input::keyboard::KeyCode>>()
            .add_systems(Update, |world: &mut World| {
                let _ = world.entities().len();
            });
        let result = scan_result(app);
        for class in DeviceClass::ALL {
            assert_eq!(
                result.reader_count(class),
                1,
                "exclusive system must count as a reader of {class:?} even when its \
                 signals are not registered"
            );
        }
    }

    #[test]
    fn read_all_system_counts_unregistered_classes_too() {
        // Same trap, read-all arm (r4467758979 explicitly asks for
        // the fix in both branches): `Query<EntityRef>` observes any
        // component at runtime, so unregistered classes must not be
        // left at zero.
        let mut app = App::new();
        app.init_resource::<bevy_input::prelude::ButtonInput<bevy_input::keyboard::KeyCode>>()
            .add_systems(Update, read_all_system);
        let result = scan_result(app);
        for class in DeviceClass::ALL {
            assert_eq!(
                result.reader_count(class),
                1,
                "read-all system must count as a reader of {class:?} even when its \
                 signals are not registered"
            );
        }
    }

    #[test]
    fn uninitialized_schedule_marks_the_scan_incomplete() {
        // Copilot r4467758892 (scanner side): a schedule whose
        // systems were never initialized hides every read it would
        // have declared, so the scan result must say so via
        // `complete: false` — the flow's prune derivation refuses
        // zero counts from such a scan (see the flow tests).
        let mut app = App::new();
        app.init_resource::<bevy_input::prelude::ButtonInput<bevy_input::keyboard::KeyCode>>()
            .add_systems(Update, keyboard_reader);
        // Update systems never `.initialize()`d: the bare App's
        // schedules are stored uninitialized, and the scan must
        // mark the result incomplete rather than report the absence
        // of those systems' reads as meaningful zeros. NOTE: bypass
        // the `scan_result` helper — it calls `app.update()`, which
        // is exactly the initialization this test must avoid.
        let result = static_device_readers(&app);
        assert!(
            !result.complete,
            "an uninitialized schedule holding a game system must mark the scan incomplete"
        );
        assert!(
            result
                .notes
                .iter()
                .any(|n| n.contains("was not initialized at scan time")),
            "the incompleteness must be surfaced as a note: {:?}",
            result.notes
        );
    }

    #[test]
    fn fully_initialized_app_yields_a_complete_scan() {
        // Mirror guard: a NORMAL initialized app (schedules run at
        // least once before the scan, as the takeover guarantees)
        // must come back complete — the flag must not refuse pruning
        // for healthy scans.
        let mut app = App::new();
        app.init_resource::<bevy_input::prelude::ButtonInput<bevy_input::keyboard::KeyCode>>()
            .init_resource::<PlayerState>()
            .add_systems(Update, keyboard_reader);
        app.update(); // initializes schedules
        let result = scan_result(app);
        assert!(
            result.complete,
            "a fully initialized app must scan complete (notes: {:?})",
            result.notes
        );
    }

    #[test]
    fn read_all_semantics_of_current_bevy_are_pinned() {
        // Semantics pin, guarding the guard: in bevy_ecs 0.20.1 the
        // reads set is an `InvertibleComponentIdSet` where `read_all()`
        // switches it to the unbounded `Excluded(∅)` representation,
        // so `has_read(cid)` returns TRUE for every id already, and
        // `has_read_all()` is precisely `reads.is_all()`. If this pin
        // ever fails, Bevy changed the read-all representation and the
        // explicit `has_read_all()` branch in `static_device_readers`
        // becomes LOAD-BEARING rather than belt-and-braces.
        let any_id = ComponentId::new(0);
        let mut access = Access::default();
        access.read_all();
        assert!(access.has_read_all());
        // The property the scan's correctness rests on: read-all
        // answers per-id queries affirmatively for ANY id.
        assert!(access.has_read(any_id));
        assert!(access_observes(&access, any_id));
    }

    #[test]
    fn write_only_access_observes() {
        // Unit pin for the `|| has_write` term in `access_observes`.
        //
        // IMPORTANT — verified against the bevy_ecs 0.20.1 source
        // (`Access::add_write`): `add_write` inserts BOTH the write
        // bit AND the read bit, so a provably WRITE-ONLY access set
        // cannot be constructed in this Bevy version; every writer
        // (`ResMut`, `add_write`) is already visible to `has_read`.
        // The `|| has_write` term is therefore belt-and-braces: it
        // documents that input-state writers (menu key-clearing,
        // rebind layers) must count as observers, and keeps the scan
        // correct if a future Bevy stops making writes imply reads.
        //
        // What CAN be pinned today: any access with the write bit
        // set is observed (regardless of the read bit), and the
        // mutual implication between the bits holds.
        let any_id = ComponentId::new(0);
        let mut access = Access::default();
        access.add_write(any_id);
        assert!(
            access.has_write(any_id),
            "precondition: write bit set by add_write"
        );
        assert!(
            access_observes(&access, any_id),
            "write bit must imply observation"
        );

        // Mutual implication in current Bevy (guards the guard: if
        // this ever fails, `add_write` semantics changed and the
        // synthetic pin above becomes the load-bearing one).
        assert_eq!(access.has_read(any_id), access.has_write(any_id));

        // Symmetric control: read-only access also observes.
        let mut read_access = Access::default();
        read_access.add_read(any_id);
        assert!(access_observes(&read_access, any_id));

        // Neither-bit control: unrelated ids do not observe.
        let unrelated = ComponentId::new(1);
        assert!(!access_observes(&access, unrelated));
    }

    #[test]
    fn run_condition_blind_spot_is_real_but_fail_open() {
        // The system gated by input_just_pressed(Space) appears in the
        // schedule with NO keyboard access of its own (condition owns
        // the access). The scan must therefore report zero keyboard
        // readers here — and callers must keep the keyboard class
        // fail-open regardless of that (asserted by policy tests, not
        // by the scan itself).
        use bevy_input::common_conditions::input_just_pressed;
        use bevy_input::keyboard::KeyCode;
        let mut app = App::new();
        app.init_resource::<bevy_input::prelude::ButtonInput<KeyCode>>()
            .add_systems(
                Update,
                guarded_system.run_if(input_just_pressed(KeyCode::Space)),
            );
        let result = scan_result(app);
        assert_eq!(
            result.reader_count(DeviceClass::Keyboard),
            0,
            "BLIND SPOT BROKEN: guarded system unexpectedly reports keyboard \
             access; re-evaluate the fail-open strategy in the design doc"
        );
    }

    #[test]
    fn scan_reports_no_readers_for_engine_noise_only_app() {
        // A bare App with no game systems: no input resources exist,
        // so the scan must find nothing (and not crash on the
        // type-erased lookups returning None).
        let result = scan_result(App::new());
        assert!(result.readers.is_empty());
    }
}
