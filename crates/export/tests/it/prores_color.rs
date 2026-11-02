//! ProRes `colr` atom (#2801): it describes the frames the render path made, read back with
//! FilmCraft's demuxer and, when installed, ffprobe as an external oracle.

use std::path::Path;
use std::process::Command;

use effectcraft_color::Label;
use effectcraft_export::render_queue::{Channels, OutputFormat, OutputModule, RenderSettings, TimeSpan};
use effectcraft_export::{Job, export};
use effectcraft_project::{ColorSpace, Comp, ItemKind, LayerSource, Project, Solid, build};
use effectcraft_render::NoFootage;
use effectcraft_time::{FrameRate, Tick};
use filmcraft_isobmff::{ColorInfo, TrackKind};

struct Row {
    name: &'static str,
    working: Option<ColorSpace>,
    output: Option<ColorSpace>,
    channels: Channels,
    /// H.273 primaries, transfer and matrix.
    codes: [u16; 3],
    /// The same as ffprobe names them.
    names: [&'static str; 3],
}

const SRGB: Option<ColorSpace> = Some(ColorSpace::Srgb);
const PQ: Option<ColorSpace> = Some(ColorSpace::Rec2100Pq);
const BT709: [&str; 3] = ["bt709", "bt709", "bt709"];

const ROWS: &[Row] = &[
    Row { name: "unmanaged", working: None, output: None, channels: Channels::Rgb, codes: [1, 1, 1], names: BT709 },
    Row { name: "unmanaged_ignores_output", working: None, output: PQ, channels: Channels::Rgb, codes: [1, 1, 1], names: BT709 },
    Row { name: "srgb_output_stays_1_1_1", working: SRGB, output: None, channels: Channels::Rgb, codes: [1, 1, 1], names: BT709 },
    Row { name: "rec709", working: SRGB, output: Some(ColorSpace::Rec709), channels: Channels::Rgb, codes: [1, 1, 1], names: BT709 },
    Row { name: "rec2020", working: SRGB, output: Some(ColorSpace::Rec2020), channels: Channels::Rgb, codes: [9, 1, 1], names: ["bt2020", "bt709", "bt709"] },
    Row {
        name: "p3",
        working: SRGB,
        output: Some(ColorSpace::DisplayP3),
        channels: Channels::Rgb,
        codes: [12, 13, 1],
        names: ["smpte432", "iec61966-2-1", "bt709"],
    },
    Row { name: "pq", working: SRGB, output: PQ, channels: Channels::Rgb, codes: [9, 16, 1], names: ["bt2020", "smpte2084", "bt709"] },
    Row {
        name: "hlg",
        working: SRGB,
        output: Some(ColorSpace::Rec2100Hlg),
        channels: Channels::Rgb,
        codes: [9, 18, 1],
        names: ["bt2020", "arib-std-b67", "bt709"],
    },
    Row { name: "aces", working: SRGB, output: Some(ColorSpace::AcesCg), channels: Channels::Rgb, codes: [2, 8, 1], names: ["unknown", "linear", "bt709"] },
    Row { name: "pq_with_alpha", working: SRGB, output: PQ, channels: Channels::Rgba, codes: [9, 16, 1], names: ["bt2020", "smpte2084", "bt709"] },
    Row { name: "alpha_matte", working: SRGB, output: PQ, channels: Channels::Alpha, codes: [1, 1, 1], names: BT709 },
];

fn render(row: &Row, path: &Path) {
    let mut p = Project::default();
    p.settings.working_space = row.working;
    p.settings.output_space = row.output;
    let mut comp = Comp::new(64, 48, FrameRate::new(10, 1), Tick::from_seconds_f64(0.2));
    let sid = p.add_item("Solid", Label::Red, None, ItemKind::Solid(Solid { color: [0.75, 0.25, 0.5], width: 64, height: 48, pixel_aspect: 1.0 }));
    comp.layers.push(build::layer(&mut p, &comp, "Solid", LayerSource::Solid { item: sid }, (64, 48), None));
    let comp = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.into()));
    let settings = RenderSettings { time_span: TimeSpan::LengthOfComp, ..Default::default() };
    let output = OutputModule { channels: row.channels, ..OutputModule::for_format(OutputFormat::ProRes) };
    let out = path.to_string_lossy().to_string();
    let job = Job {
        project: &p,
        footage: &NoFootage,
        expr: None,
        accel: None,
        comp,
        settings: &settings,
        output: &output,
        path: &out,
        sink: None,
        options: Default::default(),
        nested_switches: true,
    };
    export(&job, &mut |_| true).expect("export");
}

/// ffprobe's primaries, transfer and matrix for the stream, or `None` without ffprobe.
fn ffprobe_tags(path: &Path) -> Option<Vec<String>> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=color_primaries,color_transfer,color_space", "-of", "default=nw=1"])
        .arg(path)
        .output()
        .ok()?;
    assert!(out.status.success(), "ffprobe rejected {}: {}", path.display(), String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let tag = |key: &str| text.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix('=')).unwrap_or("").to_string();
    Some(vec![tag("color_primaries"), tag("color_transfer"), tag("color_space")])
}

#[test]
fn prores_colr_describes_the_frames() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out/export/prores_color");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    for row in ROWS {
        let path = dir.join(format!("{}.mov", row.name));
        render(row, &path);

        let bytes = std::fs::read(&path).expect("read movie");
        let mov = filmcraft_isobmff::open(bytes.as_slice()).expect("demux");
        let track = mov.tracks.iter().find(|t| t.kind == TrackKind::Video).expect("video track");
        let [primaries, transfer, matrix] = row.codes;
        let colr = track.entries[0].video.as_ref().and_then(|v| v.color.clone());
        assert_eq!(colr, Some(ColorInfo::Nclc { primaries, transfer, matrix }), "{}: colr", row.name);

        if let Some(stream) = ffprobe_tags(&path) {
            assert_eq!(stream, row.names, "{}: ffprobe stream tags", row.name);
        }
    }
}
