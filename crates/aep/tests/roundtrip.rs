//! Byte-identical round trip over generated fixtures, and the structural facts of the container.

use std::collections::BTreeMap;

mod common;

use common::{FIXTURES, fixture};
use effectcraft_aep::{AepFile, Body, parse, write};

fn id(c: &[u8; 4]) -> String {
    String::from_utf8_lossy(c).into_owned()
}

#[test]
fn every_fixture_round_trips_byte_for_byte() {
    for name in FIXTURES {
        let bytes = fixture(name);
        let file = parse(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        let back = write(&file).unwrap();
        assert_eq!(back.len(), bytes.len(), "{name}: length");
        assert!(back == bytes, "{name}: bytes differ");
        // Owned copy round-trips too.
        let owned: AepFile<'static> = file.into_owned();
        assert!(write(&owned).unwrap() == bytes, "{name}: owned copy differs");
    }
}

#[test]
fn header_and_trailer_facts() {
    for name in FIXTURES {
        let bytes = fixture(name);
        assert_eq!(&bytes[..4], b"RIFX");
        assert_eq!(&bytes[8..12], b"Egg!");
        let declared = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
        let file = parse(&bytes).unwrap();
        // The declared size ends before the end of the file; the rest is the XMP packet.
        assert!(declared + 8 < bytes.len(), "{name}");
        assert_eq!(file.trailer.len(), bytes.len() - declared - 8, "{name}");
        assert!(file.trailer.starts_with(b"<?xpacket begin"), "{name}");
        let xmp = file.xmp().unwrap_or_else(|| panic!("{name}: no xmp"));
        assert!(xmp.starts_with(b"<?xpacket begin") && xmp.ends_with(b"?>"), "{name}");
        assert!(std::str::from_utf8(xmp).is_ok(), "{name}: xmp is UTF-8");
    }
}

#[test]
fn top_level_chunks_of_the_base_project() {
    let bytes = fixture("000-base.aep");
    let file = parse(&bytes).unwrap();
    let ids: Vec<String> = file.chunks.iter().map(|c| if c.id == *b"LIST" { format!("LIST {}", id(&c.list_type().unwrap())) } else { id(&c.id) }).collect();
    // The first chunks, in file order (observed in every generated file).
    assert_eq!(&ids[..3], ["svap", "head", "nhed"]);
    assert!(ids.iter().any(|i| i == "LIST Fold"), "{ids:?}");
    // Chunk offsets are 2-byte aligned and every id is 4 printable ASCII bytes.
    for (_, c) in file.walk() {
        assert!(c.id.iter().all(|b| (0x20..0x7f).contains(b)), "{:?}", c.id);
    }
}

#[test]
fn nesting_depth_is_below_the_limit() {
    for name in FIXTURES {
        let bytes = fixture(name);
        let file = parse(&bytes).unwrap();
        let max = file.walk().map(|(d, _)| d).max().unwrap();
        assert!(max <= 13, "{name}: depth {max}");
        // Nothing is kept opaque because of the depth limit.
        assert!(max < 64);
    }
}

#[test]
fn nested_chunk_ids_hold_one_utf8_chunk() {
    let mut nested = BTreeMap::<String, usize>::new();
    for name in FIXTURES {
        let bytes = fixture(name);
        let file = parse(&bytes).unwrap();
        for (_, c) in file.walk() {
            if matches!(c.body, Body::Chunks(_)) {
                *nested.entry(id(&c.id)).or_default() += 1;
                // Each holds exactly one Utf8 chunk.
                assert_eq!(c.children().len(), 1);
                assert_eq!(c.children()[0].id, *b"Utf8");
            }
        }
    }
    assert!(nested.contains_key("tdsn"), "{nested:?}");
}

#[test]
fn raw_text_lists_are_kept_opaque() {
    // LIST btdk holds raw text, not chunks (observed once a project has a text layer).
    let bytes = fixture("110-text-A.aep");
    let file = parse(&bytes).unwrap();
    let mut found = 0;
    for (_, c) in file.walk() {
        if let Body::OpaqueList { list_type, data } = &c.body {
            assert!(!data.is_empty());
            if list_type == b"btdk" {
                found += 1;
                assert!(data.iter().all(|b| b.is_ascii() || *b >= 0x80), "btdk is text");
            }
        }
    }
    assert!(found >= 1, "expected a LIST btdk kept opaque");
}

#[test]
fn list_types_ending_in_a_space_are_kept() {
    // `LIST Pin ` and `LIST FEE ` occur in every generated project that has an item (all but the
    // empty one); the space is part of the type.
    for name in FIXTURES.iter().filter(|n| **n != "001-empty.aep") {
        let bytes = fixture(name);
        let file = parse(&bytes).unwrap();
        for ty in [b"Pin ", b"FEE "] {
            assert!(file.walk().any(|(_, c)| c.is_list(ty)), "{name}: no LIST {:?}", String::from_utf8_lossy(ty));
        }
    }
}
