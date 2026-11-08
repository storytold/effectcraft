//! Cameras and lights.
//!
//! Both are layers without a source (`ldta` byte 40 is 0 for a camera and `0xffff_ffff` for a light)
//! whose type is 2 (camera) or 1 (light). Their `ldta` carries the auto-orient bit (byte 38, `0x40`)
//! and, for a light, the light type (see [`LayerHeader::light_type`](crate::container::LayerHeader)). The
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

use crate::container::typed::{Layer, PropNode};

/// The Position of a camera or light layer as stored.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraPosition {
    /// `[x, y, z]` in pixels as stored: the absolute position once set by the user, `[0, 0, -zoom]`
    /// in the auto state (camera) or `[0, 0, -1333.33]` (default light).
    pub stored: [f64; 3],
    /// True while the position is derived (`tdb4` bytes 76 and 77 are 1 without an expression).
    pub auto_state: bool,
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
}

/// The camera zoom After Effects uses when `ADBE Camera Zoom` is absent: 2666.67 px for the 50 mm
/// default camera in the 1920 px wide corpus composition. Whether it follows other composition
/// widths was not sampled.
pub const DEFAULT_CAMERA_ZOOM: f64 = 2_666.666_666_666_666_5;
