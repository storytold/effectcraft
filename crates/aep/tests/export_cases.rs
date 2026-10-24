//! Cases made for the exporter (session X1 / X2, 600-610): a renamed effect instance and a
//! composition that shows two precompositions. Every expected value is the one the case was made
//! with (its MANIFEST row).

mod common;

use common::{fixture, open};
use effectcraft_aep::Project;

#[test]
fn a_renamed_effect_keeps_its_name_in_the_value_group() {
    let bytes = fixture("610-fx-tint-named-My-Tint.aep");
    let file = open(&bytes);
    let project = Project::new(&file).expect("project");
    let comp = project.all_items().into_iter().find_map(|i| i.comp()).expect("comp");
    let layer = comp.layers().next().expect("layer");
    let fx = layer.effects();
    assert_eq!(fx.len(), 1);
    assert_eq!(fx[0].match_name(), "ADBE Tint");
    // fnam keeps the effect's own name, the instance name is the value group's tdsn.
    assert_eq!(fx[0].name().as_deref(), Some("Tint"));
    assert_eq!(fx[0].user_name().as_deref(), Some("My Tint"));
    assert_eq!(fx[0].display_name().as_deref(), Some("My Tint"));
    // An unnamed instance (356) shows the effect's name.
    let bytes = fixture("356-fx-tint-default.aep");
    let file = open(&bytes);
    let project = Project::new(&file).expect("project");
    let comp = project.all_items().into_iter().find_map(|i| i.comp()).expect("comp");
    let fx = comp.layers().next().expect("layer").effects();
    assert_eq!(fx[0].user_name(), None);
    assert_eq!(fx[0].display_name().as_deref(), Some("Tint"));
}

#[test]
fn a_composition_counts_the_compositions_it_shows() {
    let bytes = fixture("601-precomp-two-comps-in-C.aep");
    let file = open(&bytes);
    let project = Project::new(&file).expect("project");
    let items = project.all_items();
    let id = |n: &str| items.iter().find(|i| i.display_name().as_deref() == Some(n)).and_then(|i| i.id()).expect(n);
    let c = items.iter().find(|i| i.display_name().as_deref() == Some("C")).expect("C");
    assert_eq!(c.used_item_ids(), vec![id("P30"), id("Q")]);
    let idpc = c.chunk().child(b"idpc").and_then(|x| x.bytes()).expect("idpc");
    assert_eq!(&idpc[..4], &2u32.to_le_bytes());
    // The precomposition layers: P30 640x360 30 fps 3 s, Q 320x240 25 fps 2 s.
    let comp = c.comp().expect("comp");
    let sizes: Vec<_> = comp.layers().filter_map(|l| project.layer_source_size(&l)).collect();
    assert_eq!(sizes, vec![(320, 240), (640, 360), (1920, 1080)]);
}

#[test]
fn an_image_sequence_knows_its_numbering() {
    // 603: shot_0101.exr .. shot_0103.exr imported as a sequence (64x48, 3 frames at 30 fps).
    let bytes = fixture("603-footage-exrseq-0101-as-layer.aep");
    let file = open(&bytes);
    let project = Project::new(&file).expect("project");
    let item = project.all_items().into_iter().find(|i| i.footage().is_some_and(|f| !f.is_solid)).expect("sequence");
    let f = item.footage().expect("footage");
    assert_eq!((f.file_type.as_str(), f.width, f.height, f.native_frame_rate, f.is_sequence), ("oEXR", 64, 48, 30, true));
    assert_eq!(f.sequence_frame_count(), Some(3));
    let s = item.sequence_info().expect("numbering");
    assert_eq!((s.prefix.as_str(), s.suffix.as_str(), s.start, s.end, s.digits), ("shot_", ".exr", 101, 103, 4));
    assert_eq!(s.file_name(101), "shot_0101.exr");
    // A still has no numbering (000: the solid; the base has no other footage).
    let bytes = fixture("126-footage-png-layer.aep");
    let file = open(&bytes);
    let project = Project::new(&file).expect("project");
    assert!(project.all_items().iter().all(|i| i.sequence_info().is_none()));
}

#[test]
fn project_settings_are_read_from_the_header_chunks() {
    use effectcraft_aep::{ProjectSettings, TimeDisplay, WorkingSpace};
    let read = |name: &str| {
        let bytes = fixture(name);
        let file = open(&bytes);
        ProjectSettings::read(&file)
    };
    // The base (620 = 000): 32 bpc, sRGB, gamma 2.2, both linear switches off, compensation on,
    // timecode, frames from 0.
    let base = read("000-base.aep");
    assert_eq!(
        base,
        ProjectSettings {
            bits_per_channel: Some(32),
            working_space: Some(WorkingSpace::Profile("sRGB IEC61966-2.1".into())),
            working_gamma_24: Some(false),
            linearize: false,
            blend_linear: false,
            scene_compensation: Some(true),
            time_display: Some(TimeDisplay::Timecode),
            frame_start: Some(0),
        }
    );
    // One change each (MANIFEST 621-634).
    assert_eq!(read("621-settings-bpc-8.aep"), ProjectSettings { bits_per_channel: Some(8), ..base.clone() });
    assert_eq!(read("622-settings-bpc-16.aep"), ProjectSettings { bits_per_channel: Some(16), ..base.clone() });
    assert_eq!(read("623-settings-workingspace-none.aep"), ProjectSettings { working_space: Some(WorkingSpace::None), ..base.clone() });
    assert_eq!(read("625-settings-gamma-2.4.aep"), ProjectSettings { working_gamma_24: Some(true), ..base.clone() });
    assert_eq!(read("628-settings-scene-compensation-off.aep"), ProjectSettings { scene_compensation: Some(false), ..base.clone() });
    assert_eq!(read("629-settings-time-display-frames.aep"), ProjectSettings { time_display: Some(TimeDisplay::Frames), ..base.clone() });
    assert_eq!(read("630-settings-frame-start-1.aep"), ProjectSettings { frame_start: Some(1), ..base.clone() });
    assert_eq!(read("632-settings-linearize-and-blend-linear.aep"), ProjectSettings { linearize: true, blend_linear: true, ..base.clone() });
    assert_eq!(
        read("633-settings-feet-frames-35.aep"),
        ProjectSettings { time_display: Some(TimeDisplay::FeetFrames { frames_per_foot: 16 }), ..base.clone() }
    );
    assert_eq!(
        read("634-settings-feet-frames-16.aep"),
        ProjectSettings { time_display: Some(TimeDisplay::FeetFrames { frames_per_foot: 40 }), ..base.clone() }
    );
    // nnhd repeats the depth at byte 24.
    for (name, v) in [("621-settings-bpc-8.aep", 0u8), ("622-settings-bpc-16.aep", 1), ("000-base.aep", 2)] {
        let bytes = fixture(name);
        let file = open(&bytes);
        assert_eq!(file.chunk(b"nnhd").and_then(|c| c.bytes()).and_then(|b| b.get(24).copied()), Some(v), "{name}");
    }
}

#[test]
fn a_conformed_clip_keeps_its_native_rate_and_frames() {
    // 645: the 25 fps, 25-frame mp4 of 606 conformed to 30 fps; 644 the same file unconformed.
    let read = |name: &str| {
        let bytes = fixture(name);
        let file = open(&bytes);
        let project = Project::new(&file).expect("project");
        let item = project.all_items().into_iter().find(|i| i.footage().is_some_and(|f| !f.is_solid)).expect("clip");
        let f = item.footage().expect("footage");
        (f.native_frame_rate, item.conform_frame_rate(), f.duration.map(|d| (d.num, d.den)))
    };
    assert_eq!(read("645-footage-mp4-conform-30.aep"), (25, Some(30), Some((25, 25))));
    // Not conformed (the exr sequence 603, the stills of 650-653).
    assert_eq!(read("603-footage-exrseq-0101-as-layer.aep").1, None);
    for name in ["650-footage-tga-still-as-layer.aep", "651-footage-psd-flat-as-layer.aep", "652-footage-ai-as-layer.aep", "653-footage-eps-as-layer.aep"] {
        assert_eq!(read(name).1, None, "{name}");
    }
}

#[test]
fn still_footage_of_four_more_types() {
    // 650-653: a 64 x 48 tga, a flat psd, an ai and an eps imported as footage (MANIFEST); ai and
    // eps share the type tag `TEXT`.
    for (name, tag) in [
        ("650-footage-tga-still-as-layer.aep", "TPIC"),
        ("651-footage-psd-flat-as-layer.aep", "8BPS"),
        ("652-footage-ai-as-layer.aep", "TEXT"),
        ("653-footage-eps-as-layer.aep", "TEXT"),
    ] {
        let bytes = fixture(name);
        let file = open(&bytes);
        let project = Project::new(&file).expect("project");
        let f = project.all_items().into_iter().find_map(|i| i.footage().filter(|f| !f.is_solid)).expect(name);
        assert_eq!((f.width, f.height, f.native_frame_rate, f.is_sequence), (64, 48, 0, false), "{name}");
        assert_eq!(f.file_type, tag, "{name}");
        let ext = name.get(12..).and_then(|r| r.split('-').next()).unwrap_or_default();
        assert!(f.path.as_deref().is_some_and(|p| p.ends_with(&format!("still.{ext}"))), "{name}: {:?}", f.path);
    }
}
