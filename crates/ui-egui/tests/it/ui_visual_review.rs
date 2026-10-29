//! Opt-in visual review of every built-in panel, workspaces and common dialogs.
//! Original fixture; captures are review outputs, not shipped assets.
use effectcraft_engine::Session;
use effectcraft_ui_egui::{Dialog, EffectcraftApp, dock::PanelKind};
use egui_kittest::Harness;
use serde_json::json;

#[test]
#[ignore]
fn all_ui_review_captures() {
    let out = std::path::PathBuf::from(std::env::var("UI_REVIEW_DIR").expect("set UI_REVIEW_DIR"));
    std::fs::create_dir_all(&out).unwrap();
    for (label, size) in [("wide", egui::vec2(1600.0, 1000.0)), ("compact", egui::vec2(1024.0, 768.0))] {
        let mut s = Session::default();
        s.execute("comp.new", json!({"name":"UI Review","width":640,"height":360,"duration":4})).unwrap();
        s.execute("layer.newText", json!({"text":"EffectCraft","name":"Title"})).unwrap();
        s.execute("layer.newSolid", json!({"name":"Plate","color":"#406080","width":160,"height":120})).unwrap();
        s.execute("renderQueue.add", json!({"format":"png","output":"review-[#####].png"})).unwrap();
        std::sync::Arc::make_mut(&mut s.project).settings.gpu_acceleration = false;
        let mut h = Harness::builder().with_size(size).build_eframe(|_| EffectcraftApp::new(s));
        h.run_steps(4);
        h.render().unwrap().save(out.join(format!("{label}-workspace.png"))).unwrap();
        let ctx = h.ctx.clone();
        h.state_mut().set_theme(&ctx, effectcraft_ui_egui::theme::ThemeKind::Light);
        h.run_steps(4);
        h.render().unwrap().save(out.join(format!("{label}-workspace-light.png"))).unwrap();
        h.state_mut().set_theme(&ctx, effectcraft_ui_egui::theme::ThemeKind::Dark);
        for p in PanelKind::ALL {
            h.state_mut().show_panel(p);
            h.state_mut().ui.maximized = Some(p);
            h.run_steps(4);
            assert!(h.state().auto.find(&format!("panel.{}", p.id())).is_some(), "{} opens", p.title());
            h.render().unwrap().save(out.join(format!("{label}-{}.png", p.id()))).unwrap();
        }
        h.state_mut().ui.maximized = None;
        for (name, dialog) in [
            ("about", Dialog::About),
            ("new-comp", Dialog::NewComp),
            ("preferences", Dialog::Settings),
            ("shortcuts", Dialog::Shortcuts),
            ("palette", Dialog::CommandPalette),
        ] {
            h.state_mut().dialog = Some(dialog);
            h.run_steps(4);
            h.render().unwrap().save(out.join(format!("{label}-dialog-{name}.png"))).unwrap();
            h.state_mut().dialog = None;
            h.run_steps(2);
        }
    }
}
