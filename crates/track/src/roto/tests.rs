//! Roto Brush on synthetic sequences: a textured disk moving over a textured background.

use super::refine::{MatteParams, decontaminate, matte_alpha};
use super::*;

pub(crate) const W: u32 = 200;
pub(crate) const H: u32 = 150;
pub(crate) const R: f64 = 32.0;

/// Smooth value noise in 0..1.
pub(crate) fn noise(x: f64, y: f64, seed: u32) -> f64 {
    fn h(i: i64, j: i64, s: u32) -> f64 {
        let mut v = (i.wrapping_mul(73856093) ^ j.wrapping_mul(19349663) ^ (s as i64).wrapping_mul(83492791)) as u64;
        v ^= v >> 13;
        v = v.wrapping_mul(0x5bd1e995);
        v ^= v >> 15;
        (v % 10_000) as f64 / 10_000.0
    }
    let mut out = 0.0;
    for (cell, amp) in [(9.0, 0.6), (3.5, 0.4)] {
        let (fx, fy) = (x / cell, y / cell);
        let (i, j) = (fx.floor() as i64, fy.floor() as i64);
        let (tx, ty) = (fx - i as f64, fy - j as f64);
        let (sx, sy) = (tx * tx * (3.0 - 2.0 * tx), ty * ty * (3.0 - 2.0 * ty));
        let a = h(i, j, seed) + (h(i + 1, j, seed) - h(i, j, seed)) * sx;
        let b = h(i, j + 1, seed) + (h(i + 1, j + 1, seed) - h(i, j + 1, seed)) * sx;
        out += amp * (a + (b - a) * sy);
    }
    out
}

pub(crate) fn fg_color(x: f64, y: f64) -> [f32; 3] {
    let n = noise(x, y, 11);
    [(0.75 + 0.25 * n) as f32, (0.35 + 0.3 * n) as f32, (0.1 + 0.15 * n) as f32]
}

pub(crate) fn bg_color(x: f64, y: f64) -> [f32; 3] {
    let n = noise(x, y, 5);
    let m = noise(x * 1.7, y * 1.7, 9);
    [(0.05 + 0.25 * m) as f32, (0.3 + 0.4 * n) as f32, (0.45 + 0.4 * m) as f32]
}

pub(crate) fn center(f: i64) -> [f64; 2] {
    let t = f as f64;
    [60.0 + 3.1 * t, 70.0 + 1.3 * t + 4.0 * (t * 0.3).sin()]
}

/// Frame `f`: the disk (texture moving with it) over the static background, with a 1-pixel
/// anti-aliased edge; `c` overrides the centre.
pub(crate) fn frame_at(c: [f64; 2]) -> (Image, Vec<u8>) {
    let mut img = Image::new(W, H);
    let mut gt = vec![0u8; (W * H) as usize];
    for y in 0..H {
        for x in 0..W {
            let p = [x as f64 + 0.5, y as f64 + 0.5];
            let d = (p[0] - c[0]).hypot(p[1] - c[1]);
            let a = (R + 0.5 - d).clamp(0.0, 1.0) as f32;
            let f = fg_color(p[0] - c[0], p[1] - c[1]);
            let b = bg_color(p[0], p[1]);
            img.set(x, y, [f[0] * a + b[0] * (1.0 - a), f[1] * a + b[1] * (1.0 - a), f[2] * a + b[2] * (1.0 - a), 1.0]);
            gt[(y * W + x) as usize] = (d <= R) as u8;
        }
    }
    (img, gt)
}

fn line(kind: StrokeKind, frame: i64, a: [f64; 2], b: [f64; 2], radius: f64) -> Stroke {
    Stroke { kind, frame, radius, points: (0..=10).map(|k| [a[0] + (b[0] - a[0]) * k as f64 / 10.0, a[1] + (b[1] - a[1]) * k as f64 / 10.0]).collect() }
}

/// The strokes an artist would paint on frame `f`: one through the disk, one around it.
pub(crate) fn base_strokes(f: i64) -> Vec<Stroke> {
    let c = center(f);
    vec![
        line(StrokeKind::Fg, f, [c[0] - 18.0, c[1] - 6.0], [c[0] + 18.0, c[1] + 6.0], 5.0),
        line(StrokeKind::Bg, f, [c[0] - 50.0, c[1] - 45.0], [c[0] + 50.0, c[1] - 45.0], 5.0),
        line(StrokeKind::Bg, f, [c[0] + 50.0, c[1] + 45.0], [c[0] - 45.0, c[1] + 45.0], 5.0),
    ]
}

#[test]
fn strokes_segment_the_base_frame() {
    let (img, gt) = frame_at(center(0));
    let st = base_strokes(0);
    let refs: Vec<&Stroke> = st.iter().collect();
    let seg = segment(&img, &refs, None, &SegOpts::default(), 0.0);
    let j = iou(&seg.matte, &gt);
    assert!(j > 0.95, "IoU {j}");
    // Deterministic.
    let again = segment(&img, &refs, None, &SegOpts::default(), 0.0);
    assert_eq!(seg, again);
    // The binary form round-trips exactly and is compact (run-length encoded planes).
    let bytes = seg.to_bytes();
    assert_eq!(FrameSeg::from_bytes(&bytes).as_ref(), Some(&seg));
    assert!(bytes.len() < seg.w * seg.h / 4, "{} bytes", bytes.len());
    assert!(FrameSeg::from_bytes(&bytes[..bytes.len() - 1]).is_none());
    assert!(FrameSeg::from_bytes(b"nope").is_none());

    // Untrusted plane dimensions and run lengths are rejected before allocation/resize.
    let mut hostile = b"ECSG".to_vec();
    hostile.extend_from_slice(&u32::MAX.to_le_bytes());
    hostile.extend_from_slice(&u32::MAX.to_le_bytes());
    hostile.extend_from_slice(&0u32.to_le_bytes());
    assert!(FrameSeg::from_bytes(&hostile).is_none());
    let mut overrun = bytes.clone();
    let plane_at = 16 + u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    overrun[plane_at..plane_at + 2].copy_from_slice(&u16::MAX.to_le_bytes());
    assert!(FrameSeg::from_bytes(&overrun).is_none());
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(FrameSeg::from_bytes(&trailing).is_none());
    assert_eq!(FrameSeg::empty(usize::MAX, 2), FrameSeg::default());
}

#[test]
fn coarse_to_fine_segments_large_frames() {
    // A 2× upscaled frame goes through the reduced-size path (Standard works at 480 px).
    let (small, _) = frame_at(center(0));
    let img = effectcraft_raster::resample(&small, W * 3, H * 3);
    let c = center(0);
    let st: Vec<Stroke> = base_strokes(0)
        .into_iter()
        .map(|mut s| {
            s.points.iter_mut().for_each(|p| *p = [p[0] * 3.0, p[1] * 3.0]);
            s.radius *= 3.0;
            s
        })
        .collect();
    let refs: Vec<&Stroke> = st.iter().collect();
    let seg = segment(&img, &refs, None, &SegOpts::default(), 0.0);
    let gt: Vec<u8> =
        (0..W * H * 9).map(|i| ((((i % (W * 3)) as f64 + 0.5) / 3.0 - c[0]).hypot((((i / (W * 3)) as f64 + 0.5) / 3.0) - c[1]) <= R) as u8).collect();
    let j = iou(&seg.matte, &gt);
    assert!(j > 0.95, "IoU {j}");
}

#[test]
fn propagation_follows_the_disk_for_20_frames() {
    let (img0, _) = frame_at(center(0));
    let st = base_strokes(0);
    let refs: Vec<&Stroke> = st.iter().collect();
    let opts = SegOpts::default();
    let mut seg = segment(&img0, &refs, None, &opts, 0.0);
    let mut prev = img0;
    let mut worst: f64 = 1.0;
    for f in 1..=20 {
        let (img, gt) = frame_at(center(f));
        seg = propagate(&prev, &seg, &img, &[], &opts, 0.0);
        let j = iou(&seg.matte, &gt);
        worst = worst.min(j);
        prev = img;
    }
    assert!(worst > 0.9, "worst IoU {worst}");
}

#[test]
fn a_correction_stroke_fixes_a_bad_frame() {
    let opts = SegOpts::default();
    let (img0, _) = frame_at(center(0));
    let st = base_strokes(0);
    let refs: Vec<&Stroke> = st.iter().collect();
    let seg0 = segment(&img0, &refs, None, &opts, 0.0);
    // The disk jumps far away on frame 1: propagation cannot follow.
    let jump = [150.0, 90.0];
    let (img1, gt1) = frame_at(jump);
    let bad = propagate(&img0, &seg0, &img1, &[], &opts, 0.0);
    assert!(iou(&bad.matte, &gt1) < 0.5, "the jump should break propagation");
    // Paint the disk in and the old place out.
    let c0 = center(0);
    let fix = [
        line(StrokeKind::Fg, 1, [jump[0] - 15.0, jump[1]], [jump[0] + 15.0, jump[1]], 5.0),
        line(StrokeKind::Bg, 1, [c0[0] - 10.0, c0[1] - 10.0], [c0[0] + 10.0, c0[1] + 10.0], 6.0),
    ];
    let fr: Vec<&Stroke> = fix.iter().collect();
    let good = propagate(&img0, &seg0, &img1, &fr, &opts, 0.0);
    let j = iou(&good.matte, &gt1);
    assert!(j > 0.95, "corrected IoU {j}");
    // And propagation continues from the corrected frame.
    let c2 = [jump[0] + 3.0, jump[1] + 1.0];
    let (img2, gt2) = frame_at(c2);
    let next = propagate(&img1, &good, &img2, &[], &opts, 0.0);
    assert!(iou(&next.matte, &gt2) > 0.93);
}

/// A stroke corrects the ones painted before it: foreground over an earlier background stroke
/// restores the area, and background over that removes it again.
#[test]
fn later_strokes_override_earlier_ones() {
    let c = center(0);
    let (img, gt) = frame_at(c);
    let spot = [c[0], c[1] + 20.0];
    let i = spot[1] as usize * W as usize + spot[0] as usize;
    let red = line(StrokeKind::Bg, 0, [spot[0] - 6.0, spot[1]], [spot[0] + 6.0, spot[1]], 6.0);
    let green = Stroke { kind: StrokeKind::Fg, ..red.clone() };
    let seg = |st: &[Stroke]| segment(&img, &st.iter().collect::<Vec<_>>(), None, &SegOpts::default(), 0.0);
    let mut st = base_strokes(0);
    st.push(red.clone());
    assert_eq!(seg(&st).matte[i], 0, "background over the disk removes it");
    st.push(green);
    let restored = seg(&st);
    assert_eq!(restored.matte[i], 1, "foreground over the background stroke restores it");
    assert!(iou(&restored.matte, &gt) > 0.95, "IoU {}", iou(&restored.matte, &gt));
    st.push(red);
    assert_eq!(seg(&st).matte[i], 0, "background again removes it again");
}

/// A soft-edged disk (alpha ramp over `width` pixels) over a background.
fn soft_frame(c: [f64; 2], width: f64, fg: [f32; 3], bg: [f32; 3]) -> (Image, Vec<f32>) {
    let mut img = Image::new(W, H);
    let mut gt = vec![0.0f32; (W * H) as usize];
    for y in 0..H {
        for x in 0..W {
            let p = [x as f64 + 0.5, y as f64 + 0.5];
            let d = (p[0] - c[0]).hypot(p[1] - c[1]);
            let a = ((R + width / 2.0 - d) / width).clamp(0.0, 1.0) as f32;
            // Faint texture on both layers.
            let tf = (noise(p[0], p[1], 3) as f32 - 0.5) * 0.04;
            let tb = (noise(p[0], p[1], 4) as f32 - 0.5) * 0.04;
            let f = [fg[0] + tf, fg[1] + tf, fg[2] + tf];
            let b = [bg[0] + tb, bg[1] + tb, bg[2] + tb];
            img.set(x, y, [f[0] * a + b[0] * (1.0 - a), f[1] * a + b[1] * (1.0 - a), f[2] * a + b[2] * (1.0 - a), 1.0]);
            gt[(y * W + x) as usize] = a;
        }
    }
    (img, gt)
}

fn ring(frame: i64, c: [f64; 2], r: f64, radius: f64) -> Stroke {
    Stroke {
        kind: StrokeKind::Refine,
        frame,
        radius,
        points: (0..=48).map(|k| k as f64 / 48.0 * std::f64::consts::TAU).map(|a| [c[0] + r * a.cos(), c[1] + r * a.sin()]).collect(),
    }
}

#[test]
fn refine_edge_recovers_a_soft_edge_and_decontaminates() {
    let c = [100.0, 75.0];
    let (fgc, bgc) = ([0.9f32, 0.25, 0.1], [0.1f32, 0.3, 0.9]);
    let (img, gt) = soft_frame(c, 10.0, fgc, bgc);
    let mut st = vec![
        line(StrokeKind::Fg, 0, [c[0] - 15.0, c[1]], [c[0] + 15.0, c[1]], 5.0),
        line(StrokeKind::Bg, 0, [c[0] - 60.0, c[1] - 55.0], [c[0] + 60.0, c[1] - 55.0], 5.0),
    ];
    st.push(ring(0, c, R, 9.0));
    let refs: Vec<&Stroke> = st.iter().collect();
    let seg = segment(&img, &refs, None, &SegOpts::default(), 9.0);
    assert!(seg.refine.iter().filter(|v| **v != 0).count() > 1000);
    let p = MatteParams::default();
    let rgb = rgb_of(&img);
    let a = matte_alpha(&rgb, &seg, None, None, &p, 9.0, 1.0);
    // Inside the ramp the matte is fractional and close to the truth.
    let (mut err, mut n, mut frac) = (0.0f64, 0usize, 0usize);
    let (mut err_bin, mut n_bin) = (0.0f64, 0usize);
    for i in 0..a.len() {
        if gt[i] > 0.0 && gt[i] < 1.0 {
            err += (a[i] - gt[i]).abs() as f64;
            err_bin += (seg.matte[i] as f32 - gt[i]).abs() as f64;
            n += 1;
            n_bin += 1;
            frac += (a[i] > 0.05 && a[i] < 0.95) as usize;
        }
    }
    let (err, err_bin) = (err / n as f64, err_bin / n_bin as f64);
    assert!(err < 0.03, "mean alpha error {err} (binary {err_bin})");
    assert!(err < err_bin * 0.4);
    assert!(frac as f64 > 0.6 * n as f64, "{frac}/{n} fractional");
    // Decontamination: edge pixels' colour is the foreground's, not a blue blend.
    let mut col = rgb.clone();
    let map = decontaminate(&mut col, &a, &seg.refine, (W as usize, H as usize), &p, 9.0, 1.0);
    let (mut blue_before, mut blue_after, mut m) = (0.0f64, 0.0f64, 0usize);
    for i in 0..a.len() {
        if a[i] > 0.2 && a[i] < 0.8 {
            blue_before += rgb[i][2] as f64;
            blue_after += col[i][2] as f64;
            m += 1;
        }
    }
    let (bb, ba) = (blue_before / m as f64, blue_after / m as f64);
    assert!(ba < 0.2 && bb > 0.35, "blue before {bb}, after {ba}");
    assert!(map.iter().any(|v| *v > 0.2));
    // Without the band, the binary matte passes through.
    let mut seg2 = seg.clone();
    seg2.refine.iter_mut().for_each(|v| *v = 0);
    let a2 = matte_alpha(&rgb, &seg2, None, None, &p, 9.0, 1.0);
    assert!(a2.iter().zip(&seg.matte).all(|(x, y)| (*x - *y as f32).abs() < 1e-6));
}

/// Refine Edge strokes correct each other the same way: refine over an earlier erase restores
/// the band there.
#[test]
fn later_refine_strokes_override_earlier_erases() {
    let c = [100.0, 75.0];
    let (img, _) = soft_frame(c, 10.0, [0.9, 0.25, 0.1], [0.1, 0.3, 0.9]);
    let spot = [c[0] + R, c[1]];
    let i = spot[1] as usize * W as usize + spot[0] as usize;
    let erase = Stroke { kind: StrokeKind::RefineErase, frame: 0, radius: 6.0, points: vec![spot] };
    let refine = Stroke { kind: StrokeKind::Refine, ..erase.clone() };
    let band = |st: &[Stroke]| segment(&img, &st.iter().collect::<Vec<_>>(), None, &SegOpts::default(), 9.0).refine[i];
    let mut st = vec![
        line(StrokeKind::Fg, 0, [c[0] - 15.0, c[1]], [c[0] + 15.0, c[1]], 5.0),
        line(StrokeKind::Bg, 0, [c[0] - 60.0, c[1] - 55.0], [c[0] + 60.0, c[1] - 55.0], 5.0),
        ring(0, c, R, 9.0),
    ];
    assert_eq!(band(&st), 1);
    st.push(erase.clone());
    assert_eq!(band(&st), 0, "erase removes the band");
    st.push(refine);
    assert_eq!(band(&st), 1, "refine over the erase restores it");
    st.push(erase);
    assert_eq!(band(&st), 0, "erase again removes it again");
}

#[test]
fn reduce_chatter_and_motion_blur_use_the_neighbours() {
    let (w, h) = (40usize, 20usize);
    let mk = |x0: usize| FrameSeg {
        w,
        h,
        matte: (0..w * h).map(|i| (i % w >= x0 && i % w < x0 + 10) as u8).collect(),
        refine: vec![0; w * h],
        ..Default::default()
    };
    let (a, b, c) = (mk(10), mk(14), mk(18));
    let rgb = vec![[0.5f32; 3]; w * h];
    let p = MatteParams { rb_chatter: 100.0, ..Default::default() };
    let al = matte_alpha(&rgb, &b, Some(&a), Some(&c), &p, 0.0, 1.0);
    // Pixel 12 is only in the previous frame: partially on.
    assert!(al[12] > 0.2 && al[12] < 0.5, "{}", al[12]);
    let p = MatteParams { motion_blur: true, ..Default::default() };
    let al = matte_alpha(&rgb, &b, Some(&a), Some(&c), &p, 0.0, 1.0);
    assert!(al[14] > 0.3 && al[14] < 0.9, "{}", al[14]);
    assert!(al[19] > 0.99);
}

#[test]
fn chain_keys_restart_at_correction_frames() {
    let mut d = RotoData::default();
    for s in base_strokes(5) {
        d.add(s, 20, [0, 30]);
    }
    assert_eq!(d.base, Some(5));
    assert_eq!(d.span, [0, 25]);
    let k1 = d.chain_keys(1);
    d.add(line(StrokeKind::Fg, 12, [0.0, 0.0], [1.0, 1.0], 3.0), 20, [0, 30]);
    let k2 = d.chain_keys(1);
    for f in 0..=11 {
        assert_eq!(k1[&f], k2[&f], "frame {f}");
    }
    for f in 12..=25 {
        assert_ne!(k1[&f], k2[&f], "frame {f}");
    }
    assert_ne!(d.chain_keys(2)[&5], k2[&5]);
    let back = RotoData::from_json(&d.to_json());
    assert_eq!(back, d);
    assert_eq!(d.source_of(12), Some(11));
    assert_eq!(d.source_of(3), Some(4));
    assert_eq!(d.source_of(5), None);
}

/// `cargo test -p effectcraft-track --release roto_perf -- --ignored --nocapture`
#[test]
#[ignore]
fn roto_perf_1080p() {
    let up = |c: [f64; 2]| {
        let (img, _) = frame_at(c);
        effectcraft_raster::resample(&img, 1920, 1080)
    };
    let s = 1920.0 / W as f64;
    let st: Vec<Stroke> = base_strokes(0)
        .into_iter()
        .map(|mut k| {
            k.points.iter_mut().for_each(|p| *p = [p[0] * s, p[1] * s]);
            k.radius *= s;
            k
        })
        .collect();
    let refs: Vec<&Stroke> = st.iter().collect();
    let opts = SegOpts { search_radius: 15.0 * s / 4.0, ..Default::default() };
    let a = up(center(0));
    let t = std::time::Instant::now();
    let seg = segment(&a, &refs, None, &opts, 0.0);
    eprintln!("segment 1080p: {:?}", t.elapsed());
    let b = up(center(1));
    let t = std::time::Instant::now();
    let next = propagate(&a, &seg, &b, &[], &opts, 0.0);
    eprintln!("propagate 1080p: {:?} (area {})", t.elapsed(), next.area());
    let mut seg2 = next.clone();
    let c = center(1);
    let ring: Vec<[f64; 2]> = (0..=64).map(|k| k as f64 / 64.0 * std::f64::consts::TAU).map(|t| [(c[0] + R * t.cos()) * s, (c[1] + R * t.sin()) * s]).collect();
    let st = [Stroke { kind: StrokeKind::Refine, frame: 1, radius: 20.0, points: ring }];
    let r = rasterize(&st.iter().collect::<Vec<_>>(), &[StrokeKind::Refine], 1.0, 1920, 1080);
    seg2.refine = r.iter().map(|v| *v as u8).collect();
    let rgb = rgb_of(&b);
    let t = std::time::Instant::now();
    let _ = matte_alpha(&rgb, &seg2, None, None, &MatteParams::default(), 20.0, 1.0);
    eprintln!("refine 1080p: {:?}", t.elapsed());
}

/// A stand-in trained model: the foreground is a fixed rectangle (or nothing, or an error).
struct FakeModel(Option<[usize; 4]>, bool);

impl effectcraft_segment::MaskModel for FakeModel {
    fn info(&self) -> &'static effectcraft_segment::ModelInfo {
        &effectcraft_segment::MOBILE_SAM
    }
    fn segment(&self, _: &[[f32; 3]], w: usize, h: usize, prompt: &Prompt) -> Result<Vec<f32>, String> {
        if self.1 {
            return Err("broken".into());
        }
        assert!(prompt.points.iter().any(|(_, fg)| *fg), "prompted with foreground points");
        Ok((0..w * h)
            .map(|i| match self.0 {
                Some([x0, y0, x1, y1]) => ((i % w) >= x0 && (i % w) < x1 && (i / w) >= y0 && (i / w) < y1) as u8 as f32 * 0.98 + 0.01,
                None => 0.01,
            })
            .collect())
    }
}

/// A grey bar on a two-tone background whose halves match the bar's halves: colour alone can't
/// find its edge, a model can. The frame, its truth and the bar's rectangle.
fn bar_frame() -> (Image, Vec<u8>, [usize; 4]) {
    let (w, h) = (160usize, 100usize);
    let rect = [40, 30, 120, 70];
    let mut img = Image::new(w as u32, h as u32);
    let mut gt = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let inside = x >= rect[0] && x < rect[2] && y >= rect[1] && y < rect[3];
            // Left half dark, right half light, inside or out; a faint stripe pattern inside.
            let base = if x < w / 2 { 0.2 } else { 0.8 };
            let v = if inside { base + 0.02 * ((x / 3) % 2) as f32 } else { base };
            img.set(x as u32, y as u32, [v, v, v, 1.0]);
            gt[y * w + x] = inside as u8;
        }
    }
    (img, gt, rect)
}

#[test]
fn a_model_prior_shapes_the_cut_and_bad_models_are_ignored() {
    let (img, gt, rect) = bar_frame();
    let st = [line(StrokeKind::Fg, 0, [50.0, 50.0], [110.0, 50.0], 4.0)];
    let refs: Vec<&Stroke> = st.iter().collect();
    let opts = SegOpts::default();
    let classical = segment(&img, &refs, None, &opts, 0.0);
    let good = FakeModel(Some(rect), false);
    let with = segment_with(&img, &refs, None, &opts, 0.0, Some(&good));
    let (jc, jm) = (iou(&classical.matte, &gt), iou(&with.matte, &gt));
    assert!(jm > 0.95 && jm > jc + 0.2, "model {jm} vs classical {jc}");
    // A failing model: the classical result, unchanged.
    let broken = FakeModel(None, true);
    assert_eq!(segment_with(&img, &refs, None, &opts, 0.0, Some(&broken)), classical);
    // Propagation: a model that loses the object (disagrees with the flow) is ignored.
    let next = img.clone();
    let lost = FakeModel(None, false);
    let a = propagate_with(&img, &with, &next, &[], &opts, 0.0, Some(&lost));
    let b = propagate(&img, &with, &next, &[], &opts, 0.0);
    assert_eq!(a, b);
    // A good model keeps the bar through propagation.
    let kept = propagate_with(&img, &with, &next, &[], &opts, 0.0, Some(&good));
    assert!(iou(&kept.matte, &gt) > 0.95, "{}", iou(&kept.matte, &gt));
}

/// With the real MobileSAM weights (`EFFECTCRAFT_MOBILESAM=path/to/mobile_sam.pt`, else
/// skipped): the base frame and 20 propagated frames of the moving textured disk stay at least
/// as accurate as the classic engine.
#[test]
fn mobilesam_segments_and_propagates_the_disk() {
    let Ok(path) = std::env::var("EFFECTCRAFT_MOBILESAM") else { return };
    let effectcraft_segment::Loaded::Mask(model) = effectcraft_segment::load("mobilesam", &std::fs::read(path).unwrap()).unwrap() else {
        panic!("not a mask model")
    };
    let m = Some(model.as_ref());
    let (img0, gt0) = frame_at(center(0));
    let st = base_strokes(0);
    let refs: Vec<&Stroke> = st.iter().collect();
    let opts = SegOpts::default();
    let base = segment_with(&img0, &refs, None, &opts, 0.0, m);
    let jb = iou(&base.matte, &gt0);
    let (mut seg, mut cseg) = (base, segment(&img0, &refs, None, &opts, 0.0));
    let mut prev = img0;
    let (mut worst, mut cworst): (f64, f64) = (1.0, 1.0);
    let t = std::time::Instant::now();
    for f in 1..=20 {
        let (img, gt) = frame_at(center(f));
        seg = propagate_with(&prev, &seg, &img, &[], &opts, 0.0, m);
        cseg = propagate(&prev, &cseg, &img, &[], &opts, 0.0);
        worst = worst.min(iou(&seg.matte, &gt));
        cworst = cworst.min(iou(&cseg.matte, &gt));
        prev = img;
    }
    eprintln!("MobileSAM: base IoU {jb:.4}, worst propagated {worst:.4} (classic {cworst:.4}), {:.0} ms/frame", t.elapsed().as_secs_f64() * 1000.0 / 20.0);
    assert!(jb > 0.95 && worst > 0.93 && worst >= cworst - 0.01, "base {jb}, worst {worst} vs classic {cworst}");
}

/// A box stroke becomes the model's box prompt (scaled, corners in any order); the graph cut
/// ignores it; a prior's box does not replace it.
#[test]
fn box_stroke_is_the_model_box_prompt() {
    let fg = Stroke { kind: StrokeKind::Fg, frame: 0, radius: 2.0, points: vec![[50.0, 40.0]] };
    let bx = Stroke { kind: StrokeKind::Box, frame: 0, radius: 0.0, points: vec![[80.0, 60.0], [20.0, 10.0]] };
    let p = stroke_prompt(&[&fg, &bx], &[], 0.5, 0, 0);
    assert_eq!(p.bbox, Some([10.0, 5.0, 40.0, 30.0]));
    assert_eq!(p.points, vec![([25.0, 20.0], true)]);
    // Degenerate or non-finite boxes are no prompt.
    for pts in [vec![[20.0, 10.0]], vec![[20.0, 10.0], [20.0, 50.0]], vec![[f64::NAN, 10.0], [80.0, 60.0]]] {
        let b = Stroke { points: pts, ..bx.clone() };
        assert_eq!(stroke_prompt(&[&fg, &b], &[], 1.0, 0, 0).bbox, None);
    }
    assert!(!StrokeKind::Box.is_segmentation());
    assert!(rasterize(&[&bx], &[StrokeKind::Fg, StrokeKind::Bg], 1.0, 100, 80).iter().all(|v| !v));
    let mut q = stroke_prompt(&[&fg, &bx], &[], 1.0, 0, 0);
    let soft: Vec<f32> = (0..100 * 80).map(|i| ((i % 100) < 10) as u8 as f32).collect();
    assert!(add_prior_prompt(&mut q, &soft, &[], 100, 80));
    assert_eq!(q.bbox, Some([20.0, 10.0, 80.0, 60.0]));
}

/// A model is never prompted against the latest correction: stroke points that a later stroke
/// of the other kind painted over are dropped, and the propagated matte's points and mask leave
/// out what this frame's background strokes removed.
#[test]
fn model_prompts_follow_the_latest_stroke() {
    let green = Stroke { kind: StrokeKind::Fg, frame: 0, radius: 4.0, points: vec![[50.0, 40.0]] };
    let red = Stroke { kind: StrokeKind::Bg, ..green.clone() };
    let (w, h) = (100, 80);
    let prompt = |st: &[&Stroke]| stroke_prompt(st, &resolve(st, StrokeKind::Fg, StrokeKind::Bg, 1.0, w, h), 1.0, w, h).points;
    assert_eq!(prompt(&[&green, &red]), vec![([50.0, 40.0], false)]);
    assert_eq!(prompt(&[&green, &red, &green]), vec![([50.0, 40.0], true); 2]);
    let soft: Vec<f32> = (0..w * h).map(|i| ((i % w) < 50) as u8 as f32).collect();
    let erase = Stroke { kind: StrokeKind::Bg, frame: 0, radius: 15.0, points: vec![[25.0, 40.0]] };
    let hard = resolve(&[&erase], StrokeKind::Fg, StrokeKind::Bg, 1.0, w, h);
    let mut q = Prompt::default();
    assert!(add_prior_prompt(&mut q, &soft, &hard, w, h));
    assert!(!q.points.is_empty());
    assert!(q.points.iter().all(|(p, fg)| *fg && owner(&hard, w, h, *p) != Some(HARD_BG)), "{:?}", q.points);
    let mask = q.mask.unwrap();
    assert!(hard.iter().zip(&mask).all(|(hd, m)| *hd != HARD_BG || *m == 0.0));
}

/// A stand-in model that counts its calls; its foreground is a fixed rectangle.
struct CountingModel([usize; 4], std::sync::atomic::AtomicUsize);

impl effectcraft_segment::MaskModel for CountingModel {
    fn info(&self) -> &'static effectcraft_segment::ModelInfo {
        &effectcraft_segment::MOBILE_SAM
    }
    fn segment(&self, _: &[[f32; 3]], w: usize, h: usize, _: &Prompt) -> Result<Vec<f32>, String> {
        self.1.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let [x0, y0, x1, y1] = self.0;
        Ok((0..w * h).map(|i| ((i % w) >= x0 && (i % w) < x1 && (i / w) >= y0 && (i / w) < y1) as u8 as f32 * 0.98 + 0.01).collect())
    }
}

/// A small background correction in the middle of a large foreground dot removes the only point
/// sampled along the dot, but most of the dot is still foreground: the model is still prompted
/// with it, from a point inside what is left. A dot painted over completely prompts nothing.
#[test]
fn a_correction_keeps_the_model_prompted_with_what_is_left() {
    let (img, gt, rect) = bar_frame();
    let (w, h) = (img.width as usize, img.height as usize);
    let dot = Stroke { kind: StrokeKind::Fg, frame: 0, radius: 15.0, points: vec![[80.0, 50.0]] };
    let tap = Stroke { kind: StrokeKind::Bg, radius: 2.0, ..dot.clone() };
    let refs = [&dot, &tap];
    let model = CountingModel(rect, Default::default());
    let seg = segment_with(&img, &refs, None, &SegOpts::default(), 0.0, Some(&model));
    assert_eq!(model.1.load(std::sync::atomic::Ordering::Relaxed), 1, "the model was prompted");
    assert!(iou(&seg.matte, &gt) > 0.95, "IoU {}", iou(&seg.matte, &gt));
    let hard = resolve(&refs, StrokeKind::Fg, StrokeKind::Bg, 1.0, w, h);
    let fg: Vec<[f32; 2]> = stroke_prompt(&refs, &hard, 1.0, w, h).points.into_iter().filter(|p| p.1).map(|p| p.0).collect();
    let [q] = fg.as_slice() else { panic!("one foreground point, got {fg:?}") };
    let r = (q[0] - 80.0).hypot(q[1] - 50.0);
    assert!(r > 5.0 && r < 13.0, "{q:?} sits inside what is left of the dot");
    assert_eq!(hard[q[1] as usize * w + q[0] as usize], HARD_FG);
    let cover = Stroke { radius: 16.0, ..tap };
    let refs = [&dot, &cover];
    let hard = resolve(&refs, StrokeKind::Fg, StrokeKind::Bg, 1.0, w, h);
    assert!(stroke_prompt(&refs, &hard, 1.0, w, h).points.iter().all(|p| !p.1));
}
