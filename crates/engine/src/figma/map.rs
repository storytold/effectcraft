//! Figma node JSON (the REST API's `GET /v1/files/:key/nodes?geometry=paths` format, which the
//! EffectCraft Figma plug-in also produces) → an import [`Plan`]: compositions and layers with
//! native shape contents, text documents, layer styles and track mattes, plus the nodes that must
//! come in as rendered images. Pure: no I/O, no session; [`super::build`] turns a plan into
//! project items in one undo step.
//!
//! Coordinates: every node's world (canvas) transform is worked out from `absoluteTransform`
//! (plug-in) or from `relativeTransform` + `absoluteBoundingBox` (REST); a layer's transform is
//! that matrix mapped into its composition's space and decomposed into position, rotation and
//! scale, with the anchor at the node's centre so animating it pivots like the Figma object.
//! Shape contents stay in the node's own coordinates (0,0 = its top-left corner).

use effectcraft_color::BlendMode;
use effectcraft_keyframe::text_doc::Justify;
use effectcraft_keyframe::{Gradient, ShapePath, TextDoc};
use effectcraft_project::MatteKind;
use kurbo::{Affine, Point, Rect};
use serde_json::{Value as J, json};

/// Deeper Figma trees are cut off (with a note) rather than recursed into.
const MAX_DEPTH: usize = 64;
/// Upper bound on nodes in one import (hostile or accidental huge selections).
const MAX_NODES: usize = 20_000;

/// Where the import goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// One new composition per selected frame (frame size → comp size), or one for the whole
    /// selection when it isn't only frames.
    NewComp,
    /// Into the active composition, centred.
    ActiveComp { width: u32, height: u32 },
}

#[derive(Clone, Debug)]
pub struct Options {
    pub target: Target,
}

/// A planned composition. `layers` are in Figma paint order (bottom first).
#[derive(Clone, Debug, Default)]
pub struct PComp {
    pub name: String,
    pub figma_id: String,
    pub width: u32,
    pub height: u32,
    pub layers: Vec<PLayer>,
}

#[derive(Clone, Debug)]
pub enum PKind {
    /// Shape layer contents.
    Shape(Vec<Item>),
    Text(Box<TextDoc>),
    /// A precomp layer showing `comps[i]`.
    Precomp(usize),
    /// A null standing in for a Figma group (its children are parented to it).
    Null,
    /// A rendered image (`rasters[i]`).
    Raster(usize),
}

#[derive(Clone, Debug)]
pub struct Shadow {
    pub inner: bool,
    pub blend: BlendMode,
    pub show_behind: bool,
    pub color: [f64; 3],
    /// 0–100.
    pub opacity: f64,
    /// Light angle in degrees (Photoshop convention: the shadow falls away from it).
    pub angle: f64,
    pub distance: f64,
    pub size: f64,
    /// 0–100 (Spread / Choke).
    pub spread: f64,
}

#[derive(Clone, Debug)]
pub struct PLayer {
    pub name: String,
    pub figma_id: String,
    pub kind: PKind,
    /// Layer-space anchor, and position / rotation (degrees) / scale (percent) in comp space.
    /// Positions of parented layers are still comp-space here; the builder converts them.
    pub anchor: [f64; 2],
    pub position: [f64; 2],
    pub rotation: f64,
    pub scale: [f64; 2],
    /// 0–100.
    pub opacity: f64,
    pub blend: BlendMode,
    pub visible: bool,
    /// Index of the parent layer (a group null) in the same composition.
    pub parent: Option<usize>,
    pub matte: Option<(usize, MatteKind)>,
    /// Used as a track matte (its video switch is off, as After Effects does).
    pub is_matte: bool,
    pub shadows: Vec<Shadow>,
    /// Gaussian Blur blurriness (Figma layer blur).
    pub blur: Option<f64>,
    /// Layer masks in layer space (a clipping frame's rounded corners).
    pub masks: Vec<ShapePath>,
    /// Original bitmap pixels already placed by their paint transform.
    pub original_image: bool,
    /// Pass-through frames draw children directly in the containing composition.
    pub collapse: bool,
}

impl PLayer {
    fn new(name: &str, id: &str, kind: PKind) -> PLayer {
        PLayer {
            name: if name.trim().is_empty() { "Layer".into() } else { name.to_string() },
            figma_id: id.to_string(),
            kind,
            anchor: [0.0; 2],
            position: [0.0; 2],
            rotation: 0.0,
            scale: [100.0; 2],
            opacity: 100.0,
            blend: BlendMode::Normal,
            visible: true,
            parent: None,
            matte: None,
            is_matte: false,
            shadows: vec![],
            blur: None,
            masks: vec![],
            original_image: false,
            collapse: false,
        }
    }
}

/// Shape layer contents (top of the list paints on top, as in the Timeline).
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Group {
        name: String,
        items: Vec<Item>,
    },
    Rect {
        size: [f64; 2],
        position: [f64; 2],
        roundness: f64,
    },
    Ellipse {
        size: [f64; 2],
        position: [f64; 2],
    },
    Star {
        star: bool,
        points: f64,
        position: [f64; 2],
        outer: f64,
        inner: f64,
    },
    Path(ShapePath),
    Fill {
        color: [f64; 3],
        opacity: f64,
        even_odd: bool,
    },
    GradientFill {
        radial: bool,
        kind: u32,
        width: Option<[f64; 2]>,
        start: [f64; 2],
        end: [f64; 2],
        gradient: Gradient,
        opacity: f64,
        even_odd: bool,
    },
    Stroke(Stroke),
    /// Offset Paths by `amount` (inside / outside stroke alignment of free-form paths).
    Offset(f64),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stroke {
    /// Solid colour, or a gradient `(radial, start, end, gradient)`.
    pub color: [f64; 3],
    pub gradient: Option<GradientPaint>,
    pub opacity: f64,
    pub width: f64,
    /// 0 butt, 1 round, 2 projecting.
    pub cap: u32,
    /// 0 miter, 1 round, 2 bevel.
    pub join: u32,
    pub miter: f64,
    pub dash: Option<(f64, f64)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GradientPaint {
    pub kind: u32,
    pub start: [f64; 2],
    pub end: [f64; 2],
    pub width: Option<[f64; 2]>,
    pub gradient: Gradient,
}

/// A node that comes in as a rendered PNG.
#[derive(Clone, Debug)]
pub struct Raster {
    pub node_id: String,
    pub name: String,
    /// The node's bounding box size: the image is scaled to it.
    pub width: f64,
    pub height: f64,
    pub reason: String,
}

#[derive(Clone, Debug, Default)]
pub struct Plan {
    pub comps: Vec<PComp>,
    /// New compositions (open the first).
    pub roots: Vec<usize>,
    /// The planned composition whose layers go into the active comp (`Target::ActiveComp`).
    pub into_active: Option<usize>,
    pub rasters: Vec<Raster>,
    pub notes: Vec<String>,
    /// Display name for the import (the first selected node).
    pub name: String,
}

impl Plan {
    /// Layers made natively (shape, text, precomp, group null) and as images.
    pub fn counts(&self) -> (usize, usize) {
        let all = self.comps.iter().flat_map(|c| c.layers.iter());
        let raster = all.clone().filter(|l| matches!(l.kind, PKind::Raster(_))).count();
        (all.count() - raster, raster)
    }

    /// A JSON summary (for `dryRun`).
    pub fn summary(&self) -> J {
        let comps: Vec<J> = self
            .comps
            .iter()
            .map(|c| {
                let layers: Vec<J> = c
                    .layers
                    .iter()
                    .rev()
                    .map(|l| {
                        let kind = match &l.kind {
                            PKind::Shape(_) => "shape".to_string(),
                            PKind::Text(_) => "text".into(),
                            PKind::Precomp(i) => format!("precomp:{}", self.comps.get(*i).map(|c| c.name.as_str()).unwrap_or("?")),
                            PKind::Null => "null".into(),
                            PKind::Raster(_) => "image".into(),
                        };
                        json!({"name": l.name, "figmaId": l.figma_id, "kind": kind, "matte": l.matte.is_some(), "isMatte": l.is_matte,
                               "styles": l.shadows.len(), "blur": l.blur})
                    })
                    .collect();
                json!({"name": c.name, "width": c.width, "height": c.height, "layers": layers})
            })
            .collect();
        let (native, raster) = self.counts();
        json!({
            "comps": comps,
            "native": native,
            "rasterized": self.rasters.iter().map(|r| json!({"node": r.node_id, "name": r.name, "reason": r.reason})).collect::<Vec<_>>(),
            "notes": self.notes,
            "images": raster,
        })
    }
}

// ---------------------------------------------------------------- input selection

/// The nodes to import from any accepted payload shape:
/// - the REST `GET /v1/files/:key/nodes` reply (`{"nodes": {"1:2": {"document": node}}}`),
/// - a REST file (`{"document": DOCUMENT}`: the first page's top-level nodes, or `ids`),
/// - the plug-in payload (`{"nodes": [node, …]}`), an array of nodes or a single node.
///
/// `ids` (when given) picks those nodes, in that order, anywhere in the tree.
pub fn select_nodes(payload: &J, ids: &[String]) -> Result<Vec<J>, String> {
    let mut found: Vec<J> = match payload {
        J::Array(a) => a.clone(),
        J::Object(o) => {
            if let Some(nodes) = o.get("nodes") {
                match nodes {
                    J::Array(a) => a.clone(),
                    J::Object(m) => {
                        let mut v = vec![];
                        for (id, entry) in m {
                            match entry.get("document") {
                                Some(d) => v.push(d.clone()),
                                None if entry.is_null() => return Err(format!("Figma returned no node {id} (deleted, or no access)")),
                                None => {}
                            }
                        }
                        // REST maps are keyed by id; keep the requested order.
                        if !ids.is_empty() {
                            v.sort_by_key(|n| ids.iter().position(|i| Some(i.as_str()) == n.get("id").and_then(J::as_str)).unwrap_or(usize::MAX));
                        }
                        v
                    }
                    _ => return Err("`nodes` must be an array or an id → node map".into()),
                }
            } else if let Some(doc) = o.get("document") {
                vec![doc.clone()]
            } else if o.contains_key("type") {
                vec![payload.clone()]
            } else {
                return Err("no Figma nodes in the payload (expected `nodes`, `document` or a node object)".into());
            }
        }
        _ => return Err("the payload must be a JSON object or array".into()),
    };
    if !ids.is_empty() {
        let mut picked = vec![];
        for id in ids {
            if let Some(n) = found.iter().find_map(|n| find_id(n, id, 0)) {
                picked.push(n.clone());
            } else {
                return Err(format!("node {id} is not in the payload"));
            }
        }
        found = picked;
    }
    // Documents and pages: their top-level nodes.
    let mut out = vec![];
    for n in found {
        match ty(&n) {
            "DOCUMENT" => {
                if let Some(page) = n.get("children").and_then(J::as_array).and_then(|c| c.first()) {
                    out.extend(page.get("children").and_then(J::as_array).cloned().unwrap_or_default());
                }
            }
            "CANVAS" => out.extend(n.get("children").and_then(J::as_array).cloned().unwrap_or_default()),
            _ => out.push(n),
        }
    }
    if out.is_empty() {
        return Err("nothing to import (the selection is empty)".into());
    }
    Ok(out)
}

fn find_id<'a>(n: &'a J, id: &str, depth: usize) -> Option<&'a J> {
    if n.get("id").and_then(J::as_str) == Some(id) {
        return Some(n);
    }
    if depth > MAX_DEPTH {
        return None;
    }
    n.get("children").and_then(J::as_array)?.iter().find_map(|c| find_id(c, id, depth + 1))
}

// ---------------------------------------------------------------- small readers

fn ty(n: &J) -> &str {
    n.get("type").and_then(J::as_str).unwrap_or("")
}
fn name(n: &J) -> String {
    n.get("name").and_then(J::as_str).unwrap_or("").to_string()
}
fn id(n: &J) -> String {
    n.get("id").and_then(J::as_str).unwrap_or("").to_string()
}
fn num(n: &J, k: &str) -> Option<f64> {
    n.get(k).and_then(J::as_f64).filter(|x| x.is_finite())
}
fn visible(n: &J) -> bool {
    n.get("visible").and_then(J::as_bool).unwrap_or(true)
}
fn children(n: &J) -> Vec<&J> {
    n.get("children").and_then(J::as_array).map(|a| a.iter().collect()).unwrap_or_default()
}
fn rect_of(v: Option<&J>) -> Option<Rect> {
    let v = v?;
    let (x, y, w, h) = (num(v, "x")?, num(v, "y")?, num(v, "width")?, num(v, "height")?);
    let rect = Rect::new(x, y, x + w.max(0.0), y + h.max(0.0));
    [rect.x1, rect.y1, rect.width(), rect.height()].iter().all(|v| v.is_finite()).then_some(rect)
}
/// A Figma colour `{r, g, b, a}` (0–1).
fn rgba(v: Option<&J>) -> [f64; 4] {
    let g = |k: &str, d: f64| v.and_then(|c| num(c, k)).unwrap_or(d).clamp(0.0, 1.0);
    [g("r", 0.0), g("g", 0.0), g("b", 0.0), g("a", 1.0)]
}
/// A 2×3 matrix `[[a, c, e], [b, d, f]]`.
fn matrix(v: Option<&J>) -> Option<Affine> {
    let rows = v?.as_array()?;
    let r0 = rows.first()?.as_array()?;
    let r1 = rows.get(1)?.as_array()?;
    let g = |r: &Vec<J>, i: usize| r.get(i).and_then(J::as_f64).filter(|x| x.is_finite());
    Some(Affine::new([g(r0, 0)?, g(r1, 0)?, g(r0, 1)?, g(r1, 1)?, g(r0, 2)?, g(r1, 2)?]))
}

fn size_of(n: &J) -> [f64; 2] {
    if let Some(s) = n.get("size")
        && let (Some(x), Some(y)) = (num(s, "x"), num(s, "y"))
    {
        return [x.max(0.0), y.max(0.0)];
    }
    if let (Some(w), Some(h)) = (num(n, "width"), num(n, "height")) {
        return [w.max(0.0), h.max(0.0)];
    }
    rect_of(n.get("absoluteBoundingBox")).map(|r| [r.width(), r.height()]).unwrap_or([0.0; 2])
}

fn linear(m: Affine) -> Affine {
    let c = m.as_coeffs();
    Affine::new([c[0], c[1], c[2], c[3], 0.0, 0.0])
}

fn aabb(m: Affine, r: Rect) -> Rect {
    let pts = [Point::new(r.x0, r.y0), Point::new(r.x1, r.y0), Point::new(r.x0, r.y1), Point::new(r.x1, r.y1)].map(|p| m * p);
    let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for p in pts {
        x0 = x0.min(p.x);
        y0 = y0.min(p.y);
        x1 = x1.max(p.x);
        y1 = y1.max(p.y);
    }
    Rect::new(x0, y0, x1, y1)
}

/// A node's world transform (node space → canvas) given its container's linear part.
fn world(n: &J, parent_lin: Affine) -> Affine {
    if let Some(m) = matrix(n.get("absoluteTransform")) {
        return m;
    }
    let [w, h] = size_of(n);
    let rel = matrix(n.get("relativeTransform"));
    let lin = parent_lin * rel.map(linear).unwrap_or(Affine::IDENTITY);
    match rect_of(n.get("absoluteBoundingBox")) {
        Some(bb) => {
            let b = aabb(lin, Rect::new(0.0, 0.0, w, h));
            lin.then_translate(kurbo::Vec2::new(bb.x0 - b.x0, bb.y0 - b.y0))
        }
        None => {
            let t = rel.map(|r| r.translation()).unwrap_or_default();
            lin.then_translate(t)
        }
    }
}

/// World bounds including effects (`absoluteRenderBounds`), else the bounding box.
fn render_bounds(n: &J, w: Affine) -> Rect {
    rect_of(n.get("absoluteRenderBounds")).or_else(|| rect_of(n.get("absoluteBoundingBox"))).unwrap_or_else(|| {
        let [sw, sh] = size_of(n);
        aabb(w, Rect::new(0.0, 0.0, sw, sh))
    })
}

fn blend(s: Option<&str>) -> BlendMode {
    match s.unwrap_or("NORMAL") {
        "DARKEN" => BlendMode::Darken,
        "MULTIPLY" => BlendMode::Multiply,
        "LINEAR_BURN" => BlendMode::LinearBurn,
        "COLOR_BURN" => BlendMode::ColorBurn,
        "LIGHTEN" => BlendMode::Lighten,
        "SCREEN" => BlendMode::Screen,
        "LINEAR_DODGE" => BlendMode::Add,
        "COLOR_DODGE" => BlendMode::ColorDodge,
        "OVERLAY" => BlendMode::Overlay,
        "SOFT_LIGHT" => BlendMode::SoftLight,
        "HARD_LIGHT" => BlendMode::HardLight,
        "DIFFERENCE" => BlendMode::Difference,
        "EXCLUSION" => BlendMode::Exclusion,
        "HUE" => BlendMode::Hue,
        "SATURATION" => BlendMode::Saturation,
        "COLOR" => BlendMode::Color,
        "LUMINOSITY" => BlendMode::Luminosity,
        _ => BlendMode::Normal,
    }
}

/// Visible paints, bottom first.
fn paints<'a>(n: &'a J, key: &str) -> Vec<&'a J> {
    n.get(key).and_then(J::as_array).map(|a| a.iter().filter(|p| visible(p) && num(p, "opacity").unwrap_or(1.0) > 0.0).collect()).unwrap_or_default()
}

fn has_image_paint(n: &J) -> bool {
    ["fills", "strokes"].iter().any(|k| paints(n, k).iter().any(|p| matches!(ty(p), "IMAGE" | "VIDEO" | "EMOJI" | "PATTERN")))
}

// ---------------------------------------------------------------- the mapper

struct Mapper<'a> {
    plan: &'a mut Plan,
    nodes: usize,
}

/// The coordinate space layers are placed in: comp space from world space.
#[derive(Clone, Copy)]
struct Space {
    from_world: Affine,
}

impl Mapper<'_> {
    fn note(&mut self, s: String) {
        if !self.plan.notes.contains(&s) && self.plan.notes.len() < 500 {
            self.plan.notes.push(s);
        }
    }

    fn new_comp(&mut self, name: &str, fid: &str, w: f64, h: f64) -> usize {
        let clamp = |v: f64| if v.is_finite() { v.ceil().clamp(1.0, 30_000.0) as u32 } else { 1 };
        self.plan.comps.push(PComp { name: name.to_string(), figma_id: fid.to_string(), width: clamp(w), height: clamp(h), layers: vec![] });
        self.plan.comps.len() - 1
    }

    fn push(&mut self, comp: usize, l: PLayer) -> Option<usize> {
        let c = self.plan.comps.get_mut(comp)?;
        c.layers.push(l);
        Some(c.layers.len() - 1)
    }

    /// Set a layer's transform from `m` (node space → comp space) for a node of size `size`.
    fn place(l: &mut PLayer, m: Affine, size: [f64; 2]) -> Option<String> {
        let c = m.as_coeffs();
        let sx = c[0].hypot(c[1]);
        let det = c[0] * c[3] - c[1] * c[2];
        let theta = c[1].atan2(c[0]);
        let sy = if sx > 1e-12 { det / sx } else { c[2].hypot(c[3]) };
        let centre = [size[0] / 2.0, size[1] / 2.0];
        let p = m * Point::new(centre[0], centre[1]);
        l.anchor = centre;
        l.position = [p.x, p.y];
        l.rotation = theta.to_degrees();
        l.scale = [sx * 100.0, sy * 100.0];
        let skew = (c[0] * c[2] + c[1] * c[3]) / (sx * sx.max(1e-12)).max(1e-12);
        (skew.abs() > 1e-3).then(|| format!("{}: skewed; placed without the skew", l.name))
    }

    fn common(&mut self, l: &mut PLayer, n: &J) {
        l.opacity = (num(n, "opacity").unwrap_or(1.0) * 100.0).clamp(0.0, 100.0);
        l.blend = blend(n.get("blendMode").and_then(J::as_str));
        l.visible = visible(n);
        for e in n.get("effects").and_then(J::as_array).into_iter().flatten().filter(|e| visible(e)) {
            match ty(e) {
                t @ ("DROP_SHADOW" | "INNER_SHADOW") => {
                    let c = rgba(e.get("color"));
                    let off = e.get("offset");
                    let (ox, oy) = (off.and_then(|o| num(o, "x")).unwrap_or(0.0), off.and_then(|o| num(o, "y")).unwrap_or(0.0));
                    let size = num(e, "radius").unwrap_or(0.0).max(0.0);
                    let spread = num(e, "spread").unwrap_or(0.0);
                    let spread_pct = if spread > 0.0 { (spread / (spread + size.max(1e-9)) * 100.0).clamp(0.0, 100.0) } else { 0.0 };
                    if spread < 0.0 {
                        self.note(format!("{}: negative shadow spread is not supported (left at 0)", l.name));
                    }
                    let inner = t == "INNER_SHADOW";
                    let mut angle = oy.atan2(-ox).to_degrees();
                    if ox == 0.0 && oy == 0.0 {
                        angle = 90.0;
                    }
                    l.shadows.push(Shadow {
                        inner,
                        blend: blend(e.get("blendMode").and_then(J::as_str)),
                        show_behind: e.get("showBehind").and_then(J::as_bool).unwrap_or(false),
                        color: [c[0], c[1], c[2]],
                        opacity: c[3] * 100.0,
                        angle,
                        distance: ox.hypot(oy),
                        size: size + spread.max(0.0),
                        spread: spread_pct,
                    });
                }
                "LAYER_BLUR" => l.blur = Some(num(e, "radius").unwrap_or(0.0).max(0.0)),
                "BACKGROUND_BLUR" => self.note(format!("{}: background blur is not supported (skipped)", l.name)),
                other => self.note(format!("{}: effect {other} is not supported (skipped)", l.name)),
            }
        }
    }

    fn raster(&mut self, comp: usize, n: &J, w: Affine, space: Space, reason: &str) -> Vec<usize> {
        let bb = rect_of(n.get("absoluteBoundingBox")).unwrap_or_else(|| {
            let [sw, sh] = size_of(n);
            aabb(w, Rect::new(0.0, 0.0, sw, sh))
        });
        let r = Raster { node_id: id(n), name: name(n), width: bb.width().max(1.0), height: bb.height().max(1.0), reason: reason.to_string() };
        self.plan.rasters.push(r);
        let ri = self.plan.rasters.len() - 1;
        let mut l = PLayer::new(&name(n), &id(n), PKind::Raster(ri));
        // The image is the node's world bounding box: an axis-aligned rectangle.
        let m = space.from_world * Affine::translate((bb.x0, bb.y0));
        let _ = Self::place(&mut l, m, [bb.width(), bb.height()]);
        l.blend = blend(n.get("blendMode").and_then(J::as_str));
        l.visible = visible(n);
        self.push(comp, l).into_iter().collect()
    }

    /// Original image fills remain footage, clipped by an editable native geometry matte.
    fn image_paints(&mut self, comp: usize, n: &J, m: Affine) -> Vec<usize> {
        let mut made = vec![];
        for (index, paint) in paints(n, "fills").into_iter().enumerate() {
            let mut geometry = n.clone();
            geometry["strokes"] = json!([]);
            geometry["fills"] = json!([if ty(paint) == "IMAGE" { json!({"type":"SOLID", "color":{"r":1,"g":1,"b":1,"a":1}}) } else { paint.clone() }]);
            let contents = match shape_contents(&geometry) {
                Ok(contents) => contents,
                Err(why) => {
                    self.note(format!("{}: {why}", name(n)));
                    continue;
                }
            };
            let mut shape = PLayer::new(&format!("{} Fill {}", name(n), index + 1), &id(n), PKind::Shape(contents));
            let _ = Self::place(&mut shape, m, size_of(n));
            if ty(paint) != "IMAGE" {
                made.extend(self.push(comp, shape));
                continue;
            }
            let (Some(reference), Some(transform), Some(dimensions)) =
                (paint.get("imageRef").and_then(J::as_str), matrix(paint.get("figcraftImageTransform")), paint.get("figcraftImageSize").and_then(J::as_array))
            else {
                self.note(format!("{}: image paint has no validated asset or transform", name(n)));
                continue;
            };
            let (Some(iw), Some(ih)) = (dimensions.first().and_then(J::as_f64), dimensions.get(1).and_then(J::as_f64)) else { continue };
            let raster = self.plan.rasters.iter().position(|r| r.node_id == reference).unwrap_or_else(|| {
                self.plan.rasters.push(Raster {
                    node_id: reference.into(),
                    name: reference.into(),
                    width: iw,
                    height: ih,
                    reason: "original image fill".into(),
                });
                self.plan.rasters.len() - 1
            });
            shape.name = format!("{} Image Mask {}", name(n), index + 1);
            shape.is_matte = true;
            let Some(mask) = self.push(comp, shape) else { continue };
            let mut image = PLayer::new(&format!("{} Image {}", name(n), index + 1), &id(n), PKind::Raster(raster));
            if let Some(note) = Self::place(&mut image, m * transform, [iw, ih]) {
                self.note(note);
            }
            image.original_image = true;
            image.opacity = num(paint, "opacity").unwrap_or(1.0).clamp(0.0, 1.0) * 100.0;
            image.blend = blend(paint.get("blendMode").and_then(J::as_str));
            image.matte = Some((mask, MatteKind::Alpha));
            made.push(mask);
            made.extend(self.push(comp, image));
        }
        if !paints(n, "strokes").is_empty() {
            if paints(n, "strokes").iter().any(|p| ty(p) == "IMAGE") {
                self.note(format!("{}: image stroke is not supported; image fills remain editable", name(n)));
            }
            let mut strokes = n.clone();
            strokes["fills"] = json!([]);
            if let Ok(contents) = shape_contents(&strokes) {
                let mut layer = PLayer::new(&format!("{} Stroke", name(n)), &id(n), PKind::Shape(contents));
                let _ = Self::place(&mut layer, m, size_of(n));
                made.extend(self.push(comp, layer));
            }
        }
        made
    }

    fn image_node(&mut self, comp: usize, n: &J, w: Affine, space: Space, depth: usize) -> Vec<usize> {
        let [width, height] = size_of(n);
        let bounds = aabb(w.inverse(), render_bounds(n, w)).union(Rect::new(0.0, 0.0, width, height));
        let bounds = Rect::new(bounds.x0.floor(), bounds.y0.floor(), bounds.x1.ceil(), bounds.y1.ceil());
        let inner = self.new_comp(&name(n), &id(n), bounds.width(), bounds.height());
        let shift = Affine::translate((-bounds.x0, -bounds.y0));
        self.image_paints(inner, n, shift);
        self.children(inner, n, linear(w), Space { from_world: shift * w.inverse() }, None, depth + 1);
        let mut layer = PLayer::new(&name(n), &id(n), PKind::Precomp(inner));
        if let Some(note) = Self::place(&mut layer, space.from_world * w, [width, height]) {
            self.note(note);
        }
        layer.anchor = [width / 2.0 - bounds.x0, height / 2.0 - bounds.y0];
        self.common(&mut layer, n);
        self.push(comp, layer).into_iter().collect()
    }

    /// Map `n` (with container linear part `parent_lin`) into `comp`. Returns every layer made
    /// (the node's own and, for flattened groups, its descendants').
    fn node(&mut self, comp: usize, n: &J, parent_lin: Affine, space: Space, parent: Option<usize>, depth: usize) -> Vec<usize> {
        self.nodes += 1;
        if self.nodes > MAX_NODES {
            if self.nodes == MAX_NODES + 1 {
                self.note(format!("more than {MAX_NODES} nodes: the rest were skipped"));
            }
            return vec![];
        }
        if depth > MAX_DEPTH {
            self.note(format!("{}: nested more than {MAX_DEPTH} levels deep (skipped)", name(n)));
            return vec![];
        }
        let w = world(n, parent_lin);
        let t = ty(n);
        let made = match t {
            "SLICE" => vec![],
            _ if has_image_paint(n) && n.get("figcraft").and_then(J::as_bool) == Some(true) => {
                if matches!(t, "FRAME" | "COMPONENT" | "COMPONENT_SET" | "INSTANCE" | "SECTION") {
                    self.frame(comp, n, w, space, depth)
                } else {
                    self.image_node(comp, n, w, space, depth)
                }
            }
            _ if has_image_paint(n) => self.raster(comp, n, w, space, "image fill"),
            "FRAME" | "COMPONENT" | "COMPONENT_SET" | "INSTANCE" | "SECTION" => self.frame(comp, n, w, space, depth),
            "GROUP" => self.group(comp, n, w, parent_lin, space, depth),
            "RECTANGLE" | "ELLIPSE" | "LINE" | "VECTOR" | "STAR" | "REGULAR_POLYGON" | "BOOLEAN_OPERATION" => match shape_contents(n) {
                Ok(items) => {
                    let mut l = PLayer::new(&name(n), &id(n), PKind::Shape(items));
                    if let Some(note) = Self::place(&mut l, space.from_world * w, size_of(n)) {
                        self.note(note);
                    }
                    self.common(&mut l, n);
                    self.push(comp, l).into_iter().collect()
                }
                Err(why) => self.raster(comp, n, w, space, &why),
            },
            "TEXT" => {
                let (doc, notes) = text_doc(n);
                for x in notes {
                    self.note(x);
                }
                let mut l = PLayer::new(&name(n), &id(n), PKind::Text(Box::new(doc)));
                if let Some(note) = Self::place(&mut l, space.from_world * w, size_of(n)) {
                    self.note(note);
                }
                self.common(&mut l, n);
                self.push(comp, l).into_iter().collect()
            }
            other => self.raster(comp, n, w, space, &format!("{} node", if other.is_empty() { "unknown" } else { other })),
        };
        if let Some(p) = parent
            && let Some(c) = self.plan.comps.get_mut(comp)
        {
            for &i in &made {
                if let Some(l) = c.layers.get_mut(i)
                    && l.parent.is_none()
                    && i != p
                {
                    l.parent = Some(p);
                }
            }
        }
        made
    }

    /// Children in paint order, with Figma masks (`isMask`) turned into track mattes for the
    /// siblings above them. `depth` is the child depth; the virtual selection root adds none.
    fn children(&mut self, comp: usize, n: &J, lin: Affine, space: Space, parent: Option<usize>, depth: usize) -> Vec<usize> {
        let mut all = vec![];
        let mut mask: Option<(usize, MatteKind)> = None;
        for c in children(n) {
            let is_mask = c.get("isMask").and_then(J::as_bool).unwrap_or(false) && visible(c);
            let made = self.node(comp, c, lin, space, parent, depth);
            if is_mask {
                // The mask's own layer: the first renderable one made for it.
                let m =
                    made.iter().copied().find(|i| self.plan.comps.get(comp).and_then(|cc| cc.layers.get(*i)).is_some_and(|l| !matches!(l.kind, PKind::Null)));
                if let Some(m) = m {
                    let kind = match c.get("maskType").and_then(J::as_str) {
                        Some("LUMINANCE") => MatteKind::Luma,
                        _ => MatteKind::Alpha,
                    };
                    if let Some(l) = self.plan.comps.get_mut(comp).and_then(|cc| cc.layers.get_mut(m)) {
                        l.is_matte = true;
                    }
                    mask = Some((m, kind));
                }
            } else if let Some((m, kind)) = mask {
                let mut nested = false;
                if let Some(cc) = self.plan.comps.get_mut(comp) {
                    for &i in &made {
                        if let Some(l) = cc.layers.get_mut(i) {
                            if l.matte.is_some() {
                                nested = true;
                            } else if !matches!(l.kind, PKind::Null) {
                                l.matte = Some((m, kind));
                            }
                        }
                    }
                }
                if nested {
                    self.note(format!("{}: nested masks: the inner mask is kept", name(c)));
                }
            }
            all.extend(made);
        }
        all
    }

    /// A background shape layer for a frame's fills / strokes (node space → comp via `m`).
    fn frame_background(&mut self, comp: usize, n: &J, m: Affine) -> Option<usize> {
        if has_image_paint(n) && n.get("figcraft").and_then(J::as_bool) == Some(true) {
            return self.image_paints(comp, n, m).first().copied();
        }
        if paints(n, "fills").is_empty() && (paints(n, "strokes").is_empty() || num(n, "strokeWeight").unwrap_or(0.0) <= 0.0) {
            return None;
        }
        let items = shape_contents(n).ok()?;
        let mut l = PLayer::new(&format!("{} Background", name(n)), &id(n), PKind::Shape(items));
        let _ = Self::place(&mut l, m, size_of(n));
        self.push(comp, l)
    }

    /// A frame as the root of a new composition (frame size → comp size).
    fn root_frame(&mut self, n: &J) -> usize {
        if n.get("figcraft").and_then(J::as_bool) == Some(true) {
            let w = world(n, Affine::IDENTITY);
            let [fw, fh] = size_of(n);
            let bounds = render_bounds(n, w).union(aabb(w, Rect::new(0.0, 0.0, fw, fh)));
            let comp = self.new_comp(&name(n), &id(n), bounds.width(), bounds.height());
            let space = Space { from_world: Affine::translate((-bounds.x0, -bounds.y0)) };
            self.frame(comp, n, w, space, 0);
            return comp;
        }
        let [fw, fh] = size_of(n);
        let comp = self.new_comp(&name(n), &id(n), fw, fh);
        let w = world(n, Affine::IDENTITY);
        let space = Space { from_world: w.inverse() };
        self.frame_background(comp, n, Affine::IDENTITY);
        self.children(comp, n, linear(w), space, None, 1);
        if num(n, "opacity").unwrap_or(1.0) < 1.0 || n.get("effects").and_then(J::as_array).is_some_and(|e| e.iter().any(visible)) {
            self.note(format!("{}: the frame's own opacity and effects stay in Figma (it became the composition)", name(n)));
        }
        comp
    }

    /// A nested frame: a precomp (clipping frames clip to their size; others extend to their
    /// content).
    fn frame(&mut self, comp: usize, n: &J, w: Affine, space: Space, depth: usize) -> Vec<usize> {
        let [fw, fh] = size_of(n);
        let clips = n.get("clipsContent").and_then(J::as_bool).unwrap_or(false);
        let inv = w.inverse();
        let mut bounds = Rect::new(0.0, 0.0, fw, fh);
        if !clips {
            for c in children(n) {
                let cw = world(c, linear(w));
                bounds = bounds.union(aabb(inv, render_bounds(c, cw)));
            }
            bounds = Rect::new(bounds.x0.floor(), bounds.y0.floor(), bounds.x1.ceil(), bounds.y1.ceil());
        }
        let inner = self.new_comp(&name(n), &id(n), bounds.width(), bounds.height());
        let shift = Affine::translate((-bounds.x0, -bounds.y0));
        let inner_space = Space { from_world: shift * inv };
        self.frame_background(inner, n, shift);
        self.children(inner, n, linear(w), inner_space, None, depth + 1);
        let mut l = PLayer::new(&name(n), &id(n), PKind::Precomp(inner));
        if let Some(note) = Self::place(&mut l, space.from_world * w, [fw, fh]) {
            self.note(note);
        }
        // Layer space is the precomp's pixels: the frame's centre sits at -bounds.origin + centre.
        l.anchor = [fw / 2.0 - bounds.x0, fh / 2.0 - bounds.y0];
        self.common(&mut l, n);
        l.collapse = n.get("figcraft").and_then(J::as_bool) == Some(true)
            && n.get("blendMode").and_then(J::as_str) == Some("PASS_THROUGH")
            && l.opacity == 100.0
            && !clips
            && !n.get("effects").and_then(J::as_array).is_some_and(|effects| effects.iter().any(visible));
        if clips && let Some(r) = corner_radii(n).filter(|r| r.iter().any(|x| *x > 0.0)) {
            l.masks.extend(effectcraft_path::from_kurbo(&rounded_rect([fw, fh], r)));
        }
        self.push(comp, l).into_iter().collect()
    }

    /// A plain group: a null with the children parented to it. A group with opacity, a blend
    /// mode, effects or a mask role becomes a precomp of its content instead.
    fn group(&mut self, comp: usize, n: &J, w: Affine, parent_lin: Affine, space: Space, depth: usize) -> Vec<usize> {
        let simple = num(n, "opacity").unwrap_or(1.0) >= 1.0
            && matches!(n.get("blendMode").and_then(J::as_str).unwrap_or("PASS_THROUGH"), "PASS_THROUGH" | "NORMAL")
            && !n.get("effects").and_then(J::as_array).is_some_and(|e| e.iter().any(visible))
            && !n.get("isMask").and_then(J::as_bool).unwrap_or(false);
        let bb = rect_of(n.get("absoluteBoundingBox")).unwrap_or_else(|| aabb(w, Rect::new(0.0, 0.0, size_of(n)[0], size_of(n)[1])));
        if simple {
            let mut null = PLayer::new(&name(n), &id(n), PKind::Null);
            let c = space.from_world * bb.center();
            null.anchor = [50.0, 50.0];
            null.position = [c.x, c.y];
            null.visible = visible(n);
            let Some(ni) = self.push(comp, null) else { return vec![] };
            let mut made = vec![ni];
            let kids = self.children(comp, n, parent_lin, space, Some(ni), depth + 1);
            if !visible(n)
                && let Some(cc) = self.plan.comps.get_mut(comp)
            {
                for &i in &kids {
                    if let Some(l) = cc.layers.get_mut(i) {
                        l.visible = false;
                    }
                }
            }
            made.extend(kids);
            return made;
        }
        let mut bounds = render_bounds(n, w);
        for c in children(n) {
            bounds = bounds.union(render_bounds(c, world(c, parent_lin)));
        }
        let bounds = Rect::new(bounds.x0.floor(), bounds.y0.floor(), bounds.x1.ceil(), bounds.y1.ceil());
        let inner = self.new_comp(&name(n), &id(n), bounds.width(), bounds.height());
        let inner_space = Space { from_world: Affine::translate((-bounds.x0, -bounds.y0)) };
        self.children(inner, n, parent_lin, inner_space, None, depth + 1);
        let mut l = PLayer::new(&name(n), &id(n), PKind::Precomp(inner));
        let _ = Self::place(&mut l, space.from_world * Affine::translate((bounds.x0, bounds.y0)), [bounds.width(), bounds.height()]);
        self.common(&mut l, n);
        self.push(comp, l).into_iter().collect()
    }
}

/// Plan an import of `roots` (see [`select_nodes`]).
pub fn plan(roots: &[J], opts: &Options) -> Plan {
    let mut plan = Plan { name: roots.first().map(name).unwrap_or_default(), ..Default::default() };
    let mut m = Mapper { plan: &mut plan, nodes: 0 };
    let frame_like = |n: &J| {
        matches!(ty(n), "FRAME" | "COMPONENT" | "COMPONENT_SET" | "INSTANCE" | "SECTION")
            && (!has_image_paint(n) || n.get("figcraft").and_then(J::as_bool) == Some(true))
    };
    match opts.target {
        Target::NewComp if roots.iter().all(frame_like) => {
            for r in roots {
                let c = m.root_frame(r);
                m.plan.roots.push(c);
            }
        }
        target => {
            let mut u: Option<Rect> = None;
            for r in roots {
                let b = rect_of(r.get("absoluteBoundingBox")).unwrap_or_else(|| {
                    let [w, h] = size_of(r);
                    aabb(world(r, Affine::IDENTITY), Rect::new(0.0, 0.0, w, h))
                });
                // A new canvas must include stroke and shadow overflow. Existing-comp paste
                // keeps centring the geometry, and malformed render bounds cannot shrink it.
                let b = if matches!(target, Target::NewComp) {
                    rect_of(r.get("absoluteRenderBounds"))
                        .filter(|r| r.width() > 0.0 && r.height() > 0.0 && [r.x0, r.y0, r.x1, r.y1, r.width(), r.height()].iter().all(|v| v.is_finite()))
                        .map_or(b, |render| b.union(render))
                } else {
                    b
                };
                u = Some(u.map_or(b, |x| x.union(b)));
            }
            let u = u.unwrap_or_default();
            let label = if roots.len() == 1 { name(roots.first().unwrap_or(&J::Null)) } else { format!("{} (Figma)", m.plan.name) };
            let (comp, space) = match target {
                Target::ActiveComp { width, height } => {
                    let c = m.new_comp(&label, "", width as f64, height as f64);
                    let dx = (width as f64 - u.width()) / 2.0 - u.x0;
                    let dy = (height as f64 - u.height()) / 2.0 - u.y0;
                    m.plan.into_active = Some(c);
                    (c, Space { from_world: Affine::translate((dx, dy)) })
                }
                Target::NewComp => {
                    let c = m.new_comp(&label, "", u.width(), u.height());
                    m.plan.roots.push(c);
                    (c, Space { from_world: Affine::translate((-u.x0, -u.y0)) })
                }
            };
            // A virtual container: the roots as siblings, masks included.
            let container = json!({"children": roots});
            m.children(comp, &container, Affine::IDENTITY, space, None, 0);
        }
    }
    plan
}

// ---------------------------------------------------------------- shapes

/// Corner radii `[top-left, top-right, bottom-right, bottom-left]`.
fn corner_radii(n: &J) -> Option<[f64; 4]> {
    if let Some(a) = n.get("rectangleCornerRadii").and_then(J::as_array) {
        let g = |i: usize| a.get(i).and_then(J::as_f64).filter(|x| x.is_finite()).unwrap_or(0.0).max(0.0);
        return Some([g(0), g(1), g(2), g(3)]);
    }
    let names = ["topLeftRadius", "topRightRadius", "bottomRightRadius", "bottomLeftRadius"];
    if names.iter().any(|k| n.get(*k).is_some()) {
        let g = |k: &str| num(n, k).unwrap_or(0.0).max(0.0);
        return Some(names.map(g));
    }
    num(n, "cornerRadius").map(|r| [r.max(0.0); 4])
}

fn rounded_rect(size: [f64; 2], r: [f64; 4]) -> kurbo::BezPath {
    use kurbo::Shape as _;
    let max = size[0].min(size[1]) / 2.0;
    let r = r.map(|x| x.min(max));
    kurbo::RoundedRect::from_rect(Rect::new(0.0, 0.0, size[0], size[1]), kurbo::RoundedRectRadii::new(r[0], r[1], r[2], r[3])).to_path(0.01)
}

/// Path geometry from `vectorPaths` (plug-in: the editable centre lines) or `fillGeometry`
/// (REST), as SVG path data in node space.
fn geometry_paths(n: &J) -> Option<(Vec<ShapePath>, bool)> {
    for key in ["vectorPaths", "fillGeometry"] {
        let Some(a) = n.get(key).and_then(J::as_array).filter(|a| !a.is_empty()) else { continue };
        let mut out = vec![];
        let mut even_odd = false;
        for g in a {
            let d = g.get("path").or_else(|| g.get("data")).and_then(J::as_str).unwrap_or("");
            even_odd |= matches!(g.get("windingRule").and_then(J::as_str), Some("EVENODD"));
            out.extend(effectcraft_path::from_kurbo(&effectcraft_svg::parse_path_data(d)));
        }
        if !out.is_empty() {
            return Some((out, even_odd));
        }
    }
    None
}

/// Geometry items, whether all paths are closed, the fill rule, and an inset-able
/// parametric kind for stroke alignment.
enum Geom {
    Rect {
        size: [f64; 2],
        radius: f64,
    },
    Ellipse {
        size: [f64; 2],
    },
    /// `inner`: the inner radius as a fraction of the outer one.
    Star {
        star: bool,
        points: f64,
        inner: f64,
        size: [f64; 2],
    },
    Paths(Vec<ShapePath>),
}

impl Geom {
    fn items(&self, inset: f64) -> Vec<Item> {
        match self {
            Geom::Rect { size, radius } => {
                vec![Item::Rect {
                    size: [(size[0] - 2.0 * inset).max(0.0), (size[1] - 2.0 * inset).max(0.0)],
                    position: [size[0] / 2.0, size[1] / 2.0],
                    roundness: (radius - inset).max(0.0),
                }]
            }
            Geom::Ellipse { size } => {
                vec![Item::Ellipse { size: [(size[0] - 2.0 * inset).max(0.0), (size[1] - 2.0 * inset).max(0.0)], position: [size[0] / 2.0, size[1] / 2.0] }]
            }
            Geom::Star { star, points, inner, size } => {
                let outer = (size[0].min(size[1]) / 2.0 - inset).max(0.0);
                vec![Item::Star { star: *star, points: *points, position: [size[0] / 2.0, size[1] / 2.0], outer, inner: outer * inner }]
            }
            Geom::Paths(p) => {
                let mut v: Vec<Item> = p.iter().cloned().map(Item::Path).collect();
                if inset != 0.0 {
                    v.push(Item::Offset(-inset));
                }
                v
            }
        }
    }
    fn closed(&self) -> bool {
        match self {
            Geom::Paths(p) => p.iter().all(|s| s.closed),
            _ => true,
        }
    }
}

fn geometry(n: &J) -> Result<(Geom, bool), String> {
    let [w, h] = size_of(n);
    match ty(n) {
        "RECTANGLE" | "FRAME" | "COMPONENT" | "COMPONENT_SET" | "INSTANCE" | "SECTION" => match corner_radii(n) {
            Some(r) if r.iter().any(|x| (x - r[0]).abs() > 1e-6) => Ok((Geom::Paths(effectcraft_path::from_kurbo(&rounded_rect([w, h], r))), false)),
            r => Ok((Geom::Rect { size: [w, h], radius: r.map(|r| r[0]).unwrap_or(0.0).min(w.min(h) / 2.0) }, false)),
        },
        "ELLIPSE" => {
            let partial = n.get("arcData").is_some_and(|a| {
                let s = num(a, "startingAngle").unwrap_or(0.0);
                let e = num(a, "endingAngle").unwrap_or(std::f64::consts::TAU);
                num(a, "innerRadius").unwrap_or(0.0) > 0.0 || ((e - s).abs() - std::f64::consts::TAU).abs() > 1e-6
            });
            match (partial, geometry_paths(n)) {
                (true, Some((p, eo))) => Ok((Geom::Paths(p), eo)),
                _ => Ok((Geom::Ellipse { size: [w, h] }, false)),
            }
        }
        "LINE" => Ok((
            Geom::Paths(vec![ShapePath {
                vertices: vec![[0.0, 0.0], [w, 0.0]],
                in_tangents: vec![[0.0; 2]; 2],
                out_tangents: vec![[0.0; 2]; 2],
                closed: false,
                feather: vec![],
            }]),
            false,
        )),
        _ => {
            if let Some((p, eo)) = geometry_paths(n) {
                return Ok((Geom::Paths(p), eo));
            }
            // Plug-in fields for parametric polygons and stars without geometry.
            if let Some(points) = num(n, "pointCount").filter(|p| *p >= 3.0 && *p <= 100.0) {
                let star = ty(n) == "STAR";
                return Ok((Geom::Star { star, points, inner: num(n, "innerRadius").unwrap_or(0.38).clamp(0.0, 1.0), size: [w, h] }, false));
            }
            Err(format!("{} without geometry (request with geometry=paths)", ty(n).to_lowercase()))
        }
    }
}

fn gradient(p: &J, size: [f64; 2]) -> Option<GradientPaint> {
    let kind = match ty(p) {
        "GRADIENT_LINEAR" => 0,
        "GRADIENT_RADIAL" => 1,
        "GRADIENT_ANGULAR" => 2,
        "GRADIENT_DIAMOND" => 3,
        _ => return None,
    };
    let radial = kind != 0;
    let h = p.get("gradientHandlePositions").and_then(J::as_array);
    let pt = |i: usize, d: [f64; 2]| {
        let v = h.and_then(|h| h.get(i));
        [v.and_then(|v| num(v, "x")).unwrap_or(d[0]) * size[0], v.and_then(|v| num(v, "y")).unwrap_or(d[1]) * size[1]]
    };
    let (a, mut b) = if radial { (pt(0, [0.5, 0.5]), pt(1, [1.0, 0.5])) } else { (pt(0, [0.0, 0.5]), pt(1, [1.0, 0.5])) };
    // Three handles define an affine gradient: the third direction is a constant-colour
    // axis. A native linear gradient needs the equivalent perpendicular axis in pixels.
    if !radial
        && let Some(third) = h.and_then(|h| h.get(2))
        && let (Some(x), Some(y)) = (num(third, "x"), num(third, "y"))
    {
        let v = [x * size[0] - a[0], y * size[1] - a[1]];
        let normal = [v[1], -v[0]];
        let denominator = normal[0] * normal[0] + normal[1] * normal[1];
        let projection = (b[0] - a[0]) * normal[0] + (b[1] - a[1]) * normal[1];
        if denominator.is_finite() && denominator > 1e-12 && projection.is_finite() && projection.abs() > 1e-12 {
            let factor = projection / denominator;
            let end = [a[0] + normal[0] * factor, a[1] + normal[1] * factor];
            if end.iter().all(|v| v.is_finite()) {
                b = end;
            }
        }
    }
    let stops: Vec<(f64, [f64; 4])> = p
        .get("gradientStops")
        .and_then(J::as_array)
        .into_iter()
        .flatten()
        .map(|s| (num(s, "position").unwrap_or(0.0).clamp(0.0, 1.0), rgba(s.get("color"))))
        .collect();
    if stops.is_empty() {
        return None;
    }
    let g = Gradient {
        colors: stops.iter().map(|(o, c)| (*o, [c[0] as f32, c[1] as f32, c[2] as f32, 1.0])).collect(),
        opacities: stops.iter().map(|(o, c)| (*o, c[3] as f32)).collect(),
    };
    let width = (radial && h.is_some_and(|h| h.len() >= 3)).then(|| pt(2, [0.5, 1.0]));
    Some(GradientPaint { kind, start: a, end: b, width, gradient: g })
}

fn fill_items(n: &J, size: [f64; 2], even_odd: bool) -> Vec<Item> {
    // Top fill first (paints above in the Timeline paint on top).
    paints(n, "fills")
        .iter()
        .rev()
        .filter_map(|p| {
            let op = num(p, "opacity").unwrap_or(1.0).clamp(0.0, 1.0);
            match ty(p) {
                "SOLID" => {
                    let c = rgba(p.get("color"));
                    Some(Item::Fill { color: [c[0], c[1], c[2]], opacity: op * c[3] * 100.0, even_odd })
                }
                _ => gradient(p, size).map(|g| Item::GradientFill {
                    radial: g.kind != 0,
                    kind: g.kind,
                    width: g.width,
                    start: g.start,
                    end: g.end,
                    gradient: g.gradient,
                    opacity: op * 100.0,
                    even_odd,
                }),
            }
        })
        .collect()
}

fn stroke_items(n: &J, size: [f64; 2]) -> Vec<Item> {
    let width = num(n, "strokeWeight").unwrap_or(1.0);
    if width <= 0.0 {
        return vec![];
    }
    let cap = match n.get("strokeCap").and_then(J::as_str) {
        Some("ROUND") => 1,
        Some("SQUARE") => 2,
        _ => 0,
    };
    let join = match n.get("strokeJoin").and_then(J::as_str) {
        Some("ROUND") => 1,
        Some("BEVEL") => 2,
        _ => 0,
    };
    // Figma's miter angle (degrees) → a miter limit.
    let miter = num(n, "strokeMiterLimit")
        .filter(|m| *m >= 1.0)
        .or_else(|| num(n, "strokeMiterAngle").filter(|a| *a > 0.0 && *a < 180.0).map(|a| 1.0 / (a.to_radians() / 2.0).sin()))
        .unwrap_or(4.0)
        .min(100.0);
    let dashes: Vec<f64> = n
        .get("strokeDashes")
        .and_then(J::as_array)
        .map(|a| a.iter().filter_map(J::as_f64).filter(|x| x.is_finite() && *x >= 0.0).collect())
        .unwrap_or_default();
    let dash = match dashes.as_slice() {
        [] => None,
        [d] => Some((*d, *d)),
        [d, g, ..] => Some((*d, *g)),
    };
    paints(n, "strokes")
        .iter()
        .rev()
        .filter_map(|p| {
            let op = num(p, "opacity").unwrap_or(1.0).clamp(0.0, 1.0);
            let (color, gradient, alpha) = match ty(p) {
                "SOLID" => {
                    let c = rgba(p.get("color"));
                    ([c[0], c[1], c[2]], None, c[3])
                }
                _ => ([0.0; 3], Some(gradient(p, size)?), 1.0),
            };
            Some(Item::Stroke(Stroke { color, gradient, opacity: op * alpha * 100.0, width, cap, join, miter, dash }))
        })
        .collect()
}

/// Shape layer contents for a shape node (or a frame's background): one group named after the
/// node with its geometry, strokes and fills. Inside / outside strokes on closed geometry get a
/// group of their own whose geometry is inset / outset by half the stroke width.
pub fn shape_contents(n: &J) -> Result<Vec<Item>, String> {
    let size = size_of(n);
    let (geom, even_odd) = geometry(n)?;
    let fills = fill_items(n, size, even_odd);
    let strokes = stroke_items(n, size);
    let width = num(n, "strokeWeight").unwrap_or(1.0);
    let align = n.get("strokeAlign").and_then(J::as_str).unwrap_or("CENTER");
    let inset = match align {
        "INSIDE" if geom.closed() => width / 2.0,
        "OUTSIDE" if geom.closed() => -width / 2.0,
        _ => 0.0,
    };
    let mut items = vec![];
    if strokes.is_empty() || inset == 0.0 {
        items.extend(geom.items(0.0));
        items.extend(strokes);
        items.extend(fills);
    } else {
        let mut s = geom.items(inset);
        s.extend(strokes);
        items.push(Item::Group { name: "Stroke".into(), items: s });
        if !fills.is_empty() {
            let mut f = geom.items(0.0);
            f.extend(fills);
            items.push(Item::Group { name: "Fill".into(), items: f });
        }
    }
    let label = if name(n).trim().is_empty() { "Shape".to_string() } else { name(n) };
    Ok(vec![Item::Group { name: label, items }])
}

// ---------------------------------------------------------------- text

fn weight_style(w: f64, italic: bool) -> String {
    let base = match w.round() as i64 {
        ..=150 => "Thin",
        151..=250 => "ExtraLight",
        251..=350 => "Light",
        351..=450 => "Regular",
        451..=550 => "Medium",
        551..=650 => "SemiBold",
        651..=750 => "Bold",
        751..=850 => "ExtraBold",
        _ => "Black",
    };
    match (base, italic) {
        ("Regular", true) => "Italic".into(),
        (b, true) => format!("{b} Italic"),
        (b, false) => b.into(),
    }
}

/// Character attributes of a Figma TypeStyle (plus the node's fills) as `TextDoc::set_attr`
/// keys. Only what the style sets is returned (overrides are partial).
fn style_attrs(st: &J, fills: Option<&J>, base_size: f64) -> Vec<(&'static str, J)> {
    let mut v = vec![];
    if let Some(f) = st.get("fontFamily").and_then(J::as_str) {
        v.push(("font", json!(f)));
    }
    let italic = st.get("italic").and_then(J::as_bool).unwrap_or(false);
    if let Some(s) = st.get("fontStyle").and_then(J::as_str) {
        v.push(("style", json!(s)));
    } else if let Some(w) = num(st, "fontWeight") {
        v.push(("style", json!(weight_style(w, italic))));
    } else if italic {
        v.push(("style", json!("Italic")));
    }
    let size = num(st, "fontSize").unwrap_or(base_size).max(0.1);
    if num(st, "fontSize").is_some() {
        v.push(("size", json!(size)));
    }
    if let Some(ls) = num(st, "letterSpacing") {
        // Pixels (REST) → 1/1000 em; the plug-in sends {value, unit} converted already.
        v.push(("tracking", json!(ls / size * 1000.0)));
    }
    if let Some(lh) = num(st, "lineHeightPx") {
        v.push(("leading", json!(lh)));
    }
    match st.get("textCase").and_then(J::as_str) {
        Some("UPPER") => v.push(("allCaps", json!(true))),
        Some("SMALL_CAPS" | "SMALL_CAPS_FORCED") => v.push(("smallCaps", json!(true))),
        Some("ORIGINAL") => v.push(("allCaps", json!(false))),
        _ => {}
    }
    let fills = st.get("fills").or(fills);
    if let Some(p) = fills.and_then(J::as_array).and_then(|a| a.iter().rev().find(|p| visible(p))) {
        let c = match ty(p) {
            "SOLID" => Some(rgba(p.get("color"))),
            _ => p.get("gradientStops").and_then(J::as_array).and_then(|s| s.first()).map(|s| rgba(s.get("color"))),
        };
        if let Some(c) = c {
            let a = c[3] * num(p, "opacity").unwrap_or(1.0);
            v.push(("fill", json!([c[0], c[1], c[2], a.clamp(0.0, 1.0)])));
        }
    }
    v
}

/// A text node's Source Text: paragraph text in a box the size of the node (point-sized text
/// gets room to spare so a different font's metrics don't wrap it), style runs from
/// `characterStyleOverrides`.
pub fn text_doc(n: &J) -> (TextDoc, Vec<String>) {
    let mut notes = vec![];
    let chars = n.get("characters").and_then(J::as_str).unwrap_or("").to_string();
    let st = n.get("style").cloned().unwrap_or(json!({}));
    let mut doc = TextDoc { text: chars.clone(), ..Default::default() };
    let size = num(&st, "fontSize").unwrap_or(12.0).clamp(0.1, 10_000.0);
    doc.size = size;
    doc.font = "Inter".into();
    doc.style = "Regular".into();
    for (k, v) in style_attrs(&st, n.get("fills"), size) {
        let _ = doc.set_attr(k, &v, None);
    }
    // set_attr on an empty selection only touches runs; make the base match too.
    if let Some(f) = st.get("fontFamily").and_then(J::as_str) {
        doc.font = f.to_string();
    }
    doc.justify = match st.get("textAlignHorizontal").and_then(J::as_str) {
        Some("CENTER") => Justify::Center,
        Some("RIGHT") => Justify::Right,
        Some("JUSTIFIED") => Justify::JustifyLastLeft,
        _ => Justify::Left,
    };
    match st.get("textCase").and_then(J::as_str) {
        Some("LOWER") => doc.text = doc.text.to_lowercase(),
        Some("TITLE") => {
            let mut prev_space = true;
            doc.text = doc
                .text
                .chars()
                .map(|c| {
                    let out = if prev_space { c.to_uppercase().next().unwrap_or(c) } else { c };
                    prev_space = c.is_whitespace();
                    out
                })
                .collect();
        }
        _ => {}
    }
    if matches!(st.get("textDecoration").and_then(J::as_str), Some("UNDERLINE" | "STRIKETHROUGH")) {
        notes.push(format!("{}: underline / strikethrough is not supported", name(n)));
    }
    if let Some(ps) = num(&st, "paragraphSpacing").filter(|p| *p > 0.0) {
        doc.space_after = ps;
    }
    // Style runs: `characterStyleOverrides` holds one style id per UTF-16 code unit.
    let overrides: Vec<u64> =
        n.get("characterStyleOverrides").and_then(J::as_array).map(|a| a.iter().map(|x| x.as_u64().unwrap_or(0)).collect()).unwrap_or_default();
    let table = n.get("styleOverrideTable");
    if !overrides.is_empty()
        && let Some(table) = table
    {
        let mut per_char: Vec<u64> = vec![];
        let mut unit = 0usize;
        for c in chars.chars() {
            per_char.push(overrides.get(unit).copied().unwrap_or(0));
            unit = unit.saturating_add(c.len_utf16());
        }
        let mut start = 0usize;
        while start < per_char.len() {
            let sid = per_char.get(start).copied().unwrap_or(0);
            let mut end = start + 1;
            while per_char.get(end) == Some(&sid) {
                end += 1;
            }
            if sid != 0
                && let Some(o) = table.get(sid.to_string())
            {
                let osize = num(o, "fontSize").unwrap_or(size);
                // A style id that only changes the size keeps the base letter spacing in px.
                for (k, v) in style_attrs(o, None, osize) {
                    let _ = doc.set_attr(k, &v, Some(start..end));
                }
            }
            start = end;
        }
    }
    let [w, h] = size_of(n);
    let auto = n.get("textAutoResize").or_else(|| st.get("textAutoResize")).and_then(J::as_str).unwrap_or("NONE");
    let slack = if auto == "WIDTH_AND_HEIGHT" { (w * 0.1).max(size * 0.5).max(8.0) } else { 0.0 };
    let bx = match doc.justify {
        Justify::Center => -slack / 2.0,
        Justify::Right => -slack,
        _ => 0.0,
    };
    // Figma centres the font's content area in each line; After Effects puts the first baseline
    // one ascent below the box top. Shift by the half-leading difference.
    let line = num(&st, "lineHeightPx").unwrap_or(size * 1.21);
    let mut by = (line - size * 1.21) / 2.0;
    let lines = doc.text.split('\n').count().max(1) as f64;
    match st.get("textAlignVertical").and_then(J::as_str) {
        Some("CENTER") if auto == "NONE" => by += ((h - lines * line) / 2.0).max(0.0),
        Some("BOTTOM") if auto == "NONE" => by += (h - lines * line).max(0.0),
        _ => {}
    }
    doc.box_pos = [bx, by];
    doc.box_size = Some([(w + slack).max(1.0), (h + size * 2.0).max(1.0)]);
    if let Some(baseline) = num(n, "figcraftTextBaseline") {
        doc.box_pos[1] = 0.0;
        if let Some(first) = effectcraft_text::layout_doc(&doc).line_boxes.first() {
            doc.box_pos[1] = baseline - first[1];
        }
    }
    (doc, notes)
}
