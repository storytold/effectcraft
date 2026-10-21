//! On-canvas text editing in the Composition viewer (Type tool, double-click a text layer).
//!
//! - **Type tool**: click empty space for point text, drag for paragraph (box) text, click a text
//!   layer to put the caret there. Cmd+T toggles horizontal / vertical type.
//! - **Editing**: caret at glyph boundaries, mouse click / drag selection, double-click selects a
//!   word and triple-click a line; arrows, Home / End and word / paragraph jumps (Cmd), Shift
//!   extends; typing, Backspace / Delete (Cmd or Alt: by word), Enter (new paragraph), Tab, Cmd+A,
//!   copy / cut / paste, IME composition; Alt+←/→ kerning (caret) or tracking (selection) ±20
//!   (Cmd: ±100); Cmd+Shift+< / > font size ±2; Cmd+Z / Cmd+Shift+Z undo / redo. Escape or
//!   Cmd+Enter commits. Paragraph text shows its box with handles that resize it.
//! - Selection highlight, caret and box are drawn in comp space through the layer's transform
//!   (and parenting).
//!
//! Every change runs an engine command (`text.*`, `layer.setText`, `edit.*`), so it's undoable and
//! agents can do the same over the control channel.

use effectcraft_engine::commands::text_edit::layer_doc;
use effectcraft_engine::geom::{Mat3, vec2 as gv2};
use effectcraft_engine::keyframe::{Kerning, TextDoc};
use effectcraft_engine::project::{Layer, LayerId, LayerSource};
use effectcraft_engine::render::EvalCtx;
use effectcraft_engine::text::kurbo::Point;
use effectcraft_engine::text::{TextLayout, layout_doc};
use egui::{Color32, Pos2, Rect, Stroke, StrokeKind, vec2};
use serde_json::{Value, json};

use super::viewer::ViewerMap;
use crate::EffectcraftApp;
use crate::dock::PanelKind;
use crate::state::Tool;

/// Pointer interaction owned by text editing (kept across frames while the button is down).
#[derive(Clone, Debug)]
enum Drag {
    /// Extending the selection from the press.
    Select { layer: LayerId },
    /// Type tool: a click (point text) or a drag (paragraph box) from `start` (comp space).
    Create { start: [f64; 2], vertical: bool },
    /// Resizing a paragraph box by one of its 8 handles.
    Resize { layer: LayerId, handle: usize, start_box: [f64; 4] },
}

fn drag_id() -> egui::Id {
    egui::Id::new("viewer-text-drag")
}
fn focus_id() -> egui::Id {
    egui::Id::new("viewer-text-focus")
}

/// The geometry of an edited text layer.
struct Edited {
    layer: Layer,
    doc: TextDoc,
    lay: TextLayout,
    /// Layer → comp.
    m: Mat3,
    inv: Mat3,
    /// Per caret position: static layout → animated / on-path character placement.
    carets: Vec<Mat3>,
}

impl Edited {
    fn to_screen(&self, map: &ViewerMap, p: (f64, f64)) -> Pos2 {
        let q = self.m.apply(gv2(p.0, p.1));
        map.to_screen([q.x, q.y])
    }
    fn to_layer(&self, map: &ViewerMap, s: Pos2) -> (f64, f64) {
        let c = map.to_comp(s);
        let q = self.inv.apply(gv2(c[0], c[1]));
        (q.x, q.y)
    }
    /// The text's region in layer space: the box, or the laid-out lines.
    fn region(&self) -> [f64; 4] {
        if let Some(b) = self.doc.box_size {
            return [self.doc.box_pos[0], self.doc.box_pos[1], self.doc.box_pos[0] + b[0], self.doc.box_pos[1] + b[1]];
        }
        let b = self.lay.layout.bounds;
        let corners = [(b[0], b[1]), (b[2], b[1]), (b[2], b[3]), (b[0], b[3])].map(|(x, y)| self.lay.to_layer * Point::new(x as f64, y as f64));
        let xs = corners.iter().map(|p| p.x);
        let ys = corners.iter().map(|p| p.y);
        let pad = 6.0;
        [
            xs.clone().fold(f64::INFINITY, f64::min) - pad,
            ys.clone().fold(f64::INFINITY, f64::min) - pad,
            xs.fold(f64::NEG_INFINITY, f64::max) + pad,
            ys.fold(f64::NEG_INFINITY, f64::max) + pad,
        ]
    }
    fn contains(&self, map: &ViewerMap, s: Pos2) -> bool {
        let (x, y) = self.to_layer(map, s);
        let r = self.region();
        x >= r[0] && x <= r[2] && y >= r[1] && y <= r[3]
    }
    fn hit(&self, map: &ViewerMap, s: Pos2) -> usize {
        let (x, y) = self.to_layer(map, s);
        self.lay.hit(Point::new(x, y))
    }
    /// Box handle positions (layer space): corners and edge midpoints, clockwise from top-left.
    fn handles(&self) -> Vec<(f64, f64)> {
        let Some(b) = self.doc.box_size else { return vec![] };
        let (x0, y0) = (self.doc.box_pos[0], self.doc.box_pos[1]);
        let (x1, y1) = (x0 + b[0], y0 + b[1]);
        let (mx, my) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        vec![(x0, y0), (mx, y0), (x1, y0), (x1, my), (x1, y1), (mx, y1), (x0, y1), (x0, my)]
    }
}

fn edited(app: &EffectcraftApp, ectx: &EvalCtx, lid: LayerId) -> Option<Edited> {
    let layer = ectx.comp.layer(lid)?.clone();
    if !matches!(layer.source, LayerSource::Text) {
        return None;
    }
    let doc = layer_doc(&app.session, lid)?;
    let lay = layout_doc(&doc);
    let m = super::viewer::l2c(ectx, &layer).0;
    let inv = m.inverse()?;
    let carets = effectcraft_engine::render::text::caret_maps(ectx, &layer, lay.chars);
    Some(Edited { layer, doc, lay, m, inv, carets })
}

/// Type-tool clicks prefer a selected text layer, as viewer dragging does (#230).
/// Double-clicks retain topmost picking. Both use the same visibility/lock/time/solo filter.
fn text_layer_at(app: &EffectcraftApp, ectx: &EvalCtx, map: &ViewerMap, s: Pos2, prefer_selected: bool) -> Option<Edited> {
    let mut topmost = None;
    for e in super::viewer::selectable_layers(ectx.comp, ectx.time)
        .filter(|l| matches!(l.source, LayerSource::Text))
        .filter_map(|l| edited(app, ectx, l.id))
        .filter(|e| e.contains(map, s))
    {
        if !prefer_selected || app.session.state.selected_layers.contains(&e.layer.id) {
            return Some(e);
        }
        if topmost.is_none() {
            topmost = Some(e);
        }
    }
    topmost
}

fn exec(app: &mut EffectcraftApp, id: &str, p: Value) -> Option<Value> {
    match app.session.execute(id, p) {
        Ok(v) => Some(v),
        Err(e) => {
            app.ui.status = e.to_string();
            None
        }
    }
}

/// The panels a text edit stays open across: the viewers, and Character, Paragraph and
/// Properties, which format the selected characters.
fn keeps_text_edit(p: PanelKind) -> bool {
    super::viewers::id_of(p).is_some() || matches!(p, PanelKind::Character | PanelKind::Paragraph | PanelKind::Properties)
}

/// A click in panel `p`. Any other panel (the Timeline, the Project panel, …) ends the edit, as a
/// click outside the text does, and gives the keyboard back to the app (#831).
pub fn panel_clicked(app: &mut EffectcraftApp, ctx: &egui::Context, p: PanelKind) {
    if app.session.state.text_edit.is_some() && !keeps_text_edit(p) {
        exec(app, "text.endEdit", json!({}));
        ctx.memory_mut(|m| m.surrender_focus(focus_id()));
    }
}

/// Typing merge key: consecutive keystrokes fold into one undo step until the caret moves.
fn typing_key(ui: &egui::Ui, bump: bool) -> String {
    let id = egui::Id::new("viewer-text-typing");
    let mut n: u64 = ui.data(|d| d.get_temp(id)).unwrap_or(0);
    if bump {
        n += 1;
        ui.data_mut(|d| d.insert_temp(id, n));
    }
    format!("text-typing-{n}")
}

/// Viewer hook: draws the edit overlay and handles text pointer and keyboard input. Returns
/// true when it owns this frame's pointer interaction (the viewer then skips its own gestures).
pub fn hook(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    map: &ViewerMap,
    ectx: &EvalCtx,
    resp: &egui::Response,
    space_pan: bool,
) -> bool {
    let tool = app.ui.tool;
    let type_tool = matches!(tool, Tool::Type | Tool::TypeVertical);
    let last_tool_id = egui::Id::new("viewer-text-last-tool");
    let last_tool = ui.data(|d| d.get_temp::<Tool>(last_tool_id));
    ui.data_mut(|d| d.insert_temp(last_tool_id, tool));
    if last_tool.is_some_and(|previous| matches!(previous, Tool::Type | Tool::TypeVertical) && !type_tool) {
        exec(app, "text.endEdit", json!({}));
        ui.data_mut(|d| d.remove::<Drag>(drag_id()));
    }
    // The session ends when its layer leaves the active comp.
    if let Some(e) = app.session.state.text_edit.clone()
        && ectx.comp.layer(e.layer).is_none()
    {
        exec(app, "text.endEdit", json!({}));
    }
    let editing = app.session.state.text_edit.clone();
    let cur = editing.as_ref().and_then(|e| edited(app, ectx, e.layer));
    let mut owns = false;
    let mut drag: Option<Drag> = ui.data(|d| d.get_temp(drag_id()));
    let (pressed, down, released, mods) = ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down(), i.pointer.primary_released(), i.modifiers));
    let press = ui.input(|i| i.pointer.press_origin());
    let pointer = ui.input(|i| i.pointer.interact_pos());
    let handle_hit = |e: &Edited, s: Pos2| e.handles().iter().position(|h| e.to_screen(map, *h).distance(s) < 7.0);

    // ---- press
    if pressed
        && !space_pan
        && resp.hovered()
        && !matches!(tool, Tool::Hand | Tool::Zoom)
        && let Some(pos) = press
    {
        let mut handled = false;
        if let Some(e) = &cur {
            if let Some(h) = handle_hit(e, pos) {
                let b = e.doc.box_size.unwrap_or([0.0; 2]);
                drag = Some(Drag::Resize { layer: e.layer.id, handle: h, start_box: [e.doc.box_pos[0], e.doc.box_pos[1], b[0], b[1]] });
                handled = true;
            } else if e.contains(map, pos) {
                let ci = e.hit(map, pos);
                let anchor = if mods.shift { editing.as_ref().map_or(ci, |sel| sel.anchor) } else { ci };
                exec(app, "text.setSelection", json!({"anchor": anchor, "caret": ci}));
                typing_key(ui, true);
                drag = Some(Drag::Select { layer: e.layer.id });
                handled = true;
            } else {
                // Clicking elsewhere commits the edit.
                exec(app, "text.endEdit", json!({}));
            }
        }
        if !handled && type_tool {
            if let Some(e) = text_layer_at(app, ectx, map, pos, true) {
                let ci = e.hit(map, pos);
                exec(app, "text.edit", json!({"layer": e.layer.id.0, "caret": ci}));
                drag = Some(Drag::Select { layer: e.layer.id });
            } else {
                drag = Some(Drag::Create { start: map.to_comp(pos), vertical: tool == Tool::TypeVertical });
            }
            handled = true;
        }
        if handled {
            ui.memory_mut(|m| m.request_focus(focus_id()));
            owns = true;
        }
    }

    // ---- drag / release
    match drag.clone() {
        Some(Drag::Select { layer }) => {
            owns = true;
            if down
                && let Some(p) = pointer
                && let Some(e) = edited(app, ectx, layer)
                && press.is_some_and(|q| q.distance(p) > 2.0)
            {
                let ci = e.hit(map, p);
                if app.session.state.text_edit.as_ref().is_some_and(|s| s.caret != ci) {
                    let anchor = app.session.state.text_edit.as_ref().map_or(ci, |s| s.anchor);
                    exec(app, "text.setSelection", json!({"anchor": anchor, "caret": ci}));
                }
            }
        }
        Some(Drag::Create { start, vertical }) => {
            owns = true;
            let end = pointer.map(|p| map.to_comp(p)).unwrap_or(start);
            let a = map.to_screen(start);
            let b = map.to_screen(end);
            let is_box = a.distance(b) > 6.0;
            if is_box && down {
                painter.rect_stroke(Rect::from_two_pos(a, b), 0.0, Stroke::new(1.0, app.tokens.accent), StrokeKind::Middle);
            }
            if released {
                let mut p = json!({"text": "", "edit": true, "size": 72, "fill": [1.0, 1.0, 1.0], "vertical": vertical});
                if is_box {
                    let (x0, y0) = (start[0].min(end[0]), start[1].min(end[1]));
                    p["box"] = json!([x0, y0, (end[0] - start[0]).abs(), (end[1] - start[1]).abs()]);
                } else {
                    p["position"] = json!([start[0], start[1]]);
                    p["justify"] = json!("left");
                }
                exec(app, "layer.newText", p);
                ui.memory_mut(|m| m.request_focus(focus_id()));
            }
        }
        Some(Drag::Resize { layer, handle, start_box }) => {
            owns = true;
            if down
                && let (Some(p), Some(q)) = (pointer, press)
                && let Some(e) = edited(app, ectx, layer)
            {
                let (px, py) = e.to_layer(map, p);
                let (qx, qy) = e.to_layer(map, q);
                let (dx, dy) = (px - qx, py - qy);
                let [mut x0, mut y0, w, h] = start_box;
                let (mut x1, mut y1) = (x0 + w, y0 + h);
                if matches!(handle, 0 | 6 | 7) {
                    x0 += dx;
                }
                if matches!(handle, 2..=4) {
                    x1 += dx;
                }
                if matches!(handle, 0..=2) {
                    y0 += dy;
                }
                if matches!(handle, 4..=6) {
                    y1 += dy;
                }
                let bx = [x0.min(x1), y0.min(y1), (x1 - x0).abs().max(4.0), (y1 - y0).abs().max(4.0)];
                exec(app, "layer.setText", json!({"layer": layer.0, "box": bx, "merge": format!("text-box-{}", layer.0)}));
            }
        }
        None => {}
    }
    // Double / triple click: word / line; double-click a text layer to edit it.
    if resp.triple_clicked()
        && let Some(p) = pointer
        && let Some(e) = app.session.state.text_edit.clone().and_then(|s| edited(app, ectx, s.layer))
        && e.contains(map, p)
    {
        let (a, b) = e.lay.line_span(e.hit(map, p));
        exec(app, "text.setSelection", json!({"anchor": a, "caret": b}));
        owns = true;
    } else if resp.double_clicked()
        && let Some(p) = pointer
    {
        let cur2 = app.session.state.text_edit.clone().and_then(|s| edited(app, ectx, s.layer));
        match cur2 {
            Some(e) if e.contains(map, p) => {
                let r = effectcraft_engine::keyframe::text_doc::word_at(&e.doc.text, e.hit(map, p));
                exec(app, "text.setSelection", json!({"anchor": r.start, "caret": r.end}));
                owns = true;
            }
            _ if !matches!(tool, Tool::Hand | Tool::Zoom) && !tool.is_pen() && !type_tool => {
                if let Some(e) = text_layer_at(app, ectx, map, p, false) {
                    exec(app, "text.edit", json!({"layer": e.layer.id.0}));
                    ui.memory_mut(|m| m.request_focus(focus_id()));
                    owns = true;
                }
            }
            _ => {}
        }
    }
    if released || !down {
        ui.data_mut(|d| d.remove::<Drag>(drag_id()));
    } else if let Some(d) = drag {
        ui.data_mut(|dd| dd.insert_temp(drag_id(), d));
    }

    // ---- keyboard
    if app.session.state.text_edit.is_some() && app.dialog.is_none() && ui.is_enabled() {
        // Register after pointer handling so new edits take focus in this frame. An inactive
        // editor must not take Tab focus and immediately surrender it before viewer controls.
        ui.interact(Rect::from_min_size(resp.rect.min, vec2(1.0, 1.0)), focus_id(), egui::Sense::focusable_noninteractive());
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(focus_id(), egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true });
        });
        // A session started elsewhere (menu, agent, Properties) takes the keyboard once…
        let seen = egui::Id::new("viewer-text-seen");
        let key = app.session.state.text_edit.as_ref().map(|e| e.layer.0);
        // …and gets it back when a click in the viewer or a formatting panel left nothing holding
        // it; not merely because nothing has the focus (#831).
        let regain = ui.memory(|m| m.focused().is_none()) && keeps_text_edit(app.ui.focused);
        if ui.data(|d| d.get_temp::<Option<u64>>(seen)).flatten() != key || regain {
            ui.data_mut(|d| d.insert_temp(seen, key));
            ui.memory_mut(|m| m.request_focus(focus_id()));
        }
        if ui.memory(|m| m.has_focus(focus_id())) {
            keyboard(app, ui, resp.rect);
        }
    } else {
        ui.data_mut(|d| d.insert_temp::<Option<u64>>(egui::Id::new("viewer-text-seen"), None));
        if ui.memory(|m| m.has_focus(focus_id())) {
            ui.memory_mut(|m| m.surrender_focus(focus_id()));
        }
    }

    // ---- overlay
    if let Some(s) = app.session.state.text_edit.clone()
        && let Some(e) = edited(app, ectx, s.layer)
    {
        draw(app, ui, painter, map, &e, s.range(), s.caret);
    }
    owns
}

/// This frame's keys, in order, into the edited text, up to a press outside the `viewer`: the
/// panel it lands in decides (after this frame's panels) whether the edit ends, so the keys after
/// it in this frame are neither typed nor run as shortcuts (#831).
fn keyboard(app: &mut EffectcraftApp, ui: &mut egui::Ui, viewer: Rect) {
    let events = ui.input(|i| i.events.clone());
    let ctx = ui.ctx().clone();
    for ev in events {
        let Some(sel) = app.session.state.text_edit.clone() else { return };
        match ev {
            egui::Event::PointerButton { pos, pressed: true, .. } if !viewer.contains(pos) => return,
            egui::Event::Text(t) if !t.chars().any(char::is_control) => {
                let k = typing_key(ui, false);
                exec(app, "text.insert", json!({"text": t, "merge": k}));
            }
            egui::Event::Ime(egui::ImeEvent::Commit(t)) if !t.is_empty() => {
                let k = typing_key(ui, false);
                exec(app, "text.insert", json!({"text": t, "merge": k}));
                ui.data_mut(|d| d.remove::<String>(egui::Id::new("viewer-text-preedit")));
            }
            egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) => {
                ui.data_mut(|d| d.insert_temp(egui::Id::new("viewer-text-preedit"), text));
            }
            egui::Event::Copy => {
                if let Some(v) = exec(app, "edit.copy", json!({}))
                    && let Some(t) = v["text"].as_str()
                {
                    ctx.copy_text(t.to_string());
                }
            }
            egui::Event::Cut => {
                if let Some(v) = exec(app, "edit.cut", json!({}))
                    && let Some(t) = v["text"].as_str()
                {
                    ctx.copy_text(t.to_string());
                }
                typing_key(ui, true);
            }
            egui::Event::Paste(t) => {
                exec(app, "edit.paste", json!({"text": t}));
                typing_key(ui, true);
            }
            egui::Event::Key { key, pressed: true, modifiers: m, .. } => {
                use egui::Key;
                let cmd = m.command;
                let mv = |app: &mut EffectcraftApp, to: &str| {
                    exec(app, "text.moveCaret", json!({"to": to, "extend": m.shift}));
                };
                match key {
                    Key::ArrowLeft | Key::ArrowRight if m.alt => {
                        // Kerning at the caret, tracking over a selection.
                        let step = if cmd { 100.0 } else { 20.0 } * if key == Key::ArrowLeft { -1.0 } else { 1.0 };
                        let doc = layer_doc(&app.session, sel.layer).unwrap_or_default();
                        if sel.is_empty() {
                            if sel.caret < doc.char_len() {
                                let cur = match doc.style_at(sel.caret).kerning {
                                    Kerning::Manual(v) => v,
                                    _ => 0.0,
                                };
                                exec(
                                    app,
                                    "layer.setText",
                                    json!({"layer": sel.layer.0, "range": [sel.caret, sel.caret + 1], "kerning": cur + step, "merge": format!("text-kern-{}", sel.caret)}),
                                );
                            }
                        } else {
                            let r = sel.range();
                            let cur = doc.style_at(r.start).tracking;
                            exec(app, "layer.setText", json!({"layer": sel.layer.0, "range": [r.start, r.end], "tracking": cur + step, "merge": "text-track"}));
                        }
                    }
                    Key::ArrowLeft => mv(app, if cmd { "wordLeft" } else { "left" }),
                    Key::ArrowRight => mv(app, if cmd { "wordRight" } else { "right" }),
                    Key::ArrowUp => mv(app, if cmd { "paraUp" } else { "up" }),
                    Key::ArrowDown => mv(app, if cmd { "paraDown" } else { "down" }),
                    Key::Home => mv(app, if cmd { "start" } else { "lineStart" }),
                    Key::End => mv(app, if cmd { "end" } else { "lineEnd" }),
                    Key::Backspace => {
                        let k = typing_key(ui, false);
                        exec(app, "text.delete", json!({"word": cmd || m.alt, "merge": k}));
                    }
                    Key::Delete => {
                        let k = typing_key(ui, false);
                        exec(app, "text.delete", json!({"direction": "forward", "word": cmd || m.alt, "merge": k}));
                    }
                    Key::Enter if cmd => {
                        exec(app, "text.endEdit", json!({}));
                    }
                    Key::Enter => {
                        let k = typing_key(ui, false);
                        exec(app, "text.insert", json!({"text": "\n", "merge": k}));
                    }
                    Key::Tab => {
                        let k = typing_key(ui, false);
                        exec(app, "text.insert", json!({"text": "\t", "merge": k}));
                    }
                    Key::Escape => {
                        exec(app, "text.endEdit", json!({}));
                    }
                    Key::A if cmd => {
                        exec(app, "text.setSelection", json!({"select": "all"}));
                    }
                    Key::Z if cmd => {
                        let _ = crate::menus::invoke(app, &ctx, if m.shift { "edit.redo" } else { "edit.undo" }, json!({}));
                    }
                    Key::Comma | Key::Period if cmd && m.shift => {
                        let doc = layer_doc(&app.session, sel.layer).unwrap_or_default();
                        let r = sel.range();
                        let size = sel.pending.as_ref().map_or_else(|| doc.style_at(r.start).size, |p| p.size);
                        let ns = (size + if key == Key::Period { 2.0 } else { -2.0 }).max(1.0);
                        exec(app, "layer.setText", json!({"layer": sel.layer.0, "range": [r.start, r.end], "size": ns}));
                    }
                    _ => {}
                }
                if !matches!(key, Key::Backspace | Key::Delete | Key::Enter | Key::Tab) {
                    typing_key(ui, true);
                }
            }
            _ => {}
        }
    }
}

fn draw(app: &mut EffectcraftApp, ui: &mut egui::Ui, painter: &egui::Painter, map: &ViewerMap, e: &Edited, sel: std::ops::Range<usize>, caret: usize) {
    let t = app.tokens;
    let to_s = |p: Point| e.to_screen(map, (p.x, p.y));
    // Paragraph box and its handles.
    if e.doc.box_size.is_some() {
        let hs: Vec<Pos2> = e.handles().iter().map(|h| e.to_screen(map, *h)).collect();
        let corners = [hs[0], hs[2], hs[4], hs[6], hs[0]];
        for w in corners.windows(2) {
            painter.line_segment([w[0], w[1]], Stroke::new(1.0, Color32::from_white_alpha(170)));
        }
        for (i, h) in hs.iter().enumerate() {
            let r = Rect::from_center_size(*h, vec2(7.0, 7.0));
            painter.rect_filled(r, 0.0, Color32::WHITE);
            painter.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::BLACK), StrokeKind::Middle);
            app.auto.add(&format!("viewer.textBox.handle.{i}"), r, "Text box handle");
        }
    }
    // Selection highlight.
    if !sel.is_empty() {
        for q in e.lay.selection_quads(sel.start, sel.end) {
            let pts: Vec<Pos2> = q.iter().map(|p| to_s(*p)).collect();
            painter.add(egui::Shape::convex_polygon(pts, t.accent.gamma_multiply(0.38), Stroke::NONE));
        }
    }
    // Caret (blinking) and IME composition.
    // The caret follows the animated layout and text on a path.
    let (a, b) = e.lay.caret(caret);
    let cm = e.carets.get(caret).copied().unwrap_or(Mat3::IDENTITY);
    let anim = |p: Point| {
        let q = cm.apply(gv2(p.x, p.y));
        Point::new(q.x, q.y)
    };
    let (a, b) = (anim(a), anim(b));
    let (sa, sb) = (to_s(a), to_s(b));
    let now = ui.input(|i| i.time);
    let preedit: Option<String> = ui.data(|d| d.get_temp(egui::Id::new("viewer-text-preedit")));
    if let Some(pe) = preedit.filter(|s| !s.is_empty()) {
        let h = (sb - sa).length().max(10.0);
        let g = painter.layout_no_wrap(pe, egui::FontId::proportional(h * 0.7), Color32::WHITE);
        let r = Rect::from_min_size(sa, g.size());
        painter.rect_filled(r.expand(2.0), 2.0, Color32::from_black_alpha(160));
        painter.galley(sa, g, Color32::WHITE);
        painter.line_segment([r.left_bottom(), r.right_bottom()], Stroke::new(1.0, Color32::WHITE));
    } else if (now * 2.0) as i64 % 2 == 0 || !sel.is_empty() {
        painter.line_segment([sa, sb], Stroke::new(3.0, Color32::from_black_alpha(160)));
        painter.line_segment([sa, sb], Stroke::new(1.5, Color32::WHITE));
    }
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
    // The text region for automation, and the IME candidate window position.
    let r = e.region();
    let quad = [(r[0], r[1]), (r[2], r[1]), (r[2], r[3]), (r[0], r[3])].map(|p| e.to_screen(map, p));
    let rect = Rect::from_points(&quad);
    app.auto.add("viewer.textEdit", rect, &format!("Editing {}", e.layer.name));
    let cursor_rect = Rect::from_two_pos(sa, sb).expand2(vec2(1.0, 0.0));
    ui.ctx().output_mut(|o| {
        o.ime = Some(egui::output::IMEOutput { purpose: egui::IMEPurpose::Normal, rect, cursor_rect, should_interrupt_composition: false });
    });
}
