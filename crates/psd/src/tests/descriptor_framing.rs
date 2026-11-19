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
