use super::*;
use crate::{EffectEnv, EffectHost, Image, LayerPixels, run_fx};

fn close(a: M3, b: M3, tol: f64) {
    for i in 0..3 {
        for j in 0..3 {
            assert!((a[i][j] - b[i][j]).abs() < tol, "{a:?} vs {b:?}");
        }
    }
}

#[test]
fn aces_matrices_match_published_values() {
    let ap0 = space_by_name("ACES2065-1").unwrap();
    let ap1 = space_by_name("ACEScg").unwrap();
    // Academy S-2014-004 AP0 → AP1.
    close(
        matrix(ap0, ap1),
        [
            [1.451_439_316_1, -0.236_510_746_9, -0.214_928_569_3],
            [-0.076_553_773_4, 1.176_229_699_8, -0.099_675_926_4],
            [0.008_316_148_4, -0.006_032_449_8, 0.997_716_301_4],
        ],
        2e-4,
    );
    // ACEScg → linear Rec.709 with Bradford adaptation (published ACES config value).
    let lin = space_by_name("Linear Rec.709 (sRGB)").unwrap();
    close(matrix(ap1, lin), [[1.705_05, -0.621_79, -0.083_26], [-0.130_26, 1.140_80, -0.010_55], [-0.024_00, -0.128_97, 1.152_97]], 2e-3);
    // White maps to white.
    let w = mul_vec(&matrix(ap1, lin), [1.0, 1.0, 1.0]);
    assert!(w.iter().all(|v| (v - 1.0).abs() < 1e-3), "{w:?}");
}

#[test]
fn transfer_functions_round_trip() {
    for tf in [Tf::Srgb, Tf::Gamma(2.4), Tf::AcesCct, Tf::Pq] {
        for v in [0.0, 0.001, 0.18, 0.5, 1.0, 4.0] {
            let r = tf.decode(tf.encode(v));
            assert!((r - v).abs() < 1e-6 * v.max(1.0), "{tf:?} {v} {r}");
        }
    }
    // ACEScct of 18 % grey (S-2016-001).
    assert!((Tf::AcesCct.encode(0.18) - 0.413_588).abs() < 1e-5);
    // PQ: 100 nits (1.0) encodes to ≈ 0.508.
    assert!((Tf::Pq.encode(1.0) - 0.508).abs() < 1e-3);
}

#[test]
fn color_space_transform_effect_round_trips() {
    let img = Image::filled(4, 4, [0.2, 0.5, 0.7, 1.0]);
    let srgb = SPACES.iter().position(|s| s.name == "sRGB").unwrap() as u32;
    let cg = SPACES.iter().position(|s| s.name == "ACEScg").unwrap() as u32;
    let a = run_fx("ec.color.ociocolorspace", &[("source", Value::Enum(srgb)), ("destination", Value::Enum(cg))], img.clone(), 0.0, EffectEnv::default());
    assert_ne!(a.img.data, img.data);
    let b = run_fx("ec.color.ociocolorspace", &[("source", Value::Enum(cg)), ("destination", Value::Enum(srgb))], a.img.clone(), 0.0, EffectEnv::default());
    let c = run_fx(
        "ec.color.ociocolorspace",
        &[("source", Value::Enum(srgb)), ("destination", Value::Enum(cg)), ("direction", Value::Enum(1))],
        a.img,
        0.0,
        EffectEnv::default(),
    );
    for (x, y) in b.img.data.iter().zip(&img.data).chain(c.img.data.iter().zip(&img.data)) {
        assert!((0..4).all(|k| (x[k] - y[k]).abs() < 1e-4), "{x:?} {y:?}");
    }
}

#[test]
fn cdl_math() {
    let cdl = Cdl { slope: [1.2, 1.0, 0.8], offset: [0.05, 0.0, -0.02], power: [1.1, 1.0, 0.9], sat: 0.8 };
    let c = [0.3, 0.5, 0.7];
    // Hand computation of the v1.2 formula.
    let sop = [(0.3f64 * 1.2 + 0.05).powf(1.1), 0.5, (0.7f64 * 0.8 - 0.02).powf(0.9)];
    let l = sop[0] * 0.2126 + sop[1] * 0.7152 + sop[2] * 0.0722;
    let want = sop.map(|v| l + 0.8 * (v - l));
    let got = cdl.apply(c, true);
    for k in 0..3 {
        assert!((got[k] - want[k]).abs() < 1e-12);
    }
    let back = cdl.invert(got, true);
    for k in 0..3 {
        assert!((back[k] - c[k]).abs() < 1e-9);
    }
    // v1.2 clamps, No Clamp keeps negatives.
    let neg = Cdl { offset: [-0.5; 3], ..Cdl::default() };
    assert_eq!(neg.apply([0.2; 3], true), [0.0; 3]);
    assert!((neg.apply([0.2; 3], false)[0] + 0.3).abs() < 1e-12);
    let img = Image::filled(2, 2, [0.3, 0.5, 0.7, 1.0]);
    let out = run_fx("ec.color.ociocdl", &[("slopeRed", num(1.2)), ("offsetRed", num(0.05)), ("powerRed", num(1.1))], img, 0.0, EffectEnv::default());
    let r = (0.3f64 * 1.2 + 0.05).powf(1.1);
    assert!((out.img.get(0, 0)[0] as f64 - r).abs() < 1e-5);
}

fn identity_lattice(n: usize, f: impl Fn([f32; 3]) -> [f32; 3]) -> Vec<[f32; 3]> {
    let mut v = vec![];
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                let s = (n - 1) as f32;
                v.push(f([r as f32 / s, g as f32 / s, b as f32 / s]));
            }
        }
    }
    v
}

/// Sony Imageworks `.spi1d`, `.spi3d` and `.spimtx` (Blender's configs use them, #410), and
/// malformed ones rejected without panicking.
#[test]
fn spi_files_parse() {
    let spi1d = "Version 1\nFrom 0.0 2.0\nLength 3\nComponents 1\n{\n0.0\n0.25\n1.0\n}\n";
    let x = parse_file(spi1d, "spi1d").unwrap();
    let o = x.apply([1.0, 0.5, 2.0], 1, false, "");
    assert!((o[0] - 0.25).abs() < 1e-6 && (o[1] - 0.125).abs() < 1e-6 && (o[2] - 1.0).abs() < 1e-6, "{o:?}");
    let back = x.apply(o, 1, true, "");
    assert!((back[0] - 1.0).abs() < 1e-5 && (back[2] - 2.0).abs() < 1e-5, "{back:?}");
    let three = parse_spi1d("Version 1\nFrom 0 1\nLength 2\nComponents 3\n{\n0 0 0\n1 0.5 2\n}\n").unwrap();
    assert_eq!(three.d1, vec![[0.0; 3], [1.0, 0.5, 2.0]]);
    let mut spi3d = String::from("SPILUT 1.0\n3 3\n2 2 2\n");
    for (r, g, b) in (0..8).map(|i| (i & 1, (i >> 1) & 1, i >> 2)) {
        spi3d += &format!("{r} {g} {b} {} {} {}\n", r as f32 * 0.5, g, b);
    }
    let x = parse_file(&spi3d, "spi3d").unwrap();
    let o = x.apply([1.0, 0.5, 0.25], 1, false, "");
    assert!((o[0] - 0.5).abs() < 1e-6 && (o[1] - 0.5).abs() < 1e-6 && (o[2] - 0.25).abs() < 1e-6, "{o:?}");
    let m = parse_file("2 0 0 6553.5\n0 1 0 0\n0 0 1 0\n", "spimtx").unwrap();
    let o = m.apply([0.25, 0.5, 1.0], 1, false, "");
    assert!((o[0] - 0.6).abs() < 1e-6 && o[1] == 0.5 && o[2] == 1.0, "{o:?}");
    assert!((m.apply(o, 1, true, "")[0] - 0.25).abs() < 1e-6);
    for bad in [
        "Version 1\nFrom 0 1\nLength 99999999999\nComponents 1\n{\n0\n}\n",
        "Version 1\nFrom 1 0\nLength 2\nComponents 1\n{\n0\n1\n}\n",
        "Version 1\nFrom 0 1\nLength 3\nComponents 1\n{\n0\n1\n}\n",
        "Version 1\nFrom 0 1\nLength 2\nComponents 2\n{\n0 0\n1 1\n}\n",
    ] {
        assert!(parse_spi1d(bad).is_none(), "{bad}");
    }
    assert!(parse_spi3d("SPILUT 1.0\n3 3\n4000 4000 4000\n").is_none());
    assert!(parse_spi3d("SPILUT 1.0\n3 3\n2 2 2\n9 0 0 1 1 1\n").is_none());
    assert!(parse_spi3d("SPILUT 1.0\n3 3\n2 2 2\n0 0 0 1 1 1\n").is_none(), "incomplete lattice");
    assert!(parse_spimtx("1 0 0\n0 1 0\n").is_none());
    assert!(parse_spimtx("NaN 0 0 0 0 1 0 0 0 0 1 0").is_none());
}

#[test]
fn lut_files_parse_and_interpolate() {
    // .cube with a non-linear lattice: trilinear between lattice points.
    let f = |c: [f32; 3]| [c[0] * c[0], c[1], 1.0 - c[2]];
    let mut cube = String::from("LUT_3D_SIZE 3\n");
    for r in identity_lattice(3, f) {
        cube += &format!("{} {} {}\n", r[0], r[1], r[2]);
    }
    let Some(FileXform::Lut { lut, .. }) = parse_file(&cube, "cube") else { panic!() };
    // Trilinear between 0.5 (0.25) and 1.0 (1.0) at 0.75 → 0.625.
    let o = lut.apply_interp([0.75, 0.3, 0.2], 1);
    assert!((o[0] - 0.625).abs() < 1e-6 && (o[1] - 0.3).abs() < 1e-6 && (o[2] - 0.8).abs() < 1e-6, "{o:?}");
    // Nearest snaps.
    assert_eq!(lut.apply_interp([0.74, 0.0, 0.0], 0)[0], 0.25);
    // .3dl (blue fastest, 10-bit integers, mesh line).
    let mut tdl = String::from("0 511 1023\n");
    for r in 0..3 {
        for g in 0..3 {
            for b in 0..3 {
                let v = [r, g, b].map(|i| i as f64 * 511.5);
                tdl += &format!("{} {} {}\n", v[0].round(), v[1].round(), (1023.0 - v[2]).round());
            }
        }
    }
    let Some(FileXform::Lut { lut, .. }) = parse_file(&tdl, "3dl") else { panic!() };
    let o = lut.apply_interp([0.25, 0.5, 0.25], 1);
    assert!((o[0] - 0.25).abs() < 2e-3 && (o[1] - 0.5).abs() < 2e-3 && (o[2] - 0.75).abs() < 2e-3, "{o:?}");
    // .csp with a squaring pre-LUT on red and an identity 2³ lattice.
    let mut csp = String::from("CSPLUTV100\n3D\n\n3\n0 0.5 1\n0 0.25 1\n2\n0 1\n0 1\n2\n0 1\n0 1\n\n2 2 2\n");
    for r in identity_lattice(2, |c| c) {
        csp += &format!("{} {} {}\n", r[0], r[1], r[2]);
    }
    let x = parse_file(&csp, "csp").unwrap();
    let o = x.apply([0.5, 0.4, 0.6], 2, false, "");
    assert!((o[0] - 0.25).abs() < 1e-6 && (o[1] - 0.4).abs() < 1e-6, "{o:?}");
    let inv = x.apply(o, 2, true, "");
    assert!((inv[0] - 0.5).abs() < 1e-4, "{inv:?}");
    // CDL collection: pick by id.
    let ccc = r#"<ColorCorrectionCollection><ColorCorrection id="a"><SOPNode><Slope>2 2 2</Slope><Offset>0 0 0</Offset><Power>1 1 1</Power></SOPNode></ColorCorrection>
            <ColorCorrection id='b'><SOPNode><Slope>0.5 0.5 0.5</Slope></SOPNode><SatNode><Saturation>1</Saturation></SatNode></ColorCorrection></ColorCorrectionCollection>"#;
    let x = parse_file(ccc, "ccc").unwrap();
    assert!((x.apply([0.4; 3], 2, false, "b")[0] - 0.2).abs() < 1e-6);
    assert!((x.apply([0.4; 3], 2, false, "a")[0] - 0.8).abs() < 1e-6);
    assert!((x.apply([0.4; 3], 2, false, "")[0] - 0.8).abs() < 1e-6);
    // Effect with inline content; inverse of a 3D lattice.
    let img = Image::filled(2, 2, [0.6, 0.3, 0.2, 1.0]);
    let out = run_fx("ec.color.ociofile", &[("file", Value::Str(cube.clone()))], img.clone(), 0.0, EffectEnv::default());
    let back = run_fx("ec.color.ociofile", &[("file", Value::Str(cube)), ("direction", Value::Enum(1))], out.img, 0.0, EffectEnv::default());
    assert!((back.img.get(0, 0)[0] - 0.6).abs() < 1e-3, "{:?}", back.img.get(0, 0));
    assert!(parse_file("garbage", "3dl").is_none());
}

#[test]
fn display_and_look_transforms() {
    // Linear 0.18 through the sRGB display / Standard view = sRGB encoding of 0.18.
    let img = Image::filled(2, 2, [0.18, 0.18, 0.18, 1.0]);
    let out = run_fx("ec.color.ociodisplay", &[], img.clone(), 0.0, EffectEnv::default());
    assert!((out.img.get(0, 0)[0] as f64 - Tf::Srgb.encode(0.18)).abs() < 1e-5);
    let back = run_fx("ec.color.ociodisplay", &[("direction", Value::Enum(1))], out.img, 0.0, EffectEnv::default());
    assert!((back.img.get(0, 0)[0] - 0.18).abs() < 1e-5);
    // Tone mapped view keeps highlights below 1.
    let hot = Image::filled(1, 1, [8.0, 8.0, 8.0, 1.0]);
    let tm = run_fx("ec.color.ociodisplay", &[("view", Value::Enum(1))], hot, 0.0, EffectEnv::default());
    assert!(tm.img.get(0, 0)[0] < 1.0);
    assert!((tonemap_inv(tonemap(3.0)) - 3.0).abs() < 1e-9);
    // Looks: Warm raises red over blue; its inverse undoes it.
    let grey = Image::filled(1, 1, [0.18, 0.18, 0.18, 1.0]);
    let w = run_fx("ec.color.ociolook", &[("looks", Value::Str("Warm".into()))], grey.clone(), 0.0, EffectEnv::default());
    let px = w.img.get(0, 0);
    assert!(px[0] > px[2]);
    let u = run_fx("ec.color.ociolook", &[("looks", Value::Str("-Warm".into()))], w.img, 0.0, EffectEnv::default());
    assert!((u.img.get(0, 0)[0] - 0.18).abs() < 1e-4 && (u.img.get(0, 0)[2] - 0.18).abs() < 1e-4);
    assert_eq!(parse_looks("Warm, -Cool, nope").len(), 2);
}

#[test]
fn profile_converter_converts_and_round_trips() {
    let img = Image::filled(2, 2, [0.8, 0.4, 0.1, 1.0]);
    // sRGB → Rec. 2020 (relative), then back.
    let a = run_fx(
        "ec.utility.colorprofileconverter",
        &[("inputProfile", Value::Enum(1)), ("outputProfile", Value::Enum(3))],
        img.clone(),
        0.0,
        EffectEnv::default(),
    );
    assert_ne!(a.img.data, img.data);
    let b =
        run_fx("ec.utility.colorprofileconverter", &[("inputProfile", Value::Enum(3)), ("outputProfile", Value::Enum(1))], a.img, 0.0, EffectEnv::default());
    assert!((0..3).all(|k| (b.img.get(0, 0)[k] - img.get(0, 0)[k]).abs() < 1e-4));
    // Same profile both sides: identity. Working space defaults to sRGB.
    let c = run_fx(
        "ec.utility.colorprofileconverter",
        &[("inputProfile", Value::Enum(0)), ("outputProfile", Value::Enum(1))],
        img.clone(),
        0.0,
        EffectEnv::default(),
    );
    assert_eq!(c.img.data, img.data);
    // Perceptual pulls out-of-gamut colours inside: saturated Rec.2020 green into sRGB.
    let g = Image::filled(1, 1, [0.0, 1.0, 0.0, 1.0]);
    let rel = run_fx(
        "ec.utility.colorprofileconverter",
        &[("inputProfile", Value::Enum(3)), ("outputProfile", Value::Enum(1))],
        g.clone(),
        0.0,
        EffectEnv::default(),
    );
    assert!(rel.img.get(0, 0)[0] < 0.0);
    let per = run_fx(
        "ec.utility.colorprofileconverter",
        &[("inputProfile", Value::Enum(3)), ("outputProfile", Value::Enum(1)), ("intent", Value::Enum(0))],
        g,
        0.0,
        EffectEnv::default(),
    );
    assert!(per.img.get(0, 0).iter().all(|v| *v >= -1e-6));
}

struct RefHost(Image);
impl EffectHost for RefHost {
    fn layer(&self, _: u64, _: bool) -> Option<LayerPixels> {
        None
    }
    fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
        None
    }
    fn self_at(&self, _: f64, _: usize) -> Option<Buf> {
        Some(Buf { img: self.0.clone(), offset: [0.0; 2], scale: 1.0 })
    }
}

#[test]
fn color_stabilizer_matches_the_reference_frame() {
    let mk = |k: f32, o: f32| {
        let mut img = Image::new(40, 40);
        for y in 0..40 {
            for x in 0..40 {
                let v = (x as f32 / 40.0) * k + o;
                img.set(x, y, [v, v * 0.9, v * 0.8, 1.0]);
            }
        }
        img
    };
    let reference = mk(1.0, 0.0);
    let flicker = mk(0.8, 0.1);
    let host = RefHost(reference.clone());
    let env = EffectEnv { host: Some(&host), ..Default::default() };
    let pts = [("blackPoint", Value::Vec2([5.0, 20.0])), ("midPoint", Value::Vec2([20.0, 20.0])), ("whitePoint", Value::Vec2([35.0, 20.0]))];
    let mut vals: Vec<(&str, Value)> = pts.to_vec();
    vals.push(("stabilize", Value::Enum(1)));
    vals.push(("sampleSize", num(1.0)));
    let out = run_fx("ec.color.colorstabilizer", &vals, flicker.clone(), 0.0, env);
    for x in [5i64, 20, 35] {
        let (o, r) = (out.img.get(x, 20), reference.get(x, 20));
        assert!((o[0] - r[0]).abs() < 0.01, "x {x}: {o:?} vs {r:?}");
    }
    // Deterministic, and a no-op without a host.
    let again = run_fx("ec.color.colorstabilizer", &vals, flicker.clone(), 0.0, env);
    assert_eq!(out.img.data, again.img.data);
    let none = run_fx("ec.color.colorstabilizer", &vals, flicker.clone(), 0.0, EffectEnv::default());
    assert_eq!(none.img.data, flicker.data);
    // Brightness mode corrects an offset at the black point.
    let lifted = mk(1.0, 0.1);
    let mut vals2: Vec<(&str, Value)> = pts.to_vec();
    vals2.push(("sampleSize", num(1.0)));
    let b = run_fx("ec.color.colorstabilizer", &vals2, lifted, 0.0, env);
    assert!((b.img.get(5, 20)[0] - reference.get(5, 20)[0]).abs() < 0.01);
}
