//! Warm-frame benchmark of #3333 (run by hand): three layers of the same 1080p footage frame,
//! 32 bpc, a 30-frame sequence already decoded (as the media pool serves a warm frame), with a
//! layer cache as the Render Queue has. Prints the median and mean ms per frame and a hash of
//! every frame (to compare the pixels of two builds).
//!
//! `cargo test -p effectcraft-gpu --release --lib bench_share -- --ignored --nocapture`

use std::collections::HashMap;
use std::sync::Mutex;

use super::*;

struct Decoded {
    frames: Mutex<HashMap<i64, Arc<Image>>>,
}

impl FootageSource for Decoded {
    fn frame(&self, item: ItemId, f: &Footage, t: Tick) -> Option<Arc<Image>> {
        let i = f.frame_rate.frame_at(t).clamp(0, 29);
        let mut m = self.frames.lock().unwrap();
        Some(m.entry(i).or_insert_with(|| Arc::new(pattern(item.0 as u32 + i as u32, f.width, f.height))).clone())
    }
}

#[test]
#[ignore]
fn bench_share() {
    let Some(g) = gpu() else { return };
    let (w, h) = (1920, 1080);
    let mut p = Project::default();
    p.settings.bit_depth = BitDepth::Bpc32;
    let comp = Comp::new(w, h, FrameRate::FPS_30, Tick::from_seconds_f64(1.0));
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    let f = Footage {
        path: "seq.exr".into(),
        kind: FootageKind::Sequence,
        width: w,
        height: h,
        pixel_aspect: 1.0,
        frame_rate: FrameRate::FPS_30,
        duration: Tick::from_seconds_f64(1.0),
        has_video: true,
        alpha: AlphaMode::Premultiplied,
        ..Default::default()
    };
    let fid = p.add_item("Sequence", Label::Lavender, None, ItemKind::Footage(f));
    for i in 0..3 {
        let mut l = build::layer(&mut p, &comp, "Sequence", LayerSource::Footage { item: fid }, (w, h), None);
        if i > 0 {
            l.blend_mode = BlendMode::Add;
            set(&mut l, "transform/opacity", Value::Scalar(25.0));
        }
        p.comp_mut(cid).unwrap().layers.insert(0, l);
    }
    let src = Decoded { frames: Mutex::new(HashMap::new()) };
    let frames = 30;
    let mut runs = vec![];
    let mut hashes = vec![];
    for pass in 0..4 {
        let cache = effectcraft_render::cache::LayerCache::new(1 << 30);
        let mut r = Renderer::new(&p, &src, RenderOpts { backend: Backend::Gpu, ..Default::default() });
        r.accel = Some(g);
        r.cache = Some(&cache);
        for k in 0..frames {
            let t0 = std::time::Instant::now();
            let img = g.render(&r, cid, Tick::from_seconds_f64(k as f64 / 30.0)).expect("the GPU renders the frame");
            let ms = t0.elapsed().as_secs_f64() * 1e3;
            if pass > 0 {
                runs.push(ms);
            }
            if pass == 1 {
                use std::hash::{Hash, Hasher};
                let mut hs = std::collections::hash_map::DefaultHasher::new();
                img.data.iter().for_each(|px| px.map(f32::to_bits).hash(&mut hs));
                hashes.push(hs.finish());
            }
        }
    }
    runs.sort_by(f64::total_cmp);
    let mean = runs.iter().sum::<f64>() / runs.len() as f64;
    eprintln!("bench_share: {} frames, median {:.1} ms, mean {:.1} ms, min {:.1} ms", runs.len(), runs[runs.len() / 2], mean, runs[0]);
    eprintln!("bench_share frame hashes: {hashes:016x?}");
}
