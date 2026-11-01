//! Roto Brush & Refine Edge end to end through the commands, on synthetic footage: a textured
//! disk moving over a textured background.

use effectcraft_project::LayerId;
use effectcraft_raster::Image;
use effectcraft_render::RenderOpts;
use effectcraft_track::roto::rle;
use serde_json::{Value, json};

use crate::Session;
use crate::tests::track::{H, W, frame_time, setup};

/// Keep cache/model mutation tests from clearing segmentations under another fixture.
/// Libtest runs each test on its own thread; the guard lives until that thread exits.
pub(crate) fn hold_roto_cache_test_lock() {
    use std::cell::RefCell;
    use std::sync::{Mutex, MutexGuard, PoisonError};
    static LOCK: Mutex<()> = Mutex::new(());
    thread_local! {
        static HELD: RefCell<Option<MutexGuard<'static, ()>>> = const { RefCell::new(None) };
    }
    HELD.with(|h| {
        if h.borrow().is_none() {
            *h.borrow_mut() = Some(LOCK.lock().unwrap_or_else(PoisonError::into_inner));
        }
    });
}
const R: f64 = 40.0;

fn span_after_trimming_strokes(trim: Value, span: Value, expected: [i64; 2]) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"width": 64, "height": 64, "frameRate": 25, "duration": 1})).unwrap();
    let layer = s.execute("layer.newSolid", json!({"width": 64, "height": 64})).unwrap()["layer"].as_u64().unwrap();
    for frame in [0, 20] {
        s.execute("roto.stroke", json!({"layer": layer, "frame": frame, "points": [[32, 32]], "radius": 4})).unwrap();
    }
    s.execute("layer.timing", trim).unwrap();
    let before = s.project.to_json();
    let error = s.execute("roto.span", span.clone()).unwrap_err().to_string();
    assert!(error.contains("strokes outside the layer"), "{error}");
    assert_eq!(s.project.to_json(), before, "refusing the span must preserve strokes and project state");
    assert!(s.undo(), "the failed command must not add an undo step");
    let result = s.execute("roto.span", span).unwrap();
    assert_eq!(result["span"], json!(expected));
    let status = s.execute("roto.status", json!({"layer": layer})).unwrap();
    assert_eq!(status["strokeFrames"], json!([0, 20]));
}

#[test]
fn roto_span_rejects_strokes_before_trimmed_in_point() {
    span_after_trimming_strokes(json!({"in": 0.2}), json!({"start": 0}), [0, 20]);
}

#[test]
fn roto_span_rejects_strokes_after_trimmed_out_point() {
    span_after_trimming_strokes(json!({"out": 0.5}), json!({"end": 24}), [0, 24]);
}

fn noise(x: f64, y: f64, seed: u32) -> f64 {
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

/// Disk centre on frame `f`.
fn center(f: u32) -> [f64; 2] {
    let t = f as f64;
    [90.0 + 4.0 * t, 110.0 + 1.5 * t + 5.0 * (t * 0.3).sin()]
}

/// Disk centre with a jump at frame 8 (propagation loses it there).
fn jumping(f: u32) -> [f64; 2] {
    if f < 8 { center(f) } else { [230.0 + 2.0 * (f - 8) as f64, 150.0] }
}

fn draw(c: [f64; 2]) -> Image {
    let mut img = Image::new(W, H);
    for y in 0..H {
        for x in 0..W {
            let p = [x as f64 + 0.5, y as f64 + 0.5];
            let d = (p[0] - c[0]).hypot(p[1] - c[1]);
            let a = (R + 0.5 - d).clamp(0.0, 1.0) as f32;
            let n = noise(p[0] - c[0], p[1] - c[1], 11);
            let f = [(0.75 + 0.25 * n) as f32, (0.35 + 0.3 * n) as f32, (0.1 + 0.15 * n) as f32];
            let (bn, bm) = (noise(p[0], p[1], 5), noise(p[0] * 1.7, p[1] * 1.7, 9));
            let b = [(0.05 + 0.25 * bm) as f32, (0.3 + 0.4 * bn) as f32, (0.45 + 0.4 * bm) as f32];
            img.set(x, y, [f[0] * a + b[0] * (1.0 - a), f[1] * a + b[1] * (1.0 - a), f[2] * a + b[2] * (1.0 - a), 1.0]);
        }
    }
    img
}

fn gt(c: [f64; 2]) -> String {
    let m: Vec<u8> = (0..W * H).map(|i| (((i % W) as f64 + 0.5 - c[0]).hypot((i / W) as f64 + 0.5 - c[1]) <= R) as u8).collect();
    rle::encode(&m)
}

fn line(a: [f64; 2], b: [f64; 2]) -> Value {
    json!((0..=8).map(|k| [a[0] + (b[0] - a[0]) * k as f64 / 8.0, a[1] + (b[1] - a[1]) * k as f64 / 8.0]).collect::<Vec<_>>())
}

/// A session with the clip (the Track solid removed) and foreground / background strokes on
/// frame 0. `name`: the footage file name (segmentations are cached process-wide by content key,
/// which includes the footage's identity: different clips need different names).
fn painted(path: fn(u32) -> [f64; 2], name: &str) -> (Session, LayerId, u64) {
    hold_roto_cache_test_lock();
    let (mut s, clip, solid) = setup(move |f| draw(path(f)));
    let cid = s.active_comp_id().unwrap();
    s.edit("remove solid", None, |p, _| {
        p.comp_mut(cid).unwrap().layers.retain(|l| l.id != solid);
        for it in p.items.values_mut() {
            if let effectcraft_project::ItemKind::Footage(f) = &mut it.kind {
                f.path = name.into();
            }
        }
        Ok(())
    })
    .unwrap();
    s.execute("time.set", json!({"time": 0})).unwrap();
    let c = path(0);
    let r =
        s.execute_checked("roto.stroke", json!({"layer": clip.0, "points": line([c[0] - 22.0, c[1] - 8.0], [c[0] + 22.0, c[1] + 8.0]), "radius": 6})).unwrap();
    let uid = r["effect"].as_u64().unwrap();
    assert_eq!(r["base"], 0);
    assert_eq!(r["span"], json!([0, 20]));
    for (a, b) in [([c[0] - 60.0, c[1] - 58.0], [c[0] + 70.0, c[1] - 58.0]), ([c[0] + 70.0, c[1] + 58.0], [c[0] - 60.0, c[1] + 58.0])] {
        s.execute_checked("roto.stroke", json!({"layer": clip.0, "kind": "bg", "points": line(a, b), "radius": 6})).unwrap();
    }
    (s, clip, uid)
}

/// Drop an instance's derived segmentations from the process-wide cache.
fn forget(s: &Session, clip: LayerId, uid: u64) {
    let comp = s.active_comp().unwrap();
    let l = comp.layer(clip).unwrap();
    let g = l.props.find_group(uid).unwrap();
    let size = crate::roto::layer_size(&s.project, comp, l);
    let ch = effectcraft_effects::roto::Chain::new(&crate::roto::params_static(g), size, comp.frame_rate.as_f64(), 1.0);
    effectcraft_effects::roto::forget(ch.keys.values().copied());
}

fn iou_at(s: &mut Session, clip: LayerId, f: u32, c: [f64; 2]) -> f64 {
    let st = s.execute("roto.status", json!({"layer": clip.0, "frame": f, "compute": true, "compareTo": gt(c)})).unwrap();
    st["iou"].as_f64().unwrap_or_else(|| panic!("no matte on frame {f}: {st}"))
}

fn alpha_at(s: &Session, f: u32) -> Vec<f32> {
    let img = s.render(s.active_comp_id().unwrap(), frame_time(f), RenderOpts { scale: 1.0, ..Default::default() });
    img.data.iter().map(|p| p[3]).collect()
}

/// Green, then Alt (red) over part of the object, then green there again: the last stroke wins,
/// in the render and through Undo / Redo (#1646).
#[test]
fn roto_brush_strokes_correct_earlier_ones() {
    let (mut s, clip, _) = painted(center, "disk-1646.mov");
    let c = center(0);
    let spot = [c[0], c[1] + 25.0];
    let i = spot[1] as usize * W as usize + spot[0] as usize;
    let paint = |s: &mut Session, kind: &str| {
        s.execute_checked(
            "roto.stroke",
            json!({"layer": clip.0, "kind": kind, "points": line([spot[0] - 6.0, spot[1]], [spot[0] + 6.0, spot[1]]), "radius": 7}),
        )
        .unwrap();
    };
    assert!(alpha_at(&s, 0)[i] > 0.5, "the spot starts inside the matte");
    paint(&mut s, "bg");
    assert!(alpha_at(&s, 0)[i] < 0.5, "Alt removes it");
    paint(&mut s, "fg");
    assert!(alpha_at(&s, 0)[i] > 0.5, "green over the Alt stroke restores it");
    assert!(iou_at(&mut s, clip, 0, c) > 0.95);
    assert!(s.undo());
    assert!(alpha_at(&s, 0)[i] < 0.5, "undo brings the removal back");
    assert!(s.redo());
    assert!(alpha_at(&s, 0)[i] > 0.5, "redo restores it again");
}

#[test]
fn roto_brush_segments_and_propagates() {
    let (mut s, clip, uid) = painted(center, "disk-a.mov");
    let l = s.active_comp().unwrap().layer(clip).unwrap().clone();
    assert_eq!(l.effects().unwrap().groups().filter(|g| crate::roto::is_roto(g)).count(), 1, "one effect for all strokes");
    let j0 = iou_at(&mut s, clip, 0, center(0));
    assert!(j0 > 0.95, "base frame IoU {j0}");
    // Propagate the span (blocking).
    let r = s.execute_checked("roto.propagate", json!({"layer": clip.0, "direction": "both", "wait": true})).unwrap();
    assert_eq!(r["frames"], 21, "{r}");
    assert_eq!(r["status"]["computed"].as_array().unwrap().len(), 21);
    let mut worst: f64 = 1.0;
    for f in 0..=20 {
        worst = worst.min(iou_at(&mut s, clip, f, center(f)));
    }
    assert!(worst > 0.9, "worst IoU over the span {worst}");
    // The render applies the matte; outside the span the layer is transparent.
    let a = alpha_at(&s, 12);
    let c = center(12);
    let mut wrong = 0;
    for (i, v) in a.iter().enumerate() {
        let d = (((i as u32 % W) as f64 + 0.5 - c[0]).hypot((i as u32 / W) as f64 + 0.5 - c[1])) - R;
        if (d < -3.0 && *v < 0.5) || (d > 3.0 && *v > 0.5) {
            wrong += 1;
        }
    }
    assert!(wrong < 150, "{wrong} wrong pixels in the render");
    assert!(alpha_at(&s, 23).iter().all(|v| *v == 0.0));
    // Extending the span propagates further.
    let r = s.execute("roto.propagate", json!({"layer": clip.0, "direction": "forward", "to": 24, "wait": true})).unwrap();
    assert_eq!(r["status"]["span"], json!([0, 24]));
    assert!(iou_at(&mut s, clip, 24, center(24)) > 0.9);
    // Undo / redo of strokes and the span.
    let n = |s: &Session| {
        let st = s.active_comp().unwrap().layer(clip).unwrap().props.find_group(uid).unwrap().get(effectcraft_effects::roto::STROKES).unwrap().value.clone();
        match st {
            effectcraft_keyframe::Value::Str(j) => effectcraft_track::roto::RotoData::from_json(&j),
            _ => panic!(),
        }
    };
    assert_eq!(n(&s).span, [0, 24]);
    assert!(s.undo());
    assert_eq!(n(&s).span, [0, 20]);
    assert_eq!(n(&s).strokes.len(), 3);
    assert!(s.undo());
    assert_eq!(n(&s).strokes.len(), 2);
    assert!(s.redo());
    assert!(s.redo());
    assert_eq!(n(&s).strokes.len(), 3);
    assert_eq!(n(&s).span, [0, 24]);
    // Serde keeps the strokes; the reopened project renders the same matte.
    let back = effectcraft_project::Project::from_json(&s.project.to_json()).unwrap();
    let g = back.comp(s.active_comp_id().unwrap()).unwrap().layer(clip).unwrap().props.find_group(uid).unwrap().clone();
    assert_eq!(
        crate::roto::params_static(&g).get("strokes"),
        crate::roto::params_static(s.active_comp().unwrap().layer(clip).unwrap().props.find_group(uid).unwrap()).get("strokes")
    );
    // Bad parameters are reported.
    assert!(s.execute_checked("roto.stroke", json!({"layer": clip.0, "kind": "sideways", "points": [[1, 1]]})).is_err());
    assert!(s.execute_checked("roto.propagate", json!({"layer": clip.0, "bogus": 1})).is_err());
    assert!(s.execute_checked("roto.stroke", json!({"layer": clip.0, "points": [[1, 1]], "frame": 99})).is_err());
}

#[test]
fn correction_strokes_restart_propagation() {
    let (mut s, clip, _) = painted(jumping, "disk-jump.mov");
    s.execute("roto.propagate", json!({"layer": clip.0, "wait": true})).unwrap();
    assert!(iou_at(&mut s, clip, 6, jumping(6)) > 0.9);
    let bad = iou_at(&mut s, clip, 8, jumping(8));
    assert!(bad < 0.5, "the jump should break propagation ({bad})");
    // Correct frame 8: paint the disk in and its old place out.
    let (c, old) = (jumping(8), center(8));
    s.execute("roto.stroke", json!({"layer": clip.0, "frame": 8, "points": line([c[0] - 20.0, c[1]], [c[0] + 20.0, c[1]]), "radius": 6})).unwrap();
    s.execute(
        "roto.stroke",
        json!({"layer": clip.0, "frame": 8, "kind": "bg", "points": line([old[0] - 15.0, old[1] - 15.0], [old[0] + 15.0, old[1] + 15.0]), "radius": 8}),
    )
    .unwrap();
    let st = s.execute("roto.status", json!({"layer": clip.0})).unwrap();
    let computed: Vec<i64> = serde_json::from_value(st["computed"].clone()).unwrap();
    assert!(computed.contains(&7) && !computed.contains(&8) && !computed.contains(&9), "frames from the correction on are recomputed: {computed:?}");
    assert_eq!(st["strokeFrames"], json!([0, 8]));
    s.execute("roto.propagate", json!({"layer": clip.0, "direction": "forward", "wait": true})).unwrap();
    for f in 8..=20 {
        let j = iou_at(&mut s, clip, f, jumping(f));
        assert!(j > 0.9, "frame {f}: IoU {j}");
    }
    // Clearing the correction brings the bad frame back.
    let r = s.execute("roto.clearStrokes", json!({"layer": clip.0, "frame": 8})).unwrap();
    assert_eq!(r["removed"], 2);
    assert!(iou_at(&mut s, clip, 8, jumping(8)) < 0.5);
}

#[test]
fn freeze_caches_the_mattes() {
    let (mut s, clip, uid) = painted(center, "disk-freeze.mov");
    // A refine band along the edge on the base frame.
    let c = center(0);
    let ring: Vec<[f64; 2]> = (0..=40).map(|k| k as f64 / 40.0 * std::f64::consts::TAU).map(|a| [c[0] + R * a.cos(), c[1] + R * a.sin()]).collect();
    s.execute_checked("roto.stroke", json!({"layer": clip.0, "kind": "refine", "points": ring, "radius": 5})).unwrap();
    s.execute("roto.span", json!({"layer": clip.0, "end": 3})).unwrap();
    let live: Vec<Vec<f32>> = (0..=3).map(|f| alpha_at(&s, f)).collect();
    // Freeze in the background and wait for it.
    let r = s.execute_checked("roto.freeze", json!({"layer": clip.0})).unwrap();
    assert_eq!(r["frames"], 4);
    let t0 = std::time::Instant::now();
    while s.roto_job.is_some() {
        assert!(t0.elapsed().as_secs() < 300);
        std::thread::sleep(std::time::Duration::from_millis(5));
        s.poll_roto(false);
    }
    let st = s.execute("roto.status", json!({"layer": clip.0})).unwrap();
    assert_eq!(st["frozen"], true, "{st}");
    // Frozen renders match the live ones (8-bit alpha), even without the derived segmentations.
    forget(&s, clip, uid);
    s.layer_cache = Default::default();
    for (f, want) in live.iter().enumerate() {
        let got = alpha_at(&s, f as u32);
        let worst = got.iter().zip(want).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(worst <= 1.0 / 255.0 + 1e-4, "frame {f}: {worst}");
    }
    assert!(st["computed"].as_array().unwrap().len() >= 4);
    // Frozen effects refuse strokes; Unfreeze is undoable.
    assert!(s.execute("roto.stroke", json!({"layer": clip.0, "points": [[10, 10]]})).is_err());
    s.execute("roto.unfreeze", json!({"layer": clip.0})).unwrap();
    assert_eq!(s.execute("roto.status", json!({"layer": clip.0})).unwrap()["frozen"], false);
    assert!(s.undo());
    assert_eq!(s.execute("roto.status", json!({"layer": clip.0})).unwrap()["frozen"], true);
    // Changing the source frames (the layer's timing) drops the frozen mattes.
    let cid = s.active_comp_id().unwrap();
    s.edit("shift", None, |p, _| {
        let l = p.comp_mut(cid).unwrap().layer_mut(clip).unwrap();
        l.start_time = frame_time(1);
        Ok(())
    })
    .unwrap();
    assert_eq!(s.execute("roto.status", json!({"layer": clip.0, "effect": uid})).unwrap()["frozen"], false);
}

#[test]
fn propagation_is_deterministic_and_runs_in_background() {
    let (mut s, clip, uid) = painted(center, "disk-bg.mov");
    s.execute("roto.span", json!({"layer": clip.0, "end": 5})).unwrap();
    s.execute("roto.propagate", json!({"layer": clip.0})).unwrap();
    let t0 = std::time::Instant::now();
    while s.roto_job.is_some() {
        assert!(t0.elapsed().as_secs() < 300);
        std::thread::sleep(std::time::Duration::from_millis(5));
        s.poll_roto(false);
    }
    let st = s.execute("roto.status", json!({"layer": clip.0})).unwrap();
    assert_eq!(st["computed"], json!([0, 1, 2, 3, 4, 5]), "{st}");
    let a = s.execute("roto.status", json!({"layer": clip.0, "frame": 5, "matte": true})).unwrap()["matte"].clone();
    // Recomputing from scratch gives the same matte.
    forget(&s, clip, uid);
    let b = s.execute("roto.status", json!({"layer": clip.0, "frame": 5, "matte": true})).unwrap()["matte"].clone();
    assert!(a.is_string());
    assert_eq!(a, b);
    // Tool options.
    let o = s.execute_checked("roto.options", json!({"diameter": 30, "view": "alphaOverlay"})).unwrap();
    assert_eq!(o["view"], "alphaOverlay");
    assert_eq!(s.state.roto.diameter, 30.0);
    assert!(s.execute("roto.options", json!({"view": "sideways"})).is_err());
    // The default radius follows the tool diameter.
    let r = s.execute("roto.stroke", json!({"layer": clip.0, "points": [[90, 110]]})).unwrap();
    assert_eq!(r["radius"], 15.0);
}

/// Runs offloaded jobs at once in a second session sharing the footage, through JSON (what the
/// browser's Web Worker does, minus the thread).
struct InlineWorker(std::sync::Arc<dyn effectcraft_render::FootageSource>);

impl crate::offload::Offload for InlineWorker {
    fn start(&self, req: crate::offload::WorkerRequest, inbox: std::sync::Arc<crate::offload::Inbox>) -> Result<(), String> {
        let req: crate::offload::WorkerRequest = serde_json::from_str(&serde_json::to_string(&req).unwrap()).unwrap();
        let mut w = Session { footage: self.0.clone(), ..Default::default() };
        let post: crate::offload::Post = std::rc::Rc::new(move |r| inbox.push(serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap()));
        crate::offload::run_request(&mut w, req, &post);
        Ok(())
    }
    fn cancel(&self, _: u64) {}
}

#[test]
fn offloaded_propagation_streams_segmentations_back() {
    let (mut s, clip, uid) = painted(center, "disk-offload.mov");
    s.execute("roto.span", json!({"layer": clip.0, "end": 5})).unwrap();
    s.offload = Some(std::sync::Arc::new(InlineWorker(s.footage.clone())));
    let r = s.execute("roto.propagate", json!({"layer": clip.0})).unwrap();
    assert_eq!(r["frames"], 6, "{r}");
    assert!(s.roto_job.is_none(), "offloaded, not on a thread");
    assert!(s.is_roto_running());
    assert_eq!(s.roto_progress().unwrap().task, Some(crate::roto::RotoTask::Propagate));
    assert_eq!(s.roto_target().map(|t| (t.1, t.2)), Some((clip, uid)));
    assert!(s.jobs().iter().any(|j| j.id == "roto"));
    // The worker has its own segmentation cache: here nothing is computed until its replies
    // are applied.
    forget(&s, clip, uid);
    assert_eq!(s.execute("roto.status", json!({"layer": clip.0})).unwrap()["computed"], json!([]));
    let rev = s.revision;
    assert!(s.poll_offload());
    assert!(!s.is_roto_running());
    assert!(s.revision > rev, "frames are redrawn");
    let st = s.execute("roto.status", json!({"layer": clip.0})).unwrap();
    assert_eq!(st["computed"], json!([0, 1, 2, 3, 4, 5]), "{st}");
    assert!(s.events.iter().any(|e| matches!(e, crate::Event::Toast { message, error: false } if message.contains("propagated 6 frame"))));
    // The matte matches one computed here.
    let a = s.execute("roto.status", json!({"layer": clip.0, "frame": 5, "matte": true})).unwrap()["matte"].clone();
    forget(&s, clip, uid);
    let b = s.execute("roto.status", json!({"layer": clip.0, "frame": 5, "matte": true})).unwrap()["matte"].clone();
    assert!(a.is_string());
    assert_eq!(a, b);
    // `wait` still runs here.
    s.execute("roto.propagate", json!({"layer": clip.0, "wait": true})).unwrap();
    assert!(s.offloaded.is_empty());
}

/// Object Matte: one click on the disk on frame 0 isolates it there and on the frames it is
/// followed to; the Input Key follows the layer's frames; bad prompts are refused.
#[test]
fn object_matte_from_a_click() {
    hold_roto_cache_test_lock();
    let (mut s, clip, solid) = setup(|f| draw(center(f)));
    let cid = s.active_comp_id().unwrap();
    s.edit("remove solid", None, |p, _| {
        p.comp_mut(cid).unwrap().layers.retain(|l| l.id != solid);
        for it in p.items.values_mut() {
            if let effectcraft_project::ItemKind::Footage(f) = &mut it.kind {
                f.path = "object-matte.mov".into();
            }
        }
        Ok(())
    })
    .unwrap();
    s.execute("time.set", json!({"time": 0})).unwrap();
    let c = center(0);
    let r = s.execute_checked("objectMatte.apply", json!({"layer": clip.0, "point": c, "after": 6})).unwrap();
    assert_eq!(r["frame"], 0);
    assert_eq!(r["span"], json!([-30, 6]));
    let uid = r["effect"].as_u64().unwrap();
    let key = |s: &Session| {
        let g = s.active_comp().unwrap().layer(clip).unwrap().props.find_group(uid).unwrap().clone();
        crate::roto::params_static(&g).get(effectcraft_effects::roto::INPUT_KEY).cloned()
    };
    let k0 = key(&s);
    assert!(matches!(&k0, Some(effectcraft_keyframe::Value::Str(k)) if !k.is_empty()), "{k0:?}");
    let wrong = |s: &Session, f: u32| {
        let c = center(f);
        alpha_at(s, f)
            .iter()
            .enumerate()
            .filter(|(i, v)| {
                let d = (((*i as u32 % W) as f64 + 0.5 - c[0]).hypot((*i as u32 / W) as f64 + 0.5 - c[1])) - R;
                (d < -3.0 && **v < 0.5) || (d > 3.0 && **v > 0.5)
            })
            .count()
    };
    let w0 = wrong(&s, 0);
    assert!(w0 < 40, "{w0} wrong pixels on the reference frame");
    let w4 = wrong(&s, 4);
    assert!(w4 < 60, "{w4} wrong pixels on frame 4");
    // Past Frames After: transparent.
    assert!(alpha_at(&s, 8).iter().all(|v| *v == 0.0));
    // Other frames under the effect: a new Input Key (the mattes are recomputed).
    s.edit("swap clip", None, |p, _| {
        for it in p.items.values_mut() {
            if let effectcraft_project::ItemKind::Footage(f) = &mut it.kind {
                f.path = "object-matte-2.mov".into();
            }
        }
        Ok(())
    })
    .unwrap();
    assert_ne!(key(&s), k0);
    // Bad prompts are refused.
    for bad in [
        json!({"layer": clip.0}),
        json!({"layer": clip.0, "point": [-10, 5]}),
        json!({"layer": clip.0, "point": [10, "x"]}),
        json!({"layer": clip.0, "point": c, "box": [0, 0, 10, 10]}),
        json!({"layer": clip.0, "point": c, "frame": 999}),
    ] {
        assert!(s.execute_checked("objectMatte.apply", bad.clone()).is_err(), "{bad}");
    }
    // A box and a static prompt are accepted; the second instance is numbered.
    let r = s
        .execute_checked("objectMatte.apply", json!({"layer": clip.0, "point": c, "box": [c[0] + 50.0, c[1] + 50.0, c[0] - 50.0, c[1] - 50.0], "track": false}))
        .unwrap();
    let g = s.active_comp().unwrap().layer(clip).unwrap().props.find_group(r["effect"].as_u64().unwrap()).unwrap().clone();
    assert_eq!(g.name, "Object Matte 2");
}

/// `objectMatte.apply` with `effect` re-prompts that instance in one undo step: the new point and
/// box, the box dropped when none is given, the reach kept unless given.
#[test]
fn object_matte_reprompt_updates_the_instance() {
    let (mut s, clip, solid) = setup(|f| draw(center(f)));
    let cid = s.active_comp_id().unwrap();
    s.edit("remove solid", None, |p, _| {
        p.comp_mut(cid).unwrap().layers.retain(|l| l.id != solid);
        Ok(())
    })
    .unwrap();
    let c = center(0);
    let r = s
        .execute_checked(
            "objectMatte.apply",
            json!({"layer": clip.0, "point": c, "box": [c[0] - 50.0, c[1] - 50.0, c[0] + 50.0, c[1] + 50.0], "after": 6, "frame": 0}),
        )
        .unwrap();
    let uid = r["effect"].as_u64().unwrap();
    let count = |s: &Session| s.active_comp().unwrap().layer(clip).unwrap().effects().unwrap().groups().count();
    let ps = |s: &Session| crate::roto::params_static(s.active_comp().unwrap().layer(clip).unwrap().props.find_group(uid).unwrap());
    let undo0 = s.history.undo.len();
    let r = s.execute_checked("objectMatte.apply", json!({"layer": clip.0, "effect": uid, "point": [c[0] + 4.0, c[1]], "frame": 2})).unwrap();
    assert_eq!((r["effect"].as_u64(), r["updated"].as_bool()), (Some(uid), Some(true)));
    assert_eq!(r["span"], json!([-28, 8]));
    assert_eq!(count(&s), 1);
    assert_eq!(s.history.undo.len(), undo0 + 1);
    let p = ps(&s);
    assert_eq!(p.get("objectPoint"), Some(&effectcraft_keyframe::Value::Vec2([c[0] + 4.0, c[1]])));
    assert_eq!(p.get("useBox"), Some(&effectcraft_keyframe::Value::Bool(false)));
    assert_eq!(effectcraft_effects::object_matte::reference_frame(&p), 2);
    assert_eq!(effectcraft_effects::object_matte::reach(&p), [30, 6]);
    // Undo restores the first prompt.
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(ps(&s).get("useBox"), Some(&effectcraft_keyframe::Value::Bool(true)));
    // An unknown or non-Object Matte effect is refused.
    assert!(s.execute_checked("objectMatte.apply", json!({"layer": clip.0, "effect": 999_999, "point": c})).is_err());
}

/// Object Matte runs in the background like Roto Brush propagation: queued by the edit, a job
/// that returns at once and reports progress, stops when cancelled (keeping what it computed),
/// computes the tracked frames when left to run, and Freeze stores mattes that renders then use
/// without segmenting again.
#[test]
fn object_matte_background_job_cancel_and_freeze() {
    hold_roto_cache_test_lock();
    let (mut s, clip, solid) = setup(|f| draw(center(f)));
    let cid = s.active_comp_id().unwrap();
    s.edit("remove solid", None, |p, _| {
        p.comp_mut(cid).unwrap().layers.retain(|l| l.id != solid);
        for it in p.items.values_mut() {
            if let effectcraft_project::ItemKind::Footage(f) = &mut it.kind {
                f.path = "object-matte-job.mov".into();
            }
        }
        Ok(())
    })
    .unwrap();
    let c = center(0);
    let r = s.execute_checked("objectMatte.apply", json!({"layer": clip.0, "point": c, "before": 0, "after": 24, "frame": 0})).unwrap();
    let uid = r["effect"].as_u64().unwrap();
    assert!(s.roto_pending.contains(&(cid, clip, uid)), "the new prompt is queued for tracking");
    let wait_done = |s: &mut Session| {
        let t = std::time::Instant::now();
        while s.is_roto_running() {
            assert!(t.elapsed().as_secs() < 120, "the job never finished");
            std::thread::sleep(std::time::Duration::from_millis(5));
            s.poll_roto(false);
        }
        s.poll_roto(false);
    };
    let toast = |s: &Session, m: &str| s.events.iter().any(|e| matches!(e, crate::Event::Toast { message, .. } if message.starts_with(m)));
    let status = |s: &mut Session| s.execute("objectMatte.status", json!({"layer": clip.0, "effect": uid})).unwrap();

    // Cancelled at once: it stops early, and says so.
    let r = s.execute_checked("objectMatte.propagate", json!({"layer": clip.0, "effect": uid})).unwrap();
    assert_eq!(r["frames"], 25);
    assert!(!s.roto_pending.contains(&(cid, clip, uid)));
    let _ = s.execute("roto.cancel", json!({}));
    wait_done(&mut s);
    assert!(toast(&s, "Object Matte: stopped"), "{:?}", s.events);
    let st = status(&mut s);
    assert_eq!(st["frames"].as_array().unwrap().len(), 25, "{st}");
    let early = st["computed"].as_array().unwrap().len();
    assert!(early < 25, "{st}");

    // Left to run: the call returns while the job works (progress visible), then every tracked
    // frame is computed.
    let r = s.execute_checked("objectMatte.propagate", json!({"layer": clip.0, "effect": uid})).unwrap();
    assert_eq!(r["running"], true, "{r}");
    let pr = s.roto_progress().unwrap();
    assert!(pr.object_matte && pr.banner().starts_with("Tracking Object Matte"), "{pr:?}");
    // One job at a time.
    assert!(s.execute_checked("objectMatte.propagate", json!({"layer": clip.0, "effect": uid})).is_err());
    wait_done(&mut s);
    assert!(toast(&s, "Object Matte: tracked"), "{:?}", s.events);
    assert_eq!(status(&mut s)["computed"].as_array().unwrap().len(), 25);
    let before = alpha_at(&s, 4);

    // Freeze, then drop the segmentations: renders use the frozen mattes and compute nothing.
    let r = s.execute_checked("objectMatte.freeze", json!({"layer": clip.0, "effect": uid, "wait": true})).unwrap();
    assert_eq!(r["frozen"], true, "{r}");
    let keys: Vec<u64> = s.roto_keys(cid, clip, uid).unwrap().into_iter().map(|(_, k)| k).collect();
    assert_eq!(keys.len(), 25);
    effectcraft_effects::roto::forget(keys.iter().copied());
    let frozen = alpha_at(&s, 4);
    let diff = before.iter().zip(&frozen).filter(|(a, b)| (**a - **b).abs() > 0.02).count();
    assert!(diff < 30, "{diff} pixels differ from the computed matte");
    assert!(status(&mut s)["computed"].as_array().unwrap().is_empty(), "a frozen frame was segmented again");
    assert_eq!(status(&mut s)["frozen"], true);
    // A frozen prompt is refused; Unfreeze computes again.
    assert!(s.execute_checked("objectMatte.apply", json!({"layer": clip.0, "effect": uid, "point": c})).is_err());
    s.execute_checked("objectMatte.unfreeze", json!({"layer": clip.0, "effect": uid})).unwrap();
    let _ = alpha_at(&s, 4);
    assert!(status(&mut s)["computed"].as_array().unwrap().iter().any(|f| f == 4));
    // Undo brings the frozen mattes back.
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(status(&mut s)["frozen"], true);
}

/// Computed Object Matte segmentations go to the persistent disk cache: a second session on the
/// same folder and project gets them back without segmenting (its frames cannot even be read
/// here), and another prompt (other keys) finds nothing.
#[test]
fn object_matte_mattes_persist_in_the_disk_cache() {
    use effectcraft_effects::object_matte as om;
    use effectcraft_render::disk_cache::Kind;
    hold_roto_cache_test_lock();
    let dir = std::env::temp_dir().join(format!("effectcraft-om-disk-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let enable = |s: &mut Session| {
        s.execute_checked("prefs.set", json!({"values": {"disk.diskCacheFolder": dir.to_string_lossy(), "disk.diskCacheEnabled": true}})).unwrap();
    };
    let (mut s, clip, solid) = setup(|f| draw(center(f)));
    enable(&mut s);
    let cid = s.active_comp_id().unwrap();
    s.edit("remove solid", None, |p, _| {
        p.comp_mut(cid).unwrap().layers.retain(|l| l.id != solid);
        for it in p.items.values_mut() {
            if let effectcraft_project::ItemKind::Footage(f) = &mut it.kind {
                f.path = "object-matte-disk.mov".into();
            }
        }
        Ok(())
    })
    .unwrap();
    let c = center(0);
    let r = s.execute_checked("objectMatte.apply", json!({"layer": clip.0, "point": c, "before": 0, "after": 5, "frame": 0})).unwrap();
    let uid = r["effect"].as_u64().unwrap();
    s.execute_checked("objectMatte.propagate", json!({"layer": clip.0, "effect": uid, "wait": true})).unwrap();
    let keys = s.roto_keys(cid, clip, uid).unwrap();
    assert_eq!(keys.len(), 6);
    let dc = s.disk_cache.clone().unwrap();
    dc.flush();
    assert!(dc.keys(Kind::Matte).len() >= 6, "{:?}", dc.stats());
    let before = alpha_at(&s, 3);
    let saved = s.project.clone();
    drop(dc);
    drop(s);
    effectcraft_effects::roto::forget(keys.iter().map(|(_, k)| *k));

    // "Reopen": a new session on the same folder and project.
    let (mut s2, _, _) = setup(|f| draw(center(f)));
    enable(&mut s2);
    s2.project = saved;
    s2.bump();
    s2.open_comp(cid);
    assert_eq!(s2.roto_keys(cid, clip, uid).unwrap(), keys, "the same input key, so the same keys");
    let comp = s2.active_comp().unwrap().clone();
    let l = comp.layer(clip).unwrap();
    let ps = crate::roto::params_static(l.props.find_group(uid).unwrap());
    let size = crate::roto::layer_size(&s2.project, &comp, l);
    // No frames to segment from: whatever comes back was read from disk.
    for (f, _) in &keys {
        assert!(om::ensure(&ps, size, 25.0, *f, None, &mut |_| None, &|| false).is_some(), "frame {f} not reused from disk");
    }
    assert!(s2.disk_cache.as_ref().unwrap().stats().hits >= 6);
    let again = alpha_at(&s2, 3);
    assert_eq!(again, before, "the disk-cached matte renders the same");
    // Another prompt: other keys, nothing on disk.
    let mut moved = ps.clone();
    moved.values.insert("objectPoint".into(), effectcraft_keyframe::Value::Vec2([c[0] + 3.0, c[1]]));
    assert!(om::ensure(&moved, size, 25.0, 0, None, &mut |_| None, &|| false).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

/// SAM 2 tracks the clip end to end (needs the real weights: `EFFECTCRAFT_SAM2=path/to/model.safetensors`).
/// Every frame comes from the tracker with a confidence, the mattes are right, and nothing changes
/// once the model is deselected.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn sam2_tracks_the_clip_through_roto_propagate() {
    let Ok(weights) = std::env::var("EFFECTCRAFT_SAM2") else { return };
    let dir = std::env::temp_dir().join(format!("ec-roto-sam2-{}", std::process::id()));
    hold_roto_cache_test_lock();
    let (mut s, clip, _) = painted(center, "disk-sam2.mov");
    s.models_dir = Some(dir.join("models"));
    std::fs::create_dir_all(dir.join("models")).unwrap();
    s.execute_checked("roto.model.install", json!({"path": weights})).unwrap();
    s.execute_checked("roto.model.select", json!({"id": "sam2.1-hiera-tiny", "wait": true})).unwrap();
    assert_eq!(s.models.status(effectcraft_segment::Task::Mask).active.as_deref(), Some("sam2.1-hiera-tiny"));
    let r = s.execute_checked("roto.propagate", json!({"layer": clip.0, "direction": "both", "wait": true})).unwrap();
    assert_eq!(r["frames"], 21, "{r}");
    let mut worst: f64 = 1.0;
    for f in 0..=20 {
        let j = iou_at(&mut s, clip, f, center(f));
        let st = s.execute("roto.status", json!({"layer": clip.0, "frame": f})).unwrap();
        eprintln!("frame {f}: IoU {j:.3}, {:?}", st["tracker"]["frames"][f.to_string()]);
        worst = worst.min(j);
    }
    assert!(worst > 0.9, "worst IoU over the span with SAM 2: {worst}");
    let st = s.execute("roto.status", json!({"layer": clip.0, "frame": 12})).unwrap();
    assert_eq!(st["source"], "tracker", "{st}");
    let frames = st["tracker"]["frames"].as_object().unwrap();
    assert_eq!(frames.len(), 21);
    assert!(frames.values().all(|t| t["occluded"] == false && t["confidence"].as_f64().unwrap() > 0.3), "{frames:?}");
    assert_eq!(st["tracker"]["model"], "sam2.1-hiera-tiny");
    // Settings choose the classic engine: the saved tracking stays until a Propagate replaces it.
    s.execute_checked("roto.model.select", json!({"id": "classical", "wait": true})).unwrap();
    let st = s.execute("roto.status", json!({"layer": clip.0, "frame": 12})).unwrap();
    assert_eq!(st["source"], "tracker", "{st}");
    assert!(st["notice"].as_str().unwrap().contains("Propagate again"), "{st}");
    assert_eq!(st["computed"].as_array().unwrap().len(), 21, "still computed under another model: {st}");
    // A classic Propagate: its own segmentation, no tracker data.
    s.execute_checked("roto.propagate", json!({"layer": clip.0, "direction": "both", "wait": true})).unwrap();
    let st = s.execute("roto.status", json!({"layer": clip.0, "frame": 12})).unwrap();
    assert_eq!(st["source"], "classical", "{st}");
    assert_eq!(st["tracker"]["frames"].as_object().unwrap().len(), 0);
    let _ = std::fs::remove_dir_all(dir);
}

/// Choosing a model that is not installed must not fall back to the classic engine silently:
/// propagate refuses and `roto.status` says why. Choosing the classic engine runs it again.
#[test]
fn a_selected_model_that_is_not_installed_refuses_to_propagate() {
    hold_roto_cache_test_lock();
    let (mut s, clip, _) = painted(center, "disk-not-installed.mov");
    s.models_dir = Some(std::env::temp_dir().join(format!("ec-roto-none-{}", std::process::id())));
    s.execute_checked("roto.model.select", json!({"id": "sam2.1-hiera-tiny"})).unwrap();
    let st = s.execute("roto.status", json!({"layer": clip.0})).unwrap();
    assert_eq!(st["model"]["selected"], "sam2.1-hiera-tiny", "{st}");
    assert_eq!(st["model"]["installed"], false, "{st}");
    assert!(st["notice"].as_str().unwrap().contains("not installed"), "{st}");
    let e = s.execute_checked("roto.propagate", json!({"layer": clip.0, "direction": "both", "wait": true})).unwrap_err().to_string();
    assert!(e.contains("not installed"), "{e}");
    assert_eq!(s.execute("roto.status", json!({"layer": clip.0, "frame": 10})).unwrap()["source"], "classical");
    s.execute_checked("roto.model.select", json!({"id": "classical"})).unwrap();
    let r = s.execute_checked("roto.propagate", json!({"layer": clip.0, "direction": "both", "wait": true})).unwrap();
    assert_eq!(r["frames"], 21, "{r}");
    assert!(s.execute("roto.status", json!({"layer": clip.0})).unwrap()["notice"].is_null());
}

/// Selecting SAM 2 without `wait` and propagating at once: propagate loads the model first, so the
/// tracker (not the classic engine) does the work. Needs EFFECTCRAFT_SAM2 like the test above.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn sam2_selected_without_wait_is_loaded_before_propagate() {
    let Ok(weights) = std::env::var("EFFECTCRAFT_SAM2") else { return };
    hold_roto_cache_test_lock();
    let dir = std::env::temp_dir().join(format!("ec-roto-sam2-wait-{}", std::process::id()));
    let (mut s, clip, _) = painted(center, "disk-sam2-wait.mov");
    s.models_dir = Some(dir.join("models"));
    std::fs::create_dir_all(dir.join("models")).unwrap();
    s.execute_checked("roto.model.install", json!({"path": weights})).unwrap();
    s.execute_checked("roto.model.select", json!({"id": "sam2.1-hiera-tiny"})).unwrap();
    let r = s.execute_checked("roto.propagate", json!({"layer": clip.0, "direction": "both", "wait": true})).unwrap();
    assert_eq!(r["frames"], 21, "{r}");
    let st = s.execute("roto.status", json!({"layer": clip.0, "frame": 12})).unwrap();
    assert_eq!(st["source"], "tracker", "{st}");
    assert_eq!(st["tracker"]["model"], "sam2.1-hiera-tiny");
    assert!(st["notice"].is_null(), "{st}");
    let _ = std::fs::remove_dir_all(dir);
}

/// A tracker that follows the disc exactly: its true mask on every frame of the request. A stand-in
/// for SAM 2, so the saved tracking can be tested without the weights.
struct Exact;

impl effectcraft_segment::MaskModel for Exact {
    fn info(&self) -> &'static effectcraft_segment::ModelInfo {
        &effectcraft_segment::SAM2_TINY
    }
    fn segment(&self, _: &[[f32; 3]], _: usize, _: usize, _: &effectcraft_segment::Prompt) -> effectcraft_segment::Result<Vec<f32>> {
        Err("no weights".into())
    }
    fn tracker(&self) -> Option<&dyn effectcraft_segment::ObjectTracker> {
        Some(self)
    }
}

impl effectcraft_segment::ObjectTracker for Exact {
    fn track(
        &self,
        req: &effectcraft_segment::TrackRequest,
        frame: &mut dyn FnMut(i64) -> Option<std::sync::Arc<effectcraft_segment::ClipFrame>>,
        _: &dyn Fn() -> bool,
        out: &mut dyn FnMut(usize, i64, effectcraft_segment::TrackedFrame),
    ) -> effectcraft_segment::Result<()> {
        for g in req.first..req.first + req.count as i64 {
            let Some(img) = frame(g) else { return Err(format!("no frame {g}")) };
            let c = center(g as u32);
            let prob = (0..img.w * img.h)
                .map(|i| ((((i % img.w) as f64 + 0.5 - c[0]).hypot((i / img.w) as f64 + 0.5 - c[1])) <= R) as u8 as f32)
                .collect::<Vec<f32>>();
            out(0, g, effectcraft_segment::TrackedFrame { prob, w: img.w, h: img.h, confidence: 0.99, occluded: false });
        }
        Ok(())
    }
}

/// Saved tracking is part of the project: written with the instance, it applies when the project
/// opens, with or without the model loaded, and Settings choosing the classic engine leaves it in place
/// until a Propagate replaces it.
#[test]
fn saved_tracking_is_kept_in_the_project_and_applies_when_it_reopens() {
    let sam2 = "sam2.1-hiera-tiny";
    let (s, clip, uid, path) = saved_project("saved");
    // Reopened with SAM 2 chosen but not loaded (none is installed here): the saved frames apply.
    forget(&s, clip, uid);
    let mut s2 = Session::default();
    s2.execute_checked("file.open", json!({"path": path})).unwrap();
    s2.execute_checked("roto.model.select", json!({"id": sam2})).unwrap();
    let st = s2.execute("roto.status", json!({"layer": clip.0, "frame": 12})).unwrap();
    assert_eq!(st["source"], "tracker", "{st}");
    assert_eq!(st["tracker"]["saved"], sam2, "{st}");
    assert_eq!(st["tracker"]["frames"].as_object().unwrap().len(), 21, "{st}");
    // The span bar's "computed" frames include the saved ones before anything has been decoded.
    assert_eq!(st["computed"].as_array().unwrap().len(), 21, "{st}");
    let j = iou_at(&mut s2, clip, 12, center(12));
    assert!(j > 0.9, "IoU of a saved frame: {j}");
    // The render path (Chain::ensure) reads the same frame.
    let comp = s2.active_comp().unwrap();
    let l = comp.layer(clip).unwrap();
    let g = l.props.find_group(uid).unwrap();
    let size = crate::roto::layer_size(&s2.project, comp, l);
    let ch = effectcraft_effects::roto::Chain::new(&crate::roto::params_static(g), size, comp.frame_rate.as_f64(), 1.0);
    let seg = ch.ensure(12, &mut |_| None, &|| false).expect("the render path reads the saved frame");
    let truth = rle::decode(&gt(center(12))).unwrap();
    let inter = seg.matte.iter().zip(&truth).filter(|(a, b)| **a != 0 && **b != 0).count();
    let uni = seg.matte.iter().zip(&truth).filter(|(a, b)| **a != 0 || **b != 0).count();
    assert!(inter as f64 / uni as f64 > 0.9, "the render path's frame: IoU {}", inter as f64 / uni as f64);
    // Settings choose the classic engine: the saved frames still apply, and the notice says so.
    s2.execute_checked("roto.model.select", json!({"id": "classical"})).unwrap();
    let st = s2.execute("roto.status", json!({"layer": clip.0, "frame": 12})).unwrap();
    assert_eq!(st["source"], "tracker", "{st}");
    assert!(st["notice"].as_str().unwrap().contains("Propagate again"), "{st}");
    // A classic Propagate replaces the saved tracking with nothing.
    // The synthetic source is not a file on disk; give the reopened session its provider before
    // asking it to recompute frames. The checks above intentionally rendered without that provider.
    s2.footage = s.footage.clone();
    s2.execute_checked("roto.propagate", json!({"layer": clip.0, "direction": "both", "wait": true})).unwrap();
    let st = s2.execute("roto.status", json!({"layer": clip.0, "frame": 12})).unwrap();
    assert_eq!(st["source"], "classical", "{st}");
    assert!(st["tracker"]["saved"].is_null() && st["notice"].is_null(), "{st}");
    let _ = std::fs::remove_dir_all(std::path::Path::new(&path).parent().unwrap());
}

/// A reopened project propagated again with the same model and nothing new to track keeps the tracking it has: the
/// saved frames are not replaced by an empty one, so a later reopen still finds them.
#[test]
fn propagating_with_nothing_new_keeps_the_saved_tracking() {
    let (s, clip, uid, path) = saved_project("keep-saved");
    forget(&s, clip, uid);
    let mut s2 = Session::default();
    s2.execute_checked("file.open", json!({"path": path})).unwrap();
    s2.footage = s.footage.clone();
    effectcraft_effects::roto::set_model(Some(std::sync::Arc::new(Exact)));
    let result = s2.execute_checked("roto.propagate", json!({"layer": clip.0, "direction": "both", "wait": true}));
    effectcraft_effects::roto::set_model(None);
    result.unwrap();
    let st = s2.execute("roto.status", json!({"layer": clip.0, "frame": 12})).unwrap();
    assert_eq!(st["tracker"]["saved"], "sam2.1-hiera-tiny", "{st}");
    assert_eq!(st["tracker"]["frames"].as_object().unwrap().len(), 21, "{st}");
    // and it is still in the file after saving and reopening
    s2.execute_checked("file.save", json!({})).unwrap();
    let mut s3 = Session::default();
    s3.execute_checked("file.open", json!({"path": path})).unwrap();
    let st = s3.execute("roto.status", json!({"layer": clip.0, "frame": 12})).unwrap();
    assert_eq!(st["tracker"]["frames"].as_object().unwrap().len(), 21, "{st}");
    let _ = std::fs::remove_dir_all(std::path::Path::new(&path).parent().unwrap());
}

/// Freeze reads saved tracking without its model: choosing a model that is not installed does not stop it when
/// the saved frames cover every frame it freezes.
#[test]
fn freeze_uses_complete_saved_tracking_without_the_model() {
    let (s, clip, uid, path) = saved_project("freeze-saved");
    forget(&s, clip, uid);
    let mut s2 = Session::default();
    s2.execute_checked("file.open", json!({"path": path})).unwrap();
    s2.footage = s.footage.clone();
    s2.execute_checked("roto.model.select", json!({"id": "sam2.1-hiera-tiny"})).unwrap();
    s2.execute_checked("roto.freeze", json!({"layer": clip.0, "wait": true})).unwrap();
    let _ = std::fs::remove_dir_all(std::path::Path::new(&path).parent().unwrap());
}

/// Propagate replaces a fully saved span when a different tracking model is active.
#[test]
fn propagating_saved_tracking_uses_the_new_model() {
    struct OtherModel;
    impl effectcraft_segment::MaskModel for OtherModel {
        fn info(&self) -> &'static effectcraft_segment::ModelInfo {
            &effectcraft_segment::SAM2_SMALL
        }
        fn segment(&self, _: &[[f32; 3]], _: usize, _: usize, _: &effectcraft_segment::Prompt) -> effectcraft_segment::Result<Vec<f32>> {
            Err("no weights".into())
        }
        fn tracker(&self) -> Option<&dyn effectcraft_segment::ObjectTracker> {
            Some(&Exact)
        }
    }
    let (mut s, clip, _uid, path) = saved_project("model-switch");
    effectcraft_effects::roto::set_model(Some(std::sync::Arc::new(OtherModel)));
    let result = s.execute_checked("roto.propagate", json!({"layer": clip.0, "direction": "both", "wait": true}));
    effectcraft_effects::roto::set_model(None);
    result.unwrap();
    let status = s.execute("roto.status", json!({"layer": clip.0, "frame": 12})).unwrap();
    assert_eq!(status["tracker"]["saved"], "sam2.1-hiera-small", "{status}");
    assert_eq!(status["tracker"]["frames"].as_object().unwrap().len(), 21, "{status}");
    let _ = std::fs::remove_dir_all(std::path::Path::new(&path).parent().unwrap());
}

/// A tracked clip saved to a project file: the session that tracked it, its layer and instance, and the
/// file.
fn saved_project(name: &str) -> (Session, LayerId, u64, String) {
    let (mut s, clip, uid) = painted(center, &format!("disk-{name}.mov"));
    effectcraft_effects::roto::set_model(Some(std::sync::Arc::new(Exact)));
    let r = s.execute_checked("roto.propagate", json!({"layer": clip.0, "direction": "both", "wait": true}));
    effectcraft_effects::roto::set_model(None);
    assert_eq!(r.unwrap()["frames"], 21);
    let dir = std::env::temp_dir().join(format!("ec-roto-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("tracked.ecproj").to_string_lossy().to_string();
    s.execute_checked("file.saveAs", json!({"path": path})).unwrap();
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.contains("sam2.1-hiera-tiny"), "the project file carries the tracking and the model that made it");
    (s, clip, uid, path)
}

/// A stroke change makes the saved tracking stale: the instance is classical until the next Propagate.
#[test]
fn a_stroke_change_makes_the_saved_tracking_stale() {
    let (_s, clip, _uid, path) = saved_project("stale");
    let mut s2 = Session::default();
    s2.execute_checked("file.open", json!({"path": path})).unwrap();
    s2.execute_checked("roto.model.select", json!({"id": "sam2.1-hiera-tiny"})).unwrap();
    assert_eq!(s2.execute("roto.status", json!({"layer": clip.0, "frame": 12})).unwrap()["source"], "tracker");
    s2.execute_checked("roto.stroke", json!({"layer": clip.0, "kind": "bg", "points": line([10.0, 10.0], [60.0, 12.0]), "radius": 4})).unwrap();
    let st = s2.execute("roto.status", json!({"layer": clip.0, "frame": 12})).unwrap();
    assert_eq!(st["source"], "classical", "{st}");
    assert_eq!(st["tracker"]["frames"].as_object().unwrap().len(), 0, "{st}");
    let _ = std::fs::remove_dir_all(std::path::Path::new(&path).parent().unwrap());
}

/// A tracker that emits a completed frame before failing must leave only classic results behind.
#[test]
fn a_partially_failing_tracker_falls_back_to_the_classic_engine() {
    use effectcraft_segment::{ClipFrame, MaskModel, ModelInfo, ObjectTracker, Prompt, TrackRequest, TrackedFrame};
    use std::sync::Arc;
    struct Broken;
    impl MaskModel for Broken {
        fn info(&self) -> &'static ModelInfo {
            &effectcraft_segment::SAM2_TINY
        }
        fn segment(&self, _: &[[f32; 3]], _: usize, _: usize, _: &Prompt) -> effectcraft_segment::Result<Vec<f32>> {
            Err("no weights".into())
        }
        fn tracker(&self) -> Option<&dyn ObjectTracker> {
            Some(self)
        }
    }
    impl ObjectTracker for Broken {
        fn track(
            &self,
            req: &TrackRequest,
            frame: &mut dyn FnMut(i64) -> Option<Arc<ClipFrame>>,
            _: &dyn Fn() -> bool,
            out: &mut dyn FnMut(usize, i64, TrackedFrame),
        ) -> effectcraft_segment::Result<()> {
            let img = frame(req.first).ok_or("missing test frame")?;
            for object in 0..req.objects.len() {
                out(object, req.first, TrackedFrame { prob: vec![1.0; img.w * img.h], w: img.w, h: img.h, confidence: 0.9, occluded: false });
            }
            Err("the tracker broke after emitting a frame".into())
        }
    }
    hold_roto_cache_test_lock();
    let (mut s, clip, uid) = painted(center, "disk-partially-broken-tracker.mov");
    s.execute_checked("roto.stroke", json!({"layer": clip.0, "object": 1, "points": [center(0)], "radius": 4})).unwrap();
    effectcraft_effects::roto::set_model(Some(Arc::new(Broken)));
    let r = s.execute_checked("roto.propagate", json!({"layer": clip.0, "direction": "both", "wait": true}));
    let st = s.execute("roto.status", json!({"layer": clip.0, "frame": 0}));
    let comp = s.active_comp().unwrap();
    let layer = comp.layer(clip).unwrap();
    let params = crate::roto::params_static(layer.props.find_group(uid).unwrap());
    let size = crate::roto::layer_size(&s.project, comp, layer);
    let mut chain = effectcraft_effects::roto::Chain::new(&params, size, comp.frame_rate.as_f64(), 1.0);
    let index = layer.effects().unwrap().groups().position(|g| g.uid == uid).unwrap();
    let mut source =
        crate::roto::FrameSource::new(&s.project, s.footage.as_ref(), s.expr.as_deref(), &s.layer_cache, s.active_comp_id().unwrap(), comp, layer, index, size);
    let mut get = |f: i64| source.get(f);
    let actual: Option<Vec<_>> = (0..=20).map(|f| chain.ensure(f, &mut get, &|| false)).collect();
    let retained_object = chain.object_seg(0, 0).is_some() || chain.object_seg(0, 1).is_some();
    let retained_trust = chain.trust_at(0);
    // Build the reference only after capturing the fallback, with no model or cached union.
    effectcraft_effects::roto::forget(chain.keys.values().copied());
    chain.model = None;
    let expected: Option<Vec<_>> = (0..=20).map(|f| chain.ensure(f, &mut get, &|| false)).collect();
    effectcraft_effects::roto::set_model(None);
    let r = r.unwrap();
    assert_eq!(r["frames"], 21, "{r}");
    let st = st.unwrap();
    assert_ne!(st["source"], "tracker", "{st}");
    assert_eq!(st["tracker"]["frames"].as_object().unwrap().len(), 0);
    assert!(!retained_object, "partial per-object mattes must be discarded");
    assert_eq!(retained_trust, None, "partial tracker confidence must be discarded");
    for (frame, (actual, expected)) in actual.unwrap().into_iter().zip(expected.unwrap()).enumerate() {
        assert!(actual == expected, "fallback frame {frame} differs from the classic engine");
    }
}

/// Without a tracking model the Roto Brush chain is exactly the classic engine: every frame
/// equals a direct call of `segment_with` / `propagate_with` (no model), bit for bit.
#[test]
fn without_a_tracker_the_chain_is_exactly_the_classic_engine() {
    use effectcraft_track::roto as rb;
    use std::sync::Arc;
    let (s, clip, uid) = painted(center, "disk-bitexact.mov");
    forget(&s, clip, uid);
    let comp = s.active_comp().unwrap();
    let l = comp.layer(clip).unwrap();
    let g = l.props.find_group(uid).unwrap();
    let size = crate::roto::layer_size(&s.project, comp, l);
    let chain = effectcraft_effects::roto::Chain::new(&crate::roto::params_static(g), size, comp.frame_rate.as_f64(), 1.0);
    assert!(!chain.has_tracker());
    let img = |f: i64| Arc::new(draw(center(f as u32)));
    let mut prev: Option<(Arc<Image>, rb::FrameSeg)> = None;
    for f in 0..=5i64 {
        let got = chain.ensure(f, &mut |g| Some(img(g)), &|| false).unwrap();
        let cur = img(f);
        let strokes = chain.data.strokes_at(f);
        let want = match &prev {
            None => rb::segment_with(&cur, &strokes, None, &chain.opts, chain.refine_radius, None),
            Some((pi, ps)) => rb::propagate_with(pi, ps, &cur, &strokes, &chain.opts, chain.refine_radius, None),
        };
        assert_eq!(*got, want, "frame {f} differs from the classic engine");
        prev = Some((cur, want));
    }
    forget(&s, clip, uid);
}

/// Records the windows it is asked to track, and follows the disc exactly like [`Exact`].
#[derive(Default)]
struct Recorder {
    requests: std::sync::Mutex<Vec<(i64, usize, Vec<i64>)>>,
}

impl effectcraft_segment::MaskModel for Recorder {
    fn info(&self) -> &'static effectcraft_segment::ModelInfo {
        &effectcraft_segment::SAM2_TINY
    }
    fn segment(&self, _: &[[f32; 3]], _: usize, _: usize, _: &effectcraft_segment::Prompt) -> effectcraft_segment::Result<Vec<f32>> {
        Err("no weights".into())
    }
    fn tracker(&self) -> Option<&dyn effectcraft_segment::ObjectTracker> {
        Some(self)
    }
}

impl effectcraft_segment::ObjectTracker for Recorder {
    fn track(
        &self,
        req: &effectcraft_segment::TrackRequest,
        frame: &mut dyn FnMut(i64) -> Option<std::sync::Arc<effectcraft_segment::ClipFrame>>,
        cancel: &dyn Fn() -> bool,
        out: &mut dyn FnMut(usize, i64, effectcraft_segment::TrackedFrame),
    ) -> effectcraft_segment::Result<()> {
        let prompted = req.objects.iter().flatten().map(|(f, _)| *f).collect();
        self.requests.lock().unwrap().push((req.first, req.count, prompted));
        effectcraft_segment::ObjectTracker::track(&Exact, req, frame, cancel, out)
    }
}

/// A correction stroke re-tracks only what it changes: the frames from it on, restarted at the
/// corrected frame; the frames before it keep their tracked results.
#[test]
fn a_correction_re_tracks_only_the_frames_it_changes() {
    let (mut s, clip, uid) = painted(center, "disk-retrack.mov");
    let cid = s.active_comp_id().unwrap();
    assert!(s.roto_trust(cid, clip, uid).is_empty(), "no tracker data before tracking");
    let rec = std::sync::Arc::new(Recorder::default());
    effectcraft_effects::roto::set_model(Some(rec.clone()));
    let run = |s: &mut Session| s.execute_checked("roto.propagate", json!({"layer": clip.0, "direction": "forward", "wait": true}));
    let first = run(&mut s);
    assert_eq!(rec.requests.lock().unwrap().as_slice(), [(0, 21, vec![0])], "the first pass tracks the whole span");
    first.unwrap();
    // Nothing changed: nothing to track.
    run(&mut s).unwrap();
    assert_eq!(rec.requests.lock().unwrap().len(), 1, "an unchanged span is not tracked again");
    // A correction on frame 8: the frames from 8 on are tracked again, restarting at frame 8.
    let c = center(8);
    s.execute_checked("roto.stroke", json!({"layer": clip.0, "frame": 8, "points": line([c[0] - 20.0, c[1]], [c[0] + 20.0, c[1]]), "radius": 6})).unwrap();
    let again = run(&mut s);
    let reqs = rec.requests.lock().unwrap().clone();
    effectcraft_effects::roto::set_model(None);
    again.unwrap();
    assert_eq!(reqs.len(), 2);
    assert_eq!(reqs[1], (8, 13, vec![8]), "the window runs from the corrected frame to the end of the span");
    // The frames before the correction are still the first pass's, the rest the second's.
    let st = s.execute("roto.status", json!({"layer": clip.0, "frame": 3})).unwrap();
    assert_eq!(st["source"], "tracker", "{st}");
    assert_eq!(st["tracker"]["frames"].as_object().unwrap().len(), 21, "{st}");
    // The span bar's data: the tracker's confidence for every tracked frame.
    let trust = s.roto_trust(cid, clip, uid);
    assert_eq!(trust.len(), 21);
    assert!(trust.values().all(|t| t.confidence > 0.9 && !t.occluded));
}

#[test]
fn strokes_belong_to_the_current_object() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"width": 64, "height": 64, "frameRate": 25, "duration": 1})).unwrap();
    let layer = s.execute("layer.newSolid", json!({"width": 64, "height": 64})).unwrap()["layer"].as_u64().unwrap();
    s.execute("roto.stroke", json!({"layer": layer, "points": [[10, 10]], "radius": 4})).unwrap();
    // the current object is an option shared by the tool and the commands
    s.execute("roto.options", json!({"object": 1})).unwrap();
    s.execute("roto.stroke", json!({"layer": layer, "points": [[50, 50]], "radius": 4})).unwrap();
    s.execute("roto.stroke", json!({"layer": layer, "kind": "bg", "points": [[30, 30]], "radius": 4})).unwrap();
    // an explicit object wins over the option, and is clamped to the supported range
    s.execute("roto.stroke", json!({"layer": layer, "points": [[20, 50]], "radius": 4, "object": 99})).unwrap();
    let st = s.execute("roto.status", json!({"layer": layer})).unwrap();
    assert_eq!(st["currentObject"], 1);
    assert_eq!(st["objects"], json!([{"id": 0, "strokes": 1}, {"id": 1, "strokes": 2}, {"id": 31, "strokes": 1}]));
    // removing one object's strokes leaves the others
    let r = s.execute("roto.clearStrokes", json!({"layer": layer, "object": 1})).unwrap();
    assert_eq!(r["removed"], 2);
    let st = s.execute("roto.status", json!({"layer": layer})).unwrap();
    assert_eq!(st["objects"], json!([{"id": 0, "strokes": 1}, {"id": 31, "strokes": 1}]));
    // the object survives a save and a load
    let json = s.project.to_json();
    assert!(json.contains("\\\"object\\\":31"), "the project file carries the object");
}

#[test]
fn clearing_one_object_keeps_shared_refine_edge_strokes() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"width": 64, "height": 64, "duration": 1})).unwrap();
    let layer = s.execute("layer.newSolid", json!({})).unwrap()["layer"].as_u64().unwrap();
    for (object, kind) in [(0, "fg"), (0, "bg"), (0, "refine"), (0, "refineErase"), (1, "fg")] {
        s.execute("roto.stroke", json!({"layer": layer, "object": object, "kind": kind, "points": [[20, 20]], "radius": 4})).unwrap();
    }
    let before = s.project.to_json();
    let result = s.execute("roto.clearStrokes", json!({"layer": layer, "object": 0})).unwrap();
    assert_eq!(result["removed"], 2, "only the object's foreground/background strokes are owned by it");
    assert_eq!(result["strokes"], 3);
    assert!(s.undo());
    assert_eq!(s.project.to_json(), before);
    for kind in ["refine", "refineErase"] {
        let result = s.execute("roto.clearStrokes", json!({"layer": layer, "kind": kind})).unwrap();
        assert_eq!(result["removed"], 1, "explicitly clearing shared {kind} still works");
    }
}

#[test]
fn clearing_strokes_rejects_invalid_object_filters_without_editing() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"width": 64, "height": 64, "duration": 1})).unwrap();
    let layer = s.execute("layer.newSolid", json!({})).unwrap()["layer"].as_u64().unwrap();
    s.execute("roto.stroke", json!({"layer": layer, "points": [[20, 20]], "radius": 4})).unwrap();
    let before = s.project.to_json();
    for object in [json!(-1), json!(-0.1), json!(32), json!(1e30), json!(null), json!("0")] {
        let result = s.execute("roto.clearStrokes", json!({"layer": layer, "object": object}));
        assert!(result.is_err(), "invalid object {object} must not delete object zero: {result:?}");
        assert_eq!(s.project.to_json(), before);
    }
}
