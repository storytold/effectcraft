//! Synthetic footage: a textured patch moving along a known path (with rotation and scale) over a
//! textured background, with noise. Ground truth is exact, so recovered positions are compared
//! against it directly.

use super::*;

/// Smooth, band-limited texture (periods of 9–40 px).
fn texture(x: f64, y: f64, seed: u32) -> [f32; 3] {
    let mut v = [0.0f64; 3];
    let mut s = seed.wrapping_mul(2654435761).wrapping_add(12345);
    let mut rnd = || {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        (s % 10_000) as f64 / 10_000.0
    };
    for _ in 0..7 {
        let ang = rnd() * std::f64::consts::TAU;
        let period = 9.0 + rnd() * 31.0;
        let phase = rnd() * std::f64::consts::TAU;
        let k = std::f64::consts::TAU / period;
        let w = (k * (x * ang.cos() + y * ang.sin()) + phase).sin();
        let c = [rnd(), rnd(), rnd()];
        for i in 0..3 {
            v[i] += w * (0.3 + c[i]) * 0.12;
        }
    }
    v.map(|q| (0.5 + q).clamp(0.0, 1.0) as f32)
}

fn noise(x: u32, y: u32, f: u32) -> f32 {
    effectcraft_raster::hash_noise(x, y.wrapping_add(f.wrapping_mul(7919)), 99) - 0.5
}

/// Patch pose on a frame: centre, rotation (degrees), scale.
#[derive(Clone, Copy)]
struct Pose {
    c: [f64; 2],
    rot: f64,
    scale: f64,
}

impl Pose {
    /// Where the patch-local point `u` lands in the frame.
    fn map(&self, u: [f64; 2]) -> [f64; 2] {
        let (s, c) = self.rot.to_radians().sin_cos();
        [self.c[0] + self.scale * (c * u[0] - s * u[1]), self.c[1] + self.scale * (s * u[0] + c * u[1])]
    }
    fn local(&self, p: [f64; 2]) -> [f64; 2] {
        let (s, c) = self.rot.to_radians().sin_cos();
        let d = [(p[0] - self.c[0]) / self.scale, (p[1] - self.c[1]) / self.scale];
        [c * d[0] + s * d[1], -s * d[0] + c * d[1]]
    }
}

/// Render a frame: background texture, the patch (half size `half`) on top, Gaussian-ish noise
/// of amplitude `amp`, and an optional flat occluder rectangle.
fn frame(w: u32, h: u32, pose: Pose, half: f64, amp: f32, f: u32, occluder: Option<[f64; 4]>) -> Image {
    let mut img = Image::new(w, h);
    let ss = [0.25, 0.75];
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0.0f32; 3];
            for sy in ss {
                for sx in ss {
                    let p = [x as f64 + sx, y as f64 + sy];
                    let l = pose.local(p);
                    let t = if l[0].abs() <= half && l[1].abs() <= half { texture(l[0], l[1], 7) } else { texture(p[0] * 0.7, p[1] * 0.7, 3).map(|v| v * 0.6) };
                    for i in 0..3 {
                        acc[i] += t[i] * 0.25;
                    }
                }
            }
            let n = noise(x, y, f) * amp;
            let mut px = [acc[0] + n, acc[1] + n, acc[2] + n, 1.0];
            if let Some([ox, oy, ow, oh]) = occluder {
                let (cx, cy) = (x as f64 + 0.5, y as f64 + 0.5);
                if cx >= ox && cx < ox + ow && cy >= oy && cy < oy + oh {
                    px = [0.4, 0.4, 0.4, 1.0];
                }
            }
            img.set(x, y, px);
        }
    }
    img
}

fn spec(c: [f64; 2], feature: f64, search: f64) -> PointSpec {
    PointSpec { center: c, feature_size: [feature, feature], search_offset: [0.0, 0.0], search_size: [search, search] }
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

fn bits(v: &Option<([f64; 2], [f64; 2])>) -> Option<[u64; 4]> {
    v.map(|(c, a)| [c[0].to_bits(), c[1].to_bits(), a[0].to_bits(), a[1].to_bits()])
}

#[test]
fn scorer_matches_ncc_for_integer_offsets() {
    let pose = Pose { c: [60.0, 50.0], rot: 0.0, scale: 1.0 };
    let img = frame(120, 100, pose, 20.0, 0.02, 1, None);
    let crop = Crop::new(&Frame::new(&img), &TrackOptions::default(), [60.0, 50.0], [40.0, 30.0], 1);
    for (lvl, c, a) in [(0, [30.3, 25.9], [0.97, 0.21]), (0, [12.0, 9.5], [1.2, -0.3]), (1, [20.4, 14.2], [0.8, 0.0]), (0, [30.3, 25.9], [1.0, 0.0])] {
        let plane = &crop.levels[lvl];
        let r = [7, 5];
        let patch = Patch::sample(plane, c, a, r);
        let sc = Scorer::new(plane, &patch, [c[0] + 0.3, c[1] - 0.45], a);
        for j in -9..=9 {
            for i in -9..=9 {
                let q = [c[0] + 0.3 + i as f64, c[1] - 0.45 + j as f64];
                assert_eq!(sc.at(i, j).to_bits(), ncc(plane, &patch, q, a).to_bits(), "level {lvl} {a:?} ({i}, {j})");
            }
        }
    }
}

#[test]
fn lk_refine_with_a_gradient_window_equals_the_full_gradients() {
    let pose = Pose { c: [100.0, 90.0], rot: 5.0, scale: 1.05 };
    let (img0, img1) = (frame(200, 180, pose, 30.0, 0.02, 1, None), frame(200, 180, Pose { c: [103.2, 88.1], rot: 6.5, scale: 1.08 }, 30.0, 0.02, 2, None));
    let (opts, r) = (TrackOptions::default(), [24, 24]);
    let (c0, a0) = ([100.0, 90.0], [1.0, 0.0]);
    let tpl = Crop::new(&Frame::new(&img0), &opts, c0, [60.0, 60.0], 0);
    let crop = Crop::new(&Frame::new(&img1), &opts, c0, [60.0, 60.0], 0);
    let plane = &crop.levels[0];
    let patch = Patch::sample(&tpl.levels[0], tpl.to_level(c0, 0), a0, r);
    let mid = crop.to_level(c0, 0);
    // Starts within the window, and far enough off that the iterations leave it (even near the edge).
    for (d, edge) in [([0.0, 0.0], false), ([1.3, -0.8], false), ([4.0, 5.0], false), ([9.0, -9.0], false), ([0.0, 0.0], true)] {
        let start = if edge { [4.0, 4.0] } else { [mid[0] + d[0], mid[1] + d[1]] };
        for shape in [true, false] {
            let (mut full, mut win) = (Grad::full(plane), Grad::around(plane, start, a0, r));
            let x = lk_refine(&crop, &mut full, &patch, start, a0, shape);
            let y = lk_refine(&crop, &mut win, &patch, start, a0, shape);
            assert_eq!(bits(&x), bits(&y), "{d:?} edge {edge} shape {shape}");
            // A window that misses the taps altogether is replaced by the full gradients.
            let mut off = Grad::around(plane, [start[0] + 15.0, start[1] + 15.0], a0, r);
            assert_eq!(bits(&x), bits(&lk_refine(&crop, &mut off, &patch, start, a0, shape)), "{d:?} edge {edge} shape {shape}: missed window");
        }
    }
}

#[test]
fn recovers_translation_path_subpixel() {
    let path = |f: usize| {
        let t = f as f64;
        Pose { c: [100.0 + 4.37 * t + 6.0 * (t * 0.3).sin(), 90.0 + 2.71 * t - 3.0 * (t * 0.21).cos()], rot: 0.0, scale: 1.0 }
    };
    let f0 = frame(320, 240, path(0), 40.0, 0.02, 0, None);
    let mut tr = Tracker::new(TrackOptions::default(), &[spec(path(0).c, 31.0, 71.0)], &Frame::new(&f0));
    let mut worst: f64 = 0.0;
    for f in 1..24 {
        let img = frame(320, 240, path(f), 40.0, 0.02, f as u32, None);
        let r = tr.step(&Frame::new(&img))[0];
        let e = dist(r.center, path(f).c);
        worst = worst.max(e);
        assert!(r.confidence > 90.0, "frame {f}: confidence {}", r.confidence);
        assert_eq!(r.status, Status::Tracked);
    }
    eprintln!("worst error {worst:.4} px");
    assert!(worst < 0.25, "worst error {worst} px");
}

#[test]
fn recovers_after_sudden_jumps_the_prediction_misses() {
    // Alternating large steps defeat the velocity prediction, so the fine stage must keep its
    // wider neighbourhood; smooth frames in between use the narrow one.
    let path = |f: usize| {
        let t = f as f64;
        let jump = [0.0, 7.0, -5.0, 9.0, -8.0, 6.0, -7.0, 8.0];
        let j = jump[f % jump.len()];
        Pose { c: [120.0 + 1.5 * t + j, 100.0 + 1.1 * t - 0.6 * j], rot: 0.0, scale: 1.0 }
    };
    let f0 = frame(360, 260, path(0), 40.0, 0.02, 0, None);
    let mut tr = Tracker::new(TrackOptions::default(), &[spec(path(0).c, 31.0, 91.0)], &Frame::new(&f0));
    for f in 1..24 {
        let img = frame(360, 260, path(f), 40.0, 0.02, f as u32, None);
        let r = tr.step(&Frame::new(&img))[0];
        let e = dist(r.center, path(f).c);
        assert!(e < 0.3, "frame {f}: error {e} px");
        assert_eq!(r.status, Status::Tracked);
    }
}

#[test]
fn layer_offset_and_channels() {
    let pose = |f: usize| Pose { c: [120.0 + 3.3 * f as f64, 100.0 - 1.7 * f as f64], rot: 0.0, scale: 1.0 };
    for channel in Channel::ALL {
        for (blur, enhance) in [(0.0, false), (3.0, false), (0.0, true)] {
            let opts = TrackOptions { channel, blur, enhance, ..Default::default() };
            let f0 = frame(240, 200, pose(0), 36.0, 0.01, 0, None);
            // Layer coordinates are image pixels minus the offset.
            let off = [10.0, -5.0];
            let c0 = [pose(0).c[0] - off[0], pose(0).c[1] - off[1]];
            let mut tr = Tracker::new(opts, &[spec(c0, 29.0, 61.0)], &Frame { img: &f0, offset: off });
            for f in 1..6 {
                let img = frame(240, 200, pose(f), 36.0, 0.01, f as u32, None);
                let r = tr.step(&Frame { img: &img, offset: off })[0];
                let want = [pose(f).c[0] - off[0], pose(f).c[1] - off[1]];
                assert!(dist(r.center, want) < 0.3, "{channel:?} blur {blur} enhance {enhance} frame {f}: {:?} vs {want:?}", r.center);
            }
        }
    }
}

#[test]
fn two_points_recover_rotation_and_scale() {
    let pose = |f: usize| Pose { c: [160.0 + 1.5 * f as f64, 120.0 + 0.8 * f as f64], rot: 1.8 * f as f64, scale: 1.0 + 0.01 * f as f64 };
    let u = [[-26.0, 0.0], [26.0, 0.0]];
    let p0 = pose(0);
    let f0 = frame(320, 240, p0, 52.0, 0.01, 0, None);
    let mut tr = Tracker::new(TrackOptions::default(), &u.map(|q| spec(p0.map(q), 23.0, 51.0)), &Frame::new(&f0));
    let (a0, l0) = angle_and_length(p0.map(u[0]), p0.map(u[1]));
    let mut worst: f64 = 0.0;
    for f in 1..20 {
        let p = pose(f);
        let img = frame(320, 240, p, 52.0, 0.01, f as u32, None);
        let r = tr.step(&Frame::new(&img));
        for k in 0..2 {
            worst = worst.max(dist(r[k].center, p.map(u[k])));
        }
        let (a, l) = angle_and_length(r[0].center, r[1].center);
        assert!((unwrap_degrees(0.0, a - a0) - p.rot).abs() < 0.5, "frame {f}: rotation {} vs {}", a - a0, p.rot);
        assert!((l / l0 - p.scale).abs() < 0.01, "frame {f}: scale {} vs {}", l / l0, p.scale);
        // The per-feature shape estimate follows too.
        assert!((r[0].rotation - p.rot).abs() < 1.0, "frame {f}: feature rotation {}", r[0].rotation);
        assert!((r[0].scale - p.scale).abs() < 0.02, "frame {f}: feature scale {}", r[0].scale);
    }
    eprintln!("worst error {worst:.4} px");
    assert!(worst < 0.25, "worst error {worst} px");
}

#[test]
fn confidence_drops_on_occlusion_and_actions() {
    let pose = |f: usize| Pose { c: [110.0 + 2.0 * f as f64, 100.0], rot: 0.0, scale: 1.0 };
    let occ = |f: usize| (f == 5).then(|| [pose(f).c[0] - 12.0, pose(f).c[1] - 30.0, 40.0, 60.0]);
    let run = |action: ConfidenceAction| {
        let f0 = frame(240, 200, pose(0), 36.0, 0.01, 0, None);
        let opts = TrackOptions { action, ..Default::default() };
        let mut tr = Tracker::new(opts, &[spec(pose(0).c, 31.0, 61.0)], &Frame::new(&f0));
        (1..9)
            .map(|f| {
                let img = frame(240, 200, pose(f), 36.0, 0.01, f as u32, occ(f));
                tr.step(&Frame::new(&img))[0]
            })
            .collect::<Vec<_>>()
    };
    let r = run(ConfidenceAction::Continue);
    assert!(r[3].confidence > 95.0, "{}", r[3].confidence);
    assert!(r[4].confidence < 70.0, "occluded frame confidence {}", r[4].confidence);
    let r = run(ConfidenceAction::Extrapolate);
    assert_eq!(r[4].status, Status::Extrapolated);
    assert!(dist(r[4].center, pose(5).c) < 0.5, "extrapolated {:?}", r[4].center);
    // Back on track after the occlusion.
    assert!(dist(r[6].center, pose(7).c) < 0.25 && r[6].status == Status::Tracked);
    let r = run(ConfidenceAction::Stop);
    assert_eq!(r[4].status, Status::Stopped);
    let r = run(ConfidenceAction::Adapt);
    assert_eq!(r[4].status, Status::Adapted);
}

#[test]
fn backward_and_whole_pixel() {
    let pose = |f: usize| Pose { c: [100.0 + 3.25 * f as f64, 100.0 + 1.5 * f as f64], rot: 0.0, scale: 1.0 };
    let last = 6;
    let fl = frame(240, 200, pose(last), 36.0, 0.0, 0, None);
    let opts = TrackOptions { subpixel: false, ..Default::default() };
    let mut tr = Tracker::new(opts, &[spec(pose(last).c, 31.0, 61.0)], &Frame::new(&fl));
    for f in (0..last).rev() {
        let img = frame(240, 200, pose(f), 36.0, 0.0, 0, None);
        let r = tr.step(&Frame::new(&img))[0];
        let want = pose(f).c;
        // Whole-pixel displacements of the start position.
        let d = [r.center[0] - pose(last).c[0], r.center[1] - pose(last).c[1]];
        assert!((d[0] - d[0].round()).abs() < 1e-9 && (d[1] - d[1].round()).abs() < 1e-9);
        assert!(dist(r.center, want) < 0.75, "frame {f}: {:?} vs {want:?}", r.center);
    }
}

/// 1080p, one point: tracking cost per frame (frames are generated outside the timing).
#[test]
fn performance_1080p_single_point() {
    let w = 1920;
    let h = 1080;
    // A cheap textured frame (the full synthetic renderer is slow at 1080p in debug builds).
    let make = |dx: f64, dy: f64| {
        let mut img = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let (fx, fy) = (x as f64 + 0.5 - dx, y as f64 + 0.5 - dy);
                let v = 0.5 + 0.2 * (fx * 0.31).sin() * (fy * 0.17).cos() + 0.15 * ((fx + fy) * 0.11).sin() + 0.1 * ((fx * 0.05).sin() * (fy * 0.23).sin());
                img.set(x, y, [v as f32, v as f32, v as f32, 1.0]);
            }
        }
        img
    };
    let frames: Vec<Image> = (0..6).map(|f| make(2.3 * f as f64, 1.1 * f as f64)).collect();
    let c0 = [960.0, 540.0];
    let s = PointSpec { center: c0, feature_size: [48.0, 48.0], search_offset: [0.0; 2], search_size: [128.0, 128.0] };
    let mut tr = Tracker::new(TrackOptions::default(), &[s], &Frame::new(&frames[0]));
    let mut last = PointResult { center: c0, confidence: 0.0, rotation: 0.0, scale: 1.0, status: Status::Tracked };
    // The fastest step: a loaded CI machine (the FreeBSD VM runs the whole suite on two cores)
    // stalls individual steps, which the best one doesn't see.
    let mut per = f64::INFINITY;
    for f in frames.iter().skip(1) {
        let t0 = std::time::Instant::now();
        last = tr.step(&Frame::new(f))[0];
        per = per.min(t0.elapsed().as_secs_f64());
    }
    eprintln!("1080p single-point track: {:.2} ms/frame ({:.0} fps)", per * 1e3, 1.0 / per);
    assert!(dist(last.center, [c0[0] + 2.3 * 5.0, c0[1] + 1.1 * 5.0]) < 0.25, "{:?}", last.center);
    // Catches an order-of-magnitude regression, not a slow runner.
    assert!(per < 0.25, "{per} s per frame");
}

#[test]
fn featureless_regions_stay_put() {
    let flat = Image::filled(200, 160, [0.3, 0.3, 0.3, 1.0]);
    for action in ConfidenceAction::ALL {
        let opts = TrackOptions { action, ..Default::default() };
        let mut tr = Tracker::new(opts, &[spec([100.0, 80.0], 21.0, 61.0)], &Frame::new(&flat));
        for _ in 0..10 {
            let r = tr.step(&Frame::new(&flat))[0];
            assert!(dist(r.center, [100.0, 80.0]) < 1e-9, "{action:?}: drifted to {:?}", r.center);
            assert!(r.confidence < 1.0);
        }
    }
}
