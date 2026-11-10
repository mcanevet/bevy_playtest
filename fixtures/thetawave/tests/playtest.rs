//! Seeded by hand-porting upstream thetawave (see fixture README NOTICE)
//! — COMMITTED, edit freely. Plain `cargo test` runs the default
//! playtest battery against the game binary (compiled with the
//! playtest feature via the self-dev-dependency). Per-game assertions
//! belong here.
//!
//! This is the REAL-WORLD fixture: no ground-truth file, no numeric
//! accuracy claims. Assertions are honest-demo level: the game boots
//! headless under takeover, a declared Leafwing action drives real
//! gameplay, and the discovery reader enumerates the game's actual
//! InputMaps.

/// Scenario that walks the main menu with the game's own declared
/// Leafwing actions: Enter confirms the selected entry (idx 0 =
/// `EnterCharacterSelection`), arrows/WASD drive the menu's
/// `NavigateXxxKeyboard` actions. Deterministic input script; the
/// game itself remains free to run (background picking uses
/// thread-local RNG, so NO determinism claim is made about the
/// world — the claims below are all per-run-derivable).
///
/// Frames anchor to SIMULATED time (tps=60) while the game's
/// `bevy_asset_loader` states advance in WALL time — on a loaded CI
/// runner the first scripted press could land while
/// `LoadingAssets` is still active and get swallowed. The first
/// press therefore sits at frame 270 (4.5 s lead-in) so loading
/// completes well before any input; keep this margin when editing.
const MENU_DRIVE_SCENARIO: &str = r#"{
  "bot": {
    "type": "synthetic_keyboard",
    "key_presses": [
      {"frame": 270, "key": "Enter", "hold_frames": 3},
      {"frame": 330, "key": "Enter", "hold_frames": 3},
      {"frame": 390, "key": "ArrowDown", "hold_frames": 2},
      {"frame": 450, "key": "KeyS", "hold_frames": 2},
      {"frame": 510, "key": "ArrowLeft", "hold_frames": 2},
      {"frame": 570, "key": "ArrowRight", "hold_frames": 2},
      {"frame": 630, "key": "ArrowUp", "hold_frames": 2},
      {"frame": 690, "key": "KeyW", "hold_frames": 2},
      {"frame": 750, "key": "Enter", "hold_frames": 3},
      {"frame": 840, "key": "Escape", "hold_frames": 3}
    ]
  },
  "tps": 60,
  "duration_s": 16.0
}"#;

/// Write [`MENU_DRIVE_SCENARIO`] to a fresh temp file (RAII-removed on
/// every exit path, panic included) and return its path.
fn menu_drive_scenario_file(tag: &str) -> (std::path::PathBuf, impl Drop) {
    let dir = std::env::temp_dir().join(format!(
        "bevy_playtest_thetawave_menudrive_{}_{}_{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("temp scenario dir creatable");
    let path = dir.join("menu_drive_scenario.json");
    std::fs::write(&path, MENU_DRIVE_SCENARIO).expect("scenario file writable");
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    (path, Cleanup(dir))
}

/// Run the fixture under the deterministic menu-drive scenario
/// (headless, via the battery runner) and return the parsed report —
/// the LW declared rows and tap stats ride the report out of the
/// child process.
fn run_menu_drive(tag: &str) -> bevy_playtest::state::PlaytestReport {
    let (scenario, _cleanup) = menu_drive_scenario_file(tag);
    let config = bevy_playtest::battery::BatteryConfig {
        binary: env!("CARGO_BIN_EXE_fixture_thetawave").to_string(),
        scenario: scenario.to_string_lossy().into_owned(),
        timeout_secs: 300,
    };
    let report = bevy_playtest::battery::run_with_config(&config)
        .expect("menu-drive run completes and reports");
    assert_eq!(
        report.status,
        bevy_playtest::enums::PlaytestStatus::Pass,
        "the real game must run clean under a scenario that drives its \
         own declared menu actions — this is the real-world fixture's \
         basic health claim"
    );
    report
}

#[cfg(feature = "playtest")]
#[test]
fn playtest_battery() {
    bevy_playtest::battery::run_and_expect_clean(
        env!("CARGO_BIN_EXE_fixture_thetawave"),
        bevy_playtest::battery::BUILTIN_BATTERY,
    );
}

/// READER DEMO (honest claims only — thetawave is NOT a planted
/// oracle and there is NO ground-truth file):
///
/// - The pure-reflection Leafwing reader enumerates the real game's
///   RON-loaded `MenuAction` map (bound in
///   `src/options/input.ron`, loaded at startup) — recognized action
///   names and inputs straight from the real game, not from us.
/// - The intent-tap correlates the scenario's scripted presses with
///   the game's own `ActionState`s (presses of Enter/WASD/arrows are
///   observed firing the declared actions they are bound to).
/// - The reconcile composition grades at least one really-driven
///   action `manager_pipeline_ok` (declared ∧ tap-fired) and keeps
///   the whole run free of `DeclaredButDead` alarms (a redundant-
///   by-design tripwire — see the assertion comment).
/// - NO numeric precision/recall bar: this asserts presence, not
///   completeness (the map population changes with game state; the
///   reader snapshots the first non-empty frame).
#[cfg(feature = "playtest")]
#[test]
fn reader_demo_enumerates_real_input_maps() {
    let report = run_menu_drive("reader_demo");

    // READER ENUMERATION: declared rows exist and carry the real
    // game's action labels. Spot the map by its reflected type
    // path; the action names are thetawave's own enum variants.
    let declared = &report.lw_declared;
    assert!(
        !declared.is_empty(),
        "the LW reader must find at least one InputMap in the real game"
    );
    let menu_rows: Vec<_> = declared
        .iter()
        .filter(|d| d.action_map.contains("MenuAction"))
        .collect();
    assert!(
        !menu_rows.is_empty(),
        "the reader must enumerate thetawave's real MenuAction InputMap"
    );
    for (action, input) in [
        ("Confirm", "Enter"),
        ("NavigateUpKeyboard", "ArrowUp"),
        ("NavigateDownKeyboard", "ArrowDown"),
        ("NavigateLeftKeyboard", "ArrowLeft"),
        ("NavigateRightKeyboard", "ArrowRight"),
        ("Back", "Escape"),
    ] {
        assert!(
            menu_rows
                .iter()
                .any(|d| d.action == action && d.input == input),
            "reader missed the real game's declared binding {action} <- \
             {input} (from thetawave's own input.ron)"
        );
    }

    // INTENT-TAP: the scenario pressed these keys and the tap
    // observed them firing the very actions they are bound to.
    for action in [
        "Confirm",
        "NavigateUpKeyboard",
        "NavigateDownKeyboard",
        "NavigateLeftKeyboard",
        "NavigateRightKeyboard",
    ] {
        let fired = report.lw_tap_stats.iter().any(|s| {
            s.action_map.contains("MenuAction") && s.action == action && s.fired_frames > 0
        });
        assert!(
            fired,
            "intent-tap must observe {action} firing under the scripted \
             press of its declared key (manager pipeline live)"
        );
    }

    // RECONCILE: declared ∧ fired earns `manager_pipeline_ok`; and
    // no driven menu action is reported dead. `retracted_residue`
    // must cover every declared input (all are declared, hence any
    // draft DiscoveredButUndeclared residue on them retracts).
    let reconciled = bevy_playtest::discovery::lw::reconcile(
        &bevy_playtest::discovery::lw::LwRunEvidence {
            declared: report.lw_declared.clone(),
            stats: report.lw_tap_stats.clone(),
        },
        &Default::default(),
    );
    assert!(
        reconciled.entries.iter().any(|e| matches!(
            e.grade,
            bevy_playtest::discovery::provenance::ProvenanceGrade::ManagerPipelineOk
        ) && e.action_label.as_deref() == Some("Confirm")),
        "reconcile must grade the really-driven Confirm action \
         manager_pipeline_ok"
    );
    // Whole-run dead-wiring tripwire: DeclaredButDead fires only for
    // declared bindings the scenario PRESSED that never fired
    // (pressed-never-fired arm), or for disabled-and-never-fired
    // actions. For the five driven actions asserted `fired_frames > 0`
    // above this is subsumed by construction — but the scenario also
    // presses Enter/Escape, whose OTHER menu bindings
    // (`Back`/`ExitPauseMenu`/`PauseGame`, `JoinKeyboard`/
    // `PlayerReadyKeyboard`) are not individually asserted, and those
    // rows keep real signal here. It is a redundant-by-design safety
    // net, not an independent demonstration.
    assert!(
        !reconciled
            .findings
            .iter()
            .any(|f| f.kind
                == bevy_playtest::discovery::surface::DiscoveryFindingKind::DeclaredButDead),
        "no DeclaredButDead alarm may fire anywhere in the menu-drive \
         run — pressed bindings must fire, disabled ones are expected \
         to re-enable, not die"
    );
}

// `cargo package` strips `[dev-dependencies]`, so the self-dev-dependency
// that auto-activates `playtest` is gone in an unpacked archive. Rather
// than fail to compile (missing `bevy_playtest`) or silently run zero
// playtests, surface it as an ignored test pointing at the fix.
#[cfg(not(feature = "playtest"))]
#[test]
#[ignore = "playtest feature inactive: self-dev-dependency stripped (e.g. cargo package). Run `cargo test --features playtest`."]
fn playtest_battery() {}

#[cfg(not(feature = "playtest"))]
#[test]
#[ignore = "playtest feature inactive: self-dev-dependency stripped (e.g. cargo package). Run `cargo test --features playtest`."]
fn reader_demo_enumerates_real_input_maps() {}
