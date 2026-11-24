//! Onion skinning for the Composition viewer: which neighbouring frames to ghost, how strongly,
//! and how a ghost is tinted and laid over (or under) the frame on screen.

use serde::{Deserialize, Serialize};

pub const MAX_GHOSTS_PER_SIDE: u32 = 8;
pub const MAX_STEP: u32 = 120;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OnionSkin {
    pub enabled: bool,
    pub before: u32,
    pub after: u32,
    pub step: u32,
    pub tint_before: [u8; 3],
    pub tint_after: [u8; 3],
    pub tint_amount: f32,
    pub opacity: f32,
    pub falloff: f32,
    pub under: bool,
}

impl Default for OnionSkin {
    fn default() -> Self {
        OnionSkin {
            enabled: false,
            before: 2,
            after: 2,
            step: 1,
            tint_before: [0xff, 0x40, 0x40],
            tint_after: [0x40, 0xc0, 0x60],
            tint_amount: 0.8,
            opacity: 0.5,
            falloff: 0.5,
            under: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ghost {
    pub frame: i64,
    pub before: bool,
    pub alpha: f32,
    pub tint: [u8; 3],
}

fn unit(v: f32, fallback: f32) -> f32 {
    if v.is_finite() { v.clamp(0.0, 1.0) } else { fallback }
}

impl OnionSkin {
    pub fn clamped(&self) -> OnionSkin {
        let d = OnionSkin::default();
        OnionSkin {
            before: self.before.min(MAX_GHOSTS_PER_SIDE),
            after: self.after.min(MAX_GHOSTS_PER_SIDE),
            step: self.step.clamp(1, MAX_STEP),
            tint_amount: unit(self.tint_amount, d.tint_amount),
            opacity: unit(self.opacity, d.opacity),
            falloff: unit(self.falloff, d.falloff),
            ..self.clone()
        }
    }

    pub fn max_ghosts(&self) -> usize {
        let c = self.clamped();
        (c.before + c.after) as usize
    }

    /// The ghosts around frame `cur` inside `first..=last`, the nearest first. The k-th ghost of
    /// a side has opacity `opacity * (1 - falloff)^(k - 1)`.
    pub fn ghosts(&self, cur: i64, first: i64, last: i64) -> Vec<Ghost> {
        let c = self.clamped();
        if !c.enabled {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (n, before) in [(c.before, true), (c.after, false)] {
            for k in 1..=n {
                let off = i64::from(k).saturating_mul(i64::from(c.step));
                let frame = if before { cur.saturating_sub(off) } else { cur.saturating_add(off) };
                if frame < first || frame > last || frame == cur {
                    continue;
                }
                let alpha = c.opacity * (1.0 - c.falloff).powi(k as i32 - 1);
                out.push(Ghost { frame, before, alpha, tint: if before { c.tint_before } else { c.tint_after } });
            }
        }
        out
    }

    /// The ghosts whose frames are not available yet: what the viewer asks the renderer for.
    /// Never more than the configured ghost count.
    pub fn missing(&self, cur: i64, first: i64, last: i64, have: impl Fn(i64) -> bool) -> Vec<Ghost> {
        let mut g = self.ghosts(cur, first, last);
        g.retain(|g| !have(g.frame));
        g.truncate(self.max_ghosts());
        g
    }
}

fn luma(r: u8, g: u8, b: u8) -> f32 {
    (0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b)) / 255.0
}

/// A ghost of premultiplied RGBA8 `frame`: its colours mixed (`amount`) towards `tint` shaded by
/// the frame's luminance, then everything scaled by `alpha`. Premultiplied RGBA8 out.
pub fn ghost_pixels(frame: &[u8], tint: [u8; 3], amount: f32, alpha: f32) -> Vec<u8> {
    let amount = unit(amount, 0.0);
    let alpha = unit(alpha, 0.0);
    let mut out = Vec::with_capacity(frame.len());
    for p in frame.as_chunks::<4>().0 {
        let l = luma(p[0], p[1], p[2]);
        for (c, t) in p[..3].iter().zip(tint) {
            let tinted = f32::from(t) * l;
            let mixed = f32::from(*c) * (1.0 - amount) + tinted * amount;
            out.push((mixed.min(f32::from(p[3])) * alpha + 0.5) as u8);
        }
        out.push((f32::from(p[3]) * alpha + 0.5) as u8);
    }
    out
}

/// `src` over `dst` (premultiplied RGBA8).
pub fn over(dst: [u8; 4], src: [u8; 4]) -> [u8; 4] {
    let inv = 255 - u32::from(src[3]);
    [0, 1, 2, 3].map(|i| (u32::from(src[i]) + (u32::from(dst[i]) * inv + 127) / 255).min(255) as u8)
}

/// The frame with `ghosts` (farthest first) laid over it, or under it with `under`.
pub fn composite(frame: &[u8], ghosts: &[&[u8]], under: bool) -> Vec<u8> {
    if under {
        let mut stack = vec![0u8; frame.len()];
        for g in ghosts {
            for (d, s) in stack.as_chunks_mut::<4>().0.iter_mut().zip(g.as_chunks::<4>().0) {
                *d = over(*d, *s);
            }
        }
        let mut out = frame.to_vec();
        for (d, s) in out.as_chunks_mut::<4>().0.iter_mut().zip(stack.as_chunks::<4>().0) {
            *d = over(*s, *d);
        }
        return out;
    }
    let mut out = frame.to_vec();
    for g in ghosts {
        for (d, s) in out.as_chunks_mut::<4>().0.iter_mut().zip(g.as_chunks::<4>().0) {
            *d = over(*d, *s);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on() -> OnionSkin {
        OnionSkin { enabled: true, ..OnionSkin::default() }
    }

    #[test]
    fn ghosts_follow_count_step_and_falloff() {
        let o = OnionSkin { before: 2, after: 1, step: 3, opacity: 0.8, falloff: 0.5, ..on() };
        let g = o.ghosts(10, 0, 100);
        let frames: Vec<(i64, bool)> = g.iter().map(|g| (g.frame, g.before)).collect();
        assert_eq!(frames, [(7, true), (4, true), (13, false)]);
        assert!((g[0].alpha - 0.8).abs() < 1e-6 && (g[1].alpha - 0.4).abs() < 1e-6);
        assert_eq!(g[0].tint, o.tint_before);
        assert_eq!(g[2].tint, o.tint_after);
    }

    #[test]
    fn ghosts_stay_inside_the_comp_and_off_when_disabled() {
        let o = OnionSkin { before: 3, after: 3, ..on() };
        let frames: Vec<i64> = o.ghosts(1, 0, 3).iter().map(|g| g.frame).collect();
        assert_eq!(frames, [0, 2, 3]);
        assert!(OnionSkin::default().ghosts(5, 0, 10).is_empty());
    }

    #[test]
    fn request_budget_never_exceeds_the_configured_ghosts() {
        for (b, a) in [(0, 0), (1, 0), (2, 3), (8, 8), (500, 500)] {
            let o = OnionSkin { before: b, after: a, ..on() };
            let want = (b.min(MAX_GHOSTS_PER_SIDE) + a.min(MAX_GHOSTS_PER_SIDE)) as usize;
            let missing = o.missing(1000, 0, 5000, |_| false);
            assert_eq!(missing.len(), want);
            assert!(missing.len() <= o.max_ghosts());
        }
        let o = OnionSkin { before: 2, after: 2, ..on() };
        let missing = o.missing(10, 0, 100, |f| f == 9 || f == 11);
        assert_eq!(missing.iter().map(|g| g.frame).collect::<Vec<_>>(), [8, 12]);
        let hostile = OnionSkin { before: u32::MAX, after: u32::MAX, step: u32::MAX, opacity: f32::NAN, falloff: f32::INFINITY, ..on() };
        assert!(hostile.missing(i64::MAX - 2, i64::MIN, i64::MAX, |_| false).len() <= 16);
    }

    #[test]
    fn ghost_tint_and_alpha_on_a_synthetic_frame() {
        let white = [255u8, 255, 255, 255, 0, 0, 0, 0];
        let full = ghost_pixels(&white, [255, 0, 0], 1.0, 1.0);
        assert_eq!(&full[..4], &[255, 0, 0, 255]);
        assert_eq!(&full[4..], &[0, 0, 0, 0]);
        let half = ghost_pixels(&white, [255, 0, 0], 1.0, 0.5);
        assert_eq!(&half[..4], &[128, 0, 0, 128]);
        let untinted = ghost_pixels(&white, [255, 0, 0], 0.0, 0.5);
        assert_eq!(&untinted[..4], &[128, 128, 128, 128]);
        let grey = ghost_pixels(&[128, 128, 128, 255], [0, 0, 255], 1.0, 1.0);
        assert_eq!(grey, [0, 0, 128, 255]);
    }

    #[test]
    fn ghosts_composite_over_and_under() {
        let frame = [0u8, 0, 255, 255, 0, 0, 0, 0];
        let ghost = [255u8, 0, 0, 128, 255, 0, 0, 128];
        let over_px = composite(&frame, &[&ghost], false);
        assert_eq!(&over_px[..4], &[255, 0, 127, 255]);
        assert_eq!(&over_px[4..], &[255, 0, 0, 128]);
        let under_px = composite(&frame, &[&ghost], true);
        assert_eq!(&under_px[..4], &[0, 0, 255, 255]);
        assert_eq!(&under_px[4..], &[255, 0, 0, 128]);
    }

    #[test]
    fn under_keeps_the_nearer_ghost_on_top_of_the_farther() {
        let frame = [0u8; 4];
        let far = [128u8, 0, 0, 128];
        let near = [0u8, 128, 0, 128];
        let under_px = composite(&frame, &[&far, &near], true);
        let over_px = composite(&frame, &[&far, &near], false);
        assert_eq!(under_px, over_px);
        assert!(under_px[1] > under_px[0]);
    }
}
