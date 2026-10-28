//! The one place the tests reach the generated `.aep` / `.aepx` test projects.
//!
//! Every test addresses a case by its case name (`"058-key-opacity-2key.aep"`). [`resolve`] maps
//! the name to a file; when the file is not there the test prints `SKIPPED: fixture <name>
//! missing` and returns without failing (the macros below), so the suite runs without the
//! projects. The directory is `tests/fixtures` of this crate, or the one in the
//! `EFFECTCRAFT_AEP_FIXTURES` environment variable.
#![allow(dead_code, unused_macros)]

use std::path::PathBuf;

/// The directory the test projects are read from.
pub fn dir() -> PathBuf {
    match std::env::var_os("EFFECTCRAFT_AEP_FIXTURES") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => [env!("CARGO_MANIFEST_DIR"), "tests", "fixtures"].iter().collect(),
    }
}

/// The file of a case, or `None` (after printing the skip line) when it is missing.
pub fn resolve(name: &str) -> Option<PathBuf> {
    let path = dir().join(name);
    if path.is_file() {
        Some(path)
    } else {
        skipped(name);
        None
    }
}

/// The bytes of a case, or `None` when it is missing.
pub fn try_fixture(name: &str) -> Option<Vec<u8>> {
    let path = resolve(name)?;
    Some(std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
}

/// The case names with this extension (sorted), or `None` when there are none.
pub fn names(ext: &str) -> Option<Vec<String>> {
    let mut v: Vec<String> = std::fs::read_dir(dir())
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| std::path::Path::new(n).extension().is_some_and(|x| x == ext))
        .collect();
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

/// The bytes of a case; returns from the calling test when it is missing.
macro_rules! fixture {
    ($name:expr) => {
        match $crate::common::try_fixture(&$name) {
            Some(bytes) => bytes,
            None => return,
        }
    };
}

/// The path of a case; returns from the calling test when it is missing.
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
