//! Gradients exported at 16 bpc (or with a 16 bpc Color Depth override) don't band (#2022): the
//! float frame is dithered on its way to 8 bits, and the Render Settings' Color Depth reaches the
//! render itself.

use std::sync::{Arc, Mutex};

use effectcraft_color::Label;
use effectcraft_export::render_queue::*;
use effectcraft_export::{Job, JobOptions, Sink, export};
use effectcraft_keyframe::Value;
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{BitDepth, Comp, ItemId, ItemKind, LayerSource, Project, Solid};
use effectcraft_render::NoFootage;
use effectcraft_time::{FrameRate, Tick};

const W: u32 = 1024;
const H: u32 = 16;
/// The ramp runs from this grey to `HI` across the comp: about 25 eight-bit codes in 1024 px,
/// so plain rounding makes bands about 40 px wide.
const LO: f64 = 0.20;
const HI: f64 = 0.30;
/// Width of the column blocks whose mean is compared with the ideal ramp.
const WINDOW: u32 = 8;

/// A grey horizontal ramp over a comp of the given project depth.
fn ramp_project(depth: BitDepth) -> (Project, ItemId) {
    let mut p = Project::default();
    p.settings.bit_depth = depth;
    let mut comp = Comp::new(W, H, FrameRate::new(25, 1), Tick::from_seconds_f64(1.0));
    comp.background = [0.0, 0.0, 0.0];
    let sid = p.add_item("Ramp", Label::Red, None, ItemKind::Solid(Solid { color: [1.0, 1.0, 1.0], width: W, height: H, pixel_aspect: 1.0 }));
    let mut layer = build::layer(&mut p, &comp, "Ramp", LayerSource::Solid { item: sid }, (W, H), None);
    let spec = effectcraft_effects::find("ec.generate.gradientramp").expect("gradient ramp effect");
    let mut next = p.next_id;
    let mut group = effectcraft_effects::instantiate(spec, &mut Ids(&mut next), spec.name, [W as f64, H as f64]);
    p.next_id = next;
    for (k, v) in [
        ("start", Value::Vec2([0.0, 0.0])),
        ("end", Value::Vec2([W as f64, 0.0])),
        ("startColor", Value::Color([LO, LO, LO, 1.0])),
        ("endColor", Value::Color([HI, HI, HI, 1.0])),
    ] {
        group.get_mut(k).expect("ramp parameter").value = v;
    }
    layer.props.sub_mut("effects").expect("effects group").children.push(group.into());
    comp.layers.push(layer);
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.into()));
    (p, cid)
}

/// Export frame 0 as PNG with the Render Settings' Color Depth `depth` and return the grey of
/// every row.
fn export_rows(p: &Project, cid: ItemId, depth: ColorDepth) -> Vec<Vec<u8>> {
    let files: Arc<Mutex<Vec<(String, Vec<u8>)>>> = Default::default();
    let f = files.clone();
    let sink: Box<Sink> = Box::new(move |path: &str, data: Vec<u8>| f.lock().unwrap().push((path.to_string(), data)));
    let settings =
        RenderSettings { time_span: TimeSpan::Custom { start: Tick::ZERO, end: Tick::from_seconds_f64(0.04) }, color_depth: depth, ..Default::default() };
    let om = OutputModule::for_format(OutputFormat::PngSequence);
    let job = Job {
        project: p,
        footage: &NoFootage,
        expr: None,
        accel: None,
        comp: cid,
        settings: &settings,
        output: &om,
        path: "/banding/f_[#####].png",
        sink: Some(&*sink),
        options: JobOptions::default(),
        nested_switches: true,
    };
    export(&job, &mut |_| true).expect("export");
    let files = files.lock().unwrap();
    let (_, png) = files.iter().find(|(path, _)| path.ends_with(".png")).expect("a PNG");
    let img = image::load_from_memory(png).expect("png").to_rgba8();
    (0..H).map(|y| (0..W).map(|x| img.get_pixel(x, y).0[1]).collect()).collect()
}

/// The ideal ramp at column `x`, in 8-bit codes.
fn ideal(x: f64) -> f64 {
    255.0 * (LO + (HI - LO) * (x + 0.5) / W as f64)
}

/// Longest run of equal neighbours in the row: a band's width.
fn longest_run(row: &[u8]) -> usize {
    let (mut best, mut run) = (1, 1);
    for w in row.windows(2) {
        run = if w[0] == w[1] { run + 1 } else { 1 };
        best = best.max(run);
    }
    best
}

/// Horizontal neighbours with different values, over all rows: ~25 per row when every band is a
/// flat step, hundreds when the steps are dithered.
fn transitions(rows: &[Vec<u8>]) -> usize {
    rows.iter().map(|r| r.windows(2).filter(|w| w[0] != w[1]).count()).sum()
}

/// Largest difference, in 8-bit codes, between the mean of a `WINDOW`-column block over all rows
/// and the ideal ramp's mean over it. Plain rounding leaves up to ~0.4 codes of it beside a step
/// edge; dither averages it away (its noise on a mean of `WINDOW * H` samples is under 0.05).
fn worst_window_error(rows: &[Vec<u8>]) -> f64 {
    (0..W / WINDOW)
        .map(|i| {
            let (x0, x1) = ((i * WINDOW) as usize, ((i + 1) * WINDOW) as usize);
            let sum: f64 = rows.iter().flat_map(|r| &r[x0..x1]).map(|&v| v as f64).sum();
            let mean = sum / (rows.len() * (x1 - x0)) as f64;
            (mean - ideal((x0 + x1) as f64 / 2.0 - 1.0)).abs()
        })
        .fold(0.0, f64::max)
}

#[test]
fn a_16_bpc_gradient_export_is_dithered() {
    let (p, cid) = ramp_project(BitDepth::Bpc16);
    let rows = export_rows(&p, cid, ColorDepth::Current);
    let (steps, err) = (transitions(&rows), worst_window_error(&rows));
    eprintln!("16 bpc project: {steps} transitions, worst {WINDOW}-px mean error {err:.3} codes");
    // Undithered there are ~25 flat steps per row (400 transitions) and the block means are off
    // by up to ~0.4 codes.
    assert!(steps > 2000, "the steps survive: {steps} transitions");
    assert!(err < 0.25, "the local mean is off by {err} codes");
    // Dither never moves a value a whole code from the ideal ramp.
    for row in &rows {
        for (x, &v) in row.iter().enumerate() {
            let want = ideal(x as f64);
            assert!((v as f64 - want).abs() < 1.05, "x {x}: {v} vs {want}");
        }
    }
}

#[test]
fn the_export_is_the_same_every_time() {
    let (p, cid) = ramp_project(BitDepth::Bpc16);
    assert_eq!(export_rows(&p, cid, ColorDepth::Current), export_rows(&p, cid, ColorDepth::Current));
}

#[test]
fn the_render_color_depth_reaches_the_render() {
    // An 8 bpc project renders every layer at 8 bits: flat steps, nothing for the output to dither.
    let (p, cid) = ramp_project(BitDepth::Bpc8);
    let banded = export_rows(&p, cid, ColorDepth::Current);
    assert!(longest_run(&banded[0]) >= 30, "an 8 bpc render is quantised before export");
    assert!(transitions(&banded) < 16 * 30);
    // The Render Settings' 16 bpc depth overrides the project's: the render keeps its precision
    // until the final conversion, which dithers.
    let deep = export_rows(&p, cid, ColorDepth::Bpc16);
    let (steps, err) = (transitions(&deep), worst_window_error(&deep));
    eprintln!("8 bpc project, 16 bpc render: {steps} transitions, worst {WINDOW}-px mean error {err:.3} codes");
    assert!(steps > 2000 && err < 0.25, "a 16 bpc render still bands: {steps} transitions, error {err}");
    // And 8 bpc on a 16 bpc project renders at 8 bits.
    let (p16, cid16) = ramp_project(BitDepth::Bpc16);
    assert_eq!(export_rows(&p16, cid16, ColorDepth::Bpc8), banded);
}
