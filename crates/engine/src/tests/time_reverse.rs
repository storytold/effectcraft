//! Layer ▸ Time ▸ Time-Reverse Layer (#3245): a reversed layer is the exact mirror of its span,
//! laid out as After Effects lays it out (and as an imported .aep has it): the start at the later
//! end, the In point at layer time D. Every source frame shows once, backwards, with nothing
//! skipped or repeated and no empty frame.

use std::sync::Arc;

use effectcraft_project::{Footage, FootageKind, FrameBlend, ItemId, ItemKind, Layer, LayerId};
use effectcraft_raster::Image;
use effectcraft_time::{FrameRate, Tick};
use serde_json::json;

use crate::Session;
use crate::render::{FootageSource, RenderOpts};

/// Frames of the test sequence.
const N: i64 = 10;

/// A sequence of `N` frames, 10 wide and 2 high: frame `i` is opaque white in column `i` only
/// and transparent elsewhere. There is no frame before the first or after the last.
struct Numbered;

impl FootageSource for Numbered {
    fn frame(&self, _: ItemId, f: &Footage, t: Tick) -> Option<Arc<Image>> {
        let i = f.frame_rate.frame_at(t);
        if !(0..N).contains(&i) {
            return None;
        }
        let mut img = Image::new(N as u32, 2);
        for y in 0..2 {
            img.set(i as u32, y, [1.0, 1.0, 1.0, 1.0]);
        }
        Some(Arc::new(img))
    }
}

/// A `fps` comp of one second with the 10 fps sequence as its only layer: (session, comp, layer).
fn scene(fps: u32) -> (Session, ItemId, u64) {
    let mut s = Session { footage: Arc::new(Numbered), ..Default::default() };
    let files: Vec<String> = (1..=N).map(|k| format!("/seq/f_{k:04}.png")).collect();
    let mut f = Footage {
        path: files[0].clone(),
        kind: FootageKind::Sequence,
        frame_rate: FrameRate::new(10, 1),
        sequence: files,
        width: N as u32,
        height: 2,
        has_video: true,
        ..Default::default()
    };
    f.sync_sequence_duration();
    assert_eq!(f.duration, Tick::from_seconds_f64(1.0));
    let item = Arc::make_mut(&mut s.project).add_item("f_[0001-0010].png", Default::default(), None, ItemKind::Footage(f));
    s.execute("comp.new", json!({"name": "C", "width": N, "height": 2, "frameRate": fps, "duration": 1})).unwrap();
    let cid = s.active_comp_id().unwrap();
    let l = s.execute("layer.addItem", json!({"item": item.0})).unwrap()["layer"].as_u64().unwrap();
    (s, cid, l)
}

fn layer(s: &Session, cid: ItemId, l: u64) -> Layer {
    s.project.comp(cid).unwrap().layer(LayerId(l)).unwrap().clone()
}

/// The source frame comp frame `k` shows, or `None` when it shows nothing; panics when it shows
/// more than one frame (a blend).
fn shown(s: &Session, cid: ItemId, k: i64) -> Option<i64> {
    let rate = s.project.comp(cid).unwrap().frame_rate;
    let img = s.render(cid, rate.tick_of(k), RenderOpts { scale: 1.0, ..Default::default() });
    let alpha = column_alpha(&img);
    let lit: Vec<i64> = (0..N).filter(|x| alpha[*x as usize] > 0.5).collect();
    let faint: Vec<i64> = (0..N).filter(|x| !lit.contains(x) && alpha[*x as usize] > 1e-3).collect();
    assert!(lit.len() <= 1 && faint.is_empty(), "comp frame {k}: columns {lit:?}, blended {faint:?}");
    lit.first().copied()
}

/// The highest alpha of each of the first `N` columns (the comp may be taller than the sequence:
/// sizes are rounded up to even).
fn column_alpha(img: &Image) -> Vec<f32> {
    (0..N).map(|x| (0..i64::from(img.height)).map(|y| img.get(x, y)[3]).fold(0.0, f32::max)).collect()
}

fn frames(s: &Session, cid: ItemId, count: i64) -> Vec<Option<i64>> {
    (0..count).map(|k| shown(s, cid, k)).collect()
}

#[test]
fn time_reverse_layer_mirrors_the_span_like_after_effects() {
    let (mut s, cid, l) = scene(10);
    assert_eq!(frames(&s, cid, N), (0..N).map(Some).collect::<Vec<_>>());
    let before = layer(&s, cid, l);
    s.execute("layer.timeReverse", json!({"layers": [l]})).unwrap();
    let r = layer(&s, cid, l);
    assert_eq!(r.stretch, -100.0);
    assert_eq!((r.in_point, r.out_point), (before.in_point, before.out_point), "the layer bar stays in place");
    // After Effects' layout (start at the later end): the In point is at layer time D, the Out
    // point at 0. The command used to put the In point one frame earlier (D - 1 frame).
    let one = Tick::from_seconds_f64(1.0);
    assert_eq!(r.start_time, before.out_point);
    assert_eq!((r.layer_time(r.in_point), r.layer_time(r.out_point)), (one, Tick::ZERO));
    // Comp frame 0 shows the last source frame, the last comp frame the first; every frame once.
    assert_eq!(frames(&s, cid, N), (0..N).rev().map(Some).collect::<Vec<_>>());
    // Time Remapping on the reversed layer keeps showing the same frames.
    s.execute("layer.enableTimeRemap", json!({"layers": [l]})).unwrap();
    assert_eq!(frames(&s, cid, N), (0..N).rev().map(Some).collect::<Vec<_>>());
}

#[test]
fn a_reversed_layer_in_a_faster_comp_holds_each_frame_as_long_as_forwards() {
    // 10 fps footage in a 30 fps comp: forwards each source frame shows for three comp frames,
    // reversed too, last frame first.
    let (mut s, cid, l) = scene(30);
    assert_eq!(frames(&s, cid, 3 * N), (0..3 * N).map(|k| Some(k / 3)).collect::<Vec<_>>());
    s.execute("layer.timeReverse", json!({"layers": [l]})).unwrap();
    assert_eq!(frames(&s, cid, 3 * N), (0..3 * N).map(|k| Some(N - 1 - k / 3)).collect::<Vec<_>>());
}

#[test]
fn an_after_effects_reversed_layer_shows_every_frame_backwards() {
    // Laid out as an .aep stores a reversed layer (981-983): -100 %, starting at its Out point.
    // It used to show nothing at comp frame 0 (layer time D, past the last frame), every other
    // frame one frame late, and never frame 0.
    for blend in [false, true] {
        let (mut s, cid, l) = scene(10);
        {
            let comp = Arc::make_mut(&mut s.project).comp_mut(cid).unwrap();
            comp.enable_frame_blending = blend;
            let ly = comp.layer_mut(LayerId(l)).unwrap();
            ly.stretch = -100.0;
            ly.start_time = ly.out_point;
            ly.switches.frame_blend = if blend { FrameBlend::FrameMix } else { FrameBlend::Off };
        }
        // With Frame Mix on, a reversed layer on the footage's own frames blends nothing.
        assert_eq!(frames(&s, cid, N), (0..N).rev().map(Some).collect::<Vec<_>>(), "frame blending {blend}");
    }
}

#[test]
fn reversing_twice_restores_the_layer_exactly() {
    for percent in [None, Some(150.0), Some(-50.0)] {
        let (mut s, cid, l) = scene(30);
        // A trimmed layer, so the span is not the whole source.
        Arc::make_mut(&mut s.project).comp_mut(cid).unwrap().layer_mut(LayerId(l)).unwrap().in_point = FrameRate::FPS_30.tick_of(6);
        if let Some(p) = percent {
            s.execute("layer.timeStretch", json!({"layers": [l], "percent": p})).unwrap();
        }
        let before = layer(&s, cid, l);
        s.execute("layer.timeReverse", json!({"layers": [l]})).unwrap();
        let once = layer(&s, cid, l);
        assert_eq!(once.stretch, -before.stretch);
        assert_eq!((once.in_point, once.out_point), (before.in_point, before.out_point));
        // The exact mirror at any stretch: each end shows the layer time the other end showed.
        assert_eq!(once.layer_time(once.in_point), before.layer_time(before.out_point), "{percent:?}");
        assert_eq!(once.layer_time(once.out_point), before.layer_time(before.in_point), "{percent:?}");
        s.execute("layer.timeReverse", json!({"layers": [l]})).unwrap();
        assert_eq!(layer(&s, cid, l), before, "{percent:?}");
    }
}

/// The alpha of each column of comp frame `k`.
fn columns(s: &Session, cid: ItemId, k: i64) -> Vec<f32> {
    let rate = s.project.comp(cid).unwrap().frame_rate;
    let img = s.render(cid, rate.tick_of(k), RenderOpts { scale: 1.0, ..Default::default() });
    column_alpha(&img)
}

/// Reverses layer `l` of `cid` (a 30 fps comp over the 10 fps sequence, directly or through a
/// precomp) and checks every comp frame is the mirror of the forward one: column `c` reversed is
/// column `N - 1 - c` forwards, blends included.
fn assert_mirrored(s: &mut Session, cid: ItemId, l: u64, what: &str) {
    let count = 3 * N;
    let forward: Vec<Vec<f32>> = (0..count).map(|k| columns(s, cid, k)).collect();
    assert!(forward.iter().all(|c| c.iter().any(|a| *a > 0.1)), "{what}: every forward frame shows the sequence");
    s.execute("layer.timeReverse", json!({"comp": cid.0, "layers": [l]})).unwrap();
    for (k, fwd) in forward.iter().enumerate() {
        let rev = columns(s, cid, k as i64);
        let want: Vec<f32> = fwd.iter().rev().copied().collect();
        assert!(rev.iter().zip(&want).all(|(a, b)| (a - b).abs() < 5e-3), "{what}: comp frame {k}: {rev:?}, want {want:?}");
    }
}

fn set_blending(s: &mut Session, cid: ItemId, l: u64, on: bool) {
    let comp = Arc::make_mut(&mut s.project).comp_mut(cid).unwrap();
    comp.enable_frame_blending = on;
    comp.layer_mut(LayerId(l)).unwrap().switches.frame_blend = if on { FrameBlend::FrameMix } else { FrameBlend::Off };
}

#[test]
fn reversed_footage_and_precomps_mirror_forward_playback_with_and_without_blending() {
    for blend in [false, true] {
        // The sequence in a 30 fps comp: each source frame for three comp frames, or blended.
        let (mut s, cid, l) = scene(30);
        set_blending(&mut s, cid, l, blend);
        assert_mirrored(&mut s, cid, l, &format!("footage, blending {blend}"));
        // The same through a 10 fps precomp of it: the precomp's frames, or blends of them.
        let (mut s, inner, _) = scene(10);
        s.execute("comp.new", json!({"name": "Outer", "width": N, "height": 2, "frameRate": 30, "duration": 1})).unwrap();
        let outer = s.active_comp_id().unwrap();
        let pre = s.execute("layer.addItem", json!({"item": inner.0})).unwrap()["layer"].as_u64().unwrap();
        set_blending(&mut s, outer, pre, blend);
        assert_mirrored(&mut s, outer, pre, &format!("precomp, blending {blend}"));
    }
}

#[test]
fn a_reversed_precomp_shows_its_own_frames_last_first() {
    // A 10 fps precomp whose 1 px solid moves one column per frame (column k at its frame k),
    // reversed in a 10 fps comp: comp frame k shows the precomp's frame 9 - k. Its keys are
    // evaluated on the precomp's frames, in step with footage in it; they used to be evaluated a
    // tick before the layer time (the In point: past the last frame, the solid off the right edge).
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Inner", "width": N, "height": 2, "frameRate": 10, "duration": 1})).unwrap();
    let inner = s.active_comp_id().unwrap();
    let solid = s.execute("layer.newSolid", json!({"color": "#ffffff", "width": 1, "height": 2})).unwrap()["layer"].as_u64().unwrap();
    for (t, x) in [(0.0, 0.5), (1.0, 10.5)] {
        s.execute("prop.addKey", json!({"layer": solid, "path": "transform/position", "time": t, "value": [x, 1.0]})).unwrap();
    }
    assert_eq!(frames(&s, inner, N), (0..N).map(Some).collect::<Vec<_>>());
    s.execute("comp.new", json!({"name": "Outer", "width": N, "height": 2, "frameRate": 10, "duration": 1})).unwrap();
    let outer = s.active_comp_id().unwrap();
    let pre = s.execute("layer.addItem", json!({"item": inner.0})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.timeReverse", json!({"layers": [pre]})).unwrap();
    assert_eq!(frames(&s, outer, N), (0..N).rev().map(Some).collect::<Vec<_>>());
}

#[test]
fn related_times_follow_the_frame_a_reversed_precomp_shows() {
    // Synchronize Time of All Related Items: comp frame 2 of a reversed 10 fps precomp shows its
    // frame 7 (layer time 0.8 s, the frame ending there), and back.
    let (mut s, inner, _) = scene(10);
    s.execute("comp.new", json!({"name": "Outer", "width": N, "height": 2, "frameRate": 10, "duration": 1})).unwrap();
    let outer = s.active_comp_id().unwrap();
    let pre = s.execute("layer.addItem", json!({"item": inner.0})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.timeReverse", json!({"layers": [pre]})).unwrap();
    let rate = FrameRate::new(10, 1);
    s.sync_related_times(outer, rate.tick_of(2));
    assert_eq!(s.state.times.get(&inner), Some(&rate.tick_of(7)));
    s.sync_related_times(inner, rate.tick_of(7));
    assert_eq!(s.state.times.get(&outer), Some(&rate.tick_of(2)));
}

#[test]
fn projects_saved_with_the_old_time_reverse_layout_open_in_after_effects_layout() {
    let (mut s, cid, l) = scene(10);
    // A second, trimmed reversed layer: either layout is possible, so it is only named.
    let t = s.execute("layer.addItem", json!({"item": layer(&s, cid, l).source.item().unwrap().0})).unwrap()["layer"].as_u64().unwrap();
    Arc::make_mut(&mut s.project).comp_mut(cid).unwrap().layer_mut(LayerId(t)).unwrap().in_point = FrameRate::new(10, 1).tick_of(2);
    s.execute("layer.timeReverse", json!({"layers": [l, t]})).unwrap();
    let new = layer(&s, cid, l);
    let trimmed = layer(&s, cid, t);
    // What Time-Reverse Layer made before #3245: the same layer one comp frame earlier.
    let fd = FrameRate::new(10, 1).frame_duration();
    for id in [l, t] {
        let ly = Arc::make_mut(&mut s.project).comp_mut(cid).unwrap().layer_mut(LayerId(id)).unwrap();
        ly.start_time -= fd;
    }
    // A file from before #3245 has no layout marker (it reads 0).
    Arc::make_mut(&mut s.project).settings.reverse_layout = 0;
    let dir = std::env::temp_dir().join(format!("ec-time-reverse-old-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("old.ecproj").to_string_lossy().to_string();
    s.execute("file.saveAs", json!({"path": path})).unwrap();
    let r = s.execute("file.open", json!({"path": path})).unwrap();
    let cid = s.state.active_comp.unwrap_or(cid);
    assert_eq!(layer(&s, cid, l).start_time, new.start_time, "the untrimmed layer is moved to the new layout");
    assert_eq!(layer(&s, cid, t).start_time, trimmed.start_time - fd, "the trimmed one is left as it is");
    assert_eq!(r["reversedLayers"]["moved"].as_array().unwrap().len(), 1, "{r}");
    assert_eq!(r["reversedLayers"]["unchanged"].as_array().unwrap().len(), 1, "{r}");
    // Checked once: saved again, it carries the marker and opens untouched.
    assert_eq!(s.project.settings.reverse_layout, effectcraft_project::REVERSE_LAYOUT_AE);
    s.execute("file.save", json!({})).unwrap();
    let r = s.execute("file.open", json!({"path": path})).unwrap();
    let cid = s.state.active_comp.unwrap_or(cid);
    assert_eq!(layer(&s, cid, l).start_time, new.start_time);
    assert_eq!(r["reversedLayers"], json!({"moved": [], "unchanged": []}));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_project_saved_since_the_fix_reopens_with_its_reversed_layers_untouched() {
    // Two reversed layers in After Effects' layout that look like the old one: an untrimmed layer
    // whose Out point is one frame past its source (layer time -1 frame there), and a trimmed one.
    // The file carries the layout marker, so neither moves nor is reported (they used to be: the
    // check went by the saving version, which is still 0.21.0 with the fix).
    let (mut s, cid, l) = scene(10);
    let fd = FrameRate::new(10, 1).frame_duration();
    let t = s.execute("layer.addItem", json!({"item": layer(&s, cid, l).source.item().unwrap().0})).unwrap()["layer"].as_u64().unwrap();
    Arc::make_mut(&mut s.project).comp_mut(cid).unwrap().layer_mut(LayerId(t)).unwrap().in_point = FrameRate::new(10, 1).tick_of(2);
    s.execute("layer.timeReverse", json!({"layers": [l, t]})).unwrap();
    Arc::make_mut(&mut s.project).comp_mut(cid).unwrap().layer_mut(LayerId(l)).unwrap().out_point += fd;
    let before = [layer(&s, cid, l), layer(&s, cid, t)];
    assert_eq!(before[0].layer_time(before[0].out_point), Tick::ZERO - fd, "the old layout's signature");
    let dir = std::env::temp_dir().join(format!("ec-time-reverse-new-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("new.ecproj").to_string_lossy().to_string();
    s.execute("file.saveAs", json!({"path": path})).unwrap();
    let r = s.execute("file.open", json!({"path": path})).unwrap();
    let cid = s.state.active_comp.unwrap_or(cid);
    assert_eq!([layer(&s, cid, l), layer(&s, cid, t)], before);
    assert_eq!(r["reversedLayers"], json!({"moved": [], "unchanged": []}));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn project_settings_without_the_layout_marker_read_as_the_old_layout() {
    let mut v = serde_json::to_value(effectcraft_project::ProjectSettings::default()).unwrap();
    assert_eq!(v["reverse_layout"], json!(effectcraft_project::REVERSE_LAYOUT_AE), "a new project has the current layout");
    v.as_object_mut().unwrap().remove("reverse_layout");
    let old: effectcraft_project::ProjectSettings = serde_json::from_value(v).unwrap();
    assert_eq!(old.reverse_layout, 0);
}

#[test]
fn reversing_a_remapped_layer_shows_every_frame_backwards_and_back() {
    // Time Remap on (keys at the identity), then Time-Reverse Layer: the keys are mirrored with
    // the span, so the In point names the source's end (D). A reversed layer reads the source
    // just before the time it names, remapped or not: comp frame 0 shows frame 9, not the frame
    // past the end (nothing, or the last frame held twice and frame 0 never shown).
    let (mut s, cid, l) = scene(10);
    s.execute("layer.enableTimeRemap", json!({"layers": [l]})).unwrap();
    let before = layer(&s, cid, l);
    s.execute("layer.timeReverse", json!({"layers": [l]})).unwrap();
    assert_eq!(frames(&s, cid, N), (0..N).rev().map(Some).collect::<Vec<_>>());
    // Reversing again restores the layer, keys included, and forward playback.
    s.execute("layer.timeReverse", json!({"layers": [l]})).unwrap();
    assert_eq!(layer(&s, cid, l), before);
    assert_eq!(frames(&s, cid, N), (0..N).map(Some).collect::<Vec<_>>());
}

/// Comp frames 0..3N of `cid` (column alphas) for reversed layer `l` with Frame Mix on, before
/// and after Enable Time Remapping, which keys it at the identity: they must be the same.
fn assert_remap_keeps_frames(s: &mut Session, cid: ItemId, l: u64, what: &str) {
    set_blending(s, cid, l, true);
    s.execute("layer.timeReverse", json!({"comp": cid.0, "layers": [l]})).unwrap();
    let count = 3 * N;
    let before: Vec<Vec<f32>> = (0..count).map(|k| columns(s, cid, k)).collect();
    s.execute("layer.enableTimeRemap", json!({"comp": cid.0, "layers": [l]})).unwrap();
    for (k, want) in before.iter().enumerate() {
        let got = columns(s, cid, k as i64);
        assert!(got.iter().zip(want).all(|(a, b)| (a - b).abs() < 5e-3), "{what}: comp frame {k}: {got:?}, was {want:?}");
    }
}

#[test]
fn enabling_time_remap_on_a_reversed_layer_keeps_its_blended_frames() {
    // 10 fps footage reversed in a 30 fps comp with Frame Mix: the remapped layer used to skip
    // the reversed blend (0.1 s showed mostly frame 9 instead of frame 8).
    let (mut s, cid, l) = scene(30);
    assert_remap_keeps_frames(&mut s, cid, l, "footage");
    // The same through an unproxied 10 fps precomp (Preserve Frame Rate off): it used to show
    // frame 9 at 0.1 s and nothing at its first frame.
    let (mut s, inner, _) = scene(10);
    s.execute("comp.new", json!({"name": "Outer", "width": N, "height": 2, "frameRate": 30, "duration": 1})).unwrap();
    let outer = s.active_comp_id().unwrap();
    let pre = s.execute("layer.addItem", json!({"item": inner.0})).unwrap()["layer"].as_u64().unwrap();
    assert_remap_keeps_frames(&mut s, outer, pre, "precomp");
}
