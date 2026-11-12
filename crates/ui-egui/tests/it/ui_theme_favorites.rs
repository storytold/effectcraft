use crate::gpu_test::RenderSerial;
use effectcraft_engine::{Session, config::MemoryConfig, prefs::Prefs};
use effectcraft_ui_egui::{EffectcraftApp, menus};
use egui::{Event, Modifiers, PointerButton, Pos2, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;
use std::sync::Arc;

fn at(h: &Harness<'_, EffectcraftApp>, id: &str) -> Pos2 {
    let r = h.state().auto.find(id).unwrap_or_else(|| panic!("missing {id}")).rect;
    pos2(r[0] + r[2] / 2.0, r[1] + r[3] / 2.0)
}
fn click(h: &mut Harness<'_, EffectcraftApp>, p: Pos2, button: PointerButton) {
    h.event(Event::PointerMoved(p));
    h.step();
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos: p, button, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    h.run_steps(3);
}
#[test]
fn favorites_validate_and_survive_settings_reload() {
    let loaded = Prefs::from_json(r#"{"appearance":{"favoriteThemes":["dark","unknown","dark","light"]}}"#);
    assert_eq!(loaded.appearance.favorite_themes, vec!["dark", "light"]);
    let mut p = Prefs::default();
    for bad in [json!(["dark", "dark"]), json!(["unknown"]), json!([3]), json!("dark")] {
        assert!(p.set("appearance.favoriteThemes", bad).is_err());
        assert!(p.appearance.favorite_themes.is_empty());
    }
    p.set("appearance.favoriteThemes", json!(["light", "darker"])).unwrap();
    let loaded = Prefs::from_json(&p.to_json());
    assert_eq!(loaded.appearance.favorite_themes, p.appearance.favorite_themes);
}
#[test]
fn star_controls_save_and_favorite_menu_selects_the_family() {
    let store = Arc::new(MemoryConfig::default());
    let mut session = Session { config: Some(store.clone()), ..Default::default() };
    session.execute("comp.new", json!({"width":32,"height":32,"duration":2})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(session));
    h.run_steps(4);
    let ctx = h.ctx.clone();
    menus::invoke(h.state_mut(), &ctx, "app.settings", json!({"page":"appearance"})).unwrap();
    h.run_steps(3);
    let p = at(&h, "settings.appearance.lightTheme.light");
    h.event(Event::PointerMoved(p));
    h.run_steps(2);
    let p = at(&h, "settings.appearance.favoriteThemes.light");
    click(&mut h, p, PointerButton::Primary);
    assert_eq!(h.state().session.prefs.appearance.favorite_themes, vec!["light"]);
    if let Some(path) = std::env::var_os("FAVORITES_SCREENSHOT_DIR") {
        std::fs::create_dir_all(&path).unwrap();
        h.render_serialized().unwrap().save(std::path::Path::new(&path).join("effectcraft-favorites.png")).unwrap();
    }
    assert_eq!(h.state().session.prefs.appearance.appearance_mode, "dark");
    let p = at(&h, "settings.ok");
    click(&mut h, p, PointerButton::Primary);
    let mut saved = Session { config: Some(store), ..Default::default() };
    saved.load_settings();
    assert_eq!(saved.prefs.appearance.favorite_themes, vec!["light"]);
    h.state_mut().set_pref("appearance.favoriteThemes", json!(["darker", "light"])).unwrap();
    let p = at(&h, "header.appearance");
    click(&mut h, p, PointerButton::Secondary);
    let p = at(&h, "header.favoriteTheme.light");
    assert!(p.y < at(&h, "header.favoriteTheme.darker").y);
    click(&mut h, p, PointerButton::Primary);
    assert_eq!(h.state().session.prefs.appearance.appearance_mode, "light");
    assert_eq!(h.state().session.prefs.appearance.dark_theme, "dark");
}
