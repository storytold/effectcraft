//! Which files the [`MediaPool`](crate::MediaPool) reads for a render
//! ([`FootageSource::recording`](effectcraft_render::FootageSource::recording)).
//!
//! Every read of a file notes it, with the identity it had when read, in the innermost scope open
//! on this thread. The pool's caches (decoded frames, opened movies, GIFs, models, EXR channels)
//! keep what their entries were made of and note it again on each hit, so a frame served from
//! memory reports its file without touching the disk.
//!
//! The scopes are per thread: a read noted while a render's scope is open on the thread belongs
//! to that render. A thread waiting inside a decode can run another job meanwhile (rayon); that
//! job's reads go to its own scope when it opens one, and otherwise to the scope below it, which
//! then lists a file more than it read (never fewer).

use std::cell::RefCell;

use effectcraft_render::reads::{ReadFile, ReadFiles, ReadLog};

/// Scopes nested deeper than this are not opened (a thread that keeps taking jobs while it waits
/// cannot pile them up without bound): their reads go to the scope below, and they come back
/// incomplete.
const MAX_DEPTH: usize = 64;

thread_local! {
    /// The open scopes on this thread, innermost last, each with the files noted in it.
    static SCOPES: RefCell<Vec<ReadFiles>> = const { RefCell::new(Vec::new()) };
}

/// Note `file` in the innermost open scope on this thread (nothing when none is open).
pub(crate) fn note(file: ReadFile) {
    let _ = SCOPES.try_with(|s| {
        if let Ok(mut s) = s.try_borrow_mut()
            && let Some(top) = s.last_mut()
        {
            top.add(file);
        }
    });
}

/// Note every file of `files` in the innermost open scope on this thread.
pub(crate) fn note_all(files: &ReadFiles) {
    if files.is_empty() && files.is_complete() {
        return;
    }
    let _ = SCOPES.try_with(|s| {
        if let Ok(mut s) = s.try_borrow_mut()
            && let Some(top) = s.last_mut()
        {
            top.extend(files);
        }
    });
}

/// Closes the scope it opened, also when what runs inside panics: its files go to `out`.
struct Scope<'a> {
    out: Out<'a>,
    opened: bool,
}

enum Out<'a> {
    /// A render's log ([`recording`]).
    Log(&'a ReadLog),
    /// The caller's ([`captured`]).
    Files(&'a mut ReadFiles),
}

impl Scope<'_> {
    /// Starts collecting the files read on this thread into `out`.
    /// At the nesting limit the scope stays closed and reports an incomplete list.
    fn open(out: Out<'_>) -> Scope<'_> {
        let opened = SCOPES
            .try_with(|s| match s.try_borrow_mut() {
                Ok(mut s) if s.len() < MAX_DEPTH => {
                    s.push(ReadFiles::default());
                    true
                }
                _ => false,
            })
            .unwrap_or(false);
        Scope { out, opened }
    }
}

impl Drop for Scope<'_> {
    /// Ends the scope and hands the collected files to its output, marked incomplete when the scope could not be opened.
    fn drop(&mut self) {
        let files = if self.opened { SCOPES.try_with(|s| s.try_borrow_mut().ok().and_then(|mut s| s.pop())).ok().flatten() } else { None };
        // A scope that could not open saw nothing: what it made can't name its files.
        let files = files.unwrap_or_else(|| {
            let mut f = ReadFiles::default();
            f.mark_incomplete();
            f
        });
        match &mut self.out {
            Out::Log(log) => log.note_all(&files),
            Out::Files(out) => out.extend(&files),
        }
    }
}

/// Run `f` with the files it reads on this thread noted in `log`
/// ([`FootageSource::recording`](effectcraft_render::FootageSource::recording)).
pub(crate) fn recording(log: &ReadLog, f: &mut dyn FnMut()) {
    let _scope = Scope::open(Out::Log(log));
    f();
}

/// Run `f` and return what it returns with the files it read on this thread (to keep with a
/// cache entry made of them). They are not noted in the enclosing scope: the caller notes them,
/// as it does on each later hit of the entry.
pub(crate) fn captured<T>(f: impl FnOnce() -> T) -> (T, ReadFiles) {
    let mut files = ReadFiles::default();
    let out = {
        let _scope = Scope::open(Out::Files(&mut files));
        f()
    };
    (out, files)
}

#[cfg(test)]
mod tests;
