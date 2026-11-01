//! Mask tracking: follow the pixels inside a mask from frame to frame and report the motion as a
//! 2D transform (translation, + scale, + rotation, + skew, or perspective), which the caller
//! applies to the mask's vertices and tangents.
//!
//! Each step detects Shi–Tomasi features inside the current mask outline ([`crate::klt`]), tracks
//! them into the next frame with pyramidal Lucas–Kanade and a forward–backward check, and fits the
//! chosen model robustly with RANSAC ([`crate::fit`]), falling back to simpler models when too few
//! features survive. Only a crop around the mask (plus a margin for the motion) is analysed.

use effectcraft_raster::Image;

use crate::Frame;
use crate::fit::{Model, ransac};
use crate::klt::{CornerOpts, GrayPyramid, LkOpts, analysis_factor, good_features, luma_plane, track};
use crate::solve::Homography;

/// Point-in-polygon (even–odd rule).
pub fn inside_polygon(poly: &[[f64; 2]], p: [f64; 2]) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut c = false;
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
            c = !c;
        }
        j = i;
    }
    c
}

/// Bounding box `[x0, y0, x1, y1]` of points.
pub fn bounds(p: &[[f64; 2]]) -> [f64; 4] {
    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for q in p {
        b[0] = b[0].min(q[0]);
        b[1] = b[1].min(q[1]);
        b[2] = b[2].max(q[0]);
        b[3] = b[3].max(q[1]);
    }
    b
}

/// Crop `img` to `region` (layer pixels), with the layer offset of the crop's origin: the part
/// of the frame the tracker reads. `None` when the region leaves too little of the frame.
fn crop_region(img: &Image, off: [f64; 2], region: [f64; 4]) -> Option<(Image, [f64; 2])> {
    let x0 = (region[0] + off[0]).floor().max(0.0) as i64;
    let y0 = (region[1] + off[1]).floor().max(0.0) as i64;
    let x1 = ((region[2] + off[0]).ceil() as i64).min(img.width as i64);
    let y1 = ((region[3] + off[1]).ceil() as i64).min(img.height as i64);
    if x1 <= x0 + 8 || y1 <= y0 + 8 {
        return None;
    }
    Some((img.crop(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32), [off[0] - x0 as f64, off[1] - y0 as f64]))
}

/// Mask tracker state.
#[derive(Clone, Debug)]
pub struct MaskTracker {
    pub model: Model,
    /// The mask outline (layer pixels) on the last tracked frame.
    pub outline: Vec<[f64; 2]>,
    /// The last frame-to-frame motion (a constant-velocity guess for the next frame).
    velocity: Homography,
    /// The previous frame cropped to the region the next step reads, with its layer offset.
    prev: Option<(Image, [f64; 2])>,
    /// Features used on the last step (layer pixels), for display and tests.
    pub last_features: usize,
}

/// One mask tracking step's outcome.
#[derive(Clone, Debug)]
pub struct MaskStep {
    /// Motion from the previous frame to this one (layer pixels).
    pub motion: Homography,
    /// Correspondences that agreed with the motion.
    pub inliers: usize,
    /// The model actually fitted (may be simpler than requested when features are scarce).
    pub model: Model,
}

impl MaskTracker {
    /// Start on `frame` with the mask `outline` (a closed polyline in layer pixels).
    pub fn new(model: Model, outline: Vec<[f64; 2]>, frame: &Frame) -> MaskTracker {
        let mut t = MaskTracker { model, outline, velocity: Homography::IDENTITY, prev: None, last_features: 0 };
        t.prev = t.region().and_then(|r| crop_region(frame.img, frame.offset, r));
        t
    }

    /// Crop region (layer pixels) of the next step: the mask plus room for its motion.
    fn region(&self) -> Option<[f64; 4]> {
        let b = bounds(&self.outline);
        if !(b[0].is_finite() && b[2] > b[0] && b[3] > b[1]) {
            return None;
        }
        let margin = (0.35 * (b[2] - b[0]).max(b[3] - b[1])).max(32.0);
        Some([b[0] - margin, b[1] - margin, b[2] + margin, b[3] + margin])
    }

    /// Track into `frame` (the next frame in either direction). `None` when the mask's pixels
    /// can't be followed (no texture, or the mask left the frame).
    pub fn step(&mut self, frame: &Frame) -> Option<MaskStep> {
        let (ca, oa) = self.prev.take()?;
        let res = self.step_inner(ca, oa, frame);
        if let Some(r) = &res {
            self.outline = self.outline.iter().map(|p| r.motion.apply(*p)).collect();
            self.velocity = r.motion;
        }
        self.prev = self.region().and_then(|r| crop_region(frame.img, frame.offset, r));
        res
    }

    fn step_inner(&mut self, ca: Image, oa: [f64; 2], frame: &Frame) -> Option<MaskStep> {
        let b = bounds(&self.outline);
        let (bw, bh) = (b[2] - b[0], b[3] - b[1]);
        // Both frames share one plane coordinate system: the crops are in image pixels.
        let (cb, ob) = crop_region(frame.img, frame.offset, self.region()?)?;
        let factor = analysis_factor(ca.width.max(cb.width), ca.height.max(cb.height), 720);
        // Pyramids must share coordinates: use a common origin (layer pixels → plane).
        let levels = 3;
        let pa = GrayPyramid::from_plane(luma_plane(&ca, factor), oa, factor as f64, levels);
        let pb_raw = luma_plane(&cb, factor);
        // Shift frame b's crop into frame a's plane coordinates when the offsets differ.
        let pb = if (oa[0] - ob[0]).abs() < 1e-9 && (oa[1] - ob[1]).abs() < 1e-9 && ca.width == cb.width && ca.height == cb.height {
            GrayPyramid::from_plane(pb_raw, oa, factor as f64, levels)
        } else {
            let mut q = crate::plane::Plane::new(pa.levels[0].w, pa.levels[0].h, 1);
            let mut v = [0.0f32];
            for y in 0..q.h {
                for x in 0..q.w {
                    // plane a (x, y) → layer → plane b.
                    let l = pa.to_layer([x as f64 + 0.5, y as f64 + 0.5]);
                    let pbp = [(l[0] + ob[0]) / factor as f64, (l[1] + ob[1]) / factor as f64];
                    pb_raw.sample(pbp[0], pbp[1], &mut v);
                    q.data[y * q.w + x] = v[0];
                }
            }
            GrayPyramid::from_plane(q, oa, factor as f64, levels)
        };
        let outline: Vec<[f64; 2]> = self.outline.iter().map(|p| pa.to_plane(*p)).collect();
        let area = bw * bh / (factor * factor) as f64;
        let md = (area / 500.0).sqrt().clamp(3.0, 12.0);
        let inside = |p: [f64; 2]| inside_polygon(&outline, p);
        let feats = good_features(&pa, &CornerOpts { max_features: 500, min_distance: md, quality: 0.005, window: 2, border: 3 }, Some(&inside));
        self.last_features = feats.len();
        if feats.len() < 3 {
            return None;
        }
        // Constant-velocity guess.
        let guesses: Vec<[f64; 2]> = feats
            .iter()
            .map(|p| {
                let l = pa.to_layer(*p);
                let m = self.velocity.apply(l);
                [(m[0] - l[0]) / factor as f64, (m[1] - l[1]) / factor as f64]
            })
            .collect();
        let tracked = track(&pa, &pb, &feats, Some(&guesses), &LkOpts { radius: 6, fb_max: 0.75, ..Default::default() });
        let (src, dst): (Vec<[f64; 2]>, Vec<[f64; 2]>) = feats.iter().zip(&tracked).filter_map(|(p, q)| q.map(|q| (pa.to_layer(*p), pa.to_layer(q)))).unzip();
        let thr = 1.5 * factor as f64;
        let chain: &[Model] = match self.model {
            Model::Homography => &[Model::Homography, Model::Affine, Model::Similarity, Model::Translation],
            Model::Affine => &[Model::Affine, Model::Similarity, Model::Translation],
            Model::Similarity => &[Model::Similarity, Model::Translation],
            Model::TranslationScale => &[Model::TranslationScale, Model::Translation],
            Model::Translation => &[Model::Translation],
        };
        for m in chain {
            // Enough support for the model's freedom.
            let need = (m.min_points() * 3).max(3);
            if src.len() < need {
                continue;
            }
            if let Some(f) = ransac(*m, &src, &dst, thr, 400, 0x5eed + src.len() as u64) {
                let c = f.inlier_count();
                if c >= need && c * 3 >= src.len() {
                    return Some(MaskStep { motion: f.h, inliers: c, model: *m });
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tex(x: f64, y: f64) -> f32 {
        let v = (x * 0.37).sin() * (y * 0.29).cos() + 0.6 * ((x + 1.7 * y) * 0.21).sin() + 0.4 * ((x * 0.83 - y * 0.61).sin());
        (0.5 + 0.18 * v) as f32
    }

    /// A textured disk of radius 50 posed by `h` (object → layer) over flat grey.
    fn frame(h: &Homography) -> Image {
        let inv = h.inverse().unwrap();
        let mut im = Image::new(320, 240);
        for y in 0..240 {
            for x in 0..320 {
                let o = inv.apply([x as f64 + 0.5, y as f64 + 0.5]);
                let v = if o[0].hypot(o[1]) <= 50.0 { tex(o[0], o[1]) } else { 0.3 };
                im.set(x, y, [v, v, v, 1.0]);
            }
        }
        im
    }

    #[test]
    fn follows_a_rotating_scaling_disk() {
        let pose = |f: usize| {
            let a = (f as f64 * 4.0).to_radians();
            let s = 1.0 + 0.03 * f as f64;
            Homography([[s * a.cos(), -s * a.sin(), 140.0 + 4.0 * f as f64], [s * a.sin(), s * a.cos(), 110.0 + 2.0 * f as f64], [0.0, 0.0, 1.0]])
        };
        let ring: Vec<[f64; 2]> = (0..24).map(|i| (i as f64 * 15.0).to_radians()).map(|a| [40.0 * a.cos(), 40.0 * a.sin()]).collect();
        let f0 = frame(&pose(0));
        let mut mt = MaskTracker::new(Model::Similarity, ring.iter().map(|p| pose(0).apply(*p)).collect(), &Frame::new(&f0));
        for f in 1..8 {
            let img = frame(&pose(f));
            let st = mt.step(&Frame::new(&img)).expect("tracked");
            assert!(st.inliers > 10);
            let want: Vec<[f64; 2]> = ring.iter().map(|p| pose(f).apply(*p)).collect();
            let err = mt.outline.iter().zip(&want).map(|(a, b)| (a[0] - b[0]).hypot(a[1] - b[1])).fold(0.0, f64::max);
            assert!(err < 1.5, "frame {f}: max vertex error {err}");
        }
    }

    #[test]
    fn follows_the_disk_through_a_frame_offset() {
        let off = [40.0, -25.0];
        let pose = |f: usize| Homography([[1.0, 0.0, 140.0 + 5.0 * f as f64], [0.0, 1.0, 110.0 - 3.0 * f as f64], [0.0, 0.0, 1.0]]);
        let ring: Vec<[f64; 2]> = (0..24).map(|i| (i as f64 * 15.0).to_radians()).map(|a| [40.0 * a.cos(), 40.0 * a.sin()]).collect();
        let layer = |f: usize| -> Vec<[f64; 2]> { ring.iter().map(|p| pose(f).apply(*p)).map(|q| [q[0] - off[0], q[1] - off[1]]).collect() };
        let f0 = frame(&pose(0));
        let mut mt = MaskTracker::new(Model::Translation, layer(0), &Frame { img: &f0, offset: off });
        for f in 1..6 {
            let img = frame(&pose(f));
            mt.step(&Frame { img: &img, offset: off }).expect("tracked");
            let err = mt.outline.iter().zip(&layer(f)).map(|(a, b)| (a[0] - b[0]).hypot(a[1] - b[1])).fold(0.0, f64::max);
            assert!(err < 1.0, "frame {f}: max vertex error {err}");
        }
    }
}
