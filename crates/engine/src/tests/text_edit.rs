//! On-canvas text editing and per-character styles through commands: run-scoped
//! `layer.setText`, text.insert / delete / setSelection / moveCaret, the text clipboard and the
//! paste-formatting commands, with undo / redo.

use effectcraft_keyframe::{BaselineOption, Justify, Kerning, TextDoc};
use serde_json::json;

use crate::Session;
use crate::commands::text_edit::{MAX_IME_TEXT_BYTES, ime_stamp_revision, ime_surrounding, layer_doc};
use crate::tests::three_d::layer_count;

fn session(text: &str) -> (Session, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "C", "width": 640, "height": 360, "duration": 4})).unwrap();
    let t = s.execute("layer.newText", json!({"text": text, "size": 60, "justify": "left"})).unwrap()["layer"].as_u64().unwrap();
    (s, t)
}

fn doc(s: &Session, t: u64) -> TextDoc {
    layer_doc(s, effectcraft_project::LayerId(t)).unwrap()
}

fn sel(s: &Session) -> (usize, usize) {
    let e = s.state.text_edit.as_ref().unwrap();
    (e.anchor, e.caret)
}

/// Start editing the text layer with everything selected (no caret given).
fn begin_edit(s: &mut Session, t: u64) {
    s.execute("text.edit", json!({"layer": t})).unwrap();
}

/// Start editing the text layer with the caret at `caret`.
fn begin_edit_at(s: &mut Session, t: u64, caret: usize) {
    s.execute("text.edit", json!({"layer": t, "caret": caret})).unwrap();
}

/// A session with one text layer already open in the editor at `caret`.
fn editing(text: &str, caret: usize) -> (Session, u64) {
    let (mut s, t) = session(text);
    begin_edit_at(&mut s, t, caret);
    (s, t)
}

/// A session with one text layer already open in the editor with everything selected.
fn editing_all(text: &str) -> (Session, u64) {
    let (mut s, t) = session(text);
    begin_edit(&mut s, t);
    (s, t)
}

/// The IME owner token of the open editor.
fn ime_owner(s: &Session) -> u64 {
    s.state.text_edit.as_ref().unwrap().session
}

fn undo_steps(s: &Session) -> usize {
    s.history.undo.len()
}

/// The editor kept its document (same IME owner).
fn assert_ime_owner(s: &Session, expected: u64, why: &str) {
    assert_eq!(ime_owner(s), expected, "{why}");
}

/// The editor's document was replaced (new IME owner/epoch).
fn assert_ime_epoch_changed(s: &Session, previous: u64, why: &str) {
    assert_ne!(ime_owner(s), previous, "{why}");
}

fn assert_text(s: &Session, t: u64, expected: &str) {
    assert_eq!(doc(s, t).text, expected);
}

/// A new, empty composition called "Other" (the standard second comp).
fn new_other_comp(s: &mut Session) -> u64 {
    s.execute("comp.new", json!({"name": "Other", "width": 320, "height": 180, "duration": 2})).unwrap()["comp"].as_u64().unwrap()
}

/// Stopwatch the layer's Source Text (one key at the CTI).
fn animate_source(s: &mut Session, t: u64) {
    s.execute("prop.toggleAnimation", json!({"layer": t, "path": "text/sourceText"})).unwrap();
}

/// Add a Source Text key at `time`.
fn add_source_key(s: &mut Session, t: u64, time: f64, value: &str) {
    s.execute("prop.addKey", json!({"layer": t, "path": "text/sourceText", "time": time, "value": value})).unwrap();
}

/// Write Source Text as a static value (a replacement when it differs from the shown text).
fn set_source(s: &mut Session, t: u64, value: &str) {
    s.execute("prop.set", json!({"layer": t, "path": "text/sourceText", "value": value})).unwrap();
}

/// Write the Source Text key at `time`.
fn set_source_key(s: &mut Session, t: u64, time: f64, value: &str) {
    s.execute("prop.set", json!({"layer": t, "path": "text/sourceText", "time": time, "value": value})).unwrap();
}

/// Set the Source Text key at the CTI (`keys.set`).
fn set_source_cti_key(s: &mut Session, t: u64, time: f64, value: &str) {
    s.execute("keys.set", json!({"layer": t, "path": "text/sourceText", "time": time, "value": value})).unwrap();
}

/// Replace the whole Source Text of the layer (`layer.setText`).
fn replace_text(s: &mut Session, t: u64, text: &str) {
    s.execute("layer.setText", json!({"layer": t, "text": text})).unwrap();
}

/// Select the character range `start..end` in the open editor.
fn select(s: &mut Session, start: usize, end: usize) {
    s.execute("text.setSelection", json!({"start": start, "end": end})).unwrap();
}

/// Set both selection ends, keeping their direction (`anchor` is the other end).
fn select_anchor(s: &mut Session, anchor: usize, caret: usize) {
    s.execute("text.setSelection", json!({"anchor": anchor, "caret": caret})).unwrap();
}

#[test]
fn set_text_with_a_range_creates_runs_and_undoes() {
    let (mut s, t) = session("Hello World");
    s.execute("layer.setText", json!({"layer": t, "range": [6, 11], "size": 30, "fill": "#ff0000", "kerning": "optical"})).unwrap();
    let d = doc(&s, t);
    assert_eq!(d.runs().len(), 2);
    assert_eq!(d.style_at(0).size, 60.0);
    assert_eq!(d.style_at(7).size, 30.0);
    assert_eq!(d.style_at(7).fill, [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(d.style_at(7).kerning, Kerning::Optical);
    // Paragraph attributes apply to the paragraphs the range touches; document ones to all.
    s.execute("layer.setText", json!({"layer": t, "range": [0, 1], "justify": "justifyAll", "indentFirst": 20, "spaceAfter": 6, "direction": "rtl"})).unwrap();
    let d = doc(&s, t);
    assert_eq!((d.justify, d.indent_first, d.space_after), (Justify::JustifyAll, 20.0, 6.0));
    // Unknown attributes and bad values are errors (nothing changes).
    assert!(s.execute("layer.setText", json!({"layer": t, "sizee": 3})).is_err());
    assert!(s.execute("layer.setText", json!({"layer": t, "size": "big"})).is_err());
    assert!(s.execute("edit.undo", json!({})).is_ok());
    assert_eq!(doc(&s, t).justify, Justify::Left);
    assert!(s.execute("edit.undo", json!({})).is_ok());
    assert!(doc(&s, t).is_uniform());
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(doc(&s, t).runs().len(), 2);
    // Without a range everything changes and the runs collapse again.
    s.execute("layer.setText", json!({"layer": t, "size": 44, "superscript": true})).unwrap();
    let d = doc(&s, t);
    assert!(d.runs().iter().all(|r| r.style.size == 44.0 && r.style.baseline == BaselineOption::Superscript));
    // Replacing a range's text keeps the formatting around it.
    s.execute("layer.setText", json!({"layer": t, "range": [0, 5], "text": "Howdy", "fauxBold": true})).unwrap();
    let d = doc(&s, t);
    assert_eq!(d.text, "Howdy World");
    assert!(d.style_at(2).faux_bold && !d.style_at(7).faux_bold);
}

#[test]
fn typing_deleting_and_caret_movement() {
    let (mut s, t) = session("abc def");
    s.execute("text.edit", json!({"layer": t})).unwrap();
    assert_eq!(sel(&s), (0, 7), "editing starts with everything selected");
    s.execute("text.setSelection", json!({"start": 3})).unwrap();
    s.execute("text.insert", json!({"text": "XY", "merge": "typing-1"})).unwrap();
    s.execute("text.insert", json!({"text": "Z", "merge": "typing-1"})).unwrap();
    assert_eq!(doc(&s, t).text, "abcXYZ def");
    assert_eq!(sel(&s), (6, 6));
    // One undo step for the merged typing.
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(doc(&s, t).text, "abc def");
    assert_eq!(sel(&s), (6, 6));
    s.execute("edit.redo", json!({})).unwrap();
    s.execute("text.setSelection", json!({"start": 6})).unwrap();
    // Backspace / Delete, by character and by word.
    s.execute("text.delete", json!({})).unwrap();
    assert_eq!(doc(&s, t).text, "abcXY def");
    s.execute("text.delete", json!({"direction": "forward", "word": true})).unwrap();
    assert_eq!(doc(&s, t).text, "abcXY");
    s.execute("text.delete", json!({"word": true})).unwrap();
    assert_eq!(doc(&s, t).text, "");
    s.execute("text.insert", json!({"text": "one two\nthree"})).unwrap();
    assert_eq!(sel(&s), (13, 13));
    // Caret movement: words, lines, up / down, extend.
    let mv = |s: &mut Session, to: &str, extend: bool| -> (usize, usize) {
        let r = s.execute("text.moveCaret", json!({"to": to, "extend": extend})).unwrap();
        (r["anchor"].as_u64().unwrap() as usize, r["caret"].as_u64().unwrap() as usize)
    };
    assert_eq!(mv(&mut s, "lineStart", false), (8, 8));
    assert_eq!(mv(&mut s, "up", false).1, 0);
    assert_eq!(mv(&mut s, "wordRight", false), (3, 3));
    assert_eq!(mv(&mut s, "wordRight", true), (3, 7));
    assert_eq!(mv(&mut s, "left", false), (3, 3), "left collapses a selection to its start");
    assert_eq!(mv(&mut s, "lineEnd", false), (7, 7));
    assert_eq!(mv(&mut s, "down", false).1, 13);
    assert_eq!(mv(&mut s, "start", true), (13, 0));
    assert_eq!(mv(&mut s, "paraEnd", false), (7, 7));
    // Select All while editing selects the text, not the layers.
    s.execute("edit.selectAll", json!({})).unwrap();
    assert_eq!(sel(&s), (0, 13));
    // Undo past the insertion clamps the selection.
    s.execute("edit.undo", json!({})).unwrap();
    assert!(sel(&s).0 <= doc(&s, t).char_len() && sel(&s).1 <= doc(&s, t).char_len());
    s.execute("text.endEdit", json!({})).unwrap();
    assert!(s.state.text_edit.is_none());
}

/// Duplicating during a text edit moves the selection to the copy: the edit on the original ends
/// (#2226), as in After Effects — typing must not keep editing an unselected layer.
#[test]
fn duplicate_during_a_text_edit_ends_the_edit() {
    let (mut s, t) = session("Hello");
    s.execute("text.edit", json!({"layer": t, "caret": 5})).unwrap();
    let copy = s.execute("edit.duplicate", json!({"layers": [t]})).unwrap()[0].as_u64().unwrap();
    assert_ne!(copy, t, "the duplicate is created");
    assert_eq!(s.state.selected_layers, vec![effectcraft_project::LayerId(copy)], "the copy is selected");
    assert!(s.state.text_edit.is_none(), "the edit ended with the selection move (#2226)");
    assert_eq!(doc(&s, t).text, "Hello", "the original keeps its text");
    assert_eq!(doc(&s, copy).text, "Hello", "and the copy carries it");
}

#[test]
fn caret_style_applies_to_the_next_typed_text() {
    let (mut s, t) = session("ab");
    s.execute("text.edit", json!({"layer": t, "caret": 1})).unwrap();
    s.execute("layer.setText", json!({"layer": t, "range": [1, 1], "size": 12, "fill": [0, 1, 0]})).unwrap();
    assert!(doc(&s, t).is_uniform(), "a caret-only change doesn't touch the text");
    s.execute("text.insert", json!({"text": "Q"})).unwrap();
    let d = doc(&s, t);
    assert_eq!(d.text, "aQb");
    assert_eq!(d.style_at(1).size, 12.0);
    assert_eq!(d.style_at(1).fill, [0.0, 1.0, 0.0, 1.0]);
    assert_eq!(d.style_at(2).size, 60.0);
    // Typing again continues the new style (the character before the caret).
    s.execute("text.insert", json!({"text": "R"})).unwrap();
    assert_eq!(doc(&s, t).style_at(2).size, 12.0);
}

#[test]
fn clipboard_and_paste_formatting() {
    let (mut s, t) = session("red plain");
    s.execute("layer.setText", json!({"layer": t, "range": [0, 3], "fill": "#ff0000", "size": 20})).unwrap();
    s.execute("text.edit", json!({"layer": t, "select": [0, 3]})).unwrap();
    let r = s.execute("edit.copy", json!({})).unwrap();
    assert_eq!(r["text"], "red");
    // Paste keeps the copied formatting.
    s.execute("text.setSelection", json!({"start": 9})).unwrap();
    s.execute("edit.paste", json!({})).unwrap();
    let d = doc(&s, t);
    assert_eq!(d.text, "red plainred");
    assert_eq!(d.style_at(10).fill, [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(sel(&s), (12, 12));
    // Different text from the system clipboard pastes in the caret's style.
    s.execute("text.setSelection", json!({"start": 4})).unwrap();
    s.execute("edit.paste", json!({"text": "new "})).unwrap();
    assert_eq!(doc(&s, t).text, "red new plainred");
    assert_eq!(doc(&s, t).style_at(5).size, 60.0);
    // Paste Text and Match Formatting: the copied text in the caret's style.
    s.execute("text.setSelection", json!({"start": 8})).unwrap();
    s.execute("edit.pasteTextMatchFormatting", json!({})).unwrap();
    let d = doc(&s, t);
    assert_eq!(d.text, "red new redplainred");
    assert_eq!(d.style_at(9).fill, [1.0; 4]);
    // Paste Text Formatting Only onto a selection.
    s.execute("text.setSelection", json!({"start": 11, "end": 16})).unwrap();
    s.execute("edit.pasteTextFormattingOnly", json!({})).unwrap();
    let d = doc(&s, t);
    assert_eq!(d.style_at(12).fill, [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(d.style_at(12).size, 20.0);
    // Cut removes the selection.
    s.execute("text.setSelection", json!({"start": 0, "end": 4})).unwrap();
    assert_eq!(s.execute("edit.cut", json!({})).unwrap()["text"], "red ");
    assert_eq!(doc(&s, t).text, "new redplainred");
    // Not editing: Paste Text Formatting Only restyles the whole selected text layer.
    s.execute("text.endEdit", json!({})).unwrap();
    s.execute("layer.select", json!({"layers": [t]})).unwrap();
    s.execute("edit.pasteTextFormattingOnly", json!({})).unwrap();
    assert!(doc(&s, t).is_uniform());
    assert_eq!(doc(&s, t).size, 20.0);
}

#[test]
fn type_tool_layers_point_box_and_empty_layer_is_kept() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "C", "width": 640, "height": 360, "duration": 4})).unwrap();
    let t = s.execute("layer.newText", json!({"text": "", "position": [100, 200], "justify": "left", "edit": true})).unwrap()["layer"].as_u64().unwrap();
    assert_eq!(s.state.text_edit.as_ref().map(|e| e.layer.0), Some(t));
    // Leaving an untouched Type-tool layer keeps it, as After Effects does.
    s.execute("text.endEdit", json!({})).unwrap();
    assert_eq!(s.active_comp().unwrap().layers.len(), 1, "an empty Type-tool layer is kept, as in AE");
    let b = s.execute("layer.newText", json!({"text": "", "box": [40, 50, 300, 120], "edit": true})).unwrap()["layer"].as_u64().unwrap();
    s.execute("text.insert", json!({"text": "Boxed"})).unwrap();
    let d = doc(&s, b);
    assert_eq!(d.box_size, Some([300.0, 120.0]));
    assert_eq!(d.box_pos, [-150.0, -60.0]);
    let l = s.active_comp().unwrap().layers.iter().find(|l| l.id.0 == b).expect("the boxed layer").clone();
    assert_eq!(l.name, "Boxed", "named after its text");
    let pos = l.props.prop("transform/position").unwrap().value.as_vec3();
    assert_eq!([pos[0], pos[1]], [190.0, 110.0]);
    s.execute("text.endEdit", json!({})).unwrap();
    assert_eq!(s.active_comp().unwrap().layers.len(), 2, "both text layers stay, the empty one included");
    // Resizing the box (layer space) is an attribute too.
    s.execute("layer.setText", json!({"layer": b, "box": [-100, -60, 200, 120]})).unwrap();
    assert_eq!(doc(&s, b).box_size, Some([200.0, 120.0]));
    s.execute("layer.setText", json!({"layer": b, "vertical": true})).unwrap();
    assert!(doc(&s, b).vertical);
}

#[test]
fn edit_state_round_trips_through_serde() {
    let (mut s, t) = session("abc");
    s.execute("text.edit", json!({"layer": t, "select": [1, 2]})).unwrap();
    let j = serde_json::to_value(&s.state).unwrap();
    let back: crate::EditorState = serde_json::from_value(j).unwrap();
    let before = s.state.text_edit.clone().expect("editing");
    let after = back.text_edit.clone().expect("editing");
    assert_eq!((after.layer, after.anchor, after.caret), (before.layer, before.anchor, before.caret));
    assert_ne!(after.session, before.session, "a restored editor gets a process-unique replacement epoch");
    assert_ne!(after.session, 0, "the restored epoch is always a real session token");
}

#[test]
fn ime_session_epoch_tracks_replaced_text_and_layer_restoration() {
    let (mut s, t) = editing_all("abc");
    let first = ime_owner(&s);

    // Replacing Source Text is a new document even though the layer id stays.
    replace_text(&mut s, t, "replaced");
    let replaced = ime_owner(&s);
    assert_ne!(replaced, first, "layer.setText starts a new IME epoch");

    // Undo restores another document: it must not revive the token from before either edit.
    s.execute("edit.undo", json!({})).unwrap();
    let undone = ime_owner(&s);
    assert_ne!(undone, replaced);
    assert_ne!(undone, first);

    // Clear + immediate undo in the same command sequence: the layer is gone long enough to
    // end the session, and the restored layer cannot inherit a composition.
    begin_edit(&mut s, t);
    s.execute("edit.clear", json!({"layers": [t]})).unwrap();
    assert!(s.state.text_edit.is_none(), "clearing the edited layer ends the session");
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.state.text_edit.is_none(), "undo does not recreate the old composition session");
    begin_edit(&mut s, t);
    assert_ne!(ime_owner(&s), first);
}

#[test]
fn no_op_ime_deletion_keeps_a_static_plain_text_value_and_records_no_step() {
    use effectcraft_keyframe::Value as KV;
    let (mut s, t) = session("abc");
    // Legacy/imported text can still be stored as a plain Str rather than a TextDoc.
    let cid = s.active_comp_id().unwrap();
    let pr = std::sync::Arc::make_mut(&mut s.project)
        .comp_mut(cid)
        .unwrap()
        .layer_mut(effectcraft_project::LayerId(t))
        .unwrap()
        .props
        .prop_mut("text/sourceText")
        .unwrap();
    pr.value = KV::Str("abc".into());
    begin_edit_at(&mut s, t, 0);
    let steps = undo_steps(&s);

    // A non-zero before count at the document start deletes nothing.
    s.execute("text.deleteSurrounding", json!({"before": 1, "after": 0})).unwrap();

    let pr = s.active_comp().unwrap().layer(effectcraft_project::LayerId(t)).unwrap().props.prop("text/sourceText").unwrap();
    assert!(matches!(&pr.value, KV::Str(_)), "a no-op keeps the static Str variant");
    assert!(pr.keys.is_empty());
    assert_eq!(undo_steps(&s), steps, "a no-op adds no undo step");
}

#[test]
fn no_op_ime_deletion_does_not_add_a_keyframe_to_animated_source_text() {
    let (mut s, t) = editing("abc", 0);
    animate_source(&mut s, t);
    let lid = effectcraft_project::LayerId(t);
    let keys = |s: &Session| s.active_comp().unwrap().layer(lid).unwrap().props.prop("text/sourceText").unwrap().keys.len();
    assert_eq!(keys(&s), 1, "one key from the stopwatch");
    s.execute("time.set", json!({"time": 1.0})).unwrap();
    let steps = undo_steps(&s);

    // A non-zero before count at the document start deletes nothing, at a time with no key.
    s.execute("text.deleteSurrounding", json!({"before": 1, "after": 0})).unwrap();

    assert_eq!(keys(&s), 1, "a no-op does not key Source Text at the CTI");
    assert_eq!(undo_steps(&s), steps, "a no-op adds no undo step");
}

#[test]
fn property_writes_replace_the_edited_document_only_when_the_text_changes() {
    let (mut s, t) = session("abc");
    let other = s.execute("layer.newText", json!({"text": "other"})).unwrap()["layer"].as_u64().unwrap();
    begin_edit_at(&mut s, t, 1);
    let owner = ime_owner(&s);
    let steps = undo_steps(&s);

    // Writing the text the document already has is a no-op: same document, same owner, no step.
    set_source(&mut s, t, "abc");
    assert_ime_owner(&s, owner, "a no-op write keeps the IME owner");
    assert_eq!(undo_steps(&s), steps, "a no-op write records no undo step");
    replace_text(&mut s, t, "abc");
    assert_ime_owner(&s, owner, "a no-op layer.setText keeps the IME owner");
    assert_eq!(undo_steps(&s), steps, "a no-op layer.setText records no undo step");

    // Another property of the edited layer leaves its document alone.
    s.execute("prop.set", json!({"layer": t, "path": "transform/opacity", "value": 50})).unwrap();
    assert_ime_owner(&s, owner, "an unrelated property keeps the IME owner");

    // Source Text of another layer is not the editor's document either.
    set_source(&mut s, other, "changed");
    assert_ime_owner(&s, owner, "another layer's text keeps the IME owner");

    // Replacing the edited layer's Source Text is a document replacement: a new IME epoch.
    set_source(&mut s, t, "replaced");
    assert_text(&s, t, "replaced");
    assert_ime_epoch_changed(&s, owner, "replacing the document changes the IME owner");
}

#[test]
fn undo_and_redo_of_an_unrelated_property_keep_the_ime_owner_and_selection() {
    let (mut s, t) = editing("abcdef", 3);
    let owner = ime_owner(&s);

    s.execute("prop.set", json!({"layer": t, "path": "transform/opacity", "value": 50})).unwrap();
    assert_ime_owner(&s, owner, "the opacity write keeps the owner");
    assert_eq!(sel(&s), (3, 3), "the opacity write keeps the selection");

    // The opacity step does not change Source Text: restoring it keeps the same editor.
    s.execute("edit.undo", json!({})).unwrap();
    assert_ime_owner(&s, owner, "undoing only opacity keeps the owner");
    assert_eq!(sel(&s), (3, 3), "undoing only opacity keeps the selection");

    s.execute("edit.redo", json!({})).unwrap();
    assert_ime_owner(&s, owner, "redoing only opacity keeps the owner");
    assert_eq!(sel(&s), (3, 3), "redoing only opacity keeps the selection");
}

#[test]
fn a_style_only_text_change_and_its_undo_replace_the_editor_epoch() {
    let (mut s, t) = editing("abcdef", 3);
    let owner = ime_owner(&s);

    // The same characters with another run style are still a different document.
    s.execute("layer.setText", json!({"layer": t, "range": [1, 4], "size": 12})).unwrap();
    assert_eq!(doc(&s, t).text, "abcdef");
    assert_eq!(doc(&s, t).style_at(2).size, 12.0);
    assert_ime_epoch_changed(&s, owner, "a style-only change starts a new IME epoch");
    assert_eq!(sel(&s), (3, 3), "a same-length style change keeps the caret in range");
    let styled = ime_owner(&s);

    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(doc(&s, t).style_at(2).size, 60.0);
    assert_ime_epoch_changed(&s, styled, "undoing a style change starts a new IME epoch");
}

#[test]
fn history_jumps_keep_the_owner_over_unrelated_steps_and_replace_it_when_the_text_changes() {
    let (mut s, t) = editing("abcdef", 3);
    let owner = ime_owner(&s);
    s.execute("prop.set", json!({"layer": t, "path": "transform/opacity", "value": 50})).unwrap();
    s.execute("prop.set", json!({"layer": t, "path": "transform/opacity", "value": 25})).unwrap();
    let states = s.execute("edit.history.list", json!({})).unwrap()["states"].as_array().unwrap().clone();
    let cur = states.iter().find(|n| n["current"] == true).unwrap()["index"].as_u64().unwrap();
    let parent = states.iter().find(|n| n["index"].as_u64() == Some(cur)).unwrap()["parent"].as_u64().unwrap();

    // Both states show the same Source Text: jumping between them keeps the editor.
    s.execute("edit.history.goto", json!({"index": parent})).unwrap();
    assert_ime_owner(&s, owner, "jumping to another opacity state keeps the owner");
    assert_eq!(sel(&s), (3, 3), "and the selection");
    s.execute("edit.history.goto", json!({"index": cur})).unwrap();
    assert_ime_owner(&s, owner, "jumping back keeps the owner");
    assert_eq!(sel(&s), (3, 3), "and the selection");

    // A history state whose Source Text differs is a document replacement.
    set_source(&mut s, t, "abc");
    let changed = ime_owner(&s);
    assert_ne!(changed, owner);
    s.execute("edit.history.goto", json!({"index": parent})).unwrap();
    assert_text(&s, t, "abcdef");
    assert_ime_epoch_changed(&s, changed, "jumping to a different document starts a new epoch");
}

#[test]
fn atomic_batch_rollback_keeps_the_owner_for_unrelated_steps_and_replaces_it_for_text_changes() {
    let (mut s, t) = editing("abcdef", 3);
    let owner = ime_owner(&s);
    let r = s.execute(
        "engine.batch",
        json!({"atomic": true, "steps": [
            {"command": "prop.set", "params": {"layer": t, "path": "transform/opacity", "value": 50}},
            {"command": "no.suchCommand", "params": {}}
        ]}),
    );
    assert!(r.is_err());
    assert_ime_owner(&s, owner, "rolling back an unrelated step keeps the owner");
    assert_eq!(sel(&s), (3, 3), "and the selection");

    // A partially applied text replacement was still shown before the rollback restores the
    // original document, so the editor must not revive the composition it had before the batch.
    let r = s.execute(
        "engine.batch",
        json!({"atomic": true, "steps": [
            {"command": "layer.setText", "params": {"layer": t, "text": "ab"}},
            {"command": "no.suchCommand", "params": {}}
        ]}),
    );
    assert!(r.is_err());
    assert_text(&s, t, "abcdef");
    assert_ime_epoch_changed(&s, owner, "a rolled-back text change does not revive the old owner");
    assert_eq!(sel(&s), (3, 3), "the restored document keeps the caret in range");
}

#[test]
fn atomic_batch_rollback_after_a_transient_editor_restart_bumps_the_epoch() {
    let (mut s, t) = editing("abcdef", 3);
    let owner = ime_owner(&s);

    // `text.edit` restarts the session even on the layer that is already edited. The rollback
    // restores the pre-batch TextEdit (with the old token), so the intermediate restart must be
    // detected from the snapshot: neither the old composition nor its token may be revived.
    let r = s.execute(
        "engine.batch",
        json!({"atomic": true, "steps": [
            {"command": "text.edit", "params": {"layer": t, "caret": 1}},
            {"command": "no.suchCommand", "params": {}}
        ]}),
    );
    assert!(r.is_err());
    assert_eq!(sel(&s), (3, 3), "the restored selection is the pre-batch one");
    assert_ime_epoch_changed(&s, owner, "a canceled intermediate editor must not revive the old token");
}

#[test]
fn atomic_batch_rollback_after_a_cross_comp_editor_switch_bumps_the_epoch() {
    let (mut s, t) = session("abcdef");
    let original = s.active_comp_id().unwrap();
    // Give the other comp a layer with the same id and the same Source Text, so only the comp
    // and editor-session identity distinguishes the two editors.
    let layer = s.active_comp().unwrap().layer(effectcraft_project::LayerId(t)).unwrap().clone();
    let other = new_other_comp(&mut s);
    let other_id = effectcraft_project::ItemId(other);
    std::sync::Arc::make_mut(&mut s.project).comp_mut(other_id).unwrap().layers.push(layer);
    s.execute("comp.open", json!({"comp": original.0})).unwrap();
    begin_edit_at(&mut s, t, 3);
    let owner = ime_owner(&s);

    let r = s.execute(
        "engine.batch",
        json!({"atomic": true, "steps": [
            {"command": "comp.open", "params": {"comp": other}},
            {"command": "text.edit", "params": {"layer": t, "caret": 1}},
            {"command": "no.suchCommand", "params": {}}
        ]}),
    );
    assert!(r.is_err());
    assert_eq!(s.active_comp_id(), Some(original), "the batch restored the original comp");
    assert_eq!(sel(&s), (3, 3), "and the pre-batch selection");
    assert_ime_epoch_changed(&s, owner, "an identical layer in another comp must not revive the old token");
}

#[test]
fn an_atomic_batch_drops_the_events_its_failed_steps_queued() {
    let (mut s, _) = session("abcdef");
    let original = s.active_comp_id().unwrap();
    // `comp.new` opens the comp it creates; go back so the batch's switch is a real one.
    let other = new_other_comp(&mut s);
    s.execute("comp.open", json!({"comp": original.0})).unwrap();
    s.drain_events();
    let r = s.execute(
        "engine.batch",
        json!({"atomic": true, "steps": [
            {"command": "comp.open", "params": {"comp": other}},
            {"command": "no.suchCommand", "params": {}}
        ]}),
    );
    assert!(r.is_err());
    assert_eq!(s.active_comp_id(), Some(original), "the batch restored the original comp");
    assert!(
        !s.drain_events().iter().any(|e| matches!(e, crate::Event::OpenComp(id) if id.0 == other)),
        "the rolled-back step's OpenComp is not queued for the UI"
    );
}

#[test]
fn atomic_batch_rollback_after_closing_the_editor_does_not_revive_it() {
    let (mut s, t) = session("abcdef");
    let original = s.active_comp_id().unwrap();
    let other = new_other_comp(&mut s);
    s.execute("comp.open", json!({"comp": original.0})).unwrap();
    begin_edit_at(&mut s, t, 3);
    let owner = ime_owner(&s);

    // The failing state has no editor at all: the restoration brings the pre-batch one back and
    // must treat it as unverified rather than revive its composition.
    let r = s.execute(
        "engine.batch",
        json!({"atomic": true, "steps": [
            {"command": "comp.open", "params": {"comp": other}},
            {"command": "no.suchCommand", "params": {}}
        ]}),
    );
    assert!(r.is_err());
    assert!(s.state.text_edit.is_some(), "the batch restored the editor state");
    assert_eq!(sel(&s), (3, 3));
    assert_ime_epoch_changed(&s, owner, "a canceled editor close must not revive the old token");
}

#[test]
fn restoring_without_an_editor_snapshot_replaces_an_open_editor() {
    let (mut s, _) = editing("abcdef", 3);
    let owner = ime_owner(&s);

    // `None` means no editor was open when the state was replaced; an editor that is open
    // afterwards cannot prove it kept its document.
    s.sanitize_after_replacement(None);
    assert_ime_epoch_changed(&s, owner, "an unverified restored editor starts a new epoch");
    assert_eq!(sel(&s), (3, 3));
}

#[test]
fn undo_to_a_shorter_document_clamps_the_caret() {
    let (mut s, t) = editing("abc", 3);
    replace_text(&mut s, t, "abcdef");
    select_anchor(&mut s, 6, 6);
    let owner = ime_owner(&s);
    assert_eq!(sel(&s), (6, 6));

    s.execute("edit.undo", json!({})).unwrap();
    assert_text(&s, t, "abc");
    assert_eq!(sel(&s), (3, 3), "undoing to a shorter document clamps the caret");
    assert_ime_epoch_changed(&s, owner, "and starts a new epoch");
}

#[test]
fn the_ime_stamp_revision_follows_the_editor_not_unrelated_project_edits() {
    let (mut s, t) = editing("abcdef", 6);
    let revision = ime_stamp_revision(&s);

    s.execute("prop.set", json!({"layer": t, "path": "transform/opacity", "value": 50})).unwrap();
    assert_eq!(ime_stamp_revision(&s), revision, "an unrelated property write does not advance it");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(ime_stamp_revision(&s), revision, "undoing it does not either");

    let r = s.execute(
        "engine.batch",
        json!({"atomic": true, "steps": [
            {"command": "prop.set", "params": {"layer": t, "path": "transform/opacity", "value": 25}},
            {"command": "no.suchCommand", "params": {}}
        ]}),
    );
    assert!(r.is_err());
    assert_eq!(ime_stamp_revision(&s), revision, "a rolled-back unrelated batch restores it");

    s.execute("text.insert", json!({"text": "x"})).unwrap();
    assert!(ime_stamp_revision(&s) > revision, "typing advances it");
    let typed = ime_stamp_revision(&s);
    s.execute("layer.setText", json!({"layer": t, "text": "y"})).unwrap();
    assert!(ime_stamp_revision(&s) > typed, "a replaced document advances it too");
}

#[test]
fn a_failed_atomic_batch_with_a_transient_edit_keeps_its_high_water_ime_revision() {
    let (mut s, t) = editing("abcdef", 6);
    let revision = ime_stamp_revision(&s);
    let r = s.execute(
        "engine.batch",
        json!({"atomic": true, "steps": [
            {"command": "text.insert", "params": {"text": "x"}},
            {"command": "text.delete", "params": {}},
            {"command": "no.suchCommand", "params": {}}
        ]}),
    );
    assert!(r.is_err());
    assert_text(&s, t, "abcdef");
    // The intermediate document edit is real even though the final document equals the
    // pre-batch one: its stamp must remain the high-water mark.
    assert!(ime_stamp_revision(&s) > revision, "a transient edit must not rewind the IME revision");
    let after_transient = ime_stamp_revision(&s);

    // An unrelated failed batch really did not edit the document: it must preserve the revision
    // exactly, or every wide-selection composition would be canceled by unrelated work.
    let r = s.execute(
        "engine.batch",
        json!({"atomic": true, "steps": [
            {"command": "prop.set", "params": {"layer": t, "path": "transform/opacity", "value": 25}},
            {"command": "no.suchCommand", "params": {}}
        ]}),
    );
    assert!(r.is_err());
    assert_eq!(ime_stamp_revision(&s), after_transient, "an unrelated rolled-back batch preserves the revision");
}

#[test]
fn a_replacing_property_write_clamps_the_selection_so_the_next_backspace_deletes() {
    let (mut s, t) = editing("abcdef", 6);
    let owner = ime_owner(&s);

    set_source(&mut s, t, "a");
    assert_text(&s, t, "a");
    assert_eq!(sel(&s), (1, 1), "the shortened document leaves the caret in range");
    assert_ne!(ime_owner(&s), owner);

    // Backspace at the clamped caret removes the character instead of no-opping on 5..6.
    s.execute("text.delete", json!({})).unwrap();
    assert_text(&s, t, "");
    assert_eq!(sel(&s), (0, 0));
}

#[test]
fn keyframe_writes_on_source_text_replace_the_edited_document_only_when_the_cti_value_changes() {
    let (mut s, t) = editing("abc", 1);
    // Keying the text the CTI already shows leaves the document equal.
    animate_source(&mut s, t);
    let owner = ime_owner(&s);

    set_source_cti_key(&mut s, t, 0.0, "abc");
    assert_ime_owner(&s, owner, "a no-op key write keeps the IME owner");

    // `keys.set` changes the key at the CTI: a document replacement.
    set_source_cti_key(&mut s, t, 0.0, "replaced");
    assert_text(&s, t, "replaced");
    assert_ime_epoch_changed(&s, owner, "a changed keyframe changes the IME owner");
}

#[test]
fn deleting_the_cti_keyframe_first_replaces_the_document_then_the_last_key_is_a_noop() {
    let (mut s, t) = editing("abc", 1);
    animate_source(&mut s, t);
    add_source_key(&mut s, t, 1.0, "later");
    let owner = ime_owner(&s);
    let lid = effectcraft_project::LayerId(t);
    let pr = s.active_comp().unwrap().layer(lid).unwrap().props.prop("text/sourceText").unwrap();
    let uid = pr.uid;
    let times: Vec<_> = pr.keys.iter().map(|k| k.time).collect();
    assert_eq!(times.len(), 2);

    // Deleting the key at the CTI (0s) while the 1s key remains shows the later text: replacing
    // the document starts a new epoch.
    s.state.selected_keys = vec![crate::KeyRef { layer: lid, prop: uid, time: times[0] }];
    assert_eq!(s.execute("keys.delete", json!({})).unwrap(), json!(1));
    assert_text(&s, t, "later");
    let replaced = ime_owner(&s);
    assert_ne!(replaced, owner, "deleting the CTI key changed the document");

    // Removing the last key turns its value into the static CTI value. That value is what the
    // CTI already showed, so the document does not change and the new owner stays.
    s.state.selected_keys = vec![crate::KeyRef { layer: lid, prop: uid, time: times[1] }];
    assert_eq!(s.execute("keys.delete", json!({})).unwrap(), json!(1));
    assert_text(&s, t, "later");
    assert_ime_owner(&s, replaced, "removing the last key is a document no-op");
}

#[test]
fn moving_the_cti_to_a_shorter_animated_key_clamps_and_starts_a_new_epoch() {
    let (mut s, t) = session("abcdef");
    animate_source(&mut s, t);
    add_source_key(&mut s, t, 1.0, "a");
    begin_edit_at(&mut s, t, 6);
    let owner = ime_owner(&s);

    // The same time still shows the same document: no new epoch.
    s.execute("time.set", json!({"time": 0.0})).unwrap();
    assert_text(&s, t, "abcdef");
    assert_ime_owner(&s, owner, "an unchanged CTI document keeps the IME owner");

    // The key at 1s holds shorter text: the document changes, the selection is clamped and the
    // editor gets a new epoch.
    s.execute("time.set", json!({"time": 1.0})).unwrap();
    assert_text(&s, t, "a");
    assert_eq!(sel(&s), (1, 1), "the shortened document leaves the caret in range");
    let replaced = ime_owner(&s);
    assert_ne!(replaced, owner, "a changed CTI document starts a new IME epoch");

    // Backspace at the clamped caret removes the character instead of no-opping on 5..6.
    s.execute("text.delete", json!({})).unwrap();
    assert_text(&s, t, "");
    assert_eq!(sel(&s), (0, 0));
    assert_ime_owner(&s, replaced, "typing after the replacement keeps its owner");

    // Staying at the same time after the edit is a no-op for the document too.
    s.execute("time.set", json!({"time": 1.0})).unwrap();
    assert_text(&s, t, "");
    assert_ime_owner(&s, replaced, "a no-op time move keeps the IME owner");
}

#[test]
fn sync_related_times_replaces_the_edited_document_when_it_moves_the_active_comp() {
    let (mut s, t) = session("abcdef");
    animate_source(&mut s, t);
    add_source_key(&mut s, t, 1.0, "a");
    let outer = s.active_comp_id().unwrap();
    let r = s.execute("layer.precompose", json!({"layers": [t], "name": "Inner", "open": true})).unwrap();
    let inner = effectcraft_project::ItemId(r["comp"].as_u64().unwrap());
    assert_eq!(s.active_comp_id(), Some(inner));

    begin_edit_at(&mut s, t, 6);
    let owner = ime_owner(&s);
    s.sync_related_times(outer, effectcraft_time::Tick::from_seconds_f64(1.0));

    assert_text(&s, t, "a");
    assert_eq!(sel(&s), (1, 1), "the related time move clamps the selection");
    assert_ime_epoch_changed(&s, owner, "the related time move changed the document");
}

#[test]
fn applying_a_preset_that_writes_source_text_replaces_the_edited_document() {
    let (mut s, t) = editing("abc", 1);
    let owner = ime_owner(&s);
    let preset = json!({
        "effectcraftAnimationPreset": 1,
        "properties": [{"path": "text/sourceText", "value": {"t": "Text", "v": {"text": "replaced"}}, "keys": []}],
    });
    s.execute("anim.applyPreset", json!({"preset": preset, "layers": [t]})).unwrap();
    assert_text(&s, t, "replaced");
    assert_ime_epoch_changed(&s, owner, "a preset that replaces source text changes the IME owner");
}

#[test]
fn a_failed_property_write_keeps_the_edited_document_and_its_owner() {
    let (mut s, t) = editing("abc", 1);
    let owner = ime_owner(&s);
    let steps = undo_steps(&s);

    // null cannot coerce to a Text value: the edit fails before anything is committed.
    assert!(s.execute("prop.set", json!({"layer": t, "path": "text/sourceText", "value": null})).is_err());
    assert_text(&s, t, "abc");
    assert_ime_owner(&s, owner, "a failed write keeps the IME owner");
    assert_eq!(undo_steps(&s), steps, "a failed write records no undo step");
}

#[test]
fn property_writes_off_the_cti_keep_the_owner_until_they_change_the_cti_document() {
    let (mut s, t) = editing("abc", 1);
    animate_source(&mut s, t);
    add_source_key(&mut s, t, 1.0, "later");
    let owner = ime_owner(&s);

    // Writing a key at 1s leaves the CTI (0s) value as it was.
    set_source_key(&mut s, t, 1.0, "changed");
    assert_text(&s, t, "abc");
    assert_ime_owner(&s, owner, "an off-CTI write keeps the IME owner");

    // Writing the CTI value itself replaces the document.
    set_source(&mut s, t, "cti");
    assert_text(&s, t, "cti");
    assert_ime_epoch_changed(&s, owner, "a CTI write changes the IME owner");
}

#[test]
fn unrelated_project_cleanup_keeps_the_ime_owner_but_replacement_changes_it() {
    let (mut s, t) = editing("abc", 1);
    let owner = ime_owner(&s);

    // file.removeUnusedFootage calls sanitize_state while the edited document is untouched.
    s.execute("file.importSolid", json!({"name": "Unused"})).unwrap();
    let removed = s.execute("file.removeUnusedFootage", json!({})).unwrap();
    assert_eq!(removed["removed"], json!(1), "the unused solid was actually removed");
    assert_ime_owner(&s, owner, "unrelated cleanup keeps the IME owner");

    // Replacing the Source Text is a document replacement: a new owner.
    replace_text(&mut s, t, "replaced");
    assert_ime_epoch_changed(&s, owner, "replacing the document changes the owner");
}

#[test]
fn ime_session_does_not_cross_a_composition_switch() {
    let (mut s, _) = editing_all("abc");
    let other = new_other_comp(&mut s);
    s.execute("comp.open", json!({"comp": other})).unwrap();
    assert!(s.state.text_edit.is_none(), "the old comp's editor is not reused in another comp");
}

#[test]
fn removing_the_active_comp_does_not_retarget_its_text_editor() {
    for replacement in [false, true] {
        let (mut s, t) = session("original");
        let original = s.active_comp_id().unwrap();
        let layer = s.active_comp().unwrap().layer(effectcraft_project::LayerId(t)).unwrap().clone();
        let other = new_other_comp(&mut s);
        let other_id = effectcraft_project::ItemId(other);
        std::sync::Arc::make_mut(&mut s.project).comp_mut(other_id).unwrap().layers.push(layer);
        s.execute("comp.open", json!({"comp": other})).unwrap();
        s.execute("comp.open", json!({"comp": original.0})).unwrap();
        begin_edit_at(&mut s, t, 1);
        assert!(s.state.text_edit.is_some());

        // Reproduce cleanup after a project replacement, bypassing Session::edit's earlier
        // document guard so this specifically exercises sanitize_state's fallback-comp branch.
        let edited = crate::commands::text_edit::active_editor_snapshot(&s);
        std::sync::Arc::make_mut(&mut s.project).items.remove(&original);
        if replacement {
            s.sanitize_after_replacement(edited.as_ref());
        } else {
            s.sanitize_state();
        }
        assert_eq!(s.active_comp_id(), Some(other_id));
        assert!(s.state.text_edit.is_none(), "the same-numbered layer belongs to a different comp");
        assert!(s.execute("text.insert", json!({"text": "wrong document"})).is_err());
        assert_eq!(doc(&s, t).text, "original", "the fallback comp was not edited");
    }
}

#[test]
fn opentype_features_per_range_render_and_query() {
    let (mut s, t) = session("Office 1/2");
    s.execute("layer.setText", json!({"layer": t, "range": [7, 10], "fractions": true, "ss01": true})).unwrap();
    let d = doc(&s, t);
    assert!(!d.style_at(0).opentype.fractions);
    assert!(d.style_at(8).opentype.fractions && d.style_at(8).opentype.stylistic_set(1));
    // The laid-out glyphs change (frac turns 1/2 into a fraction).
    let plain = effectcraft_text::layout_doc(&TextDoc { text: "1/2".into(), ..Default::default() });
    let frac = effectcraft_text::layout_doc(&d.slice(7..10));
    let ids = |l: &effectcraft_text::TextLayout| l.glyphs.iter().map(|g| g.gid).collect::<Vec<_>>();
    assert_ne!(ids(&plain), ids(&frac));
    // Undo restores the uniform style.
    s.execute("edit.undo", json!({})).unwrap();
    assert!(doc(&s, t).is_uniform());
    // Font feature query (font by name, or the layer's).
    let f = s.execute("text.fontFeatures", json!({"font": "Noto Serif"})).unwrap();
    assert_eq!(f["options"]["smallCaps"], true);
    assert!(f["features"].as_array().unwrap().iter().any(|t| t == "onum"));
    let f = s.execute("text.fontFeatures", json!({"layer": t})).unwrap();
    assert_eq!(f["family"], "Inter");
    assert!(f["options"]["stylisticSets"].as_array().unwrap().contains(&json!(1)));
    assert_eq!(f["options"]["smallCaps"], false);
}

#[test]
fn fonts_lists_bundled_and_installed_families() {
    let mut s = Session::default();
    let r = s.execute("text.fonts", json!({})).unwrap();
    let fams = r["families"].as_array().unwrap();
    assert_eq!(r["count"].as_u64().unwrap() as usize, fams.len());
    let inter = fams.iter().find(|f| f["family"] == "Inter").unwrap();
    assert_eq!(inter["origin"], "bundled");
    assert!(inter["styles"].as_array().unwrap().contains(&json!("Bold")));
    // Installed fonts are listed without any layer having asked for one.
    use effectcraft_text::fonts::FontSource as _;
    let installed = effectcraft_text::fonts::DirectorySource::system().faces();
    for face in &installed {
        assert!(fams.iter().any(|f| f["family"].as_str().unwrap().eq_ignore_ascii_case(&face.family)), "{} not listed", face.family);
    }
    // Filtered by name; rescanning keeps what was there.
    let r = s.execute("text.fonts", json!({"query": "noto ser", "rescan": true})).unwrap();
    assert_eq!(r["families"][0]["family"], "Noto Serif");
    // The bundled face is Regular; the OS can also provide Bold, Italic, or other faces.
    // Model `add_source()` on the independent directory inventory: a system face whose family
    // and primary style duplicate an earlier face is skipped, with its named instances.
    let mut expected_styles = std::collections::BTreeSet::from(["regular".to_string()]);
    let mut retained = std::collections::BTreeSet::from([("noto serif".to_string(), "regular".to_string())]);
    for face in installed.iter().filter(|face| face.family.eq_ignore_ascii_case("Noto Serif")) {
        if retained.insert((face.family.to_ascii_lowercase(), face.style.to_ascii_lowercase())) {
            expected_styles.insert(face.style.to_ascii_lowercase());
            expected_styles.extend(face.instances.iter().map(|i| i.style.to_ascii_lowercase()));
        }
    }
    let styles: Vec<String> = r["families"][0]["styles"].as_array().unwrap().iter().map(|style| style.as_str().unwrap().to_string()).collect();
    // `families()` keeps the first spelling of a style that differs only in case.
    assert_eq!(styles.iter().map(|style| style.to_ascii_lowercase()).collect::<std::collections::BTreeSet<_>>(), expected_styles);
    assert_eq!(styles.len(), expected_styles.len(), "style menu has no duplicates");
    assert!(r["families"].as_array().unwrap().iter().all(|f| f["family"].as_str().unwrap().to_lowercase().contains("noto ser")));
    // The style menus offer a family's own styles.
    assert_eq!(crate::font_styles("noto serif"), styles);
    assert_eq!(crate::font_styles("NOTO SERIF"), styles);
    assert_eq!(crate::font_styles("No Such Family").len(), 5);
}

/// A big-endian `u16` array, as sfnt tables store them.
fn u16s(v: &[u16]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_be_bytes()).collect()
}

/// A `name` table with one Windows/Unicode English record per `(id, text)`.
fn name_table(strings: &[(u16, String)]) -> Vec<u8> {
    let enc: Vec<(u16, Vec<u8>)> = strings.iter().map(|(id, s)| (*id, s.encode_utf16().flat_map(|u| u.to_be_bytes()).collect())).collect();
    let mut name = u16s(&[0, enc.len() as u16, 6 + 12 * enc.len() as u16]);
    let mut off = 0;
    for (id, st) in &enc {
        name.extend(u16s(&[3, 1, 0x409, *id, st.len() as u16, off]));
        off += st.len() as u16;
    }
    enc.iter().for_each(|(_, st)| name.extend(st));
    name
}

/// A single-face font with `tables` in order (zero checksums, offsets after the directory).
fn sfnt(tables: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let mut out = u16s(&[1, 0, tables.len() as u16, 0, 0, 0]);
    let mut body: Vec<u8> = vec![];
    for (tag, data) in tables {
        out.extend(*tag);
        out.extend([0u8; 4]);
        out.extend(((12 + 16 * tables.len() + body.len()) as u32).to_be_bytes());
        out.extend((data.len() as u32).to_be_bytes());
        body.extend(data);
    }
    out.extend(body);
    out
}

/// A font file of just `name` (the family, Regular) and an `fvar` table with one axis pinned at
/// 400 (so the helpers that look for an installed font with a real axis range never pick it).
fn pinned_variable_font(family: &str) -> Vec<u8> {
    let mut fvar = u16s(&[1, 0, 16, 2, 1, 20, 0, 8]);
    fvar.extend(b"wght".iter().copied().chain(u16s(&[0x190, 0, 0x190, 0, 0x190, 0, 0, 256])));
    let name = name_table(&[(1, family.to_owned()), (2, "Regular".to_owned())]);
    sfnt(&[(b"fvar", fvar), (b"name", name)])
}

/// The font menu's Variable Fonts filter (After Effects 26.3): `text.fonts {variable: true}`
/// lists only families with variation axes, every listing says which families are variable, and
/// the font menu's rows carry the flag the filter uses.
#[test]
fn fonts_filter_variable_families() {
    const FAMILY: &str = "EffectCraft Variable Filter Test";
    assert_eq!(effectcraft_text::fonts::add_font_data(pinned_variable_font(FAMILY)).len(), 1);
    let mut s = Session::default();
    let all = s.execute("text.fonts", json!({})).unwrap();
    let fams = all["families"].as_array().unwrap();
    let flag = |name: &str| fams.iter().find(|f| f["family"] == name).map(|f| f["variable"].clone());
    assert_eq!(flag("Inter"), Some(json!(false)), "the bundled fonts are static");
    assert_eq!(flag(FAMILY), Some(json!(true)));
    let var = s.execute("text.fonts", json!({"variable": true})).unwrap();
    let listed = var["families"].as_array().unwrap();
    assert_eq!(var["count"].as_u64(), Some(listed.len() as u64));
    assert!(listed.iter().all(|f| f["variable"] == true), "{var}");
    assert!(listed.iter().any(|f| f["family"] == FAMILY), "{var}");
    assert!(listed.len() < fams.len(), "the filter drops the static families");
    // Combined with the name query.
    let both = s.execute("text.fonts", json!({"variable": true, "query": "variable filter"})).unwrap();
    assert_eq!(both["families"].as_array().map(Vec::len), Some(1), "{both}");
    let none = s.execute("text.fonts", json!({"variable": true, "query": "jetbrains mono"})).unwrap();
    assert!(!none["families"].as_array().unwrap().iter().any(|f| f["family"] == "JetBrains Mono"), "{none}");
    // The font menu's rows.
    let menu = crate::font_menu(&s.prefs);
    assert!(menu.iter().any(|r| r.family == FAMILY && r.variable));
    assert!(menu.iter().any(|r| r.family == "Inter" && !r.variable));
}

/// A variable font of `name` and `fvar` only: one `wght` axis 100..900 (default 300, style
/// `Light`) and the named `instances` (style, `wght`), each with a PostScript name
/// `<family without spaces>-<style without spaces>`.
fn instanced_variable_font(family: &str, instances: &[(&str, f32)]) -> Vec<u8> {
    let squash = |s: &str| s.chars().filter(char::is_ascii_alphanumeric).collect::<String>();
    let mut strings: Vec<(u16, String)> = vec![(1, family.into()), (2, "Light".into())];
    let mut fvar = u16s(&[1, 0, 16, 2, 1, 20, instances.len() as u16, 10]);
    fvar.extend(b"wght".iter().copied().chain(u16s(&[100, 0, 300, 0, 900, 0, 0, 256])));
    for (i, (style, w)) in instances.iter().enumerate() {
        let id = 256 + 2 * i as u16;
        strings.push((id, (*style).into()));
        strings.push((id + 1, format!("{}-{}", squash(family), squash(style))));
        fvar.extend(u16s(&[id, 0]));
        fvar.extend(((w * 65536.0) as i32).to_be_bytes());
        fvar.extend(u16s(&[id + 1]));
    }
    sfnt(&[(b"fvar", fvar), (b"name", name_table(&strings))])
}

/// #1563: a variable font's named instances are styles of its family in `text.fonts`, and a
/// text layer set in one (by style, or by the instance's PostScript name as After Effects scripts
/// do) is laid out at the instance's axis values; an axis set on the text still wins.
#[test]
fn variable_font_named_instances_are_styles() {
    const FAMILY: &str = "EffectCraft Named Instance Test";
    assert_eq!(effectcraft_text::fonts::add_font_data(instanced_variable_font(FAMILY, &[("Light", 300.0), ("Regular", 400.0), ("Black", 900.0)])).len(), 1);
    let (mut s, t) = session("Hi");
    let fonts = s.execute("text.fonts", json!({"query": FAMILY})).unwrap();
    assert_eq!(fonts["families"][0]["styles"], json!(["Light", "Regular", "Black"]), "{fonts}");
    let wght = |s: &Session| -> Vec<Vec<(String, f32)>> { effectcraft_text::layout_doc(&doc(s, t)).glyphs.iter().map(|g| g.variations.clone()).collect() };
    let black = vec![("wght".to_string(), 900.0)];
    s.execute("layer.setText", json!({"layer": t, "font": FAMILY, "style": "Black"})).unwrap();
    let by_style = wght(&s);
    assert!(!by_style.is_empty() && by_style.iter().all(|v| v == &black), "{by_style:?}");
    s.execute("layer.setText", json!({"layer": t, "font": "EffectCraftNamedInstanceTest-Black", "style": "Regular"})).unwrap();
    let by_ps = wght(&s);
    assert!(!by_ps.is_empty() && by_ps.iter().all(|v| v == &black), "{by_ps:?}");
    s.execute("layer.setText", json!({"layer": t, "variations": {"wght": 650}})).unwrap();
    assert!(wght(&s).iter().all(|v| v == &[("wght".to_string(), 650.0)]), "{:?}", wght(&s));
}

/// `layer.setText`'s schema lists every OpenType key as optional, so checked execution (control
/// channel, MCP) accepts them and requires none (#520).
#[test]
fn set_text_schema_accepts_every_opentype_key_and_requires_none() {
    let spec = crate::commands::find("layer.setText").unwrap();
    let schema = crate::commands::params_schema(spec);
    assert_eq!(schema["required"], json!([]));
    for key in ["discretionaryLigatures", "ss01", "ss02", "ss20", "swash"] {
        assert!(schema["properties"].get(key).is_some(), "{key} is not in the schema");
    }
    assert!(schema["properties"].get("OpenType").is_none());
    let (mut s, t) = session("Office 1/2");
    s.execute_checked("layer.setText", json!({"layer": t, "discretionaryLigatures": true, "ss02": true, "ss20": true})).unwrap();
    assert!(doc(&s, t).style_at(0).opentype.stylistic_set(20));
}

/// Incoming IME replacement (stock `DeleteSurrounding` / the fork's `DeleteSurroundingBytes`):
/// the characters around the selection go, the selected text stays for the following commit, and
/// a byte count that ends inside a UTF-8 character leaves that character.
#[test]
fn ime_replacement_keeps_the_selection_and_never_splits_a_character() {
    let (mut s, t) = editing_all("abcdef");
    select(&mut s, 1, 4);
    // One character before "bcd" and everything after it: the selection survives for the commit.
    s.execute("text.deleteSurrounding", json!({"before": 1, "after": 99, "merge": "typing-0"})).unwrap();
    assert_text(&s, t, "bcd");
    assert_eq!(sel(&s), (0, 3), "the selection is kept for the commit that follows");
    s.execute("text.insert", json!({"text": "X", "merge": "typing-0"})).unwrap();
    assert_text(&s, t, "X");
    assert_eq!(sel(&s), (1, 1));
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(doc(&s, t).text, "abcdef", "the surrounding delete and its commit are one undo step");
    // An empty count is a no-op (deletion-only batches, empty commits), never an undo step.
    s.execute("edit.redo", json!({})).unwrap();
    select(&mut s, 0, 1);
    s.execute("text.deleteSurrounding", json!({"before": 0, "after": 0})).unwrap();
    assert_text(&s, t, "X");

    // Byte counts end inside a character: it stays whole (never a split UTF-8 sequence).
    let (mut s, t) = editing_all("A😀B");
    select(&mut s, 3, 3);
    s.execute("text.deleteSurrounding", json!({"before": 2, "after": 0, "bytes": true})).unwrap();
    assert_eq!(doc(&s, t).text, "A😀", "B is whole, the emoji only partly covered, so both are not one range");
    select(&mut s, 2, 2);
    s.execute("text.deleteSurrounding", json!({"before": 4, "after": 0, "bytes": true})).unwrap();
    assert_eq!(doc(&s, t).text, "A", "four bytes cover the whole emoji");

    // A combining mark is one character of two bytes: one byte of it removes nothing.
    let (mut s, t) = editing_all("e\u{301}");
    select(&mut s, 2, 2);
    s.execute("text.deleteSurrounding", json!({"before": 1, "after": 0, "bytes": true})).unwrap();
    assert_eq!(doc(&s, t).text, "e\u{301}", "a partial combining mark stays");
    s.execute("text.deleteSurrounding", json!({"before": 2, "after": 0, "bytes": true})).unwrap();
    assert_eq!(doc(&s, t).text, "e", "the whole combining mark is one character");
}

/// The outgoing IME query: logical committed text (the pre-edit never enters the model), UTF-8
/// byte offsets in either selection direction, a bounded window, and an explicit unavailable
/// answer for a selection that cannot be represented.
#[test]
fn ime_surrounding_maps_characters_to_bytes_and_preserves_direction() {
    let (mut s, _) = editing_all("café😀");
    select_anchor(&mut s, 2, 5);
    let w = ime_surrounding(&s).expect("an available document");
    assert_eq!(w.text, "café😀");
    assert_eq!((w.cursor, w.anchor), (9, 2), "the caret is six characters in, after the emoji; the anchor stays at byte 2");
    assert_eq!(&w.text[w.cursor..], "");
    assert_eq!(&w.text[..w.anchor], "ca");

    // Reversed: the caret stays the primary end and the anchor the other one.
    select_anchor(&mut s, 5, 2);
    let reversed = ime_surrounding(&s).expect("an available document");
    assert_eq!((reversed.cursor, reversed.anchor), (2, 9), "cursor > anchor is preserved after the conversion");
    assert_eq!(&reversed.text[reversed.cursor..reversed.anchor], "fé😀");

    // An available empty document is not the unavailable state.
    let (s, _) = editing_all("");
    let empty = ime_surrounding(&s).expect("empty text is available");
    assert_eq!((empty.text.as_str(), empty.cursor, empty.anchor), ("", 0, 0));

    // No edit session: unavailable, not an empty document.
    let (s, _) = session("text");
    assert!(ime_surrounding(&s).is_none());
}

#[test]
fn ime_surrounding_is_bounded_and_refuses_a_too_wide_selection() {
    // A long line: the query copies only a window around the caret, never the document.
    let text = "a".repeat(20_000);
    let (mut s, _) = editing(&text, 10_000);
    let w = ime_surrounding(&s).expect("a collapsed caret fits");
    assert!(w.text.len() <= MAX_IME_TEXT_BYTES, "the payload is bounded");
    assert!(w.text.len() > 1000, "the caret keeps a useful amount of context");
    assert!(w.text.is_char_boundary(w.cursor) && w.text.is_char_boundary(w.anchor));
    assert_eq!(w.cursor, w.anchor);

    // A selection wider than the protocol limit is unavailable, not a truncated one.
    select(&mut s, 0, 20_000);
    assert!(ime_surrounding(&s).is_none(), "a too-wide selection is unavailable");

    // Multi-byte text at the window boundary is never split, and offsets stay on boundaries.
    let text = "é".repeat(5_000);
    let (s, _) = editing(&text, 2_500);
    let w = ime_surrounding(&s).expect("a collapsed caret fits");
    assert!(w.text.len() <= MAX_IME_TEXT_BYTES);
    assert!(w.text.is_char_boundary(w.cursor));
    assert_eq!(&w.text[w.cursor..w.cursor + "é".len()], "é");
}

/// A reversed selection survives a byte-count deletion: both endpoints shift by the removed
/// characters before the selection, its direction stays reversed, and partial characters on
/// either side stay whole.
#[test]
fn ime_byte_deletion_keeps_a_reversed_selection_direction() {
    let (mut s, t) = editing_all("😀ab😀");
    select_anchor(&mut s, 3, 1);
    let (anchor, caret) = sel(&s);
    assert_eq!((anchor, caret), (3, 1), "reversed before the replacement");

    // Three bytes cover only part of either four-byte emoji: neither is split or removed.
    s.execute("text.deleteSurrounding", json!({"before": 3, "after": 3, "bytes": true})).unwrap();
    assert_text(&s, t, "😀ab😀");

    // Four bytes cover each whole emoji: the selection text stays, shifted, still reversed.
    s.execute("text.deleteSurrounding", json!({"before": 4, "after": 4, "bytes": true})).unwrap();
    assert_text(&s, t, "ab");
    assert_eq!(sel(&s), (2, 0), "the anchor stays after the caret");
    let w = ime_surrounding(&s).expect("available");
    assert_eq!((w.text.as_str(), w.cursor, w.anchor), ("ab", 0, 2), "the outgoing offsets keep the direction");
}

#[test]
fn deselect_all_ends_the_open_edit() {
    let (mut s, t) = editing("Hello", 5);
    s.execute("text.insert", json!({"text": "!"})).unwrap();
    assert_eq!(doc(&s, t).text, "Hello!");
    s.execute("edit.deselectAll", json!({})).unwrap();
    assert!(s.state.text_edit.is_none(), "Deselect All ends the open text edit on every route");
    assert!(s.state.selected_layers.is_empty(), "and leaves nothing selected");
    assert_eq!(doc(&s, t).text, "Hello!", "the text stays committed");
    // Ending an edit is no undoable step of its own (as with text.endEdit): undo takes
    // the typed text, it does not reopen the edit.
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(doc(&s, t).text, "Hello", "undo reverts the typed text, not the deselection");
    assert!(s.state.text_edit.is_none(), "undo does not reopen the edit");
}

#[test]
fn deselect_all_keeps_an_empty_type_tool_layer() {
    let (mut s, _keep) = session("Hello");
    let new = s.execute("layer.newText", json!({"text": "", "position": [80.0, 80.0], "edit": true})).unwrap()["layer"].as_u64().unwrap();
    assert!(
        s.state.text_edit.as_ref().is_some_and(|e| e.created && e.layer == effectcraft_project::LayerId(new)),
        "the Type tool opens the edit on its new, empty layer"
    );
    let before = layer_count(&s);
    s.execute("edit.deselectAll", json!({})).unwrap();
    assert!(s.state.text_edit.is_none());
    assert!(s.state.selected_layers.is_empty());
    assert_eq!(layer_count(&s), before, "an empty Type-tool layer is kept, as in After Effects");
}

#[test]
fn duplicate_ends_an_edit_on_already_committed_text() {
    // The engine route (MCP, control channel, script) with text that was committed before the
    // edit opened and nothing typed inside it: the edit still ends and the copy is selected.
    let (mut s, t) = editing("Hello", 5);
    let copy = s.execute("edit.duplicate", json!({})).unwrap()[0].as_u64().unwrap();
    assert!(s.state.text_edit.is_none(), "the open edit ends on a direct edit.duplicate");
    assert_eq!(s.state.selected_layers, vec![effectcraft_project::LayerId(copy)], "and the copy is selected");
    assert_eq!(doc(&s, t).text, "Hello", "the committed text is kept on the original");
    assert_eq!(doc(&s, copy).text, "Hello", "and carried by the copy");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(layer_count(&s), 1, "undo removes the copy");
    assert!(s.state.text_edit.is_none(), "undo does not reopen the edit");
}

#[test]
fn duplicate_during_an_edit_copies_the_layer_not_a_selected_property_group() {
    use effectcraft_project::LayerId;
    // Opening the editor by command, as a Type-tool click does, keeps the property selection, so
    // an effect, shape item or text animator can still be selected while the user types.
    for what in ["effect", "shape item", "text animator"] {
        let (mut s, t) = session("Hello");
        let (owner, path) = match what {
            "effect" => {
                s.execute("effect.apply", json!({"layers": [t], "effect": "Gaussian Blur"})).unwrap();
                (t, "effects")
            }
            "shape item" => (s.execute("layer.newShape", json!({"kind": "rect"})).unwrap()["layer"].as_u64().unwrap(), "contents"),
            _ => {
                s.execute("layer.addTextAnimator", json!({"layer": t, "properties": ["opacity"]})).unwrap();
                (t, "text/animators")
            }
        };
        let items =
            |s: &Session| s.active_comp().unwrap().layer(LayerId(owner)).unwrap().props.group(path).unwrap().groups().map(|g| g.uid).collect::<Vec<_>>();
        let item = items(&s)[0];
        let n = items(&s).len();
        let before = layer_count(&s);
        s.execute("prop.select", json!({"layer": owner, "prop": item})).unwrap();
        begin_edit_at(&mut s, t, 5);
        let r = s.execute("edit.duplicate", json!({})).unwrap();
        assert!(s.state.text_edit.is_none(), "{what}: Duplicate ends the open edit");
        assert_eq!(items(&s).len(), n, "{what}: the selected {what} is not duplicated");
        assert_eq!(layer_count(&s), before + 1, "{what}: the edited layer is");
        assert_eq!(s.state.selected_layers, vec![LayerId(r[0].as_u64().unwrap())], "{what}: and its copy is selected");
        // The selection the edit passed over goes with it: the next Duplicate copies the selected
        // copy rather than reaching back to the original's {what}.
        s.execute("edit.duplicate", json!({})).unwrap();
        assert_eq!(items(&s).len(), n, "{what}: a second Duplicate leaves the original's {what} alone");
        assert_eq!(layer_count(&s), before + 2, "{what}: and copies the copy");
        // With no edit open, the selected item keeps its precedence over the layers.
        s.execute("prop.select", json!({"layer": owner, "prop": item})).unwrap();
        s.execute("edit.duplicate", json!({})).unwrap();
        assert_eq!(items(&s).len(), n + 1, "{what}: outside an edit Duplicate copies the selected {what}");
        assert_eq!(layer_count(&s), before + 2, "{what}: and no layer");
    }
}

#[test]
fn duplicate_during_an_edit_leaves_no_key_or_vertex_selected_on_the_original() {
    use effectcraft_project::LayerId;
    // Opening the editor keeps the keyframe and mask-vertex selection too. Once the copy takes the
    // selection, Clear deletes the copy, not a key or a vertex of the original.
    for what in ["keyframe", "mask vertex"] {
        let (mut s, t) = session("Hello");
        let mask = match what {
            "keyframe" => {
                for time in [0.0, 1.0] {
                    s.execute("prop.addKey", json!({"layer": t, "path": "transform/opacity", "time": time, "value": 50})).unwrap();
                }
                s.execute("keys.select", json!({"keys": [{"layer": t, "path": "transform/opacity", "time": 0.0}]})).unwrap();
                None
            }
            // The Pen leaves the new mask's last vertex selected.
            _ => Some(s.execute("mask.new", json!({"layer": t, "vertices": [[0, 0], [40, 0], [40, 40]], "closed": true})).unwrap()["mask"].as_u64().unwrap()),
        };
        let original = |s: &Session| {
            let l = s.active_comp().unwrap().layer(LayerId(t)).unwrap();
            match mask {
                None => l.props.prop("transform/opacity").unwrap().keys.len(),
                Some(m) => l.props.find_group(m).unwrap().get("path").unwrap().value.as_path().unwrap().vertices.len(),
            }
        };
        let (n, before) = (original(&s), layer_count(&s));
        begin_edit_at(&mut s, t, 5);
        let copy = s.execute("edit.duplicate", json!({})).unwrap()[0].as_u64().unwrap();
        assert_eq!(s.state.selected_layers, vec![LayerId(copy)], "{what}: the copy is selected");
        s.execute("edit.clear", json!({})).unwrap();
        assert_eq!(layer_count(&s), before, "{what}: Clear deletes the selected copy");
        assert_eq!(original(&s), n, "{what}: and leaves the original's {what} alone");
    }
}

#[test]
fn duplicate_during_an_edit_copies_the_edited_layer_after_another_is_selected() {
    use effectcraft_project::LayerId;
    // An engine route (MCP, control channel, script) can select another layer while the edit
    // stays open. The implicit Duplicate still means the edited layer, as Clear does; a named
    // target keeps its meaning.
    let (mut s, t) = session("Hello");
    let other = s.execute("layer.newText", json!({"text": "Other"})).unwrap()["layer"].as_u64().unwrap();
    begin_edit_at(&mut s, t, 5);
    s.execute("layer.select", json!({"layer": other})).unwrap();
    assert!(s.state.text_edit.as_ref().is_some_and(|e| e.layer == LayerId(t)), "selecting another layer leaves the edit open");
    let before = layer_count(&s);
    let copy = s.execute("edit.duplicate", json!({})).unwrap()[0].as_u64().unwrap();
    assert!(s.state.text_edit.is_none(), "Duplicate ends the open edit");
    assert_eq!(layer_count(&s), before + 1, "one layer is copied");
    assert_eq!(doc(&s, copy).text, "Hello", "the edited layer is copied, not the newly selected one");
    assert_eq!(s.state.selected_layers, vec![LayerId(copy)], "and its copy is selected");
    // Null parameters name no layer, as everywhere else layers are read.
    for unnamed in [json!({"layers": null}), json!({"layer": null})] {
        begin_edit_at(&mut s, t, 5);
        s.execute("layer.select", json!({"layer": other})).unwrap();
        let copy = s.execute("edit.duplicate", unnamed.clone()).unwrap()[0].as_u64().unwrap();
        assert_eq!(doc(&s, copy).text, "Hello", "{unnamed}: the edited layer is copied");
    }
    begin_edit_at(&mut s, t, 5);
    let named = s.execute("edit.duplicate", json!({"layer": other})).unwrap()[0].as_u64().unwrap();
    assert_eq!(doc(&s, named).text, "Other", "a named layer is copied as named");
}

#[test]
fn duplicate_during_an_edit_with_nothing_selected_copies_the_edited_layer() {
    use effectcraft_project::LayerId;
    // An engine route can also clear the selection while the edit stays open. Duplicate stays
    // available, since its unnamed target is the edited layer, not the selection.
    let (mut s, _) = editing("Hello", 5);
    s.execute("layer.select", json!({"layers": []})).unwrap();
    assert!(s.state.selected_layers.is_empty() && s.state.text_edit.is_some(), "the edit is open with nothing selected");
    let copy = s.execute("edit.duplicate", json!({})).unwrap()[0].as_u64().unwrap();
    assert!(s.state.text_edit.is_none(), "Duplicate ends the open edit");
    assert_eq!(doc(&s, copy).text, "Hello", "and copies the edited layer");
    assert_eq!(s.state.selected_layers, vec![LayerId(copy)], "which is selected");
    // With no edit open, nothing selected still means nothing to duplicate.
    s.execute("layer.select", json!({"layers": []})).unwrap();
    let before = layer_count(&s);
    assert!(s.execute("edit.duplicate", json!({})).is_err(), "outside an edit Duplicate needs a selection");
    assert_eq!(layer_count(&s), before, "and copies nothing");
}

#[test]
fn duplicate_ends_the_open_edit_and_selects_the_copy() {
    let (mut s, t) = editing("Hello", 5);
    s.execute("text.insert", json!({"text": "!"})).unwrap();
    assert_eq!(doc(&s, t).text, "Hello!");
    let copy = s.execute("edit.duplicate", json!({})).unwrap()[0].as_u64().unwrap();
    // A deselected layer never keeps an open text edit: Duplicate commits the edit through the
    // same route as text.endEdit before the selection moves, so every route (menu, shortcut,
    // palette, MCP, script) leaves one final state.
    assert!(s.state.text_edit.is_none(), "Duplicate ends the open text edit, as Deselect All does");
    assert_eq!(s.state.selected_layers, vec![effectcraft_project::LayerId(copy)], "and selects the copy");
    assert_eq!(doc(&s, t).text, "Hello!", "the original keeps the typed text");
    assert_eq!(doc(&s, copy).text, "Hello!", "the copy carries it too");
    // The edit ending is no undoable step of its own (as with text.endEdit): undo takes the
    // duplicate, it does not reopen the edit or undo the typing.
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(layer_count(&s), 1, "undo removed the copy");
    assert_eq!(doc(&s, t).text, "Hello!", "the typed text stays committed");
    assert!(s.state.text_edit.is_none(), "undo does not reopen the edit");
    // With no open edit Duplicate is untouched by all of this: same behavior, no extra steps.
    s.execute("edit.duplicate", json!({"layers": [t]})).unwrap();
    assert_eq!(layer_count(&s), 2, "a plain duplicate still duplicates");
    assert!(s.state.text_edit.is_none());
}

#[test]
fn duplicate_of_an_empty_type_tool_layer_ends_the_edit_and_copies_it() {
    use effectcraft_project::LayerId;
    let (mut s, _keep) = session("Hello");
    let new = s.execute("layer.newText", json!({"text": "", "position": [80.0, 80.0], "edit": true})).unwrap()["layer"].as_u64().unwrap();
    assert!(s.state.text_edit.as_ref().is_some_and(|e| e.created && e.layer == LayerId(new)), "the Type tool opens the edit on its new, empty layer");
    let before = layer_count(&s);
    let copy = s.execute("edit.duplicate", json!({})).unwrap()[0].as_u64().unwrap();
    assert!(s.state.text_edit.is_none(), "the edit ends the way text.endEdit ends it");
    assert_eq!(layer_count(&s), before + 1, "the empty layer is kept, as text.endEdit keeps it, and copied");
    assert_eq!(s.state.selected_layers, vec![LayerId(copy)], "and the copy is selected");
}
