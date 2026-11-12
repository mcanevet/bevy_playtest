//! Authoring-time CONTRACT SUGGESTIONS (plug-and-play work item 2).
//!
//! When discovery runs (env-gated, unchanged trigger — this module
//! only executes inside the static-scan capture that the probe
//! pool's first control job requests), the draft gains suggestions
//! derived PURELY from structural reads of the game's schedule
//! graph: writer/reader lists per game-owned resource, a candidate
//! invariant menu for written resources, and dead-message-channel
//! candidates (channels with zero readers).
//!
//! Epistemic status — suggestions are NOT findings:
//!
//! - Every row is a DRAFT suggestion awaiting human confirmation.
//!   None of this data feeds grading, alarms, pruning, or any
//!   gate: it lands in [`super::surface::DiscoveredSurface`] and
//!   [`super::draft::DraftOutcome`] as a purely additive, reported
//!   section (the `lw` precedent), and consumers must treat every
//!   row as machine-proposed, never as a claim about game
//!   correctness.
//! - Every row names its derivation in `derivation` (a
//!   `systems_with_access` schedule walk at load time; static,
//!   structural, no play-grounding) so downstream consumers know
//!   exactly what stands behind it.
//!
//! Scope honesty: the brief asks for writer/reader lists "per
//! input-relevant resource". Probe effects are HASH-ONLY (see
//! [`super::diff`]) — no artifact can name the resources a probe
//! touched — so the walk covers EVERY game-owned resource instead:
//! a structural superset of "input-relevant", same provenance on
//! every row, and the draft-limit notes say so. Restricting to
//! input-signal-adjacent resources would guess at relevance the
//! evidence cannot support.
//!
//! Blindness doctrine (mirrors [`super::static_scan`]):
//!
//! - Zero-reader claims ("dead channel") are the only NEGATIVE
//!   suggestions here, and negatives are exactly where blindness
//!   bites. They are emitted ONLY when the walk can vouch it saw
//!   everything: every schedule initialized, no exclusive or
//!   read-all/write-all system, no observers, names readable, and
//!   no condition-held reads on the target (run conditions live
//!   outside the system body; `collect_condition_blindness`
//!   harvests their access before `Schedule::initialize` moves it
//!   out of public view — the
//!   [`super::static_scan::capture_static_scan`] precedent).
//!   Otherwise `complete` is `false`, the blocker is stated in a
//!   note, and the row is withheld: fail-open.
//! - Consumer builds MASK component and system names
//!   (`bevy_ecs/debug` off): rows are withheld with a note instead
//!   of listing possibly-wrong names — same doctrine as the census.
//! - Engine machinery never appears: system names are filtered by
//!   the exact driver list plus the `bevy_*`/`bevy::` prefix
//!   (`super::census::is_engine_type`; a game crate named `bevy_*`
//!   is excluded, the documented trade-off), and resource/channel
//!   targets are classified the same way. The census shares these
//!   filters verbatim so the two walks can never disagree about
//!   what counts as game-owned.
//! - The walk is structurally neutral: it reads initialized access
//!   sets and adds nothing to the world (the capture's
//!   force-initialization already happened around it).

use bevy_app::App;
use bevy_ecs::component::ComponentId;
use bevy_ecs::prelude::{Schedule, World};
use bevy_ecs::query::Access;
use bevy_ecs::system::SystemAccess;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use super::census::{
    is_engine_type, ENGINE_DRIVER_NAMES, MASKED_NAME_SENTINEL, MESSAGES_RESOURCE_PREFIX,
};

/// The provenance every suggestion row carries, verbatim: names the
/// derivation (structural schedule read at load time) and its
/// epistemic limits (no play ever grounded these rows).
pub(crate) const SUGGESTION_DERIVATION: &str =
    "derived from a `systems_with_access` schedule walk at load time \
     (static structural read; no play-grounding; machine-proposed, \
     not a finding)";

/// One authoring-time suggestion. Flat by design: `target` is the
/// resource or message-type path the row is about, `writers`/
/// `readers` name the systems (empty when the kind does not list
/// them or names are masked), and `suggestion_text` is the
/// human-directed menu line (empty for pure data rows).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Suggestion {
    /// Row flavor: data annotation (`ResourceAccess`), or a
    /// candidate the author should confirm (`DeadMessageChannel`).
    pub kind: SuggestionKind,
    /// The game-owned resource (plain kind) or message type (inside
    /// the `Messages<...>` brackets) this row is about, as a
    /// type-path string.
    pub target: String,
    /// Systems whose access set writes `target` (game systems
    /// only; engine machinery is filtered), sorted by name.
    pub writers: Vec<String>,
    /// Systems whose access set reads `target` without writing it,
    /// sorted by name. Conditions held by
    /// `collect_condition_blindness` count as readers-of-last-
    /// resort and are NOT listed — they suppress dead-channel rows
    /// instead.
    pub readers: Vec<String>,
    /// The machine-proposed menu line for the author ("candidate
    /// for a monotonicity/bounds rule — confirm and write the
    /// one-liner", "candidate unconsumed message channel — confirm
    /// before wiring an alarm"). Empty when the row is pure data.
    pub suggestion_text: String,
    /// Provenance, verbatim `SUGGESTION_DERIVATION`.
    pub derivation: String,
}

/// What a suggestion row asks the author to look at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionKind {
    /// Writer/reader annotation for one game-owned resource, plus
    /// (when written) the candidate-invariant menu line.
    ResourceAccess,
    /// A game-owned message channel with zero readers — candidate
    /// for the unconsumed-message oracle, as a SUGGESTION first.
    DeadMessageChannel,
}

/// The additive suggestions section: rows plus the honesty frame.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContractSuggestions {
    /// Sorted by (kind, target): deterministic, no timestamps —
    /// two sweeps of the same binary produce byte-identical rows.
    pub rows: Vec<Suggestion>,
    /// Limitations as they affect the suggestions (masked names,
    /// blindness blockers, scope statement).
    pub notes: Vec<String>,
    /// Whether the walk can vouch it saw every system that could
    /// touch a target (all schedules initialized, no exclusive /
    /// read-all systems, names readable). `false` means the
    /// POSITIVE rows may under-report (annotations stay valid for
    /// what was seen) and no negative (dead-channel) row was
    /// emitted — fail-open, never guess.
    ///
    /// Deliberately WEAKER than the static scan's `complete`: the
    /// scan's flag refuses input pruning (an automated gate), so
    /// any blindness withholds; this flag grades a human-confirmed
    /// DRAFT, so the pre-update runtime-growth caveat of a takeover
    /// capture rides in `notes` instead of flipping `complete` (see
    /// the asymmetry rationale in
    /// `static_scan::capture_scan_without_parking`). Consumers must
    /// NOT treat `complete: true` as license for automated action —
    /// a dead-channel row is a proposal to confirm, never an
    /// alarm.
    #[serde(default)]
    pub complete: bool,
}

/// Condition-held access harvested at capture time, before
/// `Schedule::initialize` moves conditions out of public view.
/// `touched` collects every `ComponentId` any run condition reads or
/// writes; `unbounded` flags a condition whose access is exclusive
/// or read-all/write-all (it may touch ANYTHING — the whole walk
/// must go incomplete).
#[derive(Clone, Debug, Default)]
pub(crate) struct ConditionBlindness {
    pub(crate) touched: BTreeSet<ComponentId>,
    pub(crate) unbounded: bool,
}

/// Harvest run-condition access for one schedule. MUST run inside
/// the capture loop, before `Schedule::initialize` — see
/// [`super::static_scan::condition_hidden_classes`] for why the
/// timing is load-bearing (building moves conditions into executor
/// storage that is `pub(super)` in bevy_ecs 0.20.1). The FIRST
/// builder wins and everyone after it harvests empty lists: this is
/// why the census in `crate::driver` runs BELOW the capture block,
/// and why NOTHING may call `Schedule::initialize` (or `App::update`)
/// between app boot and the capture.
///
/// Safe to call before OR after its sibling
/// `condition_hidden_classes` (both populate condition access via
/// the non-draining `Graph::initialize`, which initializes only
/// not-yet-initialized conditions — the second call is a no-op),
/// but BOTH must run before the first build.
///
/// Covers conditions attached DIRECTLY to systems AND to system
/// sets: `Graph::initialize` populates both containers' condition
/// access, and a `run_if` on a SET gates every member system just
/// as much as a per-system one — skipping set conditions would let
/// a set-gated channel read masquerade as a dead channel. (The
/// device-class sibling in `static_scan` cannot attribute set
/// conditions to member systems and documents that as a residual
/// hole; here no attribution is needed, so the hole does not
/// exist.)
pub(crate) fn collect_condition_blindness(
    schedule: &mut Schedule,
    world: &mut World,
) -> ConditionBlindness {
    let mut blind = ConditionBlindness::default();
    // Populate condition access without draining (same mechanism as
    // `condition_hidden_classes`; see its doc for the receipts).
    schedule.graph_mut().initialize(world);
    for (_key, system, conds) in schedule.graph().systems.iter() {
        // Driver exemption, mirroring `condition_hidden_classes`:
        // every default App seeds
        // `message_update_system.run_if(message_update_condition)`
        // and counting engine-held access as blindness would
        // blindfold every capture identically.
        if ENGINE_DRIVER_NAMES.contains(&system.name().to_string().as_str()) {
            continue;
        }
        for cond in conds {
            match &cond.access {
                SystemAccess::None => {}
                SystemAccess::Exclusive => blind.unbounded = true,
                SystemAccess::Shared(filtered) => {
                    let access = filtered.combined_access();
                    // ANY inverted read/write set marks the walk
                    // unbounded — not just the fully-inverted
                    // `read_all()`/`write_all()` cases
                    // (`has_read_all()` is `Excluded(empty)` only).
                    // `Excluded(non-empty)` is genuinely
                    // constructible: a condition param of
                    // `Query<EntityRefExcept<B>>` does `read_all()`
                    // then removes B's ids, and such a condition
                    // reads almost every component — folding its
                    // finite ids is meaningless while its infinite
                    // complement is what matters. Withhold (fail-open)
                    // for the whole run.
                    if access.reads().is_unbounded() || access.writes().is_unbounded() {
                        blind.unbounded = true;
                    } else {
                        collect_access_ids(access, &mut blind.touched);
                    }
                }
            }
        }
    }
    for (_key, _set, conds) in schedule.graph().system_sets.iter() {
        // No driver exemption needed on sets: engine sets carry no
        // `run_if` conditions (probed against a default App's
        // schedules — only `message_update_system`, a bare system,
        // is conditioned).
        for cond in conds {
            match &cond.access {
                SystemAccess::None => {}
                SystemAccess::Exclusive => blind.unbounded = true,
                SystemAccess::Shared(filtered) => {
                    let access = filtered.combined_access();
                    // ANY inverted read/write set marks the walk
                    // unbounded — not just the fully-inverted
                    // `read_all()`/`write_all()` cases
                    // (`has_read_all()` is `Excluded(empty)` only).
                    // `Excluded(non-empty)` is genuinely
                    // constructible: a condition param of
                    // `Query<EntityRefExcept<B>>` does `read_all()`
                    // then removes B's ids, and such a condition
                    // reads almost every component — folding its
                    // finite ids is meaningless while its infinite
                    // complement is what matters. Withhold (fail-open)
                    // for the whole run.
                    if access.reads().is_unbounded() || access.writes().is_unbounded() {
                        blind.unbounded = true;
                    } else {
                        collect_access_ids(access, &mut blind.touched);
                    }
                }
            }
        }
    }
    blind
}

/// Fold every finite read/write id of an [`Access`] into `touched`.
///
/// Inverted accesses (`Excluded` sets — "everything except these
/// ids") yield `None` from `as_finite_set()` and contribute
/// nothing here. That is only safe because the CALLER marks the
/// whole walk `unbounded` for any inverted set (see
/// `collect_condition_blindness`): an `Excluded` read means "almost
/// every component", and its call-site handlers never reach this
/// function for such accesses. Finite `Included` sets are the only
/// inputs that arrive here.
fn collect_access_ids(access: &Access, touched: &mut BTreeSet<ComponentId>) {
    if let Some(reads) = access.reads().as_finite_set() {
        touched.extend(reads.iter());
    }
    if let Some(writes) = access.writes().as_finite_set() {
        touched.extend(writes.iter());
    }
}

/// A suggestion target: a game-owned resource (plain or message
/// channel) with its `ComponentId`, ready for the systems walk.
#[derive(Clone, Debug)]
struct Target {
    id: ComponentId,
    /// Display path: the resource type-path, or the message type
    /// path for channels (the name inside the `Messages<...>`
    /// brackets — the thing an author would wire an oracle for).
    display: String,
    /// Channel targets participate in the dead-channel check.
    is_channel: bool,
}

/// Classify the game's resources into suggestion targets. Names come
/// from `ComponentInfo` and are MASKED in consumer builds: a single
/// masked name means classification as a whole is impossible (the
/// caller downgrades to rows-withheld-plus-note, never a possibly
/// wrong classification).
fn suggestion_targets(world: &World) -> Option<Vec<Target>> {
    let mut targets = Vec::new();
    for (id, _entity) in world.resource_entities().iter() {
        let Some(info) = world.components().get_info(id) else {
            continue;
        };
        let name: &str = &info.name();
        if name == MASKED_NAME_SENTINEL {
            return None;
        }
        // Channels: the ENGINE filter applies to the type INSIDE
        // the brackets (every `Messages<T>` path starts with
        // `bevy_ecs::` regardless of who owns the channel — the
        // census's verified spelling and classification order).
        if let Some(inner) = name.strip_prefix(MESSAGES_RESOURCE_PREFIX) {
            let inner = inner.strip_suffix('>').unwrap_or(inner);
            if !is_engine_type(inner) {
                targets.push(Target {
                    id,
                    display: inner.to_string(),
                    is_channel: true,
                });
            }
            continue;
        }
        if !is_engine_type(name) {
            targets.push(Target {
                id,
                display: name.to_string(),
                is_channel: false,
            });
        }
    }
    targets.sort_by(|a, b| a.display.cmp(&b.display));
    Some(targets)
}

/// Per-target writer/reader lists keyed by `ComponentId`.
type AccessLists = BTreeMap<ComponentId, (Vec<String>, Vec<String>)>;

/// The result of [`walk_systems`]: per-target lists plus the walk's
/// completeness verdict and blindness notes.
struct WalkedSystems {
    lists: AccessLists,
    complete: bool,
    notes: Vec<String>,
}

/// Walk every initialized schedule and record, per target id, which
/// game systems write it and which read it. Also decides blindness:
/// an uninitialized schedule, an exclusive system, or a read-all /
/// write-all system makes per-target lists unable to vouch for
/// "everything that touches it", and masked system names make the
/// lists unnameable — each degrades `complete` with a note.
fn walk_systems(app: &App, targets: &[Target]) -> WalkedSystems {
    let world = app.world();
    let mut lists: AccessLists = BTreeMap::new();
    let mut complete = true;
    let mut notes = Vec::new();
    let Some(schedules) = world.get_resource::<bevy_ecs::prelude::Schedules>() else {
        notes.push("no Schedules resource at suggestion-capture time".to_string());
        return WalkedSystems {
            lists,
            complete: false,
            notes,
        };
    };
    // Scope note: this walk (and the census) reads the MAIN world's
    // `Schedules` resource only. Schedules registered inside a
    // SubApp's own world (e.g. a render sub-app) are invisible
    // here. That is fail-open today — suggestions never gate
    // anything — but a future consumer must not read "complete"
    // as "every schedule in the whole App, sub-apps included".
    for (_label, schedule) in schedules.iter() {
        let Ok(systems) = schedule.systems_with_access() else {
            complete = false;
            notes.push(
                "at least one schedule was not initialized at suggestion-capture \
                 time: its systems are invisible to the writer/reader walk"
                    .to_string(),
            );
            continue;
        };
        for (_key, swa) in systems {
            let name: &str = &swa.system().name();
            if name == MASKED_NAME_SENTINEL {
                // Masked names abort the walk: rows are withheld
                // with a note (the caller's `complete = false`),
                // never drafted against possibly-wrong names.
                notes.push(
                    "system names are masked in this build (bevy_ecs `debug` \
                     feature off): no writer/reader lists were drafted"
                        .to_string(),
                );
                return WalkedSystems {
                    lists: BTreeMap::new(),
                    complete: false,
                    notes,
                };
            }
            if ENGINE_DRIVER_NAMES.contains(&name) || is_engine_type(name) {
                continue;
            }
            match swa.access() {
                SystemAccess::None => {}
                SystemAccess::Exclusive => {
                    complete = false;
                    notes.push(
                        "an exclusive (&mut World) system exists: it may touch \
                         any resource, so writer/reader lists may under-report"
                            .to_string(),
                    );
                }
                SystemAccess::Shared(filtered) => {
                    let access = filtered.combined_access();
                    if access.has_read_all() || access.has_write_all() {
                        complete = false;
                        notes.push(
                            "a read-all/write-all system exists (e.g. \
                             Query<EntityRef>): writer/reader lists may \
                             under-report"
                                .to_string(),
                        );
                        continue;
                    }
                    // Unlike the CONDITION harvests above (which widen
                    // any `Excluded(non-empty)` set to `unbounded`),
                    // system bodies need no such treatment:
                    // `Access::has_read`/`has_write` handle
                    // `Excluded(non-empty)` exactly — a system with
                    // e.g. `Query<EntityRefExcept<B>>` is classified
                    // as reader/writer of precisely the ids it can
                    // actually access, never blindly nor
                    // fail-open-blindly. The `has_read_all()` check
                    // above only catches `Excluded(empty)` (a true
                    // read-everything system); non-empty exclusion
                    // sets stay precise here.
                    for target in targets {
                        let entry = lists.entry(target.id).or_default();
                        if access.has_write(target.id) {
                            // Writes imply reads (bevy_ecs receipt), so a
                            // writer never doubles as a reader row.
                            if !entry.0.contains(&name.to_string()) {
                                entry.0.push(name.to_string());
                            }
                        } else if access.has_read(target.id) && !entry.1.contains(&name.to_string())
                        {
                            entry.1.push(name.to_string());
                        }
                    }
                }
            }
        }
    }
    WalkedSystems {
        lists,
        complete,
        notes,
    }
}

/// Build the suggestions section for a booted game App. Pure
/// structural read; must run AFTER the capture's schedule
/// force-initialization so `systems_with_access` can see every
/// system.
///
/// `condition_blindness` must be the merge of
/// `collect_condition_blindness` over every schedule in the same
/// capture (it can only be harvested pre-build); condition-touched
/// targets lose dead-channel eligibility — a condition reading a
/// channel is a consumer, and it is invisible to the post-pass.
pub(crate) fn collect_contract_suggestions(
    app: &App,
    condition_blindness: &ConditionBlindness,
) -> ContractSuggestions {
    let mut out = ContractSuggestions::default();
    // ONE completeness verdict, decided once at the end from every
    // blocker seen along the way (never partially assigned then
    // overwritten).
    let mut complete = !condition_blindness.unbounded;
    if condition_blindness.unbounded {
        out.notes.push(
            "a run condition with exclusive or read-all access exists: it may \
             consume any channel, so no dead-channel candidates were drafted"
                .to_string(),
        );
    }

    let Some(targets) = suggestion_targets(app.world()) else {
        out.notes.push(
            "resource names are masked in this build (bevy_ecs `debug` \
             feature off): no suggestion targets could be classified"
                .to_string(),
        );
        out.complete = false;
        return out;
    };

    let walked = walk_systems(app, &targets);
    complete &= walked.complete;
    out.notes.extend(walked.notes);
    let lists = walked.lists;

    for target in &targets {
        let (writers, readers) = lists.get(&target.id).cloned().unwrap_or_default();
        if target.is_channel {
            // Dead-channel candidate: zero readers across every
            // access set the walk saw. Withheld unless the walk can
            // vouch it saw everything (blind blockers already
            // pushed their notes) and no run condition touches the
            // channel — a condition-held read is a consumer the
            // post-pass cannot see.
            if condition_blindness.touched.contains(&target.id) {
                out.notes.push(format!(
                    "message channel `{}` is read by a run condition: it is \
                     not dead",
                    target.display
                ));
                continue;
            }
            if complete && readers.is_empty() {
                // Compute the text BEFORE the struct literal moves
                // `writers` into the row (the plain-resource arm's
                // pattern: bound counts first, then build).
                let writer_count = writers.len();
                let suggestion_text = format!(
                    "`{}` has {writer_count} writer(s) and no readers across \
                     the schedule graph — candidate unconsumed message \
                     channel; confirm before wiring an alarm",
                    target.display
                );
                out.rows.push(Suggestion {
                    kind: SuggestionKind::DeadMessageChannel,
                    target: target.display.clone(),
                    writers,
                    readers,
                    suggestion_text,
                    derivation: SUGGESTION_DERIVATION.to_string(),
                });
            }
            continue;
        }
        // Plain resource: annotation row plus the invariant menu
        // line when it is written. The menu is finding-1-adjacent
        // (§6.1): a written resource is where monotonicity/bounds
        // rules usually live — propose, never claim.
        let writer_count = writers.len();
        let suggestion_text = if writer_count > 0 {
            format!(
                "`{}` is written by {writer_count} system(s): candidate for a \
                 monotonicity/bounds rule — confirm and write the one-liner",
                target.display
            )
        } else {
            String::new()
        };
        out.rows.push(Suggestion {
            kind: SuggestionKind::ResourceAccess,
            target: target.display.clone(),
            writers,
            readers,
            suggestion_text,
            derivation: SUGGESTION_DERIVATION.to_string(),
        });
    }

    // Deterministic order: kind rank then target path.
    out.rows
        .sort_by(|a, b| (a.kind, a.target.as_str()).cmp(&(b.kind, b.target.as_str())));
    out.complete = complete;
    out
}
