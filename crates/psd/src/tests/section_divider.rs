use super::*;

fn parse_section_blocks(blocks: Vec<([u8; 4], Vec<u8>)>) -> (Section, [u8; 4]) {
    let mut doc = WDoc::new(1, 1);
    doc.layers = vec![WLayer::empty("Section", blocks).with_blend(b"mul ")];
    let layer = Psd::parse(write(&doc)).unwrap().layers.remove(0);
    (layer.section, layer.blend)
}

#[test]
fn section_prefixes_and_invalid_optional_signatures_preserve_prior_state() {
    let full = [0, 0, 0, 2, b'8', b'B', b'I', b'M', b'z', b'z', b'z', b'z', 0, 0, 0, 7, 255];
    for key in [*b"lsct", *b"lsdk"] {
        for len in 0..=17 {
            let data = full[..len].to_vec();
            assert_eq!(
                parse_section_blocks(vec![(key, data.clone())]),
                (if len < 4 { Section::Layer } else { Section::ClosedFolder }, if len < 12 { *b"mul " } else { *b"zzzz" }),
                "{key:?} length {len}"
            );
            let previous = vec![0, 0, 0, 1, b'8', b'B', b'I', b'M', b's', b'c', b'r', b'n'];
            let other = if key == *b"lsct" { *b"lsdk" } else { *b"lsct" };
            assert_eq!(
                parse_section_blocks(vec![(other, previous), (key, data.clone())]),
                (if len < 4 { Section::OpenFolder } else { Section::ClosedFolder }, if len < 12 { *b"scrn" } else { *b"zzzz" }),
            );
            if len >= 12 {
                let mut bad = data;
                bad[4..8].copy_from_slice(b"nope");
                assert_eq!(parse_section_blocks(vec![(key, bad)]), (Section::ClosedFolder, *b"mul "));
            }
        }
    }
}

#[test]
fn section_unknown_kinds_and_sequential_keys_keep_blend_until_replaced() {
    for kind in [0u32, 1, 2, 3, 99, u32::MAX] {
        let expected = match kind {
            1 => Section::OpenFolder,
            2 => Section::ClosedFolder,
            3 => Section::Divider,
            _ => Section::Layer,
        };
        for keys in [[*b"lsct", *b"lsdk"], [*b"lsdk", *b"lsct"]] {
            let previous = vec![0, 0, 0, 1, b'8', b'B', b'I', b'M', b'z', b'z', b'z', b'z'];
            let mut bad = kind.to_be_bytes().to_vec();
            bad.extend_from_slice(b"nopepass");
            bad.extend_from_slice(&7u32.to_be_bytes());
            assert_eq!(parse_section_blocks(vec![(keys[0], previous), (keys[1], bad)]), (expected, *b"zzzz"));
        }
    }
}
