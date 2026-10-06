//! Character controls remain usable in short panels without invoking a renderer.
use effectcraft_engine::Session;
use effectcraft_ui_egui::{EffectcraftApp, dock::PanelKind, panels::text_panels};
use egui::{Event, Modifiers, PointerButton, Rect, pos2, vec2};
use serde_json::json;

fn panel() -> Rect {
    Rect::from_min_size(pos2(20.0, 20.0), vec2(400.0, 220.0))
}

fn fixture() -> (EffectcraftApp, egui::Context) {
    let mut session = Session::default();
    session.execute("comp.new", json!({"width":2,"height":2,"duration":1})).unwrap();
    session.execute("layer.newText", json!({"text":"Original type fixture"})).unwrap();
    let mut app = EffectcraftApp::new(session);
    app.ui.focused = PanelKind::Timeline;
    let ctx = egui::Context::default();
    effectcraft_ui_egui::theme::install(&ctx, &app.tokens);
    for _ in 0..3 {
        draw(&mut app, &ctx, vec![]);
    }
    (app, ctx)
}

fn draw(app: &mut EffectcraftApp, ctx: &egui::Context, events: Vec<Event>) {
    app.auto.begin_frame();
    ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(panel()).id_salt("character-fixture"));
        child.set_clip_rect(panel());
        text_panels::character(app, &mut child, panel());
    })
    .drop_without_applying_deltas();
}

fn ligatures(app: &EffectcraftApp) -> Rect {
    let e = app.auto.elements.iter().find(|e| e.id == "character.ligatures").unwrap();
    Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3]))
}

fn wheel(app: &mut EffectcraftApp, ctx: &egui::Context, pointer: egui::Pos2) {
    draw(
        app,
        ctx,
        vec![
            Event::PointerMoved(pointer),
            Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: vec2(0.0, -1000.0), modifiers: Modifiers::NONE, phase: egui::TouchPhase::Move },
        ],
    );
    for _ in 0..20 {
        draw(app, ctx, vec![]);
    }
}

#[test]
fn short_character_panel_scrolls_to_ligatures_with_click_and_undo() {
    let (mut app, ctx) = fixture();
    let original = text_panels::text_target(&app).unwrap().doc.ligatures;
    assert!(ligatures(&app).min.y >= panel().max.y, "fixture must start with hidden controls");
    wheel(&mut app, &ctx, panel().center());
    let r = ligatures(&app);
    assert!(panel().contains(r.min) && panel().contains(r.max), "Ligatures remains clipped after scrolling: {r:?}");
    let p = r.center();
    for pressed in [true, false] {
        draw(
            &mut app,
            &ctx,
            vec![Event::PointerMoved(p), Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }],
        );
    }
    assert_eq!(text_panels::text_target(&app).unwrap().doc.ligatures, !original);
    app.session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(text_panels::text_target(&app).unwrap().doc.ligatures, original);
    assert_eq!(text_panels::text_target(&app).unwrap().doc.text, "Original type fixture");
}

#[test]
fn wheel_outside_character_panel_leaves_controls_and_text_unchanged() {
    let (mut app, ctx) = fixture();
    let before = ligatures(&app);
    let original = text_panels::text_target(&app).unwrap().doc;
    wheel(&mut app, &ctx, pos2(panel().max.x + 50.0, panel().center().y));
    assert_eq!(ligatures(&app), before);
    assert_eq!(text_panels::text_target(&app).unwrap().doc, original);
    assert_eq!(app.ui.focused, PanelKind::Timeline);
}

#[test]
fn scrolling_preserves_selected_character_styling_and_undo() {
    use effectcraft_engine::{commands::text_edit::layer_doc, project::LayerId};
    let (mut app, ctx) = fixture();
    app.session.execute("text.edit", json!({"select":[0, 8]})).unwrap();
    let lid = app.session.state.text_edit.as_ref().unwrap().layer;
    wheel(&mut app, &ctx, panel().center());
    let r = ligatures(&app);
    assert!(r.max.y <= panel().max.y - 20.0, "controls must leave room for selection status");
    let original = layer_doc(&app.session, LayerId(lid.0)).unwrap();
    let p = r.center();
    for pressed in [true, false] {
        draw(
            &mut app,
            &ctx,
            vec![Event::PointerMoved(p), Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }],
        );
    }
    let changed = layer_doc(&app.session, lid).unwrap();
    assert_eq!(changed.style_at(2).ligatures, !original.style_at(2).ligatures);
    assert_eq!(changed.style_at(12).ligatures, original.style_at(12).ligatures, "unselected characters must retain their style");
    app.session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(layer_doc(&app.session, lid).unwrap(), original);
}
