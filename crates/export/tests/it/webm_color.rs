//! WebM renders (VP9 and AV1) describe the frames the render path made in the Video track's
//! Matroska `Colour` element (#2925), read back with FilmCraft's demuxer and, when installed,
//! ffprobe as an external oracle.

use std::path::Path;
use std::process::Command;

use effectcraft_color::Label;
use effectcraft_export::render_queue::{OutputFormat, OutputModule, RenderSettings, TimeSpan, WebmVideoCodec};
use effectcraft_export::{Job, export};
use effectcraft_project::{ColorSpace, Comp, ItemKind, LayerSource, Project, Solid, build};
use effectcraft_render::NoFootage;
use effectcraft_time::{FrameRate, Tick};

/// (name, Output color space, H.273 primaries / transfer / matrix, the same as ffprobe names them)
const ROWS: &[(&str, Option<ColorSpace>, [u64; 3], [&str; 3])] = &[
    ("default", None, [1, 1, 1], ["bt709", "bt709", "bt709"]),
    ("rec2020", Some(ColorSpace::Rec2020), [9, 1, 1], ["bt2020", "bt709", "bt709"]),
    ("p3", Some(ColorSpace::DisplayP3), [12, 13, 1], ["smpte432", "iec61966-2-1", "bt709"]),
    ("pq", Some(ColorSpace::Rec2100Pq), [9, 16, 1], ["bt2020", "smpte2084", "bt709"]),
    ("hlg", Some(ColorSpace::Rec2100Hlg), [9, 18, 1], ["bt2020", "arib-std-b67", "bt709"]),
];

fn render(output: Option<ColorSpace>, codec: WebmVideoCodec, path: &Path) {
    let mut p = Project::default();
    p.settings.working_space = Some(ColorSpace::Srgb);
    p.settings.output_space = output;
    let mut comp = Comp::new(64, 48, FrameRate::new(10, 1), Tick::from_seconds_f64(0.2));
    let sid = p.add_item("Solid", Label::Red, None, ItemKind::Solid(Solid { color: [0.75, 0.25, 0.5], width: 64, height: 48, pixel_aspect: 1.0 }));
    comp.layers.push(build::layer(&mut p, &comp, "Solid", LayerSource::Solid { item: sid }, (64, 48), None));
    let comp = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.into()));
    let settings = RenderSettings { time_span: TimeSpan::LengthOfComp, ..Default::default() };
    let om = OutputModule { webm_codec: codec, ..OutputModule::for_format(OutputFormat::WebM) };
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

/// ffprobe's primaries, transfer, matrix and range for the video stream, or `None` without ffprobe.
fn ffprobe_tags(path: &Path) -> Option<Vec<String>> {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=color_primaries,color_transfer,color_space,color_range",
            "-of",
            "default=nw=1",
        ])
        .arg(path)
        .output()
        .ok()?;
    assert!(out.status.success(), "ffprobe rejected {}: {}", path.display(), String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let tag = |key: &str| text.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix('=')).unwrap_or("").to_string();
    Some(vec![tag("color_primaries"), tag("color_transfer"), tag("color_space"), tag("color_range")])
}

#[test]
fn webm_colour_describes_the_frames() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out/export/webm_color");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    for (codec, label) in [(WebmVideoCodec::Vp9, "vp9"), (WebmVideoCodec::Av1, "av1")] {
        for &(name, output, [primaries, transfer, matrix], names) in ROWS {
            let path = dir.join(format!("{label}_{name}.webm"));
            render(output, codec, &path);

            let bytes = std::fs::read(&path).expect("read movie");
            let mkv = filmcraft_matroska::open(bytes).expect("demux");
            let colour = mkv.tracks.iter().find_map(|t| t.video.as_ref()).and_then(|v| v.colour.clone()).unwrap_or_default();
            assert_eq!(
                (colour.primaries, colour.transfer_characteristics, colour.matrix_coefficients, colour.range),
                (Some(primaries), Some(transfer), Some(matrix), Some(1)),
                "{label} {name}: Colour"
            );

            if let Some(tags) = ffprobe_tags(&path) {
                assert_eq!(tags, [names[0], names[1], names[2], "tv"], "{label} {name}: ffprobe stream tags");
            }
        }
    }
}
