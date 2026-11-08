//! Layer switches, blend modes, composition frame rates and background, cameras, lights and solid
//! colours, checked against the values the corpus was made with (its MANIFEST, batch 2).

use crate::container::helpers::{assert_near, base_layer, first_comp, layer_of_type, layer_shown_as, open};
use effectcraft_aep::{BlendMode, CompHeader, LayerHeader, LayerType, LightType, Project};

fn l_header(name: &str) -> Option<LayerHeader> {
    Some({
        let bytes = crate::common::try_fixture(name)?;
        let file = open(&bytes);
        let project = Project::new(&file).unwrap();
        base_layer(&first_comp(&project)).header().unwrap()
    })
}

fn comp_header(name: &str) -> Option<CompHeader> {
    Some({
        let bytes = crate::common::try_fixture(name)?;
        let file = open(&bytes);
        let project = Project::new(&file).unwrap();
        first_comp(&project).header().unwrap()
    })
}

#[test]
fn video_audio_and_effects_switches_are_bits_1_2_and_4_of_byte_39() {
    // The base has all three on (byte 39 = 07); 222 turns the video switch off (06), 223 the audio switch
    // (05), 224 the effects switch (03). Each case changes only that byte.
    for (name, byte, video, audio, effects) in [
        ("000-base.aep", 0x07, true, true, true),
        ("222-layer-video-off.aep", 0x06, false, true, true),
        ("223-layer-audio-off.aep", 0x05, true, false, true),
        ("224-layer-effects-off.aep", 0x03, true, true, false),
    ] {
        let h = skip!(l_header(name));
        assert_eq!(h.flag_bytes[2], byte, "{name}");
        assert_eq!((h.is_video_enabled(), h.is_audio_enabled(), h.is_effects_enabled()), (video, audio, effects), "{name}");
        assert_eq!(h.is_enabled(), video, "{name}");
        // The other switches stay as in the base.
        assert!(!h.is_shy() && !h.is_locked() && !h.is_motion_blur() && !h.collapses_transformations(), "{name}");
    }
    let bytes = fixture!("222-layer-video-off.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let layer = base_layer(&first_comp(&project));
    assert_eq!((layer.is_enabled(), layer.is_audio_enabled(), layer.is_effects_enabled()), (Some(false), Some(true), Some(true)));
    // Only byte 39 of ldta differs from the base layer (besides the layer id, bytes 0-3: the two
    // are layers of one combined file).
    let base_bytes = fixture!("000-base.aep");
    let base_file = open(&base_bytes);
    let base_project = Project::new(&base_file).unwrap();
    let base_ldta = base_layer(&first_comp(&base_project)).chunk().child(b"ldta").unwrap().bytes().unwrap().to_vec();
    let ldta = layer.chunk().child(b"ldta").unwrap().bytes().unwrap();
    let differing: Vec<_> = (4..ldta.len()).filter(|i| ldta[*i] != base_ldta[*i]).collect();
    assert_eq!(differing, [39]);
}

#[test]
fn blend_mode_codes() {
    // Normal 2 (base), Multiply 5 (016), Screen 6 (225), Add 4 (226), Overlay 7 (227), Difference 26 (228).
    for (name, mode, code) in [
        ("000-base.aep", BlendMode::Normal, 2),
        ("016-layer-blend-multiply.aep", BlendMode::Multiply, 5),
        ("225-layer-blend-screen.aep", BlendMode::Screen, 6),
        ("226-layer-blend-add.aep", BlendMode::Add, 4),
        ("227-layer-blend-overlay.aep", BlendMode::Overlay, 7),
        ("228-layer-blend-difference.aep", BlendMode::Difference, 26),
    ] {
        let h = skip!(l_header(name));
        assert_eq!((h.blend_mode, h.blend_mode.code()), (mode, code), "{name}");
        assert_eq!(BlendMode::from_code(code), mode);
    }
    // A mode that was not sampled keeps its raw code.
    assert_eq!(BlendMode::from_code(30), BlendMode::Other(30));
    assert_eq!(BlendMode::Other(30).code(), 30);
}

#[test]
fn ntsc_frame_rates_are_exact_fractions() {
    // 229: frameRate 29.97 -> read back 29.9700012207031; the duration snaps from 5 s to 150 frames (5.005005 s).
    let h = skip!(comp_header("229-comp-fps-29.97.aep"));
    assert_eq!((h.time_base.frame_duration, h.time_base.timescale), (800, 23976));
    assert!((h.frame_rate().unwrap() - 29.97).abs() < 1e-12);
    // The 16.16 rate holds the float32 29.97 (29.9700012207031).
    assert_eq!(h.frame_rate_fixed, 1_964_114);
    // 150 frames of 800 units.
    assert_eq!(h.duration_units, 120_000);
    assert!((h.duration_seconds().unwrap() - 5.005_005_005_005_01).abs() < 1e-9);
    // 230: frameRate 23.976 -> read back 23.9759979248047; 120 frames.
    let h = skip!(comp_header("230-comp-fps-23.976.aep"));
    assert_eq!((h.time_base.frame_duration, h.time_base.timescale), (1000, 23976));
    assert!((h.frame_rate().unwrap() - 23.976).abs() < 1e-12);
    // The 16.16 rate holds the float32 23.976 (23.9759979248047).
    assert_eq!(h.frame_rate_fixed, 1_571_291);
    // 120 frames of 1000 units.
    assert_eq!(h.duration_units, 120_000);
    assert!((h.duration_seconds().unwrap() - 5.005_005_005_005_01).abs() < 1e-9);
    // 25 fps stays 25.
    let h = skip!(comp_header("000-base.aep"));
    assert_eq!((h.time_base.timescale, h.time_base.frame_duration, h.duration_units), (25600, 1024, 128_000));
}

#[test]
fn keyframe_times_follow_the_ntsc_timescale() {
    // All keyframe time scales in the composition follow its timescale (tdb4 byte 12 = 23976 in every property).
    for name in ["229-comp-fps-29.97.aep", "230-comp-fps-23.976.aep"] {
        let bytes = fixture!(name);
        let file = open(&bytes);
        let project = Project::new(&file).unwrap();
        let layer = base_layer(&first_comp(&project));
        let mut scales = std::collections::BTreeSet::new();
        let mut stack = vec![layer.properties().unwrap()];
        while let Some(group) = stack.pop() {
            for e in group.entries() {
                match e.node {
                    effectcraft_aep::PropNode::Group(g) => stack.push(g),
                    effectcraft_aep::PropNode::Property(p) => {
                        scales.extend(p.time_scale());
                    }
                    effectcraft_aep::PropNode::Other(_) => {}
                }
            }
        }
        assert_eq!(scales.into_iter().collect::<Vec<_>>(), [23976], "{name}");
    }
}

#[test]
fn background_is_8_bits_per_channel() {
    // 231: bgColor [0.5, 0.5, 0.5] is stored as 128 and reads back as 128/255 = 0.50196.
    let h = skip!(comp_header("231-comp-bg-grey-0.5.aep"));
    assert_eq!(h.background, [128, 128, 128]);
    // The frame rate stays 25 even though an earlier edit of the AE session left other numbers in the cache-like fields.
    assert_eq!((h.time_base.frame_duration, h.time_base.timescale), (1024, 25600));
}

#[test]
fn cameras_auto_orient_and_position() {
    // 256: the default two-node camera (ldta byte 38 = 0x44); 251: one-node (autoOrient off, 0x04).
    let bytes = fixture!("256-camera-two-node-default.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let comp = first_comp(&project);
    let camera = layer_of_type(&comp, LayerType::Camera);
    let h = camera.header().unwrap();
    assert_eq!((h.flag_bytes[1], h.is_auto_oriented(), h.is_three_d(), h.source_id, h.light_type()), (0x44, true, true, None, None));
    // The default Position is an "auto state" record that stores [0, 0, -2666.67].
    let pos = camera.camera_position().unwrap();
    assert!(pos.auto_state);
    assert_near(&pos.stored, &[0.0, 0.0, -2666.67], 0.01);

    let bytes = fixture!("251-camera-one-node.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let camera = layer_of_type(&first_comp(&project), LayerType::Camera);
    let h = camera.header().unwrap();
    assert_eq!((h.flag_bytes[1], h.is_auto_oriented(), h.is_three_d()), (0x04, false, true));
    // AE reports [0, 0, -2666.67] for the one-node camera: the stored value.
    let pos = camera.camera_position().unwrap();
    assert!(pos.auto_state);
    assert_near(&pos.stored, &[0.0, 0.0, -2666.67], 0.01);

    // 253: Position set to [100, 200, -1000]: stored as given, no auto state.
    let bytes = fixture!("253-camera-position-100-200-m1000.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let camera = layer_of_type(&first_comp(&project), LayerType::Camera);
    let pos = camera.camera_position().unwrap();
    assert!(!pos.auto_state);
    assert_eq!(pos.stored, [100.0, 200.0, -1000.0]);
}

/// Lights type and position.
#[test]
fn lights_type_and_position() {
    // 254: a spot light (its Intensity 50 is checked on import).
    let bytes = fixture!("254-light-spot-intensity-50.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let light = layer_of_type(&first_comp(&project), LayerType::Light);
    let h = light.header().unwrap();
    assert_eq!((h.light_type(), h.light_type_code, h.source_id, h.is_auto_oriented(), h.is_three_d()), (Some(LightType::Spot), 1, None, true, true));
    assert!(light.camera_position().unwrap().auto_state);
    assert_near(&light.camera_position().unwrap().stored, &[0.0, 0.0, -1333.33], 0.01);

    // 252: a point light (light type 2).
    let bytes = fixture!("252-light-point.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let light = layer_of_type(&first_comp(&project), LayerType::Light);
    let h = light.header().unwrap();
    assert_eq!((h.light_type(), h.light_type_code), (Some(LightType::Point), 2));
    // The solid under it is no light.
    let solid = base_layer(&first_comp(&project)).header().unwrap();
    assert_eq!((solid.light_type(), solid.light_type_code), (None, 0));
}

#[test]
fn solid_colour_is_four_float32_argb() {
    // 259: a new solid `Blue` [0, 0, 1] in the 1920x1080 composition: float32 ARGB 1.0, 0, 0, 1.0.
    let bytes = fixture!("259-solid-blue-0-0-1.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let comp = first_comp(&project);
    let blue = layer_shown_as(&project, &comp, "Blue");
    assert_eq!(project.layer_source_size(&blue), Some((1920, 1080)));
    let f = project.layer_source(&blue).unwrap().footage().unwrap();
    assert!(f.is_solid);
    assert_eq!(f.solid_argb, Some([1.0, 0.0, 0.0, 1.0]));
    assert_eq!(f.solid_rgb(), Some([0.0, 0.0, 1.0]));
    // L is still red.
    let red = project.layer_source(&base_layer(&comp)).unwrap().footage().unwrap();
    assert_eq!(red.solid_rgb(), Some([1.0, 0.0, 0.0]));
    // The new layer has no name of its own (it shows its source's name).
    assert_eq!(blue.name(), None);
}
