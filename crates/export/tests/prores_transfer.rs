//! ProRes transfer tagging is opt-in and preserves encoded RGB/alpha samples.
use effectcraft_color::{ColorSpace, Label};
use effectcraft_export::{
    Job, Sink, export,
    render_queue::{AlphaMode, AudioOutput, Channels, OutputFormat, OutputModule, ProResProfile, RenderSettings, TimeSpan},
};
use effectcraft_keyframe::Value;
use effectcraft_media::MediaPool;
use effectcraft_project::{BitDepth, Comp, ItemId, ItemKind, LayerSource, Project, Solid, build};
use effectcraft_render::{NoFootage, RenderOpts, Renderer};
use effectcraft_time::{FrameRate, Tick};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

const MIDTONE: [f32; 3] = [0.8, 0.59, 0.52];

fn project(managed: bool, output: Option<ColorSpace>, color: [f32; 3], opacity: f64) -> (Project, ItemId) {
    let mut p = Project::default();
    p.settings.bit_depth = BitDepth::Bpc32;
    p.settings.working_space = managed.then_some(ColorSpace::Srgb);
    p.settings.linearize = managed;
    p.settings.output_space = output;
    p.settings.gpu_acceleration = false;
    let rate = FrameRate::new(24, 1);
    let mut comp = Comp::new(32, 32, rate, rate.tick_of(1));
    let item = p.add_item("Original midtone", Label::Red, None, ItemKind::Solid(Solid { color, width: 32, height: 32, pixel_aspect: 1.0 }));
    let mut layer = build::layer(&mut p, &comp, "Midtone", LayerSource::Solid { item }, (32, 32), None);
    layer.props.prop_mut("transform/opacity").unwrap().value = Value::Scalar(opacity);
    comp.layers.push(layer);
    let comp = p.add_item("Main", Label::Sandstone, None, ItemKind::Comp(comp.into()));
    (p, comp)
}

fn module(profile: ProResProfile, channels: Channels, srgb_transfer: Option<bool>) -> OutputModule {
    let mut output = OutputModule {
        prores_profile: profile,
        channels,
        alpha_mode: AlphaMode::Straight,
        audio: AudioOutput::Off,
        ..OutputModule::for_format(OutputFormat::ProRes)
    };
    assert!(!output.prores_srgb_transfer, "new output modules preserve the legacy transfer tag");
    if let Some(enabled) = srgb_transfer {
        output.prores_srgb_transfer = enabled;
    }
    output
}

fn media_data(bytes: &[u8]) -> &[u8] {
    let mut at = 0;
    while at < bytes.len() {
        let header = bytes.get(at..at + 8).unwrap();
        let short = u32::from_be_bytes(header[..4].try_into().unwrap());
        let (size, head) = match short {
            1 => (usize::try_from(u64::from_be_bytes(bytes.get(at + 8..at + 16).unwrap().try_into().unwrap())).unwrap(), 16),
            0 => (bytes.len() - at, 8),
            n => (n as usize, 8),
        };
        assert!(size >= head && size <= bytes.len() - at);
        if header.get(4..8) == Some(b"mdat") {
            return bytes.get(at + head..at + size).unwrap();
        }
        at += size;
    }
    panic!("actual generated MOV must contain a media-data atom");
}

fn check(name: &str, project: &Project, comp: ItemId, output: &OutputModule, transfer: u8, expected: [f32; 4]) {
    let reference = Renderer::new(project, &NoFootage, RenderOpts::default()).comp_frame(comp, Tick::ZERO).straight(16, 16);
    for channel in 0..4 {
        assert!((reference[channel] - expected[channel]).abs() < 1e-5, "{name}: rendered={reference:?}, expected={expected:?}");
    }
    let settings = RenderSettings { time_span: TimeSpan::LengthOfComp, ..Default::default() };
    let files: Arc<Mutex<Vec<Vec<u8>>>> = Default::default();
    let capture = files.clone();
    let sink: Box<Sink> = Box::new(move |_, data| capture.lock().unwrap().push(data));
    let job = Job {
        project,
        footage: &NoFootage,
        expr: None,
        accel: None,
        comp,
        settings: &settings,
        output,
        path: "original.midtones.mov",
        sink: Some(&*sink),
        options: Default::default(),
        nested_switches: true,
    };
    let report = export(&job, &mut |_| true).expect("public ProRes export");
    assert_eq!((report.frames, report.width, report.height, report.audio), (1, 32, 32, false));
    let files = files.lock().unwrap();
    assert_eq!(files.len(), 1);
    let bytes = &files[0];
    let decoded = filmcraft_prores::decode_frame(media_data(bytes)).expect("decode actual exported ProRes sample through public codec API");
    assert_eq!((decoded.width, decoded.height), (32, 32));
    let profile = output.prores_profile;
    let (expected_tag, chroma) = match profile {
        ProResProfile::P4444 => ("ap4h", filmcraft_prores::ChromaFormat::Yuv444),
        ProResProfile::P4444Xq => ("ap4x", filmcraft_prores::ChromaFormat::Yuv444),
        ProResProfile::Hq => ("apch", filmcraft_prores::ChromaFormat::Yuv422),
        _ => panic!("this regression requests 4444, 4444 XQ or opaque HQ"),
    };
    assert_eq!(decoded.chroma, chroma);
    assert_eq!(decoded.color.primaries, 1);
    assert_eq!(decoded.color.matrix, 1);
    assert_eq!(decoded.color.transfer, transfer, "{name}: transfer tag follows the explicit output-module choice and output encoding");
    assert_eq!(decoded.alpha.is_some(), output.channels == Channels::Rgba, "{name}: only RGBA exports retain an alpha plane");
    let directory = std::env::var_os("EFFECTCRAFT_PRORES340_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out/export/prores-transfer"));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("{name}-{}.mov", std::process::id()));
    std::fs::write(&path, bytes).unwrap();
    match std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=codec_tag_string,color_primaries,color_transfer,color_space",
            "-of",
            "default=noprint_wrappers=1",
        ])
        .arg(&path)
        .output()
    {
        Ok(out) => {
            assert!(out.status.success(), "ffprobe rejected {}: {}", path.display(), String::from_utf8_lossy(&out.stderr));
            let actual = String::from_utf8(out.stdout).expect("ffprobe stream metadata is UTF-8");
            let fields: BTreeMap<_, _> = actual.lines().filter_map(|line| line.split_once('=')).collect();
            assert_eq!(fields.get("codec_tag_string").copied(), Some(expected_tag), "MOV must retain the requested {profile:?} profile");
            assert_eq!(fields.get("color_primaries").copied(), Some("bt709"), "{name}: {actual}");
            assert_eq!(fields.get("color_space").copied(), Some("bt709"), "{name}: {actual}");
            let expected_transfer = if transfer == 13 { "iec61966-2-1" } else { "bt709" };
            assert_eq!(fields.get("color_transfer").copied(), Some(expected_transfer), "{name}: {actual}");
            eprintln!("PRORES_COLOR_ORACLE case={name} profile={expected_tag} primaries=bt709 matrix=bt709 transfer={expected_transfer}");
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => eprintln!("ffprobe not found: skipping profile and color metadata oracle"),
        Err(error) => panic!("could not run profile and color metadata oracle: {error}"),
    }
    let footage = effectcraft_media::probe(&path).unwrap();
    assert_eq!((footage.width, footage.height, footage.frame_rate), (32, 32, FrameRate::new(24, 1)));
    assert!(footage.has_video && !footage.has_audio);
    let image = MediaPool::new().frame_at(&footage, Tick::ZERO).unwrap();
    assert!(image.data.iter().flatten().all(|v| v.is_finite()));
    let pixel = image.straight(16, 16);
    let expected = match output.channels {
        Channels::Alpha => [expected[3], expected[3], expected[3], 1.0],
        Channels::Rgb => {
            assert_eq!(expected[3], 1.0, "the RGB control uses an opaque source");
            expected
        }
        Channels::Rgba => expected,
    };
    for channel in 0..3 {
        assert!((pixel[channel] - expected[channel]).abs() < 0.006, "{name}: profile={profile:?}, expected={expected:?}, decoded={pixel:?}");
    }
    assert!((pixel[3] - expected[3]).abs() < 0.004, "{name}: alpha remains unchanged: {pixel:?} vs {expected:?}");
    std::fs::remove_file(path).unwrap();
}

#[test]
fn prores_srgb_transfer_preserves_4444_and_xq_pixels_and_fractional_alpha() {
    for (profile_name, profile) in [("4444", ProResProfile::P4444), ("xq", ProResProfile::P4444Xq)] {
        for (space_name, space) in [("default", None), ("explicit", Some(ColorSpace::Srgb))] {
            let (p, comp) = project(true, space, MIDTONE, 50.0);
            let output = module(profile, Channels::Rgba, Some(true));
            check(&format!("{profile_name}-{space_name}"), &p, comp, &output, 13, [0.8, 0.59, 0.52, 0.5]);
        }
    }
}

#[test]
fn prores_unmanaged_pixels_are_srgb_even_with_an_unused_output_setting() {
    let (p, comp) = project(false, Some(ColorSpace::Rec709), MIDTONE, 100.0);
    check("unmanaged", &p, comp, &module(ProResProfile::P4444, Channels::Rgba, Some(true)), 13, [0.8, 0.59, 0.52, 1.0]);
}

#[test]
fn prores_explicit_rec709_preserves_its_selected_renderer_output() {
    let (p, comp) = project(true, Some(ColorSpace::Rec709), MIDTONE, 50.0);
    // sRGB-authored channels decoded to linear, then encoded with the output's 2.4 curve.
    let expected = [0.81042653, 0.6113744, 0.5450237, 0.5];
    check("rec709", &p, comp, &module(ProResProfile::P4444Xq, Channels::Rgba, Some(true)), 1, expected);
}

#[test]
fn prores_default_and_explicit_false_retain_the_legacy_transfer_tag() {
    for (name, choice) in [("legacy-default", None), ("legacy-false", Some(false))] {
        let (p, comp) = project(true, None, MIDTONE, 50.0);
        check(name, &p, comp, &module(ProResProfile::P4444, Channels::Rgba, choice), 1, [0.8, 0.59, 0.52, 0.5]);
    }
}

#[test]
fn prores_srgb_opt_in_respects_working_gamma_and_managed_output() {
    for (name, managed, output_space, gamma, linearize, expected, transfer) in [
        ("unmanaged-22", false, Some(ColorSpace::Rec709), 2.2, false, 0.735357, 13),
        ("unmanaged-24", false, Some(ColorSpace::Srgb), 2.4, false, 0.74915355, 1),
        ("managed-24-linear", true, None, 2.4, true, 0.735357, 13),
        ("managed-24-encoded", true, Some(ColorSpace::Srgb), 2.4, false, 0.735357, 13),
    ] {
        let (mut p, comp) = project(managed, output_space, [1.0; 3], 50.0);
        p.settings.working_gamma = gamma;
        p.settings.linearize = linearize;
        p.settings.blend_linear = true;
        let c = p.comp(comp).unwrap().clone();
        let black = p.add_item("Black", Label::Red, None, ItemKind::Solid(Solid { color: [0.0; 3], width: 32, height: 32, pixel_aspect: 1.0 }));
        let background = build::layer(&mut p, &c, "Black", LayerSource::Solid { item: black }, (32, 32), None);
        p.comp_mut(comp).unwrap().layers.push(background);
        // White at 50% over opaque black is 0.5 linear. The literal values above are
        // 1.055 * 0.5^(1/2.4) - 0.055 (sRGB) and 0.5^(1/2.4) (Working Gamma 2.4).
        check(name, &p, comp, &module(ProResProfile::P4444, Channels::Rgba, Some(true)), transfer, [expected, expected, expected, 1.0]);
    }
}

#[test]
fn prores_alpha_only_ignores_the_srgb_option() {
    let (p, comp) = project(true, None, MIDTONE, 50.0);
    check("alpha-only", &p, comp, &module(ProResProfile::P4444, Channels::Alpha, Some(true)), 1, [0.8, 0.59, 0.52, 0.5]);
}

#[test]
fn prores_opaque_hq_accepts_srgb_tagging_without_profile_promotion() {
    let (p, comp) = project(true, None, MIDTONE, 100.0);
    check("opaque-hq", &p, comp, &module(ProResProfile::Hq, Channels::Rgb, Some(true)), 13, [0.8, 0.59, 0.52, 1.0]);
}
