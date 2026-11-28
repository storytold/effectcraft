//! The 3D system shared by Card Dance, Shatter and Card Wipe: Camera System (Camera Position,
//! Corner Pins, Comp Camera), Lighting (Distant Source, Point Source, First Comp Light) and
//! Material (diffuse, specular, highlight sharpness).
//!
//! Pieces live in "buffer world" space: x, y in buffer pixels, z in buffer pixels away from the
//! viewer (the layer plane is z = 0). A [`Proj`] maps buffer-world points to buffer pixels
//! (homogeneous), so every camera system reduces to one 3×4 matrix. CC Particle World sees its
//! particles through the comp camera with the same [`comp_proj`].

use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;

use crate::sim::pt;
use crate::{Buf, EffectCtx, ParamSpec, col, num, p, popup, slider};

pub(crate) type M3 = [[f64; 3]; 3];

/// Camera System options.
pub const CAMERA_SYSTEMS: [&str; 3] = ["Camera Position", "Corner Pins", "Comp Camera"];
/// Transform Order options (rotation order, and whether the camera position applies before or
/// after the rotation).
pub const TRANSFORM_ORDERS: [&str; 12] = [
    "Rotate XYZ, Position",
    "Rotate XZY, Position",
    "Rotate YXZ, Position",
    "Rotate YZX, Position",
    "Rotate ZXY, Position",
    "Rotate ZYX, Position",
    "Position, Rotate XYZ",
    "Position, Rotate XZY",
    "Position, Rotate YXZ",
    "Position, Rotate YZX",
    "Position, Rotate ZXY",
    "Position, Rotate ZYX",
];
/// Light Type options.
pub const LIGHT_TYPES: [&str; 3] = ["Distant Source", "Point Source", "First Comp Light"];

/// Camera System, Camera Position and Corner Pins parameters. `z` is the default camera
/// distance (Z Position, in layer sizes at the 70 mm-equivalent Focal Length).
pub(crate) fn camera_params(z: f64) -> Vec<ParamSpec> {
    vec![
        p("cameraSystem", "Camera System", Value::Enum(0), popup(&CAMERA_SYSTEMS)),
        p("cameraPosition/xRotation", "X Rotation", num(0.0), ParamUi::Angle),
        p("cameraPosition/yRotation", "Y Rotation", num(0.0), ParamUi::Angle),
        p("cameraPosition/zRotation", "Z Rotation", num(0.0), ParamUi::Angle),
        p("cameraPosition/xyPosition", "X, Y Position", pt(0.5, 0.5), ParamUi::Point),
        p("cameraPosition/cameraZ", "Z Position", num(z), slider(0.05, 10.0, 0.5, 5.0, 2)),
        p("cameraPosition/focalLength", "Focal Length", num(70.0), slider(1.0, 500.0, 10.0, 200.0, 1)),
        p("cameraPosition/transformOrder", "Transform Order", Value::Enum(0), popup(&TRANSFORM_ORDERS)),
        p("cornerPins/upperLeftCorner", "Upper Left Corner", pt(0.0, 0.0), ParamUi::Point),
        p("cornerPins/upperRightCorner", "Upper Right Corner", pt(1.0, 0.0), ParamUi::Point),
        p("cornerPins/lowerLeftCorner", "Lower Left Corner", pt(0.0, 1.0), ParamUi::Point),
        p("cornerPins/lowerRightCorner", "Lower Right Corner", pt(1.0, 1.0), ParamUi::Point),
        p("cornerPins/autoFocalLength", "Auto Focal Length", Value::Bool(true), ParamUi::Checkbox),
        p("cornerPins/focalLength", "Focal Length", num(70.0), slider(1.0, 500.0, 10.0, 200.0, 1)),
    ]
}

/// Lighting parameters.
pub(crate) fn lighting_params(intensity: f64, ambient: f64) -> Vec<ParamSpec> {
    vec![
        p("lighting/lightType", "Light Type", Value::Enum(0), popup(&LIGHT_TYPES)),
        p("lighting/lightIntensity", "Light Intensity", num(intensity), slider(0.0, 4.0, 0.0, 2.0, 2)),
        p("lighting/lightColor", "Light Color", col(1.0, 1.0, 1.0), ParamUi::Color),
        p("lighting/lightPosition", "Light Position", pt(0.5, 0.5), ParamUi::Point),
        p("lighting/lightDepth", "Light Depth", num(1.0), slider(-10.0, 10.0, 0.0, 5.0, 3)),
        p("lighting/ambientLight", "Ambient Light", num(ambient), slider(0.0, 2.0, 0.0, 1.0, 2)),
    ]
}

/// Material parameters. Specular defaults to 0 so pieces at rest reproduce the layer.
pub(crate) fn material_params(diffuse: f64) -> Vec<ParamSpec> {
    vec![
        p("material/diffuse", "Diffuse Reflection", num(diffuse), slider(0.0, 2.0, 0.0, 1.0, 2)),
        p("material/specular", "Specular Reflection", num(0.0), slider(0.0, 2.0, 0.0, 1.0, 2)),
        p("material/highlightSharpness", "Highlight Sharpness", num(15.0), slider(1.0, 100.0, 1.0, 100.0, 1)),
    ]
}

/// Whether the camera / lighting parameter `param` is shown for the current choices.
pub(crate) fn shown(param: &str, e: &dyn Fn(&str) -> u32) -> Option<bool> {
    if param == "cameraPosition" || param.starts_with("cameraPosition/") {
        return Some(e("cameraSystem") == 0);
    }
    if param == "cornerPins" || param.starts_with("cornerPins/") {
        return Some(e("cameraSystem") == 1);
    }
    if param == "lighting/lightPosition" || param == "lighting/lightDepth" {
        return Some(e("lighting/lightType") != 2);
    }
    None
}

pub(crate) fn mat_mul(a: &M3, b: &M3) -> M3 {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}

fn rx(a: f64) -> M3 {
    let (s, c) = a.sin_cos();
    [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]]
}
fn ry(a: f64) -> M3 {
    let (s, c) = a.sin_cos();
    [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]]
}
fn rz(a: f64) -> M3 {
    let (s, c) = a.sin_cos();
    [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]]
}

/// Rotation applying the axes in `order` (0 XYZ, 1 XZY, 2 YXZ, 3 YZX, 4 ZXY, 5 ZYX; the first
/// letter is applied first). Angles in radians.
pub(crate) fn rot_ordered(x: f64, y: f64, z: f64, order: u32) -> M3 {
    let (mx, my, mz) = (rx(x), ry(y), rz(z));
    let seq: [&M3; 3] = match order % 6 {
        1 => [&mx, &mz, &my],
        2 => [&my, &mx, &mz],
        3 => [&my, &mz, &mx],
        4 => [&mz, &mx, &my],
        5 => [&mz, &my, &mx],
        _ => [&mx, &my, &mz],
    };
    mat_mul(seq[2], &mat_mul(seq[1], seq[0]))
}

/// A camera: buffer-world → buffer pixels (homogeneous rows X, Y, W; W is the depth in front
/// of the camera, in the units `near` uses).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Proj {
    pub m: [[f64; 4]; 3],
    /// Points with W below this are behind / too close to the camera.
    pub near: f64,
}

impl Proj {
    /// Looking down +z at the layer plane from `d` pixels in front of buffer point `c`.
    pub fn simple(c: [f64; 2], d: f64) -> Proj {
        Proj { m: [[d, 0.0, c[0], 0.0], [0.0, d, c[1], 0.0], [0.0, 0.0, 1.0, d]], near: 0.05 * d }
    }

    /// `self` after the affine scene transform `p ↦ a·p + t`.
    fn after(&self, a: &M3, t: [f64; 3]) -> Proj {
        let mut m = [[0.0; 4]; 3];
        for i in 0..3 {
            for j in 0..3 {
                m[i][j] = (0..3).map(|k| self.m[i][k] * a[k][j]).sum();
            }
            m[i][3] = (0..3).map(|k| self.m[i][k] * t[k]).sum::<f64>() + self.m[i][3];
        }
        Proj { m, near: self.near }
    }

    /// A screen-space homography applied after `self`.
    fn then(&self, h: &M3) -> Proj {
        let mut m = [[0.0; 4]; 3];
        for i in 0..3 {
            for j in 0..4 {
                m[i][j] = (0..3).map(|k| h[i][k] * self.m[k][j]).sum();
            }
        }
        // Keep W's scale (and so `near`) comparable when H rescales it.
        Proj { m, near: self.near * h[2][2].abs().max(1e-9) }
    }

    /// Project a buffer-world point (None behind the camera).
    #[cfg(test)]
    pub fn project(&self, p: [f64; 3]) -> Option<[f64; 2]> {
        let r = |i: usize| self.m[i][0] * p[0] + self.m[i][1] * p[1] + self.m[i][2] * p[2] + self.m[i][3];
        let w = r(2);
        (w > self.near).then(|| [r(0) / w, r(1) / w])
    }

    /// How a buffer-world point `p` moving by `v` is seen: its buffer pixel, its depth W, the
    /// buffer pixels per buffer-world pixel of a camera-facing size there, and its velocity in
    /// buffer pixels. `None` behind or too close to the camera, or for a degenerate camera.
    pub fn view(&self, p: [f64; 3], v: [f64; 3]) -> Option<Seen> {
        let row = |i: usize| [self.m[i][0], self.m[i][1], self.m[i][2]];
        let (m0, m1, m2) = (row(0), row(1), row(2));
        let w = dot(m2, p) + self.m[2][3];
        if !(w > self.near && w.is_finite()) {
            return None;
        }
        let at = [(dot(m0, p) + self.m[0][3]) / w, (dot(m1, p) + self.m[1][3]) / w];
        // The focal length (pixels per unit at unit depth): |m_i × m2| / |m2|² for K·[R|t]
        // scaled by |m2|; the geometric mean of the two image axes.
        let l2 = dot(m2, m2);
        let scale = (len(cross(m0, m2)) * len(cross(m1, m2))).sqrt() / (l2.sqrt() * w);
        if !(scale > 0.0 && scale.is_finite() && at.iter().all(|a| a.is_finite())) {
            return None;
        }
        // d(X / W) = (m_i − at_i · m2) · dp / W.
        let vel = [(dot(m0, v) - at[0] * dot(m2, v)) / w, (dot(m1, v) - at[1] * dot(m2, v)) / w];
        Some(Seen { at, depth: w, scale, vel })
    }

    /// The camera's centre in buffer-world space (where the projection collapses), or a point
    /// far in front of the layer when it has none (orthographic).
    pub fn eye(&self) -> [f64; 3] {
        let a: M3 = std::array::from_fn(|i| [self.m[i][0], self.m[i][1], self.m[i][2]]);
        match inv3(&a) {
            Some(ai) => {
                let t = [-self.m[0][3], -self.m[1][3], -self.m[2][3]];
                std::array::from_fn(|i| (0..3).map(|k| ai[i][k] * t[k]).sum())
            }
            None => [0.0, 0.0, -1e9],
        }
    }
}

/// A point seen through a [`Proj`] (see [`Proj::view`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Seen {
    pub at: [f64; 2],
    pub depth: f64,
    pub scale: f64,
    pub vel: [f64; 2],
}

pub(crate) fn inv3(m: &M3) -> Option<M3> {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-12 {
        return None;
    }
    let d = 1.0 / det;
    Some([
        [(m[1][1] * m[2][2] - m[1][2] * m[2][1]) * d, (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * d, (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * d],
        [(m[1][2] * m[2][0] - m[1][0] * m[2][2]) * d, (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * d, (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * d],
        [(m[1][0] * m[2][1] - m[1][1] * m[2][0]) * d, (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * d, (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * d],
    ])
}

/// Homography taking the unit square's corners (0,0) (1,0) (1,1) (0,1) to `q`.
fn square_to_quad(q: [[f64; 2]; 4]) -> M3 {
    let v = |p: [f64; 2]| effectcraft_geom::Vec2 { x: p[0], y: p[1] };
    effectcraft_geom::Mat3::square_to_quad([v(q[0]), v(q[1]), v(q[2]), v(q[3])]).0
}

/// The camera an effect renders through, from its Camera System parameters.
pub(crate) fn projection(ctx: &EffectCtx, b: &Buf) -> Proj {
    let pr = ctx.params;
    let [lw, lh] = ctx.layer_size;
    let s = b.scale;
    let c = [b.offset[0] + lw * 0.5 * s, b.offset[1] + lh * 0.5 * s];
    let size = lw.max(lh) * s;
    let f = |id: &str, d: f64| pr.get(id).map(Value::as_f64).unwrap_or(d);
    match pr.e("cameraSystem") {
        1 => {
            // Corner Pins: the layer's corners land on the four pins; the focal length sets how
            // strongly depth (pieces leaving the plane) is foreshortened.
            let focal = if pr.get("cornerPins/autoFocalLength").is_none_or(|v| v.as_bool()) { 70.0 } else { f("cornerPins/focalLength", 70.0).max(1.0) };
            let base = Proj::simple(c, size * 2.0 * focal / 70.0);
            let pin = |id: &str| {
                let (x, y) = b.to_px(pr.v2(id));
                [x, y]
            };
            let quad =
                [pin("cornerPins/upperLeftCorner"), pin("cornerPins/upperRightCorner"), pin("cornerPins/lowerRightCorner"), pin("cornerPins/lowerLeftCorner")];
            let rect = [
                [b.offset[0], b.offset[1]],
                [b.offset[0] + lw * s, b.offset[1]],
                [b.offset[0] + lw * s, b.offset[1] + lh * s],
                [b.offset[0], b.offset[1] + lh * s],
            ];
            match inv3(&square_to_quad(rect)) {
                Some(ri) => base.then(&mat_mul(&square_to_quad(quad), &ri)),
                None => base,
            }
        }
        2 => match ctx.env.host.and_then(|h| h.comp_scene()).and_then(|sc| sc.camera) {
            Some(cam) => comp_proj(&cam, c, b),
            None => Proj::simple(c, size * 2.0),
        },
        _ => {
            let zpos = f("cameraPosition/cameraZ", 2.0).max(0.05);
            let focal = f("cameraPosition/focalLength", 70.0).max(1.0);
            let base = Proj::simple(c, size * zpos * focal / 140.0);
            let order = pr.e("cameraPosition/transformOrder");
            let r = rot_ordered(
                f("cameraPosition/xRotation", 0.0).to_radians(),
                f("cameraPosition/yRotation", 0.0).to_radians(),
                f("cameraPosition/zRotation", 0.0).to_radians(),
                order,
            );
            let xy = pr.get("cameraPosition/xyPosition").map(|v| v.as_vec2()).unwrap_or([lw * 0.5, lh * 0.5]);
            let (px, py) = b.to_px(xy);
            let shift = [px - c[0], py - c[1], 0.0];
            // Rotate about the layer centre, then move by the X, Y Position offset (or the
            // other way round).
            let c3 = [c[0], c[1], 0.0];
            let rc: [f64; 3] = std::array::from_fn(|i| c3[i] - (0..3).map(|k| r[i][k] * c3[k]).sum::<f64>());
            let t = if order >= 6 {
                // p ↦ R(p + shift − c) + c
                std::array::from_fn(|i| rc[i] + (0..3).map(|k| r[i][k] * shift[k]).sum::<f64>())
            } else {
                std::array::from_fn(|i| rc[i] + shift[i])
            };
            base.after(&r, t)
        }
    }
}

/// The comp camera `cam` ([`CompScene::camera`], layer px → layer px) looking at buffer `b`,
/// whose layer centre is buffer pixel `c`.
pub(crate) fn comp_proj(cam: &[[f64; 4]; 3], c: [f64; 2], b: &Buf) -> Proj {
    let s = b.scale;
    // buffer world → layer: (p − offset) / s; layer → buffer: · s + offset.
    let mut m = [[0.0; 4]; 3];
    for i in 0..3 {
        for j in 0..3 {
            m[i][j] = cam[i][j] / s;
        }
        m[i][3] = cam[i][3] - (cam[i][0] * b.offset[0] + cam[i][1] * b.offset[1]) / s;
    }
    let back: M3 = [[s, 0.0, b.offset[0]], [0.0, s, b.offset[1]], [0.0, 0.0, 1.0]];
    let p = Proj { m, near: 1e-6 }.then(&back);
    // W of the layer centre sets the scale of `near`.
    let wc = p.m[2][0] * c[0] + p.m[2][1] * c[1] + p.m[2][3];
    Proj { near: (wc.abs() * 0.02).max(1e-9), ..p }
}

/// The comp's first light as the effect sees it (layer pixels), from [`crate::EffectHost::comp_scene`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CompLight {
    /// Position in layer pixels (z away from the viewer).
    pub pos: [f64; 3],
    /// Direction the light travels (parallel / spot lights), unit, layer space.
    pub dir: [f64; 3],
    /// Colour × intensity (1 = 100 %).
    pub color: [f32; 3],
    /// 0 parallel, 1 point / spot, 2 ambient.
    pub kind: u8,
}

/// The comp's camera and lights relative to the effect's layer.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CompScene {
    /// Effect-space pixels (x, y, z) → effect-space pixels, homogeneous rows X, Y, W: the comp
    /// camera's view of the layer brought back into the layer's own pixel grid (identity on the
    /// layer plane for the default camera).
    pub camera: Option<[[f64; 4]; 3]>,
    /// `camera` is an active camera layer's, not the comp's default view (CC Particle World
    /// follows only a camera layer).
    pub camera_layer: bool,
    pub light: Option<CompLight>,
    /// The camera's zoom (comp pixels): with `camera`'s W the depth in front of it, a length at
    /// that depth shows `zoom / W` times as long. 0 when unknown. W is the true depth for a 2D
    /// layer and for a 3D layer facing the camera; across a tilted 3D layer's plane it is the
    /// anchor's depth.
    pub zoom: f64,
    /// The camera's depth of field when it is on: focus distance (comp pixels from the eye),
    /// aperture (comp pixels) and Blur Level (1 = 100 %).
    pub dof: Option<[f64; 3]>,
}

impl CompScene {
    /// The scene measured from `origin` (in its current coordinates, as [`crate::Buf::rebase`]):
    /// a point `p` becomes `p − origin`.
    pub fn rebased(mut self, origin: [f64; 2]) -> CompScene {
        if let Some(c) = &mut self.camera {
            for row in c.iter_mut() {
                row[3] += row[0] * origin[0] + row[1] * origin[1];
            }
            let w = c[2];
            for (row, o) in c.iter_mut().zip(origin) {
                for (v, wv) in row.iter_mut().zip(w) {
                    *v -= o * wv;
                }
            }
        }
        if let Some(l) = &mut self.light {
            l.pos[0] -= origin[0];
            l.pos[1] -= origin[1];
        }
        self
    }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn len(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}
fn norm(a: [f64; 3]) -> [f64; 3] {
    let l = dot(a, a).sqrt();
    if l < 1e-12 { [0.0, 0.0, -1.0] } else { [a[0] / l, a[1] / l, a[2] / l] }
}

/// Lighting and Material, ready to shade pieces.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Lighting {
    /// 0 distant, 1 point, 2 ambient only.
    kind: u8,
    color: [f32; 3],
    pos: [f64; 3],
    /// Distant light: direction towards the light.
    to_light: [f64; 3],
    ambient: f32,
    kd: f32,
    ks: f32,
    sharp: f32,
}

impl Lighting {
    pub fn from(ctx: &EffectCtx, b: &Buf) -> Lighting {
        let pr = ctx.params;
        let f = |id: &str, d: f64| pr.get(id).map(Value::as_f64).unwrap_or(d);
        let [lw, lh] = ctx.layer_size;
        let s = b.scale;
        let centre = [b.offset[0] + lw * 0.5 * s, b.offset[1] + lh * 0.5 * s, 0.0];
        let intensity = f("lighting/lightIntensity", 1.0) as f32;
        let lc = pr.get("lighting/lightColor").map(|v| v.as_color()).unwrap_or([1.0; 4]);
        let mut color = [lc[0] * intensity, lc[1] * intensity, lc[2] * intensity];
        let lp = pr.get("lighting/lightPosition").map(|v| v.as_vec2()).unwrap_or([lw * 0.5, lh * 0.5]);
        let (x, y) = b.to_px(lp);
        let mut pos = [x, y, -f("lighting/lightDepth", 1.0) * lw.max(lh) * s];
        let mut kind = if pr.e("lighting/lightType") == 1 { 1 } else { 0 };
        let mut to_light = norm(sub(pos, centre));
        if pr.e("lighting/lightType") == 2 {
            match ctx.env.host.and_then(|h| h.comp_scene()).and_then(|sc| sc.light) {
                Some(l) => {
                    color = [l.color[0] * intensity, l.color[1] * intensity, l.color[2] * intensity];
                    pos = [l.pos[0] * s + b.offset[0], l.pos[1] * s + b.offset[1], l.pos[2] * s];
                    kind = l.kind;
                    to_light = norm([-l.dir[0], -l.dir[1], -l.dir[2]]);
                }
                // No light in the comp: ambient only.
                None => kind = 2,
            }
        }
        Lighting {
            kind,
            color,
            pos,
            to_light,
            ambient: f("lighting/ambientLight", 0.25) as f32,
            kd: f("material/diffuse", 0.75) as f32,
            ks: f("material/specular", 0.0) as f32,
            sharp: f("material/highlightSharpness", 15.0).max(1.0) as f32,
        }
    }

    /// Multiplier and additive highlight for a piece with unit normal `n` (rotated front normal)
    /// at `at`, seen from `eye`. Pieces are lit on whichever side faces the viewer.
    pub fn shade(&self, n: [f64; 3], at: [f64; 3], eye: [f64; 3]) -> ([f32; 3], [f32; 3]) {
        let v = norm(sub(eye, at));
        let n = if dot(n, v) < 0.0 { [-n[0], -n[1], -n[2]] } else { n };
        if self.kind == 2 {
            let a = self.ambient;
            return ([a + self.kd * self.color[0], a + self.kd * self.color[1], a + self.kd * self.color[2]], [0.0; 3]);
        }
        let l = if self.kind == 1 { norm(sub(self.pos, at)) } else { self.to_light };
        let ndl = dot(n, l).max(0.0) as f32;
        let h = norm([l[0] + v[0], l[1] + v[1], l[2] + v[2]]);
        let spec = if self.ks > 0.0 && ndl > 0.0 { self.ks * (dot(n, h).max(0.0) as f32).powf(self.sharp) } else { 0.0 };
        (std::array::from_fn(|k| self.ambient + self.kd * ndl * self.color[k]), std::array::from_fn(|k| spec * self.color[k]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_camera_keeps_the_layer_plane() {
        let p = Proj::simple([10.0, 20.0], 100.0);
        assert_eq!(p.project([3.0, 4.0, 0.0]), Some([3.0, 4.0]));
        let e = p.eye();
        assert!((e[0] - 10.0).abs() < 1e-9 && (e[1] - 20.0).abs() < 1e-9 && (e[2] + 100.0).abs() < 1e-9);
        // Further away looks smaller (towards the centre).
        let q = p.project([30.0, 20.0, 100.0]).unwrap();
        assert!((q[0] - 20.0).abs() < 1e-9);
    }

    #[test]
    fn rotation_orders_differ() {
        let a = rot_ordered(0.5, 0.3, 0.2, 0);
        let b = rot_ordered(0.5, 0.3, 0.2, 5);
        assert!((0..3).any(|i| (0..3).any(|j| (a[i][j] - b[i][j]).abs() > 1e-3)));
        let z = rot_ordered(0.0, 0.0, 0.0, 3);
        assert_eq!(z, [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
    }
}
