//! Importing the `.aepx` of a case gives the same project and report as importing its `.aep`.
//!
//! The two files of a pair hold the same chunk tree apart from the save counter in `head` (see
//! `crates/aep/tests/aepx.rs`), which the importer does not read, so the results are equal, not
//! only similar.

mod common;

use common::fixture_bytes;
use effectcraft_aep_import::{AepImportError, ImportOptions, analyze, import};
use effectcraft_project::Project;

const PAIRS: &[&str] = &["000-base", "058-key-opacity-2key", "060-expr-opacity-wiggle", "110-text-A", "124-precomp-Pre"];

fn import_bytes(bytes: &[u8]) -> (Project, effectcraft_aep_import::ImportResult) {
    let mut project = Project::default();
    let none = |_: &str| false;
    let opts = ImportOptions { allow_losses: true, ..ImportOptions::new(&none) };
    let r = import(&mut project, bytes, &opts).unwrap();
    (project, r)
}

#[test]
fn aepx_imports_like_the_aep() {
    for name in PAIRS {
        let aep = fixture_bytes(&format!("{name}.aep"));
        let aepx = fixture_bytes(&format!("{name}.aepx"));
        let (pa, ra) = import_bytes(&aep);
        let (px, rx) = import_bytes(&aepx);
        assert_eq!(ra, rx, "{name}: import result and report");
        assert_eq!(pa, px, "{name}: project");
        assert!(!ra.comps.is_empty(), "{name}");
        let none = |_: &str| false;
        assert_eq!(analyze(&aep, &none).unwrap(), analyze(&aepx, &none).unwrap(), "{name}: dry run");
    }
}

#[test]
fn lossless_aepx_imports_without_allowing_losses() {
    // The base project imports completely, so the refusal rule applies to `.aepx` as to `.aep`.
    let mut project = Project::default();
    let none = |_: &str| false;
    let r = import(&mut project, &fixture_bytes("000-base.aepx"), &ImportOptions::new(&none)).unwrap();
    assert!(r.report.is_lossless());
    // The text layer of 110 is imported (with its Source Text), so it needs no `allow_losses`
    // either.
    let mut project = Project::default();
    let r = import(&mut project, &fixture_bytes("110-text-A.aepx"), &ImportOptions::new(&none)).unwrap();
    assert!(r.report.is_lossless(), "{:?}", r.report.losses);
    assert_eq!(r.report.imported.text_layers, 1);
}

#[test]
fn damaged_aepx_is_an_error_not_a_panic() {
    let aepx = fixture_bytes("058-key-opacity-2key.aepx");
    let none = |_: &str| false;
    for cut in (0..aepx.len()).step_by(4999) {
        let _ = analyze(&aepx[..cut], &none);
    }
    let mut state = 0x9e37_79b9_u32;
    for _ in 0..200 {
        let mut damaged = aepx.clone();
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        let at = state as usize % damaged.len();
        damaged[at] = damaged[at].wrapping_add(1 + (state >> 24) as u8 % 7);
        let _ = analyze(&damaged, &none);
    }
    assert!(matches!(analyze(b"<?xml version=\"1.0\"?><Other/>", &none), Err(AepImportError::Container(_))));
    assert!(matches!(analyze(b"<AfterEffectsProject/>", &none), Err(AepImportError::NoProject)));
}

/// Every pair of the local corpus (not in the repository): `AEP_CORPUS=<folder> cargo test
/// --release -p effectcraft-aep-import --test aepx -- --ignored --nocapture`.
#[test]
#[ignore = "needs the local corpus (AEP_CORPUS)"]
fn every_corpus_pair_imports_alike() {
    let Ok(dir) = std::env::var("AEP_CORPUS") else { return };
    let mut paths: Vec<_> =
        std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "aepx")).collect();
    paths.sort();
    let none = |_: &str| false;
    let mut same = 0;
    for x in &paths {
        let aepx = std::fs::read(x).unwrap();
        let aep = std::fs::read(x.with_extension("aep")).unwrap();
        let opts = ImportOptions { allow_losses: true, ..ImportOptions::new(&none) };
        let (mut pa, mut px) = (Project::default(), Project::default());
        let ra = import(&mut pa, &aep, &opts).map_err(|e| e.to_string());
        let rx = import(&mut px, &aepx, &opts).map_err(|e| e.to_string());
        if ra == rx && pa == px {
            same += 1;
        } else {
            println!("{}: differs", x.display());
        }
    }
    println!("{same} of {} pairs import alike", paths.len());
}
