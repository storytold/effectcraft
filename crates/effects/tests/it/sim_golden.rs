//! Golden hashes of the CPU simulation effects (M13.27).
//!
//! Renders each simulation effect whose CPU path draws through a shared per-frame plan (the
//! sprite plans of CC Rainfall / Snowfall / Star Burst / Hair, Wave World and Foam; the piece
//! plans of Shatter, Card Dance and Card Wipe; the closed-form lists of CC Bubbles / Drizzle /
//! Mr. Mercury; Caustics' setup) on a generated layer, at default and non-default parameters,
//! at several layer times (including a seek backwards through the simulation caches), in an
//! 8 bpc pipeline (input and output clamped and quantised) and a 32 bpc one (overbright input,
//! raw output), at full and at half resolution. Each frame's pixels are hashed (FNV-1a over the
//! `f32` bits) and compared with the pinned hashes below: any change to the CPU output of these
//! effects, however small, fails the test.
//!
//! The hashes are exact `f32` bit patterns, so they depend on the platform's libm (`sin`, `cos`,
//! `powf`…); they are pinned for aarch64 macOS (where the gates run). Elsewhere the test only
//! checks that every case renders deterministically. A case whose frame differs between macOS
//! releases only through libm also accepts that release's hash ([`LIBM_VARIANTS`]).
//!
//! `SIM_GOLDEN_PRINT=1` prints the table (to re-pin after an intended change);
//! `SIM_GOLDEN_OUT=<dir>` also writes every frame's raw pixels (`f32` little-endian RGBA) there.
//! To tell a libm difference from a change in the code, render the cases with `SIM_GOLDEN_OUT`
//! here and at the commit that pinned the hash, and compare the frames: identical frames mean
//! the code didn't change, and the new hash goes into [`LIBM_VARIANTS`].

use effectcraft_effects::{Buf, EffectCtx, EffectEnv, EffectHost, LayerPixels, Params, apply, default_value, find};
use effectcraft_keyframe::Value;
use effectcraft_raster::Image;

const W: u32 = 64;
const H: u32 = 48;
/// Layer ids served by [`Host`]: a texture and a luminance map.
const TEX: u64 = 7;
const MAP: u64 = 8;

/// The effect's own layer: a colour ramp with a clear hole and a half-transparent band
/// (`hdr` makes part of it overbright).
fn layer(hdr: bool) -> Image {
    let mut img = Image::new(W, H);
    for y in 0..H {
        for x in 0..W {
            let (u, v) = (x as f32 / W as f32, y as f32 / H as f32);
            let (dx, dy) = (x as f32 - 40.0, y as f32 - 20.0);
            let a = if dx * dx + dy * dy < 64.0 {
                0.0
            } else if (30..36).contains(&y) {
                0.45
            } else {
                1.0
            };
            let gain = if hdr && x < 20 { 1.6 } else { 1.0 };
            let c = [u * gain, v * gain, (0.3 + 0.4 * (u * 9.0).sin().abs()) * gain];
            img.set(x, y, [c[0] * a, c[1] * a, c[2] * a, a]);
        }
    }
    img
}

/// Another layer: a checker texture (`TEX`) or a radial luminance map (`MAP`).
fn other(id: u64) -> Image {
    let mut img = Image::new(W, H);
    for y in 0..H {
        for x in 0..W {
            let p = if id == TEX {
                let k = ((x / 8 + y / 8) % 2) as f32;
                [0.9 * k + 0.1, 0.3 + 0.5 * (1.0 - k), 0.6, 1.0]
            } else {
                let d = ((x as f32 - 32.0).powi(2) + (y as f32 - 24.0).powi(2)).sqrt() / 40.0;
                let l = (1.0 - d).clamp(0.0, 1.0);
                [l, l, l, 1.0]
            };
            img.set(x, y, p);
        }
    }
    img
}

struct Host;
impl EffectHost for Host {
    fn layer(&self, id: u64, _: bool) -> Option<LayerPixels> {
        let img = other(id);
        Some(LayerPixels { buf: Buf { img, offset: [0.0; 2], scale: 1.0 }, size: [W as f64, H as f64] })
    }
    fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
        None
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

/// (effect id, parameter sets) — the first set of each is the defaults.
fn cases() -> Vec<(&'static str, Vec<Set>)> {
    vec![
        (
            "ec.sim.ccrainfall",
            vec![
                vec![],
                vec![("drops", n(2000.0)), ("size", n(3.0)), ("wind", n(500.0)), ("extras/appearance", e(1)), ("transferMode", e(0))],
                vec![("compositeWithOriginal", Value::Bool(false)), ("windVariation", n(50.0)), ("spread", n(20.0)), ("extras/randomSeed", n(9.0))],
            ],
        ),
        (
            "ec.sim.ccsnowfall",
            vec![
                vec![],
                vec![("flakes", n(3000.0)), ("size", n(6.0)), ("transferMode", e(1)), ("wind", n(-100.0))],
                vec![("compositeWithOriginal", Value::Bool(false)), ("opacity", n(60.0))],
            ],
        ),
        ("ec.sim.ccstarburst", vec![vec![], vec![("gridSpacing", n(4.0)), ("blendWithOriginal", n(40.0)), ("speed", n(-2.0)), ("phase", n(30.0))]]),
        ("ec.sim.ccbubbles", vec![vec![], vec![("bubbleAmount", n(300.0)), ("reflectionType", e(1)), ("shadingType", e(3)), ("bubbleSize", n(2.0))]]),
        (
            "ec.sim.ccdrizzle",
            vec![vec![], vec![("dripRate", n(30.0)), ("longevity", n(2.0)), ("displacement", n(20.0)), ("shading/diffuse", n(50.0)), ("rippling", n(720.0))]],
        ),
        (
            "ec.sim.cchair",
            vec![
                vec![],
                vec![
                    ("length", n(12.0)),
                    ("density", n(300.0)),
                    ("hairColor/colorInheritance", n(60.0)),
                    ("weight", n(1.0)),
                    ("constantMass", Value::Bool(true)),
                ],
                vec![("hairfallMap/mapLayer", l(MAP)), ("hairfallMap/mapStrength", n(50.0)), ("hairfallMap/addNoise", n(30.0))],
            ],
        ),
        (
            "ec.sim.ccmrmercury",
            vec![vec![], vec![("birthRate", n(5.0)), ("velocity", n(3.0)), ("gravity", n(1.0)), ("animation", e(1)), ("blobInfluence", n(150.0))]],
        ),
        (
            "ec.sim.caustics",
            vec![
                vec![],
                vec![
                    ("bottom/bottom", l(TEX)),
                    ("water/waterSurface", l(MAP)),
                    ("water/causticsStrength", n(0.6)),
                    ("bottom/blur", n(4.0)),
                    ("lighting/lightType", e(1)),
                ],
                vec![
                    ("sky/sky", l(TEX)),
                    ("water/surfaceOpacity", n(0.7)),
                    ("bottom/repeatMode", e(1)),
                    ("bottom/scaling", n(1.5)),
                    ("water/waterSurface", l(MAP)),
                ],
            ],
        ),
        (
            "ec.sim.waveworld",
            vec![
                vec![],
                vec![("view", e(1))],
                vec![("view", e(1)), ("ground/ground", l(MAP)), ("ground/waveStrength", n(0.5)), ("heightMapControls/renderDryAreasAs", e(1))],
                vec![("wireframeControls/horizontalRotation", n(30.0)), ("simulation/gridResolution", n(30.0)), ("simulation/reflectEdges", e(5))],
            ],
        ),
        (
            "ec.sim.foam",
            vec![
                vec![],
                vec![("view", e(2)), ("producer/productionRate", n(3.0)), ("rendering/bubbleTexture", e(1)), ("rendering/blendMode", e(1))],
                vec![("view", e(2)), ("producer/productionRate", n(2.0)), ("rendering/bubbleOrientation", e(2))],
                vec![("view", e(1)), ("flowMap/flowMap", l(MAP)), ("flowMap/flowMapSteepness", n(0.5))],
                vec![("view", e(2)), ("rendering/environmentMap", l(TEX)), ("rendering/reflectionStrength", n(0.5))],
                vec![("view", e(2)), ("rendering/bubbleTexture", e(5)), ("rendering/bubbleTextureLayer", l(TEX))],
            ],
        ),
        (
            "ec.sim.shatter",
            vec![
                vec![],
                vec![("view", e(0))],
                vec![("view", e(0)), ("render", e(1))],
                vec![("view", e(0)), ("render", e(2)), ("shape/pattern", e(3)), ("cameraPosition/yRotation", n(25.0))],
                vec![
                    ("view", e(0)),
                    ("textures/backLayer", l(TEX)),
                    ("textures/frontMode", e(2)),
                    ("gradient/gradientLayer", l(MAP)),
                    ("gradient/shatterThreshold", n(50.0)),
                ],
                vec![("view", e(2))],
                vec![("view", e(4)), ("force2/force2Radius", n(0.3))],
            ],
        ),
        (
            "ec.sim.carddance",
            vec![
                vec![],
                vec![("yRotation/yRotSource", e(1)), ("yRotation/yRotMultiplier", n(90.0)), ("backLayer", l(TEX))],
                vec![("gradientLayer1", l(MAP)), ("zPosition/zPosSource", e(1)), ("zPosition/zPosMultiplier", n(2.0)), ("cameraPosition/yRotation", n(30.0))],
            ],
        ),
        (
            "ec.transition.cardwipe",
            vec![
                vec![],
                vec![("completion", n(40.0))],
                vec![
                    ("completion", n(60.0)),
                    ("flipAxis", e(2)),
                    ("flipOrder", e(8)),
                    ("gradientLayer", l(MAP)),
                    ("backLayer", l(TEX)),
                    ("timingRandomness", n(0.5)),
                    ("rotationJitter/xRotJitterAmount", n(20.0)),
                ],
                vec![("completion", n(50.0)), ("backSelf", Value::Bool(true)), ("cameraPosition/yRotation", n(20.0)), ("positionJitter/zJitterAmount", n(1.0))],
            ],
        ),
    ]
}

/// Layer times, in order (the third seeks backwards).
const TIMES: [f64; 4] = [0.5, 1.5, 0.8, 2.2];

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

fn render(id: &str, set: &Set, time: f64, bpc8: bool, half: bool) -> Image {
    let spec = find(id).unwrap_or_else(|| panic!("no effect {id}"));
    let size = [W as f64, H as f64];
    let mut params = Params { values: spec.params.iter().map(|p| (p.id.to_string(), default_value(p, size))).collect() };
    for (k, v) in set {
        assert!(params.values.contains_key(*k), "{id}: unknown param {k}");
        params.values.insert(k.to_string(), v.clone());
    }
    let mut img = layer(!bpc8);
    if bpc8 {
        img.clamp01();
        img.quantize(255.0);
    }
    let mut buf = Buf { img, offset: [0.0; 2], scale: 1.0 };
    if half {
        // A half-resolution preview buffer padded by 3 px.
        let (w2, h2) = (W / 2, H / 2);
        let mut small = Image::new(w2, h2);
        for y in 0..h2 {
            for x in 0..w2 {
                small.set(x, y, buf.img.sample_bilinear(x as f64 * 2.0 + 1.0, y as f64 * 2.0 + 1.0));
            }
        }
        buf = Buf { img: small, offset: [0.0; 2], scale: 0.5 };
        buf.pad(3);
    }
    let host = Host;
    let env = EffectEnv { host: Some(&host), frame_rate: 30.0, comp_time: time, ..Default::default() };
    let ctx = EffectCtx { params: &params, time, layer_size: size, seed: 1, adjustment: false, env };
    let mut out = apply(spec, &ctx, buf).img;
    if bpc8 {
        out.clamp01();
        out.quantize(255.0);
    }
    out
}

/// Every case's name and hash, in a fixed order.
fn hashes() -> Vec<(String, u64)> {
    let out_dir = std::env::var_os("SIM_GOLDEN_OUT").map(std::path::PathBuf::from);
    if let Some(d) = &out_dir {
        std::fs::create_dir_all(d).unwrap();
    }
    let mut v = Vec::new();
    for (id, sets) in cases() {
        for (si, set) in sets.iter().enumerate() {
            for (bpc8, half) in [(true, false), (false, false), (true, true)] {
                if half && si > 0 {
                    continue;
                }
                for t in TIMES {
                    let img = render(id, set, t, bpc8, half);
                    let name = format!("{id}/{si}/{}{}/t{t}", if bpc8 { "8bpc" } else { "32bpc" }, if half { "/half" } else { "" });
                    if let Some(d) = &out_dir {
                        let bytes: Vec<u8> = img.data.iter().flat_map(|p| p.iter().flat_map(|c| c.to_le_bytes())).collect();
                        let file = format!("{}_{}x{}.f32", name.replace('/', "_"), img.width, img.height);
                        std::fs::write(d.join(file), bytes).unwrap();
                    }
                    v.push((name, fnv(&img)));
                }
            }
        }
    }
    v
}

#[test]
fn sim_effects_golden_hashes() {
    let got = hashes();
    if std::env::var_os("SIM_GOLDEN_PRINT").is_some() {
        for (name, h) in &got {
            println!("    (\"{name}\", 0x{h:016x}),");
        }
    }
    // Deterministic: a second pass (warm simulation caches) renders the same frames.
    assert_eq!(got, hashes(), "simulation effects render deterministically");
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        assert_eq!(got.len(), GOLDEN.len(), "case count");
        let bad = mismatches(&got, GOLDEN, LIBM_VARIANTS);
        assert!(bad.is_empty(), "CPU output changed (if it may be libm, see the module docs):\n{}", bad.join("\n"));
    }
}

/// The cases whose hash is neither the pinned one nor one of the case's [`LIBM_VARIANTS`].
fn mismatches(got: &[(String, u64)], golden: &[(&str, u64)], variants: &[(&str, u64, &str)]) -> Vec<String> {
    got.iter()
        .zip(golden)
        .filter(|((n, h), (gn, gh))| n != gn || (h != gh && !variants.iter().any(|(vn, vh, _)| vn == n && vh == h)))
        .map(|((n, h), (_, gh))| format!("{n}: 0x{h:016x} (pinned 0x{gh:016x})"))
        .collect()
}

/// A libm variant passes for its own case only (#232).
#[test]
fn libm_variants_are_accepted_for_their_case_only() {
    let golden = [("a", 1), ("b", 2)];
    let variants = [("a", 9, "another libm")];
    let got = |a: u64, b: u64| vec![("a".to_string(), a), ("b".to_string(), b)];
    assert!(mismatches(&got(1, 2), &golden, &variants).is_empty());
    assert!(mismatches(&got(9, 2), &golden, &variants).is_empty(), "the variant");
    assert_eq!(mismatches(&got(9, 9), &golden, &variants), ["b: 0x0000000000000009 (pinned 0x0000000000000002)"], "not another case's");
    assert_eq!(mismatches(&got(3, 2), &golden, &variants).len(), 1);
    assert!(LIBM_VARIANTS.iter().all(|(n, ..)| GOLDEN.iter().any(|(g, _)| g == n)), "every variant names a pinned case");
}

/// Hashes also accepted for a case: the same frame rendered by the same code with another macOS
/// release's libm (case, hash, where it was seen).
const LIBM_VARIANTS: &[(&str, u64, &str)] = &[
    // #232: the pinning commit (ce6ece4) renders byte-identical frames there.
    ("ec.sim.ccmrmercury/1/32bpc/t2.2", 0x3823bd73a3cdab0d, "macOS 27.2 (26B5091g), rustc 1.98.0"),
];

/// Pinned hashes (aarch64 macOS).
const GOLDEN: &[(&str, u64)] = &[
    ("ec.sim.ccrainfall/0/8bpc/t0.5", 0xc9f1eb02dd83841c),
    ("ec.sim.ccrainfall/0/8bpc/t1.5", 0xa0c1cb9f7863ab73),
    ("ec.sim.ccrainfall/0/8bpc/t0.8", 0x76a8457047798c0b),
    ("ec.sim.ccrainfall/0/8bpc/t2.2", 0x0bc982029615526c),
    ("ec.sim.ccrainfall/0/32bpc/t0.5", 0x2ec57976f619e3aa),
    ("ec.sim.ccrainfall/0/32bpc/t1.5", 0x25887ad6e4460fbe),
    ("ec.sim.ccrainfall/0/32bpc/t0.8", 0x1062fa4190ca91e4),
    ("ec.sim.ccrainfall/0/32bpc/t2.2", 0x2fb4da789b215918),
    ("ec.sim.ccrainfall/0/8bpc/half/t0.5", 0x01fce92cb9ff8c8f),
    ("ec.sim.ccrainfall/0/8bpc/half/t1.5", 0xf1ba0d8ee63c3e0b),
    ("ec.sim.ccrainfall/0/8bpc/half/t0.8", 0xa8a0164bc92a14ff),
    ("ec.sim.ccrainfall/0/8bpc/half/t2.2", 0x67d35c99f25a13df),
    ("ec.sim.ccrainfall/1/8bpc/t0.5", 0x146342179f5f7b55),
    ("ec.sim.ccrainfall/1/8bpc/t1.5", 0x82938b1a9b62e199),
    ("ec.sim.ccrainfall/1/8bpc/t0.8", 0xad8315d113236c1e),
    ("ec.sim.ccrainfall/1/8bpc/t2.2", 0x12812d5fb9c0d681),
    ("ec.sim.ccrainfall/1/32bpc/t0.5", 0x6aa245f8792720e3),
    ("ec.sim.ccrainfall/1/32bpc/t1.5", 0xdc9f5b59dd2e2a4b),
    ("ec.sim.ccrainfall/1/32bpc/t0.8", 0xb7b04ec5a16b8131),
    ("ec.sim.ccrainfall/1/32bpc/t2.2", 0xebb1f9196da63f2d),
    ("ec.sim.ccrainfall/2/8bpc/t0.5", 0x1babb77ebff11f11),
    ("ec.sim.ccrainfall/2/8bpc/t1.5", 0x41bf388f3f3b3339),
    ("ec.sim.ccrainfall/2/8bpc/t0.8", 0x435293816d81099a),
    ("ec.sim.ccrainfall/2/8bpc/t2.2", 0x9a297933a8832d80),
    ("ec.sim.ccrainfall/2/32bpc/t0.5", 0xcdbb17ccd20f4f0a),
    ("ec.sim.ccrainfall/2/32bpc/t1.5", 0x69b76c53583ac694),
    ("ec.sim.ccrainfall/2/32bpc/t0.8", 0x4c4b97b43ffd4892),
    ("ec.sim.ccrainfall/2/32bpc/t2.2", 0xc000e3afa8cc1802),
    ("ec.sim.ccsnowfall/0/8bpc/t0.5", 0x4902ee2cc8121cec),
    ("ec.sim.ccsnowfall/0/8bpc/t1.5", 0xec6e258d904e2e58),
    ("ec.sim.ccsnowfall/0/8bpc/t0.8", 0xb6fa24e1d6531303),
    ("ec.sim.ccsnowfall/0/8bpc/t2.2", 0x967dc8b4ad50b800),
    ("ec.sim.ccsnowfall/0/32bpc/t0.5", 0x5efa2c1abce0583a),
    ("ec.sim.ccsnowfall/0/32bpc/t1.5", 0x7989ee5157a68bb7),
    ("ec.sim.ccsnowfall/0/32bpc/t0.8", 0xbe5a1aecf46b06c6),
    ("ec.sim.ccsnowfall/0/32bpc/t2.2", 0xd2cb8abc99074a71),
    ("ec.sim.ccsnowfall/0/8bpc/half/t0.5", 0x714c277ac1498abb),
    ("ec.sim.ccsnowfall/0/8bpc/half/t1.5", 0x1d9a7a459132d01f),
    ("ec.sim.ccsnowfall/0/8bpc/half/t0.8", 0x9e8ccc56836d3cee),
    ("ec.sim.ccsnowfall/0/8bpc/half/t2.2", 0x6230ba51f651ec04),
    ("ec.sim.ccsnowfall/1/8bpc/t0.5", 0xd4567d9f2e0eef94),
    ("ec.sim.ccsnowfall/1/8bpc/t1.5", 0xb984d079bf586b0b),
    ("ec.sim.ccsnowfall/1/8bpc/t0.8", 0x0f35cbdf3edcd1ac),
    ("ec.sim.ccsnowfall/1/8bpc/t2.2", 0x8ab2b3ec603f6357),
    ("ec.sim.ccsnowfall/1/32bpc/t0.5", 0x5b80a72e5b9e19d1),
    ("ec.sim.ccsnowfall/1/32bpc/t1.5", 0x942a71721808d306),
    ("ec.sim.ccsnowfall/1/32bpc/t0.8", 0xbc3c3d18f59b57e0),
    ("ec.sim.ccsnowfall/1/32bpc/t2.2", 0xd9aa7cee5eced805),
    ("ec.sim.ccsnowfall/2/8bpc/t0.5", 0xd178b436f537bccd),
    ("ec.sim.ccsnowfall/2/8bpc/t1.5", 0xf9266f14378fb055),
    ("ec.sim.ccsnowfall/2/8bpc/t0.8", 0xd2aa631eb71bebc5),
    ("ec.sim.ccsnowfall/2/8bpc/t2.2", 0x135daf08008becf5),
    ("ec.sim.ccsnowfall/2/32bpc/t0.5", 0xa1f25e8ebf21c5f5),
    ("ec.sim.ccsnowfall/2/32bpc/t1.5", 0xd0a62f035f3ec9fd),
    ("ec.sim.ccsnowfall/2/32bpc/t0.8", 0x870e13f276a711d5),
    ("ec.sim.ccsnowfall/2/32bpc/t2.2", 0x9741c0ed03cd6c3d),
    ("ec.sim.ccstarburst/0/8bpc/t0.5", 0xe80ad07d5bc3b678),
    ("ec.sim.ccstarburst/0/8bpc/t1.5", 0xda7dcfdb20e295e1),
    ("ec.sim.ccstarburst/0/8bpc/t0.8", 0xa8db6303514533c7),
    ("ec.sim.ccstarburst/0/8bpc/t2.2", 0x7c275f7c06398711),
    ("ec.sim.ccstarburst/0/32bpc/t0.5", 0x6ceb6aa0551271e5),
    ("ec.sim.ccstarburst/0/32bpc/t1.5", 0xa50179d8d7da314c),
    ("ec.sim.ccstarburst/0/32bpc/t0.8", 0x3bc00d0f8df2db1d),
    ("ec.sim.ccstarburst/0/32bpc/t2.2", 0xe00e29974180f9a9),
    ("ec.sim.ccstarburst/0/8bpc/half/t0.5", 0x17c16c265ebeea85),
    ("ec.sim.ccstarburst/0/8bpc/half/t1.5", 0x9a2ac9097b23dc38),
    ("ec.sim.ccstarburst/0/8bpc/half/t0.8", 0x230c029f2a9f16cd),
    ("ec.sim.ccstarburst/0/8bpc/half/t2.2", 0xc0c35f4be8027854),
    ("ec.sim.ccstarburst/1/8bpc/t0.5", 0x6cfc05cdfd447035),
    ("ec.sim.ccstarburst/1/8bpc/t1.5", 0xc893b074efe2bda4),
    ("ec.sim.ccstarburst/1/8bpc/t0.8", 0x36741b8f3f32f2b6),
    ("ec.sim.ccstarburst/1/8bpc/t2.2", 0x60ddae12bc2caae9),
    ("ec.sim.ccstarburst/1/32bpc/t0.5", 0x021ba744f467c7a6),
    ("ec.sim.ccstarburst/1/32bpc/t1.5", 0x5c39f8b1bd0e9e43),
    ("ec.sim.ccstarburst/1/32bpc/t0.8", 0xdf20aa9dcf4b3e22),
    ("ec.sim.ccstarburst/1/32bpc/t2.2", 0x4fa441737bfcb64d),
    ("ec.sim.ccbubbles/0/8bpc/t0.5", 0xdcd187e75eeb2fae),
    ("ec.sim.ccbubbles/0/8bpc/t1.5", 0x835ca088747736ed),
    ("ec.sim.ccbubbles/0/8bpc/t0.8", 0x5cbb1a6fbb77b41d),
    ("ec.sim.ccbubbles/0/8bpc/t2.2", 0x3b168068f5bfa533),
    ("ec.sim.ccbubbles/0/32bpc/t0.5", 0x524b84d14edd77e2),
    ("ec.sim.ccbubbles/0/32bpc/t1.5", 0xfd7a17f0d40f2a27),
    ("ec.sim.ccbubbles/0/32bpc/t0.8", 0x70dd2b2b4ff0f7ce),
    ("ec.sim.ccbubbles/0/32bpc/t2.2", 0xae21dd5cc2c2c07c),
    ("ec.sim.ccbubbles/0/8bpc/half/t0.5", 0x931b4ca707604220),
    ("ec.sim.ccbubbles/0/8bpc/half/t1.5", 0x21c25d4555abebe9),
    ("ec.sim.ccbubbles/0/8bpc/half/t0.8", 0x7e05972683359632),
    ("ec.sim.ccbubbles/0/8bpc/half/t2.2", 0xadc4f7219dab4921),
    ("ec.sim.ccbubbles/1/8bpc/t0.5", 0x89d27a2fe2011067),
    ("ec.sim.ccbubbles/1/8bpc/t1.5", 0x2bbfee897bf5bf6e),
    ("ec.sim.ccbubbles/1/8bpc/t0.8", 0x26cc704e7b06fffc),
    ("ec.sim.ccbubbles/1/8bpc/t2.2", 0x959d331545d96479),
    ("ec.sim.ccbubbles/1/32bpc/t0.5", 0xaf486cebfcbf4641),
    ("ec.sim.ccbubbles/1/32bpc/t1.5", 0xe150e9fdb558036f),
    ("ec.sim.ccbubbles/1/32bpc/t0.8", 0x6395f3967cb24284),
    ("ec.sim.ccbubbles/1/32bpc/t2.2", 0x68a55894d86ffb9e),
    ("ec.sim.ccdrizzle/0/8bpc/t0.5", 0x2bdfbc17d92b65b5),
    ("ec.sim.ccdrizzle/0/8bpc/t1.5", 0x8c354c46f99e35d3),
    ("ec.sim.ccdrizzle/0/8bpc/t0.8", 0x891f331ba47ee557),
    ("ec.sim.ccdrizzle/0/8bpc/t2.2", 0x8db151e9e451b59f),
    ("ec.sim.ccdrizzle/0/32bpc/t0.5", 0x47f83c1a0a74380b),
    ("ec.sim.ccdrizzle/0/32bpc/t1.5", 0x9a69e5dbd2bddb8e),
    ("ec.sim.ccdrizzle/0/32bpc/t0.8", 0x4ec301d126c74a0f),
    ("ec.sim.ccdrizzle/0/32bpc/t2.2", 0x88cd09e82039660d),
    ("ec.sim.ccdrizzle/0/8bpc/half/t0.5", 0xa38bd19f2a38adff),
    ("ec.sim.ccdrizzle/0/8bpc/half/t1.5", 0xe414af8369177cbd),
    ("ec.sim.ccdrizzle/0/8bpc/half/t0.8", 0x64c7bfedc7d8f8a0),
    ("ec.sim.ccdrizzle/0/8bpc/half/t2.2", 0x1030b0612f0a7064),
    ("ec.sim.ccdrizzle/1/8bpc/t0.5", 0x2f18567ef33ecb77),
    ("ec.sim.ccdrizzle/1/8bpc/t1.5", 0xf546d29ebd62b4d7),
    ("ec.sim.ccdrizzle/1/8bpc/t0.8", 0x2135dd3c9690cc59),
    ("ec.sim.ccdrizzle/1/8bpc/t2.2", 0x753b0d3a53c3fc50),
    ("ec.sim.ccdrizzle/1/32bpc/t0.5", 0xbaba25231b465246),
    ("ec.sim.ccdrizzle/1/32bpc/t1.5", 0x014aa9f4b64188e1),
    ("ec.sim.ccdrizzle/1/32bpc/t0.8", 0x19dd31b810b48450),
    ("ec.sim.ccdrizzle/1/32bpc/t2.2", 0x9b17242f972e8f01),
    ("ec.sim.cchair/0/8bpc/t0.5", 0xdced134d06f409bb),
    ("ec.sim.cchair/0/8bpc/t1.5", 0xdced134d06f409bb),
    ("ec.sim.cchair/0/8bpc/t0.8", 0xdced134d06f409bb),
    ("ec.sim.cchair/0/8bpc/t2.2", 0xdced134d06f409bb),
    ("ec.sim.cchair/0/32bpc/t0.5", 0xd47914a2af05029f),
    ("ec.sim.cchair/0/32bpc/t1.5", 0xd47914a2af05029f),
    ("ec.sim.cchair/0/32bpc/t0.8", 0xd47914a2af05029f),
    ("ec.sim.cchair/0/32bpc/t2.2", 0xd47914a2af05029f),
    ("ec.sim.cchair/0/8bpc/half/t0.5", 0xb5c4943bff2b89e8),
    ("ec.sim.cchair/0/8bpc/half/t1.5", 0xb5c4943bff2b89e8),
    ("ec.sim.cchair/0/8bpc/half/t0.8", 0xb5c4943bff2b89e8),
    ("ec.sim.cchair/0/8bpc/half/t2.2", 0xb5c4943bff2b89e8),
    ("ec.sim.cchair/1/8bpc/t0.5", 0x33d0c6b05d019d2c),
    ("ec.sim.cchair/1/8bpc/t1.5", 0x33d0c6b05d019d2c),
    ("ec.sim.cchair/1/8bpc/t0.8", 0x33d0c6b05d019d2c),
    ("ec.sim.cchair/1/8bpc/t2.2", 0x33d0c6b05d019d2c),
    ("ec.sim.cchair/1/32bpc/t0.5", 0xf5c9d8cb3eb214d0),
    ("ec.sim.cchair/1/32bpc/t1.5", 0xf5c9d8cb3eb214d0),
    ("ec.sim.cchair/1/32bpc/t0.8", 0xf5c9d8cb3eb214d0),
    ("ec.sim.cchair/1/32bpc/t2.2", 0xf5c9d8cb3eb214d0),
    ("ec.sim.cchair/2/8bpc/t0.5", 0xdf8c1f2987c48746),
    ("ec.sim.cchair/2/8bpc/t1.5", 0xdf8c1f2987c48746),
    ("ec.sim.cchair/2/8bpc/t0.8", 0xdf8c1f2987c48746),
    ("ec.sim.cchair/2/8bpc/t2.2", 0xdf8c1f2987c48746),
    ("ec.sim.cchair/2/32bpc/t0.5", 0x089012eaec7ba181),
    ("ec.sim.cchair/2/32bpc/t1.5", 0x089012eaec7ba181),
    ("ec.sim.cchair/2/32bpc/t0.8", 0x089012eaec7ba181),
    ("ec.sim.cchair/2/32bpc/t2.2", 0x089012eaec7ba181),
    ("ec.sim.ccmrmercury/0/8bpc/t0.5", 0xc5ae9666156ac6dd),
    ("ec.sim.ccmrmercury/0/8bpc/t1.5", 0x11bfbc79d925e257),
    ("ec.sim.ccmrmercury/0/8bpc/t0.8", 0x8e9a8bfa24486a1b),
    ("ec.sim.ccmrmercury/0/8bpc/t2.2", 0xcbd32934872f2d8b),
    ("ec.sim.ccmrmercury/0/32bpc/t0.5", 0x90ed046e71ec328a),
    ("ec.sim.ccmrmercury/0/32bpc/t1.5", 0xa81014a6becc05c4),
    ("ec.sim.ccmrmercury/0/32bpc/t0.8", 0x3ef80645ace7696d),
    ("ec.sim.ccmrmercury/0/32bpc/t2.2", 0x9e2394847bff48d9),
    ("ec.sim.ccmrmercury/0/8bpc/half/t0.5", 0x67178855cecf6517),
    ("ec.sim.ccmrmercury/0/8bpc/half/t1.5", 0xe7cbe48bb03c0c53),
    ("ec.sim.ccmrmercury/0/8bpc/half/t0.8", 0x2c620ddf4def721a),
    ("ec.sim.ccmrmercury/0/8bpc/half/t2.2", 0x0d5ac246f91420d2),
    ("ec.sim.ccmrmercury/1/8bpc/t0.5", 0x8fdf48fde422b87c),
    ("ec.sim.ccmrmercury/1/8bpc/t1.5", 0x3cfa3751ddfa8fb8),
    ("ec.sim.ccmrmercury/1/8bpc/t0.8", 0xd888b8992c30d568),
    ("ec.sim.ccmrmercury/1/8bpc/t2.2", 0x2fa9235ca54e59d7),
    ("ec.sim.ccmrmercury/1/32bpc/t0.5", 0xe2a9bce2c0816db7),
    ("ec.sim.ccmrmercury/1/32bpc/t1.5", 0xd7dee79eb28d5682),
    ("ec.sim.ccmrmercury/1/32bpc/t0.8", 0x6bffba0e29931b07),
    ("ec.sim.ccmrmercury/1/32bpc/t2.2", 0x2fc62cc580704437),
    ("ec.sim.caustics/0/8bpc/t0.5", 0x5dba5c7617fb1684),
    ("ec.sim.caustics/0/8bpc/t1.5", 0x5dba5c7617fb1684),
    ("ec.sim.caustics/0/8bpc/t0.8", 0x5dba5c7617fb1684),
    ("ec.sim.caustics/0/8bpc/t2.2", 0x5dba5c7617fb1684),
    ("ec.sim.caustics/0/32bpc/t0.5", 0xbd813c461befaa49),
    ("ec.sim.caustics/0/32bpc/t1.5", 0xbd813c461befaa49),
    ("ec.sim.caustics/0/32bpc/t0.8", 0xbd813c461befaa49),
    ("ec.sim.caustics/0/32bpc/t2.2", 0xbd813c461befaa49),
    ("ec.sim.caustics/0/8bpc/half/t0.5", 0x9a602ac746c9e93a),
    ("ec.sim.caustics/0/8bpc/half/t1.5", 0x9a602ac746c9e93a),
    ("ec.sim.caustics/0/8bpc/half/t0.8", 0x9a602ac746c9e93a),
    ("ec.sim.caustics/0/8bpc/half/t2.2", 0x9a602ac746c9e93a),
    ("ec.sim.caustics/1/8bpc/t0.5", 0x282968317c36ccdf),
    ("ec.sim.caustics/1/8bpc/t1.5", 0x282968317c36ccdf),
    ("ec.sim.caustics/1/8bpc/t0.8", 0x282968317c36ccdf),
    ("ec.sim.caustics/1/8bpc/t2.2", 0x282968317c36ccdf),
    ("ec.sim.caustics/1/32bpc/t0.5", 0xc12df8561f396d45),
    ("ec.sim.caustics/1/32bpc/t1.5", 0xc12df8561f396d45),
    ("ec.sim.caustics/1/32bpc/t0.8", 0xc12df8561f396d45),
    ("ec.sim.caustics/1/32bpc/t2.2", 0xc12df8561f396d45),
    ("ec.sim.caustics/2/8bpc/t0.5", 0x52ab258ae56e9b15),
    ("ec.sim.caustics/2/8bpc/t1.5", 0x52ab258ae56e9b15),
    ("ec.sim.caustics/2/8bpc/t0.8", 0x52ab258ae56e9b15),
    ("ec.sim.caustics/2/8bpc/t2.2", 0x52ab258ae56e9b15),
    ("ec.sim.caustics/2/32bpc/t0.5", 0x9a820d0e7c6d11ea),
    ("ec.sim.caustics/2/32bpc/t1.5", 0x9a820d0e7c6d11ea),
    ("ec.sim.caustics/2/32bpc/t0.8", 0x9a820d0e7c6d11ea),
    ("ec.sim.caustics/2/32bpc/t2.2", 0x9a820d0e7c6d11ea),
    ("ec.sim.waveworld/0/8bpc/t0.5", 0x425d5d639d2a6495),
    ("ec.sim.waveworld/0/8bpc/t1.5", 0x233d7370e580804a),
    ("ec.sim.waveworld/0/8bpc/t0.8", 0x866da86ce5e78eb2),
    ("ec.sim.waveworld/0/8bpc/t2.2", 0xcbd7a044c6dea382),
    ("ec.sim.waveworld/0/32bpc/t0.5", 0x4614542961ef7f4c),
    ("ec.sim.waveworld/0/32bpc/t1.5", 0xa115264f4e963258),
    ("ec.sim.waveworld/0/32bpc/t0.8", 0xc85126a32bd330d5),
    ("ec.sim.waveworld/0/32bpc/t2.2", 0x46f9701affa77153),
    ("ec.sim.waveworld/0/8bpc/half/t0.5", 0xc343bb097002dc3a),
    ("ec.sim.waveworld/0/8bpc/half/t1.5", 0x93659d968d9bba0a),
    ("ec.sim.waveworld/0/8bpc/half/t0.8", 0xf4f1c3f612f7d50e),
    ("ec.sim.waveworld/0/8bpc/half/t2.2", 0xa9ee2f5da7fe832d),
    ("ec.sim.waveworld/1/8bpc/t0.5", 0x657d503fbc866cfd),
    ("ec.sim.waveworld/1/8bpc/t1.5", 0xf60841d4222a3db5),
    ("ec.sim.waveworld/1/8bpc/t0.8", 0x47df2b1e4d321a2d),
    ("ec.sim.waveworld/1/8bpc/t2.2", 0x44b790c2d32ba3b5),
    ("ec.sim.waveworld/1/32bpc/t0.5", 0x7474aa4c1c87d185),
    ("ec.sim.waveworld/1/32bpc/t1.5", 0xf47149ace39fc9a8),
    ("ec.sim.waveworld/1/32bpc/t0.8", 0x02a92148923fb8cf),
    ("ec.sim.waveworld/1/32bpc/t2.2", 0x140848626206b32e),
    ("ec.sim.waveworld/2/8bpc/t0.5", 0x657d503fbc866cfd),
    ("ec.sim.waveworld/2/8bpc/t1.5", 0xf60841d4222a3db5),
    ("ec.sim.waveworld/2/8bpc/t0.8", 0x47df2b1e4d321a2d),
    ("ec.sim.waveworld/2/8bpc/t2.2", 0x44b790c2d32ba3b5),
    ("ec.sim.waveworld/2/32bpc/t0.5", 0x7474aa4c1c87d185),
    ("ec.sim.waveworld/2/32bpc/t1.5", 0xf47149ace39fc9a8),
    ("ec.sim.waveworld/2/32bpc/t0.8", 0x02a92148923fb8cf),
    ("ec.sim.waveworld/2/32bpc/t2.2", 0x140848626206b32e),
    ("ec.sim.waveworld/3/8bpc/t0.5", 0xe66fe569bb504a87),
    ("ec.sim.waveworld/3/8bpc/t1.5", 0x135d95d12b5e9593),
    ("ec.sim.waveworld/3/8bpc/t0.8", 0x34fb2aeae7d23e3e),
    ("ec.sim.waveworld/3/8bpc/t2.2", 0x6742f5ea06644b41),
    ("ec.sim.waveworld/3/32bpc/t0.5", 0xf69a9ce9999451b8),
    ("ec.sim.waveworld/3/32bpc/t1.5", 0xe57d90b7f78d18a8),
    ("ec.sim.waveworld/3/32bpc/t0.8", 0x4261511821a6b346),
    ("ec.sim.waveworld/3/32bpc/t2.2", 0x1a8799c88f1de19d),
    ("ec.sim.foam/0/8bpc/t0.5", 0x5cd278dc5f5c9a2f),
    ("ec.sim.foam/0/8bpc/t1.5", 0x228bdb75e77a6aff),
    ("ec.sim.foam/0/8bpc/t0.8", 0xc8ddad379cccc367),
    ("ec.sim.foam/0/8bpc/t2.2", 0x67a62c95a2f3a57b),
    ("ec.sim.foam/0/32bpc/t0.5", 0x66c096dc124a6112),
    ("ec.sim.foam/0/32bpc/t1.5", 0x900d3e620dbe305d),
    ("ec.sim.foam/0/32bpc/t0.8", 0x3f56f85e02e7e418),
    ("ec.sim.foam/0/32bpc/t2.2", 0x0e73e0a0dca49782),
    ("ec.sim.foam/0/8bpc/half/t0.5", 0x574f9cde21ff4f21),
    ("ec.sim.foam/0/8bpc/half/t1.5", 0x0902634f55e51052),
    ("ec.sim.foam/0/8bpc/half/t0.8", 0x66f0f874708ae5d2),
    ("ec.sim.foam/0/8bpc/half/t2.2", 0x6ea1e7ce8ea003ee),
    ("ec.sim.foam/1/8bpc/t0.5", 0x895c58d34f83c027),
    ("ec.sim.foam/1/8bpc/t1.5", 0x384cc6dddfa509cb),
    ("ec.sim.foam/1/8bpc/t0.8", 0x10edb2950f3abf20),
    ("ec.sim.foam/1/8bpc/t2.2", 0x5828e73579b7a3ef),
    ("ec.sim.foam/1/32bpc/t0.5", 0x2b7f5c7523fe3d7c),
    ("ec.sim.foam/1/32bpc/t1.5", 0x3b97de79cb2af9d7),
    ("ec.sim.foam/1/32bpc/t0.8", 0xdbdebb5ee13bc1ac),
    ("ec.sim.foam/1/32bpc/t2.2", 0x83b8e3157a0fa3f1),
    ("ec.sim.foam/2/8bpc/t0.5", 0x8cbb009b4ae00eed),
    ("ec.sim.foam/2/8bpc/t1.5", 0xa1c76ae4a8f5e81d),
    ("ec.sim.foam/2/8bpc/t0.8", 0x71b078831f7c3825),
    ("ec.sim.foam/2/8bpc/t2.2", 0x6cbce434729fac45),
    ("ec.sim.foam/2/32bpc/t0.5", 0xdb815e0dc2564222),
    ("ec.sim.foam/2/32bpc/t1.5", 0x2c21e9929bb56ccc),
    ("ec.sim.foam/2/32bpc/t0.8", 0xec192d356612dc7b),
    ("ec.sim.foam/2/32bpc/t2.2", 0xcb35a37e005e956c),
    ("ec.sim.foam/3/8bpc/t0.5", 0x5cd278dc5f5c9a2f),
    ("ec.sim.foam/3/8bpc/t1.5", 0x228bdb75e77a6aff),
    ("ec.sim.foam/3/8bpc/t0.8", 0xc8ddad379cccc367),
    ("ec.sim.foam/3/8bpc/t2.2", 0x67a62c95a2f3a57b),
    ("ec.sim.foam/3/32bpc/t0.5", 0x66c096dc124a6112),
    ("ec.sim.foam/3/32bpc/t1.5", 0x900d3e620dbe305d),
    ("ec.sim.foam/3/32bpc/t0.8", 0x3f56f85e02e7e418),
    ("ec.sim.foam/3/32bpc/t2.2", 0x0e73e0a0dca49782),
    ("ec.sim.foam/4/8bpc/t0.5", 0x076f7eb41645efcd),
    ("ec.sim.foam/4/8bpc/t1.5", 0xa6c1c8c00dad7fe5),
    ("ec.sim.foam/4/8bpc/t0.8", 0x48577b43693836a5),
    ("ec.sim.foam/4/8bpc/t2.2", 0x2be729c4982520c5),
    ("ec.sim.foam/4/32bpc/t0.5", 0x3d878b31ba482c2a),
    ("ec.sim.foam/4/32bpc/t1.5", 0x06d56bccfa4a2a8f),
    ("ec.sim.foam/4/32bpc/t0.8", 0x223e3462616c09f9),
    ("ec.sim.foam/4/32bpc/t2.2", 0x712f7519ccb0ee85),
    ("ec.sim.foam/5/8bpc/t0.5", 0x1d5aff045cd59bb3),
    ("ec.sim.foam/5/8bpc/t1.5", 0xa5af2fbc438496c1),
    ("ec.sim.foam/5/8bpc/t0.8", 0xc2f37cbbfa58c9cf),
    ("ec.sim.foam/5/8bpc/t2.2", 0xa799e74d65a38b9c),
    ("ec.sim.foam/5/32bpc/t0.5", 0xc20ceff8ce81b2ab),
    ("ec.sim.foam/5/32bpc/t1.5", 0xc63b20f331aff24f),
    ("ec.sim.foam/5/32bpc/t0.8", 0x9b9336377932c657),
    ("ec.sim.foam/5/32bpc/t2.2", 0xe3e4a79eb5274405),
    ("ec.sim.shatter/0/8bpc/t0.5", 0x11bc1fc550fd36b3),
    ("ec.sim.shatter/0/8bpc/t1.5", 0x11bc1fc550fd36b3),
    ("ec.sim.shatter/0/8bpc/t0.8", 0x11bc1fc550fd36b3),
    ("ec.sim.shatter/0/8bpc/t2.2", 0x11bc1fc550fd36b3),
    ("ec.sim.shatter/0/32bpc/t0.5", 0xbd9e64dc060ca484),
    ("ec.sim.shatter/0/32bpc/t1.5", 0x57a2e177aefeb471),
    ("ec.sim.shatter/0/32bpc/t0.8", 0x6c80f0dad08201e8),
    ("ec.sim.shatter/0/32bpc/t2.2", 0x073710aac52ada1e),
    ("ec.sim.shatter/0/8bpc/half/t0.5", 0xa3b0e44d0b451b64),
    ("ec.sim.shatter/0/8bpc/half/t1.5", 0xa3b0e44d0b451b64),
    ("ec.sim.shatter/0/8bpc/half/t0.8", 0xa3b0e44d0b451b64),
    ("ec.sim.shatter/0/8bpc/half/t2.2", 0xa3b0e44d0b451b64),
    ("ec.sim.shatter/1/8bpc/t0.5", 0xc88b6a4f62591eeb),
    ("ec.sim.shatter/1/8bpc/t1.5", 0x69cbada9a90d0d41),
    ("ec.sim.shatter/1/8bpc/t0.8", 0x219f597f218a6701),
    ("ec.sim.shatter/1/8bpc/t2.2", 0x1e23f14cb92266dc),
    ("ec.sim.shatter/1/32bpc/t0.5", 0xb001f4e51f4f4588),
    ("ec.sim.shatter/1/32bpc/t1.5", 0x17c644c7ea8a45b6),
    ("ec.sim.shatter/1/32bpc/t0.8", 0x8b44b550a809723a),
    ("ec.sim.shatter/1/32bpc/t2.2", 0x69d7e5d356158e58),
    ("ec.sim.shatter/2/8bpc/t0.5", 0xb8c5537a29ef32dd),
    ("ec.sim.shatter/2/8bpc/t1.5", 0xb8c5537a29ef32dd),
    ("ec.sim.shatter/2/8bpc/t0.8", 0xb8c5537a29ef32dd),
    ("ec.sim.shatter/2/8bpc/t2.2", 0xb8c5537a29ef32dd),
    ("ec.sim.shatter/2/32bpc/t0.5", 0xee76d3faecd5d4aa),
    ("ec.sim.shatter/2/32bpc/t1.5", 0xee76d3faecd5d4aa),
    ("ec.sim.shatter/2/32bpc/t0.8", 0xee76d3faecd5d4aa),
    ("ec.sim.shatter/2/32bpc/t2.2", 0xee76d3faecd5d4aa),
    ("ec.sim.shatter/3/8bpc/t0.5", 0x8cbd4969338f29fb),
    ("ec.sim.shatter/3/8bpc/t1.5", 0x0f2e41b20477a7dc),
    ("ec.sim.shatter/3/8bpc/t0.8", 0x612787e664f0926f),
    ("ec.sim.shatter/3/8bpc/t2.2", 0x8d8a6e7a0d146b65),
    ("ec.sim.shatter/3/32bpc/t0.5", 0x1f8d55c160d494d2),
    ("ec.sim.shatter/3/32bpc/t1.5", 0xfcbf3d570106c4e6),
    ("ec.sim.shatter/3/32bpc/t0.8", 0x530d7e17f5d5ec8f),
    ("ec.sim.shatter/3/32bpc/t2.2", 0xb494747e58a22a9c),
    ("ec.sim.shatter/4/8bpc/t0.5", 0xf04d46c84a886095),
    ("ec.sim.shatter/4/8bpc/t1.5", 0x05d5d8062a7ae9b4),
    ("ec.sim.shatter/4/8bpc/t0.8", 0x444588b36b1f5504),
    ("ec.sim.shatter/4/8bpc/t2.2", 0x0a57aea5d9681bef),
    ("ec.sim.shatter/4/32bpc/t0.5", 0xee5ee39dc353dbb2),
    ("ec.sim.shatter/4/32bpc/t1.5", 0x3bc5403e37b9ddfc),
    ("ec.sim.shatter/4/32bpc/t0.8", 0x34f5803d12e9d128),
    ("ec.sim.shatter/4/32bpc/t2.2", 0x4a219d6e6cea1354),
    ("ec.sim.shatter/5/8bpc/t0.5", 0x7f04135ea7bc2091),
    ("ec.sim.shatter/5/8bpc/t1.5", 0xcf52da008e5af549),
    ("ec.sim.shatter/5/8bpc/t0.8", 0x24bd9267cec1c98f),
    ("ec.sim.shatter/5/8bpc/t2.2", 0x5e80d87871f083e3),
    ("ec.sim.shatter/5/32bpc/t0.5", 0xd90121f4b7e4532d),
    ("ec.sim.shatter/5/32bpc/t1.5", 0x4c5656a128853105),
    ("ec.sim.shatter/5/32bpc/t0.8", 0x62fdec8088cb1a35),
    ("ec.sim.shatter/5/32bpc/t2.2", 0x31c1ae800bf051c2),
    ("ec.sim.shatter/6/8bpc/t0.5", 0xf26a4f818409aa12),
    ("ec.sim.shatter/6/8bpc/t1.5", 0xf436cc239e3476f5),
    ("ec.sim.shatter/6/8bpc/t0.8", 0x1f1f246a8cc33892),
    ("ec.sim.shatter/6/8bpc/t2.2", 0xe6d0cd12c76f2c4e),
    ("ec.sim.shatter/6/32bpc/t0.5", 0x694cf382fa46e3aa),
    ("ec.sim.shatter/6/32bpc/t1.5", 0x127feb0a3c17e61c),
    ("ec.sim.shatter/6/32bpc/t0.8", 0xa5e024cf96d0c1d8),
    ("ec.sim.shatter/6/32bpc/t2.2", 0x751efd2c81c4745a),
    ("ec.sim.carddance/0/8bpc/t0.5", 0xc19ca1915b02090a),
    ("ec.sim.carddance/0/8bpc/t1.5", 0xc19ca1915b02090a),
    ("ec.sim.carddance/0/8bpc/t0.8", 0xc19ca1915b02090a),
    ("ec.sim.carddance/0/8bpc/t2.2", 0xc19ca1915b02090a),
    ("ec.sim.carddance/0/32bpc/t0.5", 0x9835eb69b01310cd),
    ("ec.sim.carddance/0/32bpc/t1.5", 0x9835eb69b01310cd),
    ("ec.sim.carddance/0/32bpc/t0.8", 0x9835eb69b01310cd),
    ("ec.sim.carddance/0/32bpc/t2.2", 0x9835eb69b01310cd),
    ("ec.sim.carddance/0/8bpc/half/t0.5", 0x90d874f633ea4408),
    ("ec.sim.carddance/0/8bpc/half/t1.5", 0x90d874f633ea4408),
    ("ec.sim.carddance/0/8bpc/half/t0.8", 0x90d874f633ea4408),
    ("ec.sim.carddance/0/8bpc/half/t2.2", 0x90d874f633ea4408),
    ("ec.sim.carddance/1/8bpc/t0.5", 0x8e86ed3501025de5),
    ("ec.sim.carddance/1/8bpc/t1.5", 0x8e86ed3501025de5),
    ("ec.sim.carddance/1/8bpc/t0.8", 0x8e86ed3501025de5),
    ("ec.sim.carddance/1/8bpc/t2.2", 0x8e86ed3501025de5),
    ("ec.sim.carddance/1/32bpc/t0.5", 0xd6df6185ccdb6999),
    ("ec.sim.carddance/1/32bpc/t1.5", 0xd6df6185ccdb6999),
    ("ec.sim.carddance/1/32bpc/t0.8", 0xd6df6185ccdb6999),
    ("ec.sim.carddance/1/32bpc/t2.2", 0xd6df6185ccdb6999),
    ("ec.sim.carddance/2/8bpc/t0.5", 0xf7c304abbe670be3),
    ("ec.sim.carddance/2/8bpc/t1.5", 0xf7c304abbe670be3),
    ("ec.sim.carddance/2/8bpc/t0.8", 0xf7c304abbe670be3),
    ("ec.sim.carddance/2/8bpc/t2.2", 0xf7c304abbe670be3),
    ("ec.sim.carddance/2/32bpc/t0.5", 0xd39adf204986b4e9),
    ("ec.sim.carddance/2/32bpc/t1.5", 0xd39adf204986b4e9),
    ("ec.sim.carddance/2/32bpc/t0.8", 0xd39adf204986b4e9),
    ("ec.sim.carddance/2/32bpc/t2.2", 0xd39adf204986b4e9),
    ("ec.transition.cardwipe/0/8bpc/t0.5", 0x8ad88caae775ea83),
    ("ec.transition.cardwipe/0/8bpc/t1.5", 0x8ad88caae775ea83),
    ("ec.transition.cardwipe/0/8bpc/t0.8", 0x8ad88caae775ea83),
    ("ec.transition.cardwipe/0/8bpc/t2.2", 0x8ad88caae775ea83),
    ("ec.transition.cardwipe/0/32bpc/t0.5", 0xe2b8332b96a1c76e),
    ("ec.transition.cardwipe/0/32bpc/t1.5", 0xe2b8332b96a1c76e),
    ("ec.transition.cardwipe/0/32bpc/t0.8", 0xe2b8332b96a1c76e),
    ("ec.transition.cardwipe/0/32bpc/t2.2", 0xe2b8332b96a1c76e),
    ("ec.transition.cardwipe/0/8bpc/half/t0.5", 0x730351fffb20e0c8),
    ("ec.transition.cardwipe/0/8bpc/half/t1.5", 0x730351fffb20e0c8),
    ("ec.transition.cardwipe/0/8bpc/half/t0.8", 0x730351fffb20e0c8),
    ("ec.transition.cardwipe/0/8bpc/half/t2.2", 0x730351fffb20e0c8),
    ("ec.transition.cardwipe/1/8bpc/t0.5", 0x955dfd5e5761773a),
    ("ec.transition.cardwipe/1/8bpc/t1.5", 0x955dfd5e5761773a),
    ("ec.transition.cardwipe/1/8bpc/t0.8", 0x955dfd5e5761773a),
    ("ec.transition.cardwipe/1/8bpc/t2.2", 0x955dfd5e5761773a),
    ("ec.transition.cardwipe/1/32bpc/t0.5", 0x90476fca5ecce0d5),
    ("ec.transition.cardwipe/1/32bpc/t1.5", 0x90476fca5ecce0d5),
    ("ec.transition.cardwipe/1/32bpc/t0.8", 0x90476fca5ecce0d5),
    ("ec.transition.cardwipe/1/32bpc/t2.2", 0x90476fca5ecce0d5),
    ("ec.transition.cardwipe/2/8bpc/t0.5", 0xbd2f2ff54b9ec732),
    ("ec.transition.cardwipe/2/8bpc/t1.5", 0x65d542cd14443396),
    ("ec.transition.cardwipe/2/8bpc/t0.8", 0xdfbe7b6b78a0a3f0),
    ("ec.transition.cardwipe/2/8bpc/t2.2", 0x165d56d4a9d1335f),
    ("ec.transition.cardwipe/2/32bpc/t0.5", 0x9434385a35a2dd49),
    ("ec.transition.cardwipe/2/32bpc/t1.5", 0xfe39b6b0d8f82589),
    ("ec.transition.cardwipe/2/32bpc/t0.8", 0x48cd644a9101ebff),
    ("ec.transition.cardwipe/2/32bpc/t2.2", 0x5ffafba81e1a2add),
    ("ec.transition.cardwipe/3/8bpc/t0.5", 0x011acd70baced759),
    ("ec.transition.cardwipe/3/8bpc/t1.5", 0x4f3e12557fa83c91),
    ("ec.transition.cardwipe/3/8bpc/t0.8", 0x31ba3f7b9e98633a),
    ("ec.transition.cardwipe/3/8bpc/t2.2", 0x040a6d3bd233ce47),
    ("ec.transition.cardwipe/3/32bpc/t0.5", 0x5f3be2dae40280cc),
    ("ec.transition.cardwipe/3/32bpc/t1.5", 0x3abe44793029ef0f),
    ("ec.transition.cardwipe/3/32bpc/t0.8", 0xa729a1df32f69328),
    ("ec.transition.cardwipe/3/32bpc/t2.2", 0xc6083cd48e2bcc52),
];
