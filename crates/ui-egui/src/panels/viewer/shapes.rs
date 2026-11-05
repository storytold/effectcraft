use super::geometry::{l2c, layer_quad, point_in_quad};
use super::{
    Color32, Comp, EffectcraftApp, EvalCtx, Layer, LayerId, LayerSource, Mat3, PaintKind, Pos2, Rect, Stroke, Tick, Tokens, Tool, ViewerMap, gv2, json, ov,
    pos2,
};

/// Visible layers eligible for viewer hit tests, locked or not, in front-to-back order.
/// Solo scope matches the renderer: any soloed AV layer activates it at every time (as in After
/// Effects), even when hidden, locked or outside its in-out range.
fn hit_testable_layers(comp: &Comp, time: Tick) -> impl Iterator<Item = &Layer> {
    let any_solo = comp.layers.iter().any(|l| l.switches.solo && l.source.is_av());
    comp.layers.iter().filter(move |l| l.is_active_at(time) && l.switches.video && !l.is_camera() && !l.is_light() && (!any_solo || l.switches.solo))
}

/// Visible, unlocked layers eligible for viewer hit tests, in front-to-back order.
pub(crate) fn selectable_layers(comp: &Comp, time: Tick) -> impl Iterator<Item = &Layer> {
    hit_testable_layers(comp, time).filter(|l| !l.switches.locked)
}

/// Selectable layers under a comp point, front to back.
pub(crate) fn layers_at<'a, 'b>(ectx: &'b EvalCtx<'a>, cpt: [f64; 2]) -> impl Iterator<Item = &'a Layer> + 'b {
    selectable_layers(ectx.comp, ectx.time).filter(move |l| layer_quad(ectx, l).is_some_and(|(_, q, _)| point_in_quad(cpt, &q)))
}

/// Topmost selectable layer under a comp point.
pub(crate) fn layer_at<'a>(ectx: &EvalCtx<'a>, cpt: [f64; 2]) -> Option<&'a Layer> {
    layers_at(ectx, cpt).next()
}

/// Topmost visible layer under a comp point, locked or not. An Alt-drop uses it so a locked layer in
/// front is refused rather than skipped, which would replace the layer behind it.
pub(crate) fn layer_at_including_locked<'a>(ectx: &EvalCtx<'a>, cpt: [f64; 2]) -> Option<&'a Layer> {
    // A layer scaled to nothing has a degenerate quad that `point_in_quad` accepts everywhere.
    hit_testable_layers(ectx.comp, ectx.time).find(|l| layer_quad(ectx, l).is_some_and(|(_, q, _)| quad_area(&q) > 1e-9 && point_in_quad(cpt, &q)))
}

/// Area of a quad (shoelace formula); zero when its corners coincide or are collinear.
fn quad_area(q: &[[f64; 2]; 4]) -> f64 {
    (0..4).map(|i| q[i][0] * q[(i + 1) % 4][1] - q[(i + 1) % 4][0] * q[i][1]).sum::<f64>().abs() / 2.0
}

/// The layer a press at a comp point acts on, selected (Shift toggles the topmost layer). As in
/// After Effects, a selected layer under the point comes before the layers in front of it, so it
/// can be dragged where they cover it; otherwise the topmost layer (#230).
pub(crate) fn pick(app: &mut EffectcraftApp, ectx: &EvalCtx, cpt: [f64; 2], toggle: bool) -> Option<LayerId> {
    if toggle {
        let hit = layer_at(ectx, cpt)?.id;
        let _ = app.session.execute("layer.select", json!({"layers": [hit.0], "toggle": true}));
        return Some(hit);
    }
    let selected = &app.session.state.selected_layers;
    let hits: Vec<LayerId> = layers_at(ectx, cpt).map(|l| l.id).collect();
    let hit = *hits.iter().find(|id| selected.contains(id)).or(hits.first())?;
    if !selected.contains(&hit) {
        let _ = app.session.execute("layer.select", json!({"layers": [hit.0]}));
    }
    Some(hit)
}

/// Where a shape drag is going to land: a mask on the selected layer, a group inside it, or a new
/// shape layer.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ShapeTarget {
    Mask,
    IntoLayer,
    NewLayer,
}

impl ShapeTarget {
    /// What the drag will make and where, named the way the ghost captions it.
    fn label(self, layer_name: &str) -> String {
        match self {
            ShapeTarget::Mask => format!("Mask on “{layer_name}”"),
            ShapeTarget::IntoLayer => format!("Into “{layer_name}”"),
            ShapeTarget::NewLayer => "New shape layer".to_string(),
        }
    }
}

/// A shape drag as it stands, after the Tools' normalisations: the path it will make and where it
/// will land.
pub(super) struct ShapeDrag {
    /// The `shape.newShape` / `layer.addMask` kind for the tool.
    kind: &'static str,
    /// Comp-space centre and size, what the drag will make.
    centre: [f64; 2],
    size: [f64; 2],
    target: ShapeTarget,
    /// The selected layer's name and the index its next mask would take, for the ghost's caption
    /// and colour.
    layer_name: String,
    mask_index: usize,
}

/// The kind `tool` draws, as `shape.newShape` / `layer.addMask` name it.
fn shape_kind(tool: Tool) -> &'static str {
    match tool {
        Tool::Rectangle => "rect",
        Tool::RoundedRect => "rounded",
        Tool::Ellipse => "ellipse",
        Tool::Polygon => "polygon",
        _ => "star",
    }
}

/// What a shape tool drag from `start` to `end` makes, or `None` while it is too small to make
/// anything. Alt (Option) draws from the press point and Shift squares the box; the live ghost and
/// [`create_shape`] both go through this, so the drag always shows what it will commit.
pub(super) fn shape_drag(app: &EffectcraftApp, tool: Tool, start: [f64; 2], end: [f64; 2], mods: egui::Modifiers) -> Option<ShapeDrag> {
    let comp = app.session.active_comp()?;
    let selected = app.session.state.selected_layers.first().copied();
    let sel = selected.and_then(|id| comp.layer(id));
    // A footage, solid or other non-shape layer selected, or a shape layer with Tool Creates
    // Mask: a mask on that layer. Otherwise the engine's own target: the first selected unlocked
    // shape layer, else a new one.
    let target = match sel {
        Some(l) if l.source.is_av() && (app.session.state.shape_tool.creates_mask || !matches!(l.source, LayerSource::Shape)) => (ShapeTarget::Mask, Some(l)),
        _ => match effectcraft_engine::commands::shape_tool::selected_shape_target(&app.session, comp).and_then(|id| comp.layer(id)) {
            Some(l) => (ShapeTarget::IntoLayer, Some(l)),
            None => (ShapeTarget::NewLayer, None),
        },
    };
    let (target, layer) = target;
    let mask_index = if target == ShapeTarget::Mask { layer.and_then(|l| l.masks()).map_or(0, |m| m.groups().count()) } else { 0 };
    let (mut w, mut h) = ((end[0] - start[0]).abs(), (end[1] - start[1]).abs());
    // Ctrl/Cmd draws from the press point as the centre, as in After Effects.
    if mods.command {
        w *= 2.0;
        h *= 2.0;
    }
    if w < 2.0 && h < 2.0 {
        return None;
    }
    if mods.shift {
        let m = w.max(h);
        w = m;
        h = m;
    }
    Some(ShapeDrag {
        kind: shape_kind(tool),
        centre: if mods.command { start } else { [start[0].min(end[0]) + w / 2.0, start[1].min(end[1]) + h / 2.0] },
        size: [w, h],
        target,
        layer_name: layer.map_or_else(String::new, |l| l.name.clone()),
        mask_index,
    })
}

/// The Tools bar's paint colour as bytes, dimmed by `alpha` so the comp shows through the ghost.
fn paint_rgb(c: [f32; 3], opacity: f64, alpha: f32) -> Color32 {
    let f = |k: f32| (k.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgb(f(c[0]), f(c[1]), f(c[2])).gamma_multiply((opacity.clamp(0.0, 100.0) / 100.0) as f32 * alpha)
}

/// Draw a shape drag as it will land: the tool's own path, filled with the Tools bar's Fill and
/// outlined in its Stroke (or the mask colour, for a mask), captioned with where it goes.
pub(super) fn draw_shape_ghost(painter: &egui::Painter, map: &ViewerMap, app: &EffectcraftApp, d: &ShapeDrag, t: &Tokens) {
    // The engine's own geometry for the kind, so the ghost is the shape and not a stand-in for it.
    let Some(sp) = effectcraft_engine::commands::shape_tool::mask_path(d.kind, d.centre, d.size) else { return };
    let mut pts = ov::flatten(&sp, &Mat3::IDENTITY, map);
    if pts.first() == pts.last() {
        pts.pop();
    }
    if pts.len() < 3 {
        return;
    }
    let tool = &app.session.state.shape_tool;
    let (fill, line) = match d.target {
        ShapeTarget::Mask => {
            let [r, g, b] = effectcraft_engine::prefs::mask_color(app.session.prefs.appearance.cycle_mask_colors, d.mask_index);
            (Color32::from_rgb(r, g, b).gamma_multiply(0.25), Color32::from_rgb(r, g, b))
        }
        _ => {
            let stroke_on = tool.stroke.kind != PaintKind::None && tool.stroke_width > 0.0;
            (
                if tool.fill.kind == PaintKind::None { Color32::TRANSPARENT } else { paint_rgb(tool.fill.color, tool.fill.opacity, 0.45) },
                if stroke_on { paint_rgb(tool.stroke.color, tool.stroke.opacity, 1.0) } else { t.accent },
            )
        }
    };
    let Some(bounds) = bounds_of(&pts) else { return };
    // A screen-width outline reads at every zoom, where a comp-pixel hairline does not.
    painter.add(egui::Shape::Path(egui::epaint::PathShape { points: pts, closed: true, fill, stroke: Stroke::new(1.5, line).into() }));
    // Caption it with where the shape lands: a mask, a group in the selected shape layer or a
    // new layer.
    let font = Tokens::ui(11.0);
    let galley = painter.layout_no_wrap(d.target.label(&d.layer_name), font, line);
    let caption = Rect::from_min_size(pos2(bounds.left(), bounds.top() - 5.0 - galley.size().y), galley.size());
    painter.rect_filled(caption.expand(3.0), 2.0, Color32::from_black_alpha(150));
    painter.galley(caption.min, galley, line);
}

/// The bounds of points on screen, or `None` for an empty set.
fn bounds_of(pts: &[Pos2]) -> Option<Rect> {
    let first = *pts.first()?;
    let mut r = Rect::from_min_max(first, first);
    for p in pts.iter().skip(1) {
        r.extend_with(*p);
    }
    Some(r)
}

pub(super) fn create_shape(app: &mut EffectcraftApp, tool: Tool, a: [f64; 2], b: [f64; 2], mods: egui::Modifiers) {
    let Some(d) = shape_drag(app, tool, a, b, mods) else { return };
    let (kind, (w, h), (cx, cy)) = (d.kind, (d.size[0], d.size[1]), (d.centre[0], d.centre[1]));
    // A mask on the selected layer, as the ghost showed.
    if d.target == ShapeTarget::Mask {
        // In layer space: invert the layer transform.
        let sel_layer = app.session.state.selected_layers.first().copied().and_then(|id| app.session.active_comp().and_then(|c| c.layer(id)).cloned());
        if let Some(l) = sel_layer
            && let Some(comp) = app.session.active_comp_arc()
        {
            let ectx = EvalCtx {
                project: &app.session.project,
                comp_id: app.session.active_comp_id().unwrap_or_default(),
                comp: &comp,
                time: app.session.time(),
                expr: None,
                footage: None,
                property_overrides: &[],
                media_overrides: &[],
            };
            let inv = l2c(&ectx, &l).0.inverse().unwrap_or(Mat3::IDENTITY);
            let p0 = inv.apply(gv2(cx - w / 2.0, cy - h / 2.0));
            let p1 = inv.apply(gv2(cx + w / 2.0, cy + h / 2.0));
            let rect = [p0.x.min(p1.x), p0.y.min(p1.y), (p1.x - p0.x).abs(), (p1.y - p0.y).abs()];
            if let Err(e) = app.session.execute("layer.addMask", json!({"layer": l.id.0, "shape": kind, "rect": rect})) {
                app.ui.status = e.to_string();
            }
        }
        return;
    }
    // Into the selected shape layer's Contents as a new group (as in After Effects), else a new
    // shape layer; painted with the Tools bar's Fill and Stroke.
    if let Err(e) = app.session.execute("shape.newShape", json!({"kind": kind, "size": [w, h], "position": [cx, cy]})) {
        app.ui.status = e.to_string();
    }
}
