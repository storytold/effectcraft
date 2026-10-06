//! CPU-only human Render Queue template selection. No renderer or exporter starts.
use effectcraft_engine::{Session, project::render_queue::RenderQuality};
use effectcraft_ui_egui::{EffectcraftApp, panels::render_queue};
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use egui_kittest::{Harness, kittest::Queryable as _};
use serde_json::json;

fn harness(top: f32) -> Harness<'static, EffectcraftApp> {
    let mut session = Session::default();
    session.execute("comp.new", json!({"width": 64, "height": 64, "duration": 1})).unwrap();
    session.execute("renderQueue.add", json!({})).unwrap();
    let item = session.project.render_queue[0].id;
    let pane = Rect::from_min_size(pos2(0.0, top), vec2(900.0, 300.0));
    let app = EffectcraftApp::new(session);
    let mut h = Harness::builder().with_size(vec2(960.0, 720.0)).build_ui_state(
        move |ui, app: &mut EffectcraftApp| {
            let installed = egui::Id::new("render-template-fixture-fonts");
            if !ui.data(|d| d.get_temp::<bool>(installed).unwrap_or(false)) {
                effectcraft_ui_egui::theme::install(ui.ctx(), &app.tokens);
                ui.data_mut(|d| d.insert_temp(installed, true));
                return;
            }
            app.auto.begin_frame();
            ui.set_clip_rect(pane);
            render_queue::show(app, ui, pane);
        },
        app,
    );
    h.ctx.data_mut(|d| d.insert_temp(egui::Id::new("rq-ui"), (vec![item], Some(item))));
    h.run_steps(3);
    h
}

fn click(h: &mut Harness<'_, EffectcraftApp>, pos: Pos2) {
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerMoved(pos));
        h.input_mut().events.push(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    h.run_steps(3);
}

fn open_settings(h: &mut Harness<'_, EffectcraftApp>) {
    let e = h.state().auto.find("renderQueue.item.1.renderSettings").unwrap();
    let button = Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3]));
    click(h, button.center());
}

#[test]
fn draft_template_changes_settings_and_supports_undo_redo() {
    for top in [0.0, 400.0] {
        let mut h = harness(top);
        let initial = h.state().session.project.render_queue[0].settings.clone();
        assert_eq!(initial.quality, RenderQuality::Best);
        open_settings(&mut h);
        h.get_by_label("   Draft Settings").click();
        h.run_steps(3);
        let draft = h.state().session.project.render_queue[0].settings.clone();
        assert_eq!(draft.name, "Draft Settings", "pane top={top}, status={}", h.state().ui.status);
        assert_eq!(draft.quality, RenderQuality::Draft);
        assert_eq!(draft.resolution, 0.5);
        h.state_mut().session.execute("edit.undo", json!({})).unwrap();
        assert_eq!(h.state().session.project.render_queue[0].settings, initial);
        h.state_mut().session.execute("edit.redo", json!({})).unwrap();
        assert_eq!(h.state().session.project.render_queue[0].settings, draft);
    }
}

#[test]
fn outside_click_dismisses_settings_without_applying_a_template() {
    let mut h = harness(400.0);
    let initial = h.state().session.project.render_queue[0].settings.clone();
    let item = h.state().session.project.render_queue[0].id;
    let popup = egui::Id::new(("rq-dd", item, 0_i32)).with("open");
    open_settings(&mut h);
    assert!(h.ctx.data(|d| d.get_temp::<bool>(popup).unwrap_or(false)));
    click(&mut h, pos2(950.0, 719.0));
    assert!(!h.ctx.data(|d| d.get_temp::<bool>(popup).unwrap_or(false)));
    assert_eq!(h.state().session.project.render_queue[0].settings, initial);
}
