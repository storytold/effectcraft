//! CPU-only Project panel resize regressions; folders need no renderer or thumbnails.

use effectcraft_engine::Session;
use effectcraft_ui_egui::{EffectcraftApp, panels::project};
use egui::{Pos2, Rect, vec2};
use serde_json::json;

fn draw_at_sizes(sizes: &[[f32; 2]]) {
    let mut session = Session::default();
    for n in 0..4 {
        session.execute("project.newFolder", json!({"name":format!("Original folder {n}")})).unwrap();
    }
    session.state.project_selection.clear();
    let mut app = EffectcraftApp::new(session);
    let ctx = egui::Context::default();
    effectcraft_ui_egui::theme::install(&ctx, &app.tokens);
    for &[width, height] in sizes {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(width, height));
        app.auto.begin_frame();
        ctx.run_ui(egui::RawInput::default(), |ui| {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).id_salt("compact-project-fixture"));
            child.set_clip_rect(rect);
            project::show(&mut app, &mut child, rect);
        })
        .drop_without_applying_deltas();
        assert_eq!(app.session.project.children(None).len(), 4, "resizing must preserve the project at {width}×{height}");
        assert!(app.ui.project_scroll.is_finite() && app.ui.project_scroll >= 0.0);
    }
}

#[test]
fn short_project_panel_survives_scrollbar_minimum_height_boundary() {
    // The fixed header/footer consume 152 points, and the scrollbar track loses
    // another eight. A 170-point panel has a ten-point track: below its old 16-point minimum.
    draw_at_sizes(&[[400.0, 170.0], [400.0, 175.0], [400.0, 176.0], [400.0, 177.0], [400.0, 300.0]]);
}

#[test]
fn tiny_project_panel_survives_an_absent_list_viewport() {
    draw_at_sizes(&[[400.0, 152.0], [400.0, 153.0], [400.0, 160.0], [400.0, 1.0], [1.0, 1.0], [400.0, 300.0]]);
}
