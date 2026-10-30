//! The Environment light through the app's GPU compositor (egui_kittest with wgpu): adding one
//! visibly changes the viewer's GPU frames (#2436). Skips without a GPU adapter (CI).

use effectcraft_engine::Session;
use effectcraft_engine::project::Renderer;
use effectcraft_ui_egui::EffectcraftApp;
use egui_kittest::Harness;
use serde_json::json;

fn settle(h: &mut Harness<'_, EffectcraftApp>) {
    for _ in 0..600 {
        h.step();
        if h.state().frames.inflight() == 0 && h.state().frames.last_ms.lock().map(|v| *v > 0.0).unwrap_or(false) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    for _ in 0..4 {
        h.step();
    }
}

/// egui_kittest's wgpu setup on the adapter the headless compositor picked, so the viewer's GPU
/// compositor attaches (see ui_gpu.rs).
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

/// A red Environment light at full intensity lights the scene on the GPU compositor too: the
/// viewer's GPU frames change when the light is added.
#[test]
fn environment_light_lights_the_scene_on_the_gpu() {
    let Some(probe) = effectcraft_gpu::Gpu::headless() else {
        eprintln!("no GPU adapter: skipping");
        return;
    };
    let adapter = effectcraft_engine::render::Accelerator::name(&probe);
    eprintln!("headless compositor adapter: {adapter}");
    let gl = adapter.ends_with("(Gl)");
    drop(probe);
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("comp.open", json!({"comp": "3D Showcase"})).unwrap();
    s.execute("time.set", json!({"time": 5.0})).unwrap();
    s.execute("comp.settings", json!({"renderer": "classic3D"})).unwrap();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1400.0, 900.0))
        .with_pixels_per_point(1.0)
        .wgpu_setup(same_adapter(adapter))
        .build_eframe(|_| EffectcraftApp::new(s));
    settle(&mut h);
    if gl && h.state().gpu_adapter().is_none() {
        eprintln!("egui-wgpu's GL device has WebGL2 limits: the viewer stays on the CPU, skipping");
        return;
    }
    assert!(h.state().gpu_adapter().is_some(), "GPU compositor on egui-wgpu's device");
    let before = h.state_mut().viewer_pixels().expect("GPU frame before adding the light").pixels.clone();

    h.state_mut().session.execute("layer.newLight", json!({"kind": "Environment", "color": [1.0, 0.0, 0.0], "intensity": 100.0})).unwrap();
    assert_eq!(h.state().session.active_comp().unwrap().renderer, Renderer::Advanced3D);
    settle(&mut h);
    let after = h.state_mut().viewer_pixels().expect("GPU frame after adding the light").pixels.clone();

    assert_eq!(before.len(), after.len());
    let diff = before.iter().zip(&after).filter(|(a, b)| a != b).count();
    assert!(diff > 500, "a red Environment light at 100 % changed only {diff} GPU pixels");
}
