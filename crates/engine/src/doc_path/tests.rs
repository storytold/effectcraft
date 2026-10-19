use std::sync::{Arc, Mutex};

use effectcraft_project::{Footage, ItemId, ItemKind};
use serde_json::json;

use crate::{Event, Services, Session};

/// File access that records every path it is asked about.
#[derive(Default)]
struct Recording(Mutex<Vec<String>>);

impl Recording {
    fn note(&self, path: &str) {
        self.0.lock().unwrap().push(path.to_string());
    }
}

impl Services for Recording {
    fn read_file(&self, path: &str) -> std::io::Result<Vec<u8>> {
        self.note(path);
        std::fs::read(path)
    }
    fn write_file(&self, path: &str, data: &[u8]) -> std::io::Result<()> {
        std::fs::write(path, data)
    }
    fn exists(&self, path: &str) -> bool {
        self.note(path);
        std::path::Path::new(path).exists()
    }
    fn stamp(&self, path: &str) -> Option<(u64, u128)> {
        self.note(path);
        let meta = std::fs::metadata(path).ok()?;
        Some((meta.len(), meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos()))
    }
}

/// Probes every file as a still.
struct Stills;

impl crate::Importer for Stills {
    fn probe(&self, path: &str) -> Result<Footage, String> {
        if !std::path::Path::new(path).is_file() {
            return Err(format!("{path}: no such file"));
        }
        Ok(Footage { path: path.into(), kind: effectcraft_project::FootageKind::Still, width: 64, height: 32, has_video: true, ..Default::default() })
    }
}

fn footage_id(s: &Session, name: &str) -> ItemId {
    s.project.items.values().find(|i| i.name == name).unwrap().id
}

fn footage<'a>(s: &'a Session, name: &str) -> &'a Footage {
    match &s.project.item(footage_id(s, name)).unwrap().kind {
        ItemKind::Footage(f) => f,
        _ => panic!("{name} is not footage"),
    }
}

/// Footage paths to other computers stored in a project are never looked at: not on open, by
/// the footage check, by the changed-on-disk scan, by Edit Original or by Reveal in Finder. A
/// relative path is taken from the project's folder, and a file the user imports in this session
/// is used as given until the project is saved and opened again.
#[test]
fn footage_on_other_computers_is_never_looked_at() {
    let root = std::env::temp_dir().join(format!("ec-doc-path-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let dir = root.join("job");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(root.join("media")).unwrap();
    std::fs::write(dir.join("clip.png"), b"x").unwrap();
    std::fs::write(root.join("media").join("far.png"), b"x").unwrap();
    let mut stored = vec![("unc", r"\\192.0.2.1\share\x.png".to_string()), ("url", "file://192.0.2.1/s/x.png".to_string()), ("beside", "clip.png".to_string())];
    // Linux reads `//tmp/…/far.png` as the local file: a sink that skipped the check would find it.
    if cfg!(unix) {
        stored.push(("slashes", format!("/{}", root.join("media").join("far.png").display())));
    }
    let mut s = Session::default();
    for (name, path) in &stored {
        let f = Footage { path: path.clone(), width: 64, height: 48, has_video: true, ..Default::default() };
        Arc::make_mut(&mut s.project).add_item(name, crate::color::Label::None, None, ItemKind::Footage(f));
    }
    let project = dir.join("p.ecproj");
    s.execute("file.saveAs", json!({"path": project.to_string_lossy()})).unwrap();

    let rec = Arc::new(Recording::default());
    let mut o = Session { services: rec.clone(), ..Default::default() };
    o.execute("file.open", json!({"path": project.to_string_lossy()})).unwrap();
    for (name, _) in &stored {
        assert_eq!(footage(&o, name).missing, *name != "beside", "{name}");
    }
    assert_eq!(std::path::Path::new(&footage(&o, "beside").path), dir.join("clip.png"));
    let r = o.execute("footage.check", json!({"wait": true})).unwrap();
    assert_eq!(r["missing"].as_u64(), Some(stored.len() as u64 - 1), "{r}");
    o.execute("footage.reloadChanged", json!({})).unwrap();
    o.drain_events();
    for (name, _) in &stored {
        o.state.project_selection = vec![footage_id(&o, name)];
        let opened = o.execute("edit.editOriginal", json!({}));
        let revealed = o.execute("file.revealInFinder", json!({}));
        assert_eq!((opened.is_ok(), revealed.is_ok()), (*name == "beside", *name == "beside"), "{name}: {opened:?} {revealed:?}");
    }
    let urls: Vec<String> = o.drain_events().into_iter().filter_map(|e| if let Event::OpenUrl(u) = e { Some(u) } else { None }).collect();
    assert_eq!(urls.len(), 2, "{urls:?}");

    let seen = rec.0.lock().unwrap().clone();
    assert!(seen.iter().any(|p| std::path::Path::new(p) == dir.join("clip.png")), "{seen:?}");
    // Only files in the temporary folder (the project's folder, where moved footage is looked for).
    let local = root.to_string_lossy().into_owned();
    for p in &seen {
        assert!(p.starts_with(&local), "looked at {p}");
    }

    // Chosen in this session, the same file is followed as given (as a file on a NAS would be)…
    #[cfg(unix)]
    {
        let far = stored.iter().find(|(n, _)| *n == "slashes").unwrap().1.clone();
        o.importer = Some(Arc::new(Stills));
        o.execute("file.import", json!({"paths": [far.clone()]})).unwrap();
        let imported = |s: &Session| -> Vec<ItemId> {
            s.project.items.values().filter(|i| matches!(&i.kind, ItemKind::Footage(f) if f.path == far)).map(|i| i.id).collect()
        };
        let id = *imported(&o).iter().find(|id| matches!(&o.project.item(**id).unwrap().kind, ItemKind::Footage(f) if !f.missing)).unwrap();
        o.state.project_selection = vec![id];
        o.execute("edit.editOriginal", json!({})).unwrap();
        o.execute("file.revealInFinder", json!({})).unwrap();
        assert!(o.execute("footage.check", json!({"wait": true, "items": [id.0]})).unwrap()["missing"] == 0);
        // …and is a stored path again once the project is saved and opened.
        o.execute("file.save", json!({})).unwrap();
        let mut q = Session::default();
        q.execute("file.open", json!({"path": project.to_string_lossy()})).unwrap();
        let ids = imported(&q);
        assert_eq!(ids.len(), 2);
        for id in ids {
            assert!(matches!(&q.project.item(id).unwrap().kind, ItemKind::Footage(f) if f.missing));
            q.state.project_selection = vec![id];
            assert!(q.execute("edit.editOriginal", json!({})).is_err());
            assert!(q.execute("file.revealInFinder", json!({})).is_err());
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn stored_paths_resolve_against_the_project() {
    let doc = std::env::temp_dir().join("art").join("intro.ecproj");
    let doc = doc.to_str().unwrap();
    assert_eq!(super::resolve("clip.mov", Some(doc)), Ok(std::env::temp_dir().join("art").join("clip.mov")));
    assert_eq!(super::resolve("clip.mov", None), Err(super::NotFollowed::NoProjectFolder));
    assert_eq!(super::resolve(r"\\192.0.2.1\share\x.mov", Some(doc)), Err(super::NotFollowed::Refused(super::Refused::Network)));
    #[cfg(windows)]
    assert!(super::resolve(r"\\server\share\x.mov", Some(r"\\SERVER\share\job\intro.ecproj")).is_ok());
}
