//! Cameras and lights.
//!
//! Both are layers without a source (`ldta` byte 40 is 0 for a camera and `0xffff_ffff` for a light)
//! whose type is 2 (camera) or 1 (light). Their `ldta` carries the auto-orient bit (byte 38, `0x40`)
//! and, for a light, the light type (see [`LayerHeader::light_type`](crate::LayerHeader)). The
//! properties sit in the same Transform group as on other layers but follow other conventions:
//!
//! * **Position** is stored as given once the user has set it. While it is at its computed default
//!   the record is an "auto state" (`tdb4` bytes 76 and 77 are 1 although there is no expression)
//!   that holds `[0, 0, -zoom]`. After Effects reports and renders the stored value, also for a
//!   two-node camera (256 read back `[0, 0, -2666.67]` in session S16; 537 renders from there). An
//!   earlier batch-3 note had a two-node camera report the composition centre; that was not
//!   reproduced.
//! * **Anchor Point** of a camera is its point of interest, in pixels (not a fraction).
//! * A light's **Intensity** (`ADBE Light Options Group/ADBE Light Intensity`, percent) is stored
//!   only when it is not the default 100.

use crate::typed::{Layer, PropNode};
use crate::values::{Tracked, ValueSource, read, read_opt};

/// The Position of a camera or light layer as stored.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraPosition {
    /// `[x, y, z]` in pixels as stored: the absolute position once set by the user, `[0, 0, -zoom]`
    /// in the auto state (camera) or `[0, 0, -1333.33]` (default light).
    pub stored: [f64; 3],
    /// True while the position is derived (`tdb4` bytes 76 and 77 are 1 without an expression).
    pub auto_state: bool,
}

impl CameraPosition {
    /// The position as the batch-3 notes had After Effects report it: in the auto state of a
    /// two-node camera (`two_node`) x and y are the composition centre; otherwise the stored
    /// value. **Contradicted by 256 and 537 (session S16): After Effects reports and renders the
    /// stored value**; use `stored` for what After Effects shows.
    pub fn reported(&self, comp_width: u32, comp_height: u32, two_node: bool) -> [f64; 3] {
        if self.auto_state && two_node { [f64::from(comp_width) / 2.0, f64::from(comp_height) / 2.0, self.stored[2]] } else { self.stored }
    }
}

impl<'f, 'a> Layer<'f, 'a> {
    /// The Position record of a camera or light layer.
    pub fn camera_position(&self) -> Option<CameraPosition> {
        let Some(PropNode::Group(transform)) = self.properties()?.get("ADBE Transform Group") else { return None };
        let Some(PropNode::Property(position)) = transform.get("ADBE Position") else { return None };
        let v = position.static_value()?;
        let stored = *v.as_slice().first_chunk::<3>()?;
        let h = position.header()?;
        let derived = h.get(76) == Some(&1) && h.get(77) == Some(&1) && position.expression().is_none();
        Some(CameraPosition { stored, auto_state: derived })
    }

    /// The point of interest of a camera in pixels (its Anchor Point record); `None` when absent.
    pub fn camera_point_of_interest(&self) -> Option<[f64; 3]> {
        let Some(PropNode::Group(transform)) = self.properties()?.get("ADBE Transform Group") else { return None };
        let tracked = read(&transform, "ADBE Anchor Point", [0.0; 3], 1.0);
        matches!(tracked.source, ValueSource::Stored | ValueSource::Keyframed).then_some(tracked.value)
    }

    /// A light's intensity in percent; the default 100 when the property is absent.
    pub fn light_intensity(&self) -> Option<Tracked<f64>> {
        let root = self.properties()?;
        let Some(PropNode::Group(options)) = root.get("ADBE Light Options Group") else { return None };
        let t = read(&options, "ADBE Light Intensity", [100.0], 1.0);
        Some(Tracked { value: t.value[0], source: t.source })
    }
}

/// The camera zoom After Effects uses when `ADBE Camera Zoom` is absent: 2666.67 px for the 50 mm
/// default camera in the 1920 px wide corpus composition. Whether it follows other composition
/// widths was not sampled.
pub const DEFAULT_CAMERA_ZOOM: f64 = 2_666.666_666_666_666_5;

impl<'f, 'a> Layer<'f, 'a> {
    fn camera_options(&self) -> Option<crate::typed::PropGroup<'f, 'a>> {
        match self.properties()?.get("ADBE Camera Options Group") {
            Some(PropNode::Group(g)) => Some(g),
            _ => None,
        }
    }

    fn light_options(&self) -> Option<crate::typed::PropGroup<'f, 'a>> {
        match self.properties()?.get("ADBE Light Options Group") {
            Some(PropNode::Group(g)) => Some(g),
            _ => None,
        }
    }

    /// A camera's zoom in pixels (`ADBE Camera Options Group/ADBE Camera Zoom`); absent = the
    /// default, [`DEFAULT_CAMERA_ZOOM`] (341: 2000 is stored). The auto Position keeps `-2666.67` and
    /// is not recomputed from the zoom.
    pub fn camera_zoom(&self) -> Option<Tracked<f64>> {
        let t = read(&self.camera_options()?, "ADBE Camera Zoom", [DEFAULT_CAMERA_ZOOM], 1.0);
        Some(Tracked { value: t.value[0], source: t.source })
    }

    /// The depth of field switch (`ADBE Camera Depth of Field`, a checkbox; off by default, 342).
    pub fn camera_depth_of_field(&self) -> Option<Tracked<bool>> {
        let t = read(&self.camera_options()?, "ADBE Camera Depth of Field", [0.0], 1.0);
        Some(Tracked { value: t.value[0] != 0.0, source: t.source })
    }

    /// A camera's focus distance in pixels (`ADBE Camera Focus Distance`); `None` while absent (its
    /// default is not known).
    pub fn camera_focus_distance(&self) -> Option<Tracked<f64>> {
        let t = read_opt::<1>(&self.camera_options()?, "ADBE Camera Focus Distance", 1.0)?;
        Some(Tracked { value: t.value[0], source: t.source })
    }

    /// A camera's aperture (`ADBE Camera Aperture`); `None` while absent (its default is not known).
    pub fn camera_aperture(&self) -> Option<Tracked<f64>> {
        let t = read_opt::<1>(&self.camera_options()?, "ADBE Camera Aperture", 1.0)?;
        Some(Tracked { value: t.value[0], source: t.source })
    }

    /// A light's colour as `[A, R, G, B]` fractions (`ADBE Light Options Group/ADBE Light Color`,
    /// stored as doubles 0..255); white when absent (343 stores red).
    pub fn light_color(&self) -> Option<Tracked<[f64; 4]>> {
        Some(read(&self.light_options()?, "ADBE Light Color", [1.0; 4], 1.0 / 255.0))
    }

    /// A spot light's cone angle in degrees (`ADBE Light Cone Angle`); 90 when absent (344 stores 60).
    /// A point light stores 90 with `tdsb` 3.
    pub fn light_cone_angle(&self) -> Option<Tracked<f64>> {
        let t = read(&self.light_options()?, "ADBE Light Cone Angle", [90.0], 1.0);
        Some(Tracked { value: t.value[0], source: t.source })
    }

    /// A spot light's cone feather in percent (`ADBE Light Cone Feather 2`); 50 when absent (345
    /// stores 30).
    pub fn light_cone_feather(&self) -> Option<Tracked<f64>> {
        let t = read(&self.light_options()?, "ADBE Light Cone Feather 2", [50.0], 1.0);
        Some(Tracked { value: t.value[0], source: t.source })
    }
}
