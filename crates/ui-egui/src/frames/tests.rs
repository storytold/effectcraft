use super::*;

#[test]
fn render_scales_are_the_factors_the_thousandths_stand_for() {
    assert_eq!(render_scale(333, Some(3)), 1.0 / 3.0);
    assert_eq!(render_scale(333, None), 1.0 / 3.0);
    assert_eq!(render_scale(167, Some(6)), 1.0 / 6.0);
    assert_eq!((render_scale(1000, None), render_scale(500, None), render_scale(250, Some(4))), (1.0, 0.5, 0.25));
    // 1/34 and 1/35 are both 29 thousandths: the comp's factor decides.
    assert_eq!((render_scale(29, Some(35)), render_scale(29, Some(34))), (1.0 / 35.0, 1.0 / 34.0));
    assert_eq!(render_scale(29, Some(2)), 1.0 / 34.0);
}

#[test]
fn converting_a_frame_with_nan_alpha_keeps_the_viewer_alive() {
    let image = effectcraft_engine::render::Image::filled(1, 1, [0.5, 0.25, 0.1, f32::NAN]);
    assert_eq!(to_color_image(&image).pixels, [egui::Color32::TRANSPARENT]);
}

/// Frames without the HDR step (`RenderOpts::skip_hdr_step`) never share a RAM or disk
/// cache key with the HDR-mapped frames of the same options.
#[test]
fn frames_without_the_hdr_step_have_keys_of_their_own() {
    let opts = RenderOpts::default();
    assert_ne!(opts_hash(&opts), opts_hash(&RenderOpts { skip_hdr_step: true, ..opts }));
}

struct ProbeDisk {
    keys: Mutex<HashSet<u128>>,
    probes: std::sync::atomic::AtomicUsize,
}

impl RemoteFrames for ProbeDisk {
    fn slots(&self) -> usize {
        0
    }

    fn disk(&self) -> bool {
        true
    }

    fn disk_contains(&self, key: u128) -> bool {
        self.probes.fetch_add(1, Ordering::Relaxed);
        self.keys.lock().unwrap().contains(&key)
    }

    fn start(&self, _: RemoteJob, _: RemoteDone) {}
}

fn disk_fixture(stored_frames: &[i64]) -> (Frames, RenderSource, FrameKey, RenderOpts, Arc<ProbeDisk>, u128) {
    let mut session = effectcraft_engine::Session::default();
    session.execute("comp.new", serde_json::json!({"width": 32, "height": 18, "duration": 1})).unwrap();
    let comp = session.active_comp_id().unwrap();
    let opts = RenderOpts::default();
    let content = 1234;
    let mut base = key(1, 0, opts_hash(&opts));
    base.revision = 7;
    base.comp = comp.0;
    let disk = Arc::new(ProbeDisk { keys: Mutex::new(HashSet::new()), probes: 0.into() });
    disk.keys.lock().unwrap().extend(stored_frames.iter().map(|frame| disk_cache::frame_key(content, *frame, base.scale, base.view ^ opts_hash(&opts))));
    let mut frames = Frames::default();
    frames.set_remote(Some(disk.clone()));
    frames.content_keys.lock().unwrap().insert((base.revision, base.comp, 0), content);
    let source = RenderSource {
        project: session.project.clone(),
        footage: session.footage.clone(),
        expr: None,
        layer_cache: Arc::default(),
        gpu: None,
        gpu_display: false,
        disk: None,
    };
    (frames, source, base, opts, disk, content)
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn disk_indicator_does_not_hash_on_ui_thread_and_coalesces_edits() {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    struct Disk(u128);
    impl RemoteFrames for Disk {
        fn slots(&self) -> usize {
            1
        }
        fn disk(&self) -> bool {
            true
        }
        fn disk_contains(&self, key: u128) -> bool {
            self.0 == key
        }
        fn start(&self, _: RemoteJob, done: RemoteDone) {
            done(Err("test has no renderer".into()));
        }
    }
    let mut s = effectcraft_engine::Session::default();
    s.execute("comp.new", serde_json::json!({"width":32,"height":18,"duration":1})).unwrap();
    let cid = s.active_comp_id().unwrap();
    let opts = RenderOpts::default();
    let stored = disk_cache::frame_key(disk_cache::comp_content_key(&s.project, cid), 2, 1000, opts_hash(&opts));
    let mut frames = Frames { pool: Some(rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap()), ..Default::default() };
    frames.set_remote(Some(Arc::new(Disk(stored))));
    let (entered, started) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    frames.pool.as_ref().unwrap().spawn(move || {
        entered.send(()).unwrap();
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
    });
    started.recv_timeout(Duration::from_secs(3)).unwrap();
    let src = RenderSource {
        project: s.project.clone(),
        footage: s.footage.clone(),
        expr: None,
        layer_cache: Arc::default(),
        gpu: None,
        gpu_display: false,
        disk: None,
    };
    let mut base = key(1, 0, 0);
    base.comp = cid.0;
    for revision in 1..50 {
        base.revision = revision;
        assert!(frames.disk_frames(&src, &base, 0..30, &opts).is_empty());
    }
    assert!(frames.content_keys.lock().unwrap().is_empty(), "painting computed a content hash while the worker was blocked");
    assert_eq!(frames.content_key_jobs.lock().unwrap().pending.len(), 1);
    release.send(()).unwrap();
    let until = Instant::now() + Duration::from_secs(3);
    while !frames.content_keys.lock().unwrap().contains_key(&(base.revision, base.comp, 0)) {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(frames.disk_frames(&src, &base, 0..30, &opts), vec![2], "the blue indicator must return once its key is ready");
    assert_eq!(frames.content_keys.lock().unwrap().len(), 1, "superseded revisions were hashed");
}

#[test]
fn disk_indicator_range_tracks_live_membership() {
    let (frames, src, base, opts, disk, _) = disk_fixture(&[10, 12]);
    assert_eq!(frames.disk_frames(&src, &base, 10..13, &opts), vec![10, 12]);
    assert_eq!(disk.probes.load(Ordering::Relaxed), 3);

    disk.keys.lock().unwrap().insert(disk_cache::frame_key(1234, 11, base.scale, base.view ^ opts_hash(&opts)));
    assert_eq!(frames.disk_frames(&src, &base, 10..13, &opts), vec![10, 11, 12]);

    insert(&frames, FrameKey { frame: 11, ..base });
    disk.probes.store(0, Ordering::Relaxed);
    assert_eq!(frames.disk_frames(&src, &base, 10..13, &opts), vec![10, 12]);
    assert_eq!(disk.probes.load(Ordering::Relaxed), 2);

    disk.keys.lock().unwrap().remove(&disk_cache::frame_key(1234, 12, base.scale, base.view ^ opts_hash(&opts)));
    assert_eq!(frames.disk_frames(&src, &base, 10..13, &opts), vec![10]);

    let mut revised = base;
    revised.revision += 1;
    frames.content_keys.lock().unwrap().insert((revised.revision, revised.comp, 0), 5678);
    assert!(frames.disk_frames(&src, &revised, 10..13, &opts).is_empty());

    let changed = FrameKey { scale: base.scale + 1, ..base };
    assert!(frames.disk_frames(&src, &changed, 10..13, &opts).is_empty());
    let changed = FrameKey { view: base.view + 1, ..base };
    assert!(frames.disk_frames(&src, &changed, 10..13, &opts).is_empty());
    assert!(frames.disk_frames(&src, &base, 10..13, &RenderOpts { draft: true, ..opts }).is_empty());
}

#[test]
fn disk_indicator_range_is_bounded() {
    let (frames, src, base, opts, disk, _) = disk_fixture(&[0, 1, 99_999]);
    assert_eq!(frames.disk_frames(&src, &base, -5..2, &opts), vec![0, 1]);
    assert_eq!(disk.probes.load(Ordering::Relaxed), 2);
    disk.probes.store(0, Ordering::Relaxed);
    assert_eq!(frames.disk_frames(&src, &base, 99_999..i64::MAX, &opts), vec![99_999]);
    assert_eq!(disk.probes.load(Ordering::Relaxed), 1);
    disk.probes.store(0, Ordering::Relaxed);
    assert!(frames.disk_frames(&src, &base, 2..2, &opts).is_empty());
    // Written as a struct literal: `3..2` trips clippy::reversed_empty_ranges.
    let reversed = std::ops::Range { start: 3, end: 2 };
    assert!(frames.disk_frames(&src, &base, reversed, &opts).is_empty());
    assert_eq!(disk.probes.load(Ordering::Relaxed), 0);
}

/// Frames larger than the texture limit are averaged down by a whole factor (#201: an
/// 11000×2200 comp at Full resolution against an 8192 limit).
#[test]
fn frames_over_the_texture_limit_are_averaged_down() {
    assert_eq!(fitted_size([11000, 2200], 8192), [5500, 1100]);
    assert_eq!(fitted_size([8192, 100], 8192), [8192, 100]);
    assert_eq!(fitted_size([30000, 30000], 16384), [15000, 15000]);
    assert_eq!(fitted_size([0, 0], 0), [0, 0]);
    // 5×2 (three columns of white, then black) into 2 px: 3×2 blocks (the last one 2×2).
    let w = egui::Color32::WHITE;
    let b = egui::Color32::from_rgba_premultiplied(0, 0, 0, 255);
    let img = egui::ColorImage::new([5, 2], vec![w, w, w, b, b, w, w, w, b, b]);
    let out = fit_texture(img.clone(), 2);
    assert_eq!(out.size, [2, 1]);
    assert_eq!(out.pixels, vec![w, b]);
    assert_eq!(fit_texture(img.clone(), 5).pixels, img.pixels);
    let mixed = fit_texture(egui::ColorImage::new([2, 1], vec![w, b]), 1);
    assert_eq!(mixed.pixels, vec![egui::Color32::from_rgba_premultiplied(128, 128, 128, 255)]);
}

#[test]
fn retirement_reaches_existing_workers_and_keeps_cpu_frames_and_completion_live() {
    let frames = Frames::default();
    let worker = frames.worker();
    let first = key(1, 0, 0);
    insert(&frames, first);
    frames.retire_gpu();
    assert!(worker.gpu_retired.load(Ordering::Acquire), "already queued workers share retirement");
    assert!(frames.is_cached(&first), "healthy materialized CPU frames survive");
    let next = key(1, 1, 0);
    frames.inflight.lock().unwrap().insert(next);
    frames.queue.lock().unwrap().running.insert(next);
    worker.finish_key(next, false, img(), web_time::Instant::now());
    assert!(frames.is_cached(&next), "CPU retry can still publish");
    assert_eq!(frames.inflight(), 0);
    assert!(frames.queue.lock().unwrap().running.is_empty());
}

fn key(content: u64, frame: i64, opts: u64) -> FrameKey {
    FrameKey { revision: content, content, comp: 1, frame, scale: 1000, view: 0, opts }
}

fn img() -> FrameImage {
    FrameImage::Cpu(Arc::new(egui::ColorImage::new([2, 2], vec![egui::Color32::BLACK; 4])))
}

fn insert(f: &Frames, k: FrameKey) {
    f.cache.lock().unwrap().insert(k, img());
}

/// A frame whose render panics (a GPU out of memory used to, on every frame thread) is
/// released, not left "in progress" forever, so the viewer can ask for it again (#106).
#[test]
fn a_frame_whose_render_panics_is_released() {
    use effectcraft_engine::Session;
    use effectcraft_engine::project::{Footage, FootageKind};
    use effectcraft_engine::render::Image;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[derive(Default)]
    struct Explodes(AtomicUsize);
    impl FootageSource for Explodes {
        fn frame(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<Image>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            panic!("a decoder bug");
        }
    }
    let mut s = Session::default();
    let cid = ItemId(s.execute("comp.new", json!({"name": "A", "width": 8, "height": 8, "duration": 1})).unwrap()["comp"].as_u64().unwrap());
    let clip = Footage { kind: FootageKind::Still, width: 8, height: 8, has_video: true, ..Default::default() };
    let item = Arc::make_mut(&mut s.project).add_item("clip.png", Default::default(), None, ItemKind::Footage(clip));
    s.execute("layer.addItem", json!({"item": item.0})).unwrap();
    let footage = Arc::new(Explodes::default());
    let src = RenderSource {
        project: s.project.clone(),
        footage: footage.clone(),
        expr: None,
        layer_cache: Arc::default(),
        gpu: None,
        gpu_display: false,
        disk: None,
    };
    let f = Frames::default();
    let key = FrameKey { revision: s.revision, content: f.content_of(&s.project, s.revision, cid), ..key(0, 0, 0) };
    let key = FrameKey { comp: cid.0, ..key };
    f.request(&src, key, cid, Tick::ZERO, RenderOpts::default());
    for _ in 0..1000 {
        if f.inflight() == 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(footage.0.load(Ordering::SeqCst) > 0, "the render reached the footage");
    assert_eq!(f.inflight(), 0, "released");
    assert!(!f.is_cached(&key));
}

/// #647: a pool thread waiting inside a render (on a parallel loop) never starts another
/// frame render on top of it. Dragging a layer queues a frame every UI frame; renders used to
/// nest on every thread, each holding its buffers, until memory ran out.
#[test]
fn a_waiting_render_never_starts_another_on_its_thread() {
    use effectcraft_engine::Session;
    use effectcraft_engine::project::{Footage, FootageKind};
    use effectcraft_engine::render::Image;
    use effectcraft_engine::time::FrameRate;
    use serde_json::json;
    use std::cell::Cell;
    use std::sync::atomic::AtomicUsize;
    thread_local! {
        static DEPTH: Cell<usize> = const { Cell::new(0) };
    }
    /// A decoder that works in parallel, as the renderer does: its thread joins the loop and
    /// may wait for the parts other threads took.
    #[derive(Default)]
    struct Parallel {
        calls: AtomicUsize,
        deepest: AtomicUsize,
    }
    impl FootageSource for Parallel {
        fn frame(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<Image>> {
            let d = DEPTH.get() + 1;
            DEPTH.set(d);
            self.deepest.fetch_max(d, Ordering::SeqCst);
            self.calls.fetch_add(1, Ordering::SeqCst);
            (0..64).into_par_iter().for_each(|_| std::thread::sleep(std::time::Duration::from_millis(1)));
            DEPTH.set(d - 1);
            Some(Arc::new(Image::new(4, 4)))
        }
    }
    let mut s = Session::default();
    let cid = ItemId(s.execute("comp.new", json!({"name": "A", "width": 8, "height": 8, "duration": 10})).unwrap()["comp"].as_u64().unwrap());
    let clip = Footage {
        kind: FootageKind::Video,
        width: 4,
        height: 4,
        has_video: true,
        frame_rate: FrameRate::FPS_30,
        duration: Tick::from_seconds_f64(10.0),
        ..Default::default()
    };
    let item = Arc::make_mut(&mut s.project).add_item("clip.mov", Default::default(), None, ItemKind::Footage(clip));
    s.execute("layer.addItem", json!({"item": item.0})).unwrap();
    let footage = Arc::new(Parallel::default());
    let src = RenderSource {
        project: s.project.clone(),
        footage: footage.clone(),
        expr: None,
        layer_cache: Arc::default(),
        gpu: None,
        gpu_display: false,
        disk: None,
    };
    let f = Frames::default();
    let content = f.content_of(&s.project, s.revision, cid);
    let frames = 60;
    for frame in 0..frames {
        let key = FrameKey { revision: s.revision, content, comp: cid.0, frame, scale: 1000, view: 0, opts: 0 };
        f.request(&src, key, cid, FrameRate::FPS_30.tick_of(frame), RenderOpts::default());
    }
    for _ in 0..4000 {
        if f.inflight() == 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(f.inflight(), 0, "every queued frame rendered");
    assert!(footage.calls.load(Ordering::SeqCst) >= frames as usize);
    assert_eq!(footage.deepest.load(Ordering::SeqCst), 1, "renders nested on a waiting thread");
}

/// #329: scrubbing a long movie, the viewer frame of every step the pointer passed started
/// rendering and seeked the movie's decoder in turn; the frame under the pointer waited for
/// all of them. A newer viewer frame of another time calls off the one rendering, which is
/// not cached (its footage may be missing).
#[test]
fn a_newer_viewer_frame_calls_off_the_one_rendering() {
    use effectcraft_engine::Session;
    use effectcraft_engine::project::{Footage, FootageKind};
    use effectcraft_engine::render::Image;
    use effectcraft_engine::time::FrameRate;
    use serde_json::json;
    use std::cell::RefCell;
    thread_local! {
        static CANCEL: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
    }
    /// A movie whose first frames take a long seek, which a called-off read gives up.
    #[derive(Default)]
    struct Seeking {
        seeking: AtomicBool,
        called_off: AtomicBool,
    }
    impl FootageSource for Seeking {
        fn cancellable(&self, cancel: &Arc<AtomicBool>, f: &mut dyn FnMut()) {
            let prev = CANCEL.replace(Some(cancel.clone()));
            f();
            CANCEL.set(prev);
        }
        fn frame(&self, _: ItemId, _: &Footage, t: Tick) -> Option<Arc<Image>> {
            if t < Tick::from_seconds_f64(0.25) {
                self.seeking.store(true, Ordering::SeqCst);
                for _ in 0..1000 {
                    if CANCEL.with_borrow(|c| c.as_ref().is_some_and(|c| c.load(Ordering::SeqCst))) {
                        self.called_off.store(true, Ordering::SeqCst);
                        return None;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }
            Some(Arc::new(Image::new(4, 4)))
        }
    }
    let mut s = Session::default();
    let cid = ItemId(s.execute("comp.new", json!({"name": "A", "width": 8, "height": 8, "duration": 10})).unwrap()["comp"].as_u64().unwrap());
    let clip = Footage {
        kind: FootageKind::Video,
        width: 4,
        height: 4,
        has_video: true,
        frame_rate: FrameRate::FPS_30,
        duration: Tick::from_seconds_f64(10.0),
        ..Default::default()
    };
    let item = Arc::make_mut(&mut s.project).add_item("clip.mov", Default::default(), None, ItemKind::Footage(clip));
    s.execute("layer.addItem", json!({"item": item.0})).unwrap();
    let footage = Arc::new(Seeking::default());
    let src = RenderSource {
        project: s.project.clone(),
        footage: footage.clone(),
        expr: None,
        layer_cache: Arc::default(),
        gpu: None,
        gpu_display: false,
        disk: None,
    };
    let f = Frames::default();
    let content = f.content_of(&s.project, s.revision, cid);
    let key = |frame| FrameKey { revision: s.revision, content, comp: cid.0, frame, scale: 1000, view: 0, opts: 0 };
    f.request_urgent(&src, key(0), cid, Tick::ZERO, RenderOpts::default());
    for _ in 0..1000 {
        if footage.seeking.load(Ordering::SeqCst) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(footage.seeking.load(Ordering::SeqCst), "the first frame's render reached the footage");
    f.request_urgent(&src, key(150), cid, FrameRate::FPS_30.tick_of(150), RenderOpts::default());
    for _ in 0..1000 {
        if f.inflight() == 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(footage.called_off.load(Ordering::SeqCst), "the superseded frame's seek ran on");
    assert!(f.is_cached(&key(150)), "the newest frame rendered");
    assert!(!f.is_cached(&key(0)), "a called-off frame is not cached");
    assert_eq!(f.inflight(), 0, "released");
}

/// #284: scrubbing asks for a viewer frame at every step; a newer one drops the comp's viewer
/// frames still waiting (they would all render first), but not prefetch frames or another
/// comp's viewer frame.
#[test]
fn a_newer_viewer_frame_drops_the_ones_still_waiting() {
    /// Starts nothing, so every request stays queued.
    struct Idle;
    impl RemoteFrames for Idle {
        fn slots(&self) -> usize {
            0
        }
        fn start(&self, _: RemoteJob, _: RemoteDone) {}
    }
    let mut f = Frames::default();
    f.set_remote(Some(Arc::new(Idle)));
    let src = RenderSource {
        project: Arc::default(),
        footage: Arc::new(effectcraft_engine::render::NoFootage),
        expr: None,
        layer_cache: Arc::default(),
        gpu: None,
        gpu_display: false,
        disk: None,
    };
    let request = |k: FrameKey, urgent: bool| {
        f.request_with(&src, k, ItemId(k.comp), Tick::ZERO, RenderOpts::default(), if urgent { Want::Viewer } else { Want::Prefetch })
    };
    let other = FrameKey { comp: 2, ..key(1, 0, 0) };
    request(other, true);
    request(key(1, 20, 0), false);
    for frame in 0..10 {
        request(key(1, frame, 0), true);
    }
    let mut queued: Vec<(u64, i64, bool)> = f.queue.lock().unwrap().jobs.iter().map(|j| (j.key.comp, j.key.frame, j.urgent)).collect();
    queued.sort();
    assert_eq!(queued, [(1, 9, true), (1, 20, false), (2, 0, false)]);
    assert_eq!(f.inflight(), 3, "dropped frames aren't in flight: the viewer can ask again");
}

/// One OOM burst halves once; separate episodes back off to a floor and healthy GPU work recovers.
#[test]
fn running_out_of_video_memory_halves_the_gpu_frame_budget() {
    let f = Frames::default();
    let mut c = f.cache.lock().unwrap();
    assert_eq!(c.gpu_budget, GPU_BUDGET);
    assert!(c.gpu_ready());
    c.gpu_out_of_memory();
    assert!(!c.gpu_ready(), "allocation failures must not be retried for every frame");
    assert_eq!(c.gpu_budget, GPU_BUDGET / 2);
    for _ in 0..20 {
        c.gpu_out_of_memory();
    }
    assert_eq!(c.gpu_budget, GPU_BUDGET / 2, "one burst must not collapse the shared budget");
    for _ in 0..20 {
        c.gpu_pressure = None;
        c.gpu_out_of_memory();
    }
    assert_eq!(c.gpu_budget, MIN_GPU_BUDGET);
    let before = web_time::Instant::now() - GPU_RECOVERY_INTERVAL;
    c.gpu_pressure = Some(before);
    assert!(c.gpu_ready(), "GPU attempts resume after the cooldown");
    c.gpu_adjusted = Some(before);
    c.gpu_succeeded();
    assert_eq!(c.gpu_budget, 2 * MIN_GPU_BUDGET);
    c.gpu_succeeded();
    assert_eq!(c.gpu_budget, 2 * MIN_GPU_BUDGET, "recovery is paced");
    drop(c);
    f.clear();
    assert_eq!(f.cache.lock().unwrap().gpu_budget, GPU_BUDGET);
}

#[test]
fn least_recently_shown_frames_go_first() {
    let f = Frames::default();
    f.set_budget(3 * 16);
    for i in 0..3 {
        insert(&f, key(1, i, 0));
    }
    // Frame 0 is shown again, so frame 1 is now the oldest.
    assert!(f.get(&key(1, 0, 0)).is_some());
    insert(&f, key(1, 3, 0));
    assert!(f.is_cached(&key(1, 0, 0)) && !f.is_cached(&key(1, 1, 0)));
    assert!(f.is_cached(&key(1, 2, 0)) && f.is_cached(&key(1, 3, 0)));
}

fn playhead(cur: i64, looping: bool) -> Playhead {
    Playhead { series: key(1, 0, 0), cur, start: 0, end: 99, step: 1, looping }
}

/// #595: a playing preview's cache is full. The frames just ahead of the playhead were
/// rendered before the ones just shown, so plain LRU evicted them first and the preview
/// played into a gap. Frames the preview no longer reaches go first, then the ones it
/// reaches last.
#[test]
fn a_full_cache_keeps_the_frames_just_ahead_of_the_playhead() {
    let f = Frames::default();
    f.set_budget(4 * 16);
    // Prefetch rendered 11..=13, then 10 was shown (the most recently used).
    for i in [11, 12, 13, 10] {
        insert(&f, key(1, i, 0));
    }
    let cached = |f: &Frames| f.cached_frames(&key(1, 0, 0));
    f.set_playhead(Some(playhead(10, false)));
    // The next frame rendered is the one the preview reaches last: the frames just ahead stay.
    insert(&f, key(1, 14, 0));
    assert_eq!(cached(&f), [10, 11, 12, 13]);
    // Without looping, a frame behind the playhead goes before any frame ahead of it.
    f.set_playhead(Some(playhead(12, false)));
    insert(&f, key(1, 14, 0));
    assert_eq!(cached(&f), [10, 12, 13, 14]);
    // Looping, the frame just behind is the one reached last.
    f.set_playhead(Some(playhead(13, true)));
    insert(&f, key(1, 0, 0));
    assert_eq!(cached(&f), [0, 10, 13, 14]);
    // Stopped: least recently shown first again.
    f.set_playhead(None);
    insert(&f, key(1, 50, 0));
    assert!(!f.is_cached(&key(1, 13, 0)) && f.is_cached(&key(1, 50, 0)));
}

/// A frame source that queues jobs and never renders them: the tests check the queue.
struct Idle;
impl RemoteFrames for Idle {
    fn slots(&self) -> usize {
        0
    }
    fn start(&self, _: RemoteJob, _: RemoteDone) {}
}

fn idle_source() -> RenderSource {
    RenderSource {
        project: Arc::default(),
        footage: Arc::new(effectcraft_engine::render::NoFootage),
        expr: None,
        layer_cache: Arc::default(),
        gpu: None,
        gpu_display: false,
        disk: None,
    }
}

/// #595: prefetch renders the frames the preview reaches first, whatever order they were
/// asked for in (a frame evicted and asked for again waited behind every frame further
/// ahead); queued frames the preview no longer reaches are dropped and not left in flight.
#[test]
fn prefetch_renders_the_frames_the_playhead_reaches_first() {
    let mut f = Frames::default();
    f.set_remote(Some(Arc::new(Idle)));
    let src = idle_source();
    for frame in [40, 30, 20, 5, 12, 11] {
        f.request(&src, key(1, frame, 0), ItemId(1), Tick::ZERO, RenderOpts::default());
    }
    f.request(&src, FrameKey { comp: 2, ..key(1, 1, 0) }, ItemId(2), Tick::ZERO, RenderOpts::default());
    f.set_playhead(Some(playhead(10, false)));
    let mut order = vec![];
    loop {
        let mut q = f.queue.lock().unwrap();
        let Some(i) = q.best() else { break };
        let j = q.jobs.swap_remove(i);
        order.push((j.key.comp, j.key.frame));
    }
    assert_eq!(order, [(1, 11), (1, 12), (1, 20), (1, 30), (1, 40), (2, 1)]);
    assert_eq!(f.inflight(), 6, "frame 5, behind the playhead, is no longer in flight");
}

/// A decoder that works in parallel, as the renderer does, in a comp that shows it.
#[derive(Default)]
struct ParallelClip {
    started: std::sync::atomic::AtomicUsize,
    /// Frames started by the time frame 1 was done.
    started_at_first: std::sync::atomic::AtomicUsize,
}

impl FootageSource for ParallelClip {
    fn frame(&self, _: ItemId, _: &effectcraft_engine::project::Footage, t: Tick) -> Option<Arc<effectcraft_engine::render::Image>> {
        self.started.fetch_add(1, Ordering::SeqCst);
        (0..16).into_par_iter().for_each(|_| (0..4).into_par_iter().for_each(|_| std::thread::sleep(std::time::Duration::from_millis(1))));
        if effectcraft_engine::time::FrameRate::FPS_30.frame_at(t) == 1 {
            self.started_at_first.store(self.started.load(Ordering::SeqCst), Ordering::SeqCst);
        }
        Some(Arc::new(effectcraft_engine::render::Image::new(4, 4)))
    }
}

/// A 10 s comp of a [`ParallelClip`]: the frame cache, the clip, the render source, the comp
/// and the key of its frame 0.
fn parallel_clip() -> (Frames, Arc<ParallelClip>, RenderSource, ItemId, FrameKey) {
    use effectcraft_engine::Session;
    use effectcraft_engine::project::{Footage, FootageKind};
    use effectcraft_engine::time::FrameRate;
    use serde_json::json;
    let mut s = Session::default();
    let cid = ItemId(s.execute("comp.new", json!({"name": "A", "width": 8, "height": 8, "duration": 10})).unwrap()["comp"].as_u64().unwrap());
    let clip = Footage {
        kind: FootageKind::Video,
        width: 4,
        height: 4,
        has_video: true,
        frame_rate: FrameRate::FPS_30,
        duration: Tick::from_seconds_f64(10.0),
        ..Default::default()
    };
    let item = Arc::make_mut(&mut s.project).add_item("clip.mov", Default::default(), None, ItemKind::Footage(clip));
    s.execute("layer.addItem", json!({"item": item.0})).unwrap();
    let footage = Arc::new(ParallelClip::default());
    let src = RenderSource {
        project: s.project.clone(),
        footage: footage.clone(),
        expr: None,
        layer_cache: Arc::default(),
        gpu: None,
        gpu_display: false,
        disk: None,
    };
    let f = Frames::default();
    let series = FrameKey { revision: s.revision, content: f.content_of(&s.project, s.revision, cid), comp: cid.0, frame: 0, scale: 1000, view: 0, opts: 0 };
    (f, footage, src, cid, series)
}

fn wait_for_renders(f: &Frames) {
    for _ in 0..8000 {
        if f.inflight() == 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(f.inflight(), 0, "every queued frame rendered");
}

/// A playing preview's frames render side by side, and each works in parallel: a render
/// waiting for its own parallel work took over pieces of the frames started after it, and
/// its frame was done only once theirs were. The frames right ahead of the playhead, started
/// first, came last: the preview stood still while the frames further ahead turned green.
/// Cache Frames When Idle left the same gaps.
#[test]
fn frames_started_first_are_done_first() {
    use effectcraft_engine::time::FrameRate;
    for playing in [true, false] {
        let (f, footage, src, cid, series) = parallel_clip();
        let frames = 60;
        if playing {
            f.set_playhead(Some(Playhead { series, cur: 0, start: 0, end: 299, step: 1, looping: false }));
        }
        for frame in 1..=frames {
            let (key, t) = (FrameKey { frame, ..series }, FrameRate::FPS_30.tick_of(frame));
            if playing {
                f.request(&src, key, cid, t, RenderOpts::default());
            } else {
                f.request_background(&src, key, cid, t, RenderOpts::default());
            }
            if frame == 1 {
                // Its parallel work has started (and idle threads have taken pieces of it).
                std::thread::sleep(std::time::Duration::from_millis(3));
            }
        }
        wait_for_renders(&f);
        assert_eq!(f.cached_frames(&series).len(), frames as usize);
        let before = footage.started_at_first.load(Ordering::SeqCst);
        assert!(before <= 2 * f.parallelism(), "{before} frames were started before the first one was done (playing: {playing})");
    }
}

/// Prefetch frames queued when a preview starts or stops, or that become the viewer's
/// frame, still render: they change hands between the lanes and the pool.
#[test]
fn queued_frames_render_when_a_preview_starts_or_stops() {
    use effectcraft_engine::time::FrameRate;
    let (f, _, src, cid, series) = parallel_clip();
    let playhead = Playhead { series, cur: 0, start: 0, end: 299, step: 1, looping: false };
    let request = |frames: std::ops::Range<i64>| {
        for frame in frames {
            f.request(&src, FrameKey { frame, ..series }, cid, FrameRate::FPS_30.tick_of(frame), RenderOpts::default());
        }
    };
    // Queued for the pool, then a preview starts.
    request(1..40);
    f.set_playhead(Some(playhead));
    // Queued for the lanes, then the preview stops.
    request(40..80);
    f.set_playhead(None);
    wait_for_renders(&f);
    // Queued for the lanes, then the viewer shows one of them.
    for frame in 80..120 {
        f.request_background(&src, FrameKey { frame, ..series }, cid, FrameRate::FPS_30.tick_of(frame), RenderOpts::default());
    }
    f.request_urgent(&src, FrameKey { frame: 119, ..series }, cid, FrameRate::FPS_30.tick_of(119), RenderOpts::default());
    wait_for_renders(&f);
    assert_eq!(f.cached_frames(&series).len(), 119);
}

/// The cache is full. A zoom (or preview-resolution) change left the frames of the old
/// size to the plain least-recently-shown order, so the frames of the zoom the viewer is back
/// at went before the ones of the abandoned zoom. The other size's frames go first, then the
/// order is as before.
#[test]
fn a_size_change_evicts_frames_of_other_sizes_first() {
    let mut f = Frames::default();
    f.set_remote(Some(Arc::new(Idle)));
    f.set_budget(4 * 16);
    let at = |frame, scale| FrameKey { scale, ..key(1, frame, 0) };
    // 100% (frames 0..=1) is cached, then 50% (2..=3): the 100% frames are the oldest.
    for (frame, scale) in [(0, 1000), (1, 1000), (2, 500), (3, 500)] {
        insert(&f, at(frame, scale));
    }
    // A passive viewer at 50% asks for a frame: that doesn't change which size is kept.
    f.request(&idle_source(), at(9, 500), ItemId(1), Tick::ZERO, RenderOpts::default());
    // Back at 100%: the viewer asks for frame 4.
    f.request_urgent(&idle_source(), at(4, 1000), ItemId(1), Tick::ZERO, RenderOpts::default());
    f.request(&idle_source(), at(10, 500), ItemId(1), Tick::ZERO, RenderOpts::default());
    insert(&f, at(4, 1000));
    assert!(
        f.is_cached(&at(0, 1000)) && f.is_cached(&at(1, 1000)) && !f.is_cached(&at(2, 500)) && f.is_cached(&at(3, 500)),
        "the first frame of the old size goes, not the cached ones of the current size"
    );
    insert(&f, at(5, 1000));
    assert!(!f.is_cached(&at(3, 500)), "then the last frame of the old size");
    assert_eq!(f.cached_frames(&at(0, 1000)), [0, 1, 4, 5]);
    // No other size left: least recently shown first again.
    insert(&f, at(6, 1000));
    assert_eq!(f.cached_frames(&at(0, 1000)), [1, 4, 5, 6]);
}

#[test]
fn frames_are_keyed_by_content_and_series_are_kept_apart() {
    let f = Frames::default();
    insert(&f, key(1, 0, 0));
    insert(&f, key(2, 0, 0));
    insert(&f, key(2, 1, 7));
    // Other content or render options (Draft, shadows…) are another series.
    assert_eq!(f.cached_frames(&key(2, 0, 0)), vec![0]);
    assert_eq!(f.cached_frames(&key(2, 0, 7)), vec![1]);
    // A later revision with the same content finds the frame (an edit elsewhere, an undo).
    assert!(f.is_cached(&FrameKey { revision: 99, ..key(1, 0, 0) }));
    assert!(!f.is_cached(&key(3, 0, 0)));
}

#[test]
fn live_edits_present_the_newest_completed_revision_without_going_backwards() {
    let f = Frames::default();
    let shown = key(1, 0, 0);
    let newest = key(3, 0, 0);
    let wanted = key(5, 0, 0);
    insert(&f, shown);
    insert(&f, newest);
    insert(&f, key(2, 0, 0)); // Older worker finishes after the newer one.
    let displayed = f.get_for_viewer(&wanted, Some(&shown)).unwrap().0;
    assert_eq!(displayed, newest);
    assert!(f.get_for_viewer(&wanted, Some(&displayed)).is_none(), "never replay an older completion");
    insert(&f, wanted);
    assert_eq!(f.get_for_viewer(&wanted, Some(&displayed)).unwrap().0, wanted, "settle on the exact final edit");
    // Undo reuses the content of an earlier revision immediately.
    let undo = FrameKey { revision: 6, ..shown };
    let reused = f.get_for_viewer(&undo, Some(&wanted)).unwrap().0;
    assert_eq!(reused.content, shown.content);
    assert_eq!(reused.revision, undo.revision);
}

#[test]
fn live_edits_do_not_substitute_other_times_views_or_render_settings() {
    let f = Frames::default();
    let shown = key(1, 0, 0);
    let wanted = key(3, 0, 0);
    let done = key(2, 0, 0);
    for mismatch in [
        FrameKey { comp: 9, ..done },
        FrameKey { frame: 1, ..done },
        FrameKey { scale: 500, ..done },
        FrameKey { view: 1, ..done },
        FrameKey { opts: 1, ..done },
        FrameKey { revision: 4, ..done },
    ] {
        f.clear();
        insert(&f, mismatch);
        assert!(f.get_for_viewer(&wanted, Some(&shown)).is_none(), "wrong candidate: {mismatch:?}");
    }
    f.clear();
    insert(&f, done);
    assert!(f.get_for_viewer(&wanted, None).is_none(), "no stale content on opening a viewer");
    assert!(f.get_for_viewer(&wanted, Some(&FrameKey { comp: 9, ..shown })).is_none());
    assert!(f.get_for_viewer(&wanted, Some(&FrameKey { frame: 1, ..shown })).is_none());
    assert!(f.get_for_viewer(&wanted, Some(&FrameKey { view: 1, ..shown })).is_none());
}

#[test]
fn an_edit_keeps_the_frames_of_comps_it_does_not_touch() {
    use effectcraft_engine::Session;
    use serde_json::json;
    let mut s = Session::default();
    let a = ItemId(s.execute("comp.new", json!({"name": "A", "width": 8, "height": 8, "duration": 1})).unwrap()["comp"].as_u64().unwrap());
    let b = ItemId(s.execute("comp.new", json!({"name": "B", "width": 8, "height": 8, "duration": 1})).unwrap()["comp"].as_u64().unwrap());
    let f = Frames::default();
    let at = |s: &Session, c: ItemId| f.content_of(&s.project, s.revision, c);
    let (a0, b0) = (at(&s, a), at(&s, b));
    // Edit B: A's content (and frames) stay, B's change.
    s.execute("layer.newSolid", json!({"comp": b.0, "color": "#ff0000"})).unwrap();
    assert_eq!(at(&s, a), a0);
    assert_ne!(at(&s, b), b0);
    // Undo: B is back to the content its old frames were rendered for.
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(at(&s, b), b0);
}

#[test]
fn a_long_drag_keeps_a_bounded_number_of_states() {
    use effectcraft_engine::Session;
    use serde_json::json;
    let mut s = Session::default();
    let a = ItemId(s.execute("comp.new", json!({"name": "A", "width": 8, "height": 8, "duration": 1})).unwrap()["comp"].as_u64().unwrap());
    let f = Frames::default();
    let mut last = None;
    for i in 0..200 {
        s.execute("comp.settings", json!({"width": 8 + i, "merge": "drag"})).unwrap();
        let k = FrameKey { content: f.content_of(&s.project, s.revision, a), ..key(0, 0, 0) };
        insert(&f, k);
        last = Some(k);
    }
    // The snapshots and the frames kept stay bounded (a dropped identity's frames go with it,
    // so a later state that reuses its addresses can't be shown an old frame).
    assert!(f.keepers.lock().unwrap().1.len() <= MAX_KEEPERS);
    assert!(f.cache.lock().unwrap().map.len() <= MAX_KEEPERS);
    assert!(f.is_cached(&last.unwrap()), "the current state's frame stays");
}

#[test]
fn an_identity_keeps_its_comps_so_an_edit_in_place_still_changes_it() {
    use effectcraft_engine::Session;
    use serde_json::json;
    let mut s = Session::default();
    let a = ItemId(s.execute("comp.new", json!({"name": "A", "width": 8, "height": 8, "duration": 1})).unwrap()["comp"].as_u64().unwrap());
    // A project nothing else holds (no undo history): make_mut would edit the comp in place,
    // at the same address, if the identity hadn't kept a snapshot holding it.
    let mut p = Arc::new((*s.project).clone());
    drop(s);
    let f = Frames::default();
    let before = f.content_of(&p, 1, a);
    let comp = Arc::make_mut(&mut p).comp_mut(a).unwrap();
    comp.width = 16;
    assert_ne!(f.content_of(&p, 2, a), before);
}

/// Audio, Lock and Shy (and Hide Shy Layers) don't change pixels: toggling them, in the comp
/// or a comp it nests, keeps its frames, also when toggled back (#103).
#[test]
fn audio_lock_and_shy_switches_keep_the_frames() {
    use effectcraft_engine::Session;
    use serde_json::json;
    let mut s = Session::default();
    let id = |v: serde_json::Value, k: &str| v[k].as_u64().unwrap();
    let inner = ItemId(id(s.execute("comp.new", json!({"name": "Inner", "width": 8, "height": 8, "duration": 1})).unwrap(), "comp"));
    let solid = id(s.execute("layer.newSolid", json!({"comp": inner.0, "color": "#ff0000"})).unwrap(), "layer");
    let outer = ItemId(id(s.execute("comp.new", json!({"name": "Outer", "width": 8, "height": 8, "duration": 1})).unwrap(), "comp"));
    let nested = id(s.execute("layer.addItem", json!({"comp": outer.0, "item": inner.0})).unwrap(), "layer");
    let f = Frames::default();
    let at = |s: &Session| f.content_of(&s.project, s.revision, outer);
    let o0 = at(&s);
    let switch = |s: &mut Session, comp: ItemId, layer: u64, sw: &str, v: bool| {
        s.execute("layer.setSwitch", json!({"comp": comp.0, "layers": [layer], "switch": sw, "value": v})).unwrap();
    };
    for (comp, layer, sw) in [(outer, nested, "audio"), (outer, nested, "lock"), (outer, nested, "shy"), (inner, solid, "audio"), (inner, solid, "shy")] {
        // Audio off; Lock and Shy on.
        switch(&mut s, comp, layer, sw, sw != "audio");
        assert_eq!(at(&s), o0, "{sw} on in {comp:?}");
    }
    s.execute("comp.setSwitch", json!({"comp": outer.0, "switch": "hideShy", "value": true})).unwrap();
    assert_eq!(at(&s), o0, "Hide Shy Layers");
    switch(&mut s, outer, nested, "audio", true);
    switch(&mut s, outer, nested, "lock", false);
    assert_eq!(at(&s), o0, "toggled back");
    // Switches that change pixels still do.
    switch(&mut s, inner, solid, "video", false);
    let o1 = at(&s);
    assert_ne!(o1, o0, "Video");
    switch(&mut s, outer, nested, "audio", false);
    assert_eq!(at(&s), o1, "a mute after a real edit keeps that edit's frames");
    switch(&mut s, inner, solid, "video", true);
    assert_ne!(at(&s), o1);
}

/// A cut or trim recaches only the frames of the layers it touched (#1641), and those of the
/// layers the change reaches (here a child of the trimmed layer).
#[test]
fn an_edit_keeps_the_frames_outside_the_layers_it_changed() {
    use effectcraft_engine::Session;
    use serde_json::json;
    let mut s = Session::default();
    let id = |v: serde_json::Value, k: &str| v[k].as_u64().unwrap();
    let comp = ItemId(id(s.execute("comp.new", json!({"name": "Edit", "width": 8, "height": 8, "duration": 10})).unwrap(), "comp"));
    let a = id(s.execute("layer.newSolid", json!({"comp": comp.0, "color": "#ff0000"})).unwrap(), "layer");
    let b = id(s.execute("layer.newSolid", json!({"comp": comp.0, "color": "#00ff00"})).unwrap(), "layer");
    s.execute("layer.timing", json!({"layers": [a], "in": 0, "out": 4})).unwrap();
    s.execute("layer.timing", json!({"layers": [b], "in": 4, "out": 10})).unwrap();
    let (fr, out) = s.project.comp(comp).map(|c| (c.frame_rate, c.layers.iter().find(|l| l.id.0 == a).unwrap().out_point)).unwrap();
    let frames: Vec<i64> = (0..fr.frame_at(Tick::from_seconds_f64(10.0))).collect();
    let f = Frames::default();
    let cache_all = |s: &Session| {
        let c = f.content_of(&s.project, s.revision, comp);
        frames.iter().for_each(|&n| insert(&f, key(c, n, 0)));
        c
    };
    let cached = |c: u64| frames.iter().filter(|&&n| f.is_cached(&key(c, n, 0))).copied().collect::<Vec<_>>();
    let before = cache_all(&s);
    s.execute("time.set", json!({"time": 2.0})).unwrap();
    s.execute("edit.splitLayer", json!({"layers": [a]})).unwrap();
    let after = f.content_of(&s.project, s.revision, comp);
    assert_ne!(after, before);
    let kept: Vec<i64> = (0..fr.frame_at(Tick::from_seconds_f64(2.0)) - 1).chain(fr.frame_at(out) + 2..frames.len() as i64).collect();
    assert_eq!(cached(after), kept, "the copy's half (and one frame either side) renders again; the rest stay");

    s.execute("layer.setParent", json!({"layers": [b], "parent": a})).unwrap();
    let parented = cache_all(&s);
    s.execute("layer.timing", json!({"layers": [a], "out": 1})).unwrap();
    // The cut (1–2 s) and the trimmed layer's child (4–10 s) render again, a frame either side;
    // the split's copy (2–4 s) stays.
    assert_eq!(cached(f.content_of(&s.project, s.revision, comp)), (0..=27).chain(62..=117).collect::<Vec<_>>(), "the trimmed layer's child moves where it is");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(f.content_of(&s.project, s.revision, comp), parented);
    assert_eq!(cached(parented).len(), frames.len(), "undo finds every earlier frame");
}

/// Every frame an edit keeps renders as a fresh render of the edited project does: Solo
/// hides the other layers at every frame, and a camera reaches the frames whose shutter
/// samples it (#1641 review).
#[test]
fn the_frames_an_edit_keeps_match_a_fresh_render() {
    use effectcraft_engine::Session;
    use effectcraft_engine::render::NoFootage;
    use serde_json::json;
    // Cache every frame of `comp`, run `edit`, check the frames kept against fresh renders.
    fn kept(s: &mut Session, comp: ItemId, edit: serde_json::Value) -> Vec<i64> {
        let (fr, n) = s.project.comp(comp).map(|c| (c.frame_rate, c.frame_rate.frame_at(c.duration))).unwrap();
        let f = Frames::default();
        let before = f.content_of(&s.project, s.revision, comp);
        (0..n).for_each(|i| insert(&f, key(before, i, 0)));
        let old = s.project.clone();
        s.execute(edit["cmd"].as_str().unwrap(), edit["params"].clone()).unwrap();
        let after = f.content_of(&s.project, s.revision, comp);
        let kept: Vec<i64> = (0..n).filter(|&i| f.is_cached(&key(after, i, 0))).collect();
        let render = |p, i| Renderer::new(p, &NoFootage, RenderOpts::default()).comp_frame(comp, fr.tick_of(i));
        for &i in &kept {
            assert_eq!(render(&old, i), render(&s.project, i), "kept frame {i} draws other pixels now");
        }
        kept
    }
    let id = |v: serde_json::Value, k: &str| v[k].as_u64().unwrap();
    let mut s = Session::default();

    // Solo on a 2–3 s layer hides the 10 s underlay everywhere; turning it off shows it again.
    let comp = ItemId(id(s.execute("comp.new", json!({"name": "Solo", "width": 8, "height": 8, "duration": 10})).unwrap(), "comp"));
    s.execute("layer.newSolid", json!({"comp": comp.0, "color": "#ff0000"})).unwrap();
    let short = id(s.execute("layer.newSolid", json!({"comp": comp.0, "color": "#00ff00"})).unwrap(), "layer");
    s.execute("layer.timing", json!({"layers": [short], "in": 2, "out": 3})).unwrap();
    let solo = |on: bool| json!({"cmd": "layer.setSwitch", "params": {"layers": [short], "switch": "solo", "value": on}});
    assert_eq!(kept(&mut s, comp, solo(true)), Vec::<i64>::new());
    assert_eq!(kept(&mut s, comp, solo(false)), Vec::<i64>::new());

    // The work area is a preview and render range: setting it (B, N) keeps every frame, on
    // disk too.
    let n = s.project.comp(comp).map(|c| c.frame_rate.frame_at(c.duration)).unwrap();
    let disk = |s: &Session| effectcraft_engine::render::disk_cache::comp_content_key_with(&s.project, comp, None);
    let d0 = disk(&s);
    let work_area = |start: f64, end: f64| json!({"cmd": "comp.workArea", "params": {"comp": comp.0, "start": start, "end": end}});
    assert_eq!(kept(&mut s, comp, work_area(1.0, 4.0)), (0..n).collect::<Vec<_>>());
    assert_eq!(kept(&mut s, comp, work_area(2.0, 3.0)), (0..n).collect::<Vec<_>>());
    assert_eq!(disk(&s), d0, "the disk cache keeps them");

    // A 2–3 s camera over a 4 s motion-blurred 3D layer, shutter 720° at phase 360°: a frame
    // samples 1 to 3 frames ahead, so moving the camera reaches the 3 frames before it.
    let comp = ItemId(id(
        s.execute("comp.new", json!({"name": "Cam", "width": 16, "height": 16, "duration": 4, "frameRate": 30, "shutterAngle": 720, "shutterPhase": 360}))
            .unwrap(),
        "comp",
    ));
    let plane = id(s.execute("layer.newSolid", json!({"comp": comp.0, "color": "#ff0000"})).unwrap(), "layer");
    for sw in ["threeD", "motionBlur"] {
        s.execute("layer.setSwitch", json!({"layers": [plane], "switch": sw, "value": true})).unwrap();
    }
    let cam = id(s.execute("layer.newCamera", json!({"comp": comp.0})).unwrap(), "layer");
    s.execute("layer.timing", json!({"layers": [cam], "in": 2, "out": 3})).unwrap();
    let moved = kept(&mut s, comp, json!({"cmd": "prop.set", "params": {"comp": comp.0, "layer": cam, "path": "transform/position", "value": [40, 8, -30]}}));
    assert!(moved.contains(&0) && moved.contains(&100), "frames far from the camera stay: {moved:?}");
    assert!(!(56..=91).any(|f| moved.contains(&f)), "frames whose shutter samples the camera render again: {moved:?}");
}

#[test]
fn short_solid_additions_and_removals_keep_unaffected_frames() {
    use effectcraft_engine::Session;
    use serde_json::json;
    let mut s = Session::default();
    let comp = ItemId(s.execute("comp.new", json!({"width": 8, "height": 8, "frameRate": 30, "duration": 20})).unwrap()["comp"].as_u64().unwrap());
    s.execute("layer.newSolid", json!({"color": "#ff0000"})).unwrap();
    let f = Frames::default();
    let cache_all = |s: &Session| {
        let content = f.content_of(&s.project, s.revision, comp);
        (0..600).for_each(|n| insert(&f, key(content, n, 0)));
    };
    let cached = |s: &Session| f.cached_frames(&key(f.content_of(&s.project, s.revision, comp), 0, 0));
    // 2–3 s, a quarter frame of the default shutter and a frame of rounding either side.
    let kept: Vec<i64> = (0..=57).chain(92..600).collect();
    cache_all(&s);
    let solid = s.execute("layer.newSolid", json!({"color": "#00ff00"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.timing", json!({"layers": [solid], "in": 2, "out": 3})).unwrap();
    assert_eq!(cached(&s), kept, "adding a source invalidates only frames 58..=91");
    cache_all(&s);
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(cached(&s), (0..600).collect::<Vec<_>>(), "undoing the addition restores every frame");
    s.execute("edit.redo", json!({})).unwrap();
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(cached(&s), (0..600).collect::<Vec<_>>(), "redoing the addition restores every frame");
    s.execute("edit.clear", json!({"layers": [solid]})).unwrap();
    assert_eq!(cached(&s), kept, "removing its layer invalidates the same span");
}

#[test]
fn adding_a_source_does_not_hide_changes_to_existing_dependencies() {
    use effectcraft_engine::Session;
    use serde_json::json;
    for (command, params) in [
        ("layer.settings", json!({"comp": "Outer", "layer": "Underlay", "color": "#0000ff"})),
        ("prop.set", json!({"comp": "Inner", "layer": "Inner solid", "path": "transform/opacity", "value": 40})),
    ] {
        let mut s = Session::default();
        let comp = ItemId(
            s.execute("comp.new", json!({"name": "Outer", "width": 8, "height": 8, "frameRate": 30, "duration": 20})).unwrap()["comp"].as_u64().unwrap(),
        );
        s.execute("layer.newSolid", json!({"name": "Underlay", "color": "#ff0000"})).unwrap();
        s.execute("comp.new", json!({"name": "Inner", "width": 8, "height": 8, "duration": 20})).unwrap();
        s.execute("layer.newSolid", json!({"name": "Inner solid", "color": "#00ff00"})).unwrap();
        s.execute("comp.open", json!({"comp": "Outer"})).unwrap();
        s.execute("layer.addItem", json!({"item": "Inner"})).unwrap();
        let f = Frames::default();
        let before = f.content_of(&s.project, s.revision, comp);
        (0..600).for_each(|n| insert(&f, key(before, n, 0)));
        s.execute(command, params).unwrap();
        let solid = s.execute("layer.newSolid", json!({"color": "#ffffff"})).unwrap()["layer"].as_u64().unwrap();
        s.execute("layer.timing", json!({"layers": [solid], "in": 2, "out": 3})).unwrap();
        let after = f.content_of(&s.project, s.revision, comp);
        assert!(f.cached_frames(&key(after, 0, 0)).is_empty(), "{command} changes pixels outside the added layer");
    }
}

/// Adding, moving or deleting a marker (Num* while a preview plays) keeps the cached frames:
/// the sound would otherwise stop at the next frame that has to be rendered again (#1083).
/// Only expressions that read markers make them part of the pixels.
#[test]
fn markers_keep_the_frames_unless_an_expression_reads_them() {
    use effectcraft_engine::Session;
    use serde_json::json;
    let mut s = Session::default();
    let id = |v: serde_json::Value, k: &str| v[k].as_u64().unwrap();
    let inner = ItemId(id(s.execute("comp.new", json!({"name": "Inner", "width": 8, "height": 8, "duration": 2})).unwrap(), "comp"));
    let solid = id(s.execute("layer.newSolid", json!({"comp": inner.0, "color": "#ff0000"})).unwrap(), "layer");
    let outer = ItemId(id(s.execute("comp.new", json!({"name": "Outer", "width": 8, "height": 8, "duration": 2})).unwrap(), "comp"));
    id(s.execute("layer.addItem", json!({"comp": outer.0, "item": inner.0})).unwrap(), "layer");
    let f = Frames::default();
    let at = |s: &Session, c: ItemId| f.content_of(&s.project, s.revision, c);
    let (o0, i0) = (at(&s, outer), at(&s, inner));
    s.execute("comp.addMarker", json!({"comp": inner.0, "time": 0.5, "comment": "beat"})).unwrap();
    s.execute("comp.addMarker", json!({"comp": outer.0, "time": 1.0})).unwrap();
    s.execute("comp.addMarker", json!({"comp": outer.0, "time": 1.5})).unwrap();
    assert_eq!((at(&s, outer), at(&s, inner)), (o0, i0), "comp markers");
    s.execute("markers.set", json!({"comp": inner.0, "layer": solid, "new": true, "time": 0.25})).unwrap();
    assert_eq!((at(&s, outer), at(&s, inner)), (o0, i0), "layer marker");
    // An expression reading markers makes them count again.
    s.execute("prop.setExpression", json!({"comp": inner.0, "layer": solid, "path": "transform/opacity", "expression": "thisComp.marker.numKeys"})).unwrap();
    let (o1, i1) = (at(&s, outer), at(&s, inner));
    assert_ne!(o1, o0);
    s.execute("comp.addMarker", json!({"comp": inner.0, "time": 1.0})).unwrap();
    assert_ne!(at(&s, inner), i1, "marker read by an expression");
    assert_ne!(at(&s, outer), o1, "marker read by an expression in a nested comp");
}

/// A protected time marker with a duration sets the comp's timing (Responsive Design), so it
/// changes the frames even though plain markers do not; and any expression, which can read
/// markers, makes every marker count.
#[test]
fn timing_markers_and_expressions_still_change_the_frames() {
    use effectcraft_engine::Session;
    use serde_json::json;
    let mut s = Session::default();
    let id = |v: serde_json::Value, k: &str| v[k].as_u64().unwrap();
    let outer = ItemId(id(s.execute("comp.new", json!({"name": "Outer", "width": 8, "height": 8, "duration": 1})).unwrap(), "comp"));
    let solid = id(s.execute("layer.newSolid", json!({"comp": outer.0, "color": "#ff0000"})).unwrap(), "layer");
    let f = Frames::default();
    let at = |s: &Session| f.content_of(&s.project, s.revision, outer);
    let o0 = at(&s);
    s.execute("layer.addMarker", json!({"comp": outer.0, "layers": [solid], "time": 0.5, "comment": "beat"})).unwrap();
    assert_eq!(at(&s), o0, "a plain marker changes no pixels, so it keeps the frames");
    s.execute("comp.addMarker", json!({"comp": outer.0, "time": 0.25})).unwrap();
    s.execute("markers.set", json!({"comp": outer.0, "index": 0, "duration": 0.25, "protected": true})).unwrap();
    let o1 = at(&s);
    assert_ne!(o1, o0, "a protected time marker with a duration sets the timing");
    s.execute("prop.setExpression", json!({"comp": outer.0, "layer": solid, "path": "transform/opacity", "expression": "1"})).unwrap();
    let o2 = at(&s);
    s.execute("layer.addMarker", json!({"comp": outer.0, "layers": [solid], "time": 0.75})).unwrap();
    assert_ne!(at(&s), o2, "an expression can read the marker, so the frames change");
}

/// The oracle for [`Comp::changed_spans`]: every frame an edit keeps renders the same in the edited
/// project. Moves, trims, splits, deletes and duplicates of moving clips, under adjustment
/// layers, with seeded noise, motion blur, mattes, parents and effects that read other times.
#[test]
fn kept_frames_render_the_same() {
    use effectcraft_engine::Session;
    use effectcraft_engine::render::render_frame;
    use serde_json::{Value as Json, json};
    fn run(s: &mut Session, cmd: &str, p: Json) -> Json {
        s.execute(cmd, p).unwrap_or_else(|e| panic!("{cmd}: {e}"))
    }
    /// Set a layer's start, in and out (frames).
    fn timing(s: &mut Session, cid: ItemId, l: u64, start: i64, inp: i64, out: i64) {
        let c = Arc::make_mut(&mut s.project).comp_mut(cid).unwrap();
        let fr = c.frame_rate;
        let l = c.layer_mut(effectcraft_engine::project::LayerId(l)).unwrap();
        (l.start_time, l.in_point, l.out_point) = (fr.tick_of(start), fr.tick_of(inp), fr.tick_of(out));
    }
    /// A 10×10 solid moving across the comp.
    fn solid(s: &mut Session, color: &str) -> u64 {
        let l = run(s, "layer.newSolid", json!({"color": color, "width": 10, "height": 10}))["layer"].as_u64().unwrap();
        run(s, "prop.addKey", json!({"layer": l, "path": "transform/position", "time": 0.0, "value": [0.0, 4.0, 0.0]}));
        run(s, "prop.addKey", json!({"layer": l, "path": "transform/position", "time": 1.5, "value": [16.0, 12.0, 0.0]}));
        l
    }
    fn split(s: &mut Session, cid: ItemId, l: u64, at: i64) {
        let fr = s.project.comp(cid).unwrap().frame_rate;
        s.set_time(fr.tick_of(at));
        run(s, "edit.splitLayer", json!({"layers": [l]}));
    }
    fn adjustment(s: &mut Session, cid: ItemId, effect: &str) -> u64 {
        let l = run(s, "layer.newAdjustment", json!({}))["layer"].as_u64().unwrap();
        run(s, "effect.apply", json!({"layers": [l], "effect": effect}));
        timing(s, cid, l, 0, 10, 35);
        l
    }
    /// Echo reaching 9 frames back, three times (past the shutter margin).
    fn long_echo(s: &mut Session, l: u64) {
        run(s, "effect.apply", json!({"layers": [l], "effect": "Echo"}));
        run(s, "prop.set", json!({"layer": l, "path": "effects/#1/echoTime", "value": -0.3}));
        run(s, "prop.set", json!({"layer": l, "path": "effects/#1/numberOfEchoes", "value": 3.0}));
    }
    // Clip a on frames 0–20, clip b on 20–40 of a 45-frame comp.
    type Step = fn(&mut Session, ItemId, &mut Vec<u64>);
    let none: Step = |_, _, _| {};
    /// An audio-only file (music) and a layer of it over the whole comp.
    fn music(s: &mut Session, cid: ItemId) -> u64 {
        use effectcraft_engine::project::{Footage, FootageKind};
        let song = Footage { kind: FootageKind::Audio, has_video: false, has_audio: true, ..Default::default() };
        let song = Arc::make_mut(&mut s.project).add_item("song.wav", Default::default(), None, ItemKind::Footage(song));
        let l = run(s, "layer.addItem", json!({"item": song.0}))["layer"].as_u64().unwrap();
        timing(s, cid, l, 0, 0, 45);
        l
    }
    /// What an edit keeps: every frame, some, or none.
    #[derive(Debug, PartialEq)]
    enum Keep {
        All,
        Part,
        Nothing,
    }
    let scenarios: Vec<(&str, Step, Step, Keep)> = vec![
        ("nudge", none, |s, c, l| timing(s, c, l[1], 21, 21, 41), Keep::Part),
        ("trim out", none, |s, c, l| timing(s, c, l[1], 20, 20, 32), Keep::Part),
        ("trim in", none, |s, c, l| timing(s, c, l[1], 20, 28, 40), Keep::Part),
        ("split", none, |s, c, l| split(s, c, l[1], 30), Keep::Part),
        ("delete", none, |s, _, l| _ = run(s, "edit.clear", json!({"layers": [l[1]]})), Keep::Part),
        ("duplicate", none, |s, _, l| _ = run(s, "edit.duplicate", json!({"layers": [l[0]]})), Keep::Part),
        ("adjustment trim", |s, c, l| l.push(adjustment(s, c, "Gaussian Blur")), |s, c, l| timing(s, c, l[2], 0, 10, 25), Keep::Part),
        ("adjustment split", |s, c, l| l.push(adjustment(s, c, "Invert")), |s, c, l| split(s, c, l[2], 22), Keep::Part),
        (
            "adjustment fx off",
            |s, c, l| l.push(adjustment(s, c, "Invert")),
            |s, _, l| _ = run(s, "layer.setSwitch", json!({"layers": [l[2]], "switch": "fx", "value": false})),
            Keep::Part,
        ),
        // Music changes no picture; neither does it when imported with the same action.
        ("add music", none, |s, c, _| _ = music(s, c), Keep::All),
        ("trim music", |s, c, l| l.push(music(s, c)), |s, c, l| timing(s, c, l[2], 0, 0, 20), Keep::All),
        ("move music", |s, c, l| l.push(music(s, c)), |s, c, l| timing(s, c, l[2], 5, 5, 45), Keep::All),
        ("delete music", |s, c, l| l.push(music(s, c)), |s, _, l| _ = run(s, "edit.clear", json!({"layers": [l[2]]})), Keep::All),
        ("hide a clip", none, |s, _, l| _ = run(s, "layer.setSwitch", json!({"layers": [l[1]], "switch": "video", "value": false})), Keep::Part),
        // Music another layer reads isn't silent: like any layer another one reads, editing
        // it re-renders everything.
        (
            "trim music an audio spectrum reads",
            |s, c, l| {
                let m = music(s, c);
                run(s, "effect.apply", json!({"layers": [l[0]], "effect": "Audio Spectrum"}));
                run(s, "prop.set", json!({"layer": l[0], "path": "effects/#1/audioLayer", "value": m}));
                l.push(m);
            },
            |s, c, l| timing(s, c, l[2], 0, 0, 20),
            Keep::Nothing,
        ),
        (
            "noise split",
            |s, _, l| _ = run(s, "effect.apply", json!({"layers": [l[1]], "effect": "Fractal Noise"})),
            |s, c, l| split(s, c, l[1], 30),
            Keep::Part,
        ),
        (
            "motion blur trim",
            |s, c, l| {
                Arc::make_mut(&mut s.project).comp_mut(c).unwrap().enable_motion_blur = true;
                run(s, "layer.setSwitch", json!({"layers": [l[1]], "switch": "motionBlur", "value": true}));
            },
            |s, c, l| timing(s, c, l[1], 20, 20, 32),
            Keep::Part,
        ),
        (
            "matted clip trim",
            |s, c, l| {
                let m = solid(s, "#808080");
                timing(s, c, m, 0, 0, 45);
                run(s, "layer.setTrackMatte", json!({"layer": l[1], "matte": m}));
            },
            |s, c, l| timing(s, c, l[1], 20, 20, 32),
            Keep::Part,
        ),
        (
            "parent move",
            |s, _, l| _ = run(s, "layer.setParent", json!({"layers": [l[0]], "parent": l[1]})),
            |s, c, l| timing(s, c, l[1], 21, 21, 41),
            // (the child re-renders where it is active, the rest stays)
            Keep::Part,
        ),
        ("echo trim in", |s, _, l| long_echo(s, l[1]), |s, c, l| timing(s, c, l[1], 20, 28, 40), Keep::Part),
        ("echo trim out", |s, _, l| long_echo(s, l[1]), |s, c, l| timing(s, c, l[1], 20, 20, 32), Keep::Part),
        (
            "posterize time trim in",
            |s, _, l| {
                run(s, "effect.apply", json!({"layers": [l[1]], "effect": "Posterize Time"}));
                run(s, "prop.set", json!({"layer": l[1], "path": "effects/#1/frameRate", "value": 3.0}));
            },
            |s, c, l| timing(s, c, l[1], 20, 28, 40),
            Keep::Part,
        ),
        (
            "time displacement trim in",
            |s, _, l| _ = run(s, "effect.apply", json!({"layers": [l[1]], "effect": "Time Displacement"})),
            |s, c, l| timing(s, c, l[1], 20, 28, 40),
            Keep::Part,
        ),
        // (Echo on an adjustment layer doesn't read other times yet, `FxHost::self_at`;
        // when it does, an edit under it reaches frames after the edit.)
        (
            "echo adjustment nudge",
            |s, _, l| {
                let a = run(s, "layer.newAdjustment", json!({}))["layer"].as_u64().unwrap();
                long_echo(s, a);
                l.push(a);
            },
            |s, c, l| timing(s, c, l[1], 21, 21, 41),
            Keep::Part,
        ),
    ];
    for (name, setup, edit, expect) in scenarios {
        let mut s = Session::default();
        let cid = ItemId(run(&mut s, "comp.new", json!({"name": "A", "width": 16, "height": 16, "duration": 1.5}))["comp"].as_u64().unwrap());
        let mut l = vec![solid(&mut s, "#ff0000"), solid(&mut s, "#00ff00")];
        timing(&mut s, cid, l[0], 0, 0, 20);
        timing(&mut s, cid, l[1], 20, 20, 40);
        setup(&mut s, cid, &mut l);
        let p0 = s.project.clone();
        edit(&mut s, cid, &mut l);
        let p1 = s.project.clone();
        let fr = p1.comp(cid).unwrap().frame_rate;
        let frames = fr.frame_at(p1.comp(cid).unwrap().duration);
        let dirty = dirty_frames(&p0, &p1, cid, &p1.heard_layers().union(&p0.heard_layers()), false);
        let kept: Vec<i64> = match &dirty {
            Some(d) => (0..frames).filter(|f| !d.iter().any(|(lo, hi)| (*lo..=*hi).contains(f))).collect(),
            None => vec![],
        };
        let got = match kept.len() as i64 {
            0 => Keep::Nothing,
            n if n == frames => Keep::All,
            _ => Keep::Part,
        };
        assert_eq!(got, expect, "{name}: dirty {dirty:?}");
        for f in kept {
            let (a, b) = (render_frame(&p0, cid, fr.tick_of(f), 1.0), render_frame(&p1, cid, fr.tick_of(f), 1.0));
            assert!(a.data == b.data, "{name}: kept frame {f} renders differently after the edit");
        }
    }
}

/// An edit keeps the cached frames it can't reach: nudging a clip re-renders only where it
/// was and is, a trim only the part it cut, a split only the copy's half (its new ids seed
/// randomness). Undo finds them all again; a clip another layer reads (its parent here) also
/// drops the frames of that layer.
#[test]
fn an_edit_keeps_the_frames_it_cant_reach() {
    use effectcraft_engine::Session;
    use effectcraft_engine::project::{Layer, LayerId};
    use serde_json::json;
    type Edit = fn(&mut Comp, LayerId, &dyn Fn(i64) -> Tick);
    let nudge: Edit = |c, b, t| {
        let l = c.layer_mut(b).unwrap();
        (l.start_time, l.in_point, l.out_point) = (l.start_time + t(1), l.in_point + t(1), l.out_point + t(1));
    };
    let trim: Edit = |c, b, t| c.layer_mut(b).unwrap().out_point = t(75);
    let split: Edit = |c, b, t| {
        let i = c.layers.iter().position(|l| l.id == b).unwrap();
        let mut copy: Layer = c.layers[i].clone();
        copy.id = LayerId(1000);
        copy.props.reassign_uids(&mut 1000);
        copy.in_point = t(75);
        c.layers[i].out_point = t(75);
        c.layers.insert(i, copy);
    };
    let cut: Vec<i64> = (0..=72).chain(92..=95).collect();
    for (edit, kept) in [(nudge, (0..=57).chain(93..=95).collect::<Vec<_>>()), (trim, cut.clone()), (split, cut)] {
        for parented in [false, true] {
            let mut s = Session::default();
            let cid = ItemId(s.execute("comp.new", json!({"name": "A", "width": 8, "height": 8, "duration": 4})).unwrap()["comp"].as_u64().unwrap());
            s.execute("layer.newSolid", json!({})).unwrap();
            s.execute("layer.newSolid", json!({})).unwrap();
            let fr = s.project.comp(cid).unwrap().frame_rate;
            let t = |f: i64| fr.tick_of(f);
            let b = {
                let c = Arc::make_mut(&mut s.project).comp_mut(cid).unwrap();
                let (a, b): (LayerId, LayerId) = (c.layers[0].id, c.layers[1].id);
                let l = c.layer_mut(a).unwrap();
                (l.in_point, l.out_point) = (t(0), t(30));
                l.parent = parented.then_some(b);
                let l = c.layer_mut(b).unwrap();
                (l.start_time, l.in_point, l.out_point) = (t(60), t(60), t(90));
                b
            };
            let p0 = s.project.clone();
            let f = Frames::default();
            let k = |content: u64, frame: i64| FrameKey { content, comp: cid.0, ..key(0, frame, 0) };
            let id0 = f.content_of(&p0, 1, cid);
            for fr in 0..=95 {
                insert(&f, k(id0, fr));
            }
            let mut p1 = p0.clone();
            edit(Arc::make_mut(&mut p1).comp_mut(cid).unwrap(), b, &t);
            let id1 = f.content_of(&p1, 2, cid);
            assert_ne!(id0, id1);
            let got = f.cached_frames(&k(id1, 0));
            // (the child, 0–30, renders again too)
            assert_eq!(got, if parented { kept.iter().copied().filter(|f| *f > 31).collect() } else { kept.clone() });
            assert_eq!(f.content_of(&p0, 3, cid), id0, "undo");
            assert_eq!(f.cached_frames(&k(id0, 0)), (0..=95).collect::<Vec<_>>());
        }
    }
}

/// A saved and reopened project finds its frames on disk: the content key is the same.
#[test]
fn disk_keys_survive_save_and_reopen() {
    use serde_json::json;
    let dir = std::env::temp_dir().join(format!("ec-disk-keys-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("still.png");
    image::RgbaImage::from_pixel(4, 4, image::Rgba([200, 100, 50, 255])).save(&png).unwrap();
    let mut s = effectcraft_host::session();
    let cid = ItemId(s.execute("comp.new", json!({"name": "Keys", "width": 16, "height": 16, "duration": 1})).unwrap()["comp"].as_u64().unwrap());
    let item = s.execute("file.import", json!({"paths": [png.to_string_lossy()]})).unwrap()["items"][0].clone();
    s.execute("layer.addItem", json!({"item": item})).unwrap();
    s.execute("layer.newSolid", json!({"color": "#00ff00", "width": 8, "height": 8})).unwrap();
    let adj = s.execute("layer.newAdjustment", json!({})).unwrap()["layer"].clone();
    s.execute("effect.apply", json!({"layers": [adj], "effect": "Gaussian Blur"})).unwrap();
    let before = disk_cache::comp_content_key(&s.project, cid);
    let path = dir.join("keys.ecproj");
    s.execute("file.saveAs", json!({"path": path.to_string_lossy()})).unwrap();
    let mut again = effectcraft_host::session();
    again.execute("file.open", json!({"path": path.to_string_lossy()})).unwrap();
    assert_eq!(disk_cache::comp_content_key(&again.project, cid), before);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Frames in RAM that the disk lacks (an edit kept them; a write was dropped) are written
/// when idle, so the reopened project shows them all as the blue bar. The RAM series carries a
/// Roto Brush model stamp in its opts; the disk keys are the worker's, without it.
#[test]
fn ram_frames_missing_on_disk_are_written() {
    use effectcraft_engine::Session;
    use serde_json::json;
    let dir = std::env::temp_dir().join(format!("ec-persist-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let disk = DiskCache::open(&dir, 1 << 30).unwrap();
    let mut s = Session::default();
    let cid = ItemId(s.execute("comp.new", json!({"name": "A", "width": 8, "height": 8, "duration": 1})).unwrap()["comp"].as_u64().unwrap());
    s.execute("layer.newSolid", json!({})).unwrap();
    let src = RenderSource {
        project: s.project.clone(),
        footage: Arc::new(effectcraft_engine::render::NoFootage),
        expr: None,
        layer_cache: Arc::default(),
        gpu: None,
        gpu_display: false,
        disk: Some(disk.clone()),
    };
    let f = Frames::default();
    let series =
        FrameKey { revision: 1, content: f.content_of(&src.project, 1, cid), comp: cid.0, opts: opts_hash(&RenderOpts::default()) ^ 1, ..key(0, 0, 0) };
    for fr in 0..10 {
        insert(&f, FrameKey { frame: fr, ..series });
    }
    let on_disk = |fr: i64| {
        disk.contains(
            disk_cache::Kind::Frame,
            frame_disk_key(
                &f.content_keys,
                &src.project,
                &FrameKey { frame: fr, ..series },
                opts_hash(&RenderOpts::default()),
                f.virtual_epoch.load(Ordering::Relaxed),
                None,
            ),
        )
    };
    let persist = || {
        // The content key is computed on the frame pool first.
        for _ in 0..1000 {
            f.persist_missing(&src, &series, &RenderOpts::default());
            disk.flush();
            // (`flush` waits for the disk writes, not for the conversion to forget its keys)
            if (0..10).all(on_disk) && f.persisted.lock().unwrap().is_empty() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        false
    };
    let evict = || {
        disk.set_max_bytes(1);
        disk.set_max_bytes(1 << 30);
        assert!(!(0..10).any(on_disk));
    };
    assert!(persist(), "every RAM frame written");
    assert!(!f.persist_missing(&src, &series, &RenderOpts::default()), "nothing left to write: no more idle passes");
    // Evicted from disk: the RAM frames are written again, once.
    evict();
    assert!(persist(), "evicted frames written again");
    // Reopened: a new preview cache over the same disk shows every frame in blue.
    let reopened = Frames::default();
    let reopened_series = FrameKey { content: reopened.content_of(&src.project, 1, cid), ..series };
    let mut blue = vec![];
    for _ in 0..1000 {
        blue = reopened.disk_frames(&src, &reopened_series, 0..10, &RenderOpts::default());
        if blue.len() == 10 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(blue, (0..10).collect::<Vec<_>>());
    // Evicted again (a disk cache too small for them): not rewritten on every idle pass.
    evict();
    assert!(!f.persist_missing(&src, &series, &RenderOpts::default()), "evicted twice: no more idle passes");
    disk.flush();
    assert!(!(0..10).any(on_disk));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Audio changes pixels only where something draws it: a clip's Audio Levels, audio effects and
/// Audio switch keep every frame, in RAM and on disk, until a waveform picks the clip (on another
/// layer, on itself, or through a precomp of it); then the frames of the reader render again.
#[test]
fn audio_edits_keep_the_frames_until_something_draws_the_audio() {
    use effectcraft_engine::Session;
    use effectcraft_engine::project::{Footage, GroupKind, LayerId, Value, build};
    use effectcraft_engine::render::disk_cache::comp_content_key_with;
    use serde_json::json;
    let mut s = Session::default();
    let cid = ItemId(s.execute("comp.new", json!({"name": "A", "width": 8, "height": 8, "duration": 4, "frameRate": 30})).unwrap()["comp"].as_u64().unwrap());
    s.execute("layer.newSolid", json!({})).unwrap();
    let mut p = (*s.project).clone();
    let clip =
        p.add_item("clip.mov", effectcraft_color::Label::None, None, ItemKind::Footage(Footage { has_video: true, has_audio: true, ..Default::default() }));
    let comp = p.comp(cid).unwrap().clone();
    let fr = comp.frame_rate;
    let t = move |f: i64| fr.tick_of(f);
    let v = build::layer(&mut p, &comp, "Clip", LayerSource::Footage { item: clip }, (8, 8), None);
    let (clip_layer, solid) = (v.id, comp.layers[0].id);
    p.comp_mut(cid).unwrap().layers.push(v);
    for (id, a, b) in [(solid, 0, 30), (clip_layer, 90, 120)] {
        let l = p.comp_mut(cid).unwrap().layer_mut(id).unwrap();
        (l.start_time, l.in_point, l.out_point) = (t(a), t(a), t(b));
    }
    // The outer comp shows the whole of A.
    let outer = p.add_item("Outer", effectcraft_color::Label::None, None, ItemKind::Comp(Arc::new(Comp { layers: Default::default(), ..comp.clone() })));
    let oc = p.comp(outer).unwrap().clone();
    let mut pre = build::layer(&mut p, &oc, "Pre", LayerSource::Comp { item: cid }, (8, 8), None);
    (pre.start_time, pre.in_point, pre.out_point) = (t(0), t(0), t(120));
    let (pre_layer, reader) = (pre.id, oc.layers.first().map(|l| l.id));
    assert!(reader.is_none());
    p.comp_mut(outer).unwrap().layers.push(pre);
    // An effect on `on` (of comp `c`) that picks layer `pick`.
    let pick = |p: &mut Project, c: ItemId, on: LayerId, pick: LayerId| {
        let mut n = 1_000_000;
        let mut ids = build::Ids(&mut n);
        let mut g = ids.group("waveform", "Audio Waveform");
        g.kind = GroupKind::Effect { effect: "ec.generate.audiowaveform".into() };
        g.children.push(ids.prop("audioLayer", "Audio Layer", Value::Layer(Some(pick.0))).into());
        p.comp_mut(c).unwrap().layer_mut(on).unwrap().props.group_mut("effects").unwrap().children.push(g.into());
    };
    let levels = |p: &mut Project| {
        p.comp_mut(cid).unwrap().layer_mut(clip_layer).unwrap().props.prop_mut("audio/levels").unwrap().set_value_at(t(0), Value::Vec2([-6.0, -6.0]))
    };
    let backwards = |p: &mut Project| {
        let mut n = 2_000_000;
        let mut g = build::Ids(&mut n).group("backwards", "Backwards");
        g.kind = GroupKind::Effect { effect: "ec.audio.backwards".into() };
        p.comp_mut(cid).unwrap().layer_mut(clip_layer).unwrap().props.group_mut("effects").unwrap().children.push(g.into());
    };
    let mute = |p: &mut Project| p.comp_mut(cid).unwrap().layer_mut(clip_layer).unwrap().switches.audio = false;
    // The frames of `view` an edit keeps, and whether its disk key stays.
    let kept = |p0: &Project, view: ItemId, edit: &dyn Fn(&mut Project)| {
        let p0 = Arc::new(p0.clone());
        let f = Frames::default();
        let k = |content: u64, frame: i64| FrameKey { content, comp: view.0, ..key(0, frame, 0) };
        let id0 = f.content_of(&p0, 1, view);
        (0..120).for_each(|n| insert(&f, k(id0, n)));
        let mut p1 = p0.clone();
        edit(Arc::make_mut(&mut p1));
        let id1 = f.content_of(&p1, 2, view);
        (f.cached_frames(&k(id1, 0)), comp_content_key_with(&p0, view, None) == comp_content_key_with(&p1, view, None))
    };
    let all: Vec<i64> = (0..120).collect();
    let edits: [(&str, &dyn Fn(&mut Project)); 3] = [("levels", &levels), ("audio effect", &backwards), ("audio switch", &mute)];
    for (name, edit) in edits {
        assert_eq!(kept(&p, cid, edit), (all.clone(), true), "{name}: nothing draws the clip's audio");
        assert_eq!(kept(&p, outer, edit), (all.clone(), true), "{name}: nor through the precomp");

        let mut q = p.clone();
        pick(&mut q, cid, solid, clip_layer);
        let (frames, disk) = kept(&q, cid, edit);
        assert!(!disk, "{name}: the solid draws the clip's audio");
        assert!(frames.contains(&60) && !frames.contains(&10) && !frames.contains(&100), "{name}: the reader's and the clip's frames go: {frames:?}");

        let mut q = p.clone();
        pick(&mut q, cid, clip_layer, clip_layer);
        let (frames, disk) = kept(&q, cid, edit);
        assert!(!disk, "{name}: the clip draws its own audio");
        assert!(frames.contains(&10) && frames.contains(&60) && !frames.contains(&100), "{name}: the clip's frames go: {frames:?}");

        let mut q = p.clone();
        let r = q.comp(cid).unwrap().layers[0].clone();
        let reader = LayerId(r.id.0 + 500_000);
        q.comp_mut(outer).unwrap().layers.insert(0, effectcraft_engine::project::Layer { id: reader, ..r });
        pick(&mut q, outer, reader, pre_layer);
        let (frames, disk) = kept(&q, outer, edit);
        assert!(
            !disk && frames.contains(&60) && !frames.contains(&10) && !frames.contains(&100),
            "{name}: a waveform in the outer comp draws the precomp's audio: {frames:?}"
        );
    }
}

/// An expression reaches only the frames of its own layer: with one in the comp, an edit (here
/// or elsewhere in the project: a trim, a rename, a marker, another comp) re-renders that layer's
/// frames besides the edit's own, and every frame kept renders as before, expressions evaluated,
/// also when the expression reads the edited layer.
#[test]
fn an_expression_reaches_only_the_frames_of_its_layer() {
    use effectcraft_engine::project::{LayerId, Marker};
    use effectcraft_engine::render::NoFootage;
    use serde_json::json;
    let mut s = effectcraft_host::session();
    let cid = ItemId(s.execute("comp.new", json!({"name": "A", "width": 16, "height": 16, "duration": 1.5})).unwrap()["comp"].as_u64().unwrap());
    let other = ItemId(s.execute("comp.new", json!({"name": "Other", "width": 8, "height": 8, "duration": 1})).unwrap()["comp"].as_u64().unwrap());
    s.execute("comp.open", json!({"comp": cid.0})).ok();
    let mut solid = |name: &str| {
        let l = s.execute("layer.newSolid", json!({"comp": cid.0, "color": "#ff0000", "width": 10, "height": 10})).unwrap()["layer"].as_u64().unwrap();
        s.execute("layer.rename", json!({"comp": cid.0, "layer": l, "name": name})).unwrap();
        for (t, x) in [(0.0, 0.0), (1.5, 16.0)] {
            s.execute("prop.addKey", json!({"comp": cid.0, "layer": l, "path": "transform/position", "time": t, "value": [x, 8.0, 0.0]})).unwrap();
        }
        LayerId(l)
    };
    let (a, b, c) = (solid("A"), solid("B"), solid("C"));
    s.execute(
        "prop.setExpression",
        json!({"comp": cid.0, "layer": c.0, "path": "transform/position", "expression": "thisComp.layer(\"B\").transform.position + [0, time * 4]"}),
    )
    .unwrap();
    let fr = s.project.comp(cid).unwrap().frame_rate;
    let n = fr.frame_at(s.project.comp(cid).unwrap().duration);
    let mut p0 = (*s.project).clone();
    for (id, i, o) in [(a, 0, 20), (b, 20, 40), (c, 40, 45)] {
        let l = p0.comp_mut(cid).unwrap().layer_mut(id).unwrap();
        (l.start_time, l.in_point, l.out_point) = (fr.tick_of(i), fr.tick_of(i), fr.tick_of(o));
    }
    let p0 = Arc::new(p0);
    let host = s.expr.clone().unwrap();
    let render = |p: &Project, i: i64| {
        let mut r = Renderer::new(p, &NoFootage, RenderOpts::default());
        r.expr = Some(&*host);
        r.comp_frame(cid, fr.tick_of(i))
    };
    type Edit = fn(&mut Project, ItemId, ItemId, [LayerId; 3]);
    let edits: [(&str, Edit, i64); 4] = [
        ("trim B", |p, c, _, l| p.comp_mut(c).unwrap().layer_mut(l[1]).unwrap().out_point = p.comp(c).unwrap().frame_rate.tick_of(32), 30),
        ("rename A", |p, c, _, l| p.comp_mut(c).unwrap().layer_mut(l[0]).unwrap().name = "Renamed".into(), 38),
        ("comp marker", |p, c, _, _| p.comp_mut(c).unwrap().markers.push(Marker { time: Tick::ZERO, ..Default::default() }), 38),
        ("another comp", |p, _, o, _| p.comp_mut(o).unwrap().width = 9, 38),
    ];
    for (name, edit, kept_from) in edits {
        let f = Frames::default();
        let k = |content: u64, frame: i64| FrameKey { content, comp: cid.0, ..key(0, frame, 0) };
        let id0 = f.content_of(&p0, 1, cid);
        (0..n).for_each(|i| insert(&f, k(id0, i)));
        let mut p1 = p0.clone();
        edit(Arc::make_mut(&mut p1), cid, other, [a, b, c]);
        let kept = f.cached_frames(&k(f.content_of(&p1, 2, cid), 0));
        // C (frames 40–45) reads anything: it renders again, the frames before it (and before a
        // trim of B) stay.
        assert!((0..kept_from).all(|i| kept.contains(&i)) && !kept.contains(&42), "{name}: {kept:?}");
        for i in kept {
            assert_eq!(render(&p0, i), render(&p1, i), "{name}: kept frame {i} draws other pixels now");
        }
    }
}

/// An edit inside a precomp re-renders only the frames where the precomp shows what it changed:
/// shifted by the precomp layer's start, scaled by its stretch, and its whole span under a time
/// remap. Every frame kept renders as before.
#[test]
fn an_edit_in_a_precomp_reaches_only_where_it_shows() {
    use effectcraft_engine::Session;
    use effectcraft_engine::project::{LayerId, Value, build};
    use effectcraft_engine::render::render_frame;
    use serde_json::json;
    let mut s = Session::default();
    let id = |v: serde_json::Value, k: &str| v[k].as_u64().unwrap();
    let inner = ItemId(id(s.execute("comp.new", json!({"name": "Inner", "width": 16, "height": 16, "duration": 4, "frameRate": 30})).unwrap(), "comp"));
    let solid = |s: &mut Session| {
        let l = id(s.execute("layer.newSolid", json!({"comp": inner.0, "color": "#ff0000", "width": 6, "height": 6})).unwrap(), "layer");
        for (t, x) in [(0.0, 0.0), (4.0, 16.0)] {
            s.execute("prop.addKey", json!({"comp": inner.0, "layer": l, "path": "transform/position", "time": t, "value": [x, 8.0, 0.0]})).unwrap();
        }
        LayerId(l)
    };
    let (a, b) = (solid(&mut s), solid(&mut s));
    let outer = ItemId(id(s.execute("comp.new", json!({"name": "Outer", "width": 16, "height": 16, "duration": 6, "frameRate": 30})).unwrap(), "comp"));
    let pre = LayerId(id(s.execute("layer.addItem", json!({"comp": outer.0, "item": inner.0})).unwrap(), "layer"));
    let fr = s.project.comp(outer).unwrap().frame_rate;
    let t = |f: i64| fr.tick_of(f);
    let mut p0 = (*s.project).clone();
    for (l, i, o) in [(a, 0, 30), (b, 30, 60)] {
        let l = p0.comp_mut(inner).unwrap().layer_mut(l).unwrap();
        (l.start_time, l.in_point, l.out_point) = (t(i), t(i), t(o));
    }
    let n = fr.frame_at(p0.comp(outer).unwrap().duration);
    // The precomp starts at frame 15; `layout` sets its stretch and time remap.
    let place = |p: &mut Project, stretch: f64, remap: bool| {
        let l = p.comp_mut(outer).unwrap().layer_mut(pre).unwrap();
        (l.start_time, l.in_point, l.out_point, l.stretch) = (t(15), t(15), t(15) + Tick((t(120).0 as f64 * stretch / 100.0) as i64), stretch);
        if remap {
            let mut uid = 9_000_000;
            l.props.children.push(build::Ids(&mut uid).prop("timeRemap", "Time Remap", Value::Scalar(1.5)).into());
        }
    };
    // Trim B (inner frames 30–60) to end at 45.
    let trim = |p: &mut Project| p.comp_mut(inner).unwrap().layer_mut(b).unwrap().out_point = t(45);
    for (name, stretch, remap, kept_frames, dirty_frames) in [
        ("offset", 100.0, false, vec![30, 55, 80, 130], vec![60, 75]),
        ("stretched", 200.0, false, vec![30, 100, 140, 179], vec![105, 135]),
        ("time remap", 100.0, true, vec![5], vec![30, 100]),
    ] {
        let mut q = p0.clone();
        place(&mut q, stretch, remap);
        let q = Arc::new(q);
        let f = Frames::default();
        let k = |content: u64, frame: i64| FrameKey { content, comp: outer.0, ..key(0, frame, 0) };
        let id0 = f.content_of(&q, 1, outer);
        (0..n).for_each(|i| insert(&f, k(id0, i)));
        let mut q1 = q.clone();
        trim(Arc::make_mut(&mut q1));
        let kept = f.cached_frames(&k(f.content_of(&q1, 2, outer), 0));
        assert!(kept_frames.iter().all(|i| kept.contains(i)) && !dirty_frames.iter().any(|i| kept.contains(i)), "{name}: {kept:?}");
        for i in kept {
            assert!(
                render_frame(&q, outer, t(i), 1.0).data == render_frame(&q1, outer, t(i), 1.0).data,
                "{name}: kept frame {i} renders differently after the edit"
            );
        }
    }
}

/// Frames only on disk (the blue bar) outlive an edit that can't change them: a rename keeps them
/// all, a trim all but the frames it cut, and undo finds the earlier ones again.
#[test]
fn disk_frames_outlive_an_edit_that_cant_change_them() {
    use effectcraft_engine::Session;
    use effectcraft_engine::project::LayerId;
    use serde_json::json;
    let dir = std::env::temp_dir().join(format!("ec-disk-carry-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let disk = DiskCache::open(&dir, 1 << 30).unwrap();
    let mut s = Session::default();
    let cid = ItemId(s.execute("comp.new", json!({"name": "A", "width": 8, "height": 8, "duration": 1, "frameRate": 30})).unwrap()["comp"].as_u64().unwrap());
    let a = LayerId(s.execute("layer.newSolid", json!({"color": "#ff0000"})).unwrap()["layer"].as_u64().unwrap());
    let b = LayerId(s.execute("layer.newSolid", json!({"color": "#00ff00"})).unwrap()["layer"].as_u64().unwrap());
    let fr = s.project.comp(cid).unwrap().frame_rate;
    let mut p0 = (*s.project).clone();
    for (l, i, o) in [(a, 0, 15), (b, 15, 30)] {
        let l = p0.comp_mut(cid).unwrap().layer_mut(l).unwrap();
        (l.start_time, l.in_point, l.out_point) = (fr.tick_of(i), fr.tick_of(i), fr.tick_of(o));
    }
    let p0 = Arc::new(p0);
    let opts = RenderOpts::default();
    let oh = opts_hash(&opts);
    let src = |p: &Arc<Project>| RenderSource {
        project: p.clone(),
        footage: Arc::new(effectcraft_engine::render::NoFootage),
        expr: None,
        layer_cache: Arc::default(),
        gpu: None,
        gpu_display: false,
        disk: Some(disk.clone()),
    };
    // Every frame of `p0` on disk, none in RAM.
    let content0 = disk_cache::comp_content_key_with(&p0, cid, None);
    for f in 0..30 {
        disk.put_frame_now(disk_cache::frame_key(content0, f, 1000, oh), 1, 1, &[1, 2, 3, 255]);
    }
    let rename: fn(&mut Project, ItemId, LayerId) = |p, c, l| p.comp_mut(c).unwrap().layer_mut(l).unwrap().name = "Renamed".into();
    let trim: fn(&mut Project, ItemId, LayerId) = |p, c, l| {
        let fr = p.comp(c).unwrap().frame_rate;
        p.comp_mut(c).unwrap().layer_mut(l).unwrap().out_point = fr.tick_of(22);
    };
    for (name, edit, layer, kept) in [("rename", rename, a, (0..30).collect::<Vec<i64>>()), ("trim", trim, b, (0..20).collect())] {
        let f = Frames::default();
        let series = |p: &Arc<Project>, rev: u64| FrameKey { revision: rev, content: f.content_of(p, rev, cid), comp: cid.0, opts: oh, ..key(0, 0, 0) };
        // Blue frames of `p` once the disk carries and the content key are done.
        let blue = |p: &Arc<Project>, rev: u64, want: &[i64]| {
            let base = series(p, rev);
            let mut got = vec![];
            for _ in 0..1000 {
                got = f.disk_frames(&src(p), &base, 0..30, &opts);
                if got == want {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            got
        };
        assert_eq!(blue(&p0, 1, &(0..30).collect::<Vec<_>>()), (0..30).collect::<Vec<_>>(), "{name}: before");
        let mut p1 = p0.clone();
        edit(Arc::make_mut(&mut p1), cid, layer);
        assert_eq!(blue(&p1, 2, &kept), kept, "{name}: after");
        assert_eq!(blue(&p0, 3, &(0..30).collect::<Vec<_>>()), (0..30).collect::<Vec<_>>(), "{name}: undo");
        // A drag: an edit per paint, each paint asking for the blue bar. One carry job drains
        // them in order, and none is left scheduled.
        let mut p = p0.clone();
        for rev in 4..20 {
            Arc::make_mut(&mut p).comp_mut(cid).unwrap().layer_mut(a).unwrap().name = format!("Drag {rev}");
            let _ = f.disk_frames(&src(&p), &series(&p, rev), 0..30, &opts);
        }
        assert_eq!(blue(&p, 19, &(0..30).collect::<Vec<_>>()), (0..30).collect::<Vec<_>>(), "{name}: dragged");
        for _ in 0..1000 {
            if !f.carry_scheduled.load(Ordering::Acquire) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!f.carry_scheduled.load(Ordering::Acquire) && f.disk_carries.lock().unwrap().is_empty(), "{name}: carry job finished");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A keyframe edit re-renders only around the keys it changed: two keys either side (auto-Bezier
/// and continuous tangents reach the neighbouring segments), and from the layer's in point when
/// the first key changes. A static value or an expression still reaches the whole layer. Every
/// frame kept renders as before.
#[test]
fn a_keyframe_edit_reaches_only_the_keys_around_it() {
    use effectcraft_engine::Session;
    use effectcraft_engine::project::{LayerId, Value};
    use effectcraft_engine::render::render_frame;
    use effectcraft_keyframe::Interp;
    use serde_json::json;
    let mut s = Session::default();
    let cid = ItemId(s.execute("comp.new", json!({"name": "A", "width": 16, "height": 16, "duration": 4, "frameRate": 30})).unwrap()["comp"].as_u64().unwrap());
    let l = s.execute("layer.newSolid", json!({"color": "#ff0000", "width": 4, "height": 4})).unwrap()["layer"].as_u64().unwrap();
    let fr = s.project.comp(cid).unwrap().frame_rate;
    // Keys at frames 0, 20, 40, 60, 80, 100, zig-zagging, all auto-Bezier and continuous.
    for (i, f) in (0..6).map(|i| (i, i * 20)) {
        let y = if i % 2 == 0 { 3.0 } else { 13.0 };
        s.execute("prop.addKey", json!({"layer": l, "path": "transform/position", "time": fr.tick_of(f).seconds(), "value": [2.0 + i as f64 * 2.0, y, 0.0]}))
            .unwrap();
    }
    let mut p0 = (*s.project).clone();
    for k in &mut p0.comp_mut(cid).unwrap().layer_mut(LayerId(l)).unwrap().props.prop_mut("transform/position").unwrap().keys {
        (k.in_interp, k.out_interp, k.auto_bezier, k.continuous, k.spatial_continuous) = (Interp::Bezier, Interp::Bezier, true, true, true);
    }
    let p0 = Arc::new(p0);
    let n = 120;
    type Edit = fn(&mut effectcraft_engine::project::Property);
    let edits: [(&str, Edit, Vec<i64>, Vec<i64>); 5] = [
        ("middle value", |p| p.keys[3].value = Value::Vec3([8.0, 8.0, 0.0]), vec![0, 17, 103, 119], vec![22, 60, 98]),
        ("middle interpolation", |p| p.keys[3].out_interp = Interp::Hold, vec![0, 17, 103, 119], vec![22, 60, 98]),
        ("first value", |p| p.keys[0].value = Value::Vec3([1.0, 1.0, 0.0]), vec![43, 119], vec![0, 20, 39]),
        ("static", |p| p.keys.clear(), vec![], vec![0, 60, 119]),
        ("expression", |p| p.expr = Some(effectcraft_engine::project::Expression { text: "value".into(), enabled: true }), vec![], vec![0, 60, 119]),
    ];
    for (name, edit, kept_frames, dirty_frames) in edits {
        let f = Frames::default();
        let k = |content: u64, frame: i64| FrameKey { content, comp: cid.0, ..key(0, frame, 0) };
        let id0 = f.content_of(&p0, 1, cid);
        (0..n).for_each(|i| insert(&f, k(id0, i)));
        let mut p1 = p0.clone();
        edit(Arc::make_mut(&mut p1).comp_mut(cid).unwrap().layer_mut(LayerId(l)).unwrap().props.prop_mut("transform/position").unwrap());
        let kept = f.cached_frames(&k(f.content_of(&p1, 2, cid), 0));
        assert!(kept_frames.iter().all(|i| kept.contains(i)) && !dirty_frames.iter().any(|i| kept.contains(i)), "{name}: {kept:?}");
        let same = |i: i64| render_frame(&p0, cid, fr.tick_of(i), 1.0).data == render_frame(&p1, cid, fr.tick_of(i), 1.0).data;
        // (the edit shows: the check below compares something)
        assert!(name == "expression" || !(0..n).all(same), "{name}: the edit changes no pixels");
        for i in kept {
            assert!(same(i), "{name}: kept frame {i} renders differently after the edit");
        }
    }
}

/// Music's Audio Levels keep every frame, in RAM and on disk, while the project's expressions
/// can't read audio (a wiggle); an expression naming the music's Audio Levels re-renders its layer.
#[test]
fn music_volume_keeps_the_frames_with_expressions_that_dont_read_audio() {
    use effectcraft_engine::Session;
    use effectcraft_engine::project::{Expression, Footage, FootageKind, LayerId, Value, build};
    use effectcraft_engine::render::disk_cache::comp_content_key_with;
    use serde_json::json;
    let mut s = Session::default();
    let cid = ItemId(s.execute("comp.new", json!({"name": "A", "width": 8, "height": 8, "duration": 4, "frameRate": 30})).unwrap()["comp"].as_u64().unwrap());
    let solid = LayerId(s.execute("layer.newSolid", json!({})).unwrap()["layer"].as_u64().unwrap());
    let mut p = (*s.project).clone();
    let song = p.add_item(
        "song.wav",
        effectcraft_color::Label::None,
        None,
        ItemKind::Footage(Footage { kind: FootageKind::Audio, has_video: false, has_audio: true, ..Default::default() }),
    );
    let comp = p.comp(cid).unwrap().clone();
    let music = build::layer(&mut p, &comp, "Music", LayerSource::Footage { item: song }, (8, 8), None);
    let music_id = music.id;
    p.comp_mut(cid).unwrap().layers.push(music);
    let kept = |text: &str| {
        let mut p0 = p.clone();
        p0.comp_mut(cid).unwrap().layer_mut(solid).unwrap().props.prop_mut("transform/position").unwrap().expr =
            Some(Expression { text: text.into(), enabled: true });
        let p0 = Arc::new(p0);
        let f = Frames::default();
        let k = |content: u64, frame: i64| FrameKey { content, comp: cid.0, ..key(0, frame, 0) };
        let id0 = f.content_of(&p0, 1, cid);
        (0..120).for_each(|n| insert(&f, k(id0, n)));
        let mut p1 = p0.clone();
        Arc::make_mut(&mut p1).comp_mut(cid).unwrap().layer_mut(music_id).unwrap().props.prop_mut("audio/levels").unwrap().value = Value::Vec2([-6.0, -6.0]);
        let id1 = f.content_of(&p1, 2, cid);
        (f.cached_frames(&k(id1, 0)).len(), comp_content_key_with(&p0, cid, None) == comp_content_key_with(&p1, cid, None))
    };
    assert_eq!(kept("wiggle(2, 20)"), (120, true), "a wiggle reads no audio");
    let (frames, disk) = kept("value + thisComp.layer(\"Music\")(\"Audio\")(\"Audio Levels\")[0]");
    assert!(frames < 120 && !disk, "the expression reads the music's levels: {frames} frames kept, disk kept {disk}");
}

/// A Scale edit with only two keys re-renders from the other key on (values hold past it), not
/// the layer's whole length; a full-length layer whose expression reads only itself (a wiggle)
/// keeps its frames. Every frame kept renders as before.
#[test]
fn a_two_key_edit_beside_a_wiggle_keeps_the_other_frames() {
    use effectcraft_engine::project::{Expression, LayerId, Value};
    use effectcraft_engine::render::NoFootage;
    use serde_json::json;
    let mut s = effectcraft_host::session();
    let cid = ItemId(s.execute("comp.new", json!({"name": "A", "width": 16, "height": 16, "duration": 4, "frameRate": 30})).unwrap()["comp"].as_u64().unwrap());
    let wig = LayerId(s.execute("layer.newSolid", json!({"color": "#0000ff", "width": 4, "height": 4})).unwrap()["layer"].as_u64().unwrap());
    let l = LayerId(s.execute("layer.newSolid", json!({"color": "#ff0000", "width": 4, "height": 4})).unwrap()["layer"].as_u64().unwrap());
    let fr = s.project.comp(cid).unwrap().frame_rate;
    for (f, v) in [(20, 50.0), (80, 150.0)] {
        s.execute("prop.addKey", json!({"layer": l.0, "path": "transform/scale", "time": fr.tick_of(f).seconds(), "value": [v, v, 100.0]})).unwrap();
    }
    let mut p0 = (*s.project).clone();
    p0.comp_mut(cid).unwrap().layer_mut(wig).unwrap().props.prop_mut("transform/position").unwrap().expr =
        Some(Expression { text: "wiggle(2, 3)".into(), enabled: true });
    let p0 = Arc::new(p0);
    let host = s.expr.clone().unwrap();
    let render = |p: &Project, i: i64| {
        let mut r = Renderer::new(p, &NoFootage, RenderOpts::default());
        r.expr = Some(&*host);
        r.comp_frame(cid, fr.tick_of(i))
    };
    // (key index, frames that must stay, frames that must go)
    for (n, keep, go) in [(1, 0..18, [21, 60, 119]), (0, 82..120, [0, 20, 79])] {
        let f = Frames::default();
        let k = |content: u64, frame: i64| FrameKey { content, comp: cid.0, ..key(0, frame, 0) };
        let id0 = f.content_of(&p0, 1, cid);
        (0..120).for_each(|i| insert(&f, k(id0, i)));
        let mut p1 = p0.clone();
        Arc::make_mut(&mut p1).comp_mut(cid).unwrap().layer_mut(l).unwrap().props.prop_mut("transform/scale").unwrap().keys[n].value =
            Value::Vec3([90.0, 90.0, 100.0]);
        let kept = f.cached_frames(&k(f.content_of(&p1, 2, cid), 0));
        assert!(keep.clone().all(|i| kept.contains(&i)) && !go.iter().any(|i| kept.contains(i)), "key {n}: {kept:?}");
        for i in kept {
            assert_eq!(render(&p0, i), render(&p1, i), "key {n}: kept frame {i} draws other pixels now");
        }
    }
}

/// An expression that reads its own layer's markers (no other layer or comp named) re-renders
/// that layer when one of its markers changes. Every frame kept renders as before.
#[test]
fn a_marker_read_by_its_own_layer_re_renders_it() {
    use effectcraft_engine::render::NoFootage;
    use serde_json::json;
    let mut s = effectcraft_host::session();
    let cid = ItemId(s.execute("comp.new", json!({"name": "A", "width": 16, "height": 16, "duration": 4, "frameRate": 30})).unwrap()["comp"].as_u64().unwrap());
    let l = s.execute("layer.newSolid", json!({"color": "#ff0000", "width": 4, "height": 4})).unwrap()["layer"].as_u64().unwrap();
    s.execute("prop.setExpression", json!({"layer": l, "path": "transform/opacity", "expression": "marker.numKeys > 0 ? 0 : 100"})).unwrap();
    let p0 = s.project.clone();
    s.execute("markers.set", json!({"comp": cid.0, "layer": l, "new": true, "time": 3.0})).unwrap();
    let p1 = s.project.clone();
    let host = s.expr.clone().unwrap();
    let fr = p0.comp(cid).unwrap().frame_rate;
    let render = |p: &Project, i: i64| {
        let mut r = Renderer::new(p, &NoFootage, RenderOpts::default());
        r.expr = Some(&*host);
        r.comp_frame(cid, fr.tick_of(i))
    };
    let f = Frames::default();
    let k = |content: u64, frame: i64| FrameKey { content, comp: cid.0, ..key(0, frame, 0) };
    let id0 = f.content_of(&p0, 1, cid);
    (0..120).for_each(|i| insert(&f, k(id0, i)));
    let kept = f.cached_frames(&k(f.content_of(&p1, 2, cid), 0));
    assert!(!kept.contains(&0), "the layer's frames must go: {kept:?}");
    for i in kept {
        assert_eq!(render(&p0, i), render(&p1, i), "kept frame {i} draws other pixels now");
    }
}

/// An expression that reads its own layer's audio re-renders that layer when its volume
/// changes.
#[test]
fn a_volume_read_by_its_own_layer_re_renders_it() {
    use effectcraft_engine::Session;
    use effectcraft_engine::project::{Expression, Footage, FootageKind, Value, build};
    use serde_json::json;
    let mut s = Session::default();
    let cid = ItemId(s.execute("comp.new", json!({"name": "A", "width": 8, "height": 8, "duration": 4, "frameRate": 30})).unwrap()["comp"].as_u64().unwrap());
    let mut p = (*s.project).clone();
    let clip = p.add_item(
        "clip.mp4",
        effectcraft_color::Label::None,
        None,
        ItemKind::Footage(Footage { kind: FootageKind::Video, has_video: true, has_audio: true, width: 8, height: 8, ..Default::default() }),
    );
    let comp = p.comp(cid).unwrap().clone();
    let mut layer = build::layer(&mut p, &comp, "Clip", LayerSource::Footage { item: clip }, (8, 8), None);
    layer.props.prop_mut("transform/opacity").unwrap().expr =
        Some(Expression { text: "100 + thisLayer(\"Audio\")(\"Audio Levels\")[0]".into(), enabled: true });
    let id = layer.id;
    p.comp_mut(cid).unwrap().layers.push(layer);
    let p0 = Arc::new(p);
    let f = Frames::default();
    let k = |content: u64, frame: i64| FrameKey { content, comp: cid.0, ..key(0, frame, 0) };
    let id0 = f.content_of(&p0, 1, cid);
    (0..120).for_each(|i| insert(&f, k(id0, i)));
    let mut p1 = p0.clone();
    Arc::make_mut(&mut p1).comp_mut(cid).unwrap().layer_mut(id).unwrap().props.prop_mut("audio/levels").unwrap().value = Value::Vec2([-6.0, -6.0]);
    let kept = f.cached_frames(&k(f.content_of(&p1, 2, cid), 0));
    assert!(!kept.contains(&0), "the clip's frames must go: {kept:?}");
}

/// An expression on the keyed layer can read the keyed property at other times (Rotation
/// reading Position a second earlier): a key edit re-renders the layer's whole length. Every
/// frame kept renders as before.
#[test]
fn a_keyframe_edit_read_at_other_times_by_its_layer_re_renders_it() {
    use effectcraft_engine::project::{LayerId, Value};
    use effectcraft_engine::render::NoFootage;
    use serde_json::json;
    let mut s = effectcraft_host::session();
    let cid = ItemId(s.execute("comp.new", json!({"name": "A", "width": 16, "height": 16, "duration": 4, "frameRate": 30})).unwrap()["comp"].as_u64().unwrap());
    let l = LayerId(s.execute("layer.newSolid", json!({"color": "#ff0000", "width": 4, "height": 4})).unwrap()["layer"].as_u64().unwrap());
    let fr = s.project.comp(cid).unwrap().frame_rate;
    for (f, x) in [(20, 2.0), (80, 14.0)] {
        s.execute("prop.addKey", json!({"layer": l.0, "path": "transform/position", "time": fr.tick_of(f).seconds(), "value": [x, 8.0, 0.0]})).unwrap();
    }
    s.execute("prop.setExpression", json!({"layer": l.0, "path": "transform/rotation", "expression": "transform.position.valueAtTime(time - 1)[0] * 3"}))
        .unwrap();
    let p0 = s.project.clone();
    let host = s.expr.clone().unwrap();
    let render = |p: &Project, i: i64| {
        let mut r = Renderer::new(p, &NoFootage, RenderOpts::default());
        r.expr = Some(&*host);
        r.comp_frame(cid, fr.tick_of(i))
    };
    let f = Frames::default();
    let k = |content: u64, frame: i64| FrameKey { content, comp: cid.0, ..key(0, frame, 0) };
    let id0 = f.content_of(&p0, 1, cid);
    (0..120).for_each(|i| insert(&f, k(id0, i)));
    let mut p1 = p0.clone();
    Arc::make_mut(&mut p1).comp_mut(cid).unwrap().layer_mut(l).unwrap().props.prop_mut("transform/position").unwrap().keys[0].value =
        Value::Vec3([8.0, 2.0, 0.0]);
    let kept = f.cached_frames(&k(f.content_of(&p1, 2, cid), 0));
    assert!(!kept.contains(&100), "Rotation at frame 100 reads Position at frame 70: {kept:?}");
    for i in kept {
        assert_eq!(render(&p0, i), render(&p1, i), "kept frame {i} draws other pixels now");
    }
}

/// An edit two precomps down (Outer → Mid → Inner) re-renders Outer where it shows, although
/// Mid's own comp didn't change. Every frame kept renders as before.
#[test]
fn an_edit_two_precomps_down_reaches_the_outer_comp() {
    use effectcraft_engine::Session;
    use effectcraft_engine::project::LayerId;
    use effectcraft_engine::render::render_frame;
    use serde_json::json;
    let mut s = Session::default();
    let id = |v: serde_json::Value, k: &str| v[k].as_u64().unwrap();
    let comp = |s: &mut Session, name: &str| {
        ItemId(id(s.execute("comp.new", json!({"name": name, "width": 16, "height": 16, "duration": 4, "frameRate": 30})).unwrap(), "comp"))
    };
    let inner = comp(&mut s, "Inner");
    let solid = |s: &mut Session, color: &str| {
        LayerId(id(s.execute("layer.newSolid", json!({"comp": inner.0, "color": color, "width": 6, "height": 6})).unwrap(), "layer"))
    };
    let (a, b) = (solid(&mut s, "#ff0000"), solid(&mut s, "#00ff00"));
    let mid = comp(&mut s, "Mid");
    s.execute("layer.addItem", json!({"comp": mid.0, "item": inner.0})).unwrap();
    let outer = comp(&mut s, "Outer");
    s.execute("layer.addItem", json!({"comp": outer.0, "item": mid.0})).unwrap();
    let fr = s.project.comp(outer).unwrap().frame_rate;
    let t = |f: i64| fr.tick_of(f);
    let mut p0 = (*s.project).clone();
    for (l, i, o) in [(a, 0, 30), (b, 30, 60)] {
        let l = p0.comp_mut(inner).unwrap().layer_mut(l).unwrap();
        (l.start_time, l.in_point, l.out_point) = (t(i), t(i), t(o));
    }
    let p0 = Arc::new(p0);
    let f = Frames::default();
    let k = |content: u64, frame: i64| FrameKey { content, comp: outer.0, ..key(0, frame, 0) };
    let id0 = f.content_of(&p0, 1, outer);
    (0..120).for_each(|i| insert(&f, k(id0, i)));
    let mut p1 = p0.clone();
    Arc::make_mut(&mut p1).comp_mut(inner).unwrap().layer_mut(b).unwrap().out_point = t(45);
    let kept = f.cached_frames(&k(f.content_of(&p1, 2, outer), 0));
    assert!(kept.contains(&10) && kept.contains(&100) && !kept.contains(&50), "{kept:?}");
    for i in kept {
        assert!(render_frame(&p0, outer, t(i), 1.0).data == render_frame(&p1, outer, t(i), 1.0).data, "kept frame {i} renders differently after the edit");
    }
}

/// An expression reading its own layer's in point (a fade from `inPoint`) re-renders the layer
/// when it is trimmed, and one reading its `index` re-renders it when a short layer is added
/// above it. Every frame kept renders as before.
#[test]
fn expressions_reading_their_own_layer_re_render_on_a_trim_or_a_layer_above() {
    use effectcraft_engine::Session;
    use effectcraft_engine::project::LayerId;
    use effectcraft_engine::render::NoFootage;
    use serde_json::json;
    /// Trim the layer to start at frame 10.
    fn trim(s: &mut Session, cid: ItemId, l: LayerId) {
        let fr = s.project.comp(cid).unwrap().frame_rate;
        Arc::make_mut(&mut s.project).comp_mut(cid).unwrap().layer_mut(l).unwrap().in_point = fr.tick_of(10);
    }
    /// Add a solid above the layer on frames 0–5.
    fn layer_above(s: &mut Session, cid: ItemId, _: LayerId) {
        let fr = s.project.comp(cid).unwrap().frame_rate;
        let a = s.execute("layer.newSolid", json!({"comp": cid.0, "color": "#00ff00", "width": 2, "height": 2})).unwrap()["layer"].as_u64().unwrap();
        let a = Arc::make_mut(&mut s.project).comp_mut(cid).unwrap().layer_mut(LayerId(a)).unwrap();
        (a.start_time, a.in_point, a.out_point) = (fr.tick_of(0), fr.tick_of(0), fr.tick_of(5));
    }
    type Edit = fn(&mut Session, ItemId, LayerId);
    let edits: [(&str, &str, &str, Edit); 2] =
        [("trim", "transform/opacity", "linear(time, inPoint, inPoint + 2, 20, 100)", trim), ("layer above", "transform/rotation", "index * 20", layer_above)];
    for (name, path, expression, edit) in edits {
        let mut s = effectcraft_host::session();
        let new = json!({"name": "A", "width": 16, "height": 16, "duration": 4, "frameRate": 30});
        let cid = ItemId(s.execute("comp.new", new).unwrap()["comp"].as_u64().unwrap());
        let l = LayerId(s.execute("layer.newSolid", json!({"comp": cid.0, "color": "#ff0000", "width": 6, "height": 6})).unwrap()["layer"].as_u64().unwrap());
        s.execute("prop.setExpression", json!({"comp": cid.0, "layer": l.0, "path": path, "expression": expression})).unwrap();
        let p0 = s.project.clone();
        edit(&mut s, cid, l);
        let p1 = s.project.clone();
        let fr = p0.comp(cid).unwrap().frame_rate;
        let host = s.expr.clone().unwrap();
        let render = |p: &Project, i: i64| {
            let mut r = Renderer::new(p, &NoFootage, RenderOpts::default());
            r.expr = Some(&*host);
            r.comp_frame(cid, fr.tick_of(i))
        };
        assert_ne!(render(&p0, 60), render(&p1, 60), "{name}: the edit changes frame 60");
        let f = Frames::default();
        let k = |content: u64, frame: i64| FrameKey { content, comp: cid.0, ..key(0, frame, 0) };
        let id0 = f.content_of(&p0, 1, cid);
        (0..120).for_each(|i| insert(&f, k(id0, i)));
        let kept = f.cached_frames(&k(f.content_of(&p1, 2, cid), 0));
        assert!(!kept.contains(&60), "{name}: frame 60 must go: {kept:?}");
        for i in kept {
            assert_eq!(render(&p0, i), render(&p1, i), "{name}: kept frame {i} draws other pixels now");
        }
    }
}

/// An expression inside a precomp reading a layer of the comp nesting it (a hidden Controls
/// layer, `comp("Main").layer("Controls")`) re-renders where the precomp shows it when that
/// layer changes. Every frame kept renders as before.
#[test]
fn an_edit_read_by_a_precomp_expression_reaches_the_precomp() {
    use effectcraft_engine::project::LayerId;
    use effectcraft_engine::render::NoFootage;
    use serde_json::json;
    let mut s = effectcraft_host::session();
    let id = |v: serde_json::Value, k: &str| v[k].as_u64().unwrap();
    let new_comp = |s: &mut effectcraft_engine::Session, name: &str| {
        ItemId(id(s.execute("comp.new", json!({"name": name, "width": 16, "height": 16, "duration": 4, "frameRate": 30})).unwrap(), "comp"))
    };
    let inner = new_comp(&mut s, "Inner");
    let dot = id(s.execute("layer.newSolid", json!({"comp": inner.0, "color": "#ff0000", "width": 6, "height": 6})).unwrap(), "layer");
    s.execute(
        "prop.setExpression",
        json!({"comp": inner.0, "layer": dot, "path": "transform/opacity", "expression": "comp(\"Main\").layer(\"Controls\").transform.rotation"}),
    )
    .unwrap();
    let main = new_comp(&mut s, "Main");
    s.execute("layer.addItem", json!({"comp": main.0, "item": inner.0})).unwrap();
    let controls = json!({"comp": main.0, "name": "Controls", "color": "#0000ff", "width": 2, "height": 2});
    let controls = LayerId(id(s.execute("layer.newSolid", controls).unwrap(), "layer"));
    // (hidden, and short: the precomp reads it at every frame all the same)
    let fr = s.project.comp(main).unwrap().frame_rate;
    let c = Arc::make_mut(&mut s.project).comp_mut(main).unwrap().layer_mut(controls).unwrap();
    (c.switches.video, c.start_time, c.in_point, c.out_point) = (false, fr.tick_of(0), fr.tick_of(0), fr.tick_of(10));
    let p0 = s.project.clone();
    s.execute("prop.set", json!({"comp": main.0, "layer": controls.0, "path": "transform/rotation", "value": 60.0})).unwrap();
    let p1 = s.project.clone();
    let host = s.expr.clone().unwrap();
    let render = |p: &Project, i: i64| {
        let mut r = Renderer::new(p, &NoFootage, RenderOpts::default());
        r.expr = Some(&*host);
        r.comp_frame(main, fr.tick_of(i))
    };
    assert_ne!(render(&p0, 60), render(&p1, 60), "the dot's opacity follows Controls");
    let f = Frames::default();
    let k = |content: u64, frame: i64| FrameKey { content, comp: main.0, ..key(0, frame, 0) };
    let id0 = f.content_of(&p0, 1, main);
    (0..120).for_each(|i| insert(&f, k(id0, i)));
    let kept = f.cached_frames(&k(f.content_of(&p1, 2, main), 0));
    assert!(!kept.contains(&60), "the precomp's frames must go: {kept:?}");
    for i in kept {
        assert_eq!(render(&p0, i), render(&p1, i), "kept frame {i} draws other pixels now");
    }
}

/// An edit inside a precomp whose layer runs Echo (it reads the precomp at earlier times)
/// re-renders the precomp layer's whole span, not only where the precomp shows the edit. Every
/// frame kept renders as before.
#[test]
fn an_edit_in_a_precomp_under_a_time_effect_reaches_its_whole_layer() {
    use effectcraft_engine::Session;
    use effectcraft_engine::project::LayerId;
    use effectcraft_engine::render::render_frame;
    use serde_json::json;
    let mut s = Session::default();
    let id = |v: serde_json::Value, k: &str| v[k].as_u64().unwrap();
    let inner = ItemId(id(s.execute("comp.new", json!({"name": "Inner", "width": 16, "height": 16, "duration": 4, "frameRate": 30})).unwrap(), "comp"));
    let solid = |s: &mut Session, color: &str| {
        LayerId(id(s.execute("layer.newSolid", json!({"comp": inner.0, "color": color, "width": 6, "height": 6})).unwrap(), "layer"))
    };
    let (a, b) = (solid(&mut s, "#ff0000"), solid(&mut s, "#00ff00"));
    let outer = ItemId(id(s.execute("comp.new", json!({"name": "Outer", "width": 16, "height": 16, "duration": 4, "frameRate": 30})).unwrap(), "comp"));
    let pre = id(s.execute("layer.addItem", json!({"comp": outer.0, "item": inner.0})).unwrap(), "layer");
    // Echo reaching 9 frames back, once (more echoes of one solid add up past full and look alike).
    s.execute("effect.apply", json!({"comp": outer.0, "layers": [pre], "effect": "Echo"})).unwrap();
    s.execute("prop.set", json!({"comp": outer.0, "layer": pre, "path": "effects/#1/echoTime", "value": -0.3})).unwrap();
    let fr = s.project.comp(outer).unwrap().frame_rate;
    let t = |f: i64| fr.tick_of(f);
    let mut p0 = (*s.project).clone();
    for (l, i, o) in [(a, 0, 30), (b, 30, 60)] {
        let l = p0.comp_mut(inner).unwrap().layer_mut(l).unwrap();
        (l.start_time, l.in_point, l.out_point) = (t(i), t(i), t(o));
    }
    let p0 = Arc::new(p0);
    let f = Frames::default();
    let k = |content: u64, frame: i64| FrameKey { content, comp: outer.0, ..key(0, frame, 0) };
    let id0 = f.content_of(&p0, 1, outer);
    (0..120).for_each(|i| insert(&f, k(id0, i)));
    let mut p1 = p0.clone();
    Arc::make_mut(&mut p1).comp_mut(inner).unwrap().layer_mut(b).unwrap().out_point = t(45);
    let same = |i: i64| render_frame(&p0, outer, t(i), 1.0).data == render_frame(&p1, outer, t(i), 1.0).data;
    assert!(!same(65), "Echo at frame 65 shows frame 56, which the trim removed");
    let kept = f.cached_frames(&k(f.content_of(&p1, 2, outer), 0));
    assert!(!kept.contains(&65), "{kept:?}");
    for i in kept {
        assert!(same(i), "kept frame {i} renders differently after the edit");
    }
}

/// A keyframe edit beside muted keys reaches back to the nearest keys that play: muted keys are
/// skipped, so the segment the edited key ends can start far before the two keys before it.
/// Every frame kept renders as before.
#[test]
fn a_keyframe_edit_beside_muted_keys_reaches_the_keys_that_play() {
    use effectcraft_engine::Session;
    use effectcraft_engine::project::{LayerId, Value};
    use effectcraft_engine::render::render_frame;
    use serde_json::json;
    let mut s = Session::default();
    let new = json!({"name": "A", "width": 16, "height": 16, "duration": 4, "frameRate": 30});
    let cid = ItemId(s.execute("comp.new", new).unwrap()["comp"].as_u64().unwrap());
    let l = LayerId(s.execute("layer.newSolid", json!({"color": "#ff0000", "width": 4, "height": 4})).unwrap()["layer"].as_u64().unwrap());
    let fr = s.project.comp(cid).unwrap().frame_rate;
    // Keys at frames 0, 20, 40, 60, 80, 100; the ones at 20 and 40 muted.
    for i in 0..6 {
        let add = json!({"layer": l.0, "path": "transform/position", "time": fr.tick_of(i * 20).seconds(), "value": [2.0 + i as f64 * 2.0, 8.0, 0.0]});
        s.execute("prop.addKey", add).unwrap();
    }
    let mut p0 = (*s.project).clone();
    for k in p0.comp_mut(cid).unwrap().layer_mut(l).unwrap().props.prop_mut("transform/position").unwrap().keys.iter_mut().skip(1).take(2) {
        k.muted = true;
    }
    let p0 = Arc::new(p0);
    let mut p1 = p0.clone();
    Arc::make_mut(&mut p1).comp_mut(cid).unwrap().layer_mut(l).unwrap().props.prop_mut("transform/position").unwrap().keys[3].value =
        Value::Vec3([8.0, 2.0, 0.0]);
    let same = |i: i64| render_frame(&p0, cid, fr.tick_of(i), 1.0).data == render_frame(&p1, cid, fr.tick_of(i), 1.0).data;
    assert!(!same(10), "the edited key's segment starts at frame 0");
    let f = Frames::default();
    let k = |content: u64, frame: i64| FrameKey { content, comp: cid.0, ..key(0, frame, 0) };
    let id0 = f.content_of(&p0, 1, cid);
    (0..120).for_each(|i| insert(&f, k(id0, i)));
    let kept = f.cached_frames(&k(f.content_of(&p1, 2, cid), 0));
    assert!(!kept.contains(&10), "{kept:?}");
    for i in kept {
        assert!(same(i), "kept frame {i} renders differently after the edit");
    }
}
