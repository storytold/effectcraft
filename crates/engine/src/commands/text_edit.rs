//! On-canvas text editing (Type tool): the edit session (layer, selection, insertion style), caret
//! movement, typing, deleting, the text clipboard and Paste Text and Match Formatting / Paste Text
//! Formatting Only. Every change to the text goes through an undoable edit of Source Text at the
//! CTI; the selection lives in [`EditorState::text_edit`] so agents and the UI share it.
//!
//! Positions are character indices. The selection is `anchor..caret` (either order).

use effectcraft_keyframe::{CharStyle, TextDoc, Value as KV};
use effectcraft_project::{ItemId, LayerId, LayerSource};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Session tokens for the on-canvas editor, unique across the process (so a recreated session
/// never reuses a live IME context). Exhaustion wraps; it cannot panic.
static NEXT_EDIT_SESSION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub(super) fn next_edit_session() -> u64 {
    NEXT_EDIT_SESSION.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Process-global checked high-water allocator for [`crate::Session::ime_revision`] stamps.
///
/// egui's unavailable-IME receipt identity must be process-monotonic. Allocating the stamp from
/// one process-global counter means neither a second session nor a rollback can reuse a value.
/// The counter fails closed at `u64::MAX`: it returns `None` without wrapping, and callers keep
/// their existing stamp. An unavailable before/after pair then compares equal, so it earns no
/// receipt rather than claiming a mutation with a reused revision.
static NEXT_IME_REVISION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(crate) fn next_ime_revision() -> Option<u64> {
    allocate_ime_revision(&NEXT_IME_REVISION)
}

fn allocate_ime_revision(counter: &std::sync::atomic::AtomicU64) -> Option<u64> {
    use std::sync::atomic::Ordering::Relaxed;
    let mut current = counter.load(Relaxed);
    loop {
        let next = current.checked_add(1)?;
        match counter.compare_exchange_weak(current, next, Relaxed, Relaxed) {
            Ok(_) => return Some(next),
            Err(observed) => current = observed,
        }
    }
}

#[test]
fn ime_revision_allocation_stops_at_exhaustion_without_reusing_a_stamp() {
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    let counter = AtomicU64::new(u64::MAX - 1);
    assert_eq!(allocate_ime_revision(&counter), Some(u64::MAX));
    assert_eq!(allocate_ime_revision(&counter), None);
    assert_eq!(allocate_ime_revision(&counter), None);
    assert_eq!(counter.load(Relaxed), u64::MAX);
}

use super::{CommandSpec, b_p, bad, comp_id, layer_mut, resolve_layer, str_p};
use crate::{EditorState, EngineError, Result, Session, cmd};

/// An on-canvas text editing session.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextEdit {
    pub layer: LayerId,
    pub anchor: usize,
    pub caret: usize,
    /// Character panel changes made with only a caret: the style the next typed text takes.
    #[serde(default)]
    pub pending: Option<CharStyle>,
    /// The layer was made by a Type tool click; leaving it empty deletes it.
    #[serde(default)]
    pub created: bool,
    /// Line position kept by repeated up / down moves (layout space).
    #[serde(default)]
    pub goal_x: Option<f64>,
    /// This editing session's identity.
    ///
    /// It changes when the editor is started and when its layer or document is replaced, by
    /// undo/redo, a history jump, a rollback, or a time change showing another document. A
    /// restore that keeps the same composition, layer and byte/style-equal document (an opacity
    /// undo while editing) keeps it, so an active IME composition survives an unrelated change.
    /// It is deliberately not serialized: a saved token belongs to a previous process and
    /// cannot identify a new target. The IME owner uses it as its replacement epoch, so an old
    /// composition is never inherited by a recreated layer or a replaced document.
    #[serde(skip, default = "next_edit_session")]
    pub session: u64,
}

impl Default for TextEdit {
    fn default() -> Self {
        Self { layer: LayerId(0), anchor: 0, caret: 0, pending: None, created: false, goal_x: None, session: next_edit_session() }
    }
}

impl TextEdit {
    pub fn range(&self) -> std::ops::Range<usize> {
        self.anchor.min(self.caret)..self.anchor.max(self.caret)
    }
    pub fn is_empty(&self) -> bool {
        self.anchor == self.caret
    }
}

/// The Source Text document of a layer in the active comp at the CTI (keyframed value).
pub fn layer_doc(s: &Session, lid: LayerId) -> Option<TextDoc> {
    let l = s.active_comp()?.layer(lid)?;
    match l.props.prop("text/sourceText")?.value_at(l.layer_time(s.time())) {
        KV::Text(d) => Some(*d),
        KV::Str(t) => Some(TextDoc::plain(&t)),
        _ => None,
    }
}

/// The largest committed-text window an IME observation copies.
///
/// This mirrors `egui::output::MAX_IME_SURROUNDING_TEXT_BYTES`: a selection wider than this is
/// reported as unavailable rather than silently truncated.
pub const MAX_IME_TEXT_BYTES: usize = 4000;

/// The editor's committed-text revision for an IME receipt whose surrounding text is
/// unavailable (a selection wider than [`MAX_IME_TEXT_BYTES`]).
///
/// Unlike [`Session::revision`], unrelated project edits (an opacity undo) do not advance it, so
/// they cannot make an active composition look stale.
pub fn ime_stamp_revision(s: &Session) -> u64 {
    s.ime_revision
}

/// Bounded committed text around an IME selection, with UTF-8 byte offsets.
///
/// The Type tool keeps the input method's pre-edit in egui's temporary data, never in the
/// layer's [`TextDoc`], so [`Self::text`] is committed text only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImeSurrounding {
    /// At most [`MAX_IME_TEXT_BYTES`] bytes of the edited layer's text.
    pub text: String,
    /// The caret (primary selection end) as a UTF-8 byte offset in [`Self::text`].
    pub cursor: usize,
    /// The other selection end; larger than `cursor` for a reversed selection.
    pub anchor: usize,
}

impl ImeSurrounding {
    /// A bounded window of `text` around the selection `cursor`/`anchor`.
    ///
    /// Returns `None` when an offset is past the text or inside a character, or when the
    /// selection itself is wider than [`MAX_IME_TEXT_BYTES`]: the caller then reports
    /// surrounding text as unavailable instead of sending a truncated selection.
    pub fn from_text(text: &str, cursor: usize, anchor: usize) -> Option<Self> {
        if cursor > text.len() || anchor > text.len() || !text.is_char_boundary(cursor) || !text.is_char_boundary(anchor) {
            return None;
        }
        let (start, end) = (cursor.min(anchor), cursor.max(anchor));
        let selection = end - start;
        if selection > MAX_IME_TEXT_BYTES {
            return None;
        }
        // Spend the remaining budget on both sides of the selection, giving unused room on one
        // side to the other. Only this bounded window is copied.
        let budget = MAX_IME_TEXT_BYTES - selection;
        let mut before = (budget / 2).min(start);
        let mut after = (budget - before).min(text.len() - end);
        before = (budget - after).min(start);
        after = (budget - before).min(text.len() - end);
        let mut window_start = start - before;
        let mut window_end = end + after;
        // Byte budgets can land inside a multi-byte character: shrink the window so it never
        // splits one and the selection stays inside it.
        while window_start < start && !text.is_char_boundary(window_start) {
            window_start += 1;
        }
        while end < window_end && !text.is_char_boundary(window_end) {
            window_end -= 1;
        }
        Some(Self {
            text: text.get(window_start..window_end)?.to_owned(),
            cursor: cursor.checked_sub(window_start)?,
            anchor: anchor.checked_sub(window_start)?,
        })
    }
}

/// The edited layer's committed text around its selection, or `None` when there is no edit
/// session, the layer is gone, or the selection is wider than [`MAX_IME_TEXT_BYTES`].
///
/// The caret and the anchor are converted to UTF-8 byte offsets separately, so a reversed
/// selection stays reversed. A valid empty document is `Some` with empty text, never `None`.
pub fn ime_surrounding(s: &Session) -> Option<ImeSurrounding> {
    let e = s.state.text_edit.as_ref()?;
    let comp = s.active_comp()?;
    let layer = comp.layer(e.layer)?;
    let prop = layer.props.prop("text/sourceText")?;
    // A static Source Text (the normal case) is borrowed in place; only an animated one has to
    // be evaluated into a temporary. Either way only the bounded window is copied.
    if !prop.is_animated() {
        return match &prop.value {
            KV::Text(doc) => {
                let (caret, anchor) = doc_bytes(doc, e.caret, e.anchor);
                ImeSurrounding::from_text(&doc.text, caret, anchor)
            }
            KV::Str(text) => {
                let (caret, anchor) = char_bytes(text, e.caret, e.anchor);
                ImeSurrounding::from_text(text, caret, anchor)
            }
            _ => None,
        };
    }
    match prop.value_at(layer.layer_time(s.time())) {
        KV::Text(doc) => {
            let (caret, anchor) = doc_bytes(&doc, e.caret, e.anchor);
            ImeSurrounding::from_text(&doc.text, caret, anchor)
        }
        KV::Str(text) => {
            let (caret, anchor) = char_bytes(&text, e.caret, e.anchor);
            ImeSurrounding::from_text(&text, caret, anchor)
        }
        _ => None,
    }
}

/// Byte offsets of two character positions in `text` (clamped to its end), scanning once.
///
/// The second position may be before the first; the returned pair keeps the caller's order.
fn char_bytes(text: &str, first: usize, second: usize) -> (usize, usize) {
    let (lo, hi) = if first <= second { (first, second) } else { (second, first) };
    let mut lo_byte = if lo == 0 { 0 } else { text.len() };
    let mut hi_byte = if hi == 0 { 0 } else { text.len() };
    if hi != 0 {
        for (i, (b, _)) in text.char_indices().enumerate() {
            if i == lo {
                lo_byte = b;
            }
            if i == hi {
                hi_byte = b;
                break;
            }
        }
    }
    if first <= second { (lo_byte, hi_byte) } else { (hi_byte, lo_byte) }
}

/// Byte offsets of the editor's caret and anchor in a document, in that order.
fn doc_bytes(doc: &TextDoc, caret: usize, anchor: usize) -> (usize, usize) {
    char_bytes(&doc.text, caret, anchor)
}

/// The text layer a text command targets: `layer`, the edited layer, or the selected text layer.
fn target(s: &Session, p: &Value, cmd: &str) -> Result<LayerId> {
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let lid = match p.get("layer") {
        Some(v) => resolve_layer(comp, v).ok_or_else(|| bad(cmd, format!("no layer {v}")))?,
        None => match &s.state.text_edit {
            Some(e) => e.layer,
            None => s
                .state
                .selected_layers
                .iter()
                .copied()
                .find(|id| comp.layer(*id).is_some_and(|l| matches!(l.source, LayerSource::Text)))
                .ok_or_else(|| bad(cmd, "select a text layer"))?,
        },
    };
    match comp.layer(lid) {
        Some(l) if matches!(l.source, LayerSource::Text) => Ok(lid),
        _ => Err(bad(cmd, "not a text layer")),
    }
}

/// The edit session on `lid` (started with the caret at the end when there is none).
fn session_on(s: &mut Session, lid: LayerId) -> TextEdit {
    match &s.state.text_edit {
        Some(e) if e.layer == lid => e.clone(),
        _ => {
            let n = layer_doc(s, lid).map_or(0, |d| d.char_len());
            let e = TextEdit { layer: lid, anchor: n, caret: n, session: next_edit_session(), ..Default::default() };
            s.state.selected_layers = vec![lid];
            s.state.text_edit = Some(e.clone());
            e
        }
    }
}

fn first_line_name(text: &str) -> String {
    text.lines().next().unwrap_or("").chars().take(40).collect()
}

/// Edit a layer's Source Text at the CTI (one undo step, or merged by `merge`), then set the
/// session's selection.
pub(crate) fn edit_doc(
    s: &mut Session,
    label: &str,
    merge: Option<&str>,
    lid: LayerId,
    f: impl FnOnce(&mut TextDoc, &mut EditorState) -> Result<()>,
) -> Result<()> {
    let cid = s.active_comp_id().ok_or(EngineError::NoComp)?;
    let t = s.time();
    let revision = s.revision;
    s.edit_text_doc(label, merge, |proj, st| {
        let l = layer_mut(proj, cid, lid)?;
        let lt = l.layer_time(t);
        let pr = l.props.prop_mut("text/sourceText").ok_or_else(|| bad("text", "not a text layer"))?;
        let mut doc = match pr.value_at(lt) {
            KV::Text(d) => *d,
            KV::Str(t) => TextDoc::plain(&t),
            _ => TextDoc::default(),
        };
        let original = doc.clone();
        f(&mut doc, st)?;
        // A no-op (an IME deletion with nothing around the selection, a set to the value it
        // already has) must not turn a static Str into Text or add a keyframe at the CTI: that
        // would change the project and record an empty undo step.
        if doc != original {
            let before = first_line_name(&original.text);
            let after = first_line_name(&doc.text);
            pr.set_value_at(lt, KV::Text(Box::new(doc)));
            // Text layers are named after their text until renamed.
            if before != after && !after.is_empty() && (l.name == before || l.name.starts_with("Text") || l.name.is_empty()) {
                l.name = after;
            }
        }
        Ok(())
    })?;
    // The editor's unavailable-text receipt identity follows only its own document: an edit to
    // another text layer while this editor is open must not cancel the composition.
    if s.revision != revision && s.state.text_edit.as_ref().is_some_and(|e| e.layer == lid) {
        s.note_text_edit_change();
    }
    Ok(())
}

fn range_p(p: &Value) -> Option<std::ops::Range<usize>> {
    let a = p.get("range")?.as_array()?;
    let g = |i: usize| a.get(i).and_then(Value::as_u64).map(|x| x as usize);
    let (x, y) = (g(0)?, g(1).or(g(0))?);
    Some(x.min(y)..x.max(y))
}

fn state_json(s: &Session) -> Value {
    match &s.state.text_edit {
        Some(e) => json!({"layer": e.layer.0, "anchor": e.anchor, "caret": e.caret, "start": e.range().start, "end": e.range().end}),
        None => Value::Null,
    }
}

/// `text.edit`: start editing a text layer.
fn edit(s: &mut Session, p: &Value) -> Result<Value> {
    let lid = target(s, p, "text.edit")?;
    let n = layer_doc(s, lid).map_or(0, |d| d.char_len());
    let (anchor, caret) = match p.get("select") {
        Some(Value::Array(a)) => {
            let g = |i: usize| a.get(i).and_then(Value::as_u64).map_or(n, |x| (x as usize).min(n));
            (g(0), g(1))
        }
        Some(Value::String(x)) if x == "none" => (n, n),
        _ => match p.get("caret").and_then(Value::as_u64) {
            Some(c) => ((c as usize).min(n), (c as usize).min(n)),
            None => (0, n),
        },
    };
    let created = b_p(p, "created").unwrap_or(false);
    s.state.selected_layers = vec![lid];
    s.state.text_edit = Some(TextEdit { layer: lid, anchor, caret, created, session: next_edit_session(), ..Default::default() });
    s.bump();
    Ok(state_json(s))
}

/// `text.endEdit`: stop editing (a Type-tool layer left empty is deleted).
pub(crate) fn end_edit(s: &mut Session, _: &Value) -> Result<Value> {
    let Some(e) = s.state.text_edit.take() else { return Ok(Value::Null) };
    if e.created && layer_doc(s, e.layer).is_some_and(|d| d.text.is_empty()) {
        let cid = s.active_comp_id().ok_or(EngineError::NoComp)?;
        s.edit("Delete Empty Text Layer", None, |proj, st| {
            if let Some(c) = proj.comp_mut(cid) {
                c.layers.retain(|l| l.id != e.layer);
            }
            st.selected_layers.retain(|l| *l != e.layer);
            Ok(())
        })?;
    }
    s.bump();
    Ok(Value::Null)
}

fn set_selection(s: &mut Session, p: &Value) -> Result<Value> {
    let lid = target(s, p, "text.setSelection")?;
    let n = layer_doc(s, lid).map_or(0, |d| d.char_len());
    let mut e = session_on(s, lid);
    let g = |k: &str| p.get(k).and_then(Value::as_u64).map(|x| (x as usize).min(n));
    let (a, c) = match (g("start").or(g("anchor")), g("end").or(g("caret"))) {
        (Some(a), Some(c)) => (a, c),
        (Some(a), None) | (None, Some(a)) => (a, a),
        (None, None) => match str_p(p, "select") {
            Some("all") => (0, n),
            _ => return Err(bad("text.setSelection", "give start/end (or anchor/caret), or select: \"all\"")),
        },
    };
    if (e.anchor, e.caret) != (a, c) {
        e.pending = None;
    }
    e.anchor = a;
    e.caret = c;
    e.goal_x = None;
    s.state.text_edit = Some(e);
    s.bump();
    Ok(state_json(s))
}

/// Caret targets for `text.moveCaret`.
fn move_target(s: &Session, e: &TextEdit, doc: &TextDoc, to: &str) -> Result<(usize, Option<f64>)> {
    let n = doc.char_len();
    let (lo, hi) = (e.range().start, e.range().end);
    let collapse = !e.is_empty();
    let lay = || effectcraft_text::layout_doc(doc);
    Ok(match to {
        "left" if collapse => (lo, None),
        "right" if collapse => (hi, None),
        "left" => (e.caret.saturating_sub(1), None),
        "right" => ((e.caret + 1).min(n), None),
        "wordLeft" => (effectcraft_keyframe::text_doc::word_left(&doc.text, e.caret), None),
        "wordRight" => (effectcraft_keyframe::text_doc::word_right(&doc.text, e.caret), None),
        "lineStart" | "home" => (lay().line_span(e.caret).0, None),
        "lineEnd" => (lay().line_span(e.caret).1, None),
        "up" | "down" => {
            let (c, g) = lay().move_vertical(e.caret, if to == "up" { -1 } else { 1 }, e.goal_x);
            (c, Some(g))
        }
        "paraStart" | "paraUp" => {
            let r = &doc.para_ranges()[doc.para_of(e.caret)];
            let pi = doc.para_of(e.caret);
            if e.caret == r.start && pi > 0 { (doc.para_ranges()[pi - 1].start, None) } else { (r.start, None) }
        }
        "paraEnd" | "paraDown" => {
            let ranges = doc.para_ranges();
            let pi = doc.para_of(e.caret);
            if e.caret == ranges[pi].end && pi + 1 < ranges.len() { (ranges[pi + 1].end, None) } else { (ranges[pi].end, None) }
        }
        "start" => (0, None),
        "end" | "docEnd" => (n, None),
        _ => {
            let _ = s;
            return Err(bad("text.moveCaret", format!("unknown target {to}")));
        }
    })
}

fn move_caret(s: &mut Session, p: &Value) -> Result<Value> {
    let lid = target(s, p, "text.moveCaret")?;
    let doc = layer_doc(s, lid).ok_or_else(|| bad("text.moveCaret", "not a text layer"))?;
    let e = session_on(s, lid);
    let to =
        str_p(p, "to").ok_or_else(|| bad("text.moveCaret", "give to: left|right|wordLeft|wordRight|lineStart|lineEnd|up|down|paraStart|paraEnd|start|end"))?;
    let extend = b_p(p, "extend").unwrap_or(false);
    // Extending moves the caret itself, not the collapsed selection edge.
    let probe = if extend { TextEdit { anchor: e.caret, ..e.clone() } } else { e.clone() };
    let (c, goal) = move_target(s, &probe, &doc, to)?;
    let mut e = e;
    e.caret = c;
    if !extend {
        e.anchor = c;
    }
    e.goal_x = goal;
    e.pending = None;
    s.state.text_edit = Some(e);
    s.bump();
    Ok(state_json(s))
}

/// Replace the selection (or `range`) with `text`; the caret goes after it.
fn insert(s: &mut Session, p: &Value) -> Result<Value> {
    let lid = target(s, p, "text.insert")?;
    let text = str_p(p, "text").ok_or_else(|| bad("text.insert", "give text"))?.replace("\r\n", "\n").replace('\r', "\n");
    let e = session_on(s, lid);
    let r = range_p(p).unwrap_or_else(|| e.range());
    let pending = e.pending.clone();
    let merge = str_p(p, "merge").map(str::to_string);
    edit_doc(s, "Edit Text", merge.as_deref(), lid, |doc, st| {
        let n = doc.char_len();
        let r = r.start.min(n)..r.end.min(n);
        doc.replace_range(r.clone(), &text, pending.as_ref());
        let c = r.start + text.chars().count();
        if let Some(ed) = st.text_edit.as_mut().filter(|ed| ed.layer == lid) {
            ed.anchor = c;
            ed.caret = c;
            ed.pending = None;
            ed.goal_x = None;
        }
        Ok(())
    })?;
    Ok(state_json(s))
}

/// Delete the selection, or one character / word before (backward) or after the caret.
fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    let lid = target(s, p, "text.delete")?;
    let doc = layer_doc(s, lid).ok_or_else(|| bad("text.delete", "not a text layer"))?;
    let e = session_on(s, lid);
    let n = doc.char_len();
    let forward = str_p(p, "direction") == Some("forward");
    let word = b_p(p, "word").unwrap_or(false);
    let r = match range_p(p) {
        Some(r) => r.start.min(n)..r.end.min(n),
        None if !e.is_empty() => e.range(),
        None if forward => e.caret..if word { effectcraft_keyframe::text_doc::word_right(&doc.text, e.caret) } else { (e.caret + 1).min(n) },
        None => (if word { effectcraft_keyframe::text_doc::word_left(&doc.text, e.caret) } else { e.caret.saturating_sub(1) })..e.caret,
    };
    if r.is_empty() {
        return Ok(state_json(s));
    }
    let merge = str_p(p, "merge").map(str::to_string);
    edit_doc(s, "Edit Text", merge.as_deref(), lid, |doc, st| {
        doc.replace_range(r.clone(), "", None);
        if let Some(ed) = st.text_edit.as_mut().filter(|ed| ed.layer == lid) {
            ed.anchor = r.start;
            ed.caret = r.start;
            ed.pending = None;
            ed.goal_x = None;
        }
        Ok(())
    })?;
    Ok(state_json(s))
}

/// How many whole characters `before_bytes` / `after_bytes` cover around `r`.
///
/// This is the input method's own byte count, as winit reports it: a count that ends inside a
/// UTF-8 character leaves that whole character (stock egui's `surrounding_bytes_to_chars`), so a
/// malformed or platform-specific offset never splits or corrupts the document.
fn surrounding_bytes_as_chars(text: &str, n: usize, r: std::ops::Range<usize>, before_bytes: usize, after_bytes: usize) -> (usize, usize) {
    let r = r.start.min(n)..r.end.min(n);
    let (start_byte, end_byte) = char_bytes(text, r.start, r.end);
    let from = start_byte.saturating_sub(before_bytes);
    let before = text.get(..start_byte).map_or(0, |head| head.char_indices().rev().take_while(|(i, _)| *i >= from).count());
    let after = text.get(end_byte..).map_or(0, |tail| tail.char_indices().take_while(|(i, c)| i.saturating_add(c.len_utf8()) <= after_bytes).count());
    (before.min(r.start), after.min(n.saturating_sub(r.end)))
}

/// Delete the text an input method marks for replacement around the selection
/// (`DeleteSurrounding`, `DeleteSurroundingBytes`).
///
/// Unlike [`delete`] with an explicit range, the **selection stays**: the characters before its
/// start and after its end are removed in two separate ranges, and the following `Commit`
/// replaces the selected text. `bytes` counts UTF-8 bytes (`before`/`after` are characters
/// otherwise); a count that ends inside a character leaves that character.
///
/// The counts are computed from the one document `edit_doc` already evaluated, so an animated
/// Source Text and a large static document are not copied or evaluated again just to decide what
/// the input method asked to replace.
fn delete_surrounding(s: &mut Session, p: &Value) -> Result<Value> {
    let lid = target(s, p, "text.deleteSurrounding")?;
    // Only an open edit session has a selection to keep: without one there is nothing an input
    // method could be replacing.
    let e = match &s.state.text_edit {
        Some(e) if e.layer == lid => e.clone(),
        _ => return Err(bad("text.deleteSurrounding", "not editing this text layer")),
    };
    let count = |k: &str| p.get(k).and_then(Value::as_u64).map_or(0, |v| usize::try_from(v).unwrap_or(usize::MAX));
    let (before_count, after_count) = (count("before"), count("after"));
    let bytes = b_p(p, "bytes").unwrap_or(false);
    let merge = str_p(p, "merge").map(str::to_string);
    edit_doc(s, "Edit Text", merge.as_deref(), lid, |doc, st| {
        let n = doc.char_len();
        let r = e.range().start.min(n)..e.range().end.min(n);
        let (before, after) = if bytes {
            surrounding_bytes_as_chars(&doc.text, n, r.clone(), before_count, after_count)
        } else {
            (before_count.min(r.start), after_count.min(n.saturating_sub(r.end)))
        };
        let start = r.start.saturating_sub(before);
        let end = r.end.saturating_add(after).min(n);
        if start == r.start && end == r.end {
            // Deletion-only batches with an empty count, or a document with nothing around the
            // selection: no empty undo step.
            return Ok(());
        }
        // The selection keeps its text: remove after it first, then before it, so the two ranges
        // stay valid and the caret/selection shift by the characters before it.
        if end > r.end {
            doc.replace_range(r.end..end, "", None);
        }
        if r.start > start {
            doc.replace_range(start..r.start, "", None);
        }
        if let Some(ed) = st.text_edit.as_mut().filter(|ed| ed.layer == lid) {
            let shift = r.start.saturating_sub(start);
            ed.anchor = ed.anchor.saturating_sub(shift);
            ed.caret = ed.caret.saturating_sub(shift);
            ed.pending = None;
            ed.goal_x = None;
        }
        Ok(())
    })?;
    Ok(state_json(s))
}

/// Selected text (plain) of the session, and its formatted fragment.
fn selection_doc(s: &Session) -> Option<TextDoc> {
    let e = s.state.text_edit.as_ref()?;
    let doc = layer_doc(s, e.layer)?;
    (!e.is_empty()).then(|| doc.slice(e.range()))
}

/// Edit ▸ Copy while editing text.
pub(crate) fn copy(s: &mut Session) -> Result<Value> {
    let frag = selection_doc(s).ok_or_else(|| bad("edit.copy", "select some text first"))?;
    let text = frag.text.clone();
    s.state.text_clipboard = Some(frag);
    Ok(json!({"text": text}))
}

/// Edit ▸ Cut while editing text.
pub(crate) fn cut(s: &mut Session) -> Result<Value> {
    let r = copy(s)?;
    delete(s, &json!({}))?;
    Ok(r)
}

/// Edit ▸ Paste while editing text: the copied text with its formatting, or `text` (from the
/// system clipboard) in the caret's style when it's something else.
pub(crate) fn paste(s: &mut Session, p: &Value) -> Result<Value> {
    let Some(e) = s.state.text_edit.clone() else { return Err(bad("edit.paste", "not editing text")) };
    let clip = s.state.text_clipboard.clone();
    match (str_p(p, "text"), clip) {
        (Some(t), Some(c)) if t.replace("\r\n", "\n") != c.text => insert(s, &json!({"text": t})),
        (_, Some(c)) => {
            let r = e.range();
            edit_doc(s, "Paste", None, e.layer, |doc, st| {
                doc.insert_doc(r.clone(), &c);
                let at = r.start + c.char_len();
                if let Some(ed) = st.text_edit.as_mut() {
                    ed.anchor = at;
                    ed.caret = at;
                    ed.pending = None;
                }
                Ok(())
            })?;
            Ok(state_json(s))
        }
        (Some(t), None) => insert(s, &json!({"text": t})),
        (None, None) => Err(bad("edit.paste", "nothing to paste")),
    }
}

fn editing_or_text_layer(s: &Session) -> std::result::Result<(), String> {
    if s.state.text_edit.is_some() {
        return Ok(());
    }
    super::has_comp(s)?;
    let comp = s.active_comp().ok_or("no composition")?;
    if s.state.selected_layers.iter().any(|id| comp.layer(*id).is_some_and(|l| matches!(l.source, LayerSource::Text))) {
        Ok(())
    } else {
        Err("edit text or select a text layer".into())
    }
}

fn has_text_clip(s: &Session) -> std::result::Result<(), String> {
    editing_or_text_layer(s)?;
    if s.state.text_clipboard.is_some() { Ok(()) } else { Err("copy some text first".into()) }
}

/// Edit ▸ Paste Text and Match Formatting: the clipboard's plain text in the style at the caret.
fn paste_match(s: &mut Session, p: &Value) -> Result<Value> {
    let text = match str_p(p, "text") {
        Some(t) => t.to_string(),
        None => s.state.text_clipboard.as_ref().map(|c| c.text.clone()).ok_or_else(|| bad("edit.pasteTextMatchFormatting", "nothing to paste"))?,
    };
    if s.state.text_edit.is_none() {
        // Not editing: replace the selected text layers' text, keeping their formatting.
        let lid = target(s, p, "edit.pasteTextMatchFormatting")?;
        edit_doc(s, "Paste Text and Match Formatting", None, lid, |doc, _| {
            doc.set_text(&text);
            Ok(())
        })?;
        return Ok(Value::Null);
    }
    let e = session_on(s, target(s, p, "edit.pasteTextMatchFormatting")?);
    let doc = layer_doc(s, e.layer).ok_or_else(|| bad("edit.pasteTextMatchFormatting", "not a text layer"))?;
    let style = e.pending.clone().unwrap_or_else(|| if e.is_empty() { doc.insertion_style(e.caret) } else { doc.style_at(e.range().start) });
    let r = e.range();
    edit_doc(s, "Paste Text and Match Formatting", None, e.layer, |doc, st| {
        doc.replace_range(r.clone(), &text, Some(&style));
        let at = r.start + text.chars().count();
        if let Some(ed) = st.text_edit.as_mut() {
            ed.anchor = at;
            ed.caret = at;
            ed.pending = None;
        }
        Ok(())
    })?;
    Ok(state_json(s))
}

/// Edit ▸ Paste Text Formatting Only: the copied text's character style (its first character)
/// and paragraph settings on the selected text, or on the whole of the selected text layers.
fn paste_formatting(s: &mut Session, p: &Value) -> Result<Value> {
    let clip = s.state.text_clipboard.clone().ok_or_else(|| bad("edit.pasteTextFormattingOnly", "copy some text first"))?;
    let style = clip.style_at(0);
    let para = clip.para(0);
    let targets: Vec<(LayerId, Option<std::ops::Range<usize>>)> = match &s.state.text_edit {
        Some(e) => vec![(e.layer, (!e.is_empty()).then(|| e.range()))],
        None => {
            let comp = s.active_comp().ok_or(EngineError::NoComp)?;
            let ids: Vec<LayerId> = match p.get("layer") {
                Some(_) => vec![target(s, p, "edit.pasteTextFormattingOnly")?],
                None => s.state.selected_layers.iter().copied().filter(|id| comp.layer(*id).is_some_and(|l| matches!(l.source, LayerSource::Text))).collect(),
            };
            ids.into_iter().map(|l| (l, None)).collect()
        }
    };
    if let Some(e) = s.state.text_edit.as_mut()
        && e.is_empty()
    {
        // Caret only: the next typed text takes the formatting.
        e.pending = Some(style);
        return Ok(state_json(s));
    }
    for (lid, r) in targets {
        edit_doc(s, "Paste Text Formatting Only", None, lid, |doc, _| {
            let n = doc.char_len();
            let r = r.clone().unwrap_or(0..n.max(1));
            doc.apply_style(r.clone(), |st| *st = style.clone());
            doc.apply_para(r, |pp| *pp = para.clone());
            Ok(())
        })?;
    }
    Ok(state_json(s))
}

/// Give the current editor a fresh replacement epoch after its document/layer was replaced.
pub(crate) fn invalidate_session(s: &mut Session) {
    let replaced = s.state.text_edit.is_some();
    if let Some(e) = s.state.text_edit.as_mut() {
        e.session = next_edit_session();
    }
    if replaced {
        s.note_text_edit_change();
    }
}

/// Clamp the editor selection and give it a fresh IME epoch after its document was replaced.
///
/// This is the one replacement path for document changes outside `edit_doc`:
/// [`crate::Session::edit`] calls [`after_external_change`] after every edit, [`crate::Session::set_time`]
/// and [`crate::Session::sync_related_times`] call it after moving the CTI, and
/// [`crate::Session::sanitize_after_replacement`] calls it with the editor document snapshotted
/// before a history restore or rollback.
/// Clamping first means a shortened document leaves the caret in range, so the next Backspace
/// deletes instead of being swallowed by an out-of-range selection.
pub(crate) fn document_replaced(s: &mut Session) {
    sanitize(s);
    invalidate_session(s);
}

/// The identity and document of the active on-canvas editor at one moment.
///
/// The composition and the editor session are part of the identity: a restoration must not keep
/// a composition alive when the snapshot came from another comp, or from an intermediate editor
/// that was restarted during the operation being rolled back.
#[derive(Clone, Debug, PartialEq)]
pub struct EditorSnapshot {
    /// The composition the editor was attached to.
    pub comp: ItemId,
    /// The edited layer.
    pub layer: LayerId,
    /// The editor session token, unique across the process.
    pub session: u64,
    /// The Source Text the editor was showing.
    pub doc: TextDoc,
}

/// The identity and document the active on-canvas editor is showing.
///
/// [`crate::Session::edit`] and the time setters snapshot this before a change and compare it
/// afterwards: only a change that actually replaces this document starts a new epoch, while a
/// write to another property, layer or composition, a move to a time showing the same text, or a
/// write that leaves the text as it was, keeps the editor's owner. With no editor open this is a
/// single `Option` check, with no document copy.
pub fn active_editor_snapshot(s: &Session) -> Option<EditorSnapshot> {
    let e = s.state.text_edit.as_ref()?;
    Some(EditorSnapshot { comp: s.active_comp_id()?, layer: e.layer, session: e.session, doc: layer_doc(s, e.layer)? })
}

/// Finish an edit outside the editor: if it replaced the document the editor owned, clamp the
/// selection and start a new IME epoch.
///
/// An edit that itself moves or closes the editor owns the resulting session, so an identity
/// mismatch is not a replacement here.
pub(crate) fn after_external_change(s: &mut Session, before: Option<&EditorSnapshot>) {
    let Some(before) = before else { return };
    let Some(e) = s.state.text_edit.as_ref() else { return };
    if s.state.active_comp != Some(before.comp) || e.layer != before.layer || e.session != before.session {
        return;
    }
    if layer_doc(s, before.layer).as_ref() != Some(&before.doc) {
        document_replaced(s);
    }
}

/// Finish a project-state replacement (undo, redo, a history jump, a rollback): keep the editor
/// only when the restored state still has the same composition, layer and editor session and
/// shows byte/style-equal text.
///
/// The restored session token is the one saved in the replaced state, so an intermediate
/// restart or cross-comp switch must be detected against the snapshot instead of being assumed
/// to be the editor's own change.
pub(crate) fn after_replacement(s: &mut Session, before: Option<&EditorSnapshot>) {
    let Some(before) = before else {
        // No editor was open when the snapshot was taken: a restored editor must not inherit a
        // composition from the state the replacement replaced.
        if s.state.text_edit.is_some() {
            document_replaced(s);
        }
        return;
    };
    let same = s.state.text_edit.as_ref().is_some_and(|e| s.state.active_comp == Some(before.comp) && e.layer == before.layer && e.session == before.session);
    if !same || layer_doc(s, before.layer).as_ref() != Some(&before.doc) {
        document_replaced(s);
    }
}

/// Keep the session valid after routine state cleanup: clamp the selection, drop it when the
/// layer is gone or not in the active comp.
///
/// This is also called after project-item cleanup that leaves the edited document alone, so it
/// must not advance [`TextEdit::session`]: that would make the viewer treat unrelated cleanup as
/// a document replacement and interrupt the active IME composition. Replacement paths call
/// [`document_replaced`] instead.
pub(crate) fn sanitize(s: &mut Session) {
    let Some(e) = s.state.text_edit.clone() else { return };
    match layer_doc(s, e.layer) {
        Some(d) => {
            let n = d.char_len();
            if let Some(ed) = s.state.text_edit.as_mut() {
                ed.anchor = ed.anchor.min(n);
                ed.caret = ed.caret.min(n);
            }
        }
        None => s.state.text_edit = None,
    }
}

/// The OpenType features of a font (`font` / `style`, else the target text layer's style at the
/// caret or its first character), and which Character panel options it sets with its own glyphs
/// (the others are synthesized or have no effect).
pub fn font_features(s: &mut Session, p: &Value) -> Result<Value> {
    let (family, style) = match p.get("font").and_then(Value::as_str) {
        Some(f) => (f.to_string(), p.get("style").and_then(Value::as_str).unwrap_or("Regular").to_string()),
        None => {
            let lid = target(s, p, "text.fontFeatures")?;
            let doc = layer_doc(s, lid).ok_or_else(|| bad("text.fontFeatures", "not a text layer"))?;
            let at = s.state.text_edit.as_ref().filter(|e| e.layer == lid).map_or(0, |e| e.range().start);
            let st = doc.style_at(at);
            (st.font, st.style)
        }
    };
    let r = effectcraft_text::resolve(&family, &style);
    let face = effectcraft_text::fonts::face(r.face);
    let feats = face.features();
    let has = |t: &str| feats.iter().any(|f| f == t);
    let sets: Vec<u32> = (1..=20u32).filter(|n| has(&format!("ss{n:02}"))).collect();
    Ok(json!({
        "family": face.info.family, "style": face.info.style, "missing": r.missing,
        "features": feats,
        "options": {
            "kerning": has("kern"), "ligatures": has("liga") || has("clig"), "discretionaryLigatures": has("dlig"),
            "contextualAlternates": has("calt"), "stylisticAlternates": has("salt"), "stylisticSets": sets,
            "swash": has("swsh"), "titling": has("titl"), "ordinals": has("ordn"), "fractions": has("frac"),
            "smallCaps": has("smcp"), "allSmallCaps": has("c2sc"), "superscript": has("sups"), "subscript": has("subs"),
            "oldStyleFigures": has("onum"), "liningFigures": has("lnum"), "tabularFigures": has("tnum"), "proportionalFigures": has("pnum"),
        },
    }))
}

/// The installed font families (bundled and system), with their styles, where each comes from
/// and its name in the font's own language. `query` keeps the families whose English or native
/// name contains it (case-insensitive); `rescan` first looks for fonts installed since the app
/// started.
pub fn fonts(_s: &mut Session, p: &Value) -> Result<Value> {
    use effectcraft_text::fonts;
    let added = if p.get("rescan").and_then(Value::as_bool).unwrap_or(false) { fonts::rescan_system() } else { 0 };
    let query = p.get("query").and_then(Value::as_str).map(str::to_lowercase).unwrap_or_default();
    let only_variable = p.get("variable").and_then(Value::as_bool).unwrap_or(false);
    let native = fonts::native_families();
    let variable = fonts::variable_families();
    let mut origin: std::collections::BTreeMap<String, &'static str> = Default::default();
    for f in fonts::all_faces() {
        origin.entry(f.info.family.clone()).or_insert(f.info.origin);
    }
    let list: Vec<Value> = effectcraft_text::families()
        .into_iter()
        .filter(|(f, _)| query.is_empty() || f.to_lowercase().contains(&query) || native.get(f).is_some_and(|n| n.to_lowercase().contains(&query)))
        .filter(|(f, _)| !only_variable || variable.contains(f))
        .map(|(f, styles)| {
            let mut v = json!({"family": f, "styles": styles, "origin": origin.get(&f).copied().unwrap_or("system"), "variable": variable.contains(&f)});
            if let Some(n) = native.get(&f) {
                v["nativeName"] = json!(n);
            }
            v
        })
        .collect();
    Ok(json!({"count": list.len(), "families": list, "added": added}))
}

fn always_ok(_: &Session) -> std::result::Result<(), String> {
    Ok(())
}

fn editing(s: &Session) -> std::result::Result<(), String> {
    if s.state.text_edit.is_some() { Ok(()) } else { Err("not editing text".into()) }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "text.edit",
            "Edit Text",
            [],
            None,
            "{layer?, select?: [anchor, caret]|\"none\", caret?, created?} → {layer, anchor, caret}",
            editing_or_text_layer,
            edit
        ),
        cmd!("text.endEdit", "Exit Text Editing", [], None, "{}", always_ok, end_edit),
        cmd!(
            "text.fontFeatures",
            "OpenType Features",
            [],
            None,
            "{layer?, font?, style?} → {family, style, features: [tags], options: {smallCaps, superscript, stylisticSets: [n], fractions, …}} (what the font sets with its own glyphs)",
            always_ok,
            font_features
        ),
        cmd!(
            "text.fonts",
            "List Fonts",
            [],
            None,
            "{query?, variable?, rescan?} → {count, families: [{family, styles, origin: bundled|system|user, variable, nativeName?}], added} (variable: only variable fonts; rescan picks up fonts installed since launch)",
            always_ok,
            fonts
        ),
        cmd!(
            "text.setSelection",
            "Set Text Selection",
            [],
            None,
            "{layer?, start, end?} | {anchor, caret} | {select: \"all\"} (character indices)",
            editing_or_text_layer,
            set_selection
        ),
        cmd!(
            "text.moveCaret",
            "Move Text Caret",
            [],
            None,
            "{to: left|right|wordLeft|wordRight|lineStart|lineEnd|up|down|paraStart|paraEnd|start|end, extend?}",
            editing,
            move_caret
        ),
        cmd!("text.insert", "Type Text", [], None, "{layer?, text, range?: [start, end], merge?} (replaces the selection)", editing_or_text_layer, insert),
        cmd!(
            "text.delete",
            "Delete Text",
            [],
            None,
            "{layer?, direction?: backward|forward, word?, range?: [start, end], merge?}",
            editing_or_text_layer,
            delete
        ),
        cmd!(
            "text.deleteSurrounding",
            "Delete Text Around Selection",
            [],
            None,
            "{layer?, before?, after?, bytes?, merge?} (input-method replacement: the characters before and after the selection are deleted, the selection stays; bytes counts UTF-8 bytes)",
            editing_or_text_layer,
            delete_surrounding
        ),
        cmd!(
            "edit.pasteTextMatchFormatting",
            "Paste Text and Match Formatting",
            ["Edit"],
            None,
            "{text?} (the clipboard's text in the style at the caret)",
            editing_or_text_layer,
            paste_match
        ),
        cmd!(
            "edit.pasteTextFormattingOnly",
            "Paste Text Formatting Only",
            ["Edit"],
            None,
            "{layer?} (the copied text's formatting on the selected text or text layers)",
            has_text_clip,
            paste_formatting
        ),
    ]
}

/// Whether a layer is being edited (for the Character / Paragraph panels).
pub fn editing_layer(s: &Session) -> Option<LayerId> {
    s.state.text_edit.as_ref().map(|e| e.layer)
}
