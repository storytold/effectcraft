use super::*;
use crate::{Params, find};

fn run(id: &str, img: &Image, over: &[(&str, Value)]) -> Image {
    let s = find(id).unwrap();
    let mut params = Params { values: s.params.iter().map(|p| (p.id.to_string(), p.default.clone())).collect() };
    for (k, v) in over {
        params.values.insert(k.to_string(), v.clone());
    }
    let ctx = EffectCtx { params: &params, time: 0.0, layer_size: [img.width as f64, img.height as f64], seed: 1, adjustment: false, env: Default::default() };
    (s.render)(&ctx, Buf { img: img.clone(), offset: [0.0, 0.0], scale: 1.0 }).img
}

fn ramp() -> Image {
    let mut img = Image::new(16, 8);
    for y in 0..8 {
        for x in 0..16 {
            let v = x as f32 / 15.0;
            let a = if y == 0 { 0.5 } else { 1.0 };
            img.set(x, y, [v * a, (1.0 - v) * a, 0.3 * a, a]);
        }
    }
    img
}

fn close(a: &Image, b: &Image, eps: f32) -> bool {
    a.data.iter().zip(&b.data).all(|(p, q)| (0..4).all(|i| (p[i] - q[i]).abs() <= eps))
}

#[test]
fn curve_parsing() {
    assert!(Curve::parse("0,0 1,1").is_none());
    assert!(Curve::parse("garbage").is_none());
    let c = Curve::parse("1,1; 0,0 0.5,0.7").unwrap();
    assert_eq!(c.xs, vec![0.0, 0.5, 1.0]);
    assert!((c.eval(0.5) - 0.7).abs() < 1e-3);
    // Monotone: never decreases.
    let mut last = -1.0;
    for i in 0..=100 {
        let v = c.eval(i as f32 / 100.0);
        assert!(v >= last - 1e-6);
        last = v;
    }
}

#[test]
fn curves_identity_is_identity() {
    let img = ramp();
    assert_eq!(run("ec.color.curves", &img, &[]), img);
    let explicit = run("ec.color.curves", &img, &[("rgb", Value::Str("0,0 0.25,0.25 1,1".into()))]);
    assert!(close(&explicit, &img, 1e-4));
}

#[test]
fn curves_midpoint_brightens() {
    let img = Image::filled(4, 4, [0.5, 0.5, 0.5, 1.0]);
    let out = run("ec.color.curves", &img, &[("rgb", Value::Str("0,0 0.5,0.75 1,1".into()))]);
    assert!((out.data[0][0] - 0.75).abs() < 1e-3);
    let red = run("ec.color.curves", &img, &[("red", Value::Str("0,1 1,1".into()))]);
    assert!((red.data[0][0] - 1.0).abs() < 1e-4 && (red.data[0][1] - 0.5).abs() < 1e-6);
}

/// After Effects 2026, 32 bpc project, Levels (Individual Controls) with RGB Input Black 0.1,
/// Input White 0.9, Gamma 1.5, Output Black 0.05, Output White 0.95, Red Gamma 0.8, Green
/// Input White 0.8, Blue Output Black 0.2, both clips "Off for 32 bpc Color", on grey steps:
/// the channel controls run first, nothing clips (8-bit captures clamp the results).
#[test]
fn levels_ic_matches_after_effects_in_32_bpc() {
    let steps = [0, 16, 32, 64, 96, 128, 160, 192, 224, 255];
    let ae = [
        [0, 0, 70],
        [0, 0, 88],
        [0, 52, 104],
        [61, 108, 132],
        [102, 150, 158],
        [138, 187, 181],
        [171, 220, 203],
        [203, 250, 223],
        [233, 255, 243],
        [255, 255, 255],
    ];
    let mut img = Image::new(10, 1);
    for (x, v) in steps.iter().enumerate() {
        let v = *v as f32 / 255.0;
        img.set(x as u32, 0, [v, v, v, 1.0]);
    }
    let vals = [
        ("rgb/rgbInBlack", Value::Scalar(0.1)),
        ("rgb/rgbInWhite", Value::Scalar(0.9)),
        ("rgb/rgbGamma", Value::Scalar(1.5)),
        ("rgb/rgbOutBlack", Value::Scalar(0.05)),
        ("rgb/rgbOutWhite", Value::Scalar(0.95)),
        ("red/redGamma", Value::Scalar(0.8)),
        ("green/greenInWhite", Value::Scalar(0.8)),
        ("blue/blueOutBlack", Value::Scalar(0.2)),
    ];
    let s = find("ec.color.levelsic").unwrap();
    let mut params = Params { values: s.params.iter().map(|p| (p.id.to_string(), p.default.clone())).collect() };
    for (k, v) in &vals {
        params.values.insert(k.to_string(), v.clone());
    }
    let render = |float_depth: bool| {
        let env = crate::EffectEnv { float_depth, ..Default::default() };
        let ctx = EffectCtx { params: &params, time: 0.0, layer_size: [10.0, 1.0], seed: 1, adjustment: false, env };
        (s.render)(&ctx, Buf { img: img.clone(), offset: [0.0, 0.0], scale: 1.0 }).img
    };
    let out = render(true);
    for (x, want) in ae.iter().enumerate() {
        let p = out.get(x as i64, 0);
        for c in 0..3 {
            let got = p[c].clamp(0.0, 1.0) * 255.0;
            assert!((got - want[c] as f32).abs() <= 2.0, "step {} channel {c}: {got} vs AE {}", steps[x], want[c]);
        }
    }
    // In 8 / 16 bpc "Off for 32 bpc Color" clips: black stops at Output Black.
    let clipped = render(false);
    assert!((clipped.get(0, 0)[0] - 0.05).abs() < 1e-5, "{:?}", clipped.get(0, 0));
}

/// After Effects 2026 (cases 962 / 963): Curves through RGB (0,0) (125,199)
/// (255,255) and Red (0,0) (128,44) (255,255) on grey steps 0..255, 8-bit results.
#[test]
fn curves_follow_a_natural_spline_as_in_after_effects() {
    let steps = [0u8, 16, 32, 64, 96, 128, 160, 192, 224, 255];
    let mut img = Image::new(10, 1);
    for (x, v) in steps.iter().enumerate() {
        let v = *v as f32 / 255.0;
        img.set(x as u32, 0, [v, v, v, 1.0]);
    }
    let p = |pts: &[(f32, f32)]| Value::Str(pts.iter().map(|(x, y)| format!("{},{}", x / 255.0, y / 255.0)).collect::<Vec<_>>().join(" "));
    let rgb = run("ec.color.curves", &img, &[("rgb", p(&[(0.0, 0.0), (125.0, 199.0), (255.0, 255.0)]))]);
    let red = run("ec.color.curves", &img, &[("red", p(&[(0.0, 0.0), (128.0, 44.0), (255.0, 255.0)]))]);
    let ae_rgb = [0, 30, 59, 115, 164, 202, 227, 242, 250, 255];
    let ae_red = [0, 0, 1, 6, 19, 44, 83, 135, 194, 255];
    for x in 0..10 {
        let g = (rgb.get(x as i64, 0)[1] * 255.0).round() as i32;
        let r = (red.get(x as i64, 0)[0] * 255.0).round() as i32;
        assert!((g - ae_rgb[x]).abs() <= 1, "RGB step {}: {g} vs AE {}", steps[x], ae_rgb[x]);
        assert!((r - ae_red[x]).abs() <= 1, "Red step {}: {r} vs AE {}", steps[x], ae_red[x]);
    }
}

#[test]
fn levels_ic_identity_and_red_only() {
    let img = ramp();
    assert_eq!(run("ec.color.levelsic", &img, &[]), img);
    let out = run("ec.color.levelsic", &img, &[("red/redOutWhite", num(0.5))]);
    let p = out.get(15, 3);
    assert!((p[0] - 0.5).abs() < 1e-4 && (p[1] - 0.0).abs() < 1e-4);
}

#[test]
fn auto_levels_stretches_low_contrast() {
    let mut img = Image::new(32, 4);
    for y in 0..4 {
        for x in 0..32 {
            let v = 0.4 + 0.2 * x as f32 / 31.0;
            img.set(x, y, [v, v, v, 1.0]);
        }
    }
    let out = run("ec.color.autolevels", &img, &[("blackClip", num(0.0)), ("whiteClip", num(0.0))]);
    let lo = out.data.iter().map(|p| p[0]).fold(1.0, f32::min);
    let hi = out.data.iter().map(|p| p[0]).fold(0.0, f32::max);
    assert!(lo < 0.02 && hi > 0.98, "{lo} {hi}");
    let ac = run("ec.color.autocontrast", &img, &[("blackClip", num(0.0)), ("whiteClip", num(0.0))]);
    assert!(ac.data.iter().map(|p| p[0]).fold(0.0, f32::max) > 0.98);
}

#[test]
fn auto_color_neutralises_cast() {
    let mut img = Image::new(16, 4);
    for y in 0..4 {
        for x in 0..16 {
            let v = x as f32 / 15.0;
            img.set(x, y, [v * 0.6 + 0.1, v * 0.9, v * 0.5, 1.0]);
        }
    }
    let out = run("ec.color.autocolor", &img, &[]);
    let p = out.get(8, 1);
    assert!((p[0] - p[1]).abs() < 0.08 && (p[1] - p[2]).abs() < 0.08, "{p:?}");
}

#[test]
fn equalize_flattens_histogram() {
    let mut img = Image::new(64, 1);
    for x in 0..64 {
        let v = if x < 48 { 0.1 + x as f32 * 0.001 } else { 0.9 };
        img.set(x, 0, [v, v, v, 1.0]);
    }
    let out = run("ec.color.equalize", &img, &[]);
    // The crowded dark values are spread across the range.
    assert!(out.get(47, 0)[0] > 0.6, "{}", out.get(47, 0)[0]);
    assert!(out.get(0, 0)[0] < 0.1);
    assert_eq!(run("ec.color.equalize", &img, &[("amount", num(0.0))]), img);
}

#[test]
fn selective_color_zero_is_identity_and_reds_respond() {
    let img = ramp();
    assert_eq!(run("ec.color.selectivecolor", &img, &[]), img);
    let red = Image::filled(2, 2, [1.0, 0.1, 0.1, 1.0]);
    let out = run("ec.color.selectivecolor", &red, &[("details/reds/redsCyan", num(100.0)), ("method", Value::Enum(1))]);
    assert!(out.data[0][0] < 0.3, "{:?}", out.data[0]);
    let blue = Image::filled(2, 2, [0.1, 0.1, 1.0, 1.0]);
    assert_eq!(run("ec.color.selectivecolor", &blue, &[("details/reds/redsCyan", num(100.0))]), blue);
}

#[test]
fn color_offset_full_turn_is_identity() {
    let img = ramp();
    let out = run("ec.color.cccoloroffset", &img, &[("redPhase", num(360.0)), ("greenPhase", num(720.0))]);
    assert_eq!(out, img);
    let out = run("ec.color.cccoloroffset", &img, &[("redPhase", num(180.0))]);
    assert!((out.get(0, 3)[0] - 0.5).abs() < 1e-4);
}

#[test]
fn broadcast_reduces_unsafe_color() {
    let img = Image::filled(2, 2, [1.0, 1.0, 0.0, 1.0]);
    let out = run("ec.color.broadcast", &img, &[]);
    assert!(out.data[0][0] < 1.0);
    let key = run("ec.color.broadcast", &img, &[("howToMakeSafe", Value::Enum(2))]);
    assert_eq!(key.data[0][3], 0.0);
    let gray = Image::filled(2, 2, [0.5, 0.5, 0.5, 1.0]);
    assert_eq!(run("ec.color.broadcast", &gray, &[]), gray);
}

#[test]
fn color_balance_hls_zero_identity_and_hue_shift() {
    let img = ramp();
    assert_eq!(run("ec.color.colorbalancehls", &img, &[]), img);
    let red = Image::filled(1, 1, [1.0, 0.0, 0.0, 1.0]);
    let out = run("ec.color.colorbalancehls", &red, &[("hue", num(120.0))]);
    assert!(out.data[0][1] > 0.99 && out.data[0][0] < 0.01);
}

#[test]
fn color_link_fills_with_average_keeping_alpha() {
    let mut img = Image::new(2, 1);
    img.set(0, 0, [1.0, 0.0, 0.0, 1.0]);
    img.set(1, 0, [0.0, 0.0, 0.5, 0.5]);
    let out = run("ec.color.colorlink", &img, &[]);
    assert!((out.data[1][3] - 0.5).abs() < 1e-5);
    let (c, _) = unpremul(out.data[0]);
    assert!((c[0] - 2.0 / 3.0).abs() < 1e-3 && (c[2] - 1.0 / 3.0).abs() < 1e-3, "{c:?}");
}

#[test]
fn cc_kernel_identity_and_toner_duotone() {
    let img = ramp();
    assert_eq!(run("ec.color.cckernel", &img, &[]), img);
    let g = Image::filled(1, 1, [0.5, 0.5, 0.5, 1.0]);
    let t = run("ec.color.cctoner", &g, &[("tones", Value::Enum(0)), ("highlights", col(1.0, 0.0, 0.0))]);
    assert!((t.data[0][0] - 0.5).abs() < 1e-3 && t.data[0][1] < 1e-3);
}

#[test]
fn shadow_highlight_lifts_shadows() {
    let img = Image::filled(8, 8, [0.1, 0.1, 0.1, 1.0]);
    let out = run("ec.color.shadowhighlight", &img, &[("autoAmounts", Value::Bool(false))]);
    assert!(out.data[0][0] > 0.15);
}

#[test]
fn levels_ic_clip_popups() {
    let img = Image::filled(2, 2, [0.9, 0.9, 0.9, 1.0]);
    let lv = [("rgb/rgbInWhite", num(0.5))];
    assert!((run("ec.color.levelsic", &img, &lv).data[0][0] - 1.0).abs() < 1e-6);
    let off = [("rgb/rgbInWhite", num(0.5)), ("clipToOutputWhite", Value::Enum(0))];
    assert!(run("ec.color.levelsic", &img, &off).data[0][0] > 1.5);
}

/// A host whose other layers and other frames are a fixed image.
struct ImgHost(Image);
impl crate::EffectHost for ImgHost {
    fn layer(&self, _: u64, _: bool) -> Option<crate::LayerPixels> {
        Some(crate::LayerPixels { buf: Buf { img: self.0.clone(), offset: [0.0; 2], scale: 1.0 }, size: [2.0, 2.0] })
    }
    fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
        None
    }
    fn self_at(&self, _: f64, _: usize) -> Option<Buf> {
        Some(Buf { img: self.0.clone(), offset: [0.0; 2], scale: 1.0 })
    }
}

fn run_host(id: &str, img: &Image, over: &[(&str, Value)], host: &ImgHost) -> Image {
    let env = crate::EffectEnv { host: Some(host), frame_rate: 10.0, ..Default::default() };
    crate::run_fx(id, over, img.clone(), 1.0, env).img
}

#[test]
fn color_link_samples_clip_and_source_layer() {
    let mut img = Image::new(5, 1);
    for (x, v) in [0.0f32, 0.1, 0.2, 0.3, 1.0].iter().enumerate() {
        img.set(x as u32, 0, [*v, *v, *v, 1.0]);
    }
    // Median ignores the outlier the average follows.
    let med = run("ec.color.colorlink", &img, &[("sampleSource", Value::Enum(1))]);
    assert!((med.data[0][0] - 0.2).abs() < 1e-5);
    // Brightest with 20 % clip skips the brightest pixel.
    let br = run("ec.color.colorlink", &img, &[("sampleSource", Value::Enum(2)), ("clip", num(20.0))]);
    assert!((br.data[0][0] - 0.3).abs() < 1e-5);
    // Min Alpha of a half-transparent source layer sets the alpha.
    let host = ImgHost(Image::filled(2, 2, [0.25, 0.25, 0.25, 0.5]));
    let out = run_host("ec.color.colorlink", &img, &[("sourceLayer", Value::Layer(Some(9))), ("sampleSource", Value::Enum(9))], &host);
    assert!((out.data[4][3] - 0.5).abs() < 1e-5);
    // Average of the source layer (not the effect's own pixels).
    let out = run_host("ec.color.colorlink", &img, &[("sourceLayer", Value::Layer(Some(9)))], &host);
    assert!((out.data[4][0] - 0.5).abs() < 1e-5);
}

#[test]
fn auto_levels_temporal_smoothing_reads_neighbour_frames() {
    let mut img = Image::new(4, 1);
    for x in 0..4 {
        let v = 0.3 + 0.1 * x as f32;
        img.set(x, 0, [v, v, v, 1.0]);
    }
    let wide = ImgHost(ramp());
    let p0 = [("blackClip", num(0.0)), ("whiteClip", num(0.0))];
    let alone = run_host("ec.color.autolevels", &img, &p0, &wide);
    let smooth = run_host("ec.color.autolevels", &img, &[p0[0].clone(), p0[1].clone(), ("temporalSmoothing", num(0.2))], &wide);
    // Alone the frame is stretched to full range; with full-range neighbours much less.
    assert!(alone.data[0][0] < 0.01);
    assert!(smooth.data[0][0] > 0.1, "{:?}", smooth.data[0]);
}

#[test]
fn shadow_highlight_clip_keeps_the_tonal_range() {
    let img = ramp();
    let lift = [("autoAmounts", Value::Bool(false)), ("shadowAmount", num(80.0))];
    let unclipped =
        run("ec.color.shadowhighlight", &img, &[lift[0].clone(), lift[1].clone(), ("moreOptions/blackClip", num(0.0)), ("moreOptions/whiteClip", num(0.0))]);
    let clipped = run("ec.color.shadowhighlight", &img, &[lift[0].clone(), lift[1].clone(), ("moreOptions/blackClip", num(1.0))]);
    let min = |im: &Image| im.data.iter().filter(|p| p[3] > 0.99).map(|p| luminance(p[0], p[1], p[2])).fold(f32::MAX, f32::min);
    assert!(min(&clipped) < min(&unclipped));
}
