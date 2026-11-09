//! Chunks the exporter builds in code instead of copying them from projects After Effects saved.
//!
//! Every byte written here is a decoded field (`docs/aep-format.md`), a named constant with its
//! evidence, or zero where After Effects was shown to accept zero (the read-backs listed in
//! `docs/aep-export.md`, "Built in code").

use crate::{Body, Chunk};

/// `tdmn`: a match name padded to 40 bytes.
pub(crate) fn tdmn(name: &str) -> Chunk<'static> {
    let mut b = vec![0u8; 40];
    for (slot, c) in b.iter_mut().zip(name.bytes().take(39)) {
        *slot = c;
    }
    Chunk::data(*b"tdmn", b)
}

/// The user name of a property that has none (`-_0_/-`, aep-format.md 3.4).
pub(crate) const NO_NAME: &str = "-_0_/-";

/// `tdsn`: the user name, a nested chunk holding one `Utf8`.
pub(crate) fn tdsn(name: &str) -> Chunk<'static> {
    Chunk { id: *b"tdsn", body: Body::Chunks(vec![Chunk::data(*b"Utf8", name.as_bytes().to_vec())]), pad: None }
}

/// How a property's keys are stored (`tdb4` byte 7, aep-format.md 3.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeyTemplate {
    /// Value and type-major ease doubles: Opacity, Rotation, Scale, sliders.
    Plain,
    /// Ease, value and tangents; byte 7 is 3 on positions and points, 1 on colours and paths.
    Spatial(u8),
}

/// The u32 at `tdb4` bytes 8..11. Its two u16 halves follow the kind of property (aep-format.md
/// 3.4, "class word"); without it After Effects drops a property's keys (read-back e1-bz5).
pub(crate) mod class {
    /// Transform Scale, Rotation and Opacity; the other one-dimensional layer properties.
    pub const LAYER_SCALAR: u32 = 0x0001_ffff;
    /// Positions and points (spatial, no limits); effect sliders and angles.
    pub const SPATIAL: u32 = 0xffff_ffff;
    /// Effect sliders (integer, fixed, float) and angles.
    pub const EFFECT_NUMBER: u32 = 0xffff_ffff;
    /// Effect checkboxes and popups.
    pub const EFFECT_CHOICE: u32 = 0x0001_0004;
    /// Colours.
    pub const COLOR: u32 = 0x0002_ffff;
    /// Effect layer parameters, groups and buttons.
    pub const EFFECT_OTHER: u32 = 0x0001_0000;
    /// Arbitrary data (Hue/Saturation, Levels, Curves, Mesh Warp).
    pub const ARBITRARY: u32 = 0x0006_0007;
    /// Mask and shape paths.
    pub const PATH: u32 = 0x0002_0007;
}

/// Byte 5 bits 1..3 of a spatial property, next to the static bit 0: `0x0e` on positions and
/// points (`0x0f` static, `0x0e` keyed: Anchor Point, Position, Gradient Ramp's points), `0x06` on
/// colours (`0x07` / `0x06`: Tint's colours). Meaning not decoded.
fn spatial_byte5(template: u8) -> u8 {
    if template == 3 { 0x0e } else { 0x06 }
}

/// What a property record needs to know about its property.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RecSpec {
    /// Components of a value (`tdb4` byte 3).
    pub dims: u8,
    pub template: KeyTemplate,
    /// `tdb4` bytes 8..11 (see [`class`]).
    pub class: u32,
    /// `tdum` / `tduM`, written when the property has them (Scale `0, 0`, Opacity `0, 100`).
    pub limits: Option<(f64, f64)>,
    /// `tdb4` byte 5 of the static record when it is not the one [`tdb4`] derives (bits 1..3 are
    /// not decoded; the keyed record clears bit 0).
    pub byte5: Option<u8>,
}

impl RecSpec {
    /// Bytes of a static value (`cdat`): the key record without its 8-byte header, value first,
    /// the rest zero (aep-format.md 3.4 / 3.5: `40 n` plain, `24 n` spatial).
    pub fn cdat_len(&self) -> usize {
        let n = usize::from(self.dims);
        match self.template {
            KeyTemplate::Plain => 40 * n,
            KeyTemplate::Spatial(_) => 24 * n,
        }
    }

    /// Bytes of one key record (aep-format.md 3.5).
    pub fn key_len(&self) -> usize {
        let n = usize::from(self.dims);
        match self.template {
            KeyTemplate::Plain => 8 + 40 * n,
            KeyTemplate::Spatial(_) => 56 + 24 * n,
        }
    }
}

/// The 124-byte `tdb4` header of a property (aep-format.md 3.4). The decoded bytes, the class
/// word and the constant doubles at 16..56 are written; After Effects reads the other bytes as
/// zero (read-back e1-ztdb4d).
pub(crate) fn tdb4(spec: &RecSpec, keyed: bool, time_scale: u32) -> Vec<u8> {
    let mut b = vec![0u8; 124];
    let set = |b: &mut Vec<u8>, at: usize, bytes: &[u8]| {
        if let Some(dst) = b.get_mut(at..at + bytes.len()) {
            dst.copy_from_slice(bytes);
        }
    };
    set(&mut b, 0, &[0xdb, 0x99, 0x00, spec.dims]);
    let spatial = match spec.template {
        KeyTemplate::Plain => 0,
        KeyTemplate::Spatial(t) => t,
    };
    let byte5 = match spec.byte5 {
        Some(b) => (b & !0x01) | u8::from(!keyed),
        None => (if spatial == 0 { 0 } else { spatial_byte5(spatial) }) | u8::from(!keyed),
    };
    set(&mut b, 5, &[byte5]);
    set(&mut b, 7, &[spatial]);
    set(&mut b, 8, &spec.class.to_be_bytes());
    set(&mut b, 12, &time_scale.to_be_bytes());
    // Every record After Effects writes carries 0.0001 and four 1.0 here; without them a roving
    // key reads back at time zero instead of its retimed place (read-back s14-re-keyframes).
    set(&mut b, 16, &1e-4f64.to_be_bytes());
    for at in [24, 32, 40, 48] {
        set(&mut b, at, &1.0f64.to_be_bytes());
    }
    set(&mut b, 68, &[u8::from(keyed)]);
    // Set on a static spatial property, cleared once it is keyed (aep-format.md 3.4, byte 79).
    set(&mut b, 79, &[u8::from(spatial != 0 && !keyed)]);
    b
}

/// A key list (`LIST list`: `lhd3` + `ldat`) of `count` records of `size` bytes (aep-format.md
/// 3.5): word 0 `0x00d00bee`, word 2 the count, word 3 the blocks of four (at least one), word 4
/// the size, words 5 and 7 the capacity (the count rounded up to four, at least two), word 6 one.
pub(crate) fn key_list(size: u32, count: u32, records: Vec<u8>) -> Chunk<'static> {
    let mut lhd3 = vec![0u8; 52];
    // Cannot fail: the header is 52 bytes.
    let _ = crate::export::tree::put_u32(&mut lhd3, 0, 0x00d0_0bee);
    let _ = crate::export::tree::put_u32(&mut lhd3, 24, 1);
    let _ = put_key_counts(&mut lhd3, count, size);
    Chunk::list(*b"list", vec![Chunk::data(*b"lhd3", lhd3), Chunk::data(*b"ldat", records)])
}

/// Sets the count words of a key list header for `count` keys of `size` bytes.
pub(crate) fn put_key_counts(lhd3: &mut [u8], count: u32, size: u32) -> crate::export::tree::R<()> {
    let capacity = count.div_ceil(4).saturating_mul(4).max(2);
    put_counts(lhd3, count, count.div_ceil(4).max(1), size, capacity, capacity)
}

/// Sets the count words of a list header: word 2 the record count, word 3 `blocks`, word 4 the
/// record size, word 5 `w5`, word 7 the capacity. A key list has `w5` = the capacity; a path
/// list counts vertices: `blocks` = `w5` = the vertex count, the capacity four times it (090:
/// 12 points, 4, 4 and 16; aep-format.md 3.5).
pub(crate) fn put_counts(lhd3: &mut [u8], count: u32, blocks: u32, size: u32, w5: u32, capacity: u32) -> crate::export::tree::R<()> {
    use crate::export::tree::put_u32;
    put_u32(lhd3, 8, count)?;
    put_u32(lhd3, 12, blocks)?;
    put_u32(lhd3, 16, size)?;
    put_u32(lhd3, 20, w5)?;
    put_u32(lhd3, 28, capacity)
}

/// A property record (`LIST tdbs`) with a zero static value or an empty key list, for the
/// exporter to fill: `tdsb` 1 (written), no user name, the header, then `cdat` or the key list,
/// then the limits.
pub(crate) fn tdbs(spec: &RecSpec, keyed: bool, time_scale: u32) -> Chunk<'static> {
    let mut kids = vec![Chunk::data(*b"tdsb", 1u32.to_be_bytes().to_vec()), tdsn(NO_NAME), Chunk::data(*b"tdb4", tdb4(spec, keyed, time_scale))];
    if keyed {
        kids.push(key_list(u32::try_from(spec.key_len()).unwrap_or(u32::MAX), 0, Vec::new()));
    } else {
        kids.push(Chunk::data(*b"cdat", vec![0u8; spec.cdat_len()]));
    }
    if let Some((lo, hi)) = spec.limits {
        kids.push(Chunk::data(*b"tdum", lo.to_be_bytes().to_vec()));
        kids.push(Chunk::data(*b"tduM", hi.to_be_bytes().to_vec()));
    }
    Chunk::list(*b"tdbs", kids)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_has_the_decoded_layout() {
        let pos = RecSpec { dims: 3, template: KeyTemplate::Spatial(3), class: class::SPATIAL, limits: None, byte5: None };
        let h = tdb4(&pos, false, 25_600);
        assert_eq!(&h[..8], &[0xdb, 0x99, 0, 3, 0, 0x0f, 0, 3]);
        assert_eq!(&h[8..16], &[0xff, 0xff, 0xff, 0xff, 0, 0, 0x64, 0]);
        assert_eq!((h[68], h[79]), (0, 1));
        // A roving key needs these to keep its time (read-back s14-re-keyframes).
        assert_eq!(&h[16..24], &1e-4f64.to_be_bytes());
        assert!(h[24..56].chunks(8).all(|d| d == 1.0f64.to_be_bytes()));
        assert_eq!(pos.cdat_len(), 72);
        assert_eq!(pos.key_len(), 128);
        let opacity = RecSpec { dims: 1, template: KeyTemplate::Plain, class: class::LAYER_SCALAR, limits: Some((0.0, 100.0)), byte5: None };
        let k = tdb4(&opacity, true, 25_600);
        assert_eq!((k[5], k[7], k[68]), (0, 0, 1));
        let rec = tdbs(&opacity, false, 25_600);
        let ids: Vec<[u8; 4]> = rec.children().iter().map(|c| c.id).collect();
        assert_eq!(ids, [*b"tdsb", *b"tdsn", *b"tdb4", *b"cdat", *b"tdum", *b"tduM"]);
        assert_eq!(rec.child(b"cdat").and_then(|c| c.bytes()).map(<[u8]>::len), Some(40));
    }

    #[test]
    fn a_key_list_header_counts_and_rounds() {
        let l = key_list(48, 5, vec![0; 240]);
        let h = l.child(b"lhd3").and_then(|c| c.bytes()).unwrap();
        let w = |i: usize| u32::from_be_bytes(h[i * 4..i * 4 + 4].try_into().unwrap());
        assert_eq!((w(0), w(2), w(3), w(4), w(5), w(6), w(7)), (0x00d0_0bee, 5, 2, 48, 8, 1, 8));
        assert_eq!(h.len(), 52);
    }
}
