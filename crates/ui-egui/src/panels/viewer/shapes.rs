use super::geometry::{l2c, layer_quad, point_in_quad};
use super::{
    Color32, Comp, EffectcraftApp, EvalCtx, Layer, LayerId, LayerSource, Mat3, PaintKind, Pos2, Rect, ShapeOptions, Stroke, Tick, Tokens, Tool, ViewerMap, gv2,
    json, ov, pos2,
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
    /// What the keys made of it while it was dragged out.
    options: ShapeOptions,
    /// The selected layer's name and the index its next mask would take, for the ghost's caption
    /// and colour.
    layer_name: String,
    mask_index: usize,
}

/// The kind `tool` draws, as `shape.newShape` / `layer.addMask` name it.
pub(super) fn shape_kind(tool: Tool) -> &'static str {
    match tool {
        Tool::Rectangle => "rect",
        Tool::RoundedRect => "rounded",
        Tool::Ellipse => "ellipse",
        Tool::Polygon => "polygon",
        _ => "star",
    }
}

/// What a shape tool drag from `start` to `end` makes, or `None` while it is too small to make
/// anything. Cmd (Ctrl) draws from the press point and Shift squares the box; the live ghost and
/// [`create_shape`] both go through this, so the drag always shows what it will commit.
pub(super) fn shape_drag(
    app: &EffectcraftApp,
    tool: Tool,
    start: [f64; 2],
    end: [f64; 2],
    mods: egui::Modifiers,
    mut options: ShapeOptions,
) -> Option<ShapeDrag> {
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
    if matches!(tool, Tool::Polygon | Tool::Star) {
        let radius = (end[0] - start[0]).hypot(end[1] - start[1]);
        if radius < 1.0 {
            return None;
        }
        options.rotation = if mods.shift { 0.0 } else { (end[1] - start[1]).atan2(end[0] - start[0]).to_degrees() + 90.0 };
        return Some(ShapeDrag {
            kind: shape_kind(tool),
            centre: start,
            size: [radius * 2.0; 2],
            target,
            options,
            layer_name: layer.map_or_else(String::new, |l| l.name.clone()),
            mask_index,
        });
    }
    let (mut w, mut h) = ((end[0] - start[0]).abs(), (end[1] - start[1]).abs());
    // Ctrl/Cmd draws from the press point as the centre; on a star it changes the outer radius
    // alone instead (`shape_draft`).
    let centred = mods.command && tool != Tool::Star;
    if centred {
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
        centre: if centred { start } else { [start[0] + (end[0] - start[0]).signum() * w / 2.0, start[1] + (end[1] - start[1]).signum() * h / 2.0] },
        size: [w, h],
        target,
        options,
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
    let Some(sp) = effectcraft_engine::commands::shape_tool::mask_path(d.kind, d.centre, d.size, &d.options) else { return };
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

pub(super) fn create_shape(app: &mut EffectcraftApp, tool: Tool, a: [f64; 2], b: [f64; 2], mods: egui::Modifiers, options: ShapeOptions) {
    let Some(d) = shape_drag(app, tool, a, b, mods, options) else { return };
    // The shape's own parameters join the command's.
    let with_options = |mut q: serde_json::Value| {
        if let (Some(q), serde_json::Value::Object(o)) = (q.as_object_mut(), d.options.to_params()) {
            q.extend(o);
        }
        q
    };
    let (kind, (w, h), (cx, cy)) = (d.kind, (d.size[0], d.size[1]), (d.centre[0], d.centre[1]));
    if d.target == ShapeTarget::Mask {
        create_mask(app, &d);
        return;
    }
    // Into the selected shape layer's Contents as a new group (as in After Effects), else a new
    // shape layer; painted with the Tools bar's Fill and Stroke.
    if let Err(e) = app.session.execute("shape.newShape", with_options(json!({"kind": kind, "size": [w, h], "position": [cx, cy]}))) {
        app.ui.status = e.to_string();
    }
}

/// Commit the complete preview path, including Bezier handles, in the target layer's space.
fn create_mask(app: &mut EffectcraftApp, draft: &ShapeDrag) {
    let Some(comp) = app.session.active_comp_arc() else { return };
    let Some(layer) = app.session.state.selected_layers.first().and_then(|id| comp.layer(*id)) else { return };
    let Some(cid) = app.session.active_comp_id() else { return };
    let ctx = EvalCtx { expr: app.session.expr.as_deref(), ..EvalCtx::new(&app.session.project, cid, &comp, app.session.time()) };
    let Some(inv) = l2c(&ctx, layer).0.inverse() else {
        app.ui.status = "Cannot create a mask on a layer with a non-invertible transform".into();
        return;
    };
    let Some(mut path) = effectcraft_engine::commands::shape_tool::mask_path(draft.kind, draft.centre, draft.size, &draft.options) else { return };
    for point in &mut path.vertices {
        let q = inv.apply(gv2(point[0], point[1]));
        *point = [q.x, q.y];
    }
    for tangent in path.in_tangents.iter_mut().chain(&mut path.out_tangents) {
        let q = inv.apply_vec(gv2(tangent[0], tangent[1]));
        *tangent = [q.x, q.y];
    }
    if let Err(error) = app.session.execute(
        "mask.new",
        json!({
            "layer":layer.id.0,"vertices":path.vertices,"inTangents":path.in_tangents,"outTangents":path.out_tangents,"closed":path.closed
        }),
    ) {
        app.ui.status = error.to_string();
    }
}

#[cfg(test)]
mod mask_creation_tests {
    use super::{EffectcraftApp, EvalCtx, ShapeOptions, Tool, create_shape, gv2, json, shape_drag};
    use effectcraft_engine::Session;

    #[test]
    fn created_mask_matches_composition_preview_on_transformed_layer() {
        for (tool, kind) in
            [(Tool::Rectangle, "rect"), (Tool::RoundedRect, "rounded"), (Tool::Ellipse, "ellipse"), (Tool::Polygon, "polygon"), (Tool::Star, "star")]
        {
            let mut session = Session::default();
            session.execute("comp.new", json!({"width":640,"height":360,"duration":2})).unwrap();
            let lid = session.execute("layer.newSolid", json!({"width":320,"height":200})).unwrap()["layer"].as_u64().unwrap();
            for (path, value) in [("position", json!([280, 180, 0])), ("rotation", json!(37)), ("scale", json!([125, 75, 100]))] {
                session.execute("prop.set", json!({"layer":lid,"path":format!("transform/{path}"),"value":value})).unwrap();
            }
            let mut app = EffectcraftApp::new(session);
            let options = ShapeOptions { points: Some(7.0), roundness: Some(35.0), outer_roundness: 15.0, inner_roundness: 10.0, ..Default::default() };
            let draft = shape_drag(&app, tool, [100.0, 80.0], [300.0, 220.0], egui::Modifiers::NONE, options).unwrap();
            let expected = effectcraft_engine::commands::shape_tool::mask_path(kind, draft.centre, draft.size, &draft.options).unwrap();
            create_shape(&mut app, tool, [100.0, 80.0], [300.0, 220.0], egui::Modifiers::NONE, options);
            assert!(app.ui.status.is_empty(), "{}", app.ui.status);
            let cid = app.session.active_comp_id().unwrap();
            let comp = app.session.active_comp().unwrap();
            let layer = comp.layer(effectcraft_engine::project::LayerId(lid)).unwrap();
            let ctx = EvalCtx::new(&app.session.project, cid, comp, app.session.time());
            let matrix = ctx.layer_to_comp(layer).0;
            let mask = layer.masks().unwrap().groups().last().unwrap();
            let actual = mask.get("path").unwrap().value.as_path().unwrap();
            assert_eq!(actual.vertices.len(), expected.vertices.len(), "{kind}");
            for (a, b) in actual.vertices.iter().zip(&expected.vertices) {
                let p = matrix.apply(gv2(a[0], a[1]));
                assert!((p.x - b[0]).abs() < 1e-7 && (p.y - b[1]).abs() < 1e-7, "{kind}: vertex {p:?} != {b:?}");
            }
            for (a, b) in actual.in_tangents.iter().chain(&actual.out_tangents).zip(expected.in_tangents.iter().chain(&expected.out_tangents)) {
                let p = matrix.apply_vec(gv2(a[0], a[1]));
                assert!((p.x - b[0]).abs() < 1e-7 && (p.y - b[1]).abs() < 1e-7, "{kind}: tangent {p:?} != {b:?}");
            }
        }
    }
}
