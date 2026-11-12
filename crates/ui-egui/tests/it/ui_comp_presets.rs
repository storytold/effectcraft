//! Composition Settings: saving, picking and deleting your own presets (egui_kittest).

use effectcraft_engine::Session;
use effectcraft_ui_egui::{Dialog, EffectcraftApp};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, pos2};
use egui_kittest::Harness;
use serde_json::json;

fn harness() -> Harness<'static, EffectcraftApp> {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Edit", "width": 2560, "height": 1440, "frameRate": 60, "duration": 4})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_step_dt(1.0 / 60.0).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    h
}

fn center(h: &Harness<'_, EffectcraftApp>, id: &str) -> Pos2 {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}"));
    pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0)
}

fn click(h: &mut Harness<'_, EffectcraftApp>, id: &str) {
    let p = center(h, id);
    h.input_mut().events.push(Event::PointerMoved(p));
    h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.run_steps(1);
    h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    // The automation registry still holds the previous frame's elements: one more frame.
    h.run_steps(3);
}

fn key(h: &mut Harness<'_, EffectcraftApp>, key: Key) {
    h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

fn open_settings(h: &mut Harness<'_, EffectcraftApp>) {
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "comp.settings", json!({})).unwrap();
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::CompSettings));
}

fn preset_names(h: &Harness<'_, EffectcraftApp>) -> Vec<String> {
    h.state().session.prefs.comp_presets.iter().map(|q| q.name.clone()).collect()
}

/// Save the comp's size and rate as a preset (Enter saves without pressing OK), then delete it.
#[test]
fn composition_settings_save_and_delete_your_own_preset() {
    let mut h = harness();
    open_settings(&mut h);
    assert!(h.state().auto.find("dialog.comp.presetDelete").is_none(), "nothing of yours to delete yet");
    click(&mut h, "dialog.comp.presetSave");
    // Choose Name opens with the keyboard in its field.
    h.input_mut().events.push(Event::Text("1440p 60".into()));
    h.run_steps(2);
    key(&mut h, Key::Enter);
    assert_eq!(preset_names(&h), ["1440p 60"]);
    let q = &h.state().session.prefs.comp_presets[0];
    assert_eq!((q.width, q.height, q.pixel_aspect, q.frame_rate), (2560, 1440, 1.0, 60.0));
    assert_eq!(h.state().dialog, Some(Dialog::CompSettings), "Enter in the name field doesn't press OK");
    assert!(h.state().auto.find("dialog.comp.presetName").is_none(), "the name row closes");
    // The comp's values are now that preset, so it can be deleted.
    click(&mut h, "dialog.comp.presetDelete");
    assert!(preset_names(&h).is_empty());
    assert!(h.state().auto.find("dialog.comp.presetDelete").is_none());
}

/// A built-in preset's name can't be taken; Cancel closes the name row without saving.
#[test]
fn a_preset_name_cannot_be_a_built_in_one() {
    let mut h = harness();
    open_settings(&mut h);
    click(&mut h, "dialog.comp.presetSave");
    click(&mut h, "dialog.comp.presetName");
    h.input_mut().events.push(Event::Text("HDTV 1080 24".into()));
    h.run_steps(2);
    click(&mut h, "dialog.comp.presetNameSave");
    assert!(preset_names(&h).is_empty(), "the built-in name is refused");
    click(&mut h, "dialog.comp.presetNameCancel");
    assert!(h.state().auto.find("dialog.comp.presetName").is_none());
    assert!(preset_names(&h).is_empty());
}

/// Cancel and OK stay where they are when switching between Basic, Advanced and 3D Renderer,
/// at any interface scale: the tabs' heights are measured, not assumed.
#[test]
fn composition_settings_buttons_stay_put_between_tabs() {
    for zoom in [1.0, 1.3] {
        let mut h = harness();
        h.ctx.set_zoom_factor(zoom);
        h.run_steps(3);
        open_settings(&mut h);
        let rects = |h: &Harness<'_, EffectcraftApp>| {
            ["dialog.comp.ok", "dialog.comp.cancel", "dialog.comp.name"].map(|id| h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}")).rect)
        };
        let basic = rects(&h);
        for tab in ["advanced", "renderer", "basic"] {
            click(&mut h, &format!("dialog.comp.tab.{tab}"));
            assert_eq!(rects(&h), basic, "{tab} at {zoom}x");
        }
    }
}

/// Composition Settings keeps Cancel and OK in view in every interface language: a translation
/// wider than the English (French, #2841) widens the dialog instead of pushing OK past its right
/// edge, where it could be reached only by scrolling sideways.
#[test]
fn composition_settings_buttons_fit_every_language() {
    // English, then French (#2841) and the other catalogs whose rows overflowed the 600 points.
    for language in ["en", "fr", "es", "ka", "cs", "pt-br"] {
        let mut s = Session::default();
        s.execute("comp.new", json!({"name": "Edit", "width": 2560, "height": 1440, "frameRate": 60, "duration": 4})).unwrap();
        s.prefs.general.language = language.into();
        let mut h =
            Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_step_dt(1.0 / 60.0).allow_missing_glyphs().build_eframe(|_| EffectcraftApp::new(s));
        h.run_steps(3);
        open_settings(&mut h);
        let mut oks = vec![];
        for tab in ["basic", "advanced", "renderer"] {
            click(&mut h, &format!("dialog.comp.tab.{tab}"));
            let rect = |id: &str| h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}")).rect;
            let (view, ok, cancel) = (rect("dialog.comp.view"), rect("dialog.comp.ok"), rect("dialog.comp.cancel"));
            for (name, r) in [("OK", ok), ("Cancel", cancel)] {
                assert!(r[0] >= view[0] && r[0] + r[2] <= view[0] + view[2] + 0.5, "{language} {tab}: {name} {r:?} is outside the dialog {view:?}");
            }
            if language == "en" {
                // English still fits the 600-point dialog (18-point margins).
                assert_eq!(view[2], 600.0 - 36.0, "{tab}");
            }
            oks.push(ok);
        }
        // The dialog takes the width of its widest tab, so OK stays put between tabs.
        assert!(oks.windows(2).all(|w| w[0] == w[1]), "{language}: OK moves between tabs: {oks:?}");
    }
}

fn rect_of(h: &Harness<'_, EffectcraftApp>, id: &str) -> egui::Rect {
    let r = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}")).rect;
    egui::Rect::from_min_size(pos2(r[0], r[1]), egui::vec2(r[2], r[3]))
}

/// In a window too short for it, the dialog scrolls: its registered view stays the scroll
/// area's viewport (not a part of it shifted by the scroll), and OK scrolls into it.
#[test]
fn composition_settings_view_stays_put_when_it_scrolls() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Edit", "width": 2560, "height": 1440, "frameRate": 60, "duration": 4})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 520.0)).with_step_dt(1.0 / 60.0).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    open_settings(&mut h);
    let (view, ok) = (rect_of(&h, "dialog.comp.view"), rect_of(&h, "dialog.comp.ok"));
    assert!(ok.bottom() > view.bottom(), "the dialog must be taller than the window for this test: OK {ok:?}, view {view:?}");
    h.input_mut().events.push(Event::PointerMoved(view.center()));
    h.input_mut().events.push(Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -2000.0),
        modifiers: Modifiers::NONE,
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Wheel,
    });
    h.run_steps(30);
    let (scrolled, ok) = (rect_of(&h, "dialog.comp.view"), rect_of(&h, "dialog.comp.ok"));
    // The scroll bar may take a few points on the right while it is shown.
    assert_eq!((scrolled.y_range(), scrolled.left()), (view.y_range(), view.left()), "the view is the viewport, wherever the content is scrolled to");
    assert!(scrolled.contains_rect(ok), "OK {ok:?} scrolls into the view {scrolled:?}");
}

/// Changing the interface language with the dialog open refits it to the new text and then
/// settles, with OK inside it: its scroll bars don't trade places every frame while it grows.
/// Switching back to English returns it to 600 points.
#[test]
fn composition_settings_refits_when_the_language_changes() {
    let mut s = Session::default();
    // Idle caching keeps repainting while it renders; this test waits for repaints to stop.
    s.prefs.previews.cache_frames_when_idle = false;
    s.execute("comp.new", json!({"name": "Edit", "width": 2560, "height": 1440, "frameRate": 60, "duration": 4})).unwrap();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1600.0, 1000.0))
        .with_step_dt(1.0 / 60.0)
        .with_max_steps(30)
        // `run` checks accessibility after settling; that is not what this test is about.
        .with_accessibility_check(false)
        .build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    open_settings(&mut h);
    click(&mut h, "dialog.comp.tab.advanced");
    h.run_ok().expect("the open dialog settles");
    assert_eq!(rect_of(&h, "dialog.comp.view").width(), 600.0 - 36.0);
    for (language, wider) in [("fr", true), ("en", false)] {
        h.state_mut().session.prefs.general.language = language.into();
        // Steps while anything asks for a repaint, and fails if that never stops.
        let steps = h.try_run().unwrap_or_else(|e| panic!("{language}: {e}"));
        let (view, ok) = (rect_of(&h, "dialog.comp.view"), rect_of(&h, "dialog.comp.ok"));
        assert!(view.contains_rect(ok), "{language}: after {steps} steps OK {ok:?} is outside the dialog {view:?}");
        assert_eq!(view.width() > 600.0 - 36.0, wider, "{language}: the dialog is {} wide", view.width());
    }
}

/// OK in the New Composition dialog opened over the Home screen (the native menu shows it there)
/// leaves the Home screen for the new composition; Cancel keeps it.
#[test]
fn new_composition_dialog_ok_leaves_the_home_screen() {
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_step_dt(1.0 / 60.0).build_eframe(|_| EffectcraftApp::new(Session::default()));
    h.run_steps(3);
    h.state_mut().ui.start_screen = true;
    h.state_mut().dialog = Some(Dialog::NewComp);
    h.run_steps(3);
    click(&mut h, "dialog.comp.cancel");
    assert_eq!(h.state().dialog, None);
    assert!(h.state().ui.start_screen, "Cancel keeps the Home screen");
    h.state_mut().dialog = Some(Dialog::NewComp);
    h.run_steps(3);
    click(&mut h, "dialog.comp.ok");
    assert_eq!(h.state().dialog, None);
    assert!(h.state().session.active_comp_id().is_some(), "a composition was made");
    assert!(!h.state().ui.start_screen, "OK leaves the Home screen");
}
