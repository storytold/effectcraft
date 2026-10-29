//! GPU 3D Channel family vs the CPU effects (the oracle): direct on a buffer (full and half
//! resolution, padded offsets, aux channels at 1× and 2× the layer resolution) and composited
//! at 8 and 32 bpc from footage that carries auxiliary channels.

use std::sync::Arc;

use effectcraft_effects::{EffectEnv, EffectHost, LayerPixels};
use effectcraft_keyframe::Value;
use effectcraft_project::{BitDepth, Footage, ItemId};
use effectcraft_raster::channels3d::{BACKGROUND_DEPTH, crypto_float, crypto_hash};
use effectcraft_raster::{AuxChannels, Image};
use effectcraft_render::{Backend, FootageSource, RenderOpts, Renderer};
use effectcraft_time::Tick;

use crate::tests::{Pattern, Scene, c, check, diff, gpu, n, opts, pattern, set, tolerance};

pub(crate) struct AuxHost(pub(crate) Arc<AuxChannels>);

impl EffectHost for AuxHost {
    fn layer(&self, _: u64, _: bool) -> Option<LayerPixels> {
        None
    }
    fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
        None
    }
    fn aux(&self) -> Option<Arc<AuxChannels>> {
        Some(self.0.clone())
    }
}

const NAMES: [&str; 5] = ["alpha", "beta", "gamma", "delta", "epsilon"];

/// Aux channels for a `w`×`h` layer at `scale` aux pixels per layer pixel: a depth ramp with a
/// background corner, object / material ID blocks, coverage, UVs, normals, background RGB, raw
/// RGB and a two-rank Cryptomatte layer with a manifest.
pub(crate) fn aux_scene(w: u32, h: u32, scale: f64) -> AuxChannels {
    let (aw, ah) = ((w as f64 * scale) as u32, (h as f64 * scale) as u32);
    let mut a = AuxChannels::new(aw, ah, scale);
    let n = (aw * ah) as usize;
    let mut ch: Vec<(String, Vec<f32>)> = [
        "Z",
        "ObjectID",
        "MaterialID",
        "Coverage",
        "UV.U",
        "UV.V",
        "N.X",
        "N.Y",
        "N.Z",
        "BG.R",
        "BG.G",
        "BG.B",
        "R",
        "G",
        "B",
        "CryptoObject00.R",
        "CryptoObject00.G",
        "CryptoObject00.B",
        "CryptoObject00.A",
    ]
    .iter()
    .map(|s| (s.to_string(), vec![0.0; n]))
    .collect();
    let hashes: Vec<u32> = NAMES.iter().map(|s| crypto_hash(s)).collect();
    for y in 0..ah {
        for x in 0..aw {
            let i = (y * aw + x) as usize;
            let (u, v) = (x as f32 / aw as f32, y as f32 / ah as f32);
            let bg = u > 0.8 && v < 0.25;
            let obj = ((x as f64 / scale) as u32 / 11 + (y as f64 / scale) as u32 / 9 * 3) % 5;
            let vals = [
                if bg { BACKGROUND_DEPTH } else { 50.0 + 900.0 * u + 300.0 * (v * 7.0).sin() },
                if bg { 0.0 } else { obj as f32 + 1.0 },
                ((obj * 7) % 3) as f32,
                0.5 + 0.5 * (u * 9.0).sin(),
                u,
                v,
                (u - 0.5) * 1.6,
                (0.5 - v) * 1.4,
                0.7,
                0.2 + 0.6 * v,
                0.4,
                0.9 - 0.5 * u,
                u * 1.8,
                v * 0.7,
                0.3 + u * v * 2.5,
                crypto_float(hashes[obj as usize]),
                0.7,
                crypto_float(hashes[(obj as usize + 1) % 5]),
                0.3,
            ];
            for (k, val) in vals.iter().enumerate() {
                ch[k].1[i] = *val;
            }
        }
    }
    a.channels = ch;
    a.manifests = vec![("CryptoObject".into(), NAMES.iter().zip(&hashes).map(|(n, h)| (n.to_string(), *h)).collect())];
    a
}

/// The effect alone on a buffer with aux channels (`allow` = fraction of pixels that may
/// differ by more than 1e-3).
pub(crate) fn direct(id: &str, vals: &[(&str, Value)], allow: f64) {
    let Some(g) = gpu() else { return };
    let spec = effectcraft_effects::find(id).unwrap();
    let size = [70.0, 44.0];
    let mut params =
        effectcraft_effects::Params { values: spec.params.iter().map(|ps| (ps.id.to_string(), effectcraft_effects::default_value(ps, size))).collect() };
    for (k, v) in vals {
        params.values.insert(k.to_string(), v.clone());
    }
    for aux_scale in [1.0, 2.0] {
        let host = AuxHost(Arc::new(aux_scene(70, 44, aux_scale)));
        for (scale, offset) in [(1.0, [0.0, 0.0]), (0.5, [0.0, 0.0]), (1.0, [3.0, 2.0])] {
            let (w, h) = ((size[0] * scale) as u32, (size[1] * scale) as u32);
            let mut img = pattern(7, w + 2 * offset[0] as u32, h + 2 * offset[1] as u32);
            if offset[0] > 0.0 {
                // A padded buffer: transparent margins.
                for (i, px) in img.data.iter_mut().enumerate() {
                    let (x, y) = (i as u32 % img.width, i as u32 / img.width);
                    if x < offset[0] as u32 || y < offset[1] as u32 || x >= w + offset[0] as u32 || y >= h + offset[1] as u32 {
                        *px = [0.0; 4];
                    }
                }
            }
            let buf = effectcraft_effects::Buf { img, offset, scale };
            let ctx = || effectcraft_effects::EffectCtx {
                params: &params,
                time: 0.25,
                layer_size: size,
                seed: 11,
                adjustment: false,
                env: EffectEnv { host: Some(&host), ..Default::default() },
            };
            let cpu = (spec.render)(&ctx(), buf.clone());
            let out =
                effectcraft_render::Accelerator::effects(g, &[effectcraft_render::FxStep { spec, ctx: ctx() }], &buf, None).expect("the GPU runs the effect");
            assert_eq!((out.offset, out.scale), (cpu.offset, cpu.scale), "{id}: geometry");
            let d = diff(&cpu.img, &out.img, 1e-3);
            assert!(
                d.over as f64 <= allow * d.total as f64,
                "{id} {vals:?} aux {aux_scale} scale {scale} offset {offset:?}: {} of {} pixels over 1e-3 (max {}); worst {:?}",
                d.over,
                d.total,
                d.max,
                d.worst
            );
        }
    }
}

/// Footage frames from [`Pattern`] with [`aux_scene`] channels.
struct AuxSource;

impl FootageSource for AuxSource {
    fn frame(&self, item: ItemId, f: &Footage, t: Tick) -> Option<Arc<Image>> {
        Pattern.frame(item, f, t)
    }
    fn aux(&self, _item: ItemId, f: &Footage, _t: Tick) -> Option<Arc<AuxChannels>> {
        Some(Arc::new(aux_scene(f.width, f.height, 1.0)))
    }
}

fn compare(s: &Scene, o: RenderOpts) -> Option<crate::tests::Diff> {
    let g = gpu()?;
    let cpu = Renderer::new(&s.p, &AuxSource, RenderOpts { backend: Backend::Cpu, ..o }).comp_frame_cpu(s.cid, Tick::ZERO);
    let mut r = Renderer::new(&s.p, &AuxSource, RenderOpts { backend: Backend::Gpu, ..o });
    r.accel = Some(g);
    let img = g.render(&r, s.cid, Tick::ZERO).expect("the GPU renders the frame");
    let mut d = diff(&cpu, &img, tolerance(s.p.settings.bit_depth));
    d.quantized = s.p.settings.bit_depth != BitDepth::Bpc32;
    Some(d)
}

/// Direct, then on a rotated footage layer over a background at 8 and 32 bpc (full and half
/// resolution).
fn case(id: &str, vals: &[(&str, Value)]) {
    direct(id, vals, 0.0);
    composited(id, vals, 0.0);
}

fn composited(id: &str, vals: &[(&str, Value)], allow: f64) {
    for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
        let mut s = Scene::new(depth);
        let bg = s.solid([0.15, 0.1, 0.2], 97, 61);
        let bg_id = s.push(bg);
        let mut l = s.footage(70, 44);
        let mut vals = vals.to_vec();
        for v in vals.iter_mut() {
            if v.1 == Value::Layer(Some(0)) {
                v.1 = Value::Layer(Some(bg_id.0));
            }
        }
        s.effect(&mut l, id, &vals);
        set(&mut l, "transform/rotation", Value::Scalar(8.0));
        s.push(l);
        check(&format!("{id} {vals:?} {depth:?}"), compare(&s, opts()), allow);
        check(&format!("{id} {vals:?} {depth:?} half"), compare(&s, RenderOpts { scale: 0.5, ..opts() }), allow);
    }
}

fn e(v: u32) -> Value {
    Value::Enum(v)
}

fn on() -> Value {
    Value::Bool(true)
}

#[test]
fn channel_extract() {
    case("ec.3d.channelextract", &[("blackPoint", n(0.0)), ("whitePoint", n(1200.0))]);
    case("ec.3d.channelextract", &[("whitePoint", n(400.0)), ("clampOutput", Value::Bool(false)), ("invertDepthMap", on())]);
    case("ec.3d.channelextract", &[("blackPoint", n(900.0)), ("whitePoint", n(100.0))]);
    case("ec.3d.channelextract", &[("channel", e(1)), ("whitePoint", n(5.0))]);
    case("ec.3d.channelextract", &[("channel", e(2)), ("whitePoint", n(1.0))]);
    case("ec.3d.channelextract", &[("channel", e(3)), ("whitePoint", n(1.0))]);
    case("ec.3d.channelextract", &[("channel", e(4)), ("whitePoint", n(1.0)), ("antialias", on())]);
    case("ec.3d.channelextract", &[("channel", e(5)), ("whitePoint", n(1.0))]);
    case("ec.3d.channelextract", &[("channel", e(6))]);
    case("ec.3d.channelextract", &[("channel", e(7)), ("whitePoint", n(2.0))]);
    case("ec.3d.channelextract", &[("blackPoint", n(0.0)), ("whitePoint", n(1000.0)), ("antialias", on())]);
}

#[test]
fn depth_matte_fog_and_dof() {
    case("ec.3d.depthmatte", &[("depth", n(500.0))]);
    case("ec.3d.depthmatte", &[("depth", n(400.0)), ("feather", n(300.0)), ("invert", on())]);
    case("ec.3d.fog3d", &[]);
    case("ec.3d.fog3d", &[("fogColor", c(0.3, 0.5, 0.9)), ("fogStartDepth", n(200.0)), ("fogEndDepth", n(900.0)), ("scatteringDensity", n(10.0))]);
    case("ec.3d.fog3d", &[("foggyBackground", Value::Bool(false)), ("fogOpacity", n(70.0))]);
    composited("ec.3d.fog3d", &[("gradientLayer", Value::Layer(Some(0))), ("layerContribution", n(60.0))], 0.0);
    case("ec.3d.depthoffield", &[("focalPlane", n(400.0)), ("maximumRadius", n(8.0))]);
    case("ec.3d.depthoffield", &[("focalPlane", n(200.0)), ("maximumRadius", n(5.0)), ("focalPlaneThickness", n(150.0)), ("focalBias", n(80.0))]);
}

#[test]
fn ids_and_cryptomatte() {
    case("ec.3d.idmatte", &[("idSelection", n(2.0))]);
    case("ec.3d.idmatte", &[("idSelection", n(3.0)), ("feather", n(3.0)), ("useCoverage", on()), ("invert", on())]);
    case("ec.3d.idmatte", &[("auxChannel", e(1)), ("idSelection", n(1.0))]);
    case("ec.3d.identifier", &[]);
    case("ec.3d.identifier", &[("id", n(2.0))]);
    case("ec.3d.identifier", &[("display", e(1)), ("id", n(4.0))]);
    case("ec.3d.identifier", &[("display", e(2)), ("id", n(1.0))]);
    case("ec.3d.identifier", &[("display", e(3)), ("channelType", e(1))]);
    case("ec.3d.cryptomatte", &[("selection", Value::Str("beta, delta".into()))]);
    case("ec.3d.cryptomatte", &[("selection", Value::Str("*a".into())), ("display", e(0))]);
    case("ec.3d.cryptomatte", &[("selection", Value::Str("gamma".into())), ("display", e(1))]);
    case("ec.3d.cryptomatte", &[("selection", Value::Str("alpha".into())), ("matteOnly", on())]);
}

/// The GPU skips the ID colours for displays that do not read them
/// ([`effectcraft_effects::CryptoAux::for_display`]): every display × Matte Only must still match
/// the CPU effect, so a shader that starts reading RGB in another mode fails here.
#[test]
fn cryptomatte_every_display_matches_the_cpu() {
    for display in 0..3 {
        for matte_only in [false, true] {
            case("ec.3d.cryptomatte", &[("selection", Value::Str("beta, delta".into())), ("display", e(display)), ("matteOnly", Value::Bool(matte_only))]);
        }
    }
}

#[test]
fn extractor() {
    case("ec.3d.extractor", &[("red", Value::Str("Z".into())), ("green", Value::Str("UV.U".into())), ("whitePoint", n(1000.0))]);
    case(
        "ec.3d.extractor",
        &[("red", Value::Str("G".into())), ("blue", Value::Str("Coverage".into())), ("alpha", Value::Str("UV.V".into())), ("unMult", on())],
    );
    case("ec.3d.extractor", &[("red", Value::Str("A".into())), ("blackPoint", n(0.2)), ("whitePoint", n(0.8)), ("clip", on())]);
    case("ec.3d.extractor", &[("green", Value::Str("".into())), ("alpha", Value::Str("nothing".into()))]);
}
