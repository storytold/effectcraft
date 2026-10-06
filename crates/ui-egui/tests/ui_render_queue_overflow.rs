//! CPU-only Render Queue layout checks. No compositor, exporter, or GPU is started.

use effectcraft_engine::Session;
use effectcraft_ui_egui::{EffectcraftApp, dock::PanelKind, panels::render_queue};
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use serde_json::json;

fn panel() -> Rect {
    Rect::from_min_size(Pos2::ZERO, vec2(900.0, 300.0))
}

fn fixture() -> (EffectcraftApp, egui::Context) {
    let mut session = Session::default();
    session.execute("comp.new", json!({"name":"Original queue fixture", "width":2, "height":2, "duration":1})).unwrap();
    for _ in 0..12 {
        session.execute("renderQueue.add", json!({})).unwrap();
    }
    let mut app = EffectcraftApp::new(session);
    app.ui.focused = PanelKind::RenderQueue;
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
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(panel()).id_salt("queue-fixture"));
        child.set_clip_rect(panel());
        render_queue::show(app, &mut child, panel());
    })
    .drop_without_applying_deltas();
}

fn row(app: &EffectcraftApp, number: usize) -> Rect {
    control(app, number, "row")
}

fn control(app: &EffectcraftApp, number: usize, name: &str) -> Rect {
    let item = app.auto.find(&format!("renderQueue.item.{number}.{name}")).unwrap();
    Rect::from_min_size(pos2(item.rect[0], item.rect[1]), vec2(item.rect[2], item.rect[3]))
}

fn scroll_to_end(app: &mut EffectcraftApp, ctx: &egui::Context) {
    draw(
        app,
        ctx,
        vec![
            Event::PointerMoved(pos2(450.0, 180.0)),
            Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: vec2(0.0, -1000.0), modifiers: Modifiers::NONE, phase: egui::TouchPhase::Move },
        ],
    );
    for _ in 0..12 {
        draw(app, ctx, vec![]);
    }
}

fn selected(ctx: &egui::Context) -> Option<u64> {
    ctx.data(|d| d.get_temp::<(Vec<u64>, Option<u64>)>(egui::Id::new("rq-ui"))).and_then(|(_, selected)| selected)
}

fn click(app: &mut EffectcraftApp, ctx: &egui::Context, pos: Pos2) {
    for pressed in [true, false] {
        draw(app, ctx, vec![Event::PointerMoved(pos), Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }]);
    }
    draw(app, ctx, vec![]);
}

#[test]
fn long_render_queue_scrolls_to_last_item_and_keeps_it_selectable() {
    let (mut app, ctx) = fixture();
    app.ui.focused = PanelKind::Timeline;
    // The panel keeps a 24-point status footer below its scrollable item list.
    let list = Rect::from_min_max(pos2(panel().min.x, row(&app, 1).min.y), pos2(panel().max.x, panel().max.y - 24.0));
    assert!(row(&app, 12).min.y >= list.max.y, "the fixture must overflow before scrolling");
    draw(
        &mut app,
        &ctx,
        vec![
            Event::PointerMoved(list.center()),
            Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: vec2(0.0, -1000.0), modifiers: Modifiers::NONE, phase: egui::TouchPhase::Move },
        ],
    );
    for _ in 0..12 {
        draw(&mut app, &ctx, vec![]);
    }
    let last = row(&app, 12);
    assert!(last.min.y >= list.min.y && last.max.y <= list.max.y, "last item remains outside the list after scrolling: {last:?} vs {list:?}");
    let last_id = app.session.project.render_queue[11].id;
    click(&mut app, &ctx, pos2(last.max.x - 40.0, last.center().y));
    assert_eq!(selected(&ctx), Some(last_id));
    assert_eq!(app.session.project.render_queue.len(), 12);
}

#[test]
fn expanded_last_queue_item_keeps_its_output_editor_reachable() {
    let (mut app, ctx) = fixture();
    scroll_to_end(&mut app, &ctx);
    let twirl = control(&app, 12, "twirl").center();
    click(&mut app, &ctx, twirl);
    scroll_to_end(&mut app, &ctx);
    let output = control(&app, 12, "outputPath");
    assert!(output.min.y >= 96.0 && output.max.y <= panel().max.y - 24.0, "expanded output editor must fit inside the scrolled list: {output:?}");
    let item_id = app.session.project.render_queue[11].id;
    click(&mut app, &ctx, output.center());
    let editing = ctx.data(|d| d.get_temp::<(u64, String)>(egui::Id::new("rq-edit-output")));
    assert_eq!(editing.map(|(id, _)| id), Some(item_id), "the visible output path must enter its inline editor");
    assert_eq!(app.session.project.render_queue.len(), 12);
}

#[test]
fn clipped_render_queue_rows_cannot_intercept_fixed_footer_clicks() {
    let (mut app, ctx) = fixture();
    let footer_click = pos2(panel().max.x - 40.0, panel().max.y - 8.0);
    assert!((1..=12).any(|n| row(&app, n).contains(footer_click)), "the regression needs an overflowing row behind the footer");
    assert_eq!(selected(&ctx), None);
    click(&mut app, &ctx, footer_click);
    assert_eq!(selected(&ctx), None, "a visually clipped row must not take a click on the fixed status footer");
    assert_eq!(app.session.project.render_queue.len(), 12);
}
