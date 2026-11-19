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

fn assert_written_payload(block: ([u8; 4], Vec<u8>), key: [u8; 4], expected: Vec<u8>) {
    assert_eq!(block.0, key);
    assert_eq!(block.1, expected);
    let mut doc = WDoc::new(1, 1);
    doc.layers = vec![WLayer::empty("raw adjustment", vec![block])];
    let bytes = write(&doc);
    let file = craft_psd::PsdFile::from_bytes(&bytes).unwrap();
    assert_eq!(file.layer_info.as_ref().unwrap().layers[0].block(&key).unwrap().data, expected);
}

#[test]
fn hue_writer_keeps_all_six_unused_ranges_and_signed_master_values() {
    let mut expected = vec![0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0x7f, 0xff, 0xff, 0xff];
    expected.extend([0; 144]);
    assert_written_payload(hue_sat_block(i16::MIN, i16::MAX, -1), *b"hue2", expected);
    let mut expected = vec![0; 160];
    expected[1] = 2;
    assert_written_payload(hue_sat_block(0, 0, 0), *b"hue2", expected);
}

#[test]
fn levels_writer_preserves_gamma_conversion_and_all_default_records() {
    // Expected bytes are constants, not another execution of the conversion.
    for (gamma, gamma_bytes) in [
        (0.125, [0, 13]),
        (-0.125, [255, 243]),
        (0.0, [0, 0]),
        (-0.0, [0, 0]),
        (f64::NAN, [0, 0]),
        (f64::INFINITY, [127, 255]),
        (f64::NEG_INFINITY, [128, 0]),
        (f64::MAX, [127, 255]),
        (-f64::MAX, [128, 0]),
        (400.0, [127, 255]),
        (-400.0, [128, 0]),
        (1.0, [0, 100]),
    ] {
        let mut expected = vec![0, 2, 0x80, 0, 0x7f, 0xff, 0xff, 0xff, 0, 0];
        expected.extend(gamma_bytes);
        expected.extend([0, 0, 0, 255, 0, 0, 0, 255, 0, 100].repeat(28));
        assert_written_payload(levels_block(i16::MIN, i16::MAX, -1, 0, gamma), *b"levl", expected);
    }
}
