//! Footage paths stored in a document (the project file, an imported project, the New Project
//! template) reach the file system only through [`resolve`]: a relative path is taken from the
//! document's folder, and only local files, or files on the network share the document itself is
//! on, are followed (see `craft_docpath`).
//!
//! Loading a document resolves its footage paths once (`resolve_footage`): followed paths are
//! replaced by the resolved path, and an item with a refused path is marked missing, so the media
//! layer never reads it. The session then remembers which paths came from which document
//! ([`StoredPaths`]); the footage check, the changed-on-disk scan, Reload Footage, Relink, Edit
//! Original and Reveal in Finder follow a path only through it. Paths the user imports or relinks
//! in this session are followed as given.
//!
//! The network shares the user names in Settings ▸ Import ▸ Trusted Network Folders
//! ([`trusted_shares`]) are followed from any document, as the document's own share is: a project
//! saved on the local disk keeps finding its footage on a NAS after it is opened again.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use craft_docpath::Refused;
use effectcraft_project::{Footage, ItemId, ItemKind, Project};

/// Why a stored path is not followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotFollowed {
    /// The path is not a local file, or is not on the project's own share.
    Refused(Refused),
    /// The path is relative and the project has no folder to take it from.
    NoProjectFolder,
}

impl fmt::Display for NotFollowed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(r) => r.fmt(f),
            Self::NoProjectFolder => f.write_str("the path is relative and the project has not been saved"),
        }
    }
}

impl std::error::Error for NotFollowed {}

/// The file to open for `stored`, a footage path or `file:` URL stored in the project saved at
/// `project` (`None` for a project without a file), or why it is not followed. A relative
/// `project` is taken from the working directory, where it was opened from.
pub fn resolve(stored: &str, project: Option<&str>) -> Result<PathBuf, NotFollowed> {
    let project = project.filter(|p| !p.is_empty()).map(|p| {
        let p = Path::new(p);
        std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
    });
    craft_docpath::resolve_absolute(stored, project.as_deref()).map_err(|why| match why {
        // Resolved, but still relative: it would be taken from the working directory, which the
        // project does not choose. A Windows drive or share path (`C:\x`) stays invalid outside
        // Windows even with a project.
        Refused::Invalid if project.is_none() && craft_docpath::resolve(stored, None).is_ok_and(|p| cfg!(windows) || !is_windows_rooted(&p)) => {
            NotFollowed::NoProjectFolder
        }
        why => NotFollowed::Refused(why),
    })
}

/// [`resolve`], also following a network path on one of the `trusted` shares (`\\server\share`
/// roots, [`trusted_shares`]). A trusted share is treated as the document's own share would be,
/// with the same checks (`craft_docpath`), so `..` cannot step out of it.
pub fn resolve_trusting(stored: &str, project: Option<&str>, trusted: &[String]) -> Result<PathBuf, NotFollowed> {
    match resolve(stored, project) {
        Err(NotFollowed::Refused(Refused::Network)) => trusted
            .iter()
            .find_map(|root| {
                // A document at the share's root: paths on the same share are followed.
                let doc = format!("{}/_", root.trim_end_matches(['\\', '/']));
                craft_docpath::resolve_absolute(stored, Some(Path::new(&doc))).ok()
            })
            .ok_or(NotFollowed::Refused(Refused::Network)),
        other => other,
    }
}

/// The trusted network shares of the setting `text` (Settings ▸ Import ▸ Trusted Network Folders):
/// `\\server\share` roots separated by `;` or line breaks; entries that are not a share are
/// ignored.
pub fn trusted_shares(text: &str) -> Vec<String> {
    text.split([';', '\n', '\r']).map(str::trim).filter(|t| craft_docpath::classify(t) == craft_docpath::PathKind::Network).map(str::to_string).collect()
}

/// Whether `p` is a Windows drive or share path, which a project cannot make absolute outside Windows.
fn is_windows_rooted(p: &Path) -> bool {
    let s = p.to_string_lossy();
    let b = s.as_bytes();
    (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':') || s.starts_with(r"\\") || s.starts_with("//")
}

/// The paths of `f` (its file and every frame of an image sequence) resolved against the document
/// saved at `document`, or why one of them is not followed.
fn footage_paths(f: &Footage, document: Option<&str>, trusted: &[String]) -> Result<(String, Vec<String>), (String, NotFollowed)> {
    let one = |q: &String| -> Result<String, (String, NotFollowed)> {
        if q.is_empty() {
            return Ok(String::new());
        }
        resolve_trusting(q, document, trusted).map(|p| p.to_string_lossy().into_owned()).map_err(|why| (q.clone(), why))
    };
    Ok((one(&f.path)?, f.sequence.iter().map(one).collect::<Result<_, _>>()?))
}

fn paths_of(f: &Footage) -> impl Iterator<Item = &String> {
    std::iter::once(&f.path).chain(&f.sequence).filter(|q| !q.is_empty())
}

/// Footage paths that came from a document, with the path of that document (`None`: one without
/// a file). Any other path was chosen in this session and is followed as given.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StoredPaths(Arc<HashMap<String, Option<String>>>, Arc<Vec<String>>);

impl StoredPaths {
    /// Record the footage paths of `p`, loaded from the document at `document`. A path already
    /// recorded keeps the document it came from first.
    pub(crate) fn add(&mut self, p: &Project, document: Option<&str>) {
        self.add_items(p, |_| true, document);
    }

    /// Record the footage paths of the items of `p` that `pick` selects, loaded from the document
    /// at `document` (an imported file: the rest of the project keeps its own origin).
    pub(crate) fn add_items(&mut self, p: &Project, pick: impl Fn(ItemId) -> bool, document: Option<&str>) {
        let map = Arc::make_mut(&mut self.0);
        for item in p.items.values().filter(|i| pick(i.id)) {
            let original = match &item.kind {
                ItemKind::Footage(f) => Some(f),
                _ => None,
            };
            for f in original.into_iter().chain(item.proxy.as_ref().map(|p| &p.footage)) {
                for q in paths_of(f) {
                    map.entry(q.clone()).or_insert_with(|| document.map(str::to_string));
                }
            }
        }
    }

    /// The user chose the files of `f` in this session (import, relink, Replace Footage): follow
    /// them as given from now on.
    pub(crate) fn allow(&mut self, f: &Footage) {
        if paths_of(f).any(|q| self.0.contains_key(q)) {
            let map = Arc::make_mut(&mut self.0);
            for q in paths_of(f) {
                map.remove(q);
            }
        }
    }

    /// The network shares followed from any document ([`trusted_shares`]).
    pub fn trusted(&self) -> Arc<Vec<String>> {
        self.1.clone()
    }

    /// Settings ▸ Import ▸ Trusted Network Folders changed.
    pub(crate) fn set_trusted(&mut self, shares: Vec<String>) {
        if *self.1 != shares {
            self.1 = Arc::new(shares);
        }
    }

    /// The file to open for footage path `path`.
    pub fn follow(&self, path: &str) -> Result<PathBuf, NotFollowed> {
        match self.0.get(path) {
            Some(document) => resolve_trusting(path, document.as_deref(), &self.1),
            None => Ok(PathBuf::from(path)),
        }
    }

    /// Whether the file at footage path `path` exists. A path that is not followed is not looked
    /// for.
    pub(crate) fn exists(&self, services: &dyn crate::Services, path: &str) -> bool {
        !path.is_empty() && self.follow(path).is_ok_and(|p| p.to_str().is_some_and(|p| services.exists(p)))
    }

    /// Whether every file of `f` is followed, or the first that is not and why.
    pub(crate) fn check(&self, f: &Footage) -> Result<(), (String, NotFollowed)> {
        paths_of(f).try_for_each(|q| self.follow(q).map(|_| ()).map_err(|why| (q.clone(), why)))
    }
}

/// Resolve the footage paths stored in `p`, the project saved at `project`: followed paths are
/// replaced by the file they resolve to, and an item with a path that is not followed is marked
/// missing. Returns those items with the refused path and why. Paths on the `trusted` shares are
/// followed ([`resolve_trusting`]).
pub(crate) fn resolve_footage(p: &mut Project, project: Option<&str>, trusted: &[String]) -> Vec<(ItemId, String, NotFollowed)> {
    resolve_footage_of(p, |_| true, project, trusted)
}

/// [`resolve_footage`] for the items of `p` that `pick` selects (the ones an import added).
pub(crate) fn resolve_footage_of(
    p: &mut Project,
    pick: impl Fn(ItemId) -> bool,
    project: Option<&str>,
    trusted: &[String],
) -> Vec<(ItemId, String, NotFollowed)> {
    let mut refused = vec![];
    for item in p.items.values_mut().filter(|i| pick(i.id)) {
        let original = match &mut item.kind {
            ItemKind::Footage(f) => Some(f),
            _ => None,
        };
        for f in original.into_iter().chain(item.proxy.as_mut().map(|p| &mut p.footage)) {
            match footage_paths(f, project, trusted) {
                Ok((path, sequence)) => {
                    f.path = path;
                    f.sequence = sequence;
                }
                Err((path, why)) => {
                    f.missing = true;
                    refused.push((item.id, path, why));
                }
            }
        }
    }
    refused
}

/// The message for footage whose stored path is not followed.
pub(crate) fn refusal(stored: &str, why: NotFollowed) -> String {
    format!("the footage file {stored} is not opened: {why}")
}

impl crate::Session {
    /// The file to open for footage path `stored` in this session's project.
    pub fn stored_path(&self, stored: &str) -> Result<PathBuf, NotFollowed> {
        self.stored_paths.follow(stored)
    }
}

#[cfg(test)]
mod tests;
