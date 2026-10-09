//! Image sequences on import (File ▸ Import ▸ File…, dropped files and folders, Replace
//! Footage, Reload, proxies): numbered stills of one run (`shot_0001.png`, `shot_0002.png`, …)
//! become one footage item, as with After Effects' "PNG Sequence" import option.
//!
//! - One picked numbered file brings its whole run from its folder ([`crate::Services::list_dir`]).
//! - Several picked files of one run import as one sequence of just those files (a range).
//! - A picked or dropped folder imports its files; each run in it becomes a sequence.
//! - Force Alphabetical Order: every image of the same type in the folder, in name order,
//!   whatever the numbering (gaps don't count).
//!
//! The browser has no folders: its file table lists the files picked or dropped so far, so
//! picking every frame of a sequence imports it there too.

use effectcraft_project::sequence::{frame_number, missing_frame_ranges, sequence_name, split_frame_number};
use effectcraft_project::{Footage, FootageKind, MissingFrames};
use effectcraft_time::FrameRate;

use crate::{Importer, Services};

/// How an import treats numbered stills.
#[derive(Clone, Copy, Debug)]
pub struct SequenceOptions {
    /// The "<format> Sequence" checkbox: numbered stills of one run import as a sequence.
    pub sequence: bool,
    /// Force Alphabetical Order.
    pub alphabetical: bool,
}

impl Default for SequenceOptions {
    fn default() -> Self {
        SequenceOptions { sequence: true, alphabetical: false }
    }
}

impl SequenceOptions {
    /// From command parameters `sequence?` (default on) and `alphabetical?`.
    pub fn from_params(p: &serde_json::Value) -> SequenceOptions {
        let flag = |k: &str| p.get(k).and_then(serde_json::Value::as_bool);
        SequenceOptions { sequence: flag("sequence").unwrap_or(true), alphabetical: flag("alphabetical").unwrap_or(false) }
    }
}

/// What one import unit makes: a file's footage, or an image sequence of files (in order).
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    File(String),
    Sequence { files: Vec<String>, alphabetical: bool },
}

impl Source {
    /// The path naming it (a sequence's first file).
    pub fn path(&self) -> &str {
        match self {
            Source::File(p) => p,
            Source::Sequence { files, .. } => files.first().map_or("", String::as_str),
        }
    }

    /// The Project panel name: the file name, or a sequence's `shot_[0001-0250].png`.
    pub fn name(&self) -> String {
        match self {
            Source::Sequence { files, alphabetical: false } => sequence_name(files).unwrap_or_else(|| file_name(self.path())),
            _ => file_name(self.path()),
        }
    }
}

/// The file name of a path (either separator).
pub fn file_name(path: &str) -> String {
    path.rsplit(['/', '\\']).find(|n| !n.is_empty()).unwrap_or(path).to_string()
}

/// The folder of a path (`""` for a bare name).
fn folder(path: &str) -> &str {
    path.rfind(['/', '\\']).map_or("", |i| path.get(..i.max(1)).unwrap_or(""))
}

fn extension(path: &str) -> String {
    let name = file_name(path);
    name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default()
}

/// Stills that can form a sequence (not layered or vector documents, which import as comps).
fn sequenceable(path: &str) -> bool {
    crate::media_browser::kind_of(path) == Some("image") && !matches!(extension(path).as_str(), "svg" | "psd" | "psb")
}

/// The run a still belongs to: (folder, name prefix, extension); alphabetical runs ignore the
/// name. `None` for files that can't be part of one.
fn run_key(path: &str, alphabetical: bool) -> Option<(String, String, String)> {
    if !sequenceable(path) {
        return None;
    }
    let ext = extension(path);
    if alphabetical {
        return Some((folder(path).to_string(), String::new(), ext));
    }
    let name = file_name(path);
    let stem = name.get(..name.len().saturating_sub(ext.len() + usize::from(!ext.is_empty()))).unwrap_or("");
    let (prefix, _) = split_frame_number(stem)?;
    Some((folder(path).to_string(), prefix.to_string(), ext))
}

/// Put a run's files in order: by frame number, or by name (Force Alphabetical Order).
fn sort_run(files: &mut Vec<String>, alphabetical: bool) {
    if alphabetical {
        files.sort_by_key(|p| file_name(p).to_lowercase());
    } else {
        files.sort_by_key(|p| (frame_number(p), p.clone()));
    }
    files.dedup();
}

/// The files of the run `path` belongs to, from its folder.
fn run_files(services: &dyn Services, path: &str, alphabetical: bool) -> Vec<String> {
    let Some(key) = run_key(path, alphabetical) else { return vec![] };
    let mut files: Vec<String> = services.list_dir(&key.0).into_iter().filter(|p| run_key(p, alphabetical).as_ref() == Some(&key)).collect();
    sort_run(&mut files, alphabetical);
    files
}

/// What importing `paths` makes, in the order picked: folders become their importable files,
/// and (with the Sequence option) numbered stills of one run one [`Source::Sequence`].
pub fn group(services: &dyn Services, paths: &[String], opts: SequenceOptions) -> Vec<Source> {
    let mut files: Vec<String> = vec![];
    for p in paths {
        if services.is_dir(p) {
            let mut inside: Vec<String> =
                services.list_dir(p).into_iter().filter(|f| matches!(crate::media_browser::kind_of(f), Some(k) if k != "project")).collect();
            inside.sort_by_key(|f| file_name(f).to_lowercase());
            files.extend(inside);
        } else {
            files.push(p.clone());
        }
    }
    if !opts.sequence {
        return files.into_iter().map(Source::File).collect();
    }
    // Runs in the order their first file was picked.
    let mut runs: Vec<((String, String, String), Vec<String>)> = vec![];
    let mut out: Vec<Option<Source>> = vec![];
    let mut slot: Vec<usize> = vec![];
    for f in files {
        match run_key(&f, opts.alphabetical) {
            Some(key) => match runs.iter_mut().find(|(k, _)| *k == key) {
                Some((_, run)) => run.push(f),
                None => {
                    runs.push((key, vec![f]));
                    slot.push(out.len());
                    out.push(None);
                }
            },
            None => out.push(Some(Source::File(f))),
        }
    }
    for ((_, mut picked), at) in runs.into_iter().zip(slot) {
        sort_run(&mut picked, opts.alphabetical);
        // One file: its whole run from the folder.
        if let [one] = picked.as_slice() {
            let all = run_files(services, one, opts.alphabetical);
            if all.len() >= 2 {
                picked = all;
            }
        }
        let src = match picked.len() {
            0 => None,
            1 => picked.pop().map(Source::File),
            _ => Some(Source::Sequence { files: picked, alphabetical: opts.alphabetical }),
        };
        if let Some(o) = out.get_mut(at) {
            *o = src;
        }
    }
    out.into_iter().flatten().collect()
}

/// The image sequences importing `paths` would make (the Import dialog asks about them).
pub fn sequences(services: &dyn Services, paths: &[String], alphabetical: bool) -> Vec<Vec<String>> {
    group(services, paths, SequenceOptions { sequence: true, alphabetical })
        .into_iter()
        .filter_map(|s| match s {
            Source::Sequence { files, .. } => Some(files),
            Source::File(_) => None,
        })
        .collect()
}

/// Probe what a [`Source`] makes. A sequence is its first file's footage with every file, at
/// `rate`, showing gaps in its numbering as placeholders (or closing them up when
/// alphabetical).
pub fn probe(importer: &dyn Importer, src: &Source, rate: FrameRate) -> Result<Footage, String> {
    let Source::Sequence { files, alphabetical } = src else { return importer.probe(src.path()) };
    let mut f = importer.probe(src.path())?;
    if f.kind != FootageKind::Still {
        return Ok(f);
    }
    f.kind = FootageKind::Sequence;
    f.sequence = files.clone();
    f.missing_frames = if *alphabetical { MissingFrames::Skip } else { MissingFrames::Placeholder };
    f.start_frame = None;
    f.frame_rate = rate;
    f.native_rate = None;
    f.sync_sequence_duration();
    importer.add_frames(files.get(1..).unwrap_or_default());
    Ok(f)
}

/// Probe one path the way it imports by default: a numbered still with numbered siblings
/// brings its run as a sequence (Replace Footage, Reload, proxies, render outputs).
pub fn probe_path(importer: &dyn Importer, services: &dyn Services, path: &str, rate: FrameRate) -> Result<Footage, String> {
    probe(importer, &source_of(services, path, SequenceOptions::default()), rate)
}

/// What importing the one file `path` makes.
pub fn source_of(services: &dyn Services, path: &str, opts: SequenceOptions) -> Source {
    group(services, &[path.to_string()], opts).into_iter().next().unwrap_or_else(|| Source::File(path.to_string()))
}

impl crate::Session {
    /// Image sequences' frame rate on import: Settings ▸ Import ▸ Sequence Footage.
    pub fn sequence_rate(&self) -> FrameRate {
        FrameRate::from_f64(self.prefs.import.sequence_fps)
    }

    /// Probe `path` as footage the way it imports by default ([`probe_path`]).
    pub(crate) fn probe_footage(&self, path: &str) -> Result<Footage, String> {
        let importer = self.importer.as_ref().ok_or("media import is not available in this build")?;
        probe_path(importer.as_ref(), self.services.as_ref(), path, self.sequence_rate())
    }
}

/// Settings ▸ Import ▸ Report Missing Frames: "3 missing frames (3–4, 6)". `None` when the
/// numbering is continuous.
pub fn missing_report(files: &[String]) -> Option<String> {
    let gaps = missing_frame_ranges(files);
    let total: i64 = gaps.iter().map(|(a, b)| b.saturating_sub(*a).saturating_add(1)).fold(0, i64::saturating_add);
    let list: Vec<String> = gaps.iter().map(|(a, b)| if a == b { a.to_string() } else { format!("{a}–{b}") }).collect();
    (!gaps.is_empty()).then(|| format!("{total} missing frame{} ({})", if total == 1 { "" } else { "s" }, list.join(", ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file table: listed folders and their files.
    struct Table(Vec<&'static str>);
    impl Services for Table {
        fn read_file(&self, path: &str) -> std::io::Result<Vec<u8>> {
            Err(std::io::Error::other(path.to_string()))
        }
        fn write_file(&self, _: &str, _: &[u8]) -> std::io::Result<()> {
            Ok(())
        }
        fn list_dir(&self, dir: &str) -> Vec<String> {
            self.0.iter().filter(|p| folder(p) == dir).map(|p| p.to_string()).collect()
        }
        fn is_dir(&self, path: &str) -> bool {
            self.0.iter().any(|p| folder(p) == path)
        }
    }

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn groups_runs_ranges_folders_and_alphabetical() {
        let t = Table(vec!["/s/a_0002.png", "/s/a_0001.png", "/s/a_0010.png", "/s/b_1.png", "/s/notes.txt", "/s/c.png", "/s/a_0003.jpg", "/x/one_1.png"]);
        let on = SequenceOptions::default();
        // One picked file: its run from the folder, in frame order.
        assert_eq!(
            group(&t, &strs(&["/s/a_0010.png"]), on),
            [Source::Sequence { files: strs(&["/s/a_0001.png", "/s/a_0002.png", "/s/a_0010.png"]), alphabetical: false }]
        );
        // A picked range stays that range; other files import as they are.
        assert_eq!(
            group(&t, &strs(&["/s/c.png", "/s/a_0010.png", "/s/a_0002.png", "/s/b_1.png", "/x/one_1.png"]), on),
            [
                Source::File("/s/c.png".into()),
                Source::Sequence { files: strs(&["/s/a_0002.png", "/s/a_0010.png"]), alphabetical: false },
                Source::File("/s/b_1.png".into()),
                Source::File("/x/one_1.png".into()),
            ]
        );
        // Sequence off: every file on its own.
        assert_eq!(group(&t, &strs(&["/s/a_0001.png"]), SequenceOptions { sequence: false, alphabetical: false }), [Source::File("/s/a_0001.png".into())]);
        // A folder: its media files, runs as sequences.
        let g = group(&t, &strs(&["/s"]), on);
        assert_eq!(g.len(), 4, "{g:?}");
        assert_eq!(g[0], Source::Sequence { files: strs(&["/s/a_0001.png", "/s/a_0002.png", "/s/a_0010.png"]), alphabetical: false });
        assert_eq!(g[0].name(), "a_[0001-0010].png");
        assert!(!g.iter().any(|s| s.path().ends_with(".txt")));
        // Force Alphabetical Order: every PNG in the folder, by name.
        let a = group(&t, &strs(&["/s/c.png"]), SequenceOptions { sequence: true, alphabetical: true });
        assert_eq!(a, [Source::Sequence { files: strs(&["/s/a_0001.png", "/s/a_0002.png", "/s/a_0010.png", "/s/b_1.png", "/s/c.png"]), alphabetical: true }]);
        assert_eq!(a[0].name(), "a_0001.png");
        assert_eq!(sequences(&t, &strs(&["/s/a_0001.png", "/s/c.png"]), false).len(), 1);
    }

    #[test]
    fn windows_paths_and_reports() {
        let t = Table(vec!["C:\\r\\f_01.exr", "C:\\r\\f_02.exr", "C:\\r\\f_05.exr"]);
        let g = group(&t, &strs(&["C:\\r\\f_02.exr"]), SequenceOptions::default());
        assert_eq!(g, [Source::Sequence { files: strs(&["C:\\r\\f_01.exr", "C:\\r\\f_02.exr", "C:\\r\\f_05.exr"]), alphabetical: false }]);
        assert_eq!(missing_report(&strs(&["a_001.png", "a_002.png", "a_005.png", "a_007.png"])).as_deref(), Some("3 missing frames (3–4, 6)"));
        assert_eq!(missing_report(&strs(&["a1.png", "a2.png"])), None);
        assert_eq!(folder("/x.png"), "/");
        assert_eq!(file_name("C:\\r\\f_01.exr"), "f_01.exr");
    }
}
