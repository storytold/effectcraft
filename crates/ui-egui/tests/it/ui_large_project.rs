//! M13.14: the UI with a large project. The Project panel scrolls (only the rows in view are
//! drawn), reveals an item selected elsewhere and renders thumbnails off the UI thread; the
//! Timeline reaches the last of thousands of layers; the Progress panel lists the footage check
//! started by a lazy open.

use effectcraft_engine::Session;
use effectcraft_engine::perf::{LargeSpec, large_project};
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use egui_kittest::Harness;
use serde_json::json;

fn spec() -> LargeSpec {
    LargeSpec { comps: 8, main_layers: 1500, comp_layers: 3, footage: 90, sequence_frames: 12, nest_depth: 3, expression_every: 5, ..Default::default() }
}

fn harness(s: Session) -> Harness<'static, EffectcraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    h
}

fn large() -> Session {
    let mut s = Session::default();
    s.replace_project(large_project(&spec()), None);
    s
}

#[test]
fn project_panel_scrolls_and_reveals_the_selection() {
    let mut h = harness(large());
    let folders: Vec<u64> = h.state().session.project.items.values().filter(|i| i.is_folder()).map(|i| i.id.0).collect();
    h.state_mut().ui.project_open_folders.extend(folders);
    h.state_mut().show_panel(PanelKind::Project);
    h.run_steps(3);
    // Far more rows than fit: a scroll bar, and the bottom rows aren't drawn yet.
    assert!(h.state().auto.find("project.vscroll").is_some());
    let drawn = |h: &Harness<'_, EffectcraftApp>| {
        h.state().auto.previous.iter().filter(|e| e.id.starts_with("project.item.") && e.id.matches('.').count() == 2).count()
    };
    let rows = drawn(&h);
    assert!(rows > 5 && rows < 80, "only the rows in view are drawn: {rows}");
    // Selecting an item (a command, an agent) scrolls it into view.
    let footage =
        h.state().session.project.items.values().filter(|i| matches!(i.kind, effectcraft_engine::project::ItemKind::Footage(_))).map(|i| i.id.0).max().unwrap();
    h.state_mut().session.execute("project.select", json!({"items": [footage]})).unwrap();
    h.run_steps(3);
    assert!(h.state().auto.find(&format!("project.item.{footage}")).is_some(), "selected item scrolled into view");
    assert!(h.state().ui.project_scroll > 0.0);
    // Scrolling to the end shows the last rows.
    h.state_mut().ui.project_scroll = 1.0e6;
    h.run_steps(2);
    let shown: Vec<String> = h.state().auto.previous.iter().filter(|e| e.id.starts_with("project.item.")).map(|e| e.id.clone()).collect();
    assert!(!shown.is_empty() && drawn(&h) < 80, "{shown:?}");
}

#[test]
fn comp_thumbnail_renders_off_the_ui_thread() {
    let mut h = harness(large());
    let main = h.state().session.active_comp_id().unwrap();
    h.state_mut().session.execute("project.select", json!({"items": [main.0]})).unwrap();
    h.state_mut().show_panel(PanelKind::Project);
    // The frame that asks for the thumbnail doesn't wait for it; it lands a few frames later.
    let mut landed = false;
    for _ in 0..400 {
        h.step();
        if h.state().auto.find("project.thumbnail").is_some()
            && h.ctx.data(|d| d.get_temp::<(u64, egui::TextureHandle)>(egui::Id::new(("proj-thumb", main.0)))).is_some()
        {
            landed = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(landed, "thumbnail texture");
}

#[test]
fn timeline_reaches_the_last_of_many_layers() {
    let mut h = harness(large());
    h.state_mut().show_panel(PanelKind::Timeline);
    h.state_mut().ui.maximized = Some(PanelKind::Timeline);
    let last = h.state().session.active_comp().unwrap().layers.last().unwrap().id.0;
    let first = h.state().session.active_comp().unwrap().layers[0].id.0;
    h.run_steps(2);
    assert!(h.state().auto.find(&format!("timeline.layer.{first}.bar")).is_some());
    assert!(h.state().auto.find(&format!("timeline.layer.{last}.bar")).is_none(), "off-screen rows aren't drawn");
    h.state_mut().ui.timeline.scroll_y = 1.0e7;
    h.run_steps(2);
    assert!(h.state().auto.find(&format!("timeline.layer.{last}.bar")).is_some());
    // Track matte and parent menus list every layer only while open.
    let pop = h.state().auto.find(&format!("timeline.layer.{last}.parent")).cloned();
    assert!(pop.is_some());
}

#[test]
fn lazy_open_lists_the_footage_check_in_the_progress_panel() {
    let path = std::env::temp_dir().join(format!("ec-ui-lazy-open-{}.ecproj", std::process::id()));
    let mut s = large();
    s.execute("file.saveAs", json!({"path": path.to_string_lossy()})).unwrap();
    let mut o = Session { check_footage_on_open: true, ..Default::default() };
    o.execute("file.open", json!({"path": path.to_string_lossy()})).unwrap();
    assert!(o.jobs().iter().any(|j| j.kind == "footageCheck"));
    let mut h = harness(o);
    h.state_mut().show_panel(PanelKind::Progress);
    // The check finishes in the background; polling applies it: every file is missing.
    for _ in 0..500 {
        h.step();
        if h.state().session.tasks.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(h.state().session.tasks.is_empty());
    let missing =
        h.state().session.project.items.values().filter(|i| matches!(&i.kind, effectcraft_engine::project::ItemKind::Footage(f) if f.missing)).count();
    assert_eq!(missing, spec().footage);
    assert!(!h.state().session.is_dirty(), "the check doesn't modify the project");
    let _ = std::fs::remove_file(path);
}
