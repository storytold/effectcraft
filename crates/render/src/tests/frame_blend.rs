//! Frame blending (Frame Mix and Pixel Motion) for footage and precomp layers.

use std::sync::Arc;

use rayon::prelude::*;

use effectcraft_color::Label;
use effectcraft_keyframe::{Keyframe, Value};
use effectcraft_project::build;
use effectcraft_project::{AlphaMode, BitDepth, Comp, Footage, FootageKind, FrameBlend, ItemId, ItemKind, LayerSource, Project, Solid};
use effectcraft_time::{FrameRate, Tick};

use crate::{FootageSource, Image, RenderOpts, Renderer, frame_position};

#[test]
fn frame_mix_weights_for_24_to_30_conform() {
    // Comp frame k at 30 fps sits at source frame 0.8·k of 24 fps footage.
    let want = [(0, 0.0), (0, 0.8), (1, 0.6), (2, 0.4), (3, 0.2), (4, 0.0), (4, 0.8)];
    for (k, (i, w)) in want.into_iter().enumerate() {
        let t = FrameRate::FPS_30.tick_of(k as i64);
        let (fi, fw) = frame_position(t, FrameRate::FPS_24);
        assert_eq!(fi, i, "comp frame {k}");
        assert!((fw - w).abs() < 1e-12, "comp frame {k}: {fw} vs {w}");
    }
    // NTSC 29.97 footage in a 29.97 comp lines up exactly.
    for k in [0, 1, 7, 1001, 30_000] {
        assert_eq!(frame_position(FrameRate::FPS_29_97.tick_of(k), FrameRate::FPS_29_97), (k, 0.0));
    }
}

/// Footage whose frame `i` is a flat grey of `i / 10` (24 fps), or a textured pattern moving
/// 6 px right per frame.
struct Frames {
    moving: bool,
}

fn pattern(w: u32, h: u32, dx: f64) -> Image {
    let mut img = Image::new(w, h);
    img.rows_mut().for_each(|(y, row)| {
        for (x, p) in row.iter_mut().enumerate() {
            let (u, v) = (x as f64 - dx, y as f64);
            let s = (0.5 + 0.2 * (u * 0.21 + v * 0.07).sin() + 0.15 * (u * 0.05 - v * 0.19).cos()) as f32;
            *p = [s, s * 0.8, 1.0 - s, 1.0];
        }
    });
    img
}

impl FootageSource for Frames {
    fn frame(&self, _: ItemId, f: &Footage, t: Tick) -> Option<Arc<Image>> {
        let i = f.frame_rate.frame_at(t);
        if self.moving {
            return Some(Arc::new(pattern(f.width, f.height, 6.0 * i as f64)));
        }
        let g = i as f32 / 10.0;
        Some(Arc::new(Image::filled(f.width, f.height, [g, g, g, 1.0])))
    }
}

fn setup(mode: FrameBlend, size: (u32, u32)) -> (Project, ItemId) {
    let mut p = Project::default();
    p.settings.bit_depth = BitDepth::Bpc32;
    let comp = Comp::new(size.0, size.1, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    let f = Footage {
        path: "clip.mov".into(),
        kind: FootageKind::Video,
        width: size.0,
        height: size.1,
        pixel_aspect: 1.0,
        frame_rate: FrameRate::FPS_24,
        native_rate: None,
        duration: Tick::from_seconds_f64(2.0),
        has_video: true,
        has_audio: false,
        alpha: AlphaMode::Ignore,
        premul_color: [0.0; 3],
        loop_count: 1,
        codec: String::new(),
        missing: false,
        sequence: vec![],
        color_profile: None,
        ..Default::default()
    };
    let fid = p.add_item("clip", Label::Aqua, None, ItemKind::Footage(f));
    let mut l = build::layer(&mut p, &comp, "clip", LayerSource::Footage { item: fid }, size, None);
    l.switches.frame_blend = mode;
    p.comp_mut(cid).unwrap().layers.push(l);
    (p, cid)
}

fn frame(p: &Project, cid: ItemId, k: i64, moving: bool) -> Image {
    let src = Frames { moving };
    Renderer::new(p, &src, RenderOpts::default()).comp_frame(cid, FrameRate::FPS_30.tick_of(k))
}

#[test]
fn frame_mix_blends_footage_frames() {
    let (p, cid) = setup(FrameBlend::FrameMix, (16, 8));
    // Comp frame 1 → source 0.8: 0.2·frame 0 + 0.8·frame 1 = 0.08.
    let v = frame(&p, cid, 1, false).get(4, 4)[0];
    assert!((v - 0.08).abs() < 1e-6, "{v}");
    let v = frame(&p, cid, 2, false).get(4, 4)[0];
    assert!((v - (0.4 * 0.1 + 0.6 * 0.2)).abs() < 1e-6, "{v}");
    // Off (layer switch or comp switch): the frame at or before.
    let (p, cid) = setup(FrameBlend::Off, (16, 8));
    assert!((frame(&p, cid, 1, false).get(4, 4)[0] - 0.0).abs() < 1e-6);
    let (mut p, cid) = setup(FrameBlend::FrameMix, (16, 8));
    p.comp_mut(cid).unwrap().enable_frame_blending = false;
    assert!((frame(&p, cid, 1, false).get(4, 4)[0] - 0.0).abs() < 1e-6);
}

fn mae(a: &Image, b: &Image, margin: u32) -> f32 {
    let mut s = 0.0;
    let mut n = 0;
    for y in margin..a.height - margin {
        for x in margin..a.width - margin {
            let (p, q) = (a.get(x as i64, y as i64), b.get(x as i64, y as i64));
            s += (0..3).map(|c| (p[c] - q[c]).abs()).sum::<f32>();
            n += 3;
        }
    }
    s / n as f32
}

#[test]
fn pixel_motion_beats_frame_mix_on_moving_footage() {
    let size = (128, 96);
    // Comp frame 2 → source frame 1.6: the pattern 9.6 px along.
    let truth = pattern(size.0, size.1, 6.0 * 1.6);
    let (p, cid) = setup(FrameBlend::FrameMix, size);
    let mix = mae(&frame(&p, cid, 2, true), &truth, 14);
    let (p, cid) = setup(FrameBlend::PixelMotion, size);
    let pm = mae(&frame(&p, cid, 2, true), &truth, 14);
    assert!(pm < mix * 0.3, "pixel motion {pm} vs frame mix {mix}");
}

#[test]
fn time_stretched_precomp_frame_mixes() {
    // Inner comp: a white solid moving 10 px per frame from x = 10. The precomp is stretched to
    // 200%, so odd outer frames fall halfway between inner frames.
    let mut p = Project::default();
    p.settings.bit_depth = BitDepth::Bpc32;
    let mut inner = Comp::new(400, 20, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
    let sid = p.add_item("S", Label::Red, None, ItemKind::Solid(Solid { color: [1.0; 3], width: 10, height: 20, pixel_aspect: 1.0 }));
    let mut l = build::layer(&mut p, &inner, "S", LayerSource::Solid { item: sid }, (10, 20), None);
    // Anchor at the left edge: x = 10 + 10 per frame.
    l.props.prop_mut("transform/anchor").unwrap().value = Value::Vec3([0.0, 10.0, 0.0]);
    l.props.prop_mut("transform/position").unwrap().keys =
        vec![Keyframe::new(Tick::ZERO, Value::Vec3([10.0, 10.0, 0.0])), Keyframe::new(Tick::from_seconds_f64(1.0), Value::Vec3([310.0, 10.0, 0.0]))];
    inner.layers.push(l);
    let iid = p.add_item("Inner", Label::Sandstone, None, ItemKind::Comp(inner.into()));
    let outer = Comp::new(400, 20, FrameRate::FPS_30, Tick::from_seconds_f64(4.0));
    let cid = p.add_item("Outer", Label::Sandstone, None, ItemKind::Comp(outer.clone().into()));
    let mut pre = build::layer(&mut p, &outer, "Inner", LayerSource::Comp { item: iid }, (400, 20), None);
    pre.stretch = 200.0;
    pre.out_point = Tick::from_seconds_f64(4.0);
    pre.switches.frame_blend = FrameBlend::FrameMix;
    p.comp_mut(cid).unwrap().layers.push(pre);
    // Outer frame 1 → inner frame 0.5: solids at 10..20 (frame 0) and 20..30 (frame 1), half each.
    let img = crate::render_frame(&p, cid, FrameRate::FPS_30.tick_of(1), 1.0);
    for x in [12, 17, 22, 27] {
        assert!((img.get(x, 10)[3] - 0.5).abs() < 5e-3, "x {x}: {:?}", img.get(x, 10));
    }
    // Without frame blending the nested comp renders at the in-between time: one solid, 15..25.
    p.comp_mut(cid).unwrap().layers[0].switches.frame_blend = FrameBlend::Off;
    let img = crate::render_frame(&p, cid, FrameRate::FPS_30.tick_of(1), 1.0);
    assert!(img.get(12, 10)[3] < 1e-3 && (img.get(17, 10)[3] - 1.0).abs() < 1e-3 && (img.get(22, 10)[3] - 1.0).abs() < 1e-3 && img.get(27, 10)[3] < 1e-3);
}
