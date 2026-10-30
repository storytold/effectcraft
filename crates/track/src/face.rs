//! Face tracking (Tracker panel ▸ Method ▸ Face Tracking (Outline Only) / (Detailed Features)),
//! a classical pipeline with no learned weights:
//!
//! 1. **Skin model**: on the first frame, the colour of the middle of the mask (luma and
//!    normalised r, g chromaticity: median and median absolute deviation) defines skin; every
//!    later frame is classified against it inside a search window around the previous face.
//! 2. **Face region and outline**: the skin component containing the face centre is flood-filled;
//!    its outer boundary is found along 48 rays from its centroid (so the non-skin holes of eyes,
//!    brows, nostrils and mouth don't matter), median-smoothed, and an ellipse is fitted to the
//!    outline polygon from its area moments (centre, half-height, roll). The outline is what
//!    *Outline Only* keys into the mask.
//! 3. **Facial features**: the non-skin connected components inside the face are matched to the
//!    eyes, brows, nostrils and mouth by their position in the face frame (predicted from the
//!    previous frame); landmark points are measured on each component (extremes along the face
//!    axes for corners and lids, a darkness-weighted centroid for the pupil). Chin and jaw points
//!    come from the outline.
//! 4. **Shape model**: an active-shape point distribution model (Cootes, Taylor, Cooper & Graham,
//!    "Active Shape Models — Their Training and Application", CVIU 61(1), 1995) regularises the
//!    landmarks: a weighted similarity alignment and shape-parameter fit (parameters clamped to
//!    ±3 standard deviations) fills landmarks whose feature was not found (a closed eye, a
//!    merged component) and replaces detections that disagree with the model. The model is
//!    *trained on synthetic shapes* produced by our own parametric face generator
//!    ([`synth_shape`]: eye and mouth openness, mouth width, brow raise, head yaw and pitch) by
//!    principal component analysis — no external data or weights.
//!
//! With a trained model (Settings ▸ Face Tracking: an `effectcraft_segment::face::FaceModel` such
//! as MediaPipe Face Landmarker), the model finds the face in the mask and follows it; its
//! outline and named points replace steps 1–4, and chin and jaw come from its outline the same
//! way, so measurements mean the same with either engine ([`FaceTracker::new_with`]). When the
//! model finds no face in the mask, the classical pipeline runs.
//!
//! Coordinates are layer pixels; the face frame is centred on the face ellipse, with the
//! half-height as unit, `u` right and `v` down (image-left is "Left").

use std::sync::{Arc, OnceLock};

use effectcraft_raster::Image;
use effectcraft_segment::face::{Face, FaceModel, Topology};

use crate::Frame;
use crate::camtrack::linalg::{cholesky_solve, sym_eigen};

/// Landmarks: (id, display name, canonical position in the face frame).
pub const LANDMARKS: [(&str, &str, [f64; 2]); 26] = [
    ("leftEyebrowInner", "Left Eyebrow Inner", [-0.16, -0.44]),
    ("leftEyebrowMiddle", "Left Eyebrow Middle", [-0.32, -0.50]),
    ("leftEyebrowOuter", "Left Eyebrow Outer", [-0.48, -0.44]),
    ("rightEyebrowInner", "Right Eyebrow Inner", [0.16, -0.44]),
    ("rightEyebrowMiddle", "Right Eyebrow Middle", [0.32, -0.50]),
    ("rightEyebrowOuter", "Right Eyebrow Outer", [0.48, -0.44]),
    ("leftEyeInner", "Left Eye Inner", [-0.17, -0.22]),
    ("leftEyeOuter", "Left Eye Outer", [-0.45, -0.22]),
    ("leftEyeTop", "Left Eye Top", [-0.31, -0.292]),
    ("leftEyeBottom", "Left Eye Bottom", [-0.31, -0.148]),
    ("leftPupil", "Left Pupil", [-0.31, -0.22]),
    ("rightEyeInner", "Right Eye Inner", [0.17, -0.22]),
    ("rightEyeOuter", "Right Eye Outer", [0.45, -0.22]),
    ("rightEyeTop", "Right Eye Top", [0.31, -0.292]),
    ("rightEyeBottom", "Right Eye Bottom", [0.31, -0.148]),
    ("rightPupil", "Right Pupil", [0.31, -0.22]),
    ("noseTip", "Nose Tip", [0.0, 0.14]),
    ("leftNostril", "Left Nostril", [-0.08, 0.20]),
    ("rightNostril", "Right Nostril", [0.08, 0.20]),
    ("mouthLeft", "Mouth Left", [-0.24, 0.50]),
    ("mouthRight", "Mouth Right", [0.24, 0.50]),
    ("mouthTop", "Mouth Top", [0.0, 0.435]),
    ("mouthBottom", "Mouth Bottom", [0.0, 0.565]),
    ("chin", "Chin", [0.0, 1.0]),
    ("leftJaw", "Left Jaw", [-0.614, 0.574]),
    ("rightJaw", "Right Jaw", [0.614, 0.574]),
];

/// Number of landmarks.
pub const N: usize = LANDMARKS.len();

/// Landmark index by id.
pub fn landmark(id: &str) -> Option<usize> {
    LANDMARKS.iter().position(|l| l.0 == id)
}

const BROW: [[usize; 3]; 2] = [[0, 1, 2], [3, 4, 5]];
/// Eye landmarks: inner, outer, top, bottom, pupil.
const EYE: [[usize; 5]; 2] = [[6, 7, 8, 9, 10], [11, 12, 13, 14, 15]];
const NOSE_TIP: usize = 16;
const NOSTRIL: [usize; 2] = [17, 18];
/// Mouth: left, right, top, bottom.
const MOUTH: [usize; 4] = [19, 20, 21, 22];
const CHIN: usize = 23;
const JAW: [usize; 2] = [24, 25];
/// Half-width of the face relative to its half-height.
const ASPECT: f64 = 0.75;

// ---------------------------------------------------------------- synthetic faces

/// Parameters of a synthetic face (the shape model's training data and the tests' footage).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceParams {
    /// Face centre (layer pixels) and half-height (pixels).
    pub center: [f64; 2],
    pub height: f64,
    /// Head rotation in degrees: roll (in the image plane), yaw (turn), pitch (nod).
    pub roll: f64,
    pub yaw: f64,
    pub pitch: f64,
    /// Eye openness (0 closed … 1 open), left and right.
    pub eye_open: [f64; 2],
    /// Mouth openness (0 … 1) and width (1 = neutral).
    pub mouth_open: f64,
    pub mouth_width: f64,
    /// Eyebrow raise (−1 … 1), left and right.
    pub brow_raise: [f64; 2],
}

impl Default for FaceParams {
    fn default() -> Self {
        FaceParams {
            center: [0.0; 2],
            height: 60.0,
            roll: 0.0,
            yaw: 0.0,
            pitch: 0.0,
            eye_open: [1.0; 2],
            mouth_open: 0.2,
            mouth_width: 1.0,
            brow_raise: [0.0; 2],
        }
    }
}

fn eye_half_height(open: f64) -> f64 {
    0.012 + 0.06 * open.clamp(0.0, 1.0)
}
fn mouth_half_height(open: f64) -> f64 {
    0.015 + 0.11 * open.clamp(0.0, 1.0)
}

/// The landmarks of a synthetic face in the face frame (before roll, translation and scale).
pub fn synth_shape(p: &FaceParams) -> [[f64; 2]; N] {
    let mut s = [[0.0; 2]; N];
    let (sy, sp) = (p.yaw.to_radians().sin(), p.pitch.to_radians().sin());
    // Inner features slide with the head's turn; the nose (closer to the camera) slides more.
    let shift = |depth: f64| [0.35 * sy * depth, 0.3 * sp * depth];
    for (i, l) in LANDMARKS.iter().enumerate() {
        s[i] = l.2;
    }
    for side in 0..2 {
        let sg = if side == 0 { -1.0 } else { 1.0 };
        let raise = -0.06 * p.brow_raise[side].clamp(-1.0, 1.0);
        for &i in &BROW[side] {
            s[i][1] += raise;
        }
        let h = eye_half_height(p.eye_open[side]);
        let e = EYE[side];
        s[e[2]] = [sg * 0.31, -0.22 - h];
        s[e[3]] = [sg * 0.31, -0.22 + h];
    }
    let mw = 0.24 * p.mouth_width.clamp(0.5, 1.6);
    let mh = mouth_half_height(p.mouth_open);
    s[MOUTH[0]] = [-mw, 0.5];
    s[MOUTH[1]] = [mw, 0.5];
    s[MOUTH[2]] = [0.0, 0.5 - mh];
    s[MOUTH[3]] = [0.0, 0.5 + mh];
    for (i, q) in s.iter_mut().enumerate() {
        if i == CHIN || JAW.contains(&i) {
            continue;
        }
        let d = if i == NOSE_TIP {
            1.3
        } else if NOSTRIL.contains(&i) {
            1.15
        } else {
            1.0
        };
        let o = shift(d);
        q[0] += o[0];
        q[1] += o[1];
    }
    s
}

/// Face frame → layer pixels.
fn to_layer(c: [f64; 2], b: f64, roll_deg: f64, q: [f64; 2]) -> [f64; 2] {
    let (sn, cs) = roll_deg.to_radians().sin_cos();
    [c[0] + b * (cs * q[0] - sn * q[1]), c[1] + b * (sn * q[0] + cs * q[1])]
}

/// Layer pixels → face frame.
fn to_face(c: [f64; 2], b: f64, roll_deg: f64, p: [f64; 2]) -> [f64; 2] {
    face_mapper(c, b, roll_deg)(p)
}

/// [`to_face`] with the rotation worked out once, for mapping many points.
fn face_mapper(c: [f64; 2], b: f64, roll_deg: f64) -> impl Fn([f64; 2]) -> [f64; 2] {
    let (sn, cs) = roll_deg.to_radians().sin_cos();
    move |p| {
        let d = [(p[0] - c[0]) / b, (p[1] - c[1]) / b];
        [cs * d[0] + sn * d[1], -sn * d[0] + cs * d[1]]
    }
}

/// A synthetic face's landmarks in layer pixels.
pub fn synth_landmarks(p: &FaceParams) -> [[f64; 2]; N] {
    synth_shape(p).map(|q| to_layer(p.center, p.height, p.roll, q))
}

fn hash(i: i64, j: i64) -> f64 {
    let mut v = (i.wrapping_mul(73_856_093) ^ j.wrapping_mul(19_349_663)) as u64;
    v ^= v >> 13;
    v = v.wrapping_mul(0x5bd1_e995);
    v ^= v >> 15;
    (v % 10_000) as f64 / 10_000.0
}

fn seg_dist(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1]];
    let l2 = d[0] * d[0] + d[1] * d[1];
    let t = if l2 > 0.0 { (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (p[0] - a[0] - t * d[0]).hypot(p[1] - a[1] - t * d[1])
}

/// Colour of a synthetic face at face-frame point `q` (`None` outside the face).
fn face_color(p: &FaceParams, s: &[[f64; 2]; N], q: [f64; 2]) -> Option<[f32; 3]> {
    if (q[0] / ASPECT).powi(2) + q[1] * q[1] > 1.0 {
        return None;
    }
    let shade = (0.95 - 0.08 * q[1]) as f32;
    let mut col = [0.86 * shade, 0.64 * shade, 0.52 * shade];
    for side in 0..2 {
        let b = BROW[side];
        let d = seg_dist(q, s[b[0]], s[b[1]]).min(seg_dist(q, s[b[1]], s[b[2]]));
        if d < 0.035 {
            col = [0.28, 0.18, 0.12];
        }
        let e = EYE[side];
        let ec = [(s[e[0]][0] + s[e[1]][0]) / 2.0, (s[e[2]][1] + s[e[3]][1]) / 2.0];
        let (hw, hh) = ((s[e[1]][0] - s[e[0]][0]).abs() / 2.0, (s[e[3]][1] - s[e[2]][1]).abs() / 2.0);
        if ((q[0] - ec[0]) / hw).powi(2) + ((q[1] - ec[1]) / hh).powi(2) <= 1.0 {
            let pupil = s[e[4]];
            col = if (q[0] - pupil[0]).hypot(q[1] - pupil[1]) < 0.055 { [0.12, 0.08, 0.06] } else { [0.95, 0.95, 0.93] };
        }
    }
    let _ = p;
    for &n in &NOSTRIL {
        if ((q[0] - s[n][0]) / 0.035).powi(2) + ((q[1] - s[n][1]) / 0.022).powi(2) <= 1.0 {
            col = [0.3, 0.13, 0.1];
        }
    }
    let m = MOUTH;
    let mc = [(s[m[0]][0] + s[m[1]][0]) / 2.0, (s[m[2]][1] + s[m[3]][1]) / 2.0];
    let (mw, mh) = ((s[m[1]][0] - s[m[0]][0]) / 2.0, (s[m[3]][1] - s[m[2]][1]) / 2.0);
    if ((q[0] - mc[0]) / mw).powi(2) + ((q[1] - mc[1]) / mh).powi(2) <= 1.0 {
        col = [0.42, 0.08, 0.1];
    }
    Some(col)
}

/// Render a synthetic face over a textured blue-green background (2 × 2 supersampled): the
/// test footage of the face tracker.
pub fn render_synthetic(w: u32, h: u32, p: &FaceParams, background_seed: i64) -> Image {
    use rayon::prelude::*;
    let s = synth_shape(p);
    let mut img = Image::new(w, h);
    img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let mut acc = [0.0f32; 3];
            for sy in 0..2 {
                for sx in 0..2 {
                    let l = [x as f64 + 0.25 + 0.5 * sx as f64, y as f64 + 0.25 + 0.5 * sy as f64];
                    let q = to_face(p.center, p.height, p.roll, l);
                    let c = face_color(p, &s, q).unwrap_or_else(|| {
                        let n = hash((l[0] / 9.0) as i64 + background_seed, (l[1] / 9.0) as i64) as f32;
                        [0.15 + 0.1 * n, 0.35 + 0.2 * n, 0.45 + 0.25 * n]
                    });
                    for k in 0..3 {
                        acc[k] += c[k] * 0.25;
                    }
                }
            }
            *px = [acc[0], acc[1], acc[2], 1.0];
        }
    });
    img
}

// ---------------------------------------------------------------- shape model

/// Point distribution model: mean shape and principal modes (face frame).
pub struct Pdm {
    pub mean: Vec<f64>,
    /// Modes as rows of length `2N`.
    pub modes: Vec<Vec<f64>>,
    /// Variance of each mode.
    pub var: Vec<f64>,
}

/// The shape model, trained once on synthetic shapes.
pub fn pdm() -> &'static Pdm {
    static P: OnceLock<Pdm> = OnceLock::new();
    P.get_or_init(|| {
        let samples = 600;
        let dim = 2 * N;
        let mut rng = 0x2545_f491_4f6c_dd1du64;
        let mut u = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            (rng % 1_000_000) as f64 / 1_000_000.0
        };
        let mut data = Vec::with_capacity(samples);
        for _ in 0..samples {
            let p = FaceParams {
                yaw: (u() - 0.5) * 60.0,
                pitch: (u() - 0.5) * 40.0,
                eye_open: [u(), u()],
                mouth_open: u(),
                mouth_width: 0.8 + 0.45 * u(),
                brow_raise: [u() * 2.0 - 1.0, u() * 2.0 - 1.0],
                ..Default::default()
            };
            data.push(synth_shape(&p).iter().flat_map(|q| [q[0], q[1]]).collect::<Vec<f64>>());
        }
        let mut mean = vec![0.0; dim];
        for d in &data {
            for (m, v) in mean.iter_mut().zip(d) {
                *m += v / samples as f64;
            }
        }
        let mut cov = vec![0.0; dim * dim];
        for d in &data {
            for a in 0..dim {
                let da = d[a] - mean[a];
                for b in 0..dim {
                    cov[a * dim + b] += da * (d[b] - mean[b]) / samples as f64;
                }
            }
        }
        let (vals, vecs) = sym_eigen(&cov, dim);
        let total: f64 = vals.iter().map(|v| v.max(0.0)).sum();
        let mut modes = vec![];
        let mut var = vec![];
        let mut acc = 0.0;
        for k in (0..dim).rev() {
            if acc >= 0.995 * total || vals[k] <= 1e-12 {
                break;
            }
            acc += vals[k];
            modes.push((0..dim).map(|r| vecs[r * dim + k]).collect());
            var.push(vals[k]);
        }
        Pdm { mean, modes, var }
    })
}

/// A similarity `p ↦ s R q + t` as `[a, b, tx, ty]` (`x = a u − b v + tx`, `y = b u + a v + ty`).
fn fit_similarity(src: &[[f64; 2]], dst: &[[f64; 2]], w: &[f64]) -> Option<[f64; 4]> {
    let ws: f64 = w.iter().sum();
    if ws < 2.0 - 1e-9 {
        return None;
    }
    let mut cs = [0.0; 2];
    let mut cd = [0.0; 2];
    for i in 0..src.len() {
        for k in 0..2 {
            cs[k] += w[i] * src[i][k] / ws;
            cd[k] += w[i] * dst[i][k] / ws;
        }
    }
    let (mut ss, mut dt, mut cr) = (0.0, 0.0, 0.0);
    for i in 0..src.len() {
        let a = [src[i][0] - cs[0], src[i][1] - cs[1]];
        let b = [dst[i][0] - cd[0], dst[i][1] - cd[1]];
        ss += w[i] * (a[0] * a[0] + a[1] * a[1]);
        dt += w[i] * (a[0] * b[0] + a[1] * b[1]);
        cr += w[i] * (a[0] * b[1] - a[1] * b[0]);
    }
    if ss < 1e-12 {
        return None;
    }
    let (a, b) = (dt / ss, cr / ss);
    Some([a, b, cd[0] - (a * cs[0] - b * cs[1]), cd[1] - (b * cs[0] + a * cs[1])])
}

fn sim_apply(t: &[f64; 4], q: [f64; 2]) -> [f64; 2] {
    [t[0] * q[0] - t[1] * q[1] + t[2], t[1] * q[0] + t[0] * q[1] + t[3]]
}

fn sim_inverse(t: &[f64; 4], p: [f64; 2]) -> [f64; 2] {
    let d = [p[0] - t[2], p[1] - t[3]];
    let s2 = t[0] * t[0] + t[1] * t[1];
    [(t[0] * d[0] + t[1] * d[1]) / s2, (-t[1] * d[0] + t[0] * d[1]) / s2]
}

/// Fit the shape model to detected landmarks (`w` = 0 for missing ones), starting from the
/// face-frame similarity `init`. Returns (model landmarks in layer pixels, similarity).
pub fn fit_shape(detected: &[[f64; 2]; N], w: &[f64; N], init: [f64; 4]) -> ([[f64; 2]; N], [f64; 4]) {
    let m = pdm();
    let k = m.modes.len();
    let mut b = vec![0.0; k];
    let mut t = init;
    let shape = |b: &[f64]| -> [[f64; 2]; N] {
        let mut s = [[0.0; 2]; N];
        for i in 0..N {
            for d in 0..2 {
                let j = 2 * i + d;
                s[i][d] = m.mean[j] + m.modes.iter().zip(b).map(|(row, bk)| row[j] * bk).sum::<f64>();
            }
        }
        s
    };
    for _ in 0..10 {
        let x = shape(&b);
        if let Some(nt) = fit_similarity(&x, detected, w) {
            t = nt;
        }
        // Detections in the model frame.
        let y: Vec<[f64; 2]> = detected.iter().map(|p| sim_inverse(&t, *p)).collect();
        let mut ata = vec![0.0; k * k];
        let mut atb = vec![0.0; k];
        for i in 0..N {
            if w[i] <= 0.0 {
                continue;
            }
            for d in 0..2 {
                let j = 2 * i + d;
                let r = y[i][d] - m.mean[j];
                for a in 0..k {
                    atb[a] += w[i] * m.modes[a][j] * r;
                    for c in 0..k {
                        ata[a * k + c] += w[i] * m.modes[a][j] * m.modes[c][j];
                    }
                }
            }
        }
        for a in 0..k {
            // A weak prior towards the mean (Tikhonov, scaled by the mode's variance).
            ata[a * k + a] += 1e-3 / m.var[a].max(1e-9) * 1e-3 + 1e-9;
        }
        if cholesky_solve(&mut ata, k, &mut atb) {
            for a in 0..k {
                let lim = 3.0 * m.var[a].sqrt();
                b[a] = atb[a].clamp(-lim, lim);
            }
        }
    }
    let x = shape(&b);
    (x.map(|q| sim_apply(&t, q)), t)
}

// ---------------------------------------------------------------- tracking

/// The skin colour model (luma, r and g chromaticity: median and spread).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Skin {
    med: [f32; 3],
    tol: [f32; 3],
}

fn features(px: [f32; 4]) -> [f32; 3] {
    let s = (px[0] + px[1] + px[2]).max(1e-4);
    [0.2126 * px[0] + 0.7152 * px[1] + 0.0722 * px[2], px[0] / s, px[1] / s]
}

impl Skin {
    fn is_skin(&self, px: [f32; 4]) -> bool {
        let f = features(px);
        (0..3).all(|k| (f[k] - self.med[k]).abs() <= self.tol[k])
    }
}

/// One frame's face fit.
#[derive(Clone, Debug, PartialEq)]
pub struct FaceFit {
    /// Landmarks ([`LANDMARKS`] order), layer pixels.
    pub landmarks: [[f64; 2]; N],
    /// Which landmarks were measured on this frame (the others come from the shape model).
    pub detected: [bool; N],
    /// The face outline: a closed polygon (layer pixels).
    pub outline: Vec<[f64; 2]>,
    /// Face ellipse centre, half-height (pixels) and roll (degrees).
    pub center: [f64; 2],
    pub height: f64,
    pub roll: f64,
}

/// Follows one face through a clip.
#[derive(Clone, Debug)]
pub struct FaceTracker {
    skin: Skin,
    prev: FaceFit,
    /// Previous landmarks in the face frame (the expected feature positions).
    expect: [[f64; 2]; N],
    /// The trained model following the face, if one does.
    learned: Option<Learned>,
}

/// A trained model and where it last saw the face.
#[derive(Clone)]
struct Learned {
    model: Arc<dyn FaceModel>,
    face: Face,
}

impl std::fmt::Debug for Learned {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Learned").field("model", &self.model.info().id).finish_non_exhaustive()
    }
}

/// A frame as trained models see it: straight RGB, size.
fn model_input(frame: &Frame) -> (Vec<[f32; 3]>, usize, usize) {
    (crate::roto::rgb_of(frame.img), frame.img.width as usize, frame.img.height as usize)
}

/// Where the outline meets the direction of canonical face-frame point `q` from `center`.
fn outline_point(outline: &[[f64; 2]], center: [f64; 2], roll: f64, q: [f64; 2]) -> [f64; 2] {
    let dir = to_layer([0.0; 2], 1.0, roll, q);
    let rad = ray_radius(outline, center, dir[1].atan2(dir[0]));
    let len = dir[0].hypot(dir[1]).max(1e-12);
    [center[0] + dir[0] / len * rad, center[1] + dir[1] / len * rad]
}

/// A trained model's face as a fit: its outline and named points (all measured); chin and jaw
/// where the outline meets their canonical directions, as in the classical fit.
fn fit_from_face(face: &Face, topo: &Topology, offset: [f64; 2]) -> Option<FaceFit> {
    let pt = |i: usize| face.points.get(i).map(|p| [p[0] as f64 - offset[0], p[1] as f64 - offset[1]]);
    let outline: Vec<[f64; 2]> = topo.outline.iter().map(|&i| pt(i)).collect::<Option<_>>()?;
    let (center, b, _, ellipse_roll) = polygon_ellipse(&outline)?;
    if !b.is_finite() || b <= 1.0 {
        return None;
    }
    let mut detected = [false; N];
    let mut named = [None; N];
    for (id, i) in topo.landmarks {
        if let (Some(k), Some(p)) = (landmark(id), pt(*i)) {
            named[k] = Some(p);
        }
    }
    // Roll: the line between the outer eye corners; else the outline's axis.
    let roll = match (named[EYE[0][1]], named[EYE[1][1]]) {
        (Some(l), Some(r)) => (r[1] - l[1]).atan2(r[0] - l[0]).to_degrees(),
        _ => ellipse_roll,
    };
    let mut landmarks = LANDMARKS.map(|l| to_layer(center, b, roll, l.2));
    for k in 0..N {
        if let Some(p) = named[k] {
            landmarks[k] = p;
            detected[k] = true;
        }
    }
    for k in [CHIN, JAW[0], JAW[1]] {
        landmarks[k] = outline_point(&outline, center, roll, LANDMARKS[k].2);
        detected[k] = true;
    }
    Some(FaceFit { landmarks, detected, outline, center, height: b, roll })
}

/// Find the first skin pixel in successive rings around the predicted center.
fn nearest_skin(skin: &[bool], size: (usize, usize), center: (usize, usize), radius: i64) -> Option<(usize, usize)> {
    let (w, h) = size;
    let (ci, cj) = center;
    for r in 0..radius {
        for dj in -r..=r {
            for di in -r..=r {
                if di.abs() != r && dj.abs() != r {
                    continue;
                }
                let (i, j) = (ci as i64 + di, cj as i64 + dj);
                if i >= 0 && j >= 0 && (i as usize) < w && (j as usize) < h && skin[j as usize * w + i as usize] {
                    return Some((i as usize, j as usize));
                }
            }
        }
    }
    None
}

struct FaceGeometry {
    center: [f64; 2],
    height: f64,
    roll: f64,
}

/// A pixel window of a frame in layer coordinates.
struct Window {
    x0: i64,
    y0: i64,
    w: usize,
    h: usize,
}

impl Window {
    fn around(frame: &Frame, c: [f64; 2], rx: f64, ry: f64) -> Option<Window> {
        let (ox, oy) = (frame.offset[0], frame.offset[1]);
        let x0 = ((c[0] - rx + ox).floor() as i64).max(0);
        let y0 = ((c[1] - ry + oy).floor() as i64).max(0);
        let x1 = ((c[0] + rx + ox).ceil() as i64).min(frame.img.width as i64);
        let y1 = ((c[1] + ry + oy).ceil() as i64).min(frame.img.height as i64);
        (x1 > x0 + 4 && y1 > y0 + 4).then(|| Window { x0, y0, w: (x1 - x0) as usize, h: (y1 - y0) as usize })
    }
    fn px(&self, frame: &Frame, i: usize, j: usize) -> [f32; 4] {
        frame.img.get(self.x0 + i as i64, self.y0 + j as i64)
    }
    /// Layer position of the centre of window pixel `(i, j)`.
    fn layer(&self, frame: &Frame, i: usize, j: usize) -> [f64; 2] {
        [(self.x0 + i as i64) as f64 + 0.5 - frame.offset[0], (self.y0 + j as i64) as f64 + 0.5 - frame.offset[1]]
    }
    fn cell(&self, frame: &Frame, p: [f64; 2]) -> Option<(usize, usize)> {
        let x = (p[0] + frame.offset[0]).floor() as i64 - self.x0;
        let y = (p[1] + frame.offset[1]).floor() as i64 - self.y0;
        (x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h).then_some((x as usize, y as usize))
    }
}

/// Per pixel of a `w × h` window: not skin and inside `poly` (even-odd rule, same comparisons as
/// [`crate::mask::inside_polygon`] at `layer(i, j)`). A scanline crosses the polygon at the same
/// edges for every pixel of a row, so the crossings are found once per row and each pixel's
/// parity follows from how many lie right of it.
fn non_skin_inside(poly: &[[f64; 2]], skin: &[bool], layer: impl Fn(usize, usize) -> [f64; 2], w: usize, h: usize) -> Vec<bool> {
    let mut feat = vec![false; w * h];
    let n = poly.len();
    if n < 3 {
        return feat;
    }
    let mut xs: Vec<f64> = Vec::with_capacity(n);
    for j in 0..h {
        let y = layer(0, j)[1];
        xs.clear();
        let mut prev = poly[n - 1];
        for &cur in poly {
            let (a, b) = (cur, prev);
            if (a[1] > y) != (b[1] > y) {
                xs.push((b[0] - a[0]) * (y - a[1]) / (b[1] - a[1]) + a[0]);
            }
            prev = cur;
        }
        if xs.is_empty() {
            continue;
        }
        xs.sort_by(f64::total_cmp);
        let mut left = 0;
        for i in 0..w {
            let x = layer(i, j)[0];
            while left < xs.len() && xs[left] <= x {
                left += 1;
            }
            feat[j * w + i] = (xs.len() - left) % 2 == 1 && !skin[j * w + i];
        }
    }
    feat
}

fn median(v: &mut [f32]) -> f32 {
    v.sort_by(f32::total_cmp);
    v[v.len() / 2]
}

/// Area-moment ellipse of a closed polygon: (centroid, half-height along the major axis,
/// half-width, major axis angle from vertical in degrees).
fn polygon_ellipse(poly: &[[f64; 2]]) -> Option<([f64; 2], f64, f64, f64)> {
    let n = poly.len();
    let (mut a, mut cx, mut cy) = (0.0, 0.0, 0.0);
    let (mut ixx, mut iyy, mut ixy) = (0.0, 0.0, 0.0);
    for i in 0..n {
        let (p, q) = (poly[i], poly[(i + 1) % n]);
        let cr = p[0] * q[1] - q[0] * p[1];
        a += cr;
        cx += (p[0] + q[0]) * cr;
        cy += (p[1] + q[1]) * cr;
        ixx += (p[1] * p[1] + p[1] * q[1] + q[1] * q[1]) * cr;
        iyy += (p[0] * p[0] + p[0] * q[0] + q[0] * q[0]) * cr;
        ixy += (p[0] * q[1] + 2.0 * p[0] * p[1] + 2.0 * q[0] * q[1] + q[0] * p[1]) * cr;
    }
    a *= 0.5;
    if a.abs() < 1e-9 {
        return None;
    }
    cx /= 6.0 * a;
    cy /= 6.0 * a;
    // Central second moments (per unit area): var x, var y, cov xy.
    let vxx = iyy / (12.0 * a) - cx * cx;
    let vyy = ixx / (12.0 * a) - cy * cy;
    let vxy = ixy / (24.0 * a) - cx * cy;
    let tr = vxx + vyy;
    let det = vxx * vyy - vxy * vxy;
    let disc = (tr * tr / 4.0 - det).max(0.0).sqrt();
    let (l1, l2) = (tr / 2.0 + disc, (tr / 2.0 - disc).max(0.0));
    // Major axis direction.
    let ang = 0.5 * (2.0 * vxy).atan2(vxx - vyy); // from +x
    let mut from_vertical = ang.to_degrees() - 90.0;
    while from_vertical > 90.0 {
        from_vertical -= 180.0;
    }
    while from_vertical < -90.0 {
        from_vertical += 180.0;
    }
    Some(([cx, cy], 2.0 * l1.sqrt(), 2.0 * l2.sqrt(), from_vertical))
}

/// A non-skin component inside the face.
struct Blob {
    /// Face-frame positions of its pixels and their darkness (skin luma − luma).
    pts: Vec<([f64; 2], f64)>,
    centroid: [f64; 2],
}

impl Blob {
    fn extreme(&self, dir: [f64; 2]) -> [f64; 2] {
        self.pts.iter().map(|p| p.0).max_by(|a, b| (a[0] * dir[0] + a[1] * dir[1]).total_cmp(&(b[0] * dir[0] + b[1] * dir[1]))).unwrap_or(self.centroid)
    }
    /// Extreme along `v` (`sign` −1 = top) among the pixels within `band` of the column `u`.
    fn lid(&self, u: f64, band: f64, sign: f64) -> [f64; 2] {
        let c = self.pts.iter().filter(|p| (p.0[0] - u).abs() <= band).map(|p| p.0);
        let best = c.max_by(|a, b| (sign * a[1]).total_cmp(&(sign * b[1])));
        best.map(|p| [u, p[1]]).unwrap_or(self.centroid)
    }
    fn dark_centroid(&self) -> [f64; 2] {
        let (mut s, mut w) = ([0.0; 2], 0.0);
        for (q, d) in &self.pts {
            let k = d.max(0.0).powi(2);
            s[0] += k * q[0];
            s[1] += k * q[1];
            w += k;
        }
        if w > 1e-12 { [s[0] / w, s[1] / w] } else { self.centroid }
    }
}

impl FaceTracker {
    /// Start on `frame` with the face inside `region` (`[x0, y0, x1, y1]`, layer pixels: the
    /// mask's bounds). `None` when no face-like skin region is found.
    pub fn new(frame: &Frame, region: [f64; 4]) -> Option<(FaceTracker, FaceFit)> {
        let c = [(region[0] + region[2]) / 2.0, (region[1] + region[3]) / 2.0];
        let (hw, hh) = ((region[2] - region[0]) / 2.0, (region[3] - region[1]) / 2.0);
        if !(hw > 4.0 && hh > 4.0) {
            return None;
        }
        // Skin colour: the middle of the region (cheeks and nose).
        let win = Window::around(frame, c, hw * 0.35, hh * 0.3)?;
        let mut f: [Vec<f32>; 3] = [vec![], vec![], vec![]];
        for j in 0..win.h {
            for i in 0..win.w {
                let v = features(win.px(frame, i, j));
                for k in 0..3 {
                    f[k].push(v[k]);
                }
            }
        }
        let mut med = [0.0f32; 3];
        let mut tol = [0.0f32; 3];
        let floors = [0.12f32, 0.025, 0.025];
        for k in 0..3 {
            med[k] = median(&mut f[k]);
            let mut dev: Vec<f32> = f[k].iter().map(|v| (v - med[k]).abs()).collect();
            tol[k] = (4.0 * median(&mut dev)).max(floors[k]);
        }
        let skin = Skin { med, tol };
        let canon: [[f64; 2]; N] = LANDMARKS.map(|l| l.2);
        let guess = FaceFit {
            landmarks: canon.map(|q| to_layer(c, hh, 0.0, q)),
            detected: [false; N],
            outline: vec![],
            center: c,
            height: hh.max(hw / ASPECT),
            roll: 0.0,
        };
        let mut t = FaceTracker { skin, prev: guess, expect: canon, learned: None };
        let fit = t.fit(frame)?;
        // A face shows at least two of: left eye, right eye, mouth.
        let found = [EYE[0][4], EYE[1][4], MOUTH[0]].iter().filter(|i| fit.detected[**i]).count();
        if found < 2 {
            return None;
        }
        t.accept(&fit);
        Some((t, fit))
    }

    /// [`FaceTracker::new`] with a trained model, if any: the model finds the face in `region`
    /// and follows it. Without a model, or when it finds no face there, the classical tracker.
    pub fn new_with(frame: &Frame, region: [f64; 4], model: Option<Arc<dyn FaceModel>>) -> Option<(FaceTracker, FaceFit)> {
        if let Some(model) = model {
            let (rgb, w, h) = model_input(frame);
            let (ox, oy) = (frame.offset[0], frame.offset[1]);
            let r = [region[0] + ox, region[1] + oy, region[2] + ox, region[3] + oy].map(|v| v as f32);
            if let Ok(Some(face)) = model.find(&rgb, w, h, r)
                && let Some(fit) = fit_from_face(&face, model.topology(), frame.offset)
            {
                let mut t = FaceTracker { skin: Skin::default(), prev: fit.clone(), expect: LANDMARKS.map(|l| l.2), learned: Some(Learned { model, face }) };
                t.accept(&fit);
                return Some((t, fit));
            }
        }
        Self::new(frame, region)
    }

    /// The trained model following the face (`None`: the classical tracker).
    pub fn model(&self) -> Option<&'static effectcraft_segment::ModelInfo> {
        self.learned.as_ref().map(|l| l.model.info())
    }

    /// The fit of the last tracked frame.
    pub fn last(&self) -> &FaceFit {
        &self.prev
    }

    fn accept(&mut self, fit: &FaceFit) {
        self.expect = fit.landmarks.map(|p| to_face(fit.center, fit.height, fit.roll, p));
        self.prev = fit.clone();
    }

    /// Track into the next frame (either direction). `None` when the face is lost.
    pub fn step(&mut self, frame: &Frame) -> Option<FaceFit> {
        let fit = match &mut self.learned {
            Some(l) => {
                let (rgb, w, h) = model_input(frame);
                let face = l.model.follow(&rgb, w, h, &l.face).ok().flatten()?;
                let fit = fit_from_face(&face, l.model.topology(), frame.offset)?;
                l.face = face;
                fit
            }
            None => self.fit(frame)?,
        };
        self.accept(&fit);
        Some(fit)
    }

    /// Connected dark features inside the fitted face, in normalized face coordinates.
    fn feature_blobs(&self, frame: &Frame, win: &Window, skin: &[bool], luma: &[f32], outline: &[[f64; 2]], geometry: FaceGeometry) -> Vec<Blob> {
        let FaceGeometry { center, height: b, roll } = geometry;
        let (w, h) = (win.w, win.h);
        // Non-skin components inside the face outline (shrunk a little: anti-aliased edges).
        let inner: Vec<[f64; 2]> = outline.iter().map(|p| [center[0] + 0.9 * (p[0] - center[0]), center[1] + 0.9 * (p[1] - center[1])]).collect();
        let feat = non_skin_inside(&inner, skin, |i, j| win.layer(frame, i, j), w, h);
        let min_area = (0.0006 * b * b).max(2.0) as usize;
        let mut seen = vec![false; w * h];
        let mut blobs: Vec<Blob> = vec![];
        let skin_luma = self.skin.med[0] as f64;
        let to_face = face_mapper(center, b, roll);
        for j0 in 0..h {
            for i0 in 0..w {
                if !feat[j0 * w + i0] || seen[j0 * w + i0] {
                    continue;
                }
                let mut st = vec![(i0, j0)];
                seen[j0 * w + i0] = true;
                let mut pts = vec![];
                while let Some((i, j)) = st.pop() {
                    let q = to_face(win.layer(frame, i, j));
                    pts.push((q, skin_luma - luma[j * w + i] as f64));
                    for dj in -1i64..=1 {
                        for di in -1i64..=1 {
                            let (a, c) = (i as i64 + di, j as i64 + dj);
                            if a < 0 || c < 0 || a as usize >= w || c as usize >= h {
                                continue;
                            }
                            let (a, c) = (a as usize, c as usize);
                            if feat[c * w + a] && !seen[c * w + a] {
                                seen[c * w + a] = true;
                                st.push((a, c));
                            }
                        }
                    }
                }
                if pts.len() >= min_area {
                    let n = pts.len() as f64;
                    let centroid = [pts.iter().map(|p| p.0[0]).sum::<f64>() / n, pts.iter().map(|p| p.0[1]).sum::<f64>() / n];
                    blobs.push(Blob { pts, centroid });
                }
            }
        }
        blobs
    }

    fn fit(&self, frame: &Frame) -> Option<FaceFit> {
        let pv = &self.prev;
        let b0 = pv.height;
        let reach = 1.7 * b0;
        let win = Window::around(frame, pv.center, reach, reach)?;
        let (w, h) = (win.w, win.h);
        let mut skin = vec![false; w * h];
        let mut luma = vec![0.0f32; w * h];
        for j in 0..h {
            for i in 0..w {
                let px = win.px(frame, i, j);
                skin[j * w + i] = self.skin.is_skin(px);
                luma[j * w + i] = features(px)[0];
            }
        }
        // Seed: the skin pixel nearest the predicted centre.
        let (ci, cj) = win.cell(frame, pv.center)?;
        let (si, sj) = nearest_skin(&skin, (w, h), (ci, cj), (0.6 * b0).ceil() as i64)?;
        // Flood fill (4-connected).
        let mut comp = vec![false; w * h];
        let mut stack = vec![(si, sj)];
        comp[sj * w + si] = true;
        let mut count = 0usize;
        let (mut mx, mut my) = (0.0, 0.0);
        while let Some((i, j)) = stack.pop() {
            count += 1;
            let p = win.layer(frame, i, j);
            mx += p[0];
            my += p[1];
            let nb = [(i.wrapping_sub(1), j), (i + 1, j), (i, j.wrapping_sub(1)), (i, j + 1)];
            for (a, b) in nb {
                if a < w && b < h && !comp[b * w + a] && skin[b * w + a] {
                    comp[b * w + a] = true;
                    stack.push((a, b));
                }
            }
        }
        if (count as f64) < 0.15 * std::f64::consts::PI * ASPECT * b0 * b0 {
            return None;
        }
        let c0 = [mx / count as f64, my / count as f64];
        // Outline along rays: the farthest component pixel.
        let rays = 48;
        let mut radii = vec![0.0f64; rays];
        for (k, r) in radii.iter_mut().enumerate() {
            let a = k as f64 / rays as f64 * std::f64::consts::TAU;
            let d = [a.cos(), a.sin()];
            let mut t = 0.0;
            while t < reach * 1.4 {
                let p = [c0[0] + d[0] * t, c0[1] + d[1] * t];
                match win.cell(frame, p) {
                    Some((i, j)) => {
                        if comp[j * w + i] {
                            *r = t + 0.5;
                        }
                    }
                    None => break,
                }
                t += 0.5;
            }
        }
        // Median of 5, circularly.
        let sm: Vec<f64> = (0..rays)
            .map(|k| {
                let mut v: Vec<f64> = (0..5).map(|d| radii[(k + rays + d - 2) % rays]).collect();
                v.sort_by(f64::total_cmp);
                v[2]
            })
            .collect();
        let outline: Vec<[f64; 2]> = (0..rays)
            .map(|k| {
                let a = k as f64 / rays as f64 * std::f64::consts::TAU;
                [c0[0] + a.cos() * sm[k], c0[1] + a.sin() * sm[k]]
            })
            .collect();
        let (center, major, _minor, ang) = polygon_ellipse(&outline)?;
        if !(major > 0.3 * b0 && major < 2.5 * b0) {
            return None;
        }
        // Roll: the ellipse axis nearest the previous roll (it is ambiguous by 180°).
        let mut roll = ang;
        while roll - pv.roll > 90.0 {
            roll -= 180.0;
        }
        while roll - pv.roll < -90.0 {
            roll += 180.0;
        }
        let b = major;
        let blobs = self.feature_blobs(frame, &win, &skin, &luma, &outline, FaceGeometry { center, height: b, roll });
        // Match components to features near their expected positions.
        let ex = &self.expect;
        let mean_of = |ids: &[usize]| {
            let n = ids.len() as f64;
            [ids.iter().map(|i| ex[*i][0]).sum::<f64>() / n, ids.iter().map(|i| ex[*i][1]).sum::<f64>() / n]
        };
        let mut used = vec![false; blobs.len()];
        let take = |target: [f64; 2], radius: f64, prefer_large: bool, used: &mut Vec<bool>| -> Option<usize> {
            let mut best: Option<(usize, f64)> = None;
            for (k, bl) in blobs.iter().enumerate() {
                if used[k] {
                    continue;
                }
                let d = (bl.centroid[0] - target[0]).hypot(bl.centroid[1] - target[1]);
                if d > radius {
                    continue;
                }
                let score = if prefer_large { -(bl.pts.len() as f64) } else { d };
                if best.is_none_or(|b| score < b.1) {
                    best = Some((k, score));
                }
            }
            if let Some((k, _)) = best {
                used[k] = true;
            }
            best.map(|b| b.0)
        };
        let mut det = [[0.0; 2]; N];
        let mut wgt = [0.0; N];
        let set = |i: usize, q: [f64; 2], det: &mut [[f64; 2]; N], wgt: &mut [f64; N]| {
            det[i] = to_layer(center, b, roll, q);
            wgt[i] = 1.0;
        };
        let eye_ids = [EYE[0].to_vec(), EYE[1].to_vec()];
        let mouth = take(mean_of(&MOUTH), 0.25, true, &mut used);
        let eyes = [take(mean_of(&eye_ids[0]), 0.15, false, &mut used), take(mean_of(&eye_ids[1]), 0.15, false, &mut used)];
        let brows = [take(mean_of(&BROW[0]), 0.15, false, &mut used), take(mean_of(&BROW[1]), 0.15, false, &mut used)];
        let nostrils = [take(ex[NOSTRIL[0]], 0.07, false, &mut used), take(ex[NOSTRIL[1]], 0.07, false, &mut used)];
        for side in 0..2 {
            let sg = if side == 0 { -1.0 } else { 1.0 };
            if let Some(k) = eyes[side] {
                let bl = &blobs[k];
                let e = EYE[side];
                let inner = bl.extreme([-sg, 0.0]);
                let outer = bl.extreme([sg, 0.0]);
                let mid = 0.5 * (inner[0] + outer[0]);
                let band = 0.15 * (outer[0] - inner[0]).abs();
                let px = 0.5 / b;
                set(e[0], [inner[0] - sg * px, inner[1]], &mut det, &mut wgt);
                set(e[1], [outer[0] + sg * px, outer[1]], &mut det, &mut wgt);
                let top = bl.lid(mid, band, -1.0);
                let bot = bl.lid(mid, band, 1.0);
                set(e[2], [top[0], top[1] - px], &mut det, &mut wgt);
                set(e[3], [bot[0], bot[1] + px], &mut det, &mut wgt);
                set(e[4], bl.dark_centroid(), &mut det, &mut wgt);
            }
            if let Some(k) = brows[side] {
                let bl = &blobs[k];
                let br = BROW[side];
                let inner = bl.extreme([-sg, 0.0]);
                let outer = bl.extreme([sg, 0.0]);
                set(br[0], inner, &mut det, &mut wgt);
                set(br[2], outer, &mut det, &mut wgt);
                // The middle: the brow's centre line at its mid column.
                let mid = 0.5 * (inner[0] + outer[0]);
                let band = 0.1 * (outer[0] - inner[0]).abs().max(0.05);
                let col: Vec<f64> = bl.pts.iter().filter(|p| (p.0[0] - mid).abs() <= band).map(|p| p.0[1]).collect();
                if !col.is_empty() {
                    set(br[1], [mid, col.iter().sum::<f64>() / col.len() as f64], &mut det, &mut wgt);
                }
            }
            if let Some(k) = nostrils[side] {
                set(NOSTRIL[side], blobs[k].centroid, &mut det, &mut wgt);
            }
        }
        if let (Some(a), Some(c)) = (nostrils[0], nostrils[1]) {
            let (p, q) = (blobs[a].centroid, blobs[c].centroid);
            set(NOSE_TIP, [0.5 * (p[0] + q[0]), 0.5 * (p[1] + q[1]) - 0.06], &mut det, &mut wgt);
        }
        if let Some(k) = mouth {
            let bl = &blobs[k];
            let l = bl.extreme([-1.0, 0.0]);
            let r = bl.extreme([1.0, 0.0]);
            let px = 0.5 / b;
            set(MOUTH[0], [l[0] - px, l[1]], &mut det, &mut wgt);
            set(MOUTH[1], [r[0] + px, r[1]], &mut det, &mut wgt);
            let mid = 0.5 * (l[0] + r[0]);
            let band = 0.12 * (r[0] - l[0]).abs();
            let top = bl.lid(mid, band, -1.0);
            let bot = bl.lid(mid, band, 1.0);
            set(MOUTH[2], [top[0], top[1] - px], &mut det, &mut wgt);
            set(MOUTH[3], [bot[0], bot[1] + px], &mut det, &mut wgt);
        }
        // Chin and jaw from the outline, along their canonical directions.
        for i in [CHIN, JAW[0], JAW[1]] {
            det[i] = outline_point(&outline, center, roll, LANDMARKS[i].2);
            wgt[i] = 1.0;
        }
        // Shape model: fill the missing landmarks and replace inconsistent detections.
        let (sn, cs) = roll.to_radians().sin_cos();
        let init = [b * cs, b * sn, center[0], center[1]];
        let (model, _) = fit_shape(&det, &wgt, init);
        let mut w2 = wgt;
        for i in 0..N {
            if wgt[i] > 0.0 && (det[i][0] - model[i][0]).hypot(det[i][1] - model[i][1]) > 0.12 * b {
                w2[i] = 0.0;
            }
        }
        let (model, sim) = if w2 != wgt { fit_shape(&det, &w2, init) } else { (model, init) };
        let _ = sim;
        let mut landmarks = model;
        let mut detected = [false; N];
        for i in 0..N {
            if w2[i] > 0.0 {
                landmarks[i] = det[i];
                detected[i] = true;
            }
        }
        // Roll from the jaw line (it does not move with yaw); fall back to the ellipse.
        let jl = landmarks[JAW[0]];
        let jr = landmarks[JAW[1]];
        let roll_jaw = (jr[1] - jl[1]).atan2(jr[0] - jl[0]).to_degrees();
        let roll = if (roll_jaw - roll).abs() < 20.0 { roll_jaw } else { roll };
        Some(FaceFit { landmarks, detected, outline, center, height: b, roll })
    }
}

/// Distance from `c` to the polygon `poly` (star-shaped around `c`) along angle `ang`.
fn ray_radius(poly: &[[f64; 2]], c: [f64; 2], ang: f64) -> f64 {
    let d = [ang.cos(), ang.sin()];
    let mut best = 0.0f64;
    let n = poly.len();
    for i in 0..n {
        let (p, q) = (poly[i], poly[(i + 1) % n]);
        let e = [q[0] - p[0], q[1] - p[1]];
        let den = d[0] * e[1] - d[1] * e[0];
        if den.abs() < 1e-12 {
            continue;
        }
        let w = [p[0] - c[0], p[1] - c[1]];
        let t = (w[0] * e[1] - w[1] * e[0]) / den;
        let s = (w[0] * d[1] - w[1] * d[0]) / den;
        if t > 0.0 && (-1e-9..=1.0 + 1e-9).contains(&s) {
            best = best.max(t);
        }
    }
    best
}

// ---------------------------------------------------------------- measurements

/// Face measurements (Extract & Copy Face Measurements): (id, display name).
pub const MEASUREMENTS: [(&str, &str); 14] = [
    ("headPositionX", "Head Position X"),
    ("headPositionY", "Head Position Y"),
    ("headScale", "Head Scale"),
    ("headOrientationX", "Head Orientation X"),
    ("headOrientationY", "Head Orientation Y"),
    ("headOrientationZ", "Head Orientation Z"),
    ("leftEyeOpenness", "Left Eye Openness"),
    ("rightEyeOpenness", "Right Eye Openness"),
    ("leftEyebrowRaise", "Left Eyebrow Distance from Eye"),
    ("rightEyebrowRaise", "Right Eyebrow Distance from Eye"),
    ("mouthOpenness", "Mouth Openness"),
    ("mouthWidth", "Mouth Width"),
    ("mouthOffsetX", "Mouth Offset X"),
    ("mouthOffsetY", "Mouth Offset Y"),
];

/// Face centre, half-height and roll from the chin and jaw landmarks.
pub fn face_frame(l: &[[f64; 2]; N]) -> ([f64; 2], f64, f64) {
    let (ch, jl, jr) = (l[CHIN], l[JAW[0]], l[JAW[1]]);
    let jm = [0.5 * (jl[0] + jr[0]), 0.5 * (jl[1] + jr[1])];
    let k = 1.0 - LANDMARKS[JAW[0]].2[1];
    let c = [ch[0] + (jm[0] - ch[0]) / k, ch[1] + (jm[1] - ch[1]) / k];
    // Half-height from the chin and from the jaw width (averaged: less noise).
    let jw = (jr[0] - jl[0]).hypot(jr[1] - jl[1]) / (2.0 * LANDMARKS[JAW[1]].2[0]);
    let b = (0.5 * ((ch[0] - c[0]).hypot(ch[1] - c[1]) + jw)).max(1e-9);
    let roll = (jr[1] - jl[1]).atan2(jr[0] - jl[0]).to_degrees();
    (c, b, roll)
}

/// The measurements of a frame's landmarks ([`MEASUREMENTS`] order); `reference_height`: the
/// face half-height that is Head Scale 100 %.
pub fn measure(l: &[[f64; 2]; N], reference_height: f64) -> [f64; 14] {
    let (c, b, roll) = face_frame(l);
    let q = l.map(|p| to_face(c, b, roll, p));
    let canon = LANDMARKS.map(|x| x.2);
    // Inner features slide with yaw and pitch (see `synth_shape`).
    let inner = [EYE[0][4], EYE[1][4], MOUTH[0], MOUTH[1], EYE[0][0], EYE[1][0], EYE[0][1], EYE[1][1]];
    let n = inner.len() as f64;
    let du = inner.iter().map(|i| q[*i][0] - canon[*i][0]).sum::<f64>() / n;
    let dv = inner.iter().map(|i| q[*i][1] - canon[*i][1]).sum::<f64>() / n;
    let yaw = (du / 0.35).clamp(-1.0, 1.0).asin().to_degrees();
    let pitch = (dv / 0.3).clamp(-1.0, 1.0).asin().to_degrees();
    let open = |e: [usize; 5]| {
        let wdt = (q[e[1]][0] - q[e[0]][0]).abs().max(1e-9);
        (q[e[3]][1] - q[e[2]][1]).max(0.0) / wdt * 100.0
    };
    let brow = |side: usize| {
        let e = EYE[side];
        let eye_v = 0.5 * (q[e[2]][1] + q[e[3]][1]);
        (eye_v - q[BROW[side][1]][1]) * 100.0
    };
    let mw = (q[MOUTH[1]][0] - q[MOUTH[0]][0]).abs();
    let mc = [0.5 * (q[MOUTH[0]][0] + q[MOUTH[1]][0]), 0.5 * (q[MOUTH[2]][1] + q[MOUTH[3]][1])];
    [
        c[0],
        c[1],
        b / reference_height.max(1e-9) * 100.0,
        pitch,
        yaw,
        roll,
        open(EYE[0]),
        open(EYE[1]),
        brow(0),
        brow(1),
        (q[MOUTH[3]][1] - q[MOUTH[2]][1]).max(0.0) / mw.max(1e-9) * 100.0,
        mw / (2.0 * ASPECT) * 100.0,
        (mc[0] - q[NOSE_TIP][0]) * 100.0,
        (mc[1] - q[NOSE_TIP][1]) * 100.0,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pearson correlation.
    fn corr(a: &[f64], b: &[f64]) -> f64 {
        let n = a.len() as f64;
        let (ma, mb) = (a.iter().sum::<f64>() / n, b.iter().sum::<f64>() / n);
        let (mut sab, mut saa, mut sbb) = (0.0, 0.0, 0.0);
        for (x, y) in a.iter().zip(b) {
            sab += (x - ma) * (y - mb);
            saa += (x - ma).powi(2);
            sbb += (y - mb).powi(2);
        }
        sab / (saa * sbb).sqrt()
    }

    fn frame_params(t: usize) -> FaceParams {
        let tf = t as f64;
        FaceParams {
            center: [160.0 + 30.0 * (tf * 0.15).sin(), 120.0 + 12.0 * (tf * 0.1).cos()],
            height: 62.0 * (1.0 + 0.08 * (tf * 0.07).sin()),
            roll: 14.0 * (tf * 0.12).sin(),
            yaw: 12.0 * (tf * 0.09).sin(),
            pitch: 6.0 * (tf * 0.11).cos(),
            eye_open: [0.8 + 0.2 * (tf * 0.3).sin(), 0.8 + 0.2 * (tf * 0.3).sin()],
            mouth_open: 0.5 + 0.45 * (tf * 0.2).sin(),
            mouth_width: 1.0 + 0.1 * (tf * 0.13).cos(),
            brow_raise: [0.4 * (tf * 0.17).sin(), 0.4 * (tf * 0.17).sin()],
        }
    }

    #[test]
    fn shape_model_spans_the_synthetic_shapes() {
        let m = pdm();
        assert!(m.modes.len() >= 5 && m.modes.len() <= 14, "{} modes", m.modes.len());
        // A synthetic shape is reproduced from half its landmarks.
        let p = FaceParams { yaw: 15.0, mouth_open: 0.8, eye_open: [0.3, 0.9], center: [100.0, 80.0], height: 50.0, roll: 10.0, ..Default::default() };
        let truth = synth_landmarks(&p);
        let mut w = [0.0; N];
        for (i, wi) in w.iter_mut().enumerate() {
            *wi = if i % 2 == 0 || i >= 19 { 1.0 } else { 0.0 };
        }
        let (sn, cs) = 10f64.to_radians().sin_cos();
        let (fit, _) = fit_shape(&truth, &w, [50.0 * cs, 50.0 * sn, 100.0, 80.0]);
        let err = (0..N).map(|i| (fit[i][0] - truth[i][0]).hypot(fit[i][1] - truth[i][1])).fold(0.0, f64::max);
        assert!(err < 0.06 * 50.0, "max error {err}");
    }

    #[test]
    fn row_crossings_match_the_point_in_polygon_test() {
        let (w, h) = (97, 83);
        let off = [13.0, 7.0];
        let layer = |i: usize, j: usize| [(20 + i) as f64 + 0.5 - off[0] + 0.25, (11 + j) as f64 + 0.5 - off[1]];
        let star: Vec<[f64; 2]> = (0..48)
            .map(|k| {
                let a = k as f64 / 48.0 * std::f64::consts::TAU;
                let r = 30.0 + 9.0 * (5.0 * a).sin() + 3.0 * (k % 3) as f64;
                [50.0 + a.cos() * r, 40.0 + a.sin() * r * 1.3]
            })
            .collect();
        // Vertices on pixel-centre rows and columns exercise the tie cases.
        let tied: Vec<[f64; 2]> = vec![[10.0, 5.0], [70.0, 5.0], [70.0, 60.5], [40.0, 30.5], [10.0, 60.5], [10.0, 5.0]];
        for poly in [star, tied, vec![[1.0, 1.0], [5.0, 5.0]]] {
            let skin: Vec<bool> = (0..w * h).map(|i| (i * 7 + i / w) % 5 == 0).collect();
            let got = non_skin_inside(&poly, &skin, layer, w, h);
            let mut inside = 0;
            for j in 0..h {
                for i in 0..w {
                    let want = !skin[j * w + i] && crate::mask::inside_polygon(&poly, layer(i, j));
                    assert_eq!(got[j * w + i], want, "({i}, {j})");
                    inside += want as usize;
                }
            }
            assert!(inside > 0 || poly.len() < 3);
        }
    }

    #[test]
    fn tracks_a_moving_rotating_synthetic_face() {
        let (w, h) = (320, 240);
        let p0 = frame_params(0);
        let img0 = render_synthetic(w, h, &p0, 3);
        // The user's mask: a loose box around the face.
        let region = [p0.center[0] - 60.0, p0.center[1] - 75.0, p0.center[0] + 60.0, p0.center[1] + 75.0];
        let (mut tr, fit0) = FaceTracker::new(&Frame { img: &img0, offset: [0.0; 2] }, region).expect("face found");
        let mut fits = vec![fit0];
        let n = 40;
        for t in 1..n {
            let img = render_synthetic(w, h, &frame_params(t), 3);
            fits.push(tr.step(&Frame { img: &img, offset: [0.0; 2] }).unwrap_or_else(|| panic!("lost at {t}")));
        }
        let mut per = [0.0f64; N];
        let mut worst = 0.0f64;
        let mut outline_err = 0.0;
        let mut meas = vec![];
        for (t, f) in fits.iter().enumerate() {
            let p = frame_params(t);
            let truth = synth_landmarks(&p);
            for i in 0..N {
                let e = (f.landmarks[i][0] - truth[i][0]).hypot(f.landmarks[i][1] - truth[i][1]) / p.height;
                per[i] += e / n as f64;
                worst = worst.max(e);
            }
            // Outline: on the face ellipse.
            let mut oe = 0.0;
            for q in &f.outline {
                let u = to_face(p.center, p.height, p.roll, *q);
                oe += ((u[0] / ASPECT).powi(2) + u[1] * u[1]).sqrt() - 1.0;
            }
            outline_err += (oe / f.outline.len() as f64).abs() / n as f64;
            meas.push((p, measure(&f.landmarks, fits[0].height)));
            assert!((f.roll - p.roll).abs() < 3.0, "frame {t}: roll {} vs {}", f.roll, p.roll);
        }
        let mean = per.iter().sum::<f64>() / N as f64;
        eprintln!("face landmarks: mean error {mean:.4}, worst {worst:.4} face heights; outline {outline_err:.4}");
        for (i, e) in per.iter().enumerate() {
            assert!(*e < 0.04, "{}: mean error {e:.3} (face heights)", LANDMARKS[i].0);
        }
        assert!(mean < 0.02, "mean landmark error {mean:.4}");
        assert!(worst < 0.08, "worst landmark error {worst:.3}");
        assert!(outline_err < 0.04, "outline error {outline_err:.3}");
        // Measurements follow the generator: mouth openness and yaw correlate strongly.
        let mo: Vec<f64> = meas.iter().map(|m| m.1[10]).collect();
        let mt: Vec<f64> = meas.iter().map(|m| m.0.mouth_open).collect();
        assert!(corr(&mo, &mt) > 0.95, "mouth openness r = {}", corr(&mo, &mt));
        let yw: Vec<f64> = meas.iter().map(|m| m.1[4]).collect();
        let yt: Vec<f64> = meas.iter().map(|m| m.0.yaw).collect();
        assert!(corr(&yw, &yt) > 0.9, "yaw r = {}", corr(&yw, &yt));
        let rl: Vec<f64> = meas.iter().map(|m| m.1[5]).collect();
        let rt: Vec<f64> = meas.iter().map(|m| m.0.roll).collect();
        assert!(corr(&rl, &rt) > 0.98, "roll r = {}", corr(&rl, &rt));
        let sc: Vec<f64> = meas.iter().map(|m| m.1[2]).collect();
        let st: Vec<f64> = meas.iter().map(|m| m.0.height).collect();
        assert!(corr(&sc, &st) > 0.95, "scale r = {}", corr(&sc, &st));
    }

    #[test]
    fn measurements_of_the_synthetic_model_are_exact() {
        let p = FaceParams { center: [50.0, 60.0], height: 40.0, roll: 20.0, yaw: 10.0, pitch: -5.0, ..Default::default() };
        let m = measure(&synth_landmarks(&p), 40.0);
        assert!((m[0] - 50.0).abs() < 1e-6 && (m[1] - 60.0).abs() < 1e-6, "{m:?}");
        assert!((m[2] - 100.0).abs() < 1e-6);
        assert!((m[5] - 20.0).abs() < 1e-6);
        assert!((m[4] - 10.0).abs() < 0.5 && (m[3] + 5.0).abs() < 0.5, "{m:?}");
    }

    /// A stand-in trained model: it "sees" the synthetic clip's true landmarks (points
    /// `0..23` in `LANDMARKS` order) and its true face ellipse (points `N..N + 36`), frame after
    /// frame, until `lose_at`; `blind`, it finds no face.
    struct Fake {
        frame: std::sync::atomic::AtomicUsize,
        lose_at: usize,
        blind: bool,
    }

    fn fake(lose_at: usize, blind: bool) -> Arc<Fake> {
        Arc::new(Fake { frame: 0.into(), lose_at, blind })
    }

    const OUTLINE_POINTS: usize = 36;
    static FAKE_OUTLINE: [usize; OUTLINE_POINTS] = {
        let mut a = [0; OUTLINE_POINTS];
        let mut i = 0;
        while i < OUTLINE_POINTS {
            a[i] = N + i;
            i += 1;
        }
        a
    };
    static FAKE_LANDMARKS: [(&str, usize); CHIN] = {
        let mut a = [("", 0); CHIN];
        let mut i = 0;
        while i < CHIN {
            a[i] = (LANDMARKS[i].0, i);
            i += 1;
        }
        a
    };
    static FAKE_TOPOLOGY: Topology = Topology { outline: &FAKE_OUTLINE, landmarks: &FAKE_LANDMARKS };
    static FAKE_INFO: effectcraft_segment::ModelInfo = effectcraft_segment::ModelInfo { id: "fake", name: "Fake", ..effectcraft_segment::FACE_LANDMARKER };

    impl Fake {
        fn face(t: usize) -> Face {
            let p = frame_params(t);
            let mut points: Vec<[f32; 2]> = synth_landmarks(&p).iter().map(|q| [q[0] as f32, q[1] as f32]).collect();
            for k in 0..OUTLINE_POINTS {
                let a = k as f64 / OUTLINE_POINTS as f64 * std::f64::consts::TAU;
                let q = to_layer(p.center, p.height, p.roll, [ASPECT * a.sin(), -a.cos()]);
                points.push([q[0] as f32, q[1] as f32]);
            }
            Face { points, score: 1.0 }
        }
    }

    impl FaceModel for Fake {
        fn info(&self) -> &'static effectcraft_segment::ModelInfo {
            &FAKE_INFO
        }
        fn topology(&self) -> &'static Topology {
            &FAKE_TOPOLOGY
        }
        fn find(&self, _: &[[f32; 3]], _: usize, _: usize, r: [f32; 4]) -> effectcraft_segment::Result<Option<Face>> {
            let c = frame_params(0).center;
            let inside = (r[0] as f64) < c[0] && c[0] < r[2] as f64 && (r[1] as f64) < c[1] && c[1] < r[3] as f64;
            Ok((inside && !self.blind).then(|| Fake::face(0)))
        }
        fn follow(&self, _: &[[f32; 3]], _: usize, _: usize, _: &Face) -> effectcraft_segment::Result<Option<Face>> {
            let t = self.frame.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            Ok((t < self.lose_at).then(|| Fake::face(t)))
        }
    }

    #[test]
    fn a_trained_model_drives_the_fit() {
        let (w, h) = (320, 240);
        let p0 = frame_params(0);
        let img0 = render_synthetic(w, h, &p0, 3);
        let frame0 = Frame { img: &img0, offset: [0.0; 2] };
        let region = [p0.center[0] - 60.0, p0.center[1] - 75.0, p0.center[0] + 60.0, p0.center[1] + 75.0];
        let (mut tr, fit0) = FaceTracker::new_with(&frame0, region, Some(fake(30, false))).expect("face found");
        assert_eq!(tr.model().map(|m| m.id), Some("fake"));
        let mut fits = vec![fit0];
        for t in 1..30 {
            let img = render_synthetic(w, h, &frame_params(t), 3);
            fits.push(tr.step(&Frame { img: &img, offset: [0.0; 2] }).unwrap_or_else(|| panic!("lost at {t}")));
        }
        let mut meas = vec![];
        for (t, f) in fits.iter().enumerate() {
            let p = frame_params(t);
            let truth = synth_landmarks(&p);
            // The model's points are the landmarks; chin and jaw come from its outline.
            for i in 0..N {
                let e = (f.landmarks[i][0] - truth[i][0]).hypot(f.landmarks[i][1] - truth[i][1]) / p.height;
                assert!(e < if i < CHIN { 1e-4 } else { 0.06 }, "frame {t} {}: {e}", LANDMARKS[i].0);
            }
            assert!(f.detected.iter().all(|d| *d) && f.outline.len() == OUTLINE_POINTS);
            assert!((f.height - p.height).abs() < 0.03 * p.height, "frame {t}: height {} vs {}", f.height, p.height);
            assert!((f.roll - p.roll).abs() < 3.0, "frame {t}: roll {} vs {}", f.roll, p.roll);
            meas.push((p, measure(&f.landmarks, fits[0].height)));
        }
        let pick = |k: usize| meas.iter().map(|m| m.1[k]).collect::<Vec<_>>();
        let mt: Vec<f64> = meas.iter().map(|m| m.0.mouth_open).collect();
        assert!(corr(&pick(10), &mt) > 0.95);
        let rt: Vec<f64> = meas.iter().map(|m| m.0.roll).collect();
        assert!(corr(&pick(5), &rt) > 0.98);
        // The model loses the face: so does the tracker.
        let img = render_synthetic(w, h, &frame_params(30), 3);
        assert!(tr.step(&Frame { img: &img, offset: [0.0; 2] }).is_none());
        // A model that finds no face in the mask leaves the face to the classical tracker.
        let (classic, _) = FaceTracker::new_with(&frame0, region, Some(fake(30, true))).expect("the classic tracker finds it");
        assert!(classic.model().is_none());
        // Layer offsets: frame pixels = layer + offset; fits stay in layer pixels.
        let shifted = Frame { img: &img0, offset: [10.0, -4.0] };
        let moved = [region[0] - 10.0, region[1] + 4.0, region[2] - 10.0, region[3] + 4.0];
        let (_, f) = FaceTracker::new_with(&shifted, moved, Some(fake(9, false))).expect("found");
        let truth = synth_landmarks(&p0);
        assert!((f.landmarks[EYE[0][4]][0] - (truth[EYE[0][4]][0] - 10.0)).abs() < 1e-3);
    }

    /// With the official MediaPipe Face Landmarker (`EFFECTCRAFT_FACE_LANDMARKER` = path to
    /// `face_landmarker.task`, else skipped): it recognises the synthetic face and follows it
    /// through the moving, rolling clip. Brows and lips are drawn differently from where a real
    /// face's mesh points sit, so only the points both agree on are held to ground truth: eyes and
    /// chin tightly, the mouth corners (the cartoon mouth's ends, as it turns) more loosely.
    #[test]
    fn mediapipe_follows_the_synthetic_face() {
        let Ok(path) = std::env::var("EFFECTCRAFT_FACE_LANDMARKER") else { return };
        let Ok(effectcraft_segment::Loaded::Face(model)) = effectcraft_segment::load("mediapipe-face", &std::fs::read(path).unwrap()) else {
            panic!("not a face model")
        };
        let (w, h) = (320, 240);
        let p0 = frame_params(0);
        let img0 = render_synthetic(w, h, &p0, 3);
        let region = [p0.center[0] - 60.0, p0.center[1] - 75.0, p0.center[0] + 60.0, p0.center[1] + 75.0];
        let (mut tr, fit0) = FaceTracker::new_with(&Frame { img: &img0, offset: [0.0; 2] }, region, Some(model)).expect("face found");
        assert_eq!(tr.model().map(|m| m.id), Some("mediapipe-face"));
        let mut fits = vec![fit0];
        let n = 40;
        for t in 1..n {
            let img = render_synthetic(w, h, &frame_params(t), 3);
            fits.push(tr.step(&Frame { img: &img, offset: [0.0; 2] }).unwrap_or_else(|| panic!("lost at {t}")));
        }
        let held = [(EYE[0][1], 0.04), (EYE[0][4], 0.04), (EYE[1][1], 0.04), (EYE[1][4], 0.04), (CHIN, 0.04), (MOUTH[0], 0.08), (MOUTH[1], 0.08)];
        for (t, f) in fits.iter().enumerate() {
            let p = frame_params(t);
            let truth = synth_landmarks(&p);
            for &(i, bound) in &held {
                let e = (f.landmarks[i][0] - truth[i][0]).hypot(f.landmarks[i][1] - truth[i][1]) / p.height;
                assert!(e < bound, "frame {t} {}: {e:.3} face heights", LANDMARKS[i].0);
            }
            assert!((f.roll - p.roll).abs() < 3.0, "frame {t}: roll {} vs {}", f.roll, p.roll);
        }
        // Head Scale follows the face's size (relative to the first frame).
        let h0 = frame_params(0).height;
        let base = measure(&fits[0].landmarks, 1.0)[2];
        let worst_scale = fits.iter().enumerate().map(|(t, f)| (measure(&f.landmarks, 1.0)[2] / base - frame_params(t).height / h0).abs()).fold(0.0, f64::max);
        assert!(worst_scale < 0.05, "Head Scale off by {worst_scale:.3}");
    }
}
