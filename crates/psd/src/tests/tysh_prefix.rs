use super::*;
use craft_psd::descriptor::{Class, Descriptor as SharedDescriptor, Id, ReferenceItem, UnicodeString, Value};

fn block(transform: [f64; 6], descriptor: SharedDescriptor) -> Vec<u8> {
    let mut b = 999u16.to_be_bytes().to_vec();
    b.extend(transform.into_iter().flat_map(f64::to_be_bytes));
    b.extend(999u16.to_be_bytes());
    b.extend(999u32.to_be_bytes());
    b.extend(descriptor.to_bytes());
    b
}

fn text_descriptor(text: &str) -> SharedDescriptor {
    SharedDescriptor::new("TxLr").with("Txt ", Value::Text(UnicodeString::new(text)))
}

fn parse_text(blocks: Vec<Vec<u8>>) -> Option<TextInfo> {
    let mut doc = WDoc::new(1, 1);
    doc.layers = vec![WLayer::empty("Text", blocks.into_iter().map(|b| (*b"TySh", b)).collect())];
    Psd::parse(write(&doc)).unwrap().layers.remove(0).text
}

#[test]
fn text_prefix_recovers_suffixes_and_repeated_blocks_without_changing_text_policy() {
    let transform = [2.0, 0.0, 0.0, 3.0, 4.0, 5.0];
    let data = block(transform, text_descriptor("a\r\nb\rc\n\n"));
    let expected = TextInfo { text: "a\nb\nc\n".into(), transform, ..Default::default() };
    for suffix in [vec![], vec![255], vec![0; 6], vec![255; 40]] {
        let mut b = data.clone();
        b.extend(suffix);
        assert_eq!(parse_text(vec![b]), Some(expected.clone()));
    }
    for cut in 0..data.len() {
        assert_eq!(parse_text(vec![data[..cut].to_vec()]), None, "cut {cut}");
        assert_eq!(parse_text(vec![data.clone(), data[..cut].to_vec()]), Some(expected.clone()));
    }
    let last = block(transform, text_descriptor("last"));
    assert_eq!(parse_text(vec![data, last]).unwrap().text, "last");
    for index in 0..6 {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut t = transform;
            t[index] = bad;
            assert_eq!(parse_text(vec![block(t, text_descriptor("x"))]).unwrap().transform, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        }
    }
}

#[test]
fn text_prefix_preserves_descriptor_normalization_and_bounds_precedence() {
    let bounds = SharedDescriptor::new("bounds")
        .with("Left", Value::Double(f64::NAN))
        .with("Top ", Value::UnitFloat { unit: *b"#Pnt", value: f64::INFINITY })
        .with("Rght", Value::Double(20.0))
        .with("Btom", Value::Double(10.0));
    let d = text_descriptor("x").with("bounds", Value::Descriptor(bounds.clone()));
    assert_eq!(parse_text(vec![block([1.0; 6], d)]).unwrap().bounds, Some([0.0, 0.0, 20.0, 10.0]));
    let d = text_descriptor("x")
        .with("bounds", Value::Descriptor(SharedDescriptor::new("bounds").with("Left", Value::Double(1.0))))
        .with("boundingBox", Value::Descriptor(bounds.clone()));
    assert_eq!(parse_text(vec![block([1.0; 6], d)]).unwrap().bounds, None);
    let d = text_descriptor("x").with("boundingBox", Value::Descriptor(bounds));
    assert_eq!(parse_text(vec![block([1.0; 6], d)]).unwrap().bounds, Some([0.0, 0.0, 20.0, 10.0]));

    let class = Class { name: UnicodeString::new("named"), class_id: Id::Str(b"abcd".to_vec()) };
    let d = text_descriptor("x")
        .with("ref ", Value::Reference(vec![ReferenceItem::Identifier { class: class.clone(), id: 7 }]))
        .with("cls ", Value::Class(class.clone()))
        .with("nums", Value::UnitFloats { unit: *b"#Pxl", values: vec![f64::NEG_INFINITY, 2.0] });
    let raw = craft_psd::text::TypeLayerPrefix::from_bytes_with_limits(&block([1.0; 6], d), descriptor::PARSE_LIMITS).unwrap();
    let converted = descriptor::from_craft_descriptor(raw.descriptor);
    assert_eq!(converted.class_id, "TxLr");
    assert_eq!(converted.items[1].1, descriptor::DValue::Reference(vec![descriptor::RefItem::IdentifierWithClass(class, 7)]));
    assert_eq!(converted.items[2].1, descriptor::DValue::Class("named".into(), "abcd".into()));
    assert_eq!(converted.items[3].1, descriptor::DValue::UnitFloats("#Pxl".into(), vec![0.0, 2.0]));
}

#[test]
fn text_prefix_enforces_effect_descriptor_count_limit_before_reading_items() {
    let mut data = block([1.0; 6], SharedDescriptor::new("TxLr"));
    let end = data.len();
    data[end - 4..].copy_from_slice(&100_001u32.to_be_bytes());
    let err = craft_psd::text::TypeLayerPrefix::from_bytes_with_limits(&data, descriptor::PARSE_LIMITS).unwrap_err();
    assert!(matches!(descriptor::desc_err(err), Error::Invalid(_) | Error::Unsupported(_)));
    assert_eq!(parse_text(vec![data]), None);
}

#[test]
fn text_prefix_reads_independent_raw_header_and_text_descriptor() {
    let mut data = vec![0, 9];
    for value in [1.0f64, 0.0, 0.0, 1.0, 12.0, -3.0] {
        data.extend(value.to_be_bytes());
    }
    // Ignored text/descriptor versions; empty class name, code class, one text item.
    data.extend([0, 8, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 0]);
    data.extend(b"TxLr");
    data.extend(1u32.to_be_bytes());
    data.extend(0u32.to_be_bytes());
    data.extend(b"Txt TEXT");
    data.extend(4u32.to_be_bytes());
    data.extend([0, 65, 0, 13, 0, 10, 0, 66]);
    let t = parse_text(vec![data]).unwrap();
    assert_eq!(t.text, "A\nB");
    assert_eq!(t.transform, [1.0, 0.0, 0.0, 1.0, 12.0, -3.0]);
}

#[test]
fn text_prefix_keeps_first_engine_style() {
    let engine = craft_psd::engine_data::minimal("x", "Courier", 24.0, [0.2, 0.4, 0.6], 1);
    let d = text_descriptor("x").with("EngineData", Value::RawData(engine));
    let style = parse_text(vec![block([1.0; 6], d)]).unwrap().style;
    assert_eq!(style.font.as_deref(), Some("Courier"));
    assert_eq!(style.size, Some(24.0));
    assert_eq!(style.color, Some([0.2, 0.4, 0.6]));
    assert_eq!(style.justification, Some(1));
}
