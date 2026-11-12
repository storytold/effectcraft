//! Incoming IME edits on the custom text editor (the Type tool's own session, not a stock
//! `TextEdit`). Winit reports the bytes around the selection *before* the commit it belongs
//! to, so the deletion and the insert must share one undo step.

use super::*;
use effectcraft_engine::Session;
use effectcraft_engine::commands::text_edit::TextEdit;
use serde_json::json;

fn editor(text: &str, anchor: usize, caret: usize) -> (EffectcraftApp, LayerId) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "IME", "width": 160, "height": 90, "duration": 1})).unwrap();
    let layer = s.execute("layer.newText", json!({"text": text})).unwrap()["layer"].as_u64().unwrap();
    let lid = LayerId(layer);
    s.state.text_edit = Some(TextEdit { layer: lid, anchor, caret, ..Default::default() });
    (EffectcraftApp::new(s), lid)
}

fn doc_text(app: &EffectcraftApp, lid: LayerId) -> String {
    layer_doc(&app.session, lid).map(|d| d.text).unwrap_or_default()
}

fn feed(app: &mut EffectcraftApp, events: Vec<egui::Event>) {
    let ctx = egui::Context::default();
    let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 180.0))), events, ..Default::default() };
    let mut out = ctx.run_ui(raw, |ui| keyboard(app, ui, egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 180.0))));
    out.textures_delta.clear();
}

#[test]
fn preedit_preview_is_bounded_on_a_character_boundary() {
    let text = "é".repeat(MAX_IME_TEXT_BYTES);
    let capped = bounded_preedit(text);
    assert!(capped.len() <= MAX_IME_TEXT_BYTES);
    assert!(capped.is_char_boundary(capped.len()));
    assert_eq!(capped.len() % "é".len(), 0, "the cap never leaves half a character");
}

/// Ending the edit from another panel clears this viewport's pre-edit. The production path
/// used to resolve `viewport_id()` (a context read lock) inside `data_mut` (a write lock);
/// that nested context access deadlocked every pass that drew the viewer.
#[test]
fn ending_an_edit_from_another_panel_clears_the_preedit() {
    let (mut app, lid) = editor("abc", 0, 0);
    let session = app.session.state.text_edit.as_ref().map_or(1, |e| e.session);
    let ctx = egui::Context::default();
    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
        let viewport = ui.ctx().viewport_id();
        let preedit = Preedit { owner: ImeOwner { layer: lid, session }, text: "x".into() };
        ui.data_mut(|d| d.insert_temp(ime_preedit_id(viewport), preedit));
        panel_clicked(&mut app, ui.ctx(), PanelKind::Timeline);
        let viewport = ui.ctx().viewport_id();
        assert!(ui.data(|d| d.get_temp::<Preedit>(ime_preedit_id(viewport))).is_none(), "the ended edit leaves no preview behind");
    });
    out.textures_delta.clear();
    assert!(app.session.state.text_edit.is_none(), "a click in the Timeline ends the text edit");
}

#[test]
fn ime_delete_surrounding_bytes_and_the_commit_are_one_undo_step() {
    // "café😀": the last character is four bytes, so a byte-count deletion that treated bytes
    // as characters would corrupt the document.
    let (mut app, lid) = editor("café😀", 5, 5);
    feed(
        &mut app,
        vec![
            egui::Event::Ime(egui::ImeEvent::DeleteSurroundingBytes { before_bytes: 4, after_bytes: 0 }),
            egui::Event::Ime(egui::ImeEvent::Commit("X".into())),
        ],
    );
    assert_eq!(doc_text(&app, lid), "caféX", "the emoji is replaced by the committed text");
    app.session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(doc_text(&app, lid), "café😀", "the replacement is one undo step");
}

#[test]
fn ime_delete_surrounding_chars_keeps_the_selection_for_the_commit() {
    // Stock egui deletes the characters around the selection, not the selection itself: the
    // commit that follows replaces "bcd", it does not insert next to a deleted document.
    let (mut app, lid) = editor("abcdef", 1, 4);
    feed(&mut app, vec![egui::Event::Ime(egui::ImeEvent::DeleteSurrounding { before_chars: 1, after_chars: 99 })]);
    assert_eq!(doc_text(&app, lid), "bcd", "one character before and everything after go");
    let sel = app.session.state.text_edit.clone().expect("editing continues");
    assert_eq!(sel.range(), 0..3, "the selected text is still selected");
    // A deletion-only batch (no commit) leaves "bcd", not an empty document.
    app.session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(doc_text(&app, lid), "abcdef", "clamped deletion is still one undo step");

    // Delete surround + commit: the commit replaces the kept selection.
    let (mut app, lid) = editor("abcdef", 1, 4);
    feed(
        &mut app,
        vec![egui::Event::Ime(egui::ImeEvent::DeleteSurrounding { before_chars: 1, after_chars: 99 }), egui::Event::Ime(egui::ImeEvent::Commit("XY".into()))],
    );
    assert_eq!(doc_text(&app, lid), "XY", "the commit replaces the selection");
    app.session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(doc_text(&app, lid), "abcdef", "the deletion and its commit are one undo step");
}

/// One UI pass with the editor focused, events applied, and the overlay's observation.
struct ImePass {
    ctx: egui::Context,
    viewport: egui::ViewportId,
    rect: egui::Rect,
}

impl ImePass {
    fn new(viewport: egui::ViewportId) -> Self {
        let ctx = egui::Context::default();
        install_hooks(&ctx);
        Self { ctx, viewport, rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 180.0)) }
    }

    /// Run one `Context` frame. `overlay` is false for a pass in which the viewer (and so
    /// the edit overlay) never drew; `discard` forces a second pass of the same frame.
    fn frame(&mut self, app: &mut EffectcraftApp, events: Vec<egui::Event>, overlay: bool, discard: bool) -> Option<egui::output::IMEOutput> {
        let ctx = self.ctx.clone();
        let rect = self.rect;
        let viewport = self.viewport;
        let mut viewports = egui::ViewportIdMap::default();
        viewports.insert(self.viewport, egui::ViewportInfo::default());
        let raw = egui::RawInput { viewport_id: self.viewport, viewports, screen_rect: Some(rect), events, ..Default::default() };
        let mut asked = false;
        let mut out = ctx.run_ui(raw, |ui| {
            begin_ui(ui.ctx());
            ui.interact(rect, focus_id(), egui::Sense::focusable_noninteractive());
            ui.memory_mut(|m| m.request_focus(focus_id()));
            sync_ime_target(ui, app);
            keyboard(app, ui, rect);
            if overlay {
                let pass = ui.ctx().cumulative_pass_nr_for(viewport);
                ui.data_mut(|d| d.insert_temp(ime_geometry_id(viewport), (pass, rect, rect)));
            }
            finish_ime(app, ui);
            if discard && !asked {
                asked = true;
                ui.ctx().request_discard("ime pass test");
            }
        });
        out.textures_delta.clear();
        out.platform_output.ime
    }
}

/// One native floating window holding the Timeline's group and then the Viewer's.
fn two_group_window() -> craft_ui::docking::Floating<PanelKind> {
    let groups = vec![craft_ui::docking::Group::new(vec![PanelKind::Timeline], 0), craft_ui::docking::Group::new(vec![PanelKind::Viewer(1)], 0)];
    craft_ui::docking::Floating::stack(groups, [0.0, 0.0, 320.0, 360.0]).expect("two groups make a window")
}

impl ImePass {
    /// One pass of a native window with a body that edits no text and the Viewer's editor,
    /// in the order given, publishing as `NativeIme` decides or, with `every`, after each
    /// body. Returns the descriptor and the preedit the pass ended with.
    fn window(
        &mut self,
        app: &mut EffectcraftApp,
        window: &craft_ui::docking::Floating<PanelKind>,
        events: Vec<egui::Event>,
        viewer_first: bool,
        every: bool,
    ) -> (Option<egui::output::IMEOutput>, Option<String>) {
        let ctx = self.ctx.clone();
        let (rect, viewport) = (self.rect, self.viewport);
        let mut viewports = egui::ViewportIdMap::default();
        viewports.insert(viewport, egui::ViewportInfo::default());
        let raw = egui::RawInput { viewport_id: viewport, viewports, screen_rect: Some(rect), events, ..Default::default() };
        let mut preedit = None;
        let mut out = ctx.run_ui(raw, |ui| {
            let mut ime = crate::dock_ui::NativeIme::new(std::slice::from_ref(window), &[]);
            let order = if viewer_first { [true, false] } else { [false, true] };
            for viewer in order {
                if ime.begin_body(viewport) {
                    begin_ui(ui.ctx());
                }
                if viewer {
                    ui.interact(rect, focus_id(), egui::Sense::focusable_noninteractive());
                    ui.memory_mut(|m| m.request_focus(focus_id()));
                    sync_ime_target(ui, app);
                    keyboard(app, ui, rect);
                    let pass = ui.ctx().cumulative_pass_nr_for(viewport);
                    ui.data_mut(|d| d.insert_temp(ime_geometry_id(viewport), (pass, rect, rect)));
                }
                if ime.publishes(viewport, ime_recorded(ui.ctx())) || every {
                    finish_ime(app, ui);
                }
            }
            assert!(ime.unpublished().is_empty(), "the window published");
            preedit = read_preedit(ui).map(|p| p.text);
        });
        out.textures_delta.clear();
        (out.platform_output.ime, preedit)
    }
}

#[test]
fn native_window_with_a_group_before_the_viewer_keeps_the_composition() {
    let window = two_group_window();
    let viewport = craft_ui::docking::DockArea::<PanelKind>::window_viewport(crate::dock_ui::area_id(), window.id);
    let preview = || egui::Event::Ime(egui::ImeEvent::Preedit { text: "é".into(), active_range_chars: None });
    for viewer_first in [false, true] {
        let (mut app, _) = editor("abcd", 4, 4);
        let mut pass = ImePass::new(viewport);
        let (first, preedit) = pass.window(&mut app, &window, vec![preview()], viewer_first, false);
        let first = first.expect("the editor publishes");
        assert_eq!(preedit.as_deref(), Some("é"));
        for _ in 0..2 {
            let (ime, preedit) = pass.window(&mut app, &window, vec![], viewer_first, false);
            let ime = ime.expect("the editor keeps publishing");
            assert!(!ime.should_interrupt_composition, "viewer first: {viewer_first}");
            assert_eq!((ime.owner, ime.context_generation), (first.owner, first.context_generation), "the same IME context");
            assert_eq!(preedit.as_deref(), Some("é"), "the preedit survives the other body");
        }
    }
    // Publishing after the body drawn before the Viewer clears the preedit and interrupts.
    let (mut app, _) = editor("abcd", 4, 4);
    let mut pass = ImePass::new(viewport);
    pass.window(&mut app, &window, vec![preview()], false, true);
    let (ime, preedit) = pass.window(&mut app, &window, vec![], false, true);
    assert!(ime.expect("the editor publishes").should_interrupt_composition);
    assert_eq!(preedit, None);
    // A window that drew fewer bodies than its layout counts publishes after every body in
    // its next pass.
    let mut ime = crate::dock_ui::NativeIme::new(std::slice::from_ref(&window), &[]);
    ime.begin_body(viewport);
    assert!(!ime.publishes(viewport, false));
    let unpublished = ime.unpublished();
    assert_eq!(unpublished, vec![viewport]);
    let mut ime = crate::dock_ui::NativeIme::new(std::slice::from_ref(&window), &unpublished);
    ime.begin_body(viewport);
    assert!(ime.publishes(viewport, false));
}

fn ime_text(ime: &egui::output::IMEOutput) -> (&str, usize, usize) {
    let p = ime.surrounding_text.available().expect("committed surrounding text");
    (p.text.as_str(), p.cursor, p.anchor)
}

#[test]
fn discarded_custom_request_then_home_rearms_the_identical_caret() {
    let (mut app, lid) = editor("abcd", 4, 4);
    app.ui.start_screen = false;
    app.ui.tool = Tool::Type;
    app.ui.focused = PanelKind::Composition;
    let ctx = egui::Context::default();
    install_hooks(&ctx);
    let mut frame = eframe::Frame::_new_kittest();
    let raw = || egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 800.0))), ..Default::default() };
    for _ in 0..3 {
        let mut output = ctx.run_ui(raw(), |ui| {
            eframe::App::logic(&mut app, ui.ctx(), &mut frame);
            app.draw(ui, &frame);
        });
        output.textures_delta.clear();
    }
    let mut input = raw();
    input.events.push(egui::Event::Ime(egui::ImeEvent::Preedit { text: "é".into(), active_range_chars: None }));
    let mut output = ctx.run_ui(input, |ui| {
        eframe::App::logic(&mut app, ui.ctx(), &mut frame);
        app.draw(ui, &frame);
    });
    output.textures_delta.clear();
    assert!(output.platform_output.ime.is_some());
    app.session.execute("text.setSelection", json!({"caret": 1})).unwrap();
    let selection = app.session.state.text_edit.clone();
    let mut passes = 0;
    let mut discarded = None;
    let mut output = ctx.run_ui(raw(), |ui| {
        passes += 1;
        if passes > 1 {
            app.ui.start_screen = true;
        }
        eframe::App::logic(&mut app, ui.ctx(), &mut frame);
        app.draw(ui, &frame);
        if passes == 1 {
            assert!(ui.ctx().output(|output| output.ime.is_some()));
            discarded = read_reset(ui.ctx()).and_then(|guard| guard.request);
            ui.ctx().request_discard("custom IME descriptor discarded before Home");
        }
    });
    output.textures_delta.clear();
    assert!(passes >= 2);
    let first = discarded.expect("the actual discarded custom pass requested a fence");
    assert!(output.platform_output.ime.is_none());
    assert!(read_reset(&ctx).unwrap().suspended);
    assert!(read_reset(&ctx).unwrap().request.is_none());
    assert_eq!(app.session.state.text_edit, selection, "Home did not move the model caret");
    app.ui.start_screen = false;
    let mut output = ctx.run_ui(raw(), |ui| {
        eframe::App::logic(&mut app, ui.ctx(), &mut frame);
        app.draw(ui, &frame);
    });
    output.textures_delta.clear();
    let second = output.platform_output.ime.unwrap().reset_request.expect("same caret reactivation publishes a new real ID");
    assert_ne!(first, second);
    let mut output = ctx.run_ui(raw(), |ui| {
        eframe::App::logic(&mut app, ui.ctx(), &mut frame);
        app.draw(ui, &frame);
    });
    output.textures_delta.clear();
    assert!(output.platform_output.ime.unwrap().reset_request.is_none());
    let mut input = raw();
    input.events.extend([
        egui::Event::ImeResetCompleted { request_id: first },
        egui::Event::Ime(egui::ImeEvent::Commit("X".into())),
        egui::Event::ImeResetCompleted { request_id: second },
        egui::Event::Ime(egui::ImeEvent::Preedit { text: "が".into(), active_range_chars: None }),
        egui::Event::Ime(egui::ImeEvent::Commit("が".into())),
    ]);
    let mut output = ctx.run_ui(input, |ui| {
        eframe::App::logic(&mut app, ui.ctx(), &mut frame);
        app.draw(ui, &frame);
    });
    output.textures_delta.clear();
    assert_eq!(doc_text(&app, lid), "aがbcd");
}

#[test]
fn viewport_fences_and_duplicate_completion_keep_their_exact_lifetime() {
    let (mut app, lid) = editor("abcd", 4, 4);
    let mut root = ImePass::new(egui::ViewportId::ROOT);
    let mut detached = ImePass::new(egui::ViewportId::from_hash_of("effect-ime-detached"));
    let preview = || egui::Event::Ime(egui::ImeEvent::Preedit { text: "é".into(), active_range_chars: None });
    root.frame(&mut app, vec![preview()], true, false);
    detached.frame(&mut app, vec![preview()], true, false);
    app.session.execute("text.setSelection", json!({"caret": 1})).unwrap();
    let first = root.frame(&mut app, vec![], true, false).unwrap().reset_request.unwrap();
    let second = detached.frame(&mut app, vec![], true, false).unwrap().reset_request.unwrap();
    assert_ne!(first, second);
    detached.frame(&mut app, vec![egui::Event::ImeResetCompleted { request_id: first }, egui::Event::Ime(egui::ImeEvent::Commit("X".into()))], true, false);
    assert_eq!(doc_text(&app, lid), "abcd");
    let pending = detached.ctx.data(|data| data.get_temp::<ResetGuard>(reset_id(detached.viewport))).unwrap().request;
    assert_eq!(pending, Some(second), "a root marker cannot release the detached viewport's full ID");
    let output = detached
        .frame(
            &mut app,
            vec![
                egui::Event::ImeResetCompleted { request_id: second },
                preview(),
                egui::Event::ImeResetCompleted { request_id: second },
                egui::Event::Ime(egui::ImeEvent::Commit("が".into())),
            ],
            true,
            false,
        )
        .unwrap();
    assert_eq!(doc_text(&app, lid), "aがbcd");
    assert!(output.reset_request.is_none(), "a duplicate control cannot reconstruct the cleared guard");
    // An actual context-generation change at unchanged selection still supersedes a wait.
    root.frame(&mut app, vec![], true, false);
    let replaced = root.frame(&mut app, vec![], false, false);
    assert!(replaced.is_none());
    let replacement = root.frame(&mut app, vec![], true, false).unwrap().reset_request.unwrap();
    assert_ne!(first, replacement);
}

#[test]
fn ime_descriptor_sends_committed_text_and_preserves_a_reversed_selection() {
    let (mut app, _) = editor("café😀", 2, 5);
    let mut pass = ImePass::new(egui::ViewportId::ROOT);
    let first = pass.frame(&mut app, vec![], true, false).expect("the focused editor owns IME");
    assert_eq!(first.owner, ime_target(&app).expect("an edit target").owner);
    assert!(first.context_generation > 0);
    assert_eq!(ime_text(&first), ("café😀", 9, 2), "UTF-8 bytes, with the caret's direction kept");
    assert_eq!(first.change_cause, egui::ImeChangeCause::Other, "starting a context is application origin");
    assert_eq!((first.rect, first.cursor_rect), (pass.rect, pass.rect), "the real caret geometry");

    let again = pass.frame(&mut app, vec![], true, false).expect("still editing");
    assert_eq!(again.context_generation, first.context_generation, "an unchanged pass keeps the context");
    assert_eq!(again.surrounding_text, first.surrounding_text);
}

#[test]
fn accepted_ime_commit_is_input_method_and_ordinary_typing_is_other() {
    let (mut app, lid) = editor("ab", 2, 2);
    let mut pass = ImePass::new(egui::ViewportId::ROOT);
    let first = pass.frame(&mut app, vec![], true, false).expect("editing");
    let generation = first.context_generation;

    let commit = pass.frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::Commit("日".into()))], true, false).expect("still editing");
    assert_eq!(doc_text(&app, lid), "ab日");
    assert_eq!(ime_text(&commit), ("ab日", 5, 5));
    assert_eq!(commit.change_cause, egui::ImeChangeCause::InputMethod, "only the accepted commit earns it");
    assert_eq!(commit.context_generation, generation, "an ordinary text change is not a new context");

    let typed = pass.frame(&mut app, vec![egui::Event::Text("x".into())], true, false).expect("still editing");
    assert_eq!(doc_text(&app, lid), "ab日x");
    assert_eq!(typed.change_cause, egui::ImeChangeCause::Other, "typing is application work");
    assert_eq!(typed.context_generation, generation);
}

#[test]
fn ime_delete_and_commit_chain_is_input_method_with_a_reversed_selection() {
    let (mut app, lid) = editor("abcdef", 4, 1);
    let mut pass = ImePass::new(egui::ViewportId::ROOT);
    pass.frame(&mut app, vec![], true, false).expect("editing");

    let deleted = pass
        .frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::DeleteSurrounding { before_chars: 1, after_chars: 1 })], true, false)
        .expect("still editing");
    assert_eq!(doc_text(&app, lid), "bcdf", "one character goes before and one after the selection");
    assert_eq!(ime_text(&deleted), ("bcdf", 0, 3), "the selected text and its reversed direction stay");
    assert_eq!(deleted.change_cause, egui::ImeChangeCause::InputMethod);

    let committed = pass.frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::Commit("X".into()))], true, false).expect("still editing");
    assert_eq!(doc_text(&app, lid), "Xf", "the commit replaces the kept selection");
    assert_eq!(committed.change_cause, egui::ImeChangeCause::InputMethod, "the delete and commit are one accepted chain");
    app.session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(doc_text(&app, lid), "abcdef", "the replacement is one undo step");
}

#[test]
fn an_application_edit_in_the_same_frame_dominates_the_ime_cause() {
    // Application edit before the IME commit.
    let (mut app, lid) = editor("ab", 2, 2);
    let mut pass = ImePass::new(egui::ViewportId::ROOT);
    pass.frame(&mut app, vec![], true, false).expect("editing");
    let mixed =
        pass.frame(&mut app, vec![egui::Event::Text("x".into()), egui::Event::Ime(egui::ImeEvent::Commit("日".into()))], true, false).expect("still editing");
    assert_eq!(doc_text(&app, lid), "abx日");
    assert_eq!(mixed.change_cause, egui::ImeChangeCause::Other, "the earlier application edit dominates");

    // Application edit after the IME commit.
    let (mut app, lid) = editor("ab", 2, 2);
    let mut pass = ImePass::new(egui::ViewportId::ROOT);
    pass.frame(&mut app, vec![], true, false).expect("editing");
    let mixed =
        pass.frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::Commit("日".into())), egui::Event::Text("x".into())], true, false).expect("still editing");
    assert_eq!(doc_text(&app, lid), "ab日x");
    assert_eq!(mixed.change_cause, egui::ImeChangeCause::Other, "the later application edit dominates");
}

#[test]
fn preedit_is_excluded_from_the_committed_snapshot_and_keeps_the_context() {
    let (mut app, lid) = editor("ab", 2, 2);
    let mut pass = ImePass::new(egui::ViewportId::ROOT);
    let first = pass.frame(&mut app, vec![], true, false).expect("editing");

    let preview = pass
        .frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::Preedit { text: "に".into(), active_range_chars: None })], true, false)
        .expect("still editing");
    assert_eq!(doc_text(&app, lid), "ab", "the pre-edit never enters the document");
    assert_eq!(ime_text(&preview), ("ab", 2, 2), "only committed text is sent");
    assert_eq!(preview.context_generation, first.context_generation, "pre-edit updates keep the composition");

    let commit = pass.frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::Commit("に".into()))], true, false).expect("still editing");
    assert_eq!(doc_text(&app, lid), "abに");
    assert_eq!(commit.change_cause, egui::ImeChangeCause::InputMethod, "clearing the pre-edit and committing is IME work");
    assert_eq!(commit.context_generation, first.context_generation);
}

#[test]
fn empty_and_unavailable_surrounding_text_are_distinct() {
    let (mut app, _) = editor("", 0, 0);
    let mut pass = ImePass::new(egui::ViewportId::ROOT);
    let empty = pass.frame(&mut app, vec![], true, false).expect("editing");
    assert_eq!(ime_text(&empty), ("", 0, 0), "an available-empty document is not unavailable");

    let text = "a".repeat(5000);
    let (mut app, _) = editor(&text, 0, 5000);
    let mut pass = ImePass::new(egui::ViewportId::ROOT);
    let wide = pass.frame(&mut app, vec![], true, false).expect("editing");
    assert!(matches!(wide.surrounding_text, egui::output::IMESurroundingText::Unavailable), "a selection wider than the limit is unavailable");
    assert_eq!(wide.owner, ime_target(&app).expect("an edit target").owner, "identity survives unavailable text");
}

#[test]
fn an_unrelated_undo_keeps_a_wide_selection_composition() {
    // A selection wider than the IME limit reports unavailable surrounding text, whose
    // receipt identity is a revision. That revision must follow the editor, not the whole
    // project: undoing an unrelated opacity change used to look like an editor mutation and
    // cancel an active composition.
    let text = "a".repeat(5000);
    let (mut app, lid) = editor(&text, 0, 5000);
    let mut pass = ImePass::new(egui::ViewportId::ROOT);
    let first = pass.frame(&mut app, vec![], true, false).expect("editing");
    assert!(matches!(first.surrounding_text, egui::output::IMESurroundingText::Unavailable), "the wide selection is unavailable");

    let composing = pass
        .frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::Preedit { text: "に".into(), active_range_chars: None })], true, false)
        .expect("still editing");
    assert_eq!(composing.context_generation, first.context_generation, "the pre-edit keeps the context");
    let owner = ime_owner(&app).expect("an editor owner");
    assert!(matches!(
        pass.ctx.data(|d| d.get_temp::<ImeComposition>(ime_composition_id(pass.viewport))),
        Some(ImeComposition::Active(active)) if active == owner
    ));

    app.session.execute("prop.set", json!({"layer": lid.0, "path": "transform/opacity", "value": 50})).unwrap();
    app.session.execute("edit.undo", json!({})).unwrap();
    let after = pass.frame(&mut app, vec![], true, false).expect("still editing");
    assert_eq!(after.context_generation, first.context_generation, "an unrelated undo does not start a new context");
    assert!(
        matches!(
            pass.ctx.data(|d| d.get_temp::<ImeComposition>(ime_composition_id(pass.viewport))),
            Some(ImeComposition::Active(active)) if active == owner
        ),
        "the wide-selection composition survives an unrelated undo"
    );
    assert!(read_reset(&pass.ctx).is_none(), "no IME reset was requested");

    // The composition is still the live one: its commit is accepted and earns the
    // input-method cause rather than being poisoned as leftover application work.
    let commit = pass.frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::Commit("x".into()))], true, false).expect("still editing");
    assert_eq!(doc_text(&app, lid), "x", "the preserved composition commits into the wide selection");
    assert_eq!(commit.change_cause, egui::ImeChangeCause::InputMethod);
}

#[test]
fn a_failed_batch_with_a_transient_edit_does_not_revive_a_wide_selection_composition() {
    // The final document equals the pre-batch one, but the intermediate insert/delete was a
    // real document change. Its IME revision must stay the high-water mark; otherwise the
    // active wide-selection composition would look current and accept its old commit.
    let text = "a".repeat(5000);
    let (mut app, lid) = editor(&text, 0, 5000);
    let mut pass = ImePass::new(egui::ViewportId::ROOT);
    pass.frame(&mut app, vec![], true, false).expect("editing");
    pass.frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::Preedit { text: "に".into(), active_range_chars: None })], true, false).expect("still editing");
    let owner = ime_owner(&app).expect("an editor owner");
    assert!(matches!(
        pass.ctx.data(|d| d.get_temp::<ImeComposition>(ime_composition_id(pass.viewport))),
        Some(ImeComposition::Active(active)) if active == owner
    ));

    let r = app.session.execute(
        "engine.batch",
        json!({"atomic": true, "steps": [
            {"command": "text.insert", "params": {"text": "x", "range": [5000, 5000]}},
            {"command": "text.delete", "params": {}},
            {"command": "no.suchCommand", "params": {}}
        ]}),
    );
    assert!(r.is_err());
    assert_eq!(doc_text(&app, lid), text, "the batch restored the final document");

    pass.frame(&mut app, vec![], true, false).expect("still editing");
    assert!(read_reset(&pass.ctx).is_some(), "the advanced revision retires the old IME context");
    assert!(
        matches!(pass.ctx.data(|d| d.get_temp::<ImeComposition>(ime_composition_id(pass.viewport))), Some(ImeComposition::Stale)),
        "the failed batch's transient edit leaves the old composition stale"
    );
    let stale = pass.frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::Commit("z".into()))], true, false).expect("still editing");
    assert_eq!(doc_text(&app, lid), text, "the stale composition cannot commit into the restored document");
    assert_ne!(stale.change_cause, egui::ImeChangeCause::InputMethod, "a rejected commit earns no IME receipt");
}

#[test]
fn a_wide_content_change_invalidates_the_active_composition() {
    let text = "a".repeat(5000);
    let (mut app, lid) = editor(&text, 0, 5000);
    let mut pass = ImePass::new(egui::ViewportId::ROOT);
    pass.frame(&mut app, vec![], true, false).expect("editing");
    pass.frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::Preedit { text: "に".into(), active_range_chars: None })], true, false).expect("still editing");

    let replaced = "b".repeat(5000);
    app.session.execute("layer.setText", json!({"layer": lid.0, "text": replaced})).unwrap();
    let after = pass.frame(&mut app, vec![], true, false).expect("still editing");
    assert_eq!(doc_text(&app, lid), replaced);
    assert!(after.should_interrupt_composition, "replacing the wide document starts a new IME context");
    assert!(
        matches!(pass.ctx.data(|d| d.get_temp::<ImeComposition>(ime_composition_id(pass.viewport))), Some(ImeComposition::Stale)),
        "the old composition is stale after the content change"
    );
    pass.frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::Commit("z".into()))], true, false).expect("still editing");
    assert_eq!(doc_text(&app, lid), replaced, "a delayed commit from the old document is rejected");
}

#[test]
fn a_wide_selection_change_invalidates_the_active_composition() {
    let text = "a".repeat(5000);
    let (mut app, lid) = editor(&text, 0, 5000);
    let mut pass = ImePass::new(egui::ViewportId::ROOT);
    pass.frame(&mut app, vec![], true, false).expect("editing");
    pass.frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::Preedit { text: "に".into(), active_range_chars: None })], true, false).expect("still editing");

    // Still wider than the IME window limit, so this exercises the unavailable-text
    // selection identity rather than merely making the surrounding text available.
    app.session.execute("text.setSelection", json!({"start": 0, "end": 4500})).unwrap();
    pass.frame(&mut app, vec![], true, false).expect("still editing");
    assert!(read_reset(&pass.ctx).is_some(), "an application selection change retires the composition");
    assert!(
        matches!(pass.ctx.data(|d| d.get_temp::<ImeComposition>(ime_composition_id(pass.viewport))), Some(ImeComposition::Stale)),
        "the old composition cannot follow the selection"
    );
    pass.frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::Commit("z".into()))], true, false).expect("still editing");
    assert_eq!(doc_text(&app, lid), text, "the stale composition cannot mutate the wide selection");
}

#[test]
fn replaced_edit_session_and_hidden_passes_start_a_new_context() {
    let (mut app, _) = editor("text", 4, 4);
    let mut pass = ImePass::new(egui::ViewportId::ROOT);
    let before = pass.frame(&mut app, vec![], true, false).expect("editing");
    if let Some(edit) = app.session.state.text_edit.as_mut() {
        edit.session = edit.session.wrapping_add(1);
    }
    let replaced = pass.frame(&mut app, vec![], true, false).expect("editing");
    assert!(replaced.context_generation > before.context_generation, "a recreated target does not inherit the old context");
    assert_eq!(replaced.change_cause, egui::ImeChangeCause::Other);
    assert!(replaced.should_interrupt_composition, "a new replacement epoch cancels the platform composition");

    let (mut app, _) = editor("abc", 3, 3);
    let mut pass = ImePass::new(egui::ViewportId::ROOT);
    let visible = pass.frame(&mut app, vec![], true, false).expect("editing");
    assert!(pass.frame(&mut app, vec![], false, false).is_none(), "a hidden editor reports no IME owner");
    let back = pass.frame(&mut app, vec![], true, false).expect("editing again");
    assert!(back.context_generation > visible.context_generation, "reappearing starts exactly one new context");
}

#[test]
fn preedit_from_a_replaced_target_cannot_commit_into_the_new_session() {
    let (mut app, first) = editor("A", 1, 1);
    let second = app.session.execute("layer.newText", json!({"text": "B"})).unwrap()["layer"].as_u64().unwrap();
    let mut pass = ImePass::new(egui::ViewportId::ROOT);
    pass.frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::Preedit { text: "か".into(), active_range_chars: None })], true, false)
        .expect("editing the first layer");

    app.session.execute("text.edit", json!({"layer": second, "caret": 1})).unwrap();
    let switched = pass.frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::Commit("日".into()))], true, false).expect("editing the second layer");
    assert_eq!(doc_text(&app, LayerId(second)), "B", "a delayed commit from the old composition is ignored");
    assert_eq!(doc_text(&app, first), "A", "the old layer is not changed either");
    assert_eq!(switched.owner, ime_target(&app).expect("second target").owner);
    assert!(switched.should_interrupt_composition, "the target switch interrupts the old composition");
}

#[test]
fn hidden_pass_clears_this_viewports_preedit_preview() {
    let (mut app, _) = editor("ab", 2, 2);
    let mut pass = ImePass::new(egui::ViewportId::ROOT);
    pass.frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::Preedit { text: "に".into(), active_range_chars: None })], true, false).expect("editing");
    assert!(pass.ctx.data(|d| d.get_temp::<Preedit>(ime_preedit_id(pass.viewport))).is_some());
    assert!(pass.frame(&mut app, vec![], false, false).is_none());
    assert!(pass.ctx.data(|d| d.get_temp::<Preedit>(ime_preedit_id(pass.viewport))).is_none(), "a pass without the overlay clears this viewport's preview");
}

#[test]
fn a_discarded_pass_keeps_the_frame_cause_and_the_context() {
    let (mut app, lid) = editor("ab", 2, 2);
    let mut pass = ImePass::new(egui::ViewportId::ROOT);
    let first = pass.frame(&mut app, vec![], true, false).expect("editing");
    let committed = pass.frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::Commit("日".into()))], true, true).expect("the final pass of the frame");
    assert_eq!(doc_text(&app, lid), "ab日", "the discarded pass did not replay the event");
    assert_eq!(committed.change_cause, egui::ImeChangeCause::InputMethod, "an unchanged later pass keeps the frame's cause");
    assert_eq!(committed.context_generation, first.context_generation, "both passes are one context");
}

#[test]
fn two_viewports_keep_independent_ime_contexts() {
    let (mut app, _) = editor("ab", 2, 2);
    let mut first = ImePass::new(egui::ViewportId::ROOT);
    let mut second = ImePass::new(egui::ViewportId::from_hash_of("ime-test-second-viewport"));
    let one = first.frame(&mut app, vec![], true, false).expect("editing in the first viewport");
    let two = second.frame(&mut app, vec![], true, false).expect("editing in the second viewport");
    assert_ne!(one.context_generation, two.context_generation, "each viewport has its own generation");
    let again = first.frame(&mut app, vec![], true, false).expect("editing in the first viewport");
    assert_eq!(again.context_generation, one.context_generation, "the second viewport did not disturb the first");
}

/// Draw one real `EffectcraftApp::draw` pass, so `draw`'s all-exit finalizer runs.
fn draw_app_pass(app: &mut EffectcraftApp, ctx: &egui::Context, frame: &eframe::Frame, events: Vec<egui::Event>) -> egui::FullOutput {
    let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0))), events, ..Default::default() };
    let mut out = ctx.run_ui(raw, |ui| app.draw(ui, frame));
    out.textures_delta.clear();
    out
}

/// One headless pass with the custom viewer and a real stock `TextEdit` side by side.
///
/// The first call focuses the custom editor before `keyboard` (the outside press arrives
/// there). The second call leaves the stock field's focus alone, so the stock descriptor
/// is published by the same `output.ime` writer that an unrelated field would use.
fn stock_pass(
    pass: &mut ImePass,
    app: &mut EffectcraftApp,
    events: Vec<egui::Event>,
    stock: &mut String,
    custom_focus: bool,
) -> Option<egui::output::IMEOutput> {
    let ctx = pass.ctx.clone();
    let viewport = pass.viewport;
    let custom = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(160.0, 90.0));
    let stock_rect = egui::Rect::from_min_size(egui::pos2(180.0, 30.0), egui::vec2(120.0, 24.0));
    let stock_id = egui::Id::unique("effectcraft-ime-test-stock");
    let mut viewports = egui::ViewportIdMap::default();
    viewports.insert(viewport, egui::ViewportInfo::default());
    let raw = egui::RawInput { viewport_id: viewport, viewports, screen_rect: Some(pass.rect), events, ..Default::default() };
    let mut out = ctx.run_ui(raw, |ui| {
        begin_ui(ui.ctx());
        if custom_focus {
            ui.interact(custom, focus_id(), egui::Sense::focusable_noninteractive());
            ui.memory_mut(|m| m.request_focus(focus_id()));
            sync_ime_target(ui, app);
            keyboard(app, ui, custom);
        } else {
            // The stock field holds focus after the outside press; keep that ownership while
            // the second pass publishes the actual stock descriptor.
            ui.memory_mut(|m| m.request_focus(stock_id));
        }
        let _ = ui.put(stock_rect, egui::TextEdit::singleline(stock).id(stock_id));
        finish_ime(app, ui);
    });
    out.textures_delta.clear();
    out.platform_output.ime
}

#[test]
fn outside_pointer_to_stock_allocates_no_new_request_and_defers_the_replacement() {
    let (mut app, lid) = editor("abcd", 4, 4);
    let mut pass = ImePass::new(egui::ViewportId::ROOT);
    let preview = || egui::Event::Ime(egui::ImeEvent::Preedit { text: "é".into(), active_range_chars: None });
    pass.frame(&mut app, vec![preview()], true, false).expect("the custom editor starts a composition");
    app.session.execute("text.setSelection", json!({"caret": 1})).unwrap();
    let first = pass.frame(&mut app, vec![], true, false).expect("the caret change publishes a request").reset_request.expect("the real request ID");
    // Complete the first interruption, then start a fresh composition with no waiting guard.
    pass.frame(&mut app, vec![egui::Event::ImeResetCompleted { request_id: first }], true, false);
    let active = pass.frame(&mut app, vec![preview()], true, false).expect("a fresh composition");
    assert!(active.reset_request.is_none(), "a fresh composition does not wait on an old request");
    assert_eq!(doc_text(&app, lid), "abcd");

    // The press outside the viewer also gives the stock field focus. It must not allocate a
    // new request: core would attach that request to the stock descriptor next pass.
    let outside = egui::pos2(240.0, 42.0);
    let mut stock = String::new();
    stock_pass(
        &mut pass,
        &mut app,
        vec![
            egui::Event::PointerMoved(outside),
            egui::Event::PointerButton { pos: outside, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE },
        ],
        &mut stock,
        true,
    );
    let guard = pass.ctx.data(|d| d.get_temp::<ResetGuard>(reset_id(pass.viewport))).expect("the custom guard is retired locally");
    assert!(guard.suspended && guard.request.is_none(), "the outside press retires the old request without allocating another");
    let stock_ime = stock_pass(&mut pass, &mut app, vec![], &mut stock, false).expect("the actual stock field owns IME");
    assert_ne!(stock_ime.owner, ime_target(&app).expect("the custom target is still live").owner);
    assert!(stock_ime.reset_request.is_none(), "a new outside-press request would surface on the stock output");
    assert_eq!(doc_text(&app, lid), "abcd");

    // Returning to a confirmed eligible custom output allocates the one replacement.
    let replacement = pass.frame(&mut app, vec![], true, false).expect("the custom editor owns IME again").reset_request.expect("the deferred replacement");
    assert_ne!(first, replacement);
    assert!(pass.frame(&mut app, vec![], true, false).expect("still editing").reset_request.is_none(), "the replacement is one-shot");
}

#[test]
fn fonts_and_device_loss_all_exit_finalizers_suspend_without_allocating() {
    let (mut app, _lid) = editor("abcd", 1, 1);
    app.ui.start_screen = false;
    app.ui.tool = Tool::Type;
    app.ui.focused = PanelKind::Composition;
    app.show_panel(PanelKind::Composition);
    let ctx = egui::Context::default();
    install_hooks(&ctx);
    let frame = eframe::Frame::_new_kittest();
    let preview = || egui::Event::Ime(egui::ImeEvent::Preedit { text: "é".into(), active_range_chars: None });
    // The first pass installs fonts and the next draws the real viewer.
    for _ in 0..3 {
        draw_app_pass(&mut app, &ctx, &frame, vec![]);
    }
    draw_app_pass(&mut app, &ctx, &frame, vec![preview()]);
    app.session.execute("text.setSelection", json!({"caret": 2})).unwrap();
    let first = draw_app_pass(&mut app, &ctx, &frame, vec![])
        .platform_output
        .ime
        .expect("the normal pass publishes the custom descriptor")
        .reset_request
        .expect("one request for the caret interruption");

    // A real font-install pass draws nothing, but `draw` still reaches the finalizer.
    app.styled = false;
    let fonts = draw_app_pass(&mut app, &ctx, &frame, vec![]);
    assert!(!app.is_ready(), "the font-install pass cannot draw");
    assert!(fonts.platform_output.ime.is_none(), "no custom descriptor is published");
    let guard = ctx.data(|d| d.get_temp::<ResetGuard>(reset_id(egui::ViewportId::ROOT))).expect("the waiting guard");
    assert!(guard.suspended && guard.request.is_none(), "the all-exit finalizer retired it without allocating");
    let second = draw_app_pass(&mut app, &ctx, &frame, vec![])
        .platform_output
        .ime
        .expect("returning to a drawing pass publishes the custom descriptor")
        .reset_request
        .expect("the deferred request");
    assert_ne!(first, second);

    // The device-loss early return is the other all-exit path. Preset the close bookkeeping
    // so this test does not autosave; the pass must still retire the waiting guard.
    app.gpu_failure_applied = Some(true);
    app.device_lost_close = Some(true);
    let lost = draw_app_pass(&mut app, &ctx, &frame, vec![]);
    assert!(lost.platform_output.ime.is_none(), "a lost renderer publishes no descriptor");
    let guard = ctx.data(|d| d.get_temp::<ResetGuard>(reset_id(egui::ViewportId::ROOT))).expect("the waiting guard");
    assert!(guard.suspended && guard.request.is_none(), "the device-loss finalizer retired it without allocating");
    app.gpu_failure_applied = None;
    let third = draw_app_pass(&mut app, &ctx, &frame, vec![])
        .platform_output
        .ime
        .expect("a drawing pass publishes the custom descriptor again")
        .reset_request
        .expect("the deferred request is one-shot");
    assert_ne!(third, second);
}

#[test]
fn ime_delete_without_a_session_or_with_an_empty_range_does_nothing() {
    let (mut app, lid) = editor("abc", 1, 1);
    feed(&mut app, vec![egui::Event::Ime(egui::ImeEvent::DeleteSurroundingBytes { before_bytes: 0, after_bytes: 0 })]);
    assert_eq!(doc_text(&app, lid), "abc");
    app.session.state.text_edit = None;
    feed(&mut app, vec![egui::Event::Ime(egui::ImeEvent::DeleteSurroundingBytes { before_bytes: 2, after_bytes: 0 })]);
    assert_eq!(doc_text(&app, lid), "abc");
}
