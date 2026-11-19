use super::*;

fn scalars(blocks: Vec<([u8; 4], Vec<u8>)>) -> (Option<u32>, u8, u8) {
    let mut doc = WDoc::new(1, 1);
    doc.layers = vec![WLayer::empty("Scalars", blocks).with_opacity(37)];
    let layer = Psd::parse(write(&doc)).unwrap().layers.remove(0);
    (layer.id, layer.fill_opacity, layer.opacity)
}

#[test]
fn scalar_prefixes_keep_defaults_and_ignore_trailing_bytes() {
    assert_eq!(scalars(vec![]), (None, 255, 37));
    let bytes = [0x89, 0xab, 0xcd, 0xef, 255, 0, 23];
    for end in 0..=bytes.len() {
        assert_eq!(scalars(vec![(*b"lyid", bytes[..end].to_vec())]), (if end < 4 { None } else { Some(0x89ab_cdef) }, 255, 37), "lyid prefix {end}");
        assert_eq!(scalars(vec![(*b"iOpa", bytes[..end].to_vec())]), (None, if end == 0 { 255 } else { 0x89 }, 37), "iOpa prefix {end}");
    }
    for id in [0u32, 1, 0x8000_0000, u32::MAX] {
        for opacity in [0, 1, 254, 255] {
            assert_eq!(scalars(vec![(*b"lyid", id.to_be_bytes().to_vec()), (*b"iOpa", vec![opacity])]), (Some(id), opacity, 37));
        }
    }
}

#[test]
fn repeated_scalar_blocks_preserve_prior_values_on_short_fields_in_both_orders() {
    for reverse in [false, true] {
        for short in 0..4 {
            let mut initial = vec![(*b"lyid", vec![0, 0, 0, 42]), (*b"iOpa", vec![99])];
            if reverse {
                initial.reverse();
            }
            let mut blocks = initial.clone();
            blocks.extend([(*b"lyid", vec![255; short]), (*b"iOpa", vec![]), (*b"zzzz", vec![0; 4])]);
            assert_eq!(scalars(blocks.clone()), (Some(42), 99, 37));
            blocks.extend([(*b"iOpa", vec![0, 77]), (*b"lyid", vec![0, 0, 0, 7, 255])]);
            assert_eq!(scalars(blocks), (Some(7), 0, 37));
        }
    }
}
