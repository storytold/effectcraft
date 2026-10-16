//! Packed layer cache entries (`packed.rs`): lossless for quantised 8/16 bpc buffers, never
//! applied to anything else.

use std::sync::Arc;

use crate::packed::PackedBuf;
use crate::{Buf, LayerCache};
use effectcraft_raster::Image;

/// Deterministic premultiplied pixels (colour ≤ alpha, some transparent, some opaque).
fn noise(w: u32, h: u32, seed: u32) -> Image {
    let mut s = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        (s >> 8) as f32 / (1u32 << 24) as f32
    };
    let mut img = Image::new(w, h);
    for (i, p) in img.data.iter_mut().enumerate() {
        let a = match i % 7 {
            0 => 0.0,
            1 => 1.0,
            _ => next(),
        };
        *p = [next() * a, next() * a, next() * a, a];
    }
    img
}

fn buf(img: Image) -> Buf {
    Buf { img, offset: [-3.5, 12.25], scale: 0.5 }
}

fn bits(b: &Buf) -> Vec<[u32; 4]> {
    b.img.data.iter().map(|p| p.map(f32::to_bits)).collect()
}

#[test]
fn quantised_buffers_round_trip_bit_exactly() {
    for (levels, px_bytes) in [(255.0, 4), (32768.0, 8)] {
        let mut img = noise(37, 23, levels as u32);
        crate::color::quantize(&mut img, levels);
        let b = buf(img);
        let p = PackedBuf::pack(&b).unwrap_or_else(|| panic!("{levels} levels packs"));
        assert_eq!(p.bytes(), b.img.data.len() * px_bytes, "{levels} levels");
        let u = p.unpack();
        assert_eq!(bits(&u), bits(&b), "{levels} levels");
        assert_eq!((u.img.width, u.img.height, u.offset, u.scale), (b.img.width, b.img.height, b.offset, b.scale));
    }
}

#[test]
fn unquantised_buffers_are_not_packed() {
    // 32 bpc: off-grid values, over-range values, negative zero.
    assert!(PackedBuf::pack(&buf(noise(16, 16, 5))).is_none());
    let mut img = noise(4, 4, 6);
    crate::color::quantize(&mut img, 255.0);
    img.data[5] = [1.5, 1.0, 1.0, 1.0];
    assert!(PackedBuf::pack(&buf(img.clone())).is_none());
    img.data[5] = [-0.0, 0.0, 0.0, 0.0];
    assert!(PackedBuf::pack(&buf(img)).is_none());
}

/// Over budget, quantised entries are packed (counted at their packed size) instead of evicted,
/// and come back bit-identical; float entries are untouched (evicted least recently used).
#[test]
fn cache_packs_under_pressure_and_counts_packed_bytes() {
    let f32_bytes = 64 * 64 * 16;
    let cache = LayerCache::new(4 * f32_bytes);
    let mut kept = vec![];
    for i in 0..6u64 {
        let mut img = noise(64, 64, i as u32);
        crate::color::quantize(&mut img, 255.0);
        let b = Arc::new(buf(img));
        kept.push(bits(&b));
        cache.insert(i, b);
    }
    let st = cache.stats();
    assert_eq!(st.entries, 6, "nothing evicted");
    // The fifth insert went over budget: the four oldest were packed to u8.
    assert_eq!(st.bytes, 4 * f32_bytes / 4 + 2 * f32_bytes, "packed entries count at their packed size");
    let back = cache.get(0).unwrap_or_else(|| panic!("entry 0"));
    assert_eq!(bits(&back), kept[0]);
    // A hit unpacks the entry for good: the next hit shares the same buffer.
    let again = cache.get(0).unwrap_or_else(|| panic!("entry 0"));
    assert!(Arc::ptr_eq(&back, &again));
    assert_eq!(cache.stats().bytes, 3 * f32_bytes / 4 + 3 * f32_bytes);
    for (i, want) in kept.iter().enumerate() {
        assert_eq!(&bits(&cache.get(i as u64).unwrap_or_else(|| panic!("entry {i}"))), want, "entry {i}");
    }

    // 32 bpc: never packed, the same Arc is served back.
    let cache = LayerCache::new(4 * f32_bytes);
    let bufs: Vec<_> = (0..6u64).map(|i| Arc::new(buf(noise(64, 64, 100 + i as u32)))).collect();
    for (i, b) in bufs.iter().enumerate() {
        cache.insert(i as u64, b.clone());
    }
    let st = cache.stats();
    assert_eq!(st.bytes, st.entries * f32_bytes);
    assert!(st.entries < 6, "evicted, not packed");
    let last = cache.get(5).unwrap_or_else(|| panic!("newest kept"));
    assert!(Arc::ptr_eq(&last, &bufs[5]));
}

/// The renderer's own buffers: an 8 bpc layer's cached content packs and unpacks exactly.
#[test]
fn rendered_8bpc_layer_packs_losslessly() {
    let mut img = noise(40, 30, 77);
    crate::color::Pipe::of(&effectcraft_project::ProjectSettings::default()).quantize(&mut img);
    let b = buf(img);
    let p = PackedBuf::pack(&b).unwrap_or_else(|| panic!("default project (8 bpc) packs"));
    assert_eq!(p.bytes(), b.img.data.len() * 4);
    assert_eq!(bits(&p.unpack()), bits(&b));
}
