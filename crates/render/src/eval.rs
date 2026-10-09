//! Property evaluation (keyframes + expressions) and layer transforms.
//!
//! Keyframe times are stored in **layer time** (they move with the layer), so evaluating a
//! property at comp time `t` first maps `t` through the layer's start time and stretch.

use effectcraft_geom::{Mat3, Mat4, Vec3, vec2, vec3};
use effectcraft_keyframe::Value;
use effectcraft_project::{Comp, ItemId, Layer, LayerId, LayerSource, Project, PropGroup, Property};
use effectcraft_time::Tick;

/// Expression evaluation hook (implemented by `effectcraft-expr`).
pub trait ExprHost: Send + Sync {
    /// Evaluate the expression of `prop` (on `layer`, comp time `t`), given the keyframed value.
    fn eval(&self, ctx: &EvalCtx, layer: &Layer, prop: &Property, value: &Value) -> Result<Value, String>;
    /// Evaluate a text Expression Selector's Amount expression (on `prop`) for one unit:
    /// `textIndex` (1-based), `textTotal` and `selectorValue` (percent, per dimension) are
    /// defined. Returns the amount in percent per dimension.
    fn eval_text_selector(
        &self,
        _ctx: &EvalCtx,
        _layer: &Layer,
        _prop: &Property,
        _index: usize,
        _total: usize,
        _selector: [f64; 3],
    ) -> Result<[f64; 3], String> {
        Err("no expression engine".into())
    }
}

#[derive(Clone, Copy)]
pub struct EvalCtx<'a> {
    pub project: &'a Project,
    pub comp_id: ItemId,
    pub comp: &'a Comp,
    /// Comp time.
    pub time: Tick,
    pub expr: Option<&'a dyn ExprHost>,
    /// Footage frames for expressions that read rendered pixels (`sampleImage`): the renderer's
    /// own source while rendering; `None` renders footage as transparent.
    pub footage: Option<&'a dyn crate::FootageSource>,
}

impl<'a> EvalCtx<'a> {
    pub fn new(project: &'a Project, comp_id: ItemId, comp: &'a Comp, time: Tick) -> EvalCtx<'a> {
        EvalCtx { project, comp_id, comp, time, expr: None, footage: None }
    }
    pub fn at(&self, time: Tick) -> EvalCtx<'a> {
        EvalCtx { time, ..*self }
    }
    /// Value of a property at the context time (keyframes, then expression).
    pub fn value(&self, layer: &Layer, prop: &Property) -> Value {
        // Separated Position reads as the combination of X/Y/Z Position.
        if prop.match_id == "position"
            && let Some(tr) = layer.transform()
            && let Some(px) = tr.get("positionX")
            && tr.get("position").is_some_and(|p| p.uid == prop.uid)
        {
            let z = tr.get("positionZ").map(|p| self.value(layer, p).as_f64()).unwrap_or(0.0);
            let y = tr.get("positionY").map(|p| self.value(layer, p).as_f64()).unwrap_or(0.0);
            return Value::Vec3([self.value(layer, px).as_f64(), y, z]);
        }
        let lt = layer.layer_time(self.time);
        let v = prop.value_at(lt);
        if prop.has_expression()
            && let Some(h) = self.expr
        {
            return h.eval(self, layer, prop, &v).unwrap_or(v);
        }
        v
    }
    pub fn group_value(&self, layer: &Layer, g: &PropGroup, m: &str) -> Option<Value> {
        g.get(m).map(|p| self.value(layer, p))
    }
    pub fn f(&self, layer: &Layer, g: &PropGroup, m: &str, d: f64) -> f64 {
        self.group_value(layer, g, m).map(|v| v.as_f64()).unwrap_or(d)
    }
    pub fn v2(&self, layer: &Layer, g: &PropGroup, m: &str, d: [f64; 2]) -> [f64; 2] {
        self.group_value(layer, g, m).map(|v| v.as_vec2()).unwrap_or(d)
    }
    pub fn v3(&self, layer: &Layer, g: &PropGroup, m: &str, d: [f64; 3]) -> [f64; 3] {
        self.group_value(layer, g, m).map(|v| v.as_vec3()).unwrap_or(d)
    }
    pub fn color(&self, layer: &Layer, g: &PropGroup, m: &str) -> [f32; 4] {
        self.group_value(layer, g, m).map(|v| v.as_color()).unwrap_or([1.0; 4])
    }
    pub fn e(&self, layer: &Layer, g: &PropGroup, m: &str) -> u32 {
        self.group_value(layer, g, m).map(|v| v.as_enum()).unwrap_or(0)
    }
    pub fn b(&self, layer: &Layer, g: &PropGroup, m: &str) -> bool {
        self.group_value(layer, g, m).map(|v| v.as_bool()).unwrap_or(false)
    }
    /// Layer Position, honouring Separate Dimensions (X/Y/Z Position properties).
    pub fn position(&self, layer: &Layer, tr: &PropGroup) -> [f64; 3] {
        match tr.get("positionX") {
            Some(px) => [self.value(layer, px).as_f64(), self.f(layer, tr, "positionY", 0.0), self.f(layer, tr, "positionZ", 0.0)],
            None => self.v3(layer, tr, "position", [0.0; 3]),
        }
    }
    /// Source time of a layer at the context time: Time Remap's value when enabled, else the
    /// (stretch-aware) layer time.
    pub fn source_time(&self, layer: &Layer) -> Tick {
        if let Some(tr) = layer.props.get("timeRemap") {
            return Tick::from_seconds_f64(self.value(layer, tr).as_f64());
        }
        // Responsive Design — Time: a time-stretched precomp keeps its protected regions at
        // their original speed.
        if let LayerSource::Comp { item } = layer.source
            && (layer.stretch - 100.0).abs() > 1e-9
            && let Some(nc) = self.project.comp(item)
            && let Some(t) = responsive_source_time(nc, layer.stretch / 100.0, (self.time - layer.start_time).seconds())
        {
            return t;
        }
        layer.layer_time(self.time)
    }
    /// The time a precomp layer's nested comp is shown at: [`Self::source_time`], floored to
    /// the nested comp's own frames when it preserves its frame rate (Composition Settings ▸
    /// Preserve frame rate when nested or in render queue).
    pub fn nested_time(&self, layer: &Layer) -> Tick {
        let t = self.source_time(layer);
        match layer.source {
            LayerSource::Comp { item } => match self.project.comp(item) {
                Some(nc) if nc.preserve_frame_rate => nc.frame_rate.tick_of(crate::frame_position(t, nc.frame_rate).0),
                _ => t,
            },
            _ => t,
        }
    }
    pub fn layer(&self, id: LayerId) -> Option<&'a Layer> {
        self.comp.layer(id)
    }

    /// Local (parent-space) transform of a layer.
    pub fn local_matrix(&self, layer: &Layer) -> Mat4 {
        self.local_matrix_par(layer, 1.0)
    }

    /// Source pixel aspect relative to the comp's: non-square footage (or solids, or precomps
    /// with another pixel aspect) is stretched horizontally by this factor in the comp.
    pub fn par_ratio(&self, layer: &Layer) -> f64 {
        let par = match &layer.source {
            LayerSource::Footage { item } | LayerSource::Solid { item } | LayerSource::Comp { item } => match self.project.item(*item).map(|i| &i.kind) {
                Some(effectcraft_project::ItemKind::Footage(f)) => f.pixel_aspect,
                Some(effectcraft_project::ItemKind::Solid(s)) => s.pixel_aspect,
                Some(effectcraft_project::ItemKind::Comp(c)) => c.pixel_aspect,
                _ => 1.0,
            },
            _ => 1.0,
        };
        let r = par / if self.comp.pixel_aspect > 0.0 { self.comp.pixel_aspect } else { 1.0 };
        if r.is_finite() && r > 0.0 && (r - 1.0).abs() > 1e-9 { r } else { 1.0 }
    }

    /// [`Self::local_matrix`] with the source's pixel aspect folded into the scale (`par`; 1 for
    /// a parent's matrix, which children don't inherit).
    fn local_matrix_par(&self, layer: &Layer, par: f64) -> Mat4 {
        let Some(tr) = layer.transform() else { return Mat4::IDENTITY };
        let three = layer.is_3d();
        let pos = self.position(layer, tr);
        let rz = self.f(layer, tr, "rotation", 0.0);
        if layer.is_camera() || layer.is_light() {
            // Children of cameras/lights follow their position and full rotation.
            return Mat4::translate(Vec3::from(pos)) * crate::three_d::camera::rig_rotation(self, layer);
        }
        let anchor = self.v3(layer, tr, "anchor", [0.0; 3]);
        let mut scale = self.v3(layer, tr, "scale", [100.0; 3]);
        scale[0] *= par;
        if !three {
            scale[2] = 100.0;
            let a = Vec3::from([anchor[0], anchor[1], 0.0]);
            let p = Vec3::from([pos[0], pos[1], 0.0]);
            let rz = rz + crate::three_d::camera::auto_orient_2d(self, layer);
            return Mat4::layer_3d(a, p, Vec3::from(scale), Vec3::ZERO, vec3(0.0, 0.0, rz));
        }
        let rx = self.f(layer, tr, "rotationX", 0.0);
        let ry = self.f(layer, tr, "rotationY", 0.0);
        let p = Vec3::from(pos);
        // Auto-orient (along path / towards camera) replaces Orientation.
        let orient = crate::three_d::camera::auto_orient_3d(self, layer, p)
            .unwrap_or_else(|| Mat4::orientation(Vec3::from(self.v3(layer, tr, "orientation", [0.0; 3]))));
        Mat4::translate(p)
            * orient
            * Mat4::rotate_z(rz)
            * Mat4::rotate_y(ry)
            * Mat4::rotate_x(rx)
            * Mat4::scale(Vec3::from(scale) / 100.0)
            * Mat4::translate(-Vec3::from(anchor))
    }

    /// Layer space → comp (world) space, including parents.
    pub fn world_matrix(&self, layer: &Layer) -> Mat4 {
        let mut m = self.local_matrix_par(layer, self.par_ratio(layer));
        let mut cur = layer.parent;
        let mut guard = 0;
        while let Some(pid) = cur {
            guard += 1;
            if guard > 64 {
                break;
            }
            let Some(p) = self.comp.layer(pid) else { break };
            m = self.local_matrix(p) * m;
            cur = p.parent;
        }
        m
    }

    pub fn opacity(&self, layer: &Layer) -> f64 {
        layer.transform().map(|tr| self.f(layer, tr, "opacity", 100.0)).unwrap_or(100.0) / 100.0
    }

    /// The comp's active camera at this time: (view-projection to comp pixels, eye, zoom).
    pub fn camera(&self) -> (Mat4, Vec3, f64) {
        let c = crate::three_d::active_camera(self);
        (c.projection(self.comp.width as f64, self.comp.height as f64), c.eye, c.zoom)
    }

    /// Layer space → comp pixel space as a 2D projective matrix, plus camera-space depth of the
    /// layer's anchor (for 3D sorting). 2D layers ignore the camera.
    pub fn layer_to_comp(&self, layer: &Layer) -> (Mat3, f64) {
        let world = self.world_matrix(layer);
        if layer.is_3d() {
            let (cam, _, _) = self.camera();
            let full = cam * world;
            let anchor = layer.transform().map(|tr| self.v3(layer, tr, "anchor", [0.0; 3])).unwrap_or([0.0; 3]);
            let a = world.apply(Vec3::from(anchor));
            let depth = cam.0[3][0] * a.x + cam.0[3][1] * a.y + cam.0[3][2] * a.z + cam.0[3][3];
            (full.plane_to_mat3(), depth)
        } else {
            let m = &world.0;
            (Mat3([[m[0][0], m[0][1], m[0][3]], [m[1][0], m[1][1], m[1][3]], [0.0, 0.0, 1.0]]), 0.0)
        }
    }
    /// [`effect_bounds`] of a layer of this comp.
    pub fn effect_bounds(&self, layer: &Layer) -> ([f64; 2], [f64; 2]) {
        effect_bounds(self.project, self.comp, layer)
    }
    /// Effect space (see [`effect_bounds`]) → comp matrix.
    pub fn effect_to_comp(&self, layer: &Layer) -> Mat3 {
        let o = self.effect_bounds(layer).1;
        self.layer_to_comp(layer).0 * Mat3::translate(vec2(o[0], o[1]))
    }
}

/// The protected regions of a comp (Responsive Design — Time markers), merged, sorted and
/// clamped to the comp, in seconds.
pub fn protected_regions(comp: &Comp) -> Vec<(f64, f64)> {
    let d = comp.duration.seconds();
    let mut v: Vec<(f64, f64)> = comp
        .markers
        .iter()
        .filter(|m| m.protected && m.duration > Tick::ZERO)
        .map(|m| (m.time.seconds().clamp(0.0, d), (m.time + m.duration).seconds().clamp(0.0, d)))
        .filter(|(a, b)| b > a)
        .collect();
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out: Vec<(f64, f64)> = vec![];
    for (a, b) in v {
        match out.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// Source time of a precomp time-stretched by `stretch` (1 = 100%), `elapsed` comp seconds
/// after the layer's start, with Responsive Design — Time: protected regions play at their
/// original speed and the unprotected parts absorb the whole stretch. `None` when the comp has
/// no protected regions or the stretch can't be absorbed (reversed, or shorter than the
/// protected regions), so the plain stretch applies.
pub fn responsive_source_time(comp: &Comp, stretch: f64, elapsed: f64) -> Option<Tick> {
    let regions = protected_regions(comp);
    if regions.is_empty() || stretch <= 0.0 {
        return None;
    }
    let d = comp.duration.seconds();
    let lp: f64 = regions.iter().map(|(a, b)| b - a).sum();
    let lu = d - lp;
    let k = if lu > 1e-9 { (d * stretch - lp) / lu } else { 1.0 };
    if k <= 1e-9 {
        return None;
    }
    if elapsed < 0.0 {
        return Some(Tick::from_seconds_f64(elapsed / k));
    }
    // Segments in source order: (start, end, protected).
    let mut segs = vec![];
    let mut at = 0.0;
    for (a, b) in &regions {
        if *a > at {
            segs.push((at, *a, false));
        }
        segs.push((*a, *b, true));
        at = *b;
    }
    if d > at {
        segs.push((at, d, false));
    }
    let mut pos = 0.0;
    for (a, b, prot) in segs {
        let speed = if prot { 1.0 } else { k };
        let len = (b - a) * speed;
        if elapsed < pos + len {
            return Some(Tick::from_seconds_f64(a + (elapsed - pos) / speed));
        }
        pos += len;
    }
    Some(Tick::from_seconds_f64(d + (elapsed - pos) / k))
}

/// A layer's effect bounds ([`effectcraft_effects::EffectCtx::layer_size`]) and their top-left
/// corner in layer coordinates: the source rectangle at (0, 0), or for layers without one
/// (shape, text) a comp-sized rectangle centred on the layer's origin, which their content
/// surrounds. Effects work in **effect space**, measured from that corner (After Effects' layer
/// space for effect points: a shape layer's default effect point is its bounds' centre).
pub fn effect_bounds(project: &Project, comp: &Comp, layer: &Layer) -> ([f64; 2], [f64; 2]) {
    match source_size(project, layer) {
        (0, _) => {
            let (w, h) = (comp.width as f64, comp.height as f64);
            ([w, h], [-w / 2.0, -h / 2.0])
        }
        (w, h) => ([w as f64, h as f64], [0.0; 2]),
    }
}

/// The layer's source time range in layer time seconds as (first, end) for
/// [`effectcraft_effects::EffectEnv::layer_span`]: footage and nested comps last their item's
/// duration from layer time 0. With Time Remap layer time is no longer source time, so the layer's
/// visible span (in to out point, in layer time) is reported instead. `None` for sources without
/// a duration (stills, solids, text, shapes...).
pub fn layer_span(project: &Project, layer: &Layer) -> Option<(f64, f64)> {
    let span = if layer.props.get("timeRemap").is_some() {
        let (a, b) = (layer.layer_time(layer.in_point).seconds(), layer.layer_time(layer.out_point).seconds());
        (a.min(b), a.max(b))
    } else {
        match &layer.source {
            LayerSource::Footage { item } | LayerSource::Comp { item } => (0.0, project.item(*item)?.duration()?.seconds()),
            _ => return None,
        }
    };
    (span.0.is_finite() && span.1.is_finite() && span.1 > span.0).then_some(span)
}

/// Size of a layer's source in layer pixels.
pub fn source_size(project: &Project, layer: &Layer) -> (u32, u32) {
    match &layer.source {
        LayerSource::Footage { item } | LayerSource::Comp { item } | LayerSource::Solid { item } => {
            project.item(*item).and_then(|i| i.dimensions()).unwrap_or((0, 0))
        }
        LayerSource::Null => (100, 100),
        _ => (0, 0),
    }
}
