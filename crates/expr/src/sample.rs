//! `sampleImage`: read a layer's rendered pixels from an expression.
//!
//! The layer is rendered at full resolution through the renderer that is evaluating the
//! expression (its footage source; [`EvalCtx::footage`]): source → masks → effects for
//! `postEffect = true`, the bare source otherwise, in layer space (before the transform). Buffers
//! are cached per thread for the frame, so many expressions sampling the same layer at the same
//! time render it once.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use effectcraft_project::{ItemId, Layer};
use effectcraft_render::{EvalCtx, NoFootage, RenderOpts, Renderer};
use effectcraft_time::Tick;

/// A rendered layer buffer: straight-from-the-renderer premultiplied pixels with the layer
/// space → buffer pixel mapping.
#[derive(Clone)]
struct Sampled {
    img: Arc<effectcraft_render::Image>,
    offset: [f64; 2],
    scale: f64,
}

type Key = (usize, u64, u64, u64, bool, u64);

thread_local! {
    static CACHE: RefCell<HashMap<Key, Option<Sampled>>> = RefCell::new(HashMap::new());
    /// Nested sampleImage renders (a sampled layer whose expressions sample again).
    static DEPTH: RefCell<usize> = const { RefCell::new(0) };
}

const MAX_CACHE: usize = 32;
const MAX_DEPTH: usize = 2;

/// A fingerprint of the layer's definition (so edits never reuse stale pixels).
fn fingerprint(layer: &Layer) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{layer:?}").hash(&mut h);
    h.finish()
}

fn render(ctx: EvalCtx, comp: ItemId, layer: &Layer, post: bool, t: f64) -> Option<Sampled> {
    let depth = DEPTH.with(|d| *d.borrow());
    if depth >= MAX_DEPTH {
        return None;
    }
    let key: Key = (ctx.project as *const _ as usize, comp.0, layer.id.0, t.to_bits(), post, fingerprint(layer));
    if let Some(hit) = CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return hit;
    }
    DEPTH.with(|d| *d.borrow_mut() += 1);
    struct Pop;
    impl Drop for Pop {
        fn drop(&mut self) {
            DEPTH.with(|d| *d.borrow_mut() -= 1);
        }
    }
    let _pop = Pop;
    let footage: &dyn effectcraft_render::FootageSource = ctx.footage.unwrap_or(&NoFootage);
    let mut r = Renderer::new(ctx.project, footage, RenderOpts { motion_blur: false, ..Default::default() });
    r.expr = ctx.expr;
    let ectx = r.eval_ctx(comp, Tick::from_seconds_f64(t))?;
    let out = if post {
        r.content_buf(&ectx, layer).map(|b| Sampled { img: Arc::new(b.img.clone()), offset: b.offset, scale: b.scale })
    } else {
        r.layer_source(&ectx, layer).map(|b| Sampled { img: Arc::new(b.img), offset: b.offset, scale: b.scale })
    };
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() >= MAX_CACHE {
            c.clear();
        }
        c.insert(key, out.clone());
    });
    out
}

/// The average colour (straight RGBA, 0..1) of `layer`'s pixels in the box `point ± radius`
/// (layer pixels) at comp time `t`. Transparent outside the layer.
pub fn sample_image(ctx: EvalCtx, comp: ItemId, layer: &Layer, point: [f64; 2], radius: [f64; 2], post: bool, t: f64) -> Option<[f32; 4]> {
    if !layer.source.is_av() {
        return Some([0.0; 4]);
    }
    let Some(b) = render(ctx, comp, layer, post, t) else { return Some([0.0; 4]) };
    let to_px = |x: f64, y: f64| (x * b.scale + b.offset[0], y * b.scale + b.offset[1]);
    let (rx, ry) = (radius[0].abs() * b.scale, radius[1].abs() * b.scale);
    let (cx, cy) = to_px(point[0], point[1]);
    // Pixels whose centres (at +0.5) fall in the box; at least the one under the point.
    let (x0, x1, y0, y1) = if rx < 0.5 && ry < 0.5 {
        let (px, py) = (cx.floor() as i64, cy.floor() as i64);
        (px, px, py, py)
    } else {
        let lo = |c: f64, r: f64| (c - r - 0.5).ceil() as i64;
        let hi = |c: f64, r: f64| (c + r - 0.5).floor() as i64;
        (lo(cx, rx), hi(cx, rx).max(lo(cx, rx)), lo(cy, ry), hi(cy, ry).max(lo(cy, ry)))
    };
    let mut acc = [0.0f64; 4];
    let mut n = 0.0;
    for y in y0..=y1.min(y0.saturating_add(4096)) {
        for x in x0..=x1.min(x0.saturating_add(4096)) {
            let p = b.img.get(x, y);
            for k in 0..4 {
                acc[k] += p[k] as f64;
            }
            n += 1.0;
        }
    }
    if n == 0.0 {
        return Some([0.0; 4]);
    }
    let a = acc[3] / n;
    if a <= 1e-9 {
        return Some([0.0; 4]);
    }
    Some([(acc[0] / n / a) as f32, (acc[1] / n / a) as f32, (acc[2] / n / a) as f32, a as f32])
}
