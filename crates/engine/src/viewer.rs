//! Composition viewer maths shared by the frontends and tests: snapping (targets, tolerance and
//! priority), the Show Channel / Exposure display transforms, and the viewer snapshot.
//!
//! Nothing here touches the project; edits go through commands.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use effectcraft_color::{ColorSpace, space};
use effectcraft_geom::{Mat3, vec2};
use effectcraft_keyframe::Value;
use effectcraft_project::{Layer, LayerId, Property};
use effectcraft_raster::Image;
use effectcraft_render::{EvalCtx, ExprHost};
use effectcraft_time::Tick;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------- snapping

/// What a snap target is: a point (snaps both axes) or an axis-aligned line.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum SnapKind {
    Point,
    /// A vertical line at `pos[0]` (snaps x only).
    VLine,
    /// A horizontal line at `pos[1]` (snaps y only).
    HLine,
}

/// Where a snap target comes from (for feedback: the highlighted box / cross).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum SnapSource {
    Layer(LayerId),
    Comp,
    Guide,
    Grid,
    /// A mask or shape path vertex of a layer.
    Vertex(LayerId),
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SnapTarget {
    pub kind: SnapKind,
    /// Comp pixels.
    pub pos: [f64; 2],
    /// Lower wins when two targets are equally near (vertices and layer features before
    /// guides, guides before the grid).
    pub priority: u8,
    pub source: SnapSource,
    /// A line's extent along itself (y of a vertical line, x of a horizontal one): a layer's
    /// edge without Snap Edges Extended. `None` runs across the whole comp.
    #[serde(default)]
    pub span: Option<[f64; 2]>,
}

impl SnapTarget {
    pub fn point(pos: [f64; 2], priority: u8, source: SnapSource) -> SnapTarget {
        SnapTarget { kind: SnapKind::Point, pos, priority, source, span: None }
    }
    pub fn vline(x: f64, priority: u8, source: SnapSource) -> SnapTarget {
        SnapTarget { kind: SnapKind::VLine, pos: [x, 0.0], priority, source, span: None }
    }
    pub fn hline(y: f64, priority: u8, source: SnapSource) -> SnapTarget {
        SnapTarget { kind: SnapKind::HLine, pos: [0.0, y], priority, source, span: None }
    }
    /// The line limited to `span` (`None`: across the comp).
    pub fn within(self, span: Option<[f64; 2]>) -> SnapTarget {
        SnapTarget { span: span.map(|[a, b]| [a.min(b), a.max(b)]), ..self }
    }
}

/// Tools bar ▸ Snapping options, the two After Effects' Snapping menu has: Snap Edges Extended
/// (layer edges snap along their whole line, beyond the layer's bounds) and Snap to Features in
/// Collapsed Compositions and Text Layers (the layers inside a precomp layer with Collapse
/// Transformations on are targets too). Both are on by default. Every layer feature snaps
/// (corners, edge midpoints, centre, anchor point, mask and shape path points), as in After
/// Effects.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SnapFeatures {
    pub edges_extended: bool,
    pub collapsed_features: bool,
}

impl Default for SnapFeatures {
    fn default() -> Self {
        SnapFeatures { edges_extended: true, collapsed_features: true }
    }
}

impl SnapFeatures {
    /// The options in the Tools bar's Snapping menu order: (parameter key, label).
    pub const OPTIONS: [(&'static str, &'static str); 2] =
        [("edgesExtended", "Snap Edges Extended"), ("collapsedFeatures", "Snap to Features in Collapsed Compositions and Text Layers")];
    /// The option called `key` (an [`OPTIONS`](Self::OPTIONS) key).
    pub fn option_mut(&mut self, key: &str) -> Option<&mut bool> {
        Some(match key {
            "edgesExtended" => &mut self.edges_extended,
            "collapsedFeatures" => &mut self.collapsed_features,
            _ => return None,
        })
    }
    pub fn option(mut self, key: &str) -> bool {
        self.option_mut(key).is_some_and(|v| *v)
    }
}

/// Priorities of the standard targets.
pub const PRI_VERTEX: u8 = 0;
pub const PRI_LAYER: u8 = 1;
pub const PRI_COMP: u8 = 2;
pub const PRI_GUIDE: u8 = 3;
pub const PRI_GRID: u8 = 4;

/// A snap: move the dragged feature by `delta` (comp pixels); `hits` are the targets it lands on.
#[derive(Clone, Debug, PartialEq)]
pub struct Snap {
    pub delta: [f64; 2],
    pub hits: Vec<SnapTarget>,
    /// The snapped feature position.
    pub at: [f64; 2],
}

fn better(a: (f64, u8), b: (f64, u8)) -> bool {
    if (a.0 - b.0).abs() < 1e-6 { a.1 < b.1 } else { a.0 < b.0 }
}

/// Snap feature points `sources` (comp pixels) to the nearest target within `tol` comp pixels.
/// A point target snaps both axes and wins when it is at least as near as the best lines;
/// otherwise the nearest vertical and horizontal lines snap x and y independently. Ties go to
/// the lower priority value.
pub fn snap(sources: &[[f64; 2]], targets: &[SnapTarget], tol: f64) -> Option<Snap> {
    let mut best_pt: Option<((f64, u8), [f64; 2], SnapTarget)> = None;
    let mut best_x: Option<((f64, u8), [f64; 2], SnapTarget)> = None;
    let mut best_y: Option<((f64, u8), [f64; 2], SnapTarget)> = None;
    for s in sources {
        for t in targets {
            // (`along`: where the source is along a line, for its span.)
            let (d, slot, along) = match t.kind {
                SnapKind::Point => (((t.pos[0] - s[0]).powi(2) + (t.pos[1] - s[1]).powi(2)).sqrt(), &mut best_pt, 0.0),
                SnapKind::VLine => ((t.pos[0] - s[0]).abs(), &mut best_x, s[1]),
                SnapKind::HLine => ((t.pos[1] - s[1]).abs(), &mut best_y, s[0]),
            };
            if d > tol || t.span.is_some_and(|[a, b]| along < a || along > b) {
                continue;
            }
            let key = (d, t.priority);
            if slot.as_ref().is_none_or(|(k, _, _)| better(key, *k)) {
                *slot = Some((key, *s, *t));
            }
        }
    }
    let line_best = [best_x.map(|b| b.0.0), best_y.map(|b| b.0.0)].into_iter().flatten().fold(f64::INFINITY, f64::min);
    if let Some((k, s, t)) = best_pt
        && k.0 <= line_best + 1e-9
    {
        return Some(Snap { delta: [t.pos[0] - s[0], t.pos[1] - s[1]], hits: vec![t], at: t.pos });
    }
    if best_x.is_none() && best_y.is_none() {
        return None;
    }
    let mut delta = [0.0; 2];
    let mut hits = vec![];
    let mut at = [f64::NAN; 2];
    if let Some((_, s, t)) = best_x {
        delta[0] = t.pos[0] - s[0];
        at = [t.pos[0], s[1]];
        hits.push(t);
    }
    if let Some((_, s, t)) = best_y {
        delta[1] = t.pos[1] - s[1];
        at = if at[0].is_nan() { [s[0], t.pos[1]] } else { [at[0], t.pos[1]] };
        hits.push(t);
    }
    Some(Snap { delta, hits, at })
}

/// Snap features of a layer in comp space: corners, edge midpoints, centre, anchor point and
/// mask / shape path vertices (no box for layers without bounds).
pub fn layer_features(ctx: &EvalCtx, layer: &Layer) -> Vec<[f64; 2]> {
    let (m, _) = ctx.layer_to_comp(layer);
    let mut out = layer_box(ctx, layer, &m).map(|b| b.points()).unwrap_or_default();
    out.extend(anchor_feature(ctx, layer, &m));
    out.extend(layer_vertices(ctx, layer));
    out
}

/// A layer's content box in comp space: corners (top left, top right, bottom right, bottom
/// left), edge midpoints (top, right, bottom, left) and centre.
struct BoxPoints {
    corners: [[f64; 2]; 4],
    mids: [[f64; 2]; 4],
    center: [f64; 2],
}

impl BoxPoints {
    /// Corners, edge midpoints and centre.
    fn points(&self) -> Vec<[f64; 2]> {
        self.corners.iter().chain(&self.mids).chain([&self.center]).copied().collect()
    }
}

/// A layer's content box (`m`: layer → comp).
fn layer_box(ctx: &EvalCtx, layer: &Layer, m: &Mat3) -> Option<BoxPoints> {
    let b = effectcraft_render::content_bounds(ctx, layer)?;
    let (cx, cy) = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
    let c = |x: f64, y: f64| {
        let q = m.apply(vec2(x, y));
        [q.x, q.y]
    };
    Some(BoxPoints {
        corners: [c(b[0], b[1]), c(b[2], b[1]), c(b[2], b[3]), c(b[0], b[3])],
        mids: [c(cx, b[1]), c(b[2], cy), c(cx, b[3]), c(b[0], cy)],
        center: c(cx, cy),
    })
}

/// A layer's anchor point (`m`: layer → comp).
fn anchor_feature(ctx: &EvalCtx, layer: &Layer, m: &Mat3) -> Option<[f64; 2]> {
    let a = ctx.v3(layer, layer.transform()?, "anchor", [0.0; 3]);
    let q = m.apply(vec2(a[0], a[1]));
    Some([q.x, q.y])
}

/// Mask and shape-path vertices of a layer in comp space.
pub fn layer_vertices(ctx: &EvalCtx, layer: &Layer) -> Vec<[f64; 2]> {
    let (m, _) = ctx.layer_to_comp(layer);
    let mut out = vec![];
    if let Some(masks) = layer.masks() {
        for g in masks.groups() {
            if let Some(sp) = g.get("path").map(|pr| ctx.value(layer, pr)).and_then(|v| v.as_path().cloned()) {
                out.extend(sp.vertices.iter().map(|v| {
                    let q = m.apply(vec2(v[0], v[1]));
                    [q.x, q.y]
                }));
            }
        }
    }
    for (uid, sp) in shape_paths(ctx, layer) {
        let pm = m * effectcraft_render::shapes::item_matrix(ctx, layer, uid).unwrap_or(Mat3::IDENTITY);
        out.extend(sp.vertices.iter().map(|v| {
            let q = pm.apply(vec2(v[0], v[1]));
            [q.x, q.y]
        }));
    }
    out
}

/// Shape-layer Path items (uid, evaluated path in the item's group space).
pub fn shape_paths(ctx: &EvalCtx, layer: &Layer) -> Vec<(u64, effectcraft_keyframe::ShapePath)> {
    let mut out = vec![];
    let Some(c) = layer.props.sub("contents") else { return out };
    fn walk(ctx: &EvalCtx, layer: &Layer, g: &effectcraft_project::PropGroup, out: &mut Vec<(u64, effectcraft_keyframe::ShapePath)>) {
        for sub in g.groups() {
            if sub.match_id == "path"
                && let Some(sp) = sub.get("path").map(|pr| ctx.value(layer, pr)).and_then(|v| v.as_path().cloned())
            {
                out.push((sub.uid, sp));
            } else if sub.match_id == "group" || sub.match_id == "contents" {
                walk(ctx, layer, sub, out);
            }
        }
    }
    walk(ctx, layer, c, &mut out);
    out
}

/// Which target families are on.
#[derive(Clone, Copy, Debug, Default)]
pub struct SnapOptions {
    /// Layer and comp targets (the Snapping checkbox, inverted while Cmd/Ctrl is held).
    pub layers: bool,
    pub features: SnapFeatures,
    pub guides: bool,
    pub grid: bool,
    pub grid_spacing: f64,
}

type SnapValues = HashMap<(u64, u64, u64, i64), Result<Value, String>>;

/// Bounds, vertices and transforms can ask for the same expression repeatedly. Share its
/// value only within this immutable snap query; a new pointer update starts a fresh cache.
struct SnapExpressions<'a> {
    inner: &'a dyn ExprHost,
    values: Mutex<SnapValues>,
}

impl ExprHost for SnapExpressions<'_> {
    fn eval(&self, ctx: &EvalCtx, layer: &Layer, prop: &Property, value: &Value) -> Result<Value, String> {
        let key = (ctx.comp_id.0, layer.id.0, prop.uid, ctx.time.0);
        if let Some(value) = self.values.lock().unwrap_or_else(PoisonError::into_inner).get(&key).cloned() {
            return value;
        }
        // Never hold the lock while evaluating: expressions may query other properties.
        let result = self.inner.eval(ctx, layer, prop, value);
        let mut values = self.values.lock().unwrap_or_else(PoisonError::into_inner);
        if values.len() < 8192 {
            values.insert(key, result.clone());
        }
        result
    }

    fn eval_text_selector(&self, ctx: &EvalCtx, layer: &Layer, prop: &Property, index: usize, total: usize, selector: [f64; 3]) -> Result<[f64; 3], String> {
        self.inner.eval_text_selector(ctx, layer, prop, index, total, selector)
    }
}

/// Snap targets of a comp at the context time: other visible layers' features and vertices and
/// the comp's edges and centre (with `layers`, as the Snapping options allow), and guides and
/// grid lines (when on). Layers in `exclude` (the dragged ones) are skipped.
pub fn targets(ctx: &EvalCtx, exclude: &[LayerId], opts: SnapOptions) -> Vec<SnapTarget> {
    let memo = ctx.expr.map(|inner| SnapExpressions { inner, values: Mutex::default() });
    let ctx = EvalCtx { expr: memo.as_ref().map(|h| h as &dyn ExprHost), ..*ctx };
    targets_in(&ctx, exclude, opts)
}

fn targets_in(ctx: &EvalCtx, exclude: &[LayerId], opts: SnapOptions) -> Vec<SnapTarget> {
    let comp = ctx.comp;
    let (w, h) = (comp.width as f64, comp.height as f64);
    let f = opts.features;
    let mut out = vec![];
    if opts.layers {
        // The comp's edge and centre lines, along the comp without Snap Edges Extended.
        let (xs, ys) = if f.edges_extended { (None, None) } else { (Some([0.0, w]), Some([0.0, h])) };
        let mut lines = |x: &[f64], y: &[f64]| {
            out.extend(x.iter().map(|x| SnapTarget::vline(*x, PRI_COMP, SnapSource::Comp).within(ys)));
            out.extend(y.iter().map(|y| SnapTarget::hline(*y, PRI_COMP, SnapSource::Comp).within(xs)));
        };
        lines(&[0.0, w], &[0.0, h]);
        lines(&[w / 2.0], &[h / 2.0]);
        out.push(SnapTarget::point([w / 2.0, h / 2.0], PRI_COMP, SnapSource::Comp));
        for l in visible_layers(ctx, exclude) {
            out.extend(layer_targets(ctx, l, true, f));
        }
    }
    if opts.guides {
        for g in &comp.guides {
            let pos = g.px(comp.width, comp.height);
            out.push(if g.vertical { SnapTarget::vline(pos, PRI_GUIDE, SnapSource::Guide) } else { SnapTarget::hline(pos, PRI_GUIDE, SnapSource::Guide) });
        }
    }
    if opts.grid && opts.grid_spacing >= 1.0 {
        let s = opts.grid_spacing;
        let mut x = 0.0;
        while x <= w + 1e-9 && out.len() < 20_000 {
            out.push(SnapTarget::vline(x, PRI_GRID, SnapSource::Grid));
            x += s;
        }
        let mut y = 0.0;
        while y <= h + 1e-9 && out.len() < 40_000 {
            out.push(SnapTarget::hline(y, PRI_GRID, SnapSource::Grid));
            y += s;
        }
    }
    out
}

/// How deep Snap to Features in Collapsed Compositions looks into nested collapsed precomps.
const MAX_COLLAPSED_DEPTH: usize = 8;

/// Visible layers of a comp at the context time: active, eye on, and soloed while any layer is.
fn visible_layers<'a>(ctx: &EvalCtx<'a>, exclude: &'a [LayerId]) -> impl Iterator<Item = &'a Layer> {
    let comp = ctx.comp;
    let time = ctx.time;
    let any_solo = comp.layers.iter().any(|l| l.switches.solo && l.source.is_av());
    comp.layers.iter().filter(move |l| l.is_active_at(time) && l.switches.video && (!any_solo || l.switches.solo) && !exclude.contains(&l.id))
}

/// A 2D layer's snap targets: its box points, its edges as lines while unrotated (across the
/// comp with Snap Edges Extended, along the layer otherwise), its anchor point unless `anchor` is
/// false (Pan Behind drags it, the box stays put), its mask and shape path vertices and, with
/// Snap to Features in Collapsed Compositions, the targets of the visible layers inside a
/// collapsed precomp layer. None for cameras, lights and 3D layers.
pub fn layer_targets(ctx: &EvalCtx, l: &Layer, anchor: bool, f: SnapFeatures) -> Vec<SnapTarget> {
    layer_targets_at(ctx, l, anchor, f, 0)
}

fn layer_targets_at(ctx: &EvalCtx, l: &Layer, anchor: bool, f: SnapFeatures, depth: usize) -> Vec<SnapTarget> {
    if l.is_camera() || l.is_light() || l.is_3d() {
        return vec![];
    }
    let (m, _) = ctx.layer_to_comp(l);
    let src = SnapSource::Layer(l.id);
    let mut out = vec![];
    if let Some(b) = layer_box(ctx, l, &m) {
        let [nw, ne, _, sw] = b.corners;
        if (nw[1] - ne[1]).abs() < 1e-6 && (nw[0] - sw[0]).abs() < 1e-6 {
            let (xs, ys) = if f.edges_extended { (None, None) } else { (Some([nw[0], ne[0]]), Some([nw[1], sw[1]])) };
            for x in [nw[0], ne[0]] {
                out.push(SnapTarget::vline(x, PRI_LAYER, src).within(ys));
            }
            for y in [nw[1], sw[1]] {
                out.push(SnapTarget::hline(y, PRI_LAYER, src).within(xs));
            }
        }
        out.extend(b.points().into_iter().map(|p| SnapTarget::point(p, PRI_LAYER, src)));
    }
    out.extend(anchor_feature(ctx, l, &m).filter(|_| anchor).map(|p| SnapTarget::point(p, PRI_LAYER, src)));
    out.extend(layer_vertices(ctx, l).into_iter().map(|p| SnapTarget::point(p, PRI_VERTEX, SnapSource::Vertex(l.id))));
    if f.collapsed_features
        && l.switches.collapse
        && depth < MAX_COLLAPSED_DEPTH
        && let effectcraft_project::LayerSource::Comp { item } = l.source
        && let Some(nc) = ctx.project.comp(item)
    {
        out.extend(collapsed_targets(ctx, l, item, nc, &m, f, depth));
    }
    out
}

/// The targets of the layers inside collapsed precomp layer `l` (showing comp `nc`), in the
/// outer comp (`m`: `l`'s layer → comp). They highlight `l` when they snap. Lines stay lines
/// only while `l` is unrotated; otherwise only the points snap.
fn collapsed_targets(
    ctx: &EvalCtx,
    l: &Layer,
    item: effectcraft_project::ItemId,
    nc: &effectcraft_project::Comp,
    m: &Mat3,
    f: SnapFeatures,
    depth: usize,
) -> Vec<SnapTarget> {
    let nctx = EvalCtx { comp_id: item, comp: nc, time: ctx.nested_time(l), ..*ctx };
    let axis_aligned = m.0[0][1].abs() < 1e-9 && m.0[1][0].abs() < 1e-9;
    let (sx, ox, sy, oy) = (m.0[0][0], m.0[0][2], m.0[1][1], m.0[1][2]);
    let mut out = vec![];
    for nl in visible_layers(&nctx, &[]) {
        for t in layer_targets_at(&nctx, nl, true, f, depth + 1) {
            let source = match t.source {
                SnapSource::Vertex(_) => SnapSource::Vertex(l.id),
                _ => SnapSource::Layer(l.id),
            };
            let mapped = match t.kind {
                SnapKind::Point => {
                    let q = m.apply(vec2(t.pos[0], t.pos[1]));
                    Some(SnapTarget::point([q.x, q.y], t.priority, source))
                }
                SnapKind::VLine if axis_aligned => {
                    Some(SnapTarget::vline(sx * t.pos[0] + ox, t.priority, source).within(t.span.map(|[a, b]| [sy * a + oy, sy * b + oy])))
                }
                SnapKind::HLine if axis_aligned => {
                    Some(SnapTarget::hline(sy * t.pos[1] + oy, t.priority, source).within(t.span.map(|[a, b]| [sx * a + ox, sx * b + ox])))
                }
                _ => None,
            };
            out.extend(mapped);
        }
    }
    out
}

// ---------------------------------------------------------------- channels and exposure

/// Show Channel and Color Management Settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Channel {
    #[default]
    Rgb,
    Red,
    Green,
    Blue,
    Alpha,
    RgbStraight,
}

impl Channel {
    pub const ALL: [Channel; 6] = [Channel::Rgb, Channel::Red, Channel::Green, Channel::Blue, Channel::Alpha, Channel::RgbStraight];
    pub fn id(self) -> &'static str {
        match self {
            Channel::Rgb => "rgb",
            Channel::Red => "red",
            Channel::Green => "green",
            Channel::Blue => "blue",
            Channel::Alpha => "alpha",
            Channel::RgbStraight => "rgbStraight",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Channel::Rgb => "RGB",
            Channel::Red => "Red",
            Channel::Green => "Green",
            Channel::Blue => "Blue",
            Channel::Alpha => "Alpha",
            Channel::RgbStraight => "RGB Straight",
        }
    }
    pub fn from_name(s: &str) -> Option<Channel> {
        let n = s.to_ascii_lowercase().replace([' ', '_', '-'], "");
        Channel::ALL.into_iter().find(|c| c.id().to_ascii_lowercase() == n || c.label().to_ascii_lowercase().replace(' ', "") == n)
    }
}

fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}
fn linear_to_srgb(v: f32) -> f32 {
    let v = v.max(0.0);
    if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

/// 8-bit exposure lookup table: `stops` applied in linear light.
pub fn exposure_lut(stops: f32) -> [u8; 256] {
    let k = 2f32.powf(stops);
    let mut lut = [0u8; 256];
    for (i, o) in lut.iter_mut().enumerate() {
        let lin = srgb_to_linear(i as f32 / 255.0) * k;
        *o = (linear_to_srgb(lin).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    }
    lut
}

/// Apply the viewer's channel and exposure to premultiplied 8-bit RGBA pixels in place.
/// RGB shows the image (exposure only); Red/Green/Blue/Alpha show one channel as an opaque
/// grayscale image (or tinted in that channel's colour when `colorized`); RGB Straight shows the
/// unpremultiplied colour, opaque.
pub fn display_transform(px: &mut [[u8; 4]], channel: Channel, colorized: bool, stops: f32) {
    let lut = (stops != 0.0).then(|| exposure_lut(stops));
    let ex = |v: u8| lut.as_ref().map(|l| l[v as usize]).unwrap_or(v);
    for p in px.iter_mut() {
        let [r, g, b, a] = *p;
        *p = match channel {
            Channel::Rgb => {
                if lut.is_none() {
                    continue;
                }
                // Exposure on straight colour, re-premultiplied.
                let un = |c: u8| if a == 0 { 0 } else { ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8 };
                let pm = |c: u8| ((ex(un(c)) as u32 * a as u32 + 127) / 255) as u8;
                [pm(r), pm(g), pm(b), a]
            }
            Channel::Red | Channel::Green | Channel::Blue => {
                let (i, c) = match channel {
                    Channel::Red => (0, r),
                    Channel::Green => (1, g),
                    _ => (2, b),
                };
                let v = ex(c);
                if colorized {
                    let mut o = [0, 0, 0, 255];
                    o[i] = v;
                    o
                } else {
                    [v, v, v, 255]
                }
            }
            Channel::Alpha => {
                let v = ex(a);
                [v, v, v, 255]
            }
            Channel::RgbStraight => {
                let un = |c: u8| if a == 0 { 0 } else { ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8 };
                [ex(un(r)), ex(un(g)), ex(un(b)), 255]
            }
        };
    }
}

// ---------------------------------------------------------------- display colour management

/// View ▸ Simulate Output: the output device a preview imitates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SimProfile {
    #[default]
    None,
    /// HDTV (Rec. 709): ITU-R BT.709 primaries, BT.1886 gamma 2.4.
    Rec709,
    /// UHDTV (Rec. 2020): ITU-R BT.2020 primaries, gamma 2.4.
    Rec2020,
    /// Display P3: SMPTE EG 432-1 primaries, D65, the sRGB curve.
    P3,
    /// Internet Standard RGB (sRGB, IEC 61966-2-1).
    Srgb,
    /// Linear light with Rec. 709 primaries (no transfer curve).
    Linear,
    /// SDTV NTSC: SMPTE 170M primaries, gamma 2.4.
    Ntsc,
    /// SDTV PAL: EBU Tech 3213 primaries, gamma 2.4.
    Pal,
    /// Legacy Macintosh RGB: Rec. 709 primaries, gamma 1.8.
    Mac18,
    /// My Custom RGB: the user's own device ([`CustomRgb`], in Settings).
    MyCustom,
}

/// A transfer curve of a simulated or display profile.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Curve {
    Srgb,
    Gamma(f32),
    Linear,
    /// A colour space's own curve (PQ, HLG…).
    Space(space::Curve),
}

impl Curve {
    fn decode(self, v: f32) -> f32 {
        let a = v.abs();
        let l = match self {
            Curve::Srgb => srgb_to_linear(a),
            Curve::Gamma(g) => a.powf(g),
            Curve::Linear => a,
            Curve::Space(c) => c.decode(a),
        };
        l.copysign(v)
    }
    fn encode(self, v: f32) -> f32 {
        let a = v.abs();
        let e = match self {
            Curve::Srgb => linear_to_srgb(a),
            Curve::Gamma(g) => a.powf(1.0 / g),
            Curve::Linear => a,
            Curve::Space(c) => c.encode(a),
        };
        e.copysign(v)
    }
}

const P709: [[f64; 2]; 3] = [[0.640, 0.330], [0.300, 0.600], [0.150, 0.060]];

impl SimProfile {
    pub const ALL: [SimProfile; 10] = [
        SimProfile::None,
        SimProfile::Rec709,
        SimProfile::Ntsc,
        SimProfile::Pal,
        SimProfile::Mac18,
        SimProfile::Srgb,
        SimProfile::Rec2020,
        SimProfile::P3,
        SimProfile::Linear,
        SimProfile::MyCustom,
    ];

    pub fn id(self) -> &'static str {
        match self {
            SimProfile::None => "none",
            SimProfile::Rec709 => "rec709",
            SimProfile::Rec2020 => "rec2020",
            SimProfile::P3 => "p3",
            SimProfile::Srgb => "srgb",
            SimProfile::Linear => "linear",
            SimProfile::Ntsc => "ntsc",
            SimProfile::Pal => "pal",
            SimProfile::Mac18 => "mac18",
            SimProfile::MyCustom => "myCustom",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SimProfile::None => "No Output Simulation",
            SimProfile::Rec709 => "HDTV (Rec. 709)",
            SimProfile::Rec2020 => "UHDTV (Rec. 2020)",
            SimProfile::P3 => "Display P3",
            SimProfile::Srgb => "Internet Standard RGB (sRGB)",
            SimProfile::Linear => "Linear (1.0 Gamma)",
            SimProfile::Ntsc => "SDTV NTSC",
            SimProfile::Pal => "SDTV PAL",
            SimProfile::Mac18 => "Legacy Macintosh RGB (Gamma 1.8)",
            SimProfile::MyCustom => "My Custom RGB",
        }
    }

    pub fn parse(s: &str) -> Option<SimProfile> {
        let k: String = s.chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_ascii_lowercase();
        SimProfile::ALL
            .into_iter()
            .find(|p| p.id().to_ascii_lowercase() == k || p.label().chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_ascii_lowercase() == k)
            .or(match k.as_str() {
                "off" => Some(SimProfile::None),
                "hdtv" | "bt709" => Some(SimProfile::Rec709),
                "bt2020" | "uhdtv" => Some(SimProfile::Rec2020),
                "displayp3" => Some(SimProfile::P3),
                "lin" | "linearlight" => Some(SimProfile::Linear),
                _ => None,
            })
    }

    /// (primaries, curve); `None` for No Output Simulation and My Custom RGB (see [`CustomRgb`]).
    pub fn space(self) -> Option<([[f64; 2]; 3], Curve)> {
        Some(match self {
            SimProfile::None | SimProfile::MyCustom => return None,
            SimProfile::Rec709 => (P709, Curve::Gamma(2.4)),
            SimProfile::Rec2020 => (ColorSpace::Rec2020.primaries(), Curve::Gamma(2.4)),
            SimProfile::P3 => (ColorSpace::DisplayP3.primaries(), Curve::Srgb),
            SimProfile::Srgb => (P709, Curve::Srgb),
            SimProfile::Linear => (P709, Curve::Linear),
            SimProfile::Ntsc => ([[0.630, 0.340], [0.310, 0.595], [0.155, 0.070]], Curve::Gamma(2.4)),
            SimProfile::Pal => ([[0.640, 0.330], [0.290, 0.600], [0.150, 0.060]], Curve::Gamma(2.4)),
            SimProfile::Mac18 => (P709, Curve::Gamma(1.8)),
        })
    }
}

/// View ▸ Simulate Output setting: the profile, and Preserve RGB (the output numbers are sent
/// to the display unconverted, as on a device that doesn't colour-manage).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct Simulation {
    pub profile: SimProfile,
    pub preserve_rgb: bool,
}

/// View ▸ Simulate Output ▸ My Custom RGB…: a user-defined output device, kept in Settings
/// (`customRgb`). Defined by primaries and white point (CIE xy) and a gamma or the sRGB curve,
/// typed in or read from an RGB ICC profile.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CustomRgb {
    pub name: String,
    pub red: [f64; 2],
    pub green: [f64; 2],
    pub blue: [f64; 2],
    pub white: [f64; 2],
    pub gamma: f64,
    /// The sRGB piecewise curve instead of a pure power curve.
    pub srgb_curve: bool,
    /// The ICC profile the numbers were read from (empty: typed in).
    pub icc: String,
    /// A LUT-based (`A2B0`) profile's bytes: the simulation runs the profile's tables (baked
    /// into a 3D LUT) instead of the numbers above, which then only approximate it.
    #[serde(skip_serializing_if = "IccLut::is_empty")]
    pub icc_lut: IccLut,
}

/// The bytes of a LUT-based ICC profile, kept in Settings as base64.
#[derive(Clone, Default, PartialEq)]
pub struct IccLut(pub Option<Arc<Vec<u8>>>);

impl IccLut {
    pub fn is_empty(&self) -> bool {
        self.0.is_none()
    }

    /// The parsed profile.
    pub fn profile(&self) -> Option<effectcraft_color::icc::LutProfile> {
        match effectcraft_color::icc::parse_profile(self.0.as_deref()?) {
            Ok(effectcraft_color::icc::Profile::Lut(p)) => Some(p),
            _ => None,
        }
    }

    fn hash(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.0.as_deref().hash(&mut h);
        h.finish()
    }
}

impl std::fmt::Debug for IccLut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            Some(b) => write!(f, "IccLut({} bytes, {:016x})", b.len(), self.hash()),
            None => write!(f, "IccLut(none)"),
        }
    }
}

impl Serialize for IccLut {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.0.as_deref().map(|b| effectcraft_track::roto::rle::base64_encode(b)).unwrap_or_default())
    }
}

impl<'de> Deserialize<'de> for IccLut {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(IccLut(effectcraft_track::roto::rle::base64_decode(&s).filter(|b| !b.is_empty()).map(Arc::new)))
    }
}

impl Default for CustomRgb {
    fn default() -> Self {
        CustomRgb {
            name: "My Custom RGB".into(),
            red: P709[0],
            green: P709[1],
            blue: P709[2],
            white: D65,
            gamma: 2.2,
            srgb_curve: false,
            icc: String::new(),
            icc_lut: IccLut::default(),
        }
    }
}

impl CustomRgb {
    pub fn primaries(&self) -> [[f64; 2]; 3] {
        [self.red, self.green, self.blue]
    }

    pub fn curve(&self) -> Curve {
        if self.srgb_curve {
            Curve::Srgb
        } else if (self.gamma - 1.0).abs() < 1e-9 {
            Curve::Linear
        } else {
            Curve::Gamma(self.gamma as f32)
        }
    }

    /// Chromaticities inside (0, 1) with y > 0, a gamma in 0.1–10, and primaries that span a
    /// triangle.
    pub fn validate(&self) -> std::result::Result<(), String> {
        for (n, c) in [("red", self.red), ("green", self.green), ("blue", self.blue), ("white", self.white)] {
            if !(c[0] > 0.0 && c[1] > 0.0 && c[0] < 1.0 && c[1] < 1.0 && c[0] + c[1] <= 1.0) {
                return Err(format!("{n}: chromaticity x, y must be in (0, 1) with x + y ≤ 1"));
            }
        }
        if !self.srgb_curve && !(0.1..=10.0).contains(&self.gamma) {
            return Err("gamma must be between 0.1 and 10".into());
        }
        let [r, g, b] = self.primaries();
        let area = (g[0] - r[0]) * (b[1] - r[1]) - (b[0] - r[0]) * (g[1] - r[1]);
        if area.abs() < 1e-6 {
            return Err("the primaries must form a triangle".into());
        }
        Ok(())
    }

    /// Take primaries, white and curve from an RGB ICC profile. A LUT-based profile is kept
    /// whole ([`CustomRgb::icc_lut`]); its numbers are the primaries' and white's colours
    /// (adapted to D65) and the gamma that fits its neutral ramp.
    pub fn from_icc(bytes: &[u8], path: &str) -> std::result::Result<CustomRgb, String> {
        use effectcraft_color::icc::{Profile, parse_profile};
        let stem = || std::path::Path::new(path).file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "ICC Profile".into());
        let name = |d: &str| if d.trim().is_empty() { stem() } else { d.trim().to_string() };
        let r = |c: [f64; 2]| c.map(|v| (v * 1e5).round() / 1e5);
        match parse_profile(bytes)? {
            Profile::Matrix(p) => {
                let gamma = (p.trc.iter().map(|t| t.fit_gamma()).sum::<f64>() / 3.0 * 1000.0).round() / 1000.0;
                Ok(CustomRgb {
                    name: name(&p.description),
                    red: r(p.primaries[0]),
                    green: r(p.primaries[1]),
                    blue: r(p.primaries[2]),
                    white: r(p.white),
                    gamma,
                    srgb_curve: p.trc[0].is_srgb(),
                    icc: path.to_string(),
                    icc_lut: IccLut::default(),
                })
            }
            Profile::Lut(p) => {
                let (prims, _, gamma) = p.approximate();
                // The PCS is D50-relative: the device white shows as the viewer's D65.
                let a = space::bradford(effectcraft_color::icc::D50, D65);
                let adapt = |c: [f64; 2]| {
                    let v = space::mul_vec(&a, [c[0] / c[1], 1.0, (1.0 - c[0] - c[1]) / c[1]]);
                    let sum = v[0] + v[1] + v[2];
                    let c = [(v[0] / sum).clamp(1e-4, 0.9998), (v[1] / sum).clamp(1e-4, 0.9998)];
                    // Table rounding can put a primary a hair outside the spectrum locus.
                    let k = (0.9999 / (c[0] + c[1])).min(1.0);
                    [c[0] * k, c[1] * k]
                };
                Ok(CustomRgb {
                    name: name(&p.description),
                    red: r(adapt(prims[0])),
                    green: r(adapt(prims[1])),
                    blue: r(adapt(prims[2])),
                    white: D65,
                    gamma: (gamma * 1000.0).round() / 1000.0,
                    srgb_curve: false,
                    icc: path.to_string(),
                    icc_lut: IccLut(Some(Arc::new(bytes.to_vec()))),
                })
            }
        }
    }

    fn prof(&self) -> Prof {
        let m = space::rgb_to_xyz(self.primaries(), self.white);
        let xyz = if self.white == D65 { m } else { space::mul(&space::bradford(self.white, D65), &m) };
        Prof { xyz, curve: self.curve() }
    }
}

/// An RGB space for viewer conversions: primaries (D65) and curve.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Prof {
    /// Linear RGB → XYZ D65.
    xyz: [[f64; 3]; 3],
    curve: Curve,
}

impl Prof {
    fn new(prims: [[f64; 2]; 3], curve: Curve) -> Prof {
        Prof { xyz: space::rgb_to_xyz(prims, D65), curve }
    }
    /// A colour space (linear when `linear`).
    fn of(cs: ColorSpace, linear: bool) -> Prof {
        let curve = if linear { Curve::Linear } else { Curve::Space(cs.curve()) };
        Prof { xyz: cs.to_xyz(), curve }
    }
}

/// A linear-RGB matrix plus curves between two profiles.
#[derive(Clone, Copy, Debug)]
struct Conv {
    from: Curve,
    m: Option<[[f32; 3]; 3]>,
    to: Curve,
}

impl Conv {
    fn new(a: Prof, b: Prof) -> Conv {
        let m = (a.xyz != b.xyz).then(|| {
            let m = space::mul(&space::invert(&b.xyz), &a.xyz);
            m.map(|r| r.map(|v| v as f32))
        });
        Conv { from: a.curve, m, to: b.curve }
    }
    fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        let l = c.map(|v| self.from.decode(v));
        let l = match &self.m {
            Some(m) => [0, 1, 2].map(|i| m[i][0] * l[0] + m[i][1] * l[1] + m[i][2] * l[2]),
            None => l,
        };
        l.map(|v| self.to.encode(v))
    }
}

const D65: [f64; 2] = [0.3127, 0.3290];

/// The viewer's colour conversion from the rendered frame (sRGB-encoded display pixels) to
/// what the monitor shows (View ▸ Use Display Color Management, View ▸ Simulate Output and
/// the display profile in Settings ▸ Previews). `None` when nothing changes.
#[derive(Clone, Debug)]
pub struct DisplayColor {
    sim: Option<(Conv, Option<Conv>)>,
    display: Option<Conv>,
    /// A LUT-based My Custom RGB profile: the whole simulation baked into a 3D LUT.
    lut: Option<Arc<effectcraft_color::icc::Lut3d>>,
}

impl DisplayColor {
    /// `working`: the project working space (None = unmanaged: nothing is converted, as in
    /// After Effects); `linear`: the working space is linear; `source`: the space rendered
    /// frames are in (the project's output space, sRGB by default); `dcm`: Use Display Color
    /// Management; `display`: the monitor's space.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        working: Option<ColorSpace>,
        linear: bool,
        source: ColorSpace,
        dcm: bool,
        display: ColorSpace,
        sim: Simulation,
        custom: &CustomRgb,
    ) -> Option<DisplayColor> {
        let ws = working?;
        let src = Prof::of(source, false);
        if !dcm {
            // Without display colour management the working space's numbers go to the screen.
            let raw = Conv::new(src, Prof::of(ws, linear));
            return Some(DisplayColor { sim: None, display: Some(raw), lut: None });
        }
        if sim.profile == SimProfile::MyCustom
            && let Some(lut) = lut_simulation(custom, src, display, sim.preserve_rgb)
        {
            return Some(DisplayColor { sim: None, display: None, lut: Some(lut) });
        }
        let out = match sim.profile {
            SimProfile::MyCustom => Some(custom.prof()),
            p => p.space().map(|(prims, curve)| Prof::new(prims, curve)),
        };
        let sim = out.map(|out| {
            (
                Conv::new(src, out),
                Some(if sim.preserve_rgb {
                    Conv::new(Prof::new(P709, Curve::Srgb), Prof::of(display, false))
                } else {
                    Conv::new(out, Prof::of(display, false))
                }),
            )
        });
        let display = (display != source).then(|| Conv::new(src, Prof::of(display, false)));
        (sim.is_some() || display.is_some()).then_some(DisplayColor { sim, display, lut: None })
    }

    /// The conversion a session's viewer uses.
    pub fn of(s: &crate::Session) -> Option<DisplayColor> {
        let st = &s.project.settings;
        let display = ColorSpace::parse(&s.prefs.previews.display_profile).unwrap_or(ColorSpace::Srgb);
        let v = &s.state.viewer;
        let linear = st.working_space.is_some_and(|w| st.linearize || w.is_linear());
        DisplayColor::new(
            st.working_space,
            linear,
            st.output_space.unwrap_or(ColorSpace::Srgb),
            v.display_color_management,
            display,
            v.simulation,
            &s.prefs.custom_rgb,
        )
    }

    /// Whether the simulation runs a LUT-based ICC profile (baked into a 3D LUT).
    pub fn is_lut(&self) -> bool {
        self.lut.is_some()
    }

    /// One straight colour (0..1 floats).
    pub fn apply_straight(&self, c: [f32; 3]) -> [f32; 3] {
        if let Some(l) = &self.lut {
            return l.eval(c.map(|v| v.clamp(0.0, 1.0))).map(|v| v.clamp(0.0, 1.0));
        }
        let c = match &self.sim {
            // Through the output's 8-bit encoding (clipped and quantised like the real output),
            // then shown on the display: converted from the output profile, or — Preserve RGB —
            // the output numbers taken as if they were sRGB.
            Some((to, show)) => {
                let o = to.apply(c).map(|v| (v.clamp(0.0, 1.0) * 255.0).round() / 255.0);
                show.map_or(o, |b| b.apply(o))
            }
            None => self.display.map_or(c, |d| d.apply(c)),
        };
        c.map(|v| v.clamp(0.0, 1.0))
    }

    /// Premultiplied 8-bit RGBA pixels in place.
    pub fn apply(&self, px: &mut [[u8; 4]]) {
        use rayon::prelude::*;
        px.par_iter_mut().for_each(|p| {
            let a = p[3];
            if a == 0 {
                return;
            }
            let af = a as f32 / 255.0;
            let c = [0, 1, 2].map(|i| (p[i] as f32 / 255.0 / af).min(1.0));
            let o = self.apply_straight(c);
            for i in 0..3 {
                p[i] = (o[i] * af * 255.0 + 0.5) as u8;
            }
        });
    }
}

/// Grid points per axis of a baked LUT simulation (a profile without `B2A0` is inverted
/// numerically per point, so its grid is coarser).
const SIM_LUT_SIZE: usize = 33;
const SIM_LUT_SIZE_INVERTED: usize = 17;

/// My Custom RGB with a LUT-based profile: rendered colours (`src`) → the device's numbers
/// (`B2A0`, or `A2B0` inverted), clipped and quantised to 8 bits like the real output → what the
/// device shows (`A2B0`; Preserve RGB: the numbers taken as sRGB) → the display. Baked once per
/// profile and conversion (the last one is cached).
fn lut_simulation(custom: &CustomRgb, src: Prof, display: ColorSpace, preserve: bool) -> Option<Arc<effectcraft_color::icc::Lut3d>> {
    type Cache = Option<(u64, Arc<effectcraft_color::icc::Lut3d>)>;
    static CACHE: std::sync::Mutex<Cache> = std::sync::Mutex::new(None);
    custom.icc_lut.0.as_ref()?;
    let key = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (custom.icc_lut.hash(), format!("{src:?}{display:?}{preserve}")).hash(&mut h);
        h.finish()
    };
    if let Some((k, l)) = CACHE.lock().ok()?.as_ref()
        && *k == key
    {
        return Some(l.clone());
    }
    let prof = custom.icc_lut.profile()?;
    let to_d50 = space::mul(&space::bradford(D65, effectcraft_color::icc::D50), &src.xyz);
    let from_d50 = space::bradford(effectcraft_color::icc::D50, D65);
    let disp = Prof::of(display, false);
    let disp_inv = space::invert(&disp.xyz);
    let srgb_to_display = Conv::new(Prof::new(P709, Curve::Srgb), disp);
    let size = if prof.b2a.is_some() { SIM_LUT_SIZE } else { SIM_LUT_SIZE_INVERTED };
    let lut = effectcraft_color::icc::Lut3d::bake(size, |c| {
        let lin = c.map(|v| src.curve.decode(v as f32) as f64);
        let dev = prof.from_xyz(space::mul_vec(&to_d50, lin), c).map(|v| (v.clamp(0.0, 1.0) * 255.0).round() / 255.0);
        if preserve {
            return srgb_to_display.apply(dev.map(|v| v as f32)).map(|v| v as f64);
        }
        let xyz = space::mul_vec(&from_d50, prof.to_xyz(dev));
        space::mul_vec(&disp_inv, xyz).map(|v| disp.curve.encode(v as f32) as f64)
    });
    let lut = Arc::new(lut);
    if let Ok(mut c) = CACHE.lock() {
        *c = Some((key, lut.clone()));
    }
    Some(lut)
}

// ---------------------------------------------------------------- snapshot

/// Take Snapshot (Shift+F5): a rendered frame kept for comparison (Show Snapshot, F5).
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub comp: effectcraft_project::ItemId,
    pub time: Tick,
    /// Render scale the image was taken at.
    pub scale: f64,
    pub image: Arc<Image>,
}

#[cfg(test)]
mod snap_expression_tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct PositionHost(AtomicUsize);
    impl ExprHost for PositionHost {
        fn eval(&self, ctx: &EvalCtx, _: &Layer, _: &Property, value: &Value) -> Result<Value, String> {
            self.0.fetch_add(1, Ordering::Relaxed);
            let v = value.components();
            Ok(Value::Vec3([v[0] + 10. * ctx.time.seconds(), v[1], 0.]))
        }
    }

    #[test]
    fn snapping_reuses_expressions_within_one_query_but_refreshes_time_and_edits() {
        let mut s = crate::Session::default();
        s.execute("comp.new", json!({"width":320,"height":180,"duration":5})).unwrap();
        let lid = s.execute("layer.newSolid", json!({"width":40,"height":20})).unwrap()["layer"].as_u64().unwrap();
        s.execute("prop.setExpression", json!({"layer":lid,"path":"transform/position","expression":"value"})).unwrap();
        let cid = s.active_comp_id().unwrap();
        let host = PositionHost(AtomicUsize::new(0));
        let opts = SnapOptions { layers: true, ..Default::default() };
        let query = |s: &crate::Session, time, cached| {
            let ctx = EvalCtx { expr: Some(&host), ..EvalCtx::new(&s.project, cid, s.project.comp(cid).unwrap(), time) };
            if cached { targets(&ctx, &[], opts) } else { targets_in(&ctx, &[], opts) }
        };
        let expected = query(&s, Tick::ZERO, false);
        assert!(host.0.swap(0, Ordering::Relaxed) > 1, "fixture must visit the same expression for multiple geometry features");
        assert_eq!(query(&s, Tick::ZERO, true), expected);
        assert_eq!(host.0.swap(0, Ordering::Relaxed), 1);
        let moved = query(&s, Tick::from_seconds_f64(1.), true);
        assert_ne!(moved, expected, "a new time must not reuse stale targets");
        assert_eq!(host.0.swap(0, Ordering::Relaxed), 1);
        s.execute("prop.set", json!({"layer":lid,"path":"transform/position","value":[200,100,0]})).unwrap();
        let edited = query(&s, Tick::ZERO, true);
        assert_ne!(edited, expected, "a new project revision must not reuse stale targets");
        assert_eq!(host.0.swap(0, Ordering::Relaxed), 1);
    }
}
