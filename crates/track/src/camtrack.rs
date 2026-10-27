//! The 3D Camera Tracker's analysis: structure from motion on a single clip, clean-room from the
//! textbook pipeline (Hartley & Zisserman, *Multiple View Geometry in Computer Vision*, 2nd ed.,
//! 2004; Triggs et al., "Bundle Adjustment — A Modern Synthesis", 2000):
//!
//! 1. **Feature tracks** ([`features::TrackAnalyzer`]): Shi–Tomasi corners followed through the
//!    clip with pyramidal Lucas–Kanade and a forward–backward check ([`crate::klt`]); lost
//!    features end their track and new ones are detected where the frame has none, so tracks
//!    are as long as possible.
//! 2. **Solve** ([`solve`]): keyframes are selected by parallax; a two-view reconstruction
//!    initialises the scene from either the **essential matrix** (normalised 8-point algorithm
//!    inside RANSAC, H&Z §11.2 and §9.6) or, for mostly flat scenes, a **homography
//!    decomposition** (Ma, Soatto, Košecká & Sastry, *An Invitation to 3-D Vision*, 2004, §5.3);
//!    the other keyframes are added incrementally (**resection** by robust pose refinement,
//!    **triangulation** of new tracks by the linear method of H&Z §12.2); a sparse
//!    **Levenberg–Marquardt bundle adjustment** with the Schur complement over the points
//!    ([`bundle`]) refines everything; the frames between keyframes are resected last. Inlier
//!    thresholds start from the analysis resolution and are re-estimated from the residuals
//!    (median absolute residual) once the keyframes are solved, so features on moving objects,
//!    occlusion edges and screen-locked overlays drop out.
//! 3. **Focal length**: *Specify Angle of View* fixes it; *Fixed Angle of View* searches it
//!    (the reprojection error of the keyframe reconstruction over a log-spaced range of focal
//!    lengths, refined by golden-section search) and then adjusts it in the bundle adjustment
//!    as one shared parameter; *Variable Zoom* starts from the same estimate and adjusts a focal
//!    length per frame, with a weak smoothness prior between neighbouring frames.
//! 4. **Tripod pan** shots (no camera translation) are solved as a pure rotation: frame-to-frame
//!    rotations from the tracked rays (orthogonal Procrustes) refined by the same bundle
//!    adjustment with the camera centres fixed. *Auto Detect* solves both ways and keeps the
//!    tripod solution when it explains the tracks as well as the general one (a quick rotation fit
//!    at the general solution's focal length skips the tripod search when it clearly fails).
//!
//! 5. **Lens distortion** (Solve Lens Distortion, or Detailed Analysis): radial distortion
//!    `p_d = p (1 + k1 ρ² + k2 ρ⁴)` (ρ = distance from the principal point over half the frame
//!    diagonal; Brown's model without the tangential terms) is solved jointly with the cameras
//!    and points in a final bundle adjustment on the raw tracks; the whole solve is then repeated
//!    on tracks undistorted with that estimate (the linear initialisations assume a pinhole
//!    camera) and adjusted jointly again. The solve stores `k1, k2`; the effect can undistort
//!    its output so composited 3D layers line up.
//!
//! The camera model has square pixels and the principal point at the layer centre. Camera space is x right, y down, z forward (After Effects' convention); a camera
//! is `(R, C, f)` with `X_cam = R (X − C)` and pixel `u = f x / z + w / 2`.
//!
//! The solve is stored in a canonical frame: the first solved frame's camera is at the origin
//! looking along +z, scaled so the median depth of the points it sees is 1. [`CameraSolve::world`]
//! maps it into composition space for a camera zoom, optionally re-based on a ground plane.

pub mod bundle;
pub mod features;
pub mod geometry;
pub mod linalg;
mod solver;
#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};

pub use features::{AnalyzeOpts, TrackAnalyzer};
use linalg::{M3, Similarity, V3};
pub use solver::{SolveError, solve};

/// One feature track: positions (layer pixels) on consecutive frames from `start`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track2D {
    pub id: u32,
    pub start: u32,
    pub pts: Vec<[f32; 2]>,
}

impl Track2D {
    pub fn end(&self) -> u32 {
        self.start + self.pts.len() as u32
    }
    /// Position on frame `k` when the track covers it.
    pub fn at(&self, k: u32) -> Option<[f64; 2]> {
        let i = k.checked_sub(self.start)? as usize;
        self.pts.get(i).map(|p| [p[0] as f64, p[1] as f64])
    }
}

/// The tracked features of a clip (step 1 of the analysis).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CameraTracks {
    pub version: u32,
    /// Layer size (pixels).
    pub size: [f64; 2],
    /// Layer time (seconds) of frame 0, and the frame duration.
    pub start: f64,
    pub frame_duration: f64,
    pub frames: u32,
    /// Analysis downsampling factor (image pixels per analysis pixel): the tracking noise scale.
    pub factor: f64,
    pub detailed: bool,
    pub tracks: Vec<Track2D>,
}

impl CameraTracks {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
    pub fn from_json(s: &str) -> Option<CameraTracks> {
        if s.trim().is_empty() {
            return None;
        }
        serde_json::from_str(s).ok()
    }
    pub fn is_empty(&self) -> bool {
        self.frames == 0
    }
    pub fn track(&self, id: u32) -> Option<&Track2D> {
        self.tracks.binary_search_by_key(&id, |t| t.id).ok().map(|i| &self.tracks[i])
    }
}

/// Radial lens distortion around the principal point (see the module docs).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Distortion {
    pub k1: f64,
    pub k2: f64,
    /// Normalisation radius ρ = 1 (pixels): half the frame diagonal; 0 = no distortion.
    pub radius: f64,
}

impl Distortion {
    pub const NONE: Distortion = Distortion { k1: 0.0, k2: 0.0, radius: 0.0 };

    /// No distortion for frames of `size` (the radius set, coefficients zero).
    pub fn for_size(size: [f64; 2]) -> Distortion {
        Distortion { k1: 0.0, k2: 0.0, radius: 0.5 * size[0].hypot(size[1]) }
    }
    pub fn is_none(&self) -> bool {
        self.radius <= 0.0 || (self.k1 == 0.0 && self.k2 == 0.0)
    }
    fn rho2(&self, p: [f64; 2]) -> f64 {
        (p[0] * p[0] + p[1] * p[1]) / (self.radius * self.radius)
    }
    /// Ideal → distorted (pixels relative to the principal point).
    pub fn distort(&self, p: [f64; 2]) -> [f64; 2] {
        if self.radius <= 0.0 {
            return p;
        }
        let r2 = self.rho2(p);
        let d = 1.0 + self.k1 * r2 + self.k2 * r2 * r2;
        [p[0] * d, p[1] * d]
    }
    /// Distorted → ideal (fixed-point iteration).
    pub fn undistort(&self, q: [f64; 2]) -> [f64; 2] {
        if self.is_none() {
            return q;
        }
        let mut p = q;
        for _ in 0..30 {
            let r2 = self.rho2(p);
            let d = 1.0 + self.k1 * r2 + self.k2 * r2 * r2;
            if d.abs() < 1e-6 {
                break;
            }
            p = [q[0] / d, q[1] / d];
        }
        p
    }
    /// `d distort / d p` (2 × 2, row-major) and `d distort / d (k1, k2)` (rows x, y).
    pub fn jacobian(&self, p: [f64; 2]) -> ([[f64; 2]; 2], [[f64; 2]; 2]) {
        if self.radius <= 0.0 {
            return ([[1.0, 0.0], [0.0, 1.0]], [[0.0; 2]; 2]);
        }
        let r2 = self.rho2(p);
        let d = 1.0 + self.k1 * r2 + self.k2 * r2 * r2;
        // d(r2)/dp = 2 p / R².
        let dd = (self.k1 + 2.0 * self.k2 * r2) * 2.0 / (self.radius * self.radius);
        let j = [[d + p[0] * dd * p[0], p[0] * dd * p[1]], [p[1] * dd * p[0], d + p[1] * dd * p[1]]];
        let k = [[p[0] * r2, p[0] * r2 * r2], [p[1] * r2, p[1] * r2 * r2]];
        (j, k)
    }
}

/// 3D Camera Tracker ▸ Shot Type.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ShotType {
    #[default]
    FixedAngle,
    VariableZoom,
    SpecifyAngle,
}

impl ShotType {
    pub const ALL: [ShotType; 3] = [ShotType::FixedAngle, ShotType::VariableZoom, ShotType::SpecifyAngle];
    pub fn label(self) -> &'static str {
        match self {
            ShotType::FixedAngle => "Fixed Angle of View",
            ShotType::VariableZoom => "Variable Zoom",
            ShotType::SpecifyAngle => "Specify Angle of View",
        }
    }
    pub fn from_name(s: &str) -> Option<ShotType> {
        let k = s.to_ascii_lowercase().replace([' ', '-', '_'], "");
        match k.as_str() {
            "fixed" | "fixedangle" | "fixedangleofview" => Some(ShotType::FixedAngle),
            "variable" | "variablezoom" | "zoom" => Some(ShotType::VariableZoom),
            "specify" | "specifyangle" | "specifyangleofview" => Some(ShotType::SpecifyAngle),
            _ => None,
        }
    }
}

/// 3D Camera Tracker ▸ Advanced ▸ Solve Method (and Method Used).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SolveMethod {
    #[default]
    Auto,
    Typical,
    MostlyFlat,
    TripodPan,
}

impl SolveMethod {
    pub const ALL: [SolveMethod; 4] = [SolveMethod::Auto, SolveMethod::Typical, SolveMethod::MostlyFlat, SolveMethod::TripodPan];
    pub fn label(self) -> &'static str {
        match self {
            SolveMethod::Auto => "Auto Detect",
            SolveMethod::Typical => "Typical",
            SolveMethod::MostlyFlat => "Mostly Flat Scene",
            SolveMethod::TripodPan => "Tripod Pan",
        }
    }
    pub fn from_name(s: &str) -> Option<SolveMethod> {
        let k = s.to_ascii_lowercase().replace([' ', '-', '_'], "");
        match k.as_str() {
            "auto" | "autodetect" => Some(SolveMethod::Auto),
            "typical" => Some(SolveMethod::Typical),
            "flat" | "mostlyflat" | "mostlyflatscene" => Some(SolveMethod::MostlyFlat),
            "tripod" | "tripodpan" => Some(SolveMethod::TripodPan),
            _ => None,
        }
    }
}

/// What a solve is made with.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SolveSettings {
    pub shot: ShotType,
    /// Horizontal angle of view (degrees) for [`ShotType::SpecifyAngle`].
    pub hfov: f64,
    pub method: SolveMethod,
    /// Track ids the user deleted.
    pub deleted: Vec<u32>,
    /// Solve radial lens distortion jointly with the cameras.
    pub lens_distortion: bool,
}

/// The solved camera of one frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SolvedFrame {
    /// World → camera rotation (axis-angle).
    pub rot: V3,
    /// Camera centre.
    pub center: V3,
    /// Focal length (layer pixels).
    pub focal: f64,
    pub solved: bool,
}

impl SolvedFrame {
    pub fn r(&self) -> M3 {
        linalg::rodrigues(self.rot)
    }
    /// Camera-space position of a world point.
    pub fn to_cam(&self, x: V3) -> V3 {
        linalg::mv(&self.r(), linalg::sub(x, self.center))
    }
    /// Project a world point to layer pixels (`None` behind the camera).
    pub fn project(&self, size: [f64; 2], x: V3) -> Option<[f64; 2]> {
        let c = self.to_cam(x);
        (c[2] > 1e-9).then(|| [self.focal * c[0] / c[2] + size[0] * 0.5, self.focal * c[1] / c[2] + size[1] * 0.5])
    }
}

/// A solved 3D track point.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SolvedPoint {
    /// The feature track it was made from.
    pub id: u32,
    pub pos: V3,
    /// Average reprojection error (pixels).
    pub error: f32,
    /// Frames `[first, last]` the point was tracked on.
    pub first: u32,
    pub last: u32,
}

/// A ground plane and origin (canonical solve coordinates).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Ground {
    pub origin: V3,
    /// Unit normal on the cameras' side ("up").
    pub normal: V3,
    /// Unit in-plane direction that becomes world +x.
    pub x_axis: V3,
}

/// A plane through solved points: After Effects' target.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Target {
    pub center: V3,
    /// Unit normal facing the camera.
    pub normal: V3,
    /// Diameter in canonical units.
    pub size: f64,
}

/// The camera solve (step 2 of the analysis).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CameraSolve {
    pub version: u32,
    pub size: [f64; 2],
    pub start: f64,
    pub frame_duration: f64,
    pub shot: ShotType,
    /// The model that was used (Auto Detect resolves to one of the others).
    pub method_used: SolveMethod,
    /// Average reprojection error (pixels) over all solved observations.
    pub average_error: f64,
    pub frames: Vec<SolvedFrame>,
    pub points: Vec<SolvedPoint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ground: Option<Ground>,
    /// The solved lens distortion (Solve Lens Distortion).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distortion: Option<Distortion>,
}

impl CameraSolve {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
    pub fn from_json(s: &str) -> Option<CameraSolve> {
        if s.trim().is_empty() {
            return None;
        }
        serde_json::from_str(s).ok()
    }
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
    /// Frame index at a layer time (held at the ends).
    pub fn frame_at(&self, layer_time: f64) -> Option<usize> {
        if self.frames.is_empty() || self.frame_duration <= 0.0 {
            return None;
        }
        let i = ((layer_time - self.start) / self.frame_duration).round();
        Some(i.clamp(0.0, (self.frames.len() - 1) as f64) as usize)
    }
    pub fn point(&self, id: u32) -> Option<&SolvedPoint> {
        self.points.iter().find(|p| p.id == id)
    }
    /// Points tracked on frame `k`.
    pub fn visible(&self, k: usize) -> impl Iterator<Item = &SolvedPoint> {
        let k = k as u32;
        self.points.iter().filter(move |p| p.first <= k && k <= p.last)
    }
    /// Median focal length of the solved frames (layer pixels).
    pub fn focal(&self) -> f64 {
        let mut f: Vec<f64> = self.frames.iter().filter(|c| c.solved).map(|c| c.focal).collect();
        if f.is_empty() {
            return 0.0;
        }
        f.sort_by(f64::total_cmp);
        f[f.len() / 2]
    }
    /// Horizontal angle of view (degrees) of focal length `f`.
    pub fn hfov(&self, f: f64) -> f64 {
        2.0 * (self.size[0] * 0.5 / f.max(1e-9)).atan().to_degrees()
    }

    /// The plane through points `ids` seen from frame `k` (needs three points; with more the
    /// plane is a least-squares fit).
    pub fn target(&self, ids: &[u32], k: usize) -> Option<Target> {
        let pts: Vec<V3> = ids.iter().filter_map(|i| self.point(*i)).map(|p| p.pos).collect();
        let cam = self.frames.get(k).filter(|c| c.solved).or_else(|| self.frames.iter().find(|c| c.solved))?;
        geometry::fit_plane(&pts).map(|(c, mut n, spread)| {
            if linalg::dot(n, linalg::sub(cam.center, c)) < 0.0 {
                n = linalg::scale(n, -1.0);
            }
            // After Effects sizes the target from the camera distance when the points are close.
            let depth = linalg::norm(linalg::sub(cam.center, c));
            Target { center: c, normal: n, size: (spread * 2.0).max(depth * 0.12) }
        })
    }

    /// A ground plane through `ids` (origin at their centroid, normal towards frame `k`'s
    /// camera, +x along that camera's right vector projected on the plane).
    pub fn ground_from(&self, ids: &[u32], k: usize) -> Option<Ground> {
        let t = self.target(ids, k)?;
        let cam = self.frames.get(k).filter(|c| c.solved).or_else(|| self.frames.iter().find(|c| c.solved))?;
        let right = linalg::mtv(&cam.r(), [1.0, 0.0, 0.0]);
        let mut x = linalg::sub(right, linalg::scale(t.normal, linalg::dot(right, t.normal)));
        if linalg::norm(x) < 1e-6 {
            let fwd = linalg::mtv(&cam.r(), [0.0, 0.0, 1.0]);
            x = linalg::cross(fwd, t.normal);
        }
        Some(Ground { origin: t.center, normal: t.normal, x_axis: linalg::normalize(x) })
    }

    /// Canonical solve coordinates → composition world, for a camera whose zoom on the first
    /// solved frame is `zoom0` (comp pixels) in a `comp` sized composition. Without a ground
    /// plane, the first solved frame's camera becomes After Effects' default camera (at
    /// `(w/2, h/2, −zoom)` looking down +z, so points at the median depth land near z = 0). With
    /// one, the ground plane becomes the X-Z plane through the origin, normal along −y (up).
    pub fn world(&self, comp: [f64; 2], zoom0: f64) -> Similarity {
        match &self.ground {
            Some(g) => {
                // Rows: canonical directions that map to world x, y, z.
                let y = linalg::scale(g.normal, -1.0);
                let x = g.x_axis;
                let z = linalg::cross(x, y);
                let r = [x, y, z];
                let t = linalg::scale(linalg::mv(&r, g.origin), -zoom0);
                Similarity { s: zoom0, r, t }
            }
            None => Similarity { s: zoom0, r: linalg::I3, t: [comp[0] * 0.5, comp[1] * 0.5, -zoom0] },
        }
    }

    /// An ideal (pinhole) image point → where it appears in the footage (layer pixels), through
    /// the solved lens distortion.
    pub fn to_image(&self, p: [f64; 2]) -> [f64; 2] {
        match &self.distortion {
            Some(d) if !d.is_none() => {
                let c = [self.size[0] * 0.5, self.size[1] * 0.5];
                let q = d.distort([p[0] - c[0], p[1] - c[1]]);
                [q[0] + c[0], q[1] + c[1]]
            }
            _ => p,
        }
    }
    /// A footage point → its ideal (undistorted) position.
    pub fn to_ideal(&self, p: [f64; 2]) -> [f64; 2] {
        match &self.distortion {
            Some(d) if !d.is_none() => {
                let c = [self.size[0] * 0.5, self.size[1] * 0.5];
                let q = d.undistort([p[0] - c[0], p[1] - c[1]]);
                [q[0] + c[0], q[1] + c[1]]
            }
            _ => p,
        }
    }
    /// Project world point `x` on frame `k`: through the lens distortion into the footage, or
    /// (`undistorted`) into the undistorted image.
    pub fn project(&self, k: usize, x: V3, undistorted: bool) -> Option<[f64; 2]> {
        let q = self.frames.get(k)?.project(self.size, x)?;
        Some(if undistorted { q } else { self.to_image(q) })
    }

    /// The first solved frame.
    pub fn first_solved(&self) -> Option<usize> {
        self.frames.iter().position(|c| c.solved)
    }
}

/// Deterministic colour of a track point (After Effects draws each target in its own colour).
pub fn point_color(id: u32) -> [f32; 3] {
    let mut h = id.wrapping_mul(0x9E37_79B9) ^ 0x5bd1_e995;
    h ^= h >> 15;
    let hue = (h % 360) as f32 / 60.0;
    let x = 1.0 - (hue % 2.0 - 1.0).abs();
    let (r, g, b) = match hue as u32 {
        0 => (1.0, x, 0.0),
        1 => (x, 1.0, 0.0),
        2 => (0.0, 1.0, x),
        3 => (0.0, x, 1.0),
        4 => (x, 0.0, 1.0),
        _ => (1.0, 0.0, x),
    };
    // Bright, slightly desaturated.
    [0.25 + 0.75 * r, 0.25 + 0.75 * g, 0.25 + 0.75 * b]
}
