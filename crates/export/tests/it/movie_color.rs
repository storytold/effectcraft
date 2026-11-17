//! H.264, HEVC and AV1 MP4s describe the frames the render path made (#2879): the `colr` box, read
//! back with FilmCraft's demuxer, and the bitstream's own colour description (the SPS VUI, AV1's
//! `color_config`), which ffprobe reports for the first frame of the bare elementary stream when
//! ffmpeg can copy it out and decode it.

use std::path::Path;
use std::process::Command;

use craft_media::isobmff::{ColorInfo, TrackKind};
use effectcraft_color::Label;
use effectcraft_export::render_queue::{CodecProfile, OutputFormat, OutputModule, ProResProfile, RenderSettings, TimeSpan};
use effectcraft_export::{Job, export};
use effectcraft_project::{ColorSpace, Comp, ItemId, ItemKind, LayerSource, Project, Solid, build};
use effectcraft_render::NoFootage;
use effectcraft_time::{FrameRate, Tick};

/// (name, Output color space, H.273 primaries / transfer / matrix, the same as ffprobe names them)
const ROWS: &[(&str, Option<ColorSpace>, [u16; 3], [&str; 3])] = &[
    ("default", None, [1, 1, 1], ["bt709", "bt709", "bt709"]),
    ("rec2020", Some(ColorSpace::Rec2020), [9, 1, 1], ["bt2020", "bt709", "bt709"]),
    ("p3", Some(ColorSpace::DisplayP3), [12, 13, 1], ["smpte432", "iec61966-2-1", "bt709"]),
    ("pq", Some(ColorSpace::Rec2100Pq), [9, 16, 9], ["bt2020", "smpte2084", "bt2020nc"]),
    ("hlg", Some(ColorSpace::Rec2100Hlg), [9, 18, 9], ["bt2020", "arib-std-b67", "bt2020nc"]),
];

fn render(output: Option<ColorSpace>, format: OutputFormat, path: &Path) {
    let mut p = Project::default();
    p.settings.working_space = Some(ColorSpace::Srgb);
    p.settings.output_space = output;
    let mut comp = Comp::new(64, 48, FrameRate::new(10, 1), Tick::from_seconds_f64(0.2));
    let sid = p.add_item("Solid", Label::Red, None, ItemKind::Solid(Solid { color: [0.75, 0.25, 0.5], width: 64, height: 48, pixel_aspect: 1.0 }));
    comp.layers.push(build::layer(&mut p, &comp, "Solid", LayerSource::Solid { item: sid }, (64, 48), None));
    let comp = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.into()));
    render_project(&p, comp, &OutputModule::for_format(format), path);
}

fn render_project(p: &Project, comp: ItemId, om: &OutputModule, path: &Path) {
    let settings = RenderSettings { time_span: TimeSpan::LengthOfComp, ..Default::default() };
    let out = path.to_string_lossy().to_string();
    let job = Job {
        project: p,
        footage: &NoFootage,
        expr: None,
        accel: None,
        comp,
        settings: &settings,
        output: om,
        path: &out,
        sink: None,
        options: Default::default(),
        nested_switches: true,
    };
    export(&job, &mut |_| true).expect("export");
}

/// ffprobe's primaries, transfer and matrix of the first frame of the movie's video copied out as a
/// bare elementary stream (`h264`, `hevc` or `obu`), so no container metadata can stand in for the
/// bitstream's own; `None` without ffmpeg or when its build can't copy or decode the stream.
fn bitstream_tags(path: &Path, format: &str) -> Option<Vec<String>> {
    let es = path.with_extension(format);
    let copied =
        Command::new("ffmpeg").args(["-v", "error", "-y", "-i"]).arg(path).args(["-map", "0:v:0", "-c", "copy", "-f", format]).arg(&es).status().ok()?;
    if !copied.success() {
        return None;
    }
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-read_intervals", "%+#1", "-show_entries", "frame=color_primaries,color_transfer,color_space", "-of", "default=nw=1"])
        .arg(&es)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let tag = |key: &str| text.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix('=')).map(str::to_string);
    Some(vec![tag("color_primaries")?, tag("color_transfer")?, tag("color_space")?])
}

#[test]
fn mp4_tags_describe_the_frames() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out/export/movie_color");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    for (format, codec, stream) in [(OutputFormat::H264, "h264", "h264"), (OutputFormat::Hevc, "hevc", "hevc"), (OutputFormat::Av1, "av1", "obu")] {
        for &(name, output, [primaries, transfer, matrix], names) in ROWS {
            let path = dir.join(format!("{codec}_{name}.mp4"));
            render(output, format, &path);

            let bytes = std::fs::read(&path).expect("read movie");
            let mp4 = craft_media::isobmff::open(bytes.as_slice()).expect("demux");
            let track = mp4.tracks.iter().find(|t| t.kind == TrackKind::Video).expect("video track");
            let colr = track.entries[0].video.as_ref().and_then(|v| v.color.clone());
            assert_eq!(colr, Some(ColorInfo::Nclx { primaries, transfer, matrix, full_range: false }), "{codec} {name}: colr");

            match bitstream_tags(&path, stream) {
                Some(tags) => assert_eq!(tags, names, "{codec} {name}: the bitstream's colour description"),
                None => eprintln!("{codec} {name}: ffprobe can't decode it here, bitstream not checked"),
            }
        }
    }
}

/// Force the inverse matrix without applying a transfer or primaries conversion.
fn matrix_pixel(path: &Path, matrix: &str) -> Option<[u8; 3]> {
    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-frames:v", "1", "-vf", &format!("scale=in_color_matrix={matrix}:out_range=full"), "-pix_fmt", "rgb24", "-f", "rawvideo", "-"])
        .output()
        .ok()?;
    let err = String::from_utf8_lossy(&out.stderr);
    // "no decoder found for: av1" today, "Decoder (codec av1) not found" from older ffmpeg.
    if !out.status.success() && (err.contains("no decoder found") || (err.contains("Decoder (codec") && err.contains("not found"))) {
        eprintln!("ffmpeg cannot decode this codec here: skipping matrix oracle ({err})");
        return None;
    }
    assert!(out.status.success(), "ffmpeg rejected {}: {err}", path.display());
    assert_eq!(out.stdout.len(), 64 * 48 * 3, "{}", path.display());
    let i = (24 * 64 + 32) * 3;
    Some(out.stdout[i..i + 3].try_into().expect("RGB pixel"))
}

#[test]
fn movie_pixels_agree_with_their_matrix_tags() {
    use effectcraft_media::MediaPool;

    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out/export/movie_matrix_pixels");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    for (space_name, space, matrix) in [("default", None, 1), ("pq", Some(ColorSpace::Rec2100Pq), 9), ("hlg", Some(ColorSpace::Rec2100Hlg), 9)] {
        let mut p = Project::default();
        p.settings.working_space = space.or(Some(ColorSpace::Srgb));
        p.settings.output_space = space;
        let mut comp = Comp::new(64, 48, FrameRate::new(10, 1), Tick::from_seconds_f64(0.2));
        let sid = p.add_item("Red", Label::Red, None, ItemKind::Solid(Solid { color: [1.0, 0.0, 0.0], width: 64, height: 48, pixel_aspect: 1.0 }));
        comp.layers.push(build::layer(&mut p, &comp, "Red", LayerSource::Solid { item: sid }, (64, 48), None));
        let comp = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.into()));
        for (codec, format, profile, prores) in [
            ("h264", OutputFormat::H264, CodecProfile::Main, ProResProfile::Hq),
            ("hevc8", OutputFormat::Hevc, CodecProfile::Main, ProResProfile::Hq),
            ("hevc10", OutputFormat::Hevc, CodecProfile::Main10, ProResProfile::Hq),
            ("av1_8", OutputFormat::Av1, CodecProfile::Main, ProResProfile::Hq),
            ("av1_10", OutputFormat::Av1, CodecProfile::Main10, ProResProfile::Hq),
            ("prores422", OutputFormat::ProRes, CodecProfile::Main, ProResProfile::Hq),
            ("prores444", OutputFormat::ProRes, CodecProfile::Main, ProResProfile::P4444),
        ] {
            let path = dir.join(format!("{codec}_{space_name}.{}", if format == OutputFormat::ProRes { "mov" } else { "mp4" }));
            let mut om = OutputModule::for_format(format);
            om.codec.profile = profile;
            om.prores_profile = prores;
            render_project(&p, comp, &om, &path);
            let bytes = std::fs::read(&path).expect("movie");
            let mov = craft_media::isobmff::open(bytes.as_slice()).expect("demux");
            let track = mov.tracks.iter().find(|t| t.kind == TrackKind::Video).expect("video");
            let tag = track.entries[0].video.as_ref().and_then(|v| v.color.as_ref()).expect("colr");
            let code = match tag {
                ColorInfo::Nclc { matrix, .. } | ColorInfo::Nclx { matrix, .. } => *matrix,
                _ => panic!("expected H.273 colour description"),
            };
            assert_eq!(code, matrix, "{codec} {space_name}");
            let footage = effectcraft_media::probe(&path).expect("probe");
            let img = MediaPool::new().frame_at(&footage, Tick::ZERO).expect("decode");
            let got = img.data[img.idx(32, 24)];
            // Allow three 8-bit levels for quantisation of these solid blocks.
            let near_red = |rgb: [f32; 3]| rgb.iter().zip([1.0, 0.0, 0.0]).all(|(g, w)| (g - w).abs() <= 3.0 / 255.0);
            assert!(near_red([got[0], got[1], got[2]]), "{codec} {space_name}: {got:?}");
            let tagged = if code == 9 { "bt2020" } else { "bt709" };
            if let Some(rgb) = matrix_pixel(&path, tagged) {
                assert!(near_red(rgb.map(|c| c as f32 / 255.0)), "{codec} {space_name}: oracle {rgb:?}");
                if code == 9 {
                    let wrong = matrix_pixel(&path, "bt709").expect("same decoder");
                    assert!(!near_red(wrong.map(|c| c as f32 / 255.0)), "{codec} {space_name}: BT.709 also matches {wrong:?}");
                }
            } else {
                eprintln!("{codec} {space_name}: matrix oracle unavailable");
            }
        }
    }
}
