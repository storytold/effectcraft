use super::*;
use crate::descriptor::{DValue, Descriptor};
fn hex(s: &str) -> Vec<u8> {
    s.split_whitespace().map(|b| u8::from_str_radix(b, 16).unwrap()).collect()
}
fn check(block: ([u8; 4], Vec<u8>), key: [u8; 4], expected: Vec<u8>) {
    assert_eq!(block.0, key);
    assert_eq!(block.1, expected);
    let mut doc = WDoc::new(1, 1);
    doc.layers = vec![WLayer::empty("framing", vec![block])];
    let bytes = write(&doc);
    let file = craft_psd::PsdFile::from_bytes(&bytes).unwrap();
    assert_eq!(file.layer_info.as_ref().unwrap().layers[0].block(&key).unwrap().data, expected);
}
#[test]
fn effects_data_keeps_headers_raw_bits_duplicate_keys_and_outer_framing() {
    check(effects_block(&Descriptor::new("null")), *b"lfx2", hex("00 00 00 00 00 00 00 10 00 00 00 01 00 00 00 00 00 00 6e 75 6c 6c 00 00 00 00"));
    let desc = Descriptor::new("null").with("same", DValue::Double(f64::from_bits(0x7ff8_1234_5678_9abc))).with("same", DValue::Text("\u{1f600}".into()));
    check(
        effects_block(&desc),
        *b"lfx2",
        hex("00 00 00 00 00 00 00 10 00 00 00 01 00 00 00 00 00 00 6e 75 6c 6c 00 00 00 02
             00 00 00 00 73 61 6d 65 64 6f 75 62 7f f8 12 34 56 78 9a bc
             00 00 00 00 73 61 6d 65 54 45 58 54 00 00 00 03 d8 3d de 00 00 00"),
    );
}
#[test]
fn solid_color_keeps_rgb_scaling_order_and_nonfinite_policy() {
    check(
        solid_color_block([0.5, -0.0, f64::INFINITY]),
        *b"SoCo",
        hex("00 00 00 10 00 00 00 01 00 00 00 00 00 00 6e 75 6c 6c 00 00 00 01
             00 00 00 00 43 6c 72 20 4f 62 6a 63
             00 00 00 01 00 00 00 00 00 00 52 47 42 43 00 00 00 03
             00 00 00 00 52 64 20 20 64 6f 75 62 40 5f e0 00 00 00 00 00
             00 00 00 00 47 72 6e 20 64 6f 75 62 80 00 00 00 00 00 00 00
             00 00 00 00 42 6c 20 20 64 6f 75 62 7f f0 00 00 00 00 00 00"),
    );
}

#[test]
fn smart_object_prefix_keeps_asymmetric_raw_transform_and_optional_warp() {
    let quad = [[1.0, -0.0], [2.0, f64::INFINITY], [f64::from_bits(0x7ff8_1234_5678_9abc), -3.0], [4.0, 5.0]];
    // Literal descriptor wire fields; no shared writer or parser constructs the oracle.
    let transform = hex("56 6c 4c 73 00 00 00 08
        64 6f 75 62 3f f0 00 00 00 00 00 00
        64 6f 75 62 80 00 00 00 00 00 00 00
        64 6f 75 62 40 00 00 00 00 00 00 00
        64 6f 75 62 7f f0 00 00 00 00 00 00
        64 6f 75 62 7f f8 12 34 56 78 9a bc
        64 6f 75 62 c0 08 00 00 00 00 00 00
        64 6f 75 62 40 10 00 00 00 00 00 00
        64 6f 75 62 40 14 00 00 00 00 00 00");
    let mut expected = hex("73 6f 4c 44 00 00 00 04 00 00 00 10
        00 00 00 01 00 00 00 00 00 00 6e 75 6c 6c 00 00 00 05
        00 00 00 00 49 64 6e 74 54 45 58 54 00 00 00 02 00 75 00 00
        00 00 00 06 70 6c 61 63 65 64 54 45 58 54 00 00 00 02 00 75 00 00
        00 00 00 00 54 72 6e 66");
    expected.extend_from_slice(&transform);
    expected.extend(hex("00 00 00 12 6e 6f 6e 41 66 66 69 6e 65 54 72 61 6e 73 66 6f 72 6d"));
    expected.extend_from_slice(&transform);
    expected.extend(hex("00 00 00 00 53 7a 20 20 4f 62 6a 63
        00 00 00 01 00 00 00 00 00 00 50 6e 74 20 00 00 00 02
        00 00 00 00 57 64 74 68 64 6f 75 62 40 24 00 00 00 00 00 00
        00 00 00 00 48 67 68 74 64 6f 75 62 40 34 00 00 00 00 00 00"));
    check(smart_object_block("u", quad, [10.0, 20.0]), *b"SoLd", expected.clone());
    // The optional warp is the sixth parent item, not a change to either list count.
    expected[29] = 6;
    expected.extend(hex("00 00 00 00 77 61 72 70 4f 62 6a 63
        00 00 00 01 00 00 00 00 00 00 77 61 72 70 00 00 00 01
        00 00 00 00 72 61 77 20 64 6f 75 62 ff f8 12 34 56 78 9a bc"));
    let warp = Descriptor::new("warp").with("raw ", DValue::Double(f64::from_bits(0xfff8_1234_5678_9abc)));
    check(smart_object_block_warped("u", quad, [10.0, 20.0], Some(warp)), *b"SoLd", expected);
}
