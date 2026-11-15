//! ProRes `colr` atom (#2801): it describes the frames the render path made, read back with
//! FilmCraft's demuxer and, when installed, ffprobe as an external oracle. A PQ or HLG render
//! read back into a managed project has the colours it was rendered from (#2849).

use std::path::Path;
use std::process::Command;

use craft_media::isobmff::{ColorInfo, TrackKind};
use effectcraft_color::Label;
use effectcraft_export::render_queue::{Channels, OutputFormat, OutputModule, RenderSettings, TimeSpan, sequence_path};
use effectcraft_export::{Job, export};
use effectcraft_media::MediaPool;
use effectcraft_project::{ColorSpace, Comp, ItemKind, LayerSource, Project, Solid, build};
use effectcraft_render::{FootageSource, NoFootage};
use effectcraft_time::{FrameRate, Tick};

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

/// The solid every row renders; with the default output it comes out as these values.
const SOLID: [f32; 3] = [0.75, 0.25, 0.5];

fn render(row: &Row, path: &Path) {
    let mut p = Project::default();
    p.settings.working_space = row.working;
    p.settings.output_space = row.output;
    let mut comp = Comp::new(64, 48, FrameRate::new(10, 1), Tick::from_seconds_f64(0.2));
    let sid = p.add_item("Solid", Label::Red, None, ItemKind::Solid(Solid { color: SOLID, width: 64, height: 48, pixel_aspect: 1.0 }));
    comp.layers.push(build::layer(&mut p, &comp, "Solid", LayerSource::Solid { item: sid }, (64, 48), None));
    let comp = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.into()));
    let output = OutputModule { channels: row.channels, ..OutputModule::for_format(OutputFormat::ProRes) };
    run(&p, comp, &NoFootage, &output, path);
}

fn run(p: &Project, comp: effectcraft_project::ItemId, footage: &dyn FootageSource, output: &OutputModule, path: &Path) {
    let settings = RenderSettings { time_span: TimeSpan::LengthOfComp, ..Default::default() };
    let out = path.to_string_lossy().to_string();
    let job = Job {
        project: p,
        footage,
        expr: None,
        accel: None,
        comp,
        settings: &settings,
        output,
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
        let mov = craft_media::isobmff::open(bytes.as_slice()).expect("demux");
        let track = mov.tracks.iter().find(|t| t.kind == TrackKind::Video).expect("video track");
        let [primaries, transfer, matrix] = row.codes;
        let colr = track.entries[0].video.as_ref().and_then(|v| v.color.clone());
        assert_eq!(colr, Some(ColorInfo::Nclc { primaries, transfer, matrix }), "{}: colr", row.name);

        if let Some(stream) = ffprobe_tags(&path) {
            assert_eq!(stream, row.names, "{}: ffprobe stream tags", row.name);
        }
    }
}

/// #2849: a PQ or HLG render, imported into a project with an sRGB working space, gets the
/// Rec. 2100 profile its `colr` atom names and shows the solid it was rendered from.
#[test]
fn hdr_renders_reimport_with_their_colours() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out/export/prores_hdr_reimport");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    let want = SOLID.map(|v| (v * 255.0 + 0.5) as i32);
    for (name, space) in [("pq", ColorSpace::Rec2100Pq), ("hlg", ColorSpace::Rec2100Hlg)] {
        let row = ROWS.iter().find(|r| r.name == name).expect("row");
        let movie = dir.join(format!("{name}.mov"));
        render(row, &movie);

        let footage = effectcraft_media::probe(&movie).expect("probe");
        let profile = footage.color_profile;
        let mut p = Project::default();
        p.settings.working_space = SRGB;
        let mut comp = Comp::new(64, 48, FrameRate::new(10, 1), Tick::from_seconds_f64(0.1));
        let fid = p.add_item(name, Label::SeaFoam, None, ItemKind::Footage(footage));
        comp.layers.push(build::layer(&mut p, &comp, name, LayerSource::Footage { item: fid }, (64, 48), None));
        let comp = p.add_item("Reimport", Label::Sandstone, None, ItemKind::Comp(comp.into()));
        let pattern = dir.join(format!("{name}_[#####].png")).to_string_lossy().to_string();
        run(&p, comp, &MediaPool::new(), &OutputModule::for_format(OutputFormat::PngSequence), Path::new(&pattern));

        let png = image::open(sequence_path(&pattern, 0)).expect("png").to_rgb8();
        let got = png.get_pixel(32, 24).0.map(i32::from);
        assert!(got.iter().zip(want).all(|(g, w)| (g - w).abs() <= 2), "{name}: centre {got:?}, want {want:?} (profile {profile:?})");
        assert_eq!(profile, Some(space), "{name}: profile");
    }
}
