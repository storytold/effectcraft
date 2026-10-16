//! The Graph Editor (Timeline ▸ Shift+F3): value graph and speed graph of the selected (or all
//! animated) properties, keyframes you can drag in time and value, Bezier/influence handles on
//! selected keys, auto-zoom / fit, and the bottom button bar (graph type, show selected,
//! auto-zoom, fit, separate dimensions, hold/linear/auto Bezier, Easy Ease family).
//!
//! Every edit is an engine command (`keys.set`, `keys.setEase`, `keys.select`, `keys.*`).

use effectcraft_engine::KeyRef;
use effectcraft_engine::keyframe::{Interp, Value, side_ease};
use effectcraft_engine::project::{Comp, Layer, LayerId, Property};
use effectcraft_engine::render::EvalCtx;
use effectcraft_engine::time::Tick;
use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use serde_json::json;

use super::timeline::TMap;
use crate::theme::Tokens;
use crate::{EffectcraftApp, widgets};

type Actions = Vec<(String, serde_json::Value)>;

const DIM_COLORS: [Color32; 4] =
    [Color32::from_rgb(0xe0, 0x55, 0x55), Color32::from_rgb(0x5c, 0xc8, 0x5c), Color32::from_rgb(0x50, 0x8c, 0xf0), Color32::from_rgb(0xe8, 0xc8, 0x40)];
const PROP_COLORS: [Color32; 6] = [
    Color32::from_rgb(0xe8, 0xc8, 0x40),
    Color32::from_rgb(0xd0, 0x70, 0xe0),
    Color32::from_rgb(0x40, 0xc8, 0xc8),
    Color32::from_rgb(0xf0, 0x90, 0x40),
    Color32::from_rgb(0x9c, 0xd8, 0x50),
    Color32::from_rgb(0xf0, 0x70, 0x90),
];

struct Handle {
    out: bool,
    /// Comp seconds / graph value of the handle end.
    t: f64,
    v: f64,
    /// Original speed (sign is kept when dragging in the speed graph).
    speed: f64,
    /// Segment duration (layer seconds) the influence is relative to.
    dur: f64,
}

struct KeyPt {
    idx: usize,
    time: Tick,
    /// Comp seconds.
    t: f64,
    v: f64,
    sel: bool,
    handles: Vec<Handle>,
}

struct Curve<'a> {
    layer: &'a Layer,
    prop: &'a Property,
    dim: usize,
    color: Color32,
    pts: Vec<(f64, f64)>,
    keys: Vec<KeyPt>,
    /// Values editable in the value graph (not spatial / combined dimensions).
    editable: bool,
}

/// Properties shown: the selected ones, or every animated property of the selected layers.
/// Whether the speed graph shows: Edit Speed Graph, or Auto-Select Graph Type with only spatial
/// properties shown (Position, Anchor Point… move along a path: their speed is what to edit).
fn speed_graph(app: &EffectcraftApp, comp: &Comp) -> bool {
    match app.ui.timeline.graph_mode.as_str() {
        "speed" => true,
        "value" => false,
        _ => {
            let props = shown_props(app, comp);
            !props.is_empty()
                && props.iter().all(|(lid, uid)| {
                    comp.layer(*lid).and_then(|l| l.props.find(*uid)).is_some_and(|pr| pr.spatial && matches!(pr.value, Value::Vec2(_) | Value::Vec3(_)))
                })
        }
    }
}

fn shown_props(app: &EffectcraftApp, comp: &Comp) -> Vec<(LayerId, u64)> {
    let st = &app.session.state;
    let numeric = |pr: &Property| !pr.value.components().is_empty() && pr.value.interpolates();
    // A selected keyframe counts as selecting its property and layer (as clicking a key does in
    // After Effects), so selecting a key in the Timeline shows its graph.
    let mut picked: Vec<(LayerId, u64)> = st.selected_props.clone();
    for k in &st.selected_keys {
        if !picked.contains(&(k.layer, k.prop)) {
            picked.push((k.layer, k.prop));
        }
    }
    if app.ui.timeline.graph_show_selected && !picked.is_empty() {
        return picked.into_iter().filter(|(l, u)| comp.layer(*l).and_then(|l| l.props.find(*u)).is_some_and(numeric)).collect();
    }
    let mut v = vec![];
    for l in comp.layers.iter().filter(|l| st.selected_layers.contains(&l.id) || picked.iter().any(|(pl, _)| *pl == l.id)) {
        l.props.walk("", &mut |_, pr| {
            if (pr.is_animated() || pr.has_expression()) && numeric(pr) {
                v.push((l.id, pr.uid));
            }
        });
    }
    v
}

fn shown_dims(l: &Layer, pr: &Property) -> usize {
    let n = pr.value.components().len();
    if pr.shown_dims > 0 && !l.is_3d() { (pr.shown_dims as usize).min(n) } else { n.min(4) }
}

/// The speed graph's value of dimension `d` at layer time `lt` (one curve for a spatial property).
fn speed_at(pr: &Property, spatial: bool, d: usize, lt: Tick) -> f64 {
    if spatial {
        effectcraft_engine::keyframe::speed(&pr.keys, lt, true)
    } else {
        effectcraft_engine::keyframe::velocity(&pr.keys, lt, false).get(d).copied().unwrap_or(0.0).abs()
    }
}

/// Build the curves for the visible time span.
fn curves<'a>(app: &EffectcraftApp, comp: &'a Comp, ectx: &EvalCtx, tm: TMap, plot: Rect, speed: bool) -> Vec<Curve<'a>> {
    let sel = &app.session.state.selected_keys;
    let n = ((plot.width() / 2.0) as usize).max(2);
    let mut out = vec![];
    for (pi, (lid, uid)) in shown_props(app, comp).into_iter().enumerate() {
        let Some(l) = comp.layer(lid) else { continue };
        let Some(pr) = l.props.find(uid) else { continue };
        let spatial = pr.spatial && matches!(pr.value, Value::Vec2(_) | Value::Vec3(_));
        let dims = if speed && spatial { 1 } else { shown_dims(l, pr) };
        for d in 0..dims {
            let color = if dims > 1 { DIM_COLORS[d % 4] } else { PROP_COLORS[pi % PROP_COLORS.len()] };
            let mut pts = Vec::with_capacity(n);
            for i in 0..n {
                let x = plot.min.x + plot.width() * i as f32 / (n - 1) as f32;
                let ct = tm.t(x);
                let tt = Tick::from_seconds_f64(ct);
                let v = if speed { speed_at(pr, spatial, d, l.layer_time(tt)) } else { ectx.at(tt).value(l, pr).components().get(d).copied().unwrap_or(0.0) };
                pts.push((ct, v));
            }
            let mut keys = vec![];
            for (i, k) in pr.keys.iter().enumerate() {
                let ks = sel.contains(&KeyRef { layer: lid, prop: uid, time: k.time });
                let kt = l.comp_time(k.time).seconds();
                let ed = if spatial { 0 } else { d };
                let mut handles = vec![];
                let mut side_speeds = vec![];
                for out_side in [false, true] {
                    let Some(e) = side_ease(&pr.keys, i, ed, pr.spatial, out_side) else { continue };
                    let j = if out_side { i + 1 } else { i - 1 };
                    let dur = (pr.keys[j].time.seconds() - k.time.seconds()).abs();
                    side_speeds.push(e.speed.abs());
                    let dt = e.influence * dur * if out_side { 1.0 } else { -1.0 };
                    let (hv, show) = if speed { (e.speed.abs(), true) } else { (k.value.components().get(d).copied().unwrap_or(0.0) + e.speed * dt, !spatial) };
                    let bezier = if out_side { k.out_interp == Interp::Bezier } else { k.in_interp == Interp::Bezier };
                    if ks && show && (bezier || speed) {
                        handles.push(Handle { out: out_side, t: kt + dt, v: hv, speed: e.speed, dur });
                    }
                }
                let v = if speed {
                    if side_speeds.is_empty() { 0.0 } else { side_speeds.iter().sum::<f64>() / side_speeds.len() as f64 }
                } else {
                    k.value.components().get(d).copied().unwrap_or(0.0)
                };
                keys.push(KeyPt { idx: i, time: k.time, t: kt, v, sel: ks, handles });
            }
            out.push(Curve { layer: l, prop: pr, dim: d, color, pts, keys, editable: !spatial });
        }
    }
    out
}

/// The Graph Editor's button bar, laid out left to right. Buttons that would run past the right
/// edge (a Timeline narrower than the whole row, #457) go to the "»" menu at its end instead of
/// being clipped, so every one stays reachable.
struct ButtonBar {
    x: f32,
    /// Where the next button must end to stay on the bar (the "»" button's room is kept free).
    limit: f32,
    /// Buttons moved to the "»" menu: (automation id, label, on, tooltip).
    hidden: Vec<(String, String, bool, String)>,
    /// The id picked from the "»" menu last frame, run by its button this frame.
    chosen: Option<String>,
}

/// The "»" menu's popup and the id it picked (read by the next frame's buttons).
const MORE_POPUP: &str = "graph-more";

impl ButtonBar {
    const MORE_W: f32 = 24.0;

    fn new(ctx: &egui::Context, bar: Rect) -> Self {
        let chosen = ctx.data_mut(|d| d.remove_temp::<String>(egui::Id::new(MORE_POPUP).with("chosen")));
        ButtonBar { x: bar.min.x + 8.0, limit: bar.max.x - 8.0 - Self::MORE_W - 4.0, hidden: Vec::new(), chosen }
    }

    /// The "»" button and its menu, when buttons did not fit.
    fn more(&mut self, app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, bar: Rect) {
        if self.hidden.is_empty() {
            return;
        }
        let t = app.tokens;
        let r = Rect::from_min_size(pos2(bar.max.x - 8.0 - Self::MORE_W, bar.center().y - 9.0), vec2(Self::MORE_W, 18.0));
        let pid = egui::Id::new(MORE_POPUP);
        let resp = ui.interact(r, pid.with("button"), Sense::click()).on_hover_text(crate::i18n::tr("More"));
        p.rect_filled(r, 3.0, if resp.hovered() { t.hover } else { Color32::from_rgb(0x2c, 0x2c, 0x2c) });
        p.text(r.center(), Align2::CENTER_CENTER, "»", Tokens::ui(12.0), t.text);
        app.auto.add("timeline.graph.more", r, "More");
        if resp.clicked() {
            let open = widgets::popup_is_open(ui, pid);
            ui.data_mut(|d| d.insert_temp(pid.with("open"), !open));
        }
        if !widgets::popup_is_open(ui, pid) {
            return;
        }
        let rows = self.hidden.len();
        let pos = r.left_top() - vec2(0.0, rows as f32 * 22.0 + 18.0);
        let chosen = widgets::popup_list(ui, pid, pos, r, rows, 0, |ui| {
            let mut chosen = None;
            for (id, label, on, tip) in &self.hidden {
                let item = ui.selectable_label(*on, label.as_str()).on_hover_text(tip.as_str());
                app.auto.add(&format!("timeline.graph.more.{id}"), item.rect, tip);
                if item.clicked() {
                    chosen = Some(id.clone());
                }
            }
            chosen
        });
        if let Some(id) = chosen {
            ui.data_mut(|d| d.insert_temp(pid.with("chosen"), id));
            ui.ctx().request_repaint();
        }
    }
}

fn text_button(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    p: &egui::Painter,
    bar: &mut ButtonBar,
    y: f32,
    label: &str,
    on: bool,
    id: &str,
    tip: &str,
) -> bool {
    let t = app.tokens;
    let w = p.layout_no_wrap(label.to_string(), Tokens::ui(11.0), t.text).size().x + 14.0;
    // Once one button overflows, the rest follow it into the menu (they keep their order).
    if !bar.hidden.is_empty() || bar.x + w > bar.limit {
        bar.hidden.push((id.to_string(), label.to_string(), on, tip.to_string()));
        return bar.chosen.as_deref() == Some(id);
    }
    let r = Rect::from_min_size(pos2(bar.x, y - 9.0), vec2(w, 18.0));
    let resp = ui.interact(r, egui::Id::new(("graph-btn", id)), Sense::click()).on_hover_text(tip);
    p.rect_filled(
        r,
        3.0,
        if on {
            t.accent.gamma_multiply(0.55)
        } else if resp.hovered() {
            t.hover
        } else {
            Color32::from_rgb(0x2c, 0x2c, 0x2c)
        },
    );
    p.text(r.center(), Align2::CENTER_CENTER, label, Tokens::ui(11.0), if on { Color32::WHITE } else { t.text });
    app.auto.add(&format!("timeline.graph.{id}"), r, tip);
    bar.x += w + 4.0;
    resp.clicked()
}

/// Draw the Graph Editor into `area` (the right side of the Timeline) and queue edits.
pub(crate) fn show(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, comp: &Comp, ectx: &EvalCtx, tm: TMap, area: Rect, actions: &mut Actions) {
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    let footer_h = 28.0;
    let plot = Rect::from_min_max(area.min + vec2(0.0, 6.0), pos2(area.max.x, area.max.y - footer_h));
    let bar = Rect::from_min_max(pos2(area.min.x, area.max.y - footer_h), area.max);
    // Background first so keys and handles (added later) sit on top for hit-testing.
    let bg = ui.interact(plot, egui::Id::new("graph-bg"), Sense::click_and_drag());
    p.rect_filled(area, 0.0, Color32::from_rgb(0x1d, 0x1d, 0x1d));
    app.auto.add("timeline.graph", plot, &app.ui.timeline.graph_mode.clone());
    let speed = speed_graph(app, comp);
    let cs = curves(app, comp, ectx, tm, plot, speed);
    // Just opened, or the graph type changed (value and speed have different units): frame the
    // graphs (Fit Selection to View, else Fit All Graphs to View).
    let was_speed_id = egui::Id::new("graph-was-speed");
    let type_changed = ctx.data(|d| d.get_temp::<bool>(was_speed_id)).is_some_and(|was| was != speed);
    ctx.data_mut(|d| d.insert_temp(was_speed_id, speed));
    if ctx.data_mut(|d| d.remove_temp::<bool>(fit_pending_id())).unwrap_or(false) || type_changed {
        if cs.iter().any(|c| c.keys.iter().any(|k| k.sel)) {
            fit_selection(app, &cs, tm, plot, speed);
        } else {
            fit_all(app);
        }
    }

    // Vertical range: auto-zoom to the curves + handles, or the stored manual range.
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for c in &cs {
        for (_, v) in &c.pts {
            lo = lo.min(*v);
            hi = hi.max(*v);
        }
        for k in &c.keys {
            for v in std::iter::once(k.v).chain(k.handles.iter().map(|h| h.v)) {
                lo = lo.min(v);
                hi = hi.max(v);
            }
        }
    }
    if !lo.is_finite() {
        (lo, hi) = (0.0, 100.0);
    }
    if speed {
        lo = lo.min(0.0);
    }
    if (hi - lo).abs() < 1e-9 {
        hi = lo + 1.0;
    }
    let pad = (hi - lo) * 0.08;
    let auto = (lo - pad, hi + pad);
    // While a key or handle is being dragged the scale stays put (no feedback loop).
    let drag_id = egui::Id::new("graph-dragging");
    let dragging = ctx.data(|d| d.get_temp::<bool>(drag_id)).unwrap_or(false) && ctx.input(|i| i.pointer.any_down());
    let (lo, hi) = if app.ui.timeline.graph_auto_zoom && !dragging { auto } else { app.ui.timeline.graph_range.unwrap_or(auto) };
    if app.ui.timeline.graph_auto_zoom && !dragging {
        app.ui.timeline.graph_range = Some((lo, hi));
    }
    let ymap = |v: f64| plot.max.y - ((v - lo) / (hi - lo)) as f32 * plot.height();
    let vmap = |y: f32| lo + (plot.max.y - y) as f64 / plot.height() as f64 * (hi - lo);

    // Grid + value labels.
    let pc = p.with_clip_rect(plot);
    let step = nice_step((hi - lo) / 6.0);
    let mut gv = (lo / step).ceil() * step;
    while gv <= hi {
        let y = ymap(gv);
        pc.line_segment([pos2(plot.min.x, y), pos2(plot.max.x, y)], Stroke::new(1.0, Color32::from_white_alpha(12)));
        pc.text(pos2(plot.min.x + 4.0, y - 1.0), Align2::LEFT_BOTTOM, fmt(gv, step), Tokens::ui(10.0), t.text_faint);
        gv += step;
    }
    if cs.is_empty() {
        p.text(
            plot.center(),
            Align2::CENTER_CENTER,
            crate::i18n::tr("Select animated properties to show them in the Graph Editor"),
            Tokens::ui(12.0),
            t.text_faint,
        );
    }

    // Show Reference Graph: the other graph type, faint, scaled to the plot.
    if app.ui.timeline.graph_reference && !cs.is_empty() {
        let other = curves(app, comp, ectx, tm, plot, !speed);
        let (mut rlo, mut rhi) = (f64::INFINITY, f64::NEG_INFINITY);
        for c in &other {
            for (_, v) in &c.pts {
                rlo = rlo.min(*v);
                rhi = rhi.max(*v);
            }
        }
        if rlo.is_finite() {
            let span = (rhi - rlo).max(1e-9);
            let lines: Vec<(Color32, Vec<Pos2>)> = other
                .iter()
                .map(|c| {
                    (
                        c.color,
                        c.pts
                            .iter()
                            .map(|(ct, v)| pos2(tm.x(*ct), plot.max.y - ((v - rlo) / span) as f32 * plot.height() * 0.9 - plot.height() * 0.05))
                            .collect(),
                    )
                })
                .collect();
            super::graph_tools::reference_graph(&pc, plot, &lines);
        }
    }
    // Curves.
    for c in &cs {
        let pts: Vec<Pos2> = c.pts.iter().map(|(ct, v)| pos2(tm.x(*ct), ymap(*v))).collect();
        pc.add(egui::Shape::line(pts, Stroke::new(1.5, c.color)));
    }
    // Transform box around several selected keys (inside below the keys, handles above).
    let sel_keys: Vec<super::graph_tools::SelKey> = cs
        .iter()
        .flat_map(|c| c.keys.iter().filter(|k| k.sel).map(move |k| super::graph_tools::SelKey { t: k.t, v: k.v, dim: c.dim, editable: c.editable }))
        .collect();
    let all_key_times: Vec<(u64, usize, f64)> = cs.iter().flat_map(|c| c.keys.iter().map(move |k| (c.prop.uid, k.idx, k.t))).collect();
    super::graph_tools::transform_box(app, ui, &pc, &sel_keys, tm, &ymap, &vmap, speed, false, actions);
    // Keys and handles.
    let mut key_screens: Vec<(serde_json::Value, Pos2)> = vec![];
    for c in &cs {
        let (lid, uid, d) = (c.layer.id, c.prop.uid, c.dim);
        for k in &c.keys {
            let kp = pos2(tm.x(k.t), ymap(k.v));
            // In the speed graph an end key rises with the handle being dragged; skipping it once it
            // leaves the plot would drop the handle's widget and end the drag at the top edge.
            let handle_dragged = || ["in", "out"].iter().any(|side| ctx.is_being_dragged(egui::Id::new(("ghandle", uid, d, k.idx, *side))));
            if !plot.expand(6.0).contains(kp) && !handle_dragged() {
                continue;
            }
            let kjson = json!({"layer": lid.0, "prop": uid, "time": k.time.seconds()});
            key_screens.push((kjson.clone(), kp));
            // Handles (selected keys).
            for h in &k.handles {
                let hp = pos2(tm.x(h.t), ymap(h.v));
                pc.line_segment([kp, hp], Stroke::new(1.0, Color32::from_rgb(0xd8, 0xd8, 0x60)));
                pc.circle_filled(hp, 3.5, Color32::from_rgb(0xd8, 0xd8, 0x60));
                let side = if h.out { "out" } else { "in" };
                let hr = Rect::from_center_size(hp, vec2(10.0, 10.0));
                let hresp = ui.interact(hr, egui::Id::new(("ghandle", uid, d, k.idx, side)), Sense::drag());
                app.auto.add(&format!("timeline.graph.handle.{uid}.{d}.{}.{side}", k.idx), hr, "Bezier handle");
                // The same side's handles of every selected key move with it, by as much as the
                // pointer moved (After Effects), in one undo step.
                if hresp.drag_started() {
                    let grabs: Vec<HandleGrab> = cs
                        .iter()
                        .flat_map(|c2| {
                            c2.keys.iter().flat_map(move |k2| {
                                k2.handles.iter().filter(|h2| h2.out == h.out).map(move |h2| HandleGrab {
                                    layer: c2.layer.id,
                                    prop: c2.prop.uid,
                                    dim: c2.editable.then_some(c2.dim),
                                    time: k2.time,
                                    out: h2.out,
                                    key: (k2.t, k2.v),
                                    end: (h2.t, h2.v),
                                    negative: h2.speed < 0.0,
                                    dur: h2.dur,
                                })
                            })
                        })
                        .collect();
                    let press = ui.input(|i| i.pointer.press_origin()).unwrap_or(hp);
                    ctx.data_mut(|dd| dd.insert_temp::<HandleDrag>(handle_drag_id(), (press, grabs)));
                }
                if hresp.dragged()
                    && let Some(pt) = hresp.interact_pointer_pos()
                    && let Some((press, grabs)) = ctx.data(|dd| dd.get_temp::<HandleDrag>(handle_drag_id()))
                {
                    ctx.data_mut(|dd| dd.insert_temp(drag_id, true));
                    // Shift keeps the handle on the time or the value axis, whichever moved more (as for keys).
                    let (time_ok, value_ok) = shift_axes(ui.input(|i| i.modifiers.shift), pt.x - press.x, pt.y - press.y);
                    let dt = if time_ok { tm.t(pt.x) - tm.t(press.x) } else { 0.0 };
                    let dv = if value_ok { vmap(pt.y) - vmap(press.y) } else { 0.0 };
                    let merge = format!("ghandle-{uid}-{}", k.idx);
                    for g in &grabs {
                        actions.push(("keys.setEase".into(), g.ease(g.end.0 + dt, g.end.1 + dv, speed, &merge)));
                    }
                }
                if hresp.drag_stopped() {
                    ctx.data_mut(|dd| {
                        dd.insert_temp(drag_id, false);
                        dd.remove::<HandleDrag>(handle_drag_id());
                    });
                    actions.push(("__endMerge".into(), json!({})));
                }
            }
            // The key itself.
            let col = if k.sel { t.keyframe_selected } else { Color32::from_gray(0xe0) };
            pc.rect_filled(Rect::from_center_size(kp, vec2(7.0, 7.0)), 0.0, col);
            pc.rect_stroke(Rect::from_center_size(kp, vec2(7.0, 7.0)), 0.0, Stroke::new(1.0, c.color), StrokeKind::Outside);
            let kr = Rect::from_center_size(kp, vec2(11.0, 11.0));
            let kresp = ui.interact(kr, egui::Id::new(("gkey", uid, d, k.idx)), Sense::click_and_drag());
            app.auto.add(&format!("timeline.graph.key.{uid}.{d}.{}", k.idx), kr, &c.prop.name);
            let mods = ui.input(|i| i.modifiers);
            let key = &c.prop.keys[k.idx];
            if kresp.clicked() && mods.command {
                // Ctrl+click: Linear ↔ Auto Bezier; Ctrl+Alt+click: Hold on / off (as in the Timeline).
                actions.push(("keys.select".into(), json!({"keys": [kjson.clone()]})));
                if mods.alt {
                    actions.push(("keys.toggleHold".into(), json!({})));
                } else {
                    let linear = key.in_interp == Interp::Linear && key.out_interp == Interp::Linear;
                    actions.push(("keys.interpolation".into(), json!({"interpolation": if linear { "autoBezier" } else { "linear" }})));
                }
            } else if kresp.clicked() && mods.shift {
                actions.push(("keys.select".into(), json!({"keys": [kjson.clone()], "toggle": true})));
            } else if (kresp.clicked() || kresp.drag_started()) && !k.sel {
                actions.push(("keys.select".into(), json!({"keys": [kjson.clone()], "add": mods.shift})));
            }
            // A drag moves every selected key, by how far the pointer went from where the button
            // went down (the key doesn't jump under the pointer); Shift keeps it to the time or
            // the value axis, whichever moved more.
            if kresp.drag_started()
                && let Some(o) = ui.input(|i| i.pointer.press_origin())
            {
                ctx.data_mut(|dd| dd.insert_temp(key_grab_id(), KeyGrab { press: o, t: k.t }));
            }
            if kresp.dragged()
                && let Some(pt) = kresp.interact_pointer_pos()
                && let Some(g) = ctx.data(|dd| dd.get_temp::<KeyGrab>(key_grab_id()))
            {
                ctx.data_mut(|dd| dd.insert_temp(drag_id, true));
                let (dx, dy) = (pt.x - g.press.x, pt.y - g.press.y);
                let (time_ok, value_ok) = shift_axes(mods.shift, dx, dy);
                // Graph Editor ▸ Snap: to the current time and other keys.
                let others: Vec<f64> = all_key_times.iter().filter(|(u, i, _)| !(*u == uid && *i == k.idx)).map(|x| x.2).collect();
                let t = if time_ok { super::graph_tools::snap_time(app, tm, (g.t + (tm.t(pt.x) - tm.t(g.press.x))).max(0.0), &others) } else { g.t };
                let mut params = json!({"timeOffset": t - g.t, "merge": "graph-key", "fromStart": true});
                if value_ok && !speed && c.editable {
                    params["valueOffset"] = json!(vmap(pt.y) - vmap(g.press.y));
                    params["dim"] = json!(d);
                }
                actions.push(("keys.transform".into(), params));
            }
            if kresp.drag_stopped() {
                ctx.data_mut(|dd| dd.insert_temp(drag_id, false));
                actions.push(("__endMerge".into(), json!({})));
            }
        }
    }

    super::graph_tools::transform_box(app, ui, &pc, &sel_keys, tm, &ymap, &vmap, speed, true, actions);

    // Background: click deselects keys, drag box-selects.
    if bg.clicked() {
        actions.push(("keys.select".into(), json!({"keys": []})));
    }
    if bg.dragged()
        && let (Some(o), Some(cur)) = (ctx.input(|i| i.pointer.press_origin()), bg.interact_pointer_pos())
    {
        let br = Rect::from_two_pos(o, cur);
        pc.rect_filled(br, 0.0, t.accent.gamma_multiply(0.12));
        pc.rect_stroke(br, 0.0, Stroke::new(1.0, t.accent), StrokeKind::Middle);
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("graph-box"), br));
    }
    if bg.drag_stopped()
        && let Some(br) = ctx.data(|d| d.get_temp::<Rect>(egui::Id::new("graph-box")))
    {
        ctx.data_mut(|d| d.remove::<Rect>(egui::Id::new("graph-box")));
        let keys: Vec<serde_json::Value> = key_screens.iter().filter(|(_, sp)| br.contains(*sp)).map(|(k, _)| k.clone()).collect();
        actions.push(("keys.select".into(), json!({"keys": keys, "add": ui.input(|i| i.modifiers.shift)})));
    }
    // Wheel over the graph with Alt/Ctrl zooms the value axis (turns auto-zoom off).
    if ui.rect_contains_pointer(plot) {
        let (dy, zoom) = ui.input(|i| (i.smooth_scroll_delta.y, i.modifiers.ctrl || i.modifiers.command));
        if zoom && dy.abs() > 0.0 {
            let k = (-dy as f64 / 200.0).exp();
            let mid = (lo + hi) / 2.0;
            app.ui.timeline.graph_auto_zoom = false;
            app.ui.timeline.graph_range = Some((mid - (mid - lo) * k, mid + (hi - mid) * k));
        }
    }

    // Button bar.
    p.rect_filled(bar, 0.0, t.panel_bg);
    p.line_segment([bar.left_top(), bar.right_top()], Stroke::new(1.0, t.separator));
    let y = bar.center().y;
    let mut buttons = ButtonBar::new(&ctx, bar);
    let tl = &app.ui.timeline;
    let (auto_type, show_sel, auto_on) = (tl.graph_mode == "auto", tl.graph_show_selected, tl.graph_auto_zoom);
    if text_button(
        app,
        ui,
        p,
        &mut buttons,
        y,
        "Auto",
        auto_type,
        "autoSelectGraphType",
        "Auto-Select Graph Type (speed graph for spatial properties, else value graph)",
    ) {
        app.ui.timeline.graph_mode = if auto_type { if speed { "speed" } else { "value" } } else { "auto" }.into();
    }
    // Choosing a graph type turns Auto-Select off, as in After Effects.
    if text_button(app, ui, p, &mut buttons, y, "Value", !speed, "valueGraph", "Edit Value Graph") {
        app.ui.timeline.graph_mode = "value".into();
    }
    if text_button(app, ui, p, &mut buttons, y, "Speed", speed, "speedGraph", "Edit Speed Graph") {
        app.ui.timeline.graph_mode = "speed".into();
    }
    buttons.x += 6.0;
    if text_button(app, ui, p, &mut buttons, y, "Selected", show_sel, "showSelected", "Show Selected Properties (else all animated properties)") {
        app.ui.timeline.graph_show_selected = !show_sel;
    }
    if text_button(app, ui, p, &mut buttons, y, "Auto-zoom", auto_on, "autoZoom", "Auto-zoom graph height") {
        app.ui.timeline.graph_auto_zoom = !auto_on;
    }
    if text_button(app, ui, p, &mut buttons, y, "Fit", false, "fit", "Fit all graphs to view") {
        fit_all(app);
    }
    if text_button(app, ui, p, &mut buttons, y, "Fit Selection", false, "fitSelection", "Fit selection to view") {
        fit_selection(app, &cs, tm, plot, speed);
    }
    buttons.x += 6.0;
    let (snap, reference, tbox) = (app.ui.timeline.graph_snap, app.ui.timeline.graph_reference, app.ui.timeline.graph_transform_box);
    if text_button(app, ui, p, &mut buttons, y, "Snap", snap, "snap", "Snap (key drags snap to the current time and other keys)") {
        app.ui.timeline.graph_snap = !snap;
    }
    if text_button(app, ui, p, &mut buttons, y, "Reference", reference, "reference", "Show Reference Graph") {
        app.ui.timeline.graph_reference = !reference;
    }
    if text_button(app, ui, p, &mut buttons, y, "Transform Box", tbox, "transformBox", "Show Transform Box when multiple keys are selected") {
        app.ui.timeline.graph_transform_box = !tbox;
    }
    buttons.x += 6.0;
    let pos_sel = app.session.state.selected_props.iter().find_map(|(l, u)| super::timeline::is_position(comp.layer(*l)?, *u).then_some(l.0));
    if text_button(app, ui, p, &mut buttons, y, "Separate Dimensions", false, "separateDimensions", "Separate Dimensions (Position)")
        && let Some(l) = pos_sel.or(app.session.state.selected_layers.first().map(|l| l.0))
    {
        actions.push(("prop.separateDimensions".into(), json!({"layer": l})));
    }
    buttons.x += 6.0;
    for (label, cmd, params, tip) in [
        ("Hold", "keys.interpolation", json!({"interpolation": "hold"}), "Convert selected keyframes to Hold"),
        ("Linear", "keys.interpolation", json!({"interpolation": "linear"}), "Convert selected keyframes to Linear"),
        ("Auto Bezier", "keys.interpolation", json!({"interpolation": "autoBezier"}), "Convert selected keyframes to Auto Bezier"),
        ("Easy Ease", "keys.easyEase", json!({}), "Easy Ease (F9)"),
        ("Ease In", "keys.easyEaseIn", json!({}), "Easy Ease In (Shift+F9)"),
        ("Ease Out", "keys.easyEaseOut", json!({}), "Easy Ease Out (Ctrl+Shift+F9)"),
    ] {
        let id = cmd.trim_start_matches("keys.").to_string() + label.split_whitespace().next().unwrap_or("");
        if text_button(app, ui, p, &mut buttons, y, label, false, &id, tip) {
            actions.push((cmd.into(), params));
        }
    }
    buttons.more(app, ui, p, bar);
}

/// Fit All Graphs to View: the whole comp in time, auto-zoomed values.
fn fit_all(app: &mut EffectcraftApp) {
    app.ui.timeline.graph_auto_zoom = true;
    app.ui.timeline.pps = None;
}

fn fit_pending_id() -> egui::Id {
    egui::Id::new("graph-fit-pending")
}

/// Called every Timeline frame with whether the Graph Editor is on: switching it on (header
/// button, menu, Shift+F3 or the control channel) frames the graphs on its first frame, when the
/// plot rect is known, as After Effects does.
pub(crate) fn track_open(ctx: &egui::Context, on: bool) {
    let was_id = egui::Id::new("graph-editor-was-on");
    let was = ctx.data(|d| d.get_temp::<bool>(was_id)).unwrap_or(false);
    ctx.data_mut(|d| {
        d.insert_temp(was_id, on);
        if on && !was {
            d.insert_temp(fit_pending_id(), true);
        }
    });
}

/// Zoom time and value to the selected keys (and their handles).
fn fit_selection(app: &mut EffectcraftApp, cs: &[Curve], tm: TMap, plot: Rect, speed: bool) {
    let mut t0 = f64::INFINITY;
    let mut t1 = f64::NEG_INFINITY;
    let mut v0 = f64::INFINITY;
    let mut v1 = f64::NEG_INFINITY;
    for c in cs {
        for k in c.keys.iter().filter(|k| k.sel) {
            for (t, v) in std::iter::once((k.t, k.v)).chain(k.handles.iter().map(|h| (h.t, h.v))) {
                t0 = t0.min(t);
                t1 = t1.max(t);
                v0 = v0.min(v);
                v1 = v1.max(v);
            }
        }
    }
    if !t0.is_finite() {
        return;
    }
    // A lone selected key (one point) also frames its neighbouring keys, so its curve shows
    // instead of a flat sliver a fraction of a frame wide.
    if t1 - t0 < 1e-6 {
        for c in cs {
            for (i, k) in c.keys.iter().enumerate().filter(|(_, k)| k.sel) {
                for n in [i.checked_sub(1).and_then(|j| c.keys.get(j)), c.keys.get(i + 1), Some(k)].into_iter().flatten() {
                    t0 = t0.min(n.t);
                    t1 = t1.max(n.t);
                    v0 = v0.min(n.v);
                    v1 = v1.max(n.v);
                }
            }
        }
    }
    // In the speed graph eased keys sit at speed 0 and the curve peaks between them (#1617):
    // frame the curve from the first to the last selected key. A lone key has its handles on
    // the speed graph, so the check above misses it: it frames its neighbouring segments here.
    if speed {
        for c in cs {
            let (Some(first), Some(last)) = (c.keys.iter().position(|k| k.sel), c.keys.iter().rposition(|k| k.sel)) else { continue };
            let (a, b) = if first == last { (first.saturating_sub(1), last + 1) } else { (first, last) };
            let (Some(ka), Some(kb)) = (c.keys.get(a), c.keys.get(b.min(c.keys.len().saturating_sub(1)))) else { continue };
            t0 = t0.min(ka.t);
            t1 = t1.max(kb.t);
            let (s0, s1) = (ka.time.seconds(), kb.time.seconds());
            const SAMPLES: usize = 128;
            for i in 0..=SAMPLES {
                let lt = Tick::from_seconds_f64(s0 + (s1 - s0) * i as f64 / SAMPLES as f64);
                let v = speed_at(c.prop, !c.editable, c.dim, lt);
                v0 = v0.min(v);
                v1 = v1.max(v);
            }
        }
    }
    let span = (t1 - t0).max(0.2);
    let pps = plot.width() as f64 * 0.8 / span;
    // Through the timeline's zoom, so the fit never zooms out past the whole comp.
    if let Some(comp) = app.session.active_comp_arc() {
        let fit = super::timeline::fit_pps(plot.width(), &comp);
        super::timeline::set_zoom(&mut app.ui.timeline, &comp, pps, fit, (t0 - span * 0.1).max(0.0), 0.0);
    }
    let pad = ((v1 - v0) * 0.15).max(1.0);
    app.ui.timeline.graph_auto_zoom = false;
    app.ui.timeline.graph_range = Some((v0 - pad, v1 + pad));
    let _ = tm;
}

fn nice_step(raw: f64) -> f64 {
    let raw = raw.abs().max(1e-9);
    let mag = 10f64.powf(raw.log10().floor());
    let n = raw / mag;
    mag * if n < 1.5 {
        1.0
    } else if n < 3.5 {
        2.0
    } else if n < 7.5 {
        5.0
    } else {
        10.0
    }
}

fn fmt(v: f64, step: f64) -> String {
    let dec = if step >= 1.0 { 0 } else { (-step.log10()).ceil() as usize };
    let s = format!("{v:.dec$}");
    if s.trim_start_matches('-').chars().all(|c| c == '0' || c == '.') { s.trim_start_matches('-').to_string() } else { s }
}

/// A Graph Editor key drag: where the button went down and the grabbed key's comp time then.
#[derive(Clone, Copy, Debug)]
struct KeyGrab {
    press: Pos2,
    t: f64,
}

fn key_grab_id() -> egui::Id {
    egui::Id::new("graph-key-grab")
}

/// A Bezier handle moving with a handle drag: its key, side and dimension, the key's place
/// (comp seconds, graph value) and the handle's end when the drag began.
#[derive(Clone, Debug)]
struct HandleGrab {
    layer: LayerId,
    prop: u64,
    /// `None` for a property drawn as one curve (spatial).
    dim: Option<usize>,
    time: Tick,
    out: bool,
    key: (f64, f64),
    end: (f64, f64),
    /// The speed was negative (the speed graph shows its magnitude).
    negative: bool,
    /// Segment duration (layer seconds) the influence is relative to.
    dur: f64,
}

impl HandleGrab {
    /// `keys.setEase` putting the handle's end at (`t` comp seconds, `v` in the graph).
    fn ease(&self, t: f64, v: f64, speed: bool, merge: &str) -> serde_json::Value {
        let dir = if self.out { 1.0 } else { -1.0 };
        let dt = ((t - self.key.0) * dir).max(self.dur * 0.001);
        let influence = (dt / self.dur.max(1e-9) * 100.0).clamp(0.1, 100.0);
        let sp = if speed {
            let mag = v.max(0.0);
            if self.negative { -mag } else { mag }
        } else {
            (v - self.key.1) / dt * dir
        };
        let side = if self.out { "out" } else { "in" };
        let mut params =
            json!({"layer": self.layer.0, "prop": self.prop, "time": self.time.seconds(), "side": side, "speed": sp, "influence": influence, "merge": merge});
        if let Some(d) = self.dim {
            params["dim"] = json!(d);
        }
        params
    }
}

/// A handle drag: where the button went down, and the handles that move.
type HandleDrag = (Pos2, Vec<HandleGrab>);

fn handle_drag_id() -> egui::Id {
    egui::Id::new("graph-handle-drag")
}

/// Which axes a drag may move on (time, value): both, or with Shift only the one the pointer moved more along.
fn shift_axes(shift: bool, dx: f32, dy: f32) -> (bool, bool) {
    if shift { (dx.abs() >= dy.abs(), dy.abs() > dx.abs()) } else { (true, true) }
}

#[cfg(test)]
mod tests {
    use super::shift_axes;

    #[test]
    fn shift_locks_drag_to_dominant_axis() {
        assert_eq!(shift_axes(false, 10.0, 3.0), (true, true));
        assert_eq!(shift_axes(true, 10.0, -3.0), (true, false));
        assert_eq!(shift_axes(true, 2.0, -9.0), (false, true));
        assert_eq!(shift_axes(true, 5.0, 5.0), (true, false));
    }
}
