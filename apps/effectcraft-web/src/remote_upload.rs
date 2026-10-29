//! Saving a project to a writable remote mount ([`crate::remote_write`] has the protocol). The
//! engine's `write_file` is synchronous, so the bytes are kept in memory under the path (reads see
//! them at once) and uploaded in the background: `.part` file, check of the target's ETag, move.
//! When the upload fails the project is marked unsaved again and the status line says why.
//!
//! One upload runs per path at a time; a save made meanwhile replaces the one waiting.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::io;

use crate::remote::cfg_and_origin;
use crate::remote_http::{Resp, fetch, header};
use crate::remote_write::{self as rw, Known, Outcome, Precondition, Req};

#[derive(Default)]
struct Uploads {
    /// The ETag each path had when this session opened or last saved it (`None`: the server sent none).
    seen: HashMap<String, Option<String>>,
    running: HashSet<String>,
    waiting: HashMap<String, Vec<u8>>,
}

thread_local! {
    static UPLOADS: RefCell<Uploads> = RefCell::new(Uploads::default());
}

/// Remember the ETag of a project file just opened from the server.
pub fn note_opened(path: &str, etag: Option<String>) {
    UPLOADS.with(|u| u.borrow_mut().seen.insert(path.to_string(), etag));
}

/// Forget the version of `path`: it cannot be tied to what this session wrote, so the next save is
/// a conflict or Save As.
fn forget_version(path: &str) {
    UPLOADS.with(|u| u.borrow_mut().seen.remove(path));
}

/// What this session knows about the version of `path` on the server.
fn known(path: &str) -> Known {
    UPLOADS.with(|u| match u.borrow().seen.get(path) {
        Some(Some(e)) => Known::Etag(e.clone()),
        Some(None) => Known::NoEtag,
        None => Known::Unknown,
    })
}

/// Send one request of a save with its time limit.
async fn send(r: &Req, body: Option<&[u8]>) -> Result<Resp, String> {
    let headers: Vec<(&str, &str)> = r.headers.iter().map(|(k, v)| (*k, v.as_str())).collect();
    fetch(r.method, &r.url, &headers, body).await
}

/// `write_file` for a path on a remote mount; `None` for any other path. A read-only mount
/// refuses with an error the engine shows; otherwise the upload starts and the write succeeds.
pub fn write(path: &str, data: &[u8]) -> Option<io::Result<()>> {
    let (cfg, _) = cfg_and_origin().ok()?;
    cfg.mount_for(path)?;
    if let Err(e) = rw::check_writable(&cfg, path) {
        return Some(Err(io::Error::new(io::ErrorKind::PermissionDenied, e)));
    }
    if data.len() > rw::MAX_UPLOAD {
        return Some(Err(io::Error::other("too large to save to the server")));
    }
    // reads of the path see the saved bytes while the upload runs
    crate::files::put_remote(path, data.to_vec().into());
    let start = UPLOADS.with(|u| {
        let mut u = u.borrow_mut();
        u.waiting.insert(path.to_string(), data.to_vec());
        u.running.insert(path.to_string())
    });
    if start {
        let path = path.to_string();
        wasm_bindgen_futures::spawn_local(async move {
            while let Some(bytes) = UPLOADS.with(|u| u.borrow_mut().waiting.remove(&path)) {
                let outcome = upload(&path, &bytes).await;
                report(&path, &outcome);
            }
            UPLOADS.with(|u| u.borrow_mut().running.remove(&path));
        });
    }
    Some(Ok(()))
}

/// A failed save leaves the project unsaved and says why.
fn report(path: &str, outcome: &Outcome) {
    let Some(msg) = rw::failure_message(outcome) else { return };
    log::warn!("saving {path}: {msg}");
    let path = path.to_string();
    crate::post(move |app| {
        // the project this path belongs to is unsaved again
        if app.session.path.as_deref() == Some(path.as_str()) {
            app.session.mark_unsaved();
        }
        app.ui.status = msg;
    });
}

/// A token for one upload's `.part` file: 128 random bits as hex, so two saves of a path (this
/// tab or another client) never share a part file.
fn new_token() -> String {
    format!("{:016x}{:016x}", js_sys::Math::random().to_bits(), js_sys::Math::random().to_bits() ^ js_sys::Date::now().to_bits())
}

/// Upload `bytes` to this upload's own `.part` file, check the target, move the upload over it.
async fn upload(path: &str, bytes: &[u8]) -> Outcome {
    let (cfg, origin) = match cfg_and_origin() {
        Ok(v) => v,
        Err(e) => return Outcome::Failed(e),
    };
    let token = new_token();
    let outcome = upload_steps(&cfg, &origin, path, &token, bytes).await;
    if outcome != Outcome::Saved {
        // best effort: the .part file is of no use
        if let Ok(d) = rw::delete_part(&cfg, path, &token) {
            let _ = send(&d, None).await;
        }
    }
    outcome
}

/// The requests of one save, in order; the first that fails ends it.
async fn upload_steps(cfg: &crate::remote_config::Config, origin: &str, path: &str, token: &str, bytes: &[u8]) -> Outcome {
    let step = |e: String| Outcome::Failed(e);
    let put = match rw::put_part(cfg, path, token) {
        Ok(r) => r,
        Err(e) => return step(e),
    };
    let mut part_etag = match send(&put, Some(bytes)).await {
        Ok(r) => {
            if let Err(e) = rw::classify_put(r.status()) {
                return step(e);
            }
            header(&r, "etag")
        }
        Err(e) => return step(e),
    };
    // the upload's own version, to tie the target's to it after the move
    if part_etag.is_none() {
        part_etag = match rw::head_part(cfg, path, token) {
            Ok(h) => send(&h, None).await.ok().filter(|r| r.status() == 200).and_then(|r| header(&r, "etag")),
            Err(e) => return step(e),
        };
    }
    let head = match rw::head_target(cfg, path) {
        Ok(r) => r,
        Err(e) => return step(e),
    };
    let (overwrite, seen) = match send(&head, None).await {
        Ok(r) => match rw::precondition(&known(path), r.status(), header(&r, "etag").as_deref()) {
            Precondition::Proceed { overwrite } => (overwrite, header(&r, "etag")),
            Precondition::Conflict => return Outcome::Conflict,
            Precondition::Exists => return Outcome::Exists,
            Precondition::Failed(e) => return step(e),
        },
        Err(e) => return step(e),
    };
    let mv = match rw::move_part(cfg, origin, path, token, overwrite, seen.as_deref()) {
        Ok(r) => r,
        Err(e) => return step(e),
    };
    let (outcome, moved_etag) = match send(&mv, None).await {
        Ok(r) => (rw::classify_move(r.status()), header(&r, "etag")),
        Err(e) => return step(e),
    };
    if outcome == Outcome::Saved {
        // The version this save made: the move's own, else the target's when it is the upload's
        // (see `saved_version`). A follow-up HEAD alone could show another writer's version.
        // `None`: the HEAD failed (not recorded as "no ETag"); `Some(None)`: it answered without one.
        let after = match send(&head, None).await {
            Ok(r) if r.status() == 200 => Some(header(&r, "etag")),
            _ => None,
        };
        match rw::saved_version(moved_etag.as_deref(), part_etag.as_deref(), after.as_ref().map(Option::as_deref)) {
            rw::SavedVersion::Etag(e) => note_opened(path, Some(e)),
            rw::SavedVersion::NoEtag => note_opened(path, None),
            rw::SavedVersion::Unbound => forget_version(path),
        }
        crate::remote::forget(path);
    }
    outcome
}
