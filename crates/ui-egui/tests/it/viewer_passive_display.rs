//! A passive Composition viewer applies the same monitor conversion as the active one (#547).
use effectcraft_engine::Session;
use effectcraft_ui_egui::{EffectcraftApp, menus, panels::viewers};
use egui::{Color32, Context, ImageData, RawInput, Rect, pos2, vec2};
use serde_json::json;
use std::collections::HashMap;

fn frames(app: &mut EffectcraftApp, ctx: &Context, textures: &mut HashMap<String, Vec<Color32>>) {
    for _ in 0..4 {
        let mut output = ctx.run_ui(RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(256.0, 256.0))), ..Default::default() }, |ui| {
            viewers::show(app, ui, 0, Rect::from_min_size(pos2(0.0, 0.0), vec2(128.0, 256.0)));
            viewers::show(app, ui, 1, Rect::from_min_size(pos2(128.0, 0.0), vec2(128.0, 256.0)));
        });
        for (&id, deltas) in &output.textures_delta.set {
            let Some(name) = ctx.tex_manager().read().meta(id).map(|m| m.name.clone()) else { continue };
            for delta in deltas {
                let ImageData::Color(image) = &delta.image;
                if delta.pos.is_none() {
                    textures.insert(name.clone(), image.pixels.clone());
                }
            }
        }
        output.textures_delta.clear();
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

fn pixel(textures: &HashMap<String, Vec<Color32>>, name: &str) -> [u8; 4] {
    let p = &textures[name];
    p[p.len() / 2].to_array()
}

#[test]
fn passive_viewer_matches_the_monitor_conversion() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Display test", "width": 16, "height": 16, "duration": 1})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Color", "color": "#406080", "width": 16, "height": 16})).unwrap();
    s.execute("edit.deselectAll", json!({})).unwrap();
    let mut app = EffectcraftApp::new(s);
    let ctx = Context::default();
    menus::invoke(&mut app, &ctx, "view.newViewer", json!({})).unwrap();
    let mut textures = HashMap::new();
    app.session.execute("file.projectSettings", json!({"workingSpace": "rec2020"})).unwrap();
    app.session.execute("prefs.set", json!({"key": "previews.displayProfile", "value": "p3"})).unwrap();
    frames(&mut app, &ctx, &mut textures);
    assert_eq!(pixel(&textures, "viewer-display"), [37, 89, 122, 255]);
    assert_eq!(pixel(&textures, "viewer-0"), [37, 89, 122, 255]);
    // Changing which viewer is active keeps the conversion in both.
    viewers::activate(&mut app, &ctx, 0);
    frames(&mut app, &ctx, &mut textures);
    assert_eq!(pixel(&textures, "viewer-display"), [37, 89, 122, 255]);
    assert_eq!(pixel(&textures, "viewer-1"), [37, 89, 122, 255]);
    // No output-to-monitor conversion needed: both show the raw frame.
    app.session.execute("prefs.set", json!({"key": "previews.displayProfile", "value": "srgb"})).unwrap();
    frames(&mut app, &ctx, &mut textures);
    assert_eq!(pixel(&textures, "viewer-frame"), [0, 90, 125, 255]);
    assert_eq!(pixel(&textures, "viewer-1"), [0, 90, 125, 255]);
}
