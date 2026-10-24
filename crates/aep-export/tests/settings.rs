//! Project settings (cases 620-634): written into the template's header chunks, read back with
//! `effectcraft-aep`, and the settings fixtures through import, export and import.

mod common;

use common::*;
use effectcraft_aep::{TimeDisplay, WorkingSpace, parse, write};
use effectcraft_aep_export::LossCategory;
use effectcraft_color::ColorSpace;
use effectcraft_project::{BitDepth, ColorEngine, Project, ProjectSettings, TimeDisplayStyle};

fn fixture_bytes(name: &str) -> Vec<u8> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../aep/tests/fixtures").join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn read_back(p: &Project) -> effectcraft_aep::ProjectSettings {
    let r = export_all(p);
    let file = parse(&r.bytes).expect("parse");
    assert_eq!(write(&file).expect("write"), r.bytes, "container consistent");
    effectcraft_aep::ProjectSettings::read(&file)
}

fn with(f: impl FnOnce(&mut ProjectSettings)) -> Project {
    let (mut p, _, _) = simple();
    f(&mut p.settings);
    p
}

#[test]
fn every_setting_is_written_as_after_effects_stores_it() {
    // EffectCraft's defaults: 8 bpc, no working space, gamma 2.2, timecode, frames from 0.
    let d = read_back(&with(|_| {}));
    assert_eq!(d.bits_per_channel, Some(8));
    assert_eq!(d.working_space, Some(WorkingSpace::None));
    assert_eq!(d.working_gamma_24, Some(false));
    assert!(!d.linearize && !d.blend_linear);
    assert_eq!(d.time_display, Some(TimeDisplay::Timecode));
    assert_eq!(d.frame_start, Some(0));
    assert_eq!(d.scene_compensation, Some(true), "kept from the template");
    for (depth, bpc) in [(BitDepth::Bpc8, 8), (BitDepth::Bpc16, 16), (BitDepth::Bpc32, 32)] {
        assert_eq!(read_back(&with(|s| s.bit_depth = depth)).bits_per_channel, Some(bpc));
    }
    let srgb = read_back(&with(|s| s.working_space = Some(ColorSpace::Srgb)));
    assert_eq!(srgb.working_space, Some(WorkingSpace::Profile("sRGB IEC61966-2.1".into())));
    assert_eq!(read_back(&with(|s| s.working_gamma = 2.4)).working_gamma_24, Some(true));
    for (lin, blend) in [(true, false), (false, true), (true, true)] {
        let r = read_back(&with(|s| {
            s.linearize = lin;
            s.blend_linear = blend;
        }));
        assert_eq!((r.linearize, r.blend_linear), (lin, blend));
    }
    for (style, want) in [
        (TimeDisplayStyle::Frames, TimeDisplay::Frames),
        (TimeDisplayStyle::Feet35, TimeDisplay::FeetFrames { frames_per_foot: 16 }),
        (TimeDisplayStyle::Feet16, TimeDisplay::FeetFrames { frames_per_foot: 40 }),
        (TimeDisplayStyle::Timecode, TimeDisplay::Timecode),
    ] {
        assert_eq!(read_back(&with(|s| s.time_display = style)).time_display, Some(want));
    }
    assert_eq!(read_back(&with(|s| s.frame_start = 1)).frame_start, Some(1));
}

fn unbase64(s: &str) -> Vec<u8> {
    let val = |c: u8| match c {
        b'A'..=b'Z' => u32::from(c - b'A'),
        b'a'..=b'z' => u32::from(c - b'a') + 26,
        b'0'..=b'9' => u32::from(c - b'0') + 52,
        b'+' => 62,
        b'/' => 63,
        _ => panic!("not base64: {c}"),
    };
    let mut out = Vec::new();
    for q in s.as_bytes().chunks(4) {
        let n = q.iter().filter(|&&c| c != b'=').count();
        let v = q.iter().filter(|&&c| c != b'=').enumerate().fold(0u32, |acc, (i, &c)| acc | (val(c) << (18 - 6 * i)));
        out.extend_from_slice(&v.to_be_bytes()[1..n]);
    }
    out
}

#[test]
fn rec709_gamma_24_is_written_with_a_profile_of_our_own() {
    let p = with(|s| s.working_space = Some(ColorSpace::Rec709));
    let r = export_all(&p);
    assert!(r.report.is_lossless(), "{:?}", r.report.losses);
    let file = parse(&r.bytes).expect("parse");
    let s = effectcraft_aep::ProjectSettings::read(&file);
    assert_eq!(s.working_space, Some(WorkingSpace::Profile("Rec.709 Gamma 2.4".into())));
    let cpid = file.chunks.iter().find(|c| c.is(b"cpid")).and_then(|c| c.bytes()).expect("cpid").to_vec();
    let pw = file.chunks.iter().position(|c| c.is(b"PwCs")).expect("PwCs");
    let json: serde_json::Value = serde_json::from_slice(file.chunks[pw + 1].bytes().expect("Utf8")).expect("json");
    assert_eq!(json["baseProfileType"], 2);
    let icc = unbase64(json["baseColorProfile"]["colorProfileData"].as_str().expect("data"));
    // An ICC v4 display profile: size, 'acsp', and the ID in cpid equals the one in the header.
    assert_eq!(u32::from_be_bytes([icc[0], icc[1], icc[2], icc[3]]) as usize, icc.len());
    assert_eq!(&icc[36..40], b"acsp");
    assert_eq!(icc[8], 4);
    assert_eq!(&icc[84..100], &cpid[..]);
    assert_ne!(cpid, vec![0xffu8; 16]);
    // Import maps it back to Rec.709.
    let mut q = Project::default();
    let none = |_: &str| false;
    let back = effectcraft_aep_import::import(&mut q, &r.bytes, &effectcraft_aep_import::ImportOptions::new(&none)).expect("import");
    assert_eq!(back.settings.working_space, Some(ColorSpace::Rec709));
}

#[test]
fn what_the_file_cannot_hold_is_reported() {
    let lossy = [
        with(|s| s.working_space = Some(ColorSpace::Rec2020)),
        with(|s| s.color_engine = ColorEngine::Ocio),
        with(|s| s.output_space = Some(ColorSpace::Rec2100Pq)),
        with(|s| s.frame_start = 7),
    ];
    for p in &lossy {
        let r = export_all(p);
        assert_eq!(r.report.count(LossCategory::ProjectSettings), 1, "{:?}", r.report.losses);
    }
    // Rec. 2020 is written as sRGB.
    assert_eq!(read_back(&lossy[0]).working_space, Some(WorkingSpace::Profile("sRGB IEC61966-2.1".into())));
    assert!(export_all(&with(|_| {})).report.is_lossless());
}

#[test]
fn settings_fixtures_round_trip() {
    let names = [
        "000-base.aep",
        "621-settings-bpc-8.aep",
        "622-settings-bpc-16.aep",
        "623-settings-workingspace-none.aep",
        "625-settings-gamma-2.4.aep",
        "629-settings-time-display-frames.aep",
        "630-settings-frame-start-1.aep",
        "632-settings-linearize-and-blend-linear.aep",
        "633-settings-feet-frames-35.aep",
        "634-settings-feet-frames-16.aep",
    ];
    for name in names {
        let bytes = fixture_bytes(name);
        let orig = effectcraft_aep::ProjectSettings::read(&parse(&bytes).expect("parse"));
        let mut p = Project::default();
        let none = |_: &str| false;
        let r = effectcraft_aep_import::import(&mut p, &bytes, &effectcraft_aep_import::ImportOptions::new(&none)).expect(name);
        p.settings = r.settings;
        let back = read_back(&p);
        assert_eq!(back, orig, "{name}");
    }
}

#[test]
fn an_output_space_equal_to_the_working_space_is_the_normal_case() {
    // After Effects renders in the working space (902): the importer sets the output space of a
    // Rec.709 Gamma 2.4 project to Rec.709, and the export of such a project loses nothing.
    let bytes = fixture_bytes("902-settings-rec709-16bpc-steps-png-grey-solid.aep");
    let mut p = Project::default();
    let none = |_: &str| false;
    let opts = effectcraft_aep_import::ImportOptions { allow_losses: true, ..effectcraft_aep_import::ImportOptions::new(&none) };
    let r = effectcraft_aep_import::import(&mut p, &bytes, &opts).expect("import");
    assert_eq!((r.settings.working_space, r.settings.output_space), (Some(ColorSpace::Rec709), Some(ColorSpace::Rec709)));
    p.settings = r.settings;
    let e = export_all(&p);
    assert_eq!(e.report.count(LossCategory::ProjectSettings), 0, "{:?}", e.report.losses);
    let mut q = Project::default();
    let back = effectcraft_aep_import::import(&mut q, &e.bytes, &opts).expect("re-import");
    assert_eq!((back.settings.working_space, back.settings.output_space), (Some(ColorSpace::Rec709), Some(ColorSpace::Rec709)));
    // Another output space is still reported.
    let other = with(|s| {
        s.working_space = Some(ColorSpace::Rec709);
        s.output_space = Some(ColorSpace::Srgb);
    });
    assert_eq!(export_all(&other).report.count(LossCategory::ProjectSettings), 1);
}
