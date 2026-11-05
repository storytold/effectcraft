//! Closing the window while a comp is bound to a link. FilmCraft's clip plays the result movie, so
//! a close must not leave it without one, and must not lose the edits of the link project.
//!
//! One test function: the live link is a process global, and the scenarios share it in turn.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use effectcraft_engine::Session;
use effectcraft_project::{CraftLinkBinding, Project};
use effectcraft_ui_egui::EffectcraftApp;
use egui::ViewportCommand;
use egui_kittest::Harness;
use serde_json::json;

const FIRST: &str = "55555555-5555-4555-8555-555555555555";
const SECOND: &str = "66666666-6666-4666-8666-666666666666";

fn link_dir(tag: &str, link: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ec-uic-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("link dir");
    std::fs::write(dir.join(format!("{link}.ecproj")), b"{}").expect("sidecar");
    dir
}

/// A harness whose active comp is bound to `link`, results in `dir`.
fn harness(dir: &Path, link: &str) -> Harness<'static, EffectcraftApp> {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Linked", "width": 64, "height": 36, "duration": 1})).expect("comp");
    s.execute("layer.newSolid", json!({"name": "Plate", "color": "#406080", "width": 64, "height": 36})).expect("solid");
    let comp = s.active_comp_id().expect("active comp");
    s.project_mut().craft_links.insert(
        link.into(),
        CraftLinkBinding {
            link_id: link.into(),
            comp,
            source: json!({"app": "filmcraft", "doc_id": "0", "item": "1"}),
            result_path: dir.join(format!("{link}.mov")).to_string_lossy().into_owned(),
            epoch: 0,
        },
    );
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |_| EffectcraftApp::new(s));
    h.run_steps(3);
    h
}

fn commands(h: &Harness<'_, EffectcraftApp>) -> Vec<ViewportCommand> {
    h.output().viewport_output.get(&egui::ViewportId::ROOT).map(|v| v.commands.clone()).unwrap_or_default()
}

fn request_close(h: &mut Harness<'_, EffectcraftApp>) {
    h.input_mut().viewports.entry(egui::ViewportId::ROOT).or_default().events.push(egui::ViewportEvent::Close);
}

fn sidecar_epoch(dir: &Path, link: &str) -> Option<u64> {
    let text = std::fs::read_to_string(dir.join(format!("{link}.ecproj"))).ok()?;
    Some(Project::from_json(&text).ok()?.craft_links.get(link)?.epoch)
}

#[test]
fn closing_a_linked_window() {
    effectcraft_link::live::install(Arc::new(|_, _| {}), Arc::new(|| {}));

    // A close with no result yet holds the window, renders, then closes.
    let dir = link_dir("render", FIRST);
    let mut h = harness(&dir, FIRST);
    assert!(!dir.join(format!("{FIRST}.mov")).exists());
    request_close(&mut h);
    h.step();
    assert!(commands(&h).contains(&ViewportCommand::CancelClose), "a close with no result must keep the window open: {:?}", commands(&h));
    assert!(h.state().ui.status.contains("Rendering result for FilmCraft"), "status: {:?}", h.state().ui.status);
    assert!(sidecar_epoch(&dir, FIRST).is_some(), "the sidecar must be on disk before the render starts, not the placeholder");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut closed = false;
    while !closed && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
        h.step();
        closed = commands(&h).contains(&ViewportCommand::Close);
    }
    assert!(closed, "the window never closed after the render");
    assert!(dir.join(format!("{FIRST}.mov")).exists(), "the window closed without the result movie");
    assert!(!dir.join(format!("{FIRST}.mov.part")).exists());
    assert_eq!(sidecar_epoch(&dir, FIRST), Some(1), "the sidecar carries the epoch of the result");
    assert!(!h.state().link_close_rendering());

    // A second close request while the render runs skips it. The sidecar is written, the window closes.
    let dir = link_dir("skip", SECOND);
    let mut h = harness(&dir, SECOND);
    request_close(&mut h);
    h.step();
    assert!(commands(&h).contains(&ViewportCommand::CancelClose));
    request_close(&mut h);
    h.step();
    // The harness project is modified, so the user's own save prompt may hold the window. That
    // prompt is about the project file. The link render must be over.
    assert!(!h.state().link_close_rendering(), "the second request must end the render hold");
    assert!(h.state().ui.status.contains("without the result"), "status: {:?}", h.state().ui.status);
    assert!(sidecar_epoch(&dir, SECOND).is_some(), "the skip left the placeholder sidecar");
    assert!(!dir.join(format!("{SECOND}.mov.part")).exists(), "a part file was left");

    // The user keeps the window: the unsaved-changes prompt is cancelled. The skip ended with
    // that close, so the next close renders the result again.
    h.step(); // the prompt draws the frame after the request
    h.key_press(egui::Key::Escape);
    h.step();
    request_close(&mut h);
    h.step();
    assert!(h.state().link_close_rendering(), "a cancelled close must not skip the next result render");
    assert!(commands(&h).contains(&ViewportCommand::CancelClose), "{:?}", commands(&h));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while h.state().link_close_rendering() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
        h.step();
    }
    assert!(!h.state().link_close_rendering(), "the render never finished");
    assert!(dir.join(format!("{SECOND}.mov")).exists(), "the second close rendered the result");

    // eframe's exit hook writes the link project whatever the user answered about their own project.
    let dir = link_dir("exit", FIRST);
    let mut h = harness(&dir, FIRST);
    assert!(sidecar_epoch(&dir, FIRST).is_none(), "the placeholder is not a project");
    eframe::App::on_exit(h.state_mut());
    assert_eq!(sidecar_epoch(&dir, FIRST), Some(0));
}
