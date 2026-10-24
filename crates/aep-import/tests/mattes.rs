//! Track mattes (cases 403-408): every matte fixture imports without losses with the mode and the
//! matte layer it was made with.

mod common;

use common::{comp, fixture_bytes, load, try_import};
use effectcraft_aep_import::{ImportOptions, LossCategory, import};
use effectcraft_project::{MatteKind, Project};

/// The layers of comp `C` as `(name, video switch, matte layer name, matte kind)`, top first.
fn mattes(project: &Project, r: &effectcraft_aep_import::ImportResult) -> Vec<(String, bool, Option<(String, MatteKind)>)> {
    let (_, c) = comp(project, r, "C");
    c.layers
        .iter()
        .map(|l| {
            let m = l.track_matte.map(|tm| (c.layer(tm.layer).map(|x| x.name.clone()).unwrap_or_default(), tm.kind));
            (l.name.clone(), l.switches.video, m)
        })
        .collect()
}

#[test]
fn the_four_modes_import_with_their_matte_layer() {
    for (case, kind) in [
        ("403-matte-L-alpha-M.aep", MatteKind::Alpha),
        ("404-matte-L-alphainv-M.aep", MatteKind::AlphaInverted),
        ("405-matte-L-luma-M.aep", MatteKind::Luma),
        ("406-matte-L-lumainv-M.aep", MatteKind::LumaInverted),
    ] {
        let mut p = Project::default();
        let r = try_import(&mut p, case, false).unwrap_or_else(|e| panic!("{case}: lossless import refused: {e}"));
        assert_eq!(r.report.imported.track_mattes, 1, "{case}");
        let l = mattes(&p, &r);
        assert_eq!(l[0], ("M".to_string(), false, None), "{case}: the matte layer is hidden, as stored");
        assert_eq!(l[1], ("L".to_string(), true, Some(("M".to_string(), kind))), "{case}");
    }
}

#[test]
fn a_matte_below_its_target_and_a_visible_matte() {
    let (p, r) = load("407-matte-M-alpha-L-below.aep");
    assert!(r.report.is_lossless());
    let l = mattes(&p, &r);
    assert_eq!(l[0], ("M".to_string(), true, Some(("L".to_string(), MatteKind::Alpha))));
    assert_eq!(l[1], ("L".to_string(), false, None));

    let (p, r) = load("408-matte-L-alpha-M-visible.aep");
    assert!(r.report.is_lossless());
    let l = mattes(&p, &r);
    assert!(l[0].1, "M stays visible");
    assert_eq!(l[1].2, Some(("M".to_string(), MatteKind::Alpha)));
}

/// Offset of the `u32` matte layer id (`ldta` byte 160) of the layer with this id.
fn matte_id_offset(bytes: &[u8], layer_id: u32) -> usize {
    let mut needle = b"ldta".to_vec();
    needle.extend_from_slice(&164u32.to_be_bytes());
    needle.extend_from_slice(&layer_id.to_be_bytes());
    let at = bytes.windows(needle.len()).position(|w| w == needle.as_slice()).expect("ldta of the layer");
    at + 8 + 160
}

fn import_patched(bytes: &[u8]) -> (Project, effectcraft_aep_import::ImportResult) {
    let mut project = Project::default();
    let none = |_: &str| false;
    let r = import(&mut project, bytes, &ImportOptions { allow_losses: true, ..ImportOptions::new(&none) }).expect("imports");
    (project, r)
}

/// A matte mode without a matte layer id (built in-test from 403; After Effects 2026 always writes
/// the id): the layer directly above is the matte, as in After Effects before 23.0.
#[test]
fn a_matte_without_a_layer_id_uses_the_layer_above() {
    let mut bytes = fixture_bytes("403-matte-L-alpha-M.aep");
    let at = matte_id_offset(&bytes, 0x1b);
    bytes[at..at + 4].copy_from_slice(&0u32.to_be_bytes());
    let (p, r) = import_patched(&bytes);
    assert!(r.report.is_lossless(), "{:?}", r.report.losses);
    assert_eq!(mattes(&p, &r)[1].2, Some(("M".to_string(), MatteKind::Alpha)));

    // On the top layer there is no layer above: reported.
    let mut bytes = fixture_bytes("407-matte-M-alpha-L-below.aep");
    let at = matte_id_offset(&bytes, 0x1d);
    bytes[at..at + 4].copy_from_slice(&0u32.to_be_bytes());
    let (p, r) = import_patched(&bytes);
    assert_eq!(r.report.count(LossCategory::TrackMattes), 1);
    assert_eq!(mattes(&p, &r)[0].2, None);
}

#[test]
fn hostile_matte_fields_are_reported() {
    // A matte layer that does not exist, the layer itself, and an unknown mode.
    for (id, mode) in [(0x7fff_u32, 1u8), (0x1b, 1), (0x1d, 9)] {
        let mut bytes = fixture_bytes("403-matte-L-alpha-M.aep");
        let at = matte_id_offset(&bytes, 0x1b);
        bytes[at..at + 4].copy_from_slice(&id.to_be_bytes());
        bytes[at - 160 + 107] = mode;
        let (p, r) = import_patched(&bytes);
        assert_eq!(r.report.count(LossCategory::TrackMattes), 1, "id {id:#x} mode {mode}");
        assert_eq!(mattes(&p, &r)[1].2, None);
    }
}
