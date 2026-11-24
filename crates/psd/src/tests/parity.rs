//! Parity and recovery tests for PSD/PSB composite alpha, malformed resources,
//! RLE/ZIP plane framing, and save/reopen round-trips.

use crate::write::*;
use crate::*;
use craft_psd::{Compression, ImageResource, Version};

/// Convert an encoded PSD into a PSB (or vice versa) with the specified compression
/// for both layers and the merged image, using `craft_psd::PsdFile`.
fn reencode_with(bytes: &[u8], version: Version, comp: Compression, merged_alpha: bool) -> Vec<u8> {
    let mut file = craft_psd::PsdFile::from_bytes(bytes).expect("parse");
    let depth = file.header.depth;
    let old_ver = file.header.version;
    file.header.version = version;
    file.write_as_psb = version.is_psb();
    if let Some(info) = file.layer_info.as_mut() {
        info.merged_alpha = merged_alpha;
        for rec in &mut info.layers {
            let rects: Vec<(usize, usize)> = rec.channels.iter().map(|ch| rec.channel_rect(ch.id).size().expect("channel rect")).collect();
            for (ch, (w, h)) in rec.channels.iter_mut().zip(rects) {
                if w > 0 && h > 0 {
                    let decoded = ch.decode(w, h, depth, old_ver).expect("decode channel");
                    *ch = craft_psd::ChannelData::encode(ch.id, comp, &decoded, w, h, depth, version).expect("encode channel");
                }
            }
        }
    }
    let decoded_merged = file.image_data.decode(&craft_psd::Header { version: old_ver, ..file.header.clone() }).expect("decode merged");
    file.image_data = craft_psd::ImageData::encode(comp, &decoded_merged, &file.header).expect("encode merged");
    file.to_bytes().expect("write")
}

#[test]
fn psd_and_psb_composite_alpha_flag_controls_alpha_channel() {
    let mut doc = WDoc::new(4, 3);
    doc.layers = vec![WLayer::solid("Semi", Rect::new(0, 0, 4, 3), [0.8, 0.4, 0.2, 0.5])];
    let base = write(&doc);

    for version in [Version::Psd, Version::Psb] {
        let with_alpha = reencode_with(&base, version, Compression::Rle, true);
        let p_alpha = Psd::parse(with_alpha).expect("parse with alpha");
        assert_eq!(p_alpha.psb, version.is_psb());
        assert!(p_alpha.merged_alpha);
        let comp_alpha = p_alpha.composite().expect("composite with alpha");
        assert!((comp_alpha.data[0][3] - 0.5).abs() < 0.02, "{version:?}: {:?}", comp_alpha.data[0]);

        let without_alpha = reencode_with(&base, version, Compression::Rle, false);
        let p_opaque = Psd::parse(without_alpha).expect("parse without alpha");
        assert_eq!(p_opaque.psb, version.is_psb());
        assert!(!p_opaque.merged_alpha);
        let comp_opaque = p_opaque.composite().expect("composite without alpha");
        assert_eq!(comp_opaque.data[0][3], 1.0, "{version:?}: {:?}", comp_opaque.data[0]);
    }
}

#[test]
fn malformed_resources_recover_in_both_psd_and_psb() {
    let mut doc = WDoc::new(6, 4);
    doc.layers = vec![WLayer::solid("L", Rect::new(1, 1, 3, 2), [0.2, 0.6, 0.9, 0.75])];
    let base = write(&doc);

    for version in [Version::Psd, Version::Psb] {
        let mut file = craft_psd::PsdFile::from_bytes(&reencode_with(&base, version, Compression::Rle, true)).expect("parse");
        file.resources.push(ImageResource::new(1005, vec![0; 16]));
        let intact = file.to_bytes().expect("write");
        let want = Psd::parse(intact.clone()).expect("intact parse");

        // Corrupt the first resource's declared length so it overruns the resource section.
        let mut damaged = intact;
        let res_at = 30 + u32::from_be_bytes(damaged[26..30].try_into().expect("cmd len")) as usize;
        let name_len = 1 + usize::from(damaged[res_at + 4 + 6]);
        let len_at = res_at + 4 + 6 + name_len + name_len % 2;
        damaged[len_at..len_at + 4].copy_from_slice(&0x7fff_fff0u32.to_be_bytes());

        let recovered = Psd::parse(damaged).expect("recovered parse");
        assert!(recovered.warnings.is_empty(), "{version:?}: {:?}", recovered.warnings);
        assert_eq!(recovered.psb, version.is_psb());
        assert_eq!(recovered.merged_alpha, want.merged_alpha);
        assert_eq!(recovered.layers.len(), want.layers.len());
        assert_eq!(recovered.composite().expect("comp").data, want.composite().expect("want comp").data);
        assert_eq!(recovered.layer_pixels(0, false).expect("layer").data, want.layer_pixels(0, false).expect("want layer").data);
    }
}

#[test]
fn rle_and_zip_planes_decode_across_psd_and_psb() {
    let mut doc = WDoc::new(8, 6);
    doc.layers =
        vec![WLayer::solid("Base", Rect::new(0, 0, 8, 6), [0.25, 0.5, 0.75, 1.0]), WLayer::solid("Overlay", Rect::new(2, 1, 4, 4), [1.0, 0.25, 0.0, 0.6])];
    let base = write(&doc);
    let reference = Psd::parse(base.clone()).expect("base parse");
    let ref_comp = reference.composite().expect("ref comp").data;
    let ref_l0 = reference.layer_pixels(0, false).expect("ref l0").data;
    let ref_l1 = reference.layer_pixels(1, true).expect("ref l1").data;

    for version in [Version::Psd, Version::Psb] {
        for comp in [Compression::Raw, Compression::Rle, Compression::Zip, Compression::ZipPrediction] {
            let bytes = reencode_with(&base, version, comp, true);
            let p = Psd::parse(bytes).expect("parse");
            assert_eq!(p.psb, version.is_psb());
            assert_eq!(p.composite().expect("comp").data, ref_comp, "{version:?} {comp:?} composite");
            assert_eq!(p.layer_pixels(0, false).expect("l0").data, ref_l0, "{version:?} {comp:?} l0");
            assert_eq!(p.layer_pixels(1, true).expect("l1").data, ref_l1, "{version:?} {comp:?} l1");
        }
    }
}

#[test]
fn save_and_reopen_parity_preserves_psd_and_psb_documents() {
    let mut doc = WDoc::new(10, 8);
    doc.layers = vec![
        WLayer::group_end(),
        WLayer::solid("Child", Rect::new(1, 2, 5, 4), [0.4, 0.8, 0.2, 0.9]).with_blend(b"mul ").with_opacity(200),
        WLayer::group("Folder", true, *b"pass"),
    ];
    let initial = write(&doc);

    for version in [Version::Psd, Version::Psb] {
        for comp in [Compression::Rle, Compression::ZipPrediction] {
            let saved = reencode_with(&initial, version, comp, true);
            let reopened_bytes = craft_psd::PsdFile::from_bytes(&saved).expect("parse saved").to_bytes().expect("resave");
            assert_eq!(reopened_bytes, saved, "{version:?} {comp:?} round-trip bytes");

            let a = Psd::parse(saved).expect("parse first");
            let b = Psd::parse(reopened_bytes).expect("parse reopened");
            assert_eq!(a.psb, b.psb);
            assert_eq!(a.merged_alpha, b.merged_alpha);
            assert_eq!(a.layers.len(), b.layers.len());
            for (la, lb) in a.layers.iter().zip(&b.layers) {
                assert_eq!((&la.name, la.blend, la.opacity, la.section), (&lb.name, lb.blend, lb.opacity, lb.section));
            }
            assert_eq!(a.composite().expect("comp a").data, b.composite().expect("comp b").data);
            assert_eq!(a.layer_pixels(1, true).expect("l1 a").data, b.layer_pixels(1, true).expect("l1 b").data);
        }
    }
}
