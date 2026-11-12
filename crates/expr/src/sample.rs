//! `sampleImage`: read a layer's rendered pixels from an expression.
//!
//! The layer is rendered at full resolution through the renderer that is evaluating the
//! expression (its footage source; [`EvalCtx::footage`]): source → masks → effects for
//! `postEffect = true`, the bare source otherwise, in layer space (before the transform). Buffers
//! are cached (one cache for every thread, within a byte budget, least recently used first), so
//! many expressions sampling the same layer at the same time render it once. The source files a sampled render reads are kept with its buffer and
//! noted in the evaluating render's log ([`EvalCtx::reads`]) on every use, so a frame whose
//! expressions sample footage lists that footage too (#3184).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use effectcraft_project::{ItemId, Layer};
use effectcraft_render::reads::{ReadFiles, ReadLog};
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

type Entry = (Option<Sampled>, Arc<ReadFiles>);

/// The sampled buffers and the source files each was made of, shared by every render thread.
/// It was a per-thread map of up to 32 full float frames that was only emptied when full: with
/// one map per Rayon worker, frame lane, preview lane and export thread (about forty), scrubbing
/// a comp whose expressions sample a layer could hold tens of gigabytes. Now one map holds at
/// most [`MAX_BYTES`] of pixels and [`MAX_CACHE`] entries, the least recently used going first.
#[derive(Default)]
struct Cache {
    map: HashMap<Key, (Entry, usize, u64)>,
    bytes: usize,
    clock: u64,
}

impl Cache {
    fn get(&mut self, key: &Key) -> Option<Entry> {
        self.clock += 1;
        let now = self.clock;
        let (e, _, last) = self.map.get_mut(key)?;
        *last = now;
        Some(e.clone())
    }

    /// Keep `e` under `key`; then drop the least recently used entries (never this one) while
    /// over the byte or entry budget.
    fn insert(&mut self, key: Key, e: Entry, max_bytes: usize) {
        self.clock += 1;
        let bytes = e.0.as_ref().map_or(0, |s| s.img.data.len().saturating_mul(16));
        if let Some((_, old, _)) = self.map.insert(key, (e, bytes, self.clock)) {
            self.bytes = self.bytes.saturating_sub(old);
        }
        self.bytes = self.bytes.saturating_add(bytes);
        while self.bytes > max_bytes || self.map.len() > MAX_CACHE {
            let Some(k) = self.map.iter().filter(|(k, _)| **k != key).min_by_key(|(_, v)| v.2).map(|(k, _)| *k) else { break };
            if let Some((_, b, _)) = self.map.remove(&k) {
                self.bytes = self.bytes.saturating_sub(b);
            }
        }
    }
}

static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(Default::default);

thread_local! {
    /// Nested sampleImage renders (a sampled layer whose expressions sample again).
    static DEPTH: RefCell<usize> = const { RefCell::new(0) };
}

const MAX_CACHE: usize = 32;
/// Bytes of sampled pixels kept for every thread together (a 4K float frame is 133 MB).
const MAX_BYTES: usize = 512 << 20;
const MAX_DEPTH: usize = 2;

fn cache() -> std::sync::MutexGuard<'static, Cache> {
    CACHE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Forget every sampled buffer (Edit ▸ Purge, closing a project).
pub fn purge() {
    let mut c = cache();
    c.map.clear();
    c.bytes = 0;
}

/// A fingerprint of the layer's definition (so edits never reuse stale pixels).
fn fingerprint(layer: &Layer) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{layer:?}").hash(&mut h);
    h.finish()
}

/// Renders `layer` of `comp` at time `t` for an expression sample, before or after effects as `post` says.
/// Returns `None` once the nesting depth limit is reached.
fn render(ctx: EvalCtx, comp: ItemId, layer: &Layer, post: bool, t: f64) -> Option<Sampled> {
    let depth = DEPTH.with(|d| *d.borrow());
    if depth >= MAX_DEPTH {
        return None;
    }
    let key: Key = (ctx.project as *const _ as usize, comp.0, layer.id.0, t.to_bits(), post, fingerprint(layer));
    let note = |files: &ReadFiles| {
        if let Some(log) = ctx.reads {
            log.note_all(files);
        }
    };
    if let Some((hit, files)) = cache().get(&key) {
        note(&files);
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
    let log = ReadLog::new();
    let mut r = Renderer::new(ctx.project, footage, RenderOpts { motion_blur: false, ..Default::default() });
    r.expr = ctx.expr;
    r.reads = Some(&log);
    let ectx = r.eval_ctx(comp, Tick::from_seconds_f64(t))?;
    let out = if post {
        r.layer_content(&ectx, layer).map(|b| Sampled { img: Arc::new(b.img.clone()), offset: b.offset, scale: b.scale })
    } else {
        r.layer_source(&ectx, layer).map(|b| Sampled { img: Arc::new(b.img), offset: b.offset, scale: b.scale })
    };
    let files = Arc::new(log.into_files());
    note(&files);
    cache().insert(key, (out.clone(), files), MAX_BYTES);
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

#[cfg(test)]
mod cache_tests {
    use super::*;

    fn entry(w: u32, h: u32) -> Entry {
        let img = Arc::new(effectcraft_render::Image::new(w, h));
        (Some(Sampled { img, offset: [0.0; 2], scale: 1.0 }), Arc::default())
    }

    /// Fail-before: the per-thread map grew to 32 full frames on every thread. The shared cache
    /// stays within its byte budget, keeps the newest entry and drops the least recently used.
    #[test]
    fn sampled_buffers_stay_within_the_byte_budget() {
        let mut c = Cache::default();
        let key = |i: u64| (0usize, 1u64, 2u64, i, true, 3u64);
        // 64×64 px = 64 KiB each; a budget of four.
        let budget = 4 * 64 * 64 * 16;
        for i in 0..10 {
            c.insert(key(i), entry(64, 64), budget);
            if i == 2 {
                // Touch the oldest so it outlives newer ones.
                assert!(c.get(&key(0)).is_some());
            }
            assert!(c.bytes <= budget, "{} bytes after {i}", c.bytes);
        }
        assert!(c.get(&key(9)).is_some(), "the newest entry was dropped");
        assert!(c.get(&key(1)).is_none(), "a stale entry was kept");
        // One entry bigger than the budget is still kept (it is the newest).
        c.insert(key(99), entry(256, 256), budget);
        assert!(c.get(&key(99)).is_some());
        assert_eq!(c.map.len(), 1);
    }
}
