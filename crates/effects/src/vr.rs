//! Immersive Video effects (VR Blur, VR Chromatic Aberrations, VR Color Gradients, VR Converter,
//! VR De-Noise, VR Digital Glitch, VR Fractal Noise, VR Glow, VR Plane to Sphere, VR Rotate
//! Sphere, VR Sharpen, VR Sphere To Plane).
//!
//! All of them treat the layer as a 360° equirectangular frame (longitude across, latitude down;
//! the front, +Z, at the centre) and are **seam-aware**: filters wrap around the left/right edge,
//! continue across the poles (a column continues on the opposite meridian), and spatial filters
//! widen their horizontal footprint by 1/cos(latitude) so a blur is the same size on the sphere
//! everywhere. Stereo footage (Frame Layout: over/under or side by side) is processed per eye.
//!
//! VR Converter maps between equirectangular, cube maps (4:3 cross, 3:2 and 6:1 strips), an
//! angular fisheye ("sphere map") and a rectilinear 2D view, through a common view direction.

use std::f64::consts::PI;

use effectcraft_color::{BlendMode, blend_pixel, luminance};
use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use effectcraft_raster::{Image, Px};
use rayon::prelude::*;

use crate::generate::value_noise;
use crate::util::{hash1, unpremul};
use crate::{Buf, EffectCtx, EffectSpec, col, num, p, popup, slider};

type V3 = [f64; 3];

fn spec(id: &'static str, name: &'static str, params: Vec<crate::ParamSpec>, render: crate::RenderFn) -> EffectSpec {
    EffectSpec { id, name, category: "Immersive Video", params, render, gpu: false, float: true }
}

pub const FRAME_LAYOUTS: [&str; 3] = ["Monoscopic", "Stereo - Over/Under", "Stereo - Side by Side"];

fn layout_param() -> crate::ParamSpec {
    p("frameLayout", "Frame Layout", Value::Enum(0), popup(&FRAME_LAYOUTS))
}

/// Eye rectangles (x, y, w, h) of a frame.
fn eyes(w: u32, h: u32, layout: u32) -> Vec<(u32, u32, u32, u32)> {
    match layout {
        1 if h >= 2 => vec![(0, 0, w, h / 2), (0, h / 2, w, h - h / 2)],
        2 if w >= 2 => vec![(0, 0, w / 2, h), (w / 2, 0, w - w / 2, h)],
        _ => vec![(0, 0, w, h)],
    }
}

/// Run `f` on every eye of `img` and reassemble.
fn per_eye(img: &Image, layout: u32, f: impl Fn(&Image, usize) -> Image + Sync) -> Image {
    let rects = eyes(img.width, img.height, layout);
    if rects.len() == 1 {
        return f(img, 0);
    }
    let outs: Vec<Image> = rects.par_iter().enumerate().map(|(i, &(x, y, w, h))| f(&img.crop(x as i64, y as i64, w, h), i)).collect();
    let mut out = Image::new(img.width, img.height);
    for (&(x0, y0, w, _), o) in rects.iter().zip(&outs) {
        for (yy, row) in o.data.chunks(w.max(1) as usize).enumerate() {
            let start = out.idx(x0, y0 + yy as u32);
            out.data[start..start + row.len()].copy_from_slice(row);
        }
    }
    out
}

// ---------------------------------------------------------------- sphere geometry

#[inline]
fn norm(v: V3) -> V3 {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-300);
    [v[0] / l, v[1] / l, v[2] / l]
}

#[inline]
fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Direction (y up, +z front) of an equirectangular position in pixels.
#[inline]
pub fn equi_dir(x: f64, y: f64, w: f64, h: f64) -> V3 {
    let lon = (x / w - 0.5) * 2.0 * PI;
    let lat = (0.5 - y / h) * PI;
    [lat.cos() * lon.sin(), lat.sin(), lat.cos() * lon.cos()]
}

/// Equirectangular pixel position of a direction.
#[inline]
pub fn equi_px(d: V3, w: f64, h: f64) -> (f64, f64) {
    let lon = d[0].atan2(d[2]);
    let lat = d[1].clamp(-1.0, 1.0).asin();
    ((lon / (2.0 * PI) + 0.5) * w, (0.5 - lat / PI) * h)
}

/// Bilinear sample of an equirectangular image: wraps horizontally, continues over the poles.
pub fn sample_equi(img: &Image, x: f64, y: f64) -> Px {
    let (w, h) = (img.width as i64, img.height as i64);
    if w == 0 || h == 0 {
        return [0.0; 4];
    }
    let fx = x - 0.5;
    let fy = y - 0.5;
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = ((fx - x0) as f32, (fy - y0) as f32);
    let at = |xi: i64, yi: i64| -> Px {
        let (mut xi, mut yi) = (xi, yi);
        if yi < 0 {
            yi = -1 - yi;
            xi += w / 2;
        } else if yi >= h {
            yi = 2 * h - 1 - yi;
            xi += w / 2;
        }
        img.data[(yi.clamp(0, h - 1) * w + xi.rem_euclid(w)) as usize]
    };
    let (x0, y0) = (x0 as i64, y0 as i64);
    let (a, b, c, d) = (at(x0, y0), at(x0 + 1, y0), at(x0, y0 + 1), at(x0 + 1, y0 + 1));
    let mut o = [0.0f32; 4];
    for k in 0..4 {
        let top = a[k] + (b[k] - a[k]) * tx;
        let bot = c[k] + (d[k] - c[k]) * tx;
        o[k] = top + (bot - top) * ty;
    }
    o
}

/// Rotation matrix (row-major) for tilt (about X), pan (about Y) and roll (about Z), degrees:
/// R = Ry(pan) · Rx(tilt) · Rz(roll).
pub fn rotation(tilt: f64, pan: f64, roll: f64) -> [[f64; 3]; 3] {
    let (a, b, c) = (tilt.to_radians(), pan.to_radians(), roll.to_radians());
    let rx = [[1.0, 0.0, 0.0], [0.0, a.cos(), -a.sin()], [0.0, a.sin(), a.cos()]];
    let ry = [[b.cos(), 0.0, b.sin()], [0.0, 1.0, 0.0], [-b.sin(), 0.0, b.cos()]];
    let rz = [[c.cos(), -c.sin(), 0.0], [c.sin(), c.cos(), 0.0], [0.0, 0.0, 1.0]];
    mm(&ry, &mm(&rx, &rz))
}

fn mm(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut o = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            o[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    o
}

#[inline]
fn mv(m: &[[f64; 3]; 3], v: V3) -> V3 {
    [dot(m[0], v), dot(m[1], v), dot(m[2], v)]
}

#[inline]
fn mtv(m: &[[f64; 3]; 3], v: V3) -> V3 {
    [m[0][0] * v[0] + m[1][0] * v[1] + m[2][0] * v[2], m[0][1] * v[0] + m[1][1] * v[1] + m[2][1] * v[2], m[0][2] * v[0] + m[1][2] * v[1] + m[2][2] * v[2]]
}

// ---------------------------------------------------------------- projections (VR Converter)

/// VR Converter's projections / layouts. Cube-map Facebook 3:2 and Cube-map EAC 3:2 put
/// left, front, right over bottom, back, top (EAC with equi-angular faces); Fisheye
/// (FullDome) is an angular fisheye looking up at the zenith with the front at the bottom.
pub const PROJECTIONS: [&str; 9] = [
    "Equirectangular 2:1",
    "Cube-map 4:3",
    "Cube-map Pano2VR 3:2",
    "Cube-map GearVR 6:1",
    "Sphere-map",
    "2D Source",
    "Cube-map Facebook 3:2",
    "Cube-map EAC 3:2",
    "Fisheye (FullDome)",
];

/// Cube faces: (forward, right, up).
const FACES: [(V3, V3, V3); 6] = [
    ([1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]),  // +X right
    ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),  // -X left
    ([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),  // +Y top
    ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),  // -Y bottom
    ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),   // +Z front
    ([0.0, 0.0, -1.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]), // -Z back
];

/// Grid size and the (column, row) of each face (+X, −X, +Y, −Y, +Z, −Z) for a cube layout.
fn cube_grid(kind: u32) -> ((u32, u32), [(u32, u32); 6]) {
    match kind {
        // Horizontal cross: top over the front; left, front, right, back across the middle.
        1 => ((4, 3), [(2, 1), (0, 1), (1, 0), (1, 2), (1, 1), (3, 1)]),
        // 3 × 2: right, left, top / bottom, front, back.
        2 => ((3, 2), [(0, 0), (1, 0), (2, 0), (0, 1), (1, 1), (2, 1)]),
        // 3 × 2: left, front, right / bottom, back, top (Facebook, EAC).
        6 | 7 => ((3, 2), [(2, 0), (0, 0), (2, 1), (0, 1), (1, 0), (1, 1)]),
        // 6 × 1 strip in face order.
        _ => ((6, 1), [(0, 0), (1, 0), (2, 0), (3, 0), (4, 0), (5, 0)]),
    }
}

/// A projection: frame position (pixels) ↔ view direction.
#[derive(Clone, Copy, Debug)]
pub struct Proj {
    pub kind: u32,
    /// Horizontal field of view in degrees (sphere map and 2D only).
    pub fov: f64,
}

impl Proj {
    /// Direction seen at frame position (x, y) of a `w`×`h` frame (None outside the image).
    pub fn dir(&self, x: f64, y: f64, w: f64, h: f64) -> Option<V3> {
        match self.kind {
            0 => Some(equi_dir(x, y, w, h)),
            1..=3 | 6 | 7 => {
                let ((gw, gh), cells) = cube_grid(self.kind);
                let (cw, ch) = (w / gw as f64, h / gh as f64);
                let (cx, cy) = ((x / cw).floor(), (y / ch).floor());
                let f = cells.iter().position(|&(c, r)| c as f64 == cx && r as f64 == cy)?;
                let mut a = (x - cx * cw) / cw * 2.0 - 1.0;
                let mut b = (y - cy * ch) / ch * 2.0 - 1.0;
                if self.kind == 7 {
                    // Equi-angular faces: positions are linear in angle.
                    a = (a * std::f64::consts::FRAC_PI_4).tan();
                    b = (b * std::f64::consts::FRAC_PI_4).tan();
                }
                let (fw, r, u) = FACES[f];
                Some(norm([fw[0] + a * r[0] - b * u[0], fw[1] + a * r[1] - b * u[1], fw[2] + a * r[2] - b * u[2]]))
            }
            8 => {
                // FullDome: 180° angular fisheye centred on the zenith (+Y), front at the bottom.
                let s = w.min(h) / 2.0;
                let (u, v) = ((x - w / 2.0) / s, (y - h / 2.0) / s);
                let r = (u * u + v * v).sqrt();
                if r > 1.0 {
                    return None;
                }
                let th = r * std::f64::consts::FRAC_PI_2;
                let (cu, cv) = if r > 1e-12 { (u / r, v / r) } else { (0.0, 0.0) };
                // Local: forward = +Y, image right = +X, image down = +Z.
                Some([th.sin() * cu, th.cos(), th.sin() * cv])
            }
            4 => {
                // Angular fisheye centred on +Z; radius ∝ angle, `fov` across the image width.
                let half = (self.fov.clamp(1.0, 360.0) / 2.0).to_radians();
                let s = w.min(h) / 2.0;
                let (u, v) = ((x - w / 2.0) / s, (h / 2.0 - y) / s);
                let r = (u * u + v * v).sqrt();
                if r > 1.0 {
                    return None;
                }
                let th = r * half;
                let (cu, cv) = if r > 1e-12 { (u / r, v / r) } else { (0.0, 0.0) };
                Some([th.sin() * cu, th.sin() * cv, th.cos()])
            }
            _ => {
                let t = (self.fov.clamp(1.0, 179.0) / 2.0).to_radians().tan();
                let u = (x / w * 2.0 - 1.0) * t;
                let v = (1.0 - y / h * 2.0) * t * h / w;
                Some(norm([u, v, 1.0]))
            }
        }
    }

    /// Frame position (pixels) of direction `d`, and for cube maps the face rectangle to clamp
    /// samples into. None when the direction is not visible.
    pub fn pos(&self, d: V3, w: f64, h: f64) -> Option<(f64, f64, Option<[f64; 4]>)> {
        match self.kind {
            0 => {
                let (x, y) = equi_px(d, w, h);
                Some((x, y, None))
            }
            1..=3 | 6 | 7 => {
                let f = (0..6).max_by(|&i, &j| dot(d, FACES[i].0).total_cmp(&dot(d, FACES[j].0)))?;
                let (fw, r, u) = FACES[f];
                let k = dot(d, fw);
                let (mut a, mut b) = (dot(d, r) / k, -dot(d, u) / k);
                if self.kind == 7 {
                    a = a.atan() / std::f64::consts::FRAC_PI_4;
                    b = b.atan() / std::f64::consts::FRAC_PI_4;
                }
                let ((gw, gh), cells) = cube_grid(self.kind);
                let (cw, ch) = (w / gw as f64, h / gh as f64);
                let (c, rr) = cells[f];
                let (x0, y0) = (c as f64 * cw, rr as f64 * ch);
                Some((x0 + (a + 1.0) * 0.5 * cw, y0 + (b + 1.0) * 0.5 * ch, Some([x0, y0, x0 + cw, y0 + ch])))
            }
            8 => {
                let th = d[1].clamp(-1.0, 1.0).acos();
                if th > std::f64::consts::FRAC_PI_2 + 1e-9 {
                    return None;
                }
                let s = w.min(h) / 2.0;
                let r = th / std::f64::consts::FRAC_PI_2;
                let l = (d[0] * d[0] + d[2] * d[2]).sqrt();
                let (cu, cv) = if l > 1e-12 { (d[0] / l, d[2] / l) } else { (0.0, 0.0) };
                Some((w / 2.0 + cu * r * s, h / 2.0 + cv * r * s, None))
            }
            4 => {
                let half = (self.fov.clamp(1.0, 360.0) / 2.0).to_radians();
                let th = d[2].clamp(-1.0, 1.0).acos();
                if th > half + 1e-9 {
                    return None;
                }
                let s = w.min(h) / 2.0;
                let r = th / half;
                let l = (d[0] * d[0] + d[1] * d[1]).sqrt();
                let (cu, cv) = if l > 1e-12 { (d[0] / l, d[1] / l) } else { (0.0, 0.0) };
                Some((w / 2.0 + cu * r * s, h / 2.0 - cv * r * s, None))
            }
            _ => {
                if d[2] <= 1e-9 {
                    return None;
                }
                let t = (self.fov.clamp(1.0, 179.0) / 2.0).to_radians().tan();
                let u = d[0] / d[2] / t;
                let v = d[1] / d[2] / (t * h / w);
                if u.abs() > 1.0 || v.abs() > 1.0 {
                    return None;
                }
                Some(((u + 1.0) * 0.5 * w, (1.0 - v) * 0.5 * h, None))
            }
        }
    }

    /// Sample image `img` (in this projection) in direction `d`.
    pub fn sample(&self, img: &Image, d: V3) -> Px {
        let (w, h) = (img.width as f64, img.height as f64);
        match self.pos(d, w, h) {
            None => [0.0; 4],
            Some((x, y, None)) => {
                if self.kind == 0 {
                    sample_equi(img, x, y)
                } else {
                    img.sample_bilinear_clamped(x, y)
                }
            }
            Some((x, y, Some(r))) => {
                // Stay inside the face so neighbouring faces don't bleed in.
                let x = x.clamp(r[0] + 0.5, r[2] - 0.5);
                let y = y.clamp(r[1] + 0.5, r[3] - 0.5);
                img.sample_bilinear_clamped(x, y)
            }
        }
    }
}

/// Re-project `src` (in `from`) into a `w`×`h` frame in `to`, rotating the view by `rot`
/// (output direction → source direction = rotᵀ · d).
pub fn convert(src: &Image, from: Proj, to: Proj, w: u32, h: u32, rot: Option<&[[f64; 3]; 3]>) -> Image {
    let mut out = Image::new(w, h);
    let (fw, fh) = (w as f64, h as f64);
    out.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            *px = match to.dir(x as f64 + 0.5, y as f64 + 0.5, fw, fh) {
                Some(d) => {
                    let d = match rot {
                        Some(m) => mtv(m, d),
                        None => d,
                    };
                    from.sample(src, d)
                }
                None => [0.0; 4],
            };
        }
    });
    out
}

// ---------------------------------------------------------------- seam-aware filters

/// Horizontal box mean with wrap-around, radius `r` (pixels, ≤ w/2) on one row.
fn box_row_wrap(src: &[Px], dst: &mut [Px], r: usize) {
    let n = src.len();
    if n == 0 {
        return;
    }
    if r == 0 {
        dst.copy_from_slice(src);
        return;
    }
    let r = r.min((n - 1) / 2);
    if r == 0 {
        dst.copy_from_slice(src);
        return;
    }
    let k = (2 * r + 1) as f64;
    let mut acc = [0.0f64; 4];
    for i in 0..=2 * r {
        let p = src[(i + n - r) % n];
        for c in 0..4 {
            acc[c] += p[c] as f64;
        }
    }
    for x in 0..n {
        for c in 0..4 {
            dst[x][c] = (acc[c] / k) as f32;
        }
        let add = src[(x + r + 1) % n];
        let sub = src[(x + n - r) % n];
        for c in 0..4 {
            acc[c] += add[c] as f64 - sub[c] as f64;
        }
    }
}

/// Vertical box mean with pole continuation, radius `r`, over all columns.
fn box_cols_pole(img: &Image, r: usize) -> Image {
    let (w, h) = (img.width as usize, img.height as usize);
    if r == 0 || h == 0 {
        return img.clone();
    }
    let at = |x: usize, y: i64| -> Px {
        let (mut x, mut y) = (x, y);
        if y < 0 {
            y = -1 - y;
            x = (x + w / 2) % w;
        } else if y >= h as i64 {
            y = 2 * h as i64 - 1 - y;
            x = (x + w / 2) % w;
        }
        img.data[y.clamp(0, h as i64 - 1) as usize * w + x]
    };
    let k = (2 * r + 1) as f64;
    let cols: Vec<Vec<Px>> = (0..w)
        .into_par_iter()
        .map(|x| {
            let mut out = vec![[0.0f32; 4]; h];
            let mut acc = [0.0f64; 4];
            for j in -(r as i64)..=(r as i64) {
                let p = at(x, j);
                for c in 0..4 {
                    acc[c] += p[c] as f64;
                }
            }
            for y in 0..h {
                for c in 0..4 {
                    out[y][c] = (acc[c] / k) as f32;
                }
                let add = at(x, y as i64 + r as i64 + 1);
                let sub = at(x, y as i64 - r as i64);
                for c in 0..4 {
                    acc[c] += add[c] as f64 - sub[c] as f64;
                }
            }
            out
        })
        .collect();
    let mut out = Image::new(img.width, img.height);
    out.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            *px = cols[x][y];
        }
    });
    out
}

/// Gaussian-like blur of an equirectangular image with standard deviation `sigma` pixels at
/// the equator (3 box passes per axis), wrapping at the seam and continuing over the poles.
pub fn sphere_blur(img: &Image, sigma: f64) -> Image {
    if sigma < 0.1 || img.is_empty() {
        return img.clone();
    }
    let (w, h) = (img.width as usize, img.height as usize);
    let radii = crate::util::box_radii(sigma, 3);
    let mut cur = img.clone();
    let mut tmp = Image::new(img.width, img.height);
    for &r in &radii {
        tmp.rows_mut().for_each(|(y, row)| {
            let lat = (0.5 - (y as f64 + 0.5) / h as f64) * PI;
            let rr = ((r as f64) / lat.cos().max(1e-3)).round().min((w / 2) as f64) as usize;
            box_row_wrap(&cur.data[y * w..(y + 1) * w], row, rr);
        });
        std::mem::swap(&mut cur, &mut tmp);
    }
    for &r in &radii {
        cur = box_cols_pole(&cur, r);
    }
    cur
}

// ---------------------------------------------------------------- effects

fn layout(ctx: &EffectCtx) -> u32 {
    ctx.params.e("frameLayout")
}

fn vr_blur(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let s = ctx.params.f("blurriness").max(0.0) * b.scale * 0.5;
    if s < 0.1 {
        return b;
    }
    b.img = per_eye(&b.img, layout(ctx), |e, _| sphere_blur(e, s));
    b
}

fn vr_sharpen(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let amt = ctx.params.f("sharpenAmount") as f32 / 100.0;
    if amt == 0.0 {
        return b;
    }
    let s = (1.5 * b.scale).max(0.5);
    b.img = per_eye(&b.img, layout(ctx), |e, _| {
        let bl = sphere_blur(e, s);
        let mut o = e.clone();
        o.data.par_iter_mut().zip(bl.data.par_iter()).for_each(|(p, q)| {
            for c in 0..3 {
                p[c] = (p[c] + (p[c] - q[c]) * amt).clamp(0.0, p[3].max(0.0) * 4.0);
            }
        });
        o
    });
    b
}

fn vr_denoise(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let level = ctx.params.f("noiseLevel").max(0.0) / 100.0;
    if level <= 0.0 {
        return b;
    }
    // Radii are capped: an expression-driven Noise Level (1e4+) asked for a padded buffer and
    // guided-filter planes of many GB, an allocation failure that aborts the process. The
    // slider's range (0..100) gives at most 5 px per unit of scale, far below the cap.
    let r = (((1.0 + level * 4.0) * b.scale).max(1.0) as usize).min(MAX_DENOISE_RADIUS);
    if ctx.params.e("noiseType") == 1 {
        // Salt-and-pepper speckles: a median removes isolated outliers outright.
        let mr = ((level * 3.0 * b.scale).round() as usize).clamp(1, MAX_DENOISE_RADIUS);
        b.img = per_eye(&b.img, layout(ctx), |e, _| crate::noise::median_image(e, mr));
        return b;
    }
    let eps = (level * level * 0.02) as f32;
    let detail = (ctx.params.f("detail") / 100.0).clamp(0.0, 1.0) as f32;
    b.img = per_eye(&b.img, layout(ctx), |e, _| {
        // Guided filter (self-guided, per channel) on a frame padded with wrapped / pole-continued
        // pixels so the seam and poles are filtered like any other place.
        // The pad `wrap_pad` applies (it clamps), so the crop below starts where the frame is.
        let pad = wrap_pad_amount(r * 2 + 1);
        let padded = wrap_pad(e, pad);
        let guide = crate::util::Plane::luma(&padded);
        let ch = crate::util::split(&padded);
        let f: Vec<crate::util::Plane> = ch.par_iter().map(|c| crate::util::guided_filter(&guide, c, r, eps.max(1e-6))).collect();
        let joined = crate::util::join(&[f[0].clone(), f[1].clone(), f[2].clone(), f[3].clone()]);
        let mut o = joined.crop(pad as i64, pad as i64, e.width, e.height);
        o.data.par_iter_mut().zip(e.data.par_iter()).for_each(|(p, s)| {
            for c in 0..4 {
                p[c] = p[c] + (s[c] - p[c]) * detail;
            }
            p[3] = p[3].clamp(0.0, 1.0);
            for c in 0..3 {
                p[c] = p[c].max(0.0);
            }
        });
        o
    });
    b
}

/// Largest VR Denoise filter radius in buffer pixels (see `vr_denoise`).
const MAX_DENOISE_RADIUS: usize = 64;

/// The pad [`wrap_pad`] applies for a requested `pad`.
pub fn wrap_pad_amount(pad: usize) -> usize {
    pad.min(effectcraft_raster::MAX_PADDED_SIDE as usize / 2)
}

/// Pad an equirectangular image by [`wrap_pad_amount`]`(pad)` pixels: wrapped columns,
/// pole-continued rows.
pub fn wrap_pad(img: &Image, pad: usize) -> Image {
    // An empty frame has nothing to wrap (`clamp(0, h - 1)` below would panic).
    if img.is_empty() {
        return img.clone();
    }
    let pad = wrap_pad_amount(pad);
    let (w, h) = (img.width as i64, img.height as i64);
    let p = pad as i64;
    let mut out = Image::new((w + 2 * p) as u32, (h + 2 * p) as u32);
    out.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let (mut sx, mut sy) = (x as i64 - p, y as i64 - p);
            if sy < 0 {
                sy = -1 - sy;
                sx += w / 2;
            } else if sy >= h {
                sy = 2 * h - 1 - sy;
                sx += w / 2;
            }
            *px = img.data[(sy.clamp(0, h - 1) * w + sx.rem_euclid(w.max(1))) as usize];
        }
    });
    out
}

fn vr_glow(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let th = ctx.params.f("luminanceThreshold") as f32 / 100.0;
    let radius = ctx.params.f("glowRadius").max(0.0) * b.scale;
    let bright = ctx.params.f("glowBrightness") as f32 / 100.0;
    let sat = ctx.params.f("glowSaturation") as f32 / 100.0;
    let tint = ctx.params.b("useTintColor");
    let tc = ctx.params.color("tintColor");
    if bright <= 0.0 {
        return b;
    }
    b.img = per_eye(&b.img, layout(ctx), |e, _| {
        let mut hi = e.clone();
        hi.data.par_iter_mut().for_each(|p| {
            let (c, a) = unpremul(*p);
            let l = luminance(c[0], c[1], c[2]);
            let k = if l > th { ((l - th) / (1.0 - th).max(1e-3)).min(4.0) } else { 0.0 };
            let mut c = c.map(|v| v * k);
            let lc = luminance(c[0], c[1], c[2]);
            c = c.map(|v| lc + (v - lc) * sat);
            if tint {
                c = [lc * tc[0], lc * tc[1], lc * tc[2]];
            }
            *p = [c[0] * a, c[1] * a, c[2] * a, a * k.min(1.0)];
        });
        let g = sphere_blur(&hi, (radius * 0.5).max(0.5));
        let mut o = e.clone();
        o.data.par_iter_mut().zip(g.data.par_iter()).for_each(|(p, q)| {
            for c in 0..3 {
                p[c] += q[c] * bright;
            }
            p[3] = (p[3] + q[3] * bright).min(1.0);
        });
        o
    });
    b
}

fn rot_params(ctx: &EffectCtx) -> [[f64; 3]; 3] {
    rotation(ctx.params.f("tilt"), ctx.params.f("pan"), ctx.params.f("roll"))
}

fn vr_rotate(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let (t, pn, r) = (ctx.params.f("tilt"), ctx.params.f("pan"), ctx.params.f("roll"));
    if t == 0.0 && pn == 0.0 && r == 0.0 {
        return b;
    }
    let mut m = rot_params(ctx);
    if ctx.params.b("invertRotation") {
        // The inverse of a rotation is its transpose.
        m = [[m[0][0], m[1][0], m[2][0]], [m[0][1], m[1][1], m[2][1]], [m[0][2], m[1][2], m[2][2]]];
    }
    let eq = Proj { kind: 0, fov: 360.0 };
    b.img = per_eye(&b.img, layout(ctx), |e, _| convert(e, eq, eq, e.width, e.height, Some(&m)));
    b
}

fn vr_converter(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let from = Proj { kind: ctx.params.e("sourceProjection"), fov: ctx.params.f("sourceHorizontalFov") };
    let to = Proj { kind: ctx.params.e("targetProjection"), fov: ctx.params.f("targetHorizontalFov") };
    let (t, pn, r) = (ctx.params.f("tilt"), ctx.params.f("pan"), ctx.params.f("roll"));
    if from.kind == to.kind && (!matches!(from.kind, 4 | 5) || from.fov == to.fov) && t == 0.0 && pn == 0.0 && r == 0.0 {
        return b;
    }
    let m = rot_params(ctx);
    b.img = per_eye(&b.img, layout(ctx), |e, _| convert(e, from, to, e.width, e.height, Some(&m)));
    b
}

fn vr_sphere_to_plane(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let fov = ctx.params.f("horizontalFov");
    let m = rot_params(ctx);
    let eq = Proj { kind: 0, fov: 360.0 };
    let flat = Proj { kind: 5, fov };
    b.img = per_eye(&b.img, layout(ctx), |e, _| convert(e, eq, flat, e.width, e.height, Some(&m)));
    b
}

fn vr_plane_to_sphere(ctx: &EffectCtx, mut b: Buf) -> Buf {
    // The layer is a flat picture placed on the sphere, Scale degrees wide.
    let fov = ctx.params.f("scale").clamp(1.0, 179.0);
    let m = rot_params(ctx);
    let feather = (ctx.params.f("feather") / 100.0).clamp(0.0, 1.0);
    let flat = Proj { kind: 5, fov };
    let eq = Proj { kind: 0, fov: 360.0 };
    let src = b.img.clone();
    let mut out = convert(&src, flat, eq, src.width, src.height, Some(&m));
    if feather > 0.0 {
        let (w, h) = (src.width as f64, src.height as f64);
        let t = (fov / 2.0).to_radians().tan();
        out.rows_mut().for_each(|(y, row)| {
            for (x, px) in row.iter_mut().enumerate() {
                let d = mtv(&m, equi_dir(x as f64 + 0.5, y as f64 + 0.5, w, h));
                if d[2] <= 0.0 {
                    continue;
                }
                let u = (d[0] / d[2] / t).abs();
                let v = (d[1] / d[2] / (t * h / w)).abs();
                let edge = 1.0 - u.max(v);
                let k = (edge / feather).clamp(0.0, 1.0) as f32;
                for c in px.iter_mut() {
                    *c *= k;
                }
            }
        });
    }
    b.img = out;
    b
}

/// Angle (radians) between two directions.
#[inline]
fn angle(a: V3, b: V3) -> f64 {
    dot(a, b).clamp(-1.0, 1.0).acos()
}

fn vr_chromatic(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let k = [ctx.params.f("aberrationRed"), ctx.params.f("aberrationGreen"), ctx.params.f("aberrationBlue")].map(|v| v / 100.0 * 0.1);
    if k.iter().all(|v| *v == 0.0) {
        return b;
    }
    let center = mv(&rotation(ctx.params.f("centerTilt"), ctx.params.f("centerPan"), 0.0), [0.0, 0.0, 1.0]);
    let falloff = (ctx.params.f("falloff") / 100.0).clamp(0.0, 1.0);
    // Falloff Invert keeps the aberration near the point of interest instead of away from it.
    let invert = ctx.params.b("falloffInvert");
    b.img = per_eye(&b.img, layout(ctx), |e, _| {
        let (w, h) = (e.width as f64, e.height as f64);
        let mut o = Image::new(e.width, e.height);
        o.rows_mut().for_each(|(y, row)| {
            for (x, px) in row.iter_mut().enumerate() {
                let d = equi_dir(x as f64 + 0.5, y as f64 + 0.5, w, h);
                let th = angle(d, center);
                // Axis perpendicular to centre and d: rotate d about it toward/away from centre.
                let ax = [center[1] * d[2] - center[2] * d[1], center[2] * d[0] - center[0] * d[2], center[0] * d[1] - center[1] * d[0]];
                let al = dot(ax, ax).sqrt();
                let weight = (th / PI).powf(1.0 + falloff * 3.0);
                let weight = if invert { 1.0 - weight } else { weight };
                let mut out = [0.0f32; 4];
                for c in 0..3 {
                    let s = if al < 1e-9 {
                        e.data[(y * e.width as usize) + x]
                    } else {
                        let new_th = th * (1.0 - k[c] * weight);
                        // Direction at angle new_th from the centre in the plane of (centre, d).
                        let perp = norm([d[0] - center[0] * th.cos(), d[1] - center[1] * th.cos(), d[2] - center[2] * th.cos()]);
                        let nd = [
                            center[0] * new_th.cos() + perp[0] * new_th.sin(),
                            center[1] * new_th.cos() + perp[1] * new_th.sin(),
                            center[2] * new_th.cos() + perp[2] * new_th.sin(),
                        ];
                        let (sx, sy) = equi_px(nd, w, h);
                        sample_equi(e, sx, sy)
                    };
                    out[c] = s[c];
                    out[3] = out[3].max(s[3]);
                }
                *px = out;
            }
        });
        o
    });
    b
}

const BLEND_OPTS: [&str; 6] = ["None", "Normal", "Add", "Multiply", "Screen", "Overlay"];

fn blend_mode(i: u32) -> Option<BlendMode> {
    match i {
        0 => None,
        2 => Some(BlendMode::Add),
        3 => Some(BlendMode::Multiply),
        4 => Some(BlendMode::Screen),
        5 => Some(BlendMode::Overlay),
        _ => Some(BlendMode::Normal),
    }
}

/// Composite a generated eye image over the original with a blend mode and opacity (mode None
/// replaces the layer).
fn composite(orig: &Image, layer: &Image, mode: u32, opacity: f32) -> Image {
    let mut o = orig.clone();
    o.data.par_iter_mut().zip(layer.data.par_iter()).for_each(|(d, s)| {
        let s = [s[0] * opacity, s[1] * opacity, s[2] * opacity, s[3] * opacity];
        *d = match blend_mode(mode) {
            None => s,
            Some(m) => {
                let mut r = blend_pixel(m, *d, s, 0.5);
                // Keep the layer's own coverage: generated pixels show only where it is opaque.
                let a = d[3];
                if a < 1.0 {
                    let k = a / r[3].max(1e-6);
                    if k < 1.0 {
                        r = r.map(|v| v * k);
                    }
                }
                r
            }
        };
    });
    o
}

fn vr_gradients(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let pr = ctx.params;
    let blend = pr.f("blend").max(0.1);
    let opacity = (pr.f("opacity") / 100.0) as f32;
    let mode = pr.e("blendingMode");
    let (lw, lh) = (ctx.layer_size[0].max(1.0), ctx.layer_size[1].max(1.0));
    let pts: Vec<(V3, [f32; 4])> = (1..=5)
        .filter(|i| pr.b(&format!("enablePoint{i}")))
        .map(|i| {
            let p = pr.v2(&format!("point{i}"));
            (equi_dir(p[0], p[1], lw, lh), pr.color(&format!("color{i}")))
        })
        .collect();
    if pts.is_empty() {
        return b;
    }
    b.img = per_eye(&b.img, layout(ctx), |e, _| {
        let (w, h) = (e.width as f64, e.height as f64);
        let mut g = Image::new(e.width, e.height);
        g.rows_mut().for_each(|(y, row)| {
            for (x, px) in row.iter_mut().enumerate() {
                let d = equi_dir(x as f64 + 0.5, y as f64 + 0.5, w, h);
                // Inverse-distance weighting on great-circle distance.
                let mut acc = [0.0f64; 3];
                let mut ws = 0.0;
                let mut exact = None;
                for (pd, c) in &pts {
                    let a = angle(d, *pd);
                    if a < 1e-6 {
                        exact = Some(*c);
                        break;
                    }
                    let wgt = 1.0 / a.powf(blend);
                    for k in 0..3 {
                        acc[k] += c[k] as f64 * wgt;
                    }
                    ws += wgt;
                }
                let c = exact.map(|c| [c[0], c[1], c[2]]).unwrap_or([(acc[0] / ws) as f32, (acc[1] / ws) as f32, (acc[2] / ws) as f32]);
                *px = [c[0], c[1], c[2], 1.0];
            }
        });
        composite(e, &g, mode, opacity)
    });
    b
}

fn vr_fractal(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let pr = ctx.params;
    let kind = pr.e("fractalType");
    let invert = pr.b("invert");
    let contrast = pr.f("contrast") as f32 / 100.0;
    let brightness = pr.f("brightness") as f32 / 100.0;
    let scale = (pr.f("transform/scale") / 100.0).max(0.01) as f32;
    let rot = rotation(pr.f("transform/tilt"), pr.f("transform/pan"), pr.f("transform/roll"));
    let infl = (pr.get("subSettings/subInfluence").map(Value::as_f64).unwrap_or(50.0) / 100.0).clamp(0.0, 1.0) as f32;
    let sub = (pr.get("subSettings/subScaling").map(Value::as_f64).unwrap_or(50.0) / 100.0).clamp(0.1, 1.0) as f32;
    let octaves = pr.f("complexity").clamp(1.0, 20.0);
    let evo = (pr.f("evolution") / 360.0) as f32;
    let opacity = (pr.f("opacity") / 100.0) as f32;
    let mode = pr.e("blendingMode");
    let seed = pr.f("randomSeed") as u32;
    let base = 4.0 / scale;
    b.img = per_eye(&b.img, layout(ctx), |e, _| {
        let (w, h) = (e.width as f64, e.height as f64);
        let mut g = Image::new(e.width, e.height);
        g.rows_mut().for_each(|(y, row)| {
            for (x, px) in row.iter_mut().enumerate() {
                let d = mv(&rot, equi_dir(x as f64 + 0.5, y as f64 + 0.5, w, h));
                let mut v = 0.0f32;
                let mut best = 0.0f32;
                let mut amp = 1.0f32;
                let mut freq = base;
                let mut norm = 0.0f32;
                let n_oct = octaves.ceil() as usize;
                for o in 0..n_oct {
                    let wgt = if o + 1 == n_oct && octaves.fract() > 0.0 { octaves.fract() as f32 } else { 1.0 };
                    let q = [d[0] as f32 * freq + evo * 0.37, d[1] as f32 * freq + evo * 0.71, d[2] as f32 * freq + evo * 0.59];
                    let n = value_noise(q[0] + o as f32 * 17.3, q[1], q[2], seed.wrapping_add(o as u32));
                    let n = match kind {
                        1 => 1.0 - (n * 2.0 - 1.0).abs(),
                        // Strings: thin bright lines along the noise's zero crossings.
                        3 => 1.0 - ((n * 2.0 - 1.0).abs() / 0.12).clamp(0.0, 1.0).powi(2),
                        _ => n,
                    };
                    best = best.max(n * amp);
                    v += n * amp * wgt;
                    norm += amp * wgt;
                    amp *= infl;
                    freq /= sub;
                }
                // Max keeps the strongest layer rather than the weighted sum.
                let mut v = if kind == 2 { best } else { v / norm.max(1e-6) };
                v = (v - 0.5) * contrast + 0.5 + brightness;
                if invert {
                    v = 1.0 - v;
                }
                let v = v.clamp(0.0, 1.0);
                *px = [v, v, v, 1.0];
            }
        });
        composite(e, &g, mode, opacity)
    });
    b
}

/// VR Digital Glitch: horizontal bands of the panorama jump sideways (Geometric Distortion,
/// split into Horizontal / Vertical displacement), their colour channels split apart (Color
/// Distortion with per-channel offsets) and scan lines darken alternate rows (Scanline
/// Spacing apart). The pattern re-randomises Distortion Rate times a second, and Distortion
/// Evolution (optionally cycling) moves it smoothly in between. Target Point limits the glitch
/// to a region of the sphere (Radius, Feather; radius 0 = everywhere).
fn vr_glitch(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let pr = ctx.params;
    let amp = (pr.f("masterAmplitude") / 100.0).max(0.0);
    if amp <= 0.0 {
        return b;
    }
    let g = |id: &str, d: f64| pr.get(id).map(Value::as_f64).unwrap_or(d);
    let rate = pr.f("distortionRate").max(0.0);
    let geo = pr.f("geometricDistortion") / 100.0 * amp;
    let (hdisp, vdisp) = (g("geometric/horizontalDisplacement", 100.0) / 100.0, g("geometric/verticalDisplacement", 0.0) / 100.0);
    let complexity = pr.f("distortionComplexity").clamp(1.0, 100.0);
    let rgb = pr.f("colorDistortion") / 100.0 * amp;
    let offs = [g("color/redOffset", -1.0), g("color/greenOffset", 0.0), g("color/blueOffset", 1.0)];
    let scan = (pr.f("scanlines") / 100.0 * amp) as f32;
    let spacing = g("scanlineSpacing", 2.0).round().max(2.0) as usize;
    let seed = (pr.f("randomSeed") as u32) ^ ctx.seed;
    // Evolution: a fraction of a slot that blends towards the next pattern.
    let mut evo = g("distortionEvolution", 0.0) / 360.0;
    if pr.b("evolutionOptions/cycleEvolution") {
        evo = evo.rem_euclid(g("evolutionOptions/cycle", 1.0).round().max(1.0));
    }
    let base = if rate > 0.0 { ctx.time * rate } else { 0.0 } + evo;
    let slot = base.floor() as i64 as u32;
    let frac = base - base.floor();
    // Target Point.
    let target = mv(&rotation(g("target/tilt", 0.0), g("target/pan", 0.0), 0.0), [0.0, 0.0, 1.0]);
    let radius = g("target/radius", 0.0).to_radians();
    let feather = g("target/feather", 0.0).to_radians();
    b.img = per_eye(&b.img, layout(ctx), |e, eye| {
        let (w, h) = (e.width as f64, e.height as f64);
        let bands = complexity.round() as u32;
        let mut o = Image::new(e.width, e.height);
        o.rows_mut().for_each(|(y, row)| {
            let band = ((y as f64 / h) * bands as f64) as u32;
            let at = |s: u32| {
                let r0 = hash1(band, s, seed ^ (eye as u32 * 977));
                let on = r0 < 0.35;
                let shift = if on { (hash1(band, s + 1, seed) as f64 * 2.0 - 1.0) * geo } else { 0.0 };
                let split = if on { rgb * w * 0.01 * (1.0 + hash1(band, s + 2, seed) as f64) } else { 0.0 };
                (shift, split)
            };
            let (s0, p0) = at(slot);
            let (shift, split) = if frac > 0.0 {
                let (s1, p1) = at(slot + 1);
                (s0 + (s1 - s0) * frac, p0 + (p1 - p0) * frac)
            } else {
                (s0, p0)
            };
            let (dx, dy) = (shift * w * 0.15 * hdisp, shift * h * 0.15 * vdisp);
            let dark = if y % spacing == 0 { 1.0 - scan * 0.5 } else { 1.0 };
            for (x, px) in row.iter_mut().enumerate() {
                let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
                let k = if radius > 0.0 {
                    let a = angle(equi_dir(fx, fy, w, h), target);
                    if a <= radius {
                        1.0
                    } else if feather > 0.0 {
                        (1.0 - (a - radius) / feather).clamp(0.0, 1.0)
                    } else {
                        0.0
                    }
                } else {
                    1.0
                };
                let orig = e.data[y * e.width as usize + x];
                if k <= 0.0 {
                    *px = orig;
                    continue;
                }
                let (sx, sy) = (fx - dx * k, (fy - dy * k).clamp(0.5, h - 0.5));
                // Horizontal shifts wrap around the seam (the frame is a full turn).
                let c: [Px; 3] = std::array::from_fn(|i| sample_equi(e, sx + offs[i] * split * k, sy));
                let d = 1.0 - (1.0 - dark) * k as f32;
                *px = [c[0][0] * d, c[1][1] * d, c[2][2] * d, c[0][3].max(c[1][3]).max(c[2][3])];
            }
        });
        o
    });
    b
}

fn ang(id: &'static str, name: &'static str) -> crate::ParamSpec {
    p(id, name, num(0.0), ParamUi::Angle)
}

pub fn specs() -> Vec<EffectSpec> {
    let pct = |d: f64| (num(d), slider(0.0, 100.0, 0.0, 100.0, 1));
    let mut grad = vec![layout_param()];
    let defaults = [
        ([0.25, 0.5], [1.0, 0.0, 0.0]),
        ([0.5, 0.25], [1.0, 1.0, 0.0]),
        ([0.75, 0.5], [0.0, 0.0, 1.0]),
        ([0.5, 0.75], [0.0, 1.0, 0.0]),
        ([0.0, 0.5], [1.0, 0.0, 1.0]),
    ];
    const IDS: [[&str; 6]; 5] = [
        ["enablePoint1", "Enable Point 1", "point1", "Point 1", "color1", "Color 1"],
        ["enablePoint2", "Enable Point 2", "point2", "Point 2", "color2", "Color 2"],
        ["enablePoint3", "Enable Point 3", "point3", "Point 3", "color3", "Color 3"],
        ["enablePoint4", "Enable Point 4", "point4", "Point 4", "color4", "Color 4"],
        ["enablePoint5", "Enable Point 5", "point5", "Point 5", "color5", "Color 5"],
    ];
    for (i, (ids, (pt, c))) in IDS.iter().zip(defaults).enumerate() {
        grad.push(p(ids[0], ids[1], Value::Bool(i < 4), ParamUi::Checkbox));
        grad.push(p(ids[2], ids[3], Value::Vec2(pt), ParamUi::Point));
        grad.push(p(ids[4], ids[5], col(c[0], c[1], c[2]), ParamUi::Color));
    }
    grad.push(p("blend", "Gradient Power", num(2.0), slider(0.1, 10.0, 0.5, 6.0, 2)));
    let (v, u) = pct(100.0);
    grad.push(p("opacity", "Opacity", v, u));
    grad.push(p("blendingMode", "Blending Mode", Value::Enum(0), popup(&BLEND_OPTS)));
    vec![
        spec("ec.vr.blur", "VR Blur", vec![layout_param(), p("blurriness", "Blurriness", num(10.0), slider(0.0, 1000.0, 0.0, 100.0, 1))], vr_blur),
        spec(
            "ec.vr.chromaticaberrations",
            "VR Chromatic Aberrations",
            vec![
                layout_param(),
                ang("centerTilt", "Center Tilt"),
                ang("centerPan", "Center Pan"),
                p("aberrationRed", "Aberration (Red)", num(10.0), slider(-100.0, 100.0, -100.0, 100.0, 1)),
                p("aberrationGreen", "Aberration (Green)", num(0.0), slider(-100.0, 100.0, -100.0, 100.0, 1)),
                p("aberrationBlue", "Aberration (Blue)", num(-10.0), slider(-100.0, 100.0, -100.0, 100.0, 1)),
                p("falloff", "Falloff Distance", num(50.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("falloffInvert", "Falloff Invert", Value::Bool(false), ParamUi::Checkbox),
            ],
            vr_chromatic,
        ),
        spec("ec.vr.colorgradients", "VR Color Gradients", grad, vr_gradients),
        spec(
            "ec.vr.converter",
            "VR Converter",
            vec![
                layout_param(),
                p("sourceProjection", "Source Projection", Value::Enum(0), popup(&PROJECTIONS)),
                p("sourceHorizontalFov", "Source Horizontal FOV", num(90.0), slider(1.0, 360.0, 1.0, 360.0, 1)),
                p("targetProjection", "Target Projection", Value::Enum(2), popup(&PROJECTIONS)),
                p("targetHorizontalFov", "Target Horizontal FOV", num(90.0), slider(1.0, 360.0, 1.0, 360.0, 1)),
                ang("tilt", "Tilt (X Axis)"),
                ang("pan", "Pan (Y Axis)"),
                ang("roll", "Roll (Z Axis)"),
            ],
            vr_converter,
        ),
        spec(
            "ec.vr.denoise",
            "VR De-Noise",
            vec![
                layout_param(),
                p("noiseType", "Noise Type", Value::Enum(0), popup(&["Random Valued", "Salt-and-Pepper"])),
                p("noiseLevel", "Noise Level", num(20.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("detail", "Detail", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
            ],
            vr_denoise,
        ),
        spec(
            "ec.vr.digitalglitch",
            "VR Digital Glitch",
            vec![
                layout_param(),
                p("masterAmplitude", "Master Amplitude", num(100.0), slider(0.0, 500.0, 0.0, 200.0, 1)),
                p("distortionRate", "Distortion Rate", num(5.0), slider(0.0, 60.0, 0.0, 30.0, 2)),
                p("distortionComplexity", "Distortion Complexity", num(16.0), slider(1.0, 100.0, 1.0, 64.0, 0)),
                ang("distortionEvolution", "Distortion Evolution"),
                p("evolutionOptions/cycleEvolution", "Cycle Evolution", Value::Bool(false), ParamUi::Checkbox),
                p("evolutionOptions/cycle", "Cycle (in Revolutions)", num(1.0), slider(1.0, 1000.0, 1.0, 20.0, 0)),
                p("geometricDistortion", "Geometric Distortion", num(50.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("geometric/horizontalDisplacement", "Horizontal Displacement", num(100.0), slider(0.0, 400.0, 0.0, 200.0, 1)),
                p("geometric/verticalDisplacement", "Vertical Displacement", num(0.0), slider(0.0, 400.0, 0.0, 200.0, 1)),
                p("colorDistortion", "Color Distortion", num(50.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("color/redOffset", "Red Offset", num(-1.0), slider(-10.0, 10.0, -2.0, 2.0, 2)),
                p("color/greenOffset", "Green Offset", num(0.0), slider(-10.0, 10.0, -2.0, 2.0, 2)),
                p("color/blueOffset", "Blue Offset", num(1.0), slider(-10.0, 10.0, -2.0, 2.0, 2)),
                p("scanlines", "Scanlines", num(20.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("scanlineSpacing", "Scanline Spacing", num(2.0), slider(2.0, 100.0, 2.0, 20.0, 0)),
                ang("target/tilt", "Target Tilt"),
                ang("target/pan", "Target Pan"),
                p("target/radius", "Target Radius", num(0.0), slider(0.0, 180.0, 0.0, 180.0, 1)),
                p("target/feather", "Target Feather", num(0.0), slider(0.0, 180.0, 0.0, 90.0, 1)),
                p("randomSeed", "Random Seed", num(0.0), slider(0.0, 10000.0, 0.0, 1000.0, 0)),
            ],
            vr_glitch,
        ),
        spec(
            "ec.vr.fractalnoise",
            "VR Fractal Noise",
            vec![
                layout_param(),
                p("fractalType", "Fractal Type", Value::Enum(0), popup(&["Basic", "Turbulent Sharp", "Max", "Strings"])),
                p("contrast", "Contrast", num(100.0), slider(0.0, 10000.0, 0.0, 400.0, 1)),
                p("brightness", "Brightness", num(0.0), slider(-10000.0, 10000.0, -200.0, 200.0, 1)),
                p("invert", "Invert", Value::Bool(false), ParamUi::Checkbox),
                p("complexity", "Complexity", num(6.0), slider(1.0, 20.0, 1.0, 20.0, 1)),
                ang("evolution", "Evolution"),
                p("transform/scale", "Scale", num(100.0), slider(1.0, 10000.0, 10.0, 600.0, 1)),
                ang("transform/tilt", "Tilt (X Axis)"),
                ang("transform/pan", "Pan (Y Axis)"),
                ang("transform/roll", "Roll (Z Axis)"),
                p("subSettings/subInfluence", "Sub Influence", num(50.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("subSettings/subScaling", "Sub Scaling", num(50.0), slider(10.0, 100.0, 10.0, 100.0, 1)),
                p("randomSeed", "Random Seed", num(0.0), slider(0.0, 10000.0, 0.0, 1000.0, 0)),
                p("opacity", "Opacity", num(100.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("blendingMode", "Blending Mode", Value::Enum(0), popup(&BLEND_OPTS)),
            ],
            vr_fractal,
        ),
        spec(
            "ec.vr.glow",
            "VR Glow",
            vec![
                layout_param(),
                p("luminanceThreshold", "Luma Threshold", num(60.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("glowRadius", "Glow Radius", num(20.0), slider(0.0, 1000.0, 0.0, 200.0, 1)),
                p("glowBrightness", "Glow Brightness", num(100.0), slider(0.0, 1000.0, 0.0, 400.0, 1)),
                p("glowSaturation", "Glow Saturation", num(100.0), slider(0.0, 400.0, 0.0, 200.0, 1)),
                p("useTintColor", "Use Tint Color", Value::Bool(false), ParamUi::Checkbox),
                p("tintColor", "Tint Color", col(1.0, 0.8, 0.5), ParamUi::Color),
            ],
            vr_glow,
        ),
        spec(
            "ec.vr.planetosphere",
            "VR Plane to Sphere",
            vec![
                layout_param(),
                p("scale", "Scale (Degrees)", num(90.0), slider(1.0, 179.0, 1.0, 179.0, 1)),
                p("feather", "Feather", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                ang("tilt", "Tilt (X Axis)"),
                ang("pan", "Pan (Y Axis)"),
                ang("roll", "Roll (Z Axis)"),
            ],
            vr_plane_to_sphere,
        ),
        spec(
            "ec.vr.rotatesphere",
            "VR Rotate Sphere",
            vec![
                layout_param(),
                p("invertRotation", "Invert Rotation", Value::Bool(false), ParamUi::Checkbox),
                ang("tilt", "Tilt (X Axis)"),
                ang("pan", "Pan (Y Axis)"),
                ang("roll", "Roll (Z Axis)"),
            ],
            vr_rotate,
        ),
        spec(
            "ec.vr.sharpen",
            "VR Sharpen",
            vec![layout_param(), p("sharpenAmount", "Sharpen Amount", num(20.0), slider(0.0, 100.0, 0.0, 100.0, 1))],
            vr_sharpen,
        ),
        spec(
            "ec.vr.spheretoplane",
            "VR Sphere to Plane",
            vec![
                layout_param(),
                p("horizontalFov", "Horizontal FOV", num(90.0), slider(1.0, 179.0, 10.0, 170.0, 1)),
                ang("tilt", "Tilt (X Axis)"),
                ang("pan", "Pan (Y Axis)"),
                ang("roll", "Roll (Z Axis)"),
            ],
            vr_sphere_to_plane,
        ),
    ]
}

#[cfg(test)]
mod tests {
    /// Fail-before: `wrap_pad` of an empty frame panicked in `clamp(0, -1)`.
    #[test]
    fn wrap_pad_survives_empty_frames() {
        let empty = effectcraft_raster::Image::new(4, 0);
        assert!(super::wrap_pad(&empty, 3).is_empty());
    }

    use super::*;
    use crate::{EffectEnv, run_fx};

    /// A smooth function of the view direction rendered as an equirectangular image.
    fn sphere_img(w: u32, h: u32) -> Image {
        let mut img = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let d = equi_dir(x as f64 + 0.5, y as f64 + 0.5, w as f64, h as f64);
                img.set(x, y, [(0.5 + 0.4 * d[0]) as f32, (0.5 + 0.4 * d[1]) as f32, (0.5 + 0.4 * d[2]) as f32, 1.0]);
            }
        }
        img
    }

    #[test]
    fn projections_are_inverse() {
        for kind in 0..9u32 {
            let pr = Proj { kind, fov: if kind == 4 { 360.0 } else { 100.0 } };
            let (w, h) = (240.0, 120.0);
            for (x, y) in [(10.5, 20.5), (100.5, 60.5), (230.5, 110.5), (130.5, 33.5)] {
                let Some(d) = pr.dir(x, y, w, h) else { continue };
                let (px, py, _) = pr.pos(d, w, h).unwrap();
                assert!((px - x).abs() < 1e-6 && (py - y).abs() < 1e-6, "kind {kind}: ({x},{y}) → ({px},{py})");
            }
        }
    }

    #[test]
    fn converter_round_trips_equirect_through_cube_maps() {
        let img = sphere_img(128, 64);
        for target in [1u32, 2, 3, 6, 7] {
            let cube = run_fx("ec.vr.converter", &[("targetProjection", Value::Enum(target))], img.clone(), 0.0, EffectEnv::default());
            let back = run_fx(
                "ec.vr.converter",
                &[("sourceProjection", Value::Enum(target)), ("targetProjection", Value::Enum(0))],
                cube.img.clone(),
                0.0,
                EffectEnv::default(),
            );
            let mut worst = 0.0f32;
            let mut sum = 0.0f32;
            for (a, b) in back.img.data.iter().zip(&img.data) {
                let e = (0..3).map(|k| (a[k] - b[k]).abs()).fold(0.0, f32::max);
                worst = worst.max(e);
                sum += e;
            }
            let mean = sum / img.data.len() as f32;
            // The 6:1 strip squeezes each face into a narrow cell, so its seams are coarser.
            let tol = if target == 3 { 0.25 } else { 0.08 };
            assert!(mean < 0.01 && worst < tol, "target {target}: mean {mean} worst {worst}");
            // Cube faces carry the expected directions: the front face centre looks at +Z.
            let pr = Proj { kind: target, fov: 90.0 };
            let (x, y, _) = pr.pos([0.0, 0.0, 1.0], 128.0, 64.0).unwrap();
            let px = cube.img.sample_bilinear_clamped(x, y);
            assert!((px[2] - 0.9).abs() < 0.02, "{px:?}");
        }
    }

    #[test]
    fn digital_glitch_full_controls() {
        let img = sphere_img(128, 64);
        let run = |vals: &[(&str, Value)], t: f64| run_fx("ec.vr.digitalglitch", vals, img.clone(), t, EffectEnv::default()).img;
        let base = run(&[], 0.0);
        for (id, v) in
            [("distortionEvolution", num(90.0)), ("geometric/verticalDisplacement", num(100.0)), ("color/redOffset", num(3.0)), ("scanlineSpacing", num(5.0))]
        {
            assert_ne!(run(&[(id, v.clone())], 0.0), base, "{id}");
        }
        // Cycle Evolution: one full cycle is the start again.
        let cyc =
            |e: f64| run(&[("distortionEvolution", num(e)), ("evolutionOptions/cycleEvolution", Value::Bool(true)), ("evolutionOptions/cycle", num(1.0))], 0.0);
        assert_eq!(cyc(0.0), cyc(360.0));
        // Target Point: a small region around the front; the back stays untouched.
        let t = run(&[("target/radius", num(30.0))], 0.0);
        assert_eq!(t.get(0, 32), img.get(0, 32), "behind the viewer");
    }

    #[test]
    fn converter_fulldome_and_eac() {
        let img = sphere_img(128, 64);
        let dome = run_fx("ec.vr.converter", &[("targetProjection", Value::Enum(8))], img.clone(), 0.0, EffectEnv::default()).img;
        // The centre looks at the zenith (+Y); the bottom edge at the front (+Z).
        let c = dome.get(64, 32);
        assert!((c[1] - 0.9).abs() < 0.03, "{c:?}");
        let f = dome.get(64, 62);
        assert!(f[2] > 0.8, "{f:?}");
        // EAC faces are equi-angular: a quarter of the front face sits at 22.5 degrees.
        let eac = Proj { kind: 7, fov: 90.0 };
        let cw = 128.0 / 3.0;
        let d = eac.dir(cw + 0.25 * cw, 16.0, 128.0, 64.0).unwrap();
        assert!((d[0].atan2(d[2]).to_degrees() + 22.5).abs() < 0.5, "{d:?}");
    }

    #[test]
    fn rotate_sphere_pans_and_round_trips() {
        let img = sphere_img(96, 48);
        let a = run_fx("ec.vr.rotatesphere", &[("pan", num(90.0))], img.clone(), 0.0, EffectEnv::default());
        let b = run_fx("ec.vr.rotatesphere", &[("pan", num(-90.0))], a.img.clone(), 0.0, EffectEnv::default());
        let err: f32 = b.img.data.iter().zip(&img.data).map(|(p, q)| (p[0] - q[0]).abs()).sum::<f32>() / img.data.len() as f32;
        assert!(err < 0.01, "{err}");
        // A 90° pan is a horizontal shift by a quarter turn.
        let (x, y) = (10u32, 24u32);
        let shifted = img.get(((x + 24) % 96) as i64, y as i64);
        let got = a.img.get(x as i64, y as i64);
        let alt = img.get(((x + 72) % 96) as i64, y as i64);
        assert!((got[0] - shifted[0]).abs() < 0.02 || (got[0] - alt[0]).abs() < 0.02);
    }

    #[test]
    fn blur_is_seamless() {
        // A frame whose left and right edges differ: after blurring, the two edge columns agree
        // (they are neighbours on the sphere).
        let mut img = Image::new(64, 32);
        for y in 0..32 {
            for x in 0..64 {
                let v = if x < 4 {
                    1.0
                } else if x >= 60 {
                    0.0
                } else {
                    0.5
                };
                img.set(x, y, [v, v, v, 1.0]);
            }
        }
        let out = run_fx("ec.vr.blur", &[("blurriness", num(8.0))], img.clone(), 0.0, EffectEnv::default());
        let (l, r) = (out.img.get(0, 16)[0], out.img.get(63, 16)[0]);
        assert!((l - r).abs() < 0.2, "{l} {r}");
        assert!(r > 0.2 && l < 0.85);
        // Pole rows are uniform (every pixel of the top row is the same point).
        let top: Vec<f32> = (0..64).map(|x| out.img.get(x, 0)[0]).collect();
        let spread = top.iter().cloned().fold(f32::MIN, f32::max) - top.iter().cloned().fold(f32::MAX, f32::min);
        assert!(spread < 0.1, "{spread}");
        // Constant images stay constant.
        let flat = Image::filled(32, 16, [0.3, 0.3, 0.3, 1.0]);
        let o = run_fx("ec.vr.blur", &[], flat, 0.0, EffectEnv::default());
        assert!(o.img.data.iter().all(|p| (p[0] - 0.3).abs() < 1e-4));
    }

    #[test]
    fn stereo_layouts_process_each_eye() {
        let mut img = Image::new(32, 32);
        for y in 0..32 {
            for x in 0..32 {
                let v = if y < 16 { 0.2 } else { 0.8 };
                img.set(x, y, [v, v, v, 1.0]);
            }
        }
        // Over/under: blurring each eye keeps the eyes separate.
        let o = run_fx("ec.vr.blur", &[("frameLayout", Value::Enum(1)), ("blurriness", num(10.0))], img.clone(), 0.0, EffectEnv::default());
        assert!((o.img.get(5, 15)[0] - 0.2).abs() < 1e-4 && (o.img.get(5, 16)[0] - 0.8).abs() < 1e-4);
        let mono = run_fx("ec.vr.blur", &[("blurriness", num(10.0))], img, 0.0, EffectEnv::default());
        assert!((mono.img.get(5, 15)[0] - 0.2).abs() > 0.01);
    }

    #[test]
    fn sphere_to_plane_and_back() {
        let img = sphere_img(128, 64);
        // The centre of a 2D view looking forward sees +Z (blue 0.9).
        let v = run_fx("ec.vr.spheretoplane", &[], img.clone(), 0.0, EffectEnv::default());
        let c = v.img.get(64, 32);
        assert!((c[2] - 0.9).abs() < 0.02, "{c:?}");
        // Looking up (tilt 90°): +Y (green 0.9).
        let up = run_fx("ec.vr.spheretoplane", &[("tilt", num(-90.0))], img.clone(), 0.0, EffectEnv::default());
        let u = up.img.get(64, 32);
        let dn = run_fx("ec.vr.spheretoplane", &[("tilt", num(90.0))], img, 0.0, EffectEnv::default());
        let d = dn.img.get(64, 32);
        assert!((u[1] - 0.9).abs() < 0.03 || (d[1] - 0.9).abs() < 0.03, "{u:?} {d:?}");
        // Plane to sphere puts the picture in front, transparent behind.
        let pic = Image::filled(64, 32, [1.0, 0.0, 0.0, 1.0]);
        let s = run_fx("ec.vr.planetosphere", &[], pic, 0.0, EffectEnv::default());
        assert!(s.img.get(32, 16)[3] > 0.99 && s.img.get(1, 16)[3] < 0.01);
    }

    #[test]
    fn rotate_invert_undoes_rotation_and_options_take_effect() {
        let img = sphere_img(64, 32);
        let rot = [("pan", num(40.0)), ("tilt", num(15.0))];
        let a = run_fx("ec.vr.rotatesphere", &rot, img.clone(), 0.0, EffectEnv::default());
        let mut inv = rot.to_vec();
        inv.push(("invertRotation", Value::Bool(true)));
        let back = run_fx("ec.vr.rotatesphere", &inv, a.img.clone(), 0.0, EffectEnv::default());
        let c = back.img.get(32, 16);
        let o = img.get(32, 16);
        assert!((0..3).all(|k| (c[k] - o[k]).abs() < 0.08), "{c:?} {o:?}");
        // Falloff Invert, salt-and-pepper De-Noise and the VR fractal types all change the output.
        let f0 = run_fx("ec.vr.chromaticaberrations", &[], img.clone(), 0.0, EffectEnv::default());
        let f1 = run_fx("ec.vr.chromaticaberrations", &[("falloffInvert", Value::Bool(true))], img.clone(), 0.0, EffectEnv::default());
        assert_ne!(f0.img.data, f1.img.data);
        let d0 = run_fx("ec.vr.denoise", &[], img.clone(), 0.0, EffectEnv::default());
        let d1 = run_fx("ec.vr.denoise", &[("noiseType", Value::Enum(1))], img.clone(), 0.0, EffectEnv::default());
        assert_ne!(d0.img.data, d1.img.data);
        let n: Vec<_> =
            (0..4).map(|t| run_fx("ec.vr.fractalnoise", &[("fractalType", Value::Enum(t))], img.clone(), 0.0, EffectEnv::default()).img.data).collect();
        assert!(n[0] != n[1] && n[1] != n[2] && n[2] != n[3]);
    }

    /// Fail-before: a Noise Level far past the slider (an expression) sized the padding and the
    /// guided filter's planes by it, many GB on a 4K frame (an allocation abort). The radius is
    /// capped; slider values are unchanged.
    #[test]
    fn denoise_with_a_hostile_noise_level_stays_bounded() {
        let img = sphere_img(64, 32);
        for t in [0, 1] {
            let out = run_fx("ec.vr.denoise", &[("noiseLevel", Value::Scalar(1.0e9)), ("noiseType", Value::Enum(t))], img.clone(), 0.0, EffectEnv::default());
            assert_eq!((out.img.width, out.img.height), (64, 32));
            assert!(out.img.data.iter().flatten().all(|v| v.is_finite()));
        }
        assert_eq!(super::wrap_pad_amount(usize::MAX), effectcraft_raster::MAX_PADDED_SIDE as usize / 2);
    }

    #[test]
    fn generators_and_filters_are_deterministic_and_seamless() {
        let img = sphere_img(64, 32);
        for (id, vals) in [
            ("ec.vr.fractalnoise", vec![]),
            ("ec.vr.colorgradients", vec![]),
            ("ec.vr.glow", vec![("luminanceThreshold", num(50.0))]),
            ("ec.vr.sharpen", vec![]),
            ("ec.vr.denoise", vec![]),
            ("ec.vr.chromaticaberrations", vec![]),
            ("ec.vr.digitalglitch", vec![]),
        ] {
            let a = run_fx(id, &vals, img.clone(), 0.5, EffectEnv::default());
            let b = run_fx(id, &vals, img.clone(), 0.5, EffectEnv::default());
            assert_eq!(a.img.data, b.img.data, "{id}");
            assert!(a.img.data.iter().all(|p| p.iter().all(|v| v.is_finite())), "{id}");
        }
        // Fractal noise is continuous across the seam: the first and last columns are close.
        let n = run_fx("ec.vr.fractalnoise", &[], img.clone(), 0.0, EffectEnv::default());
        let mut worst = 0.0f32;
        for y in 0..32 {
            worst = worst.max((n.img.get(0, y)[0] - n.img.get(63, y)[0]).abs());
        }
        assert!(worst < 0.15, "{worst}");
        // Gradients reach the point colour at the point.
        let pts = [
            ("point1", Value::Vec2([16.0, 16.0])),
            ("point2", Value::Vec2([32.0, 8.0])),
            ("point3", Value::Vec2([48.0, 16.0])),
            ("point4", Value::Vec2([32.0, 24.0])),
        ];
        let g = run_fx("ec.vr.colorgradients", &pts, img.clone(), 0.0, EffectEnv::default());
        let px = g.img.get(16, 16);
        assert!(px[0] > 0.8 && px[1] < 0.3, "{px:?}");
        // Glitch changes over time.
        let g0 = run_fx("ec.vr.digitalglitch", &[], img.clone(), 0.0, EffectEnv::default());
        let g1 = run_fx("ec.vr.digitalglitch", &[], img.clone(), 3.3, EffectEnv::default());
        assert_ne!(g0.img.data, g1.img.data);
    }
}
