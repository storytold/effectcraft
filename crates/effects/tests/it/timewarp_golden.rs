//! Golden hashes of the CPU Timewarp (M13.29), with and without a Matte Layer.
//!
//! Timewarp's frame building is shared with the GPU compositor as a plan
//! (`effectcraft_effects::timewarp_plan`); this pins the CPU output of every method / Show /
//! Matte Channel combination, with motion blur and a Warp Layer, so refactoring the plan keeps
//! it bit-identical. Same hashing and re-pinning (`SIM_GOLDEN_PRINT=1`) as `sim_golden.rs`;
//! pinned for aarch64 macOS.

use effectcraft_effects::{Buf, EffectCtx, EffectEnv, EffectHost, LayerPixels, Params, apply, default_value, find};
use effectcraft_keyframe::Value;
use effectcraft_raster::Image;

const W: u32 = 48;
const H: u32 = 32;
const FPS: f64 = 10.0;
/// Layer ids served by [`Host`]: the same texture standing still (a Warp Layer), a matte white
/// on its left part with a soft edge, and a luminance matte.
const STILL: u64 = 7;
const MATTE: u64 = 8;
const LUMA: u64 = 9;

/// A texture shifted right by `dx` px (premultiplied, partly transparent).
fn texture(dx: f64) -> Image {
    let mut img = Image::new(W, H);
    for y in 0..H {
        for x in 0..W {
            let u = x as f64 - dx;
            let a = if (10..14).contains(&y) { 0.5 } else { 1.0 };
            let c = [(0.5 + 0.4 * (u * 0.4).sin()) as f32, (0.5 + 0.4 * (y as f64 * 0.3 + u * 0.1).cos()) as f32, ((u * 0.05).rem_euclid(1.0)) as f32];
            img.set(x, y, [c[0] * a, c[1] * a, c[2] * a, a]);
        }
    }
    img
}

struct Host;
impl EffectHost for Host {
    fn layer(&self, _: u64, _: bool) -> Option<LayerPixels> {
        None
    }
    fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
        None
    }
    fn self_at(&self, t: f64, _: usize) -> Option<Buf> {
        Some(Buf { img: texture((t * FPS).round() * 2.0), offset: [0.0; 2], scale: 1.0 })
    }
    fn layer_at(&self, id: u64, t: f64, _: bool) -> Option<LayerPixels> {
        let img = match id {
            STILL => texture(0.0),
            LUMA => {
                let mut m = Image::new(W, H);
                for y in 0..H {
                    for x in 0..W {
                        let l = ((x + y) as f32 / (W + H) as f32 + (t * 0.3) as f32).fract();
                        m.set(x, y, [l, l, l, 1.0]);
                    }
                }
                m
            }
            _ => {
                let mut m = Image::new(W, H);
                let edge = 20.0 + (t * FPS).round() * 1.5;
                for y in 0..H {
                    for x in 0..W {
                        let a = ((edge - x as f64) / 3.0).clamp(0.0, 1.0) as f32;
                        m.set(x, y, [a, a, a, a]);
                    }
                }
                m
            }
        };
        Some(LayerPixels { buf: Buf { img, offset: [0.0; 2], scale: 1.0 }, size: [W as f64, H as f64] })
    }
}

fn n(v: f64) -> Value {
    Value::Scalar(v)
}
fn e(v: u32) -> Value {
    Value::Enum(v)
}
fn l(id: u64) -> Value {
    Value::Layer(Some(id))
}

type Set = Vec<(&'static str, Value)>;

fn cases() -> Vec<Set> {
    let mut v: Vec<Set> = vec![vec![], vec![("method", e(0))], vec![("method", e(1))]];
    for method in 0..3 {
        for show in 0..4 {
            v.push(vec![("method", e(method)), ("matteLayer", l(MATTE)), ("show", e(show)), ("speed", n(37.0))]);
        }
    }
    for ch in 1..4 {
        v.push(vec![("matteLayer", l(LUMA)), ("matteChannel", e(ch)), ("speed", n(63.0))]);
    }
    v.push(vec![("matteLayer", l(MATTE)), ("warpLayer", l(STILL)), ("speed", n(41.0))]);
    v.push(vec![("matteLayer", l(MATTE)), ("motionBlur/enableMotionBlur", Value::Bool(true)), ("motionBlur/shutterSamples", n(3.0)), ("show", e(1))]);
    v.push(vec![("matteLayer", l(MATTE)), ("method", e(1)), ("tuning/buildFromOneImage", Value::Bool(true)), ("show", e(2))]);
    v.push(vec![("matteLayer", l(MATTE)), ("tuning/buildFromOneImage", Value::Bool(true)), ("speed", n(28.0))]);
    v.push(vec![("matteLayer", l(MATTE)), ("adjustTimeBy", e(1)), ("sourceFrame", n(4.4)), ("show", e(3))]);
    v
}

const TIMES: [f64; 3] = [0.55, 1.23, 0.8];

fn fnv(img: &Image) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in [img.width, img.height].iter().flat_map(|v| v.to_le_bytes()) {
        h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3);
    }
    for p in &img.data {
        for c in p {
            for b in c.to_bits().to_le_bytes() {
                h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3);
            }
        }
    }
    h
}

fn render(set: &Set, time: f64) -> Image {
    let spec = find("ec.time.timewarp").unwrap();
    let size = [W as f64, H as f64];
    let mut params = Params { values: spec.params.iter().map(|p| (p.id.to_string(), default_value(p, size))).collect() };
    for (k, v) in set {
        assert!(params.values.contains_key(*k), "unknown param {k}");
        params.values.insert(k.to_string(), v.clone());
    }
    let host = Host;
    let env = EffectEnv { host: Some(&host), frame_rate: FPS, comp_time: time, ..Default::default() };
    let ctx = EffectCtx { params: &params, time, layer_size: size, seed: 1, adjustment: false, env };
    apply(spec, &ctx, Buf { img: texture(0.0), offset: [0.0; 2], scale: 1.0 }).img
}

fn hashes() -> Vec<(String, u64)> {
    let mut v = Vec::new();
    for (si, set) in cases().iter().enumerate() {
        for t in TIMES {
            v.push((format!("timewarp/{si}/t{t}"), fnv(&render(set, t))));
        }
    }
    v
}

#[test]
fn timewarp_golden_hashes() {
    let got = hashes();
    if std::env::var_os("SIM_GOLDEN_PRINT").is_some() {
        for (name, h) in &got {
            println!("    (\"{name}\", 0x{h:016x}),");
        }
    }
    assert_eq!(got, hashes(), "Timewarp renders deterministically");
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        let bad: Vec<String> = got
            .iter()
            .zip(GOLDEN)
            .filter(|((n, h), (gn, gh))| n != gn || h != gh)
            .map(|((n, h), (_, gh))| format!("{n}: 0x{h:016x} (pinned 0x{gh:016x})"))
            .collect();
        assert_eq!(got.len(), GOLDEN.len(), "case count");
        assert!(bad.is_empty(), "CPU output changed:\n{}", bad.join("\n"));
    }
}

/// Pinned hashes (aarch64 macOS).
const GOLDEN: &[(&str, u64)] = &[
    ("timewarp/0/t0.55", 0x85905ea4083d5052),
    ("timewarp/0/t1.23", 0x6499c6f962fed8e7),
    ("timewarp/0/t0.8", 0x48493cd23ed32a5f),
    ("timewarp/1/t0.55", 0x9eca0f14f7cb2d95),
    ("timewarp/1/t1.23", 0xd0c16d4df51315b4),
    ("timewarp/1/t0.8", 0x48493cd23ed32a5f),
    ("timewarp/2/t0.55", 0x9d3703747d8e5b7f),
    ("timewarp/2/t1.23", 0xa9ea3d660b9fd38b),
    ("timewarp/2/t0.8", 0x48493cd23ed32a5f),
    ("timewarp/3/t0.55", 0x9eca0f14f7cb2d95),
    ("timewarp/3/t1.23", 0x48493cd23ed32a5f),
    ("timewarp/3/t0.8", 0x9eca0f14f7cb2d95),
    ("timewarp/4/t0.55", 0x91cbdc02063b9105),
    ("timewarp/4/t1.23", 0x6060a22b299bc92d),
    ("timewarp/4/t0.8", 0x91cbdc02063b9105),
    ("timewarp/5/t0.55", 0xc2e2170269f47324),
    ("timewarp/5/t1.23", 0x3cea3a307e21500d),
    ("timewarp/5/t0.8", 0xc2e2170269f47324),
    ("timewarp/6/t0.55", 0xe55ce2acace7ba15),
    ("timewarp/6/t1.23", 0x6a90b9e251de6d15),
    ("timewarp/6/t0.8", 0xe55ce2acace7ba15),
    ("timewarp/7/t0.55", 0x54020055b7d248c6),
    ("timewarp/7/t1.23", 0x48d89e15e1f7801c),
    ("timewarp/7/t0.8", 0x628bc4aba205a8a8),
    ("timewarp/8/t0.55", 0x467f6294e2c06d6c),
    ("timewarp/8/t1.23", 0xfd442392677e46ad),
    ("timewarp/8/t0.8", 0xc26b69bf0e79c9c3),
    ("timewarp/9/t0.55", 0xd2fddb3350b55277),
    ("timewarp/9/t1.23", 0x16807a7efa8f989a),
    ("timewarp/9/t0.8", 0xb6a045bcaccdc2a2),
    ("timewarp/10/t0.55", 0xa86ae0542d872015),
    ("timewarp/10/t1.23", 0x86dba3262d03f3d5),
    ("timewarp/10/t0.8", 0xfc37965f9c2e51d5),
    ("timewarp/11/t0.55", 0x240ee936695325a8),
    ("timewarp/11/t1.23", 0x7fe2ff24fedcd3f1),
    ("timewarp/11/t0.8", 0xd57ed4c5bfca415e),
    ("timewarp/12/t0.55", 0xad462efd17ecf4be),
    ("timewarp/12/t1.23", 0xd05b6e67e92eaf90),
    ("timewarp/12/t0.8", 0xcc3f297acfa6b646),
    ("timewarp/13/t0.55", 0x80821631b420f5fc),
    ("timewarp/13/t1.23", 0x69ef153147214217),
    ("timewarp/13/t0.8", 0xe81297c3aa0752fe),
    ("timewarp/14/t0.55", 0x79ff11a84d24d2f4),
    ("timewarp/14/t1.23", 0x54fccecd7a87efb5),
    ("timewarp/14/t0.8", 0x0055f9cce950e80f),
    ("timewarp/15/t0.55", 0x193233cd7098d460),
    ("timewarp/15/t1.23", 0x0171866bfbe232b3),
    ("timewarp/15/t0.8", 0x5a15d4a6e85931d7),
    ("timewarp/16/t0.55", 0x0d4df26b38ca39a0),
    ("timewarp/16/t1.23", 0x7502f65ccf41ce0e),
    ("timewarp/16/t0.8", 0x39b6e04e7935b0fe),
    ("timewarp/17/t0.55", 0x99ae36e79014f5dc),
    ("timewarp/17/t1.23", 0xa844571a39e97251),
    ("timewarp/17/t0.8", 0x2e6961e16f50eec6),
    ("timewarp/18/t0.55", 0x9d6d36c8d9303a6e),
    ("timewarp/18/t1.23", 0x63794786e758ad00),
    ("timewarp/18/t0.8", 0x2bc9775fc6f5723e),
    ("timewarp/19/t0.55", 0x11c1925988c75ab0),
    ("timewarp/19/t1.23", 0x19c13bccca289b25),
    ("timewarp/19/t0.8", 0x7f5ec6ec4c3a2aeb),
    ("timewarp/20/t0.55", 0x386ca6db13d4675b),
    ("timewarp/20/t1.23", 0xaee266701f5f9936),
    ("timewarp/20/t0.8", 0x3cea3a307e21500d),
    ("timewarp/21/t0.55", 0x21f0abd02bad8359),
    ("timewarp/21/t1.23", 0xe8c75365a1d6505a),
    ("timewarp/21/t0.8", 0x3ee1c9c17c7760a1),
    ("timewarp/22/t0.55", 0xdc518b5e0fc85047),
    ("timewarp/22/t1.23", 0xdc518b5e0fc85047),
    ("timewarp/22/t0.8", 0xdc518b5e0fc85047),
];
