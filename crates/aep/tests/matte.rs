//! Track mattes (cases 402-408): mode and matte layer in the layer header.

mod common;

use common::{first_comp, fixture, open};
use effectcraft_aep::{MatteMode, Project, TrackMatte};

/// `(name, video switch, track matte)` of every layer of comp `C`, top first.
fn layers(name: &str) -> Vec<(String, bool, Option<TrackMatte>, u32)> {
    let bytes = fixture(name);
    let file = open(&bytes);
    let project = Project::new(&file).expect("project");
    let comp = first_comp(&project);
    comp.layers()
        .map(|l| {
            let h = l.header().expect("header");
            (project.layer_display_name(&l).unwrap_or_default(), h.is_video_enabled(), l.track_matte(), h.id)
        })
        .collect()
}

#[test]
fn the_four_matte_modes_and_the_matte_layer_id() {
    for (case, mode) in [
        ("403-matte-L-alpha-M.aep", MatteMode::Alpha),
        ("404-matte-L-alphainv-M.aep", MatteMode::AlphaInverted),
        ("405-matte-L-luma-M.aep", MatteMode::Luma),
        ("406-matte-L-lumainv-M.aep", MatteMode::LumaInverted),
    ] {
        let l = layers(case);
        assert_eq!(l.len(), 2, "{case}");
        let (m_name, m_video, m_matte, m_id) = &l[0];
        let (l_name, l_video, l_matte, _) = &l[1];
        assert_eq!((m_name.as_str(), l_name.as_str()), ("M", "L"), "{case}");
        assert_eq!(*m_id, 0x1d, "{case}");
        assert_eq!(*m_matte, None, "{case}: the matte layer has no matte itself");
        assert_eq!(*l_matte, Some(TrackMatte { mode, layer: Some(0x1d) }), "{case}");
        // After Effects switches the matte layer's video off by itself.
        assert!(!m_video, "{case}");
        assert!(l_video, "{case}");
    }
}

#[test]
fn a_matte_layer_below_its_target() {
    let l = layers("407-matte-M-alpha-L-below.aep");
    assert_eq!(l[0].2, Some(TrackMatte { mode: MatteMode::Alpha, layer: Some(l[1].3) }));
    assert_eq!(l[1].3, 0x1b);
    assert!(l[0].1, "M stays visible");
    assert!(!l[1].1, "L, the matte, is switched off");
    assert_eq!(l[1].2, None);
}

#[test]
fn a_matte_layer_whose_video_was_switched_back_on() {
    let l = layers("408-matte-L-alpha-M-visible.aep");
    assert!(l[0].1, "M visible");
    assert_eq!(l[1].2, Some(TrackMatte { mode: MatteMode::Alpha, layer: Some(0x1d) }));
}

#[test]
fn no_layer_without_a_matte_reads_one() {
    for name in common::FIXTURES.iter().filter(|n| !n.contains("-matte-")) {
        let bytes = fixture(name);
        let file = open(&bytes);
        let Some(project) = Project::new(&file) else { continue };
        for item in project.all_items() {
            let Some(comp) = item.comp() else { continue };
            for layer in comp.layers() {
                assert_eq!(layer.track_matte(), None, "{name}");
            }
        }
    }
}

#[test]
fn modes_from_codes() {
    assert_eq!(MatteMode::from_code(0), None);
    assert_eq!(MatteMode::from_code(9), Some(MatteMode::Other(9)));
}
