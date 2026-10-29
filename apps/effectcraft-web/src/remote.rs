//! Remote storage in the browser: files on a same-origin WebDAV server appear under mounted
//! virtual paths (`./effectcraft-config.json`, see [`crate::remote_config`]) and are read with
//! `fetch` (PROPFIND listings, whole-file GETs). The pure parts live in [`crate::remote_config`]
//! and [`crate::remote_dav`]; this module is the browser side: config load, listing and stat
//! caches, fetching files into the file table, and opening a project together with its media.
//!
//! A remote file is fetched whole into the file table (not persisted) when a project or an import
//! needs it, up to [`FILE_MAX`] bytes: there is no ranged reader yet. The Media Browser lists
//! synchronously, so a listing starts on the first look and answers [`RemoteInfo::LOADING`].

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use effectcraft_engine::media_browser::{Entry, kind_of};
use effectcraft_ui_egui::{EffectcraftApp, RemoteInfo};
use serde_json::json;

use crate::remote_config::{self, Config};
use crate::remote_dav::{self, Child};
use crate::remote_http::{fetch, fetch_within, header, read_body};
use crate::remote_util::{self as util, ListingView};

/// The largest remote file fetched (bytes): it is held in memory, twice while it is copied in.
pub const FILE_MAX: u64 = 1 << 30;
/// A failed listing is not retried for this long (seconds).
const ERROR_RETRY_S: f64 = 5.0;
/// Entries kept in the stat cache.
const STAT_MAX: usize = 200_000;
/// Largest config file read.
pub(crate) const CONFIG_MAX: usize = 64 * 1024;

/// What the server said about one path.
#[derive(Clone, Debug)]
pub struct Stat {
    pub size: u64,
    pub is_dir: bool,
    pub etag: Option<String>,
}

#[derive(Default)]
struct DirState {
    entries: Option<Vec<Entry>>,
    fetched: f64,
    /// Whether the listing was handed out since it was fetched (a TTL of 0 refetches after that).
    shown: bool,
    /// When the request in flight started.
    loading: Option<f64>,
    error: Option<(String, f64)>,
}

#[derive(Default)]
struct State {
    cfg: Option<Config>,
    origin: String,
    stats: HashMap<String, Stat>,
    dirs: HashMap<String, DirState>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
    static T0: web_time::Instant = web_time::Instant::now();
}

/// Seconds since the page started (monotonic).
fn now_s() -> f64 {
    T0.with(|t| t.elapsed().as_secs_f64())
}

/// Whether remote storage is configured.
pub fn enabled() -> bool {
    STATE.with(|s| s.borrow().cfg.is_some())
}

/// Whether `path` is on a remote mount.
pub fn is_remote(path: &str) -> bool {
    STATE.with(|s| s.borrow().cfg.as_ref().is_some_and(|c| c.mount_for(path).is_some()))
}

/// The mounts as (name, path).
pub fn mounts() -> Vec<(String, String)> {
    STATE.with(|s| s.borrow().cfg.iter().flat_map(|c| c.mounts.iter()).map(|m| (m.name.clone(), m.path.clone())).collect())
}

/// The cached stat of a path (from a listing or a HEAD).
pub fn stat(path: &str) -> Option<Stat> {
    STATE.with(|s| s.borrow().stats.get(path.strip_suffix('/').unwrap_or(path)).cloned())
}

/// The remote config and the page's origin, or why remote storage is off.
pub(crate) fn cfg_and_origin() -> Result<(Config, String), String> {
    STATE.with(|s| {
        let s = s.borrow();
        s.cfg.clone().map(|c| (c, s.origin.clone())).ok_or_else(|| "remote storage is not configured".to_string())
    })
}

/// Read `./effectcraft-config.json`; remote storage stays off when it is missing or invalid.
pub async fn load_config() {
    let Some(loc) = web_sys::window().map(|w| w.location()) else { return };
    let (origin, path) = (loc.origin().unwrap_or_default(), loc.pathname().unwrap_or_default());
    let base_dir = path.get(..=path.rfind('/').unwrap_or(0)).unwrap_or("/").to_string();
    // A server that does not answer must not hold the app back: the time limit turns it into "off".
    let bytes = match fetch_within("GET", "effectcraft-config.json", &[], None, util::CONFIG_TIMEOUT).await {
        Ok(r) if r.status() == 200 => read_body(&r, CONFIG_MAX).await,
        Ok(r) => Err(format!("status {}", r.status())),
        Err(e) => Err(e),
    };
    match bytes.and_then(|b| remote_config::parse(&b, &origin, &base_dir)) {
        Ok(cfg) => {
            log::info!("remote storage: {} mount(s)", cfg.mounts.len());
            STATE.with(|s| {
                let mut s = s.borrow_mut();
                s.cfg = Some(cfg);
                s.origin = origin;
            });
        }
        Err(e) => log::info!("remote storage off: {e}"),
    }
}

/// PROPFIND `dir` (Depth 1) and map the answer to virtual paths.
async fn propfind(dir: &str) -> Result<Vec<Child>, String> {
    let (cfg, origin) = cfg_and_origin()?;
    let mount = cfg.mount_for(dir).ok_or_else(|| format!("{dir}: not on a remote mount"))?.clone();
    let url = cfg.url_for(&format!("{}/", dir.trim_end_matches('/')))?;
    let r = fetch("PROPFIND", &url, &[("Depth", "1"), ("Content-Type", "application/xml")], Some(remote_dav::PROPFIND_BODY.as_bytes())).await?;
    if r.status() != 207 {
        return Err(format!("{dir}: the server answered {} to a listing", r.status()));
    }
    let entries = remote_dav::parse_multistatus(&read_body(&r, remote_dav::MAX_XML_BYTES).await?)?;
    Ok(remote_dav::children_of(&cfg, &mount, dir, &origin, entries))
}

/// A listing row for a child of a folder.
fn entry_of(c: &Child) -> Entry {
    let is_dir = c.entry.is_dir;
    Entry {
        name: c.name.clone(),
        path: c.path.clone(),
        is_dir,
        size: c.entry.size.unwrap_or(0),
        modified: c.entry.modified,
        kind: if is_dir { None } else { kind_of(&c.path) },
    }
}

/// Store a listing's answer (entries and stats, or the error) for `dir` and end its loading state.
fn record_dir(dir: &str, res: Result<Vec<Child>, String>) {
    let now = now_s();
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let st = s.dirs.entry(dir.to_string()).or_default();
        st.loading = None;
        match res {
            Ok(children) => {
                st.error = None;
                st.fetched = now;
                st.shown = false;
                st.entries = Some(children.iter().map(entry_of).collect());
                if s.stats.len() > STAT_MAX {
                    s.stats.clear();
                }
                for c in children {
                    let stat = Stat { size: c.entry.size.unwrap_or(0), is_dir: c.entry.is_dir, etag: c.entry.etag };
                    s.stats.insert(c.path, stat);
                }
            }
            Err(e) => st.error = Some((e, now)),
        }
    });
}

/// Start the PROPFIND of `dir` in the background; the Media Browser sees it on a later frame.
fn start_listing(dir: String) {
    // Look again when the last error may give way to "Loading…".
    crate::repaint_after(Duration::from_secs_f64(util::LOADING_GRACE_S + 0.05));
    wasm_bindgen_futures::spawn_local(async move {
        let res = propfind(&dir).await;
        record_dir(&dir, res);
        // The Media Browser keeps a listing for a second: look again after that too.
        crate::repaint();
        crate::repaint_after(Duration::from_millis(1100));
    });
}

/// The Media Browser's listing of a directory under a mount; `None` for other paths. A listing
/// that is not here yet starts and answers [`RemoteInfo::LOADING`]; an expired one is shown while
/// it refreshes.
pub fn list_entries(dir: &str, importable_only: bool) -> Option<Result<Vec<Entry>, String>> {
    let dir = dir.trim_end_matches('/').to_string();
    if !is_remote(&dir) {
        return None;
    }
    let now = now_s();
    let (result, start) = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let ttl = s.cfg.as_ref().map_or(remote_config::DEFAULT_LIST_CACHE_S, |c| c.list_cache_s);
        let st = s.dirs.entry(dir.clone()).or_default();
        // A listing that was not shown yet is always fresh, so a TTL of 0 does not refetch in a loop.
        let fresh = st.entries.is_some() && (!st.shown || now - st.fetched < ttl);
        let blocked = st.loading.is_some() || st.error.as_ref().is_some_and(|(_, at)| now - at < ERROR_RETRY_S);
        let start = !fresh && !blocked;
        if start {
            st.loading = Some(now);
        }
        st.shown |= st.entries.is_some();
        // The last error stays up until the retry has run for a moment (no error/Loading flicker).
        let result = match (util::listing_view(st.entries.is_some(), st.error.is_some(), st.loading.map(|t| now - t)), &st.entries, &st.error) {
            (ListingView::Entries, Some(e), _) => Ok(e.clone()),
            (ListingView::Error, _, Some((msg, _))) => Err(msg.clone()),
            _ => Err(RemoteInfo::LOADING.to_string()),
        };
        (result, start)
    });
    if start {
        start_listing(dir);
    }
    Some(result.map(|mut v| {
        // Folders first, then files, by name; "media only" hides files of no known kind.
        v.retain(|e| e.is_dir || !importable_only || e.kind.is_some());
        v.sort_by_cached_key(|e| (!e.is_dir, e.name.to_lowercase()));
        v
    }))
}

/// A file's size: cached, else from its parent's listing, else `HEAD`.
pub async fn ensure_stat(path: &str) -> Result<Stat, String> {
    if let Some(s) = stat(path) {
        return Ok(s);
    }
    if let Some(parent) = remote_config::parent(path) {
        let res = propfind(parent).await;
        if let Err(e) = &res {
            log::info!("listing {parent}: {e}");
        }
        record_dir(parent, res);
        if let Some(s) = stat(path) {
            return Ok(s);
        }
    }
    let (cfg, _) = cfg_and_origin()?;
    let r = fetch("HEAD", &cfg.url_for(path)?, &[], None).await?;
    if r.status() != 200 {
        return Err(format!("{path}: the server answered {}", r.status()));
    }
    let size = header(&r, "content-length").and_then(|h| remote_dav::parse_content_length(&h)).ok_or_else(|| format!("{path}: the server gave no size"))?;
    let s = Stat { size, is_dir: false, etag: header(&r, "etag") };
    STATE.with(|st| st.borrow_mut().stats.insert(path.to_string(), s.clone()));
    Ok(s)
}

/// Fetch the file at remote `path` whole into the file table (not persisted), unless the table
/// already holds a file of that size. Refuses files over [`FILE_MAX`].
pub async fn fetch_file(path: &str) -> Result<(), String> {
    let st = ensure_stat(path).await?;
    if st.is_dir {
        return Err(format!("{path}: is a folder"));
    }
    if st.size > FILE_MAX {
        return Err(format!("{path}: {} MiB is over the {} MiB limit for files from remote storage", st.size >> 20, FILE_MAX >> 20));
    }
    if crate::files::get(path).is_some_and(|b| b.len() as u64 == st.size) {
        return Ok(());
    }
    let (cfg, _) = cfg_and_origin()?;
    let r = fetch("GET", &cfg.url_for(path)?, &[], None).await?;
    if r.status() != 200 {
        return Err(format!("{path}: the server answered {}", r.status()));
    }
    let bytes = read_body(&r, FILE_MAX as usize).await?;
    crate::files::put_remote(path, Arc::from(bytes));
    Ok(())
}

/// Drop what is cached about `path` and its folder (after it was written): the next look asks again.
pub fn forget(path: &str) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.stats.remove(path);
        if let Some(p) = remote_config::parent(path) {
            s.dirs.remove(p);
        }
    });
}

/// Route "open project by path" through the remote fetch and offer the remote menu entries.
pub fn install_hooks(app: &mut EffectcraftApp) {
    if !enabled() {
        return;
    }
    app.hooks.remote = Some(RemoteInfo { mounts: mounts() });
    app.hooks.open_project = Some(Box::new(|path: &str| {
        let path = path.to_string();
        if !is_remote(&path) {
            crate::post(move |app| {
                if let Err(e) = app.session.execute("file.open", json!({"path": path})) {
                    app.ui.status = e.to_string();
                }
            });
            return;
        }
        wasm_bindgen_futures::spawn_local(async move {
            if let Err(e) = crate::remote_open::open_project(&path).await {
                crate::post(move |app| app.ui.status = e);
            }
        });
    }));
}
