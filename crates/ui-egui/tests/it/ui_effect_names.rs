//! Effect names in a Simplified Chinese interface (#821): the generated Effect menu and Effect
//! Controls draw the catalog's name, while the effect ids, the property tree's group name, the
//! automation labels and `effect.list` stay English.

use effectcraft_engine::Session;
use effectcraft_engine::project::LayerId;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use effectcraft_ui_egui::native_menu::{NativeNode, build as native_menu};
use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

/// The effect under test, and the names the registry and the catalog give it.
const BLUR: &str = "ec.blur.gaussian";
const BLUR_EN: &str = "Gaussian Blur";
const BLUR_ZH: &str = "高斯模糊";

fn harness(language: &str) -> (Harness<'static, EffectcraftApp>, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 640, "height": 360, "frameRate": 30, "duration": 10})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Back", "color": "#20c040"})).unwrap();
    let layer = s.execute("layer.newSolid", json!({"name": "Blurred", "color": "#ff0000", "width": 200, "height": 100})).unwrap()["layer"].as_u64().unwrap();
    let fx = s.execute("effect.apply", json!({"layers": [layer], "effect": BLUR})).unwrap()["effects"][0].as_u64().unwrap();
    // A second instance: the engine stores it as "Gaussian Blur 2", which the catalog must keep.
    s.execute("effect.apply", json!({"layers": [layer], "effect": BLUR})).unwrap();
    s.state.selected_layers = vec![LayerId(layer)];
    s.state.selected_props.clear();
    s.prefs.general.language = language.to_string();
    let mut app = EffectcraftApp::new(s);
    app.show_panel(PanelKind::EffectControls);
    app.show_panel(PanelKind::EffectsPresets);
    app.show_panel(PanelKind::Timeline);
    // The Chinese/Japanese names under test have no installed face on every host: draw tofu
    // instead of failing the missing-glyph gate (the glyph coverage test covers the bundled set).
    let mut h = Harness::builder().with_size(vec2(1400.0, 900.0)).allow_missing_glyphs().build_eframe(|_| app);
    h.run_steps(4);
    (h, fx)
}

/// How many widgets draw a label containing `needle` (the Effect Controls header registers its
/// drawn name with the accessibility tree, so a translated name shows up here).
fn drawn(h: &Harness<'_, EffectcraftApp>, needle: &str) -> usize {
    h.query_all_by_label_contains(needle).count()
}

/// The label the native menu gives the `effect.apply` entry for effect `id`, under top-level menu
/// `top` and category `category`. The in-window menu bar builds its labels through the same
/// `entry_label`, so this covers both menu bars.
fn menu_label(h: &Harness<'_, EffectcraftApp>, top: &str, category: &str, id: &str) -> Option<String> {
    let menu = native_menu(h.state());
    let effect = menu.menus.iter().find_map(|n| match n {
        NativeNode::Submenu { label, children } if label == top => Some(children),
        _ => None,
    })?;
    effect.iter().find_map(|cat| match cat {
        NativeNode::Submenu { label, children } if label == category => children.iter().find_map(|item| match item {
            NativeNode::Item(i) if i.command == "effect.apply" && i.params.get("effect").and_then(|v| v.as_str()) == Some(id) => Some(i.label.clone()),
            _ => None,
        }),
        _ => None,
    })
}

/// Effect Controls draws the catalog's name, including the instance index the engine appends to
/// a repeat; the English interface draws the registry's name.
#[test]
fn the_effect_controls_header_follows_the_interface_language() {
    let (h, _fx) = harness("zh-hans");
    assert!(drawn(&h, BLUR_ZH) > 0, "the header draws the Chinese name");
    assert_eq!(drawn(&h, "高斯模糊 2"), 1, "the second instance keeps its index");
    assert_eq!(drawn(&h, BLUR_EN), 0, "nothing draws the English name");

    let (en, _fx) = harness("en");
    assert!(drawn(&en, BLUR_EN) > 0);
    assert_eq!(drawn(&en, BLUR_ZH), 0);
}

/// The generated Effect menu — its category submenus and its entries — follows the catalog.
#[test]
fn the_generated_effect_menu_is_translated() {
    let (h, _fx) = harness("zh-hans");
    assert_eq!(menu_label(&h, "效果", "模糊和锐化", BLUR).as_deref(), Some(BLUR_ZH), "the generated entry is translated");

    let (en, _fx) = harness("en");
    assert_eq!(menu_label(&en, "Effect", "Blur & Sharpen", BLUR).as_deref(), Some(BLUR_EN), "the English menu is unchanged");
}

/// Right-click the Effect Controls header of effect `fx`.
fn open_effect_menu(h: &mut Harness<'_, EffectcraftApp>, fx: u64) {
    let e = h.state().auto.find(&format!("effectControls.effect.{fx}")).expect("the effect's header").rect;
    let pos = egui::pos2(e[0] + e[2] / 2.0, e[1] + e[3] / 2.0);
    h.event(egui::Event::PointerMoved(pos));
    h.step();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Secondary, pressed, modifiers: Default::default() });
        h.step();
    }
    h.run_steps(2);
}

/// The effect header's context menu draws its items in the interface language, like the layer
/// menu (#1308): it used to draw Copy, Paste, Duplicate, Reset, Remove, Move Up and Move Down in
/// English whatever the language.
#[test]
fn the_effect_context_menu_follows_the_interface_language() {
    const ENGLISH: [&str; 7] = ["Copy", "Paste", "Duplicate", "Reset", "Remove", "Move Up", "Move Down"];
    let (mut h, fx) = harness("zh-hans");
    open_effect_menu(&mut h, fx);
    for label in ["复制", "粘贴", "重复", "重命名", "重置", "移除", "上移", "下移"] {
        assert!(h.query_by_label(label).is_some(), "{label} missing from the effect menu");
    }
    for label in ENGLISH {
        assert!(h.query_by_label(label).is_none(), "{label} drawn in English");
    }

    let (mut en, fx) = harness("en");
    open_effect_menu(&mut en, fx);
    for label in ENGLISH {
        assert!(en.query_by_label(label).is_some(), "{label} missing from the English menu");
    }
}

/// Everything an agent or a script addresses stays English: the automation label, the property
/// tree's group name and `effect.list`.
#[test]
fn the_ids_automation_labels_and_effect_list_stay_english() {
    let (mut h, fx) = harness("zh-hans");
    assert_eq!(
        h.state().auto.find(&format!("effectControls.effect.{fx}")).map(|e| e.label.clone()),
        Some(BLUR_EN.to_string()),
        "the automation label is the property tree's name"
    );
    let listed = h.state_mut().session.execute("effect.list", json!({})).unwrap();
    let entry = listed.as_array().and_then(|a| a.iter().find(|e| e["id"] == BLUR)).cloned().unwrap_or_default();
    assert_eq!(entry["name"], BLUR_EN, "effect.list keeps the registry's name");
    assert_eq!(entry["category"], "Blur & Sharpen", "effect.list keeps the registry's category");
}
