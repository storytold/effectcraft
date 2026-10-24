//! Bezier ease the importer decodes is written back: Anchor Point keys (597, 730: speed in pixels
//! per second over the source height), effect point keys (598) and mask path keys (599, 700:
//! the ease at bytes 24..55 of the 64-byte record). The key records of the export equal After
//! Effects' in time, interpolation and ease.

mod common;

use common::*;
use effectcraft_aep::{Body, Chunk, parse};
use effectcraft_aep_export::LossCategory;
use effectcraft_aep_import::{ImportOptions, import};
use effectcraft_project::Project;

fn fixture(name: &str) -> Vec<u8> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../aep/tests/fixtures");
    std::fs::read(dir.join(name)).expect(name)
}

/// The key records (`ldat`) of the first property named `name` below a layer, split into records.
fn key_records(bytes: &[u8], name: &str) -> Vec<Vec<u8>> {
    fn walk(cs: &[Chunk<'_>], name: &str) -> Option<Vec<Vec<u8>>> {
        for (i, c) in cs.iter().enumerate() {
            if c.is(b"tdmn") && matches!(&c.body, Body::Data(d) if d.split(|x| *x == 0).next() == Some(name.as_bytes())) {
                let node = cs.get(i + 1)?;
                // A property record, or the mask shape inside its atom.
                let tdbs = if node.is_list(b"tdbs") { Some(node) } else { node.children().iter().find(|k| k.is_list(b"tdbs")) };
                if let Some(list) = tdbs.and_then(|t| t.child_list(b"list")) {
                    let size =
                        list.child(b"lhd3").and_then(|c| c.bytes()).and_then(|b| b.get(16..20)).map(|b| u32::from_be_bytes(b.try_into().unwrap()) as usize)?;
                    let ldat = list.child(b"ldat").and_then(|c| c.bytes())?;
                    return Some(ldat.chunks(size).map(<[u8]>::to_vec).collect());
                }
            }
            if let Some(r) = walk(c.children(), name) {
                return Some(r);
            }
        }
        None
    }
    let f = parse(bytes).expect("parse");
    walk(&f.chunks, name).unwrap_or_default()
}

#[test]
fn decoded_bezier_ease_is_written_as_after_effects_wrote_it() {
    for (name, prop) in [
        ("597-key-anchor-ease-out50-40-in30-60.aep", "ADBE Anchor Point"),
        ("730-key-anchor-ease-on-400x800-solid.aep", "ADBE Anchor Point"),
        ("598-key-ramp-point-ease-color-bezier.aep", "ADBE Ramp-0001"),
        ("599-key-mask-path-ease-infl75.aep", "ADBE Mask Shape"),
        ("700-key-mask-path-ease-out20-in50.aep", "ADBE Mask Shape"),
    ] {
        let bytes = fixture(name);
        let mut p = Project::default();
        let none = |_: &str| false;
        import(&mut p, &bytes, &ImportOptions { allow_losses: true, ..ImportOptions::new(&none) }).expect("import");
        let r = export_all(&p);
        assert_eq!(r.report.count(LossCategory::ApproximatedKeyframes), 0, "{name}: {:?}", r.report.losses);
        let (a, b) = (key_records(&bytes, prop), key_records(&r.bytes, prop));
        assert!(a.len() >= 2, "{name}: {}", a.len());
        assert_eq!(a.len(), b.len(), "{name}");
        let f = |r: &[u8], at: usize| f64::from_be_bytes(r[at..at + 8].try_into().unwrap());
        for (i, (x, y)) in a.iter().zip(&b).enumerate() {
            assert_eq!(x[..6], y[..6], "{name} key {i}: time and interpolation");
            // The used sides: the out side of the first key, the in side of the last.
            let sides: &[usize] = if i == 0 { &[40, 48] } else { &[24, 32] };
            for at in sides {
                assert!((f(x, *at) - f(y, *at)).abs() < 1e-9, "{name} key {i} @{at}: {} vs {}", f(x, *at), f(y, *at));
            }
        }
    }
}
