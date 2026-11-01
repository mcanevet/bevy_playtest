//! Keyboard dual-form delivery tests (mirroring the mouse wrapping tests).
//!
//! The keyboard bots now write BOTH the raw `KeyboardInput` (feeds
//! `ButtonInput<KeyCode>::just_pressed`) AND the wrapped
//! `bevy_window::WindowEvent::KeyboardInput` (what games reading the
//! wrapped stream expect, and what winit emits). Without dual-form,
//! consumers of the wrapped form would be deaf to bot typing.
//!
//! `logical_key`/`text` are populated for printable keys via
//! `logical_key_for_code` (US QWERTY assumption, same as `KeyCode` names);
//! this enables bevy_text's editable-text path to see bot typing.

#![cfg(feature = "takeover")]

// Shared test harness (run-slot serialization)
#[path = "common/mod.rs"]
mod common;

use bevy::prelude::*;
use bevy_input::ButtonState;
use bevy_playtest::run_scenario;
use bevy_playtest::scenario::Scenario;
use bevy_window::WindowEvent;

/// Probe that records wrapped `WindowEvent::KeyboardInput` deliveries.
#[derive(Resource, Default, Debug)]
struct WrappedKeyboardProbe {
    pressed_count: u32,
    released_count: u32,
    character_logical_key_count: u32,
    text_count: u32,
    space_character_count: u32,
    /// Text payloads observed on PRESSED events, in arrival order.
    pressed_texts: Vec<String>,
    /// `Character` payloads observed on PRESSED events, in arrival order.
    pressed_characters: Vec<String>,
}

fn wrapped_keyboard_probe(
    mut probe: ResMut<WrappedKeyboardProbe>,
    mut events: MessageReader<WindowEvent>,
) {
    for event in events.read() {
        let WindowEvent::KeyboardInput(evt) = event else {
            continue;
        };
        match evt.state {
            ButtonState::Pressed => probe.pressed_count += 1,
            ButtonState::Released => probe.released_count += 1,
        }
        // Restrict the `Character` assertion to PRESSED events (per the
        // review request) so repeats/releases cannot inflate the count.
        if let bevy_input::keyboard::Key::Character(s) = &evt.logical_key {
            if evt.state == ButtonState::Pressed {
                probe.character_logical_key_count += 1;
                probe.pressed_characters.push(s.to_string());
            }
        }
        if evt.state == ButtonState::Pressed {
            if let Some(text) = &evt.text {
                probe.text_count += 1;
                probe.pressed_texts.push(text.to_string());
            }
        }
        // Space is printable in winit's text-input pipeline: it carries
        // `logical_key: Key::Space` AND `text: Some(" ")`.
        if matches!(evt.logical_key, bevy_input::keyboard::Key::Space)
            && evt.state == ButtonState::Pressed
        {
            probe.space_character_count += 1;
        }
    }
}

/// Build an app wired with the wrapped-keyboard probe.
fn probe_app() -> App {
    let mut app = App::new();
    app.add_plugins(
        bevy::DefaultPlugins
            .build()
            .disable::<bevy_winit::WinitPlugin>(),
    );
    app.insert_resource(WrappedKeyboardProbe::default());
    app.add_systems(Update, wrapped_keyboard_probe);
    app
}

/// Positive case: chaos keyboard writes wrapped `WindowEvent::KeyboardInput`.
#[test]
fn wrapped_keyboard_events_reach_consumers() {
    let _slot = common::run_slot();
    let scenario: Scenario = serde_json::from_str(
        r#"{"name":"wrapped-keyboard","bot":{"type":"chaos","seed":42,"raw_surface":{"keys":["KeyA"],"mouse_buttons":[]},"input_rate_hz":10},"duration_s":2.0,"tps":60,"invariants":[]}"#,
    )
    .expect("scenario parses");

    let mut app = probe_app();
    run_scenario(&mut app, &scenario).expect("run completes");
    let probe = app.world().resource::<WrappedKeyboardProbe>();

    assert!(
        probe.pressed_count > 0,
        "CHAINED REGRESSION: chaos keyboard bot ran but wrapped \
         `WindowEvent::KeyboardInput` never reached the probe (probe: {probe:?}). \
         The bot may be emitting only raw `KeyboardInput` messages."
    );
    // Every press is eventually released (hold-expiry path).
    assert!(
        probe.released_count > 0,
        "CHAINED REGRESSION: chaos keyboard pressed {} wrapped keys but \
         never emitted a wrapped release (probe: {probe:?})",
        probe.pressed_count
    );
}

/// `logical_key` and `text` are populated for printable keys, enabling
/// bevy_text's editable-text path to see bot typing.
#[test]
fn logical_key_and_text_populated_for_printable_keys() {
    let _slot = common::run_slot();
    let scenario: Scenario = serde_json::from_str(
        r#"{"name":"logical-key-probe","bot":{"type":"chaos","seed":42,"raw_surface":{"keys":["KeyA","Digit1"],"mouse_buttons":[]},"input_rate_hz":10},"duration_s":2.0,"tps":60,"invariants":[]}"#,
    )
    .expect("scenario parses");

    let mut app = probe_app();
    run_scenario(&mut app, &scenario).expect("run completes");
    let probe = app.world().resource::<WrappedKeyboardProbe>();

    // Sanity floor: the raw form must have flowed too (this fails loudly
    // if the whole scenario misconfigures rather than the mapping regressing).
    assert!(
        probe.pressed_count > 0,
        "SANITY: no wrapped keyboard presses observed at all (probe: {probe:?})"
    );

    // `Character` logical keys are counted on PRESSED events only.
    assert!(
        probe.character_logical_key_count > 0,
        "REGRESSION: bot keyboard presses had `logical_key` set to a \
         non-`Character` value for printable keys (probe: {probe:?}). \
         This breaks bevy_text's editable-text path."
    );
    assert!(
        probe.text_count > 0,
        "REGRESSION: bot keyboard presses never populated `text` for \
         printable keys (probe: {probe:?}). Text-input games rely on this field."
    );
}

/// Synthetic-keyboard scenario: wrapped press/release behavior with explicit
/// assertions on `logical_key` and `text`, including Shift+letter uppercase
/// mapping and Space carrying `text: Some(" ")`.
///
/// Schedule (releases are processed before presses within a frame, so the
/// Shift hold must outlive the letter press):
/// - frame 1: press ShiftLeft (held through frame 5)
/// - frame 2: press KeyA while Shift held  -> logical/text "A"
/// - frame 4: press Space                  -> text " "
/// - frame 7: press KeyA with Shift gone   -> logical/text "a"
#[test]
fn synthetic_keyboard_wrapped_events_with_modifiers() {
    let _slot = common::run_slot();
    let scenario: Scenario = serde_json::from_str(
        r#"{"name":"synthetic-wrapped-probe","bot":{"type":"synthetic_keyboard","seed":42,"key_presses":[{"frame":1,"key":"ShiftLeft","hold_frames":4},{"frame":2,"key":"KeyA","hold_frames":1},{"frame":4,"key":"Space","hold_frames":1},{"frame":7,"key":"KeyA","hold_frames":1}],"mouse_clicks":[]},"duration_s":2.0,"tps":60,"invariants":[]}"#,
    )
    .expect("scenario parses");

    let mut app = probe_app();
    run_scenario(&mut app, &scenario).expect("run completes");
    let probe = app.world().resource::<WrappedKeyboardProbe>();

    // Every scheduled press must have flowed through the wrapped stream.
    assert!(
        probe.pressed_count >= 4,
        "Expected at least 4 wrapped presses (Shift, A-shifted, Space, \
         A-unshifted); got {} (probe: {probe:?})",
        probe.pressed_count
    );
    assert!(
        probe.released_count >= 4,
        "Expected at least 4 wrapped releases (one per scheduled press); \
         got {} (probe: {probe:?})",
        probe.released_count
    );

    // Shift+letter: uppercase logical key AND uppercase text.
    assert!(
        probe.pressed_characters.iter().any(|c| c == "A"),
        "REGRESSION: Shift+KeyA did not emit `logical_key: Character(\"A\")` \
         (characters seen: {:?}, probe: {probe:?})",
        probe.pressed_characters
    );
    assert!(
        probe.pressed_texts.iter().any(|t| t == "A"),
        "REGRESSION: Shift+KeyA did not emit `text: Some(\"A\")` \
         (texts seen: {:?}, probe: {probe:?})",
        probe.pressed_texts
    );
    // Unshifted letter stays lowercase.
    assert!(
        probe.pressed_characters.iter().any(|c| c == "a"),
        "REGRESSION: unshifted KeyA did not emit `logical_key: Character(\"a\")` \
         (characters seen: {:?}, probe: {probe:?})",
        probe.pressed_characters
    );

    // Space: Key::Space logical key AND text " ".
    assert!(
        probe.space_character_count >= 1,
        "REGRESSION: Space press did not carry `logical_key: Key::Space` \
         (probe: {probe:?})"
    );
    assert!(
        probe.pressed_texts.iter().any(|t| t == " "),
        "REGRESSION: Space press did not populate `text: Some(\" \")` \
         (texts seen: {:?}, probe: {probe:?}). Space is printable in \
         winit's text-input pipeline.",
        probe.pressed_texts
    );
}
