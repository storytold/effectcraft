use super::*;
use craft_psd::descriptor::{Descriptor as Shared, UnicodeString, Value};

fn bytes(key: [u8; 4], d: Shared, version: u32) -> Vec<u8> {
    let mut b = version.to_be_bytes().repeat(if key == *b"lfx2" { 2 } else { 1 });
    b.extend(d.to_bytes());
    b
}

fn state(blocks: Vec<([u8; 4], Vec<u8>)>) -> (Option<descriptor::Descriptor>, Option<[f64; 3]>, Option<Adjustment>) {
    let mut doc = WDoc::new(1, 1);
    doc.layers = vec![WLayer::empty("Descriptor", blocks)];
    let layer = Psd::parse(write(&doc)).unwrap().layers.remove(0);
    (layer.effects, layer.fill, layer.adjustment)
}

fn value(key: [u8; 4]) -> Shared {
    match &key {
        b"lfx2" => Shared::new("null").with("name", Value::Text(UnicodeString::new("effect"))),
        b"SoCo" => Shared::new("null").with(
            "Clr ",
            Value::Descriptor(Shared::new("RGBC").with("Rd  ", Value::Double(255.0)).with("Grn ", Value::Double(127.5)).with("Bl  ", Value::Double(0.0))),
        ),
        _ => Shared::new("null").with("Brgh", Value::Double(12.0)).with("Cntr", Value::Double(-4.0)).with("useLegacy", Value::Boolean(true)),
    }
}

#[test]
fn tagged_descriptor_headers_ignore_versions_and_tails_and_preserve_short_block_state() {
    for key in [*b"lfx2", *b"SoCo", *b"CgEd"] {
        let good = bytes(key, value(key), 16);
        let expected = state(vec![(key, good.clone())]);
        match &key {
            b"lfx2" => assert_eq!(expected.0.as_ref().unwrap().text("name"), Some("effect")),
            b"SoCo" => assert_eq!(expected.1, Some([1.0, 0.5, 0.0])),
            _ => assert_eq!(expected.2, Some(Adjustment::BrightnessContrast { brightness: 12.0, contrast: -4.0, legacy: true })),
        }
        for cut in 0..good.len() {
            assert_eq!(state(vec![(key, good[..cut].to_vec())]), (None, None, None), "{key:?} cut {cut}");
            assert_eq!(state(vec![(key, good.clone()), (key, good[..cut].to_vec())]), expected);
        }
        for version in [0, 99, u32::MAX] {
            for tail in [vec![], vec![255], vec![1, 2, 3, 4, 5]] {
                let mut b = bytes(key, value(key), version);
                b.extend(tail);
                assert_eq!(state(vec![(key, b)]), expected);
            }
        }
        let empty = bytes(key, Shared::new("null"), 0);
        let replaced = state(vec![(key, good), (key, empty)]);
        match &key {
            b"lfx2" => assert!(replaced.0.unwrap().items.is_empty()),
            b"SoCo" => assert_eq!(replaced.1, None),
            _ => assert_eq!(replaced.2, Some(Adjustment::BrightnessContrast { brightness: 0.0, contrast: 0.0, legacy: false })),
        }
    }
}

#[test]
fn tagged_descriptors_keep_normalization_and_adjustment_precedence() {
    let d = Shared::new("null").with("Brgh", Value::Double(f64::NAN)).with("Cntr", Value::UnitFloat { unit: *b"#Prc", value: f64::INFINITY });
    let cged = bytes(*b"CgEd", d, 99);
    let expected = Some(Adjustment::BrightnessContrast { brightness: 0.0, contrast: 0.0, legacy: false });
    assert_eq!(state(vec![(*b"nvrt", vec![]), (*b"CgEd", cged.clone())]).2, expected);
    assert_eq!(state(vec![(*b"CgEd", cged.clone()), (*b"brit", vec![0, 42, 0, 7])]).2, expected);
    assert_eq!(state(vec![(*b"CgEd", cged), (*b"nvrt", vec![])]).2, Some(Adjustment::Invert));
    let color = Shared::new("RGBC").with("Rd  ", Value::Double(f64::NEG_INFINITY)).with("Grn ", Value::Double(255.0)).with("Bl  ", Value::Double(0.0));
    assert_eq!(state(vec![(*b"SoCo", bytes(*b"SoCo", Shared::new("null").with("Clr ", Value::Descriptor(color)), 0))]).1, Some([0.0, 1.0, 0.0]));
}

#[test]
fn tagged_descriptor_raw_fixture_and_limits_do_not_depend_on_shared_writer() {
    for key in [*b"lfx2", *b"SoCo", *b"CgEd"] {
        let mut raw = vec![255; if key == *b"lfx2" { 8 } else { 4 }];
        raw.extend([0; 8]);
        raw.extend(b"null");
        raw.extend(0u32.to_be_bytes());
        let result = state(vec![(key, raw.clone())]);
        if key == *b"lfx2" {
            assert!(result.0.is_some());
        }
        if key == *b"CgEd" {
            assert!(result.2.is_some());
        }
        let end = raw.len();
        raw[end - 4..].copy_from_slice(&100_001u32.to_be_bytes());
        let block = craft_psd::tagged::TaggedBlock::new(key, raw.clone());
        assert!(matches!(block.descriptor_prefix_recovering_versions_with_limits(descriptor::PARSE_LIMITS), Some(Err(craft_psd::PsdError::LimitExceeded(_)))));
        let good = bytes(key, value(key), 16);
        assert_eq!(state(vec![(key, good.clone()), (key, raw)]), state(vec![(key, good)]));
    }
}
