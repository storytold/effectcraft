//! The viewer on the GPU compositor (egui_kittest with wgpu): frames composited on the GPU match
//! the CPU viewer. Skips without a GPU adapter (CI). Set `EC_SNAPSHOT_DIR` to keep PNGs.

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use egui_kittest::Harness;
use serde_json::json;

/// Step until the GPU compositor's worker is done and a frame has landed in the viewer.
fn settle(h: &mut Harness<'_, EffectcraftApp>) {
    for _ in 0..3000 {
        h.step();
        if !h.state().gpu_initializing() && h.state().frames.inflight() == 0 && h.state().frames.last_ms.lock().map(|v| *v > 0.0).unwrap_or(false) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    for _ in 0..4 {
        h.step();
    }
}

/// Step until the GPU compositor's worker has handed its result over (attached, or why not).
/// The first step starts the build.
pub(crate) fn wait_for_compositor(h: &mut Harness<'_, EffectcraftApp>) {
    h.step();
    for _ in 0..3000 {
        if !h.state().gpu_initializing() {
            return;
        }
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("the GPU compositor's worker gave no result within a minute");
}

/// Whether wgpu finds any adapter at all (egui_kittest's wgpu harness needs one; CI runners
/// without a GPU skip).
fn any_adapter() -> bool {
    let instance = eframe::wgpu::Instance::new(eframe::wgpu::InstanceDescriptor::new_without_display_handle());
    !pollster::block_on(instance.enumerate_adapters(eframe::wgpu::Backends::all())).is_empty()
}

fn small_session() -> Session {
    let mut s = Session::default();
    let comp = s.execute("comp.new", json!({"name": "First", "width": 64, "height": 64, "duration": 1})).unwrap()["comp"].as_u64().unwrap();
    s.execute("layer.newSolid", json!({"comp": comp, "color": "#ff0000"})).unwrap();
    s
}

/// The window opens before the GPU compositor exists: its pipelines compile on a worker thread
/// (tens of seconds on a cold shader cache), and until then the interface draws, says so, and
/// composites on the CPU. The worker is held on a gate so the "preparing" state is observable.
#[test]
fn the_interface_draws_and_composites_on_the_cpu_while_the_gpu_compositor_is_built() {
    let _device = crate::gpu_test::lock();
    if !any_adapter() {
        eprintln!("no wgpu adapter: skipping");
        return;
    }
    let (release, gate) = std::sync::mpsc::channel::<()>();
    let s = small_session();
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).wgpu().build_eframe(move |_| {
        let mut app = EffectcraftApp::new(s);
        // The status line's 5 s expiry reads the app clock: a manual one makes the steps below
        // stand for a known span of time.
        app.use_manual_clock();
        app.hooks.before_gpu_build = Some(Box::new(move || {
            let _ = gate.recv();
        }));
        app
    });
    // The first frames paint, and a CPU frame reaches the viewer, while the worker still waits.
    for _ in 0..600 {
        h.step();
        if h.state().is_ready() && !h.output().shapes.is_empty() && h.state().frames.last_ms.lock().map(|v| *v > 0.0).unwrap_or(false) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(h.state().gpu_initializing(), "the compositor is still being built");
    assert!(!h.output().shapes.is_empty(), "the interface draws before the compositor exists");
    assert!(h.state().session.accel.is_none(), "nothing renders on a compositor that doesn't exist yet");
    assert_eq!(h.state().ui.status, effectcraft_ui_egui::GPU_PREPARING, "the status line is honest about it");
    assert_eq!(h.state().session.accel_note.as_deref(), Some(effectcraft_ui_egui::GPU_PREPARING));
    assert!(h.state().frames.last_ms.lock().map(|v| *v > 0.0).unwrap_or(false), "a frame was composited on the CPU meanwhile");
    assert!(h.state_mut().viewer_pixels().is_some(), "the viewer shows that frame");
    // Other statuses clear after 5 s; this one stays for as long as the build takes (each step
    // is a quarter of a second).
    for _ in 0..30 {
        h.state_mut().clock_mut().advance(0.25);
        h.step();
    }
    assert!(h.state().gpu_initializing());
    assert_eq!(h.state().ui.status, effectcraft_ui_egui::GPU_PREPARING, "the status line still says so 7.5 s later");
    release.send(()).unwrap();
    settle(&mut h);
    assert!(!h.state().gpu_initializing());
    assert_ne!(h.state().ui.status, effectcraft_ui_egui::GPU_PREPARING, "the preparing status goes away");
    match h.state().gpu_adapter() {
        Some(adapter) => {
            eprintln!("compositor attached after the first frames: {adapter}");
            assert!(h.state().session.accel.is_some());
            assert!(h.state().session.accel_note.is_none());
        }
        // An adapter that can't run the compositor (software GL): the reason is recorded.
        None => assert!(h.state().session.accel_note.as_deref().is_some_and(|n| n != effectcraft_ui_egui::GPU_PREPARING)),
    }
}

/// A graphics backend that panics while the compositor's pipelines compile panics on the worker
/// thread, not under the UI: the window keeps drawing on the CPU and says why.
#[test]
fn a_compositor_build_that_panics_leaves_the_viewer_on_the_cpu() {
    let _device = crate::gpu_test::lock();
    if !any_adapter() {
        eprintln!("no wgpu adapter: skipping");
        return;
    }
    let s = small_session();
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).wgpu().build_eframe(move |_| {
        let mut app = EffectcraftApp::new(s);
        app.hooks.before_gpu_build = Some(Box::new(|| panic!("synthetic shader compiler panic")));
        app
    });
    settle(&mut h);
    assert!(!h.state().gpu_initializing());
    assert!(h.state().gpu_adapter().is_none());
    assert!(h.state().session.accel.is_none());
    let note = h.state().session.accel_note.clone().unwrap_or_default();
    assert!(note.contains("synthetic shader compiler panic"), "{note}");
    assert!(!h.output().shapes.is_empty(), "the interface still draws");
    assert!(h.state_mut().viewer_pixels().is_some(), "the viewer shows a CPU frame");
}

/// egui_kittest's wgpu setup on the adapter the headless compositor picked (`name (Backend)`),
/// like the desktop app's window. Left alone, egui_kittest prefers a CPU adapter, which on
/// Windows is WARP on DirectX 12, where FXC fails to compile the compositor's kernels: the
/// viewer would stay on the CPU and the comparison would never run.
fn same_adapter(name: String) -> eframe::egui_wgpu::WgpuSetup {
    let mut setup = egui_kittest::wgpu::default_wgpu_setup();
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(c) = &mut setup {
        let fallback = c.native_adapter_selector.take();
        c.native_adapter_selector = Some(std::sync::Arc::new(move |adapters, surface| {
            let named = |a: &&eframe::wgpu::Adapter| format!("{} ({:?})", a.get_info().name, a.get_info().backend) == name;
            match (adapters.iter().find(named), &fallback) {
                (Some(a), _) => Ok(a.clone()),
                (None, Some(f)) => f(adapters, surface),
                (None, None) => adapters.first().cloned().ok_or_else(|| "No adapter found".to_owned()),
            }
        }));
    }
    setup
}

#[test]
fn viewer_draws_gpu_frames_that_match_the_cpu() {
    let _device = crate::gpu_test::lock();
    let Some(probe) = effectcraft_gpu::Gpu::headless() else {
        eprintln!("no GPU adapter: skipping");
        return;
    };
    let adapter = effectcraft_engine::render::Accelerator::name(&probe);
    eprintln!("headless compositor adapter: {adapter}");
    // egui-wgpu creates its GL device with WebGL2's limits, which the compositor declines.
    let gl = adapter.ends_with("(Gl)");
    drop(probe);
    let mut h = Harness::builder()
        .with_size(egui::vec2(1400.0, 900.0))
        .with_pixels_per_point(1.0)
        .wgpu_setup(same_adapter(adapter))
        .build_eframe(|_| EffectcraftApp::new(Session::default()));
    // The project opens once the compositor (built on a worker thread) is attached. A frame the
    // CPU composited while it was being built has the GPU frame's key, so the viewer would keep
    // showing it, and the comparison below would be the CPU against itself.
    wait_for_compositor(&mut h);
    if gl && h.state().gpu_adapter().is_none() {
        eprintln!("egui-wgpu's GL device has WebGL2 limits: the viewer stays on the CPU, skipping");
        return;
    }
    assert!(h.state().gpu_adapter().is_some(), "GPU compositor on egui-wgpu's device");
    eprintln!("viewer compositor adapter: {:?}", h.state().gpu_adapter());
    h.state_mut().session.execute("file.openDemoProject", json!({})).unwrap();
    h.state_mut().session.execute("time.set", json!({"time": 2.0})).unwrap();
    settle(&mut h);
    let gpu_px = h.state_mut().viewer_pixels().expect("read back the GPU frame");
    let gpu_shot = h.render().expect("render");
    // Mercury Software Only: the same frame from the CPU.
    h.state_mut().session.execute("render.backend", json!({"backend": "cpu"})).unwrap();
    settle(&mut h);
    let cpu_px = h.state_mut().viewer_pixels().expect("CPU frame");
    let cpu_shot = h.render().expect("render");
    assert_eq!(gpu_px.size, cpu_px.size);
    let mut worst = 0i32;
    let mut over = 0usize;
    for (a, b) in gpu_px.pixels.iter().zip(&cpu_px.pixels) {
        let d = a.to_array().iter().zip(b.to_array()).map(|(x, y)| (*x as i32 - y as i32).abs()).max().unwrap_or(0);
        worst = worst.max(d);
        over += usize::from(d > 1);
    }
    assert!(over * 1000 <= gpu_px.pixels.len() * 3 && worst <= 4, "viewer GPU vs CPU: {over} pixels > 1/255, worst {worst}/255");
    if let Ok(d) = std::env::var("EC_SNAPSHOT_DIR") {
        gpu_shot.save(format!("{d}/ui_gpu_viewer.png")).unwrap();
        cpu_shot.save(format!("{d}/ui_cpu_viewer.png")).unwrap();
    }
}

/// Once the graphics device is lost, eframe's renderer cannot stage buffers and egui-wgpu
/// panicked on the main thread drawing the next frame (#591). The app now draws nothing,
/// auto-saves unsaved changes and closes its window instead.
#[test]
fn a_lost_device_draws_nothing_autosaves_and_closes() {
    let dir = std::env::temp_dir().join(format!("ec-device-lost-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut s = Session { autosave_folder_override: Some(dir.clone()), ..Default::default() };
    s.execute("comp.new", json!({"name": "Unsaved", "width": 64, "height": 64, "duration": 1})).unwrap();
    assert!(s.is_dirty());
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(|_| EffectcraftApp::new(s));
    let told = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let log = told.clone();
    h.state_mut().hooks.device_lost = Some(Box::new(move |saved| log.borrow_mut().push(saved)));
    h.run_steps(3);
    assert!(!h.output().shapes.is_empty(), "the UI draws before the loss");
    let bridge = effectcraft_ui_egui::gpu_failure::GpuFailureBridge::new(&h.ctx);
    h.state_mut().set_gpu_failure_bridge(bridge.clone());
    bridge.report("GPU device lost (Unknown): Device is lost", true);
    h.step();
    let closing = h.output().viewport_output.get(&egui::ViewportId::ROOT).is_some_and(|v| v.commands.contains(&egui::ViewportCommand::Close));
    assert!(closing, "the window closes");
    assert!(h.output().shapes.is_empty(), "nothing for the lost renderer to stage");
    h.step();
    assert!(h.output().shapes.is_empty());
    let saved = h.state().session.autosave.last_path.clone().expect("unsaved changes auto-saved");
    assert!(std::path::Path::new(&saved).is_file(), "{saved}");
    assert_eq!(*told.borrow(), vec![Some(Ok(saved))], "the user is told once, with the recovery copy");
    // The close arrives: on macOS the app quits through `-[NSApplication terminate:]` instead of
    // closing the window while AppKit's run loop is running (#341).
    h.input_mut().viewports.entry(egui::ViewportId::ROOT).or_default().events.push(egui::ViewportEvent::Close);
    h.step();
    let kept = h.output().viewport_output.get(&egui::ViewportId::ROOT).is_some_and(|v| v.commands.contains(&egui::ViewportCommand::CancelClose));
    assert_eq!(kept, cfg!(target_os = "macos"), "the platform quit closes the window on macOS");
    assert_eq!(told.borrow().len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}
