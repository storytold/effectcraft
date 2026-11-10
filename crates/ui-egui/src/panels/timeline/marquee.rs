//! A marquee owns content coordinates; clipping belongs only to its painter.

use std::sync::Arc;

use effectcraft_engine::project::Comp;
use egui::{Id, Pos2, Rect, Stroke, StrokeKind, pos2};
use serde_json::json;

use super::{Row, RowKind, Rw, TMap, edge_scroll_speed, keys_in_box, row_height};
use crate::EffectcraftApp;

#[derive(Clone)]
struct Marquee {
    owner: Id,
    comp: Arc<Comp>,
    rows: Arc<Vec<Row>>,
    graph_on: bool,
    layout: (Rect, f32, f32),
    anchor: Anchor,
    dragging: bool,
}

/// The pressed corner in content coordinates: a time (or an outline x) and a scroll-free y.
#[derive(Clone, Copy)]
pub(super) struct Anchor {
    time: f64,
    y: f32,
    outline_x: Option<f32>,
}

/// The timeline layout a marquee is projected through.
#[derive(Clone, Copy)]
pub(super) struct View {
    pub tm: TMap,
    /// Screen y of the first row at scroll 0.
    pub top: f32,
    pub graph_x0: f32,
    pub graph_on: bool,
}

impl View {
    fn of(g: &Rw) -> Self {
        Self { tm: g.tl.tm, top: g.tl.rows_rect.min.y, graph_x0: g.tl.graph_x0, graph_on: g.graph_on }
    }
}

fn id() -> Id {
    Id::unique("tl-marquee")
}

/// Capture on press, before wheel handling, even if the drag threshold is not crossed yet.
pub(super) fn prepare(app: &EffectcraftApp, ui: &egui::Ui, g: &Rw) {
    let ctx = &g.tl.ctx;
    let (pressed, down, released, escape, focused) =
        ctx.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down(), i.pointer.primary_released(), i.key_pressed(egui::Key::Escape), i.focused));
    if let Some(mut drag) = ctx.data(|d| d.get_temp::<Marquee>(id())) {
        let owned = ctx.dragged_id() == Some(drag.owner);
        let dropped = released && ctx.drag_stopped_id() == Some(drag.owner);
        let valid = ui.is_enabled()
            && focused
            && !escape
            && Arc::ptr_eq(&drag.comp, &g.tl.comp)
            && *drag.rows == *g.tl.rows
            && drag.graph_on == g.graph_on
            && drag.layout == (g.tl.rect, g.tl.rh, g.tl.graph_x0)
            && (dropped || (down && (owned || (!drag.dragging && ctx.dragged_id().is_none()))));
        drag.dragging |= owned;
        ctx.data_mut(|d| {
            if valid {
                d.insert_temp(id(), drag);
            } else {
                d.remove::<Marquee>(id());
            }
        });
    }
    if pressed && ui.is_enabled() && focused && !escape {
        let owner = [&g.outline_empty, &g.empty].into_iter().find(|r| r.is_pointer_button_down_on());
        if let Some(owner) = owner
            && let Some(p) = ctx.input(|i| i.pointer.press_origin())
        {
            ctx.data_mut(|d| {
                d.insert_temp(
                    id(),
                    Marquee {
                        owner: owner.id,
                        comp: g.tl.comp.clone(),
                        rows: g.tl.rows.clone(),
                        graph_on: g.graph_on,
                        layout: (g.tl.rect, g.tl.rh, g.tl.graph_x0),
                        anchor: Anchor::press(&View::of(g), app.ui.timeline.scroll_y, p),
                        dragging: false,
                    },
                );
            });
        }
    }
}

/// Only the marquee owner scrolls, with the same bounded edge speed as a pick whip.
pub(super) fn scroll(app: &mut EffectcraftApp, g: &Rw) {
    let ctx = &g.tl.ctx;
    if let Some(drag) = ctx.data(|d| d.get_temp::<Marquee>(id()))
        && ctx.dragged_id() == Some(drag.owner)
        && ctx.input(|i| i.pointer.primary_down())
        && let Some(p) = ctx.input(|i| i.pointer.latest_pos())
    {
        let speed = edge_scroll_speed(g.tl.rows_rect, p.y, g.tl.rh);
        let old = app.ui.timeline.scroll_y;
        app.ui.timeline.scroll_y = (old + speed * ctx.input(|i| i.stable_dt).min(0.1)).clamp(0.0, g.tl.max_scroll);
        if app.ui.timeline.scroll_y != old {
            ctx.request_repaint();
        }
    }
}

pub(super) struct SelectionBox {
    pub keys: Rect,
    pub outline: bool,
}

impl Anchor {
    /// Anchor a press at screen point `p` with the rows scrolled by `scroll`.
    pub(super) fn press(v: &View, scroll: f32, p: Pos2) -> Self {
        Self { time: v.tm.t(p.x.max(v.graph_x0)), y: p.y - v.top + scroll, outline_x: (p.x < v.graph_x0).then_some(p.x) }
    }

    /// Reproject the content anchor without clipping the logical selection bounds.
    pub(super) fn selection(&self, v: &View, scroll: f32, pointer: Pos2) -> (Rect, SelectionBox) {
        let tm = v.tm;
        let anchor = pos2(self.outline_x.unwrap_or_else(|| tm.x(self.time)), v.top + self.y - scroll);
        let paint = Rect::from_two_pos(anchor, pointer);
        let graph = self.outline_x.is_none() || pointer.x >= v.graph_x0;
        let keys =
            if graph && !v.graph_on { Rect::from_two_pos(pos2(tm.x(self.time), anchor.y), pos2(pointer.x.max(v.graph_x0), pointer.y)) } else { Rect::NOTHING };
        (paint, SelectionBox { keys, outline: self.outline_x.is_some() || pointer.x < v.graph_x0 })
    }
}

/// Query all logical rows; the renderer's visible hit rows are not selection data.
pub(super) fn targets(
    comp: &Comp,
    rows: &[Row],
    tm: &TMap,
    top: f32,
    rh: f32,
    paint: Rect,
    selection: &SelectionBox,
) -> (Vec<u64>, Vec<effectcraft_engine::KeyRef>) {
    let mut layers = vec![];
    let mut y = top;
    for row in rows {
        let bottom = y + row_height(row, rh);
        if selection.outline
            && matches!(row.kind, RowKind::Layer)
            && y <= paint.max.y
            && bottom >= paint.min.y
            && comp.layer(row.layer).is_some_and(|l| !l.switches.locked)
        {
            layers.push(row.layer.0);
        }
        y = bottom;
    }
    (layers, keys_in_box(comp, rows, tm, top, rh, selection.keys))
}

/// Commit an owned primary release; otherwise paint without changing selection.
pub(super) fn finish(app: &EffectcraftApp, ui: &egui::Ui, g: &mut Rw) {
    let ctx = &g.tl.ctx;
    let Some(drag) = ctx.data(|d| d.get_temp::<Marquee>(id())) else { return };
    let released = ctx.input(|i| i.pointer.primary_released());
    let dropped = released && ctx.drag_stopped_id() == Some(drag.owner);
    if ctx.dragged_id() != Some(drag.owner) && !dropped {
        return;
    }
    let Some(pointer) = ctx.input(|i| i.pointer.latest_pos()) else { return };
    let (paint, selection) = drag.anchor.selection(&View::of(g), app.ui.timeline.scroll_y, pointer);
    if dropped {
        ctx.data_mut(|d| d.remove::<Marquee>(id()));
        let (layers, keys) = targets(&g.tl.comp, &g.tl.rows, &g.tl.tm, g.tl.rows_rect.min.y - app.ui.timeline.scroll_y, g.tl.rh, paint, &selection);
        let add = ui.input(|i| i.modifiers.shift || i.modifiers.command);
        g.tl.actions.push(("layer.select".into(), json!({"layers": layers, "add": add})));
        if !keys.is_empty() {
            let keys: Vec<_> = keys.iter().map(|k| json!({"layer": k.layer.0, "prop": k.prop, "time": k.time.seconds()})).collect();
            g.tl.actions.push(("keys.select".into(), json!({"keys": keys, "add": true, "selectProperties": true})));
        }
    } else {
        let clip = if selection.outline { g.tl.rows_rect } else { g.right_clip };
        let painter = g.tl.p.with_clip_rect(clip);
        painter.rect_filled(paint, 0.0, g.tl.t.accent.gamma_multiply(0.12));
        painter.rect_stroke(paint, 0.0, Stroke::new(1.0, g.tl.t.accent), StrokeKind::Inside);
    }
}
