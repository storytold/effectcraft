//! Relinking footage that moved (docs/footage.md "Replacing and reloading"): Replace Footage on a
//! missing item relinks the other missing items that moved with it, as in After Effects.

use std::path::{Path, PathBuf};

use effectcraft_engine::project::ItemKind;
use serde_json::json;

fn icon() -> Vec<u8> {
    std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/app-icon/hicolor/16x16/apps/ai.storyteller.effectcraft.png")).unwrap()
}

fn footage_paths(s: &effectcraft_engine::Session) -> Vec<(u64, String, bool)> {
    let mut v: Vec<(u64, String, bool)> = s
        .project
        .items
        .values()
        .filter_map(|i| match &i.kind {
            ItemKind::Footage(f) => Some((i.id.0, f.path.clone(), f.missing)),
            _ => None,
        })
        .collect();
    v.sort();
    v
}

#[test]
fn replacing_a_missing_file_relinks_the_others_that_moved_with_it() {
    let root: PathBuf = std::env::temp_dir().join(format!("ec-host-relink-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let old = root.join("Job");
    std::fs::create_dir_all(old.join("stills")).unwrap();
    std::fs::write(old.join("stills/a.png"), icon()).unwrap();
    std::fs::write(old.join("stills/b.png"), icon()).unwrap();
    let mut s = effectcraft_host::session();
    let paths: Vec<String> = ["stills/a.png", "stills/b.png"].iter().map(|p| old.join(p).to_string_lossy().to_string()).collect();
    s.execute("file.import", json!({"paths": paths, "sequence": false})).unwrap();
    let before = footage_paths(&s);
    assert_eq!(before.len(), 2, "{before:?}");
    // The job folder moves; both files go missing.
    let new = root.join("Archive/Job");
    std::fs::create_dir_all(root.join("Archive")).unwrap();
    std::fs::rename(&old, &new).unwrap();
    let r = s.execute("footage.check", json!({"wait": true})).unwrap();
    assert_eq!(r["missing"], 2, "{r}");
    // Locating one finds the other.
    let a = before.iter().find(|(_, p, _)| p.ends_with("a.png")).unwrap().0;
    let r = s.execute("file.replaceFootage", json!({"item": a, "path": new.join("stills/a.png").to_string_lossy()})).unwrap();
    assert_eq!(r["relinked"].as_array().map(Vec::len), Some(1), "{r}");
    let after = footage_paths(&s);
    assert!(after.iter().all(|(_, p, missing)| p.starts_with(&*new.to_string_lossy()) && !missing), "{after:?}");
    // One undo step puts both back.
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(footage_paths(&s).iter().map(|(_, p, _)| p.clone()).collect::<Vec<_>>(), before.iter().map(|(_, p, _)| p.clone()).collect::<Vec<_>>());
    let _ = std::fs::remove_dir_all(root);
}
