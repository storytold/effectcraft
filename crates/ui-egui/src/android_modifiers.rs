//! Android's missing keyboard modifiers, stamped onto egui's events.
//!
//! `stamp_android_modifiers` is called only on Android (from the app's `raw_input_hook`):
//! everywhere else winit reports the real modifiers, which this would only duplicate. The
//! unit tests run on every platform, so the module is compiled for tests everywhere too.

#[cfg(any(target_os = "android", test))]
/// winit's Android backend reports no modifier state at all: it ignores the meta state Android
/// carries on every `KeyEvent` and never emits the `ModifiersChanged` event egui-winit learns
/// `Ctrl`/`Shift`/`Alt` from, so every chord — Undo, Save, the panel shortcuts — sees empty
/// modifiers and never matches. Track the modifier keys from the key events themselves, in the
/// order they happen inside the frame, stamp them onto the frame's key events, and announce the
/// result to egui with a `ModifiersChanged` of its own before any widget runs.
///
/// Called only on Android, from `raw_input_hook`; everywhere else winit reports the real
/// modifiers, which this would only duplicate. `held` is the app's cross-frame copy, cleared by
/// a lost focus so a modifier held while the app is backgrounded cannot stick. The frame's own
/// `ModifiersChanged` describes the end of the frame; the per-event stamps carry the exact
/// moment, which is what chord matching reads.
///
/// Ported from FilmCraft's Android input fix (`47f202cb` + `ecea29d6` in
/// `Kaedo17/filmcraft-mobile`), where it was validated on hardware first.
#[cfg(any(target_os = "android", test))]
pub(crate) fn stamp_android_modifiers(events: &mut Vec<egui::Event>, held: &mut egui::Modifiers) {
    for event in events.iter_mut() {
        match event {
            // A lost focus clears the tracked state, so a modifier held while the app is
            // backgrounded cannot stick.
            egui::Event::WindowFocused(false) => *held = egui::Modifiers::NONE,
            egui::Event::Key { key: egui::Key::ControlLeft | egui::Key::ControlRight, pressed, .. } => {
                held.ctrl = *pressed;
                // As egui-winit itself does on every platform but macOS: the command
                // shortcut is Ctrl, and COMMAND chords need `command` set.
                held.command = *pressed;
            }
            egui::Event::Key { key: egui::Key::ShiftLeft | egui::Key::ShiftRight, pressed, .. } => held.shift = *pressed,
            egui::Event::Key { key: egui::Key::AltLeft | egui::Key::AltRight, pressed, .. } => held.alt = *pressed,
            // The wheel and the buttons carry their own modifiers too (Alt/Ctrl + wheel and
            // Ctrl + click read them), and egui-winit fills them from its own state — always
            // empty here. Stamp them with what was held when they fired.
            egui::Event::Key { modifiers, .. } => *modifiers = *held,
            egui::Event::MouseWheel { modifiers, .. } | egui::Event::PointerButton { modifiers, .. } => {
                *modifiers = *held;
            }
            _ => {}
        }
    }
    // `held.command` mirrors ctrl here (COMMAND chords match). Appended last: egui applies
    // `ModifiersChanged` events in order, so the frame-level modifiers end up as this frame's
    // end state — not a stale value left in eframe's pending input buffer from an earlier frame.
    events.push(egui::Event::ModifiersChanged(*held));
}

#[cfg(test)]
mod android_modifier_tests {
    use egui::{Event, Key, Modifiers};

    fn key(key: Key, pressed: bool) -> Event {
        Event::Key { key, physical_key: Some(key), pressed, repeat: false, modifiers: Modifiers::NONE }
    }

    fn stamp(events: Vec<Event>) -> (Vec<Event>, Modifiers) {
        let mut events = events;
        let mut held = Modifiers::NONE;
        super::stamp_android_modifiers(&mut events, &mut held);
        (events, held)
    }

    /// The modifiers each key event of the frame carries, in order.
    fn stamped(events: &[Event]) -> Vec<Modifiers> {
        events
            .iter()
            .filter_map(|e| match e {
                Event::Key { modifiers, .. } => Some(*modifiers),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn ctrl_chord_gets_the_command_modifiers() {
        let (events, held) = stamp(vec![key(Key::ControlLeft, true), key(Key::S, true)]);
        // The Ctrl press itself is left alone; the S press under it carries Ctrl…
        assert_eq!(stamped(&events), vec![Modifiers::NONE, Modifiers { ctrl: true, command: true, ..Modifiers::NONE }]);
        assert!(held.ctrl && held.command);
    }

    #[test]
    fn a_released_ctrl_ends_the_chord_within_the_frame() {
        let ctrl = Modifiers { ctrl: true, command: true, ..Modifiers::NONE };
        let (events, held) = stamp(vec![key(Key::Z, true), key(Key::ControlLeft, true), key(Key::Z, true), key(Key::ControlRight, false), key(Key::Z, true)]);
        // Five key events, none of the modifier presses/releases stamped: the Z under Ctrl
        // carries the chord, the Z after the release does not.
        assert_eq!(stamped(&events), vec![Modifiers::NONE, Modifiers::NONE, ctrl, Modifiers::NONE, Modifiers::NONE]);
        assert_eq!(held, Modifiers::NONE);
    }

    #[test]
    fn shift_and_alt_are_tracked() {
        let both = Modifiers { shift: true, alt: true, ..Modifiers::NONE };
        let (events, held) = stamp(vec![key(Key::ShiftLeft, true), key(Key::AltRight, true), key(Key::F5, true)]);
        assert_eq!(stamped(&events)[2], both);
        assert_eq!(held, both);
    }

    #[test]
    fn a_lost_focus_clears_a_stuck_modifier() {
        let (events, held) = stamp(vec![key(Key::ControlLeft, true), Event::WindowFocused(false), key(Key::S, true)]);
        // stamped() only collects key events: the Ctrl press, then the S press after the focus loss.
        assert_eq!(stamped(&events)[1], Modifiers::NONE, "no chord may survive a focus loss");
        assert_eq!(held, Modifiers::NONE);
    }

    #[test]
    fn the_frame_announces_its_end_state() {
        let (events, _) = stamp(vec![key(Key::ControlLeft, true)]);
        assert!(matches!(events.last(), Some(Event::ModifiersChanged(m)) if m.ctrl && m.command));
        let (events, _) = stamp(vec![]);
        assert!(matches!(events.last(), Some(Event::ModifiersChanged(m)) if *m == Modifiers::NONE));
    }

    #[test]
    fn stamped_modifiers_match_the_binding_patterns() {
        // What the shortcut pass then does with them: Ctrl+S matches a COMMAND binding and not
        // a plain-key one, and an unmodified key the other way around.
        let (events, _) = stamp(vec![key(Key::ControlLeft, true), key(Key::S, true)]);
        let chord = stamped(&events)[1];
        assert!(chord.matches_logically(Modifiers::COMMAND));
        assert!(!chord.matches_logically(Modifiers::NONE));
        let (events, _) = stamp(vec![key(Key::Z, true)]);
        let plain = stamped(&events)[0];
        assert!(plain.matches_logically(Modifiers::NONE));
        assert!(!plain.matches_logically(Modifiers::COMMAND));
    }

    #[test]
    fn wheel_and_click_carry_the_held_modifiers() {
        let ctrl = Modifiers { ctrl: true, command: true, ..Modifiers::NONE };
        let wheel = Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: egui::vec2(0.0, 1.0),
            phase: egui::TouchPhase::Move,
            modifiers: Modifiers::NONE,
            source: egui::MouseWheelSource::Wheel,
        };
        let click = Event::PointerButton { pos: egui::pos2(10.0, 10.0), button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE };
        let (events, _) = stamp(vec![key(Key::ControlLeft, true), wheel, click]);
        // Alt + wheel zoom and Ctrl + click read the event's own modifiers, never the frame's.
        assert_eq!(events[1], wheel_modifiers(ctrl), "the wheel under Ctrl carries the chord");
        assert_eq!(events[2], click_modifiers(ctrl), "a click under Ctrl carries the chord");
    }

    fn wheel_modifiers(m: Modifiers) -> Event {
        Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: egui::vec2(0.0, 1.0),
            phase: egui::TouchPhase::Move,
            modifiers: m,
            source: egui::MouseWheelSource::Wheel,
        }
    }

    fn click_modifiers(m: Modifiers) -> Event {
        Event::PointerButton { pos: egui::pos2(10.0, 10.0), button: egui::PointerButton::Primary, pressed: true, modifiers: m }
    }
}
