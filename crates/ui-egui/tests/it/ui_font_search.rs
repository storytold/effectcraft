//! Real keyboard/pointer coverage for the font menus in both text panels.
use effectcraft_engine::{Session, commands::text_edit::layer_doc, project::LayerId};
use effectcraft_ui_egui::{EffectcraftApp, dock::PanelKind};
use egui::{Event, Key, Modifiers, PointerButton};
use egui_kittest::Harness;
use serde_json::json;

fn click(h: &mut Harness<'_, EffectcraftApp>, id: &str) {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("missing {id}")).clone();
    let pos = egui::pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
    h.input_mut().events.push(Event::PointerMoved(pos));
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.run_steps(2);
    }
}

fn key(h: &mut Harness<'_, EffectcraftApp>, key: Key) {
    for pressed in [true, false] {
        h.input_mut().events.push(Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE });
    }
    h.run_steps(3);
}

fn type_text(h: &mut Harness<'_, EffectcraftApp>, text: &str) {
    h.input_mut().events.push(Event::Text(text.into()));
    h.run_steps(3);
}

#[test]
fn font_search_filters_selects_and_undoes_in_both_panels() {
    for (panel, prefix) in [(PanelKind::Character, "character.font"), (PanelKind::Properties, "properties.text.font")] {
        let mut s = Session::default();
        s.execute("comp.new", json!({"width": 320, "height": 180})).unwrap();
        let lid = s.execute("layer.newText", json!({"text": "Keep this text", "font": "Inter"})).unwrap()["layer"].as_u64().unwrap();
        let builder = Harness::builder().with_size(egui::vec2(1600.0, 1000.0));
        let mut h = if std::env::var_os("EC_SNAPSHOT_DIR").is_some() {
            builder.wgpu().build_eframe(move |_| EffectcraftApp::new(s))
        } else {
            builder.build_eframe(move |_| EffectcraftApp::new(s))
        };
        h.state_mut().show_panel(panel);
        h.run_steps(3);
        click(&mut h, prefix);
        assert!(h.state().auto.find(&format!("{prefix}.search")).is_some());
        assert!(effectcraft_ui_egui::widgets::any_menu_open(&h.ctx), "font search must hold app shortcuts like other popup lists");
        let field = h.state().auto.find(prefix).unwrap();
        let search = h.state().auto.find(&format!("{prefix}.search")).unwrap();
        assert!(
            (field.rect[1] + field.rect[3] / 2.0 - search.rect[1] - search.rect[3] / 2.0).abs() < 1.0,
            "font selection and search must occupy the same row"
        );
        // Search gets focus on open. Mixed case and surrounding whitespace are accepted.
        type_text(&mut h, "  jEtBrAiNs  ");
        let rows = h.state().auto.query(&format!("{prefix}.option."));
        assert!(!rows.is_empty());
        assert!(rows.iter().all(|r| r.label.to_lowercase().contains("jetbrains")));
        if let Ok(dir) = std::env::var("EC_SNAPSHOT_DIR") {
            h.render().unwrap().save(format!("{dir}/{prefix}-search.png")).unwrap();
        }
        assert_eq!(layer_doc(&h.state().session, LayerId(lid)).unwrap().font, "Inter", "typing only filters");
        key(&mut h, Key::Enter);
        assert_eq!(layer_doc(&h.state().session, LayerId(lid)).unwrap().font, "JetBrains Mono");
        assert_eq!(layer_doc(&h.state().session, LayerId(lid)).unwrap().text, "Keep this text");
        h.state_mut().session.execute("edit.undo", json!({})).unwrap();
        assert_eq!(layer_doc(&h.state().session, LayerId(lid)).unwrap().font, "Inter");
        click(&mut h, prefix);
        type_text(&mut h, "no_such_font_123456789");
        assert!(h.state().auto.query(&format!("{prefix}.option.")).is_empty());
        assert!(h.state().auto.find(&format!("{prefix}.empty")).is_some());
        key(&mut h, Key::Enter);
        assert!(h.state().auto.find(&format!("{prefix}.search")).is_some(), "no result keeps menu open");
        key(&mut h, Key::Escape);
        assert_eq!(layer_doc(&h.state().session, LayerId(lid)).unwrap().font, "Inter");
        click(&mut h, prefix);
        // Reopening clears the previous query, and mouse selection still works.
        h.run_steps(2);
        if let Ok(dir) = std::env::var("EC_SNAPSHOT_DIR") {
            h.render().unwrap().save(format!("{dir}/{prefix}-reopened.png")).unwrap();
        }
        assert!(h.state().auto.query(&format!("{prefix}.option.")).len() > 1);
        type_text(&mut h, "JetBrains");
        click(&mut h, &format!("{prefix}.option.JetBrains Mono"));
        assert_eq!(layer_doc(&h.state().session, LayerId(lid)).unwrap().font, "JetBrains Mono");
    }
}

#[test]
fn font_search_preserves_edited_text_and_applies_only_to_selected_characters() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"width": 320, "height": 180})).unwrap();
    let lid = s.execute("layer.newText", json!({"text": "First second", "font": "Inter"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("text.edit", json!({"layer": lid})).unwrap();
    s.execute("text.setSelection", json!({"start": 0, "end": 5})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(move |_| EffectcraftApp::new(s));
    h.state_mut().show_panel(PanelKind::Character);
    h.run_steps(3);
    click(&mut h, "character.font");
    type_text(&mut h, "noto serif");
    key(&mut h, Key::ArrowDown);
    key(&mut h, Key::ArrowUp);
    key(&mut h, Key::Enter);
    let doc = layer_doc(&h.state().session, LayerId(lid)).unwrap();
    assert_eq!(doc.text, "First second");
    assert_eq!(doc.style_at(0).font, "Noto Serif");
    assert_eq!(doc.style_at(8).font, "Inter");
}

/// A font file of just `name` (the family, Regular) and an `fvar` table with one axis pinned at
/// 400: a variable font as far as the font menu's filter is concerned.
fn pinned_variable_font(family: &str) -> Vec<u8> {
    let u16s = |v: &[u16]| -> Vec<u8> { v.iter().flat_map(|x| x.to_be_bytes()).collect() };
    let strings: Vec<Vec<u8>> = [family, "Regular"].iter().map(|s| s.encode_utf16().flat_map(|u| u.to_be_bytes()).collect()).collect();
    let mut name = u16s(&[0, 2, 30]);
    let mut off = 0;
    for (id, st) in strings.iter().enumerate() {
        name.extend(u16s(&[3, 1, 0x409, id as u16 + 1, st.len() as u16, off]));
        off += st.len() as u16;
    }
    strings.iter().for_each(|st| name.extend(st));
    let mut fvar = u16s(&[1, 0, 16, 2, 1, 20, 0, 8]);
    fvar.extend(b"wght".iter().copied().chain(u16s(&[0x190, 0, 0x190, 0, 0x190, 0, 0, 256])));
    let mut out = u16s(&[1, 0, 2, 0, 0, 0]);
    let mut body: Vec<u8> = vec![];
    for (tag, data) in [(b"fvar", fvar), (b"name", name)] {
        out.extend(tag);
        out.extend([0u8; 4]);
        out.extend(((12 + 32 + body.len()) as u32).to_be_bytes());
        out.extend((data.len() as u32).to_be_bytes());
        body.extend(data);
    }
    out.extend(body);
    out
}

/// The font menu's Variable Fonts filter (After Effects 26.3): the checkbox keeps only variable
/// families, works with the typed search (which keeps the keyboard), and stays on when the menu
/// is reopened until it is turned off.
#[test]
fn font_menu_filters_variable_fonts() {
    const FAMILY: &str = "Inter Variable Menu Test";
    assert_eq!(effectcraft_engine::text::fonts::add_font_data(pinned_variable_font(FAMILY)).len(), 1);
    let variable = effectcraft_engine::text::fonts::variable_families();
    let mut s = Session::default();
    s.execute("comp.new", json!({"width": 320, "height": 180})).unwrap();
    let lid = s.execute("layer.newText", json!({"text": "Var", "font": "Inter"})).unwrap()["layer"].as_u64().unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(move |_| EffectcraftApp::new(s));
    h.state_mut().show_panel(PanelKind::Character);
    h.run_steps(3);
    let families = |h: &Harness<'_, EffectcraftApp>| -> Vec<String> {
        h.state().auto.query("character.font.option.").iter().map(|e| e.id.trim_start_matches("character.font.option.").to_string()).collect()
    };
    click(&mut h, "character.font");
    type_text(&mut h, "inter");
    let shown = families(&h);
    assert!(shown.iter().any(|f| f == "Inter") && shown.iter().any(|f| f == FAMILY), "unfiltered at first: {shown:?}");
    click(&mut h, "character.font.variable");
    let shown = families(&h);
    assert!(shown.iter().any(|f| f == FAMILY), "{shown:?}");
    assert!(shown.iter().all(|f| variable.contains(f)), "only variable families: {shown:?}");
    // The search field keeps the keyboard and narrows the filtered list further.
    type_text(&mut h, " variable menu");
    assert_eq!(families(&h), vec![FAMILY.to_string()]);
    type_text(&mut h, "x");
    assert!(families(&h).is_empty());
    assert!(h.state().auto.find("character.font.empty").is_some());
    key(&mut h, Key::Escape);
    // Reopened: still filtered; turning it off lists the static families again.
    click(&mut h, "character.font");
    h.run_steps(2);
    let shown = families(&h);
    assert!(!shown.is_empty() && shown.iter().all(|f| variable.contains(f)), "the filter stays on: {shown:?}");
    click(&mut h, "character.font.variable");
    type_text(&mut h, "inter");
    assert!(families(&h).iter().any(|f| f == "Inter"), "{:?}", families(&h));
    key(&mut h, Key::Escape);
    assert_eq!(layer_doc(&h.state().session, LayerId(lid)).unwrap().font, "Inter", "filtering changes nothing");
}

/// The font menu grows back when a query that matched nothing is cleared, while the search
/// field keeps its place and the keyboard; a click on a row then picks that font.
#[test]
fn font_menu_grows_back_after_an_empty_search() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"width": 320, "height": 180})).unwrap();
    let lid = s.execute("layer.newText", json!({"text": "Grow", "font": "Inter"})).unwrap()["layer"].as_u64().unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(move |_| EffectcraftApp::new(s));
    h.state_mut().show_panel(PanelKind::Character);
    h.run_steps(3);
    let rows = |h: &Harness<'_, EffectcraftApp>| h.state().auto.query("character.font.option.").len();
    let search = |h: &Harness<'_, EffectcraftApp>| h.state().auto.find("character.font.search").map(|e| e.rect);
    click(&mut h, "character.font");
    h.run_steps(2);
    let (full, field) = (rows(&h), search(&h));
    assert!(full > 3, "the open menu shows the fonts: {full} rows");
    let query = "no_such_font_123456789";
    type_text(&mut h, query);
    assert_eq!(rows(&h), 0);
    assert!(h.state().auto.find("character.font.empty").is_some());
    for pressed in query.chars().flat_map(|_| [true, false]) {
        h.input_mut().events.push(Event::Key { key: Key::Backspace, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE });
    }
    h.run_steps(3);
    // (Within a row: the list scrolls to the current font on opening and to the top when cleared.)
    assert!(rows(&h) + 1 >= full, "cleared, the menu shows as many fonts as when it opened: {} of {full}", rows(&h));
    assert_eq!(search(&h), field, "the search field stays put");
    // The field still has the keyboard: typing filters again.
    type_text(&mut h, "JetBrains");
    assert!(rows(&h) > 0 && h.state().auto.query("character.font.option.").iter().all(|r| r.label.contains("JetBrains")));
    click(&mut h, "character.font.option.JetBrains Mono");
    assert_eq!(layer_doc(&h.state().session, LayerId(lid)).unwrap().font, "JetBrains Mono", "a click on a row picks the font");
}
