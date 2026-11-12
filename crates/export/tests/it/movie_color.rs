//! H.264, HEVC and AV1 MP4s describe the frames the render path made (#2879): the `colr` box, read
//! back with FilmCraft's demuxer, and the bitstream's own colour description (the SPS VUI, AV1's
//! `color_config`), which ffprobe reports for the first frame of the bare elementary stream when
//! ffmpeg can copy it out and decode it.

use std::path::Path;
use std::process::Command;

use craft_media::isobmff::{ColorInfo, TrackKind};
use effectcraft_color::Label;
use effectcraft_export::render_queue::{OutputFormat, OutputModule, RenderSettings, TimeSpan};
use effectcraft_export::{Job, export};
use effectcraft_project::{ColorSpace, Comp, ItemKind, LayerSource, Project, Solid, build};
use effectcraft_render::NoFootage;
use effectcraft_time::{FrameRate, Tick};

/// (name, Output color space, H.273 primaries / transfer / matrix, the same as ffprobe names them)
const ROWS: &[(&str, Option<ColorSpace>, [u16; 3], [&str; 3])] = &[
    ("default", None, [1, 1, 1], ["bt709", "bt709", "bt709"]),
    ("rec2020", Some(ColorSpace::Rec2020), [9, 1, 1], ["bt2020", "bt709", "bt709"]),
    ("p3", Some(ColorSpace::DisplayP3), [12, 13, 1], ["smpte432", "iec61966-2-1", "bt709"]),
    ("pq", Some(ColorSpace::Rec2100Pq), [9, 16, 1], ["bt2020", "smpte2084", "bt709"]),
    ("hlg", Some(ColorSpace::Rec2100Hlg), [9, 18, 1], ["bt2020", "arib-std-b67", "bt709"]),
];

fn render(output: Option<ColorSpace>, format: OutputFormat, path: &Path) {
    let mut p = Project::default();
    p.settings.working_space = Some(ColorSpace::Srgb);
    p.settings.output_space = output;
    let mut comp = Comp::new(64, 48, FrameRate::new(10, 1), Tick::from_seconds_f64(0.2));
    let sid = p.add_item("Solid", Label::Red, None, ItemKind::Solid(Solid { color: [0.75, 0.25, 0.5], width: 64, height: 48, pixel_aspect: 1.0 }));
    comp.layers.push(build::layer(&mut p, &comp, "Solid", LayerSource::Solid { item: sid }, (64, 48), None));
    let comp = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.into()));
    let settings = RenderSettings { time_span: TimeSpan::LengthOfComp, ..Default::default() };
    let om = OutputModule::for_format(format);
    let out = path.to_string_lossy().to_string();
    let job = Job {
        project: &p,
        footage: &NoFootage,
        expr: None,
        accel: None,
        comp,
        settings: &settings,
        output: &om,
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
