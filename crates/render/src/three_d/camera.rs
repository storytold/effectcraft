//! Cameras: the comp's active camera (camera layers or the default 50 mm comp camera), the 3D
//! views of the viewer (Front/Left/Top/Back/Right/Bottom orthographic views and three custom
//! perspective views) and the camera-tool math (orbit, pan, dolly) shared by the engine commands.
//!
//! Conventions: world space is comp space (x right, y down, z into the screen). Camera space has
//! x right, y down and z forward (depth). The perspective projection maps camera space to comp
//! pixels as `x' = zoom·x/z + w/2`; orthographic views use `x' = zoom·x + w/2`.

use std::collections::BTreeMap;

use effectcraft_geom::{Mat4, Vec2, Vec3, vec2, vec3};
use effectcraft_project::{AutoOrient, Layer};
use serde::{Deserialize, Serialize};

use crate::EvalCtx;

/// Closest a perspective camera renders (camera-space z, pixels).
pub const NEAR: f64 = 1.0;

/// Depth of field settings of a camera.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Dof {
    /// Distance from the eye to the plane in focus (pixels).
    pub focus: f64,
    /// Aperture diameter (pixels).
    pub aperture: f64,
    /// Blur Level (1.0 = 100%).
    pub blur_level: f64,
    /// Iris Shape, Rotation, Roundness, Aspect Ratio and Diffraction Fringe.
    #[serde(default)]
    pub iris: super::bokeh::Iris,
    /// Highlight Gain, Threshold and Saturation.
    #[serde(default)]
    pub highlight: super::bokeh::Highlight,
}

impl Dof {
    /// Circle-of-confusion diameter (comp pixels) for a point at camera depth `z`.
    pub fn coc(&self, z: f64) -> f64 {
        if z <= NEAR || self.focus <= 0.0 {
            return 0.0;
        }
        self.aperture * (z - self.focus).abs() / z * self.blur_level
    }
}

/// An evaluated camera.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CameraState {
    /// World → camera space (rigid).
    pub view: Mat4,
    /// Eye position in world space.
    pub eye: Vec3,
    /// Perspective: distance from the eye to the image plane (pixels). Orthographic: pixels per
    /// world unit.
    pub zoom: f64,
    pub ortho: bool,
    pub dof: Option<Dof>,
}

impl CameraState {
    /// World → comp pixels (homogeneous: divide by `w` for perspective cameras).
    pub fn projection(&self, comp_w: f64, comp_h: f64) -> Mat4 {
        if self.ortho {
            let z = self.zoom;
            Mat4([[z, 0.0, 0.0, comp_w * 0.5], [0.0, z, 0.0, comp_h * 0.5], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]]) * self.view
        } else {
            effectcraft_geom::camera_matrix(comp_w, comp_h, self.eye, self.view, self.zoom)
        }
    }
    /// Camera-space depth of a world point.
    pub fn depth(&self, p: Vec3) -> f64 {
        self.view.apply(p).z
    }
    /// Unit view direction (world).
    pub fn forward(&self) -> Vec3 {
        let m = &self.view.0;
        vec3(m[2][0], m[2][1], m[2][2])
    }
    pub fn right(&self) -> Vec3 {
        let m = &self.view.0;
        vec3(m[0][0], m[0][1], m[0][2])
    }
    pub fn down(&self) -> Vec3 {
        let m = &self.view.0;
        vec3(m[1][0], m[1][1], m[1][2])
    }
    /// Project a world point to comp pixels; None when behind a perspective camera.
    pub fn project(&self, comp_w: f64, comp_h: f64, p: Vec3) -> Option<Vec2> {
        let c = self.view.apply(p);
        if self.ortho {
            return Some(vec2(self.zoom * c.x + comp_w * 0.5, self.zoom * c.y + comp_h * 0.5));
        }
        if c.z < NEAR {
            return None;
        }
        Some(vec2(self.zoom * c.x / c.z + comp_w * 0.5, self.zoom * c.y / c.z + comp_h * 0.5))
    }
    /// Direction from a shaded point towards the viewer.
    pub fn to_viewer(&self, p: Vec3) -> Vec3 {
        if self.ortho { -self.forward() } else { (self.eye - p).normalize() }
    }
}

/// View matrix from an eye, a forward direction and a screen-down hint.
pub fn basis_view(eye: Vec3, forward: Vec3, down_hint: Vec3) -> Mat4 {
    let f = forward.normalize();
    let f = if f.length() == 0.0 { vec3(0.0, 0.0, 1.0) } else { f };
    let mut d = down_hint - f * down_hint.dot(f);
    if d.length() < 1e-9 {
        d = if f.y.abs() < 0.99 { vec3(0.0, 1.0, 0.0) } else { vec3(0.0, 0.0, -1.0) };
        d = d - f * d.dot(f);
    }
    let d = d.normalize();
    let r = d.cross(f).normalize();
    Mat4([[r.x, r.y, r.z, 0.0], [d.x, d.y, d.z, 0.0], [f.x, f.y, f.z, 0.0], [0.0, 0.0, 0.0, 1.0]]) * Mat4::translate(-eye)
}

/// Orientation angles (degrees, [`Mat4::orientation`]'s order) that point +z along `dir` with
/// the layer's x axis kept level (no roll): the pitch about x, then the turn about the world's
/// vertical axis, written as Orientation.
pub fn orientation_for(dir: Vec3) -> [f64; 3] {
    let d = dir.normalize();
    let pitch = (-d.y).clamp(-1.0, 1.0).asin().to_degrees();
    let yaw = d.x.atan2(d.z).to_degrees();
    let o = (Mat4::rotate_y(yaw) * Mat4::rotate_x(pitch)).orientation_angles();
    [norm360(o.x), norm360(o.y), norm360(o.z)]
}

fn norm360(a: f64) -> f64 {
    let r = a.rem_euclid(360.0);
    if (r - 360.0).abs() < 1e-9 { 0.0 } else { r }
}

/// AE's camera presets (35 mm film width 36 mm): (name, focal length mm).
pub const PRESETS: [(&str, f64); 9] =
    [("15mm", 15.0), ("20mm", 20.0), ("24mm", 24.0), ("28mm", 28.0), ("35mm", 35.0), ("50mm", 50.0), ("80mm", 80.0), ("135mm", 135.0), ("200mm", 200.0)];

/// Film width the presets assume (mm, measured horizontally).
pub const FILM_SIZE_MM: f64 = 36.0;

/// Zoom (pixels) for a focal length on a comp `comp_w` pixels wide.
pub fn zoom_for_focal(comp_w: f64, focal_mm: f64) -> f64 {
    comp_w * focal_mm / FILM_SIZE_MM
}

/// Focal length (mm) of a zoom value.
pub fn focal_for_zoom(comp_w: f64, zoom: f64) -> f64 {
    zoom * FILM_SIZE_MM / comp_w.max(1.0)
}

/// Horizontal angle of view (degrees) of a zoom value.
pub fn angle_of_view(comp_w: f64, zoom: f64) -> f64 {
    2.0 * (comp_w * 0.5 / zoom.max(1e-9)).atan().to_degrees()
}

/// Default aperture (pixels) of the presets: 25.3 mm at a 1920-pixel-wide comp, proportional.
pub fn default_aperture(comp_w: f64) -> f64 {
    25.3 * comp_w / 1920.0
}

/// The comp camera used when there is no camera layer: the 50 mm preset at (w/2, h/2, −zoom)
/// looking at the comp centre, which shows the z = 0 plane at 100%.
pub fn default_camera(comp_w: f64, comp_h: f64) -> CameraState {
    let zoom = effectcraft_geom::default_camera_zoom(comp_w);
    let eye = vec3(comp_w / 2.0, comp_h / 2.0, -zoom);
    CameraState { view: basis_view(eye, vec3(0.0, 0.0, 1.0), vec3(0.0, 1.0, 0.0)), eye, zoom, ortho: false, dof: None }
}

/// Whether a camera layer is two-node (auto-orients towards its Point of Interest).
pub fn is_two_node(layer: &Layer) -> bool {
    layer.auto_orient == AutoOrient::TowardsPointOfInterest && layer.transform().is_some_and(|t| t.get("poi").is_some())
}

/// Rotation of a camera/light layer from its Orientation and X/Y/Z Rotation (layer → parent).
pub fn layer_rotation(ctx: &EvalCtx, layer: &Layer) -> Mat4 {
    let Some(tr) = layer.transform() else { return Mat4::IDENTITY };
    let o = ctx.v3(layer, tr, "orientation", [0.0; 3]);
    let rx = ctx.f(layer, tr, "rotationX", 0.0);
    let ry = ctx.f(layer, tr, "rotationY", 0.0);
    let rz = ctx.f(layer, tr, "rotation", 0.0);
    Mat4::orientation(Vec3::from(o)) * Mat4::rotation_xyz(rx, ry, rz)
}

/// World matrix of a layer's parent chain (identity when unparented).
pub fn parent_world(ctx: &EvalCtx, layer: &Layer) -> Mat4 {
    match layer.parent.and_then(|p| ctx.comp.layer(p)) {
        Some(p) => ctx.world_matrix(p),
        None => Mat4::IDENTITY,
    }
}

/// Rotation (local → parent) of a frame whose +z is `fwd` and whose +y is as close to
/// `down_hint` as possible.
pub fn basis_rotation(fwd: Vec3, down_hint: Vec3) -> Mat4 {
    let m = basis_view(Vec3::ZERO, fwd, down_hint).0;
    Mat4([[m[0][0], m[1][0], m[2][0], 0.0], [m[0][1], m[1][1], m[2][1], 0.0], [m[0][2], m[1][2], m[2][2], 0.0], [0.0, 0.0, 0.0, 1.0]])
}

/// Local rotation (layer → parent) of a camera or light: auto-orient towards the Point of
/// Interest (two-node), then Orientation and X/Y/Z Rotation.
pub fn rig_rotation(ctx: &EvalCtx, layer: &Layer) -> Mat4 {
    let rot = layer_rotation(ctx, layer);
    match layer.transform().filter(|_| is_two_node(layer)) {
        Some(tr) => {
            let (w, h) = (ctx.comp.width as f64, ctx.comp.height as f64);
            let pos = Vec3::from(ctx.v3(layer, tr, "position", [w / 2.0, h / 2.0, -1000.0]));
            let poi = Vec3::from(ctx.v3(layer, tr, "poi", [w / 2.0, h / 2.0, 0.0]));
            basis_rotation(poi - pos, vec3(0.0, 1.0, 0.0)) * rot
        }
        None => rot,
    }
}

// ---------------------------------------------------------------- auto-orient

/// Direction of travel of a layer's position (parent space) at the context time.
///
/// Where the position is not moving (at and after the last keyframe, before the first, or while
/// holding) the last valid direction of travel is kept, so the layer does not snap back to its
/// unrotated pose at the end of a path.
fn path_tangent(ctx: &EvalCtx, layer: &Layer) -> Option<Vec3> {
    let tr = layer.transform()?;
    // Position (or its separated X/Y/Z dimensions) must be animated.
    let dims = ["position", "positionX", "positionY", "positionZ"];
    let moving = dims.iter().any(|m| tr.get(m).is_some_and(|p| p.keys.len() > 1 || p.has_expression()));
    if !moving {
        return None;
    }
    let dt = effectcraft_time::Tick::from_seconds_f64(0.005);
    let pos = |t| Vec3::from(ctx.at(t).v3(layer, tr, "position", [0.0; 3]));
    let dir = |d: Vec3| (d.length() > 1e-9 && d.length().is_finite()).then(|| d.normalize());
    if let Some(d) = dir(pos(ctx.time + dt) - pos(ctx.time - dt)) {
        return Some(d);
    }
    // No motion here: fall back on the nearest motion before this time (a backward difference at
    // each earlier key, latest first), then on the first motion after it.
    let mut keys: Vec<effectcraft_time::Tick> = dims.iter().filter_map(|m| tr.get(m)).flat_map(|p| p.keys.iter().map(|k| layer.comp_time(k.time))).collect();

    keys.sort();
    keys.dedup();
    keys.iter()
        .rev()
        .filter(|k| **k <= ctx.time)
        .find_map(|&k| dir(pos(k) - pos(k - dt)))
        .or_else(|| keys.iter().filter(|k| **k > ctx.time).find_map(|&k| dir(pos(k + dt) - pos(k))))
}

/// Auto-Orient ▸ Orient Along Path for 2D layers: extra Z rotation in degrees.
pub fn auto_orient_2d(ctx: &EvalCtx, layer: &Layer) -> f64 {
    if layer.auto_orient != AutoOrient::AlongPath {
        return 0.0;
    }
    path_tangent(ctx, layer).map(|t| t.y.atan2(t.x).to_degrees()).unwrap_or(0.0)
}

/// Whether `id` is `layer` or one of its parents.
fn in_parent_chain(ctx: &EvalCtx, layer: &Layer, id: effectcraft_project::LayerId) -> bool {
    let mut cur = Some(layer);
    let mut guard = 0;
    while let Some(l) = cur {
        if l.id == id || guard > 64 {
            return true;
        }
        guard += 1;
        cur = l.parent.and_then(|p| ctx.comp.layer(p));
    }
    false
}

/// Auto-orient for 3D layers: the rotation that replaces Orientation (Along Path: the layer's x
/// axis follows the motion path; Towards Camera: the layer faces the active camera).
pub fn auto_orient_3d(ctx: &EvalCtx, layer: &Layer, pos: Vec3) -> Option<Mat4> {
    match layer.auto_orient {
        AutoOrient::AlongPath => {
            let t = path_tangent(ctx, layer)?;
            // x = tangent, y ≈ down, z = x × y.
            let mut y = vec3(0.0, 1.0, 0.0) - t * t.y;
            if y.length() < 1e-9 {
                y = vec3(0.0, 0.0, 1.0) - t * t.z;
            }
            let y = y.normalize();
            let z = t.cross(y);
            Some(Mat4([[t.x, y.x, z.x, 0.0], [t.y, y.y, z.y, 0.0], [t.z, y.z, z.z, 0.0], [0.0, 0.0, 0.0, 1.0]]))
        }
        AutoOrient::TowardsCamera => {
            // A camera parented (through any chain) to this layer would recurse.
            if ctx.comp.active_camera(ctx.time).is_some_and(|c| in_parent_chain(ctx, c, layer.id)) {
                return None;
            }
            let eye = active_camera(ctx).eye;
            let e = parent_world(ctx, layer).inverse().unwrap_or(Mat4::IDENTITY).apply(eye);
            let d = pos - e;
            (d.length() > 1e-9).then(|| basis_rotation(d, vec3(0.0, 1.0, 0.0)))
        }
        _ => None,
    }
}

/// (eye, forward, down) in world space of a camera or light layer.
pub fn layer_frame(ctx: &EvalCtx, layer: &Layer) -> (Vec3, Vec3, Vec3) {
    let (w, h) = (ctx.comp.width as f64, ctx.comp.height as f64);
    let Some(tr) = layer.transform() else { return (vec3(w / 2.0, h / 2.0, -1000.0), vec3(0.0, 0.0, 1.0), vec3(0.0, 1.0, 0.0)) };
    let parent = parent_world(ctx, layer);
    let pos = Vec3::from(ctx.v3(layer, tr, "position", [w / 2.0, h / 2.0, -1000.0]));
    let local = rig_rotation(ctx, layer);
    let eye = parent.apply(pos);
    let fwd = parent.apply_vec(local.apply_vec(vec3(0.0, 0.0, 1.0))).normalize();
    let down = parent.apply_vec(local.apply_vec(vec3(0.0, 1.0, 0.0))).normalize();
    (eye, fwd, down)
}

/// Evaluate a camera layer.
pub fn layer_camera(ctx: &EvalCtx, cam: &Layer) -> CameraState {
    let w = ctx.comp.width as f64;
    let (eye, fwd, down) = layer_frame(ctx, cam);
    let opts = cam.props.sub("cameraOptions");
    let zoom = opts.map(|g| ctx.f(cam, g, "zoom", 1000.0)).unwrap_or(1000.0).max(1e-3);
    let dof = opts.filter(|g| ctx.b(cam, g, "dof")).map(|g| Dof {
        focus: ctx.f(cam, g, "focusDistance", zoom),
        aperture: ctx.f(cam, g, "aperture", default_aperture(w)).max(0.0),
        blur_level: ctx.f(cam, g, "blurLevel", 100.0).max(0.0) / 100.0,
        iris: super::bokeh::Iris {
            sides: super::bokeh::Iris::sides_of_shape(ctx.e(cam, g, "irisShape")),
            rotation: ctx.f(cam, g, "irisRotation", 0.0),
            roundness: ctx.f(cam, g, "irisRoundness", 0.0) / 100.0,
            aspect: ctx.f(cam, g, "irisAspectRatio", 1.0).max(0.01),
            fringe: ctx.f(cam, g, "irisDiffractionFringe", 0.0).max(0.0) / 100.0,
        },
        highlight: super::bokeh::Highlight {
            gain: ctx.f(cam, g, "highlightGain", 0.0).clamp(0.0, 100.0) / 100.0,
            threshold: ctx.f(cam, g, "highlightThreshold", 255.0).clamp(0.0, 255.0) / 255.0,
            saturation: ctx.f(cam, g, "highlightSaturation", 0.0).clamp(0.0, 100.0) / 100.0,
        },
    });
    CameraState { view: basis_view(eye, fwd, down), eye, zoom, ortho: false, dof }
}

/// The comp's active camera at the context time (topmost active camera layer, else the default).
pub fn active_camera(ctx: &EvalCtx) -> CameraState {
    match ctx.comp.active_camera(ctx.time) {
        Some(cam) => layer_camera(ctx, cam),
        None => default_camera(ctx.comp.width as f64, ctx.comp.height as f64),
    }
}

// ---------------------------------------------------------------- 3D views

/// The viewer's 3D views (View ▸ Switch 3D View).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum View3D {
    #[default]
    ActiveCamera,
    Front,
    Left,
    Top,
    Back,
    Right,
    Bottom,
    Custom1,
    Custom2,
    Custom3,
    /// The comp's default camera (50 mm, looking at the comp centre), ignoring camera layers;
    /// orbitable like the custom views.
    Default,
}

impl View3D {
    pub const ALL: [View3D; 11] = [
        View3D::ActiveCamera,
        View3D::Default,
        View3D::Front,
        View3D::Left,
        View3D::Top,
        View3D::Back,
        View3D::Right,
        View3D::Bottom,
        View3D::Custom1,
        View3D::Custom2,
        View3D::Custom3,
    ];
    pub fn label(self) -> &'static str {
        match self {
            View3D::ActiveCamera => "Active Camera",
            View3D::Front => "Front",
            View3D::Left => "Left",
            View3D::Top => "Top",
            View3D::Back => "Back",
            View3D::Right => "Right",
            View3D::Bottom => "Bottom",
            View3D::Custom1 => "Custom View 1",
            View3D::Custom2 => "Custom View 2",
            View3D::Custom3 => "Custom View 3",
            View3D::Default => "Default",
        }
    }
    /// Stable id used by commands (`activeCamera`, `front`, `custom1`…).
    pub fn id(self) -> &'static str {
        match self {
            View3D::ActiveCamera => "activeCamera",
            View3D::Front => "front",
            View3D::Left => "left",
            View3D::Top => "top",
            View3D::Back => "back",
            View3D::Right => "right",
            View3D::Bottom => "bottom",
            View3D::Custom1 => "custom1",
            View3D::Custom2 => "custom2",
            View3D::Custom3 => "custom3",
            View3D::Default => "default",
        }
    }
    pub fn from_id(s: &str) -> Option<View3D> {
        let k: String = s.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
        View3D::ALL.into_iter().find(|v| v.id().eq_ignore_ascii_case(&k) || v.label().replace(' ', "").eq_ignore_ascii_case(&k))
    }
    pub fn is_ortho(self) -> bool {
        matches!(self, View3D::Front | View3D::Left | View3D::Top | View3D::Back | View3D::Right | View3D::Bottom)
    }
}

/// A view camera: eye, point of interest, screen-down hint, zoom (perspective) or scale (ortho).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewCam {
    pub eye: [f64; 3],
    pub poi: [f64; 3],
    pub down: [f64; 3],
    pub zoom: f64,
    pub ortho: bool,
}

impl ViewCam {
    pub fn state(&self) -> CameraState {
        let eye = Vec3::from(self.eye);
        let poi = Vec3::from(self.poi);
        CameraState { view: basis_view(eye, poi - eye, Vec3::from(self.down)), eye, zoom: self.zoom, ortho: self.ortho, dof: None }
    }
}

/// Default camera of a view for a `w`×`h` comp.
pub fn default_view_cam(v: View3D, w: f64, h: f64) -> ViewCam {
    let c = vec3(w / 2.0, h / 2.0, 0.0);
    let zoom = effectcraft_geom::default_camera_zoom(w);
    let far = 10_000.0;
    let ortho = |fwd: Vec3, down: Vec3| ViewCam { eye: arr(c - fwd * far), poi: arr(c), down: arr(down), zoom: 0.5, ortho: true };
    let custom = |yaw: f64, pitch: f64| {
        // Orbit the default camera around the comp centre (yaw about y, then pitch).
        let off = Mat4::rotate_y(yaw).apply_vec(Mat4::rotate_x(pitch).apply_vec(vec3(0.0, 0.0, -zoom * 1.6)));
        ViewCam { eye: arr(c + off), poi: arr(c), down: [0.0, 1.0, 0.0], zoom, ortho: false }
    };
    match v {
        View3D::ActiveCamera | View3D::Front => ortho(vec3(0.0, 0.0, 1.0), vec3(0.0, 1.0, 0.0)),
        View3D::Back => ortho(vec3(0.0, 0.0, -1.0), vec3(0.0, 1.0, 0.0)),
        View3D::Left => ortho(vec3(1.0, 0.0, 0.0), vec3(0.0, 1.0, 0.0)),
        View3D::Right => ortho(vec3(-1.0, 0.0, 0.0), vec3(0.0, 1.0, 0.0)),
        View3D::Top => ortho(vec3(0.0, 1.0, 0.0), vec3(0.0, 0.0, -1.0)),
        View3D::Bottom => ortho(vec3(0.0, -1.0, 0.0), vec3(0.0, 0.0, 1.0)),
        // Custom views look down at the comp from above-left, above and above-right.
        View3D::Custom1 => custom(30.0, -20.0),
        View3D::Custom2 => custom(0.0, -35.0),
        View3D::Custom3 => custom(-30.0, -20.0),
        View3D::Default => ViewCam { eye: arr(vec3(w / 2.0, h / 2.0, -zoom)), poi: arr(c), down: [0.0, 1.0, 0.0], zoom, ortho: false },
    }
}

pub fn arr(v: Vec3) -> [f64; 3] {
    [v.x, v.y, v.z]
}

/// Per-comp 3D view state of the viewer (current view + edited view cameras).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Views3D {
    pub current: View3D,
    /// The view before the last switch (View ▸ Switch to Last 3D View).
    #[serde(default)]
    pub last: View3D,
    /// Edited view cameras (views not present use their defaults).
    #[serde(default)]
    pub cams: BTreeMap<View3D, ViewCam>,
    /// The views of the extra panes of a 2- or 4-view layout (pane 1 to 3, i.e. index 0 to 2);
    /// panes not present show their default ([`Views3D::DEFAULT_PANES`]).
    #[serde(default)]
    pub panes: Vec<View3D>,
}

impl Views3D {
    /// Views the extra panes of a multi-view layout start with: Top, Front and Right.
    pub const DEFAULT_PANES: [View3D; 3] = [View3D::Top, View3D::Front, View3D::Right];
    /// The view of extra pane `i` (0 = the first beside the main view).
    pub fn pane(&self, i: usize) -> View3D {
        self.panes.get(i).or_else(|| Self::DEFAULT_PANES.get(i)).copied().unwrap_or_default()
    }
    /// Choose the view of extra pane `i` (ignored past the third).
    pub fn set_pane(&mut self, i: usize, v: View3D) {
        if i >= Self::DEFAULT_PANES.len() {
            return;
        }
        while self.panes.len() <= i {
            self.panes.push(Self::DEFAULT_PANES[self.panes.len()]);
        }
        self.panes[i] = v;
    }
    pub fn cam(&self, v: View3D, w: f64, h: f64) -> ViewCam {
        self.cams.get(&v).copied().unwrap_or_else(|| default_view_cam(v, w, h))
    }
    /// Camera override for rendering: None for the active camera view.
    pub fn override_camera(&self, w: f64, h: f64) -> Option<CameraState> {
        (self.current != View3D::ActiveCamera).then(|| self.cam(self.current, w, h).state())
    }
}

// ---------------------------------------------------------------- camera tools

/// A camera as edited by the tools: position, point of interest, zoom (two-node form).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rig {
    pub eye: Vec3,
    pub poi: Vec3,
    pub zoom: f64,
    pub ortho: bool,
}

impl Rig {
    pub fn from_view(v: &ViewCam) -> Rig {
        Rig { eye: Vec3::from(v.eye), poi: Vec3::from(v.poi), zoom: v.zoom, ortho: v.ortho }
    }
    fn frame(&self) -> (Vec3, Vec3, Vec3) {
        let s =
            CameraState { view: basis_view(self.eye, self.poi - self.eye, vec3(0.0, 1.0, 0.0)), eye: self.eye, zoom: self.zoom, ortho: self.ortho, dof: None };
        (s.forward(), s.right(), s.down())
    }
    /// Orbit the eye around the point of interest: `yaw` about world Y, `pitch` about the
    /// camera's right axis (degrees). Pitch is limited short of straight up/down.
    pub fn orbit(&mut self, yaw: f64, pitch: f64) {
        if self.ortho {
            return;
        }
        let off = self.eye - self.poi;
        let dist = off.length().max(1e-6);
        let cur_pitch = (off.y / dist).clamp(-1.0, 1.0).asin().to_degrees();
        let new_pitch = (cur_pitch + pitch).clamp(-89.0, 89.0);
        let horiz = vec3(off.x, 0.0, off.z);
        let hl = horiz.length();
        let az = if hl > 1e-9 { off.x.atan2(-off.z).to_degrees() } else { 0.0 } + yaw;
        let (sp, cp) = new_pitch.to_radians().sin_cos();
        let (sa, ca) = az.to_radians().sin_cos();
        self.eye = self.poi + vec3(sa * cp, sp, -ca * cp) * dist;
    }
    /// Pan in the image plane by (`dx`, `dy`) comp pixels (as seen at the point of interest).
    pub fn pan(&mut self, dx: f64, dy: f64) {
        let (_, r, d) = self.frame();
        let k = if self.ortho { 1.0 / self.zoom.max(1e-9) } else { (self.poi - self.eye).length() / self.zoom.max(1e-9) };
        let delta = r * (-dx * k) + d * (-dy * k);
        self.eye += delta;
        self.poi += delta;
    }
    /// Dolly along the view direction by `amount` pixels (positive = towards the POI). With
    /// `with_poi` the point of interest travels too (camera layers: Track Z); otherwise the
    /// eye stops short of the POI. Orthographic views scale instead.
    pub fn dolly(&mut self, amount: f64, with_poi: bool) {
        if self.ortho {
            self.zoom = (self.zoom * (amount / 500.0).exp()).clamp(0.01, 100.0);
            return;
        }
        let (f, _, _) = self.frame();
        if with_poi {
            self.eye += f * amount;
            self.poi += f * amount;
        } else {
            let dist = (self.poi - self.eye).length();
            let d = amount.min(dist - 1.0);
            self.eye += f * d;
        }
    }
}
