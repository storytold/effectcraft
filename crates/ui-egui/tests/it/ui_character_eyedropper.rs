//! Character Fill and Stroke can sample a displayed Composition pixel.
use crate::gpu_test::RenderSerial;
use effectcraft_engine::{Session, commands::text_edit::layer_doc, project::LayerId};
use effectcraft_ui_egui::{
    EffectcraftApp,
    dock::{self, PanelKind, SplitSize},
    panels::viewer::comp_to_screen,
};
use egui::{Event, Modifiers, PointerButton, Pos2, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

fn click(h: &mut Harness<'_, EffectcraftApp>, pos: Pos2) {
    h.event(Event::PointerMoved(pos));
    h.step();
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    h.run_steps(2);
}

fn click_auto(h: &mut Harness<'_, EffectcraftApp>, id: &str) {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("missing {id}")).clone();
    click(h, pos2(e.rect[0] + e.rect[2] * 0.5, e.rect[1] + e.rect[3] * 0.5));
}

fn settle(h: &mut Harness<'_, EffectcraftApp>) {
    for _ in 0..600 {
        h.step();
        if h.state().frames.inflight() == 0 && h.state().frames.last_ms.lock().is_ok_and(|v| *v > 0.0) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    h.run_steps(4);
}

#[test]
fn character_fill_eyedropper_samples_viewer_and_keeps_text_selection() {
    let _device = crate::gpu_test::lock();
    let mut s = Session::default();
    s.execute("comp.new", json!({"width": 320, "height": 180, "duration": 2})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Background", "color": "#20c040"})).unwrap();
    let text = s.execute("layer.newText", json!({"text": "First second", "font": "Inter"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("text.edit", json!({"layer": text})).unwrap();
    s.execute("text.setSelection", json!({"start": 0, "end": 5})).unwrap();
    let mut app = EffectcraftApp::new(s);
    app.set_workspace("Text");
    let builder = Harness::builder().with_size(vec2(1600.0, 1400.0));
    let mut h = if std::env::var_os("EC_SNAPSHOT_DIR").is_some() { builder.wgpu().build_eframe(|_| app) } else { builder.build_eframe(|_| app) };
    h.state_mut().show_panel(PanelKind::Character);
    settle(&mut h);
    click_auto(&mut h, "character.fill.eyedropper");
    assert_eq!(h.state().ui.text_color_pick.as_ref().unwrap().range, Some([0, 5]));
    let sample = comp_to_screen(&h.ctx, [20.0, 20.0]).unwrap();
    click(&mut h, sample);
    assert!(h.state().ui.text_color_pick.is_none());
    let doc = layer_doc(&h.state().session, LayerId(text)).unwrap();
    let want = [0x20 as f32 / 255.0, 0xc0 as f32 / 255.0, 0x40 as f32 / 255.0];
    for (actual, expected) in doc.style_at(0).fill.iter().zip(want) {
        assert!((actual - expected).abs() < 0.02);
    }
    assert_ne!(doc.style_at(8).fill, doc.style_at(0).fill, "unselected text keeps its colour");
}

#[test]
fn character_stroke_eyedropper_samples_viewer() {
    let _device = crate::gpu_test::lock();
    let mut s = Session::default();
    s.execute("comp.new", json!({"width": 320, "height": 180, "duration": 2})).unwrap();
    s.execute("layer.newSolid", json!({"name": "brat green", "color": "#8ACE00"})).unwrap();
    let text = s.execute("layer.newText", json!({"text": "brat", "font": "Inter"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setText", json!({"layer": text, "fill": [0.0, 0.0, 0.0], "size": 32.0})).unwrap();
    let mut app = EffectcraftApp::new(s);
    app.ui.viewer.show_layer_controls = false;
    app.ui.dock = dock::docked(dock::hsplit(SplitSize::FixedSecond(360.0), dock::tabs(&[PanelKind::Composition], 0), dock::tabs(&[PanelKind::Character], 0)));
    let builder = Harness::builder().with_size(vec2(1200.0, 760.0));
    let mut h = if std::env::var_os("EC_SNAPSHOT_DIR").is_some() { builder.wgpu().build_eframe(|_| app) } else { builder.build_eframe(|_| app) };
    h.state_mut().show_panel(PanelKind::Character);
    h.state_mut().show_panel(PanelKind::Composition);
    settle(&mut h);
    click_auto(&mut h, "character.stroke.eyedropper");
    if let Ok(dir) = std::env::var("EC_SNAPSHOT_DIR") {
        h.render_serialized().unwrap().save(format!("{dir}/character-eyedropper.png")).unwrap();
    }
    let sample = comp_to_screen(&h.ctx, [20.0, 20.0]).unwrap();
    click(&mut h, sample);
    let doc = layer_doc(&h.state().session, LayerId(text)).unwrap();
    let want = [0x8a as f32 / 255.0, 0xce as f32 / 255.0, 0.0];
    for (actual, expected) in doc.stroke.iter().zip(want) {
        assert!((actual - expected).abs() < 0.02);
    }
}
