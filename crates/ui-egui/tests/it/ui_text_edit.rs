//! Headless checks for on-canvas text editing in the viewer and the Character / Paragraph panels
//! acting on the selection (egui_kittest, UI logic only: no GPU needed).

use effectcraft_engine::Session;
use effectcraft_engine::commands::text_edit::layer_doc;
use effectcraft_engine::project::LayerId;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use egui::{Event, Key, Modifiers, PointerButton, Pos2};
use egui_kittest::Harness;
use serde_json::json;

fn app() -> EffectcraftApp {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Type", "width": 640, "height": 360, "duration": 2})).unwrap();
    EffectcraftApp::new(s)
}

/// A headless window at the size and frame step the viewer tests here are written for.
fn harness(a: EffectcraftApp) -> Harness<'static, EffectcraftApp> {
    Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_step_dt(1.0 / 60.0).build_eframe(move |cc| {
        EffectcraftApp::install_ime_reset_hooks(&cc.egui_ctx);
        a
    })
}

fn rect(h: &Harness<'_, EffectcraftApp>, id: &str) -> egui::Rect {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}"));
    egui::Rect::from_min_size(egui::pos2(e.rect[0], e.rect[1]), egui::vec2(e.rect[2], e.rect[3]))
}

fn click(h: &mut Harness<'_, EffectcraftApp>, p: Pos2, modifiers: Modifiers) {
    h.input_mut().events.push(Event::PointerMoved(p));
    h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers });
    h.run_steps(1);
    h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers });
    h.run_steps(2);
}

fn drag(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(from));
    h.input_mut().events.push(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.run_steps(1);
    for i in 1..=8 {
        h.input_mut().events.push(Event::PointerMoved(from + (to - from) * (i as f32 / 8.0)));
        h.run_steps(1);
    }
    h.input_mut().events.push(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
}

fn key(h: &mut Harness<'_, EffectcraftApp>, key: Key, modifiers: Modifiers) {
    h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers });
    h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers });
    h.run_steps(2);
}

fn type_text(h: &mut Harness<'_, EffectcraftApp>, t: &str) {
    h.input_mut().events.push(Event::Text(t.into()));
    h.run_steps(2);
}

fn edited(h: &Harness<'_, EffectcraftApp>) -> (u64, String, usize, usize) {
    let s = &h.state().session;
    let e = s.state.text_edit.clone().expect("editing");
    (e.layer.0, layer_doc(s, e.layer).unwrap().text, e.anchor, e.caret)
}

#[test]
fn type_tool_skips_hidden_and_unsoloed_text_layers() {
    for switch in ["video", "solo"] {
        let mut s = Session::default();
        s.execute("comp.new", json!({"name": "Type", "width": 640, "height": 360, "duration": 2})).unwrap();
        let mut ids = vec![];
        for _ in 0..2 {
            let lid = s.execute("layer.newText", json!({"text": "alpha beta", "size": 80, "position": [100, 200], "justify": "left"})).unwrap()["layer"]
                .as_u64()
                .unwrap();
            ids.push(lid);
        }
        let (target, value) = if switch == "video" { (ids[1], false) } else { (ids[0], true) };
        s.execute("layer.setSwitch", json!({"layers": [target], "switch": switch, "value": value})).unwrap();
        s.execute("edit.deselectAll", json!({})).unwrap();
        let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(move |_| EffectcraftApp::new(s));
        h.state_mut().ui.tool = effectcraft_ui_egui::state::Tool::Type;
        h.run_steps(3);
        let comp = rect(&h, "viewer.comp");
        let k = comp.width() / 640.0;
        click(&mut h, comp.min + egui::vec2(330.0 * k, 180.0 * k), Modifiers::NONE);
        assert_eq!(edited(&h).0, ids[0], "{switch}");
    }
}

#[test]
fn modal_blocks_background_text_events() {
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| app());
    h.run_steps(3);
    h.state_mut().ui.tool = effectcraft_ui_egui::state::Tool::Type;
    h.run_steps(1);
    let p = rect(&h, "viewer.comp").center();
    click(&mut h, p, Modifiers::NONE);
    type_text(&mut h, "Preserve");
    let before = edited(&h).1;
    h.state_mut().dialog = Some(effectcraft_ui_egui::Dialog::About);
    h.run_steps(3);
    h.input_mut().events.push(Event::Text("changed".into()));
    h.input_mut().events.push(Event::Paste("pasted".into()));
    key(&mut h, Key::Backspace, Modifiers::NONE);
    assert_eq!(edited(&h).1, before);
}

#[test]
fn type_tool_click_type_edit_and_commit() {
    let mut h = harness(app());
    h.state_mut().show_panel(PanelKind::Composition);
    h.run_steps(3);
    h.state_mut().ui.tool = effectcraft_ui_egui::state::Tool::Type;
    h.run_steps(1);
    let comp = rect(&h, "viewer.comp");
    // Click: a point text layer in edit mode, caret ready.
    click(&mut h, comp.center(), Modifiers::NONE);
    assert_eq!(h.state().session.active_comp().unwrap().layers.len(), 1);
    type_text(&mut h, "Hello");
    type_text(&mut h, " world");
    let (lid, text, a, c) = edited(&h);
    assert_eq!((text.as_str(), a, c), ("Hello world", 11, 11));
    assert!(h.state().auto.find("viewer.textEdit").is_some(), "edit region registered");
    // Arrow keys, Shift-extend, Backspace.
    key(&mut h, Key::ArrowLeft, Modifiers::NONE);
    key(&mut h, Key::ArrowLeft, Modifiers::SHIFT);
    assert_eq!((edited(&h).2, edited(&h).3), (10, 9));
    key(&mut h, Key::Backspace, Modifiers::NONE);
    assert_eq!(edited(&h).1, "Hello word");
    // Word jump and Home / End.
    key(&mut h, Key::Home, Modifiers::NONE);
    assert_eq!(edited(&h).3, 0);
    key(&mut h, Key::ArrowRight, Modifiers::COMMAND);
    assert_eq!(edited(&h).3, 5);
    key(&mut h, Key::End, Modifiers::NONE);
    assert_eq!(edited(&h).3, 10);
    // Enter starts a new paragraph.
    key(&mut h, Key::Enter, Modifiers::NONE);
    type_text(&mut h, "two");
    assert_eq!(edited(&h).1, "Hello word\ntwo");
    // Select all, then the Character panel styles the selection only.
    key(&mut h, Key::ArrowUp, Modifiers::NONE);
    assert!(edited(&h).3 <= 10, "up to the first line");
    key(&mut h, Key::Home, Modifiers::NONE);
    key(&mut h, Key::End, Modifiers::SHIFT);
    let (_, _, a, c) = edited(&h);
    assert_eq!((a, c), (0, 10));
    h.state_mut().show_panel(PanelKind::Character);
    h.run_steps(3);
    let bold = rect(&h, "character.fauxBold");
    click(&mut h, bold.center(), Modifiers::NONE);
    let doc = layer_doc(&h.state().session, LayerId(lid)).unwrap();
    assert!(doc.style_at(2).faux_bold, "selection is bold");
    assert!(!doc.style_at(12).faux_bold, "second paragraph untouched");
    assert_eq!(doc.runs().len(), 2);
    // Undo the style; Escape commits.
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert!(layer_doc(&h.state().session, LayerId(lid)).unwrap().is_uniform());
    h.state_mut().show_panel(PanelKind::Composition);
    h.run_steps(3);
    h.state_mut().session.execute("text.setSelection", json!({"start": 0})).unwrap();
    h.run_steps(2);
    key(&mut h, Key::Escape, Modifiers::NONE);
    assert!(h.state().session.state.text_edit.is_none());
}

/// #831: clicking another panel (a solid's row in the Timeline) ends text editing and gives the
/// keyboard back, whether the edit began with the Type tool (which stays selected) or by command:
/// typed characters change no text and Space plays. The Character panel keeps the edit, and a
/// text layer clicked into being and left empty goes away.
#[test]
fn clicking_another_panel_stops_typing_into_text() {
    for type_tool in [false, true] {
        let mut s = Session::default();
        s.execute("comp.new", json!({"name": "Type", "width": 640, "height": 360, "duration": 2})).unwrap();
        let solid = s.execute("layer.newSolid", json!({"name": "Plate", "color": "#406080"})).unwrap()["layer"].as_u64().unwrap();
        let title =
            s.execute("layer.newText", json!({"text": "Title", "size": 80, "position": [100, 120], "justify": "left"})).unwrap()["layer"].as_u64().unwrap();
        let mut h = harness(EffectcraftApp::new(s));
        h.state_mut().show_panel(PanelKind::Composition);
        h.run_steps(3);
        let layers = |h: &Harness<'_, EffectcraftApp>| h.state().session.active_comp().unwrap().layers.len();
        // (Its row moves down as text layers are added.)
        let row = |h: &Harness<'_, EffectcraftApp>| rect(h, &format!("timeline.layer.{solid}.row")).center();
        if type_tool {
            h.state_mut().ui.tool = effectcraft_ui_egui::state::Tool::Type;
            h.run_steps(1);
            let at = rect(&h, "viewer.comp").center();
            click(&mut h, at, Modifiers::NONE);
            assert_eq!(layers(&h), 3, "the Type tool started a text layer");
            let at_row = row(&h);
            click(&mut h, at_row, Modifiers::NONE);
            assert_eq!(layers(&h), 2, "left empty, it is removed");
            click(&mut h, at, Modifiers::NONE);
        } else {
            h.state_mut().session.execute("text.edit", json!({"layer": title})).unwrap();
            h.run_steps(2);
        }
        type_text(&mut h, "Hello");
        // The Character panel formats the selection and keeps the edit: typing carries on.
        key(&mut h, Key::ArrowLeft, Modifiers::SHIFT);
        let range = edited(&h);
        h.state_mut().show_panel(PanelKind::Character);
        h.run_steps(3);
        let bold = rect(&h, "character.fauxBold").center();
        click(&mut h, bold, Modifiers::NONE);
        assert_eq!(edited(&h), range, "type_tool {type_tool}: Character keeps the edit and its selection");
        key(&mut h, Key::End, Modifiers::NONE);
        type_text(&mut h, "!");
        let (lid, text, ..) = edited(&h);
        assert!(text.ends_with("Hello!"), "typing goes into the text: {text:?}");
        let at_row = row(&h);
        click(&mut h, at_row, Modifiers::NONE);
        assert!(h.state().session.state.text_edit.is_none(), "type_tool {type_tool}: clicking the Timeline ends the edit");
        assert_eq!(h.state().session.state.selected_layers, vec![LayerId(solid)]);
        assert!(!h.ctx.memory(|m| m.focused().is_some()), "the keyboard is the app's again");
        type_text(&mut h, "abc");
        assert!(!h.state().playback.playing);
        h.input_mut().events.push(Event::Key { key: Key::Space, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
        h.input_mut().events.push(Event::Text(" ".into()));
        h.input_mut().events.push(Event::Key { key: Key::Space, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
        h.run_steps(2);
        assert_eq!(layer_doc(&h.state().session, LayerId(lid)).unwrap().text, text, "type_tool {type_tool}: the text is unchanged");
        assert!(h.state().playback.playing, "type_tool {type_tool}: Space plays");
        assert_eq!(type_tool, matches!(h.state().ui.tool, effectcraft_ui_egui::state::Tool::Type), "the tool is kept");
    }
}

/// #831: opening a formatting panel's tab behind Effects & Presets in its group (Properties, in
/// Undocked Panels) keeps the edit and its selected characters: the press on the tab used to
/// focus the shown panel and end the edit, after which Faux Bold styled the whole layer.
#[test]
fn opening_a_formatting_tab_keeps_the_text_selection() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Type", "width": 640, "height": 360, "duration": 2})).unwrap();
    let lid =
        s.execute("layer.newText", json!({"text": "Hello world", "size": 80, "position": [60, 200], "justify": "left"})).unwrap()["layer"].as_u64().unwrap();
    let mut h = harness(EffectcraftApp::new(s));
    h.run_steps(3);
    let ctx = h.ctx.clone();
    // Effects & Presets shown, Properties, Character and Paragraph behind it.
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "window.workspace", json!({"name": "Undocked Panels"})).unwrap();
    h.state_mut().session.execute("text.edit", json!({"layer": lid})).unwrap();
    h.state_mut().session.execute("text.setSelection", json!({"anchor": 0, "caret": 5})).unwrap();
    h.run_steps(3);
    assert!(h.state().auto.find("properties.text.style").is_none(), "Properties is behind Effects & Presets");
    let tab = rect(&h, "panel.tab.Properties").center();
    click(&mut h, tab, Modifiers::NONE);
    assert!(h.state().auto.find("properties.text.style").is_some(), "the tab opened Properties");
    assert_eq!(edited(&h), (lid, "Hello world".into(), 0, 5), "the edit and its selection are kept");
    h.state_mut().show_panel(PanelKind::Character);
    h.run_steps(3);
    let bold = rect(&h, "character.fauxBold").center();
    click(&mut h, bold, Modifiers::NONE);
    let doc = layer_doc(&h.state().session, LayerId(lid)).unwrap();
    assert!(doc.style_at(1).faux_bold && !doc.style_at(7).faux_bold, "only \"Hello\" is bold");
}

/// #831: a click away and keys in one frame's input. The text gets the keys typed before the
/// click and none after it; those later keys don't run as shortcuts either (Delete would remove a
/// layer). From the next frame the keys are the app's: Space plays.
#[test]
fn keys_after_a_click_away_in_the_same_frame_skip_the_text() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Type", "width": 640, "height": 360, "duration": 2})).unwrap();
    let solid = s.execute("layer.newSolid", json!({"name": "Plate", "color": "#406080"})).unwrap()["layer"].as_u64().unwrap();
    let lid = s.execute("layer.newText", json!({"text": "Hello", "size": 80, "position": [60, 200], "justify": "left"})).unwrap()["layer"].as_u64().unwrap();
    let mut h = harness(EffectcraftApp::new(s));
    h.state_mut().show_panel(PanelKind::Composition);
    h.run_steps(3);
    h.state_mut().session.execute("text.edit", json!({"layer": lid, "caret": 5})).unwrap();
    h.run_steps(2);
    let row = rect(&h, &format!("timeline.layer.{solid}.row")).center();
    let key = |key: Key, pressed: bool| Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE };
    h.input_mut().events.extend([
        Event::Text("A".into()),
        Event::PointerMoved(row),
        Event::PointerButton { pos: row, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE },
        Event::PointerButton { pos: row, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE },
        Event::Text("x".into()),
        key(Key::Space, true),
        Event::Text(" ".into()),
        key(Key::Space, false),
        key(Key::Delete, true),
        key(Key::Delete, false),
    ]);
    h.run_steps(3);
    let comp = |h: &Harness<'_, EffectcraftApp>| h.state().session.active_comp().unwrap().layers.len();
    assert_eq!(layer_doc(&h.state().session, LayerId(lid)).unwrap().text, "HelloA", "only the key before the click");
    assert!(h.state().session.state.text_edit.is_none(), "the click ended the edit");
    assert_eq!((comp(&h), h.state().playback.playing), (2, false), "the keys after it ran no shortcut");
    h.input_mut().events.extend([key(Key::Space, true), Event::Text(" ".into()), key(Key::Space, false)]);
    h.run_steps(2);
    assert!(h.state().playback.playing, "Space plays");
    assert_eq!(layer_doc(&h.state().session, LayerId(lid)).unwrap().text, "HelloA");
}

#[test]
fn switching_from_type_to_selection_commits_and_moves_text_layer() {
    let mut h = harness(app());
    h.state_mut().show_panel(PanelKind::Composition);
    h.run_steps(3);
    h.state_mut().ui.tool = effectcraft_ui_egui::state::Tool::Type;
    h.run_steps(1);
    let comp = rect(&h, "viewer.comp");
    let k = comp.width() / 640.0;
    click(&mut h, comp.min + egui::vec2(320.0 * k, 180.0 * k), Modifiers::NONE);
    type_text(&mut h, "Move me");
    let layer_id = h.state().session.state.text_edit.as_ref().unwrap().layer;
    let layer_position = |h: &Harness<'_, EffectcraftApp>| {
        h.state().session.active_comp().unwrap().layer(layer_id).unwrap().props.prop("transform/position").unwrap().value.as_vec3()
    };
    let original_text = layer_doc(&h.state().session, layer_id).unwrap().text;
    let before = layer_position(&h);

    h.state_mut().ui.tool = effectcraft_ui_egui::state::Tool::Selection;
    h.run_steps(2);
    assert!(h.state().session.state.text_edit.is_none(), "leaving the Type tool commits text editing");
    type_text(&mut h, " ignored");
    assert_eq!(layer_doc(&h.state().session, layer_id).unwrap().text, original_text, "Selection does not type into the layer");
    let from = comp.min + egui::vec2(340.0 * k, 150.0 * k);
    let to = from + egui::vec2(60.0 * k, 50.0 * k);
    drag(&mut h, from, to);
    let after = layer_position(&h);
    assert!(after[0] > before[0] && after[1] > before[1], "Selection dragging moves the text layer: {before:?} -> {after:?}");
}

#[test]
fn double_click_enters_editing_and_selects_words() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Type", "width": 640, "height": 360, "duration": 2})).unwrap();
    let lid =
        s.execute("layer.newText", json!({"text": "alpha beta", "size": 80, "position": [100, 200], "justify": "left"})).unwrap()["layer"].as_u64().unwrap();
    let mut h = harness(EffectcraftApp::new(s));
    h.state_mut().show_panel(PanelKind::Composition);
    h.run_steps(3);
    // A point inside "beta" (comp space → window).
    let comp = rect(&h, "viewer.comp");
    let k = comp.width() / 640.0;
    let p = comp.min + egui::vec2(330.0 * k, 180.0 * k);
    click(&mut h, p, Modifiers::NONE);
    click(&mut h, p, Modifiers::NONE);
    let e = h.state().session.state.text_edit.clone().expect("double-click edits text");
    assert_eq!(e.layer.0, lid);
    assert_eq!((e.anchor.min(e.caret), e.anchor.max(e.caret)), (0, 10), "everything selected");
    // Double-clicking inside the edited text selects a word.
    h.run_steps(40);
    click(&mut h, p, Modifiers::NONE);
    click(&mut h, p, Modifiers::NONE);
    let e = h.state().session.state.text_edit.clone().unwrap();
    assert_eq!((e.anchor, e.caret), (6, 10));
    // Typing replaces it; Cmd+Z undoes while editing.
    type_text(&mut h, "gamma");
    assert_eq!(edited(&h).1, "alpha gamma");
    key(&mut h, Key::Z, Modifiers::COMMAND);
    assert_eq!(edited(&h).1, "alpha beta");
    // Paragraph panel: alignment applies to the edited paragraph (the South Asian and Middle
    // Eastern text engine shows the direction popup too).
    h.state_mut().session.prefs.type_.text_engine = "southAsian".into();
    h.state_mut().show_panel(PanelKind::Paragraph);
    h.run_steps(3);
    let r = rect(&h, "paragraph.right");
    click(&mut h, r.center(), Modifiers::NONE);
    let doc = layer_doc(&h.state().session, LayerId(lid)).unwrap();
    assert_eq!(doc.justify, effectcraft_engine::keyframe::Justify::Right);
    for id in
        ["paragraph.indentLeft", "paragraph.spaceBefore", "paragraph.direction", "paragraph.composer", "paragraph.hangingPunctuation", "paragraph.justifyAll"]
    {
        assert!(h.state().auto.find(id).is_some(), "missing {id}");
    }
}

#[test]
fn type_tool_drag_makes_paragraph_text_with_box_handles() {
    let mut h = harness(app());
    h.state_mut().show_panel(PanelKind::Composition);
    h.run_steps(3);
    h.state_mut().ui.tool = effectcraft_ui_egui::state::Tool::Type;
    h.run_steps(1);
    let comp = rect(&h, "viewer.comp");
    let a = comp.min + egui::vec2(comp.width() * 0.2, comp.height() * 0.2);
    let b = comp.min + egui::vec2(comp.width() * 0.7, comp.height() * 0.6);
    h.input_mut().events.push(Event::PointerMoved(a));
    h.input_mut().events.push(Event::PointerButton { pos: a, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.run_steps(1);
    for i in 1..=8 {
        h.input_mut().events.push(Event::PointerMoved(a + (b - a) * (i as f32 / 8.0)));
        h.run_steps(1);
    }
    h.input_mut().events.push(Event::PointerButton { pos: b, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
    let (lid, _, _, _) = edited(&h);
    let doc = layer_doc(&h.state().session, LayerId(lid)).unwrap();
    let bs = doc.box_size.expect("paragraph text");
    assert!((bs[0] - 320.0).abs() < 4.0 && (bs[1] - 144.0).abs() < 4.0, "{bs:?}");
    type_text(&mut h, "Some words that wrap inside the box when there are enough of them");
    h.run_steps(2);
    assert!(h.state().auto.find("viewer.textBox.handle.4").is_some());
    // Dragging the bottom-right handle widens the box.
    let hr = rect(&h, "viewer.textBox.handle.4").center();
    let to = hr + egui::vec2(60.0, 0.0);
    h.input_mut().events.push(Event::PointerMoved(hr));
    h.input_mut().events.push(Event::PointerButton { pos: hr, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.run_steps(1);
    for i in 1..=6 {
        h.input_mut().events.push(Event::PointerMoved(hr + (to - hr) * (i as f32 / 6.0)));
        h.run_steps(1);
    }
    h.input_mut().events.push(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
    let doc2 = layer_doc(&h.state().session, LayerId(lid)).unwrap();
    assert!(doc2.box_size.unwrap()[0] > bs[0] + 20.0, "{:?}", doc2.box_size);
    assert!(h.state().session.state.text_edit.is_some(), "still editing after resizing");
}

/// #866: Layer ▸ New ▸ Text (menu or its shortcut) puts an insertion point in the comp, as After
/// Effects does, so typed keys reach Source Text (and the layer name follows) instead of running
/// shortcuts while the layer keeps the placeholder "Text".
#[test]
fn layer_new_text_menu_and_shortcut_start_typing_into_source_text() {
    for via_shortcut in [false, true] {
        let mut s = Session::default();
        s.execute("comp.new", json!({"name": "Type", "width": 640, "height": 360, "duration": 2})).unwrap();
        s.execute("layer.newSolid", json!({"color": "#ffffff"})).unwrap();
        let mut h = harness(EffectcraftApp::new(s));
        h.state_mut().show_panel(PanelKind::Composition);
        h.run_steps(3);
        if via_shortcut {
            key(&mut h, Key::T, Modifiers::COMMAND | Modifiers::ALT | Modifiers::SHIFT);
        } else {
            let ctx = h.ctx.clone();
            effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "layer.newText", json!({})).unwrap();
            h.run_steps(2);
        }
        let (lid, text, _, _) = edited(&h);
        assert_eq!(text, "", "no placeholder to type over (shortcut: {via_shortcut})");
        type_text(&mut h, "My First Project");
        key(&mut h, Key::Escape, Modifiers::NONE);
        assert!(h.state().session.state.text_edit.is_none(), "Escape commits");
        let s = &h.state().session;
        assert_eq!(layer_doc(s, LayerId(lid)).unwrap().text, "My First Project", "typed text is the Source Text");
        assert_eq!(s.active_comp().unwrap().layer(LayerId(lid)).unwrap().name, "My First Project", "the name follows the text");
    }
}

/// Double-clicking a text layer in the Timeline, on its name or on its bar, edits its text in the
/// Composition viewer with the whole text selected, as in After Effects (Enter renames).
#[test]
fn timeline_double_click_on_a_text_layer_edits_its_text() {
    for target in ["row", "bar"] {
        let mut s = Session::default();
        s.execute("comp.new", json!({"name": "Type", "width": 640, "height": 360, "duration": 2})).unwrap();
        let lid = s.execute("layer.newText", json!({"text": "alpha beta"})).unwrap()["layer"].as_u64().unwrap();
        let mut h = harness(EffectcraftApp::new(s));
        h.state_mut().show_panel(PanelKind::Composition);
        h.run_steps(3);
        let p = rect(&h, &format!("timeline.layer.{lid}.{target}")).center();
        click(&mut h, p, Modifiers::NONE);
        click(&mut h, p, Modifiers::NONE);
        let (layer, text, anchor, caret) = edited(&h);
        assert_eq!(layer, lid, "{target}");
        assert_eq!((anchor.min(caret), anchor.max(caret)), (0, 10), "everything selected ({target})");
        assert!(h.state().auto.find(&format!("timeline.layer.{lid}.nameEdit")).is_none(), "no rename field ({target})");
        assert_eq!(text, "alpha beta");
        type_text(&mut h, "gamma");
        key(&mut h, Key::Escape, Modifiers::NONE);
        assert!(h.state().session.state.text_edit.is_none(), "Escape commits ({target})");
        assert_eq!(layer_doc(&h.state().session, LayerId(lid)).unwrap().text, "gamma", "typing replaced the text ({target})");
    }
}

#[test]
fn duplicated_text_boxes_both_remain_editable() {
    let mut a = app();
    let original = a
        .session
        .execute("layer.newText", json!({"text": "alpha beta", "size": 40, "position": [40, 100], "justify": "left", "box": [40, 60, 250, 100]}))
        .unwrap()["layer"]
        .as_u64()
        .unwrap();
    let mut h = harness(a);
    h.run_steps(3);
    h.state_mut().ui.focused = PanelKind::Composition;
    key(&mut h, Key::D, Modifiers::COMMAND);
    let duplicate = h.state().session.state.selected_layers[0].0;
    assert_ne!(original, duplicate);
    h.state_mut().session.execute("prop.set", json!({"layer": duplicate, "path": "transform/position", "value": [465, 110]})).unwrap();
    h.run_steps(3);
    for (layer, x, text) in [(original, 80.0, "original edited"), (duplicate, 380.0, "duplicate edited")] {
        h.run_steps(40);
        let comp = rect(&h, "viewer.comp");
        let k = comp.width() / 640.0;
        let p = comp.min + egui::vec2(x * k, 85.0 * k);
        click(&mut h, p, Modifiers::NONE);
        click(&mut h, p, Modifiers::NONE);
        assert_eq!(edited(&h).0, layer);
        type_text(&mut h, text);
        key(&mut h, Key::Escape, Modifiers::NONE);
        assert_eq!(layer_doc(&h.state().session, LayerId(layer)).unwrap().text, text);
    }
}

#[test]
fn overlapping_duplicate_text_remains_editable_by_double_click_and_selected_type_tool() {
    let mut a = app();
    let original = a.session.execute("layer.newText", json!({"text": "alpha beta", "size": 40, "box": [40, 60, 250, 100]})).unwrap()["layer"].as_u64().unwrap();
    let mut h = harness(a);
    h.run_steps(3);
    h.state_mut().ui.focused = PanelKind::Composition;
    key(&mut h, Key::D, Modifiers::COMMAND);
    let duplicate = h.state().session.state.selected_layers[0].0;
    assert_ne!(original, duplicate);
    let comp = rect(&h, "viewer.comp");
    let p = comp.min + egui::vec2(80.0, 85.0) * (comp.width() / 640.0);
    click(&mut h, p, Modifiers::NONE);
    click(&mut h, p, Modifiers::NONE);
    assert_eq!(edited(&h).0, duplicate, "double-click edits the topmost overlapping copy");
    type_text(&mut h, "copy edited");
    assert_eq!(edited(&h).1, "copy edited");
    key(&mut h, Key::Z, Modifiers::COMMAND);
    assert_eq!(edited(&h).1, "alpha beta", "undo restores the duplicate's text");
    key(&mut h, Key::Escape, Modifiers::NONE);
    h.state_mut().session.execute("layer.select", json!({"layers": [original]})).unwrap();
    h.state_mut().ui.focused = PanelKind::Composition;
    key(&mut h, Key::T, Modifiers::COMMAND);
    h.run_steps(40);
    click(&mut h, p, Modifiers::NONE);
    assert_eq!(edited(&h).0, original, "the Type tool must honor the explicitly selected overlapping text layer");
    key(&mut h, Key::A, Modifiers::COMMAND);
    type_text(&mut h, "original edited");
    assert_eq!(edited(&h).1, "original edited");
    assert_eq!(layer_doc(&h.state().session, LayerId(duplicate)).unwrap().text, "alpha beta", "editing the original leaves the duplicate unchanged");
    key(&mut h, Key::Z, Modifiers::COMMAND);
    assert_eq!(edited(&h).1, "alpha beta");
}

#[test]
fn type_tool_selected_text_preference_skips_ineligible_layers_and_falls_back_to_topmost() {
    for exclusion in ["hidden", "locked", "inactive", "unsoloed", "outside", "unselected"] {
        let mut a = app();
        let mut ids = vec![];
        for _ in 0..2 {
            ids.push(
                a.session.execute("layer.newText", json!({"text": "alpha beta", "size": 40, "box": [40, 60, 250, 100]})).unwrap()["layer"].as_u64().unwrap(),
            );
        }
        let original = ids[0];
        let topmost = ids[1];
        match exclusion {
            "hidden" | "locked" => {
                let switch = if exclusion == "hidden" { "video" } else { "locked" };
                a.session.execute("layer.setSwitch", json!({"layers": [original], "switch": switch, "value": exclusion == "locked"})).unwrap();
            }
            "inactive" => {
                let cid = a.session.active_comp_id().unwrap();
                std::sync::Arc::make_mut(&mut a.session.project).comp_mut(cid).unwrap().layer_mut(LayerId(original)).unwrap().out_point =
                    effectcraft_engine::time::Tick::ZERO;
            }
            "unsoloed" => {
                a.session.execute("layer.setSwitch", json!({"layers": [topmost], "switch": "solo", "value": true})).unwrap();
            }
            "outside" => {
                a.session.execute("prop.set", json!({"layer": original, "path": "transform/position", "value": [500, 300]})).unwrap();
            }
            _ => {}
        }
        a.session.state.selected_layers = if exclusion == "unselected" { vec![] } else { vec![LayerId(original)] };
        a.ui.tool = effectcraft_ui_egui::state::Tool::Type;
        let mut h = harness(a);
        h.run_steps(3);
        let comp = rect(&h, "viewer.comp");
        let p = comp.min + egui::vec2(80.0, 85.0) * (comp.width() / 640.0);
        click(&mut h, p, Modifiers::NONE);
        assert_eq!(edited(&h).0, topmost, "{exclusion} selected text must not override the eligible hit");
    }
}

/// Opens a fresh Type-tool edit holding `text` and returns the harness with the new layer id,
/// so the Deselect All tests start from the same state.
fn type_tool_edit(a: EffectcraftApp, text: &str) -> (Harness<'static, EffectcraftApp>, u64) {
    let mut h = harness(a);
    h.run_steps(3);
    h.state_mut().ui.tool = effectcraft_ui_egui::state::Tool::Type;
    h.run_steps(1);
    let comp = rect(&h, "viewer.comp");
    let p = comp.min + egui::vec2(180.0, 160.0) * (comp.width() / 640.0);
    click(&mut h, p, Modifiers::NONE);
    type_text(&mut h, text);
    let (lid, _, _, _) = edited(&h);
    (h, lid)
}

#[test]
fn deselect_all_chords_end_a_type_tool_edit() {
    for (name, k, mods) in [("Cmd/Ctrl+Shift+A", Key::A, Modifiers { shift: true, ..Modifiers::COMMAND }), ("F2", Key::F2, Modifiers::NONE)] {
        let (mut h, lid) = type_tool_edit(app(), "Hello");
        let (_, text, _, _) = edited(&h);
        assert_eq!(text, "Hello", "{name}: the Type tool click and typing made a layer");
        key(&mut h, k, mods);
        assert!(h.state().session.state.text_edit.is_none(), "{name}: Deselect All ends the edit");
        assert!(h.state().session.state.selected_layers.is_empty(), "{name}: and deselects the layer");
        assert_eq!(layer_doc(&h.state().session, LayerId(lid)).unwrap().text, "Hello", "{name}: the typed text is kept");
    }
}

#[test]
fn cmd_a_in_a_text_edit_still_selects_all_characters() {
    let (mut h, _lid) = type_tool_edit(app(), "Hello");
    key(&mut h, Key::A, Modifiers::COMMAND);
    let (_, _, a, c) = edited(&h);
    assert_eq!((a, c), (0, 5), "Select All keeps selecting every character");
    assert!(h.state().session.state.text_edit.is_some(), "the edit stays open");
    assert_eq!(h.state().session.state.selected_layers.len(), 1, "and the layer stays selected");
}

#[test]
fn rebound_deselect_all_chord_forwards_from_inside_the_edit() {
    let mut a = app();
    a.session.execute("shortcuts.set", json!({"command": "edit.deselectAll", "keys": ["Cmd+Alt+D"]})).unwrap();
    let (mut h, lid) = type_tool_edit(a, "Hello");
    // The chord belongs to the shortcut, not to a hard-coded key: the old defaults do
    // nothing special while the editor holds the keyboard…
    key(&mut h, Key::A, Modifiers { shift: true, ..Modifiers::COMMAND });
    assert!(h.state().session.state.text_edit.is_some(), "Cmd+Shift+A was rebound away");
    let (_, _, a, c) = edited(&h);
    assert_eq!((a, c), (5, 5), "and Cmd+Shift+A no longer selects all characters either");
    key(&mut h, Key::F2, Modifiers::NONE);
    assert!(h.state().session.state.text_edit.is_some(), "nor does F2");
    // …and the current binding is what ends the edit.
    key(&mut h, Key::D, Modifiers { alt: true, ..Modifiers::COMMAND });
    assert!(h.state().session.state.text_edit.is_none(), "the rebound chord ends the edit");
    assert!(h.state().session.state.selected_layers.is_empty(), "and deselects");
    assert_eq!(layer_doc(&h.state().session, LayerId(lid)).unwrap().text, "Hello", "the text is kept");
}

#[test]
fn deselect_all_rebound_onto_an_alt_variant_of_an_editor_chord_forwards() {
    let alt_cmd = |k| (k, Modifiers { alt: true, ..Modifiers::COMMAND });
    for (bound, (k, mods)) in [("Cmd+Alt+Z", alt_cmd(Key::Z)), ("Cmd+Alt+A", alt_cmd(Key::A))] {
        let mut a = app();
        a.session.execute("shortcuts.set", json!({"command": "edit.deselectAll", "keys": [bound]})).unwrap();
        let (mut h, lid) = type_tool_edit(a, "Hello");
        // The editor owns Cmd+A and Cmd+Z themselves, whatever Deselect All is bound to…
        key(&mut h, Key::A, Modifiers::COMMAND);
        let (_, _, a0, c0) = edited(&h);
        assert_eq!((a0, c0), (0, 5), "{bound}: Cmd+A still selects every character");
        type_text(&mut h, "Hi");
        key(&mut h, Key::Z, Modifiers::COMMAND);
        assert_eq!(edited(&h).1, "Hello", "{bound}: Cmd+Z still undoes the typing");
        // …but not their Alt variants, which belong to the rebound Deselect All.
        key(&mut h, k, mods);
        assert!(h.state().session.state.text_edit.is_none(), "{bound}: the rebound chord ends the edit");
        assert!(h.state().session.state.selected_layers.is_empty(), "{bound}: and deselects");
        assert_eq!(layer_doc(&h.state().session, LayerId(lid)).unwrap().text, "Hello", "{bound}: without undoing or selecting text");
    }
}

#[test]
fn bare_letter_rebinding_stays_typing_but_a_function_key_rebinding_forwards() {
    let mut a = app();
    a.session.execute("shortcuts.set", json!({"command": "edit.deselectAll", "keys": ["A"]})).unwrap();
    let (mut h, lid) = type_tool_edit(a, "Hello");
    // A binding without a command modifier is text as far as the editor is concerned: the
    // keystroke belongs to the text, since a command may not steal a key that types
    // (the app-wide dispatcher honors the same invariant by skipping focused text fields).
    key(&mut h, Key::A, Modifiers::NONE);
    assert!(h.state().session.state.text_edit.is_some(), "a bare-letter Deselect All binding does not end the edit");
    type_text(&mut h, "a");
    let (lid2, _, a0, c0) = edited(&h);
    assert_eq!(lid2, lid, "the same edit on the same layer continues");
    assert_eq!((a0, c0), (6, 6), "the letter typed at the caret");
    assert_eq!(layer_doc(&h.state().session, LayerId(lid)).unwrap().text, "Helloa", "and landed in the text");
    // A function key types nothing, so a modifier-free function-key binding is forwarded
    // (the default F2 relies on this).
    h.state_mut().session.execute("shortcuts.set", json!({"command": "edit.deselectAll", "keys": ["F7"]})).unwrap();
    key(&mut h, Key::F7, Modifiers::NONE);
    assert!(h.state().session.state.text_edit.is_none(), "a bare function-key binding ends the edit");
    assert!(h.state().session.state.selected_layers.is_empty(), "and deselects");
    assert_eq!(layer_doc(&h.state().session, LayerId(lid)).unwrap().text, "Helloa", "the text is kept");
}

/// Opens a text edit the way a user does from the Project panel — Layer ▸ New ▸ Text's shortcut
/// while that panel is focused — so the insertion point lives in the comp while focus stays on
/// Project. Returns the harness and the new layer id.
fn project_focused_text_edit(text: &str) -> (Harness<'static, EffectcraftApp>, u64) {
    let mut h = harness(app());
    h.run_steps(3);
    h.state_mut().ui.focused = PanelKind::Project;
    h.run_steps(1);
    key(&mut h, Key::T, Modifiers { alt: true, shift: true, ..Modifiers::COMMAND });
    type_text(&mut h, text);
    let (lid, typed, _, _) = edited(&h);
    assert_eq!(typed, text, "typing went into the new layer");
    assert_eq!(h.state().ui.focused, PanelKind::Project, "the Project panel is still the focused panel");
    (h, lid)
}

#[test]
fn deselect_all_chords_end_an_edit_started_while_the_project_panel_is_focused() {
    for (name, k, mods) in [("Cmd/Ctrl+Shift+A", Key::A, Modifiers { shift: true, ..Modifiers::COMMAND }), ("F2", Key::F2, Modifiers::NONE)] {
        let (mut h, lid) = project_focused_text_edit("Hi");
        key(&mut h, k, mods);
        assert!(h.state().session.state.text_edit.is_none(), "{name}: Deselect All ends the edit");
        assert!(h.state().session.state.selected_layers.is_empty(), "{name}: and deselects the layer");
        assert_eq!(layer_doc(&h.state().session, LayerId(lid)).unwrap().text, "Hi", "{name}: the typed text is kept");
    }
}

#[test]
fn deselect_all_menu_item_follows_the_edit_while_the_project_panel_is_focused() {
    let enabled = |h: &Harness<'_, EffectcraftApp>| {
        effectcraft_ui_egui::menus::menu_items(h.state()).into_iter().find(|m| m.id == "edit.deselectAll").is_some_and(|m| m.enabled)
    };
    let (mut h, _) = project_focused_text_edit("Hi");
    h.state_mut().session.execute("project.select", json!({"items": []})).unwrap();
    h.run_steps(1);
    assert!(h.state().session.state.text_edit.is_some(), "the edit is still open");
    // The item ends the edit, so an empty Project selection must not gray it out.
    assert!(enabled(&h), "Deselect All is enabled during the edit");
    key(&mut h, Key::Escape, Modifiers::NONE);
    assert!(h.state().session.state.text_edit.is_none(), "Escape ended the edit");
    assert_eq!(h.state().ui.focused, PanelKind::Project, "the Project panel is still the focused panel");
    // With no edit, the item deselects Project items again: nothing selected there, nothing to do.
    assert!(!enabled(&h), "Deselect All follows the empty Project selection once the edit is over");
}

/// The on-canvas editor's outgoing IME producer: the descriptor carries the edited layer's
/// committed text, the real owner, a stable generation and an explicit change cause. A normal
/// typed character is application work; an accepted input-method commit is not.
#[test]
fn ime_producer_follows_the_committed_text_and_cause() {
    // A Latin accent is multi-byte in UTF-8 (the byte offsets below) and Inter draws it, so the
    // harness keeps its strict missing-glyph check; no CJK fallback face is needed headlessly.
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_step_dt(1.0 / 60.0).build_eframe(|_| app());
    h.state_mut().show_panel(PanelKind::Composition);
    h.run_steps(3);
    h.state_mut().ui.tool = effectcraft_ui_egui::state::Tool::Type;
    h.run_steps(1);
    let comp = rect(&h, "viewer.comp");
    click(&mut h, comp.center(), Modifiers::NONE);
    type_text(&mut h, "Hi");
    let (lid, text, a, c) = edited(&h);
    assert_eq!((text.as_str(), a, c), ("Hi", 2, 2));

    let ime = h.output().platform_output.ime.clone().expect("the on-canvas editor owns IME");
    let comp_id = h.state().session.active_comp_id().expect("an active comp").0;
    assert_eq!(ime.owner, egui::Id::unique(("effectcraft-ime-text", comp_id, lid)), "the edited comp and layer");
    assert!(ime.context_generation > 0);
    let p = ime.surrounding_text.available().expect("committed text is available");
    assert_eq!((p.text.as_str(), p.cursor, p.anchor), ("Hi", 2, 2));
    assert_eq!(ime.change_cause, egui::ImeChangeCause::Other, "ordinary typing is application work");
    let generation = ime.context_generation;

    h.input_mut().events.push(Event::Ime(egui::ImeEvent::Commit("é".into())));
    h.run_steps(1);
    assert_eq!(edited(&h).1, "Hié");
    let ime = h.output().platform_output.ime.clone().expect("still editing");
    let p = ime.surrounding_text.available().expect("committed text is available");
    assert_eq!((p.text.as_str(), p.cursor, p.anchor), ("Hié", 4, 4), "UTF-8 bytes after the commit");
    assert_eq!(ime.change_cause, egui::ImeChangeCause::InputMethod, "an accepted IME commit earns it");
    assert_eq!(ime.context_generation, generation, "an ordinary edit keeps one context");
}

/// A click on the Timeline is processed after the viewer overlay drew. The pass must not leave
/// the ended on-canvas editor's old descriptor in the platform output.
#[test]
fn click_away_clears_the_canvas_ime_descriptor_in_the_final_pass() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Type", "width": 640, "height": 360, "duration": 2})).unwrap();
    let solid = s.execute("layer.newSolid", json!({"name": "Plate", "color": "#406080"})).unwrap()["layer"].as_u64().unwrap();
    let title = s.execute("layer.newText", json!({"text": "Title", "size": 80, "position": [100, 120], "justify": "left"})).unwrap()["layer"].as_u64().unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_step_dt(1.0 / 60.0).build_eframe(move |_| EffectcraftApp::new(s));
    h.state_mut().show_panel(PanelKind::Composition);
    h.run_steps(3);
    h.state_mut().session.execute("text.edit", json!({"layer": title, "caret": 5})).unwrap();
    h.run_steps(2);
    assert!(h.output().platform_output.ime.is_some(), "the editor owns IME before the click");
    let row = rect(&h, &format!("timeline.layer.{solid}.row")).center();
    click(&mut h, row, Modifiers::NONE);
    assert!(h.state().session.state.text_edit.is_none(), "the click ends the edit");
    assert!(h.output().platform_output.ime.is_none(), "the final pass clears the ended editor's descriptor");
}

/// A new target on the same focused widget must request a composition interrupt, and a delayed
/// commit carrying the previous composition's preview must not reach the new layer.
#[test]
fn switching_text_targets_interrupts_and_rejects_a_delayed_commit() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Type", "width": 640, "height": 360, "duration": 2})).unwrap();
    let first = s.execute("layer.newText", json!({"text": "A", "size": 80, "position": [80, 120], "justify": "left"})).unwrap()["layer"].as_u64().unwrap();
    let second = s.execute("layer.newText", json!({"text": "B", "size": 80, "position": [80, 240], "justify": "left"})).unwrap()["layer"].as_u64().unwrap();
    // The delayed commit carries a multi-byte accent Inter draws: no broad missing-glyph
    // allowance, and the stale-composition path is still exercised with real IME events.
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_step_dt(1.0 / 60.0).build_eframe(move |_| EffectcraftApp::new(s));
    h.state_mut().show_panel(PanelKind::Composition);
    h.run_steps(3);
    h.state_mut().session.execute("text.edit", json!({"layer": first, "caret": 1})).unwrap();
    h.run_steps(2);
    h.input_mut().events.push(Event::Ime(egui::ImeEvent::Preedit { text: "é".into(), active_range_chars: None }));
    h.run_steps(1);

    h.state_mut().session.execute("text.edit", json!({"layer": second, "caret": 1})).unwrap();
    h.run_steps(1);
    let comp_id = h.state().session.active_comp_id().expect("comp").0;
    let ime = h.output().platform_output.ime.clone().expect("the second editor owns IME");
    assert_eq!(ime.owner, egui::Id::unique(("effectcraft-ime-text", comp_id, second)));
    assert!(ime.should_interrupt_composition, "the session switch cancels the platform composition");

    // The delayed batch may update the old composition before it commits. That preview must not
    // re-arm the stale guard for the new layer.
    h.input_mut().events.push(Event::Ime(egui::ImeEvent::Preedit { text: "é".into(), active_range_chars: None }));
    h.input_mut().events.push(Event::Ime(egui::ImeEvent::Commit("é".into())));
    h.run_steps(2);
    assert_eq!(layer_doc(&h.state().session, LayerId(first)).unwrap().text, "A");
    assert_eq!(layer_doc(&h.state().session, LayerId(second)).unwrap().text, "B", "the stale commit is ignored");
}

/// A stale pre-edit must not re-arm the guard for a following delete-surrounding/commit batch
/// either: deleting around the new layer's caret would otherwise mutate it without a commit.
#[test]
fn a_delayed_preedit_after_a_target_switch_cannot_rearm_the_stale_guard() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Type", "width": 640, "height": 360, "duration": 2})).unwrap();
    let first = s.execute("layer.newText", json!({"text": "A", "size": 80, "position": [80, 120], "justify": "left"})).unwrap()["layer"].as_u64().unwrap();
    let second = s.execute("layer.newText", json!({"text": "B", "size": 80, "position": [80, 240], "justify": "left"})).unwrap()["layer"].as_u64().unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_step_dt(1.0 / 60.0).build_eframe(move |_| EffectcraftApp::new(s));
    h.state_mut().show_panel(PanelKind::Composition);
    h.run_steps(3);
    h.state_mut().session.execute("text.edit", json!({"layer": first, "caret": 1})).unwrap();
    h.run_steps(2);
    h.input_mut().events.push(Event::Ime(egui::ImeEvent::Preedit { text: "é".into(), active_range_chars: None }));
    h.run_steps(1);

    h.state_mut().session.execute("text.edit", json!({"layer": second, "caret": 1})).unwrap();
    h.run_steps(1);
    h.input_mut().events.push(Event::Ime(egui::ImeEvent::Preedit { text: "é".into(), active_range_chars: None }));
    h.input_mut().events.push(Event::Ime(egui::ImeEvent::DeleteSurroundingBytes { before_bytes: 1, after_bytes: 0 }));
    h.input_mut().events.push(Event::Ime(egui::ImeEvent::Commit("X".into())));
    h.run_steps(2);
    assert_eq!(layer_doc(&h.state().session, LayerId(first)).unwrap().text, "A");
    assert_eq!(layer_doc(&h.state().session, LayerId(second)).unwrap().text, "B", "the stale pre-edit cannot re-arm a delete/commit batch");
}

/// A late pre-edit from the old composition can arrive long after the target switch; timing is
/// not provenance. Protection survives idle frames and old terminals until the matching fence.
#[test]
fn late_preedit_and_commit_split_across_idle_frames_cannot_retarget() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Type", "width": 640, "height": 360, "duration": 2})).unwrap();
    let first = s.execute("layer.newText", json!({"text": "A", "size": 80, "position": [80, 120], "justify": "left"})).unwrap()["layer"].as_u64().unwrap();
    let second = s.execute("layer.newText", json!({"text": "B", "size": 80, "position": [80, 240], "justify": "left"})).unwrap()["layer"].as_u64().unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_step_dt(1.0 / 60.0).build_eframe(move |_| EffectcraftApp::new(s));
    h.state_mut().show_panel(PanelKind::Composition);
    h.run_steps(3);
    h.state_mut().session.execute("text.edit", json!({"layer": first, "caret": 1})).unwrap();
    h.run_steps(2);
    h.input_mut().events.push(Event::Ime(egui::ImeEvent::Preedit { text: "é".into(), active_range_chars: None }));
    h.run_steps(1);

    h.state_mut().session.execute("text.edit", json!({"layer": second, "caret": 1})).unwrap();
    // Well past the old two-frame suppression window: ownership, not elapsed frames, keeps the
    // tombstone.
    h.run_steps(1);
    let request_id = ime_reset_request(&h);
    h.run_steps(4);
    h.input_mut().events.push(Event::Ime(egui::ImeEvent::Preedit { text: "éé".into(), active_range_chars: None }));
    h.run_steps(3);
    h.input_mut().events.push(Event::Ime(egui::ImeEvent::Commit("é".into())));
    h.run_steps(2);
    assert_eq!(layer_doc(&h.state().session, LayerId(first)).unwrap().text, "A");
    assert_eq!(layer_doc(&h.state().session, LayerId(second)).unwrap().text, "B", "the late pre-edit and commit stayed on the old owner");

    // An actual matching fence admits the fresh composition, without an invented old terminal.
    h.input_mut().events.push(Event::ImeResetCompleted { request_id });
    h.input_mut().events.push(Event::Ime(egui::ImeEvent::Preedit { text: "é".into(), active_range_chars: None }));
    h.input_mut().events.push(Event::Ime(egui::ImeEvent::Commit("é".into())));
    h.run_steps(2);
    assert_eq!(layer_doc(&h.state().session, LayerId(second)).unwrap().text, "Bé", "a new composition after its matching fence is accepted");
}

/// A delete-surrounding event belongs to the same stale composition as its late pre-edit even
/// when idle frames separate them. It must not mutate the replacement layer; only F admits fresh
/// input, including when no old terminal commit arrives.
#[test]
fn late_preedit_and_delete_split_across_idle_frames_cannot_retarget() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Type", "width": 640, "height": 360, "duration": 2})).unwrap();
    let first = s.execute("layer.newText", json!({"text": "A", "size": 80, "position": [80, 120], "justify": "left"})).unwrap()["layer"].as_u64().unwrap();
    let second = s.execute("layer.newText", json!({"text": "B", "size": 80, "position": [80, 240], "justify": "left"})).unwrap()["layer"].as_u64().unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_step_dt(1.0 / 60.0).build_eframe(move |_| EffectcraftApp::new(s));
    h.state_mut().show_panel(PanelKind::Composition);
    h.run_steps(3);
    h.state_mut().session.execute("text.edit", json!({"layer": first, "caret": 1})).unwrap();
    h.run_steps(2);
    h.input_mut().events.push(Event::Ime(egui::ImeEvent::Preedit { text: "é".into(), active_range_chars: None }));
    h.run_steps(1);

    h.state_mut().session.execute("text.edit", json!({"layer": second, "caret": 1})).unwrap();
    h.run_steps(1);
    let request_id = ime_reset_request(&h);
    h.run_steps(3);
    h.input_mut().events.push(Event::Ime(egui::ImeEvent::Preedit { text: "éé".into(), active_range_chars: None }));
    h.run_steps(3);
    h.input_mut().events.push(Event::Ime(egui::ImeEvent::DeleteSurroundingBytes { before_bytes: 1, after_bytes: 0 }));
    h.run_steps(2);
    assert_eq!(layer_doc(&h.state().session, LayerId(first)).unwrap().text, "A");
    assert_eq!(layer_doc(&h.state().session, LayerId(second)).unwrap().text, "B", "the late delete could not mutate the new owner");

    h.input_mut().events.push(Event::ImeResetCompleted { request_id });
    h.input_mut().events.push(Event::Ime(egui::ImeEvent::Preedit { text: "é".into(), active_range_chars: None }));
    h.input_mut().events.push(Event::Ime(egui::ImeEvent::Commit("é".into())));
    h.run_steps(2);
    assert_eq!(layer_doc(&h.state().session, LayerId(second)).unwrap().text, "Bé", "the matching fence admits fresh input");
}

/// An observed empty pre-edit only ends the visible composition; its terminal commit is a
/// separate event. Switching targets in between must keep the old owner so that delayed commit is
/// rejected instead of landing at the replacement layer's caret.
#[test]
fn empty_preedit_keeps_the_old_target_until_the_pending_commit_arrives() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Type", "width": 640, "height": 360, "duration": 2})).unwrap();
    let first = s.execute("layer.newText", json!({"text": "A", "size": 80, "position": [80, 120], "justify": "left"})).unwrap()["layer"].as_u64().unwrap();
    let second = s.execute("layer.newText", json!({"text": "B", "size": 80, "position": [80, 240], "justify": "left"})).unwrap()["layer"].as_u64().unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_step_dt(1.0 / 60.0).build_eframe(move |_| EffectcraftApp::new(s));
    h.state_mut().show_panel(PanelKind::Composition);
    h.run_steps(3);
    h.state_mut().session.execute("text.edit", json!({"layer": first, "caret": 1})).unwrap();
    h.run_steps(2);
    h.input_mut().events.push(Event::Ime(egui::ImeEvent::Preedit { text: "é".into(), active_range_chars: None }));
    h.run_steps(1);

    // The input method clears the visible composition before its commit arrives.
    h.input_mut().events.push(Event::Ime(egui::ImeEvent::Preedit { text: String::new(), active_range_chars: None }));
    h.run_steps(1);
    h.state_mut().session.execute("text.edit", json!({"layer": second, "caret": 1})).unwrap();
    h.run_steps(1);

    h.input_mut().events.push(Event::Ime(egui::ImeEvent::Commit("X".into())));
    h.run_steps(2);
    assert_eq!(layer_doc(&h.state().session, LayerId(first)).unwrap().text, "A");
    assert_eq!(layer_doc(&h.state().session, LayerId(second)).unwrap().text, "B", "the observed empty pre-edit preserved the old owner for its pending commit",);
}

/// A coalesced raw batch after a target switch may carry the old composition's empty pre-edit and
/// terminal commit followed by a fence and the first pre-edit of a new composition. Only that
/// explicit boundary admits the new pre-edit for the current target.
#[test]
fn a_matching_fence_after_old_terminals_admits_the_next_preedit_in_the_same_batch() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Type", "width": 640, "height": 360, "duration": 2})).unwrap();
    let first = s.execute("layer.newText", json!({"text": "A", "size": 80, "position": [80, 120], "justify": "left"})).unwrap()["layer"].as_u64().unwrap();
    let second = s.execute("layer.newText", json!({"text": "B", "size": 80, "position": [80, 240], "justify": "left"})).unwrap()["layer"].as_u64().unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_step_dt(1.0 / 60.0).build_eframe(move |_| EffectcraftApp::new(s));
    h.state_mut().show_panel(PanelKind::Composition);
    h.run_steps(3);
    h.state_mut().session.execute("text.edit", json!({"layer": first, "caret": 1})).unwrap();
    h.run_steps(2);
    h.input_mut().events.push(Event::Ime(egui::ImeEvent::Preedit { text: "é".into(), active_range_chars: None }));
    h.run_steps(1);

    h.state_mut().session.execute("text.edit", json!({"layer": second, "caret": 1})).unwrap();
    h.run_steps(1);
    let request_id = ime_reset_request(&h);
    h.input_mut().events.push(Event::Ime(egui::ImeEvent::Preedit { text: String::new(), active_range_chars: None }));
    h.input_mut().events.push(Event::Ime(egui::ImeEvent::Commit("X".into())));
    h.input_mut().events.push(Event::ImeResetCompleted { request_id });
    h.input_mut().events.push(Event::Ime(egui::ImeEvent::Preedit { text: "é".into(), active_range_chars: None }));
    h.run_steps(1);
    assert_eq!(layer_doc(&h.state().session, LayerId(second)).unwrap().text, "B", "the stale empty pre-edit/commit pair never mutates the new layer",);

    h.input_mut().events.push(Event::Ime(egui::ImeEvent::Commit("é".into())));
    h.run_steps(2);
    assert_eq!(layer_doc(&h.state().session, LayerId(first)).unwrap().text, "A");
    assert_eq!(layer_doc(&h.state().session, LayerId(second)).unwrap().text, "Bé", "the new pre-edit after the matching fence owns the current target",);
}

/// The byte-count delete-surrounding path through the real app/hook keeps a whole UTF-8
/// character and shares the following commit's undo step.
#[test]
fn delete_surrounding_bytes_through_the_app_replaces_the_emoji() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Type", "width": 640, "height": 360, "duration": 2})).unwrap();
    let lid = s.execute("layer.newText", json!({"text": "café😀", "size": 80, "position": [80, 120], "justify": "left"})).unwrap()["layer"].as_u64().unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_step_dt(1.0 / 60.0).build_eframe(move |_| EffectcraftApp::new(s));
    h.state_mut().show_panel(PanelKind::Composition);
    h.run_steps(3);
    h.state_mut().session.execute("text.edit", json!({"layer": lid, "caret": 5})).unwrap();
    h.run_steps(2);
    h.input_mut().events.push(Event::Ime(egui::ImeEvent::DeleteSurroundingBytes { before_bytes: 4, after_bytes: 0 }));
    h.input_mut().events.push(Event::Ime(egui::ImeEvent::Commit("X".into())));
    h.run_steps(2);
    assert_eq!(layer_doc(&h.state().session, LayerId(lid)).unwrap().text, "caféX");
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(layer_doc(&h.state().session, LayerId(lid)).unwrap().text, "café😀", "the delete and commit are one undo step");
}

/// Use the real viewer's outgoing caret geometry to aim the interruption at a known position.
fn ime_interruption_editor() -> (Harness<'static, EffectcraftApp>, Pos2) {
    crate::add_canonical_test_fonts();
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Type", "width": 640, "height": 360, "duration": 2})).unwrap();
    let lid = s.execute("layer.newText", json!({"text": "abcd", "size": 80, "position": [80, 120], "justify": "left"})).unwrap()["layer"].as_u64().unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_step_dt(1.0 / 60.0).build_eframe(move |cc| {
        EffectcraftApp::install_ime_reset_hooks(&cc.egui_ctx);
        EffectcraftApp::new(s)
    });
    h.state_mut().show_panel(PanelKind::Composition);
    h.state_mut().ui.tool = effectcraft_ui_egui::state::Tool::Type;
    h.run_steps(3);
    h.state_mut().session.execute("text.edit", json!({"layer": lid, "caret": 1})).unwrap();
    h.run_steps(2);
    let point = h.output().platform_output.ime.as_ref().expect("the viewer drew caret1").cursor_rect.center();
    assert!(rect(&h, "viewer.textEdit").contains(point), "the pointer must hit the actual edited text");
    h.state_mut().session.execute("text.setSelection", json!({"caret": 4})).unwrap();
    h.run_steps(2);
    assert_eq!((edited(&h).2, edited(&h).3), (4, 4));
    (h, point)
}

/// Real viewer geometry and raw eframe input, without the split-frame `click` helper.
fn native_text_pointer_editor() -> (Harness<'static, EffectcraftApp>, u64, Pos2, Pos2) {
    let (h, caret1) = ime_interruption_editor();
    let caret4 = h.output().platform_output.ime.as_ref().expect("the viewer drew caret4").cursor_rect.center();
    let area = rect(&h, "viewer.area");
    assert!(caret1.is_finite() && caret4.is_finite());
    assert!(area.contains(caret1) && area.contains(caret4));
    assert!(rect(&h, "viewer.textEdit").contains(caret4));
    assert!(caret1.distance(caret4) > 12.0, "separated clicks must not become a double-click");
    let layer = edited(&h).0;
    (h, layer, caret1, caret4)
}

#[test]
fn native_text_pointer_same_batch_click_moves_caret_and_starts_editing() {
    for active in [true, false] {
        let (mut h, layer, caret1, caret4) = native_text_pointer_editor();
        if !active {
            h.state_mut().session.execute("text.endEdit", json!({})).unwrap();
            h.run_steps(2);
            assert!(h.state().session.state.text_edit.is_none());
        }
        let undo = h.state().session.history.undo.len();
        h.input_mut().events.extend([
            Event::PointerMoved(caret1),
            Event::PointerButton { pos: caret1, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE },
            Event::PointerButton { pos: caret1, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE },
        ]);
        h.run_steps(1);
        assert_eq!(edited(&h), (layer, "abcd".to_owned(), 1, 1), "active={active}");
        assert!(!h.ctx.input(|i| i.pointer.primary_down()));
        assert_eq!(h.state().session.history.undo.len(), undo, "placing a caret is not a document edit");
        h.input_mut().events.push(Event::PointerMoved(caret4));
        h.run_steps(1);
        assert_eq!((edited(&h).2, edited(&h).3), (1, 1), "release must end selection dragging");
        type_text(&mut h, "X");
        assert_eq!(edited(&h), (layer, "aXbcd".to_owned(), 2, 2), "active={active}");
    }
}

#[test]
fn native_text_pointer_split_press_drag_release_keeps_ownership() {
    for dragging in [false, true] {
        let (mut h, layer, caret1, caret4) = native_text_pointer_editor();
        h.input_mut().events.extend([
            Event::PointerMoved(caret1),
            Event::PointerButton { pos: caret1, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE },
        ]);
        h.run_steps(1);
        assert_eq!(edited(&h), (layer, "abcd".to_owned(), 1, 1));
        assert!(h.ctx.input(|i| i.pointer.primary_down()));
        let end = if dragging { caret4 } else { caret1 };
        if dragging {
            h.input_mut().events.push(Event::PointerMoved(end));
            h.run_steps(1);
            assert_eq!((edited(&h).2, edited(&h).3), (1, 4));
        }
        h.input_mut().events.push(Event::PointerButton { pos: end, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
        h.run_steps(1);
        let expected = if dragging { (1, 4) } else { (1, 1) };
        assert!(!h.ctx.input(|i| i.pointer.primary_down()));
        assert_eq!((edited(&h).2, edited(&h).3), expected);
        h.input_mut().events.push(Event::PointerMoved(if dragging { caret1 } else { caret4 }));
        h.run_steps(1);
        assert_eq!((edited(&h).2, edited(&h).3), expected, "a released drag must not follow later movement");
        assert_eq!(layer_doc(&h.state().session, LayerId(layer)).unwrap().text, "abcd");
    }
}

#[test]
fn native_text_pointer_two_completed_primary_clicks_use_the_latest_primary_press() {
    let (mut h, layer, caret1, caret4) = native_text_pointer_editor();
    h.input_mut().events.extend([
        Event::PointerMoved(caret4),
        Event::PointerButton { pos: caret4, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE },
        Event::PointerButton { pos: caret4, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE },
        Event::PointerMoved(caret1),
        Event::PointerButton { pos: caret1, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE },
        Event::PointerButton { pos: caret1, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE },
        // A later non-primary click must not become the recovered primary origin.
        Event::PointerMoved(caret4),
        Event::PointerButton { pos: caret4, button: PointerButton::Middle, pressed: true, modifiers: Modifiers::NONE },
        Event::PointerButton { pos: caret4, button: PointerButton::Middle, pressed: false, modifiers: Modifiers::NONE },
    ]);
    h.run_steps(1);
    assert!(h.ctx.input(|i| i.pointer.primary_pressed() && i.pointer.primary_released() && i.pointer.press_origin().is_none()));
    assert!(!h.ctx.input(|i| i.pointer.button_double_clicked(PointerButton::Primary) || i.pointer.button_triple_clicked(PointerButton::Primary)));
    assert_eq!(edited(&h), (layer, "abcd".to_owned(), 1, 1), "the first click must not replace the latest click");
}

#[test]
fn native_text_pointer_nonfinite_latest_press_does_not_replay_an_earlier_click() {
    for (earlier_finite_click, same_frame_release) in [(false, false), (false, true), (true, false), (true, true)] {
        let (mut h, layer, caret1, caret4) = native_text_pointer_editor();
        let undo = h.state().session.history.undo.len();
        if earlier_finite_click {
            h.input_mut().events.extend([
                Event::PointerMoved(caret1),
                Event::PointerButton { pos: caret1, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE },
                Event::PointerButton { pos: caret1, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE },
            ]);
        }
        let bad = egui::pos2(f32::INFINITY, caret4.y);
        h.input_mut().events.extend([
            Event::PointerButton { pos: bad, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE },
            Event::PointerMoved(caret1),
        ]);
        if same_frame_release {
            h.input_mut().events.push(Event::PointerButton { pos: caret1, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
        }
        h.run_steps(1);
        assert_eq!(edited(&h), (layer, "abcd".to_owned(), 4, 4), "earlier_finite_click={earlier_finite_click}");
        assert_eq!(h.state().session.history.undo.len(), undo);
        if !same_frame_release {
            assert!(h.ctx.input(|i| i.pointer.primary_down()));
            h.input_mut().events.push(Event::PointerButton { pos: caret1, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
            h.run_steps(1);
            assert_eq!(edited(&h), (layer, "abcd".to_owned(), 4, 4), "later release cannot retarget an invalid press");
            assert_eq!(h.state().session.history.undo.len(), undo);
        }
        assert!(!h.ctx.input(|i| i.pointer.primary_down()));
    }
}

#[test]
fn native_text_pointer_outside_area_origin_cannot_retarget_on_inside_release() {
    for active in [true, false] {
        let (mut h, layer, caret1, _) = native_text_pointer_editor();
        if !active {
            h.state_mut().session.execute("text.endEdit", json!({})).unwrap();
            h.run_steps(2);
        }
        let area = rect(&h, "viewer.area");
        let outside = area.min - egui::vec2(8.0, 8.0);
        assert!(outside.is_finite() && !area.contains(outside));
        assert!(outside.x >= 0.0 && outside.y >= 0.0, "use an actual window location outside the viewer");
        let layers = h.state().session.active_comp().unwrap().layers.len();
        let undo = h.state().session.history.undo.len();
        h.input_mut().events.extend([
            Event::PointerMoved(outside),
            Event::PointerButton { pos: outside, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE },
            Event::PointerMoved(caret1),
            Event::PointerButton { pos: caret1, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE },
        ]);
        h.run_steps(1);
        assert_eq!(layer_doc(&h.state().session, LayerId(layer)).unwrap().text, "abcd");
        assert_eq!(h.state().session.active_comp().unwrap().layers.len(), layers, "outside origins must not create text in the viewer");
        assert_eq!(h.state().session.history.undo.len(), undo);
        if active {
            // A real outside-panel action may close the edit; it must never retarget it.
            if h.state().session.state.text_edit.is_some() {
                assert_eq!(edited(&h), (layer, "abcd".to_owned(), 4, 4));
            }
        } else {
            assert!(h.state().session.state.text_edit.is_none(), "release inside is not an inside-origin click");
        }
    }
}

#[test]
fn native_text_pointer_completed_outside_panel_click_closes_the_editor() {
    for (panel, target) in [(PanelKind::Timeline, "timeline.empty"), (PanelKind::Project, "project.empty")] {
        let (mut h, layer, _, _) = native_text_pointer_editor();
        h.state_mut().show_panel(panel);
        h.run_steps(3);
        assert!(h.state().session.state.text_edit.is_some(), "opening a panel is not a pointer click");
        let outside = rect(&h, target).center();
        assert!(!rect(&h, "viewer.area").contains(outside));
        h.input_mut().events.extend([
            Event::PointerMoved(outside),
            Event::PointerButton { pos: outside, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE },
            Event::PointerButton { pos: outside, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE },
        ]);
        h.run_steps(1);
        assert!(h.state().session.state.text_edit.is_none(), "a complete {panel:?} click must close the edit");
        type_text(&mut h, "X");
        assert_eq!(layer_doc(&h.state().session, LayerId(layer)).unwrap().text, "abcd", "typing after click-away must not reach the old editor");
    }
}

#[test]
fn native_text_pointer_secondary_click_does_not_move_the_caret() {
    let (mut h, layer, caret1, _) = native_text_pointer_editor();
    h.input_mut().events.extend([
        Event::PointerMoved(caret1),
        Event::PointerButton { pos: caret1, button: PointerButton::Secondary, pressed: true, modifiers: Modifiers::NONE },
        Event::PointerButton { pos: caret1, button: PointerButton::Secondary, pressed: false, modifiers: Modifiers::NONE },
    ]);
    h.run_steps(1);
    assert!(!h.ctx.input(|i| i.pointer.primary_pressed()));
    assert_eq!(edited(&h), (layer, "abcd".to_owned(), 4, 4));
}

#[test]
fn native_text_pointer_modal_blocks_background_same_batch_click() {
    let (mut h, layer, caret1, _) = native_text_pointer_editor();
    h.state_mut().dialog = Some(effectcraft_ui_egui::Dialog::About);
    h.run_steps(3);
    h.input_mut().events.extend([
        Event::PointerMoved(caret1),
        Event::PointerButton { pos: caret1, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE },
        Event::PointerButton { pos: caret1, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE },
    ]);
    h.run_steps(1);
    assert_eq!(edited(&h), (layer, "abcd".to_owned(), 4, 4), "a disabled viewer must not recover a background click");
}

fn ime_interruption_delete(bytes: bool) -> Event {
    Event::Ime(if bytes {
        egui::ImeEvent::DeleteSurroundingBytes { before_bytes: 1, after_bytes: 0 }
    } else {
        egui::ImeEvent::DeleteSurrounding { before_chars: 1, after_chars: 0 }
    })
}

fn begin_interrupted_composition(h: &mut Harness<'_, EffectcraftApp>, terminal_pending: bool) {
    EffectcraftApp::install_ime_reset_hooks(&h.ctx);
    h.input_mut().events.push(Event::Ime(egui::ImeEvent::Preedit { text: "é".into(), active_range_chars: None }));
    h.run_steps(1);
    if terminal_pending {
        h.input_mut().events.push(Event::Ime(egui::ImeEvent::Preedit { text: String::new(), active_range_chars: None }));
        h.run_steps(1);
    }
    assert_eq!(edited(h).1, "abcd", "neither visible nor cleared preedit changes committed text");
}

fn request_at_caret(h: &mut Harness<'_, EffectcraftApp>, caret: usize) -> egui::ImeResetId {
    h.state_mut().session.execute("text.setSelection", json!({"caret": caret})).unwrap();
    h.run_steps(1);
    ime_reset_request(h)
}

fn ime_reset_request(h: &Harness<'_, EffectcraftApp>) -> egui::ImeResetId {
    h.output()
        .platform_output
        .ime
        .as_ref()
        .expect("the real viewer publishes its descriptor")
        .reset_request
        .expect("capture the final request before an unchanged idle pass consumes the one-shot output")
}

#[test]
fn pointer_caret_interruption_rejects_same_batch_ime_delete_and_commit() {
    for bytes in [false, true] {
        for terminal_pending in [false, true] {
            let (mut h, point) = ime_interruption_editor();
            begin_interrupted_composition(&mut h, terminal_pending);
            let project = h.state().session.project.clone();
            let undo = h.state().session.history.undo.len();
            h.input_mut().events.extend([
                Event::PointerMoved(point),
                Event::PointerButton { pos: point, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE },
                ime_interruption_delete(bytes),
                Event::Ime(egui::ImeEvent::Commit("X".into())),
            ]);
            h.run_steps(1);
            assert_eq!((edited(&h).1.as_str(), edited(&h).2, edited(&h).3), ("abcd", 1, 1), "the actual pointer caret survives stale IME mutations");
            assert!(std::sync::Arc::ptr_eq(&project, &h.state().session.project), "rejected IME never edits the project");
            assert_eq!(h.state().session.history.undo.len(), undo, "rejected input creates no undo entry");
            let request_id = ime_reset_request(&h);
            h.input_mut().events.push(Event::PointerButton { pos: point, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
            h.run_steps(1);
            // The explicit native fence, not the rejected terminal, admits fresh composition.
            h.input_mut().events.extend([
                Event::ImeResetCompleted { request_id },
                Event::Ime(egui::ImeEvent::Preedit { text: "é".into(), active_range_chars: None }),
                Event::Ime(egui::ImeEvent::Commit("é".into())),
            ]);
            h.run_steps(1);
            assert_eq!(edited(&h).1, "aébcd");
            assert_eq!((edited(&h).2, edited(&h).3), (2, 2));
            h.state_mut().session.execute("edit.undo", json!({})).unwrap();
            assert_eq!(edited(&h).1, "abcd", "only the fresh commit needs undo");
        }
    }
}

#[test]
fn keyboard_caret_interruption_rejects_later_ime_events_and_keeps_fresh_preedit() {
    for (key, modifiers, expected, fresh_text) in
        [(Key::ArrowLeft, Modifiers::NONE, (3, 3), "abcéd"), (Key::Home, Modifiers::SHIFT, (4, 0), "é"), (Key::A, Modifiers::COMMAND, (0, 4), "é")]
    {
        for bytes in [false, true] {
            let (mut h, _) = ime_interruption_editor();
            begin_interrupted_composition(&mut h, false);
            let project = h.state().session.project.clone();
            let undo = h.state().session.history.undo.len();
            h.input_mut().events.extend([
                Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers },
                ime_interruption_delete(bytes),
                Event::Ime(egui::ImeEvent::Commit("X".into())),
                Event::Ime(egui::ImeEvent::Preedit { text: "é".into(), active_range_chars: None }),
            ]);
            h.run_steps(1);
            assert_eq!(edited(&h).1, "abcd", "the old deletion and commit are refused");
            assert_eq!((edited(&h).2, edited(&h).3), expected, "the real key's selection survives");
            assert!(std::sync::Arc::ptr_eq(&project, &h.state().session.project));
            assert_eq!(h.state().session.history.undo.len(), undo);
            let request_id = ime_reset_request(&h);
            h.input_mut().events.extend([
                Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers },
                Event::ImeResetCompleted { request_id },
                Event::Ime(egui::ImeEvent::Preedit { text: "é".into(), active_range_chars: None }),
                Event::Ime(egui::ImeEvent::Commit("é".into())),
            ]);
            h.run_steps(1);
            assert_eq!(edited(&h).1, fresh_text, "only the fresh preedit after the matching fence is accepted");
            h.state_mut().session.execute("edit.undo", json!({})).unwrap();
            assert_eq!(edited(&h).1, "abcd");
        }
    }
}

#[test]
fn external_caret_interruption_rejects_pending_composition_mutations() {
    for bytes in [false, true] {
        let (mut h, _) = ime_interruption_editor();
        begin_interrupted_composition(&mut h, true);
        let project = h.state().session.project.clone();
        let undo = h.state().session.history.undo.len();
        // This is the same production command exposed to control clients; no replacement epoch.
        h.state_mut().session.execute("text.setSelection", json!({"caret": 1})).unwrap();
        h.input_mut().events.extend([ime_interruption_delete(bytes), Event::Ime(egui::ImeEvent::Commit("X".into()))]);
        h.run_steps(1);
        assert_eq!((edited(&h).1.as_str(), edited(&h).2, edited(&h).3), ("abcd", 1, 1));
        assert!(std::sync::Arc::ptr_eq(&project, &h.state().session.project));
        assert_eq!(h.state().session.history.undo.len(), undo);
        let request_id = ime_reset_request(&h);
        h.input_mut().events.extend([Event::ImeResetCompleted { request_id }, Event::Ime(egui::ImeEvent::Commit("Y".into()))]);
        h.run_steps(1);
        assert_eq!(edited(&h).1, "aYbcd", "a later direct commit without preedit is still accepted");
    }
}

#[test]
fn composing_ime_delete_then_commit_and_direct_commit_remain_valid() {
    for bytes in [false, true] {
        let (mut h, _) = ime_interruption_editor();
        begin_interrupted_composition(&mut h, false);
        let undo = h.state().session.history.undo.len();
        h.input_mut().events.extend([ime_interruption_delete(bytes), Event::Ime(egui::ImeEvent::Commit("X".into()))]);
        h.run_steps(1);
        assert_eq!((edited(&h).1.as_str(), edited(&h).2, edited(&h).3), ("abcX", 4, 4), "the IME deletion's own caret movement must not reject its commit");
        assert_eq!(h.state().session.history.undo.len(), undo + 1);
        h.state_mut().session.execute("edit.undo", json!({})).unwrap();
        assert_eq!(edited(&h).1, "abcd", "accepted delete and commit remain one undo step");
        h.run_steps(1);
        h.input_mut().events.push(Event::Ime(egui::ImeEvent::Commit("Y".into())));
        h.run_steps(1);
        assert_eq!(edited(&h).1, "abcdY", "noncomposing direct commit remains valid after undo");
    }
}

#[test]
fn a_noncomposing_direct_commit_can_follow_a_pointer_caret_change_in_the_same_batch() {
    let (mut h, point) = ime_interruption_editor();
    let undo = h.state().session.history.undo.len();
    h.input_mut().events.extend([
        Event::PointerMoved(point),
        Event::PointerButton { pos: point, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE },
        Event::Ime(egui::ImeEvent::Commit("Y".into())),
    ]);
    h.run_steps(1);
    assert_eq!((edited(&h).1.as_str(), edited(&h).2, edited(&h).3), ("aYbcd", 2, 2), "only an interrupted composition is rejected");
    assert_eq!(h.state().session.history.undo.len(), undo + 1);
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(edited(&h).1, "abcd");
}

fn draws_ime_preview(h: &Harness<'_, EffectcraftApp>) -> bool {
    fn contains_preview(shape: &egui::Shape) -> bool {
        match shape {
            egui::Shape::Text(text) => text.galley.text() == "é",
            egui::Shape::Vec(shapes) => shapes.iter().any(contains_preview),
            _ => false,
        }
    }
    h.output().shapes.iter().any(|shape| contains_preview(&shape.shape))
}

fn replacement_ime_edit(h: &mut Harness<'_, EffectcraftApp>) -> u64 {
    let replacement =
        h.state_mut().session.execute("layer.newText", json!({"text": "wxyz", "size": 80, "position": [80, 240], "justify": "left"})).unwrap()["layer"]
            .as_u64()
            .unwrap();
    h.state_mut().session.execute("text.edit", json!({"layer": replacement, "caret": 1})).unwrap();
    h.run_steps(1);
    assert_eq!(edited(h), (replacement, "wxyz".to_owned(), 1, 1), "the replacement editor must be live before late IME arrives");
    let comp = h.state().session.active_comp_id().unwrap().0;
    let ime = h.output().platform_output.ime.as_ref().expect("the real replacement editor publishes IME");
    assert_eq!(ime.owner, egui::Id::unique(("effectcraft-ime-text", comp, replacement)));
    assert!(!draws_ime_preview(h), "the old preview must not be drawn on the replacement");
    replacement
}

#[test]
fn deselect_all_during_composition_rejects_late_ime_on_a_live_replacement_session() {
    for rebound in [false, true] {
        for bytes in [false, true] {
            for terminal_pending in [false, true] {
                let (mut h, _) = ime_interruption_editor();
                let (chord, modifiers) = if rebound {
                    h.state_mut().session.execute("shortcuts.set", json!({"command": "edit.deselectAll", "keys": ["Cmd+Alt+Z"]})).unwrap();
                    (Key::Z, Modifiers { alt: true, ..Modifiers::COMMAND })
                } else {
                    (Key::A, Modifiers { shift: true, ..Modifiers::COMMAND })
                };
                let original = edited(&h).0;
                let old_session = h.state().session.state.text_edit.as_ref().unwrap().session;
                h.input_mut().events.push(Event::Ime(egui::ImeEvent::Preedit { text: "é".into(), active_range_chars: None }));
                h.run_steps(1);
                assert!(draws_ime_preview(&h), "a real active composition must draw before interruption");
                if terminal_pending {
                    h.input_mut().events.push(Event::Ime(egui::ImeEvent::Preedit { text: String::new(), active_range_chars: None }));
                    h.run_steps(1);
                    assert!(!draws_ime_preview(&h), "empty preedit hides the preview but leaves the terminal pending");
                }
                key(&mut h, chord, modifiers);
                assert!(h.state().session.state.text_edit.is_none(), "the actual current shortcut ends the old edit");
                assert!(h.state().session.state.selected_layers.is_empty());
                assert!(h.output().platform_output.ime.is_none(), "the ended editor must not publish a descriptor");
                assert!(!draws_ime_preview(&h), "Deselect All clears the old preview");
                assert_eq!(layer_doc(&h.state().session, LayerId(original)).unwrap().text, "abcd");

                let replacement = replacement_ime_edit(&mut h);
                let request_id = ime_reset_request(&h);
                assert_ne!(replacement, original);
                assert_ne!(h.state().session.state.text_edit.as_ref().unwrap().session, old_session);
                let project = h.state().session.project.clone();
                let history = (h.state().session.history.undo.len(), h.state().session.history.redo.len());
                let selection = h.state().session.state.selected_layers.clone();
                h.input_mut().events.extend([ime_interruption_delete(bytes), Event::Ime(egui::ImeEvent::Commit("X".into()))]);
                h.run_steps(1);
                assert_eq!(edited(&h), (replacement, "wxyz".to_owned(), 1, 1), "late delete and commit must reach the live replacement and be rejected");
                assert_eq!(h.state().session.state.selected_layers, selection);
                assert!(std::sync::Arc::ptr_eq(&project, &h.state().session.project), "rejected IME never edits either layer");
                assert_eq!((h.state().session.history.undo.len(), h.state().session.history.redo.len()), history);
                assert_eq!(layer_doc(&h.state().session, LayerId(original)).unwrap().text, "abcd");

                h.input_mut().events.extend([
                    Event::ImeResetCompleted { request_id },
                    Event::Ime(egui::ImeEvent::Preedit { text: "é".into(), active_range_chars: None }),
                    Event::Ime(egui::ImeEvent::Commit("é".into())),
                ]);
                h.run_steps(1);
                assert_eq!(edited(&h), (replacement, "wéxyz".to_owned(), 2, 2), "a fresh composition still commits after rejecting the old terminal");
                assert_eq!(h.state().session.history.undo.len(), history.0 + 1);
                h.state_mut().session.execute("edit.undo", json!({})).unwrap();
                assert_eq!(edited(&h).1, "wxyz", "only the fresh commit needs undo");
            }
        }
    }
}

#[test]
fn deselect_all_without_composition_keeps_a_replacement_direct_commit_valid() {
    let (mut h, _) = ime_interruption_editor();
    let original = edited(&h).0;
    key(&mut h, Key::A, Modifiers { shift: true, ..Modifiers::COMMAND });
    assert!(h.state().session.state.text_edit.is_none());
    assert!(h.output().platform_output.ime.is_none());
    let replacement = replacement_ime_edit(&mut h);
    let undo = h.state().session.history.undo.len();
    h.input_mut().events.push(Event::Ime(egui::ImeEvent::Commit("Y".into())));
    h.run_steps(1);
    assert_eq!(edited(&h), (replacement, "wYxyz".to_owned(), 2, 2), "a new editor without an interrupted composition accepts a direct commit");
    assert_eq!(h.state().session.history.undo.len(), undo + 1);
    assert_eq!(layer_doc(&h.state().session, LayerId(original)).unwrap().text, "abcd");
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(edited(&h).1, "wxyz");
}

fn fresh_ime(text: &str) -> [Event; 2] {
    [Event::Ime(egui::ImeEvent::Preedit { text: text.into(), active_range_chars: None }), Event::Ime(egui::ImeEvent::Commit(text.into()))]
}

#[test]
fn request_fence_alone_admits_the_first_fresh_ime_in_same_or_next_batch() {
    for terminal_pending in [false, true] {
        for next_batch in [false, true] {
            let (mut h, _) = ime_interruption_editor();
            begin_interrupted_composition(&mut h, terminal_pending);
            let request_id = request_at_caret(&mut h, 1);
            h.input_mut().events.push(Event::ImeResetCompleted { request_id });
            if next_batch {
                h.run_steps(1);
            }
            h.input_mut().events.extend(fresh_ime("が"));
            h.run_steps(1);
            assert_eq!((edited(&h).1.as_str(), edited(&h).2, edited(&h).3), ("aがbcd", 2, 2));
            assert!(h.output().platform_output.ime.as_ref().unwrap().reset_request.is_none(), "completion does not allocate another request");
            h.state_mut().session.execute("edit.undo", json!({})).unwrap();
            assert_eq!(edited(&h).1, "abcd");
        }
    }
}

#[test]
fn newest_opaque_fence_rejects_stale_foreign_duplicate_and_no_ack_payloads() {
    let (mut h, _) = ime_interruption_editor();
    begin_interrupted_composition(&mut h, false);
    let first = request_at_caret(&mut h, 1);
    let second = request_at_caret(&mut h, 2);
    assert_ne!(first, second);
    let foreign = h.ctx.request_ime_reset(egui::ViewportId::from_hash_of("effect-ime-foreign")).unwrap();
    let foreign_context = egui::Context::default().request_ime_reset(egui::ViewportId::ROOT).unwrap();
    let project = h.state().session.project.clone();
    let undo = h.state().session.history.undo.len();
    for _ in 0..3 {
        h.input_mut().events.extend([
            Event::ImeResetCompleted { request_id: first },
            Event::ImeResetCompleted { request_id: foreign },
            Event::ImeResetCompleted { request_id: foreign_context },
            ime_interruption_delete(false),
            ime_interruption_delete(true),
            Event::Ime(egui::ImeEvent::Commit("X".into())),
            Event::Ime(egui::ImeEvent::Preedit { text: "late".into(), active_range_chars: None }),
            Event::Ime(egui::ImeEvent::Preedit { text: String::new(), active_range_chars: None }),
        ]);
        h.run_steps(1);
        assert_eq!((edited(&h).1.as_str(), edited(&h).3), ("abcd", 2));
        assert!(std::sync::Arc::ptr_eq(&project, &h.state().session.project));
        assert_eq!(h.state().session.history.undo.len(), undo);
        assert!(h.output().platform_output.ime.as_ref().unwrap().reset_request.is_none(), "no-ack does not spin requests");
    }
    h.input_mut().events.extend([
        Event::ImeResetCompleted { request_id: second },
        Event::Ime(egui::ImeEvent::Preedit { text: "が".into(), active_range_chars: None }),
        Event::ImeResetCompleted { request_id: second },
        Event::Ime(egui::ImeEvent::Commit("が".into())),
    ]);
    h.run_steps(1);
    assert_eq!(edited(&h).1, "abがcd", "duplicate F cannot interrupt the newly admitted composition");
    assert!(h.output().platform_output.ime.as_ref().unwrap().reset_request.is_none());
}

#[test]
fn outside_pointer_visits_the_control_suffix_without_mutating_the_old_viewer() {
    for terminal_pending in [false, true] {
        let (mut h, _) = ime_interruption_editor();
        h.state_mut().show_panel(PanelKind::Timeline);
        h.run_steps(2);
        begin_interrupted_composition(&mut h, terminal_pending);
        let first = request_at_caret(&mut h, 1);
        let layer = edited(&h).0;
        let outside = rect(&h, "timeline.empty").center();
        assert!(!rect(&h, "viewer.area").contains(outside));
        let undo = h.state().session.history.undo.len();
        h.input_mut().events.extend([
            Event::PointerMoved(outside),
            Event::PointerButton { pos: outside, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE },
            Event::ImeResetCompleted { request_id: first },
            Event::Text("BAD".into()),
            Event::Cut,
            Event::Paste("BAD".into()),
            Event::Ime(egui::ImeEvent::Preedit { text: "late".into(), active_range_chars: None }),
            Event::Ime(egui::ImeEvent::Commit("X".into())),
        ]);
        h.run_steps(1);
        assert_eq!(layer_doc(&h.state().session, LayerId(layer)).unwrap().text, "abcd");
        assert_eq!(h.state().session.history.undo.len(), undo);
        assert!(!h.ctx.input(|input| input.events.iter().any(|event| matches!(event, Event::ImeResetCompleted { .. }))), "the suffix control was visited");
        h.input_mut().events.push(Event::PointerButton { pos: outside, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
        h.run_steps(1);
        h.state_mut().session.execute("text.edit", json!({"layer": layer, "caret": 1})).unwrap();
        h.state_mut().ui.focused = PanelKind::Composition;
        h.run_steps(1);
        let second = ime_reset_request(&h);
        assert_ne!(first, second);
        h.input_mut().events.extend([
            Event::ImeResetCompleted { request_id: first },
            Event::Ime(egui::ImeEvent::Commit("X".into())),
            Event::ImeResetCompleted { request_id: second },
        ]);
        h.input_mut().events.extend(fresh_ime("が"));
        h.run_steps(1);
        assert_eq!(edited(&h).1, "aがbcd");
    }
}

#[test]
fn old_commit_then_fence_then_outside_pointer_keeps_the_prefix_rejected() {
    let (mut h, _) = ime_interruption_editor();
    h.state_mut().show_panel(PanelKind::Timeline);
    h.run_steps(2);
    begin_interrupted_composition(&mut h, false);
    let first = request_at_caret(&mut h, 1);
    let layer = edited(&h).0;
    let outside = rect(&h, "timeline.empty").center();
    h.input_mut().events.extend([
        Event::Ime(egui::ImeEvent::Commit("X".into())),
        Event::ImeResetCompleted { request_id: first },
        Event::PointerMoved(outside),
        Event::PointerButton { pos: outside, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE },
        Event::Text("BAD".into()),
        Event::Paste("BAD".into()),
    ]);
    h.run_steps(1);
    assert_eq!(layer_doc(&h.state().session, LayerId(layer)).unwrap().text, "abcd", "F cannot admit its preceding old commit or the outside suffix");
    h.input_mut().events.push(Event::PointerButton { pos: outside, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(1);
    h.state_mut().session.execute("text.edit", json!({"layer": layer, "caret": 1})).unwrap();
    h.state_mut().ui.focused = PanelKind::Composition;
    h.run_steps(1);
    // A panel may already have ended the custom route before the viewer drew. In that case
    // its controls-only owner retired F and the actual returned replacement must be used.
    if let Some(request_id) = h.output().platform_output.ime.as_ref().unwrap().reset_request {
        assert_ne!(first, request_id);
        h.input_mut().events.push(Event::ImeResetCompleted { request_id });
    }
    h.input_mut().events.extend(fresh_ime("が"));
    h.run_steps(1);
    assert_eq!(edited(&h).1, "aがbcd");
}

#[test]
fn stock_dialog_suspends_before_sync_and_return_rearms_the_same_model_once() {
    use egui_kittest::kittest::Queryable as _;
    let (mut h, _) = ime_interruption_editor();
    begin_interrupted_composition(&mut h, true);
    let first = request_at_caret(&mut h, 1);
    let custom_owner = h.output().platform_output.ime.as_ref().unwrap().owner;
    h.state_mut().dialog = Some(effectcraft_ui_egui::Dialog::CommandPalette);
    h.state_mut().session.execute("text.setSelection", json!({"caret": 2})).unwrap();
    h.run_steps(2);
    h.input_mut().events.extend([Event::ImeResetCompleted { request_id: first }, Event::Text("xyz".into())]);
    h.run_steps(1);
    let stock = h.output().platform_output.ime.as_ref().expect("the actual stock palette field owns IME");
    assert_ne!(stock.owner, custom_owner);
    assert!(stock.reset_request.is_none(), "ineligible synchronization cannot allocate a new token into the stock descriptor");
    // The real accessible name must remain available after typing hides the placeholder.
    let palette = h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Type a command or effect…");
    assert_eq!(palette.value().as_deref(), Some("xyz"), "the visible stock field receives the text untouched by custom control routing");
    assert_eq!(edited(&h).1, "abcd");
    h.state_mut().dialog = None;
    h.state_mut().ui.focused = PanelKind::Composition;
    h.ctx.memory_mut(|memory| {
        if let Some(field) = memory.focused() {
            memory.surrender_focus(field);
        }
    });
    h.run_steps(1);
    let second = ime_reset_request(&h);
    assert_ne!(first, second);
    h.run_steps(1);
    assert!(h.output().platform_output.ime.as_ref().unwrap().reset_request.is_none());
    h.input_mut().events.extend([
        Event::ImeResetCompleted { request_id: first },
        Event::Ime(egui::ImeEvent::Commit("X".into())),
        Event::ImeResetCompleted { request_id: second },
    ]);
    h.input_mut().events.extend(fresh_ime("が"));
    h.run_steps(1);
    assert_eq!(edited(&h).1, "abがcd");
}

#[test]
fn full_pass_logic_keeps_a_request_while_hidden_logic_rearms_once_on_return() {
    let (mut h, _) = ime_interruption_editor();
    begin_interrupted_composition(&mut h, false);
    let first = request_at_caret(&mut h, 1);
    h.run_steps(2);
    assert!(h.output().platform_output.ime.as_ref().unwrap().reset_request.is_none(), "normal App::logic before UI must not rearm");
    let ctx = h.ctx.clone();
    let frame = eframe::Frame::_new_kittest();
    let raw = egui::RawInput::default();
    for _ in 0..3 {
        let _ = ctx.run_logic(&raw, |ctx| {
            h.state_mut().tick(ctx, &frame);
            h.state().finish_ime_logic(ctx);
        });
    }
    h.run_steps(1);
    let second = ime_reset_request(&h);
    assert_ne!(first, second);
    h.run_steps(1);
    assert!(h.output().platform_output.ime.as_ref().unwrap().reset_request.is_none());
    h.input_mut().events.extend([
        Event::ImeResetCompleted { request_id: first },
        Event::Ime(egui::ImeEvent::Commit("X".into())),
        Event::ImeResetCompleted { request_id: second },
    ]);
    h.input_mut().events.extend(fresh_ime("が"));
    h.run_steps(1);
    assert_eq!(edited(&h).1, "aがbcd");
}

#[test]
fn same_caret_application_text_replacement_rejects_the_previous_context_fence() {
    let (mut h, _) = ime_interruption_editor();
    begin_interrupted_composition(&mut h, false);
    let first = request_at_caret(&mut h, 1);
    let before = edited(&h);
    let generation = h.output().platform_output.ime.as_ref().unwrap().context_generation;
    h.state_mut().session.execute("layer.setText", json!({"layer": before.0, "text": "Abcd"})).unwrap();
    h.input_mut().events.extend([
        Event::ImeResetCompleted { request_id: first },
        ime_interruption_delete(true),
        Event::Ime(egui::ImeEvent::Commit("X".into())),
    ]);
    h.run_steps(1);
    assert_eq!(edited(&h), (before.0, "Abcd".to_owned(), before.2, before.3));
    let output = h.output().platform_output.ime.as_ref().unwrap();
    assert_ne!(output.context_generation, generation, "same numeric owner/caret still has a new context");
    let second = ime_reset_request(&h);
    assert_ne!(first, second);
    h.input_mut().events.push(Event::ImeResetCompleted { request_id: second });
    h.input_mut().events.extend(fresh_ime("が"));
    h.run_steps(1);
    assert_eq!(edited(&h).1, "Aがbcd");
}
