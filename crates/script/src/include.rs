//! ExtendScript's preprocessor directives, from the JavaScript Tools Guide's description of their
//! behaviour: `#include "file"` (and `//@include "file"`) inlines a file in place of the line,
//! resolved against the including file's folder, then the folders the latest `#includepath "a;b"`
//! (or `//@includepath`) named; `#target`, `#targetengine`, `#strict`, `#script` and `#engine`
//! are dropped. Each file is inlined once (which also breaks include cycles), nesting is limited
//! to [`MAX_DEPTH`], files and their total size are capped, and reads follow the scripts' file
//! gate ([`crate::scriptfile::check_read`]). Directives are recognised on lines of their own.
//!
//! Errors in the expanded code are mapped back to the file and line they came from.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use effectcraft_engine::Session;

use crate::runtime::ScriptError;
use crate::scriptfile::{self, MAX_DEPTH, MAX_FILE_BYTES};

/// The most text a script and the files it includes may add up to.
const MAX_TOTAL_BYTES: usize = 4 * MAX_FILE_BYTES;

/// A script with its includes inlined.
pub(crate) struct Expanded {
    pub(crate) code: String,
    /// Per line of `code`: the file (index into `files`) and 1-based line it came from.
    lines: Vec<(usize, u32)>,
    /// The script itself, then each included file.
    files: Vec<String>,
}

impl Expanded {
    /// Point an error in the expanded code at the file and line it came from.
    pub(crate) fn locate(&self, e: &mut ScriptError) {
        if self.files.first() != Some(&e.file) {
            return;
        }
        let Some(&(file, line)) = e.line.and_then(|l| l.checked_sub(1)).and_then(|l| self.lines.get(l as usize)) else { return };
        if let Some(name) = self.files.get(file) {
            e.file = name.clone();
            e.line = Some(line);
        }
    }
}

enum Directive<'a> {
    Include(&'a str),
    IncludePath(&'a str),
    Ignored,
}

/// The preprocessor directive on `line`, if it is one.
fn directive(line: &str) -> Option<Directive<'_>> {
    let t = line.trim();
    let (rest, comment) = match t.strip_prefix("//@") {
        Some(r) => (r, true),
        None => (t.strip_prefix('#')?, false),
    };
    let end = rest.find(|c: char| !c.is_ascii_alphabetic()).unwrap_or(rest.len());
    let (word, arg) = rest.split_at(end);
    if !(arg.is_empty() || arg.starts_with(char::is_whitespace) || arg.starts_with(['"', '\''])) {
        return None;
    }
    let arg = unquote(arg);
    match word {
        "include" => Some(Directive::Include(arg)),
        "includepath" => Some(Directive::IncludePath(arg)),
        // `//@target aftereffects` and friends are plain comments already.
        "target" | "targetengine" | "strict" | "script" | "engine" if !comment => Some(Directive::Ignored),
        _ => None,
    }
}

/// A directive's argument: `"x"`, `'x'` or `x`, with an optional trailing `;`.
fn unquote(arg: &str) -> &str {
    let a = arg.trim();
    let a = a.strip_suffix(';').map_or(a, str::trim_end);
    for q in ['"', '\''] {
        if let Some(inner) = a.strip_prefix(q).and_then(|r| r.split(q).next()) {
            return inner;
        }
    }
    a
}

struct Walk<'a> {
    s: &'a Session,
    /// The running script's folder: the file gate lets it load files below it.
    root: Option<PathBuf>,
    /// The latest `#includepath`.
    search: Vec<PathBuf>,
    seen: HashSet<PathBuf>,
    total: usize,
    out: Expanded,
}

impl Walk<'_> {
    fn push(&mut self, line: &str, file: usize, n: usize) {
        if !self.out.lines.is_empty() {
            self.out.code.push('\n');
        }
        self.out.code.push_str(line);
        self.out.lines.push((file, u32::try_from(n + 1).unwrap_or(u32::MAX)));
    }

    /// Inline `code` (file `file`, in folder `dir`), `depth` includes deep.
    fn walk(&mut self, code: &str, file: usize, dir: Option<&Path>, depth: usize) -> Result<(), ScriptError> {
        for (n, line) in code.split('\n').enumerate() {
            let err = |w: &Self, message: String| ScriptError {
                message,
                line: Some(u32::try_from(n + 1).unwrap_or(u32::MAX)),
                column: None,
                file: w.out.files.get(file).cloned().unwrap_or_default(),
            };
            match directive(line) {
                None => self.push(line, file, n),
                Some(Directive::Ignored) => self.push("", file, n),
                Some(Directive::IncludePath(paths)) => {
                    self.search =
                        paths.split(';').map(str::trim).filter(|p| !p.is_empty()).filter_map(|p| scriptfile::resolve(self.s, p, dir, &[]).ok()).collect();
                    self.push("", file, n);
                }
                Some(Directive::Include(name)) => {
                    if name.is_empty() {
                        return Err(err(self, "#include needs a file name".into()));
                    }
                    let path = scriptfile::resolve(self.s, name, dir, &self.search).map_err(|e| err(self, format!("#include {e}")))?;
                    if !self.seen.insert(path.clone()) {
                        // Already inlined (or the file including it): once is enough.
                        self.push("", file, n);
                        continue;
                    }
                    if depth >= MAX_DEPTH {
                        return Err(err(self, format!("#include \"{name}\": includes are nested too deeply (at most {MAX_DEPTH})")));
                    }
                    scriptfile::check_read(self.s, &path, self.root.as_deref()).map_err(|e| err(self, e))?;
                    let text = scriptfile::read(self.s, &path).map_err(|e| err(self, e))?;
                    self.total = self.total.saturating_add(text.len());
                    if self.total > MAX_TOTAL_BYTES {
                        return Err(err(self, format!("#include \"{name}\": the included files exceed {} MiB", MAX_TOTAL_BYTES >> 20)));
                    }
                    // The included lines take the directive's place.
                    self.out.files.push(path.to_string_lossy().to_string());
                    let idx = self.out.files.len().saturating_sub(1);
                    self.walk(&text, idx, path.parent(), depth + 1)?;
                }
            }
        }
        Ok(())
    }
}

/// `code` (the script `name`) with its directives applied, or `None` when it has none.
pub(crate) fn expand(s: &Session, code: &str, name: &str) -> Result<Option<Expanded>, ScriptError> {
    if !code.split('\n').any(|l| directive(l).is_some()) {
        return Ok(None);
    }
    let root = scriptfile::script_dir(s, name);
    let mut w = Walk {
        s,
        root: root.clone(),
        search: vec![],
        seen: HashSet::new(),
        total: code.len(),
        out: Expanded { code: String::with_capacity(code.len()), lines: vec![], files: vec![if name.is_empty() { "script" } else { name }.to_string()] },
    };
    if root.is_some() {
        w.seen.insert(crate::runtime::normalize(name));
    }
    // A script without a folder (the Script Console, code run by name) looks in the project's.
    let dir = root.or_else(|| crate::runtime::project_dir(s).map(|d| crate::runtime::normalize(&d)));
    w.walk(code, 0, dir.as_deref(), 0)?;
    Ok(Some(w.out))
}
