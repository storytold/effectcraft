//! RAM preview frames are keyed by content: editing one comp keeps the cached frames of another,
//! and undo finds the frames of the state it returns to.

use effectcraft_engine::Session;
use effectcraft_engine::project::ItemId;
use effectcraft_ui_egui::EffectcraftApp;
use egui_kittest::Harness;
use serde_json::json;

fn settle(h: &mut Harness<'_, EffectcraftApp>) {
    for _ in 0..600 {
        h.step();
        if h.state().frames.inflight() == 0 && h.state().frames.last_ms.lock().map(|v| *v > 0.0).unwrap_or(false) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    h.run_steps(2);
}

fn shown_cached(h: &Harness<'_, EffectcraftApp>, comp: u64) -> bool {
    let app = h.state();
    let key = effectcraft_ui_egui::frames::FrameKey { frame: 0, ..app.shown_series(ItemId(comp)) };
    app.frames.is_cached(&key)
}

#[test]
fn an_edit_in_another_comp_keeps_the_cached_frames_and_undo_finds_them_again() {
    let mut s = Session::default();
    let b = s.execute("comp.new", json!({"name": "B", "width": 64, "height": 36, "duration": 1})).unwrap()["comp"].as_u64().unwrap();
    let a = s.execute("comp.new", json!({"name": "A", "width": 64, "height": 36, "duration": 1})).unwrap()["comp"].as_u64().unwrap();
    s.execute("layer.newSolid", json!({"color": "#3080ff"})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_| EffectcraftApp::new(s));
    settle(&mut h);
    assert!(shown_cached(&h, a), "A's frame rendered");
    // Edit B (open it, add a layer), then come back to A: its frame is still in RAM.
    h.state_mut().session.execute("comp.open", json!({"comp": b})).unwrap();
    h.state_mut().session.execute("layer.newSolid", json!({"color": "#ff8030"})).unwrap();
    settle(&mut h);
    let b_frame = h.state().shown_series(ItemId(b));
    h.state_mut().session.execute("comp.open", json!({"comp": a})).unwrap();
    assert!(shown_cached(&h, a), "A's frame survived the edit in B");
    // Edit A, then undo: the frame of the state undo returns to is found again.
    let before = h.state().shown_series(ItemId(a));
    h.state_mut().session.execute("layer.newSolid", json!({"color": "#20c040"})).unwrap();
    settle(&mut h);
    assert_ne!(h.state().shown_series(ItemId(a)), before, "A's content changed");
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(h.state().shown_series(ItemId(a)), before);
    assert!(shown_cached(&h, a), "undo finds the earlier frame");
    // B's frame is still there too.
    assert!(h.state().frames.is_cached(&effectcraft_ui_egui::frames::FrameKey { frame: 0, ..b_frame }));
}

/// #284: the Timeline shows the cached frames as a green line under the work area bar, from
/// the first frame rendered on.
#[test]
fn the_timeline_shows_the_cached_frames_in_green() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Cache", "width": 64, "height": 36, "duration": 2})).unwrap();
    s.execute("layer.newSolid", json!({"color": "#3080ff"})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_| EffectcraftApp::new(s));
    settle(&mut h);
    h.run_steps(2);
    let green = h.state().tokens.cache_green;
    let bar = h.state().auto.find("timeline.workArea.bar").unwrap().rect;
    fn rects(s: &egui::Shape, out: &mut Vec<egui::epaint::RectShape>) {
        match s {
            egui::Shape::Rect(r) => out.push(r.clone()),
            egui::Shape::Vec(v) => v.iter().for_each(|s| rects(s, out)),
            _ => {}
        }
    }
    let mut all = vec![];
    for c in &h.output().shapes {
        rects(&c.shape, &mut all);
    }
    let line: Vec<egui::Rect> = all.iter().filter(|r| r.fill == green).map(|r| r.rect).collect();
    assert!(!line.is_empty(), "a green cache line is drawn");
    // Under the work area bar, at its start (frame 0 is cached), a visible width.
    let under = |r: &egui::Rect| r.min.y >= bar[1] + bar[3] - 1.0 && r.max.y <= bar[1] + bar[3] + 8.0;
    assert!(line.iter().any(|r| under(r) && (r.min.x - bar[0]).abs() < 2.0 && r.width() >= 1.0), "{line:?} bar {bar:?}");
}

/// Issue #65: while edits keep coming (a layer dragged in the viewer), only the viewer's frame
/// renders; prefetching the frames around it waits until the edits pause (they would be stale at
/// the next step, and would hold the CPU and GPU the next viewer frame needs), then resumes.
#[test]
fn prefetch_waits_while_edits_keep_coming() {
    let mut s = Session::default();
    let c = s.execute("comp.new", json!({"name": "Drag", "width": 64, "height": 36, "duration": 2})).unwrap()["comp"].as_u64().unwrap();
    let solid = s.execute("layer.newSolid", json!({"color": "#3080ff", "width": 16, "height": 16})).unwrap()["layer"].as_u64().unwrap();
    // Short steps between edits; the app clock below is advanced explicitly.
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(0.05).build_eframe(|_| EffectcraftApp::new(s));
    // Prefetch waits for EDIT_QUIET on the app clock; a manual clock makes the pause
    // deterministic instead of depending on how long the test host took to run the steps.
    h.state_mut().use_manual_clock();
    settle(&mut h);
    let frame = |h: &Harness<'_, EffectcraftApp>| {
        let app = h.state();
        let comp = app.session.project.comp(ItemId(c)).unwrap();
        comp.frame_rate.frame_at(app.session.time())
    };
    for i in 1..=8 {
        let v = json!([20.0 + 2.0 * i as f64, 18.0, 0.0]);
        h.state_mut().session.execute("prop.set", json!({"layer": solid, "path": "transform/position", "value": v, "merge": "drag"})).unwrap();
        h.step();
        // The viewer's frame renders...
        for _ in 0..500 {
            if !h.state().frames.urgent_pending() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        h.step();
        // ...and nothing else is queued or cached for this state of the comp.
        let series = h.state().shown_series(ItemId(c));
        assert_eq!(h.state().frames.cached_frames(&series), vec![frame(&h)], "edit {i}");
        assert_eq!(h.state().frames.inflight(), 0, "edit {i}");
    }
    // The edits pause: the frames around the current one fill in.
    h.state_mut().clock_mut().advance(1.0);
    h.run_steps(1);
    settle(&mut h);
    for _ in 0..20 {
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    settle(&mut h);
    let series = h.state().shown_series(ItemId(c));
    assert!(h.state().frames.cached_frames(&series).len() > 1, "prefetch resumed");
}

/// #595: at Third (and other resolutions that aren't a whole number of thousandths) the cache
/// bars find the rendered frames. The series they count was rebuilt from the frame key's scale
/// (333 → 0.333), whose render options hashed apart from the frames rendered at 0.3333….
#[test]
fn cached_frames_are_counted_at_one_third_resolution() {
    use effectcraft_ui_egui::state::Resolution;
    for res in [Resolution::Third, Resolution::Custom(7)] {
        let mut s = Session::default();
        let c = s.execute("comp.new", json!({"name": "Third", "width": 64, "height": 36, "duration": 1})).unwrap()["comp"].as_u64().unwrap();
        s.execute("layer.newSolid", json!({"color": "#3080ff"})).unwrap();
        // Each composition keeps its own Resolution (#417).
        if let Some(comp) = std::sync::Arc::make_mut(&mut s.project).comp_mut(ItemId(c)) {
            comp.resolution = res;
        }
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_| EffectcraftApp::new(s));
        settle(&mut h);
        let series = h.state().shown_series(ItemId(c));
        assert_eq!(series.scale, (res.scale(1.0, 1.0) * 1000.0).round() as u32, "{res:?}");
        assert!(shown_cached(&h, c), "{res:?}: the shown frame counts as cached");
        assert!(!h.state().frames.cached_frames(&series).is_empty(), "{res:?}");
    }
}
