//! Motion trackers in the property tree, laid out like After Effects':
//!
//! ```text
//! Motion Trackers                      (match `motionTrackers`, first group of the layer)
//!   Tracker 1                          (GroupKind::Tracker: type, target, options)
//!     Track Point 1                    (match `trackPoint`)
//!       Feature Center, Feature Size, Search Offset, Search Size,
//!       Confidence, Attach Point, Attach Point Offset
//! ```
//!
//! Feature Center, Confidence and Attach Point get one keyframe per analysed frame (layer time).
//! The tracking algorithms live in `effectcraft-track`; applying tracks lives in the engine.

use effectcraft_keyframe::Value;
use serde::{Deserialize, Serialize};

use crate::build::Ids;
use crate::props::{GroupKind, ParamUi, PropGroup};
use crate::{Layer, LayerId, Uid};

/// Tracker panel ▸ Track Type.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrackKind {
    Stabilize,
    #[default]
    Transform,
    /// Parallel (affine) corner pin: three points, the fourth corner completes the parallelogram.
    #[serde(alias = "parallel", alias = "parallelCornerPin")]
    Affine,
    /// Perspective corner pin: four points.
    #[serde(alias = "perspectiveCornerPin")]
    Perspective,
    /// Raw: track data only (nothing to apply).
    Raw,
}

impl TrackKind {
    pub const ALL: [TrackKind; 5] = [TrackKind::Stabilize, TrackKind::Transform, TrackKind::Affine, TrackKind::Perspective, TrackKind::Raw];
    pub fn label(self) -> &'static str {
        match self {
            TrackKind::Stabilize => "Stabilize",
            TrackKind::Transform => "Transform",
            TrackKind::Affine => "Parallel Corner Pin",
            TrackKind::Perspective => "Perspective Corner Pin",
            TrackKind::Raw => "Raw",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            TrackKind::Stabilize => "stabilize",
            TrackKind::Transform => "transform",
            TrackKind::Affine => "affine",
            TrackKind::Perspective => "perspective",
            TrackKind::Raw => "raw",
        }
    }
    pub fn from_name(s: &str) -> Option<TrackKind> {
        let n = s.to_ascii_lowercase().replace([' ', '_', '-'], "");
        match n.as_str() {
            "parallel" | "parallelcornerpin" | "affinecornerpin" => return Some(TrackKind::Affine),
            "perspectivecornerpin" => return Some(TrackKind::Perspective),
            _ => {}
        }
        TrackKind::ALL.into_iter().find(|k| k.id() == n || k.label().to_ascii_lowercase().replace(' ', "") == n)
    }
    pub fn is_corner_pin(self) -> bool {
        matches!(self, TrackKind::Affine | TrackKind::Perspective)
    }
    /// Number of track points for this type (Transform/Stabilize: 2 with rotation or scale).
    pub fn point_count(self, rotation_or_scale: bool) -> usize {
        match self {
            TrackKind::Affine => 3,
            TrackKind::Perspective => 4,
            _ if rotation_or_scale => 2,
            _ => 1,
        }
    }
}

/// Motion Tracker Options ▸ Channel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrackChannel {
    Rgb,
    #[default]
    Luminance,
    Saturation,
}

/// Motion Tracker Options ▸ "If Confidence is Below … %".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LowConfidence {
    Continue,
    Stop,
    Extrapolate,
    #[default]
    Adapt,
}

/// Motion Tracker Options.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TrackerOptions {
    pub channel: TrackChannel,
    /// Process Before Match ▸ Blur radius in pixels (0 = off).
    pub blur: f64,
    /// Process Before Match ▸ Enhance.
    pub enhance: bool,
    pub subpixel: bool,
    /// Adapt Feature on Every Frame.
    pub adapt_every_frame: bool,
    /// Confidence threshold, percent.
    pub threshold: f64,
    pub action: LowConfidence,
    /// Follow each feature's rotation and scale while matching (Visualab Effects extension).
    pub track_shape: bool,
}

impl Default for TrackerOptions {
    fn default() -> Self {
        TrackerOptions {
            channel: TrackChannel::Luminance,
            blur: 0.0,
            enhance: false,
            subpixel: true,
            adapt_every_frame: false,
            threshold: 80.0,
            action: LowConfidence::Adapt,
            track_shape: true,
        }
    }
}

/// A tracker's non-animatable settings (Tracker panel).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TrackerSettings {
    pub kind: TrackKind,
    pub position: bool,
    pub rotation: bool,
    pub scale: bool,
    /// Motion Target (None: no target; Stabilize always applies to the tracked layer).
    pub target: Option<LayerId>,
    pub options: TrackerOptions,
}

impl TrackerSettings {
    pub fn new(kind: TrackKind) -> TrackerSettings {
        TrackerSettings { kind, position: true, ..Default::default() }
    }
    pub fn point_count(&self) -> usize {
        self.kind.point_count(self.rotation || self.scale)
    }
}

/// Initial regions of a track point (layer pixels).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointInit {
    pub center: [f64; 2],
    pub feature_size: [f64; 2],
    pub search_offset: [f64; 2],
    pub search_size: [f64; 2],
    pub attach_offset: [f64; 2],
}

pub const MOTION_TRACKERS: &str = "motionTrackers";
pub const TRACK_POINT: &str = "trackPoint";

/// The (empty) Motion Trackers group.
pub fn motion_trackers(ids: &mut Ids) -> PropGroup {
    ids.group(MOTION_TRACKERS, "Motion Trackers")
}

/// One Track Point group.
pub fn track_point(ids: &mut Ids, name: &str, p: &PointInit) -> PropGroup {
    let pt = |ids: &mut Ids, m: &str, n: &str, v: [f64; 2]| ids.prop(m, n, Value::Vec2(v)).with_ui(ParamUi::Point).spatial();
    let vec = |ids: &mut Ids, m: &str, n: &str, v: [f64; 2]| ids.prop(m, n, Value::Vec2(v)).with_ui(ParamUi::Point);
    let attach = [p.center[0] + p.attach_offset[0], p.center[1] + p.attach_offset[1]];
    let fc = pt(ids, "featureCenter", "Feature Center", p.center);
    let fs = vec(ids, "featureSize", "Feature Size", p.feature_size);
    let so = vec(ids, "searchOffset", "Search Offset", p.search_offset);
    let ss = vec(ids, "searchSize", "Search Size", p.search_size);
    let cf = ids.prop("confidence", "Confidence", Value::Scalar(0.0)).with_ui(ParamUi::Percent);
    let ap = pt(ids, "attachPoint", "Attach Point", attach);
    let ao = vec(ids, "attachPointOffset", "Attach Point Offset", p.attach_offset);
    let mut g = ids.group(TRACK_POINT, name).with(fc).with(fs).with(so).with(ss).with(cf).with(ap).with(ao);
    g.kind = GroupKind::Indexed;
    g
}

/// A Tracker group with its points.
pub fn tracker(ids: &mut Ids, name: &str, settings: TrackerSettings, points: &[PointInit]) -> PropGroup {
    let mut g = ids.group("tracker", name);
    g.kind = GroupKind::Tracker { settings: Box::new(settings) };
    for (i, p) in points.iter().enumerate() {
        let tp = track_point(ids, &format!("Track Point {}", i + 1), p);
        g.children.push(tp.into());
    }
    g
}

/// Default track point layout for a tracker type, centred on a layer of size `size`.
pub fn default_points(kind: TrackKind, n: usize, size: [f64; 2]) -> Vec<PointInit> {
    let m = size[0].min(size[1]).max(1.0);
    let f = (m * 0.04).clamp(16.0, 80.0).round();
    let s = (f * 2.0).round();
    let c = [size[0] / 2.0, size[1] / 2.0];
    let mk = |p: [f64; 2]| PointInit { center: p, feature_size: [f, f], search_offset: [0.0; 2], search_size: [s, s], attach_offset: [0.0; 2] };
    let (dx, dy) = ((size[0] * 0.18).max(2.0 * s), (size[1] * 0.18).max(2.0 * s));
    match (kind, n) {
        (TrackKind::Perspective, _) | (_, 4) => {
            vec![mk([c[0] - dx, c[1] - dy]), mk([c[0] + dx, c[1] - dy]), mk([c[0] - dx, c[1] + dy]), mk([c[0] + dx, c[1] + dy])]
        }
        (TrackKind::Affine, _) | (_, 3) => vec![mk([c[0] - dx, c[1] - dy]), mk([c[0] + dx, c[1] - dy]), mk([c[0] - dx, c[1] + dy])],
        (_, 2) => vec![mk([c[0] - dx, c[1]]), mk([c[0] + dx, c[1]])],
        _ => vec![mk(c)],
    }
}

impl Layer {
    /// The Motion Trackers group, if the layer has one.
    pub fn motion_trackers(&self) -> Option<&PropGroup> {
        self.props.sub(MOTION_TRACKERS)
    }
    /// Trackers of the layer (groups with [`GroupKind::Tracker`]).
    pub fn trackers(&self) -> impl Iterator<Item = (&PropGroup, &TrackerSettings)> {
        self.motion_trackers().into_iter().flat_map(|g| g.groups()).filter_map(|g| match &g.kind {
            GroupKind::Tracker { settings } => Some((g, settings.as_ref())),
            _ => None,
        })
    }
    pub fn tracker(&self, uid: Uid) -> Option<(&PropGroup, &TrackerSettings)> {
        self.trackers().find(|(g, _)| g.uid == uid)
    }
    /// The Motion Trackers group, created (first in the layer's tree, like AE) when missing.
    pub fn motion_trackers_mut(&mut self, ids: &mut Ids) -> Option<&mut PropGroup> {
        if self.props.sub(MOTION_TRACKERS).is_none() {
            self.props.children.insert(0, motion_trackers(ids).into());
        }
        self.props.sub_mut(MOTION_TRACKERS)
    }
}

impl PropGroup {
    /// Tracker settings of a tracker group.
    pub fn tracker_settings(&self) -> Option<&TrackerSettings> {
        match &self.kind {
            GroupKind::Tracker { settings } => Some(settings),
            _ => None,
        }
    }
    pub fn tracker_settings_mut(&mut self) -> Option<&mut TrackerSettings> {
        match &mut self.kind {
            GroupKind::Tracker { settings } => Some(settings),
            _ => None,
        }
    }
    /// Track Point groups of a tracker group, in order.
    pub fn track_points(&self) -> impl Iterator<Item = &PropGroup> {
        self.groups().filter(|g| g.match_id == TRACK_POINT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracker_tree_and_serde() {
        let mut n = 1;
        let mut ids = Ids(&mut n);
        let pts = default_points(TrackKind::Perspective, 4, [1920.0, 1080.0]);
        let g = tracker(&mut ids, "Tracker 1", TrackerSettings::new(TrackKind::Perspective), &pts);
        assert_eq!(g.track_points().count(), 4);
        let tp = g.track_points().next().unwrap();
        let names: Vec<&str> = tp.props().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Feature Center", "Feature Size", "Search Offset", "Search Size", "Confidence", "Attach Point", "Attach Point Offset"]);
        let json = serde_json::to_string(&g).unwrap();
        let back: PropGroup = serde_json::from_str(&json).unwrap();
        assert_eq!(back, g);
        assert_eq!(back.tracker_settings().unwrap().kind, TrackKind::Perspective);
        assert_eq!(TrackKind::from_name("Parallel Corner Pin"), Some(TrackKind::Affine));
        assert_eq!(TrackKind::from_name("perspective"), Some(TrackKind::Perspective));
        assert_eq!(TrackerSettings { rotation: true, ..TrackerSettings::new(TrackKind::Transform) }.point_count(), 2);
    }
}
