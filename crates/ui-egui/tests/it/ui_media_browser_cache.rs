use crate::gpu_test::RenderSerial;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use effectcraft_engine::{
    Session,
    media_browser::{Browser, Entry, VirtualTree},
};
use effectcraft_ui_egui::{EffectcraftApp, dock::PanelKind};
use egui::{Event, Modifiers, PointerButton, pos2};
use egui_kittest::Harness;
use serde_json::{Value, json};

#[derive(Default)]
struct MutableBrowser {
    tree: Mutex<VirtualTree>,
    offline: AtomicBool,
}

impl Browser for MutableBrowser {
    fn home(&self) -> String {
        "/Shoot".into()
    }
    fn list(&self, dir: &str, only: bool) -> Result<Vec<Entry>, String> {
        if self.offline.load(Ordering::Relaxed) {
            return Err("Folder temporarily unavailable".into());
        }
        self.tree.lock().unwrap().list(dir, only)
    }
    fn actions(&self) -> Vec<(String, String)> {
        vec![("refresh".into(), "Refresh".into())]
    }
    fn action(&self, id: &str) -> Result<Value, String> {
        assert_eq!(id, "refresh");
        self.tree.lock().unwrap().add_file("/Shoot/From action.csv", "/files/action.csv", 1, None);
        Ok(json!({}))
    }
}

type Listing = (f64, Arc<Result<Vec<Entry>, String>>);

fn listing(h: &Harness<'_, EffectcraftApp>, only: bool) -> Listing {
    let dir = h.state().session.state.media_browser.folder.clone().unwrap_or_else(|| "/Shoot".into());
    h.ctx.data(|d| d.get_temp(egui::Id::unique(("mb-list", dir, only)))).unwrap()
}

fn names(list: &Arc<Result<Vec<Entry>, String>>) -> Vec<&str> {
    list.as_ref().as_ref().unwrap().iter().map(|e| e.name.as_str()).collect()
}

fn click(h: &mut Harness<'_, EffectcraftApp>, id: &str) {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("missing {id}"));
    let pos = pos2(e.rect[0] + 8.0, e.rect[1] + e.rect[3] / 2.0);
    h.input_mut().events.push(Event::PointerMoved(pos));
    h.step();
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    h.run_steps(2);
}

fn at(h: &mut Harness<'_, EffectcraftApp>, time: f64) {
    h.input_mut().time = Some(time);
    h.step();
    h.input_mut().time = Some(time);
    h.step();
    h.input_mut().time = None;
}

#[test]
fn browser_cache_refreshes_filters_navigates_and_recovers_from_errors() {
    let browser = Arc::new(MutableBrowser::default());
    {
        let mut tree = browser.tree.lock().unwrap();
        tree.add_file("/Shoot/Clip.csv", "/files/clip.csv", 1, None);
        tree.add_file("/Shoot/notes.txt", "/files/notes.txt", 1, None);
        tree.add_file("/Shoot/Sub/Child.csv", "/files/child.csv", 1, None);
    }
    let mut session = Session { browser: Some(browser.clone()), ..Default::default() };
    session.prefs.general.language = "en".into();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_step_dt(1.0 / 60.0).build_eframe(|_| {
        let mut app = EffectcraftApp::new(session);
        app.ui.start_screen = false;
        app.show_panel(PanelKind::MediaBrowser);
        app.ui.maximized = Some(PanelKind::MediaBrowser);
        app
    });
    h.run_steps(3);
    let (time, first) = listing(&h, false);
    assert_eq!(names(&first), ["Sub", "Clip.csv", "notes.txt"]);
    assert!(h.state().auto.find("mediaBrowser.entry.Clip.csv").is_some());
    browser.tree.lock().unwrap().add_file("/Shoot/Added.csv", "/files/added.csv", 2, Some(5));
    at(&mut h, time + 0.999);
    assert!(Arc::ptr_eq(&first, &listing(&h, false).1));
    assert!(h.state().auto.find("mediaBrowser.entry.Added.csv").is_none());
    at(&mut h, time + 1.0);
    let refreshed = listing(&h, false).1;
    assert_eq!(names(&refreshed), ["Sub", "Added.csv", "Clip.csv", "notes.txt"]);
    assert!(h.state().auto.find("mediaBrowser.entry.Added.csv").is_some());
    assert_eq!(names(&first), ["Sub", "Clip.csv", "notes.txt"]);
    click(&mut h, "mediaBrowser.importableOnly");
    assert!(h.state().session.state.media_browser.importable_only);
    assert_eq!(names(&listing(&h, true).1), ["Sub", "Added.csv", "Clip.csv"]);
    assert!(h.state().auto.find("mediaBrowser.entry.notes.txt").is_none());
    click(&mut h, "mediaBrowser.importableOnly");
    assert!(!h.state().session.state.media_browser.importable_only);
    assert!(Arc::ptr_eq(&refreshed, &listing(&h, false).1));
    click(&mut h, "mediaBrowser.action.refresh");
    assert_eq!(names(&listing(&h, false).1), ["Sub", "Added.csv", "Clip.csv", "From action.csv", "notes.txt"]);
    assert!(h.state().auto.find("mediaBrowser.entry.From action.csv").is_some());
    assert_eq!(names(&refreshed), ["Sub", "Added.csv", "Clip.csv", "notes.txt"]);
    h.run_steps(40);
    click(&mut h, "mediaBrowser.entry.Sub");
    click(&mut h, "mediaBrowser.entry.Sub");
    assert_eq!(h.state().session.state.media_browser.folder.as_deref(), Some("/Shoot/Sub"));
    assert_eq!(names(&listing(&h, false).1), ["Child.csv"]);
    assert!(h.state().auto.find("mediaBrowser.entry.Child.csv").is_some());
    browser.tree.lock().unwrap().remove("/Shoot/Sub/Child.csv");
    let time = listing(&h, false).0;
    at(&mut h, time + 1.0);
    assert_eq!(names(&listing(&h, false).1), Vec::<&str>::new());
    assert!(h.state().auto.find("mediaBrowser.entry.Child.csv").is_none());
    click(&mut h, "mediaBrowser.up");
    assert_eq!(h.state().session.state.media_browser.folder.as_deref(), Some("/Shoot"));
    assert!(h.state().auto.find("mediaBrowser.entry.Clip.csv").is_some());
    browser.offline.store(true, Ordering::Relaxed);
    let time = listing(&h, false).0;
    at(&mut h, time + 1.0);
    let (time, error) = listing(&h, false);
    assert_eq!(error.as_ref(), &Err("Folder temporarily unavailable".into()));
    assert!(h.output().shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.text() == "Folder temporarily unavailable")));
    assert!(h.state().auto.find("mediaBrowser.entry.Clip.csv").is_none());
    browser.offline.store(false, Ordering::Relaxed);
    browser.tree.lock().unwrap().remove("/Shoot/notes.txt");
    at(&mut h, time + 0.999);
    assert!(Arc::ptr_eq(&error, &listing(&h, false).1));
    at(&mut h, time + 1.0);
    assert_eq!(names(&listing(&h, false).1), ["Sub", "Added.csv", "Clip.csv", "From action.csv"]);
    assert!(h.state().auto.find("mediaBrowser.entry.Clip.csv").is_some());
    if let Ok(path) = std::env::var("MEDIA_BROWSER_SNAPSHOT") {
        h.render_serialized().unwrap().save(path).unwrap();
    }
}
