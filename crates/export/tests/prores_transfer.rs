//! ProRes output describes the renderer's transfer and preserves RGB/alpha samples.
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
    path::PathBuf,
    sync::{Arc, Mutex},
};

fn project(managed: bool, output: Option<ColorSpace>, opacity: f64) -> (Project, ItemId) {
    let mut p = Project::default();
    p.settings.bit_depth = BitDepth::Bpc32;
    p.settings.working_space = managed.then_some(ColorSpace::Srgb);
    p.settings.linearize = managed;
    p.settings.output_space = output;
    p.settings.gpu_acceleration = false;
    let rate = FrameRate::new(24, 1);
    let mut comp = Comp::new(32, 32, rate, rate.tick_of(1));
    let item = p.add_item("Original midtone", Label::Red, None, ItemKind::Solid(Solid { color: [0.8, 0.59, 0.52], width: 32, height: 32, pixel_aspect: 1.0 }));
    let mut layer = build::layer(&mut p, &comp, "Midtone", LayerSource::Solid { item }, (32, 32), None);
    layer.props.prop_mut("transform/opacity").unwrap().value = Value::Scalar(opacity);
    comp.layers.push(layer);
    let comp = p.add_item("Main", Label::Sandstone, None, ItemKind::Comp(comp.into()));
    (p, comp)
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
fn check(name: &str, managed: bool, output: Option<ColorSpace>, profile: ProResProfile, opacity: f64, transfer: u8) {
    let (project, comp) = project(managed, output, opacity);
    let reference = Renderer::new(&project, &NoFootage, RenderOpts::default()).comp_frame(comp, Tick::ZERO).straight(16, 16);
    let settings = RenderSettings { time_span: TimeSpan::LengthOfComp, ..Default::default() };
    let output = OutputModule {
        prores_profile: profile,
        channels: Channels::Rgba,
        alpha_mode: AlphaMode::Straight,
        audio: AudioOutput::Off,
        ..OutputModule::for_format(OutputFormat::ProRes)
    };
    let files: Arc<Mutex<Vec<Vec<u8>>>> = Default::default();
    let capture = files.clone();
    let sink: Box<Sink> = Box::new(move |_, data| capture.lock().unwrap().push(data));
    let job = Job {
        project: &project,
        footage: &NoFootage,
        expr: None,
        accel: None,
        comp,
        settings: &settings,
        output: &output,
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
    assert_eq!(decoded.chroma, filmcraft_prores::ChromaFormat::Yuv444);
    assert_eq!(decoded.color.primaries, 1);
    assert_eq!(decoded.color.matrix, 1);
    assert_eq!(decoded.color.transfer, transfer, "ProRes must describe the renderer's actual output encoding");
    assert!(decoded.alpha.is_some(), "RGBA preserves the alpha plane for both 4444 profiles");
    let directory = std::env::var_os("EFFECTCRAFT_PRORES340_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out/export/prores-transfer"));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("{name}-{}.mov", std::process::id()));
    std::fs::write(&path, bytes).unwrap();
    let expected_tag = match profile {
        ProResProfile::P4444 => "ap4h",
        ProResProfile::P4444Xq => "ap4x",
        _ => panic!("this regression only requests 4444 profiles"),
    };
    match std::process::Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=codec_tag_string", "-of", "default=noprint_wrappers=1:nokey=1"])
        .arg(&path)
        .output()
    {
        Ok(out) => {
            assert!(out.status.success(), "ffprobe rejected {}: {}", path.display(), String::from_utf8_lossy(&out.stderr));
            let actual = String::from_utf8(out.stdout).expect("ffprobe codec tag is UTF-8");
            assert_eq!(actual.trim(), expected_tag, "MOV must retain the requested {profile:?} profile");
            eprintln!("PRORES_PROFILE_ORACLE requested={profile:?} actual={}", actual.trim());
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => eprintln!("ffprobe not found: skipping profile identity oracle"),
        Err(error) => panic!("could not run profile identity oracle: {error}"),
    }
    let footage = effectcraft_media::probe(&path).unwrap();
    assert_eq!((footage.width, footage.height, footage.frame_rate), (32, 32, FrameRate::new(24, 1)));
    assert!(footage.has_video && !footage.has_audio);
    let image = MediaPool::new().frame_at(&footage, Tick::ZERO).unwrap();
    assert!(image.data.iter().flatten().all(|v| v.is_finite()));
    let pixel = image.straight(16, 16);
    for channel in 0..3 {
        assert!((pixel[channel] - reference[channel]).abs() < 0.006, "profile={profile:?}, reference={reference:?}, decoded={pixel:?}");
    }
    assert!((pixel[3] - reference[3]).abs() < 0.004, "fractional alpha remains unchanged: {pixel:?} vs {reference:?}");
    std::fs::remove_file(path).unwrap();
}
#[test]
fn prores_srgb_transfer_preserves_4444_and_xq_pixels_and_fractional_alpha() {
    for (profile_name, profile) in [("4444", ProResProfile::P4444), ("xq", ProResProfile::P4444Xq)] {
        for (space_name, space) in [("default", None), ("explicit", Some(ColorSpace::Srgb))] {
            check(&format!("{profile_name}-{space_name}"), true, space, profile, 50.0, 13);
        }
    }
}
#[test]
fn prores_unmanaged_pixels_are_srgb_even_with_an_unused_output_setting() {
    check("unmanaged", false, Some(ColorSpace::Rec709), ProResProfile::P4444, 100.0, 13);
}
#[test]
fn prores_explicit_rec709_preserves_its_selected_renderer_output() {
    check("rec709", true, Some(ColorSpace::Rec709), ProResProfile::P4444Xq, 50.0, 1);
}
