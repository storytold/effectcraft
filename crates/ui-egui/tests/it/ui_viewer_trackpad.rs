//! Trackpad gestures in the Composition viewer: a two-finger scroll pans, a pinch (and the mouse
//! wheel, also one that scrolls in pixels) zooms around the pointer (egui_kittest, UI logic only).

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use egui::{Event, Rect, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

fn harness() -> Harness<'static, EffectcraftApp> {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "View", "width": 640, "height": 360, "duration": 4})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.state_mut().show_panel(PanelKind::Composition);
    h.run_steps(4);
    h
}

fn rect(h: &Harness<'_, EffectcraftApp>, id: &str) -> Rect {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}"));
    Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3]))
}

fn wheel(h: &mut Harness<'_, EffectcraftApp>, unit: egui::MouseWheelUnit, delta: egui::Vec2, modifiers: egui::Modifiers) {
    h.input_mut().events.push(Event::MouseWheel { unit, delta, modifiers, phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Unknown });
    h.run_steps(30);
}

#[test]
fn trackpad_scroll_pans_the_composition() {
    let mut h = harness();
    let comp = rect(&h, "viewer.comp");
    h.input_mut().events.push(Event::PointerMoved(comp.center()));
    wheel(&mut h, egui::MouseWheelUnit::Point, vec2(80.0, -40.0), Default::default());
    let after = rect(&h, "viewer.comp");
    let moved = after.min - comp.min;
    assert!((moved.x - 80.0).abs() < 1.5 && (moved.y + 40.0).abs() < 1.5, "the scroll pans: {moved:?}");
    assert!((after.width() - comp.width()).abs() < 0.5, "a scroll doesn't zoom: {} → {}", comp.width(), after.width());
}

#[test]
fn pinch_and_mouse_wheel_zoom_around_the_pointer() {
    let mut h = harness();
    let comp = rect(&h, "viewer.comp");
    let at = comp.min + comp.size() * 0.25;
    h.input_mut().events.push(Event::PointerMoved(at));
    h.input_mut().events.push(Event::Zoom(1.5));
    h.run_steps(3);
    let zoomed = rect(&h, "viewer.comp");
    assert!((zoomed.width() / comp.width() - 1.5).abs() < 0.01, "the pinch zooms: {} → {}", comp.width(), zoomed.width());
    let still = zoomed.min + zoomed.size() * 0.25;
    assert!(still.distance(at) < 2.0, "the point under the pointer stays put: {at:?} vs {still:?}");
    // A mouse wheel (it scrolls in lines) zooms too, and so does Alt + a trackpad scroll.
    wheel(&mut h, egui::MouseWheelUnit::Line, vec2(0.0, 3.0), Default::default());
    let wheeled = rect(&h, "viewer.comp");
    assert!(wheeled.width() > zoomed.width() * 1.1, "the wheel zooms in: {} → {}", zoomed.width(), wheeled.width());
    h.input_mut().events.push(Event::ModifiersChanged(egui::Modifiers::ALT));
    wheel(&mut h, egui::MouseWheelUnit::Point, vec2(0.0, -120.0), egui::Modifiers::ALT);
    let out = rect(&h, "viewer.comp");
    assert!(out.width() < wheeled.width() * 0.9, "Alt + scroll zooms out: {} → {}", wheeled.width(), out.width());
}

/// Browsers and some high-resolution wheels report a mouse wheel in pixels, as a trackpad does:
/// it still zooms.
#[test]
fn a_pixel_unit_mouse_wheel_zooms() {
    let mut h = harness();
    let comp = rect(&h, "viewer.comp");
    let at = comp.min + comp.size() * 0.25;
    h.input_mut().events.push(Event::PointerMoved(at));
    wheel(&mut h, egui::MouseWheelUnit::Point, vec2(0.0, 120.0), Default::default());
    let zoomed = rect(&h, "viewer.comp");
    assert!(zoomed.width() > comp.width() * 1.1, "a pixel wheel zooms in: {} → {}", comp.width(), zoomed.width());
    let still = zoomed.min + zoomed.size() * 0.25;
    assert!(still.distance(at) < 2.0, "the point under the pointer stays put: {at:?} vs {still:?}");
    // Shift turns the wheel's scroll sideways: still a wheel, and the next notch zooms.
    h.input_mut().events.push(Event::ModifiersChanged(egui::Modifiers::SHIFT));
    wheel(&mut h, egui::MouseWheelUnit::Point, vec2(0.0, 120.0), egui::Modifiers::SHIFT);
    h.input_mut().events.push(Event::ModifiersChanged(egui::Modifiers::NONE));
    let before = rect(&h, "viewer.comp");
    wheel(&mut h, egui::MouseWheelUnit::Point, vec2(0.0, -120.0), Default::default());
    let out = rect(&h, "viewer.comp");
    assert!(out.width() < before.width() * 0.9, "the wheel still zooms: {} → {}", before.width(), out.width());
}

/// A straight vertical trackpad scroll pans when the touch phase says it is a trackpad,
/// and after any diagonal scroll; a mouse wheel in lines takes the zoom back.
#[test]
fn a_vertical_trackpad_scroll_pans_once_the_trackpad_is_known() {
    let mut h = harness();
    let comp = rect(&h, "viewer.comp");
    h.input_mut().events.push(Event::PointerMoved(comp.center()));
    h.input_mut().events.push(Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::Vec2::ZERO,
        modifiers: Default::default(),
        source: egui::MouseWheelSource::Unknown,
        phase: egui::TouchPhase::Start,
    });
    h.input_mut().events.push(Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(0.0, 50.0),
        modifiers: Default::default(),
        source: egui::MouseWheelSource::Unknown,
        phase: egui::TouchPhase::Move,
    });
    h.step();
    h.input_mut().events.push(Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::Vec2::ZERO,
        modifiers: Default::default(),
        source: egui::MouseWheelSource::Unknown,
        phase: egui::TouchPhase::End,
    });
    h.run_steps(30);
    let panned = rect(&h, "viewer.comp");
    assert!((panned.min.y - comp.min.y - 50.0).abs() < 1.5, "the scroll pans: {:?}", panned.min - comp.min);
    assert!((panned.width() - comp.width()).abs() < 0.5, "a trackpad scroll doesn't zoom");
    // Without phases (Windows, Linux, the web) it stays a trackpad.
    wheel(&mut h, egui::MouseWheelUnit::Point, vec2(0.0, 30.0), Default::default());
    let again = rect(&h, "viewer.comp");
    assert!((again.min.y - panned.min.y - 30.0).abs() < 1.5 && (again.width() - comp.width()).abs() < 0.5, "{again:?}");
    // A wheel that scrolls in lines is a mouse: it zooms, and so does a pixel wheel after it.
    wheel(&mut h, egui::MouseWheelUnit::Line, vec2(0.0, 3.0), Default::default());
    let zoomed = rect(&h, "viewer.comp");
    assert!(zoomed.width() > again.width() * 1.1, "the wheel zooms: {} → {}", again.width(), zoomed.width());
    wheel(&mut h, egui::MouseWheelUnit::Point, vec2(0.0, 120.0), Default::default());
    assert!(rect(&h, "viewer.comp").width() > zoomed.width() * 1.1, "a pixel wheel zooms again");
}

/// Where the platform names the device (macOS), a straight vertical trackpad scroll pans at once
/// and a wheel in points zooms.
#[test]
fn the_wheel_source_decides_where_the_platform_reports_it() {
    let mut h = harness();
    let comp = rect(&h, "viewer.comp");
    let at = comp.min + comp.size() * 0.25;
    h.input_mut().events.push(Event::PointerMoved(at));
    let scroll = |h: &mut Harness<'_, EffectcraftApp>, source, dy| {
        h.input_mut().events.push(Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, dy),
            modifiers: Default::default(),
            phase: egui::TouchPhase::Move,
            source,
        });
        h.run_steps(30);
    };
    scroll(&mut h, egui::MouseWheelSource::Trackpad, 40.0);
    let panned = rect(&h, "viewer.comp");
    assert!((panned.min.y - comp.min.y - 40.0).abs() < 1.5, "the trackpad pans: {:?}", panned.min - comp.min);
    assert!((panned.width() - comp.width()).abs() < 0.5, "a trackpad scroll doesn't zoom");
    scroll(&mut h, egui::MouseWheelSource::Wheel, 120.0);
    assert!(rect(&h, "viewer.comp").width() > panned.width() * 1.1, "the wheel zooms");
}
