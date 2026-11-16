//! The reader over craft-psd's generated corpus (every colour mode, depth, compression and PSB):
//! the model matches the shared reader's records and its channel ranges hold the stored bytes.

use crate::*;

#[test]
fn every_generated_case_reads_like_craft_psd() {
    let mut decoded = 0;
    for (i, case) in craft_psd::testgen::all_cases().into_iter().enumerate() {
        let bytes = case.file.to_bytes().expect("write");
        let shared = craft_psd::PsdFile::from_bytes(&bytes).expect("shared parse");
        let p = match Psd::parse(bytes.clone()) {
            Ok(p) => p,
            // Modes this reader does not show are refused cleanly, never half-read.
            Err(Error::Unsupported(_)) => continue,
            Err(e) => panic!("case {i}: {e}"),
        };
        assert_eq!(
            (p.width, p.height, p.depth, p.psb),
            (shared.header.width, shared.header.height, shared.header.depth, shared.header.version.is_psb()),
            "case {i}"
        );
        let records = shared.layer_info.as_ref().map_or(&[][..], |l| &l.layers[..]);
        assert_eq!(p.layers.len(), records.len(), "case {i}");
        for (l, rec) in p.layers.iter().zip(records) {
            assert_eq!(l.blend, rec.blend_mode.key(), "case {i}");
            assert_eq!(l.channels.len(), rec.channels.len(), "case {i}");
            for (c, rc) in l.channels.iter().zip(&rec.channels) {
                assert_eq!(c.id, rc.id, "case {i}");
                // The range covers the compression field and the stored bytes after it.
                assert!(c.len == 0 || c.len == rc.data.len() + 2, "case {i}");
                assert!(c.len == 0 || bytes[c.offset + 2..c.offset + c.len] == rc.data[..], "case {i}");
            }
        }
        // Decoding reads those ranges: it ends in pixels or a clean error, as before the cutover.
        if p.composite().is_ok() {
            decoded += 1;
        }
        for li in 0..p.layers.len() {
            let _ = p.layer_pixels(li, false);
        }
    }
    assert!(decoded > 100, "{decoded} composites decoded");
}

#[test]
fn a_damaged_layer_section_keeps_the_merged_image() {
    let case = craft_psd::testgen::all_cases().into_iter().find(|c| c.file.layer_info.as_ref().is_some_and(|l| !l.layers.is_empty())).expect("a layered case");
    let mut bytes = case.file.to_bytes().expect("write");
    let head = craft_psd::head::HeadView::from_bytes(&bytes).expect("head");
    // Overwrite the first layer record's signature.
    let at = head.layer_section_offset as usize + if head.header.version.is_psb() { 16 } else { 8 } + 2 + 16;
    let channels = usize::from(u16::from_be_bytes([bytes[at], bytes[at + 1]]));
    let sig = at + 2 + channels * if head.header.version.is_psb() { 10 } else { 6 };
    assert_eq!(&bytes[sig..sig + 4], b"8BIM");
    bytes[sig..sig + 4].copy_from_slice(b"XXXX");
    let p = Psd::parse(bytes).expect("flattened");
    assert!(p.layers.is_empty());
    assert!(p.warnings.iter().any(|w| w.contains("Layers could not be read")), "{:?}", p.warnings);
    assert!(p.composite().is_ok());
}

/// The first generated case that is layered and passes `keep`, with its bytes.
fn layered(keep: impl Fn(&craft_psd::PsdFile) -> bool) -> Vec<u8> {
    let case = craft_psd::testgen::all_cases()
        .into_iter()
        .find(|c| c.file.layer_info.as_ref().is_some_and(|l| !l.layers.is_empty()) && keep(&c.file))
        .expect("a layered case");
    case.file.to_bytes().expect("write")
}

#[test]
fn a_damaged_resource_keeps_the_layers() {
    let intact = layered(|f| f.header.depth == 8 && !f.resources.is_empty());
    let want = Psd::parse(intact.clone()).expect("intact");
    let mut bytes = intact;
    // The first resource: signature, id, padded Pascal name, then its length, made to run off the end.
    let res_at = 30 + u32::from_be_bytes(bytes[26..30].try_into().unwrap()) as usize;
    let name = 1 + usize::from(bytes[res_at + 4 + 6]);
    let len_at = res_at + 4 + 6 + name + name % 2;
    bytes[len_at..len_at + 4].copy_from_slice(&0x7fff_fff0u32.to_be_bytes());
    let p = Psd::parse(bytes).expect("parse");
    assert!(p.warnings.is_empty(), "{:?}", p.warnings);
    assert_eq!(p.layers.len(), want.layers.len());
    assert_eq!(p.merged_alpha, want.merged_alpha);
    assert_eq!(p.composite().expect("composite").data, want.composite().expect("composite").data);
    for i in 0..p.layers.len() {
        assert_eq!(p.layers[i].name, want.layers[i].name);
        assert_eq!(p.layer_pixels(i, false).map(|l| l.data).ok(), want.layer_pixels(i, false).map(|l| l.data).ok(), "layer {i}");
    }
}

#[test]
fn a_damaged_16_bit_layer_block_is_reported() {
    let intact = layered(|f| f.header.depth == 16 && matches!(f.layer_info_placement, craft_psd::LayerInfoPlacement::GlobalBlock { .. }));
    let want = Psd::parse(intact.clone()).expect("intact");
    assert!(want.merged_alpha, "the generated 16-bit cases carry merged alpha");
    let mut bytes = intact;
    let at = bytes.windows(8).position(|w| w == b"8BIMLr16").expect("Lr16 block");
    let psb = want.psb;
    // Block header, then the layer count and the first record's rect: its channel count follows.
    let channels = at + 8 + if psb { 8 } else { 4 } + 2 + 16;
    bytes[channels..channels + 2].copy_from_slice(&u16::MAX.to_be_bytes());
    let p = Psd::parse(bytes).expect("flattened");
    assert!(p.layers.is_empty());
    assert!(p.warnings.iter().any(|w| w.contains("Layers could not be read")), "{:?}", p.warnings);
    assert_eq!(p.merged_alpha, want.merged_alpha);
    assert_eq!(p.composite().expect("composite").data, want.composite().expect("composite").data);
}
