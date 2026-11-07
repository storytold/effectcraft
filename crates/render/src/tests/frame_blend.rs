//! Frame blending (Frame Mix and Pixel Motion) for footage and precomp layers.

use std::sync::Arc;

use rayon::prelude::*;

use effectcraft_color::Label;
use effectcraft_keyframe::{Interp, Keyframe, Value};
use effectcraft_project::build;
use effectcraft_project::{AlphaMode, BitDepth, Comp, Footage, FootageKind, FrameBlend, ItemId, ItemKind, LayerSource, Project, Solid};
use effectcraft_time::{FrameRate, Tick};

use crate::frames::MAX_MIX_FRAMES;
use crate::{FootageSource, Image, RenderOpts, Renderer, frame_position, mix_window};

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

/// [`setup_rated`] with 24 fps footage and no recorded native rate.
fn setup(mode: FrameBlend, size: (u32, u32)) -> (Project, ItemId) {
    setup_rated(mode, size, FrameRate::FPS_24, None)
}

/// [`setup`] with footage at `rate` (its Interpret Footage frame rate) recorded at `native`.
fn setup_rated(mode: FrameBlend, size: (u32, u32), rate: FrameRate, native: Option<FrameRate>) -> (Project, ItemId) {
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
        frame_rate: rate,
        native_rate: native,
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

/// Fast-conformed footage (30 fps interpreted as 300 fps) averages every source frame a comp frame spans, for Frame Mix and Pixel Motion.
#[test]
fn frame_mix_averages_every_frame_of_fast_conformed_footage() {
    // #3180: 30 fps footage interpreted as 300 fps in a 30 fps comp. Every comp frame lands
    // exactly on a source frame, but it spans ten of them: frame blending averages all ten
    // (greys 0.0 … 0.9 → 0.45), not just the frame it starts on (0.0).
    let fast = FrameRate::new(300, 1);
    let (p, cid) = setup_rated(FrameBlend::FrameMix, (16, 8), fast, Some(FrameRate::FPS_30));
    let v = frame(&p, cid, 0, false).get(4, 4)[0];
    assert!((v - 0.45).abs() < 1e-5, "{v}");
    // Pixel Motion has no in-between frame to make either: it averages the same frames.
    let (p, cid) = setup_rated(FrameBlend::PixelMotion, (16, 8), fast, Some(FrameRate::FPS_30));
    let v = frame(&p, cid, 0, false).get(4, 4)[0];
    assert!((v - 0.45).abs() < 1e-5, "{v}");
    // Off (layer switch or comp switch): the one frame at the comp frame's time.
    let (p, cid) = setup_rated(FrameBlend::Off, (16, 8), fast, Some(FrameRate::FPS_30));
    assert!(frame(&p, cid, 0, false).get(4, 4)[0].abs() < 1e-6);
    let (mut p, cid) = setup_rated(FrameBlend::FrameMix, (16, 8), fast, Some(FrameRate::FPS_30));
    p.comp_mut(cid).unwrap().enable_frame_blending = false;
    assert!(frame(&p, cid, 0, false).get(4, 4)[0].abs() < 1e-6);
}

/// Footage at the comp's rate, or only mildly faster, renders as before.
#[test]
fn frame_mix_of_common_footage_is_unchanged() {
    // Footage at the comp's rate: each comp frame is exactly one source frame, blended or not.
    let (p, cid) = setup_rated(FrameBlend::FrameMix, (16, 8), FrameRate::FPS_30, None);
    for k in 0..4 {
        let v = frame(&p, cid, k, false).get(4, 4)[0];
        assert!((v - k as f32 / 10.0).abs() < 1e-6, "comp frame {k}: {v}");
    }
    // A mild speed-up (36 fps footage in a 30 fps comp) keeps the two-frame cross-fade: comp
    // frame 1 → source frame 1.2, 0.8·frame 1 + 0.2·frame 2 = 0.12.
    let (p, cid) = setup_rated(FrameBlend::FrameMix, (16, 8), FrameRate::new(36, 1), None);
    let v = frame(&p, cid, 1, false).get(4, 4)[0];
    assert!((v - 0.12).abs() < 1e-6, "{v}");
    // 60 and 90 fps in a 30 fps comp (two and three source frames per comp frame) stay crisp:
    // comp frame 1 is source frame 2 or 3, as before.
    for (rate, want) in [(FrameRate::FPS_60, 0.2), (FrameRate::new(90, 1), 0.3)] {
        let (p, cid) = setup_rated(FrameBlend::FrameMix, (16, 8), rate, None);
        let v = frame(&p, cid, 1, false).get(4, 4)[0];
        assert!((v - want).abs() < 1e-6, "{rate:?}: {v}");
    }
}

/// Adds Time Remap with `keys` (layer time → source seconds) to the comp's footage layer.
fn remap(p: &mut Project, cid: ItemId, keys: Vec<Keyframe>) {
    let mut next = p.next_id;
    let mut tr = build::Ids(&mut next).prop("timeRemap", "Time Remap", Value::Scalar(0.0));
    p.next_id = next;
    tr.keys = keys;
    p.comp_mut(cid).unwrap().layers[0].props.children.push(effectcraft_project::Node::Prop(tr));
}

/// Time remap holds and cuts do not smear: a still frame stays a single source frame.
#[test]
fn time_remap_jumps_stay_crisp() {
    let fast = FrameRate::new(300, 1);
    let s = |frames: f64| Value::Scalar(frames / 300.0);
    let at = |k: i64| FrameRate::FPS_30.tick_of(k);
    // Hold on source frame 0, then cut to source frame 10 at comp frame 1: comp frame 0 is a
    // still frame, not a smear of the ten frames the cut skips.
    let (mut p, cid) = setup_rated(FrameBlend::FrameMix, (16, 8), fast, Some(FrameRate::FPS_30));
    remap(&mut p, cid, vec![Keyframe::new(at(0), s(0.0)).hold(), Keyframe::new(at(1), s(10.0)).hold()]);
    let v = frame(&p, cid, 0, false).get(4, 4)[0];
    assert!(v.abs() < 1e-6, "hold then cut: {v}");
    // A loop wrapping back (like `loopOut("cycle")`): from source frame 10 at normal speed,
    // back to frame 0 by the next comp frame. Comp frame 0 shows frame 10 (grey 1.0), not
    // frames 1 … 10 run backwards.
    let (mut p, cid) = setup_rated(FrameBlend::FrameMix, (16, 8), fast, Some(FrameRate::FPS_30));
    let mut top = Keyframe::new(FrameRate::FPS_60.tick_of(1), s(15.0));
    top.out_interp = Interp::Hold;
    remap(&mut p, cid, vec![Keyframe::new(at(0), s(10.0)), top, Keyframe::new(at(1), s(0.0))]);
    let v = frame(&p, cid, 0, false).get(4, 4)[0];
    assert!((v - 1.0).abs() < 1e-6, "loop wrap: {v}");
    // A remap running at normal speed through fast-conformed footage still mixes its frames.
    let (mut p, cid) = setup_rated(FrameBlend::FrameMix, (16, 8), fast, Some(FrameRate::FPS_30));
    remap(&mut p, cid, vec![Keyframe::new(at(0), s(0.0)), Keyframe::new(at(30), s(300.0))]);
    let v = frame(&p, cid, 0, false).get(4, 4)[0];
    assert!((v - 0.45).abs() < 1e-5, "steady remap: {v}");
}

/// A frame-blended dependency changes its owner's cache key when its source goes from holding to running at speed.
#[test]
fn a_frame_blended_dependency_keys_how_far_its_source_runs() {
    // A footage layer selected by another layer's effect: at comp frames 0 and 1 its source time
    // is the same (0), but at frame 0 it holds and at frame 1 it runs at speed, which mixes ten
    // source frames. The owner's cache key must tell the two apart.
    let fast = FrameRate::new(300, 1);
    let (mut p, cid) = setup_rated(FrameBlend::FrameMix, (16, 8), fast, Some(FrameRate::FPS_30));
    let at = |k: i64| FrameRate::FPS_30.tick_of(k);
    let keys = vec![Keyframe::new(at(0), Value::Scalar(0.0)).hold(), Keyframe::new(at(1), Value::Scalar(0.0)), Keyframe::new(at(31), Value::Scalar(1.0))];
    remap(&mut p, cid, keys);
    let comp = p.comp(cid).unwrap().clone();
    let footage = comp.layers[0].id;
    let sid = p.add_item("Solid", Label::Red, None, ItemKind::Solid(Solid { color: [1.0; 3], width: 16, height: 8, pixel_aspect: 1.0 }));
    let mut owner = build::layer(&mut p, &comp, "Solid", LayerSource::Solid { item: sid }, (16, 8), None);
    super::add_effect(&mut p, &mut owner, "ec.channel.setmatte", [16.0, 8.0], &[("takeMatteFromLayer", Value::Layer(Some(footage.0)))]);
    p.comp_mut(cid).unwrap().layers.push(owner);
    let key = |k: i64| {
        let c = p.comp(cid).unwrap();
        crate::cache::layer_key(&crate::EvalCtx::new(&p, cid, c, at(k)), &c.layers[1], 1.0, false, false).unwrap()
    };
    assert_ne!(key(0), key(1));
}

/// The mix window is the source span of one comp frame, and falls back to the two-frame blend below four source frames per comp frame.
#[test]
fn mix_window_covers_the_source_span_of_one_comp_frame() {
    let comp_frame = |r: FrameRate| r.frame_duration();
    let steady = |r: FrameRate| (comp_frame(r), comp_frame(r));
    // Fewer than four source frames per comp frame (slow-down, same rate, 60p in 30, 24 or 25,
    // 50p in 25, 90 in 30): the two-frame blend applies, as before.
    for (comp, footage) in [
        (FrameRate::FPS_30, FrameRate::FPS_30),
        (FrameRate::FPS_30, FrameRate::FPS_24),
        (FrameRate::FPS_30, FrameRate::new(36, 1)),
        (FrameRate::FPS_25, FrameRate::FPS_30),
        (FrameRate::FPS_30, FrameRate::FPS_60),
        (FrameRate::FPS_24, FrameRate::FPS_60),
        (FrameRate::FPS_25, FrameRate::FPS_60),
        (FrameRate::FPS_25, FrameRate::FPS_50),
        (FrameRate::FPS_29_97, FrameRate::FPS_59_94),
        (FrameRate::FPS_30, FrameRate::new(90, 1)),
    ] {
        assert_eq!(mix_window(Tick::ZERO, steady(comp), footage), None, "{footage:?} in {comp:?}");
    }
    // Four source frames per comp frame: 120 fps in a 30 fps comp mixes all four.
    assert_eq!(mix_window(Tick::ZERO, steady(FrameRate::FPS_30), FrameRate::FPS_120), Some(vec![(0, 1.0), (1, 1.0), (2, 1.0), (3, 1.0)]));
    // The report's case: 600 fps in a 25 fps comp, 24 source frames per comp frame, sampled at
    // 16 evenly spread frames.
    let w = mix_window(FrameRate::FPS_25.tick_of(3), steady(FrameRate::FPS_25), FrameRate::new(600, 1)).unwrap();
    assert_eq!(w.len(), MAX_MIX_FRAMES);
    assert!(w.iter().all(|x| (72..96).contains(&x.0) && x.1 == 1.0), "{w:?}");
    assert!(w.windows(2).all(|p| p[0].0 < p[1].0), "{w:?}");
    // 150 fps in a 30 fps comp: comp frame 1 spans source frames 5 … 10.
    let w = mix_window(FrameRate::FPS_30.tick_of(1), steady(FrameRate::FPS_30), FrameRate::new(150, 1));
    assert_eq!(w, Some((5..10).map(|j| (j, 1.0)).collect()));
    // 135 fps in a 30 fps comp: comp frame 1 spans source frames 4.5 … 9.0.
    let w = mix_window(FrameRate::FPS_30.tick_of(1), steady(FrameRate::FPS_30), FrameRate::new(135, 1)).unwrap();
    assert_eq!(w.iter().map(|x| x.0).collect::<Vec<_>>(), vec![4, 5, 6, 7, 8]);
    for (got, want) in w.iter().map(|x| x.1).zip([0.5, 1.0, 1.0, 1.0, 1.0]) {
        assert!((got - want).abs() < 1e-9, "{w:?}");
    }
    // Playing backwards: the span ends with the comp frame's own source frame.
    let back = -comp_frame(FrameRate::FPS_30);
    let w = mix_window(FrameRate::FPS_30.tick_of(1), (back, back), FrameRate::new(300, 1)).unwrap();
    assert_eq!(w.iter().map(|x| x.0).collect::<Vec<_>>(), (1..=10).collect::<Vec<_>>());
    // A hold (no local speed) or a jump (local speed and the whole frame disagree) does not mix.
    let f = comp_frame(FrameRate::FPS_30);
    assert_eq!(mix_window(Tick::ZERO, (Tick::ZERO, f), FrameRate::new(300, 1)), None);
    assert_eq!(mix_window(Tick::ZERO, (f, -f), FrameRate::new(300, 1)), None);
    // A long span is sampled at a bounded number of frames; a span beyond any footage's speed is
    // a jump.
    let ten = Tick::from_seconds_f64(10.0);
    let w = mix_window(Tick::ZERO, (ten, ten), FrameRate::FPS_60).unwrap();
    assert_eq!(w.len(), MAX_MIX_FRAMES);
    assert!(w.iter().all(|x| (0..600).contains(&x.0)));
    let far = Tick::from_seconds_f64(100.0);
    assert_eq!(mix_window(Tick::ZERO, (far, far), FrameRate::FPS_60), None);
    // Degenerate rates and huge spans don't panic.
    assert_eq!(mix_window(Tick::ZERO, (Tick::MAX, Tick::MAX), FrameRate { num: 0, den: 1 }), None);
    assert_eq!(mix_window(Tick::MIN, (Tick::MAX, Tick::MAX), FrameRate::FPS_60), None);
    assert_eq!(mix_window(Tick::MAX, (Tick::MIN, Tick::MAX), FrameRate::FPS_60), None);
}

#[test]
fn frame_position_survives_degenerate_rates() {
    for rate in [FrameRate { num: 24, den: 0 }, FrameRate { num: 0, den: 1 }, FrameRate { num: -24, den: 1 }] {
        assert_eq!(frame_position(Tick(1_000_000), rate), (0, 0.0));
    }
}
