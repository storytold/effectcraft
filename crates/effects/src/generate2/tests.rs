use super::*;
use crate::Params;
use effectcraft_raster::Image;

fn run(id: &str, set: &[(&str, Value)], img: Image) -> Buf {
    let s = crate::find(id).unwrap();
    let mut params = Params { values: s.params.iter().map(|p| (p.id.to_string(), p.default.clone())).collect() };
    let ls = [img.width as f64, img.height as f64];
    for p in &s.params {
        if let (ParamUi::Point, Value::Vec2(v)) = (&p.ui, &p.default) {
            params.values.insert(p.id.to_string(), Value::Vec2([v[0] * ls[0], v[1] * ls[1]]));
        }
    }
    for (k, v) in set {
        params.values.insert(k.to_string(), v.clone());
    }
    let ctx = EffectCtx { params: &params, time: 0.5, layer_size: ls, seed: 3, adjustment: false, env: Default::default() };
    crate::apply(s, &ctx, Buf { img, offset: [0.0, 0.0], scale: 1.0 })
}

#[test]
fn cell_pattern_is_deterministic_and_seeded() {
    let a = run("ec.generate.cellpattern", &[("size", num(10.0))], Image::new(40, 30));
    let b2 = run("ec.generate.cellpattern", &[("size", num(10.0))], Image::new(40, 30));
    assert_eq!(a.img, b2.img);
    let c = run("ec.generate.cellpattern", &[("size", num(10.0)), ("evolutionOptions/randomSeed", num(5.0))], Image::new(40, 30));
    assert_ne!(a.img, c.img);
    assert!(a.img.data.iter().all(|p| p[3] == 1.0 && (0.0..=1.0).contains(&p[0])));
}

#[test]
fn ellipse_ring_coverage() {
    let o = run("ec.generate.ellipse", &[("width", num(40.0)), ("height", num(40.0)), ("thickness", num(6.0))], Image::new(64, 64));
    // Ring passes through (32 + 20, 32); centre is empty.
    assert!(o.img.get(51, 32)[3] > 0.9, "{:?}", o.img.get(51, 32));
    assert!(o.img.get(32, 32)[3] < 1e-3);
    assert!(o.img.get(2, 2)[3] < 1e-3);
}

#[test]
fn lightning_is_deterministic_per_conductivity() {
    let a = run("ec.generate.advancedlightning", &[], Image::new(64, 48));
    let b2 = run("ec.generate.advancedlightning", &[], Image::new(64, 48));
    assert_eq!(a.img, b2.img);
    assert!(a.img.data.iter().any(|p| p[3] > 0.5));
    let c = run("ec.generate.advancedlightning", &[("conductivityState", num(11.0))], Image::new(64, 48));
    assert_ne!(a.img, c.img);
}

#[test]
fn beam_draws_between_points() {
    let o = run("ec.generate.beam", &[("length", num(100.0)), ("softness", num(0.0))], Image::new(64, 32));
    assert!(o.img.get(32, 16)[3] > 0.99);
    assert!(o.img.get(32, 2)[3] < 1e-3);
    assert!(o.img.get(2, 16)[3] < 1e-3);
}

#[test]
fn cell_pattern_tiles() {
    // 4×4-cell tiles of 8 px: the pattern repeats every 32 px.
    let set = [
        ("size", num(8.0)),
        ("tilingOptions/enableTiling", Value::Bool(true)),
        ("tilingOptions/cellsHorizontal", num(4.0)),
        ("tilingOptions/cellsVertical", num(4.0)),
    ];
    let o = run("ec.generate.cellpattern", &set, Image::new(64, 64));
    for (x, y) in [(3, 5), (10, 20), (17, 2)] {
        let (a, b2) = (o.img.get(x, y), o.img.get(x + 32, y + 32));
        assert!((a[0] - b2[0]).abs() < 1e-4, "{x},{y}: {a:?} vs {b2:?}");
    }
}

#[test]
fn radio_waves_expansion_velocity_and_star() {
    let r = |set: &[(&str, Value)], t: f64| {
        let s = crate::find("ec.generate.radiowaves").unwrap();
        let mut params = Params { values: s.params.iter().map(|p| (p.id.to_string(), p.default.clone())).collect() };
        params.values.insert("producerPoint".into(), Value::Vec2([32.0, 32.0]));
        for (k, v) in set {
            params.values.insert(k.to_string(), v.clone());
        }
        let ctx = EffectCtx { params: &params, time: t, layer_size: [64.0, 64.0], seed: 3, adjustment: false, env: Default::default() };
        crate::apply(s, &ctx, Buf { img: Image::new(64, 64), offset: [0.0, 0.0], scale: 1.0 })
    };
    // One wave born at t = 0, 0.2 s old: radius = Expansion × age = 20 px.
    let base = [("waveMotion/expansion", num(100.0)), ("waveMotion/frequency", num(0.5))];
    let o = r(&base, 0.2);
    assert!(o.img.get(52, 32)[3] > 0.9, "{:?}", o.img.get(52, 32));
    assert!(o.img.get(32, 32)[3] < 0.01);
    // Velocity moves the wave toward Direction (90° = right).
    let mut moving = base.to_vec();
    moving.push(("waveMotion/velocity", num(50.0)));
    let o = r(&moving, 0.2);
    assert!(o.img.get(62, 32)[3] > 0.9 && o.img.get(52, 32)[3] < 0.5, "{:?}", o.img.get(62, 32));
    // A 4-point star: the outline dips toward the centre between the points.
    let mut star = base.to_vec();
    star.extend([("polygon/sides", num(4.0)), ("polygon/star", Value::Bool(true)), ("polygon/starDepth", num(-0.5))]);
    let o = r(&star, 0.2);
    assert!(o.img.get(32, 12)[3] > 0.9, "point straight up");
    // Between points (45°) the outline sits near half the radius.
    assert!(o.img.get(39, 24)[3] > 0.5, "{:?}", o.img.get(39, 24));
    assert!(o.img.get(46, 18)[3] < 0.01, "no circle there");
}

#[test]
fn lightning_alpha_obstacle_and_expert_settings() {
    let sum = |i: &Image| i.data.iter().map(|p| p[3]).sum::<f32>();
    let alpha_in = |img: &Image, src: &Image| img.data.iter().zip(&src.data).filter(|(_, s)| s[3] > 0.5).map(|(p, _)| p[3]).sum::<f32>();
    // A wall of opaque pixels across the middle of the bolt's path.
    let mut wall = Image::new(64, 48);
    for y in 0..48 {
        for x in 0..64 {
            if (18..30).contains(&y) && x < 50 {
                wall.set(x, y, [0.0, 0.0, 0.0, 1.0]);
            }
        }
    }
    let opts =
        |o: f64| vec![("alphaObstacle", num(o)), ("compositeOnOriginal", Value::Bool(false)), ("glowSettings/glowRadius", num(0.0)), ("forking", num(60.0))];
    let free = run("ec.generate.advancedlightning", &opts(0.0), wall.clone());
    let avoid = run("ec.generate.advancedlightning", &opts(10.0), wall.clone());
    assert!(alpha_in(&avoid.img, &wall) < alpha_in(&free.img, &wall), "{} vs {}", alpha_in(&avoid.img, &wall), alpha_in(&free.img, &wall));
    // Fractal Type, Core Drain, Fork Strength, Termination Threshold, Min Fork Distance.
    let base = run("ec.generate.advancedlightning", &[], Image::new(64, 48));
    for (id, v) in [
        ("expertSettings/fractalType", Value::Enum(2)),
        ("expertSettings/coreDrain", num(100.0)),
        ("expertSettings/forkStrength", num(100.0)),
        ("expertSettings/forkVariation", num(100.0)),
    ] {
        assert_ne!(run("ec.generate.advancedlightning", &[(id, v)], Image::new(64, 48)).img, base.img, "{id}");
    }
    let drained = run("ec.generate.advancedlightning", &[("expertSettings/coreDrain", num(100.0))], Image::new(64, 48));
    assert!(sum(&drained.img) < sum(&base.img));
    let no_forks = run("ec.generate.advancedlightning", &[("expertSettings/minForkDistance", num(1000.0))], Image::new(64, 48));
    let term = run("ec.generate.advancedlightning", &[("expertSettings/terminationThreshold", num(100.0))], Image::new(64, 48));
    assert_eq!(no_forks.img, term.img, "no forks either way");
    assert!(sum(&no_forks.img) < sum(&base.img));
}

#[test]
fn lightning_types_and_decay_main_core() {
    let a = run("ec.generate.advancedlightning", &[], Image::new(64, 48));
    let strike = run("ec.generate.advancedlightning", &[("lightningType", Value::Enum(1))], Image::new(64, 48));
    assert_eq!(a.img, strike.img, "Strike is the default");
    for t in 0..8 {
        let o = run("ec.generate.advancedlightning", &[("lightningType", Value::Enum(t))], Image::new(64, 48));
        assert!(o.img.data.iter().any(|p| p[3] > 0.3), "type {t} draws");
    }
    let sum = |i: &Image| i.data.iter().map(|p| p[3]).sum::<f32>();
    let d = run("ec.generate.advancedlightning", &[("decayMainCore", Value::Bool(true)), ("decay", num(0.9))], Image::new(64, 48));
    let n = run("ec.generate.advancedlightning", &[("decay", num(0.9))], Image::new(64, 48));
    assert!(sum(&d.img) < sum(&n.img));
}

#[test]
fn beam_3d_perspective_controls_thickness() {
    // Short beam near the start: with 3D Perspective its thickness comes from its position
    // (thin, near the start); without, its head takes the full ending thickness.
    let set = |p: bool| {
        vec![
            ("length", num(20.0)),
            ("time", num(0.0)),
            ("softness", num(0.0)),
            ("startThickness", num(2.0)),
            ("endThickness", num(20.0)),
            ("perspective3d", Value::Bool(p)),
        ]
    };
    let on = run("ec.generate.beam", &set(true), Image::new(64, 32));
    let off = run("ec.generate.beam", &set(false), Image::new(64, 32));
    let sum = |i: &Image| i.data.iter().map(|p| p[3]).sum::<f32>();
    assert!(sum(&off.img) > sum(&on.img) * 1.5, "{} {}", sum(&off.img), sum(&on.img));
}

#[test]
fn lens_flare_brightest_at_center() {
    let o = run("ec.generate.lensflare", &[], Image::new(64, 64));
    let c = o.img.get(16, 16)[0];
    assert!(c > o.img.get(60, 4)[0]);
    assert!(c > 0.5);
}

fn rw(set: &[(&str, Value)], t: f64, env: crate::EffectEnv) -> Image {
    let mut v: Vec<(&str, Value)> =
        vec![("producerPoint", Value::Vec2([32.0, 32.0])), ("waveMotion/frequency", num(0.5)), ("waveMotion/expansion", num(100.0))];
    v.extend_from_slice(set);
    crate::run_fx("ec.generate.radiowaves", &v, Image::new(64, 64), t, env).img
}

#[test]
fn signed_distance_of_a_square() {
    let (w, h) = (20, 20);
    let inside: Vec<bool> = (0..w * h).map(|i| (5..15).contains(&(i % w)) && (5..15).contains(&(i / w))).collect();
    let d = crate::util::signed_distance(&inside, w, h);
    assert!((d.get(10, 10) + 4.5).abs() < 1e-4, "{}", d.get(10, 10));
    assert!((d.get(0, 10) - 4.5).abs() < 1e-4, "{}", d.get(0, 10));
    assert!((d.get(0, 0) - (50f32.sqrt() - 0.5)).abs() < 1e-4);
}

#[test]
fn radio_waves_mask_contours_profile_reflection_and_birth() {
    // Mask waves: a 10×10 square mask; after 0.1 s (10 px) the wave is 10 px outside it.
    let sq = crate::MaskShape { name: String::new(), points: vec![[27.0, 27.0], [37.0, 27.0], [37.0, 37.0], [27.0, 37.0]], closed: true, inverted: false };
    let masks = [sq];
    let env = crate::EffectEnv { masks: &masks, ..Default::default() };
    let m = rw(&[("waveType", Value::Enum(2)), ("waveMask/mask", num(1.0))], 0.1, env);
    assert!(m.get(47, 32)[3] > 0.9, "{:?}", m.get(47, 32));
    assert!(m.get(32, 32)[3] < 0.01 && m.get(40, 32)[3] < 0.01);
    // Rounded corner: diagonal distance 10 from (37, 37).
    assert!(m.get(44, 44)[3] > 0.5, "{:?}", m.get(44, 44));
    // No mask chosen: nothing drawn.
    assert!(rw(&[("waveType", Value::Enum(2))], 0.1, env).data.iter().all(|p| p[3] == 0.0));
    // Image Contours: the bright square of a source layer.
    struct Src;
    impl crate::EffectHost for Src {
        fn layer(&self, _: u64, _: bool) -> Option<crate::LayerPixels> {
            let mut img = Image::new(64, 64);
            for y in 27..37 {
                for x in 27..37 {
                    img.set(x, y, [1.0, 1.0, 1.0, 1.0]);
                }
            }
            Some(crate::LayerPixels { buf: Buf { img, offset: [0.0; 2], scale: 1.0 }, size: [64.0, 64.0] })
        }
        fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
            None
        }
        fn params_at(&self, t: f64) -> Option<Params> {
            // Colour animates from red (born at 0) to green (born later).
            let s = crate::find("ec.generate.radiowaves").unwrap();
            let mut p = Params { values: s.params.iter().map(|p| (p.id.to_string(), p.default.clone())).collect() };
            p.values.insert("producerPoint".into(), Value::Vec2([32.0, 32.0]));
            p.values.insert("waveMotion/frequency".into(), num(1.0));
            p.values.insert("waveMotion/expansion".into(), num(20.0));
            p.values.insert("waveStroke/color".into(), if t < 0.5 { col(1.0, 0.0, 0.0) } else { col(0.0, 1.0, 0.0) });
            Some(p)
        }
    }
    let env = crate::EffectEnv { host: Some(&Src), ..Default::default() };
    let ic = rw(
        &[
            ("waveType", Value::Enum(1)),
            ("imageContour/sourceLayer", Value::Layer(Some(1))),
            ("imageContour/preBlur", num(0.0)),
            ("imageContour/tolerance", num(0.0)),
        ],
        0.1,
        env,
    );
    assert!(ic.get(47, 32)[3] > 0.9 && ic.get(32, 32)[3] < 0.01, "{:?}", ic.get(47, 32));
    // Source Center off the square shifts the contour onto the producer point accordingly.
    let sh = rw(
        &[
            ("waveType", Value::Enum(1)),
            ("imageContour/sourceLayer", Value::Layer(Some(1))),
            ("imageContour/sourceCenter", Value::Vec2([22.0, 32.0])),
            ("imageContour/preBlur", num(0.0)),
            ("imageContour/tolerance", num(0.0)),
        ],
        0.1,
        env,
    );
    assert!(sh.get(57, 32)[3] > 0.9, "{:?}", sh.get(57, 32));
    // Profile: Taper Both thins the stroke edges; Square is solid across.
    let wide = [("waveStroke/startWidth", num(10.0)), ("waveStroke/endWidth", num(10.0))];
    let sqr = rw(&wide, 0.2, crate::EffectEnv::default());
    let mut tap = wide.to_vec();
    tap.push(("waveStroke/profile", Value::Enum(3)));
    let tap = rw(&tap, 0.2, crate::EffectEnv::default());
    assert!(sqr.get(55, 32)[3] > 0.9 && tap.get(55, 32)[3] < 0.6, "{:?} {:?}", sqr.get(55, 32), tap.get(55, 32));
    assert!(tap.get(52, 32)[3] > 0.8);
    // Reflection: a fast wave bounces back off the right edge.
    let fast = [("waveMotion/velocity", num(200.0)), ("waveMotion/expansion", num(10.0))];
    let gone = rw(&fast, 0.3, crate::EffectEnv::default());
    let mut rf = fast.to_vec();
    rf.push(("waveMotion/reflection", Value::Bool(true)));
    let back = rw(&rf, 0.3, crate::EffectEnv::default());
    let sum = |i: &Image| i.data.iter().map(|p| p[3]).sum::<f32>();
    assert!(sum(&back) > sum(&gone) + 5.0, "{} vs {}", sum(&back), sum(&gone));
    // Parameters Are Set At Birth: the older wave keeps red, the newer one is green.
    let both = rw(&[("parametersAreSetAt", Value::Enum(0)), ("waveMotion/frequency", num(1.0)), ("waveMotion/expansion", num(20.0))], 1.15, env);
    let (old, new) = (both.get(55, 32), both.get(35, 32));
    assert!(old[0] > 0.9 && old[1] < 0.1, "{old:?}");
    assert!(new[1] > 0.9 && new[0] < 0.1, "{new:?}");
}

#[test]
fn cell_pattern_hq_variants_antialias() {
    let s = crate::find("ec.generate.cellpattern").unwrap();
    let r = |pat: u32| {
        let mut params = Params { values: s.params.iter().map(|p| (p.id.to_string(), p.default.clone())).collect() };
        params.values.insert("cellPattern".into(), Value::Enum(pat));
        params.values.insert("size".into(), num(12.0));
        let ctx = EffectCtx { params: &params, time: 0.0, layer_size: [48.0, 48.0], seed: 1, adjustment: false, env: Default::default() };
        crate::apply(s, &ctx, Buf { img: Image::new(48, 48), offset: [0.0; 2], scale: 1.0 }).img
    };
    let distinct = |img: &Image| {
        let mut v: Vec<u32> = img.data.iter().map(|p| (p[0] * 10000.0) as u32).collect();
        v.sort_unstable();
        v.dedup();
        v.len()
    };
    let (std, hq) = (r(4), r(9));
    assert_eq!(CELL_PATTERNS[9], "Crystallize HQ");
    assert_ne!(std, hq);
    assert!(distinct(&hq) > distinct(&std) * 2, "{} vs {}", distinct(&hq), distinct(&std));
}
