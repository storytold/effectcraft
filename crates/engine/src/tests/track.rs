//! Motion tracking through the commands: synthetic footage generated in-test (a textured patch on
//! a known path, or a whole frame under a known homography), analysis (blocking and background),
//! apply (Transform, Stabilize, Corner Pin), undo/redo and serde.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use effectcraft_color::Label;
use effectcraft_project::build;
use effectcraft_project::tracking::TrackKind;
use effectcraft_project::{Footage, FootageKind, ItemId, ItemKind, LayerId, LayerSource};
use effectcraft_raster::Image;
use effectcraft_render::{EvalCtx, FootageSource};
use effectcraft_time::{FrameRate, Tick};
use effectcraft_track::Homography;
use serde_json::json;

use crate::Session;

pub(crate) const W: u32 = 320;
pub(crate) const H: u32 = 240;
pub(crate) const FPS: u32 = 25;

pub(crate) fn texture(x: f64, y: f64, seed: u32) -> f32 {
    let mut v = 0.0f64;
    let mut s = seed.wrapping_mul(2654435761).wrapping_add(12345);
    let mut rnd = || {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        (s % 10_000) as f64 / 10_000.0
    };
    for _ in 0..6 {
        let ang = rnd() * std::f64::consts::TAU;
        let period = 9.0 + rnd() * 25.0;
        let phase = rnd() * std::f64::consts::TAU;
        let k = std::f64::consts::TAU / period;
        v += (k * (x * ang.cos() + y * ang.sin()) + phase).sin() * (0.4 + rnd()) * 0.1;
    }
    (0.5 + v).clamp(0.0, 1.0) as f32
}

/// Footage computed per frame (cached).
pub(crate) struct Synth {
    make: Box<dyn Fn(u32) -> Image + Send + Sync>,
    cache: Mutex<HashMap<u32, Arc<Image>>>,
}

impl FootageSource for Synth {
    fn frame(&self, _: ItemId, _: &Footage, t: Tick) -> Option<Arc<Image>> {
        let f = (t.seconds() * FPS as f64).round().max(0.0) as u32;
        let mut c = self.cache.lock().unwrap();
        Some(c.entry(f).or_insert_with(|| Arc::new((self.make)(f))).clone())
    }
}

/// Patch pose on frame `f`: centre, rotation (degrees), scale.
pub(crate) type Pose = fn(u32) -> ([f64; 2], f64, f64);

pub(crate) fn patch_frames(pose: Pose) -> impl Fn(u32) -> Image + Send + Sync {
    move |f| {
        let (c, rot, sc) = pose(f);
        let (sn, cs) = rot.to_radians().sin_cos();
        let mut img = Image::new(W, H);
        for y in 0..H {
            for x in 0..W {
                let p = [x as f64 + 0.5, y as f64 + 0.5];
                let d = [(p[0] - c[0]) / sc, (p[1] - c[1]) / sc];
                let l = [cs * d[0] + sn * d[1], -sn * d[0] + cs * d[1]];
                let v = if l[0].abs() <= 50.0 && l[1].abs() <= 50.0 { texture(l[0], l[1], 7) } else { 0.6 * texture(p[0] * 0.7, p[1] * 0.7, 3) };
                img.set(x, y, [v, v, v, 1.0]);
            }
        }
        img
    }
}

/// A comp with the synthetic clip (layer `clip`) and a 100 × 100 solid above it (layer `solid`).
pub(crate) fn setup(make: impl Fn(u32) -> Image + Send + Sync + 'static) -> (Session, LayerId, LayerId) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Track", "width": W, "height": H, "frameRate": FPS, "duration": 1})).unwrap();
    let cid = s.active_comp_id().unwrap();
    let clip = s
        .edit("clip", None, |p, _| {
            let fr = FrameRate::new(FPS as i64, 1);
            let footage = Footage {
                path: "synthetic.mov".into(),
                kind: FootageKind::Video,
                width: W,
                height: H,
                pixel_aspect: 1.0,
                frame_rate: fr,
                native_rate: None,
                duration: Tick::from_seconds_f64(1.0),
                has_video: true,
                has_audio: false,
                alpha: Default::default(),
                premul_color: [0.0; 3],
                loop_count: 1,
                codec: String::new(),
                missing: false,
                sequence: vec![],
                color_profile: None,
                ..Default::default()
            };
            let item = p.add_item("clip", Label::Aqua, None, ItemKind::Footage(footage));
            let comp = p.comp(cid).unwrap().clone();
            let l = build::layer(p, &comp, "Clip", LayerSource::Footage { item }, (W, H), None);
            let id = l.id;
            p.comp_mut(cid).unwrap().layers.insert(0, l);
            Ok(id)
        })
        .unwrap();
    s.footage = Arc::new(Synth { make: Box::new(make), cache: Mutex::default() });
    let solid = s.execute("layer.newSolid", json!({"width": 100, "height": 100, "color": "#ff0000"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.select", json!({"layers": [clip.0]})).unwrap();
    (s, clip, LayerId(solid))
}

fn tracker_points(s: &Session, layer: LayerId) -> Vec<effectcraft_project::PropGroup> {
    let l = s.active_comp().unwrap().layer(layer).unwrap();
    l.trackers().next().unwrap().0.track_points().cloned().collect()
}

pub(crate) fn frame_time(f: u32) -> Tick {
    FrameRate::new(FPS as i64, 1).tick_of(f as i64)
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

fn translate_pose(f: u32) -> ([f64; 2], f64, f64) {
    let t = f as f64;
    ([150.0 + 2.37 * t + 4.0 * (t * 0.4).sin(), 110.0 + 1.13 * t], 0.0, 1.0)
}

#[test]
fn track_motion_analyze_apply_transform_undo_redo() {
    let (mut s, clip, solid) = setup(patch_frames(translate_pose));
    // The menu entry and the Tracker panel's Track Motion button.
    assert!(s.is_enabled("track.motion"));
    let r = s.execute("track.motion", json!({})).unwrap();
    assert_eq!(r["points"], 1);
    let st = s.execute("track.status", json!({})).unwrap();
    assert_eq!(st["tracker"]["settings"]["target"], json!(solid.0), "{st}");
    // Put the track point on the patch and analyze to the end (blocking).
    s.execute("track.setPoint", json!({"point": 1, "center": translate_pose(0).0, "featureSize": [36, 36], "searchSize": [80, 80]})).unwrap();
    let r = s.execute("track.analyze", json!({"direction": "forward", "wait": true})).unwrap();
    assert_eq!(r["frames"], 24);
    s.poll_track();
    assert!(s.track_job.is_none());
    let tp = &tracker_points(&s, clip)[0];
    let fc = tp.get("featureCenter").unwrap();
    assert_eq!(fc.keys.len(), 25);
    let conf = tp.get("confidence").unwrap();
    let mut worst: f64 = 0.0;
    for f in 0..25 {
        let t = frame_time(f);
        worst = worst.max(dist(fc.value_at(t).as_vec2(), translate_pose(f).0));
        assert!(conf.value_at(t).as_f64() > 90.0);
    }
    assert!(worst < 0.25, "worst {worst}");
    // CTI moved to the last analysed frame.
    assert_eq!(s.time(), frame_time(24));
    // Apply: the solid's position follows the attach point.
    s.execute("track.apply", json!({})).unwrap();
    let pos = s.active_comp().unwrap().layer(solid).unwrap().props.prop("transform/position").unwrap().clone();
    assert_eq!(pos.keys.len(), 25);
    for f in [0, 7, 24] {
        let v = pos.value_at(frame_time(f)).as_vec2();
        assert!(dist(v, translate_pose(f).0) < 0.25, "frame {f}: {v:?}");
    }
    // Undo the apply, then the analysis; redo both.
    assert!(s.undo());
    assert!(s.active_comp().unwrap().layer(solid).unwrap().props.prop("transform/position").unwrap().keys.is_empty());
    assert!(s.undo());
    assert!(tracker_points(&s, clip)[0].get("featureCenter").unwrap().keys.is_empty());
    assert!(s.redo());
    assert_eq!(tracker_points(&s, clip)[0].get("featureCenter").unwrap().keys.len(), 25);
    assert!(s.redo());
    assert_eq!(s.active_comp().unwrap().layer(solid).unwrap().props.prop("transform/position").unwrap().keys.len(), 25);
    // X only keeps the solid's Y.
    s.undo();
    s.execute("track.apply", json!({"dimensions": "x"})).unwrap();
    let pos = s.active_comp().unwrap().layer(solid).unwrap().props.prop("transform/position").unwrap().clone();
    let v = pos.value_at(frame_time(10)).as_vec2();
    assert!((v[0] - translate_pose(10).0[0]).abs() < 0.25 && (v[1] - 120.0).abs() < 1e-9, "{v:?}");
    // Serde round trip keeps the tracker.
    let json = s.project.to_json();
    let back = effectcraft_project::Project::from_json(&json).unwrap();
    let l = back.comp(s.active_comp_id().unwrap()).unwrap().layer(clip).unwrap();
    let (g, st) = l.trackers().next().unwrap();
    assert_eq!(st.kind, TrackKind::Transform);
    assert_eq!(g.track_points().next().unwrap().get("featureCenter").unwrap().keys.len(), 25);
    assert_eq!(l.props.children[0].match_id(), "motionTrackers");
}

#[test]
fn stabilize_keeps_the_feature_fixed() {
    let (mut s, clip, _) = setup(patch_frames(translate_pose));
    s.execute("track.stabilize", json!({})).unwrap();
    s.execute("track.setPoint", json!({"point": 1, "center": translate_pose(0).0, "featureSize": [36, 36], "searchSize": [80, 80]})).unwrap();
    s.execute("time.set", json!({"time": 0})).unwrap();
    s.execute("track.analyze", json!({"wait": true})).unwrap();
    s.execute("track.apply", json!({})).unwrap();
    let cid = s.active_comp_id().unwrap();
    let comp = s.project.comp(cid).unwrap().clone();
    let l = comp.layer(clip).unwrap();
    let at = |f: u32| {
        let ctx = EvalCtx::new(&s.project, cid, &comp, frame_time(f));
        let (m, _) = ctx.layer_to_comp(l);
        let c = translate_pose(f).0;
        let q = m.apply(effectcraft_geom::vec2(c[0], c[1]));
        [q.x, q.y]
    };
    let p0 = at(0);
    // The layer itself didn't move on the first frame.
    assert!(dist(p0, translate_pose(0).0) < 1e-6, "{p0:?}");
    let worst = (1..25).map(|f| dist(at(f), p0)).fold(0.0, f64::max);
    assert!(worst < 0.25, "feature drifts by {worst} px");
}

pub(crate) fn rotating_pose(f: u32) -> ([f64; 2], f64, f64) {
    let t = f as f64;
    ([160.0 + 1.2 * t, 120.0 - 0.7 * t], 1.5 * t, 1.0 + 0.008 * t)
}

#[test]
fn transform_rotation_scale_and_background_job() {
    let (mut s, clip, solid) = setup(patch_frames(rotating_pose));
    s.execute("track.new", json!({"kind": "transform", "rotation": true, "scale": true})).unwrap();
    assert_eq!(tracker_points(&s, clip).len(), 2);
    let (c, _, _) = rotating_pose(0);
    s.execute("track.setPoint", json!({"point": 1, "center": [c[0] - 26.0, c[1]], "featureSize": [26, 26], "searchSize": [56, 56]})).unwrap();
    s.execute("track.setPoint", json!({"point": 2, "center": [c[0] + 26.0, c[1]], "featureSize": [26, 26], "searchSize": [56, 56]})).unwrap();
    // Background analysis with progress, polled like the UI does.
    let r = s.execute("track.analyze", json!({"direction": "forward", "end": 0.6})).unwrap();
    assert_eq!(r["frames"], 15);
    let t0 = std::time::Instant::now();
    while s.is_tracking() {
        assert!(t0.elapsed().as_secs() < 120);
        std::thread::sleep(std::time::Duration::from_millis(5));
        s.poll_track();
    }
    s.poll_track();
    let st = s.execute("track.status", json!({})).unwrap();
    assert_eq!(st["running"], false);
    assert_eq!(st["tracker"]["points"][0]["keys"], 16);
    s.execute("track.apply", json!({})).unwrap();
    let tr = s.active_comp().unwrap().layer(solid).unwrap().props.sub("transform").unwrap().clone();
    let rot = tr.get("rotation").unwrap();
    let sc = tr.get("scale").unwrap();
    for f in [5, 10, 15] {
        let (_, r, k) = rotating_pose(f);
        let got = rot.value_at(frame_time(f)).as_f64();
        assert!((got - r).abs() < 0.3, "frame {f}: rotation {got} vs {r}");
        let gs = sc.value_at(frame_time(f)).as_vec3()[0];
        assert!((gs - 100.0 * k).abs() < 0.5, "frame {f}: scale {gs} vs {}", 100.0 * k);
    }
}

#[test]
fn perspective_corner_pin_follows_a_known_homography() {
    // Frame f shows the texture under H_f (identity → `end` over 12 frames).
    let end = Homography([[1.04, 0.05, 6.0], [-0.03, 0.97, 4.0], [0.00012, -0.00008, 1.0]]);
    let hf = move |f: u32| {
        let k = f as f64 / 12.0;
        let mut m = [[0.0; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                let id = if i == j { 1.0 } else { 0.0 };
                m[i][j] = id + (end.0[i][j] - id) * k;
            }
        }
        Homography(m)
    };
    let inv = move |f: u32, p: [f64; 2]| {
        let h = hf(f);
        // Solve H x = p by mapping four points back (exact for a homography).
        let src = [[0.0, 0.0], [W as f64, 0.0], [0.0, H as f64], [W as f64, H as f64]];
        let dst = src.map(|q| h.apply(q));
        Homography::from_points(dst, src).unwrap().apply(p)
    };
    let make = move |f: u32| {
        let h = hf(f);
        let src = [[0.0, 0.0], [W as f64, 0.0], [0.0, H as f64], [W as f64, H as f64]];
        let back = Homography::from_points(src.map(|q| h.apply(q)), src).unwrap();
        let _ = inv;
        let mut img = Image::new(W, H);
        for y in 0..H {
            for x in 0..W {
                let q = back.apply([x as f64 + 0.5, y as f64 + 0.5]);
                let v = texture(q[0], q[1], 11);
                img.set(x, y, [v, v, v, 1.0]);
            }
        }
        img
    };
    let (mut s, clip, solid) = setup(make);
    s.execute("track.new", json!({"kind": "perspective"})).unwrap();
    let corners = [[90.0, 70.0], [230.0, 70.0], [90.0, 170.0], [230.0, 170.0]];
    for (i, c) in corners.iter().enumerate() {
        s.execute("track.setPoint", json!({"point": i + 1, "center": c, "featureSize": [30, 30], "searchSize": [60, 60]})).unwrap();
    }
    s.execute("track.analyze", json!({"wait": true, "end": 12.0 / FPS as f64})).unwrap();
    let pts = tracker_points(&s, clip);
    for (i, tp) in pts.iter().enumerate() {
        let got = tp.get("featureCenter").unwrap().value_at(frame_time(12)).as_vec2();
        assert!(dist(got, end.apply(corners[i])) < 0.3, "point {i}: {got:?} vs {:?}", end.apply(corners[i]));
    }
    s.execute("track.apply", json!({})).unwrap();
    let l = s.active_comp().unwrap().layer(solid).unwrap().clone();
    let fx = l.effects().unwrap().groups().next().unwrap().clone();
    assert_eq!(fx.match_id, "ec.distort.cornerpin");
    // The solid is 100 × 100, centred: comp = layer + (110, 70).
    for (m, i) in [("ul", 0), ("ur", 1), ("ll", 2), ("lr", 3)] {
        let p = fx.get(m).unwrap();
        assert_eq!(p.keys.len(), 13);
        for f in [6, 12] {
            let want = hf(f).apply(corners[i]);
            let got = p.value_at(frame_time(f)).as_vec2();
            assert!(dist([got[0] + 110.0, got[1] + 70.0], want) < 0.35, "{m} frame {f}: {got:?} vs {want:?}");
        }
    }
}

#[test]
fn backward_frame_steps_options_and_reset() {
    let (mut s, clip, _) = setup(patch_frames(translate_pose));
    s.execute("track.motion", json!({})).unwrap();
    s.execute("time.set", json!({"time": 10.0 / FPS as f64})).unwrap();
    s.execute("track.setPoint", json!({"point": 1, "center": translate_pose(10).0, "featureSize": [36, 36], "searchSize": [80, 80]})).unwrap();
    s.execute("track.options", json!({"channel": "rgb", "blur": 2, "threshold": 70, "action": "extrapolate", "name": "Patch"})).unwrap();
    let st = s.execute("track.status", json!({})).unwrap();
    assert_eq!(st["tracker"]["name"], "Patch");
    assert_eq!(st["tracker"]["settings"]["options"]["channel"], "rgb");
    s.execute("track.analyze", json!({"direction": "frameBackward", "wait": true})).unwrap();
    s.execute("track.analyze", json!({"direction": "backward", "wait": true})).unwrap();
    assert_eq!(s.time(), Tick::ZERO);
    s.execute("track.analyze", json!({"direction": "frameForward", "wait": true, "start": 10.0 / FPS as f64})).unwrap();
    let fc = tracker_points(&s, clip)[0].get("featureCenter").unwrap().clone();
    assert_eq!(fc.keys.len(), 12);
    for f in 0..12 {
        assert!(dist(fc.value_at(frame_time(f)).as_vec2(), translate_pose(f).0) < 0.25, "frame {f}");
    }
    // Unknown params are rejected for agents.
    assert!(s.execute_checked("track.analyze", json!({"dir": "forward"})).is_err());
    s.execute("track.setType", json!({"kind": "perspective"})).unwrap();
    assert_eq!(tracker_points(&s, clip).len(), 4);
    s.execute("track.setType", json!({"kind": "transform", "rotation": false})).unwrap();
    assert_eq!(tracker_points(&s, clip).len(), 1);
    s.execute("track.reset", json!({})).unwrap();
    assert!(tracker_points(&s, clip)[0].get("featureCenter").unwrap().keys.is_empty());
    s.execute("track.delete", json!({})).unwrap();
    assert!(s.active_comp().unwrap().layer(clip).unwrap().motion_trackers().is_none());
    assert!(s.execute("track.analyze", json!({})).is_err());
}

/// End-to-end analysis speed at 1080p for one track point: footage frames through the renderer's
/// layer source and the tracker (frames are prepared before timing).
#[test]
fn analyze_1080p_single_point_speed() {
    let (w, h) = (1920u32, 1080u32);
    let mut base = Image::new(w + 64, h + 64);
    for y in 0..base.height {
        for x in 0..base.width {
            let v = 0.5 + 0.25 * ((x as f32 * 0.21).sin() * (y as f32 * 0.13).cos()) + 0.15 * ((x + 2 * y) as f32 * 0.07).sin();
            base.set(x, y, [v, v, v, 1.0]);
        }
    }
    let frames: Vec<Arc<Image>> = (0..31u32)
        .map(|f| {
            let (dx, dy) = (f as i64, (f / 2) as i64);
            let mut img = Image::new(w, h);
            for y in 0..h {
                for x in 0..w {
                    img.set(x, y, base.get(x as i64 + 32 - dx, y as i64 + 32 - dy));
                }
            }
            Arc::new(img)
        })
        .collect();
    struct Pre(Vec<Arc<Image>>);
    impl FootageSource for Pre {
        fn frame(&self, _: ItemId, _: &Footage, t: Tick) -> Option<Arc<Image>> {
            let f = (t.seconds() * FPS as f64).round().max(0.0) as usize;
            self.0.get(f.min(self.0.len() - 1)).cloned()
        }
    }
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "HD", "width": w, "height": h, "frameRate": FPS, "duration": 2})).unwrap();
    let cid = s.active_comp_id().unwrap();
    let clip = s
        .edit("clip", None, |p, _| {
            let footage = Footage {
                path: "hd.mov".into(),
                kind: FootageKind::Video,
                width: w,
                height: h,
                pixel_aspect: 1.0,
                frame_rate: FrameRate::new(FPS as i64, 1),
                native_rate: None,
                duration: Tick::from_seconds_f64(2.0),
                has_video: true,
                has_audio: false,
                alpha: Default::default(),
                premul_color: [0.0; 3],
                loop_count: 1,
                codec: String::new(),
                missing: false,
                sequence: vec![],
                color_profile: None,
                ..Default::default()
            };
            let item = p.add_item("hd", Label::Aqua, None, ItemKind::Footage(footage));
            let comp = p.comp(cid).unwrap().clone();
            let l = build::layer(p, &comp, "HD", LayerSource::Footage { item }, (w, h), None);
            let id = l.id;
            p.comp_mut(cid).unwrap().layers.insert(0, l);
            Ok(id)
        })
        .unwrap();
    s.footage = Arc::new(Pre(frames));
    s.execute("track.motion", json!({"layer": clip.0})).unwrap();
    s.execute("track.setPoint", json!({"point": 1, "center": [960, 540], "featureSize": [48, 48], "searchSize": [128, 128]})).unwrap();
    let t0 = std::time::Instant::now();
    s.execute("track.analyze", json!({"wait": true, "end": 30.0 / FPS as f64})).unwrap();
    let secs = t0.elapsed().as_secs_f64();
    let fps = 30.0 / secs;
    eprintln!("1080p single-point analysis: {fps:.0} frames/s end to end ({:.1} ms/frame)", secs * 1e3 / 30.0);
    let fc = tracker_points(&s, clip)[0].get("featureCenter").unwrap().clone();
    assert!(dist(fc.value_at(frame_time(30)).as_vec2(), [990.0, 555.0]) < 0.25);
    assert!(fps > 30.0, "{fps} frames/s");
}
