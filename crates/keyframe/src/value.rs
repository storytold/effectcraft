//! Property values.

use serde::{Deserialize, Serialize};

/// A Bezier shape in the After Effects / Lottie vertex model: vertices with tangents relative to
/// their vertex.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ShapePath {
    pub vertices: Vec<[f64; 2]>,
    pub in_tangents: Vec<[f64; 2]>,
    pub out_tangents: Vec<[f64; 2]>,
    pub closed: bool,
    /// Variable-width mask feather points (Mask Feather tool). They travel with the path, so a
    /// Mask Path keyframe carries them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub feather: Vec<FeatherPoint>,
}

/// A mask feather point: a position on the path (segment `segment`, from vertex `segment` to the
/// next, at Bezier parameter `t`), a signed feather `radius` in pixels (positive = outer
/// feather, negative = inner) and a `tension` 0–1 (0: the radius blends smoothly to the
/// neighbouring points, 1: linearly).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FeatherPoint {
    pub segment: usize,
    pub t: f64,
    pub radius: f64,
    #[serde(default)]
    pub tension: f64,
}

impl ShapePath {
    pub fn polygon(points: &[[f64; 2]], closed: bool) -> ShapePath {
        ShapePath {
            vertices: points.to_vec(),
            in_tangents: vec![[0.0; 2]; points.len()],
            out_tangents: vec![[0.0; 2]; points.len()],
            closed,
            feather: Vec::new(),
        }
    }
    pub fn len(&self) -> usize {
        self.vertices.len()
    }
    /// Give both tangent arrays one entry per vertex: missing tangents become zero (a corner),
    /// extra ones are dropped. The three arrays are read independently from a project file, so a
    /// loaded path can disagree with itself (#1250). Returns whether anything changed.
    pub fn fit_tangents(&mut self) -> bool {
        let n = self.vertices.len();
        let changed = self.in_tangents.len() != n || self.out_tangents.len() != n;
        self.in_tangents.resize(n, [0.0; 2]);
        self.out_tangents.resize(n, [0.0; 2]);
        changed
    }
    /// Number of segments (vertex to next vertex, the closing one included).
    pub fn segment_count(&self) -> usize {
        let n = self.vertices.len();
        if self.closed { n } else { n.saturating_sub(1) }
    }
    /// Keep feather points in place after a vertex was inserted at `i` (segments from `i` on
    /// shift by one; a split of segment `i - 1` at parameter `split` re-parameterises its points).
    pub fn feather_after_insert(&mut self, i: usize, split: Option<f64>) {
        for f in &mut self.feather {
            if i > 0 && f.segment == i - 1 {
                if let Some(t) = split.filter(|t| *t > 0.0 && *t < 1.0) {
                    if f.t < t {
                        f.t /= t;
                    } else {
                        f.segment += 1;
                        f.t = (f.t - t) / (1.0 - t);
                    }
                }
            } else if f.segment >= i {
                f.segment += 1;
            }
        }
    }
    /// Keep feather points after vertex `i` was removed: its two segments merge.
    pub fn feather_after_remove(&mut self, i: usize) {
        let segs = self.segment_count();
        self.feather.retain_mut(|f| {
            if i == 0 && f.segment == 0 && !self.closed {
                return false;
            }
            if i == 0 && self.closed && segs > 0 {
                // The old closing segment and segment 0 merge into the new closing one.
                if f.segment == segs {
                    f.segment = segs - 1;
                    f.t *= 0.5;
                } else if f.segment == 0 {
                    f.segment = segs - 1;
                    f.t = 0.5 + f.t * 0.5;
                } else {
                    f.segment -= 1;
                }
            } else if i > 0 && f.segment == i - 1 {
                f.t *= 0.5;
            } else if f.segment == i && i > 0 {
                f.segment = i - 1;
                f.t = 0.5 + f.t * 0.5;
            } else if f.segment > i || (i == 0 && f.segment > 0) {
                f.segment -= 1;
            }
            f.segment < segs.max(1)
        });
    }
    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty()
    }
    /// Rectangle centred at `c` (clockwise from top-left, as AE builds mask rectangles).
    pub fn rect(c: [f64; 2], w: f64, h: f64) -> ShapePath {
        let (x0, y0, x1, y1) = (c[0] - w / 2.0, c[1] - h / 2.0, c[0] + w / 2.0, c[1] + h / 2.0);
        ShapePath::polygon(&[[x0, y0], [x1, y0], [x1, y1], [x0, y1]], true)
    }
    /// Ellipse with four Bezier arcs (kappa 0.5523).
    pub fn ellipse(c: [f64; 2], w: f64, h: f64) -> ShapePath {
        const K: f64 = 0.552_284_749_8;
        let (rx, ry) = (w / 2.0, h / 2.0);
        ShapePath {
            vertices: vec![[c[0], c[1] - ry], [c[0] + rx, c[1]], [c[0], c[1] + ry], [c[0] - rx, c[1]]],
            in_tangents: vec![[-rx * K, 0.0], [0.0, -ry * K], [rx * K, 0.0], [0.0, ry * K]],
            out_tangents: vec![[rx * K, 0.0], [0.0, ry * K], [-rx * K, 0.0], [0.0, -ry * K]],
            closed: true,
            feather: Vec::new(),
        }
    }
    fn lerp(&self, o: &ShapePath, t: f64) -> ShapePath {
        if self.len() != o.len() {
            return if t < 1.0 { self.clone() } else { o.clone() };
        }
        let l = |a: &[[f64; 2]], b: &[[f64; 2]]| a.iter().zip(b).map(|(p, q)| [p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t]).collect();
        ShapePath {
            vertices: l(&self.vertices, &o.vertices),
            in_tangents: l(&self.in_tangents, &o.in_tangents),
            out_tangents: l(&self.out_tangents, &o.out_tangents),
            closed: self.closed,
            // Feather points interpolate when both keys have the same points.
            feather: if self.feather.len() == o.feather.len() {
                self.feather
                    .iter()
                    .zip(&o.feather)
                    .map(|(a, b)| FeatherPoint {
                        segment: if t < 1.0 { a.segment } else { b.segment },
                        t: if a.segment == b.segment {
                            a.t + (b.t - a.t) * t
                        } else if t < 1.0 {
                            a.t
                        } else {
                            b.t
                        },
                        radius: a.radius + (b.radius - a.radius) * t,
                        tension: a.tension + (b.tension - a.tension) * t,
                    })
                    .collect()
            } else if t < 1.0 {
                self.feather.clone()
            } else {
                o.feather.clone()
            },
        }
    }
}

pub use crate::text_doc::{Justify, TextDoc};

/// A gradient: colour stops `(position 0..1, rgba)` and opacity stops `(position, alpha)`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Gradient {
    pub colors: Vec<(f64, [f32; 4])>,
    pub opacities: Vec<(f64, f32)>,
}

impl Default for Gradient {
    fn default() -> Self {
        Gradient { colors: vec![(0.0, [1.0, 1.0, 1.0, 1.0]), (1.0, [0.0, 0.0, 0.0, 1.0])], opacities: vec![(0.0, 1.0), (1.0, 1.0)] }
    }
}

impl Gradient {
    pub fn sample(&self, t: f64) -> [f32; 4] {
        let t = t.clamp(0.0, 1.0);
        let col = sample_stops(&self.colors, t, |a, b, f| {
            let mut o = [0.0; 4];
            for i in 0..4 {
                o[i] = a[i] + (b[i] - a[i]) * f as f32;
            }
            o
        })
        .unwrap_or([1.0; 4]);
        let a = sample_stops(&self.opacities, t, |a, b, f| a + (b - a) * f as f32).unwrap_or(1.0);
        [col[0], col[1], col[2], col[3] * a]
    }

    /// The colour stops alone at `t` (0..1), RGBA as stored: unlike [`Gradient::sample`], the
    /// opacity stops are not applied. A new colour stop takes this value.
    pub fn color_at(&self, t: f64) -> [f32; 4] {
        sample_stops(&self.colors, t.clamp(0.0, 1.0), |a, b, f| {
            let mut o = [0.0; 4];
            for i in 0..4 {
                o[i] = a[i] + (b[i] - a[i]) * f as f32;
            }
            o
        })
        .unwrap_or([1.0; 4])
    }

    /// The opacity stops alone at `t` (0..1). A new opacity stop takes this value.
    pub fn opacity_at(&self, t: f64) -> f32 {
        sample_stops(&self.opacities, t.clamp(0.0, 1.0), |a, b, f| a + (b - a) * f as f32).unwrap_or(1.0)
    }
}

fn sample_stops<T: Copy>(stops: &[(f64, T)], t: f64, lerp: impl Fn(T, T, f64) -> T) -> Option<T> {
    let first = stops.first()?;
    if t <= first.0 {
        return Some(first.1);
    }
    for w in stops.windows(2) {
        if t <= w[1].0 {
            let span = (w[1].0 - w[0].0).max(1e-12);
            return Some(lerp(w[0].1, w[1].1, (t - w[0].0) / span));
        }
    }
    stops.last().map(|s| s.1)
}

/// A gradient written the way people (and agents) write it: `{colors: [[pos, color]…],
/// opacities?: [[pos, alpha]…]}`, `[[pos, color]…]` or `[color, color…]` (evenly spaced), where a
/// colour is `"#rrggbb"` or `[r, g, b, a?]` (0–1). Opacity stops default to the colours' alpha.
fn gradient_json(j: &serde_json::Value) -> Option<Gradient> {
    use serde_json::Value as J;
    fn color(c: &J) -> Option<[f32; 4]> {
        if let Some(s) = c.as_str() {
            let c = effectcraft_color::Rgba::from_hex(s)?;
            return Some([c.r, c.g, c.b, c.a]);
        }
        let a: Vec<f32> = c.as_array()?.iter().map(|x| x.as_f64().map(|v| v as f32)).collect::<Option<_>>()?;
        match a.len() {
            3 => Some([a[0], a[1], a[2], 1.0]),
            4 => Some([a[0], a[1], a[2], a[3]]),
            _ => None,
        }
    }
    fn stops(a: &[J]) -> Option<Vec<(f64, [f32; 4])>> {
        let n = a.len();
        if n < 2 {
            return None;
        }
        a.iter()
            .enumerate()
            .map(|(i, s)| match s.as_array() {
                Some(p) if p.len() == 2 && p[0].is_number() => Some((p[0].as_f64()?, color(&p[1])?)),
                _ => Some((i as f64 / (n - 1) as f64, color(s)?)),
            })
            .collect()
    }
    let (colors, opacities) = match j {
        J::Array(a) => (stops(a)?, None),
        J::Object(o) => {
            let colors = stops(o.get("colors")?.as_array()?)?;
            let op = match o.get("opacities") {
                Some(J::Array(a)) => Some(
                    a.iter()
                        .map(|s| {
                            let p = s.as_array()?;
                            Some((p.first()?.as_f64()?, p.get(1)?.as_f64()? as f32))
                        })
                        .collect::<Option<Vec<_>>>()?,
                ),
                _ => None,
            };
            (colors, op)
        }
        _ => return None,
    };
    let mut colors = colors;
    colors.sort_by(|a, b| a.0.total_cmp(&b.0));
    let opacities = opacities.unwrap_or_else(|| colors.iter().map(|(p, c)| (*p, c[3])).collect());
    Some(Gradient { colors: colors.into_iter().map(|(p, c)| (p, [c[0], c[1], c[2], 1.0])).collect(), opacities })
}

/// The value of a property.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", content = "v")]
pub enum Value {
    Scalar(f64),
    Vec2([f64; 2]),
    Vec3([f64; 3]),
    /// Straight RGBA 0..1.
    Color([f64; 4]),
    Bool(bool),
    /// Index into a popup's options (1-based like AE's dropdown values is a UI concern; this is 0-based).
    Enum(u32),
    Path(ShapePath),
    Text(Box<TextDoc>),
    /// Layer reference (layer id), e.g. Layer Control effects, track matte sources.
    Layer(Option<u64>),
    Gradient(Gradient),
    Str(String),
}

impl Value {
    /// Numeric components (empty for non-numeric values).
    pub fn components(&self) -> Vec<f64> {
        match self {
            Value::Scalar(v) => vec![*v],
            Value::Vec2(v) => v.to_vec(),
            Value::Vec3(v) => v.to_vec(),
            Value::Color(v) => v.to_vec(),
            Value::Bool(b) => vec![if *b { 1.0 } else { 0.0 }],
            Value::Enum(i) => vec![*i as f64],
            _ => vec![],
        }
    }
    pub fn dims(&self) -> usize {
        match self {
            Value::Scalar(_) | Value::Bool(_) | Value::Enum(_) => 1,
            Value::Vec2(_) => 2,
            Value::Vec3(_) => 3,
            Value::Color(_) => 4,
            _ => 0,
        }
    }
    /// Rebuild a value of the same kind from components.
    pub fn with_components(&self, c: &[f64]) -> Value {
        let g = |i: usize| c.get(i).copied().unwrap_or(0.0);
        match self {
            Value::Scalar(_) => Value::Scalar(g(0)),
            Value::Vec2(_) => Value::Vec2([g(0), g(1)]),
            Value::Vec3(_) => Value::Vec3([g(0), g(1), g(2)]),
            Value::Color(_) => Value::Color([g(0), g(1), g(2), g(3)]),
            Value::Bool(_) => Value::Bool(g(0) >= 0.5),
            Value::Enum(_) => Value::Enum(g(0).round().max(0.0) as u32),
            v => v.clone(),
        }
    }
    /// Can be interpolated (otherwise keyframes behave as hold).
    pub fn interpolates(&self) -> bool {
        matches!(self, Value::Scalar(_) | Value::Vec2(_) | Value::Vec3(_) | Value::Color(_) | Value::Path(_) | Value::Gradient(_))
    }
    pub fn lerp(&self, o: &Value, t: f64) -> Value {
        match (self, o) {
            (Value::Path(a), Value::Path(b)) => Value::Path(a.lerp(b, t)),
            (Value::Gradient(a), Value::Gradient(b)) if a.colors.len() == b.colors.len() && a.opacities.len() == b.opacities.len() => {
                let colors = a
                    .colors
                    .iter()
                    .zip(&b.colors)
                    .map(|(x, y)| {
                        let mut c = [0.0; 4];
                        for i in 0..4 {
                            c[i] = x.1[i] + (y.1[i] - x.1[i]) * t as f32;
                        }
                        (x.0 + (y.0 - x.0) * t, c)
                    })
                    .collect();
                let opacities = a.opacities.iter().zip(&b.opacities).map(|(x, y)| (x.0 + (y.0 - x.0) * t, x.1 + (y.1 - x.1) * t as f32)).collect();
                Value::Gradient(Gradient { colors, opacities })
            }
            _ if self.interpolates() && self.dims() > 0 && self.dims() == o.dims() => {
                let a = self.components();
                let b = o.components();
                self.with_components(&a.iter().zip(&b).map(|(x, y)| x + (y - x) * t).collect::<Vec<_>>())
            }
            _ => {
                if t < 1.0 {
                    self.clone()
                } else {
                    o.clone()
                }
            }
        }
    }
    pub fn as_f64(&self) -> f64 {
        self.components().first().copied().unwrap_or(0.0)
    }
    pub fn as_vec2(&self) -> [f64; 2] {
        let c = self.components();
        [c.first().copied().unwrap_or(0.0), c.get(1).copied().unwrap_or(0.0)]
    }
    pub fn as_vec3(&self) -> [f64; 3] {
        let c = self.components();
        [c.first().copied().unwrap_or(0.0), c.get(1).copied().unwrap_or(0.0), c.get(2).copied().unwrap_or(0.0)]
    }
    pub fn as_color(&self) -> [f32; 4] {
        let c = self.components();
        let g = |i: usize, d: f64| c.get(i).copied().unwrap_or(d) as f32;
        [g(0, 0.0), g(1, 0.0), g(2, 0.0), g(3, 1.0)]
    }
    pub fn as_bool(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            v => v.as_f64() != 0.0,
        }
    }
    pub fn as_enum(&self) -> u32 {
        match self {
            Value::Enum(i) => *i,
            v => v.as_f64().max(0.0) as u32,
        }
    }
    pub fn as_path(&self) -> Option<&ShapePath> {
        if let Value::Path(p) = self { Some(p) } else { None }
    }
    pub fn as_text(&self) -> Option<&TextDoc> {
        if let Value::Text(t) = self { Some(t) } else { None }
    }
    pub fn as_layer(&self) -> Option<u64> {
        if let Value::Layer(l) = self { *l } else { None }
    }
    pub fn kind_name(&self) -> &'static str {
        match self {
            Value::Scalar(_) => "scalar",
            Value::Vec2(_) => "vec2",
            Value::Vec3(_) => "vec3",
            Value::Color(_) => "color",
            Value::Bool(_) => "bool",
            Value::Enum(_) => "enum",
            Value::Path(_) => "path",
            Value::Text(_) => "text",
            Value::Layer(_) => "layer",
            Value::Gradient(_) => "gradient",
            Value::Str(_) => "string",
        }
    }
    /// Lenient conversion from JSON (numbers, arrays, booleans, strings) to a value shaped like `self`.
    pub fn coerce_json(&self, j: &serde_json::Value) -> Option<Value> {
        use serde_json::Value as J;
        let nums = |j: &J| -> Option<Vec<f64>> {
            match j {
                J::Number(n) => n.as_f64().map(|v| vec![v]),
                J::Array(a) => a.iter().map(J::as_f64).collect(),
                J::Bool(b) => Some(vec![if *b { 1.0 } else { 0.0 }]),
                _ => None,
            }
        };
        match self {
            Value::Scalar(_) | Value::Vec2(_) | Value::Vec3(_) | Value::Enum(_) => {
                let mut c = nums(j)?;
                if c.len() == 1 && self.dims() > 1 {
                    c = vec![c[0]; self.dims()];
                }
                Some(self.with_components(&c))
            }
            Value::Color(_) => {
                if let Some(s) = j.as_str() {
                    let c = effectcraft_color::Rgba::from_hex(s)?;
                    return Some(Value::Color([c.r as f64, c.g as f64, c.b as f64, c.a as f64]));
                }
                let mut c = nums(j)?;
                if c.len() == 3 {
                    c.push(1.0);
                }
                Some(self.with_components(&c))
            }
            Value::Bool(_) => j.as_bool().or_else(|| j.as_f64().map(|v| v != 0.0)).map(Value::Bool),
            Value::Str(_) => j.as_str().map(|s| Value::Str(s.to_string())),
            Value::Text(t) => {
                if let Some(s) = j.as_str() {
                    let mut t = (**t).clone();
                    t.set_text(s);
                    Some(Value::Text(Box::new(t)))
                } else {
                    serde_json::from_value::<TextDoc>(j.clone()).ok().map(|mut t| {
                        t.normalize();
                        Value::Text(Box::new(t))
                    })
                }
            }
            Value::Layer(_) => Some(Value::Layer(j.as_u64())),
            Value::Path(_) => serde_json::from_value::<ShapePath>(j.clone()).ok().map(Value::Path).or_else(|| serde_json::from_value::<Value>(j.clone()).ok()),
            Value::Gradient(_) => {
                serde_json::from_value::<Value>(j.clone()).ok().filter(|v| matches!(v, Value::Gradient(_))).or_else(|| gradient_json(j).map(Value::Gradient))
            }
        }
    }
    /// JSON for display/automation (numbers/arrays for numeric kinds).
    pub fn to_json(&self) -> serde_json::Value {
        use serde_json::json;
        match self {
            Value::Scalar(v) => json!(v),
            Value::Vec2(v) => json!(v),
            Value::Vec3(v) => json!(v),
            Value::Color(v) => json!(v),
            Value::Bool(b) => json!(b),
            Value::Enum(i) => json!(i),
            Value::Str(s) => json!(s),
            Value::Layer(l) => json!(l),
            Value::Text(t) => json!(t.text),
            v => serde_json::to_value(v).unwrap_or_default(),
        }
    }
}
