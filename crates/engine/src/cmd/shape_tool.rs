//! The shape tools (Rectangle, Rounded Rectangle, Ellipse, Polygon, Star) and the Pen's options
//! in the Tools bar (Tool Creates Shape / Tool Creates Mask, Fill and Stroke with their Fill
//! Options / Stroke Options), and drawing with them: a drawn shape goes into the selected shape
//! layer's Contents as a new group, as in After Effects, or into a new shape layer.

use effectcraft_color::BlendMode;
use effectcraft_geom::vec2;
use effectcraft_keyframe::{Gradient, ShapePath, Value as KV};
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{Comp, ItemId, LayerId, LayerSource, Project, PropGroup};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::layer::{color_p, insert_layer, place, position_p};
use super::{CommandSpec, always, b_p, bad, comp_id, has_comp, layer_mut, str_p};
use crate::{EditorState, EngineError, Result, Session, cmd};

/// The shape tools' kinds, as `shape.newShape` / `layer.newShape` name them.
const KINDS: [&str; 5] = ["rect", "rounded", "ellipse", "polygon", "star"];

/// Fill Options / Stroke Options: what new shapes are painted with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PaintKind {
    None,
    #[default]
    Solid,
    Linear,
    Radial,
}

impl PaintKind {
    pub const ALL: [PaintKind; 4] = [PaintKind::None, PaintKind::Solid, PaintKind::Linear, PaintKind::Radial];

    /// The `fillType` / `strokeType` value.
    pub fn id(self) -> &'static str {
        match self {
            PaintKind::None => "none",
            PaintKind::Solid => "solid",
            PaintKind::Linear => "linear",
            PaintKind::Radial => "radial",
        }
    }

    /// After Effects' name in Fill Options / Stroke Options.
    pub fn label(self) -> &'static str {
        match self {
            PaintKind::None => "None",
            PaintKind::Solid => "Solid Color",
            PaintKind::Linear => "Linear Gradient",
            PaintKind::Radial => "Radial Gradient",
        }
    }

    /// An id or label, in any case and spacing (`radial`, `Radial Gradient`).
    pub fn from_name(s: &str) -> Option<PaintKind> {
        let k: String = s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_ascii_lowercase();
        PaintKind::ALL.into_iter().find(|m| m.id() == k || m.label().replace(' ', "").to_ascii_lowercase() == k)
    }
}

/// The Fill or the Stroke of the Tools bar: its kind, colour (for Solid Color), blend mode and
/// opacity (%).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ToolPaint {
    pub kind: PaintKind,
    pub color: [f32; 3],
    pub blend: BlendMode,
    pub opacity: f64,
}

impl Default for ToolPaint {
    fn default() -> Self {
        ToolPaint { kind: PaintKind::Solid, color: [1.0, 1.0, 1.0], blend: BlendMode::Normal, opacity: 100.0 }
    }
}

/// The shape tools' and the Pen's options: Tool Creates Mask (with a shape layer selected they
/// draw masks on it instead of shapes) and the Fill, Stroke and Stroke Width new shapes get.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ShapeTool {
    pub creates_mask: bool,
    pub fill: ToolPaint,
    pub stroke: ToolPaint,
    /// 0 draws no stroke.
    pub stroke_width: f64,
}

impl Default for ShapeTool {
    fn default() -> Self {
        ShapeTool { creates_mask: false, fill: ToolPaint { color: [0.25, 0.55, 1.0], ..ToolPaint::default() }, stroke: ToolPaint::default(), stroke_width: 0.0 }
    }
}

/// `p`'s paint keys over `base`: `fill` / `stroke` (a colour, which makes a None paint Solid
/// Color; `null` or `false` for None), `fillType` / `strokeType` (none|solid|linear|radial),
/// `fillBlend` / `strokeBlend` (a blend mode), `fillOpacity` / `strokeOpacity` (%) and
/// `strokeWidth` (0 draws no stroke).
pub(crate) fn paint_p(p: &Value, base: &ShapeTool, cmd: &str) -> Result<ShapeTool> {
    let mut o = base.clone();
    for (key, paint) in [("fill", &mut o.fill), ("stroke", &mut o.stroke)] {
        match p.get(key) {
            None => {}
            Some(Value::Null | Value::Bool(false)) => paint.kind = PaintKind::None,
            Some(_) => {
                paint.color = color_p(p, key).ok_or_else(|| bad(cmd, format!("`{key}`: a colour [r, g, b] or #hex, or false for none")))?;
                if paint.kind == PaintKind::None {
                    paint.kind = PaintKind::Solid;
                }
            }
        }
        if let Some(v) = p.get(format!("{key}Type").as_str()) {
            paint.kind = v.as_str().and_then(PaintKind::from_name).ok_or_else(|| bad(cmd, format!("`{key}Type`: none|solid|linear|radial")))?;
        }
        if let Some(v) = p.get(format!("{key}Blend").as_str()) {
            paint.blend = v.as_str().and_then(BlendMode::from_name).ok_or_else(|| bad(cmd, format!("`{key}Blend`: a blend mode name (normal, multiply…)")))?;
        }
        if let Some(v) = p.get(format!("{key}Opacity").as_str()) {
            paint.opacity = v.as_f64().filter(|v| v.is_finite()).ok_or_else(|| bad(cmd, format!("`{key}Opacity`: a percentage")))?.clamp(0.0, 100.0);
        }
    }
    if let Some(v) = p.get("strokeWidth") {
        o.stroke_width = v.as_f64().filter(|v| v.is_finite()).ok_or_else(|| bad(cmd, "`strokeWidth`: pixels"))?.max(0.0);
    }
    Ok(o)
}

/// A Fill or Stroke item with the paint's blend mode and opacity.
fn blended(mut g: PropGroup, paint: &ToolPaint) -> PropGroup {
    let blend = BlendMode::ALL.iter().position(|m| *m == paint.blend).unwrap_or(0) as u32;
    if let Some(pr) = g.get_mut("blend") {
        pr.value = KV::Enum(blend);
    }
    if let Some(pr) = g.get_mut("opacity") {
        pr.value = KV::Scalar(paint.opacity);
    }
    g
}

/// A gradient's Start and End Points across `bounds` (`[x, y, w, h]`): left to right, or out
/// from the centre.
pub(crate) fn gradient_ends(radial: bool, bounds: [f64; 4]) -> ([f64; 2], [f64; 2]) {
    let [x, y, w, h] = bounds;
    let mid = [x + w / 2.0, y + h / 2.0];
    if radial { (mid, [mid[0] + w.max(h).max(2.0) / 2.0, mid[1]]) } else { ([x, mid[1]], [x + w.max(1.0), mid[1]]) }
}

/// The Stroke and Fill a new shape gets from `paint`; gradients run across `bounds` (`[x, y, w,
/// h]` in the group's space): left to right, or out from the centre.
pub(crate) fn paint_items(ids: &mut Ids, paint: &ShapeTool, bounds: [f64; 4]) -> Vec<PropGroup> {
    let ends = |radial: bool| gradient_ends(radial, bounds);
    let mut items = vec![];
    let (s, width) = (&paint.stroke, paint.stroke_width);
    let stroke = match s.kind {
        _ if width <= 0.0 => None,
        PaintKind::None => None,
        PaintKind::Solid => Some(build::shape_stroke(ids, rgba(s.color), width)),
        k => {
            let (a, b) = ends(k == PaintKind::Radial);
            Some(build::shape_gradient_stroke(ids, k == PaintKind::Radial, a, b, Gradient::default(), width))
        }
    };
    items.extend(stroke.map(|g| blended(g, s)));
    let f = &paint.fill;
    let fill = match f.kind {
        PaintKind::None => None,
        PaintKind::Solid => Some(build::shape_fill(ids, rgba(f.color))),
        k => {
            let (a, b) = ends(k == PaintKind::Radial);
            Some(build::shape_gradient_fill(ids, k == PaintKind::Radial, a, b, Gradient::default()))
        }
    };
    items.extend(fill.map(|g| blended(g, f)));
    items
}

fn rgba(c: [f32; 3]) -> [f64; 4] {
    [c[0] as f64, c[1] as f64, c[2] as f64, 1.0]
}

/// The Rounded Rectangle tool's corner roundness: a fixed default, as in After Effects
/// (it does not grow with the drawn size).
fn roundness() -> f64 {
    20.0
}

/// The Polygon and Star tools' Polystar for `size`: (star, points, outer radius, inner radius).
fn polystar_of(kind: &str, size: [f64; 2]) -> Option<(bool, f64, f64, f64)> {
    match kind {
        "star" => Some((true, 5.0, size[0] / 2.0, size[0] / 4.0)),
        "polygon" => Some((false, 5.0, size[0] / 2.0, 0.0)),
        _ => None,
    }
}

/// A new shape group for a shape tool `kind` of `size`, centred on the group's origin, painted
/// with `paint` ("Rectangle 1", "Ellipse 1" or "Polystar 1"). None for another kind.
pub(crate) fn shape_group(ids: &mut Ids, kind: &str, size: [f64; 2], paint: &ShapeTool) -> Option<PropGroup> {
    let (path, gname) = match kind {
        "rect" | "rectangle" => (build::shape_rect(ids, size, [0.0, 0.0], 0.0), "Rectangle 1"),
        "rounded" | "roundedRect" => (build::shape_rect(ids, size, [0.0, 0.0], roundness()), "Rectangle 1"),
        "ellipse" => (build::shape_ellipse(ids, size, [0.0, 0.0]), "Ellipse 1"),
        k => {
            let (star, points, outer, inner) = polystar_of(k, size)?;
            (build::shape_star(ids, star, points, [0.0, 0.0], outer, inner), "Polystar 1")
        }
    };
    let mut items = vec![path];
    items.extend(paint_items(ids, paint, [-size[0] / 2.0, -size[1] / 2.0, size[0], size[1]]));
    Some(build::shape_group(ids, gname, items))
}

/// The path a shape tool draws: `kind` of `size` centred at `centre` (layer space). The geometry
/// of every one of the five kinds, so a caller that only wants to draw it (the viewer's
/// drag ghost) cannot drift from what `shape.newShape` / `layer.addMask` commit.
pub fn mask_path(kind: &str, centre: [f64; 2], size: [f64; 2]) -> Option<ShapePath> {
    let bez = match kind {
        "rect" | "rectangle" => return Some(ShapePath::rect(centre, size[0], size[1])),
        "ellipse" => return Some(ShapePath::ellipse(centre, size[0], size[1])),
        "rounded" | "roundedRect" => effectcraft_path::rect(size, centre, roundness()),
        k => {
            let (star, points, outer, inner) = polystar_of(k, size)?;
            effectcraft_path::polystar(effectcraft_path::Polystar {
                star,
                points,
                pos: centre,
                rotation: 0.0,
                inner,
                outer,
                inner_round: 0.0,
                outer_round: 0.0,
            })
        }
    };
    effectcraft_path::from_kurbo(&bez).into_iter().next()
}

/// The layer a shape tool draws into when no `layer` is given: the first selected unlocked shape
/// layer, else none (a new shape layer).
pub fn selected_shape_target(s: &Session, comp: &Comp) -> Option<LayerId> {
    s.state.selected_layers.iter().copied().find(|l| comp.layer(*l).is_some_and(|l| matches!(l.source, LayerSource::Shape) && !l.switches.locked))
}

/// The shape layer a shape tool or the Pen draws into: `layer` (which must be a shape layer),
/// else the first selected unlocked shape layer, else none (a new shape layer).
pub(crate) fn draw_target(s: &Session, comp: &Comp, p: &Value, cmd: &str) -> Result<Option<LayerId>> {
    let target = match p.get("layer") {
        Some(l) => Some(super::resolve_layer(comp, l).ok_or_else(|| bad(cmd, format!("no layer {l}")))?),
        None => selected_shape_target(s, comp),
    };
    if target.and_then(|l| comp.layer(l)).is_some_and(|l| !matches!(l.source, LayerSource::Shape)) {
        return Err(bad(cmd, "the layer is not a shape layer"));
    }
    Ok(target)
}

/// A new empty shape layer ("Shape Layer n" unless `name`) with its Position at `position` (comp
/// pixels; else the comp centre), above the selected layer and selected.
pub(crate) fn new_shape_layer(
    proj: &mut Project,
    st: &mut EditorState,
    comp: &Comp,
    cid: ItemId,
    name: Option<&str>,
    position: Option<[f64; 2]>,
) -> Result<LayerId> {
    let count = comp.layers.iter().filter(|l| matches!(l.source, LayerSource::Shape)).count();
    let name = name.map(str::to_string).unwrap_or_else(|| format!("Shape Layer {}", count + 1));
    let mut l = build::layer(proj, comp, &name, LayerSource::Shape, (comp.width, comp.height), None);
    place(&mut l, position);
    insert_layer(proj, st, cid, l)
}

/// Put `g` on top of the shape layer's Contents with a name unique there ("Rectangle 2"…) and
/// its Transform's Position at `position` (layer space). Returns its uid.
pub(crate) fn add_to_contents(
    proj: &mut Project,
    cid: ItemId,
    lid: LayerId,
    mut g: PropGroup,
    position: [f64; 2],
    cmd: &str,
) -> Result<effectcraft_project::Uid> {
    let contents = layer_mut(proj, cid, lid)?.props.sub_mut("contents").ok_or_else(|| bad(cmd, "the layer has no contents"))?;
    g.name = super::effect::unique_name(contents, &g.name);
    if let Some(pr) = g.sub_mut("transform").and_then(|t| t.get_mut("position")) {
        pr.value = KV::Vec2(position);
    }
    let uid = g.uid;
    contents.children.insert(0, g.into());
    Ok(uid)
}

/// A finite `[w, h]` / `[x, y]` parameter.
fn pair_p(p: &Value, k: &str) -> Option<[f64; 2]> {
    let a = p.get(k)?.as_array()?;
    Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?]).filter(|v| v.iter().all(|x| x.is_finite()))
}

/// The shape tools: a rectangle, rounded rectangle, ellipse, polygon or star of `size`, centred
/// at `position`, painted with the Tools bar's Fill and Stroke unless given. With a shape layer
/// given or selected it goes on top of that layer's Contents as a new group ("Rectangle 2"…)
/// placed by the group's Transform, as After Effects draws into the selected shape layer;
/// otherwise a new shape layer is centred on it. `position` and `size` are in comp pixels, or
/// the layer's space with `space: "layer"`.
fn new_shape(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "shape.newShape";
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let kind = str_p(p, "kind").unwrap_or("rect");
    if !KINDS.contains(&kind) {
        return Err(bad(c, format!("unknown kind `{kind}`; one of: {}", KINDS.join(", "))));
    }
    let size = match p.get("size") {
        None => [200.0, 200.0],
        Some(_) => pair_p(p, "size").filter(|s| s.iter().all(|v| *v >= 0.0)).ok_or_else(|| bad(c, "size: [w, h], not negative"))?,
    };
    let pos = position_p(p, c)?.unwrap_or([comp.width as f64 / 2.0, comp.height as f64 / 2.0]);
    let paint = paint_p(p, &s.state.shape_tool, c)?;
    let target = draw_target(s, &comp, p, c)?;
    // In the target layer's space: the centre maps through the layer's transform, the size by
    // its scale.
    let (centre, size) = match target.and_then(|l| comp.layer(l)) {
        Some(l) if str_p(p, "space") != Some("layer") => {
            let ctx = effectcraft_render::EvalCtx::new(&s.project, cid, &comp, s.time_of(cid));
            let inv = ctx.layer_to_comp(l).0.inverse().ok_or_else(|| bad(c, "the layer is scaled to nothing"))?;
            let q = inv.apply(vec2(pos[0], pos[1]));
            ([q.x, q.y], [inv.apply_vec(vec2(size[0], 0.0)).length(), inv.apply_vec(vec2(0.0, size[1])).length()])
        }
        Some(_) => (pos, size),
        None => ([0.0, 0.0], size),
    };
    let label = match kind {
        "rounded" => "Rounded Rectangle Tool",
        "ellipse" => "Ellipse Tool",
        "polygon" => "Polygon Tool",
        "star" => "Star Tool",
        _ => "Rectangle Tool",
    };
    let name = str_p(p, "name");
    let (lid, group) = s.edit(label, None, |proj, st| {
        let lid = match target {
            Some(l) => l,
            None => new_shape_layer(proj, st, &comp, cid, name, Some(pos))?,
        };
        let mut next = proj.next_id;
        let g = shape_group(&mut Ids(&mut next), kind, size, &paint).ok_or_else(|| bad(c, "unknown kind"))?;
        proj.next_id = next;
        let group = add_to_contents(proj, cid, lid, g, centre, c)?;
        st.selected_layers = vec![lid];
        Ok((lid, group))
    })?;
    Ok(json!({"layer": lid.0, "group": group}))
}

/// The Tools bar's options for the shape tools and the Pen: Tool Creates Shape / Tool Creates
/// Mask (`createsMask`) and the Fill and Stroke new shapes get (the keys of [`paint_p`]);
/// `reset` restores the defaults first. Reports the options.
fn tool_options(s: &mut Session, p: &Value) -> Result<Value> {
    let base = if b_p(p, "reset") == Some(true) { ShapeTool::default() } else { s.state.shape_tool.clone() };
    let mut o = paint_p(p, &base, "shape.toolOptions")?;
    if let Some(m) = b_p(p, "createsMask") {
        o.creates_mask = m;
    }
    s.state.shape_tool = o;
    Ok(serde_json::to_value(&s.state.shape_tool).unwrap_or(Value::Null))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "shape.newShape",
            "Shape Tool",
            [],
            None,
            "{layer?, kind?: rect|rounded|ellipse|polygon|star, size? [w,h], position? [x,y] (the centre), space?: comp|layer, name? (a new layer's), fill?, fillType?, fillBlend?, fillOpacity?, stroke?, strokeType?, strokeBlend?, strokeOpacity?, strokeWidth? (default the Tools bar's, shape.toolOptions)} → {layer, group}",
            has_comp,
            new_shape
        ),
        cmd!(
            "shape.toolOptions",
            "Shape Tool Options",
            [],
            None,
            "{createsMask?: bool (Tool Creates Mask with a shape layer selected), fill?: [r,g,b]|#hex|false, fillType?: none|solid|linear|radial, fillBlend?: blend mode, fillOpacity? %, stroke?, strokeType?, strokeBlend?, strokeOpacity?, strokeWidth? px, reset?} → the options",
            always,
            tool_options
        ),
    ]
}
