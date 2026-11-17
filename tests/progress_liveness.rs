//! Behavioral battery for the progress-liveness oracle
//! (`stalled_progress` / `progress_surface_empty`): a churning-but-
//! dead world — the frame loop alive, transforms jittering, no game
//! state evolving — is exactly the regime the frozen-world oracle
//! cannot see (its streak never builds) and authored invariants may
//! not exist for. Case study: a headless team-sports simulation whose
//! frame loop stays hot while no state ever progresses.
//!
//! All runs hold `common::run_slot()` (process-global capture state).

#[path = "common/mod.rs"]
mod common;

use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_playtest::driver::run_scenario;
use bevy_playtest::rules::Rule;
use bevy_reflect::Reflect;
use bevy_transform::prelude::Transform;

// ---------------------------------------------------------------------------
// Game state types (reflection is what makes state verifiable)
// ---------------------------------------------------------------------------

/// A game score resource — the canonical progress marker.
#[derive(Resource, Reflect, Default)]
#[reflect(Resource)]
struct Score(u64);

/// Phase resource: a game's coarse-grained state machine.
#[derive(Resource, Reflect, Default)]
#[reflect(Resource)]
struct Phase(u32);

/// Third-party-shaped cache resource: changes every frame, lives on a
/// "vendor" path, and must not count as progress in counts-only auto
/// (plugin-noise test).
mod third_party_cache {
    use super::*;

    #[derive(Resource, Reflect, Default)]
    #[reflect(Resource)]
    pub struct Timer(pub u64);
}

/// A game's own module namespace: several reflected resources under
/// one path prefix, the shape `include_paths: ["…::"]` scoping is
/// designed for (one entry, whole namespace).
mod game_state {
    use super::*;

    pub mod rules {
        use super::*;

        #[derive(Resource, Reflect, Default)]
        #[reflect(Resource)]
        pub struct Possession(pub u32);
    }

    pub mod clock {
        use super::*;

        #[derive(Resource, Reflect, Default)]
        #[reflect(Resource)]
        pub struct MatchClock(pub u64);
    }
}

/// Two same-short-name resources in sibling modules: an exact
/// `include_paths` entry naming the short name must stay ambiguous.
mod kettle_a {
    use super::*;

    #[derive(Resource, Reflect, Default)]
    #[reflect(Resource)]
    pub struct Teapot(pub u32);
}

mod kettle_b {
    use super::*;

    #[derive(Resource, Reflect, Default)]
    #[reflect(Resource)]
    pub struct Teapot(pub u32);
}

/// Marker component on the moving entity (component-type counts are
/// part of the auto surface).
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct Player;

// ---------------------------------------------------------------------------
// Movement systems: the shapes under test
// ---------------------------------------------------------------------------

/// Frame counter driving the oscillator. Not reflected: it is a phase
/// source, not surface state.
#[derive(Resource, Default)]
struct FrameCount(u64);

/// Oscillator: transform values genuinely change EVERY frame and never
/// converge (a driven phase cannot reach a fixed point, unlike a
/// sin-recurrence). Churn is real — frozen_world stays silent, phantom
/// samples differ — while NO game state evolves. The pure host for
/// progress-liveness testing.
fn oscillate(mut q: Query<&mut Transform, With<Player>>, mut count: ResMut<FrameCount>) {
    for mut t in q.iter_mut() {
        t.translation.z = (count.0 as f32 * 0.37).sin() * 0.01;
    }
    count.0 += 1;
}

/// Phantom writer (frozen-world hardening fixture): rewrites the SAME
/// value every frame — `Changed<Transform>` fires constantly while the
/// world changes nothing. The classic churning-but-dead signature that
/// defeats streak-based detection.
fn phantom_write(mut q: Query<&mut Transform, With<Player>>) {
    for mut t in q.iter_mut() {
        t.translation.z = 1.0;
    }
}

/// Genuinely-occasional score: increments every 30th frame. At the
/// 0.25s (15-frame) sampling cadence that is a change in about half
/// the observed transitions — well under the 90% ticker bar: real
/// progress evidence.
fn healthy_score(mut score: ResMut<Score>, count: Res<FrameCount>) {
    if count.0.is_multiple_of(30) {
        score.0 += 1;
    }
}

/// Ticker: increments every frame — a change in 100% of sampled
/// transitions. Must be suppressed wherever it is watched; heartbeats
/// are not progress.
fn ticker(mut timer: ResMut<TimerForTicker>) {
    timer.0 += 1;
}

/// Near-ticker: increments every frame but one in 32. Still a change in
/// EVERY observed 15-frame transition (each interval spans at least
/// 14 increments), so it must be suppressed just like a pure ticker:
/// the suppression bar is change density at the SAMPLE grain, not
/// perfect periodicity.
fn near_ticker(mut timer: ResMut<TimerForTicker>, count: Res<FrameCount>) {
    if !count.0.is_multiple_of(32) {
        timer.0 += 1;
    }
}

/// The reflected resource the ticker tests watch.
#[derive(Resource, Reflect, Default)]
#[reflect(Resource)]
struct TimerForTicker(u64);

// ---------------------------------------------------------------------------
// Scenario builders
// ---------------------------------------------------------------------------

fn scenario(progress_json: &str) -> bevy_playtest::scenario::Scenario {
    let json = format!(
        r#"{{"name":"progtest","bot":{{"type":"chaos","seed":42}},"duration_s":12.0,"tps":60,
            "liveness":{{"timeout_s":3.0,"progress":{progress_json}}}}}"#
    );
    serde_json::from_str(&json).unwrap()
}

/// Auto-mode scenario, optionally with include_paths.
fn auto_scenario(extra: &str) -> bevy_playtest::scenario::Scenario {
    scenario(&format!(r#"{{"mode":"auto"{extra}}}"#))
}

fn base_app() -> App {
    let mut app = App::new();
    app.register_type::<Player>();
    app.register_type::<Transform>();
    app.insert_resource(FrameCount::default());
    app.world_mut()
        .spawn((Player, Transform::from_xyz(0.0, 1.0, 0.0)));
    app
}

fn assert_red(report: &bevy_playtest::state::PlaytestReport, want: &str) {
    let rules: Vec<&str> = report.violations.iter().map(|v| v.rule.as_str()).collect();
    assert!(
        rules.contains(&want),
        "expected '{want}' among {rules:?} (status {:?})",
        report.status
    );
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
}

fn assert_green(report: &bevy_playtest::state::PlaytestReport) {
    let rules: Vec<&str> = report.violations.iter().map(|v| v.rule.as_str()).collect();
    assert_eq!(
        report.status,
        bevy_playtest::enums::PlaytestStatus::Pass,
        "unexpected violations: {rules:?}"
    );
}

// ---------------------------------------------------------------------------
// Red tests: the oracle fires
// ---------------------------------------------------------------------------

/// THE case study: oscillating transforms, no game state evolves. The
/// frozen-world oracle stays silent (every frame changes something);
/// only the progress oracle sees the death.
#[test]
fn oscillating_dead_world_is_stalled() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.add_systems(Update, oscillate);
    let report = run_scenario(&mut app, &auto_scenario("")).unwrap();
    // Component-type counts (Player) make the surface non-empty, so
    // the verdict must be stalled_progress, not progress_surface_empty.
    assert_red(&report, "stalled_progress");
    // …and the run must also record progress samples.
    assert!(!report.progress_samples.is_empty());
}

/// Oscillation + a frame-counter ticker: the ticker is suppressed (it
/// changes in 100% of sampled transitions) and the run is STILL red —
/// heartbeats are not progress. The stalled detail must NAME the
/// suppressed ticker so the author knows what was discounted.
#[test]
fn ticker_does_not_fake_progress() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.register_type::<TimerForTicker>();
    app.insert_resource(TimerForTicker(0));
    app.add_systems(Update, (oscillate, ticker));
    // The ticker is included deliberately: markers name it.
    let scen = scenario(r#"{"mode":"markers","markers":["TimerForTicker"]}"#);
    let report = run_scenario(&mut app, &scen).unwrap();
    assert_red(&report, "stalled_progress");
    let entry = report
        .violations
        .iter()
        .find(|v| v.rule == "stalled_progress")
        .expect("stalled entry");
    assert!(
        entry.detail.contains("TimerForTicker"),
        "detail must name the suppressed ticker: {}",
        entry.detail
    );
}

/// Near-ticker regression pin: a resource updating 31-of-32 frames
/// still changes in EVERY observed 15-frame sample transition, so it
/// is suppressed exactly like a pure ticker — the run stays red.
/// This pins the suppression BAR semantics (change density at the
/// sample grain), guarding against a future regression that switches
/// to periodicity detection and silently lets near-tickers through.
#[test]
fn near_ticker_is_suppressed_like_a_ticker() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.register_type::<TimerForTicker>();
    app.insert_resource(TimerForTicker(0));
    app.add_systems(Update, (oscillate, near_ticker));
    let scen = scenario(r#"{"mode":"markers","markers":["TimerForTicker"]}"#);
    let report = run_scenario(&mut app, &scen).unwrap();
    assert_red(&report, "stalled_progress");
}

/// A third-party-path resource updating occasionally (plugin noise):
/// with counts-only auto it must not fake progress.
#[test]
fn counts_only_auto_ignores_foreign_resource_noise() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.register_type::<third_party_cache::Timer>();
    app.insert_resource(third_party_cache::Timer(0));
    // Occasional (not ticker) updates on a foreign path.
    app.add_systems(Update, |mut timer: ResMut<third_party_cache::Timer>| {
        timer.0 += 1; // every frame: would be a ticker anyway; also foreign-path
    });
    app.add_systems(Update, oscillate);
    let report = run_scenario(&mut app, &auto_scenario("")).unwrap();
    assert_red(&report, "stalled_progress");
}

// ---------------------------------------------------------------------------
// Green tests: genuine progress and legitimate quiet
// ---------------------------------------------------------------------------

/// Oscillating world + a genuinely-occasional score (every 30th
/// frame): healthy progress. The score is well under the 90% ticker
/// bar at the sampling grain, and the transforms give real churn
/// without converging (so frozen_world and the phantom detector stay
/// silent too).
#[test]
fn progressing_world_passes() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.register_type::<Score>();
    app.insert_resource(Score(0));
    app.add_systems(Update, (oscillate, healthy_score));
    let report = run_scenario(&mut app, &auto_scenario(r#","include_paths":["Score"]"#)).unwrap();
    assert_green(&report);
}

/// Path-scoping: include_paths grants value-level observation without
/// markers mode. Phase changes roughly every 30 frames (~50% change
/// density at the 15-frame sampling grain — under the ticker bar),
/// and the oscillating transforms give frozen_world nothing to report.
#[test]
fn include_paths_tracks_resource_values() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.register_type::<Phase>();
    app.insert_resource(Phase(0));
    app.add_systems(
        Update,
        (
            oscillate,
            |mut phase: ResMut<Phase>, count: Res<FrameCount>| {
                if count.0.is_multiple_of(30) {
                    phase.0 += 1;
                }
            },
        ),
    );
    let report = run_scenario(&mut app, &auto_scenario(r#","include_paths":["Phase"]"#)).unwrap();
    assert_green(&report);
}

// ---------------------------------------------------------------------------
// include_paths semantics: `::`-terminated entries are path prefixes
// ---------------------------------------------------------------------------

/// Prefix scoping: one `game_state::` entry pulls EVERY reflected
/// resource under that module path (here: rules::Possession AND
/// clock::MatchClock) into the surface. Both change occasionally —
/// genuine progress — so the run is green, proving the prefix matched
/// BOTH resources (a counts-only surface would stall).
#[test]
fn include_paths_prefix_matches_whole_namespace() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.register_type::<game_state::rules::Possession>();
    app.register_type::<game_state::clock::MatchClock>();
    app.insert_resource(game_state::rules::Possession(0));
    app.insert_resource(game_state::clock::MatchClock(0));
    app.add_systems(
        Update,
        (
            oscillate,
            |mut possession: ResMut<game_state::rules::Possession>,
             mut clock: ResMut<game_state::clock::MatchClock>,
             count: Res<FrameCount>| {
                if count.0.is_multiple_of(30) {
                    possession.0 += 1;
                    clock.0 += 2;
                }
            },
        ),
    );
    // Registered type paths are crate-prefixed; derive the prefix
    // rather than hard-coding the test binary's crate name.
    let prefix = format!("{}::game_state::", module_path!());
    let scen = auto_scenario(&format!(r#","include_paths":["{prefix}"]"#));
    let report = run_scenario(&mut app, &scen).unwrap();
    assert_green(&report);
}

/// Prefix that changes nothing must still mean STALLED (not vacuously
/// green): the same `game_state::` prefix, but nothing under it ever
/// changes. Proves the prefix actually attached the resources to the
/// surface (a prefix silently matching zero would stall identically —
/// the distinguishing evidence is the zero-match test below).
#[test]
fn prefix_scoped_dead_state_is_stalled() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.register_type::<game_state::rules::Possession>();
    app.register_type::<game_state::clock::MatchClock>();
    app.insert_resource(game_state::rules::Possession(0));
    app.insert_resource(game_state::clock::MatchClock(0));
    app.add_systems(Update, oscillate);
    let prefix = format!("{}::game_state::", module_path!());
    let scen = auto_scenario(&format!(r#","include_paths":["{prefix}"]"#));
    let report = run_scenario(&mut app, &scen).unwrap();
    assert_red(&report, "stalled_progress");
}

/// A prefix matching ZERO reflected resources is an authoring-visible
/// problem, not a silent no-op: loud oracle_error naming the prefix
/// with a path-or-registration hint (NOT the misleading
/// "register the resource type" text that suggests a single-type
/// resolution failure).
#[test]
fn prefix_matching_nothing_reports_oracle_error() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.register_type::<game_state::rules::Possession>();
    app.insert_resource(game_state::rules::Possession(0));
    app.add_systems(Update, oscillate);
    let report = run_scenario(
        &mut app,
        &auto_scenario(r#","include_paths":["nonexistent_module::"]"#),
    )
    .unwrap();
    let entry = report
        .violations
        .iter()
        .find(|v| v.rule == "oracle_error" && v.target == "progress")
        .expect("oracle_error for the zero-match prefix");
    assert!(
        entry.detail.contains("nonexistent_module::"),
        "detail must name the prefix: {}",
        entry.detail
    );
    assert!(
        entry
            .detail
            .contains("no reflected resources under this path prefix"),
        "detail must state the prefix-matched-nothing cause: {}",
        entry.detail
    );
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Fail);
}

/// `exclude` subtracts from prefix-matched channels: watch the whole
/// `game_state::` namespace minus its clock. Only Possession changes
/// (occasionally — genuine progress) while the excluded clock ticks
/// every frame; the run stays green because the excluded ticker never
/// entered the surface.
#[test]
fn exclude_subtracts_from_prefix_matched_channels() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.register_type::<game_state::rules::Possession>();
    app.register_type::<game_state::clock::MatchClock>();
    app.insert_resource(game_state::rules::Possession(0));
    app.insert_resource(game_state::clock::MatchClock(0));
    app.add_systems(
        Update,
        (
            oscillate,
            |mut possession: ResMut<game_state::rules::Possession>,
             mut clock: ResMut<game_state::clock::MatchClock>,
             count: Res<FrameCount>| {
                if count.0.is_multiple_of(30) {
                    possession.0 += 1;
                }
                clock.0 += 1; // every frame: ticker under the prefix
            },
        ),
    );
    // The exclude entry is a path prefix; derive both from the
    // test's module path so the test survives crate renames.
    let game_ns = format!("{}::game_state::", module_path!());
    let clock_ns = format!("{game_ns}clock::");
    let scen = auto_scenario(&format!(
        r#","include_paths":["{game_ns}"],"exclude":["{clock_ns}"]"#
    ));
    let report = run_scenario(&mut app, &scen).unwrap();
    assert_green(&report);
}

/// Exact-name entries keep full resolution: `kettle_a::Teapot` (full
/// path) works despite a same-short-name sibling. The short name alone
/// stays ambiguous (loud) for these entries.
#[test]
fn exact_full_path_entry_resolves_despite_short_name_ambiguity() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.register_type::<kettle_a::Teapot>();
    app.register_type::<kettle_b::Teapot>();
    app.insert_resource(kettle_a::Teapot(0));
    app.add_systems(
        Update,
        (
            oscillate,
            |mut teapot: ResMut<kettle_a::Teapot>, count: Res<FrameCount>| {
                if count.0.is_multiple_of(30) {
                    teapot.0 += 1;
                }
            },
        ),
    );
    let full_path = format!("{}::kettle_a::Teapot", module_path!());
    let scen = auto_scenario(&format!(r#","include_paths":["{full_path}"]"#));
    let report = run_scenario(&mut app, &scen).unwrap();
    assert_green(&report);
}

/// The ambiguity doctrine survives for NON-prefix entries: a bare
/// short name matching two registered resources is a loud
/// oracle_error naming the ambiguity — unchanged by prefix support.
#[test]
fn non_prefix_short_name_stays_loudly_ambiguous() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.register_type::<kettle_a::Teapot>();
    app.register_type::<kettle_b::Teapot>();
    // Both instances must EXIST as resources for the short name to be
    // ambiguous (resolution works on present resources, not the
    // registry).
    app.insert_resource(kettle_a::Teapot(0));
    app.insert_resource(kettle_b::Teapot(0));
    app.add_systems(Update, oscillate);
    let report = run_scenario(&mut app, &auto_scenario(r#","include_paths":["Teapot"]"#)).unwrap();
    let entry = report
        .violations
        .iter()
        .find(|v| v.rule == "oracle_error" && v.target == "progress")
        .expect("oracle_error for the ambiguous short name");
    assert!(
        entry.detail.contains("ambiguous"),
        "detail must state the ambiguity: {}",
        entry.detail
    );
}

/// Phantom writer (M3): transforms rewritten with the same value every
/// frame. The frozen_world streak never builds (Changed<Transform>
/// fires constantly), so only the value-hash interval check can catch
/// it: frozen_world must report with the "phantom writes" detail.
#[test]
fn phantom_write_world_is_frozen_with_phantom_detail() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.add_systems(Update, phantom_write);
    let report = run_scenario(&mut app, &auto_scenario("")).unwrap();
    let entry = report
        .violations
        .iter()
        .find(|v| v.rule == "frozen_world")
        .expect("frozen_world must fire on phantom writes");
    assert!(
        entry.detail.contains("phantom writes"),
        "detail must carry the phantom-writes signature: {}",
        entry.detail
    );
    assert_red(&report, "frozen_world");
}

/// Quiet, turn-based-style world: entity present, nothing changes at
/// all. That is frozen_world's finding (genuine freeze), NOT
/// stalled_progress — the progress oracle must stay silent about
/// staleness (its churn precondition is absent) but IS allowed to say
/// nothing overall only if the surface is non-empty… in this test the
/// Player component count channel exists, surface is non-empty, and
/// no stalled verdict may fire because churn is absent.
#[test]
fn quiet_world_is_frozen_not_stalled() {
    let _slot = common::run_slot();
    let mut app = base_app(); // No movement system: nothing ever changes.
    let report = run_scenario(&mut app, &auto_scenario("")).unwrap();
    let rules: Vec<&str> = report.violations.iter().map(|v| v.rule.as_str()).collect();
    assert!(
        rules.contains(&"frozen_world"),
        "a truly static world must trip frozen_world: {rules:?}"
    );
    assert!(
        !rules.contains(&"stalled_progress"),
        "the progress oracle must not fire on a non-churning (frozen) world: {rules:?}"
    );
}

// ---------------------------------------------------------------------------
// Surface emptiness
// ---------------------------------------------------------------------------

/// No registered game types, no markers: progress is not verifiable →
/// progress_surface_empty (informational, not gating by default).
#[test]
fn empty_surface_is_informational_by_default() {
    let _slot = common::run_slot();
    // Deliberately NO component registration and no spawn: nothing
    // game-verifiable exists. A moving transform keeps churn real.
    let mut app = App::new();
    app.register_type::<Transform>();
    app.add_systems(Update, |mut q: Query<&mut Transform>| {
        for mut t in q.iter_mut() {
            t.translation.z += 0.05;
        }
    });
    app.world_mut().spawn(Transform::default());
    let report = run_scenario(&mut app, &auto_scenario("")).unwrap();
    let entry = report
        .violations
        .iter()
        .find(|v| v.rule == "progress_surface_empty")
        .expect("progress_surface_empty reported");
    assert_eq!(report.status, bevy_playtest::enums::PlaytestStatus::Pass);
    let _ = entry;
}

/// Same world, require_surface: the finding escalates to gating.
#[test]
fn empty_surface_gates_under_require_surface() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.register_type::<Transform>();
    app.add_systems(Update, |mut q: Query<&mut Transform>| {
        for mut t in q.iter_mut() {
            t.translation.z += 0.05;
        }
    });
    app.world_mut().spawn(Transform::default());
    let report = run_scenario(&mut app, &auto_scenario(r#","require_surface":true"#)).unwrap();
    assert_red(&report, "progress_surface_empty");
}

/// strict_warnings escalates progress_surface_empty too.
#[test]
fn strict_warnings_escalate_empty_surface() {
    let _slot = common::run_slot();
    let mut app = App::new();
    app.register_type::<Transform>();
    app.add_systems(Update, |mut q: Query<&mut Transform>| {
        for mut t in q.iter_mut() {
            t.translation.z += 0.05;
        }
    });
    app.world_mut().spawn(Transform::default());
    let mut scen = auto_scenario("");
    scen.strict_warnings = true;
    let report = run_scenario(&mut app, &scen).unwrap();
    assert_red(&report, "progress_surface_empty");
}

/// Off mode: total silence — no samples, no violations, no escalation.
/// Even the phantom-write detector finding is moot: the progress
/// oracle is absent; only the frozen_world/phantom oracle speaks, and
/// an oscillating world gives it nothing.
#[test]
fn off_mode_is_silent() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.add_systems(Update, oscillate);
    let report = run_scenario(&mut app, &scenario(r#"{"mode":"off"}"#)).unwrap();
    assert_green(&report);
    assert!(report.progress_samples.is_empty());
}

// ---------------------------------------------------------------------------
// Validation rejections (schema level)
// ---------------------------------------------------------------------------

fn expect_reject(progress_json: &str, needle: &str) {
    let scen = scenario(progress_json);
    match bevy_playtest::scenario::validate_scenario(&scen) {
        Err(bevy_playtest::scenario::ScenarioError::Rejected(msg)) => {
            assert!(msg.contains(needle), "message {msg:?} lacks {needle:?}");
        }
        other => panic!("expected Rejected, got {other:?}"),
    }
}

#[test]
fn markers_mode_requires_markers() {
    expect_reject(r#"{"mode":"markers"}"#, "markers");
}

#[test]
fn window_must_be_shorter_than_duration() {
    expect_reject(r#"{"mode":"auto","window_s":12.0}"#, "window_s");
}

#[test]
fn sub_frame_sampling_rejected() {
    expect_reject(r#"{"mode":"auto","sample_s":0.001}"#, "sample_s");
}

#[test]
fn sample_must_be_shorter_than_window() {
    expect_reject(
        r#"{"mode":"auto","window_s":4.0,"sample_s":4.0}"#,
        "window_s",
    );
}

#[test]
fn non_finite_window_rejected() {
    let mut scen = scenario(r#"{"mode":"auto"}"#);
    scen.liveness.progress.window_s = f32::NAN;
    match bevy_playtest::scenario::validate_scenario(&scen) {
        Err(bevy_playtest::scenario::ScenarioError::Rejected(msg)) => {
            assert!(msg.contains("window_s"));
        }
        other => panic!("expected Rejected, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Registry classification
// ---------------------------------------------------------------------------

#[test]
fn progress_rules_are_registered_with_correct_gating() {
    let stalled = Rule::from_name("stalled_progress").expect("registered");
    let empty = Rule::from_name("progress_surface_empty").expect("registered");
    assert!(!stalled.is_informational(), "stalled_progress gates");
    assert!(empty.is_informational(), "surface-empty is informational");
}

/// Determinism: two identical runs produce identical progress sample
/// series (verifies the surface hashing is spawn-order-independent
/// and machine-stable).
#[test]
fn progress_samples_are_deterministic() {
    let _slot = common::run_slot();
    let run = || {
        let mut app = base_app();
        app.register_type::<Score>();
        app.insert_resource(Score(0));
        app.add_systems(Update, (oscillate, healthy_score));
        run_scenario(&mut app, &auto_scenario(r#","include_paths":["Score"]"#)).unwrap()
    };
    let a = run();
    let b = run();
    assert_eq!(a.progress_samples, b.progress_samples);
}

// ---------------------------------------------------------------------------
// Phantom-write detector hardening (full-transform hash; dominant churn)
// ---------------------------------------------------------------------------

/// Rotation-only animation (translation constant): the transform IS
/// genuinely changing every frame. A translation-bits-only value hash
/// would call this world phantom and report a false frozen_world; the
/// full-transform hash (translation AND rotation AND scale) must not.
#[test]
fn rotating_transform_is_genuine_change_not_phantom() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.add_systems(
        Update,
        |mut q: Query<&mut Transform, With<Player>>, mut count: ResMut<FrameCount>| {
            for mut t in q.iter_mut() {
                t.rotation.z = (count.0 as f32 * 0.01).sin() * 0.1;
                t.rotation.w = (count.0 as f32 * 0.01).cos();
            }
            count.0 += 1;
        },
    );
    let report = run_scenario(&mut app, &auto_scenario("")).unwrap();
    let rules: Vec<&str> = report.violations.iter().map(|v| v.rule.as_str()).collect();
    assert!(
        !rules.contains(&"frozen_world"),
        "rotating transforms are genuine change: phantom/frozen must stay \
         silent, got {rules:?}"
    );
    // No game state evolves, though: the progress oracle still fires.
    assert!(rules.contains(&"stalled_progress"), "got {rules:?}");
}

/// Dominant phantom churn: ≥90% of frames flag `Changed<Transform>`
/// with bit-identical values (one frame in 64 skips the rewrite), so
/// the strict every-frame phantom signature never accumulates. The
/// dominant-interval secondary detection must still report
/// frozen_world with the "mostly phantom writes" detail.
#[test]
fn mostly_phantom_writes_report_frozen() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.add_systems(
        Update,
        |mut q: Query<&mut Transform, With<Player>>, mut f: bevy_ecs::prelude::Local<u64>| {
            if !(*f).is_multiple_of(64) {
                for mut t in q.iter_mut() {
                    t.translation.z = 1.0; // same value every write
                }
            }
            *f += 1;
        },
    );
    let report = run_scenario(&mut app, &auto_scenario("")).unwrap();
    let entry = report
        .violations
        .iter()
        .find(|v| v.rule == "frozen_world")
        .expect("frozen_world must fire on mostly-phantom churn");
    assert!(
        entry.detail.contains("mostly phantom writes"),
        "detail must carry the dominant-churn signature: {}",
        entry.detail
    );
}

// ---------------------------------------------------------------------------
// Bookkeeping counts are not progress evidence
// ---------------------------------------------------------------------------

/// Sparse bookkeeping churn: an entity spawns every 60 frames and
/// despawns 30 frames later. Entity/archetype counts then change in
/// ~25% of the 15-frame sample transitions — under the 90% ticker bar,
/// so before the bookkeeping exemption this churn ACQUITTED windows as
/// fake progress. The stalled analysis must skip those channels and
/// the world stays red: spawn cycles are engine bookkeeping, not
/// game-state evolution.
#[derive(Resource, Default)]
struct Ephemeral(Option<Entity>);

#[test]
fn bookkeeping_counts_do_not_fake_progress() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.insert_resource(Ephemeral::default());
    app.add_systems(
        Update,
        (
            oscillate,
            |mut commands: Commands, count: Res<FrameCount>, mut slot: ResMut<Ephemeral>| {
                if count.0.is_multiple_of(60) {
                    slot.0 = Some(commands.spawn_empty().id());
                }
                if count.0 % 60 == 30 {
                    if let Some(entity) = slot.0.take() {
                        commands.entity(entity).despawn();
                    }
                }
            },
        ),
    );
    let report = run_scenario(&mut app, &auto_scenario("")).unwrap();
    assert_red(&report, "stalled_progress");
}

// ---------------------------------------------------------------------------
// Multi-window findings: faithful accumulation and detail
// ---------------------------------------------------------------------------

/// The second marker's reflected resource.
#[derive(Resource, Reflect, Default)]
#[reflect(Resource)]
struct SecondTicker(u64);

/// Two staged tickers: A ticks until frame 240, B from frame 240 on.
/// With 2.0s windows (120 frames) and 0.5s sampling (30 frames), the
/// five complete windows [0,120), [120,240), [240,360), [360,480),
/// [480,600) ALL stall: the first two suppress ticker A, the last
/// three suppress ticker B (the baseline sample of the third window
/// already absorbs A's final value, so no transition ever shows it).
/// The finding must carry count 5, the UNION of tickers across ALL
/// stalled windows (not just the last window's B), and the first/last
/// window frames. The trailing partial window [600, 720) is excluded
/// by completeness.
#[test]
fn multi_window_finding_names_every_stalled_window() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.register_type::<TimerForTicker>();
    app.register_type::<SecondTicker>();
    app.insert_resource(TimerForTicker(0));
    app.insert_resource(SecondTicker(0));
    app.add_systems(
        Update,
        (
            oscillate,
            |mut a: ResMut<TimerForTicker>, mut b: ResMut<SecondTicker>, count: Res<FrameCount>| {
                if count.0 < 240 {
                    a.0 += 1;
                }
                if count.0 >= 240 {
                    b.0 += 1;
                }
            },
        ),
    );
    let scen = scenario(
        r#"{"mode":"markers","markers":["TimerForTicker","SecondTicker"],
            "window_s":2.0,"sample_s":0.5}"#,
    );
    let report = run_scenario(&mut app, &scen).unwrap();
    let entry = report
        .violations
        .iter()
        .find(|v| v.rule == "stalled_progress")
        .expect("stalled entry");
    assert_eq!(
        entry.count, 5,
        "windows at 0,120,240,360,480 all stall: {}",
        entry.detail
    );
    assert_eq!(entry.first_frame, 0);
    assert_eq!(
        entry.last_frame, 480,
        "last complete stalled window: {}",
        entry.detail
    );
    assert!(
        entry.detail.contains("5 complete windows"),
        "detail must count the stalled windows: {}",
        entry.detail
    );
    assert!(
        entry.detail.contains("TimerForTicker") && entry.detail.contains("SecondTicker"),
        "detail must carry the union of tickers across ALL stalled windows: {}",
        entry.detail
    );
}

// ---------------------------------------------------------------------------
// Sampling-grid validation (authored configs)
// ---------------------------------------------------------------------------

/// sample_s off the whole-frame grid: 0.255s at tps 60 is 15.3 frames.
#[test]
fn non_integral_sample_period_rejected() {
    expect_reject(
        r#"{"mode":"auto","window_s":2.0,"sample_s":0.255}"#,
        "whole number of frames",
    );
}

/// window_s not a whole number of sample intervals: 0.3s (18 frames)
/// does not divide a 2.0s window (120 frames). The message must name
/// the authored product and the nearest coherent window distinctly —
/// a reader must not mistake the suggestion for their own config.
#[test]
fn window_not_multiple_of_sample_interval_rejected() {
    expect_reject(
        r#"{"mode":"auto","window_s":2.0,"sample_s":0.3}"#,
        "authored window_s·tps = 120 frames — the nearest coherent window is 126 frames (7 intervals)",
    );
}

/// A coherent authored config (0.25s = 15 frames; 2.0s = 8 intervals)
/// validates clean — the grid holds, not just any authored timing.
#[test]
fn coherent_grid_config_accepted() {
    let scen = scenario(r#"{"mode":"auto","window_s":2.0,"sample_s":0.25}"#);
    assert!(
        bevy_playtest::scenario::validate_scenario(&scen).is_ok(),
        "grid-coherent timings must validate"
    );
}

/// Blank marker/exclude entries are indistinguishable in intent from
/// typos, which fail loudly at resolution — silently dropping them
/// would hide a broken config behind partial coverage.
#[test]
fn blank_progress_entries_rejected() {
    expect_reject(r#"{"mode":"auto","markers":["   "]}"#, "must not be blank");
    expect_reject(r#"{"mode":"auto","exclude":[""]}"#, "must not be blank");
}

/// The DEFAULT config rides along unvalidated at ANY tps — including
/// tps 66, where its 0.25s sample is 16.5 frames (incoherent on
/// paper, silently re-quantized coherently by the recorder). The
/// exemption is exact structural equality with the default, so an
/// untouched `progress:{}` (or omitted) must validate at odd tps.
#[test]
fn default_config_rides_along_at_non_integral_tps() {
    let json = r#"{"name":"progrid","bot":{"type":"chaos","seed":42},
        "duration_s":12.0,"tps":66,"liveness":{"progress":{"require_surface":false}}}"#;
    let scen: bevy_playtest::scenario::Scenario = serde_json::from_str(json).unwrap();
    assert!(
        bevy_playtest::scenario::validate_scenario(&scen).is_ok(),
        "the structurally-default config must skip grid validation; the \
         recorder re-quantizes coherently at tps 66"
    );
}

/// The grid exemption keys on the TIMING fields, not whole-config
/// equality: toggling a non-timing field (here `require_surface`)
/// alone must not drag grid validation onto default timings the
/// author never wrote. At tps 66 the default timings are off-grid,
/// so the whole-config rule would have rejected this config about
/// sample_s values the author left untouched.
#[test]
fn non_timing_field_toggle_keeps_grid_exemption_at_odd_tps() {
    let json = r#"{"name":"progrid3","bot":{"type":"chaos","seed":42},
        "duration_s":12.0,"tps":66,"liveness":{"progress":{"require_surface":true}}}"#;
    let scen: bevy_playtest::scenario::Scenario = serde_json::from_str(json).unwrap();
    assert!(
        bevy_playtest::scenario::validate_scenario(&scen).is_ok(),
        "a config differing from the default only in a non-timing field \
         must keep the grid exemption — the timing validations reject \
         AUTHORED timings, and none were authored here"
    );
}

/// Timings AUTHORED at tps 66 are held to the grid: 0.25s is 16.5
/// frames there — a loud rejection naming the grid, not a silent
/// re-rounding. (window_s differs from the default so the config is
/// genuinely authored; the sample_s diagnosis is what must fire.)
#[test]
fn authored_non_integral_sample_rejected_at_odd_tps() {
    let json = r#"{"name":"progrid2","bot":{"type":"chaos","seed":42},
        "duration_s":12.0,"tps":66,
        "liveness":{"progress":{"mode":"auto","window_s":8.5,"sample_s":0.25}}}"#;
    let scen: bevy_playtest::scenario::Scenario = serde_json::from_str(json).unwrap();
    match bevy_playtest::scenario::validate_scenario(&scen) {
        Err(bevy_playtest::scenario::ScenarioError::Rejected(msg)) => {
            assert!(
                msg.contains("whole number of frames"),
                "message {msg:?} lacks the grid diagnosis"
            );
        }
        other => panic!("expected Rejected, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Structural: recorder insertion independence (input-plugin apps)
// ---------------------------------------------------------------------------

/// The progress recorder is inserted by the driver at FUNCTION scope
/// whenever progress is enabled — independent of the input-plugin
/// guard block that wraps the bot/observer systems. An app that HAS
/// bevy_input::InputPlugin must still record progress samples (the
/// guard must never swallow the recorder).
#[test]
fn input_plugin_app_still_records_progress_samples() {
    let _slot = common::run_slot();
    let mut app = base_app();
    app.add_plugins(bevy_input::InputPlugin);
    app.add_systems(Update, oscillate);
    let report = run_scenario(&mut app, &auto_scenario("")).unwrap();
    assert!(
        !report.progress_samples.is_empty(),
        "the recorder must be inserted regardless of the input-plugin path"
    );
}
