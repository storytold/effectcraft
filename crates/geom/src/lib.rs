//! Geometry for EffectCraft: 2D/3D vectors, 3×3 (2D projective) and 4×4 matrices, rectangles and
//! the layer-transform composition used by the compositor.
//!
//! Conventions follow After Effects' public documentation: y grows downwards, angles are degrees
//! clockwise on screen, 3D uses a left-handed system with +z pointing into the screen, and a 3D
//! layer's rotation is Orientation (X, Y, Z) followed by X Rotation, Y Rotation, Z Rotation.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::ops::{Add, AddAssign, Div, Mul, MulAssign, Neg, Sub, SubAssign};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

pub const fn vec2(x: f64, y: f64) -> Vec2 {
    Vec2 { x, y }
}
pub const fn vec3(x: f64, y: f64, z: f64) -> Vec3 {
    Vec3 { x, y, z }
}

impl Vec2 {
    pub const ZERO: Vec2 = vec2(0.0, 0.0);
    pub const ONE: Vec2 = vec2(1.0, 1.0);
    pub fn dot(self, o: Vec2) -> f64 {
        self.x * o.x + self.y * o.y
    }
    pub fn cross(self, o: Vec2) -> f64 {
        self.x * o.y - self.y * o.x
    }
    pub fn length(self) -> f64 {
        self.dot(self).sqrt()
    }
    pub fn normalize(self) -> Vec2 {
        let l = self.length();
        if l > 0.0 { self / l } else { self }
    }
    pub fn lerp(self, o: Vec2, t: f64) -> Vec2 {
        self + (o - self) * t
    }
    pub fn extend(self, z: f64) -> Vec3 {
        vec3(self.x, self.y, z)
    }
    pub fn rotate_deg(self, deg: f64) -> Vec2 {
        let (s, c) = deg.to_radians().sin_cos();
        vec2(self.x * c - self.y * s, self.x * s + self.y * c)
    }
    pub fn mul_elem(self, o: Vec2) -> Vec2 {
        vec2(self.x * o.x, self.y * o.y)
    }
}

impl Vec3 {
    pub const ZERO: Vec3 = vec3(0.0, 0.0, 0.0);
    pub const ONE: Vec3 = vec3(1.0, 1.0, 1.0);
    pub fn dot(self, o: Vec3) -> f64 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    pub fn cross(self, o: Vec3) -> Vec3 {
        vec3(self.y * o.z - self.z * o.y, self.z * o.x - self.x * o.z, self.x * o.y - self.y * o.x)
    }
    pub fn length(self) -> f64 {
        self.dot(self).sqrt()
    }
    pub fn normalize(self) -> Vec3 {
        let l = self.length();
        if l > 0.0 { self / l } else { self }
    }
    pub fn lerp(self, o: Vec3, t: f64) -> Vec3 {
        self + (o - self) * t
    }
    pub fn xy(self) -> Vec2 {
        vec2(self.x, self.y)
    }
    pub fn mul_elem(self, o: Vec3) -> Vec3 {
        vec3(self.x * o.x, self.y * o.y, self.z * o.z)
    }
}

impl From<[f64; 3]> for Vec3 {
    fn from(a: [f64; 3]) -> Vec3 {
        vec3(a[0], a[1], a[2])
    }
}
impl From<[f64; 2]> for Vec2 {
    fn from(a: [f64; 2]) -> Vec2 {
        vec2(a[0], a[1])
    }
}

macro_rules! vec_ops {
    ($t:ident, $($f:ident),+) => {
        impl Add for $t { type Output = $t; fn add(self, o: $t) -> $t { $t { $($f: self.$f + o.$f),+ } } }
        impl Sub for $t { type Output = $t; fn sub(self, o: $t) -> $t { $t { $($f: self.$f - o.$f),+ } } }
        impl Mul<f64> for $t { type Output = $t; fn mul(self, k: f64) -> $t { $t { $($f: self.$f * k),+ } } }
        impl Div<f64> for $t { type Output = $t; fn div(self, k: f64) -> $t { $t { $($f: self.$f / k),+ } } }
        impl Neg for $t { type Output = $t; fn neg(self) -> $t { $t { $($f: -self.$f),+ } } }
        impl AddAssign for $t { fn add_assign(&mut self, o: $t) { $(self.$f += o.$f;)+ } }
        impl SubAssign for $t { fn sub_assign(&mut self, o: $t) { $(self.$f -= o.$f;)+ } }
        impl MulAssign<f64> for $t { fn mul_assign(&mut self, k: f64) { $(self.$f *= k;)+ } }
    };
}
vec_ops!(Vec2, x, y);
vec_ops!(Vec3, x, y, z);

/// Axis-aligned rectangle (min inclusive, max exclusive for pixel rects).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl Rect {
    pub const EMPTY: Rect = Rect { x0: f64::INFINITY, y0: f64::INFINITY, x1: f64::NEG_INFINITY, y1: f64::NEG_INFINITY };
    pub fn new(x0: f64, y0: f64, x1: f64, y1: f64) -> Rect {
        Rect { x0, y0, x1, y1 }
    }
    pub fn from_size(w: f64, h: f64) -> Rect {
        Rect { x0: 0.0, y0: 0.0, x1: w, y1: h }
    }
    pub fn width(&self) -> f64 {
        (self.x1 - self.x0).max(0.0)
    }
    pub fn height(&self) -> f64 {
        (self.y1 - self.y0).max(0.0)
    }
    pub fn is_empty(&self) -> bool {
        !(self.x1 > self.x0 && self.y1 > self.y0)
    }
    pub fn center(&self) -> Vec2 {
        vec2((self.x0 + self.x1) * 0.5, (self.y0 + self.y1) * 0.5)
    }
    pub fn union(&self, o: &Rect) -> Rect {
        Rect { x0: self.x0.min(o.x0), y0: self.y0.min(o.y0), x1: self.x1.max(o.x1), y1: self.y1.max(o.y1) }
    }
    pub fn intersect(&self, o: &Rect) -> Rect {
        Rect { x0: self.x0.max(o.x0), y0: self.y0.max(o.y0), x1: self.x1.min(o.x1), y1: self.y1.min(o.y1) }
    }
    pub fn include(&mut self, p: Vec2) {
        self.x0 = self.x0.min(p.x);
        self.y0 = self.y0.min(p.y);
        self.x1 = self.x1.max(p.x);
        self.y1 = self.y1.max(p.y);
    }
    pub fn inflate(&self, d: f64) -> Rect {
        Rect { x0: self.x0 - d, y0: self.y0 - d, x1: self.x1 + d, y1: self.y1 + d }
    }
    pub fn contains(&self, p: Vec2) -> bool {
        p.x >= self.x0 && p.x < self.x1 && p.y >= self.y0 && p.y < self.y1
    }
    pub fn corners(&self) -> [Vec2; 4] {
        [vec2(self.x0, self.y0), vec2(self.x1, self.y0), vec2(self.x1, self.y1), vec2(self.x0, self.y1)]
    }
    /// Smallest integer rect covering this one.
    pub fn round_out(&self) -> Rect {
        Rect { x0: self.x0.floor(), y0: self.y0.floor(), x1: self.x1.ceil(), y1: self.y1.ceil() }
    }
}

/// Row-major 3×3 matrix acting on column vectors `(x, y, 1)`; a 2D projective transform.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mat3(pub [[f64; 3]; 3]);

impl Default for Mat3 {
    fn default() -> Self {
        Mat3::IDENTITY
    }
}

impl Mat3 {
    pub const IDENTITY: Mat3 = Mat3([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
    pub fn translate(t: Vec2) -> Mat3 {
        Mat3([[1.0, 0.0, t.x], [0.0, 1.0, t.y], [0.0, 0.0, 1.0]])
    }
    pub fn scale(s: Vec2) -> Mat3 {
        Mat3([[s.x, 0.0, 0.0], [0.0, s.y, 0.0], [0.0, 0.0, 1.0]])
    }
    /// Clockwise on screen (y down) for positive degrees.
    pub fn rotate_deg(deg: f64) -> Mat3 {
        let (s, c) = deg.to_radians().sin_cos();
        Mat3([[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]])
    }
    pub fn skew_deg(skew: f64, axis_deg: f64) -> Mat3 {
        let k = skew.to_radians().tan();
        Mat3::rotate_deg(axis_deg) * Mat3([[1.0, k, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]) * Mat3::rotate_deg(-axis_deg)
    }
    /// The After Effects 2D layer transform: layer space → parent space.
    /// `p' = position + R(rotation) · S(scale/100) · (p - anchor)`.
    pub fn layer_2d(anchor: Vec2, position: Vec2, scale_pct: Vec2, rotation_deg: f64) -> Mat3 {
        Mat3::translate(position) * Mat3::rotate_deg(rotation_deg) * Mat3::scale(scale_pct / 100.0) * Mat3::translate(-anchor)
    }
    pub fn apply(&self, p: Vec2) -> Vec2 {
        let m = &self.0;
        let x = m[0][0] * p.x + m[0][1] * p.y + m[0][2];
        let y = m[1][0] * p.x + m[1][1] * p.y + m[1][2];
        let w = m[2][0] * p.x + m[2][1] * p.y + m[2][2];
        if w != 1.0 && w != 0.0 { vec2(x / w, y / w) } else { vec2(x, y) }
    }
    /// Apply to a direction (ignores translation; affine only).
    pub fn apply_vec(&self, v: Vec2) -> Vec2 {
        let m = &self.0;
        vec2(m[0][0] * v.x + m[0][1] * v.y, m[1][0] * v.x + m[1][1] * v.y)
    }
    pub fn is_affine(&self) -> bool {
        self.0[2][0] == 0.0 && self.0[2][1] == 0.0 && (self.0[2][2] - 1.0).abs() < 1e-12
    }
    pub fn determinant(&self) -> f64 {
        let m = &self.0;
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    }
    pub fn inverse(&self) -> Option<Mat3> {
        let d = self.determinant();
        if d.abs() < 1e-300 || !d.is_finite() {
            return None;
        }
        let m = &self.0;
        let inv = [
            [(m[1][1] * m[2][2] - m[1][2] * m[2][1]) / d, (m[0][2] * m[2][1] - m[0][1] * m[2][2]) / d, (m[0][1] * m[1][2] - m[0][2] * m[1][1]) / d],
            [(m[1][2] * m[2][0] - m[1][0] * m[2][2]) / d, (m[0][0] * m[2][2] - m[0][2] * m[2][0]) / d, (m[0][2] * m[1][0] - m[0][0] * m[1][2]) / d],
            [(m[1][0] * m[2][1] - m[1][1] * m[2][0]) / d, (m[0][1] * m[2][0] - m[0][0] * m[2][1]) / d, (m[0][0] * m[1][1] - m[0][1] * m[1][0]) / d],
        ];
        Some(Mat3(inv))
    }
    /// Bounding box of a rect after transformation.
    pub fn map_rect(&self, r: &Rect) -> Rect {
        let mut out = Rect::EMPTY;
        for c in r.corners() {
            out.include(self.apply(c));
        }
        out
    }
    /// Average linear scale factor (for stroke widths, blur radii in transformed space).
    pub fn mean_scale(&self) -> f64 {
        self.determinant().abs().sqrt()
    }
    /// Projective transform mapping the unit square corners to `q` (in order (0,0),(1,0),(1,1),(0,1)).
    pub fn square_to_quad(q: [Vec2; 4]) -> Mat3 {
        let (x0, y0, x1, y1, x2, y2, x3, y3) = (q[0].x, q[0].y, q[1].x, q[1].y, q[2].x, q[2].y, q[3].x, q[3].y);
        let dx1 = x1 - x2;
        let dx2 = x3 - x2;
        let dy1 = y1 - y2;
        let dy2 = y3 - y2;
        let sx = x0 - x1 + x2 - x3;
        let sy = y0 - y1 + y2 - y3;
        if sx.abs() < 1e-12 && sy.abs() < 1e-12 {
            return Mat3([[x1 - x0, x3 - x0, x0], [y1 - y0, y3 - y0, y0], [0.0, 0.0, 1.0]]);
        }
        let den = dx1 * dy2 - dx2 * dy1;
        if den == 0.0 || !den.is_finite() {
            return Mat3::IDENTITY;
        }
        let g = (sx * dy2 - dx2 * sy) / den;
        let h = (dx1 * sy - sx * dy1) / den;
        Mat3([[x1 - x0 + g * x1, x3 - x0 + h * x3, x0], [y1 - y0 + g * y1, y3 - y0 + h * y3, y0], [g, h, 1.0]])
    }
}

impl Mul for Mat3 {
    type Output = Mat3;
    fn mul(self, o: Mat3) -> Mat3 {
        let mut r = [[0.0; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                r[i][j] = (0..3).map(|k| self.0[i][k] * o.0[k][j]).sum();
            }
        }
        Mat3(r)
    }
}

/// Row-major 4×4 matrix on column vectors `(x, y, z, 1)`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mat4(pub [[f64; 4]; 4]);

impl Default for Mat4 {
    fn default() -> Self {
        Mat4::IDENTITY
    }
}

impl Mat4 {
    pub const IDENTITY: Mat4 = Mat4([[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]]);
    pub fn translate(t: Vec3) -> Mat4 {
        let mut m = Mat4::IDENTITY;
        m.0[0][3] = t.x;
        m.0[1][3] = t.y;
        m.0[2][3] = t.z;
        m
    }
    pub fn scale(s: Vec3) -> Mat4 {
        let mut m = Mat4::IDENTITY;
        m.0[0][0] = s.x;
        m.0[1][1] = s.y;
        m.0[2][2] = s.z;
        m
    }
    pub fn rotate_x(deg: f64) -> Mat4 {
        let (s, c) = deg.to_radians().sin_cos();
        Mat4([[1.0, 0.0, 0.0, 0.0], [0.0, c, -s, 0.0], [0.0, s, c, 0.0], [0.0, 0.0, 0.0, 1.0]])
    }
    pub fn rotate_y(deg: f64) -> Mat4 {
        let (s, c) = deg.to_radians().sin_cos();
        Mat4([[c, 0.0, s, 0.0], [0.0, 1.0, 0.0, 0.0], [-s, 0.0, c, 0.0], [0.0, 0.0, 0.0, 1.0]])
    }
    pub fn rotate_z(deg: f64) -> Mat4 {
        let (s, c) = deg.to_radians().sin_cos();
        Mat4([[c, -s, 0.0, 0.0], [s, c, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]])
    }
    /// Orientation (degrees, X then Y then Z applied to the vector in that order).
    pub fn orientation(o: Vec3) -> Mat4 {
        Mat4::rotate_z(o.z) * Mat4::rotate_y(o.y) * Mat4::rotate_x(o.x)
    }
    /// The 3D layer transform: layer space → parent space.
    pub fn layer_3d(anchor: Vec3, position: Vec3, scale_pct: Vec3, orientation: Vec3, rotation: Vec3) -> Mat4 {
        Mat4::translate(position)
            * Mat4::orientation(orientation)
            * Mat4::rotate_z(rotation.z)
            * Mat4::rotate_y(rotation.y)
            * Mat4::rotate_x(rotation.x)
            * Mat4::scale(scale_pct / 100.0)
            * Mat4::translate(-anchor)
    }
    /// Embed a 2D affine transform (z untouched).
    pub fn from_mat3_affine(m: &Mat3) -> Mat4 {
        let a = &m.0;
        Mat4([[a[0][0], a[0][1], 0.0, a[0][2]], [a[1][0], a[1][1], 0.0, a[1][2]], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]])
    }
    pub fn apply(&self, p: Vec3) -> Vec3 {
        let m = &self.0;
        let x = m[0][0] * p.x + m[0][1] * p.y + m[0][2] * p.z + m[0][3];
        let y = m[1][0] * p.x + m[1][1] * p.y + m[1][2] * p.z + m[1][3];
        let z = m[2][0] * p.x + m[2][1] * p.y + m[2][2] * p.z + m[2][3];
        let w = m[3][0] * p.x + m[3][1] * p.y + m[3][2] * p.z + m[3][3];
        if w != 1.0 && w != 0.0 { vec3(x / w, y / w, z / w) } else { vec3(x, y, z) }
    }
    pub fn apply_vec(&self, v: Vec3) -> Vec3 {
        let m = &self.0;
        vec3(m[0][0] * v.x + m[0][1] * v.y + m[0][2] * v.z, m[1][0] * v.x + m[1][1] * v.y + m[1][2] * v.z, m[2][0] * v.x + m[2][1] * v.y + m[2][2] * v.z)
    }
    pub fn transpose(&self) -> Mat4 {
        let mut r = [[0.0; 4]; 4];
        for i in 0..4 {
            for j in 0..4 {
                r[i][j] = self.0[j][i];
            }
        }
        Mat4(r)
    }
    /// General inverse (Gauss-Jordan with partial pivoting).
    pub fn inverse(&self) -> Option<Mat4> {
        if !self.0.iter().flatten().all(|v| v.is_finite()) {
            return None;
        }
        let mut a = self.0;
        let mut inv = Mat4::IDENTITY.0;
        for col in 0..4 {
            let piv = (col..4).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
            if a[piv][col].abs() < 1e-300 {
                return None;
            }
            a.swap(col, piv);
            inv.swap(col, piv);
            let d = a[col][col];
            for j in 0..4 {
                a[col][j] /= d;
                inv[col][j] /= d;
            }
            for i in 0..4 {
                if i != col {
                    let f = a[i][col];
                    if f != 0.0 {
                        for j in 0..4 {
                            a[i][j] -= f * a[col][j];
                            inv[i][j] -= f * inv[col][j];
                        }
                    }
                }
            }
        }
        Some(Mat4(inv))
    }
    /// The projective 3×3 mapping a z=0 plane (x, y) through this matrix and a perspective divide
    /// by `w`: used to draw planar 3D layers. `self` must already include the projection.
    pub fn plane_to_mat3(&self) -> Mat3 {
        let m = &self.0;
        Mat3([[m[0][0], m[0][1], m[0][3]], [m[1][0], m[1][1], m[1][3]], [m[3][0], m[3][1], m[3][3]]])
    }
}

impl Mul for Mat4 {
    type Output = Mat4;
    fn mul(self, o: Mat4) -> Mat4 {
        let mut r = [[0.0; 4]; 4];
        for i in 0..4 {
            for j in 0..4 {
                r[i][j] = (0..4).map(|k| self.0[i][k] * o.0[k][j]).sum();
            }
        }
        Mat4(r)
    }
}

/// After Effects-style camera: the comp is viewed from `position` towards `poi` with `zoom` = the
/// distance (in pixels) from the eye to the image plane. Returns view×projection that maps world
/// points to comp pixels (x, y) with depth in z and the perspective divide in w.
pub fn camera_matrix(comp_w: f64, comp_h: f64, position: Vec3, view: Mat4, zoom: f64) -> Mat4 {
    let _ = position;
    // Projection: x' = zoom * x / z + cx, y' = zoom * y / z + cy (z in camera space, +z forward).
    let proj = Mat4([[zoom, 0.0, comp_w * 0.5, 0.0], [0.0, zoom, comp_h * 0.5, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 1.0, 0.0]]);
    proj * view
}

/// The default camera AE uses when a comp has 3D layers but no camera: 50 mm equivalent, placed
/// at (w/2, h/2, -zoom) looking at the comp centre.
pub fn default_camera_zoom(comp_w: f64) -> f64 {
    // 50mm preset on a 36mm film width: zoom = w * 50 / 36.
    comp_w * 50.0 / 36.0
}

/// View matrix for a camera at `pos` looking at `poi` (AE: one-node cameras look along +z of
/// their own orientation; two-node cameras auto-orient to the point of interest).
pub fn look_at(pos: Vec3, poi: Vec3, extra: Mat4) -> Mat4 {
    let f = (poi - pos).normalize();
    let f = if f.length() == 0.0 { vec3(0.0, 0.0, 1.0) } else { f };
    // y down world; choose up = -y unless parallel.
    let up = if f.cross(vec3(0.0, -1.0, 0.0)).length() < 1e-9 { vec3(0.0, 0.0, 1.0) } else { vec3(0.0, -1.0, 0.0) };
    let r = up.cross(f).normalize();
    let u = f.cross(r);
    // Camera basis: x = right (screen +x), y = down (screen +y), z = forward.
    let x = -r;
    let y = -u;
    let rot = Mat4([[x.x, x.y, x.z, 0.0], [y.x, y.y, y.z, 0.0], [f.x, f.y, f.z, 0.0], [0.0, 0.0, 0.0, 1.0]]);
    extra * rot * Mat4::translate(-pos)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A quad with three collinear corners has no projective map: identity, not NaN.
    #[test]
    fn square_to_quad_degenerate_is_finite() {
        let m = Mat3::square_to_quad([vec2(0.0, 5.0), vec2(0.0, 0.0), vec2(10.0, 0.0), vec2(20.0, 0.0)]);
        assert!(m.0.iter().flatten().all(|v| v.is_finite()), "non-finite: {m:?}");
    }

    #[test]
    fn square_to_quad_regular_is_finite() {
        let m = Mat3::square_to_quad([vec2(0.0, 0.0), vec2(10.0, 0.0), vec2(8.0, 6.0), vec2(0.0, 5.0)]);
        assert!(m.0.iter().flatten().all(|v| v.is_finite()));
    }

    fn close(a: Vec2, b: Vec2) -> bool {
        (a - b).length() < 1e-9
    }

    #[test]
    fn mat4_inverse_rejects_non_finite() {
        for (r, c, v) in [(0, 0, f64::NAN), (2, 1, f64::NAN), (1, 3, f64::INFINITY)] {
            let mut m = Mat4::IDENTITY;
            m.0[r][c] = v;
            assert!(m.inverse().is_none(), "{r},{c},{v}");
        }
        assert!(Mat4::IDENTITY.inverse().is_some());
    }

    #[test]
    fn layer_2d_maps_anchor_to_position() {
        let m = Mat3::layer_2d(vec2(50.0, 25.0), vec2(960.0, 540.0), vec2(200.0, 50.0), 30.0);
        assert!(close(m.apply(vec2(50.0, 25.0)), vec2(960.0, 540.0)));
        let inv = m.inverse().unwrap();
        let p = vec2(12.0, -7.0);
        assert!(close(inv.apply(m.apply(p)), p));
    }

    #[test]
    fn rotation_is_clockwise_on_screen() {
        let p = Mat3::rotate_deg(90.0).apply(vec2(1.0, 0.0));
        assert!(close(p, vec2(0.0, 1.0)));
    }

    #[test]
    fn quad_mapping_hits_corners() {
        let q = [vec2(10.0, 10.0), vec2(110.0, 20.0), vec2(120.0, 140.0), vec2(5.0, 100.0)];
        let m = Mat3::square_to_quad(q);
        assert!(close(m.apply(vec2(0.0, 0.0)), q[0]));
        assert!(close(m.apply(vec2(1.0, 0.0)), q[1]));
        assert!(close(m.apply(vec2(1.0, 1.0)), q[2]));
        assert!(close(m.apply(vec2(0.0, 1.0)), q[3]));
    }

    #[test]
    fn mat4_inverse_roundtrip() {
        let m = Mat4::layer_3d(vec3(1.0, 2.0, 3.0), vec3(100.0, 50.0, -20.0), vec3(150.0, 80.0, 100.0), vec3(10.0, 20.0, 30.0), vec3(5.0, -40.0, 12.0));
        let i = m.inverse().unwrap();
        let p = vec3(3.0, -4.0, 5.0);
        assert!((i.apply(m.apply(p)) - p).length() < 1e-9);
    }

    #[test]
    fn default_camera_sees_z0_plane_unscaled() {
        let (w, h) = (1920.0, 1080.0);
        let zoom = default_camera_zoom(w);
        let pos = vec3(w / 2.0, h / 2.0, -zoom);
        let view = look_at(pos, vec3(w / 2.0, h / 2.0, 0.0), Mat4::IDENTITY);
        let cam = camera_matrix(w, h, pos, view, zoom);
        let p = cam.apply(vec3(100.0, 200.0, 0.0));
        assert!((p.x - 100.0).abs() < 1e-6 && (p.y - 200.0).abs() < 1e-6, "{p:?}");
    }

    proptest::proptest! {
        #[test]
        fn mat3_inverse(ax in -100.0f64..100.0, ay in -100.0f64..100.0, s in 10.0f64..400.0, r in -720.0f64..720.0) {
            let m = Mat3::layer_2d(vec2(ax, ay), vec2(3.0, 4.0), vec2(s, s * 0.7), r);
            let i = m.inverse().unwrap();
            let p = vec2(17.0, 23.0);
            proptest::prop_assert!((i.apply(m.apply(p)) - p).length() < 1e-6);
        }
    }
}
