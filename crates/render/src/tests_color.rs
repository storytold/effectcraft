//! Bit depth, working space and linear blending.

use std::sync::Arc;

use effectcraft_color::{BlendMode, ColorSpace, Label};
use effectcraft_project::build;
use effectcraft_project::{AlphaMode, BitDepth, Comp, Footage, FootageKind, ItemId, ItemKind, LayerSource, Project, Solid};
use effectcraft_time::{FrameRate, Tick};

use crate::{FloatCurve, FootageSource, Image, LayerCache, RenderOpts, Renderer, render_frame};

fn setup(depth: BitDepth) -> (Project, ItemId, Comp) {
    let mut p = Project::default();
    p.settings.bit_depth = depth;
    let comp = Comp::new(40, 20, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    (p, cid, comp)
}

fn add_solid(p: &mut Project, cid: ItemId, color: [f32; 3], mode: BlendMode, opacity: f64) {
    let comp = p.comp(cid).unwrap().clone();
    let sid = p.add_item("S", Label::Red, None, ItemKind::Solid(Solid { color, width: 40, height: 20, pixel_aspect: 1.0 }));
    let mut l = build::layer(p, &comp, "S", LayerSource::Solid { item: sid }, (40, 20), None);
    l.blend_mode = mode;
    l.props.prop_mut("transform/opacity").unwrap().value = effectcraft_keyframe::Value::Scalar(opacity);
    // New layers go on top.
    p.comp_mut(cid).unwrap().layers.insert(0, l);
}

fn px(p: &Project, cid: ItemId) -> [f32; 4] {
    render_frame(p, cid, Tick::ZERO, 1.0).get(10, 10)
}

#[test]
fn add_clamps_in_8_and_16_but_not_32_bpc() {
    for (depth, want) in [(BitDepth::Bpc8, 1.0), (BitDepth::Bpc16, 1.0), (BitDepth::Bpc32, 1.6)] {
        let (mut p, cid, _) = setup(depth);
        add_solid(&mut p, cid, [0.8, 0.8, 0.8], BlendMode::Normal, 100.0);
        add_solid(&mut p, cid, [0.8, 0.8, 0.8], BlendMode::Add, 100.0);
        let v = px(&p, cid)[0];
        assert!((v - want).abs() < 1e-5, "{depth:?}: {v}");
    }
    // Screen of over-range values survives in 32 bpc only: a 1.6 base screened with 0.5.
    let (mut p, cid, _) = setup(BitDepth::Bpc32);
    add_solid(&mut p, cid, [0.8, 0.8, 0.8], BlendMode::Normal, 100.0);
    add_solid(&mut p, cid, [0.8, 0.8, 0.8], BlendMode::Add, 100.0);
    add_solid(&mut p, cid, [0.5, 0.5, 0.5], BlendMode::Screen, 100.0);
    let v = px(&p, cid)[0];
    assert!((v - (1.6 + 0.5 - 0.8)).abs() < 1e-5, "{v}");
}

#[test]
fn quantization_steps_per_depth() {
    let want = |depth| match depth {
        BitDepth::Bpc8 => (0.3f32 * 255.0).round() / 255.0,
        BitDepth::Bpc16 => (0.3f32 * 32768.0).round() / 32768.0,
        BitDepth::Bpc32 => 0.3,
    };
    for depth in [BitDepth::Bpc8, BitDepth::Bpc16, BitDepth::Bpc32] {
        let (mut p, cid, _) = setup(depth);
        add_solid(&mut p, cid, [0.3, 0.3, 0.3], BlendMode::Normal, 100.0);
        let v = px(&p, cid)[0];
        assert!((v - want(depth)).abs() < 1e-7, "{depth:?}: {v} vs {}", want(depth));
    }
    // 8 bpc: 77/255 = 0.30196 (one step is 1/255); 16 bpc is within 1/65536.
    let (mut p8, c8, _) = setup(BitDepth::Bpc8);
    add_solid(&mut p8, c8, [0.3, 0.3, 0.3], BlendMode::Normal, 100.0);
    assert!((px(&p8, c8)[0] - 77.0 / 255.0).abs() < 1e-7);
    // A 1% opacity layer of white over black lands on a whole 8-bit step.
    let (mut p, cid, _) = setup(BitDepth::Bpc8);
    add_solid(&mut p, cid, [0.0, 0.0, 0.0], BlendMode::Normal, 100.0);
    add_solid(&mut p, cid, [1.0, 1.0, 1.0], BlendMode::Normal, 1.0);
    let v = px(&p, cid)[0] * 255.0;
    assert!((v - v.round()).abs() < 1e-4 && (v - 2.55).abs() < 1.0, "{v}");
}

#[test]
fn linear_blending_of_red_over_green() {
    // 50% red over green: gamma blending gives (0.5, 0.5, 0); 1.0-gamma blending mixes linear
    // light, so each channel is the sRGB encoding of 0.5 linear ≈ 0.735.
    let run = |linear: bool| {
        let (mut p, cid, _) = setup(BitDepth::Bpc32);
        p.settings.blend_linear = linear;
        add_solid(&mut p, cid, [0.0, 1.0, 0.0], BlendMode::Normal, 100.0);
        add_solid(&mut p, cid, [1.0, 0.0, 0.0], BlendMode::Normal, 50.0);
        px(&p, cid)
    };
    let g = run(false);
    assert!((g[0] - 0.5).abs() < 1e-5 && (g[1] - 0.5).abs() < 1e-5 && g[2].abs() < 1e-6, "{g:?}");
    let l = run(true);
    let e = effectcraft_color::linear_to_srgb(0.5);
    assert!((l[0] - e).abs() < 1e-4 && (l[1] - e).abs() < 1e-4 && l[2].abs() < 1e-6, "{l:?}");
    assert!((e - 0.7354).abs() < 1e-3);
    // Unblended (opaque) layers are unchanged by linear blending.
    let (mut p, cid, _) = setup(BitDepth::Bpc32);
    p.settings.blend_linear = true;
    add_solid(&mut p, cid, [0.25, 0.5, 0.75], BlendMode::Normal, 100.0);
    let o = px(&p, cid);
    assert!((o[0] - 0.25).abs() < 1e-5 && (o[1] - 0.5).abs() < 1e-5 && (o[2] - 0.75).abs() < 1e-5, "{o:?}");
}

#[test]
fn linearized_working_space_blends_linear_and_round_trips_colours() {
    let (mut p, cid, _) = setup(BitDepth::Bpc32);
    p.settings.working_space = Some(ColorSpace::Srgb);
    p.settings.linearize = true;
    add_solid(&mut p, cid, [0.0, 1.0, 0.0], BlendMode::Normal, 100.0);
    add_solid(&mut p, cid, [1.0, 0.0, 0.0], BlendMode::Normal, 50.0);
    let l = px(&p, cid);
    let e = effectcraft_color::linear_to_srgb(0.5);
    assert!((l[0] - e).abs() < 1e-4 && (l[1] - e).abs() < 1e-4, "{l:?}");
    // An opaque solid shows its own colour (linearised in, encoded out).
    let (mut p, cid, _) = setup(BitDepth::Bpc32);
    p.settings.working_space = Some(ColorSpace::Srgb);
    p.settings.linearize = true;
    add_solid(&mut p, cid, [0.2, 0.5, 0.9], BlendMode::Normal, 100.0);
    let o = px(&p, cid);
    assert!((o[0] - 0.2).abs() < 1e-4 && (o[1] - 0.5).abs() < 1e-4 && (o[2] - 0.9).abs() < 1e-4, "{o:?}");
    // Linearize needs a working space (like AE, where it is disabled without one).
    let (mut p, cid, _) = setup(BitDepth::Bpc32);
    p.settings.linearize = true;
    add_solid(&mut p, cid, [0.0, 1.0, 0.0], BlendMode::Normal, 100.0);
    add_solid(&mut p, cid, [1.0, 0.0, 0.0], BlendMode::Normal, 50.0);
    assert!((px(&p, cid)[0] - 0.5).abs() < 1e-5);
}

#[test]
fn wide_working_space_colours_leave_the_display_gamut() {
    // Pure Rec. 2020 red is outside sRGB: in 32 bpc the display conversion keeps the
    // over-range/negative values; 8 bpc clamps them.
    let (mut p, cid, _) = setup(BitDepth::Bpc32);
    p.settings.working_space = Some(ColorSpace::Rec2020);
    add_solid(&mut p, cid, [1.0, 0.0, 0.0], BlendMode::Normal, 100.0);
    let o = px(&p, cid);
    assert!(o[0] > 1.0 && o[1] < 0.0 && o[2] < 0.0, "{o:?}");
    p.settings.bit_depth = BitDepth::Bpc8;
    let o = px(&p, cid);
    assert!((o[0] - 1.0).abs() < 1e-6 && o[1] == 0.0 && o[2] == 0.0, "{o:?}");
    // sRGB working space: no conversion at all.
    let (mut p, cid, _) = setup(BitDepth::Bpc32);
    p.settings.working_space = Some(ColorSpace::Srgb);
    add_solid(&mut p, cid, [0.3, 0.6, 0.9], BlendMode::Normal, 100.0);
    let o = px(&p, cid);
    assert!((o[0] - 0.3).abs() < 1e-6 && (o[2] - 0.9).abs() < 1e-6);
}

/// One flat-colour footage frame.
struct Flat([f32; 3]);
impl FootageSource for Flat {
    fn frame(&self, _: ItemId, f: &Footage, _: Tick) -> Option<Arc<Image>> {
        let c = self.0;
        Some(Arc::new(Image::filled(f.width, f.height, [c[0], c[1], c[2], 1.0])))
    }
}

fn footage(profile: Option<ColorSpace>) -> Footage {
    Footage {
        path: "x.png".into(),
        kind: FootageKind::Still,
        width: 40,
        height: 20,
        pixel_aspect: 1.0,
        frame_rate: FrameRate::FPS_30,
        native_rate: None,
        duration: Tick::ZERO,
        has_video: true,
        has_audio: false,
        alpha: AlphaMode::Ignore,
        premul_color: [0.0; 3],
        loop_count: 1,
        codec: String::new(),
        missing: false,
        sequence: vec![],
        color_profile: profile,
        ..Default::default()
    }
}

#[test]
fn footage_profile_is_converted_into_the_working_space() {
    let render = |ws: Option<ColorSpace>, profile: Option<ColorSpace>| {
        let (mut p, cid, comp) = setup(BitDepth::Bpc32);
        p.settings.working_space = ws;
        let fid = p.add_item("F", Label::Aqua, None, ItemKind::Footage(footage(profile)));
        let l = build::layer(&mut p, &comp, "F", LayerSource::Footage { item: fid }, (40, 20), None);
        p.comp_mut(cid).unwrap().layers.push(l);
        let src = Flat([1.0, 0.0, 0.0]);
        Renderer::new(&p, &src, RenderOpts::default()).comp_frame(cid, Tick::ZERO).get(5, 5)
    };
    // Unmanaged: the file's values pass straight through.
    let o = render(None, Some(ColorSpace::Rec2020));
    assert_eq!(o, [1.0, 0.0, 0.0, 1.0]);
    // Managed: Rec. 2020 red shown on an sRGB display is beyond sRGB red.
    let o = render(Some(ColorSpace::Srgb), Some(ColorSpace::Rec2020));
    assert!(o[0] > 1.0 && o[1] < 0.0, "{o:?}");
    // Untagged footage is sRGB: a round trip through a Rec. 2020 working space.
    let o = render(Some(ColorSpace::Rec2020), None);
    assert!((o[0] - 1.0).abs() < 2e-3 && o[1].abs() < 2e-3 && o[2].abs() < 2e-3, "{o:?}");
}

/// #411: Interpret Footage ▸ Preserve RGB: the footage's values enter the composition as the file
/// stores them, whatever its colour profile, Interpret As Linear Light or the working space.
#[test]
fn preserve_rgb_footage_is_not_converted() {
    let render = |linearize: bool, f: Footage| {
        let (mut p, cid, comp) = setup(BitDepth::Bpc32);
        p.settings.working_space = Some(ColorSpace::Rec2020);
        p.settings.linearize = linearize;
        let fid = p.add_item("F", Label::Aqua, None, ItemKind::Footage(f));
        let l = build::layer(&mut p, &comp, "F", LayerSource::Footage { item: fid }, (40, 20), None);
        p.comp_mut(cid).unwrap().layers.push(l);
        let src = Flat([0.18, 0.5, 0.75]);
        let out = Renderer::new(&p, &src, RenderOpts::default()).comp_frame(cid, Tick::ZERO).get(5, 5);
        // Back from the output conversion (working space → sRGB) to the working space's values.
        let ws = effectcraft_color::Conversion::new(ColorSpace::Srgb, false, ColorSpace::Rec2020, linearize).unwrap();
        let c = ws.apply([out[0], out[1], out[2]]);
        [c[0], c[1], c[2], out[3]]
    };
    let preserve = Footage { preserve_rgb: true, linear_light: true, ..footage(Some(ColorSpace::DisplayP3)) };
    for linearize in [false, true] {
        let o = render(linearize, preserve.clone());
        assert!((o[0] - 0.18).abs() < 1e-3 && (o[1] - 0.5).abs() < 1e-3 && (o[2] - 0.75).abs() < 1e-3, "linearize {linearize}: {o:?}");
        // Without Preserve RGB the profile and linear light convert the values.
        let o = render(linearize, Footage { preserve_rgb: false, ..preserve.clone() });
        assert!((o[0] - 0.18).abs() > 0.05, "linearize {linearize}: {o:?}");
    }
}

#[test]
fn layer_cache_is_keyed_by_colour_settings() {
    let (mut p, cid, _) = setup(BitDepth::Bpc8);
    add_solid(&mut p, cid, [0.3, 0.3, 0.3], BlendMode::Normal, 100.0);
    let cache = LayerCache::default();
    let frame = |p: &Project| {
        let mut r = Renderer::new(p, &crate::NoFootage, RenderOpts::default());
        r.cache = Some(&cache);
        r.comp_frame(cid, Tick::ZERO).get(10, 10)[0]
    };
    let a = frame(&p);
    assert!((a - 77.0 / 255.0).abs() < 1e-7);
    assert_eq!(cache.stats().entries, 1);
    p.settings.bit_depth = BitDepth::Bpc32;
    let b = frame(&p);
    assert!((b - 0.3).abs() < 1e-7, "stale 8 bpc pixels: {b}");
    p.settings.working_space = Some(ColorSpace::Srgb);
    p.settings.linearize = true;
    let _ = frame(&p);
    assert_eq!(cache.stats().entries, 3, "each setting renders its own entry");
    // Back to 8 bpc reuses the first entry.
    p.settings.bit_depth = BitDepth::Bpc8;
    p.settings.working_space = None;
    p.settings.linearize = false;
    let hits = cache.stats().hits;
    assert!((frame(&p) - a).abs() < 1e-9);
    assert_eq!(cache.stats().hits, hits + 1);
}

/// #495: with no working space, Interpret As Linear Light footage is encoded with the project's
/// Working Gamma: the sRGB curve at 2.2, a pure 2.4 power curve at 2.4 (as After Effects does);
/// Blend Colors Using 1.0 Gamma linearizes with the same curve. A working space ignores it.
#[test]
fn working_gamma_24_encodes_linear_footage_with_a_power_curve() {
    let render = |gamma: f64, ws: Option<ColorSpace>| {
        let (mut p, cid, comp) = setup(BitDepth::Bpc32);
        p.settings.working_gamma = gamma;
        p.settings.working_space = ws;
        let fid = p.add_item("F", Label::Aqua, None, ItemKind::Footage(Footage { linear_light: true, ..footage(None) }));
        let l = build::layer(&mut p, &comp, "F", LayerSource::Footage { item: fid }, (40, 20), None);
        p.comp_mut(cid).unwrap().layers.push(l);
        let src = Flat([0.051, 0.18, 0.5]);
        Renderer::new(&p, &src, RenderOpts::default()).comp_frame(cid, Tick::ZERO).get(5, 5)
    };
    let near = |o: [f32; 4], want: [f32; 3]| (0..3).all(|i| (o[i] - want[i]).abs() < 2e-3);
    let o = render(2.4, None);
    assert!(near(o, [0.290, 0.490, 0.749]), "2.4: {o:?}");
    let o = render(2.2, None);
    assert!(near(o, [0.250, 0.461, 0.735]), "2.2: {o:?}");
    // A working space ignores Working Gamma.
    assert_eq!(render(2.4, Some(ColorSpace::Srgb)), render(2.2, Some(ColorSpace::Srgb)));
    // Blending with 1.0 gamma: 50% white over black is 0.5 linear, encoded with the curve.
    let blend = |gamma: f64| {
        let (mut p, cid, _) = setup(BitDepth::Bpc32);
        p.settings.working_gamma = gamma;
        p.settings.blend_linear = true;
        add_solid(&mut p, cid, [0.0, 0.0, 0.0], BlendMode::Normal, 100.0);
        add_solid(&mut p, cid, [1.0, 1.0, 1.0], BlendMode::Normal, 50.0);
        px(&p, cid)[0]
    };
    assert!((blend(2.4) - 0.5f32.powf(1.0 / 2.4)).abs() < 2e-3, "{}", blend(2.4));
    assert!((blend(2.2) - 0.7354).abs() < 2e-3, "{}", blend(2.2));
    assert_ne!(crate::color::Pipe::of(&Project::default().settings).key(), {
        let mut s = Project::default().settings;
        s.working_gamma = 2.4;
        crate::color::Pipe::of(&s).key()
    });
}

/// Float footage (an OpenEXR) the way `effectcraft-media` decodes it: linear values handed over
/// as the renderer asks ([`FloatCurve`]).
struct FloatFile([f32; 3]);
impl FootageSource for FloatFile {
    fn frame(&self, item: ItemId, f: &Footage, t: Tick) -> Option<Arc<Image>> {
        self.frame_with(item, f, t, None, FloatCurve::Srgb)
    }
    fn frame_with(&self, _: ItemId, f: &Footage, _: Tick, _: Option<(u32, u32)>, float: FloatCurve) -> Option<Arc<Image>> {
        let c = self.0.map(|v| match float {
            FloatCurve::Srgb => ColorSpace::Srgb.encode(v),
            FloatCurve::Gamma24 => v.powf(1.0 / 2.4),
            FloatCurve::Linear => v,
        });
        Some(Arc::new(Image::filled(f.width, f.height, [c[0], c[1], c[2], 1.0])))
    }
}

/// #750: with no working space, float footage is encoded with the Working Gamma's curve (as
/// After Effects converts a linear EXR), and Interpret As Linear Light encodes it once, not on
/// top of the decode's own encoding. A working space ignores Working Gamma.
#[test]
fn float_footage_is_encoded_once_with_the_working_gamma() {
    let render = |gamma: f64, ws: Option<ColorSpace>, linear_light: bool| {
        let (mut p, cid, comp) = setup(BitDepth::Bpc32);
        p.settings.working_gamma = gamma;
        p.settings.working_space = ws;
        let f = Footage { path: "x.exr".into(), linear_light, ..footage(None) };
        let fid = p.add_item("F", Label::Aqua, None, ItemKind::Footage(f));
        let l = build::layer(&mut p, &comp, "F", LayerSource::Footage { item: fid }, (40, 20), None);
        p.comp_mut(cid).unwrap().layers.push(l);
        let src = FloatFile([0.051, 0.18, 0.5]);
        Renderer::new(&p, &src, RenderOpts::default()).comp_frame(cid, Tick::ZERO).get(5, 5)
    };
    let near = |o: [f32; 4], want: [f32; 3]| (0..3).all(|i| (o[i] - want[i]).abs() < 2e-3);
    for linear_light in [false, true] {
        let o = render(2.4, None, linear_light);
        assert!(near(o, [0.290, 0.490, 0.749]), "2.4, linear light {linear_light}: {o:?}");
        let o = render(2.2, None, linear_light);
        assert!(near(o, [0.250, 0.461, 0.735]), "2.2, linear light {linear_light}: {o:?}");
        let srgb = render(2.2, Some(ColorSpace::Srgb), linear_light);
        assert!(near(srgb, [0.250, 0.461, 0.735]), "sRGB working space, linear light {linear_light}: {srgb:?}");
        assert_eq!(render(2.4, Some(ColorSpace::Srgb), linear_light), srgb);
    }
}
