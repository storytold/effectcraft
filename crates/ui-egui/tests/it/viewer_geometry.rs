//! Inspect the actual viewer's painted colors and pointer-driven control geometry.

use crate::gpu_test::RenderSerial;
use effectcraft_engine::Session;
use effectcraft_ui_egui::{
    EffectcraftApp,
    panels::viewer,
    theme::{self, ThemeKind},
};
use egui::{Color32, Event, Rect, Shape, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

fn harness(width: f32, kind: ThemeKind) -> Harness<'static, EffectcraftApp> {
    harness_at_scale(width, kind, 1.0)
}

fn harness_at_scale(width: f32, kind: ThemeKind, scale: f32) -> Harness<'static, EffectcraftApp> {
    let mut session = Session::default();
    session.execute("comp.new", json!({"name":"Geometry validation", "width":640, "height":360, "duration":4})).unwrap();
    let mut h = Harness::builder().with_size(vec2(width, 360.0)).with_pixels_per_point(scale).build_ui_state(
        |ui, app: &mut EffectcraftApp| {
            app.auto.begin_frame();
            *app.frames.last_ms.lock().unwrap() = 0.0;
            viewer::show(app, ui, ui.max_rect());
        },
        EffectcraftApp::new(session),
    );
    let ctx = h.ctx.clone();
    h.state_mut().set_theme(&ctx, kind);
    theme::install(&ctx, &h.state().tokens, "en");
    h.run_steps(4);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while h.state().frames.urgent_pending() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(!h.state().frames.urgent_pending(), "synthetic viewer frame must finish rendering");
    h.run_steps(3);
    h
}

fn rect(h: &Harness<'_, EffectcraftApp>, id: &str) -> Rect {
    let e = h.state().auto.elements.iter().find(|e| e.id == id).unwrap_or_else(|| panic!("missing {id}"));
    Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3]))
}

fn luminance(color: Color32) -> f32 {
    let linear = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * linear(color.r()) + 0.7152 * linear(color.g()) + 0.0722 * linear(color.b())
}

#[test]
fn active_breadcrumb_text_contrasts_with_its_actual_painted_background() {
    let mut failures = Vec::new();
    for kind in [ThemeKind::Light, ThemeKind::Dark, ThemeKind::Darker] {
        for scale in [1.0, 2.0] {
            let h = harness_at_scale(300.0, kind, scale);
            let text = h
                .output()
                .shapes
                .iter()
                .find_map(|s| match &s.shape {
                    Shape::Text(text) if text.galley.text() == "Geometry validation" => Some(text),
                    _ => None,
                })
                .unwrap();
            let fill = h
                .output()
                .shapes
                .iter()
                .rev()
                .find_map(|s| match &s.shape {
                    Shape::Rect(r) if r.rect.height() == 18.0 && r.rect.contains(text.visual_bounding_rect().center()) => Some(r.fill),
                    _ => None,
                })
                .unwrap();
            let color = text.override_text_color.unwrap_or(text.fallback_color);
            let foreground = luminance(color);
            let background = luminance(fill);
            let contrast = (foreground.max(background) + 0.05) / (foreground.min(background) + 0.05);
            eprintln!("viewer breadcrumb: {kind:?} {scale}x, text {color:?}, background {fill:?}, contrast {contrast:.2}");
            if contrast < 4.5 {
                failures.push(format!("{kind:?} {scale}x: actual breadcrumb contrast is {contrast:.2}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn compact_control_row_scrolls_and_preserves_interactions() {
    let mut h = harness(300.0, ThemeKind::Light);
    assert!(rect(&h, "viewer.timecode").right() > 300.0);
    h.event(Event::PointerMoved(pos2(150.0, 345.0)));
    h.event(Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(-1500.0, 0.0),
        modifiers: Default::default(),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Wheel,
    });
    h.run_steps(30);
    let timecode = rect(&h, "viewer.timecode");
    let row = rect(&h, "viewer.controls");
    assert!(timecode.left() >= row.left() && timecode.right() <= row.right(), "last viewer control must scroll into view: {timecode:?}");
    let exposure = rect(&h, "viewer.resetExposure");
    assert!(exposure.left() >= row.left() && exposure.right() <= row.right());
    h.state_mut().session.execute("view.exposure", json!({"stops":2.0})).unwrap();
    h.run_steps(2);
    let point = rect(&h, "viewer.resetExposure").center();
    h.event(Event::PointerMoved(point));
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos: point, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() });
        h.step();
    }
    h.run_steps(3);
    assert_eq!(h.state().session.state.viewer.exposure, 0.0, "a scrolled control must still run its original command");

    let exposure = rect(&h, "viewer.exposure").center();
    h.event(Event::PointerMoved(exposure));
    h.event(Event::PointerButton { pos: exposure, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.step();
    for x in [10.0, 20.0, 30.0] {
        h.event(Event::PointerMoved(exposure + vec2(x, 0.0)));
        h.step();
    }
    h.event(Event::PointerButton { pos: exposure + vec2(30.0, 0.0), button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(3);
    assert!(h.state().session.state.viewer.exposure > 0.0, "the scrolled exposure control must retain dragging");

    click_control(&mut h, "viewer.snapshot");
    assert!(h.state().session.snapshot.is_some(), "the scrolled snapshot button must capture");
    let snapshot = rect(&h, "viewer.showSnapshot").center();
    h.event(Event::PointerMoved(snapshot));
    h.event(Event::PointerButton { pos: snapshot, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.run_steps(3);
    assert!(h.state().session.state.viewer.show_snapshot, "the scrolled snapshot control must retain holding");
    h.event(Event::PointerButton { pos: snapshot, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(3);
    assert!(!h.state().session.state.viewer.show_snapshot, "releasing the snapshot control must return to the current frame");

    click_control(&mut h, "viewer.fastPreviews");
    click_control(&mut h, "viewer.fastPreviewsItem.1");
    assert_eq!(h.state().session.state.viewer.fast_previews.id(), "adaptive", "a scrolled dropdown must retain its menu command");
}

fn click_control(h: &mut Harness<'_, EffectcraftApp>, id: &str) {
    let control = rect(h, id);
    let point = control.center();
    let row = rect(h, "viewer.controls");
    if !id.contains("Item.") {
        assert!(control.left() >= row.left() && control.right() <= row.right(), "{id} must be fully visible before clicking");
    }
    h.event(Event::PointerMoved(point));
    h.step();
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos: point, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() });
        h.step();
    }
    h.run_steps(3);
}

#[test]
fn tab_traversal_reveals_the_last_control() {
    let mut h = harness(300.0, ThemeKind::Dark);
    let last = egui::Id::unique("vw-timecode");
    for _ in 0..80 {
        h.key_press(egui::Key::Tab);
        h.run_steps(3);
        if h.ctx.memory(|memory| memory.focused()) == Some(last) {
            break;
        }
    }
    assert_eq!(h.ctx.memory(|memory| memory.focused()), Some(last), "Tab must reach the actual timecode control");
    h.run_steps(12);
    let response = h.ctx.read_response(last).unwrap();
    let row = rect(&h, "viewer.controls");
    assert!(response.rect.left() >= row.left() && response.rect.right() <= row.right(), "focused control must be visible: {:?}", response.rect);
    assert!(
        h.output().shapes.iter().any(|shape| matches!(&shape.shape, Shape::Rect(rect)
        if rect.rect == response.rect.shrink(1.0) && rect.stroke.color == h.state().tokens.focus)),
        "focused control must have a visible outline"
    );
}

#[test]
fn compact_scrollbar_thumb_is_distinct_and_tracks_the_scrolled_position() {
    for kind in [ThemeKind::Dark, ThemeKind::Darker, ThemeKind::Light] {
        for scale in [1.0, 2.0] {
            let mut h = harness_at_scale(300.0, kind, scale);
            let (initial_track, initial_thumb) = painted_scrollbar(&h, kind, scale, "initial");
            scroll_controls_to_end(&mut h);
            let (scrolled_track, scrolled_thumb) = painted_scrollbar(&h, kind, scale, "scrolled");
            assert_eq!(initial_track, scrolled_track, "scrolling must keep the track fixed");
            assert!(scrolled_thumb.left() > initial_thumb.left() + 1.0, "the painted thumb must move with horizontal scrolling");
        }
    }
}

fn painted_scrollbar(h: &Harness<'_, EffectcraftApp>, kind: ThemeKind, scale: f32, state: &str) -> (Rect, Rect) {
    let scrollbar = h.get_by_role(egui::accesskit::Role::ScrollBar).rect();
    let row = rect(h, "viewer.controls");
    assert!(row.contains_rect(scrollbar), "scrollbar {scrollbar:?} must stay inside {row:?}");
    let mut painted: Vec<_> = h
        .output()
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            Shape::Rect(rect)
                if rect.rect.width() > 20.0
                    && rect.rect.height() <= 8.0
                    && scrollbar.contains_rect(rect.rect)
                    && rect.fill.a() > 0
                    && shape.clip_rect.contains_rect(rect.rect) =>
            {
                Some(rect)
            }
            _ => None,
        })
        .collect();
    painted.sort_by(|a, b| a.rect.width().total_cmp(&b.rect.width()));
    assert_eq!(painted.len(), 2, "the track and thumb must each be painted without clipping");
    let (thumb, track) = (painted[0], painted[1]);
    assert!(thumb.rect.width() < track.rect.width() - 1.0 && track.rect.contains_rect(thumb.rect), "the overflow thumb must be narrower than its track");
    let foreground = luminance(thumb.fill);
    let background = luminance(track.fill);
    let contrast = (foreground.max(background) + 0.05) / (foreground.min(background) + 0.05);
    eprintln!(
        "viewer scrollbar: {kind:?} {scale}x {state}, thumb {:?} {:?}, track {:?} {:?}, contrast {contrast:.2}",
        thumb.rect, thumb.fill, track.rect, track.fill
    );
    assert!(contrast >= 3.0, "{kind:?} {scale}x {state}: actual thumb/track contrast {contrast:.2} is too low");
    (track.rect, thumb.rect)
}

fn scroll_controls_to_end(h: &mut Harness<'_, EffectcraftApp>) {
    h.event(Event::PointerMoved(pos2(150.0, 345.0)));
    h.event(Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(-1500.0, 0.0),
        modifiers: Default::default(),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Wheel,
    });
    h.run_steps(30);
    h.event(Event::PointerMoved(pos2(150.0, 180.0)));
    h.run_steps(3);
}

#[test]
fn viewer_theme_and_width_visual_matrix() {
    let Ok(directory) = std::env::var("CRAFT_UI_VISUAL_DIR") else { return };
    std::fs::create_dir_all(&directory).unwrap();
    for kind in [ThemeKind::Dark, ThemeKind::Darker, ThemeKind::Light] {
        for width in [900.0, 300.0] {
            for scale in [1.0, 2.0] {
                let mut h = harness_at_scale(width, kind, scale);
                save_stable_snapshot(&mut h, &format!("{directory}/effect-viewer-{kind:?}-{width}-{scale}x.png"), width);
                if width == 300.0 {
                    scroll_controls_to_end(&mut h);
                    save_stable_snapshot(&mut h, &format!("{directory}/effect-viewer-{kind:?}-{width}-{scale}x-scrolled.png"), width);
                }
            }
        }
    }
}

fn save_stable_snapshot(h: &mut Harness<'_, EffectcraftApp>, path: &str, width: f32) {
    let image = h.render_serialized().unwrap();
    image.save(path).unwrap();
    h.run_steps(3);
    let repeated = h.render_serialized().unwrap();
    assert!(image == repeated, "settled synthetic viewer pixels must repeat exactly: {path}");
    if width == 900.0 {
        let times: Vec<_> = h
            .output()
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                Shape::Text(text) if text.galley.text().ends_with(" ms") => Some(text.galley.text()),
                _ => None,
            })
            .collect();
        assert_eq!(times, ["0 ms"], "cached rendering must preserve the deterministic timing input");
    }
}
