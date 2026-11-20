//! #3334: what Cryptomatte costs on the GPU at 1920 × 1080 with three ranks (six ID and coverage
//! planes), at full and half resolution: the first frame and the same frame again. Ignored; run
//! with `cargo test -p effectcraft-gpu --release crypto_bench -- --ignored --nocapture`.

use std::sync::Arc;

use effectcraft_effects::EffectEnv;
use effectcraft_keyframe::Value;
use effectcraft_raster::channels3d::{crypto_float, crypto_hash};
use effectcraft_raster::{AuxChannels, Image};

use crate::tests::fx_depth::AuxHost;
use crate::tests::gpu;

const NAMES: [&str; 6] = ["floor", "wall", "chair", "table", "lamp", "cup"];

/// 1920 × 1080, three ranks: diagonal bands of objects whose edges share pixels.
fn aux_1080p() -> AuxChannels {
    let (w, h) = (1920usize, 1080usize);
    let ids: Vec<f32> = NAMES.iter().map(|n| crypto_float(crypto_hash(n))).collect();
    let mut a = AuxChannels::new(w as u32, h as u32, 1.0);
    let mut planes = vec![vec![0.0f32; w * h]; 6];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let band = (x + y / 2) / 97;
            let edge = (x + y / 2) % 97 < 3;
            let c = if edge { 0.6 } else { 1.0 };
            planes[0][i] = ids[band % 6];
            planes[1][i] = c;
            planes[2][i] = ids[(band + 1) % 6];
            planes[3][i] = 1.0 - c;
            planes[4][i] = ids[(band + 2) % 6];
            planes[5][i] = 0.0;
        }
    }
    let names = ["00.R", "00.G", "00.B", "00.A", "01.R", "01.G"];
    a.channels = names.iter().zip(planes).map(|(n, p)| (format!("CryptoObject{n}"), p)).collect();
    a.manifests = vec![("CryptoObject".into(), NAMES.iter().map(|n| (n.to_string(), crypto_hash(n))).collect())];
    a
}

#[test]
#[ignore]
fn crypto_bench() {
    let Some(g) = gpu() else { return };
    let host = AuxHost(Arc::new(aux_1080p()));
    let spec = effectcraft_effects::find("ec.3d.cryptomatte").unwrap();
    let size = [1920.0, 1080.0];
    for (label, display, matte_only) in [("Matte", 1u32, false), ("Matte Only", 1, true), ("Colors", 0, false)] {
        let mut params =
            effectcraft_effects::Params { values: spec.params.iter().map(|ps| (ps.id.to_string(), effectcraft_effects::default_value(ps, size))).collect() };
        params.values.insert("selection".into(), Value::Str("chair, lamp".into()));
        params.values.insert("display".into(), Value::Enum(display));
        params.values.insert("matteOnly".into(), Value::Bool(matte_only));
        for scale in [1.0, 0.5] {
            let (w, h) = ((size[0] * scale) as u32, (size[1] * scale) as u32);
            let buf = effectcraft_effects::Buf { img: Image::filled(w, h, [0.5, 0.4, 0.3, 1.0]), offset: [0.0; 2], scale };
            let ctx = || effectcraft_effects::EffectCtx {
                params: &params,
                time: 0.0,
                layer_size: size,
                seed: 0,
                adjustment: false,
                env: EffectEnv { host: Some(&host), ..Default::default() },
            };
            // The effect alone on a buffer already on the GPU, until its result is done (a 1 × 1
            // readback waits for it): no upload of the layer, no readback of the frame.
            let mut e = crate::context::Enc::new(&g.ctx);
            let input = g.ctx.upload_image(&buf.img).unwrap();
            let mut run = || {
                let t = std::time::Instant::now();
                let gbuf = crate::effects::GBuf { img: input.clone(), offset: buf.offset, scale };
                let out = crate::fx_depth::apply(&mut e, "ec.3d.cryptomatte", &ctx(), gbuf).expect("the GPU runs the effect");
                let one = e.crop(&out.img, 0, 0, 1, 1);
                e.download(&one).expect("a readback");
                t.elapsed().as_secs_f64() * 1000.0
            };
            let first = run();
            let mut warm: Vec<f64> = (0..20).map(|_| run()).collect();
            warm.sort_by(f64::total_cmp);
            eprintln!("crypto_bench {label} scale {scale}: first frame {first:.2} ms, same frame again {:.2} ms (median of 20)", warm[10]);
        }
    }
}
