//! A form keeps its fields and its buttons inside the dialog, whatever a choice's text is (#978).
use effectcraft_engine::Session;
use effectcraft_ui_egui::{Dialog, EffectcraftApp, menus};
use egui::{Rect, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

fn rect(h: &Harness<'_, EffectcraftApp>, id: &str) -> Rect {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("missing {id}"));
    Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3]))
}

#[test]
fn project_settings_fits_the_form_width() {
    let mut session = Session::default();
    session.prefs.disk.disk_cache_enabled = false;
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(session));
    h.run_steps(4);
    let ctx = h.ctx.clone();
    menus::invoke(h.state_mut(), &ctx, "file.projectSettings", json!({})).unwrap();
    h.run_steps(4);
    assert_eq!(h.state().dialog, Some(Dialog::Form));
    // The renderer's choice names the GPU (or its absence): longer than the 240 px column.
    let (ok, cancel, depth, renderer) = (rect(&h, "form.ok"), rect(&h, "form.cancel"), rect(&h, "form.field.bitDepth"), rect(&h, "form.field.renderer"));
    assert!((renderer.width() - depth.width()).abs() < 1.0, "a long choice does not widen its popup: {renderer:?} vs {depth:?}");
    // The dialog is 480 px wide and centred; nothing reaches past its 18 px margin.
    let dialog = Rect::from_center_size(ctx.content_rect().center(), vec2(480.0 - 36.0, f32::INFINITY));
    for (id, r) in [("form.ok", ok), ("form.cancel", cancel), ("form.field.renderer", renderer)] {
        assert!(r.left() >= dialog.left() - 1.0 && r.right() <= dialog.right() + 1.0, "{id} inside the dialog: {r:?} in {dialog:?}");
    }
    assert!(ok.top() > renderer.bottom(), "the buttons sit under the fields");
}
