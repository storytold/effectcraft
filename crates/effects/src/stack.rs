//! Per-thread state of the effect stack that is running: how the stack's buffer sits relative to
//! the buffer a blanket cover of the layer rectangle would give it, so CPU blurs round exactly as
//! they did on that buffer (#1205).

use std::cell::Cell;

thread_local! {
    /// Rows a blanket cover's buffer would have above the current buffer (0 with the cover).
    /// CPU blurs place their vertical block restarts with it, so they round exactly as on the
    /// taller buffer ([`StackScope`]).
    static ROW_PHASE: Cell<i64> = const { Cell::new(0) };
    /// The covered buffer in the current buffer's pixels (x, y, w, h), tracked through the
    /// stack's pads; `None` when the buffer was covered or not aligned. A Repeat Edge blur grows
    /// to it so it repeats the same edge pixels as the covered buffer ([`main_rect`]).
    static MAIN_RECT: Cell<Option<[i64; 4]>> = const { Cell::new(None) };
    /// Another effect runs after the current one in this stack ([`followed`]).
    static FOLLOWED: Cell<bool> = const { Cell::new(false) };
}

/// Whether another effect runs after the current one in this stack: the Repeat Edge fast path
/// ([`crate::Buf::repeat_as_transparent`]) is exact only as the stack's last effect.
pub fn followed() -> bool {
    FOLLOWED.with(|c| c.get())
}

/// Set [`followed`] (the renderer, before each CPU effect).
pub fn set_followed(v: bool) {
    FOLLOWED.with(|c| c.set(v));
}

/// The buffer changed outside this tracking (an accelerator chain ran on it): forget the
/// covered buffer's placement, so later CPU effects behave as without it (row phase 0, Repeat
/// Edge covers the layer).
pub fn forget() {
    ROW_PHASE.with(|c| c.set(0));
    MAIN_RECT.with(|c| c.set(None));
}

/// See [`MAIN_RECT`].
pub fn main_rect() -> Option<[i64; 4]> {
    MAIN_RECT.with(|c| c.get())
}

/// Set [`MAIN_RECT`].
pub fn set_main_rect(r: Option<[i64; 4]>) {
    MAIN_RECT.with(|c| c.set(r));
}

/// Both buffers grew by `pad` on every side ([`crate::Buf::pad`]).
pub(crate) fn main_rect_padded(pad: u32) {
    MAIN_RECT.with(|c| c.set(c.get().map(|[x, y, w, h]| [x, y, w + 2 * pad as i64, h + 2 * pad as i64])));
}

/// Only this buffer grew by `pad` on every side ([`crate::cover_pad`]; the covered one had the room).
pub fn main_rect_cover_pad(pad: u32) {
    let p = pad as i64;
    MAIN_RECT.with(|c| c.set(c.get().map(|[x, y, w, h]| [x + p, y + p, w - 2 * p, h - 2 * p])));
}

/// See [`ROW_PHASE`].
pub fn row_phase() -> i64 {
    ROW_PHASE.with(|c| c.get())
}

/// Set [`ROW_PHASE`] ([`crate::Buf::align`]).
pub fn set_row_phase(v: i64) {
    ROW_PHASE.with(|c| c.set(v));
}

/// The buffer grew `rows` on top that the covered buffer did not.
pub fn shift_row_phase(rows: i64) {
    ROW_PHASE.with(|c| c.set(c.get() - rows));
}

/// One layer's effect stack: the state starts empty and is restored afterwards (a nested layer's
/// stack can run inside an effect).
pub struct StackScope(i64, Option<[i64; 4]>, bool);

impl StackScope {
    pub fn new() -> StackScope {
        StackScope(ROW_PHASE.with(|c| c.replace(0)), MAIN_RECT.with(|c| c.replace(None)), FOLLOWED.with(|c| c.replace(false)))
    }
}

impl Default for StackScope {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for StackScope {
    fn drop(&mut self) {
        ROW_PHASE.with(|c| c.set(self.0));
        MAIN_RECT.with(|c| c.set(self.1));
        FOLLOWED.with(|c| c.set(self.2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Buf, EffectCtx, Params, Value, apply, default_value, find, num};
    use effectcraft_raster::Image;

    const LAYER: [f64; 2] = [300.0, 260.0];
    type Fx = (&'static str, Vec<(&'static str, Value)>);

    fn gauss(s: f64, repeat: bool) -> Fx {
        ("ec.blur.gaussian", vec![("blurriness", num(s)), ("repeatEdge", Value::Bool(repeat))])
    }
    fn fast_box(repeat: bool) -> Fx {
        ("ec.blur.fastbox", vec![("radius", num(7.0)), ("iterations", num(3.0)), ("repeatEdge", Value::Bool(repeat))])
    }
    fn displace() -> Fx {
        ("ec.distort.turbulentdisplace", vec![("amount", num(30.0))])
    }
    fn glow() -> Fx {
        ("ec.stylize.glow", vec![])
    }

    fn run(fx: &Fx, b: Buf) -> Buf {
        let s = find(fx.0).unwrap();
        let mut params = Params { values: s.params.iter().map(|p| (p.id.to_string(), default_value(p, LAYER))).collect() };
        for (k, v) in &fx.1 {
            params.values.insert(k.to_string(), v.clone());
        }
        let ctx = EffectCtx { params: &params, time: 0.0, layer_size: LAYER, seed: 3, adjustment: false, env: Default::default() };
        apply(s, &ctx, b)
    }

    /// `w`×`h` opaque pattern placed at layer (`at`).
    fn content(w: u32, h: u32, at: [f64; 2]) -> Buf {
        let mut img = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = ((x * 7 + y * 13) % 17) as f32 / 17.0;
                img.set(x, y, [v * 0.3, v * 0.7, v, 1.0]);
            }
        }
        Buf { img, offset: [-at[0], -at[1]], scale: 1.0 }
    }

    /// The renderer's stack: aligned or covered, `followed` set before each effect.
    fn stack(aligned: bool, b: &Buf, fx: &[Fx]) -> Buf {
        let _s = StackScope::new();
        let mut b = b.clone();
        if aligned {
            b.align(LAYER);
        } else {
            b.cover(LAYER);
        }
        for (i, f) in fx.iter().enumerate() {
            set_followed(i + 1 < fx.len());
            b = run(f, b);
        }
        b
    }

    /// Pixels of `a` inside the layer rectangle that differ (bits) from `c` at the same place.
    fn differing(a: &Buf, c: &Buf) -> usize {
        let mut n = 0;
        for y in 0..a.img.height as i64 {
            for x in 0..a.img.width as i64 {
                let (lx, ly) = (x - a.offset[0] as i64, y - a.offset[1] as i64);
                if lx < 0 || ly < 0 || lx >= LAYER[0] as i64 || ly >= LAYER[1] as i64 {
                    continue;
                }
                let (cx, cy) = (lx + c.offset[0] as i64, ly + c.offset[1] as i64);
                let q = if cx < c.img.width as i64 && cy < c.img.height as i64 { c.img.data[(cy * c.img.width as i64 + cx) as usize] } else { [0.0; 4] };
                n += usize::from(a.img.data[(y * a.img.width as i64 + x) as usize].map(f32::to_bits) != q.map(f32::to_bits));
            }
        }
        n
    }

    /// Repeat Edge stacks on an aligned buffer give the covered buffer's pixels bit for bit, with
    /// content near the layer's edges (within the blur's reach: the edge holds blurred content
    /// after the first pass) and with more effects after the blur.
    #[test]
    fn repeat_edge_on_an_aligned_buffer_matches_the_cover() {
        let stacks: Vec<Vec<Fx>> = vec![
            vec![gauss(10.0, true)],
            vec![fast_box(true)],
            vec![gauss(8.0, true), gauss(10.0, true)],
            vec![gauss(10.0, true), displace()],
            vec![gauss(10.0, true), glow()],
            vec![fast_box(true), gauss(6.0, true)],
        ];
        for (w, h, at) in [(60, 30, [3.0, 5.0]), (80, 60, [200.0, 190.0]), (50, 40, [70.0, 90.0]), (50, 40, [250.0, 220.0])] {
            let src = content(w, h, at);
            for fx in &stacks {
                let ids: Vec<_> = fx.iter().map(|f| f.0).collect();
                let (a, c) = (stack(true, &src, fx), stack(false, &src, fx));
                assert_eq!(differing(&a, &c), 0, "{w}x{h} at {at:?}: {ids:?}");
            }
        }
    }

    /// The fast path still runs where it is exact: content far from the edges, blur last.
    #[test]
    fn repeat_edge_fast_path_keeps_a_small_buffer() {
        let src = content(50, 40, [70.0, 90.0]);
        for fx in [gauss(10.0, true), fast_box(true)] {
            let a = stack(true, &src, std::slice::from_ref(&fx));
            assert!(a.img.width < 200 && a.img.height < 200, "{}: {}x{}", fx.0, a.img.width, a.img.height);
            assert_eq!(differing(&a, &stack(false, &src, &[fx])), 0);
        }
        // Not last: grows to the covered buffer (Glow then pads it).
        let a = stack(true, &src, &[gauss(10.0, true), glow()]);
        assert!(a.img.width >= 300 && a.img.height >= 260);
    }

    /// An accelerator chain grows the buffer without the tracking ([`forget`]): a later CPU
    /// Repeat Edge blur then covers the layer and repeats the layer's edge, as on the cover
    /// (which the same chain padded too).
    #[test]
    fn forgotten_tracking_covers_for_repeat_edge() {
        let src = content(50, 40, [250.0, 220.0]);
        let gpu_pad = |b: &mut Buf| {
            b.img = b.img.padded(9);
            b.offset = [b.offset[0] + 9.0, b.offset[1] + 9.0];
        };
        let covered = {
            let _s = StackScope::new();
            let mut b = src.clone();
            b.cover(LAYER);
            gpu_pad(&mut b);
            run(&gauss(10.0, true), b)
        };
        let aligned = || {
            let _s = StackScope::new();
            let mut b = src.clone();
            b.align(LAYER);
            gpu_pad(&mut b);
            forget();
            run(&gauss(10.0, true), b)
        };
        let max_diff = |a: &Buf| {
            let mut m = 0f32;
            for ly in 0..LAYER[1] as i64 {
                for lx in 0..LAYER[0] as i64 {
                    let at = |b: &Buf| {
                        let (x, y) = (lx + b.offset[0] as i64, ly + b.offset[1] as i64);
                        if x < 0 || y < 0 || x >= b.img.width as i64 || y >= b.img.height as i64 {
                            [0.0; 4]
                        } else {
                            b.img.data[(y * b.img.width as i64 + x) as usize]
                        }
                    };
                    let (p, q) = (at(a), at(&covered));
                    m = (0..4).fold(m, |m, i| m.max((p[i] - q[i]).abs()));
                }
            }
            m
        };
        assert!(max_diff(&aligned()) < 1e-5);
    }
}
