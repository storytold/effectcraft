use super::*;

fn record(units: &[u16]) -> Vec<u8> {
    let mut bytes = (units.len() as u32).to_be_bytes().to_vec();
    bytes.extend(units.iter().flat_map(|v| v.to_be_bytes()));
    bytes
}

fn parse_name(blocks: Vec<Vec<u8>>) -> String {
    let mut doc = WDoc::new(1, 1);
    doc.layers = vec![WLayer::empty("Fallback", blocks.into_iter().map(|b| (*b"luni", b)).collect())];
    Psd::parse(write(&doc)).unwrap().layers.remove(0).name
}

#[test]
fn unicode_names_preserve_lossy_utf16_and_trailing_nul_policy() {
    for (units, expected) in [
        (vec![], "Fallback"),
        (vec![0, 0], "Fallback"),
        (vec![65], "A"),
        (vec![65, 0, 0], "A"),
        (vec![0, 65, 0, 66, 0], "\0A\0B"),
        (vec![0x00e4, 0xd83d, 0xde00], "\u{e4}\u{1f600}"),
        (vec![0xd800, 65, 0xdc00, 0], "\u{fffd}A\u{fffd}"),
        (vec![0xd800, 0], "\u{fffd}"),
        (vec![0xd800, 0xd83d, 0xde00, 0xdc00], "\u{fffd}\u{1f600}\u{fffd}"),
    ] {
        assert_eq!(parse_name(vec![record(&units)]), expected, "{units:?}");
    }
}

#[test]
fn unicode_name_uses_first_complete_record_and_recovers_in_block_order() {
    let first = record(&[65]);
    let mut combined = first.clone();
    combined.extend(record(&[66]));
    combined.extend([0xff, 0]);
    assert_eq!(parse_name(vec![combined]), "A");
    let mut empty_then_name = record(&[]);
    empty_then_name.extend(record(&[66]));
    assert_eq!(parse_name(vec![empty_then_name]), "Fallback");
    for suffix in [vec![], vec![0xff], vec![0, 0, 0], vec![0xff; 8]] {
        let mut bytes = first.clone();
        bytes.extend(suffix);
        assert_eq!(parse_name(vec![bytes]), "A");
    }
    let mut malformed = vec![vec![], vec![0], vec![0, 0], vec![0, 0, 0], vec![0, 0, 0, 1], vec![0, 0, 0, 1, 0]];
    malformed.push([2u32.to_be_bytes().as_slice(), &[0, 65, 0]].concat());
    malformed.push(u32::MAX.to_be_bytes().to_vec());
    for bad in malformed {
        assert_eq!(parse_name(vec![bad.clone()]), "Fallback");
        assert_eq!(parse_name(vec![first.clone(), bad.clone(), record(&[0, 0])]), "A");
        assert_eq!(parse_name(vec![bad, record(&[66])]), "B");
    }
    assert_eq!(parse_name(vec![first, record(&[66])]), "B");
}

#[test]
fn unicode_layer_name_writer_keeps_code_unit_count_without_added_terminator() {
    for name in ["", "A", "\u{e4}\u{1f600}", "A\0", "\0B\0"] {
        let mut doc = WDoc::new(1, 1);
        doc.layers = vec![WLayer::empty(name, vec![])];
        let bytes = write(&doc);
        let file = craft_psd::PsdFile::from_bytes(&bytes).unwrap();
        let block = file.layer_info.as_ref().unwrap().layers[0].block(b"luni").unwrap();
        let units: Vec<_> = name.encode_utf16().collect();
        assert_eq!(block.data, record(&units));
    }
}

#[test]
fn missing_empty_or_malformed_unicode_name_keeps_pascal_name() {
    let mut doc = WDoc::new(1, 1);
    doc.layers = vec![WLayer::empty("Generated Unicode name", vec![])];
    let bytes = write(&doc);
    for payload in [None, Some(vec![]), Some(record(&[])), Some(record(&[0, 0])), Some(vec![0, 0, 0, 1, 0]), Some(u32::MAX.to_be_bytes().to_vec())] {
        let mut file = craft_psd::PsdFile::from_bytes(&bytes).unwrap();
        let layer = &mut file.layer_info.as_mut().unwrap().layers[0];
        layer.name = b"Pascal".to_vec();
        layer.blocks.retain(|b| b.key != *b"luni");
        if let Some(data) = payload {
            layer.blocks.push(craft_psd::tagged::TaggedBlock::new(*b"luni", data));
        }
        assert_eq!(Psd::parse(file.to_bytes().unwrap()).unwrap().layers[0].name, "Pascal");
    }
}
