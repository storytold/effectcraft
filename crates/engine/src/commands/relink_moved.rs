//! Relinking footage that moved together (gaps.md G4; After Effects' Replace Footage on a missing
//! file, which then finds the other missing files in the same place). Searching a folder for
//! compatible files is `file.relinkFootage` ([`super::relink`]).
//!
//! - `footage.relink {item, path}`: point a footage item at `path`, keeping its interpretation, and
//!   relink every other missing item that sits in the same place relative to it (the folder moved,
//!   or a drive was renamed). One undo step. Replace Footage on a missing item does the same.
//! - `footage.missing`: the missing items.
//! - When a project opens from a folder it wasn't saved in, the footage check ([`moved_candidates`])
//!   tries each missing file at the same place under the project's new folder.
//!
//! Paths are compared by their parts, split on `/` and `\`, so a project made on Windows relinks on
//! macOS and the other way round.

use effectcraft_project::{Footage, FootageKind, ItemId, ItemKind, Project};
use serde_json::{Value, json};

use super::{CommandSpec, bad, str_p};
use crate::{Result, Services, Session, cmd};

/// Trailing parts of a path tried under a moved project's folder.
const MOVED_PARTS: usize = 4;

fn parts(path: &str) -> Vec<&str> {
    path.split(['/', '\\']).filter(|p| !p.is_empty()).collect()
}

/// The separator `path` uses (Windows paths keep theirs).
fn sep_of(path: &str) -> char {
    if path.contains('\\') && !path.contains('/') { '\\' } else { '/' }
}

fn file_name(path: &str) -> &str {
    parts(path).last().copied().unwrap_or("")
}

/// `prefix` and `rest` joined with `prefix`'s separator.
fn join(prefix: &str, rest: &[&str]) -> String {
    let sep = sep_of(prefix);
    let mut out = prefix.trim_end_matches(['/', '\\']).to_string();
    for p in rest {
        out.push(sep);
        out.push_str(p);
    }
    out
}

/// What moved: `old` became `new`. The parts they end with in common are the same file's place
/// inside the moved folder; what comes before is the folder's old and new location.
/// `("/a/b/shots/x.mov", "/c/shots/x.mov")` → `(["a", "b"], "/c")`.
fn moved_root<'a>(old: &'a str, new: &str) -> Option<(Vec<&'a str>, String)> {
    let (o, n) = (parts(old), parts(new));
    let common = o.iter().rev().zip(n.iter().rev()).take_while(|(a, b)| a == b).count();
    // At least the file name must match, else it's a different file (then only its folder moved).
    let common = common.max(1).min(o.len()).min(n.len());
    let old_root: Vec<&str> = o.get(..o.len() - common)?.to_vec();
    // The new root keeps a leading `/` or a drive letter as written.
    let new_parts = n.len() - common;
    let mut new_root = String::new();
    if new.starts_with('/') {
        new_root.push('/');
    } else if new.starts_with("\\\\") {
        new_root.push_str("\\\\");
    }
    let sep = sep_of(new);
    for (i, p) in n.iter().take(new_parts).enumerate() {
        if i > 0 {
            new_root.push(sep);
        }
        new_root.push_str(p);
    }
    Some((old_root, new_root))
}

/// `path` under the moved root, when it was under the old one.
fn remap(path: &str, old_root: &[&str], new_root: &str) -> Option<String> {
    let p = parts(path);
    if p.len() <= old_root.len() || p.get(..old_root.len())? != old_root {
        return None;
    }
    Some(join(new_root, p.get(old_root.len()..)?))
}

/// The item's file and, for an image sequence, every frame, moved by `f`; `None` unless its main
/// file maps.
fn moved_footage(footage: &Footage, f: impl Fn(&str) -> Option<String>) -> Option<Footage> {
    let mut nf = footage.clone();
    nf.path = f(&footage.path)?;
    nf.sequence = footage.sequence.iter().map(|p| f(p).unwrap_or_else(|| p.clone())).collect();
    nf.missing = false;
    Some(nf)
}

fn present(s: &dyn Services, f: &Footage) -> bool {
    !f.path.is_empty() && s.exists(&f.path)
}

/// Footage items whose file is gone (flagged by the footage check, or not there now).
fn missing_items(s: &Session) -> Vec<(ItemId, Footage)> {
    s.project
        .items
        .values()
        .filter_map(|i| match &i.kind {
            ItemKind::Footage(f) if f.kind != FootageKind::Data && !f.path.is_empty() && (f.missing || !present(s.services.as_ref(), f)) => {
                Some((i.id, f.clone()))
            }
            _ => None,
        })
        .collect()
}

/// Where a file a moved project lost may be now: the same place under the project's new folder,
/// trying the longest tail of its old path first (`/old/proj/shots/a.mov` with the project now in
/// `/new/proj` → `/new/proj/old/proj/shots/a.mov`, …, `/new/proj/shots/a.mov`, `/new/proj/a.mov`).
pub(crate) fn moved_candidates(project_dir: &str, path: &str) -> Vec<String> {
    let p = parts(path);
    (1..=MOVED_PARTS.min(p.len())).rev().filter_map(|k| p.get(p.len() - k..)).map(|tail| join(project_dir, tail)).collect()
}

/// A missing item found again under a moved project's folder ([`moved_candidates`]), with its
/// sequence frames moved alongside.
pub(crate) fn find_moved(services: &dyn Services, project_dir: &str, f: &Footage) -> Option<Footage> {
    moved_candidates(project_dir, &f.path).into_iter().filter(|c| services.exists(c)).find_map(|hit| {
        let (old_root, new_root) = moved_root(&f.path, &hit)?;
        moved_footage(f, |q| if q == f.path { Some(hit.clone()) } else { remap(q, &old_root, &new_root) })
    })
}

/// The folder a project file is in.
pub(crate) fn project_dir(project_path: &str) -> Option<String> {
    let p = parts(project_path);
    if p.len() < 2 {
        return None;
    }
    let mut dir = join(if project_path.starts_with('/') { "/" } else { "" }, p.get(..p.len() - 1)?);
    if !project_path.starts_with('/') {
        dir = dir.trim_start_matches(['/', '\\']).to_string();
    }
    Some(dir)
}

/// Relink `found` in one undo step and say how many.
fn apply(s: &mut Session, label: &str, found: Vec<(ItemId, Footage)>) -> Result<Value> {
    if found.is_empty() {
        return Ok(json!({"relinked": [], "stillMissing": missing_items(s).len()}));
    }
    let ids: Vec<u64> = found.iter().map(|(id, _)| id.0).collect();
    s.edit(label, None, |proj: &mut Project, _| {
        for (id, nf) in found {
            if let Some(it) = proj.item_mut(id) {
                it.kind = ItemKind::Footage(nf);
            }
        }
        Ok(())
    })?;
    s.events.push(crate::Event::PurgeCaches);
    let still = missing_items(s).len();
    s.toast(match still {
        0 => format!("Relinked {} footage item(s)", ids.len()),
        n => format!("Relinked {} footage item(s); {n} still missing", ids.len()),
    });
    Ok(json!({"relinked": ids, "stillMissing": still}))
}

/// The other missing items that moved the way `item` did (`old_path` → `new_path`): the same place
/// under the moved folder, else the same name in the new file's folder. Used by `footage.relink`
/// and by Replace Footage on a missing item.
pub(crate) fn moved_with(s: &Session, item: ItemId, old_path: &str, new_path: &str) -> Vec<(ItemId, Footage)> {
    let Some((old_root, new_root)) = moved_root(old_path, new_path) else { return vec![] };
    let services = s.services.clone();
    let new_dir = project_dir(new_path);
    let mut found = vec![];
    for (id, f) in missing_items(s) {
        if id == item {
            continue;
        }
        let same_place = moved_footage(&f, |q| remap(q, &old_root, &new_root)).filter(|nf| present(services.as_ref(), nf));
        let same_folder = || {
            let dir = new_dir.as_deref()?;
            let dir_parts = parts(dir);
            let old_dir_len = parts(&f.path).len().checked_sub(1)?;
            let old_dir: Vec<&str> = parts(&f.path).get(..old_dir_len)?.to_vec();
            moved_footage(&f, |q| remap(q, &old_dir, dir).or_else(|| Some(join(dir, &[file_name(q)])))).filter(|nf| {
                // Only frames in that one folder (a name match, not a guess deeper down).
                present(services.as_ref(), nf) && parts(&nf.path).len() == dir_parts.len() + 1
            })
        };
        if let Some(nf) = same_place.or_else(same_folder) {
            found.push((id, nf));
        }
    }
    found
}

/// `footage.relink {item, path}`
fn relink(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "footage.relink";
    let path = str_p(p, "path").filter(|p| !p.trim().is_empty()).ok_or_else(|| bad(CMD, "missing `path` (the file's new location)"))?.to_string();
    let item = p.get("item").and_then(Value::as_u64).map(ItemId).ok_or_else(|| bad(CMD, "missing `item` (a footage item id)"))?;
    let Some(ItemKind::Footage(old)) = s.project.item(item).map(|i| &i.kind) else { return Err(bad(CMD, "`item` is not a footage item")) };
    let old = old.clone();
    if !s.services.exists(&path) {
        return Err(bad(CMD, format!("there is no file at {path}")));
    }
    let (old_root, new_root) = moved_root(&old.path, &path).ok_or_else(|| bad(CMD, "cannot compare the old and new paths"))?;
    // The item itself: its file is `path`; its sequence frames move with it.
    let main =
        moved_footage(&old, |q| if q == old.path { Some(path.clone()) } else { remap(q, &old_root, &new_root) }).ok_or_else(|| bad(CMD, "cannot relink"))?;
    let mut found = vec![(item, main)];
    found.extend(moved_with(s, item, &old.path, &path));
    apply(s, "Relink Footage", found)
}

/// `footage.missing`: the missing footage items and where they were.
fn missing(s: &mut Session, _: &Value) -> Result<Value> {
    let list: Vec<Value> = missing_items(s)
        .into_iter()
        .map(|(id, f)| json!({"item": id.0, "name": s.project.item(id).map(|i| i.name.clone()).unwrap_or_default(), "path": f.path}))
        .collect();
    Ok(json!({"missing": list}))
}

fn has_footage(s: &Session) -> std::result::Result<(), String> {
    if s.project.items.values().any(|i| matches!(i.kind, ItemKind::Footage(_))) { Ok(()) } else { Err("the project has no footage".into()) }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "footage.relink",
            "Relink Footage",
            [],
            None,
            "{item, path} — point a footage item at its file's new location (keeps the interpretation) and relink the other missing files found in the same place",
            has_footage,
            relink
        ),
        crate::query!("footage.missing", "Missing Footage", "{} → {missing: [{item, name, path}]}", missing),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moved_roots_and_remaps() {
        let (old, new) = moved_root("/a/b/shots/x.mov", "/c/shots/x.mov").unwrap();
        assert_eq!((old, new.as_str()), (vec!["a", "b"], "/c"));
        assert_eq!(remap("/a/b/shots/y.mov", &["a", "b"], "/c").as_deref(), Some("/c/shots/y.mov"));
        assert_eq!(remap("/a/z/y.mov", &["a", "b"], "/c"), None);
        // Windows to macOS.
        let (old, new) = moved_root("D:\\Work\\Job\\audio\\m.wav", "/Volumes/Work/Job/audio/m.wav").unwrap();
        assert_eq!((old, new.as_str()), (vec!["D:"], "/Volumes"));
        assert_eq!(remap("D:\\Work\\Job\\img\\a.png", &["D:"], "/Volumes").as_deref(), Some("/Volumes/Work/Job/img/a.png"));
        // A different file name: only the folder moved.
        let (old, new) = moved_root("/a/x.mov", "/b/y.mov").unwrap();
        assert_eq!((old, new.as_str()), (vec!["a"], "/b"));
        assert_eq!(
            moved_candidates("/new/p", "/old/p/shots/a.mov"),
            ["/new/p/old/p/shots/a.mov", "/new/p/p/shots/a.mov", "/new/p/shots/a.mov", "/new/p/a.mov"]
        );
        assert_eq!(project_dir("/x/y/p.ecproj").as_deref(), Some("/x/y"));
        assert_eq!(project_dir("p.ecproj"), None);
        // Hostile input doesn't panic.
        for (a, b) in [("", ""), ("/", "/"), ("x", ""), ("\\\\", "//"), ("/a", "/a/b/c")] {
            let _ = moved_root(a, b);
            let _ = moved_candidates(b, a);
            let _ = project_dir(a);
        }
    }

    fn temp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("ec-relink-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn add(s: &mut Session, path: &str) -> ItemId {
        let f = Footage { path: path.into(), width: 8, height: 8, has_video: true, missing: true, ..Default::default() };
        std::sync::Arc::make_mut(&mut s.project).add_item(file_name(path), crate::color::Label::None, None, ItemKind::Footage(f))
    }

    fn path_of(s: &Session, id: ItemId) -> String {
        match s.project.item(id).map(|i| &i.kind) {
            Some(ItemKind::Footage(f)) => f.path.clone(),
            _ => String::new(),
        }
    }

    #[test]
    fn relinking_one_file_finds_the_others_that_moved_with_it() {
        let d = temp("one");
        let new = d.join("moved");
        std::fs::create_dir_all(new.join("shots")).unwrap();
        std::fs::create_dir_all(new.join("audio")).unwrap();
        for f in ["shots/a.mov", "shots/b.mov", "audio/m.wav"] {
            std::fs::write(new.join(f), b"x").unwrap();
        }
        let mut s = Session::default();
        let a = add(&mut s, "/gone/proj/shots/a.mov");
        let b = add(&mut s, "/gone/proj/shots/b.mov");
        let m = add(&mut s, "/gone/proj/audio/m.wav");
        let other = add(&mut s, "/elsewhere/c.mov");
        let target = new.join("shots/a.mov").to_string_lossy().to_string();
        let r = s.execute("footage.relink", json!({"item": a.0, "path": target})).unwrap();
        assert_eq!(r["relinked"], json!([a.0, b.0, m.0]), "{r}");
        assert_eq!(r["stillMissing"], 1);
        assert_eq!(path_of(&s, m), new.join("audio/m.wav").to_string_lossy());
        assert_eq!(path_of(&s, other), "/elsewhere/c.mov");
        // One undo step brings every path back.
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(path_of(&s, b), "/gone/proj/shots/b.mov");
        // Errors: no such file, not footage.
        assert!(s.execute("footage.relink", json!({"item": a.0, "path": "/nope/a.mov"})).is_err());
        assert!(s.execute("footage.relink", json!({"item": 999_999, "path": target})).is_err());
        let _ = std::fs::remove_dir_all(d);
    }
}
