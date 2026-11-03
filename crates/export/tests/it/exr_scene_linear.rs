//! OpenEXR output holds the comp's values in linear light without Project Settings ▸ HDR
//! (compand / tone map), and decodes with the output space's own curve; 8-bit writers keep the
//! HDR step and their exact bytes.

use std::path::{Path, PathBuf};

use effectcraft_color::{ColorSpace, Conversion, Label};
use effectcraft_export::render_queue::{Channels, OutputFormat, OutputModule, RenderSettings, TimeSpan};
use effectcraft_export::{Job, export};
use effectcraft_project::{BitDepth, Comp, HdrMode, ItemId, ItemKind, LayerSource, Project, Solid, build};
use effectcraft_render::NoFootage;
use effectcraft_time::{FrameRate, Tick};

const W: u32 = 48;
const H: u32 = 16;
/// Three nested bands, outermost first: in range, inside the compand knee, over range.
const BANDS: [[f32; 3]; 3] = [[0.2, 0.5, 0.9], [0.85, 0.9, 0.95], [4.0, 2.0, 0.5]];

fn out_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("effectcraft-exr-scene-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("mkdir");
    d
}

/// A one-frame comp of [`BANDS`] (full width, 2/3, 1/3, centred) with the given colour settings.
fn project(depth: BitDepth, hdr: HdrMode, ws: Option<ColorSpace>, linearize: bool, out: Option<ColorSpace>) -> (Project, ItemId) {
    let mut p = Project::default();
    p.settings.bit_depth = depth;
    p.settings.hdr = hdr;
    p.settings.working_space = ws;
    p.settings.linearize = linearize;
    p.settings.output_space = out;
    let mut comp = Comp::new(W, H, FrameRate::new(10, 1), Tick::from_seconds_f64(0.1));
    comp.background = [0.0, 0.0, 0.0];
    for (i, color) in BANDS.into_iter().enumerate() {
        let w = W - W / 3 * i as u32;
        let sid = p.add_item(&format!("Band {i}"), Label::Red, None, ItemKind::Solid(Solid { color, width: w, height: H, pixel_aspect: 1.0 }));
        let l = build::layer(&mut p, &comp, &format!("Band {i}"), LayerSource::Solid { item: sid }, (w, H), None);
        comp.layers.insert(0, l);
    }
    let cid = p.add_item("Comp 1", Label::Sandstone, None, ItemKind::Comp(comp.into()));
    (p, cid)
}

/// Export the comp's one frame as a `fmt` sequence; the frame file's path.
fn render(p: &Project, cid: ItemId, fmt: OutputFormat, channels: Channels, dir: &Path, ext: &str) -> PathBuf {
    let mut om = OutputModule::for_format(fmt);
    om.channels = channels;
    let s = RenderSettings { time_span: TimeSpan::LengthOfComp, ..Default::default() };
    let path = dir.join(format!("f_[#####].{ext}")).to_string_lossy().to_string();
    let job = Job {
        project: p,
        footage: &NoFootage,
        expr: None,
        accel: None,
        comp: cid,
        settings: &s,
        output: &om,
        path: &path,
        sink: None,
        options: Default::default(),
        nested_switches: true,
    };
    export(&job, &mut |_| true).expect("export");
    dir.join(format!("f_00000.{ext}"))
}

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x0100_0000_01b3))
}

const DEPTHS: [BitDepth; 3] = [BitDepth::Bpc8, BitDepth::Bpc16, BitDepth::Bpc32];
const MODES: [HdrMode; 3] = [HdrMode::Clip, HdrMode::Compand, HdrMode::ToneMap];
const STILLS: [(OutputFormat, Channels, &str); 3] = [
    (OutputFormat::PngSequence, Channels::Rgba, "png"),
    (OutputFormat::TiffSequence, Channels::Rgb, "tif"),
    (OutputFormat::TargaSequence, Channels::Rgba, "tga"),
];

/// The bytes of every 8-bit still for each bit depth × HDR mode, in a managed linear sRGB
/// project with a Display P3 output.
fn still_hashes() -> Vec<u64> {
    let mut out = vec![];
    for depth in DEPTHS {
        for mode in MODES {
            let (p, cid) = project(depth, mode, Some(ColorSpace::Srgb), true, Some(ColorSpace::DisplayP3));
            for (fmt, ch, ext) in STILLS {
                let d = out_dir(&format!("golden-{depth:?}-{mode:?}-{ext}"));
                let bytes = std::fs::read(render(&p, cid, fmt, ch, &d, ext)).expect("read");
                out.push(fnv(&bytes));
                let _ = std::fs::remove_dir_all(&d);
            }
        }
    }
    out
}

/// [`still_hashes`] before float outputs skipped the HDR step: PNG, TIFF and Targa (8-bit
/// writers, which keep compand / tone mapping) are byte for byte the same.
const STILL_GOLDEN: [u64; 27] = [
    0xd0cf67f45d6b6b68,
    0x712d42f3e204cbdb,
    0x021dc457f259eb87,
    0x88a16b1ca2db8805,
    0xc6fb221b89179bdb,
    0x965636561aaabb87,
    0x85c831b7694662db,
    0x8875a5c5232a33db,
    0x42d8f3c110bd0987,
    0xd555a6850fe66f04,
    0x920b49ee0f9900e4,
    0x94e4f43742f2c8ce,
    0xab0792254da43348,
    0x647a60c0a33237ec,
    0x65cba80bc3ecff08,
    0x0726be1b60eb7276,
    0xffd0315633cb88db,
    0x4f6a13131fb7a175,
    0x07a706a7e100b51d,
    0x718600eaa65a6f80,
    0x53489230b732f638,
    0x13024d498aa2552c,
    0x3a3dae30c9e25f83,
    0xc3191a3e860aa0c5,
    0xe5d7d08ecc3ddece,
    0x29a748f729fe5c8a,
    0x78d3b9aa9e107774,
];

#[test]
fn eight_bit_stills_are_unchanged() {
    assert_eq!(still_hashes(), STILL_GOLDEN);
}

/// The decoded samples of a still EXR frame, row-major RGBA.
fn exr_samples(path: &Path) -> Vec<f32> {
    image::open(path).expect("exr").to_rgba32f().into_raw()
}

fn bits_hash(v: &[f32]) -> u64 {
    fnv(&v.iter().flat_map(|x| x.to_bits().to_le_bytes()).collect::<Vec<u8>>())
}

/// Clip-mode EXR samples with an sRGB output, for each colour setup × bit depth × channels.
fn exr_clip_hashes() -> Vec<u64> {
    let mut out = vec![];
    for (ws, linearize) in [(None, false), (Some(ColorSpace::Srgb), false), (Some(ColorSpace::Srgb), true), (Some(ColorSpace::AcesCg), true)] {
        for depth in DEPTHS {
            let (p, cid) = project(depth, HdrMode::Clip, ws, linearize, None);
            for ch in [Channels::Rgba, Channels::Rgb, Channels::Alpha] {
                let d = out_dir(&format!("clip-{ws:?}-{linearize}-{depth:?}-{ch:?}"));
                out.push(bits_hash(&exr_samples(&render(&p, cid, OutputFormat::ExrSequence, ch, &d, "exr"))));
                let _ = std::fs::remove_dir_all(&d);
            }
        }
    }
    out
}

/// [`exr_clip_hashes`] before: Clip-mode EXRs with an sRGB output keep every sample.
const EXR_CLIP_GOLDEN: [u64; 36] = [
    0x9f54fea3d3abcb25,
    0x9f54fea3d3abcb25,
    0x7d40ad9df8600325,
    0x379f5ef9e35ce325,
    0x379f5ef9e35ce325,
    0x7d40ad9df8600325,
    0xb60de6a90263b525,
    0xb60de6a90263b525,
    0x7d40ad9df8600325,
    0x9f54fea3d3abcb25,
    0x9f54fea3d3abcb25,
    0x7d40ad9df8600325,
    0x379f5ef9e35ce325,
    0x379f5ef9e35ce325,
    0x7d40ad9df8600325,
    0xb60de6a90263b525,
    0xb60de6a90263b525,
    0x7d40ad9df8600325,
    0x27858400549ac325,
    0x27858400549ac325,
    0x7d40ad9df8600325,
    0x379f5ef9e35ce325,
    0x379f5ef9e35ce325,
    0x7d40ad9df8600325,
    0xb60de6a90263b525,
    0xb60de6a90263b525,
    0x7d40ad9df8600325,
    0x31d03697119d9325,
    0x31d03697119d9325,
    0x7d40ad9df8600325,
    0xff61dc513c834b25,
    0xff61dc513c834b25,
    0x7d40ad9df8600325,
    0x0c57d24a44f45925,
    0x0c57d24a44f45925,
    0x7d40ad9df8600325,
];

#[test]
fn exr_clip_with_srgb_output_is_unchanged() {
    assert_eq!(exr_clip_hashes(), EXR_CLIP_GOLDEN);
}

/// Compand and tone mapping leave EXR output alone: the file holds the Clip mode's samples, for
/// every bit depth, channel set and colour setup (wide working spaces, a Display P3 output).
#[test]
fn exr_ignores_compand_and_tone_mapping() {
    let setups = [
        (Some(ColorSpace::Srgb), true, None),
        (Some(ColorSpace::Srgb), false, Some(ColorSpace::DisplayP3)),
        (Some(ColorSpace::AcesCg), true, None),
        (Some(ColorSpace::Rec2020), true, Some(ColorSpace::Rec709)),
    ];
    for (ws, linearize, out) in setups {
        for depth in DEPTHS {
            for ch in [Channels::Rgba, Channels::Rgb] {
                let samples = |mode: HdrMode| {
                    let (p, cid) = project(depth, mode, ws, linearize, out);
                    let d = out_dir(&format!("modes-{ws:?}-{linearize}-{out:?}-{depth:?}-{ch:?}-{mode:?}"));
                    let v = exr_samples(&render(&p, cid, OutputFormat::ExrSequence, ch, &d, "exr"));
                    let _ = std::fs::remove_dir_all(&d);
                    v
                };
                let clip = samples(HdrMode::Clip);
                for mode in [HdrMode::Compand, HdrMode::ToneMap] {
                    assert!(samples(mode) == clip, "{ws:?} {linearize} {out:?} {depth:?} {ch:?} {mode:?}");
                }
            }
        }
    }
}

/// The centre pixel (the innermost band) of a one-frame EXR render.
fn exr_centre(p: &Project, cid: ItemId, name: &str) -> [f32; 4] {
    let d = out_dir(name);
    let v = exr_samples(&render(p, cid, OutputFormat::ExrSequence, Channels::Rgba, &d, "exr"));
    let _ = std::fs::remove_dir_all(&d);
    let o = ((H / 2 * W + W / 2) * 4) as usize;
    [v[o], v[o + 1], v[o + 2], v[o + 3]]
}

fn assert_near(got: [f32; 4], want: [f32; 3], what: &str) {
    for i in 0..3 {
        assert!((got[i] - want[i]).abs() <= 1e-4 * want[i].abs().max(1.0), "{what}: channel {i}: got {got:?}, want {want:?}");
    }
    assert_eq!(got[3], 1.0, "{what}: {got:?}");
}

/// In a 32 bpc linear project with compand or tone mapping, the EXR holds the comp's scene-linear
/// values in the output space's primaries: 4.0 stays 4.0. (Solid colours are authored in the
/// working space's encoding; ACES spaces are linear.)
#[test]
fn exr_holds_scene_linear_values_in_output_primaries() {
    let [_, _, band] = BANDS;
    for mode in [HdrMode::Compand, HdrMode::ToneMap] {
        let cases = [
            // ACEScg in and out: nothing converts.
            (ColorSpace::AcesCg, ColorSpace::AcesCg, band),
            // ACEScg working space, Display P3 output: the primaries convert, the values stay linear.
            (ColorSpace::AcesCg, ColorSpace::DisplayP3, Conversion::new(ColorSpace::AcesCg, true, ColorSpace::DisplayP3, true).expect("matrix").apply(band)),
            // Linearized Rec. 2020 working space, sRGB output.
            (ColorSpace::Rec2020, ColorSpace::Srgb, Conversion::new(ColorSpace::Rec2020, false, ColorSpace::Srgb, true).expect("matrix").apply(band)),
            // Linearized sRGB in and out.
            (ColorSpace::Srgb, ColorSpace::Srgb, band.map(effectcraft_color::srgb_to_linear)),
        ];
        for (ws, out, want) in cases {
            let (p, cid) = project(BitDepth::Bpc32, mode, Some(ws), true, Some(out));
            let what = format!("{mode:?} {ws:?} to {out:?}");
            assert_near(exr_centre(&p, cid, &format!("lin-{ws:?}-{out:?}-{mode:?}")), want.map(|v| v.max(0.0)), &what);
        }
    }
    let (p, cid) = project(BitDepth::Bpc32, HdrMode::ToneMap, Some(ColorSpace::AcesCg), true, Some(ColorSpace::AcesCg));
    assert_eq!(exr_centre(&p, cid, "lin-exact")[..3], band, "4.0 is exactly 4.0");
}

/// The EXR is decoded with the output space's curve, not always sRGB's: a Rec. 709 output (pure
/// 2.4 power curve) and an unmanaged project at Working Gamma 2.4 give back the linear values.
#[test]
fn exr_decodes_with_the_output_curve() {
    let [_, _, band] = BANDS;
    // A linearized sRGB working space (same primaries as Rec. 709): the authored colour, decoded.
    let (p, cid) = project(BitDepth::Bpc32, HdrMode::Clip, Some(ColorSpace::Srgb), true, Some(ColorSpace::Rec709));
    assert_near(exr_centre(&p, cid, "curve-709"), band.map(effectcraft_color::srgb_to_linear), "Rec. 709 output");
    // Unmanaged at 2.4: the solid's colour is encoded with the 2.4 power curve.
    let (mut p, cid) = project(BitDepth::Bpc32, HdrMode::Clip, None, false, None);
    p.settings.working_gamma = 2.4;
    assert_near(exr_centre(&p, cid, "curve-gamma24"), band.map(|v| v.powf(2.4)), "Working Gamma 2.4");
    // Working Gamma 2.2 keeps the sRGB curve.
    let (p, cid) = project(BitDepth::Bpc32, HdrMode::Clip, None, false, None);
    assert_near(exr_centre(&p, cid, "curve-gamma22"), band.map(effectcraft_color::srgb_to_linear), "Working Gamma 2.2");
}
