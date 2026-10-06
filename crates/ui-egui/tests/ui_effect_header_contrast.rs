//! CPU paint checks for legible effect names in both supported themes.
use effectcraft_engine::Session;
use effectcraft_ui_egui::{
    EffectcraftApp,
    panels::effect_controls,
    theme::{ThemeKind, Tokens},
};
use egui::{Color32, Rect, pos2, vec2};
use serde_json::json;

fn luminance(color: Color32) -> f64 {
    let linear = |channel: u8| {
        let c = f64::from(channel) / 255.0;
        if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * linear(color.r()) + 0.7152 * linear(color.g()) + 0.0722 * linear(color.b())
}

#[test]
fn effect_names_remain_legible_when_selected_and_unselected_in_both_themes() {
    for kind in [ThemeKind::Dark, ThemeKind::Light] {
        for selected in [false, true] {
            let mut session = Session::default();
            session.execute("comp.new", json!({"width":2,"height":2,"duration":1})).unwrap();
            let lid = session.execute("layer.newSolid", json!({"width":2,"height":2})).unwrap()["layer"].as_u64().unwrap();
            let uid = session.execute("effect.apply", json!({"effect":"ec.blur.gaussian"})).unwrap()["effects"][0].as_u64().unwrap();
            let mut app = EffectcraftApp::new(session);
            app.tokens = Tokens::for_kind(kind);
            app.session.state.selected_props = if selected { vec![(effectcraft_engine::project::LayerId(lid), uid)] } else { vec![] };
            let ctx = egui::Context::default();
            effectcraft_ui_egui::theme::install(&ctx, &app.tokens);
            let rect = Rect::from_min_size(pos2(20.0, 20.0), vec2(400.0, 600.0));
            let output = ctx.run_ui(Default::default(), |ui| {
                effect_controls::show(&mut app, ui, rect);
            });
            let header = output
                .shapes
                .iter()
                .find_map(|s| match &s.shape {
                    egui::Shape::Rect(r) if r.rect.width() >= 390.0 && (r.rect.height() - 26.0).abs() < 0.1 => Some(r.fill),
                    _ => None,
                })
                .expect("effect header must be painted");
            let a = luminance(header);
            let b = luminance(app.tokens.text);
            let contrast = (a.max(b) + 0.05) / (a.min(b) + 0.05);
            assert!(contrast >= 4.5, "{kind:?}, selected={selected}: effect name contrast is {contrast:.2}");
            output.drop_without_applying_deltas();
        }
    }
}
