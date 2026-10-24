//! Keyframe records, property flags and expressions.
//!
//! Keyframe times are **layer time**, in units of the property's own time scale
//! ([`Property::time_scale`], equal to the composition's timescale in every sample).
//!
//! Every record starts with an 8-byte header: `i32` time, `u8` at byte 4 = the interpolation of the
//! **incoming** side, `u8` at byte 5 = the **outgoing** side (1 linear, 2 bezier, 3 hold), and a
//! `u16` of flags (label, roving). The rest depends on the property header (`tdb4`), which selects
//! one of two templates for `n` value components:
//!
//! * **plain** (`tdb4` byte 7 = 0; Opacity, Rotation, Scale, 2-D properties, sliders): the value
//!   (`n` x `f64`) at byte 8, then four `f64` per component of ease, type-major (`[in speed x n]
//!   [in influence x n][out speed x n][out influence x n]`). Size `8 + 40 n`.
//! * **spatial** (`tdb4` byte 7 != 0; Position with `n` = 3, colours with `n` = 4): a `u32` at 8, the
//!   arc length (`f64`) at 16, the four ease slots at 24 to 55, the value (`n` x `f64`) at 56, then
//!   the spatial in tangent and out tangent (`n` x `f64` each). Size `56 + 24 n`.

use crate::bytes::{be_f64, be_u16, be_u32, text};
use crate::typed::{Keyframe, Property};

/// A keyframe interpolation type (a byte of the record header).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interpolation {
    /// `1`
    Linear,
    /// `2`
    Bezier,
    /// `3`
    Hold,
    /// Any other value.
    Other(u8),
}

impl Interpolation {
    fn from_byte(b: u8) -> Self {
        match b {
            1 => Self::Linear,
            2 => Self::Bezier,
            3 => Self::Hold,
            other => Self::Other(other),
        }
    }
}

/// What a keyframe record is, judged by the property header (when the list belongs to a property)
/// and the record size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyframeKind {
    /// The plain template: `8 + 40 n` bytes, 48 for one component (Opacity, Rotation, Slider), 88 for
    /// two (Mask Feather, Rectangle Size), 128 for three (Scale).
    Plain { components: usize },
    /// The spatial template: `56 + 24 n` bytes, 128 for Position (3 components), 152 for a colour (4).
    Spatial { components: usize },
    /// 64 bytes: a mask path key; the path itself is in the matching `shap` chunk. The ease sits at
    /// byte 24 as in the spatial template (599, 700: speed 0, influences 0.75 / 0.20 / 0.50); the
    /// `f64` at byte 16 is 1 or 2 on a first key and 0 on a last one (093, 599, 700; meaning not
    /// decoded).
    MaskPath,
    /// 16 bytes: a marker (time and two hold bytes).
    Marker,
    /// Any other size.
    Other(usize),
}

/// The ease of a key: speed and influence of the incoming and outgoing side. Speed is in stored
/// value units per second (for Opacity a fraction, so 100 times smaller than the percent shown in
/// the UI; for Position pixels per second); influence is a fraction (20 % is `0.2`). After Effects
/// reports the first key's incoming speed and the last key's outgoing speed as 0, but the file
/// keeps the requested values, so these two slots are not meaningful.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeyEase {
    pub in_speed: f64,
    pub in_influence: f64,
    pub out_speed: f64,
    pub out_influence: f64,
}

/// The fields of a spatial keyframe record.
#[derive(Debug, Clone, PartialEq)]
pub struct SpatialKey {
    /// The `u32` at byte 8: `1` normally, `0` after the spatial interpolation was changed to
    /// non-auto-bezier / non-continuous (the two switches were changed together).
    pub auto_bezier_flag: u32,
    /// The `f64` at byte 16: the arc length of the segment to the next key, updated lazily by
    /// After Effects (0 right after creation). Safe to ignore.
    pub segment_length: f64,
    /// The four `f64` at bytes 24, 32, 40 and 48 as stored: in speed, in influence, out speed, out
    /// influence (see [`KeyEase`]).
    pub ease: [f64; 4],
    /// The value (x, y, z in pixels for Position; ARGB 0..255 for a colour), from byte 56.
    pub value: Vec<f64>,
    /// The spatial in tangent as an offset from the value.
    pub in_tangent: Vec<f64>,
    /// The spatial out tangent as an offset from the value.
    pub out_tangent: Vec<f64>,
}

fn doubles(b: &[u8], at: usize, n: usize) -> Option<Vec<f64>> {
    (0..n).map(|i| be_f64(b, at.checked_add(i.checked_mul(8)?)?)).collect()
}

fn ease_at(b: &[u8], at: usize) -> Option<KeyEase> {
    Some(KeyEase {
        in_speed: be_f64(b, at)?,
        in_influence: be_f64(b, at.checked_add(8)?)?,
        out_speed: be_f64(b, at.checked_add(16)?)?,
        out_influence: be_f64(b, at.checked_add(24)?)?,
    })
}

impl<'f> Keyframe<'f> {
    /// The record kind. For a list that belongs to a property, the header's component count and
    /// template flag decide; without a property, 48 bytes is read as plain with one component and
    /// 128 bytes as spatial with three.
    pub fn kind(&self) -> KeyframeKind {
        let len = self.record.len();
        if let Some(layout) = self.layout {
            let n = usize::from(layout.components);
            if n > 0 {
                if layout.spatial && Some(len) == n.checked_mul(24).and_then(|v| v.checked_add(56)) {
                    return KeyframeKind::Spatial { components: n };
                }
                if !layout.spatial && Some(len) == n.checked_mul(40).and_then(|v| v.checked_add(8)) {
                    return KeyframeKind::Plain { components: n };
                }
            }
        }
        match (len, self.layout) {
            (64, _) => KeyframeKind::MaskPath,
            (16, _) => KeyframeKind::Marker,
            (48, None) => KeyframeKind::Plain { components: 1 },
            (128, None) => KeyframeKind::Spatial { components: 3 },
            (other, _) => KeyframeKind::Other(other),
        }
    }

    /// The time in seconds given the property's time scale (units per second).
    pub fn seconds(&self, time_scale: u32) -> Option<f64> {
        if time_scale == 0 {
            return None;
        }
        Some(f64::from(self.time_units()?) / f64::from(time_scale))
    }

    /// The interpolation of the incoming side: the byte at offset 4.
    pub fn interpolation_in(&self) -> Option<Interpolation> {
        self.record.get(4).copied().map(Interpolation::from_byte)
    }

    /// The interpolation of the outgoing side: the byte at offset 5.
    pub fn interpolation_out(&self) -> Option<Interpolation> {
        self.record.get(5).copied().map(Interpolation::from_byte)
    }

    /// The flags word at byte 6.
    pub fn flags(&self) -> Option<u16> {
        be_u16(self.record, 6)
    }

    /// The keyframe label: bits 6 to 9 of the flags (label 9 gives `0x0247` on a spatial key).
    pub fn label(&self) -> Option<u8> {
        u8::try_from((self.flags()? >> 6) & 0x0f).ok()
    }

    /// True when the key is roving: bit `0x20` of the flags.
    pub fn is_roving(&self) -> Option<bool> {
        Some(self.flags()? & 0x0020 != 0)
    }

    /// The number of value components of a plain or spatial record.
    pub fn components(&self) -> Option<usize> {
        match self.kind() {
            KeyframeKind::Plain { components } | KeyframeKind::Spatial { components } => Some(components),
            KeyframeKind::MaskPath | KeyframeKind::Marker | KeyframeKind::Other(_) => None,
        }
    }

    /// The stored value of a plain (from byte 8) or spatial (from byte 56) record, in the units of
    /// the property's static value (fractions for Opacity and Scale, ARGB 0..255 for colours).
    pub fn value(&self) -> Option<Vec<f64>> {
        match self.kind() {
            KeyframeKind::Plain { components } => doubles(self.record, 8, components),
            KeyframeKind::Spatial { components } => doubles(self.record, 56, components),
            KeyframeKind::MaskPath | KeyframeKind::Marker | KeyframeKind::Other(_) => None,
        }
    }

    /// The value of a record with one component (Opacity, Rotation, a slider).
    pub fn scalar_value(&self) -> Option<f64> {
        match self.kind() {
            KeyframeKind::Plain { components: 1 } => be_f64(self.record, 8),
            _ => None,
        }
    }

    /// The value of a 3-component spatial keyframe (Position), x, y and z in pixels.
    pub fn spatial_value(&self) -> Option<[f64; 3]> {
        let v = self.value()?;
        if !matches!(self.kind(), KeyframeKind::Spatial { components: 3 }) {
            return None;
        }
        Some([*v.first()?, *v.get(1)?, *v.get(2)?])
    }

    /// The ease of the key: the four slots at byte 16 of a one-component plain record or at byte 24
    /// of a spatial record or a mask path record. For linear and hold keys all four are 0 (a bezier key stores what
    /// `setTemporalEase` was given). Plain records with several components keep four slots per
    /// component, see [`Keyframe::ease_component`] and [`Keyframe::plain_ease_slots`].
    pub fn ease(&self) -> Option<KeyEase> {
        match self.kind() {
            KeyframeKind::Plain { components: 1 } => ease_at(self.record, 16),
            KeyframeKind::Spatial { .. } | KeyframeKind::MaskPath => ease_at(self.record, 24),
            _ => None,
        }
    }

    /// The `4 n` ease `f64` of a plain record as stored (from byte `8 + 8 n`). With one component
    /// they are [`Keyframe::ease`]; with several they are type-major, see
    /// [`Keyframe::ease_component`].
    pub fn plain_ease_slots(&self) -> Option<Vec<f64>> {
        match self.kind() {
            KeyframeKind::Plain { components } => doubles(self.record, components.checked_mul(8)?.checked_add(8)?, components.checked_mul(4)?),
            _ => None,
        }
    }

    /// The ease of one value component. A plain record with `n` components stores the `4 n` ease
    /// doubles **type-major**: `[in speed x n][in influence x n][out speed x n][out influence x n]`
    /// starting at byte `8 + 8 n`, and component `i` uses slot `i` of each block (verified for the
    /// block layout with Scale `n = 3` and Mask Feather `n = 2` keys whose ease was the same in
    /// every dimension; that different values sit in the slots of their dimension is inferred).
    /// A spatial record (Position, colours) has one ease entry for all components, so only
    /// `component == 0` returns it. For one component this is [`Keyframe::ease`].
    pub fn ease_component(&self, component: usize) -> Option<KeyEase> {
        match self.kind() {
            KeyframeKind::Plain { components } if component < components => {
                let block = components.checked_mul(8)?;
                let first = block.checked_add(8)?;
                let at = |k: usize| first.checked_add(block.checked_mul(k)?)?.checked_add(component.checked_mul(8)?);
                Some(KeyEase {
                    in_speed: be_f64(self.record, at(0)?)?,
                    in_influence: be_f64(self.record, at(1)?)?,
                    out_speed: be_f64(self.record, at(2)?)?,
                    out_influence: be_f64(self.record, at(3)?)?,
                })
            }
            KeyframeKind::Spatial { .. } if component == 0 => ease_at(self.record, 24),
            _ => None,
        }
    }

    /// The ease of every value component of a plain record, see [`Keyframe::ease_component`]
    /// (one entry for a spatial record).
    pub fn ease_components(&self) -> Option<Vec<KeyEase>> {
        let n = match self.kind() {
            KeyframeKind::Plain { components } => components,
            KeyframeKind::Spatial { .. } => 1,
            KeyframeKind::MaskPath | KeyframeKind::Marker | KeyframeKind::Other(_) => return None,
        };
        (0..n).map(|i| self.ease_component(i)).collect()
    }

    /// The Euclidean ARGB distance to the next key that After Effects writes at byte 16 of a colour
    /// (4-component spatial) record whose interpolation is not linear: 255 sqrt(2) = 360.6245 for
    /// red to blue; 0 on the last key and on linear keys. `None` for other records.
    pub fn color_distance(&self) -> Option<f64> {
        match self.kind() {
            KeyframeKind::Spatial { components: 4 } => be_f64(self.record, 16),
            _ => None,
        }
    }

    /// The decoded spatial record (Position: 128 bytes, colour: 152 bytes).
    pub fn spatial(&self) -> Option<SpatialKey> {
        let KeyframeKind::Spatial { components: n } = self.kind() else { return None };
        let r = self.record;
        let span = n.checked_mul(8)?;
        let ease = ease_at(r, 24)?;
        Some(SpatialKey {
            auto_bezier_flag: be_u32(r, 8)?,
            segment_length: be_f64(r, 16)?,
            ease: [ease.in_speed, ease.in_influence, ease.out_speed, ease.out_influence],
            value: doubles(r, 56, n)?,
            in_tangent: doubles(r, 56usize.checked_add(span)?, n)?,
            out_tangent: doubles(r, 56usize.checked_add(span.checked_mul(2)?)?, n)?,
        })
    }
}

/// The bytes of a property header that mark an expression: bytes 76, 77 and 120 of `tdb4` are all 1
/// when the property has an expression (76 and 77 are also 1 on a camera's auto-computed Position,
/// which has none). Byte 119 is 1 when the expression is disabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpressionFlags {
    pub byte_76: u8,
    pub byte_77: u8,
    /// 1 when the expression is disabled (`expression.disable_all`); the text stays in the file.
    pub byte_119: u8,
    pub byte_120: u8,
}

impl ExpressionFlags {
    /// True when bytes 76, 77 and 120 are all 1.
    pub fn all_set(&self) -> bool {
        self.byte_76 == 1 && self.byte_77 == 1 && self.byte_120 == 1
    }

    /// True when byte 119 is 1.
    pub fn disabled(&self) -> bool {
        self.byte_119 == 1
    }
}

impl<'f, 'a> Property<'f, 'a> {
    /// The time scale (units per second) of this property's keyframe times: the `u32` at byte 12
    /// of `tdb4` (25600 at 25 fps, 30720 at 30 fps, 23976 at 29.97 and 23.976 fps).
    pub fn time_scale(&self) -> Option<u32> {
        be_u32(self.header()?, 12)
    }

    /// The property's `tdsb` flags: `1` for a property that was written (non-default), `3` for
    /// the default state.
    pub fn flags(&self) -> Option<u32> {
        be_u32(self.chunk.child(b"tdsb")?.bytes()?, 0)
    }

    /// True when `tdb4` byte 5 has bit 0 set: the property is static, it has no keyframes.
    pub fn is_static_flag_set(&self) -> Option<bool> {
        Some(self.header()?.get(5)? & 1 != 0)
    }

    /// The expression source: the raw UTF-8 text of the `Utf8` child that follows the value in the
    /// property (no length prefix, no engine tag; the engine is project-wide).
    pub fn expression(&self) -> Option<String> {
        self.chunk.child(b"Utf8").and_then(|c| c.bytes()).map(text)
    }

    /// The expression flag bytes of `tdb4`.
    pub fn expression_flags(&self) -> Option<ExpressionFlags> {
        let h = self.header()?;
        Some(ExpressionFlags { byte_76: *h.get(76)?, byte_77: *h.get(77)?, byte_119: *h.get(119)?, byte_120: *h.get(120)? })
    }

    /// Whether the expression is enabled: `None` when the property has no expression (byte 120 is
    /// not 1 or there is no `Utf8` text), else `Some(false)` when byte 119 marks it disabled. Byte
    /// 119 also changes by itself on built-in view layers after a save, so it only counts together
    /// with an expression.
    pub fn expression_enabled(&self) -> Option<bool> {
        let flags = self.expression_flags()?;
        if flags.byte_120 != 1 || self.expression().is_none() {
            return None;
        }
        Some(!flags.disabled())
    }

    /// The value of the first keyframe (plain or spatial record), for reading an animated property
    /// at its first key.
    pub fn first_key_value(&self) -> Option<Vec<f64>> {
        self.keys()?.keyframes().next()?.value()
    }
}
