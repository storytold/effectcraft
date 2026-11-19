use super::*;

#[test]
fn fixed_adjustment_writers_keep_exact_bytes_through_full_psd() {
    let cases = [
        (brightness_block(i16::MIN, i16::MAX), *b"brit", vec![0x80, 0, 0x7f, 0xff, 0, 0x7f, 0]),
        (brightness_block(-1, 0), *b"brit", vec![0xff, 0xff, 0, 0, 0, 0x7f, 0]),
        (exposure_block(0.0, -0.0, f32::INFINITY), *b"expA", vec![0, 1, 0, 0, 0, 0, 0x80, 0, 0, 0, 0x7f, 0x80, 0, 0]),
        (
            exposure_block(f32::NEG_INFINITY, f32::from_bits(0x7fc1_2345), f32::from_bits(0xffc5_4321)),
            *b"expA",
            vec![0, 1, 0xff, 0x80, 0, 0, 0x7f, 0xc1, 0x23, 0x45, 0xff, 0xc5, 0x43, 0x21],
        ),
        (exposure_block(1.0, -2.5, 0.125), *b"expA", vec![0, 1, 0x3f, 0x80, 0, 0, 0xc0, 0x20, 0, 0, 0x3e, 0, 0, 0]),
    ];
    for (block, key, expected) in cases {
        assert_eq!(block.0, key);
        assert_eq!(block.1, expected);
        let mut doc = WDoc::new(1, 1);
        doc.layers = vec![WLayer::empty("raw adjustment", vec![block])];
        let bytes = write(&doc);
        let file = craft_psd::PsdFile::from_bytes(&bytes).unwrap();
        let emitted = file.layer_info.as_ref().unwrap().layers[0].block(&key).unwrap();
        assert_eq!(emitted.data, expected);
    }
}
