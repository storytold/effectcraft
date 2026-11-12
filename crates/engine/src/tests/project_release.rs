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
