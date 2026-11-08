//! #3271: Project panel thumbnails are keyed by what they show, not by the project's revision:
//! an edit elsewhere keeps a comp's thumbnail, an edit of the comp, another time or size, and a
//! footage reload give it a new key.

use effectcraft_time::Tick;
use serde_json::json;

use crate::Session;

#[test]
fn thumbnail_keys_follow_the_content() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "A", "width": 64, "height": 32, "frameRate": 24, "duration": 2})).unwrap();
    let a = s.active_comp_id().unwrap();
    s.execute("layer.newSolid", json!({"name": "S", "color": "#ff8000"})).unwrap();
    let key = |s: &Session| s.thumbnail_key(a, Tick::ZERO, 192).unwrap();
    let k0 = key(&s);
    // Another comp: the project changed, this comp's picture didn't.
    s.execute("comp.new", json!({"name": "B", "width": 16, "height": 16, "frameRate": 24, "duration": 1})).unwrap();
    assert_eq!(key(&s), k0, "an edit elsewhere keeps the key");
    assert_ne!(s.thumbnail_key(a, Tick::from_seconds_f64(0.5), 192).unwrap(), k0, "another time");
    assert_ne!(s.thumbnail_key(a, Tick::ZERO, 96).unwrap(), k0, "another size");
    // An edit of the comp itself.
    std::sync::Arc::make_mut(&mut s.project).comp_mut(a).unwrap().background = [0.2, 0.4, 0.6];
    assert_ne!(key(&s), k0, "an edit of the comp gives a new key");
    // A reload: files may have changed without the project knowing.
    let before = key(&s);
    s.forget_decoded(&effectcraft_project::Footage::default());
    assert_ne!(key(&s), before, "a reload gives a new key");
}

/// #3271 review: File ▸ Reload Footage gives the footage's thumbnail a new key (the file may
/// have changed without the project knowing).
#[test]
fn reload_footage_gives_thumbnails_a_new_key() {
    struct Probe;
    impl crate::Importer for Probe {
        fn probe(&self, path: &str) -> Result<effectcraft_project::Footage, String> {
            Ok(effectcraft_project::Footage {
                path: path.into(),
                kind: effectcraft_project::FootageKind::Still,
                width: 1,
                height: 1,
                has_video: true,
                ..Default::default()
            })
        }
    }
    let dir = std::env::temp_dir().join(format!("ec-3271-thumb-reload-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("a.png");
    std::fs::write(&file, b"aa").unwrap();
    let mut s = Session { importer: Some(std::sync::Arc::new(Probe)), ..Default::default() };
    let f = effectcraft_project::Footage {
        path: file.to_string_lossy().to_string(),
        kind: effectcraft_project::FootageKind::Still,
        width: 1,
        height: 1,
        has_video: true,
        ..Default::default()
    };
    let id = s.edit("add", None, |p, _| Ok(p.add_item("a", effectcraft_color::Label::None, None, effectcraft_project::ItemKind::Footage(f)))).unwrap();
    let before = s.thumbnail_key(id, Tick::ZERO, 192).unwrap();
    s.execute_checked("file.reloadFootage", json!({"item": id.0})).unwrap();
    assert_ne!(s.thumbnail_key(id, Tick::ZERO, 192).unwrap(), before);
    let _ = std::fs::remove_dir_all(&dir);
}
