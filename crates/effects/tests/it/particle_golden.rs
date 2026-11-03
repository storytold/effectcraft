//! Golden hashes of the CPU particle effects (M13.29).
//!
//! The sibling of `sim_golden.rs` for the particle systems and the per-pixel "particle" effects
//! that the GPU compositor draws from a shared per-frame plan: CC Particle World, CC Particle
//! Systems II, Particle Playground (cannon, grid, text, layer exploder, layer map, property
//! mappers, repel), CC Ball Action, CC Pixel Polly, CC Scatterize and Curl Noise. Same set-up,
//! hashing and re-pinning (`SIM_GOLDEN_PRINT=1`, `SIM_GOLDEN_OUT=<dir>`) as `sim_golden.rs`: any
//! change to the CPU output of these effects, however small, fails the test.

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
    fn layer_at(&self, id: u64, comp_time: f64, _: bool) -> Option<LayerPixels> {
        // A frame that changes with time (Particle Playground's Layer Map times).
        let mut img = other(id);
        let k = ((comp_time * 30.0).round() as i64).rem_euclid(5) as f32 * 0.15;
        for p in img.data.iter_mut() {
            p[0] = (p[0] + k).min(1.0) * p[3];
        }
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

fn b(v: bool) -> Value {
    Value::Bool(v)
}
fn v2(x: f64, y: f64) -> Value {
    Value::Vec2([x, y])
}
fn s(v: &str) -> Value {
    Value::Str(v.into())
}

/// A Particle Playground grid (cannon off) plus `more`.
fn grid(more: Set) -> Set {
    let mut g: Set = vec![
        ("cannonEnabled", b(false)),
        ("gridEnabled", b(true)),
        ("grid/gridPosition", v2(32.0, 20.0)),
        ("grid/gridWidth", n(40.0)),
        ("grid/gridHeight", n(20.0)),
        ("grid/particlesAcross", n(5.0)),
        ("grid/particlesDown", n(3.0)),
        ("grid/gridParticleRadius", n(3.0)),
        ("gravity/gravityForce", n(20.0)),
    ];
    g.extend(more);
    g
}

/// (effect id, parameter sets) — the first set of each is the defaults.
fn cases() -> Vec<(&'static str, Vec<Set>)> {
    vec![
        (
            "ec.sim.ccparticleworld",
            vec![
                vec![],
                vec![("birthRate", n(2.0)), ("particle/particleType", e(1)), ("particle/transferMode", e(2)), ("physics/animation", e(1))],
                vec![("particle/particleType", e(4)), ("physics/animation", e(2)), ("particle/transferMode", e(1)), ("producer/radiusZ", n(0.3))],
                vec![("particle/particleType", e(8)), ("physics/animation", e(5)), ("particle/transferMode", e(3)), ("particle/birthSize", n(0.4))],
                vec![("particle/particleType", e(5)), ("physics/animation", e(8)), ("particle/sizeVariation", n(50.0)), ("physics/resistance", n(1.0))],
                vec![("particle/particleType", e(3)), ("physics/animation", e(6)), ("particle/maxOpacity", n(100.0))],
                vec![("particle/particleType", e(6)), ("physics/animation", e(4)), ("particle/deathSize", n(0.6))],
                vec![("particle/particleType", e(7)), ("physics/animation", e(3))],
                vec![("particle/particleType", e(2)), ("physics/animation", e(9))],
            ],
        ),
        (
            "ec.sim.ccparticlesystems2",
            vec![
                vec![],
                vec![("particle/particleType", e(1)), ("particle/transferMode", e(2)), ("physics/animation", e(2)), ("birthRate", n(4.0))],
                vec![("particle/particleType", e(5)), ("physics/animation", e(6)), ("particle/transferMode", e(1))],
                vec![("particle/particleType", e(7)), ("particle/transferMode", e(3)), ("physics/animation", e(9)), ("producer/radiusX", n(10.0))],
                vec![("particle/particleType", e(3)), ("physics/animation", e(4)), ("particle/sizeVariation", n(40.0))],
                vec![("particle/particleType", e(4)), ("physics/animation", e(5)), ("physics/direction", n(60.0))],
            ],
        ),
        (
            "ec.sim.particleplayground",
            vec![
                vec![],
                vec![
                    ("cannon/cannonPosition", v2(20.0, 40.0)),
                    ("cannon/particlesPerSecond", n(120.0)),
                    ("cannon/velocity", n(80.0)),
                    ("gravity/gravityForce", n(40.0)),
                ],
                grid(vec![]),
                grid(vec![("options/gridText", s("Ab C")), ("grid/gridParticleRadius", n(12.0)), ("options/autoOrientRotation", b(true))]),
                vec![
                    ("cannonEnabled", b(false)),
                    ("exploderEnabled", b(true)),
                    ("layerExploder/explodeLayer", l(TEX)),
                    ("layerExploder/radiusOfNewParticles", n(3.0)),
                ],
                grid(vec![
                    ("layerMapEnabled", b(true)),
                    ("layerMap/layerMapLayer", l(TEX)),
                    ("layerMap/timeOffsetType", e(2)),
                    ("layerMap/timeOffset", n(0.5)),
                ]),
                grid(vec![
                    ("mapperEnabled", b(true)),
                    ("persistentPropertyMapper/useLayerAsMap", l(MAP)),
                    ("persistentPropertyMapper/mapRedTo", e(8)),
                    ("persistentPropertyMapper/redMin", n(-50.0)),
                    ("persistentPropertyMapper/redMax", n(50.0)),
                ]),
                grid(vec![
                    ("ephemeralPropertyMapper/useLayerAsMap", l(MAP)),
                    ("ephemeralPropertyMapper/mapRedTo", e(1)),
                    ("ephemeralPropertyMapper/mapGreenTo", e(5)),
                    ("ephemeralPropertyMapper/greenOperator", e(1)),
                    ("ephemeralPropertyMapper/greenMax", n(4.0)),
                ]),
                grid(vec![("repel/repelForce", n(5.0)), ("repel/repelForceRadius", n(12.0)), ("options/cannonText", s("xy")), ("cannonEnabled", b(true))]),
            ],
        ),
        (
            "ec.sim.ccballaction",
            vec![
                vec![],
                vec![("scatter", n(20.0)), ("rotationAxis", e(6)), ("rotation", n(40.0)), ("twistProperty", e(3)), ("twistAngle", n(90.0))],
                vec![("gridSpacing", n(2.0)), ("ballSize", n(80.0)), ("instabilityState", n(45.0)), ("twistProperty", e(2)), ("twistAngle", n(30.0))],
            ],
        ),
        (
            "ec.sim.ccpixelpolly",
            vec![
                vec![],
                vec![("object", e(1)), ("gridSpacing", n(6.0))],
                vec![("object", e(2)), ("spinning", n(90.0))],
                vec![("object", e(3)), ("enableDepthSort", b(false)), ("gravity", n(2.0)), ("startTime", n(0.3))],
            ],
        ),
        (
            "ec.sim.ccscatterize",
            vec![vec![], vec![("scatter", n(10.0))], vec![("rightTwist", n(60.0)), ("leftTwist", n(-30.0)), ("transferMode", e(1)), ("scatter", n(4.0))]],
        ),
        (
            "ec.noise.curlnoise",
            vec![
                vec![],
                vec![("view", e(1))],
                vec![
                    ("edgeBehavior", e(1)),
                    ("steps", n(8.0)),
                    ("displacementAmount", n(60.0)),
                    ("scale", n(20.0)),
                    ("rotation", n(30.0)),
                    ("evolution", n(90.0)),
                ],
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
fn particle_effects_golden_hashes() {
    let got = hashes();
    if std::env::var_os("SIM_GOLDEN_PRINT").is_some() {
        for (name, h) in &got {
            println!("    (\"{name}\", 0x{h:016x}),");
        }
    }
    // Deterministic: a second pass (warm simulation caches) renders the same frames.
    assert_eq!(got, hashes(), "particle effects render deterministically");
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
    ("ec.sim.ccparticleworld/0/8bpc/t0.5", 0x4444ad969d8a228e),
    ("ec.sim.ccparticleworld/0/8bpc/t1.5", 0xcad2aad73a6d603e),
    ("ec.sim.ccparticleworld/0/8bpc/t0.8", 0x513b356b746dacd0),
    ("ec.sim.ccparticleworld/0/8bpc/t2.2", 0x6c1915f3f8282023),
    ("ec.sim.ccparticleworld/0/32bpc/t0.5", 0x41943ef09dec3b46),
    ("ec.sim.ccparticleworld/0/32bpc/t1.5", 0x9f90b72b4f674214),
    ("ec.sim.ccparticleworld/0/32bpc/t0.8", 0xecd3343277ff0ea8),
    ("ec.sim.ccparticleworld/0/32bpc/t2.2", 0x7aff8cf24f352f11),
    ("ec.sim.ccparticleworld/0/8bpc/half/t0.5", 0x80dc4ec079743731),
    ("ec.sim.ccparticleworld/0/8bpc/half/t1.5", 0x79b651a929d56eba),
    ("ec.sim.ccparticleworld/0/8bpc/half/t0.8", 0xc537e2eae2138c98),
    ("ec.sim.ccparticleworld/0/8bpc/half/t2.2", 0x6dec97915b8a85a0),
    ("ec.sim.ccparticleworld/1/8bpc/t0.5", 0xbe5fff3969b604b1),
    ("ec.sim.ccparticleworld/1/8bpc/t1.5", 0x0505795960f35d94),
    ("ec.sim.ccparticleworld/1/8bpc/t0.8", 0x7337a55ab762142d),
    ("ec.sim.ccparticleworld/1/8bpc/t2.2", 0x82e9d79c843b8f73),
    ("ec.sim.ccparticleworld/1/32bpc/t0.5", 0x85e6f2dfa2cc2342),
    ("ec.sim.ccparticleworld/1/32bpc/t1.5", 0x3fc16468de930ff4),
    ("ec.sim.ccparticleworld/1/32bpc/t0.8", 0x0c8eede3cae44849),
    ("ec.sim.ccparticleworld/1/32bpc/t2.2", 0xabbc670cfdf0bedf),
    ("ec.sim.ccparticleworld/2/8bpc/t0.5", 0x5dd6f8961dc8aa01),
    ("ec.sim.ccparticleworld/2/8bpc/t1.5", 0x6632451f98c2bb66),
    ("ec.sim.ccparticleworld/2/8bpc/t0.8", 0x9c9840445d995203),
    ("ec.sim.ccparticleworld/2/8bpc/t2.2", 0xaa29bed036be7d9d),
    ("ec.sim.ccparticleworld/2/32bpc/t0.5", 0x7507310c57e5bc42),
    ("ec.sim.ccparticleworld/2/32bpc/t1.5", 0x6f0216e5fe64f20c),
    ("ec.sim.ccparticleworld/2/32bpc/t0.8", 0x0def6a02489d3bc7),
    ("ec.sim.ccparticleworld/2/32bpc/t2.2", 0xb0d1af988e47a2d5),
    ("ec.sim.ccparticleworld/3/8bpc/t0.5", 0xbca7e3d574cf5e37),
    ("ec.sim.ccparticleworld/3/8bpc/t1.5", 0x5c4347a6c70b48ec),
    ("ec.sim.ccparticleworld/3/8bpc/t0.8", 0xc79131dbaa8ff129),
    ("ec.sim.ccparticleworld/3/8bpc/t2.2", 0xc08d0c5e8f87189d),
    ("ec.sim.ccparticleworld/3/32bpc/t0.5", 0x36635fe45324f86d),
    ("ec.sim.ccparticleworld/3/32bpc/t1.5", 0xf9751b9225a373a4),
    ("ec.sim.ccparticleworld/3/32bpc/t0.8", 0xb6026343f72dfaeb),
    ("ec.sim.ccparticleworld/3/32bpc/t2.2", 0x2f0e149de9623753),
    ("ec.sim.ccparticleworld/4/8bpc/t0.5", 0xdfd4cff821add47f),
    ("ec.sim.ccparticleworld/4/8bpc/t1.5", 0x4ed5d9c97c8b323e),
    ("ec.sim.ccparticleworld/4/8bpc/t0.8", 0x13c8b1929529a4b1),
    ("ec.sim.ccparticleworld/4/8bpc/t2.2", 0xd9702ad6bbd34752),
    ("ec.sim.ccparticleworld/4/32bpc/t0.5", 0xabb4834618b86cb8),
    ("ec.sim.ccparticleworld/4/32bpc/t1.5", 0x54de0787c35112ff),
    ("ec.sim.ccparticleworld/4/32bpc/t0.8", 0xfe9ed5ed6c1fd8bd),
    ("ec.sim.ccparticleworld/4/32bpc/t2.2", 0x8278e2b011770e33),
    ("ec.sim.ccparticleworld/5/8bpc/t0.5", 0x11abba82d18e4540),
    ("ec.sim.ccparticleworld/5/8bpc/t1.5", 0x1be834f3d8d64464),
    ("ec.sim.ccparticleworld/5/8bpc/t0.8", 0xf3ad76cb0f38394d),
    ("ec.sim.ccparticleworld/5/8bpc/t2.2", 0x64e10ae00b8e48e5),
    ("ec.sim.ccparticleworld/5/32bpc/t0.5", 0xc5d6ad16b9b6f662),
    ("ec.sim.ccparticleworld/5/32bpc/t1.5", 0xcb7b7777e8ba0ea1),
    ("ec.sim.ccparticleworld/5/32bpc/t0.8", 0x09692a677eed64fc),
    ("ec.sim.ccparticleworld/5/32bpc/t2.2", 0x63e921f1dd7b52bc),
    ("ec.sim.ccparticleworld/6/8bpc/t0.5", 0xa954ad832566044b),
    ("ec.sim.ccparticleworld/6/8bpc/t1.5", 0x27fd580241299755),
    ("ec.sim.ccparticleworld/6/8bpc/t0.8", 0x8749025b3c953f71),
    ("ec.sim.ccparticleworld/6/8bpc/t2.2", 0xd01aff9a0712e02d),
    ("ec.sim.ccparticleworld/6/32bpc/t0.5", 0xd2213ab758ed6cb9),
    ("ec.sim.ccparticleworld/6/32bpc/t1.5", 0xa7287da977d06904),
    ("ec.sim.ccparticleworld/6/32bpc/t0.8", 0xd02d00dc0e651852),
    ("ec.sim.ccparticleworld/6/32bpc/t2.2", 0xc8930de7171f5ab3),
    ("ec.sim.ccparticleworld/7/8bpc/t0.5", 0x9166865a7c9ac035),
    ("ec.sim.ccparticleworld/7/8bpc/t1.5", 0x93901b6547233c1f),
    ("ec.sim.ccparticleworld/7/8bpc/t0.8", 0xe5d835162c42b6a5),
    ("ec.sim.ccparticleworld/7/8bpc/t2.2", 0xe81b1c734e88c7cb),
    ("ec.sim.ccparticleworld/7/32bpc/t0.5", 0x27924fd1804d9a61),
    ("ec.sim.ccparticleworld/7/32bpc/t1.5", 0x9daca8989bbd99f1),
    ("ec.sim.ccparticleworld/7/32bpc/t0.8", 0xcd2932f1ee19e946),
    ("ec.sim.ccparticleworld/7/32bpc/t2.2", 0x886af8fedcd62969),
    ("ec.sim.ccparticleworld/8/8bpc/t0.5", 0xe08dd245cbd88371),
    ("ec.sim.ccparticleworld/8/8bpc/t1.5", 0xcbe7b22563e912f4),
    ("ec.sim.ccparticleworld/8/8bpc/t0.8", 0x81bcded7ad281386),
    ("ec.sim.ccparticleworld/8/8bpc/t2.2", 0x5d176ad19ac06da2),
    ("ec.sim.ccparticleworld/8/32bpc/t0.5", 0x1dff22388b30e96d),
    ("ec.sim.ccparticleworld/8/32bpc/t1.5", 0x6f29f64ba5e160df),
    ("ec.sim.ccparticleworld/8/32bpc/t0.8", 0xfba8c8d708e417e4),
    ("ec.sim.ccparticleworld/8/32bpc/t2.2", 0xf3b2f2a1ea8c71d6),
    ("ec.sim.ccparticlesystems2/0/8bpc/t0.5", 0x01d2a42a3a56c69b),
    ("ec.sim.ccparticlesystems2/0/8bpc/t1.5", 0x953091e15d03438f),
    ("ec.sim.ccparticlesystems2/0/8bpc/t0.8", 0x668ea154b17b2eb0),
    ("ec.sim.ccparticlesystems2/0/8bpc/t2.2", 0x1862446b0c54b86c),
    ("ec.sim.ccparticlesystems2/0/32bpc/t0.5", 0xa1fb78a64db56ef3),
    ("ec.sim.ccparticlesystems2/0/32bpc/t1.5", 0x20ec7913584e6b71),
    ("ec.sim.ccparticlesystems2/0/32bpc/t0.8", 0xab390d348d0a66fd),
    ("ec.sim.ccparticlesystems2/0/32bpc/t2.2", 0x82f78af53a7c0096),
    ("ec.sim.ccparticlesystems2/0/8bpc/half/t0.5", 0xf7cf34b83bd3af18),
    ("ec.sim.ccparticlesystems2/0/8bpc/half/t1.5", 0x461020f3a3a4410d),
    ("ec.sim.ccparticlesystems2/0/8bpc/half/t0.8", 0xcd190eb7b38db0e8),
    ("ec.sim.ccparticlesystems2/0/8bpc/half/t2.2", 0x7f35000229186023),
    ("ec.sim.ccparticlesystems2/1/8bpc/t0.5", 0xf711d4d5244de44b),
    ("ec.sim.ccparticlesystems2/1/8bpc/t1.5", 0x61ea1dd478d0f90e),
    ("ec.sim.ccparticlesystems2/1/8bpc/t0.8", 0x1d23c2c8bb26577c),
    ("ec.sim.ccparticlesystems2/1/8bpc/t2.2", 0x0ca22baad9654c06),
    ("ec.sim.ccparticlesystems2/1/32bpc/t0.5", 0x6dc7b6570e174ee2),
    ("ec.sim.ccparticlesystems2/1/32bpc/t1.5", 0x491a30b9f0dac574),
    ("ec.sim.ccparticlesystems2/1/32bpc/t0.8", 0xfd9c7f0d0abd959e),
    ("ec.sim.ccparticlesystems2/1/32bpc/t2.2", 0x95ea630041185e8b),
    ("ec.sim.ccparticlesystems2/2/8bpc/t0.5", 0x165e69bf2a5f7a6a),
    ("ec.sim.ccparticlesystems2/2/8bpc/t1.5", 0xc5ce39b6beb3a4a1),
    ("ec.sim.ccparticlesystems2/2/8bpc/t0.8", 0x635bf9e56db3087d),
    ("ec.sim.ccparticlesystems2/2/8bpc/t2.2", 0x03bb626e929e6cdd),
    ("ec.sim.ccparticlesystems2/2/32bpc/t0.5", 0x92b5a331852e40fe),
    ("ec.sim.ccparticlesystems2/2/32bpc/t1.5", 0xae453b6fd4bbaeed),
    ("ec.sim.ccparticlesystems2/2/32bpc/t0.8", 0xfa4621a685214620),
    ("ec.sim.ccparticlesystems2/2/32bpc/t2.2", 0x84c180368e4411ad),
    ("ec.sim.ccparticlesystems2/3/8bpc/t0.5", 0xf14aad048c3b4181),
    ("ec.sim.ccparticlesystems2/3/8bpc/t1.5", 0xc261983ac6805eda),
    ("ec.sim.ccparticlesystems2/3/8bpc/t0.8", 0xf93ad1d6a9c493a5),
    ("ec.sim.ccparticlesystems2/3/8bpc/t2.2", 0x88cf7156f9f8ec54),
    ("ec.sim.ccparticlesystems2/3/32bpc/t0.5", 0xdc1df37cb3b92146),
    ("ec.sim.ccparticlesystems2/3/32bpc/t1.5", 0x1c415c633baec738),
    ("ec.sim.ccparticlesystems2/3/32bpc/t0.8", 0x81a765f6e435e762),
    ("ec.sim.ccparticlesystems2/3/32bpc/t2.2", 0x7810e7f2ae55f619),
    ("ec.sim.ccparticlesystems2/4/8bpc/t0.5", 0x0106a8009fa18e2f),
    ("ec.sim.ccparticlesystems2/4/8bpc/t1.5", 0x5f12647281ea7ec9),
    ("ec.sim.ccparticlesystems2/4/8bpc/t0.8", 0x9f2655dbad770023),
    ("ec.sim.ccparticlesystems2/4/8bpc/t2.2", 0x44ca687ef817c0a1),
    ("ec.sim.ccparticlesystems2/4/32bpc/t0.5", 0x895b20656b6d3c98),
    ("ec.sim.ccparticlesystems2/4/32bpc/t1.5", 0x3a8b85da736ae6f0),
    ("ec.sim.ccparticlesystems2/4/32bpc/t0.8", 0x139f04a2dcf67d69),
    ("ec.sim.ccparticlesystems2/4/32bpc/t2.2", 0x02c844b7371128ac),
    ("ec.sim.ccparticlesystems2/5/8bpc/t0.5", 0x7b4a7112b6c8bb11),
    ("ec.sim.ccparticlesystems2/5/8bpc/t1.5", 0xf16ff18c451dbf63),
    ("ec.sim.ccparticlesystems2/5/8bpc/t0.8", 0xdedadbd43089dafe),
    ("ec.sim.ccparticlesystems2/5/8bpc/t2.2", 0x1e6fdb272dabf94c),
    ("ec.sim.ccparticlesystems2/5/32bpc/t0.5", 0xd17e5a60f476d0f7),
    ("ec.sim.ccparticlesystems2/5/32bpc/t1.5", 0xbfd43b69b45cd67c),
    ("ec.sim.ccparticlesystems2/5/32bpc/t0.8", 0x79fb89759eea8a6a),
    ("ec.sim.ccparticlesystems2/5/32bpc/t2.2", 0x0a70bc480c847d7e),
    ("ec.sim.particleplayground/0/8bpc/t0.5", 0x1842226399d43e19),
    ("ec.sim.particleplayground/0/8bpc/t1.5", 0xd5676389299c50e9),
    ("ec.sim.particleplayground/0/8bpc/t0.8", 0x30517e3b37e099d5),
    ("ec.sim.particleplayground/0/8bpc/t2.2", 0xae4bc0c296e03d75),
    ("ec.sim.particleplayground/0/32bpc/t0.5", 0x7af5da92e477f075),
    ("ec.sim.particleplayground/0/32bpc/t1.5", 0xb51f5a6d27dc1d85),
    ("ec.sim.particleplayground/0/32bpc/t0.8", 0x8da8efac055a67e5),
    ("ec.sim.particleplayground/0/32bpc/t2.2", 0x7ad7e7a36e107a6d),
    ("ec.sim.particleplayground/0/8bpc/half/t0.5", 0xe262aec0816c3ccd),
    ("ec.sim.particleplayground/0/8bpc/half/t1.5", 0x87e664a185ad2ca1),
    ("ec.sim.particleplayground/0/8bpc/half/t0.8", 0x34e36782ea9592fd),
    ("ec.sim.particleplayground/0/8bpc/half/t2.2", 0x0cf9289dd2950f89),
    ("ec.sim.particleplayground/1/8bpc/t0.5", 0x5dfbd4b5eb4382b9),
    ("ec.sim.particleplayground/1/8bpc/t1.5", 0xac3e8f2a9d4df855),
    ("ec.sim.particleplayground/1/8bpc/t0.8", 0x847292a51edbf41d),
    ("ec.sim.particleplayground/1/8bpc/t2.2", 0x58d9f45b4b984099),
    ("ec.sim.particleplayground/1/32bpc/t0.5", 0x1a3069f5d33c287d),
    ("ec.sim.particleplayground/1/32bpc/t1.5", 0x08e920bd85cd88ad),
    ("ec.sim.particleplayground/1/32bpc/t0.8", 0x692b0c490c09ac51),
    ("ec.sim.particleplayground/1/32bpc/t2.2", 0x21491c337ddd9f75),
    ("ec.sim.particleplayground/2/8bpc/t0.5", 0xb04e21f3c9ee8955),
    ("ec.sim.particleplayground/2/8bpc/t1.5", 0x3e9b871d87e66255),
    ("ec.sim.particleplayground/2/8bpc/t0.8", 0x9052ebc3a39db605),
    ("ec.sim.particleplayground/2/8bpc/t2.2", 0x4b2129a0221866d5),
    ("ec.sim.particleplayground/2/32bpc/t0.5", 0xb59ff67f1e4fdd55),
    ("ec.sim.particleplayground/2/32bpc/t1.5", 0x1ac6b864503401c5),
    ("ec.sim.particleplayground/2/32bpc/t0.8", 0x16a5d9d93e9c95c5),
    ("ec.sim.particleplayground/2/32bpc/t2.2", 0x4b2129a0221866d5),
    ("ec.sim.particleplayground/3/8bpc/t0.5", 0xbf818a3d83f45f7d),
    ("ec.sim.particleplayground/3/8bpc/t1.5", 0x02ee29d54f2f2ca5),
    ("ec.sim.particleplayground/3/8bpc/t0.8", 0x212121b5d3ff40f5),
    ("ec.sim.particleplayground/3/8bpc/t2.2", 0x4b2129a0221866d5),
    ("ec.sim.particleplayground/3/32bpc/t0.5", 0x94901b9480a11505),
    ("ec.sim.particleplayground/3/32bpc/t1.5", 0x18d82bebd984986d),
    ("ec.sim.particleplayground/3/32bpc/t0.8", 0x59b910231e182a8d),
    ("ec.sim.particleplayground/3/32bpc/t2.2", 0x4b2129a0221866d5),
    ("ec.sim.particleplayground/4/8bpc/t0.5", 0xb20c9c68c9e45a88),
    ("ec.sim.particleplayground/4/8bpc/t1.5", 0x4b2129a0221866d5),
    ("ec.sim.particleplayground/4/8bpc/t0.8", 0xdeb3ee1a1a0382d9),
    ("ec.sim.particleplayground/4/8bpc/t2.2", 0x4b2129a0221866d5),
    ("ec.sim.particleplayground/4/32bpc/t0.5", 0x7678784655ad24ef),
    ("ec.sim.particleplayground/4/32bpc/t1.5", 0x4b2129a0221866d5),
    ("ec.sim.particleplayground/4/32bpc/t0.8", 0x2fcf78fef85dc79b),
    ("ec.sim.particleplayground/4/32bpc/t2.2", 0x4b2129a0221866d5),
    ("ec.sim.particleplayground/5/8bpc/t0.5", 0x11c85638b23ca1ad),
    ("ec.sim.particleplayground/5/8bpc/t1.5", 0x0430aedb0bfe9b1d),
    ("ec.sim.particleplayground/5/8bpc/t0.8", 0x848b60fed97e4755),
    ("ec.sim.particleplayground/5/8bpc/t2.2", 0xe06dc84db33e7429),
    ("ec.sim.particleplayground/5/32bpc/t0.5", 0x0195318b4db583a1),
    ("ec.sim.particleplayground/5/32bpc/t1.5", 0x0f5e6c80a182d47d),
    ("ec.sim.particleplayground/5/32bpc/t0.8", 0xfe3ce304bf789bcd),
    ("ec.sim.particleplayground/5/32bpc/t2.2", 0x8dd7ed29d632643d),
    ("ec.sim.particleplayground/6/8bpc/t0.5", 0xb04e21f3c9ee8955),
    ("ec.sim.particleplayground/6/8bpc/t1.5", 0x3e9b871d87e66255),
    ("ec.sim.particleplayground/6/8bpc/t0.8", 0x9052ebc3a39db605),
    ("ec.sim.particleplayground/6/8bpc/t2.2", 0x4b2129a0221866d5),
    ("ec.sim.particleplayground/6/32bpc/t0.5", 0xb59ff67f1e4fdd55),
    ("ec.sim.particleplayground/6/32bpc/t1.5", 0x1ac6b864503401c5),
    ("ec.sim.particleplayground/6/32bpc/t0.8", 0x16a5d9d93e9c95c5),
    ("ec.sim.particleplayground/6/32bpc/t2.2", 0x4b2129a0221866d5),
    ("ec.sim.particleplayground/7/8bpc/t0.5", 0x5587b4a20830fbe1),
    ("ec.sim.particleplayground/7/8bpc/t1.5", 0xcdf7191b5cad91f5),
    ("ec.sim.particleplayground/7/8bpc/t0.8", 0x6e4571273b2d8959),
    ("ec.sim.particleplayground/7/8bpc/t2.2", 0x4b2129a0221866d5),
    ("ec.sim.particleplayground/7/32bpc/t0.5", 0x5080db75b8825039),
    ("ec.sim.particleplayground/7/32bpc/t1.5", 0x9070b3c2bfa25b29),
    ("ec.sim.particleplayground/7/32bpc/t0.8", 0xd406da6427e4b62d),
    ("ec.sim.particleplayground/7/32bpc/t2.2", 0x4b2129a0221866d5),
    ("ec.sim.particleplayground/8/8bpc/t0.5", 0x160e670965dffcd1),
    ("ec.sim.particleplayground/8/8bpc/t1.5", 0xd3a908531f629a05),
    ("ec.sim.particleplayground/8/8bpc/t0.8", 0x70d54b3670a26555),
    ("ec.sim.particleplayground/8/8bpc/t2.2", 0xbd22bfa694d189fd),
    ("ec.sim.particleplayground/8/32bpc/t0.5", 0x8630bbc5c1602bb9),
    ("ec.sim.particleplayground/8/32bpc/t1.5", 0xb5b7f0281f26baad),
    ("ec.sim.particleplayground/8/32bpc/t0.8", 0x8b6934363ef4dcfd),
    ("ec.sim.particleplayground/8/32bpc/t2.2", 0x0a31c17e8e34b9f5),
    ("ec.sim.ccballaction/0/8bpc/t0.5", 0x12698ec8353be38c),
    ("ec.sim.ccballaction/0/8bpc/t1.5", 0x12698ec8353be38c),
    ("ec.sim.ccballaction/0/8bpc/t0.8", 0x12698ec8353be38c),
    ("ec.sim.ccballaction/0/8bpc/t2.2", 0x12698ec8353be38c),
    ("ec.sim.ccballaction/0/32bpc/t0.5", 0x3fb733487e120261),
    ("ec.sim.ccballaction/0/32bpc/t1.5", 0x3fb733487e120261),
    ("ec.sim.ccballaction/0/32bpc/t0.8", 0x3fb733487e120261),
    ("ec.sim.ccballaction/0/32bpc/t2.2", 0x3fb733487e120261),
    ("ec.sim.ccballaction/0/8bpc/half/t0.5", 0x4946070917cc0945),
    ("ec.sim.ccballaction/0/8bpc/half/t1.5", 0x4946070917cc0945),
    ("ec.sim.ccballaction/0/8bpc/half/t0.8", 0x4946070917cc0945),
    ("ec.sim.ccballaction/0/8bpc/half/t2.2", 0x4946070917cc0945),
    ("ec.sim.ccballaction/1/8bpc/t0.5", 0xf34b3be38a4dd468),
    ("ec.sim.ccballaction/1/8bpc/t1.5", 0xf34b3be38a4dd468),
    ("ec.sim.ccballaction/1/8bpc/t0.8", 0xf34b3be38a4dd468),
    ("ec.sim.ccballaction/1/8bpc/t2.2", 0xf34b3be38a4dd468),
    ("ec.sim.ccballaction/1/32bpc/t0.5", 0x503b7238edf2912c),
    ("ec.sim.ccballaction/1/32bpc/t1.5", 0x503b7238edf2912c),
    ("ec.sim.ccballaction/1/32bpc/t0.8", 0x503b7238edf2912c),
    ("ec.sim.ccballaction/1/32bpc/t2.2", 0x503b7238edf2912c),
    ("ec.sim.ccballaction/2/8bpc/t0.5", 0x0b6bf26753ee4582),
    ("ec.sim.ccballaction/2/8bpc/t1.5", 0x0b6bf26753ee4582),
    ("ec.sim.ccballaction/2/8bpc/t0.8", 0x0b6bf26753ee4582),
    ("ec.sim.ccballaction/2/8bpc/t2.2", 0x0b6bf26753ee4582),
    ("ec.sim.ccballaction/2/32bpc/t0.5", 0x845e2c5c2719c336),
    ("ec.sim.ccballaction/2/32bpc/t1.5", 0x845e2c5c2719c336),
    ("ec.sim.ccballaction/2/32bpc/t0.8", 0x845e2c5c2719c336),
    ("ec.sim.ccballaction/2/32bpc/t2.2", 0x845e2c5c2719c336),
    ("ec.sim.ccpixelpolly/0/8bpc/t0.5", 0x793db4214db81d01),
    ("ec.sim.ccpixelpolly/0/8bpc/t1.5", 0xf69cf7f742757382),
    ("ec.sim.ccpixelpolly/0/8bpc/t0.8", 0xa81ebe4ddeab9b83),
    ("ec.sim.ccpixelpolly/0/8bpc/t2.2", 0x8d6af96aa12b70ac),
    ("ec.sim.ccpixelpolly/0/32bpc/t0.5", 0x3d03f23d9a915b5e),
    ("ec.sim.ccpixelpolly/0/32bpc/t1.5", 0x1c6a52023b0e21d0),
    ("ec.sim.ccpixelpolly/0/32bpc/t0.8", 0x6041c4d2944492e3),
    ("ec.sim.ccpixelpolly/0/32bpc/t2.2", 0x589763c3342ee81d),
    ("ec.sim.ccpixelpolly/0/8bpc/half/t0.5", 0x3d7bf2e96b64f5a5),
    ("ec.sim.ccpixelpolly/0/8bpc/half/t1.5", 0x24da849dfc0c1700),
    ("ec.sim.ccpixelpolly/0/8bpc/half/t0.8", 0x67992ff4eb32c1c5),
    ("ec.sim.ccpixelpolly/0/8bpc/half/t2.2", 0xdf434cacd5ad0471),
    ("ec.sim.ccpixelpolly/1/8bpc/t0.5", 0xf953bd43d910c59e),
    ("ec.sim.ccpixelpolly/1/8bpc/t1.5", 0x7fa196f56e470219),
    ("ec.sim.ccpixelpolly/1/8bpc/t0.8", 0x1f545b2172052197),
    ("ec.sim.ccpixelpolly/1/8bpc/t2.2", 0xd5eb9e120765f877),
    ("ec.sim.ccpixelpolly/1/32bpc/t0.5", 0xdb57e39c1f6df9f2),
    ("ec.sim.ccpixelpolly/1/32bpc/t1.5", 0x9710c951b521ccd2),
    ("ec.sim.ccpixelpolly/1/32bpc/t0.8", 0x1cc528553c49dfa9),
    ("ec.sim.ccpixelpolly/1/32bpc/t2.2", 0x8f73c69e50a6ce05),
    ("ec.sim.ccpixelpolly/2/8bpc/t0.5", 0x81720809859326c3),
    ("ec.sim.ccpixelpolly/2/8bpc/t1.5", 0x6bd1dee807745400),
    ("ec.sim.ccpixelpolly/2/8bpc/t0.8", 0x41d97e82c0554490),
    ("ec.sim.ccpixelpolly/2/8bpc/t2.2", 0x80def64ff166e725),
    ("ec.sim.ccpixelpolly/2/32bpc/t0.5", 0x10ab39b8f37c10dd),
    ("ec.sim.ccpixelpolly/2/32bpc/t1.5", 0xe66c3cc61794a13d),
    ("ec.sim.ccpixelpolly/2/32bpc/t0.8", 0x8a5fc3d42b56d274),
    ("ec.sim.ccpixelpolly/2/32bpc/t2.2", 0x9b493296a9819722),
    ("ec.sim.ccpixelpolly/3/8bpc/t0.5", 0x12b4b0832fc06783),
    ("ec.sim.ccpixelpolly/3/8bpc/t1.5", 0xe0095bedb7b3b17d),
    ("ec.sim.ccpixelpolly/3/8bpc/t0.8", 0x073b454632a54acc),
    ("ec.sim.ccpixelpolly/3/8bpc/t2.2", 0x5e0f61b3400d0b2e),
    ("ec.sim.ccpixelpolly/3/32bpc/t0.5", 0x014913b7c76771d3),
    ("ec.sim.ccpixelpolly/3/32bpc/t1.5", 0x7117b63447e076c3),
    ("ec.sim.ccpixelpolly/3/32bpc/t0.8", 0xb4c7fe2497dcf95c),
    ("ec.sim.ccpixelpolly/3/32bpc/t2.2", 0xd9fd060aaf88f744),
    ("ec.sim.ccscatterize/0/8bpc/t0.5", 0x8ad88caae775ea83),
    ("ec.sim.ccscatterize/0/8bpc/t1.5", 0x8ad88caae775ea83),
    ("ec.sim.ccscatterize/0/8bpc/t0.8", 0x8ad88caae775ea83),
    ("ec.sim.ccscatterize/0/8bpc/t2.2", 0x8ad88caae775ea83),
    ("ec.sim.ccscatterize/0/32bpc/t0.5", 0xe2b8332b96a1c76e),
    ("ec.sim.ccscatterize/0/32bpc/t1.5", 0xe2b8332b96a1c76e),
    ("ec.sim.ccscatterize/0/32bpc/t0.8", 0xe2b8332b96a1c76e),
    ("ec.sim.ccscatterize/0/32bpc/t2.2", 0xe2b8332b96a1c76e),
    ("ec.sim.ccscatterize/0/8bpc/half/t0.5", 0x730351fffb20e0c8),
    ("ec.sim.ccscatterize/0/8bpc/half/t1.5", 0x730351fffb20e0c8),
    ("ec.sim.ccscatterize/0/8bpc/half/t0.8", 0x730351fffb20e0c8),
    ("ec.sim.ccscatterize/0/8bpc/half/t2.2", 0x730351fffb20e0c8),
    ("ec.sim.ccscatterize/1/8bpc/t0.5", 0x675147db65b68b95),
    ("ec.sim.ccscatterize/1/8bpc/t1.5", 0x675147db65b68b95),
    ("ec.sim.ccscatterize/1/8bpc/t0.8", 0x675147db65b68b95),
    ("ec.sim.ccscatterize/1/8bpc/t2.2", 0x675147db65b68b95),
    ("ec.sim.ccscatterize/1/32bpc/t0.5", 0x6d822bb814b2dbe8),
    ("ec.sim.ccscatterize/1/32bpc/t1.5", 0x6d822bb814b2dbe8),
    ("ec.sim.ccscatterize/1/32bpc/t0.8", 0x6d822bb814b2dbe8),
    ("ec.sim.ccscatterize/1/32bpc/t2.2", 0x6d822bb814b2dbe8),
    ("ec.sim.ccscatterize/2/8bpc/t0.5", 0xd1f4144884af1673),
    ("ec.sim.ccscatterize/2/8bpc/t1.5", 0xd1f4144884af1673),
    ("ec.sim.ccscatterize/2/8bpc/t0.8", 0xd1f4144884af1673),
    ("ec.sim.ccscatterize/2/8bpc/t2.2", 0xd1f4144884af1673),
    ("ec.sim.ccscatterize/2/32bpc/t0.5", 0x95f58900dc6d6f58),
    ("ec.sim.ccscatterize/2/32bpc/t1.5", 0x95f58900dc6d6f58),
    ("ec.sim.ccscatterize/2/32bpc/t0.8", 0x95f58900dc6d6f58),
    ("ec.sim.ccscatterize/2/32bpc/t2.2", 0x95f58900dc6d6f58),
    ("ec.noise.curlnoise/0/8bpc/t0.5", 0xce5b0d9a6bbe529c),
    ("ec.noise.curlnoise/0/8bpc/t1.5", 0xce5b0d9a6bbe529c),
    ("ec.noise.curlnoise/0/8bpc/t0.8", 0xce5b0d9a6bbe529c),
    ("ec.noise.curlnoise/0/8bpc/t2.2", 0xce5b0d9a6bbe529c),
    ("ec.noise.curlnoise/0/32bpc/t0.5", 0x18de53fb08fbec6c),
    ("ec.noise.curlnoise/0/32bpc/t1.5", 0x18de53fb08fbec6c),
    ("ec.noise.curlnoise/0/32bpc/t0.8", 0x18de53fb08fbec6c),
    ("ec.noise.curlnoise/0/32bpc/t2.2", 0x18de53fb08fbec6c),
    ("ec.noise.curlnoise/0/8bpc/half/t0.5", 0x00124f8f5a0bb582),
    ("ec.noise.curlnoise/0/8bpc/half/t1.5", 0x00124f8f5a0bb582),
    ("ec.noise.curlnoise/0/8bpc/half/t0.8", 0x00124f8f5a0bb582),
    ("ec.noise.curlnoise/0/8bpc/half/t2.2", 0x00124f8f5a0bb582),
    ("ec.noise.curlnoise/1/8bpc/t0.5", 0x73e04e5ca0589de4),
    ("ec.noise.curlnoise/1/8bpc/t1.5", 0x73e04e5ca0589de4),
    ("ec.noise.curlnoise/1/8bpc/t0.8", 0x73e04e5ca0589de4),
    ("ec.noise.curlnoise/1/8bpc/t2.2", 0x73e04e5ca0589de4),
    ("ec.noise.curlnoise/1/32bpc/t0.5", 0x111d2c248e7b334b),
    ("ec.noise.curlnoise/1/32bpc/t1.5", 0x111d2c248e7b334b),
    ("ec.noise.curlnoise/1/32bpc/t0.8", 0x111d2c248e7b334b),
    ("ec.noise.curlnoise/1/32bpc/t2.2", 0x111d2c248e7b334b),
    ("ec.noise.curlnoise/2/8bpc/t0.5", 0xe62ab777e41bf94e),
    ("ec.noise.curlnoise/2/8bpc/t1.5", 0xe62ab777e41bf94e),
    ("ec.noise.curlnoise/2/8bpc/t0.8", 0xe62ab777e41bf94e),
    ("ec.noise.curlnoise/2/8bpc/t2.2", 0xe62ab777e41bf94e),
    ("ec.noise.curlnoise/2/32bpc/t0.5", 0xd477a194b7a2d123),
    ("ec.noise.curlnoise/2/32bpc/t1.5", 0xd477a194b7a2d123),
    ("ec.noise.curlnoise/2/32bpc/t0.8", 0xd477a194b7a2d123),
    ("ec.noise.curlnoise/2/32bpc/t2.2", 0xd477a194b7a2d123),
];
