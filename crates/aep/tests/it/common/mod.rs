//! The one place the tests reach the generated `.aep` / `.aepx` test projects.
//!
//! Every test addresses a case by its case name, the name of the single-case file it was first
//! written against (`"058-key-opacity-2key.aep"`). The cases now live in a few combined projects
//! (`tests/fixtures/CASES.md`): each case is the composition named like the case without its
//! extension (`058-key-opacity-2key`), built like the single-case file's composition `C` (plus
//! the compositions it uses). [`try_fixture`] hands a test the case as a project of its own: the
//! combined file with every project item removed that the case composition does not use
//! (directly or through precomposition layers), written back with [`effectcraft_aep::write`].
//! The case composition is renamed `C` as in the single-case files; everything else of the file
//! (project settings, the effect definitions, the XMP packet) stays, and the layers and items keep
//! the ids they have in the combined file.
//!
//! A case name that is a file of its own in the directory (an old single-case fixture or a
//! combined file's own name) is read as it is. When the file is not
//! there the test prints `SKIPPED: fixture <name> missing` and returns without failing (the
//! macros below), so the suite runs without the projects. The directory is the one in the
//! `EFFECTCRAFT_AEP_FIXTURES` environment variable (which may also hold the old single-case
//! files); without it every such test skips. The engine tests include this file too.
#![allow(dead_code, unused_macros)]

mod cases;

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use effectcraft_aep::{AepFile, Body, Chunk, ItemKind, Project};

pub use cases::CASES;

/// The directory the test projects are read from, or `None` when `EFFECTCRAFT_AEP_FIXTURES` is
/// not set (the projects are opt-in).
pub fn dir() -> Option<PathBuf> {
    std::env::var_os("EFFECTCRAFT_AEP_FIXTURES").filter(|d| !d.is_empty()).map(PathBuf::from)
}

/// The combined files the cases live in (and the empty project, a file of its own), sorted.
pub const FILES: &[&str] = &[
    "3d.aep",
    "aepx-pair.aep",
    "aepx-pair.aepx",
    "effects.aep",
    "empty.aep",
    "footage-precomp.aep",
    "fx-switch.aep",
    "keyframes.aep",
    "layers-comps.aep",
    "markers.aep",
    "masks.aep",
    "settings-a.aep",
    "settings-b.aep",
    "settings-c.aep",
    "shapes.aep",
    "styles.aep",
    "text.aep",
];

/// Where a case is read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A file of its own, read as it is.
    File(PathBuf),
    /// The composition `comp` of the combined project `file`.
    Case { file: PathBuf, comp: String },
}

impl Source {
    /// The file the case is in.
    pub fn path(&self) -> &PathBuf {
        match self {
            Source::File(p) | Source::Case { file: p, .. } => p,
        }
    }
}

/// The combined file of a case name (`None` for a name that is not a case).
pub fn combined_file(name: &str) -> Option<&'static str> {
    CASES.binary_search_by(|(n, _)| n.cmp(&name)).ok().and_then(|i| CASES.get(i)).map(|(_, f)| *f)
}

/// The name of the case's composition: the case name without its extension.
pub fn case_comp(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(stem, _)| stem)
}

/// Whether `path` is a file. Only a missing path is `false`: any other error (no permission, a
/// broken folder) fails the test, so a fixture that is there but unreadable never turns into a skip.
fn is_fixture(path: &std::path::Path) -> bool {
    match std::fs::metadata(path) {
        Ok(m) if m.is_file() => true,
        Ok(_) => panic!("{}: not a file", path.display()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => panic!("{}: {e}", path.display()),
    }
}

/// Where a case is, or `None` (after printing the skip line) when its file is missing.
pub fn locate(name: &str) -> Option<Source> {
    let Some(dir) = dir() else {
        skipped(name);
        return None;
    };
    let own = dir.join(name);
    if is_fixture(&own) {
        return Some(Source::File(own));
    }
    let Some(file) = combined_file(name) else {
        skipped(name);
        return None;
    };
    let path = dir.join(file);
    if !is_fixture(&path) {
        skipped(name);
        return None;
    }
    // The empty project is a whole file: it has no composition.
    if file == "empty.aep" {
        return Some(Source::File(path));
    }
    Some(Source::Case { file: path, comp: case_comp(name).to_string() })
}

/// The file a case is in (the combined project for a case in one), or `None` when it is missing.
pub fn resolve(name: &str) -> Option<PathBuf> {
    locate(name).map(|s| s.path().clone())
}

/// The bytes of a case (see the module comment), or `None` when it is missing.
pub fn try_fixture(name: &str) -> Option<Vec<u8>> {
    match locate(name)? {
        Source::File(path) => Some(std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))),
        Source::Case { file, comp } => Some(case_bytes(&file, &comp).as_ref().clone()),
    }
}

/// The bytes of a whole file of the directory (a combined project), or `None` when it is missing.
pub fn try_file(file: &str) -> Option<Vec<u8>> {
    let path = dir()?.join(file);
    if is_fixture(&path) {
        Some(std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
    } else {
        skipped(file);
        None
    }
}

/// The case names with this extension (sorted), or `None` when there are none: the cases of
/// [`CASES`] whose file (of its own or combined) is there, else the files with that extension.
pub fn names(ext: &str) -> Option<Vec<String>> {
    let Some(dir) = dir() else {
        skipped(&format!("*.{ext}"));
        return None;
    };
    let mut v: Vec<String> = CASES
        .iter()
        .filter(|(n, f)| std::path::Path::new(n).extension().is_some_and(|x| x == ext) && (is_fixture(&dir.join(n)) || is_fixture(&dir.join(f))))
        .map(|(n, _)| n.to_string())
        .collect();
    if v.is_empty() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries.map(|e| e.unwrap_or_else(|e| panic!("{}: {e}", dir.display()))).collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => panic!("{}: {e}", dir.display()),
        };
        v = entries
            .into_iter()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| std::path::Path::new(n).extension().is_some_and(|x| x == ext))
            .collect();
    }
    v.sort();
    if v.is_empty() {
        skipped(&format!("*.{ext}"));
        None
    } else {
        Some(v)
    }
}

pub fn skipped(name: &str) {
    println!("SKIPPED: fixture {name} missing");
}

/// A parsed combined file, read once per test binary.
fn parsed(file: &PathBuf) -> &'static AepFile<'static> {
    static FILES: OnceLock<Mutex<HashMap<PathBuf, &'static AepFile<'static>>>> = OnceLock::new();
    let mut files = FILES.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    files.entry(file.clone()).or_insert_with(|| {
        let bytes: &'static [u8] = Box::leak(std::fs::read(file).unwrap_or_else(|e| panic!("{}: {e}", file.display())).into_boxed_slice());
        Box::leak(Box::new(effectcraft_aep::parse_any(bytes).unwrap_or_else(|e| panic!("{}: {e}", file.display()))))
    })
}

/// The single-case project of composition `comp` in `file`, made once per test binary.
fn case_bytes(file: &PathBuf, comp: &str) -> Arc<Vec<u8>> {
    static CASES_MADE: OnceLock<Mutex<HashMap<(PathBuf, String), Arc<Vec<u8>>>>> = OnceLock::new();
    let key = (file.clone(), comp.to_string());
    if let Some(b) = CASES_MADE.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return b.clone();
    }
    let bytes = Arc::new(isolate(parsed(file), comp).unwrap_or_else(|e| panic!("{}: case {comp}: {e}", file.display())));
    CASES_MADE.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner()).insert(key, bytes.clone());
    bytes
}

/// The item id of an `Item` list (`idta` byte 16).
fn item_id(item: &Chunk<'_>) -> Option<u32> {
    let idta = item.child(b"idta")?.bytes()?;
    Some(u32::from_be_bytes(idta.get(16..20)?.try_into().ok()?))
}

/// The cases that import a footage file without using it in their composition, and how to find
/// that item: it stays with the case.
const UNUSED_FOOTAGE: &[(&str, &str, fn(&effectcraft_aep::Item<'_, '_>) -> bool)] = &[
    ("350-footage-wav-imported", "the WAV", |i| i.footage().and_then(|f| f.path).is_some_and(|p| p.ends_with("test-tone.wav"))),
    ("352-footage-pngseq-imported", "the png sequence", |i| i.sequence_info().is_some_and(|s| s.prefix == "seq_" && s.suffix == ".png")),
];

/// `file` with only the composition `comp` and the project items it uses (layer sources,
/// transitively), and the folders that hold them; the composition is renamed `C`.
pub fn isolate(file: &AepFile<'_>, comp: &str) -> Result<Vec<u8>, String> {
    let project = Project::new(file).ok_or("no project")?;
    let items = project.all_items();
    let case =
        items.iter().find(|i| i.kind() == Some(ItemKind::Composition) && i.name().as_deref() == Some(comp)).ok_or_else(|| format!("no composition {comp}"))?;
    let mut keep = BTreeSet::new();
    let mut todo = vec![case.id().ok_or("case without id")?];
    for (_, what, is) in UNUSED_FOOTAGE.iter().filter(|(c, _, _)| *c == comp) {
        todo.push(items.iter().find(|i| is(i)).and_then(|i| i.id()).ok_or_else(|| format!("no item {what}"))?);
    }
    while let Some(id) = todo.pop() {
        if !keep.insert(id) {
            continue;
        }
        let Some(item) = items.iter().find(|i| i.id() == Some(id)) else { continue };
        if let Some(c) = item.comp() {
            todo.extend(c.layers().filter_map(|l| l.header()?.source_id));
        }
    }
    let mut out = file.clone();
    let fold = out.chunks.iter_mut().find(|c| c.is_list(b"Fold")).ok_or("no LIST Fold")?;
    prune(fold, &keep);
    rename(fold, case.id().ok_or("case without id")?, "C");
    effectcraft_aep::write(&out).map_err(|e| e.to_string())
}

/// Removes the items of `list` (and of its folders) that are not kept; a folder stays while it
/// holds a kept item.
fn prune(list: &mut Chunk<'_>, keep: &BTreeSet<u32>) {
    let Body::List { children, .. } = &mut list.body else { return };
    children.retain_mut(|c| {
        if !c.is_list(b"Item") {
            return true;
        }
        if let Body::List { children: inner, .. } = &mut c.body
            && let Some(sfdr) = inner.iter_mut().find(|x| x.is_list(b"Sfdr"))
        {
            prune(sfdr, keep);
            let holds = sfdr.children().iter().any(|x| x.is_list(b"Item"));
            return holds || item_id(c).is_some_and(|id| keep.contains(&id));
        }
        item_id(c).is_some_and(|id| keep.contains(&id))
    });
}

/// Renames the item with this id (searching folders too).
fn rename(list: &mut Chunk<'_>, id: u32, name: &str) -> bool {
    let Body::List { children, .. } = &mut list.body else { return false };
    for c in children.iter_mut().filter(|c| c.is_list(b"Item")) {
        if item_id(c) == Some(id) {
            let Body::List { children: inner, .. } = &mut c.body else { return false };
            let Some(utf8) = inner.iter_mut().find(|x| x.is(b"Utf8")) else { return false };
            *utf8 = Chunk::data(*b"Utf8", name.as_bytes().to_vec());
            return true;
        }
        if let Body::List { children: inner, .. } = &mut c.body
            && let Some(sfdr) = inner.iter_mut().find(|x| x.is_list(b"Sfdr"))
            && rename(sfdr, id, name)
        {
            return true;
        }
    }
    false
}

/// The bytes of a case; returns from the calling test when it is missing.
macro_rules! fixture {
    ($name:expr) => {
        match $crate::common::try_fixture(&$name) {
            Some(bytes) => bytes,
            None => return,
        }
    };
}

/// The path of a case's file; returns from the calling test when it is missing.
macro_rules! fixture_path {
    ($name:expr) => {
        match $crate::common::resolve(&$name) {
            Some(path) => path,
            None => return,
        }
    };
}

/// The case names with this extension; returns from the calling test when there are none.
macro_rules! fixture_names {
    ($ext:expr) => {
        match $crate::common::names($ext) {
            Some(names) => names,
            None => return,
        }
    };
}

/// The value of a helper that returns `None` for a missing case; returns from the calling test
/// then (the helper has printed the skip line).
macro_rules! skip {
    ($e:expr) => {
        match $e {
            Some(value) => value,
            None => return,
        }
    };
}
