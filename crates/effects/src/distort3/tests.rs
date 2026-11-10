use super::*;
use crate::{EffectEnv, MaskShape, Params};

fn img(w: u32, h: u32) -> Image {
    crate::util::gen_image(w, h, |x, y| [x as f32 / w as f32, y as f32 / h as f32, ((x * 7 + y * 3) % 11) as f32 / 11.0, 1.0])
}

/// Run with Point defaults scaled to the layer (like `instantiate`).
fn run(id: &str, set: &[(&str, Value)], im: Image, masks: &[MaskShape]) -> Buf {
    let s = crate::find(id).unwrap();
    let ls = [im.width as f64, im.height as f64];
    let mut params = Params { values: s.params.iter().map(|p| (p.id.to_string(), p.default.clone())).collect() };
    for ps in &s.params {
        if let (ParamUi::Point, Value::Vec2(v)) = (&ps.ui, &ps.default) {
            params.values.insert(ps.id.to_string(), Value::Vec2([v[0] * ls[0], v[1] * ls[1]]));
        }
    }
    for (k, v) in set {
        assert!(params.values.contains_key(*k), "{id}: {k}");
        params.values.insert(k.to_string(), v.clone());
    }
    let env = EffectEnv { masks, ..Default::default() };
    let ctx = EffectCtx { params: &params, time: 0.5, layer_size: ls, seed: 1, adjustment: false, env };
    crate::apply(s, &ctx, Buf { img: im, offset: [0.0, 0.0], scale: 1.0 })
}

fn maxd(a: &Image, b: &Image) -> f32 {
    assert_eq!((a.width, a.height), (b.width, b.height));
    a.data.iter().zip(&b.data).map(|(p, q)| (0..4).map(|i| (p[i] - q[i]).abs()).fold(0.0, f32::max)).fold(0.0, f32::max)
}

fn rect_mask(x0: f64, y0: f64, x1: f64, y1: f64) -> MaskShape {
    MaskShape { name: "m".into(), points: vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]], closed: true, inverted: false }
}

#[test]
fn identity_at_neutral_settings() {
    let im = img(40, 30);
    let cases: &[(&str, Vec<(&str, Value)>)] = &[
        ("ec.distort.ccbender", vec![]),
        ("ec.distort.ccpowerpin", vec![]),
        ("ec.distort.ccripplepulse", vec![]),
        ("ec.distort.ccslant", vec![]),
        ("ec.distort.ccsmear", vec![("reach", num(0.0))]),
        ("ec.distort.ccsplit", vec![("split", num(0.0))]),
        ("ec.distort.ccsplit2", vec![("split1", num(0.0)), ("split2", num(0.0))]),
        ("ec.distort.smear", vec![]),
        ("ec.distort.reshape", vec![]),
        ("ec.distort.twirllegacy", vec![]),
        ("ec.distort.warp", vec![("warpStyle", Value::Enum(5))]),
        ("ec.distort.upscale", vec![]),
    ];
    for (id, set) in cases {
        let o = run(id, set, im.clone(), &[]);
        assert!(maxd(&o.img, &im) < 1e-5, "{id} not identity");
        assert_eq!(o.offset, [0.0, 0.0], "{id}");
    }
    // Power pin with perspective 0 at default corners is also identity.
    let o = run("ec.distort.ccpowerpin", &[("perspective", num(0.0))], im.clone(), &[]);
    assert!(maxd(&o.img, &im) < 1e-4);
}

#[test]
fn all_deterministic() {
    let im = img(36, 28);
    let masks = [rect_mask(8.0, 8.0, 16.0, 16.0), rect_mask(12.0, 10.0, 22.0, 20.0), rect_mask(2.0, 2.0, 34.0, 26.0)];
    let cases: &[(&str, Vec<(&str, Value)>)] = &[
        ("ec.distort.ccbender", vec![("amount", num(40.0)), ("style", Value::Enum(3))]),
        ("ec.distort.ccblobbylize", vec![]),
        ("ec.distort.ccpowerpin", vec![("topLeft", pt(5.0, 3.0)), ("perspective", num(40.0))]),
        ("ec.distort.ccripplepulse", vec![("pulseLevel", num(80.0))]),
        ("ec.distort.ccslant", vec![("slant", num(30.0)), ("stretching", Value::Bool(true))]),
        ("ec.distort.ccsmear", vec![]),
        ("ec.distort.ccsplit", vec![]),
        ("ec.distort.ccsplit2", vec![("split2", num(10.0))]),
        ("ec.distort.smear", vec![("sourceMask", Value::Enum(1)), ("boundaryMask", Value::Enum(3)), ("maskOffset", pt(4.0, 2.0))]),
        ("ec.distort.reshape", vec![("sourceMask", Value::Enum(1)), ("destinationMask", Value::Enum(2)), ("boundaryMask", Value::Enum(3))]),
        ("ec.distort.twirllegacy", vec![("angle", num(90.0))]),
        ("ec.distort.warp", vec![("bend", num(50.0)), ("horizontalDistortion", num(20.0))]),
        ("ec.distort.upscale", vec![("scale", num(200.0))]),
    ];
    for (id, set) in cases {
        let a = run(id, set, im.clone(), &masks);
        let b = run(id, set, im.clone(), &masks);
        assert_eq!(a.img, b.img, "{id}");
        assert!(a.img.data.iter().all(|p| p.iter().all(|v| v.is_finite())), "{id}");
        assert!(a.img.width != im.width || a.img.height != im.height || maxd(&a.img, &im) > 1e-3, "{id} did nothing");
    }
}

#[test]
fn blobbylize_defaults_match_ae() {
    let s = crate::find("ec.distort.ccblobbylize").unwrap();
    let default = |id: &str| s.params.iter().find(|p| p.id == id).unwrap().default.clone();
    assert_eq!(default("property"), Value::Enum(0), "Property defaults to Red");
    assert_eq!(default("softness"), num(20.0));
    assert_eq!(default("cutAway"), num(25.0));
    assert_eq!(default("lightHeight"), num(65.0));
    assert_eq!(default("lightPosition"), pt(0.5, 0.5));
    assert_eq!(default("ambient"), num(75.0));
    assert_eq!(default("diffuse"), num(25.0));
    assert_eq!(default("specular"), num(100.0));
    assert_eq!(default("roughness"), num(0.025));
    assert_eq!(default("metal"), num(100.0));
}

#[test]
fn blobbylize_cut_away_thresholds_a_height_ramp() {
    // AE oracle: Cut Away is a linear height threshold with a ~1 px antialiased edge.
    let im = crate::util::gen_image(1920, 8, |x, _| {
        let h = x as f32 / 1920.0;
        [h, h, h, 1.0]
    });
    let o = run("ec.distort.ccblobbylize", &[("property", Value::Enum(0)), ("softness", num(0.0)), ("cutAway", num(25.0))], im, &[]);
    assert_eq!(o.img.get(400, 4)[3], 0.0, "below Cut Away must be transparent");
    assert_eq!(o.img.get(560, 4)[3], 1.0, "above Cut Away must be opaque");
    let edge = o.img.get(480, 4)[3];
    assert!((0.45..=0.55).contains(&edge), "edge alpha {edge} is not a one-pixel threshold");
    let partial = (450..510).filter(|&x| (0.001..0.999).contains(&o.img.get(x, 4)[3])).count();
    assert!(partial <= 2, "{partial} antialiased columns instead of one");
}

#[test]
fn blobbylize_cut_away_100_is_empty() {
    // Regression: the old clamp to 0.99 left the top of the ramp opaque at Cut Away 100.
    let im = crate::util::gen_image(1920, 8, |x, _| {
        let h = x as f32 / 1920.0;
        [h, h, h, 1.0]
    });
    let o = run("ec.distort.ccblobbylize", &[("property", Value::Enum(0)), ("softness", num(0.0)), ("cutAway", num(100.0))], im, &[]);
    assert!(o.img.data.iter().all(|p| p[3] == 0.0), "Cut Away 100 left opaque pixels");
}

#[test]
fn blobbylize_cut_away_100_clears_an_opaque_edge() {
    // An opaque source reaches h = 1 on the high side of an edge; the gradient antialias
    // used to leave m = 0.5 there, so Cut Away 100 still produced visible pixels.
    let im = crate::util::gen_image(64, 8, |x, _| if x < 32 { [1.0; 4] } else { [0.0; 4] });
    let o = run("ec.distort.ccblobbylize", &[("property", Value::Enum(3)), ("softness", num(0.0)), ("cutAway", num(100.0))], im, &[]);
    assert!(o.img.data.iter().all(|p| p[3] == 0.0), "Cut Away 100 left an opaque edge");
}

#[test]
fn blobbylize_softness_warps_a_step_matte_inward() {
    // AE oracle T2: a self alpha step at x = 960, Softness 20, Cut Away 0 puts the matte
    // edge at about x = 946, not at the source edge. The PR's matte-preserving version
    // left it at 960; the effect must warp the source matte.
    let im = crate::util::gen_image(1920, 32, |x, _| if x < 960 { [1.0; 4] } else { [0.0; 4] });
    let o = run(
        "ec.distort.ccblobbylize",
        &[
            ("property", Value::Enum(3)),
            ("softness", num(20.0)),
            ("cutAway", num(0.0)),
            ("ambient", num(100.0)),
            ("diffuse", num(0.0)),
            ("specular", num(0.0)),
            ("metal", num(0.0)),
        ],
        im,
        &[],
    );
    assert!(o.img.get(945, 16)[3] > 0.95, "the matte did not move inward: {}", o.img.get(945, 16)[3]);
    assert!(o.img.get(949, 16)[3] < 0.05, "source alpha leaked past the warped edge");
    assert_eq!(o.img.get(960, 16)[3], 0.0, "the source matte was preserved instead of warped");
}

/// A text layer's buffer: 20 × 10 opaque pixels at (90, 45)–(110, 55) of its comp-sized
/// (200 × 100) layer bounds, after `id` with `set`.
fn on_text(id: &str, set: &[(&str, Value)]) -> Buf {
    let s = crate::find(id).unwrap();
    let mut params = Params { values: s.params.iter().map(|p| (p.id.to_string(), p.default.clone())).collect() };
    for (k, v) in set {
        assert!(params.values.contains_key(*k), "{id}: {k}");
        params.values.insert(k.to_string(), v.clone());
    }
    let mut im = Image::new(20, 10);
    im.data.iter_mut().for_each(|p| *p = [1.0; 4]);
    let ctx = EffectCtx { params: &params, time: 0.0, layer_size: [200.0, 100.0], seed: 1, adjustment: false, env: Default::default() };
    crate::apply(s, &ctx, Buf { img: im, offset: [-90.0, -45.0], scale: 1.0 })
}

/// Alpha at layer point `p` (0 outside the buffer).
fn alpha_at(b: &Buf, p: [f64; 2]) -> f32 {
    let (x, y) = b.to_px(p);
    b.img.get(x.floor() as i64, y.floor() as i64)[3]
}

/// #659: CC Smear and CC Tiler on a text layer reach past the text's box to the layer bounds
/// (comp-sized, as in After Effects), not cut off where the glyphs end.
#[test]
fn effects_on_text_reach_past_its_box() {
    let smear = on_text("ec.distort.ccsmear", &[("from", pt(100.0, 50.0)), ("to", pt(140.0, 50.0)), ("reach", num(100.0)), ("radius", num(30.0))]);
    assert!(alpha_at(&smear, [130.0, 50.0]) > 0.9, "the smear is cut off at the text's box");
    assert!(smear.img.width < 200, "it grows only as far as pixels move");
    let tiles = on_text("ec.distort.cctiler", &[("scale", num(50.0)), ("center", pt(100.0, 50.0))]);
    assert!(alpha_at(&tiles, [100.0, 50.0]) > 0.9);
    assert!(alpha_at(&tiles, [2.0, 50.0]) > 0.9, "tiles fill the layer bounds");
    assert!(alpha_at(&tiles, [50.0, 50.0]) < 0.1);
    // A footage layer's buffer is its bounds already: no change.
    let o = run("ec.distort.ccsmear", &[], img(40, 30), &[]);
    assert_eq!((o.img.width, o.img.height, o.offset), (40, 30, [0.0, 0.0]));
}

#[test]
fn every_warp_style_runs_and_bends() {
    let im = img(40, 30);
    for s in 0..WARP_STYLES.len() as u32 {
        for axis in 0..2 {
            let o = run("ec.distort.warp", &[("warpStyle", Value::Enum(s)), ("warpAxis", Value::Enum(axis)), ("bend", num(50.0))], im.clone(), &[]);
            let pad = o.offset[0] as u32;
            // The layer centre's neighbourhood is still made of source pixels.
            assert!(o.img.get(pad as i64 + 20, pad as i64 + 15)[3] > 0.5, "style {s}");
            assert!(o.img.width > im.width);
        }
    }
    // Arc with positive bend raises the middle of the layer: the top edge in the middle
    // moves up (opaque pixels above the original top).
    let o = run("ec.distort.warp", &[("warpStyle", Value::Enum(3)), ("bend", num(50.0))], im.clone(), &[]);
    let pad = o.offset[1] as i64;
    assert!(o.img.get(o.offset[0] as i64 + 20, pad - 3)[3] > 0.5);
    assert!(o.img.get(o.offset[0] as i64 + 1, pad - 3)[3] < 0.5);
}

#[test]
fn power_pin_moves_corners() {
    let im = img(20, 20);
    // Pin to the right half: output x in 10..20 shows the whole layer squeezed.
    let set = [("topLeft", pt(10.0, 0.0)), ("bottomLeft", pt(10.0, 20.0))];
    let o = run("ec.distort.ccpowerpin", &set, im.clone(), &[]);
    assert_eq!(o.img.get(3, 10)[3], 0.0);
    let px = o.img.get(15, 10);
    assert!((px[0] - 0.5).abs() < 0.1, "{px:?}");
}

#[test]
fn power_pin_unstretch_expands_pinned_region() {
    let im = crate::util::gen_image(24, 16, |_, _| [1.0, 0.0, 0.0, 1.0]);
    let inset = [("topLeft", pt(6.0, 4.0)), ("topRight", pt(18.0, 4.0)), ("bottomRight", pt(18.0, 12.0)), ("bottomLeft", pt(6.0, 12.0))];
    let opaque = |o: &Buf| o.img.data.iter().filter(|p| p[3] > 0.99).count();
    let off = run("ec.distort.ccpowerpin", &inset, im.clone(), &[]);
    assert_eq!(opaque(&off), 96);
    let mut set = inset.to_vec();
    set.push(("unstretch", Value::Bool(true)));
    let on = run("ec.distort.ccpowerpin", &set, im.clone(), &[]);
    assert_eq!(opaque(&on), 24 * 16);
    assert!(on.img.get(0, 0)[3] > 0.99);
    // Default corners are the identity either way.
    let id = run("ec.distort.ccpowerpin", &[("unstretch", Value::Bool(true))], im.clone(), &[]);
    assert!(maxd(&id.img, &im) < 1e-4);
}

#[test]
fn split_opens_a_gap() {
    let im = img(40, 30);
    let o = run("ec.distort.ccsplit", &[("split", num(40.0))], im.clone(), &[]);
    assert_eq!(o.img.get(20, 15)[3], 0.0);
    assert_eq!(o.img.get(2, 2), im.get(2, 2));
}

#[test]
fn slant_shears_from_floor() {
    let im = img(20, 20);
    let o = run("ec.distort.ccslant", &[("slant", num(50.0))], im.clone(), &[]);
    let pad = o.offset[0] as i64;
    // Bottom row stays put; the top row shifts right by ~10 px.
    assert!((o.img.get(pad + 5, pad + 19)[0] - im.get(5, 19)[0]).abs() < 0.05);
    assert!((o.img.get(pad + 15, pad)[0] - im.get(5, 0)[0]).abs() < 0.08);
}

#[test]
fn reshape_moves_source_to_destination() {
    // A bright square at the source mask; reshape to a shifted destination mask.
    let im = crate::util::gen_image(40, 40, |x, y| if (10..20).contains(&x) && (10..20).contains(&y) { [1.0; 4] } else { [0.0, 0.0, 0.0, 1.0] });
    let masks = [rect_mask(10.0, 10.0, 20.0, 20.0), rect_mask(16.0, 10.0, 26.0, 20.0), rect_mask(0.0, 0.0, 40.0, 40.0)];
    let o = run("ec.distort.reshape", &[("sourceMask", Value::Enum(1)), ("destinationMask", Value::Enum(2)), ("boundaryMask", Value::Enum(3))], im, &masks);
    assert!(o.img.get(24, 15)[0] > 0.8, "{:?}", o.img.get(24, 15));
    assert!(o.img.get(11, 15)[0] < 0.3, "{:?}", o.img.get(11, 15));
}

#[test]
fn reshape_correspondence_points_steer_the_match() {
    // Matching outlines with a zero pair change nothing; a quarter-turn offset twists.
    let im = img(40, 40);
    let masks = [rect_mask(10.0, 10.0, 30.0, 30.0), rect_mask(10.0, 10.0, 30.0, 30.0), rect_mask(0.0, 0.0, 40.0, 40.0)];
    let base = [("sourceMask", Value::Enum(1)), ("destinationMask", Value::Enum(2)), ("boundaryMask", Value::Enum(3))];
    let same = run("ec.distort.reshape", &[base.as_slice(), &[("correspondencePoints", Value::Str("0,0".into()))]].concat(), im.clone(), &masks);
    assert!(maxd(&same.img, &im) < 1e-3, "matching outlines: no change");
    let twisted = run("ec.distort.reshape", &[base.as_slice(), &[("correspondencePoints", Value::Str("0.25,0".into()))]].concat(), im.clone(), &masks);
    assert!(maxd(&twisted.img, &im) > 0.1);
    // Fraction mapping.
    assert!((map_fraction(&[(0.0, 0.0), (0.5, 0.25)], 0.125, true) - 0.25).abs() < 1e-9);
    assert!((map_fraction(&[(0.2, 0.0)], 0.9, true) - 0.1).abs() < 1e-9);
    assert!((map_fraction(&[(0.5, 0.5)], 0.25, false) - 0.25).abs() < 1e-9);
}

#[test]
fn upscale_grows_about_centre() {
    let im = img(20, 10);
    let o = run("ec.distort.upscale", &[("scale", num(200.0)), ("detail", num(0.0))], im.clone(), &[]);
    assert_eq!((o.img.width, o.img.height), (40, 20));
    // Grown about the layer centre: layer point (0,0) now sits at buffer pixel (10,5), so the
    // centre (10,5) lands in the middle of the 40×20 buffer.
    assert_eq!(o.offset, [10.0, 5.0]);
    let c = o.img.sample_bilinear(20.0, 10.0);
    assert!((c[0] - im.sample_bilinear(10.0, 5.0)[0]).abs() < 0.03);
}

#[test]
fn twirl_legacy_rotates_inside_radius() {
    let im = img(40, 40);
    let o = run("ec.distort.twirllegacy", &[("angle", num(180.0)), ("radius", num(100.0))], im.clone(), &[]);
    assert!(maxd(&o.img, &im) > 0.1);
    // Corners lie outside the radius.
    assert_eq!(o.img.get(0, 0), im.get(0, 0));
}

#[test]
fn half_resolution_matches_scaled_geometry() {
    // Bender at scale 0.5: the buffer is half size, parameters in layer pixels.
    let s = crate::find("ec.distort.ccbender").unwrap();
    let mut params = Params { values: s.params.iter().map(|p| (p.id.to_string(), p.default.clone())).collect() };
    params.values.insert("amount".into(), num(30.0));
    params.values.insert("top".into(), pt(20.0, 0.0));
    params.values.insert("base".into(), pt(20.0, 40.0));
    let ctx = EffectCtx { params: &params, time: 0.0, layer_size: [40.0, 40.0], seed: 1, adjustment: false, env: Default::default() };
    let full = crate::apply(s, &ctx, Buf { img: img(40, 40), offset: [0.0; 2], scale: 1.0 });
    let half = crate::apply(s, &ctx, Buf { img: effectcraft_raster::resample(&img(40, 40), 20, 20), offset: [0.0; 2], scale: 0.5 });
    let a = full.img.sample_bilinear(20.0, 4.0)[3];
    let b = half.img.sample_bilinear(10.0, 2.0)[3];
    assert!((a - b).abs() < 0.3);
}
