//! Movie exports pinned byte for byte: a comp longer than one render batch, with a rotating
//! stroked ring (mostly transparent, bilinear), a fading half-transparent solid at a fractional
//! position and a Screen-mode shape, in 8 bpc. The comp is at most four macroblock rows high,
//! so the H.264 encoder writes one slice per frame on any number of cores. The hashes are those of the files written before
//! frames rendered while the previous batch encoded, before transparent source tiles were
//! skipped and before blended pixels were quantised as they were drawn: those changes must not
//! move a single byte.

use effectcraft_color::{BlendMode, Label};
use effectcraft_export::render_queue::{OutputFormat, OutputModule, RenderSettings, TimeSpan};
use effectcraft_export::{Job, Sink, export};
use effectcraft_keyframe::{Keyframe, Value};
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{Comp, ItemId, ItemKind, LayerSource, Project, Solid};
use effectcraft_render::NoFootage;
use effectcraft_time::{FrameRate, Tick};

fn fnv1a(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

fn project() -> (Project, ItemId) {
    let mut p = Project::default();
    let mut comp = Comp::new(160, 64, FrameRate::new(30, 1), Tick::from_seconds_f64(2.0));
    comp.background = [0.05, 0.1, 0.3];
    let t = Tick::from_seconds_f64;

    let sid = p.add_item("Half", Label::Red, None, ItemKind::Solid(Solid { color: [0.9, 0.5, 0.1], width: 70, height: 40, pixel_aspect: 1.0 }));
    let mut solid = build::layer(&mut p, &comp, "Half", LayerSource::Solid { item: sid }, (70, 40), None);
    if let Some(pr) = solid.props.prop_mut("transform/position") {
        pr.value = Value::Vec3([60.25, 36.5, 0.0]);
    }
    if let Some(pr) = solid.props.prop_mut("transform/opacity") {
        pr.keys = vec![Keyframe::new(t(0.0), Value::Scalar(20.0)), Keyframe::new(t(2.0), Value::Scalar(70.0))];
    }

    let mut ring = build::layer(&mut p, &comp, "Ring", LayerSource::Shape, (160, 64), None);
    let mut box_ = build::layer(&mut p, &comp, "Box", LayerSource::Shape, (160, 64), None);
    let mut next = p.next_id;
    {
        let mut ids = Ids(&mut next);
        let e = build::shape_ellipse(&mut ids, [56.0, 56.0], [0.0, 0.0]);
        let s = build::shape_stroke(&mut ids, [0.3, 0.8, 1.0, 1.0], 3.0);
        let g = build::shape_group(&mut ids, "Ring", vec![e, s]);
        if let Some(c) = ring.props.sub_mut("contents") {
            c.children.push(g.into());
        }
        let r = build::shape_rect(&mut ids, [30.0, 20.0], [25.0, -10.0], 4.0);
        let f = build::shape_fill(&mut ids, [0.2, 1.0, 0.4, 0.8]);
        let g = build::shape_group(&mut ids, "Box", vec![r, f]);
        if let Some(c) = box_.props.sub_mut("contents") {
            c.children.push(g.into());
        }
    }
    p.next_id = next;
    if let Some(pr) = ring.props.prop_mut("transform/rotation") {
        pr.keys = vec![Keyframe::new(t(0.0), Value::Scalar(0.0)), Keyframe::new(t(2.0), Value::Scalar(130.0))];
    }
    if let Some(pr) = box_.props.prop_mut("transform/position") {
        pr.keys = vec![Keyframe::new(t(0.0), Value::Vec3([70.0, 30.0, 0.0])), Keyframe::new(t(2.0), Value::Vec3([95.5, 38.25, 0.0]))];
    }
    box_.blend_mode = BlendMode::Screen;

    comp.layers = vec![box_, ring, solid];
    let cid = p.add_item("Comp 1", Label::Sandstone, None, ItemKind::Comp(comp.into()));
    (p, cid)
}

fn export_bytes(format: OutputFormat, name: &str) -> Vec<u8> {
    let (p, cid) = project();
    let settings = RenderSettings { time_span: TimeSpan::LengthOfComp, ..Default::default() };
    let om = OutputModule::for_format(format);
    let out = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let file = out.clone();
    let sink: &Sink = &move |_: &str, data: Vec<u8>| *file.lock().unwrap() = data;
    let job = Job {
        project: &p,
        footage: &NoFootage,
        expr: None,
        accel: None,
        comp: cid,
        settings: &settings,
        output: &om,
        path: name,
        sink: Some(sink),
        options: Default::default(),
        nested_switches: true,
    };
    let r = export(&job, &mut |_| true).expect("export");
    assert_eq!(r.frames, 60);
    std::mem::take(&mut *out.lock().unwrap())
}

#[test]
fn h264_movie_bytes_are_pinned() {
    let data = export_bytes(OutputFormat::H264, "/out/pinned.mp4");
    assert_eq!((data.len(), fnv1a(&data)), (104_161, 10_466_446_817_305_548_344), "H.264 MP4");
}

#[test]
fn prores_movie_bytes_are_pinned() {
    let data = export_bytes(OutputFormat::ProRes, "/out/pinned.mov");
    assert_eq!((data.len(), fnv1a(&data)), (270_094, 8_038_709_743_204_075_800), "ProRes MOV");
}
