//! Close, New and Open Project let go of what the closed project decoded (#2042).

use std::sync::{Arc, Mutex};

use effectcraft_project::{Footage, ItemId, ItemKind, Project};
use effectcraft_time::Tick;
use serde_json::json;

use crate::render::{FootageSource, Image};
use crate::{Event, Session};

/// A footage source that records the paths it is told to forget.
#[derive(Default)]
struct Recording(Mutex<Vec<String>>);

impl FootageSource for Recording {
    fn frame(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<Image>> {
        None
    }
    fn forget(&self, path: &str) {
        self.0.lock().unwrap().push(path.to_string());
    }
}

fn with_footage(base: &Project, name: &str, path: &str) -> Project {
    let mut p = base.clone();
    p.add_item(name, Default::default(), None, ItemKind::Footage(Footage { path: path.into(), ..Default::default() }));
    p
}

#[test]
fn replacing_project_releases_old_footage() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 640, "height": 360, "frameRate": 30, "duration": 10})).unwrap();
    let src = Arc::new(Recording::default());
    s.footage = src.clone();
    let recorded = || src.0.lock().unwrap().clone();
    let open_old = |s: &mut Session| {
        let old = with_footage(&s.project, "old", "/old.mp4");
        s.replace_project(old, None);
        s.drain_events();
        src.0.lock().unwrap().clear();
    };

    // Close Project lets go of the closed project's footage and clears the RAM preview.
    open_old(&mut s);
    s.execute("file.closeProject", json!({})).unwrap();
    assert!(recorded().contains(&"/old.mp4".to_string()), "{:?}", recorded());
    assert!(s.drain_events().contains(&Event::PurgeCaches));

    // Opening a project forgets the old project's files and the new project's.
    open_old(&mut s);
    s.replace_project(with_footage(&Project::default(), "new", "/new.mp4"), None);
    let paths = recorded();
    assert!(paths.contains(&"/old.mp4".to_string()) && paths.contains(&"/new.mp4".to_string()), "{paths:?}");
    assert!(s.drain_events().contains(&Event::PurgeCaches));

    // New Project does the same.
    open_old(&mut s);
    s.execute("file.newProject", json!({})).unwrap();
    assert!(recorded().contains(&"/old.mp4".to_string()), "{:?}", recorded());
    assert!(s.drain_events().contains(&Event::PurgeCaches));
}

#[test]
fn closing_a_project_releases_its_proxies() {
    let mut s = Session::default();
    let src = Arc::new(Recording::default());
    s.footage = src.clone();
    let mut p = with_footage(&Project::default(), "old", "/old.mp4");
    let item = p.items.values_mut().next().unwrap();
    let footage = Footage { path: "/old-proxy.mov".into(), ..Default::default() };
    item.proxy = Some(Box::new(effectcraft_project::Proxy { footage, enabled: true }));
    s.replace_project(p, None);
    src.0.lock().unwrap().clear();
    s.execute("file.closeProject", json!({})).unwrap();
    assert!(src.0.lock().unwrap().contains(&"/old-proxy.mov".to_string()), "{:?}", src.0.lock().unwrap());
}

/// A footage source with a memory cache, like the media pool: a frame read keeps its file's
/// entry until the file is forgotten. With a gate, the first read waits for the test.
#[derive(Default)]
struct Caching {
    cached: Mutex<std::collections::HashSet<String>>,
    gate: Mutex<Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>>,
}

impl FootageSource for Caching {
    fn frame(&self, _: ItemId, footage: &Footage, _: Tick) -> Option<Arc<Image>> {
        let gate = self.gate.lock().unwrap().take();
        if let Some((entered, go)) = gate {
            entered.send(()).unwrap();
            go.recv().unwrap();
        }
        self.cached.lock().unwrap().insert(footage.path.clone());
        Some(Arc::new(Image::new(footage.width, footage.height)))
    }
    fn forget(&self, path: &str) {
        self.cached.lock().unwrap().remove(path);
    }
}

/// A session showing `/old.png` (a footage item, and a comp with a layer of it) through a
/// [`Caching`] source: (session, source, footage item, comp).
fn old_project() -> (Session, Arc<Caching>, ItemId, ItemId) {
    let mut s = Session::default();
    let src = Arc::new(Caching::default());
    s.footage = src.clone();
    let still = Footage { path: "/old.png".into(), has_video: true, width: 8, height: 8, ..Default::default() };
    let mut p = Project::default();
    let item = p.add_item("old", Default::default(), None, ItemKind::Footage(still));
    s.replace_project(p, None);
    s.execute("comp.new", json!({"name": "Main", "width": 8, "height": 8, "frameRate": 30, "duration": 1})).unwrap();
    s.execute("layer.addItem", json!({"item": item.0})).unwrap();
    let comp = s.active_comp_id().unwrap();
    (s, src, item, comp)
}

#[test]
fn held_thumbnail_jobs_cannot_refill_the_closed_projects_caches() {
    for close in ["file.closeProject", "file.newProject"] {
        let (mut s, src, item, comp) = old_project();
        // Project panel thumbnails, made while the project is open and run after it closed (a
        // thread that started late).
        let jobs = [s.thumbnail_job(item, Tick(0), 16).unwrap(), s.thumbnail_job(comp, Tick(0), 16).unwrap()];
        s.execute(close, json!({})).unwrap();
        let shown: Vec<_> = jobs.into_iter().map(|job| job()).collect();
        assert!(src.cached.lock().unwrap().is_empty(), "{close}: the closed project's frames came back");
        assert_eq!(s.layer_cache.stats().entries, 0, "{close}: the closed project's layers came back");
        assert!(shown.iter().all(Option::is_none), "{close}: a closed project's thumbnail isn't shown");
    }
}

#[test]
fn thumbnails_reading_through_close_let_go_of_what_they_read() {
    for which in ["footage", "comp"] {
        let (mut s, src, item, comp) = old_project();
        let (entered, entered_rx) = std::sync::mpsc::channel();
        let (go, go_rx) = std::sync::mpsc::channel();
        *src.gate.lock().unwrap() = Some((entered, go_rx));
        let job = s.thumbnail_job(if which == "comp" { comp } else { item }, Tick(0), 16).unwrap();
        let running = std::thread::spawn(job);
        // The job is decoding the closed project's still when the project closes.
        entered_rx.recv_timeout(std::time::Duration::from_secs(30)).unwrap();
        s.execute("file.closeProject", json!({})).unwrap();
        go.send(()).unwrap();
        let shown = running.join().unwrap();
        assert!(src.cached.lock().unwrap().is_empty(), "{which}: the frame decoded through Close was kept");
        assert_eq!(s.layer_cache.stats().entries, 0, "{which}: the closed project's layers were kept");
        assert!(shown.is_none(), "{which}: a closed project's thumbnail isn't shown");
    }
}

#[test]
fn an_open_projects_thumbnails_still_render() {
    let (s, src, item, comp) = old_project();
    let (w, h, px) = s.thumbnail_job(item, Tick(0), 16).unwrap()().unwrap();
    assert_eq!((w, h, px.len()), (8, 8, 8 * 8 * 4));
    assert!(s.thumbnail_job(comp, Tick(0), 16).unwrap()().is_some());
    assert!(src.cached.lock().unwrap().contains("/old.png"));
}
