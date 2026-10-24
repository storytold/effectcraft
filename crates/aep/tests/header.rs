//! The layer header (`ldta`) and the composition header (`cdta`, `PRin`), checked against the values
//! the synthetic corpus was made with (its MANIFEST): comp `C` 1920x1080, 25 fps, 5 s, background
//! black, shutter angle 180, one solid layer `L` (red, 1920x1080, label 1, Normal, Best, start 0,
//! in 0, out 5, stretch 100), then one change per file.

mod common;

use common::{first_comp, fixture, layer_with_id, open};
use effectcraft_aep::{BlendMode, LayerHeader, LayerType, Project, Quality, Rational};

const L: u32 = 0x1b;

/// The header of the solid layer `L` in a fixture.
fn l_header(name: &str) -> LayerHeader {
    let bytes = fixture(name);
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let comp = first_comp(&project);
    layer_with_id(&comp, L).header().unwrap()
}

/// The switches that the MANIFEST lists, so that a case can say which one differs from the base.
#[derive(Debug, Clone, PartialEq)]
struct Switches {
    shy: bool,
    locked: bool,
    solo: bool,
    three_d: bool,
    motion_blur: bool,
    collapse: bool,
    adjustment: bool,
    guide: bool,
}

fn switches(h: &LayerHeader) -> Switches {
    Switches {
        shy: h.is_shy(),
        locked: h.is_locked(),
        solo: h.is_solo(),
        three_d: h.is_three_d(),
        motion_blur: h.is_motion_blur(),
        collapse: h.collapses_transformations(),
        adjustment: h.is_adjustment(),
        guide: h.is_guide(),
    }
}

const NONE: Switches =
    Switches { shy: false, locked: false, solo: false, three_d: false, motion_blur: false, collapse: false, adjustment: false, guide: false };

/// Everything the base layer and every one-switch case share.
fn assert_plain(h: &LayerHeader) {
    assert_eq!(h.id, L);
    assert_eq!(h.layer_type, LayerType::Av);
    assert_eq!(h.label, 1);
    // No name was set, so the copy of the name at byte 64 is empty (the layer shows its source name).
    assert_eq!(h.name_copy, "");
    assert!(!h.name_is_user_set());
    assert!(!h.is_null_object());
    assert_eq!(h.parent_id, None);
    assert_eq!(h.source_id, Some(0x1a));
}

#[test]
fn base_layer_header() {
    let h = l_header("000-base.aep");
    assert_plain(&h);
    assert_eq!(h.quality, Quality::Best);
    assert_eq!(h.blend_mode, BlendMode::Normal);
    assert_eq!(h.stretch, 1);
    assert_eq!(h.stretch_percent(), 100.0);
    assert_eq!(switches(&h), NONE);
    // start 0 s, in 0 s, out 5 s; denominator = the composition timescale.
    assert_eq!(h.start, Rational { num: 0, den: 25600 });
    assert_eq!(h.in_relative, Rational { num: 0, den: 25600 });
    assert_eq!(h.out_relative, Rational { num: 128_000, den: 25600 });
    assert_eq!(h.start_seconds(), Some(0.0));
    assert_eq!(h.in_point_seconds(), Some(0.0));
    assert_eq!(h.out_point_seconds(), Some(5.0));
    assert_eq!(h.out_point(), Rational::new(5, 1));
}

#[test]
fn one_switch_per_case() {
    // (file, the switches that case turns on)
    let cases = [
        ("012-layer-shy.aep", Switches { shy: true, ..NONE }),
        ("013-layer-locked.aep", Switches { locked: true, ..NONE }),
        ("014-layer-solo.aep", Switches { solo: true, ..NONE }),
        ("015-layer-3d.aep", Switches { three_d: true, ..NONE }),
        ("017-layer-motionblur.aep", Switches { motion_blur: true, ..NONE }),
        ("024-layer-collapse.aep", Switches { collapse: true, ..NONE }),
        ("025-layer-adjustment.aep", Switches { adjustment: true, ..NONE }),
        ("026-layer-guide.aep", Switches { guide: true, ..NONE }),
    ];
    for (name, expected) in cases {
        let h = l_header(name);
        assert_eq!(switches(&h), expected, "{name}");
        assert_plain(&h);
        assert_eq!((h.quality, h.blend_mode, h.stretch), (Quality::Best, BlendMode::Normal, 1), "{name}");
        assert_eq!(h.out_point_seconds(), Some(5.0), "{name}");
    }
}

#[test]
fn name_blend_mode_and_quality() {
    let h = l_header("010-layer-name.aep");
    assert_eq!(h.name_copy, "Renamed");
    assert!(h.name_is_user_set());
    let bytes = fixture("010-layer-name.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let layer = layer_with_id(&first_comp(&project), L);
    // The layer name is the user's; the source footage keeps its own name.
    assert_eq!(layer.name().as_deref(), Some("Renamed"));
    assert_eq!(project.layer_source(&layer).and_then(|i| i.display_name()).as_deref(), Some("L"));
    assert_eq!(project.layer_display_name(&layer).as_deref(), Some("Renamed"));

    assert_eq!(l_header("016-layer-blend-multiply.aep").blend_mode, BlendMode::Multiply);
    assert_eq!(l_header("023-layer-quality-draft.aep").quality, Quality::Draft);
}

#[test]
fn display_name_falls_back_to_the_source_name() {
    let bytes = fixture("000-base.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let layer = layer_with_id(&first_comp(&project), L);
    assert_eq!(layer.name(), None);
    assert_eq!(project.layer_display_name(&layer).as_deref(), Some("L"));
    assert_eq!(project.layer_source_size(&layer), Some((1920, 1080)));
    let source = project.layer_source(&layer).unwrap();
    assert_eq!(source.id(), Some(0x1a));
    assert!(source.footage().unwrap().is_solid);
}

#[test]
fn time_stretch_and_the_in_out_start_convention() {
    // 018: stretch 100 -> 200: the stored out point stays 5 s, the layer ends at 10 s.
    let h = l_header("018-layer-stretch200.aep");
    assert_eq!(h.stretch, 2);
    assert_eq!(h.stretch_percent(), 200.0);
    assert_eq!(h.out_relative, Rational { num: 128_000, den: 25600 });
    assert_eq!(h.out_point_seconds(), Some(10.0));
    assert_eq!(h.in_point_seconds(), Some(0.0));

    // 019: inPoint 0 -> 1; AE moved the out point to 6 s and left the start at 0.
    let h = l_header("019-layer-inpoint1.aep");
    assert_eq!(h.start_seconds(), Some(0.0));
    assert_eq!(h.in_relative, Rational { num: 25600, den: 25600 });
    assert_eq!(h.in_point(), Rational::new(1, 1));
    assert_eq!(h.out_point_seconds(), Some(6.0));

    // 021: startTime 0 -> 0.5: in 0.5 s and out 5.5 s although the stored in point stays 0.
    let h = l_header("021-layer-starttime0.5.aep");
    assert_eq!(h.start, Rational { num: 12800, den: 25600 });
    assert_eq!(h.start_seconds(), Some(0.5));
    assert_eq!(h.in_relative.num, 0);
    assert_eq!(h.in_point(), Rational::new(1, 2));
    assert_eq!(h.in_point_seconds(), Some(0.5));
    assert_eq!(h.out_point_seconds(), Some(5.5));
    assert_eq!(h.out_point(), Rational::new(11, 2));
}

#[test]
fn parent_link_and_null_object() {
    // 022: a new null layer "Null" (source "Null 1", 100x100) on top; L is its child.
    let bytes = fixture("022-layer-parent-null.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let comp = first_comp(&project);
    let layers: Vec<_> = comp.layers().collect();
    assert_eq!(layers.len(), 2);
    let null = &layers[0];
    let nh = null.header().unwrap();
    assert_eq!(null.name().as_deref(), Some("Null"));
    assert!(nh.is_null_object());
    assert!(nh.name_is_user_set());
    assert_eq!(nh.layer_type, LayerType::Av);
    assert_eq!(nh.parent_id, None);
    assert_eq!(project.layer_source(null).and_then(|i| i.display_name()).as_deref(), Some("Null 1"));
    assert_eq!(project.layer_source_size(null), Some((100, 100)));
    let lh = layers[1].header().unwrap();
    assert_eq!(lh.id, L);
    assert_eq!(lh.parent_id, Some(nh.id));
    assert!(!lh.is_null_object());
}

#[test]
fn layer_header_is_none_when_ldta_is_too_short() {
    assert!(LayerHeader::parse(&[0; 139]).is_none());
    assert!(LayerHeader::parse(&[]).is_none());
    assert!(LayerHeader::parse(&[0; 140]).is_some());
}

#[test]
fn rational_is_exact() {
    assert_eq!(Rational::new(12800, 25600), Some(Rational { num: 1, den: 2 }));
    assert_eq!(Rational::new(-3, -6), Some(Rational { num: 1, den: 2 }));
    assert_eq!(Rational::new(3, -6), Some(Rational { num: -1, den: 2 }));
    assert_eq!(Rational::new(0, 7), Some(Rational { num: 0, den: 1 }));
    assert_eq!(Rational::new(1, 0), None);
    assert_eq!(Rational { num: 1, den: 0 }.to_f64(), None);
    assert_eq!(Rational::new(i64::MIN, 1), None);
}

// ----------------------------------------------------------------------------------- composition

use effectcraft_aep::CompHeader;

fn comp_header(name: &str) -> (CompHeader, Option<String>, Option<(String, String)>) {
    let bytes = fixture(name);
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let comp = first_comp(&project);
    (comp.header().unwrap(), comp.name(), comp.renderer().map(|r| (r.match_name, r.display_name)))
}

#[test]
fn base_comp_header() {
    let (h, name, renderer) = comp_header("000-base.aep");
    assert_eq!(name.as_deref(), Some("C"));
    assert_eq!((h.width, h.height), (1920, 1080));
    assert_eq!((h.time_base.frame_duration, h.time_base.timescale), (1024, 25600));
    assert_eq!(h.frame_rate(), Some(25.0));
    assert_eq!(h.frame_rate_from_fixed(), 25.0);
    assert_eq!(h.duration_units, 128_000);
    assert_eq!(h.duration_seconds(), Some(5.0));
    assert_eq!(h.background, [0, 0, 0]);
    assert_eq!(h.background_rgb(), [0.0, 0.0, 0.0]);
    assert_eq!(h.shutter_angle, 180);
    assert_eq!(h.work_area_start_seconds(), Some(0.0));
    assert_eq!(h.work_area_end_units, None);
    assert_eq!(h.work_area_end_seconds(), Some(5.0));
    assert_eq!(h.pixel_aspect, Rational { num: 1, den: 1 });
    assert_eq!(h.pixel_aspect_ratio(), Some(1.0));
    assert!(!h.motion_blur);
    // The manifest calls the classic renderer `ADBE Advanced 3d`; the file stores `ADBE Escher`
    // and the display name `Classic 3D`.
    assert_eq!(renderer, Some(("ADBE Escher".to_string(), "Classic 3D".to_string())));
}

#[test]
fn one_comp_change_per_case() {
    let (base, _, base_renderer) = comp_header("000-base.aep");

    let (h, _, _) = comp_header("030-comp-size-1280x720.aep");
    assert_eq!((h.width, h.height), (1280, 720));
    assert_eq!(CompHeader { width: 1920, height: 1080, ..h }, base);

    // 031: frameRate 25 -> 30: the frame duration stays 1024, the timescale becomes 30720; the
    // duration stays 5 s (153 600 units).
    let (h, _, _) = comp_header("031-comp-fps30.aep");
    assert_eq!((h.time_base.frame_duration, h.time_base.timescale), (1024, 30720));
    assert_eq!(h.frame_rate(), Some(30.0));
    assert_eq!(h.frame_rate_from_fixed(), 30.0);
    assert_eq!(h.duration_units, 153_600);
    assert_eq!(h.duration_seconds(), Some(5.0));

    // 032: duration 5 -> 10 s; the work area stays the whole composition.
    let (h, _, _) = comp_header("032-comp-duration10.aep");
    assert_eq!(h.duration_units, 256_000);
    assert_eq!(h.duration_seconds(), Some(10.0));
    assert_eq!(h.work_area_end_seconds(), Some(10.0));

    // 033: bgColor [0,0,0] -> [0,1,0].
    let (h, _, _) = comp_header("033-comp-bg-green.aep");
    assert_eq!(h.background, [0, 255, 0]);
    assert_eq!(h.background_rgb(), [0.0, 1.0, 0.0]);

    // 034: shutterAngle 180 -> 90.
    let (h, _, _) = comp_header("034-comp-shutter90.aep");
    assert_eq!(h.shutter_angle, 90);
    assert_eq!(CompHeader { shutter_angle: 180, ..h }, base);

    // 035: workAreaStart 0 -> 1, workAreaDuration 5 -> 2: the end is start + duration = 3 s.
    let (h, _, _) = comp_header("035-comp-workarea-1-2.aep");
    assert_eq!(h.work_area_start_units, 25600);
    assert_eq!(h.work_area_end_units, Some(76_800));
    assert_eq!(h.work_area_start_seconds(), Some(1.0));
    assert_eq!(h.work_area_end_seconds(), Some(3.0));

    // 037: pixelAspect 1 -> 2.
    let (h, _, _) = comp_header("037-comp-pixelaspect2.aep");
    assert_eq!(h.pixel_aspect, Rational { num: 2, den: 1 });
    assert_eq!(h.pixel_aspect_ratio(), Some(2.0));

    // 038: comp motion blur on.
    let (h, _, _) = comp_header("038-comp-motionblur.aep");
    assert!(h.motion_blur);
    assert_eq!(CompHeader { motion_blur: false, ..h }, base);

    // 039: renderer -> Advanced 3D.
    let (h, _, renderer) = comp_header("039-comp-renderer-advanced3d.aep");
    assert_eq!(renderer, Some(("ADBE Calder".to_string(), "Advanced 3D".to_string())));
    assert_ne!(renderer, base_renderer);
    assert_eq!(h, base);
}

#[test]
fn comp_header_is_none_when_cdta_is_too_short() {
    assert!(CompHeader::parse(&[0; 175]).is_none());
    assert!(CompHeader::parse(&[0; 176]).is_some());
}

#[test]
fn precomp_item_ids_and_labels() {
    // 124: L precomposed into "Pre"; the layer in C is a comp-sourced layer labelled 15.
    let bytes = fixture("124-precomp-Pre.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let comps: Vec<_> = project.all_items().into_iter().filter_map(|i| i.comp().map(|c| (i, c))).collect();
    let names: Vec<_> = comps.iter().map(|(_, c)| c.name().unwrap()).collect();
    assert_eq!(names, ["C", "Pre"]);
    let (c_item, c) = &comps[0];
    let (pre_item, pre) = &comps[1];
    let layers: Vec<_> = c.layers().collect();
    assert_eq!(layers.len(), 1);
    let layer = &layers[0];
    let h = layer.header().unwrap();
    assert_eq!(h.source_id, pre_item.id());
    assert_eq!(project.layer_source(layer).unwrap().id(), pre_item.id());
    assert_eq!(h.label, 15);
    assert!(!h.name_is_user_set());
    assert_eq!(layer.name(), None);
    assert_eq!(project.layer_display_name(layer).as_deref(), Some("Pre"));
    assert_eq!(project.layer_source_size(layer), Some((1920, 1080)));
    // The precomp layer's start time has the denominator 1; the Pre comp is 1920x1080, 25 fps, 5 s.
    assert_eq!(h.start.den, 1);
    let ph = pre.header().unwrap();
    assert_eq!((ph.width, ph.height, ph.frame_rate(), ph.duration_seconds()), (1920, 1080, Some(25.0), Some(5.0)));
    // C records that it uses the comp Pre (chunk idpi, a little-endian u32).
    assert_eq!(c_item.used_item_ids(), [pre_item.id().unwrap()]);
    assert!(pre_item.used_item_ids().is_empty());
    assert_eq!(pre_item.label(), Some(15));
    // The layer inside Pre is a new layer made from the solid L (moving the layers into a precomp
    // creates new layers).
    let inner: Vec<_> = pre.layers().collect();
    assert_eq!(inner.len(), 1);
    let ih = inner[0].header().unwrap();
    assert_ne!(ih.id, L);
    assert_eq!(ih.source_id, Some(0x1a));
    assert_eq!(project.layer_display_name(&inner[0]).as_deref(), Some("L"));
}

#[test]
fn a_negative_stretch_is_a_reversed_layer() {
    // 981: Time Reverse (stretch -100 %, start 5 s): numerator -1 (`0xffffffff`), the stored in
    // point 0 and out point 5 s relative, so the layer runs from 5 s back to 0 s; 983: -50 % = -1/2.
    let h = l_header("981-layer-stretch-minus100-start5.aep");
    assert_eq!((h.stretch, h.stretch_den), (-1, 1));
    assert_eq!(h.stretch_percent(), -100.0);
    assert_eq!((h.in_point_seconds(), h.out_point_seconds()), (Some(5.0), Some(0.0)));
    let h = l_header("983-layer-stretch-minus50-opacity-keys.aep");
    assert_eq!((h.stretch, h.stretch_den), (-1, 2));
    assert_eq!((h.in_point_seconds(), h.out_point_seconds()), (Some(5.0), Some(2.5)));
}
