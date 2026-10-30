use super::*;

#[test]
fn converting_a_frame_with_nan_alpha_keeps_the_viewer_alive() {
    let image = effectcraft_engine::render::Image::filled(1, 1, [0.5, 0.25, 0.1, f32::NAN]);
    assert_eq!(to_color_image(&image).pixels, [egui::Color32::TRANSPARENT]);
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

/// Each time the GPU runs out of memory, half as many GPU frames are kept, down to a floor.
#[test]
fn running_out_of_video_memory_halves_the_gpu_frame_budget() {
    let f = Frames::default();
    let mut c = f.cache.lock().unwrap();
    assert_eq!(c.gpu_budget, GPU_BUDGET);
    c.gpu_out_of_memory();
    assert_eq!(c.gpu_budget, GPU_BUDGET / 2);
    for _ in 0..20 {
        c.gpu_out_of_memory();
    }
    assert_eq!(c.gpu_budget, MIN_GPU_BUDGET);
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

/// A cut or trim recaches only the frames of the layers it touched (#1641), and none are
/// kept when the change reaches other layers (here a child of the trimmed layer).
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
    let kept: Vec<i64> = (fr.frame_at(out) + 2..frames.len() as i64).collect();
    assert_eq!(cached(after), kept, "the cut layer's frames (and one either side) render again; the rest stay");

    s.execute("layer.setParent", json!({"layers": [b], "parent": a})).unwrap();
    let parented = cache_all(&s);
    s.execute("layer.timing", json!({"layers": [a], "out": 1})).unwrap();
    assert_eq!(cached(f.content_of(&s.project, s.revision, comp)), Vec::<i64>::new(), "the trimmed layer's child moves everywhere");
    assert_eq!(cached(parented).len(), frames.len(), "the earlier frames stay for undo");
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
