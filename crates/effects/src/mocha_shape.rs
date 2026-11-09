//! Matte ▸ Tracked Shape Matte: renders planar-tracked roto shapes as a matte.
//!
//! Mocha's own clipboard / export formats are not publicly specified, so the effect reads shape
//! data in a small documented JSON format of ours (in the hidden `shapeData` parameter, either the
//! JSON itself or a path to a `.json` file), which converters or agents can produce:
//!
//! ```json
//! {"shapes": [{"name": "Shape 1", "closed": true, "inverted": false, "feather": 2.0,
//!              "points": [[x, y], …],
//!              "frames": [{"time": 0.0, "points": [[x, y], …]}, {"time": 1.0, "points": […]}]}]}
//! ```
//!
//! Coordinates are layer pixels. `frames` (layer time in seconds, linearly interpolated between
//! keys with the same vertex count, held outside) override the static `points`. Shapes combine
//! with Blend Mode (add / subtract / intersect / difference) into a coverage matte that is used
//! per Render Type: Shape Cutout (layer × matte), Color Composite (shape colour over the layer)
//! or Color Shape Cutout (shape colour inside the matte only).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use rayon::prelude::*;

use crate::util::{Plane, gauss_plane, point_in_poly};
use crate::{Buf, EffectCtx, EffectSpec, col, num, p, popup, slider};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MochaShape {
    pub name: String,
    pub closed: bool,
    pub inverted: bool,
    pub feather: f64,
    pub points: Vec<[f64; 2]>,
    /// (layer time, points), sorted by time.
    pub frames: Vec<(f64, Vec<[f64; 2]>)>,
}

impl MochaShape {
    /// Outline at layer time `t`.
    pub fn at(&self, t: f64) -> Vec<[f64; 2]> {
        if self.frames.is_empty() {
            return self.points.clone();
        }
        let i = self.frames.partition_point(|f| f.0 <= t);
        if i == 0 {
            return self.frames[0].1.clone();
        }
        if i == self.frames.len() {
            return self.frames[i - 1].1.clone();
        }
        let (a, b) = (&self.frames[i - 1], &self.frames[i]);
        if a.1.len() != b.1.len() {
            return a.1.clone();
        }
        let u = ((t - a.0) / (b.0 - a.0).max(1e-12)).clamp(0.0, 1.0);
        a.1.iter().zip(&b.1).map(|(p, q)| [p[0] + (q[0] - p[0]) * u, p[1] + (q[1] - p[1]) * u]).collect()
    }
}

fn pts(v: &serde_json::Value) -> Vec<[f64; 2]> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|p| {
                    let p = p.as_array()?;
                    Some([p.first()?.as_f64()?, p.get(1)?.as_f64()?])
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parse shape data JSON (see the module docs). `None` when it is not valid.
pub fn parse_shapes(text: &str) -> Option<Vec<MochaShape>> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let list = v.get("shapes").and_then(|s| s.as_array()).or_else(|| v.as_array())?;
    let mut out = vec![];
    for s in list {
        let mut frames: Vec<(f64, Vec<[f64; 2]>)> = s
            .get("frames")
            .and_then(|f| f.as_array())
            .map(|a| a.iter().filter_map(|f| Some((f.get("time")?.as_f64()?, pts(f.get("points")?)))).collect())
            .unwrap_or_default();
        frames.sort_by(|a, b| a.0.total_cmp(&b.0));
        out.push(MochaShape {
            name: s.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string(),
            closed: s.get("closed").and_then(|c| c.as_bool()).unwrap_or(true),
            inverted: s.get("inverted").and_then(|c| c.as_bool()).unwrap_or(false),
            feather: s.get("feather").and_then(|c| c.as_f64()).unwrap_or(0.0).max(0.0),
            points: s.get("points").map(pts).unwrap_or_default(),
            frames,
        });
    }
    Some(out)
}

type ShapeCache = Mutex<HashMap<String, Option<Arc<Vec<MochaShape>>>>>;

fn load(src: &str) -> Option<Arc<Vec<MochaShape>>> {
    let s = src.trim();
    if s.is_empty() {
        return None;
    }
    static CACHE: OnceLock<ShapeCache> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(v) = cache.lock().ok().and_then(|c| c.get(src).cloned()) {
        return v;
    }
    let parsed =
        if s.starts_with('{') || s.starts_with('[') { parse_shapes(s) } else { std::fs::read_to_string(s).ok().and_then(|t| parse_shapes(&t)) }.map(Arc::new);
    if let Ok(mut c) = cache.lock() {
        if c.len() > 64 {
            c.clear();
        }
        c.insert(src.to_string(), parsed.clone());
    }
    parsed
}

fn mocha_shape(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let Some(shapes) = load(ctx.params.s("shapeData")) else { return b };
    let (w, h) = (b.img.width as usize, b.img.height as usize);
    let blend = ctx.params.e("blendMode");
    let inv = 1.0 / b.scale.max(1e-9);
    let mut matte: Option<Plane> = None;
    for s in shapes.iter() {
        let outline = s.at(ctx.time);
        if outline.len() < 3 || !s.closed {
            continue;
        }
        let mut cov = Plane::new(w, h);
        cov.data.par_chunks_mut(w.max(1)).enumerate().for_each(|(y, row)| {
            for (x, v) in row.iter_mut().enumerate() {
                let lx = (x as f64 + 0.5 - b.offset[0]) * inv;
                let ly = (y as f64 + 0.5 - b.offset[1]) * inv;
                *v = if point_in_poly(&outline, lx, ly) != s.inverted { 1.0 } else { 0.0 };
            }
        });
        if s.feather > 0.0 {
            let f = s.feather * b.scale * 0.5;
            cov = gauss_plane(&cov, f, f);
        }
        matte = Some(match matte {
            None => cov,
            Some(m) => m.zip_map(&cov, |a, c| match blend {
                1 => a * (1.0 - c),
                2 => a * c,
                3 => (a - c).abs(),
                _ => a + c - a * c,
            }),
        });
    }
    let Some(mut m) = matte else { return b };
    if ctx.params.b("invert") {
        m = m.map(|v| 1.0 - v);
    }
    let kind = ctx.params.e("renderType");
    let c = ctx.params.color("shapeColour");
    let op = (ctx.params.f("opacity") / 100.0) as f32;
    let edge = ctx.params.b("renderEdgeWidth");
    b.img.data.par_iter_mut().zip(m.data.par_iter()).for_each(|(px, &k)| {
        let k = if edge { (1.0 - (2.0 * k - 1.0).abs()).clamp(0.0, 1.0) } else { k.clamp(0.0, 1.0) };
        match kind {
            1 => {
                let a = k * op;
                let s = [c[0] * a, c[1] * a, c[2] * a, a];
                *px = [s[0] + px[0] * (1.0 - a), s[1] + px[1] * (1.0 - a), s[2] + px[2] * (1.0 - a), s[3] + px[3] * (1.0 - a)];
            }
            2 => {
                let a = k * op;
                *px = [c[0] * a, c[1] * a, c[2] * a, a];
            }
            _ => {
                let a = 1.0 - (1.0 - k) * op;
                *px = px.map(|v| v * a);
            }
        }
    });
    b
}

pub fn specs() -> Vec<EffectSpec> {
    vec![EffectSpec {
        id: "ec.obsolete.mochashape",
        name: "Tracked Shape Matte",
        category: "Matte",
        params: vec![
            p("blendMode", "Blend mode", Value::Enum(0), popup(&["Add", "Subtract", "Intersect", "Difference"])),
            p("invert", "Invert", Value::Bool(false), ParamUi::Checkbox),
            p("renderEdgeWidth", "Render edge width", Value::Bool(false), ParamUi::Checkbox),
            p("renderType", "Render type", Value::Enum(0), popup(&["Shape cutout", "Color composite", "Color shape cutout"])),
            p("shapeColour", "Shape colour", col(1.0, 1.0, 1.0), ParamUi::Color),
            p("opacity", "Opacity", num(100.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
            // JSON shape data (see the module docs) or a path to a .json file.
            p("shapeData", "Shape data", Value::Str(String::new()), ParamUi::Hidden),
        ],
        render: mocha_shape,
        gpu: false,
        float: true,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EffectEnv, Image, run_fx};

    const DATA: &str = r#"{"shapes":[{"name":"S","closed":true,"points":[[2,2],[10,2],[10,10],[2,10]],
        "frames":[{"time":0,"points":[[2,2],[10,2],[10,10],[2,10]]},{"time":1,"points":[[12,2],[20,2],[20,10],[12,10]]}]}]}"#;

    #[test]
    fn parses_and_interpolates() {
        let s = parse_shapes(DATA).unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].at(0.5)[0], [7.0, 2.0]);
        assert_eq!(s[0].at(5.0)[0], [12.0, 2.0]);
        assert!(parse_shapes("not json").is_none());
    }

    #[test]
    fn cuts_out_the_shape_over_time() {
        let img = Image::filled(24, 12, [1.0, 0.0, 0.0, 1.0]);
        let a = run_fx("ec.obsolete.mochashape", &[("shapeData", Value::Str(DATA.into()))], img.clone(), 0.0, EffectEnv::default());
        assert_eq!(a.img.get(5, 5)[3], 1.0);
        assert_eq!(a.img.get(15, 5)[3], 0.0);
        let b = run_fx("ec.obsolete.mochashape", &[("shapeData", Value::Str(DATA.into()))], img.clone(), 1.0, EffectEnv::default());
        assert_eq!(b.img.get(5, 5)[3], 0.0);
        assert_eq!(b.img.get(15, 5)[3], 1.0);
        // Color shape cutout fills with the shape colour; no data = pass-through.
        let c = run_fx(
            "ec.obsolete.mochashape",
            &[("shapeData", Value::Str(DATA.into())), ("renderType", Value::Enum(2)), ("shapeColour", col(0.0, 1.0, 0.0))],
            img.clone(),
            0.0,
            EffectEnv::default(),
        );
        assert_eq!(c.img.get(5, 5), [0.0, 1.0, 0.0, 1.0]);
        let none = run_fx("ec.obsolete.mochashape", &[], img.clone(), 0.0, EffectEnv::default());
        assert_eq!(none.img.data, img.data);
    }
}
