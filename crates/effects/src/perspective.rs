//! Perspective effects (batch 2): Bevel Alpha, Bevel Edges, Radial Shadow, CC Cylinder,
//! CC Sphere and CC Spotlight.
//!
//! Angles follow the convention used by Drop Shadow: measured clockwise from "up", so a light
//! angle `a` sits in the direction `(sin a, -cos a)` (y grows downwards). 3D vectors use x right,
//! y down, z towards the viewer.

use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use effectcraft_raster::{Image, Px, gaussian_blur};
use rayon::prelude::*;

use crate::util::{Plane, gauss_plane, layer_rect, premul, smoothstep, unpremul};
use crate::{Buf, EffectCtx, EffectSpec, ParamSpec, col, num, p, popup, slider};

fn spec(id: &'static str, name: &'static str, params: Vec<ParamSpec>, render: crate::RenderFn) -> EffectSpec {
    EffectSpec { id, name, category: "Perspective", params, render, gpu: false, float: true }
}

fn norm3(v: [f64; 3]) -> [f64; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-12);
    [v[0] / l, v[1] / l, v[2] / l]
}

fn dot3(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Direction towards a light at `angle_deg` (clockwise from up) and `elevation_deg` above the plane.
fn light_dir(angle_deg: f64, elevation_deg: f64) -> [f64; 3] {
    let (a, e) = (angle_deg.to_radians(), elevation_deg.to_radians());
    norm3([a.sin() * e.cos(), -a.cos() * e.cos(), e.sin()])
}

/// Highlight/shadow shading on a premultiplied pixel: positive `s` adds light colour, negative
/// darkens.
#[inline]
fn apply_shade(px: &mut Px, s: f32, light: [f32; 4]) {
    if s > 0.0 {
        for c in 0..3 {
            px[c] += light[c] * s * px[3];
        }
    } else if s < 0.0 {
        let k = (1.0 + s).max(0.0);
        for c in 0..3 {
            px[c] *= k;
        }
    }
}

fn bevel_alpha(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let t = ctx.params.f("edgeThickness") * b.scale;
    let inten = ctx.params.f("lightIntensity") as f32;
    if t <= 0.0 || inten == 0.0 {
        return b;
    }
    let light = ctx.params.color("lightColor");
    let l = light_dir(ctx.params.f("lightAngle"), 45.0);
    let a = Plane::alpha(&b.img);
    let h = gauss_plane(&a, t * 0.5, t * 0.5);
    let k = t * 1.25;
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            if px[3] <= 0.0 {
                continue;
            }
            let (xi, yi) = (x as i64, y as i64);
            let dx = (h.get_clamped(xi + 1, yi) - h.get_clamped(xi - 1, yi)) as f64 * 0.5;
            let dy = (h.get_clamped(xi, yi + 1) - h.get_clamped(xi, yi - 1)) as f64 * 0.5;
            let n = norm3([-dx * k, -dy * k, 1.0]);
            let s = (dot3(n, l) - l[2]) as f32 * inten * 2.0;
            apply_shade(px, s, light);
        }
    });
    b
}

fn bevel_edges(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let (x0, y0, w, h) = layer_rect(ctx, &b);
    let t = ctx.params.f("edgeThickness").clamp(0.0, 0.5) * w.min(h);
    let inten = ctx.params.f("lightIntensity") as f32;
    if t <= 0.0 || inten == 0.0 {
        return b;
    }
    let light = ctx.params.color("lightColor");
    let l = light_dir(ctx.params.f("lightAngle"), 45.0);
    // A 45° bevel: the face normal tilts towards the nearest edge.
    let tilt = std::f64::consts::FRAC_1_SQRT_2;
    b.img.rows_mut().for_each(|(y, row)| {
        let py = y as f64 + 0.5;
        for (x, px) in row.iter_mut().enumerate() {
            let pxx = x as f64 + 0.5;
            let d = [pxx - x0, x0 + w - pxx, py - y0, y0 + h - py];
            if d.iter().any(|v| *v < 0.0) {
                continue;
            }
            let (mut mi, mut mv) = (0, d[0]);
            for (i, v) in d.iter().enumerate() {
                if *v < mv {
                    mv = *v;
                    mi = i;
                }
            }
            if mv >= t {
                continue;
            }
            let n2 = [[-1.0, 0.0], [1.0, 0.0], [0.0, -1.0], [0.0, 1.0]][mi];
            let n = [n2[0] * tilt, n2[1] * tilt, tilt];
            let s = (dot3(n, l) - l[2]) as f32 * inten * 2.0;
            apply_shade(px, s, light);
        }
    });
    b
}

fn radial_shadow(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let color = ctx.params.color("color");
    let opacity = ctx.params.f("opacity") as f32 / 100.0;
    let dist = ctx.params.f("projectionDistance").max(0.0);
    let soft = ctx.params.f("softness").max(0.0) * b.scale;
    let glass = ctx.params.e("render") == 1;
    let influence = ctx.params.f("colorInfluence") as f32 / 100.0;
    let only = ctx.params.b("shadowOnly");
    let k = 1.0 + dist / 100.0;
    let lp0 = ctx.params.v2("lightSource");
    if ctx.params.b("resizeLayer") && !ctx.adjustment {
        let l = b.to_px(lp0);
        let (bw, bh) = (b.img.width as f64, b.img.height as f64);
        let grow = [(0.0, 0.0), (bw, 0.0), (0.0, bh), (bw, bh)].iter().map(|(cx, cy)| ((cx - l.0).hypot(cy - l.1)) * (k - 1.0)).fold(0.0, f64::max);
        b.pad((grow + soft * 1.5).ceil() as u32 + 1);
    }
    let l = b.to_px(lp0);
    let src = b.img.clone();
    let mut sh = Image::new(src.width, src.height);
    sh.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let (qx, qy) = (x as f64 + 0.5, y as f64 + 0.5);
            let s = src.sample_bilinear(l.0 + (qx - l.0) / k, l.1 + (qy - l.1) / k);
            let (sc, sa) = unpremul(s);
            let mut a = if glass { (sa * (1.0 - sa) * 4.0).clamp(0.0, 1.0) } else { sa };
            a *= opacity * color[3];
            let c = [color[0] + (sc[0] - color[0]) * influence, color[1] + (sc[1] - color[1]) * influence, color[2] + (sc[2] - color[2]) * influence];
            *px = premul(c, a);
        }
    });
    if soft > 0.0 {
        sh = gaussian_blur(&sh, soft * 0.5, soft * 0.5, false);
    }
    if !only {
        sh.data.par_iter_mut().zip(src.data.par_iter()).for_each(|(s, o)| {
            let kk = 1.0 - o[3];
            for c in 0..4 {
                s[c] = o[c] + s[c] * kk;
            }
        });
    }
    b.img = sh;
    b
}

/// Phong-style shading shared by CC Cylinder and CC Sphere.
struct Shading {
    l: [f64; 3],
    color: [f32; 4],
    intensity: f32,
    ambient: f32,
    diffuse: f32,
    specular: f32,
    shininess: f32,
}

impl Shading {
    fn from(ctx: &EffectCtx) -> Shading {
        Shading {
            l: light_dir(ctx.params.f("lightDirection"), ctx.params.f("lightHeight")),
            color: ctx.params.color("lightColor"),
            intensity: ctx.params.f("lightIntensity") as f32 / 100.0,
            ambient: ctx.params.f("ambient") as f32 / 100.0,
            diffuse: ctx.params.f("diffuse") as f32 / 100.0,
            specular: ctx.params.f("specular") as f32 / 100.0,
            shininess: 1.0 / (ctx.params.f("roughness") as f32).max(0.001),
        }
    }
    /// Shade a premultiplied texel with surface normal `n`.
    fn shade(&self, px: Px, n: [f64; 3]) -> Px {
        let (c, a) = unpremul(px);
        if a <= 0.0 {
            return [0.0; 4];
        }
        let nl = dot3(n, self.l).max(0.0) as f32;
        let hv = norm3([self.l[0], self.l[1], self.l[2] + 1.0]);
        let spec = if nl > 0.0 { (dot3(n, hv).max(0.0) as f32).powf(self.shininess) * self.specular } else { 0.0 };
        let mut o = [0.0f32; 3];
        for i in 0..3 {
            let lc = self.color[i] * self.intensity;
            o[i] = c[i] * (self.ambient + self.diffuse * nl * lc) + spec * lc;
        }
        premul(o, a)
    }
}

fn over(top: Px, bottom: Px) -> Px {
    let k = 1.0 - top[3];
    [top[0] + bottom[0] * k, top[1] + bottom[1] * k, top[2] + bottom[2] * k, top[3] + bottom[3] * k]
}

/// Sample the layer as a texture at normalised (u, v), wrapping horizontally.
fn texel(src: &Image, rect: (f64, f64, f64, f64), u: f64, v: f64) -> Px {
    let (x0, y0, w, h) = rect;
    let u = u.rem_euclid(1.0);
    if !(0.0..=1.0).contains(&v) {
        return [0.0; 4];
    }
    src.sample_bilinear_clamped(x0 + u * w, y0 + v * h)
}

fn cylinder(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let rect = layer_rect(ctx, &b);
    let (_, _, w, h) = rect;
    let r = (ctx.params.f("radius") / 100.0 * w / std::f64::consts::TAU).max(0.5);
    let c = b.to_px(ctx.params.v2("position"));
    // Keep the pre-existing Rotation control as an additional texture spin.
    let spin = ctx.params.f("rotation").to_radians();
    let (rx, ry, rz) = (ctx.params.f("rotationX"), ctx.params.f("rotationY"), ctx.params.f("rotationZ"));
    let tilted = rx != 0.0 || ry != 0.0 || rz != 0.0;
    let m = rot_xyz(rx, ry, rz);
    // The cylinder's local Y axis after rotation, in viewer coordinates.
    let axis = [m[0][1], m[1][1], m[2][1]];
    let a = 1.0 - axis[2] * axis[2];
    let mode = ctx.params.e("render");
    let sh = Shading::from(ctx);
    let src = b.img.clone();
    b.img.rows_mut().for_each(|(y, row)| {
        let dy = y as f64 + 0.5 - c.1;
        for (x, px) in row.iter_mut().enumerate() {
            let dx = x as f64 + 0.5 - c.0;
            if !tilted {
                // Preserve exactly the mapping of projects saved before axis rotations.
                let v = dy / h + 0.5;
                let s = dx / r;
                if s.abs() >= 1.0 {
                    *px = [0.0; 4];
                    continue;
                }
                let th = s.asin();
                let cz = th.cos();
                let front = || sh.shade(texel(&src, rect, (th + spin) / std::f64::consts::TAU + 0.5, v), [s, 0.0, cz]);
                let back = || sh.shade(texel(&src, rect, (std::f64::consts::PI - th + spin) / std::f64::consts::TAU + 0.5, v), [-s, 0.0, cz]);
                *px = match mode {
                    1 => front(),
                    2 => back(),
                    _ => over(front(), back()),
                };
                continue;
            }
            // Intersect the viewing ray (dx,dy,z) with the rotated cylindrical
            // side wall: |q - dot(q,axis)*axis|^2 = radius^2.
            let da = dx * axis[0] + dy * axis[1];
            let bq = -da * axis[2];
            let discriminant = bq * bq - a * (dx * dx + dy * dy - da * da - r * r);
            if a <= 1e-10 || !discriminant.is_finite() || discriminant <= 0.0 {
                *px = [0.0; 4];
                continue;
            }
            let root = discriminant.sqrt();
            let face = |z: f64, back: bool| {
                // Rᵀ turns the intersection into cylinder-local coordinates.
                let local = [m[0][0] * dx + m[1][0] * dy + m[2][0] * z, m[0][1] * dx + m[1][1] * dy + m[2][1] * z, m[0][2] * dx + m[1][2] * dy + m[2][2] * z];
                let u = (local[0].atan2(local[2]) + spin) / std::f64::consts::TAU + 0.5;
                let v = local[1] / h + 0.5;
                let sign = if back { -1.0 } else { 1.0 };
                let nx = local[0] / r * sign;
                let nz = local[2] / r * sign;
                let normal = [m[0][0] * nx + m[0][2] * nz, m[1][0] * nx + m[1][2] * nz, m[2][0] * nx + m[2][2] * nz];
                sh.shade(texel(&src, rect, u, v), normal)
            };
            let front = || face((-bq + root) / a, false);
            let back = || face((-bq - root) / a, true);
            *px = match mode {
                1 => front(),
                2 => back(),
                _ => over(front(), back()),
            };
        }
    });
    b
}

/// Rotation matrix R = Rz · Ry · Rx (degrees).
fn rot_xyz(rx: f64, ry: f64, rz: f64) -> [[f64; 3]; 3] {
    let (sx, cx) = rx.to_radians().sin_cos();
    let (sy, cy) = ry.to_radians().sin_cos();
    let (sz, cz) = rz.to_radians().sin_cos();
    let mx = [[1.0, 0.0, 0.0], [0.0, cx, -sx], [0.0, sx, cx]];
    let my = [[cy, 0.0, sy], [0.0, 1.0, 0.0], [-sy, 0.0, cy]];
    let mz = [[cz, -sz, 0.0], [sz, cz, 0.0], [0.0, 0.0, 1.0]];
    let mul = |a: [[f64; 3]; 3], b: [[f64; 3]; 3]| {
        let mut o = [[0.0; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                o[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
            }
        }
        o
    };
    mul(mz, mul(my, mx))
}

fn sphere(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let rect = layer_rect(ctx, &b);
    let r = (ctx.params.f("radius") * b.scale).max(0.5);
    let c = b.to_px(ctx.params.v2("offset"));
    let m = rot_xyz(ctx.params.f("rotationX"), ctx.params.f("rotationY"), ctx.params.f("rotationZ"));
    let mode = ctx.params.e("render");
    let sh = Shading::from(ctx);
    let src = b.img.clone();
    // Texture lookup of a view-space unit vector: rotate by Rᵀ (inverse) then equirectangular.
    let tex = |p: [f64; 3]| {
        let t = [
            m[0][0] * p[0] + m[1][0] * p[1] + m[2][0] * p[2],
            m[0][1] * p[0] + m[1][1] * p[1] + m[2][1] * p[2],
            m[0][2] * p[0] + m[1][2] * p[1] + m[2][2] * p[2],
        ];
        let lon = t[0].atan2(t[2]);
        let lat = t[1].clamp(-1.0, 1.0).asin();
        texel(&src, rect, lon / std::f64::consts::TAU + 0.5, lat / std::f64::consts::PI + 0.5)
    };
    b.img.rows_mut().for_each(|(y, row)| {
        let sy = (y as f64 + 0.5 - c.1) / r;
        for (x, px) in row.iter_mut().enumerate() {
            let sx = (x as f64 + 0.5 - c.0) / r;
            let d2 = sx * sx + sy * sy;
            if d2 >= 1.0 {
                *px = [0.0; 4];
                continue;
            }
            let z = (1.0 - d2).sqrt();
            let front = || sh.shade(tex([sx, sy, z]), [sx, sy, z]);
            let back = || sh.shade(tex([sx, sy, -z]), [-sx, -sy, z]);
            *px = match mode {
                1 => front(),
                2 => back(),
                _ => over(front(), back()),
            };
        }
    });
    b
}

fn spotlight(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let (_, _, w, h) = layer_rect(ctx, &b);
    let from = b.to_px(ctx.params.v2("from"));
    let to = b.to_px(ctx.params.v2("to"));
    let height = (ctx.params.f("height") / 100.0 * w.max(h)).max(1e-3);
    let cone = ctx.params.f("coneAngle").clamp(0.1, 89.9).to_radians();
    let soft = (ctx.params.f("edgeSoftness") / 100.0).clamp(0.0, 1.0);
    let color = ctx.params.color("color");
    let k = ctx.params.f("intensity") as f32 / 100.0;
    let light_only = ctx.params.e("render") == 1;
    let axis = norm3([to.0 - from.0, to.1 - from.1, -height]);
    let inner = cone * (1.0 - soft);
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let v = norm3([x as f64 + 0.5 - from.0, y as f64 + 0.5 - from.1, -height]);
            let ang = dot3(v, axis).clamp(-1.0, 1.0).acos();
            let i = 1.0 - smoothstep(inner as f32, cone as f32, ang as f32);
            let m = i * k;
            if light_only {
                let a = m.clamp(0.0, 1.0);
                *px = [color[0] * a, color[1] * a, color[2] * a, a];
            } else {
                for c in 0..3 {
                    px[c] *= m * color[c];
                }
            }
        }
    });
    b
}

pub fn specs() -> Vec<EffectSpec> {
    let pt = |x, y| Value::Vec2([x, y]);
    let shading = || {
        vec![
            p("lightIntensity", "Light Intensity", num(100.0), slider(0.0, 400.0, 0.0, 200.0, 1)),
            p("lightColor", "Light Color", col(1.0, 1.0, 1.0), ParamUi::Color),
            p("lightHeight", "Light Height", num(50.0), slider(-90.0, 90.0, -90.0, 90.0, 1)),
            p("lightDirection", "Light Direction", num(-45.0), ParamUi::Angle),
            p("ambient", "Ambient", num(20.0), slider(0.0, 200.0, 0.0, 100.0, 1)),
            p("diffuse", "Diffuse", num(80.0), slider(0.0, 200.0, 0.0, 100.0, 1)),
            p("specular", "Specular", num(20.0), slider(0.0, 200.0, 0.0, 100.0, 1)),
            p("roughness", "Roughness", num(0.05), slider(0.001, 0.5, 0.001, 0.5, 3)),
        ]
    };
    let render = || popup(&["Full", "Outside", "Inside"]);
    let mut cyl = vec![
        p("radius", "Radius", num(100.0), slider(0.0, 400.0, 0.0, 200.0, 1)),
        p("position", "Position", pt(0.5, 0.5), ParamUi::Point),
        p("rotation", "Rotation", num(0.0), ParamUi::Angle),
        p("rotationX", "Rotation X", num(0.0), ParamUi::Angle),
        p("rotationY", "Rotation Y", num(0.0), ParamUi::Angle),
        p("rotationZ", "Rotation Z", num(0.0), ParamUi::Angle),
        p("render", "Render", Value::Enum(0), render()),
    ];
    cyl.extend(shading());
    let mut sph = vec![
        p("radius", "Radius", num(120.0), slider(0.0, 4000.0, 0.0, 1000.0, 1)),
        p("offset", "Offset", pt(0.5, 0.5), ParamUi::Point),
        p("rotationX", "Rotation X", num(0.0), ParamUi::Angle),
        p("rotationY", "Rotation Y", num(0.0), ParamUi::Angle),
        p("rotationZ", "Rotation Z", num(0.0), ParamUi::Angle),
        p("render", "Render", Value::Enum(0), render()),
    ];
    sph.extend(shading());
    vec![
        spec(
            "ec.perspective.bevelalpha",
            "Bevel Alpha",
            vec![
                p("edgeThickness", "Edge Thickness", num(2.0), slider(0.0, 200.0, 0.0, 10.0, 2)),
                p("lightAngle", "Light Angle", num(-60.0), ParamUi::Angle),
                p("lightColor", "Light Color", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("lightIntensity", "Light Intensity", num(0.4), slider(0.0, 1.0, 0.0, 1.0, 2)),
            ],
            bevel_alpha,
        ),
        spec(
            "ec.perspective.beveledges",
            "Bevel Edges",
            vec![
                p("edgeThickness", "Edge Thickness", num(0.1), slider(0.0, 0.5, 0.0, 0.5, 2)),
                p("lightAngle", "Light Angle", num(-60.0), ParamUi::Angle),
                p("lightColor", "Light Color", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("lightIntensity", "Light Intensity", num(0.25), slider(0.0, 1.0, 0.0, 1.0, 2)),
            ],
            bevel_edges,
        ),
        spec(
            "ec.perspective.radialshadow",
            "Radial Shadow",
            vec![
                p("color", "Shadow Color", col(0.0, 0.0, 0.0), ParamUi::Color),
                p("opacity", "Opacity", num(50.0), slider(0.0, 100.0, 0.0, 100.0, 0)),
                p("lightSource", "Light Source", pt(0.3, 0.2), ParamUi::Point),
                p("projectionDistance", "Projection Distance", num(10.0), slider(0.0, 1000.0, 0.0, 100.0, 1)),
                p("softness", "Softness", num(0.0), slider(0.0, 1000.0, 0.0, 100.0, 1)),
                p("render", "Render", Value::Enum(0), popup(&["Regular", "Glass Edge"])),
                p("colorInfluence", "Color Influence", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 0)),
                p("shadowOnly", "Shadow Only", Value::Bool(false), ParamUi::Checkbox),
                p("resizeLayer", "Resize Layer", Value::Bool(false), ParamUi::Checkbox),
            ],
            radial_shadow,
        ),
        spec("ec.perspective.cccylinder", "CC Cylinder", cyl, cylinder),
        spec("ec.perspective.ccsphere", "CC Sphere", sph, sphere),
        spec(
            "ec.perspective.ccspotlight",
            "CC Spotlight",
            vec![
                p("from", "From", pt(0.3, 0.2), ParamUi::Point),
                p("to", "To", pt(0.5, 0.5), ParamUi::Point),
                p("height", "Height", num(50.0), slider(0.0, 1000.0, 0.0, 200.0, 1)),
                p("coneAngle", "Cone Angle", num(20.0), slider(0.1, 89.9, 1.0, 89.0, 1)),
                p("edgeSoftness", "Edge Softness", num(25.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("color", "Color", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("intensity", "Intensity", num(100.0), slider(0.0, 400.0, 0.0, 200.0, 1)),
                p("render", "Render", Value::Enum(0), popup(&["Light", "Light Only"])),
            ],
            spotlight,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Params, find};

    fn run(id: &str, img: &Image, set: &[(&str, Value)]) -> Buf {
        let s = find(id).unwrap();
        let mut params = Params { values: s.params.iter().map(|p| (p.id.to_string(), p.default.clone())).collect() };
        for (k, v) in set {
            params.values.insert(k.to_string(), v.clone());
        }
        // Points: defaults are fractions; scale to the image size like instantiate() would.
        for ps in &s.params {
            if let (ParamUi::Point, Value::Vec2(f)) = (&ps.ui, &ps.default)
                && !set.iter().any(|(k, _)| *k == ps.id)
            {
                params.values.insert(ps.id.to_string(), Value::Vec2([f[0] * img.width as f64, f[1] * img.height as f64]));
            }
        }
        let ctx =
            EffectCtx { params: &params, time: 0.0, layer_size: [img.width as f64, img.height as f64], seed: 1, adjustment: false, env: Default::default() };
        (s.render)(&ctx, Buf { img: img.clone(), offset: [0.0, 0.0], scale: 1.0 })
    }

    fn square(w: u32, h: u32, m: u32, c: [f32; 3]) -> Image {
        let mut img = Image::new(w, h);
        for y in m..h - m {
            for x in m..w - m {
                img.set(x, y, [c[0], c[1], c[2], 1.0]);
            }
        }
        img
    }

    #[test]
    fn bevel_alpha_lights_one_side_and_shades_other() {
        let img = square(40, 40, 10, [0.5, 0.5, 0.5]);
        let out = run("ec.perspective.bevelalpha", &img, &[("lightAngle", num(-90.0)), ("edgeThickness", num(4.0))]).img;
        // Light from the left: the left edge brightens, the right edge darkens, centre unchanged.
        let left = out.get(10, 20)[0];
        let right = out.get(29, 20)[0];
        assert!(left > 0.55, "{left}");
        assert!(right < 0.45, "{right}");
        assert!((out.get(20, 20)[0] - 0.5).abs() < 1e-3);
        assert_eq!(out.get(20, 20)[3], 1.0);
    }

    #[test]
    fn bevel_edges_only_touches_the_border_band() {
        let img = Image::filled(40, 40, [0.5, 0.5, 0.5, 1.0]);
        let out = run("ec.perspective.beveledges", &img, &[("lightAngle", num(0.0))]).img;
        assert!(out.get(20, 1)[0] > 0.5, "top lit");
        assert!(out.get(20, 38)[0] < 0.5, "bottom shaded");
        assert_eq!(out.get(20, 20)[0], 0.5);
    }

    #[test]
    fn radial_shadow_is_behind_and_scaled() {
        let img = square(60, 60, 25, [1.0, 0.0, 0.0]);
        let out = run(
            "ec.perspective.radialshadow",
            &img,
            &[("lightSource", Value::Vec2([30.0, 30.0])), ("projectionDistance", num(100.0)), ("opacity", num(100.0))],
        )
        .img;
        // The original stays on top.
        assert_eq!(out.get(30, 30), [1.0, 0.0, 0.0, 1.0]);
        // A 2× shadow around the light covers pixels the layer does not.
        let s = out.get(30, 21);
        assert!(s[3] > 0.9 && s[0] < 0.05, "{s:?}");
        assert_eq!(out.get(30, 5)[3], 0.0);
    }

    #[test]
    fn sphere_is_round_and_shaded() {
        let img = Image::filled(64, 64, [0.8, 0.8, 0.8, 1.0]);
        let out = run("ec.perspective.ccsphere", &img, &[("radius", num(20.0))]).img;
        assert_eq!(out.get(2, 2)[3], 0.0);
        assert!(out.get(32, 32)[3] > 0.99);
        assert_eq!(out.get(32, 55)[3], 0.0);
        // Lit from the upper left: that side is brighter than the lower right.
        assert!(out.get(24, 24)[0] > out.get(40, 40)[0]);
    }

    #[test]
    fn cylinder_width_follows_radius() {
        let img = Image::filled(100, 20, [0.5, 0.5, 0.5, 1.0]);
        let out = run("ec.perspective.cccylinder", &img, &[]).img;
        // radius 100% → r = W / 2π ≈ 15.9 px
        assert!(out.get(50, 10)[3] > 0.99);
        assert!(out.get(50 + 14, 10)[3] > 0.99);
        assert_eq!(out.get(50 + 18, 10)[3], 0.0);
    }

    #[test]
    fn cylinder_axis_rotations_affect_geometry_and_texture() {
        let img = Image::filled(100, 100, [0.5, 0.5, 0.5, 1.0]);
        let default = run("ec.perspective.cccylinder", &img, &[]).img;
        let x_tilt = run("ec.perspective.cccylinder", &img, &[("rotationX", num(70.0))]).img;
        // Tilting the finite cylinder shortens its vertical projection.
        assert!(default.get(50, 10)[3] > 0.99);
        assert_eq!(x_tilt.get(50, 10)[3], 0.0);
        assert!(x_tilt.get(50, 50)[3] > 0.99);
        let z_roll = run("ec.perspective.cccylinder", &img, &[("rotationZ", num(90.0))]).img;
        // A quarter turn moves the side wall from vertical to horizontal.
        assert_eq!(default.get(5, 50)[3], 0.0);
        assert!(z_roll.get(5, 50)[3] > 0.99);

        let mut striped = Image::filled(100, 100, [0.0, 0.0, 0.0, 1.0]);
        for y in 0..100 {
            for x in 0..100 {
                striped.set(x, y, [x as f32 / 100.0, 0.2, 0.3, 1.0]);
            }
        }
        let baseline = run("ec.perspective.cccylinder", &striped, &[]).img;
        let spun = run("ec.perspective.cccylinder", &striped, &[("rotationY", num(90.0))]).img;
        assert!((baseline.get(50, 50)[0] - spun.get(50, 50)[0]).abs() > 0.05);
    }

    #[test]
    fn spotlight_darkens_outside_cone() {
        let img = Image::filled(80, 80, [1.0, 1.0, 1.0, 1.0]);
        let out =
            run("ec.perspective.ccspotlight", &img, &[("from", Value::Vec2([40.0, 40.0])), ("to", Value::Vec2([40.0, 40.0])), ("edgeSoftness", num(0.0))]).img;
        assert!((out.get(40, 40)[0] - 1.0).abs() < 1e-4);
        assert_eq!(out.get(2, 2)[0], 0.0);
        assert_eq!(out.get(2, 2)[3], 1.0);
    }
}
