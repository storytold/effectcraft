//! Shared helpers for the integration tests.
#![allow(dead_code)]

use std::path::PathBuf;

/// The generated fixtures (own test data, see ATTRIBUTION.md).
pub const FIXTURES: &[&str] = &[
    "000-base.aep",
    "001-empty.aep",
    "010-layer-name.aep",
    "012-layer-shy.aep",
    "013-layer-locked.aep",
    "014-layer-solo.aep",
    "015-layer-3d.aep",
    "016-layer-blend-multiply.aep",
    "017-layer-motionblur.aep",
    "018-layer-stretch200.aep",
    "019-layer-inpoint1.aep",
    "021-layer-starttime0.5.aep",
    "022-layer-parent-null.aep",
    "023-layer-quality-draft.aep",
    "024-layer-collapse.aep",
    "025-layer-adjustment.aep",
    "026-layer-guide.aep",
    "030-comp-size-1280x720.aep",
    "031-comp-fps30.aep",
    "032-comp-duration10.aep",
    "033-comp-bg-green.aep",
    "034-comp-shutter90.aep",
    "035-comp-workarea-1-2.aep",
    "037-comp-pixelaspect2.aep",
    "038-comp-motionblur.aep",
    "039-comp-renderer-advanced3d.aep",
    "040-static-position-100-200.aep",
    "041-static-scale-50.aep",
    "042-static-rotation-45.aep",
    "043-static-opacity-25.aep",
    "044-static-anchor-10-20.aep",
    "052-key-position-2key-ease.aep",
    "053-key-position-2key-hold-key1.aep",
    "054-key-position-2key-spatial-bezier.aep",
    "055-key-position-3key-inserted-t2.5.aep",
    "056-key-position-3key-roving-key2.aep",
    "057-key-position-3key-label9-key2.aep",
    "058-key-opacity-2key.aep",
    "060-expr-opacity-wiggle.aep",
    "071-fx-slider-value77.aep",
    "073-fx-color-0.2-0.6-1.aep",
    "077-fx-dropdown-default.aep",
    "080-fx-dropshadow-default.aep",
    "090-mask-rect.aep",
    "091-mask-rect-props.aep",
    "093-mask-path-2keys.aep",
    "110-text-A.aep",
    "113-text-Hello-size60-fillorange.aep",
    "122-marker-comp-1s.aep",
    "123-marker-layer-1s.aep",
    "124-precomp-Pre.aep",
    "126-footage-png-layer.aep",
    "202-key-out-hold-in-linear.aep",
    "203-key-in-hold-out-linear.aep",
    "204-key-ease-asymmetric.aep",
    "205-key-opacity-ease-asymmetric.aep",
    "206-key-scale-2key-linear.aep",
    "208-key-mask-feather-2key-linear.aep",
    "210-key-fill-color-2key-linear.aep",
    "211-anchor-100x100-solid-10-20.aep",
    "212-anchor-400x300-solid-100-50.aep",
    "213-expr-opacity-wiggle-disabled.aep",
    "214-fx-gaussianblur-default.aep",
    "215-fx-fill-default.aep",
    "216-fx-gaussianblur-40.aep",
    "217-fx-checkbox-on.aep",
    "218-fx-point-111-222.aep",
    "219-fx-angle-90.aep",
    "220-fx-layercontrol-to-layer1.aep",
    "221-fx-slider-2key.aep",
    "222-layer-video-off.aep",
    "223-layer-audio-off.aep",
    "224-layer-effects-off.aep",
    "225-layer-blend-screen.aep",
    "226-layer-blend-add.aep",
    "227-layer-blend-overlay.aep",
    "228-layer-blend-difference.aep",
    "229-comp-fps-29.97.aep",
    "230-comp-fps-23.976.aep",
    "231-comp-bg-grey-0.5.aep",
    "232-mask-ellipse.aep",
    "233-mask-open-path.aep",
    "234-mask-mode-intersect.aep",
    "235-mask-mode-lighten.aep",
    "236-mask-mode-darken.aep",
    "237-mask-mode-difference.aep",
    "238-shape-rect-rounded-corners.aep",
    "239-shape-stroke-green-w8-round.aep",
    "240-shape-custom-path-tangents.aep",
    "241-shape-gradient-fill-linear.aep",
    "242-shape-group-position-50-50.aep",
    "243-text-multiline-A-CR-B.aep",
    "245-text-box-400x200-Hello.aep",
    "246-text-box-center-align.aep",
    "247-text-tracking-100.aep",
    "248-text-range-fill-red-1-3.aep",
    "249-marker-layer-full.aep",
    "251-camera-one-node.aep",
    "252-light-point.aep",
    "253-camera-position-100-200-m1000.aep",
    "254-light-spot-intensity-50.aep",
    "256-camera-two-node-default.aep",
    "259-solid-blue-0-0-1.aep",
    "299-save-noise.aep",
    "300-fx-gaussianblur-0-explicit.aep",
    "301-fx-gaussianblur-40-then-25.aep",
    "302-fx-fill-opacity-100-explicit.aep",
    "303-fx-fill-opacity-50-then-100.aep",
    "304-fx-slider-0-explicit.aep",
    "305-key-scale-bezier-ease.aep",
    "306-key-maskfeather-bezier-ease.aep",
    "307-key-fillcolor-bezier.aep",
    "309-solid-100x100-mask-rect.aep",
    "310-solid-100x100-pointcontrol.aep",
    "311-position-default-explicit.aep",
    "316-layer-stretch50.aep",
    "317-layer-stretch150.aep",
    "318-layer-inpoint-0.3333.aep",
    "319-precomp-timeremap.aep",
    "320-fx-layercontrol-self-explicit.aep",
    "321-fx-layercontrol-none.aep",
    "323-fx-dropdown-5items.aep",
    "324-fx-dropdown-5items-select3.aep",
    "325-comp-pixelaspect-0.9091.aep",
    "326-comp-shutterphase-m90.aep",
    "327-comp-displaystartframe-100.aep",
    "328-mask-two-ellipse-rect.aep",
    "329-mask-ellipse-mode-none.aep",
    "330-shape-polystar-star-5-50-100.aep",
    "331-shape-polystar-polygon-5-100.aep",
    "332-shape-gradient-fill-radial.aep",
    "333-shape-group-two-rects.aep",
    "334-text-box-right-align.aep",
    "335-text-point-fauxbold.aep",
    "336-text-point-fauxitalic.aep",
    "337-text-point-allcaps.aep",
    "338-text-point-smallcaps.aep",
    "339-text-point-leading60.aep",
    "340-text-range-two-styles.aep",
    "341-camera-zoom-2000.aep",
    "342-camera-dof-800-30.aep",
    "343-light-spot-color-red.aep",
    "344-light-spot-coneangle-60.aep",
    "345-light-spot-conefeather-30.aep",
    "347-marker-layer-label3.aep",
    "348-marker-layer-navigation-cue.aep",
    "349-marker-layer-event-cue-params.aep",
    "350-footage-wav-imported.aep",
    "351-footage-wav-as-layer.aep",
    "352-footage-pngseq-imported.aep",
    "354-footage-pngseq-timeremap.aep",
];

pub fn fixture(name: &str) -> Vec<u8> {
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "tests", "fixtures", name].iter().collect();
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

use effectcraft_aep::{AepFile, Comp, Layer, LayerType, Project, parse};

/// Parses a fixture; the bytes must outlive the file, so the caller keeps them.
pub fn open(bytes: &[u8]) -> AepFile<'_> {
    parse(bytes).expect("fixture parses")
}

/// The first composition of a project (comp `C` in the corpus; in 124 the precomp `Pre` comes later).
pub fn first_comp<'f, 'a>(project: &Project<'f, 'a>) -> Comp<'f, 'a> {
    project.all_items().into_iter().find_map(|i| i.comp()).expect("a composition")
}

/// The layer with this id in the first composition (the solid `L` of the corpus has id 0x1b).
pub fn layer_with_id<'f, 'a>(comp: &Comp<'f, 'a>, id: u32) -> Layer<'f, 'a> {
    comp.layers().find(|l| l.id() == Some(id)).unwrap_or_else(|| panic!("no layer with id {id:#x}"))
}

/// The layer with this user-set name in a composition.
pub fn layer_named<'f, 'a>(comp: &Comp<'f, 'a>, name: &str) -> Layer<'f, 'a> {
    comp.layers().find(|l| l.name().as_deref() == Some(name)).unwrap_or_else(|| panic!("no layer named {name}"))
}

/// The layer that shows this name: its own name, or its source's while it has none.
pub fn layer_shown_as<'f, 'a>(project: &Project<'f, 'a>, comp: &Comp<'f, 'a>, name: &str) -> Layer<'f, 'a> {
    comp.layers().find(|l| project.layer_display_name(l).as_deref() == Some(name)).unwrap_or_else(|| panic!("no layer shown as {name}"))
}

/// The first layer of this type in a composition.
pub fn layer_of_type<'f, 'a>(comp: &Comp<'f, 'a>, layer_type: LayerType) -> Layer<'f, 'a> {
    comp.layers().find(|l| l.header().is_some_and(|h| h.layer_type == layer_type)).unwrap_or_else(|| panic!("no {layer_type:?} layer"))
}

/// Compares floats with a tolerance.
pub fn near(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

pub fn assert_near(got: &[f64], want: &[f64], tol: f64) {
    assert_eq!(got.len(), want.len(), "{got:?} vs {want:?}");
    for (g, w) in got.iter().zip(want) {
        assert!(near(*g, *w, tol), "{got:?} != {want:?}");
    }
}
