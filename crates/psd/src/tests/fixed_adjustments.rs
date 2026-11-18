use super::*;

type Block = ([u8; 4], Vec<u8>);

fn i16s(prefix: &[u8], values: &[i16]) -> Vec<u8> {
    let mut bytes = prefix.to_vec();
    bytes.extend(values.iter().flat_map(|v| v.to_be_bytes()));
    bytes
}

fn exposure(values: [f32; 3]) -> Vec<u8> {
    let mut bytes = vec![0xff, 0xfe]; // Versions are intentionally not rejected.
    bytes.extend(values.iter().flat_map(|v| v.to_be_bytes()));
    bytes
}

fn parse_adjustment(blocks: Vec<Block>) -> Option<Adjustment> {
    let mut doc = WDoc::new(1, 1);
    doc.layers = vec![WLayer::empty("adjustment", blocks)];
    Psd::parse(write(&doc)).unwrap().layers.remove(0).adjustment
}

fn cases() -> Vec<(Block, Adjustment)> {
    let mut cases = vec![
        ((*b"brit", i16s(&[], &[i16::MIN, i16::MAX])), Adjustment::BrightnessContrast { brightness: -32768.0, contrast: 32767.0, legacy: true }),
        (
            (*b"levl", i16s(&[0xff, 0xfe], &[i16::MIN, i16::MAX, -12, 300, -250])),
            Adjustment::Levels { in_black: -32768.0, in_white: 32767.0, gamma: -2.5, out_black: -12.0, out_white: 300.0 },
        ),
        ((*b"expA", exposure([-1.5, 2.25, 0.125])), Adjustment::Exposure { exposure: -1.5, offset: 2.25, gamma: 0.125 }),
    ];
    for key in [*b"hue2", *b"hue "] {
        for flag in [0, 1, 255] {
            let values = if flag == 0 { [100.0, -200.0, 300.0] } else { [-32768.0, 32767.0, -4.0] };
            cases.push((
                (key, i16s(&[0xff, 0xfe, flag, 0xa5], &[i16::MIN, i16::MAX, -4, 100, -200, 300])),
                Adjustment::HueSaturation { hue: values[0], saturation: values[1], lightness: values[2], colorize: flag != 0 },
            ));
        }
    }
    cases
}

#[test]
fn fixed_adjustments_preserve_fields_truncation_and_trailing_bytes() {
    for ((key, bytes), expected) in cases() {
        for end in 0..bytes.len() {
            assert_eq!(parse_adjustment(vec![(key, bytes[..end].to_vec())]), None, "{key:?} prefix {end}");
            assert_eq!(parse_adjustment(vec![(*b"nvrt", vec![]), (key, bytes[..end].to_vec())]), Some(Adjustment::Invert));
        }
        for tail in [vec![], vec![0xff], vec![0; 20]] {
            let mut data = bytes.clone();
            data.extend(tail);
            assert_eq!(parse_adjustment(vec![(key, data)]), Some(expected.clone()), "{key:?}");
        }
        let after_invert = if key == *b"brit" { Adjustment::Invert } else { expected };
        assert_eq!(parse_adjustment(vec![(*b"nvrt", vec![]), (key, bytes)]), Some(after_invert));
    }
}

#[test]
fn exposure_nonfinite_values_use_each_fields_original_default() {
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        for axis in 0..3 {
            let mut values = [-1.5, 2.25, 0.125];
            values[axis] = value;
            let mut expected = [-1.5, 2.25, 0.125];
            expected[axis] = [0.0, 0.0, 1.0][axis];
            assert_eq!(
                parse_adjustment(vec![(*b"expA", exposure(values))]),
                Some(Adjustment::Exposure { exposure: expected[0], offset: expected[1], gamma: expected[2] })
            );
        }
    }
    let Some(Adjustment::Exposure { exposure: e, offset, gamma }) = parse_adjustment(vec![(*b"expA", exposure([-0.0; 3]))]) else {
        panic!("exposure missing");
    };
    assert!(e.is_sign_negative() && offset.is_sign_negative() && gamma.is_sign_negative());
}

#[test]
fn fixed_adjustments_keep_brit_descriptor_and_other_block_precedence() {
    let brit = (*b"brit", i16s(&[], &[10, 20]));
    let mut cged = 16u32.to_be_bytes().to_vec();
    crate::descriptor::write_descriptor(
        &mut cged,
        &Descriptor::new("brightnessContrast").with("Brgh", DValue::Double(30.0)).with("Cntr", DValue::Double(-40.0)).with("useLegacy", DValue::Bool(false)),
    );
    let modern = Adjustment::BrightnessContrast { brightness: 30.0, contrast: -40.0, legacy: false };
    for blocks in [vec![brit.clone(), (*b"CgEd", cged.clone())], vec![(*b"CgEd", cged), brit.clone()]] {
        assert_eq!(parse_adjustment(blocks), Some(modern.clone()));
    }
    assert_eq!(parse_adjustment(vec![(*b"curv", vec![]), brit.clone()]), Some(Adjustment::Other("curv".into())));
    assert_eq!(
        parse_adjustment(vec![brit.clone(), (*b"curv", vec![]), (*b"brit", i16s(&[], &[50, 60]))]),
        Some(Adjustment::BrightnessContrast { brightness: 10.0, contrast: 20.0, legacy: true })
    );
    for ((key, data), expected) in cases().into_iter().filter(|((key, _), _)| key != b"brit") {
        assert_eq!(parse_adjustment(vec![(*b"curv", vec![]), brit.clone(), (key, data), (*b"curv", vec![]), brit.clone()]), Some(expected));
    }
}
