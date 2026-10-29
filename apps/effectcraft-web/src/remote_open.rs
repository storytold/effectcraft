//! Opening a project from remote storage together with its media, and getting the remote media
//! of a restored session back.

use std::cell::Cell;
use std::collections::HashSet;
use std::sync::Arc;

use effectcraft_engine::project::{ItemKind, Project};
use effectcraft_ui_egui::EffectcraftApp;
use serde_json::json;

use crate::remote::{cfg_and_origin, ensure_stat, fetch_file, forget, is_remote};
use crate::remote_config;
use crate::remote_dav;
use crate::remote_http::{fetch, header, read_body};
use crate::remote_util::{self as util, run_bounded};

/// Media paths of one project fetched when it opens.
const MEDIA_MAX: usize = 10_000;

/// The outcome of [`fetch_all`]: the paths now in the file table, and the messages of the rest.
#[derive(Default)]
pub struct Fetched {
    pub ok: Vec<String>,
    pub failed: Vec<String>,
}

/// The media (and proxy) file paths of `proj`, relative ones made absolute next to the project at
/// `project` (`None`: the root).
fn footage_paths(proj: &Project, project: Option<&str>) -> Vec<String> {
    let dir = project.and_then(remote_config::parent).unwrap_or("/").trim_end_matches('/').to_string();
    let mut out = Vec::new();
    for it in proj.items.values() {
        let ItemKind::Footage(f) = &it.kind else { continue };
        let proxy = it.proxy.as_ref().map(|p| &p.footage);
        for f in std::iter::once(f).chain(proxy) {
            for p in std::iter::once(&f.path).chain(&f.sequence).filter(|p| !p.is_empty()) {
                out.push(if p.starts_with('/') { p.clone() } else { format!("{dir}/{p}") });
            }
        }
    }
    out
}

/// The media paths a project file refers to.
fn media_paths(project: &str, bytes: &[u8]) -> Vec<String> {
    let Ok(text) = std::str::from_utf8(bytes) else { return Vec::new() };
    let Ok((proj, _)) = Project::from_json_repaired(text) else { return Vec::new() };
    footage_paths(&proj, Some(project))
}

/// Fetch every remote file of `paths` a few at a time (`progress(done, total)` after each); a file
/// that fails or times out is reported and never holds the others back.
pub async fn fetch_all(paths: &[String], progress: impl Fn(usize, usize)) -> Fetched {
    let mut seen = HashSet::new();
    let todo: Vec<String> = paths.iter().filter(|p| is_remote(p) && seen.insert(p.as_str())).take(MEDIA_MAX).cloned().collect();
    let total = todo.len();
    let done = Cell::new(0usize);
    let results = run_bounded(todo, util::MEDIA_CONCURRENCY, |p| {
        let (done, progress) = (&done, &progress);
        async move {
            let r = fetch_file(&p).await;
            done.set(done.get() + 1);
            progress(done.get(), total);
            (p, r)
        }
    })
    .await;
    let mut out = Fetched::default();
    for (p, r) in results {
        match r {
            Ok(()) => out.ok.push(p),
            Err(e) => {
                log::info!("remote file {p}: {e}");
                out.failed.push(e);
            }
        }
    }
    out
}

/// Open the project at a remote path: fetch it and its media, then `file.open`. Media that cannot
/// be fetched stay missing (the engine reports them offline) and are named in the status line.
pub async fn open_project(path: &str) -> Result<(), String> {
    let status = util::opening_status(path);
    crate::post(move |app| app.ui.status = status);
    let st = ensure_stat(path).await?;
    if st.is_dir || st.size > remote_dav::MAX_PROJECT_BYTES {
        return Err(format!("{path}: not a project file this size can open"));
    }
    let (cfg, _) = cfg_and_origin()?;
    let r = fetch("GET", &cfg.url_for(path)?, &[], None).await?;
    if r.status() != 200 {
        return Err(format!("{path}: the server answered {}", r.status()));
    }
    let etag = header(&r, "etag");
    let bytes = read_body(&r, remote_dav::MAX_PROJECT_BYTES as usize).await?;
    let project = path.to_string();
    let failed = fetch_all(&media_paths(path, &bytes), |done, total| {
        let status = util::media_progress_status(&project, done, total);
        crate::post(move |app| app.ui.status = status);
    })
    .await
    .failed;
    crate::files::put_remote(path, Arc::from(bytes));
    let path = path.to_string();
    crate::post(move |app| match app.session.execute("file.open", json!({"path": path})) {
        Ok(_) => {
            // the version a save is checked against: only of a project that was read whole and opened
            crate::remote_upload::note_opened(&path, etag);
            app.ui.status = if failed.is_empty() { util::opened_status(&path) } else { util::media_failed_status(&failed) };
        }
        Err(e) => app.ui.status = e.to_string(),
    });
    Ok(())
}

/// After a session was restored from the last visit: when its project or footage is on remote
/// storage, note the project's ETag (so a save checks for a newer version) and fetch the media,
/// which the restore could not (it is synchronous). Footage that came back is reloaded so the
/// media pool decodes it; when something stays offline the status line says so.
pub fn refetch_restored(app: &EffectcraftApp) {
    if !crate::remote::enabled() {
        return;
    }
    let project = app.session.path.clone().filter(|p| is_remote(p));
    let media: Vec<String> = footage_paths(&app.session.project, app.session.path.as_deref()).into_iter().filter(|p| is_remote(p)).collect();
    if project.is_none() && media.is_empty() {
        return;
    }
    wasm_bindgen_futures::spawn_local(async move {
        let mut offline = false;
        if let Some(p) = &project {
            // the version on the server now is what a save is checked against
            forget(p);
            match ensure_stat(p).await {
                Ok(st) => crate::remote_upload::note_opened(p, st.etag),
                Err(e) => {
                    log::info!("restored project {p}: {e}");
                    offline = true;
                }
            }
        }
        let got = fetch_all(&media, |_, _| {}).await;
        offline |= !got.failed.is_empty();
        crate::post(move |app| {
            if let Some(importer) = app.session.importer.clone() {
                for p in &got.ok {
                    // the bytes are in the file table; this hands them to the media pool
                    importer.register(p, &[]);
                }
            }
            let stale: Vec<u64> = app
                .session
                .project
                .items
                .iter()
                .filter(
                    |(_, it)| matches!(&it.kind, ItemKind::Footage(f) if f.missing && std::iter::once(&f.path).chain(&f.sequence).any(|p| got.ok.contains(p))),
                )
                .map(|(id, _)| id.0)
                .collect();
            if !stale.is_empty()
                && let Err(e) = app.session.execute("file.reloadFootage", json!({"items": stale}))
            {
                app.ui.status = e.to_string();
            }
            if offline {
                app.ui.status = util::RESTORE_OFFLINE_STATUS.to_string();
            }
        });
    });
}
