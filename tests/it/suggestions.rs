//! Authoring-time contract suggestions: the work-item-2 test suite.
//!
//! The rows this suite pins are DRAFT data by contract (see the
//! module docs in `src/discovery/suggestions.rs`): every assertion
//! below exists to prove the section stays a purely additive,
//! reported annotation — writer/reader lists, the candidate
//! invariant menu, and dead-channel candidates — and never feeds
//! grading, findings, entries, or any gate.
//!
//! Two placement notes:
//!
//! - The suite lives OUTSIDE the library crate because the engine
//!   filter is a type-path prefix (`bevy_*`): a lib unit test's
//!   systems would be filtered as engine machinery and the rows
//!   would be structurally empty. Here the test binary crate is
//!   `suggestions`, safely outside the filter, matching the census
//!   suite's placement doctrine.
//! - All capture paths go through the PUBLIC `capture_static_scan`
//!   (it force-initializes schedules exactly like production), so
//!   these tests also pin the transport wiring: the section rides
//!   the scan sidecar's serde-defaulted `suggestions` field.
//!
//! `takeover` gates the `draft`/`flow` modules this suite touches
//! for the DraftOutcome mirror; the whole file follows that gate so
//! the `in-process`-only feature combination skips it cleanly.

#![cfg(feature = "takeover")]

use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_playtest::discovery::static_scan::capture_static_scan;
use bevy_playtest::discovery::suggestions::{ContractSuggestions, SuggestionKind};

/// A channel nobody ever reads: the dead-channel candidate shape.
#[derive(Message)]
struct UnconsumedMessage;

/// A channel one system reads: must NOT be flagged dead.
#[derive(Message)]
struct ConsumedMessage;

/// A resource one system writes: earns the invariant-menu line.
#[derive(Resource, Default)]
struct WrittenState {
    x: f32,
}

/// A resource one system only reads: annotation row, no menu line.
#[derive(Resource, Default)]
struct ReadOnlyState {
    y: f32,
}

fn write_state(mut state: ResMut<WrittenState>) {
    state.x += 1.0;
}

fn read_state(state: Res<ReadOnlyState>) {
    let _ = state.y;
}

fn write_unconsumed(mut msgs: MessageWriter<UnconsumedMessage>) {
    msgs.write(UnconsumedMessage);
}

fn read_consumed(mut msgs: MessageReader<ConsumedMessage>) {
    for _m in msgs.read() {}
}

/// A run condition READING the unconsumed channel: proves the
/// condition-blindness harvest treats conditions as consumers.
fn channel_has_messages(msgs: Res<Messages<UnconsumedMessage>>) -> bool {
    !msgs.is_empty()
}

/// The game-shaped world: no plugins (bare `App::new()` — engine
/// drivers are name-filtered), four game systems across the two
/// interesting resources and channels.
fn game_app() -> App {
    let mut app = App::new();
    app.add_message::<UnconsumedMessage>()
        .add_message::<ConsumedMessage>()
        .init_resource::<WrittenState>()
        .init_resource::<ReadOnlyState>()
        .add_systems(
            Update,
            (write_state, read_state, write_unconsumed, read_consumed),
        );
    app
}

/// The display name a suggestion row will use for resource type `T`,
/// derived from the live component registry the same way the walk
/// does (no type-name spelling assumptions in the assertions).
fn resource_display<T: Resource>(world: &World) -> String {
    let id = world
        .components()
        .get_valid_id(std::any::TypeId::of::<T>())
        .expect("resource registered");
    world
        .components()
        .get_info(id)
        .expect("resource info")
        .name()
        .to_string()
}

/// The display name a row will use for message type `T` — the name
/// INSIDE the `Messages<...>` brackets, what an author would wire an
/// oracle for. The prefix is the engine spelling verified against
/// bevy_ecs 0.20.1 (hardcoded here because the constant is
/// `pub(crate)`; drift would fail the dead-channel assertions).
fn channel_display<T: Message>(world: &World) -> String {
    let id = world
        .components()
        .get_valid_id(std::any::TypeId::of::<Messages<T>>())
        .expect("channel registered");
    let name = world
        .components()
        .get_info(id)
        .expect("channel info")
        .name();
    let inner = name
        .strip_prefix("bevy_ecs::message::messages::Messages<")
        .expect("engine spelling of Messages<T>");
    inner.strip_suffix('>').unwrap_or(inner).to_string()
}

/// Suggestions section of a fresh capture on `app`.
fn captured_suggestions(app: &mut App) -> ContractSuggestions {
    let scan = capture_static_scan(app);
    scan.suggestions
        .expect("the capture always collects the section")
}

#[test]
fn classifies_writers_readers_and_dead_channels() {
    let mut app = game_app();
    let out = captured_suggestions(&mut app);

    assert!(
        out.complete,
        "bare app, all schedules force-initialized: no blindness expected: {:?}",
        out.notes
    );

    let dead = out
        .rows
        .iter()
        .find(|r| r.kind == SuggestionKind::DeadMessageChannel)
        .expect("written-but-unread channel earns a dead-channel row");
    assert_eq!(
        dead.target,
        channel_display::<UnconsumedMessage>(app.world())
    );
    assert_eq!(dead.readers, Vec::<String>::new());
    assert_eq!(dead.writers.len(), 1, "the writer system is listed");
    assert!(
        dead.suggestion_text.contains("no readers"),
        "the candidate states the claim a human confirms: {}",
        dead.suggestion_text
    );

    // The consumed channel must NOT be flagged dead: correspondence,
    // the anti-hardcoded-control discipline.
    assert!(
        !out.rows.iter().any(|r| {
            r.kind == SuggestionKind::DeadMessageChannel
                && r.target == channel_display::<ConsumedMessage>(app.world())
        }),
        "a channel with a reader is not dead"
    );

    let written_row = out
        .rows
        .iter()
        .find(|r| r.target == resource_display::<WrittenState>(app.world()))
        .expect("written resource earns an annotation row");
    assert_eq!(written_row.writers.len(), 1);
    assert!(written_row.readers.is_empty());
    assert!(
        written_row.suggestion_text.contains("candidate for a"),
        "written resources carry the invariant-menu line: {}",
        written_row.suggestion_text
    );

    let readonly_row = out
        .rows
        .iter()
        .find(|r| r.target == resource_display::<ReadOnlyState>(app.world()))
        .expect("read-only resource earns an annotation row");
    assert!(readonly_row.writers.is_empty());
    assert_eq!(readonly_row.readers.len(), 1);
    assert!(
        readonly_row.suggestion_text.is_empty(),
        "unwritten resources get no menu line"
    );
}

#[test]
fn every_row_carries_provenance() {
    let mut app = game_app();
    let out = captured_suggestions(&mut app);
    assert!(!out.rows.is_empty(), "the game world has targets");
    for row in &out.rows {
        assert!(
            row.derivation.contains("systems_with_access")
                && row.derivation.contains("machine-proposed")
                && row.derivation.contains("not a finding"),
            "provenance must name the derivation and the epistemic status: {}",
            row.derivation
        );
    }
}

#[test]
fn engine_machinery_never_appears() {
    let mut app = game_app();
    let out = captured_suggestions(&mut app);
    for row in &out.rows {
        assert!(
            !row.target.starts_with("bevy_") && !row.target.starts_with("bevy::"),
            "engine machinery leaked into suggestion targets: {}",
            row.target
        );
        for name in row.writers.iter().chain(row.readers.iter()) {
            assert!(
                !name.starts_with("bevy_") && !name.starts_with("bevy::"),
                "engine system leaked into a list: {}",
                name
            );
        }
    }
}

#[test]
fn rows_are_deterministic_and_sorted() {
    let mut a = game_app();
    let mut b = game_app();
    let sa = captured_suggestions(&mut a);
    let sb = captured_suggestions(&mut b);
    assert_eq!(
        sa, sb,
        "two captures of identical worlds are byte-stable (no timestamps)"
    );
    let keys: Vec<(SuggestionKind, &str)> = sa
        .rows
        .iter()
        .map(|r| (r.kind, r.target.as_str()))
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted, "rows are (kind, target)-sorted");
}

/// The suggestions collection must not perturb the scan's own
/// contract: reader counts come out identical to a
/// suggestions-free world of the same shape. (The scan's own
/// `complete=false` is EXPECTED here: the public capture always runs
/// before the world's first update, so the pre-update blindness
/// note is the honest production verdict — this test instead guards
/// against pollution of the scan fields.)
#[test]
fn capture_leaves_scan_fields_untouched() {
    let mut app = game_app();
    let scan = capture_static_scan(&mut app);
    // No input-signal types are registered in this bare world, so
    // the reader table is empty; a non-empty table here would mean
    // the suggestions walk polluted the scan.
    assert_eq!(scan.readers, Vec::new());
    assert!(
        scan.notes
            .iter()
            .any(|n| n.contains("before the world's first update")),
        "the scan's own pre-update caveat is present: {:?}",
        scan.notes
    );
    assert!(scan.suggestions.is_some());
}

/// An exclusive system blinds the writer/reader walk: annotations
/// for what WAS seen stay, but no negative (dead-channel) row may
/// be drafted.
#[test]
fn exclusive_system_withholds_dead_channel_rows() {
    let mut app = App::new();
    app.add_message::<UnconsumedMessage>()
        .add_systems(Update, write_unconsumed)
        .add_systems(Update, |world: &mut World| {
            // Exclusive (&mut World): may touch any resource.
            let _ = world.resource::<Schedules>();
        });
    let out = captured_suggestions(&mut app);
    assert!(
        !out.complete,
        "an exclusive system must withhold negatives: {:?}",
        out.notes
    );
    assert!(
        !out.rows
            .iter()
            .any(|r| r.kind == SuggestionKind::DeadMessageChannel),
        "no dead-channel rows while the walk is incomplete"
    );
    assert!(out.notes.iter().any(|n| n.contains("exclusive")));
}

/// A run condition reading the channel is a consumer the
/// post-capture walk cannot see: the harvest must demote it from
/// dead-candidate to consumed (with a note), exactly the
/// run-condition blind-spot discipline of the static scan.
#[test]
fn condition_read_counts_as_a_consumer() {
    let mut app = App::new();
    app.add_message::<UnconsumedMessage>()
        .add_message::<ConsumedMessage>()
        .add_systems(Update, (write_unconsumed, read_consumed))
        .add_systems(Update, read_state.run_if(channel_has_messages));
    let out = captured_suggestions(&mut app);
    let target = channel_display::<UnconsumedMessage>(app.world());
    assert!(
        !out.rows
            .iter()
            .any(|r| r.kind == SuggestionKind::DeadMessageChannel && r.target == target),
        "a condition-held read means the channel is consumed"
    );
    assert!(
        out.notes.iter().any(|n| n.contains("run condition")),
        "the demotion is explained, not silent: {:?}",
        out.notes
    );
}

/// The same discipline must hold for conditions attached to a
/// SYSTEM SET: a `run_if` on a set gates every member system just
/// as much as a per-system one, so a set-condition-held channel
/// read is equally a consumer. (Regression guard for the harvest
/// gap where only per-system conditions were consulted — a
/// set-gated read would have let a live channel be flagged dead.)
#[test]
fn system_set_condition_read_counts_as_a_consumer() {
    #[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone)]
    struct GatedSet;

    let mut app = App::new();
    app.add_message::<UnconsumedMessage>()
        .add_message::<ConsumedMessage>()
        .add_systems(Update, (write_unconsumed, read_consumed).in_set(GatedSet))
        .configure_sets(Update, GatedSet.run_if(channel_has_messages));
    let out = captured_suggestions(&mut app);
    let target = channel_display::<UnconsumedMessage>(app.world());
    assert!(
        !out.rows
            .iter()
            .any(|r| r.kind == SuggestionKind::DeadMessageChannel && r.target == target),
        "a set-condition-held read means the channel is consumed"
    );
    assert!(
        out.notes.iter().any(|n| n.contains("run condition")),
        "the demotion is explained, not silent: {:?}",
        out.notes
    );
}

/// Old sidecars and surfaces (pre-suggestions artifacts) deserialize
/// with the section ABSENT — fail-open, never misread as "the walk
/// found nothing".
#[test]
fn old_artifacts_deserialize_without_the_section() {
    let json = serde_json::json!({
        "readers": [["keyboard", 2]],
        "notes": [],
        "complete": true
    });
    let scan: bevy_playtest::discovery::static_scan::StaticScanResult =
        serde_json::from_value(json).expect("old sidecar shape parses");
    assert!(scan.suggestions.is_none());

    let surface: bevy_playtest::discovery::surface::DiscoveredSurface =
        serde_json::from_value(serde_json::json!({
            "keys": [],
            "mouse_buttons": [],
            "mouse_motion": null,
            "wheel": null,
            "gamepad_buttons": [],
            "gamepad_axes": [],
            "probed_at": [],
            "readers": [],
            "key_confidence": [],
            "confidence": "exact",
            "pruned_devices": [],
            "notes": []
        }))
        .expect("old surface shape parses");
    assert!(surface.suggestions.is_none());

    let draft: bevy_playtest::discovery::draft::DraftOutcome =
        serde_json::from_value(serde_json::json!({
            "draft": true,
            "schema_version": 0,
            "entries": [],
            "findings": [],
            "coverage": {
                "candidates_probed": 0,
                "verified": 0,
                "inert": 0,
                "missed_by_design": []
            },
            "surface_confidence": "statistical",
            "from_cache": false,
            "probed_at": [],
            "readers": [],
            "pruned_devices": [],
            "notes": []
        }))
        .expect("old draft shape parses");
    assert!(draft.suggestions.is_none());
}

/// The suggestion section is a REPORT, never a commitment: a
/// surface carrying suggestions drafts entries/findings IDENTICAL to
/// the same surface without them, and the section mirrors verbatim
/// into the draft artifact.
#[test]
fn suggestions_never_become_findings_or_entries() {
    use bevy_playtest::discovery::draft::DraftOutcome;
    use bevy_playtest::discovery::surface::DiscoveredSurface;

    let bare = DiscoveredSurface::default();
    let loaded = DiscoveredSurface {
        suggestions: Some(ContractSuggestions {
            rows: vec![bevy_playtest::discovery::suggestions::Suggestion {
                kind: SuggestionKind::DeadMessageChannel,
                target: "game::NeverRead".to_string(),
                writers: vec!["game::writer".to_string()],
                readers: vec![],
                suggestion_text: "candidate; confirm before wiring an alarm".to_string(),
                derivation: "derived from a `systems_with_access` schedule walk".to_string(),
            }],
            notes: vec![],
            complete: true,
        }),
        ..Default::default()
    };

    // Identical graded surface data: the section adds NOTHING to
    // entries or findings.
    assert_eq!(bare.to_entries(), loaded.to_entries());
    assert_eq!(bare.findings, loaded.findings);

    // Verbatim mirror into the draft artifact.
    let draft = DraftOutcome::from_surface(&loaded, false);
    assert_eq!(draft.suggestions, loaded.suggestions);
    let bare_draft = DraftOutcome::from_surface(&bare, false);
    assert_eq!(draft.entries, bare_draft.entries);
    assert_eq!(draft.findings, bare_draft.findings);
    assert!(bare_draft.suggestions.is_none());

    // And the section round-trips through the artifact JSON.
    let json = serde_json::to_string(&draft).expect("draft serializes");
    let back: DraftOutcome = serde_json::from_str(&json).expect("draft parses back");
    assert_eq!(back.suggestions, loaded.suggestions);
}
