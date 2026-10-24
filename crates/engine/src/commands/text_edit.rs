//! On-canvas text editing (Type tool): the edit session (layer, selection, insertion style), caret
//! movement, typing, deleting, the text clipboard and Paste Text and Match Formatting / Paste Text
//! Formatting Only. Every change to the text goes through an undoable edit of Source Text at the
//! CTI; the selection lives in [`EditorState::text_edit`] so agents and the UI share it.
//!
//! Positions are character indices. The selection is `anchor..caret` (either order).

use effectcraft_keyframe::{CharStyle, TextDoc, Value as KV};
use effectcraft_project::{LayerId, LayerSource};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{CommandSpec, b_p, bad, comp_id, layer_mut, resolve_layer, str_p};
use crate::{EditorState, EngineError, Result, Session, cmd};

/// An on-canvas text editing session.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
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
            let e = TextEdit { layer: lid, anchor: n, caret: n, ..Default::default() };
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
    s.edit(label, merge, |proj, st| {
        let l = layer_mut(proj, cid, lid)?;
        let lt = l.layer_time(t);
        let pr = l.props.prop_mut("text/sourceText").ok_or_else(|| bad("text", "not a text layer"))?;
        let mut doc = match pr.value_at(lt) {
            KV::Text(d) => *d,
            KV::Str(t) => TextDoc::plain(&t),
            _ => TextDoc::default(),
        };
        let before = first_line_name(&doc.text);
        f(&mut doc, st)?;
        let after = first_line_name(&doc.text);
        pr.set_value_at(lt, KV::Text(Box::new(doc)));
        // Text layers are named after their text until renamed.
        if before != after && !after.is_empty() && (l.name == before || l.name.starts_with("Text") || l.name.is_empty()) {
            l.name = after;
        }
        Ok(())
    })
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
    s.state.text_edit = Some(TextEdit { layer: lid, anchor, caret, created, ..Default::default() });
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

/// Keep the session valid after undo / external edits: clamp the selection, drop it when the
/// layer is gone or not in the active comp.
pub(crate) fn sanitize(s: &mut Session) {
    let Some(e) = s.state.text_edit.clone() else { return };
    match layer_doc(s, e.layer) {
        Some(d) => {
            let n = d.char_len();
            if (e.anchor > n || e.caret > n)
                && let Some(ed) = s.state.text_edit.as_mut()
            {
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
