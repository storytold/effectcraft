//! Accessibility of the app's own controls.
//!
//! The shared widgets in `widgets.rs` are raw `ui.interact` rectangles. Without a `WidgetInfo`
//! they are not in the accessibility tree at all, so neither a screen reader nor kittest can
//! reach them; and an input control without a name is unreachable even when it is in the tree.
//! `Harness::run` checks the tree it has, but only for the screen it settled on: this opens every
//! panel and checks again.
//!
//! Limit: a control that reports no `WidgetInfo` at all has no node, so it is not "unnamed" here.
//! `widgets::checkbox` and the raw-painted row controls (`icon_toggle`, `scroll_bar`, hot-number
//! values) are still in that state; naming every call site is separate work.

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;

fn harness() -> Harness<'static, EffectcraftApp> {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    // A text layer under edit gives the Character / Paragraph panels their controls.
    let _ = s.execute("layer.select", json!({"layers": ["EFFECTCRAFT"]}));
    Harness::builder().with_size(egui::vec2(1680.0, 1020.0)).build_eframe(|_| EffectcraftApp::new(s))
}

#[test]
fn custom_toolbar_buttons_are_named() {
    let mut h = harness();
    h.run();
    // An icon button carries no text of its own: its tooltip is its accessible name.
    let home = h.query_by_label("Home").expect("the toolbar's Home icon button is named by its tooltip");
    assert_eq!(home.accesskit_node().role(), egui::accesskit::Role::Button);
    assert!(h.unnamed_widgets().is_empty(), "{:?}", h.unnamed_widgets());
}

#[test]
fn every_panel_names_its_input_controls() {
    let mut h = harness();
    h.run();
    let mut unnamed = vec![];
    for p in PanelKind::ALL {
        h.state_mut().show_panel(p);
        h.state_mut().ui.maximized = Some(p);
        h.run_steps(4);
        for w in h.unnamed_widgets() {
            unnamed.push(format!("{}: {w}", p.title()));
        }
    }
    assert!(unnamed.is_empty(), "{} control(s) have no accessible name:\n{}", unnamed.len(), unnamed.join("\n"));
}
