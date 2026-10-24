//! The agent-interface audit (UI side, M13.14):
//!
//! - every registered engine command is reachable over the control channel (`engine.commands`
//!   lists it; `engine.execute` routes it: an unknown parameter is rejected by that command);
//! - every panel kind, opened headless, registers an automation id for every interactive widget
//!   it draws: egui's interactive widget rects of the frame are matched against the registered
//!   automation elements, and any widget no element covers is a gap.

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::control::{self, ControlRequest, Outcome};
use effectcraft_ui_egui::dock::PanelKind;
use egui::Rect;
use egui_kittest::Harness;
use serde_json::{Value, json};

fn session() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

fn harness(s: Session) -> Harness<'static, EffectcraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1680.0, 1020.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    h
}

fn send(h: &mut Harness<'_, EffectcraftApp>, method: &str, params: Value) -> Value {
    let ctx = h.ctx.clone();
    let (req, _rx) = ControlRequest::new(method, params);
    match control::handle(h.state_mut(), &ctx, &req) {
        Outcome::Done(v) => v,
        _ => panic!("{method}: no immediate reply"),
    }
}

#[test]
fn every_engine_command_is_reachable_over_the_control_channel() {
    let mut h = harness(session());
    let listed = send(&mut h, "engine.commands", json!({}));
    let ids: std::collections::HashSet<String> = listed["result"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap().to_string()).collect();
    for spec in effectcraft_engine::command_specs() {
        assert!(ids.contains(spec.id), "engine.commands misses {}", spec.id);
        let v = send(&mut h, "engine.execute", json!({"command": spec.id, "params": {"__audit": 1}}));
        let e = v["error"].as_str().unwrap_or_default();
        assert!(v["ok"] == false && e.contains("unknown parameter") && e.contains(spec.id), "{}: {v}", spec.id);
    }
}

/// Interactive widgets (click or drag sense, enabled, visible) of the last frame, in screen
/// points, with their accessibility label when egui knows one.
fn interactive(ctx: &egui::Context) -> Vec<(Rect, String)> {
    let screen = ctx.content_rect();
    ctx.viewport(|v| {
        let w = &v.this_pass.widgets;
        let mut out = vec![];
        for (layer, rects) in w.layers() {
            if layer.order == egui::Order::Tooltip || layer.order == egui::Order::Debug {
                continue;
            }
            for r in rects {
                let sense = r.sense;
                if !(sense.senses_click() || sense.senses_drag()) || !r.enabled {
                    continue;
                }
                let ir = r.interact_rect.intersect(screen);
                if ir.width() < 2.0 || ir.height() < 2.0 {
                    continue;
                }
                let info = w.info(r.id);
                // Selectable text (labels sense drags to select their text) is not a control.
                if info.is_some_and(|i| i.typ == egui::WidgetType::Label) {
                    continue;
                }
                out.push((ir, info.and_then(|i| i.label.clone()).unwrap_or_default()));
            }
        }
        out
    })
}

/// Automation elements that cover a widget: overlapping at least half of the smaller of the two,
/// and not a container many times the widget's size (a panel or a whole row).
fn covered(w: Rect, elements: &[Rect]) -> bool {
    elements.iter().any(|e| {
        let i = e.intersect(w);
        if i.width() <= 0.0 || i.height() <= 0.0 {
            return false;
        }
        let (aw, ae) = (w.area(), e.area());
        i.area() >= 0.5 * aw.min(ae) && ae <= 40.0 * aw.max(64.0)
    })
}

/// (panel, widget rect, label) of every interactive widget no automation element covers, for
/// one panel shown maximized.
fn gaps_of(h: &mut Harness<'_, EffectcraftApp>, p: PanelKind) -> Vec<String> {
    h.state_mut().show_panel(p);
    h.state_mut().ui.maximized = Some(p);
    h.run_steps(4);
    let ctx = h.ctx.clone();
    let widgets = interactive(&ctx);
    let app = h.state();
    let elements: Vec<Rect> = app
        .auto
        .previous
        .iter()
        .chain(app.auto.elements.iter())
        .map(|e| Rect::from_min_size(egui::pos2(e.rect[0], e.rect[1]), egui::vec2(e.rect[2], e.rect[3])))
        .collect();
    let Some(panel) =
        app.auto.find(&format!("panel.{}", p.id())).map(|e| Rect::from_min_size(egui::pos2(e.rect[0], e.rect[1]), egui::vec2(e.rect[2], e.rect[3])))
    else {
        return vec![format!("{}: panel.{} not registered", p.title(), p.id())];
    };
    widgets
        .into_iter()
        .filter(|(r, _)| panel.contains(r.center()))
        .filter(|(r, _)| !covered(*r, &elements))
        .map(|(r, l)| format!("{}: [{:.0},{:.0} {:.0}x{:.0}] {l:?}", p.title(), r.min.x, r.min.y, r.width(), r.height()))
        .collect()
}

#[test]
#[cfg_attr(not(debug_assertions), ignore = "egui records widget types only in debug builds")]
fn every_panel_registers_automation_ids_for_its_widgets() {
    let mut s = session();
    // Content for the selection-driven panels: the title (text with effects), a Render Queue
    // item, a comp marker.
    s.execute("layer.select", json!({"layers": ["EFFECTCRAFT"]})).unwrap();
    s.execute("renderQueue.add", json!({"format": "png", "output": "/tmp/ec-audit-[#####].png"})).unwrap();
    s.execute("markers.add", json!({})).ok();
    let title = s.active_comp().unwrap().layers.iter().find(|l| l.name == "EFFECTCRAFT").map(|l| l.id.0).unwrap();
    let mut h = harness(s);
    h.state_mut().ui.layer_panel = Some(title);
    // egui records widget types (to tell labels from controls) with this debug option, which
    // only debug builds have.
    #[cfg(debug_assertions)]
    h.ctx.all_styles_mut(|st| st.debug.show_interactive_widgets = true);
    let mut gaps = vec![];
    for p in PanelKind::ALL {
        gaps.extend(gaps_of(&mut h, p));
    }
    // The timeline again with the title twirled open (properties, keys) and in the Graph Editor.
    send(&mut h, "ui.set", json!({"timeline": {"openLayers": [title]}}));
    gaps.extend(gaps_of(&mut h, PanelKind::Timeline));
    send(&mut h, "ui.set", json!({"timeline": {"graphEditor": true}}));
    gaps.extend(gaps_of(&mut h, PanelKind::Timeline));
    send(&mut h, "ui.set", json!({"timeline": {"graphEditor": false}}));
    // Outside the panels: the header and the menu bar.
    h.state_mut().ui.maximized = None;
    h.run_steps(3);
    let ctx = h.ctx.clone();
    let widgets = interactive(&ctx);
    let app = h.state();
    let elements: Vec<Rect> =
        app.auto.previous.iter().map(|e| Rect::from_min_size(egui::pos2(e.rect[0], e.rect[1]), egui::vec2(e.rect[2], e.rect[3]))).collect();
    let panels: Vec<Rect> = app
        .auto
        .previous
        .iter()
        .filter(|e| e.id.starts_with("panel.") && e.id.matches('.').count() == 1)
        .map(|e| Rect::from_min_size(egui::pos2(e.rect[0], e.rect[1]), egui::vec2(e.rect[2], e.rect[3])))
        .collect();
    for (r, l) in widgets {
        if !panels.iter().any(|p| p.contains(r.center())) && !covered(r, &elements) {
            gaps.push(format!("chrome: [{:.0},{:.0} {:.0}x{:.0}] {l:?}", r.min.x, r.min.y, r.width(), r.height()));
        }
    }
    assert!(gaps.is_empty(), "{} interactive widgets without an automation id:\n{}", gaps.len(), gaps.join("\n"));
}
