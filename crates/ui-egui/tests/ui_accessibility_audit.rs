//! Accessibility audit (docs/gaps.md gap 25): renders panels headless and walks the AccessKit
//! tree of the frame. Every interactive node (button, check box, text input, combo box, slider,
//! spin button, tree or list item, tab, link, ...) must have a non-empty name, a role that says
//! what it is, and the state that role needs (checked, selected, a value).
//!
//! * The panels that had their pass (`FIXED`: Effects & Presets, Settings, Render Queue with its
//!   Templates dialog) must have **zero** findings.
//! * Every other panel is counted and printed (run with `--nocapture`), and checked against
//!   [`BASELINE`], a ratchet that can only shrink: fixing a panel lowers its number, and a number
//!   must never go up. When a panel's count falls, lower its entry (the test says so).
//!
//! What this does not measure: how a real screen reader speaks the tree (none was run), colour
//! contrast, focus order or keyboard operation (the `ui_*_keyboard.rs` tests cover those), or
//! widgets that are scrolled out of view and not drawn.

use std::collections::HashSet;

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use egui::accesskit::{Action, Role, Toggled};
use egui::{Rect, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::NodeT;
use serde_json::json;

/// Groups that must be clean. A group is the first part of a scenario name (before the `:`).
const FIXED: &[&str] = &["EffectsPresets", "Settings", "RenderQueue", "Project"];

/// (scenario, findings, of them unnamed controls) per not-yet-fixed scenario, as measured when
/// this test was written (Effect Controls with one Gaussian Blur on the layer, every other panel
/// as it opens). A ratchet: a number may only go down, never up; when a pass over a panel lowers
/// it, lower its entry in the same change, and remove the entry (and add the panel to `FIXED`)
/// when it reaches zero. A scenario that is not listed is held to zero.
const BASELINE: &[(&str, usize, usize)] = &[
    ("EffectControls:default", 25, 15),
    ("Composition:default", 16, 14),
    ("Layer:default", 4, 3),
    ("Timeline:default", 83, 55),
    ("Audio:default", 4, 2),
    ("Preview:default", 5, 5),
    ("Properties:default", 45, 28),
    ("Character:default", 13, 9),
    ("Paragraph:default", 24, 12),
    ("Align:default", 18, 11),
    ("Tracker:default", 1, 1),
    ("Wiggler:default", 2, 2),
    ("Smoother:default", 2, 1),
    ("MotionSketch:default", 4, 2),
    ("Paint:default", 5, 3),
    ("Brushes:default", 16, 9),
    ("Flowchart:default", 8, 4),
    ("History:default", 10, 5),
    ("Markers:default", 1, 1),
    ("MaskInterpolation:default", 2, 2),
    ("ScriptConsole:default", 5, 3),
    ("EssentialGraphics:default", 2, 2),
    ("Footage:default", 3, 2),
    ("Progress:default", 1, 1),
    ("VrCompEditor:default", 13, 7),
];

/// How many controls the fixed scenarios must at least show, so that "zero findings" is never
/// the result of looking at nothing. Any scenario not listed needs 2.
const MIN_CONTROLS: &[(&str, usize)] = &[
    ("EffectsPresets:default", 10),
    ("EffectsPresets:open", 10),
    ("EffectsPresets:search", 5),
    ("EffectsPresets:menu", 8),
    ("Settings:general", 12),
    ("Settings:appearance", 15),
    ("Settings:labels", 30),
    ("RenderQueue:queued", 8),
    ("RenderQueue:expanded", 12),
    ("RenderQueue:menu", 20),
    ("RenderQueue:templates:renderSettings", 8),
    ("RenderQueue:templates:outputModule", 8),
];

/// The roles that are controls: anything with one of these (or with a click or focus action) is
/// held to the rules below.
const CONTROL_ROLES: &[Role] = &[
    Role::Button,
    Role::CheckBox,
    Role::ColorWell,
    Role::ComboBox,
    Role::Link,
    Role::ListBoxOption,
    Role::ListItem,
    Role::MenuItem,
    Role::MenuItemCheckBox,
    Role::MenuItemRadio,
    Role::MultilineTextInput,
    Role::RadioButton,
    Role::SearchInput,
    Role::Slider,
    Role::SpinButton,
    Role::Switch,
    Role::Tab,
    Role::TextInput,
    Role::TreeItem,
];

#[derive(Debug, Clone)]
struct Finding {
    scenario: String,
    problem: &'static str,
    role: Role,
    name: String,
    rect: [i32; 4],
}

impl std::fmt::Display for Finding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}: {:?} {:?} at {:?}", self.scenario, self.problem, self.role, self.name, self.rect)
    }
}

fn app_session() -> Session {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "duration": 4})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Red", "color": "#c04040"})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Blue", "color": "#4040c0"})).unwrap();
    s.execute("effect.apply", json!({"effect": "Gaussian Blur"})).ok();
    s
}

fn harness() -> Harness<'static, EffectcraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(app_session()));
    h.run_steps(4);
    h
}

fn invoke(h: &mut Harness<'_, EffectcraftApp>, id: &str, params: serde_json::Value) {
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, id, params).unwrap();
    h.run_steps(4);
}

fn panel_rect(h: &Harness<'_, EffectcraftApp>, p: PanelKind) -> Option<Rect> {
    let e = h.state().auto.find(&format!("panel.{}", p.id()))?;
    Some(Rect::from_min_size(pos2(e.rect[0], e.rect[1]), egui::vec2(e.rect[2], e.rect[3])))
}

fn dialog_rect(h: &Harness<'_, EffectcraftApp>, title: &str) -> Option<Rect> {
    h.ctx.memory(|m| m.area_rect(egui::Id::new(("modal", title))))
}

/// The ids of every node now, so a later audit can look at what appeared since (a popup).
fn node_ids(h: &Harness<'_, EffectcraftApp>) -> HashSet<String> {
    h.root().children_recursive().map(|n| format!("{:?}", n.accesskit_node().id())).collect()
}

/// Walk the tree and check every control whose centre is inside `scope` (and, when `since` is
/// given, that was not in the tree before).
fn audit(h: &Harness<'_, EffectcraftApp>, scenario: &str, scope: Rect, since: Option<&HashSet<String>>) -> (Vec<Finding>, usize) {
    let mut out = Vec::new();
    let mut controls = 0;
    for n in h.root().children_recursive() {
        let a = n.accesskit_node();
        if a.is_hidden() {
            continue;
        }
        let Some(b) = a.bounding_box() else {
            // No bounds: assistive technology cannot place it. Only a control is a finding (and
            // it is one wherever it is: there is no position to scope it by).
            if CONTROL_ROLES.contains(&a.role()) {
                out.push(Finding { scenario: scenario.to_string(), problem: "no bounds", role: a.role(), name: a.label().unwrap_or_default(), rect: [0; 4] });
            }
            continue;
        };
        if !scope.contains(pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32)) {
            continue;
        }
        if since.is_some_and(|s| s.contains(&format!("{:?}", a.id()))) {
            continue;
        }
        let role = a.role();
        let actionable = a.data().supports_action(Action::Click) || a.data().supports_action(Action::Focus);
        let control = CONTROL_ROLES.contains(&role);
        if !control && !(actionable && matches!(role, Role::Unknown | Role::GenericContainer)) {
            continue;
        }
        let name = a.label().unwrap_or_default();
        controls += 1;
        let finding = |problem: &'static str| Finding {
            scenario: scenario.to_string(),
            problem,
            role,
            name: name.clone(),
            rect: [b.x0 as i32, b.y0 as i32, b.x1 as i32, b.y1 as i32],
        };
        if !control && matches!(role, Role::Unknown | Role::GenericContainer) {
            // Something you can click or focus, that no role describes.
            out.push(finding("clickable or focusable without a control role"));
        }
        if name.trim().is_empty() {
            out.push(finding("no name"));
        }
        let state_missing = match role {
            Role::CheckBox | Role::RadioButton | Role::Switch | Role::MenuItemCheckBox | Role::MenuItemRadio => {
                a.toggled().is_none() || a.toggled() == Some(Toggled::Mixed)
            }
            Role::Slider | Role::SpinButton => a.numeric_value().is_none(),
            Role::ComboBox => a.value().is_none_or(|v| v.trim().is_empty()),
            Role::TreeItem | Role::ListItem | Role::ListBoxOption | Role::Tab => a.is_selected().is_none(),
            _ => false,
        };
        if state_missing {
            out.push(finding("no state or value"));
        }
    }
    (out, controls)
}

fn count_unnamed(f: &[Finding]) -> usize {
    f.iter().filter(|f| f.problem == "no name").count()
}

struct Report {
    findings: Vec<Finding>,
    /// (scenario, findings, of them unnamed, controls looked at)
    rows: Vec<(String, usize, usize, usize)>,
}

impl Report {
    fn add(&mut self, scenario: &str, (f, controls): (Vec<Finding>, usize)) {
        self.rows.push((scenario.to_string(), f.len(), count_unnamed(&f), controls));
        self.findings.extend(f);
    }
}

fn group(scenario: &str) -> &str {
    scenario.split(':').next().unwrap_or(scenario)
}

fn collect() -> Report {
    let mut r = Report { findings: vec![], rows: vec![] };

    // ---- Effects & Presets: closed, everything open, searching, the contents menu.
    let mut h = harness();
    h.state_mut().show_panel(PanelKind::EffectsPresets);
    h.run_steps(4);
    let rect = panel_rect(&h, PanelKind::EffectsPresets).expect("Effects & Presets is drawn");
    r.add("EffectsPresets:default", audit(&h, "EffectsPresets:default", rect, None));
    {
        let s = h.state_mut();
        s.session.prefs.toggle_effect_favorite("ec.blur.gaussian");
        s.ui.effects_open.insert("favorites".into());
        s.ui.effects_open.insert("presets".into());
        s.ui.effects_open.insert("presets/Text".into());
        s.ui.effects_open.insert("Blur & Sharpen".into());
    }
    h.run_steps(4);
    r.add("EffectsPresets:open", audit(&h, "EffectsPresets:open", rect, None));
    h.state_mut().ui.effects_search = "blur".into();
    h.run_steps(4);
    r.add("EffectsPresets:search", audit(&h, "EffectsPresets:search", rect, None));
    let before = node_ids(&h);
    h.ctx.data_mut(|d| d.insert_temp(egui::Id::new("fx-contents-menu").with("open"), true));
    h.run_steps(4);
    let screen = Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(1600.0, 1000.0));
    r.add("EffectsPresets:menu", audit(&h, "EffectsPresets:menu", screen, Some(&before)));

    // ---- Settings: every page.
    let mut h = harness();
    let before = node_ids(&h);
    for page in effectcraft_engine::prefs::pages() {
        invoke(&mut h, "app.settings", json!({"page": page.id}));
        let name = format!("Settings:{}", page.id);
        let rect = dialog_rect(&h, "Settings").expect("the Settings dialog is open");
        r.add(&name, audit(&h, &name, rect, Some(&before)));
    }

    // ---- Render Queue: empty, queued and expanded, a menu open, the Templates dialogs.
    let mut h = harness();
    h.state_mut().show_panel(PanelKind::RenderQueue);
    h.run_steps(4);
    let rect = panel_rect(&h, PanelKind::RenderQueue).expect("the Render Queue is drawn");
    r.add("RenderQueue:empty", audit(&h, "RenderQueue:empty", rect, None));
    let comp = h.state().session.active_comp_id().expect("a comp").0;
    let mut items = vec![];
    for _ in 0..2 {
        let out = h.state_mut().session.execute("renderQueue.add", json!({"comp": comp})).unwrap();
        items.push(out["item"].as_u64().or_else(|| out["items"][0].as_u64()).unwrap_or(0));
    }
    h.run_steps(4);
    r.add("RenderQueue:queued", audit(&h, "RenderQueue:queued", rect, None));
    let first = h.state().session.project.render_queue.first().map(|i| i.id).expect("a queue item");
    h.ctx.data_mut(|d| d.insert_temp(egui::Id::new("rq-ui"), (vec![first], Some(first))));
    h.run_steps(4);
    r.add("RenderQueue:expanded", audit(&h, "RenderQueue:expanded", rect, None));
    let before = node_ids(&h);
    h.ctx.data_mut(|d| d.insert_temp(egui::Id::new(("rq-dd", first, 0)).with("open"), true));
    h.run_steps(4);
    r.add("RenderQueue:menu", audit(&h, "RenderQueue:menu", screen, Some(&before)));
    h.ctx.data_mut(|d| d.insert_temp(egui::Id::new(("rq-dd", first, 0)).with("open"), false));
    let before = node_ids(&h);
    for (kind, title) in [("renderSettings", "Render Settings Templates"), ("outputModule", "Output Module Templates")] {
        invoke(&mut h, "app.templates", json!({"kind": kind}));
        let name = format!("RenderQueue:templates:{kind}");
        let rect = dialog_rect(&h, title).expect("the Templates dialog is open");
        r.add(&name, audit(&h, &name, rect, Some(&before)));
    }
    let _ = items;

    // ---- Everything else, informational: each panel on its own.
    let mut h = harness();
    for p in PanelKind::ALL {
        if matches!(p, PanelKind::EffectsPresets | PanelKind::RenderQueue) {
            continue;
        }
        h.state_mut().show_panel(p);
        h.run_steps(3);
        let name = format!("{}:default", p.id());
        let Some(rect) = panel_rect(&h, p) else {
            r.rows.push((format!("{name} (not drawn)"), 0, 0, 0));
            continue;
        };
        r.add(&name, audit(&h, &name, rect, None));
    }
    r
}

#[test]
fn fixed_panels_have_no_unnamed_or_roleless_controls_and_the_others_do_not_get_worse() {
    let report = collect();
    println!("\nAccessibility audit: findings per scenario (unnamed in brackets)");
    let mut by_group: Vec<(String, usize, usize, usize)> = vec![];
    for (name, n, unnamed, controls) in &report.rows {
        println!("  {name:<44} {n:>4} findings ({unnamed} unnamed) of {controls} controls");
        let g = group(name).to_string();
        match by_group.iter_mut().find(|(k, ..)| *k == g) {
            Some(e) => {
                e.1 += n;
                e.2 += unnamed;
                e.3 += controls;
            }
            None => by_group.push((g, *n, *unnamed, *controls)),
        }
    }
    println!("\nBy panel (findings, unnamed controls, controls looked at):");
    for (g, n, unnamed, controls) in &by_group {
        let state = if FIXED.contains(&g.as_str()) { "fixed: must be 0" } else { "not yet audited" };
        println!("  {g:<28} {n:>4} findings, {unnamed:>3} unnamed, {controls:>4} controls  {state}");
    }
    let total_unnamed: usize = by_group.iter().map(|(_, _, u, _)| u).sum();
    println!("  all panels: {total_unnamed} unnamed controls\n");

    let mut failures = vec![];
    for f in &report.findings {
        if FIXED.contains(&group(&f.scenario)) {
            failures.push(f.to_string());
        }
    }
    assert!(failures.is_empty(), "{} accessibility findings in the fixed panels:\n{}", failures.len(), failures.join("\n"));

    // A clean panel must not be clean because nothing was looked at.
    for (name, _, _, controls) in &report.rows {
        if FIXED.contains(&group(name)) {
            let min = MIN_CONTROLS.iter().find(|(k, _)| k == name).map_or(2, |(_, v)| *v);
            assert!(*controls >= min, "{name}: only {controls} controls found in the tree (expected {min}); the scenario is not showing the panel");
        }
    }

    for (name, n, unnamed, _) in &report.rows {
        if FIXED.contains(&group(name)) {
            continue;
        }
        let (allowed, allowed_unnamed) = BASELINE.iter().find(|(k, ..)| k == name).map_or((0, 0), |(_, v, u)| (*v, *u));
        assert!(*n <= allowed, "{name}: {n} findings, the baseline allows {allowed}; a panel must not get worse");
        assert!(*unnamed <= allowed_unnamed, "{name}: {unnamed} unnamed controls, the baseline allows {allowed_unnamed}; a panel must not get worse");
        if *n < allowed || *unnamed < allowed_unnamed {
            println!("note: {name} improved to {n} findings ({unnamed} unnamed); lower its BASELINE entry from {allowed} ({allowed_unnamed})");
        }
    }
}
