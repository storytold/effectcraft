//! Appearance is a saved UI preference: switching styles must not edit the composition,
//! move the controls, or leave the menu and settings disagreeing.
use effectcraft_engine::{Session, prefs::Prefs};
use effectcraft_ui_egui::{
    EffectcraftApp,
    dock::PanelKind,
    menus, native_menu,
    theme::{ThemeKind, Tokens},
};
use egui_kittest::Harness;
use serde_json::json;

fn app() -> EffectcraftApp {
    let mut s = Session::default();
    s.execute("comp.new", json!({"width":640,"height":360,"duration":4})).unwrap();
    s.execute("layer.newSolid", json!({"name":"Plate","color":"#406080"})).unwrap();
    s.execute("effect.apply", json!({"effect":"Slider Control"})).unwrap();
    let mut a = EffectcraftApp::new(s);
    a.show_panel(PanelKind::EffectControls);
    a
}

#[test]
fn appearance_menu_switches_save_without_changing_the_project_or_geometry() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_| app());
    h.run_steps(4);
    let original = h.state().session.project.clone();
    let panel = h.state().auto.find("panel.EffectControls").unwrap().rect;
    let ctx = h.ctx.clone();
    for kind in ThemeKind::ALL {
        let spec = native_menu::build(h.state());
        let item = spec.items().into_iter().find(|i| i.command == "prefs.set" && i.params["value"] == kind.id()).unwrap();
        menus::invoke(h.state_mut(), &ctx, &item.command, item.params.clone()).unwrap();
        h.run_steps(3);
        assert_eq!(h.state().ui.theme, kind);
        assert_eq!(h.state().tokens.kind, kind);
        assert_eq!(h.state().auto.find("panel.EffectControls").unwrap().rect, panel);
        assert!(std::sync::Arc::ptr_eq(&h.state().session.project, &original));
        let saved = Prefs::from_json(&h.state().session.prefs.to_json());
        assert_eq!(saved.appearance.theme, kind.id());
        let spec = native_menu::build(h.state());
        let checked: Vec<_> =
            spec.items().into_iter().filter(|i| i.command == "prefs.set" && i.params["key"] == "appearance.theme" && i.checked == Some(true)).collect();
        assert_eq!(checked.len(), 1);
        assert_eq!(checked[0].params["value"], kind.id());
    }
}

#[test]
fn all_appearance_ids_are_accepted_by_preferences_and_ui_commands() {
    let mut a = app();
    let ctx = egui::Context::default();
    let choices = effectcraft_engine::prefs::APPEARANCE_THEMES;
    assert_eq!(choices.len(), ThemeKind::ALL.len());
    for (_, id) in choices {
        a.session.execute("prefs.set", json!({"key":"appearance.theme","value":id})).unwrap();
        a.apply_prefs(&ctx);
        let expected = ThemeKind::from_name(id).unwrap();
        assert_eq!(a.tokens.kind, expected);
        menus::invoke(&mut a, &ctx, &format!("view.theme.{id}"), json!({})).unwrap();
        assert_eq!(a.tokens.kind, expected);
    }
    assert!(a.session.execute("prefs.set", json!({"key":"appearance.theme","value":"invalid"})).is_err());
    a.session.execute("prefs.reset", json!({"page":"appearance"})).unwrap();
    a.apply_prefs(&ctx);
    assert_eq!(a.ui.theme, ThemeKind::Dark);
    assert_eq!(Prefs::from_json(r#"{"theme":"darker"}"#).appearance.theme, "darker");
}

fn contrast(a: egui::Color32, b: egui::Color32) -> f32 {
    let luminance = |c: egui::Color32| {
        let f = |v: u8| {
            let x = f32::from(v) / 255.0;
            if x <= 0.04045 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
    };
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

#[test]
fn appearance_styles_keep_text_readable_and_preserve_semantic_colors() {
    let default = Tokens::for_kind(ThemeKind::Dark);
    for kind in ThemeKind::ALL {
        let t = Tokens::for_kind(kind);
        for bg in [t.panel_bg, t.field_bg, t.fx_header, t.fx_header_active] {
            assert!(contrast(t.text, bg) >= 4.5, "{kind:?}: {:?} on {bg:?}", t.text);
        }
        assert!(contrast(t.hot_text, t.panel_bg) >= 3.0, "{kind:?} values");
        assert_eq!((t.row_h, t.tab_h, t.gap), (default.row_h, default.tab_h, default.gap));
        assert_eq!((t.labels, t.cache_green, t.cache_blue), (default.labels, default.cache_green, default.cache_blue));
        let ctx = egui::Context::default();
        effectcraft_ui_egui::theme::apply_visuals(&ctx, &t);
        assert_eq!(ctx.global_style().visuals.dark_mode, !kind.is_light());
    }
    assert!(Tokens::for_kind(ThemeKind::Classic).bevel);
    assert_eq!(Tokens::for_kind(ThemeKind::Classic).radius, 0.0);
}

#[test]
fn appearance_submenu_exists_on_both_platform_menu_trees() {
    fn count(nodes: &[effectcraft_engine::menus::MenuNode]) -> usize {
        use effectcraft_engine::menus::MenuNode;
        nodes
            .iter()
            .map(|n| match n {
                MenuNode::Item(e) => usize::from(e.command == "prefs.set" && e.params["key"] == "appearance.theme"),
                MenuNode::Submenu { children, .. } => count(children),
                _ => 0,
            })
            .sum()
    }
    for mac in [false, true] {
        assert_eq!(count(&effectcraft_engine::menus::parse(effectcraft_engine::menus::TREE, mac).unwrap()), 6);
    }
}
