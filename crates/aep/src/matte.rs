//! Track mattes: the matte mode and the matte layer, two fields of the layer header `ldta`.
//!
//! Cases 402-408 (a 960x540 grey solid `M` above the red solid `L`): setting a track matte on a
//! layer changes exactly two fields of that layer's `ldta` (and the video switch of the matte
//! layer, which After Effects turns off by itself):
//!
//! * byte 107 (`u8`; bytes 104..106 are 0 in every file of the corpus): the matte mode, 1 alpha
//!   (403), 2 alpha inverted (404), 3 luma (405), 4 luma inverted (406), 0 no matte (every other
//!   case);
//! * the `u32` at byte 160: the id of the matte layer (`ldta` byte 0 of that layer), `0x1d` = `M`
//!   in 403-406 and 408, `0x1b` = `L` in 407, where the matte layer is BELOW the layer it mattes.
//!   0 in every file without a matte.
//!
//! A matte mode with a matte layer id of 0 (or an `ldta` too short to hold the id) has no case in
//! the corpus: After Effects 2026 writes the id. Before After Effects 23.0 a track matte always
//! used the layer directly above (the documented behaviour of those versions), so such a matte is
//! returned with [`TrackMatte::layer`] `None` and the caller decides.

use crate::Layer;
use crate::bytes::be_u32;

/// The matte mode (`ldta` byte 107).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatteMode {
    /// `1` (403)
    Alpha,
    /// `2` (404)
    AlphaInverted,
    /// `3` (405)
    Luma,
    /// `4` (406)
    LumaInverted,
    /// Any other non-zero value (not sampled).
    Other(u8),
}

impl MatteMode {
    /// The mode for a stored code; `None` for 0 (no matte).
    pub fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            0 => return None,
            1 => Self::Alpha,
            2 => Self::AlphaInverted,
            3 => Self::Luma,
            4 => Self::LumaInverted,
            other => Self::Other(other),
        })
    }
}

/// The track matte of a layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackMatte {
    pub mode: MatteMode,
    /// The id of the matte layer (`u32` at `ldta` byte 160); `None` when it is 0 or the header is
    /// too short to hold it (the older "layer above" form, see the module documentation).
    pub layer: Option<u32>,
}

impl<'f, 'a> Layer<'f, 'a> {
    /// The track matte of the layer, `None` when it has none (or the header is unreadable).
    pub fn track_matte(&self) -> Option<TrackMatte> {
        let ldta = self.chunk().child(b"ldta")?.bytes()?;
        let mode = MatteMode::from_code(*ldta.get(107)?)?;
        let layer = be_u32(ldta, 160).filter(|id| *id != 0);
        Some(TrackMatte { mode, layer })
    }
}
