use super::*;

// Independent wire fixtures: do not use the shared path encoder to test its decoder.
fn record(selector: u16, data: [u8; 24]) -> Vec<u8> {
    let mut out = selector.to_be_bytes().to_vec();
    out.extend_from_slice(&data);
    out
}

fn length(selector: u16, count: u16) -> Vec<u8> {
    let mut data = [0; 24];
    data[..2].copy_from_slice(&count.to_be_bytes());
    record(selector, data)
}

fn knot(selector: u16) -> Vec<u8> {
    let mut data = [0; 24];
    // Stored y then x, including negative and outside-document coordinates.
    for (chunk, value) in data.chunks_exact_mut(4).zip([-(1i32 << 23), 1 << 22, 1 << 24, -(1 << 24), 1 << 23, 1 << 25]) {
        chunk.copy_from_slice(&value.to_be_bytes());
    }
    record(selector, data)
}

fn parse_mask(key: [u8; 4], bytes: Vec<u8>) -> Option<VectorMask> {
    let mut doc = WDoc::new(64, 32);
    doc.layers = vec![WLayer::empty("mask", vec![(key, bytes)])];
    Psd::parse(write(&doc)).unwrap().layers.remove(0).vector_mask
}

fn header(version: u32, flags: u32) -> Vec<u8> {
    [version.to_be_bytes(), flags.to_be_bytes()].concat()
}

#[test]
fn vector_masks_preserve_record_recovery_and_dimension_mapping() {
    for key in [*b"vmsk", *b"vsms"] {
        for version in [3, 99] {
            for tail in 0..26 {
                let mut bytes = header(version, 1 | 2 | 4 | 0x80);
                bytes.extend(knot(1)); // Orphan knots are ignored, not made into a path.
                bytes.extend(length(0, u16::MAX)); // Empty subpath is removed.
                bytes.extend(length(3, 0)); // Counts do not control app grouping.
                for selector in [1, 2, 4, 5] {
                    bytes.extend(knot(selector)); // Keep mixed selectors in the open path.
                }
                bytes.extend(record(0xffff, [0xff; 24]));
                bytes.extend(length(0, 1));
                bytes.extend(knot(4));
                bytes.extend(knot(2)); // More knots than declared still belong to this path.
                bytes.extend(length(3, 1)); // Empty final path is removed too.
                bytes.extend(std::iter::repeat_n(0xff, tail));
                let mask = parse_mask(key, bytes).unwrap();
                assert!(mask.inverted && mask.disabled);
                assert_eq!(mask.subpaths.len(), 2);
                assert!(!mask.subpaths[0].closed);
                assert!(mask.subpaths[1].closed);
                assert_eq!(mask.subpaths[0].knots.len(), 4);
                assert_eq!(mask.subpaths[1].knots.len(), 2);
                for path in mask.subpaths {
                    for points in path.knots {
                        assert_eq!(points, [[16.0, -16.0], [-64.0, 32.0], [128.0, 16.0]]);
                    }
                }
            }
        }
    }
}

#[test]
fn vector_masks_preserve_short_header_and_empty_path_recovery() {
    for key in [*b"vmsk", *b"vsms"] {
        let bytes = header(3, 0);
        for len in 0..8 {
            assert!(parse_mask(key, bytes[..len].to_vec()).is_none());
        }
        for records in [vec![], knot(5), length(3, u16::MAX), record(8, [0xff; 24])] {
            let mut bytes = header(3, 2 | 0x80);
            bytes.extend(records);
            let mask = parse_mask(key, bytes).unwrap();
            assert!(!mask.inverted && !mask.disabled);
            assert!(mask.subpaths.is_empty());
        }
    }
}
