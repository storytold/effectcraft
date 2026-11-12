//! Targa sequence output: every file is an uncompressed TGA (Truevision TGA 2.0 specification:
//! an 18-byte header, BGR(A) pixels, top-left origin, the alpha bits in the image descriptor)
//! holding exactly the pixels the lossless PNG sequence of the same job writes, and EffectCraft's
//! own Targa import (#745) reads the frames back as the same picture.
//!
//! The comp is 7×4 @ 10 fps, 3 frames: frame 0 is the background alone, frames 1 and 2 add a
//! 2×2 solid in the top two rows (so a flipped picture shows), on a half pixel horizontally, so
//! its left and right columns carry partial alpha.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use effectcraft_color::Label;
use effectcraft_export::render_queue::{AlphaMode, Channels, OutputFormat, OutputModule, RenderSettings, TimeSpan, sequence_path};
use effectcraft_export::{Job, export};
use effectcraft_keyframe::Value;
use effectcraft_media::MediaPool;
use effectcraft_project::{Comp, Footage, FootageKind, ItemId, ItemKind, LayerSource, Project, Solid, build};
use effectcraft_render::NoFootage;
use effectcraft_time::{FrameRate, Tick};

const W: u32 = 7;
const H: u32 = 4;
const FRAMES: u64 = 3;

fn out_dir(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out/export").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("mkdir");
    d
}

fn project() -> (Project, ItemId) {
    let mut p = Project::default();
    let mut comp = Comp::new(W, H, FrameRate::new(10, 1), Tick::from_seconds_f64(FRAMES as f64 / 10.0));
    comp.background = [0.1, 0.3, 0.6];
    let sid = p.add_item("Red", Label::Red, None, ItemKind::Solid(Solid { color: [0.9, 0.2, 0.1], width: 2, height: 2, pixel_aspect: 1.0 }));
    let mut l = build::layer(&mut p, &comp, "Red", LayerSource::Solid { item: sid }, (2, 2), None);
    l.props.prop_mut("transform/position").expect("position").value = Value::Vec3([3.5, 1.0, 0.0]);
    l.in_point = comp.frame_rate.tick_of(1);
    comp.layers.push(l);
    let cid = p.add_item("Comp 1", Label::Sandstone, None, ItemKind::Comp(comp.into()));
    (p, cid)
}

/// Render the comp through `om` to `dir/f_[#####].<ext>`; the first frame's path.
fn render(p: &Project, cid: ItemId, om: &OutputModule, dir: &Path) -> String {
    let s = RenderSettings { time_span: TimeSpan::LengthOfComp, ..Default::default() };
    let path = dir.join(format!("f_[#####].{}", om.format.extension())).to_string_lossy().to_string();
    let job = Job {
        project: p,
        footage: &NoFootage,
        expr: None,
        accel: None,
        comp: cid,
        settings: &s,
        output: om,
        path: &path,
        sink: None,
        options: Default::default(),
        nested_switches: true,
    };
    let r = export(&job, &mut |_| true).expect("export");
    assert_eq!((r.frames, r.width, r.height), (FRAMES, W, H));
    r.path
}

/// The numbered files of a render of `ext`: frames 0..FRAMES and nothing after.
fn frame_paths(dir: &Path, ext: &str) -> Vec<String> {
    let template = dir.join(format!("f_[#####].{ext}")).to_string_lossy().to_string();
    assert!(!Path::new(&sequence_path(&template, FRAMES as i64)).exists(), "no frame after the last");
    (0..FRAMES as i64).map(|k| sequence_path(&template, k)).collect()
}

/// A Targa file's header fields and its pixels as top-down RGBA bytes.
struct Tga {
    image_type: u8,
    width: u16,
    height: u16,
    depth: u8,
    descriptor: u8,
    rgba: Vec<u8>,
}

fn read_tga(path: &str) -> Tga {
    let b = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let u16_at = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    assert_eq!((b[0], b[1]), (0, 0), "{path}: no image ID, no colour map");
    assert_eq!((u16_at(8), u16_at(10)), (0, 0), "{path}: X / Y origin");
    let (width, height, depth, descriptor) = (u16_at(12), u16_at(14), b[16], b[17]);
    let bpp = usize::from(depth / 8);
    // Uncompressed, without an image ID, colour map or 2.0 footer: header and pixels only.
    assert_eq!(b.len(), 18 + usize::from(width) * usize::from(height) * bpp, "{path}: file size");
    let rgba = b[18..]
        .chunks(bpp)
        .flat_map(|p| match p {
            [b, g, r, a] => [*r, *g, *b, *a],
            [b, g, r] => [*r, *g, *b, 255],
            [v] => [*v, *v, *v, 255],
            _ => panic!("{path}: {depth}-bit pixels"),
        })
        .collect();
    Tga { image_type: b[2], width, height, depth, descriptor, rgba }
}

fn png_rgba(path: &str) -> Vec<u8> {
    image::open(path).unwrap_or_else(|e| panic!("{path}: {e}")).to_rgba8().into_raw()
}

#[test]
fn targa_sequence_writes_the_png_sequence_pixels() {
    let (p, cid) = project();
    for (channels, alpha_mode, image_type, depth, descriptor) in [
        (Channels::Rgb, AlphaMode::Straight, 2, 24, 0x20),
        (Channels::Rgba, AlphaMode::Straight, 2, 32, 0x28),
        (Channels::Rgba, AlphaMode::Premultiplied, 2, 32, 0x28),
        (Channels::Alpha, AlphaMode::Straight, 3, 8, 0x20),
    ] {
        let name = format!("tga-{channels:?}-{alpha_mode:?}");
        let tga_dir = out_dir(&name);
        let png_dir = out_dir(&format!("{name}-png"));
        let om = OutputModule { channels, alpha_mode, ..OutputModule::for_format(OutputFormat::TargaSequence) };
        let first = render(&p, cid, &om, &tga_dir);
        assert!(first.ends_with("f_00000.tga"), "{first}");
        render(&p, cid, &OutputModule { format: OutputFormat::PngSequence, ..om.clone() }, &png_dir);
        let mut frames = Vec::new();
        for (tga, png) in frame_paths(&tga_dir, "tga").iter().zip(frame_paths(&png_dir, "png")) {
            let t = read_tga(tga);
            assert_eq!((t.image_type, t.width, t.height, t.depth, t.descriptor), (image_type, W as u16, H as u16, depth, descriptor), "{tga}");
            assert_eq!(t.rgba, png_rgba(&png), "{tga} matches {png}");
            frames.push(t.rgba);
        }
        // Frame 0 is background only; frames 1 and 2 hold the solid, so the numbering follows
        // the comp's frames.
        let at = |f: &[u8], x: u32, y: u32| -> [u8; 4] {
            let i = ((y * W + x) * 4) as usize;
            [f[i], f[i + 1], f[i + 2], f[i + 3]]
        };
        assert_ne!(at(&frames[0], 3, 1), at(&frames[1], 3, 1), "{name}");
        assert_eq!(frames[1], frames[2], "{name}");
        // Top-down rows: the solid is in the top half, the bottom rows are still background.
        assert_eq!(at(&frames[1], 3, 3), at(&frames[0], 3, 3), "{name}: bottom row");
        match channels {
            Channels::Rgb => {
                assert_eq!(at(&frames[0], 0, 0)[3], 255);
                assert_eq!(at(&frames[1], 3, 1), [230, 51, 26, 255], "{name}: the opaque solid");
            }
            Channels::Rgba => {
                assert_eq!(at(&frames[0], 0, 0), [0, 0, 0, 0], "{name}: transparent background");
                assert_eq!(at(&frames[1], 3, 1), [230, 51, 26, 255], "{name}: the opaque solid");
                let edge = at(&frames[1], 2, 1);
                assert!(edge[3] > 0 && edge[3] < 255, "{name}: partial alpha at the solid's edge {edge:?}");
                if alpha_mode == AlphaMode::Straight {
                    // Unmatted from the renderer's premultiplied pixel: the solid's colour, within rounding.
                    assert!(edge[..3].iter().zip([230u8, 51, 26]).all(|(a, b)| a.abs_diff(b) <= 1), "{name}: straight colour {edge:?}");
                } else {
                    assert!(edge[0] < 230 && edge[0] <= edge[3], "{name}: premultiplied colour {edge:?}");
                }
            }
            Channels::Alpha => {
                assert_eq!(at(&frames[0], 0, 0), [0, 0, 0, 255], "{name}: no matte yet");
                assert_eq!(at(&frames[1], 3, 1), [255, 255, 255, 255], "{name}: the solid's matte");
            }
        }
    }
}

/// Numbered stills read back as an image sequence through the media layer (Targa: #745).
fn import_sequence(paths: &[String]) -> (Footage, MediaPool) {
    let pool = MediaPool::new();
    pool.set_read_ahead(false);
    let first = effectcraft_media::probe(&paths[0]).expect("probe our output");
    assert_eq!((first.kind, first.width, first.height), (FootageKind::Still, W, H));
    for p in paths {
        let bytes: Arc<[u8]> = std::fs::read(p).expect("read").into();
        pool.add_bytes(p, bytes);
    }
    let mut footage = Footage { kind: FootageKind::Sequence, sequence: paths.to_vec(), ..first };
    footage.sync_sequence_duration();
    (footage, pool)
}

#[test]
fn targa_sequence_round_trips_through_targa_import() {
    let (p, cid) = project();
    for alpha_mode in [AlphaMode::Straight, AlphaMode::Premultiplied] {
        let name = format!("tga-import-{alpha_mode:?}");
        let tga_dir = out_dir(&name);
        let png_dir = out_dir(&format!("{name}-png"));
        let om = OutputModule { channels: Channels::Rgba, alpha_mode, ..OutputModule::for_format(OutputFormat::TargaSequence) };
        render(&p, cid, &om, &tga_dir);
        // The reference: the same job as a straight-alpha PNG sequence (PNG's alpha is labelled).
        render(&p, cid, &OutputModule { format: OutputFormat::PngSequence, alpha_mode: AlphaMode::Straight, ..om.clone() }, &png_dir);
        let (mut tga, tga_pool) = import_sequence(&frame_paths(&tga_dir, "tga"));
        let (png, png_pool) = import_sequence(&frame_paths(&png_dir, "png"));
        assert_eq!(tga.codec, "TGA");
        assert_eq!(tga.sequence_frames(), FRAMES as i64);
        // Targa doesn't label its alpha: import guesses straight, and Interpret Footage sets
        // premultiplied for a premultiplied render.
        assert_eq!(tga.alpha, effectcraft_project::AlphaMode::Straight);
        if alpha_mode == AlphaMode::Premultiplied {
            tga.alpha = effectcraft_project::AlphaMode::Premultiplied;
        }
        // Premultiplying to 8 bits rounds partially transparent colour.
        let tol = if alpha_mode == AlphaMode::Straight { 0.0 } else { 1.0 / 255.0 };
        for k in 0..FRAMES as i64 {
            let a = tga_pool.frame_at(&tga, tga.frame_rate.tick_of(k)).expect("decode Targa");
            let b = png_pool.frame_at(&png, png.frame_rate.tick_of(k)).expect("decode PNG");
            assert_eq!((a.width, a.height), (W, H));
            for (x, y) in a.data.iter().zip(&b.data) {
                assert!(x.iter().zip(y).all(|(x, y)| (x - y).abs() <= tol), "{name} frame {k}: {x:?} != {y:?}");
            }
        }
        let opaque = tga_pool.frame_at(&tga, tga.frame_rate.tick_of(1)).expect("decode");
        assert_eq!(opaque.data[opaque.idx(0, 0)][3], 0.0, "{name}: the background stays transparent");
        assert_eq!(opaque.data[opaque.idx(3, 1)][3], 1.0, "{name}: the solid is opaque");
    }
}
