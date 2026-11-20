//! Run the viewer scheduler and the external helper protocol in separate processes.

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::path::Path;
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use effectcraft_engine::Session;
    use effectcraft_engine::effects::plugin::{EffectPlugin, PluginFrame, PluginHost, PluginManifest, PluginParams, register_plugin};
    use effectcraft_engine::project::{Footage, FootageKind, ItemId, ItemKind};
    use effectcraft_engine::render::{FootageSource, Image, RenderOpts};
    use effectcraft_engine::time::{FrameRate, Tick};
    use effectcraft_helper_host::client::{Helper, HelperEffect};
    use effectcraft_helper_host::protocol::HELPER_ARG;
    use effectcraft_helper_host::serve::{Provider, serve};
    use effectcraft_ui_egui::frames::{FrameImage, FrameKey, Frames, Playhead, RenderSource};
    use serde_json::json;

    const EFFECT: &str = "org.test.viewer.input";
    const PARALLEL_DIR: &str = "EFFECTCRAFT_VIEWER_TEST_PARALLEL_DIR";
    const WAIT: Duration = Duration::from_secs(12);
    const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
    const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
    const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];

    struct Mock(PluginManifest);

    impl EffectPlugin for Mock {
        fn manifest(&self) -> &PluginManifest {
            &self.0
        }

        fn render(&self, _: &mut PluginFrame, _: &PluginParams, _: f64) -> Result<(), String> {
            Err("a host callback is required".into())
        }

        fn render_v2(&self, frame: &mut PluginFrame, _: &PluginParams, _: f64, host: &dyn PluginHost) -> Result<(), String> {
            if let Some(dir) = std::env::var_os(PARALLEL_DIR) {
                let first = host.time() < 0.52;
                let dir = Path::new(&dir);
                let (own, other) = if first { ("first", "second") } else { ("second", "first") };
                std::fs::write(dir.join(own), []).map_err(|e| e.to_string())?;
                let deadline = Instant::now() + Duration::from_secs(5);
                while !dir.join(other).exists() {
                    if Instant::now() >= deadline {
                        return Err("the other viewer frame never reached the helper pool".into());
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                frame.pixels.fill(GREEN);
                return Ok(());
            }
            let input = host.input_at(0.0).ok_or("the callback returned no image")?;
            if input.pixels.first() != Some(&BLUE) {
                return Err("the callback returned the wrong frame".into());
            }
            frame.pixels.fill(GREEN);
            Ok(())
        }
    }

    impl Provider for Mock {
        fn name(&self) -> String {
            "Viewer test helper".into()
        }

        fn wants_padding(&self, _: &dyn EffectPlugin) -> bool {
            false
        }

        fn max_processes(&self) -> u32 {
            if std::env::var_os(PARALLEL_DIR).is_some() { 2 } else { 1 }
        }

        fn effects(&mut self) -> (Vec<Arc<dyn EffectPlugin>>, Vec<String>) {
            (vec![Arc::new(Self(self.0.clone()))], vec![])
        }
    }

    fn mock() -> Mock {
        Mock(PluginManifest {
            api: 2,
            id: EFFECT.into(),
            name: "Viewer test effect".into(),
            category: "Test".into(),
            version: "1".into(),
            author: String::new(),
            description: String::new(),
            params: vec![],
        })
    }

    struct Clip {
        gated: AtomicBool,
        entered: Sender<()>,
        queued: Mutex<Receiver<()>>,
    }

    fn wait_for(mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + WAIT;
        while !ready() {
            assert!(Instant::now() < deadline, "timed out waiting for the viewer");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    impl Clip {
        /// Arrange for exactly one other worker to steal part of the callback. The
        /// current worker waits for that part while the UI queues another viewer frame.
        fn callback_work(&self) {
            let parked = AtomicUsize::new(0);
            let release = AtomicBool::new(false);
            let other_workers = rayon::current_num_threads().saturating_sub(2);
            let owner = std::thread::current().id();
            rayon::scope(|scope| {
                for _ in 0..other_workers {
                    scope.spawn(|_| {
                        parked.fetch_add(1, Ordering::Release);
                        wait_for(|| release.load(Ordering::Acquire));
                    });
                }
                wait_for(|| parked.load(Ordering::Acquire) == other_workers);
                let (done, wait) = mpsc::channel();
                let (finished, stolen) = rayon::join(
                    move || wait.recv_timeout(WAIT),
                    || {
                        let stolen = std::thread::current().id() != owner;
                        let queued = stolen && self.entered.send(()).is_ok() && self.queued.lock().unwrap().recv_timeout(WAIT).is_ok();
                        if queued {
                            // The worker can do other Rayon work while in this branch.
                            // With separate frame drains in the pool it picks up the newly
                            // queued frame and waits for the helper this callback holds.
                            while rayon::yield_now() == Some(rayon::Yield::Executed) {}
                        }
                        let _ = done.send(());
                        queued
                    },
                );
                release.store(true, Ordering::Release);
                assert!(finished.is_ok() && stolen, "the callback work was not stolen by a second worker");
            });
        }
    }

    impl FootageSource for Clip {
        fn frame(&self, _: ItemId, _: &Footage, t: Tick) -> Option<Arc<Image>> {
            let color = if t == Tick::ZERO {
                if !self.gated.swap(true, Ordering::Relaxed) {
                    self.callback_work();
                }
                BLUE
            } else {
                RED
            };
            Some(Arc::new(Image::filled(8, 8, color)))
        }
    }

    fn viewer(footage_source: Arc<dyn FootageSource>, effect: Option<&str>) -> (Frames, RenderSource, FrameKey) {
        let mut session = Session::default();
        session.execute_checked("comp.new", json!({"width": 8, "height": 8, "frameRate": 30, "duration": 2})).unwrap();
        let comp = session.active_comp_id().unwrap();
        let footage = Footage {
            kind: FootageKind::Video,
            width: 8,
            height: 8,
            has_video: true,
            frame_rate: FrameRate::FPS_30,
            duration: Tick::from_seconds_f64(2.0),
            ..Default::default()
        };
        let item = Arc::make_mut(&mut session.project).add_item("clip.mov", Default::default(), None, ItemKind::Footage(footage));
        session.execute_checked("layer.addItem", json!({"item": item.0})).unwrap();
        if let Some(effect) = effect {
            session.execute_checked("effect.apply", json!({"effect": effect})).unwrap();
        }
        let source = RenderSource {
            project: session.project.clone(),
            footage: footage_source,
            expr: None,
            layer_cache: Arc::default(),
            gpu: None,
            gpu_display: false,
            disk: None,
        };
        let frames = Frames::with_lane_width(1);
        let first = FrameKey {
            revision: session.revision,
            content: frames.content_of(&source.project, session.revision, comp),
            comp: comp.0,
            frame: 15,
            scale: 1000,
            view: 0,
            opts: 0,
        };
        (frames, source, first)
    }

    fn frame_time(key: FrameKey) -> Tick {
        FrameRate::FPS_30.tick_of(key.frame)
    }

    fn register_helper(exe: &Path, processes: usize) -> Arc<Helper> {
        let (helper, mut effects, errors) = Helper::start(exe).unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(effects.len(), 1);
        assert_eq!(helper.status().max_processes, processes);
        register_plugin(Arc::new(HelperEffect::new(helper.clone(), effects.pop().unwrap()))).unwrap();
        helper
    }

    fn run_scenario(exe: &Path, playback: bool) {
        let state = tempfile::tempdir().unwrap();
        effectcraft_helper_host::blocklist::set_state_dir(state.path().to_owned());
        let helper = register_helper(exe, 1);

        let (entered, callback) = mpsc::channel();
        let (queued, pending) = mpsc::channel();
        let clip = Arc::new(Clip { gated: AtomicBool::new(false), entered, queued: Mutex::new(pending) });
        let (frames, source, first) = viewer(clip, Some(EFFECT));
        let comp = ItemId(first.comp);
        let second = FrameKey { frame: 16, ..first };
        let preview = FrameKey { frame: 17, ..first };
        let opts = RenderOpts::default();
        let playhead = |key: FrameKey| Playhead { series: key, cur: key.frame, start: 0, end: 59, step: 1, looping: false };
        if playback {
            frames.set_playhead(Some(playhead(first)));
        }
        frames.request_urgent(&source, first, comp, frame_time(first), opts);
        callback.recv_timeout(WAIT).expect("the helper did not request another frame on the viewer pool");
        if playback {
            frames.set_playhead(Some(playhead(second)));
        }
        let start = Instant::now();
        frames.request_urgent(&source, second, comp, frame_time(second), opts);
        queued.send(()).unwrap();
        let mode = if playback { "playback" } else { "scrubbing" };
        check_frame(&frames, second, mode, start);
        if playback {
            let start = Instant::now();
            frames.request(&source, preview, comp, frame_time(preview), opts);
            check_frame(&frames, preview, "playback prefetch", start);
        }
        wait_for(|| frames.inflight() == 0);
        assert_eq!(helper.status().processes, 1);
        helper.stop();
    }

    /// Neither helper process can finish until both viewer frames have started rendering.
    fn foreground_frames_use_multiple_helper_processes(exe: &Path) {
        let state = tempfile::tempdir().unwrap();
        effectcraft_helper_host::blocklist::set_state_dir(state.path().to_owned());
        let helper = register_helper(exe, 2);
        let (entered, _) = mpsc::channel();
        let (_, pending) = mpsc::channel();
        let source = Arc::new(Clip { gated: AtomicBool::new(true), entered, queued: Mutex::new(pending) });
        let (frames, source, first) = viewer(source, Some(EFFECT));
        let comp = ItemId(first.comp);
        let second = FrameKey { frame: 16, ..first };
        let opts = RenderOpts::default();
        let dir = std::env::var_os(PARALLEL_DIR).unwrap();
        let first_arrived = Path::new(&dir).join("first");
        frames.request_urgent(&source, first, comp, frame_time(first), opts);
        wait_for(|| first_arrived.exists());
        let start = Instant::now();
        frames.request_urgent(&source, second, comp, frame_time(second), opts);
        check_frame(&frames, second, "parallel foreground", start);
        wait_for(|| frames.inflight() == 0);
        assert_eq!(helper.status().processes, 2);
        helper.stop();
    }

    /// A slow prefetch on a paused viewer must not keep the requested frame off screen.
    fn paused_prefetch_does_not_block_the_viewer(exe: &Path) {
        struct SlowClip {
            entered: Sender<()>,
            resume: Mutex<Receiver<()>>,
        }
        impl FootageSource for SlowClip {
            fn frame(&self, _: ItemId, _: &Footage, t: Tick) -> Option<Arc<Image>> {
                if t == FrameRate::FPS_30.tick_of(15) {
                    self.entered.send(()).ok()?;
                    self.resume.lock().ok()?.recv_timeout(WAIT).ok()?;
                }
                Some(Arc::new(Image::filled(8, 8, BLUE)))
            }
        }
        let state = tempfile::tempdir().unwrap();
        effectcraft_helper_host::blocklist::set_state_dir(state.path().to_owned());
        let helper = register_helper(exe, 1);
        let (entered, started) = mpsc::channel();
        let (release, resume) = mpsc::channel();
        let (frames, source, prefetch) = viewer(Arc::new(SlowClip { entered, resume: Mutex::new(resume) }), Some(EFFECT));
        let comp = ItemId(prefetch.comp);
        let urgent = FrameKey { frame: 16, ..prefetch };
        let opts = RenderOpts::default();
        frames.request(&source, prefetch, comp, frame_time(prefetch), opts);
        started.recv_timeout(WAIT).expect("the paused prefetch did not reach the decoder");
        let start = Instant::now();
        frames.request_urgent(&source, urgent, comp, frame_time(urgent), opts);
        let hold = Duration::from_secs(1);
        let mut early = None;
        while start.elapsed() < hold && early.is_none() {
            if frames.get(&urgent).is_some() {
                early = Some(start.elapsed());
            } else {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        release.send(()).unwrap();
        wait_for(|| frames.get(&urgent).is_some());
        eprintln!(
            "paused prefetch: {:.1} ms to urgent frame (before prefetch release: {})",
            early.unwrap_or_else(|| start.elapsed()).as_secs_f64() * 1000.0,
            early.is_some()
        );
        wait_for(|| frames.inflight() == 0);
        assert!(early.is_some(), "the paused prefetch held up the urgent viewer frame");
        helper.stop();
    }

    fn check_frame(frames: &Frames, key: FrameKey, mode: &str, start: Instant) {
        let mut image = None;
        wait_for(|| {
            image = frames.get_for_viewer(&key, None).filter(|(actual, _)| *actual == key);
            image.is_some()
        });
        let Some((_, FrameImage::Cpu(image))) = image else { panic!("expected CPU pixels for {mode}") };
        let pixel = image.pixels[4 * 8 + 4];
        eprintln!("{mode}: {:.1} ms to frame {}, pixel {pixel:?}", start.elapsed().as_secs_f64() * 1000.0, key.frame);
        assert_eq!(pixel, egui::Color32::GREEN, "{mode}: the helper effect must be visible in the viewer");
    }

    pub fn main() {
        let args: Vec<String> = std::env::args().skip(1).collect();
        // Match the helper protocol test's libtest listing for cargo-nextest.
        if args.iter().any(|arg| arg == "--list") {
            if !args.iter().any(|arg| arg == "--ignored") {
                println!("viewer_helper_callbacks: test");
            }
            return;
        }
        let exe = std::env::current_exe().unwrap();
        match args.first().map(String::as_str) {
            Some(HELPER_ARG) => serve(&mut mock()).unwrap(),
            Some("--playback") => run_scenario(&exe, true),
            Some("--scrubbing") => run_scenario(&exe, false),
            Some("--paused-prefetch") => paused_prefetch_does_not_block_the_viewer(&exe),
            Some("--parallel-foreground") => foreground_frames_use_multiple_helper_processes(&exe),
            _ => {
                let dir = tempfile::tempdir().unwrap();
                let results = ["--playback", "--scrubbing", "--paused-prefetch", "--parallel-foreground"].map(|mode| {
                    let mut cmd = Command::new(&exe);
                    cmd.arg(mode)
                        .env("EFFECTCRAFT_HELPER_LOAD_TIMEOUT", "3")
                        .env("EFFECTCRAFT_HELPER_TIMEOUT", "15")
                        .env("EFFECTCRAFT_HELPER_MAX_PROCESSES", "2")
                        .env_remove(PARALLEL_DIR);
                    if mode == "--parallel-foreground" {
                        cmd.env(PARALLEL_DIR, dir.path());
                    }
                    cmd.status().unwrap()
                });
                assert!(results.iter().all(std::process::ExitStatus::success), "{results:?}");
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    native::main();
}

#[cfg(target_arch = "wasm32")]
fn main() {}
