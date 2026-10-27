//! The last-resort guard: a panic that escapes a command (a bug, never an expected error) is
//! turned into an [`EngineError`] instead of taking the app, and the user's open project, down
//! with it. Edits work on a copy of the project (see [`crate::Session::edit`]), so an edit that
//! panics leaves the project as it was. This is a safety net, not a substitute for returning
//! errors (AGENTS.md, "Never crash").

use crate::{EngineError, Result};

/// Run `f`; a panic inside it becomes an error naming `what`. The panic itself is logged by
/// the panic hook ([`crate::logging::install_panic_hook`]).
pub fn guarded<T>(what: &str, f: impl FnOnce() -> Result<T>) -> Result<T> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(payload) => {
            let msg = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown error".into());
            log::error!("{what} panicked: {msg}");
            Err(EngineError::Other(format!("{what} failed with an internal error ({msg}); it was stopped and your project is still open")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_becomes_an_error() {
        let r: Result<()> = guarded("command `test.panic`", || panic!("boom"));
        let e = r.unwrap_err().to_string();
        assert!(e.contains("test.panic") && e.contains("boom"), "{e}");
        let r: Result<u8> = guarded("x", || {
            let v: Vec<u8> = vec![];
            Ok(v[3])
        });
        assert!(r.is_err());
        assert_eq!(guarded("ok", || Ok(7)).unwrap(), 7);
        assert!(matches!(guarded::<()>("err", || Err(EngineError::NoComp)), Err(EngineError::NoComp)));
    }

    #[test]
    fn the_session_survives_a_panicking_edit() {
        let mut s = crate::Session::new();
        let before = s.project.clone();
        let r: Result<()> = guarded("command `test.edit`", || {
            s.edit("Broken", None, |p, _| {
                p.next_id += 1;
                panic!("bug in the middle of an edit")
            })
        });
        assert!(r.is_err());
        assert!(std::sync::Arc::ptr_eq(&before, &s.project), "the project is untouched");
        // Later edits still work.
        let next = s.project.next_id;
        s.edit("Fine", None, |p, _| {
            p.next_id += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(s.project.next_id, next + 1);
    }
}
