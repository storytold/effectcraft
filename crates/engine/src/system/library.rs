//! The Plug-in Library (Settings ▸ Plug-in Library; `library.*` commands): WebAssembly effect
//! plug-ins published by the community, listed in one reviewed catalog (an `index.json` in a Git
//! repository, see `docs/plugins.md#plug-in-library`), installed, updated and removed without
//! bundling them into the app.
//!
//! * The catalog names, per package, its releases: version, plug-in API, oldest EffectCraft it
//!   needs, the `.wasm` file's URL (a GitHub release asset) and its SHA-256. Nothing is compiled
//!   here: the published file is downloaded, checked against the catalog's SHA-256 and loaded by
//!   the host's [`PluginLoader`](crate::PluginLoader), so it runs in the same sandbox as any
//!   plug-in (no imports, fuel and memory limits).
//! * Installed packages live in `<Plug-ins>/Library/<id>/<version>/`, listed in
//!   `<Plug-ins>/Library/installed.json` with the SHA-256 they were installed with; the app loads
//!   them at start-up ([`load_installed`], `library.loadInstalled`). The catalog last fetched is
//!   kept as `catalog.json` there, so the list works offline.
//! * Effects can't be unregistered: removing or updating a package that is loaded takes effect
//!   at the next start, and the list says so (`restartNeeded`).
//! * Downloads use the system's `curl`, as trained models do ([`crate::models`]), so no
//!   networking code is linked into EffectCraft. Only `https://` URLs on GitHub's hosts are
//!   fetched; a local catalog (`file://`, for plug-in authors testing their entry) may also name
//!   local files.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use effectcraft_segment::sha256;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, b_p, bad, str_p};
use crate::{PluginLoader, Result, Session, cmd, query};

/// The community catalog.
pub const CATALOG_URL: &str = "https://raw.githubusercontent.com/storytold/effectcraft-plugins/main/index.json";
/// The catalog format this build reads.
pub const CATALOG_SCHEMA: u32 = 1;
/// Hosts plug-ins and catalogs may be downloaded from (release assets redirect to the last two).
const HOSTS: &[&str] = &["github.com", "raw.githubusercontent.com", "objects.githubusercontent.com", "release-assets.githubusercontent.com"];
const MAX_CATALOG: u64 = 4 << 20;
const MAX_MODULE: u64 = 64 << 20;
const MAX_PACKAGES: usize = 10_000;
const LIBRARY_DIR: &str = "Library";
const INSTALLED_FILE: &str = "installed.json";
const CACHE_FILE: &str = "catalog.json";

/// The catalog (`index.json`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    pub schema: u32,
    #[serde(default)]
    pub packages: Vec<Package>,
}

/// One plug-in package: one `.wasm` module per release.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Package {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub license: String,
    /// The source repository.
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub versions: Vec<Release>,
    /// The author's demo media.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<PackagePreview>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PackagePreview {
    /// An `https://` link to a short demo video (YouTube, Vimeo, GitHub…), opened in the browser.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<String>,
}

/// A package's effect rendered on the sample frame (`library.preview`), as straight RGBA8.
#[derive(Debug)]
pub struct Preview {
    pub id: String,
    pub version: String,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    /// The sample frame.
    pub before: Vec<u8>,
    /// The effect at default settings, one frame per `1 / fps` from time 0.
    pub frames: Vec<Vec<u8>>,
    /// The module's manifest (`{id, name, category, description, params…}`).
    pub manifest: Value,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Release {
    /// Dotted numbers (`1.2.0`).
    pub version: String,
    /// The plug-in API it implements.
    pub api: u32,
    /// The oldest EffectCraft version it runs in.
    #[serde(default, rename = "minApp", skip_serializing_if = "Option::is_none")]
    pub min_app: Option<String>,
    pub url: String,
    /// Lower-case hex SHA-256 of the file at `url`.
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// The effect ids the module registers (`library.missing` finds packages by them).
    #[serde(default)]
    pub effects: Vec<String>,
}

/// `installed.json`: package id → what is installed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Installed {
    #[serde(default)]
    pub packages: BTreeMap<String, InstalledPackage>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct InstalledPackage {
    pub version: String,
    /// The module, relative to the library folder (`<id>/<version>/<id>.wasm`).
    pub file: String,
    pub sha256: String,
    #[serde(default)]
    pub effects: Vec<String>,
}

/// `catalog.json`: the catalog last fetched, and from where.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Cache {
    url: String,
    fetched: String,
    catalog: Catalog,
}

/// What the library's worker reports, and what this session loaded.
#[derive(Clone, Debug, Default)]
pub struct Status {
    /// What is happening now ("Downloading Glow Pack 1.2.0…").
    pub busy: Option<String>,
    /// The last failure.
    pub error: Option<String>,
    /// A finished job's message and whether it failed, for a toast ([`Session::poll_library`]).
    pub done: Option<(String, bool)>,
    /// The catalog in use and where it came from.
    pub catalog: Option<(String, Arc<Catalog>)>,
    /// When it was fetched (RFC 3339).
    pub fetched: Option<String>,
    /// Packages whose module this session loaded: id → version.
    pub loaded: BTreeMap<String, String>,
    /// Packages removed or updated after they loaded: the change applies at the next start.
    pub restart: BTreeSet<String>,
    /// The last package previewed.
    pub preview: Option<Arc<Preview>>,
}

/// The session's Plug-in Library state (shared with its worker thread).
#[derive(Default)]
pub struct Library {
    status: Arc<Mutex<Status>>,
}

impl Library {
    /// A copy of the current status.
    pub fn status(&self) -> Status {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

/// Change the shared status (a poisoned lock is skipped, not a panic).
fn set(status: &Arc<Mutex<Status>>, f: impl FnOnce(&mut Status)) {
    if let Ok(mut s) = status.lock() {
        f(&mut s);
    }
}

impl Session {
    /// The effect plug-ins folder: the host's, else `Plug-ins` next to the settings.
    pub fn plugins_dir(&self) -> Option<PathBuf> {
        self.plugins_dir.clone().or_else(|| Some(self.config.as_ref()?.dir()?.join("Plug-ins")))
    }

    /// Where Plug-in Library packages are installed.
    pub fn library_dir(&self) -> Option<PathBuf> {
        self.plugins_dir().map(|d| d.join(LIBRARY_DIR))
    }

    /// Finish what the library's worker did: a toast.
    pub fn poll_library(&mut self) {
        let done = self.library.status.lock().ok().and_then(|mut s| s.done.take());
        if let Some((message, error)) = done {
            self.events.push(crate::Event::Toast { message, error });
        }
    }
}

// ---------------------------------------------------------------- catalog

/// Dotted numbers as a comparable list (`None`: not a version).
fn version_key(v: &str) -> Option<Vec<u64>> {
    if v.is_empty() || v.len() > 32 {
        return None;
    }
    let parts: Option<Vec<u64>> = v.split('.').map(|p| if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) { None } else { p.parse().ok() }).collect();
    let mut parts = parts?;
    while parts.len() > 1 && parts.last() == Some(&0) {
        parts.pop();
    }
    Some(parts)
}

/// `a` is newer than `b`.
fn newer(a: &str, b: &str) -> bool {
    match (version_key(a), version_key(b)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// A package id that is also a safe folder name: letters, digits, `.`, `-`, `_`; no leading dot.
pub fn valid_id(id: &str) -> bool {
    (1..=100).contains(&id.len()) && !id.starts_with('.') && !id.contains("..") && id.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

/// 64 hex digits.
fn valid_sha(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Why a release can't be used, if it can't.
fn release_problem(r: &Release) -> Option<String> {
    if version_key(&r.version).is_none() {
        return Some(format!("bad version `{}`", r.version));
    }
    if r.min_app.as_deref().is_some_and(|m| version_key(m).is_none()) {
        return Some(format!("{}: bad minApp", r.version));
    }
    if !valid_sha(&r.sha256) {
        return Some(format!("{}: bad sha256", r.version));
    }
    if r.url.is_empty() {
        return Some(format!("{}: no url", r.version));
    }
    None
}

/// Read a catalog: packages and releases that are malformed are left out (and reported), so one
/// bad entry doesn't hide the others.
pub fn parse_catalog(bytes: &[u8]) -> std::result::Result<(Catalog, Vec<String>), String> {
    if bytes.len() as u64 > MAX_CATALOG {
        return Err(format!("the catalog is larger than {} MB", MAX_CATALOG >> 20));
    }
    let raw: Catalog = serde_json::from_slice(bytes).map_err(|e| format!("the catalog isn't valid: {e}"))?;
    if raw.schema != CATALOG_SCHEMA {
        return Err(format!("the catalog uses format {} and this EffectCraft reads format {CATALOG_SCHEMA}: update EffectCraft", raw.schema));
    }
    let mut warnings = vec![];
    let mut seen = BTreeSet::new();
    let mut packages = vec![];
    for mut p in raw.packages.into_iter().take(MAX_PACKAGES) {
        if !valid_id(&p.id) || p.name.trim().is_empty() {
            warnings.push(format!("left out a package with a bad id or no name (`{}`)", p.id));
            continue;
        }
        if !seen.insert(p.id.clone()) {
            warnings.push(format!("left out a second package `{}`", p.id));
            continue;
        }
        p.versions.retain(|r| match release_problem(r) {
            Some(why) => {
                warnings.push(format!("{}: {why}", p.id));
                false
            }
            None => true,
        });
        for r in &mut p.versions {
            r.sha256.make_ascii_lowercase();
        }
        if let Some(video) = p.preview.as_ref().and_then(|v| v.video.as_deref())
            && !(video.starts_with("https://") && video.len() <= 500)
        {
            warnings.push(format!("{}: left out a preview video that isn't an https:// link", p.id));
            p.preview = None;
        }
        packages.push(p);
    }
    Ok((Catalog { schema: CATALOG_SCHEMA, packages }, warnings))
}

/// This build implements the release's plug-in API and is at least its `minApp`.
fn runs_here(r: &Release) -> bool {
    // Without a pre-release suffix (`0.18.0-dev` is 0.18.0).
    let app = env!("CARGO_PKG_VERSION").split(['-', '+']).next().unwrap_or_default();
    r.api == crate::effects::plugin::PLUGIN_API_VERSION && r.min_app.as_deref().is_none_or(|m| !newer(m, app))
}

/// The newest release this build can run (plug-in API and minimum app version).
pub fn latest_compatible(p: &Package) -> Option<&Release> {
    p.versions.iter().filter(|r| runs_here(r)).fold(None, |best: Option<&Release>, r| match best {
        Some(b) if !newer(&r.version, &b.version) => Some(b),
        _ => Some(r),
    })
}

// ---------------------------------------------------------------- fetching

/// Where a URL may be read from.
#[derive(Clone, Debug, PartialEq)]
enum Source {
    Https(String),
    File(PathBuf),
}

/// `file:///C:/x` or `file:///home/x` as a path (percent-decoded; no remote hosts).
fn file_url_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    if !rest.starts_with('/') {
        return None;
    }
    let b = rest.as_bytes();
    // `/C:/…` on Windows.
    let rest = if b.len() > 2 && b.get(2) == Some(&b':') { rest.get(1..)? } else { rest };
    let mut out = Vec::with_capacity(rest.len());
    let mut bytes = rest.bytes();
    while let Some(c) = bytes.next() {
        if c == b'%' {
            let hex = [bytes.next()?, bytes.next()?];
            out.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
        } else {
            out.push(c);
        }
    }
    String::from_utf8(out).ok().map(PathBuf::from)
}

/// Check `url` against the rules: `https://` on GitHub's hosts; `file://` when the catalog
/// (`catalog`, or `url` itself when it is the catalog) is local.
fn source(url: &str, catalog: Option<&str>) -> std::result::Result<Source, String> {
    if let Some(rest) = url.strip_prefix("https://") {
        let host = rest.split(['/', '?', '#']).next().unwrap_or_default().to_ascii_lowercase();
        if host.contains('@') || host.contains(':') || !HOSTS.contains(&host.as_str()) {
            return Err(format!("`{url}` isn't on GitHub: the Plug-in Library only downloads from {}", HOSTS.join(", ")));
        }
        return Ok(Source::Https(url.to_string()));
    }
    let local_catalog = catalog.is_none_or(|c| c.starts_with("file://"));
    if url.starts_with("file://") && local_catalog {
        return file_url_path(url).map(Source::File).ok_or_else(|| format!("bad file URL `{url}`"));
    }
    Err(format!("`{url}` isn't an https:// URL"))
}

/// Read `src` (at most `max` bytes), downloading into `tmp_dir`.
fn fetch(src: &Source, max: u64, tmp_dir: &Path) -> std::result::Result<Vec<u8>, String> {
    match src {
        Source::File(p) => {
            let len = std::fs::metadata(p).map_err(|e| format!("cannot read {}: {e}", p.display()))?.len();
            if len > max {
                return Err(format!("{} is larger than {} MB", p.display(), max >> 20));
            }
            std::fs::read(p).map_err(|e| format!("cannot read {}: {e}", p.display()))
        }
        Source::Https(url) => {
            std::fs::create_dir_all(tmp_dir).map_err(|e| format!("cannot create {}: {e}", tmp_dir.display()))?;
            // Unique per download: sessions in one process may download at the same time.
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let tmp = tmp_dir.join(format!(".download-{}-{n}.part", std::process::id()));
            // `-q` first: no ~/.curlrc; `--globoff`: `[]` and `{}` in a URL are literal.
            let out = std::process::Command::new("curl")
                .args([
                    "-q",
                    "--globoff",
                    "-fsSL",
                    "--proto",
                    "=https",
                    "--proto-redir",
                    "=https",
                    "--retry",
                    "2",
                    "--connect-timeout",
                    "20",
                    "--max-time",
                    "900",
                ])
                .args(["--max-filesize", &max.to_string(), "-o"])
                .arg(&tmp)
                .arg(url)
                .output()
                .map_err(|e| format!("downloading needs curl ({e})"))?;
            // The size is checked before reading: `--max-filesize` can't stop a stream of unknown length.
            let too_big = std::fs::metadata(&tmp).is_ok_and(|m| m.len() > max);
            let bytes = if too_big {
                Err(format!("the download is larger than {} MB", max >> 20))
            } else if out.status.success() {
                std::fs::read(&tmp).map_err(|e| format!("cannot read the download: {e}"))
            } else {
                Err(format!("download failed: {}", String::from_utf8_lossy(&out.stderr).trim()))
            };
            let _ = std::fs::remove_file(&tmp);
            let bytes = bytes?;
            if bytes.len() as u64 > max {
                return Err(format!("the download is larger than {} MB", max >> 20));
            }
            Ok(bytes)
        }
    }
}

// ---------------------------------------------------------------- installed packages

/// `installed.json`; missing or unreadable is an empty library.
fn read_installed(dir: &Path) -> Installed {
    std::fs::read(dir.join(INSTALLED_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Write `installed.json`.
fn write_installed(dir: &Path, inst: &Installed) -> std::result::Result<(), String> {
    use crate::config::FileOps;
    let text = serde_json::to_vec_pretty(inst).map_err(|e| e.to_string())?;
    crate::config::StdFiles.write(&dir.join(INSTALLED_FILE), &text).map_err(|e| format!("cannot write {INSTALLED_FILE}: {e}"))
}

/// An installed module's path, if `file` stays inside the library folder.
fn module_path(dir: &Path, file: &str) -> Option<PathBuf> {
    let parts: Vec<&str> = file.split('/').collect();
    let safe = parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && *p != "." && *p != ".." && !p.contains(['\\', ':']));
    safe.then(|| parts.iter().fold(dir.to_path_buf(), |d, p| d.join(p)))
}

/// Load every installed package's module (at start-up): each is checked against the SHA-256 it
/// was installed with. Returns `{loaded: [{package, version, effect…}], errors: [{package, error}]}`.
pub fn load_installed(s: &mut Session) -> Result<Value> {
    let c = "library.loadInstalled";
    let loader = s.plugin_loader.ok_or_else(|| bad(c, "WebAssembly effect plug-ins are not available in this build"))?;
    let Some(dir) = s.library_dir() else { return Ok(json!({"loaded": [], "errors": []})) };
    let inst = read_installed(&dir);
    let (mut loaded, mut errors) = (vec![], vec![]);
    for (id, p) in &inst.packages {
        if s.library.status().loaded.contains_key(id) {
            continue;
        }
        let r = module_path(&dir, &p.file)
            .ok_or_else(|| format!("bad module path `{}`", p.file))
            .and_then(|path| std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display())).map(|b| (path, b)))
            .and_then(|(path, bytes)| {
                if sha256::hex(&bytes) != p.sha256 {
                    return Err("the installed file changed since it was installed (SHA-256 differs): install it again".into());
                }
                loader(&bytes, &path.to_string_lossy())
            });
        match r {
            Ok(mut v) => {
                set(&s.library.status, |st| {
                    st.loaded.insert(id.clone(), p.version.clone());
                });
                v["package"] = json!(id);
                v["packageVersion"] = json!(p.version);
                loaded.push(v);
            }
            Err(e) => errors.push(json!({"package": id, "error": e})),
        }
    }
    Ok(json!({"loaded": loaded, "errors": errors}))
}

/// Download `rel` of `pkg` (temporary files in `dir`) and check it against the catalog.
fn download(dir: &Path, catalog_url: &str, pkg: &Package, rel: &Release) -> std::result::Result<Vec<u8>, String> {
    let src = source(&rel.url, Some(catalog_url))?;
    let bytes = fetch(&src, MAX_MODULE, dir)?;
    if rel.size.is_some_and(|n| n != bytes.len() as u64) || sha256::hex(&bytes) != rel.sha256 {
        return Err(format!("the downloaded file isn't the one the catalog lists for {} {} (size or SHA-256 differs)", pkg.name, rel.version));
    }
    Ok(bytes)
}

/// Download, check, write and load `rel` of `pkg` (the worker's job).
fn install_job(
    dir: &Path,
    catalog_url: &str,
    pkg: &Package,
    rel: &Release,
    loader: PluginLoader,
    status: &Arc<Mutex<Status>>,
) -> std::result::Result<String, String> {
    let bytes = download(dir, catalog_url, pkg, rel)?;
    let file = format!("{}/{}/{}.wasm", pkg.id, rel.version, pkg.id);
    let path = module_path(dir, &file).ok_or_else(|| format!("bad package id `{}`", pkg.id))?;
    let previous = status.lock().ok().and_then(|s| s.loaded.get(&pkg.id).cloned());
    let restart = match previous {
        // Its effects are registered already: the new version loads at the next start.
        Some(v) => v != rel.version,
        // Loaded before anything is written, so a module that fails leaves the disk as it was.
        None => {
            loader(&bytes, &path.to_string_lossy()).map_err(|e| format!("{} {} didn't load: {e}", pkg.name, rel.version))?;
            set(status, |s| {
                s.loaded.insert(pkg.id.clone(), rel.version.clone());
            });
            false
        }
    };
    {
        use crate::config::FileOps;
        crate::config::StdFiles.write(&path, &bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    }
    let mut inst = read_installed(dir);
    let old =
        inst.packages.insert(pkg.id.clone(), InstalledPackage { version: rel.version.clone(), file, sha256: rel.sha256.clone(), effects: rel.effects.clone() });
    write_installed(dir, &inst)?;
    // `installed.json` may be hand-edited: only a real version names a folder to delete.
    if let Some(old) = old.filter(|o| o.version != rel.version && version_key(&o.version).is_some()) {
        let _ = std::fs::remove_dir_all(dir.join(&pkg.id).join(&old.version));
    }
    set(status, |s| {
        if restart {
            s.restart.insert(pkg.id.clone());
        } else {
            s.restart.remove(&pkg.id);
        }
    });
    Ok(if restart {
        format!("{} {} installed: restart EffectCraft to use it", pkg.name, rel.version)
    } else {
        format!("{} {} installed", pkg.name, rel.version)
    })
}

/// Run `job` on a worker thread (`wait`: right here), reporting through `status`.
fn spawn(status: &Arc<Mutex<Status>>, what: String, wait: bool, job: impl FnOnce(&Arc<Mutex<Status>>) -> std::result::Result<String, String> + Send + 'static) {
    set(status, |s| {
        s.busy = Some(what);
        s.error = None;
    });
    let st = status.clone();
    let run = move || {
        let r = job(&st);
        set(&st, |s| {
            s.busy = None;
            match r {
                // An empty message: nothing to toast (the result shows where it was asked for).
                Ok(msg) if msg.is_empty() => {}
                Ok(msg) => s.done = Some((msg, false)),
                Err(e) => {
                    s.done = Some((format!("Plug-in Library: {e}"), true));
                    s.error = Some(e);
                }
            }
        });
    };
    if wait {
        run();
    } else if let Err(e) = std::thread::Builder::new().name("ec-library".into()).spawn(run) {
        set(status, |s| {
            s.busy = None;
            s.error = Some(format!("cannot start: {e}"));
        });
    }
}

// ---------------------------------------------------------------- commands

/// The library needs files and `curl`: not in the web build.
fn desktop_only(c: &str) -> Result<()> {
    if cfg!(target_arch = "wasm32") {
        return Err(bad(c, "the Plug-in Library is available in the desktop app"));
    }
    Ok(())
}

/// Fails while the worker runs a job (it takes one at a time).
fn idle(s: &Session, c: &str) -> Result<()> {
    match s.library.status().busy {
        Some(b) => Err(bad(c, format!("the Plug-in Library is busy ({b})"))),
        None => Ok(()),
    }
}

/// After a `wait`: the job's failure, as the command's.
fn waited(s: &Session, c: &str) -> Result<()> {
    match s.library.status().error {
        Some(e) => Err(bad(c, e)),
        None => Ok(()),
    }
}

/// The catalog in memory, else the one cached on disk.
fn catalog(s: &Session) -> Option<(String, Arc<Catalog>)> {
    if let Some(c) = s.library.status().catalog {
        return Some(c);
    }
    let dir = s.library_dir()?;
    let cache: Cache = serde_json::from_slice(&std::fs::read(dir.join(CACHE_FILE)).ok()?).ok()?;
    let (cat, _) = parse_catalog(&serde_json::to_vec(&cache.catalog).ok()?).ok()?;
    let c = (cache.url, Arc::new(cat));
    set(&s.library.status, |st| {
        st.catalog = Some(c.clone());
        st.fetched = Some(cache.fetched);
    });
    Some(c)
}

/// `library.refresh`: fetch the catalog (`catalog`, else the community one) and keep a copy for offline use.
fn refresh(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "library.refresh";
    desktop_only(c)?;
    idle(s, c)?;
    let url = str_p(p, "catalog").unwrap_or(CATALOG_URL).to_string();
    let src = source(&url, None).map_err(|e| bad(c, e))?;
    let dir = s.library_dir().ok_or_else(|| bad(c, "no settings folder for the Plug-in Library"))?;
    let wait = b_p(p, "wait").unwrap_or(false);
    spawn(&s.library.status, "Fetching the plug-in catalog…".into(), wait, move |st| {
        let bytes = fetch(&src, MAX_CATALOG, &dir)?;
        let (cat, warnings) = parse_catalog(&bytes)?;
        for w in &warnings {
            log::warn!("plug-in catalog: {w}");
        }
        let secs = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let fetched = chrono::DateTime::from_timestamp(i64::try_from(secs).unwrap_or(0), 0).map(|t| t.to_rfc3339()).unwrap_or_default();
        let cache = Cache { url: url.clone(), fetched: fetched.clone(), catalog: cat.clone() };
        if let Ok(text) = serde_json::to_vec(&cache) {
            use crate::config::FileOps;
            if let Err(e) = crate::config::StdFiles.write(&dir.join(CACHE_FILE), &text) {
                log::warn!("plug-in catalog: cannot keep a copy: {e}");
            }
        }
        let n = cat.packages.len();
        set(st, |s| {
            s.catalog = Some((url, Arc::new(cat)));
            s.fetched = Some(fetched);
        });
        Ok(format!("Plug-in catalog: {n} package(s)"))
    });
    if wait {
        waited(s, c)?;
        let n = s.library.status().catalog.map_or(0, |(_, c)| c.packages.len());
        return Ok(json!({"packages": n}));
    }
    Ok(json!({"refreshing": true}))
}

/// `q` (case-insensitive) is in the package's id, name, author, description, tags or effect ids.
fn matches(p: &Package, q: &str) -> bool {
    let q = q.to_lowercase();
    let latest = p.versions.iter().flat_map(|r| r.effects.iter());
    [&p.id, &p.name, &p.author, &p.description].into_iter().chain(p.tags.iter()).chain(latest).any(|f| f.to_lowercase().contains(&q))
}

/// One package as `library.list` reports it.
fn package_json(p: &Package, inst: Option<&InstalledPackage>, st: &Status) -> Value {
    let latest = latest_compatible(p);
    let newest_any = p.versions.iter().map(|r| r.version.as_str()).fold(None, |b: Option<&str>, v| if b.is_none_or(|b| newer(v, b)) { Some(v) } else { b });
    json!({
        "id": p.id,
        "name": p.name,
        "author": p.author,
        "description": p.description,
        "license": p.license,
        "repo": p.repo,
        "tags": p.tags,
        "latest": latest.map(|r| r.version.clone()),
        "size": latest.and_then(|r| r.size),
        "effects": latest.map(|r| r.effects.clone()).unwrap_or_default(),
        // A newer release this build can't run (it needs a newer EffectCraft or plug-in API).
        "needsNewerApp": newest_any.is_some_and(|v| latest.is_none_or(|l| newer(v, &l.version))),
        "installed": inst.map(|i| i.version.clone()),
        "updateAvailable": matches!((inst, latest), (Some(i), Some(l)) if newer(&l.version, &i.version)),
        "loaded": st.loaded.get(&p.id),
        "restartNeeded": st.restart.contains(&p.id),
        "video": p.preview.as_ref().and_then(|v| v.video.clone()),
        // `library.preview` rendered it (this session).
        "previewed": st.preview.as_ref().is_some_and(|v| v.id == p.id),
    })
}

/// `library.list`: the catalog packages with their install state, filtered, and installed ones it no longer lists.
fn list(s: &mut Session, p: &Value) -> Result<Value> {
    let cat = catalog(s);
    let st = s.library.status();
    let dir = s.library_dir();
    let inst = dir.as_deref().map(read_installed).unwrap_or_default();
    let (query, tag, only_installed) = (str_p(p, "query").unwrap_or("").trim(), str_p(p, "tag"), b_p(p, "installed").unwrap_or(false));
    let mut v = vec![];
    let mut known = BTreeSet::new();
    let mut tags = BTreeSet::new();
    if let Some((_, c)) = &cat {
        for pkg in &c.packages {
            known.insert(pkg.id.as_str());
            tags.extend(pkg.tags.iter().cloned());
            let i = inst.packages.get(&pkg.id);
            if (only_installed && i.is_none()) || (!query.is_empty() && !matches(pkg, query)) || tag.is_some_and(|t| !pkg.tags.iter().any(|x| x == t)) {
                continue;
            }
            v.push(package_json(pkg, i, &st));
        }
    }
    // Installed packages the catalog no longer lists (or no catalog yet).
    for (id, i) in &inst.packages {
        if known.contains(id.as_str()) || (!query.is_empty() && !id.to_lowercase().contains(&query.to_lowercase())) || tag.is_some() {
            continue;
        }
        let pkg = Package { id: id.clone(), name: id.clone(), ..Default::default() };
        let mut j = package_json(&pkg, Some(i), &st);
        j["effects"] = json!(i.effects);
        j["unlisted"] = json!(true);
        v.push(j);
    }
    Ok(json!({
        "catalog": cat.as_ref().map(|(u, _)| u.clone()),
        "fetched": st.fetched,
        "packages": v,
        "tags": tags,
        "busy": st.busy,
        "error": st.error,
        "folder": dir.map(|d| d.display().to_string()),
        "available": !cfg!(target_arch = "wasm32") && s.plugin_loader.is_some(),
    }))
}

/// `library.install`: download, check and load a release (`version`, else the newest compatible).
fn install(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "library.install";
    desktop_only(c)?;
    idle(s, c)?;
    let loader = s.plugin_loader.ok_or_else(|| bad(c, "WebAssembly effect plug-ins are not available in this build"))?;
    let (url, pkg, rel) = pick(s, c, p)?;
    let dir = s.library_dir().ok_or_else(|| bad(c, "no settings folder to install into"))?;
    let wait = b_p(p, "wait").unwrap_or(false);
    let what = format!("Installing {} {}…", pkg.name, rel.version);
    let (pid, version) = (pkg.id.clone(), rel.version.clone());
    spawn(&s.library.status, what, wait, move |st| install_job(&dir, &url, &pkg, &rel, loader, st));
    if wait {
        waited(s, c)?;
        let st = s.library.status();
        return Ok(json!({"id": pid, "version": version, "installed": true, "restartNeeded": st.restart.contains(&pid)}));
    }
    Ok(json!({"id": pid, "version": version, "installing": true}))
}

/// The catalog's URL, the package `id` names and its release: `version`, else the newest this
/// build runs. The release's URL is checked against the rules here, so the command fails rather
/// than the worker.
fn pick(s: &Session, c: &str, p: &Value) -> Result<(String, Package, Release)> {
    let id = str_p(p, "id").ok_or_else(|| bad(c, "missing `id` (a package in library.list)"))?;
    let (url, cat) = catalog(s).ok_or_else(|| bad(c, "no plug-in catalog yet: run library.refresh"))?;
    let pkg = cat.packages.iter().find(|x| x.id == id).cloned().ok_or_else(|| bad(c, format!("the catalog has no package `{id}`")))?;
    let rel = match str_p(p, "version") {
        Some(v) => pkg.versions.iter().find(|r| r.version == v).cloned().ok_or_else(|| bad(c, format!("{} has no version {v}", pkg.name)))?,
        None => {
            latest_compatible(&pkg).cloned().ok_or_else(|| bad(c, format!("{} has no release for this EffectCraft (plug-in API or app version)", pkg.name)))?
        }
    };
    if !runs_here(&rel) {
        return Err(bad(
            c,
            format!(
                "{} {} needs plug-in API {} and EffectCraft {} or later; this is EffectCraft {} with plug-in API {}",
                pkg.name,
                rel.version,
                rel.api,
                rel.min_app.as_deref().unwrap_or("(any)"),
                env!("CARGO_PKG_VERSION"),
                crate::effects::plugin::PLUGIN_API_VERSION
            ),
        ));
    }
    source(&rel.url, Some(&url)).map_err(|e| bad(c, e))?;
    Ok((url, pkg, rel))
}

const PREVIEW_W: u32 = 320;
const PREVIEW_H: u32 = 180;
const PREVIEW_FPS: f64 = 12.0;
const PREVIEW_FRAMES: usize = 24;

/// The preview's sample frame (opaque, premultiplied RGBA): a colour gradient with two discs and
/// a band of fine stripes, so colour, blur, edge and distortion effects all show.
fn sample_frame() -> Vec<[f32; 4]> {
    let (w, h) = (PREVIEW_W as f32, PREVIEW_H as f32);
    let mut px = Vec::with_capacity((PREVIEW_W * PREVIEW_H) as usize);
    for y in 0..PREVIEW_H {
        for x in 0..PREVIEW_W {
            let (u, v) = (x as f32 / w, y as f32 / h);
            let mut c = [0.15 + 0.7 * u, 0.2 + 0.5 * v, 0.85 - 0.6 * u];
            for (cx, cy, r, col) in [(0.3, 0.42, 0.24, [1.0, 0.85, 0.2]), (0.68, 0.5, 0.18, [0.95, 0.25, 0.35])] {
                // Coverage with a one-pixel soft edge.
                let a = (r * h - ((u - cx) * w).hypot((v - cy) * h)).clamp(0.0, 1.0);
                for (ch, k) in c.iter_mut().zip(col) {
                    *ch += (k - *ch) * a;
                }
            }
            if v > 0.82 {
                let s = if (x / 4) % 2 == 0 { 0.95 } else { 0.05 };
                c = [s; 3];
            }
            px.push([c[0], c[1], c[2], 1.0]);
        }
    }
    px
}

/// Premultiplied float pixels as straight RGBA8.
fn to_rgba8(px: &[[f32; 4]]) -> Vec<u8> {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    px.iter()
        .flat_map(|p| {
            let a = p[3].clamp(0.0, 1.0);
            let un = |c: f32| if a > 0.0 { c / a } else { 0.0 };
            [byte(un(p[0])), byte(un(p[1])), byte(un(p[2])), byte(a)]
        })
        .collect()
}

/// `library.preview`'s reply: size, frame count, whether the effect changes anything, and the manifest.
fn preview_json(v: &Preview) -> Value {
    json!({
        "id": v.id,
        "version": v.version,
        "width": v.width,
        "height": v.height,
        "fps": v.fps,
        "frames": v.frames.len(),
        // Whether the effect changes the sample frame at all, and over time.
        "changes": v.frames.iter().any(|f| *f != v.before),
        "animated": v.frames.windows(2).any(|w| w.first() != w.last()),
        "effect": v.manifest,
    })
}

/// Render a package's effect on the sample frame without installing it.
fn preview(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "library.preview";
    desktop_only(c)?;
    let previewer = s.plugin_previewer.ok_or_else(|| bad(c, "WebAssembly effect plug-ins are not available in this build"))?;
    let (url, pkg, rel) = pick(s, c, p)?;
    let same = |v: &Preview| v.id == pkg.id && v.version == rel.version;
    if let Some(v) = s.library.status().preview.filter(|v| same(v)) {
        return Ok(preview_json(&v));
    }
    idle(s, c)?;
    let dir = s.library_dir().ok_or_else(|| bad(c, "no settings folder for the Plug-in Library"))?;
    let wait = b_p(p, "wait").unwrap_or(false);
    let (pid, version) = (pkg.id.clone(), rel.version.clone());
    spawn(&s.library.status, format!("Previewing {} {}…", pkg.name, rel.version), wait, move |st| {
        let bytes = download(&dir, &url, &pkg, &rel)?;
        let before = sample_frame();
        let times: Vec<f64> = (0..PREVIEW_FRAMES).map(|i| i as f64 / PREVIEW_FPS).collect();
        let (manifest, frames) =
            previewer(&bytes, PREVIEW_W, PREVIEW_H, &before, &times).map_err(|e| format!("{} {} didn't render: {e}", pkg.name, rel.version))?;
        let v = Preview {
            id: pkg.id,
            version: rel.version,
            width: PREVIEW_W,
            height: PREVIEW_H,
            fps: PREVIEW_FPS,
            before: to_rgba8(&before),
            frames: frames.iter().map(|f| to_rgba8(f)).collect(),
            manifest,
        };
        set(st, |s| s.preview = Some(Arc::new(v)));
        Ok(String::new())
    });
    if wait {
        waited(s, c)?;
        if let Some(v) = s.library.status().preview.filter(|v| v.id == pid && v.version == version) {
            return Ok(preview_json(&v));
        }
    }
    Ok(json!({"id": pid, "version": version, "previewing": true}))
}

/// `library.update`: install the newest compatible release of every package (or `id`) with an update.
fn update(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "library.update";
    let list = list(s, &json!({"installed": true}))?;
    let due: Vec<String> = list["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|x| x["updateAvailable"].as_bool().unwrap_or(false))
        .filter_map(|x| x["id"].as_str().map(str::to_string))
        .filter(|x| str_p(p, "id").is_none_or(|want| want == x))
        .collect();
    if due.is_empty() {
        return Ok(json!({"updating": []}));
    }
    desktop_only(c)?;
    idle(s, c)?;
    let loader = s.plugin_loader.ok_or_else(|| bad(c, "WebAssembly effect plug-ins are not available in this build"))?;
    let jobs = due.iter().map(|id| pick(s, c, &json!({"id": id}))).collect::<Result<Vec<_>>>()?;
    let dir = s.library_dir().ok_or_else(|| bad(c, "no settings folder to install into"))?;
    let wait = b_p(p, "wait").unwrap_or(false);
    // One job for all of them: the worker runs one job at a time.
    spawn(&s.library.status, format!("Updating {} plug-in(s)…", jobs.len()), wait, move |st| {
        let done = jobs.iter().map(|(url, pkg, rel)| install_job(&dir, url, pkg, rel, loader, st)).collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(done.join("; "))
    });
    if wait {
        waited(s, c)?;
    }
    Ok(json!({"updating": due}))
}

/// `library.uninstall`: forget the package and delete its folder; loaded effects stay until restart.
fn uninstall(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "library.uninstall";
    idle(s, c)?;
    let id = str_p(p, "id").ok_or_else(|| bad(c, "missing `id`"))?;
    let dir = s.library_dir().ok_or_else(|| bad(c, "no settings folder"))?;
    let mut inst = read_installed(&dir);
    if inst.packages.remove(id).is_none() {
        return Ok(json!({"removed": false}));
    }
    write_installed(&dir, &inst).map_err(|e| bad(c, e))?;
    // It is uninstalled either way (it no longer loads); a folder left behind is only disk space.
    if valid_id(id)
        && let Err(e) = std::fs::remove_dir_all(dir.join(id))
    {
        log::warn!("plug-in library: cannot remove the folder of {id}: {e}");
    }
    let restart = s.library.status().loaded.contains_key(id);
    set(&s.library.status, |st| {
        if restart {
            st.restart.insert(id.to_string());
        }
    });
    s.toast(if restart { format!("Removed {id}: its effects stay until you restart EffectCraft") } else { format!("Removed {id}") });
    Ok(json!({"removed": true, "restartNeeded": restart}))
}

/// Effects used in the project that aren't registered, with the catalog packages that provide them.
fn missing(s: &mut Session, _: &Value) -> Result<Value> {
    let mut effects = BTreeSet::new();
    for (_, comp) in s.project.comps() {
        for l in &comp.layers {
            for g in l.effects().into_iter().flat_map(|fx| fx.groups()) {
                if let effectcraft_project::GroupKind::Effect { effect } = &g.kind
                    && crate::effects::find(effect).is_none()
                {
                    effects.insert(effect.clone());
                }
            }
        }
    }
    let cat = catalog(s);
    let found: Vec<Value> = effects
        .iter()
        .map(|e| {
            let pkgs: Vec<&str> = cat
                .iter()
                .flat_map(|(_, c)| c.packages.iter())
                .filter(|p| latest_compatible(p).is_some_and(|r| r.effects.iter().any(|x| x == e)))
                .map(|p| p.id.as_str())
                .collect();
            json!({"effect": e, "packages": pkgs})
        })
        .collect();
    Ok(json!({"effects": found}))
}

/// The `library.*` commands.
pub fn specs() -> Vec<CommandSpec> {
    vec![
        query!(
            "library.list",
            "Plug-in Library",
            "{query?, tag?, installed?: bool} → {catalog, fetched, packages: [{id, name, author, description, license, repo, tags, latest, effects, installed, updateAvailable, needsNewerApp, loaded, restartNeeded, video, previewed, unlisted?}], tags, busy, error, folder, available}",
            list
        ),
        query!(
            "library.missing",
            "Missing Effects in the Plug-in Library",
            "{} → {effects: [{effect, packages: [package id]}]} — effects the project uses that aren't installed, and the catalog packages that provide them",
            missing
        ),
        cmd!(
            "library.refresh",
            "Refresh Plug-in Catalog",
            [],
            None,
            "{catalog?: https:// or file:// URL of an index.json (default: the community catalog), wait?}",
            always,
            refresh
        ),
        cmd!(
            "library.preview",
            "Preview Plug-in",
            [],
            None,
            "{id, version?, wait?} → {id, version, width, height, fps, frames, changes, animated, effect: manifest} — downloads the release (checked like an install) and renders its effect on a sample frame at default settings, without installing it",
            always,
            preview
        ),
        cmd!(
            "library.install",
            "Install Plug-in",
            [],
            None,
            "{id, version?, wait?} — downloads the release, checks its SHA-256 against the catalog, loads it",
            always,
            install
        ),
        cmd!("library.update", "Update Plug-ins", [], None, "{id? (default: every package with an update), wait?}", always, update),
        cmd!("library.uninstall", "Remove Plug-in", [], None, "{id} — its effects stay registered until the next start", always, uninstall),
        cmd!("library.loadInstalled", "Load Installed Plug-ins", [], None, "{} → {loaded, errors} (the app runs it at start-up)", always, |s, _| {
            load_installed(s)
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A release of `version` for `api`, with a valid GitHub URL and checksum.
    fn rel(version: &str, api: u32, min_app: Option<&str>) -> Release {
        Release {
            version: version.into(),
            api,
            min_app: min_app.map(Into::into),
            url: "https://github.com/a/b/x.wasm".into(),
            sha256: "a".repeat(64),
            size: None,
            effects: vec![],
        }
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(newer("1.10.0", "1.9.9"));
        assert!(!newer("1.0", "1.0.0"));
        assert!(!newer("1.0.0", "1.0"));
        assert!(newer("2", "1.99"));
        assert!(!newer("x", "1.0"));
        assert_eq!(version_key("1..2"), None);
        assert_eq!(version_key("-1"), None);
        assert_eq!(version_key(&"9".repeat(40)), None);
    }

    #[test]
    fn ids_are_safe_folder_names() {
        for ok in ["org.example.glow", "a", "glow-pack_2"] {
            assert!(valid_id(ok), "{ok}");
        }
        for no in ["", ".hidden", "a/../b", "a\\b", "..", "a..b", "c:", "x y", &"a".repeat(101)] {
            assert!(!valid_id(no), "{no}");
        }
    }

    #[test]
    fn the_newest_compatible_release_wins() {
        let api = crate::effects::plugin::PLUGIN_API_VERSION;
        let p = Package {
            versions: vec![
                rel("1.0.0", api, None),
                rel("1.2.0", api, Some("0.1")),
                rel("1.1.0", api, None),
                rel("9.0.0", api, Some("999.0")),
                rel("3.0.0", api + 1, None),
            ],
            ..Default::default()
        };
        assert_eq!(latest_compatible(&p).map(|r| r.version.as_str()), Some("1.2.0"));
        let j = package_json(&p, None, &Status::default());
        assert_eq!(j["needsNewerApp"], true);
        assert!(latest_compatible(&Package::default()).is_none());
    }

    #[test]
    fn catalogs_leave_out_bad_entries() {
        let good = "a".repeat(64);
        let text = json!({"schema": 1, "packages": [
            {"id": "org.ok", "name": "Ok", "versions": [
                {"version": "1.0.0", "api": 1, "url": "https://github.com/x", "sha256": good.to_uppercase()},
                {"version": "nope", "api": 1, "url": "https://github.com/x", "sha256": good},
                {"version": "1.1.0", "api": 1, "url": "https://github.com/x", "sha256": "short"}
            ]},
            {"id": "org.ok", "name": "Second"},
            {"id": "../evil", "name": "Evil"},
            {"id": "org.noname", "name": " "},
            {"id": "org.extra", "name": "Extra", "unknownField": true}
        ]});
        let (c, warnings) = parse_catalog(text.to_string().as_bytes()).unwrap();
        let ids: Vec<&str> = c.packages.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["org.ok", "org.extra"]);
        assert_eq!(c.packages[0].versions.len(), 1);
        assert_eq!(c.packages[0].versions[0].sha256, good, "checksums are kept in lower case");
        assert_eq!(warnings.len(), 5, "{warnings:?}");
        assert!(parse_catalog(br#"{"schema": 2, "packages": []}"#).unwrap_err().contains("update EffectCraft"));
        assert!(parse_catalog(b"not json").is_err());
        assert!(parse_catalog(&vec![b' '; (MAX_CATALOG + 1) as usize]).unwrap_err().contains("larger"));
    }

    #[test]
    fn only_github_https_and_local_catalog_files_are_fetched() {
        assert!(matches!(source("https://github.com/a/b/releases/download/v1/x.wasm", Some(CATALOG_URL)), Ok(Source::Https(_))));
        assert!(matches!(source("https://GitHub.com/a", Some(CATALOG_URL)), Ok(Source::Https(_))));
        for no in [
            "http://github.com/a",
            "https://example.com/x.wasm",
            "https://github.com.evil.com/x",
            "https://user@github.com/x",
            "https://github.com:8443/x",
            "ftp://github.com/x",
        ] {
            assert!(source(no, Some(CATALOG_URL)).is_err(), "{no}");
        }
        // file:// only from a local catalog (or as the catalog).
        assert!(source("file:///tmp/x.wasm", Some(CATALOG_URL)).is_err());
        assert_eq!(source("file:///tmp/x.wasm", Some("file:///tmp/index.json")), Ok(Source::File(PathBuf::from("/tmp/x.wasm"))));
        assert_eq!(source("file:///C:/p/x.wasm", None), Ok(Source::File(PathBuf::from("C:/p/x.wasm"))));
        assert!(source("file://", None).is_err());
        // Percent-decoded; no remote hosts.
        assert_eq!(file_url_path("file:///C:/My%20Plug-ins/Jos%C3%A9%23.wasm"), Some(PathBuf::from("C:/My Plug-ins/José#.wasm")));
        assert_eq!(file_url_path("file://localhost/home/x.wasm"), Some(PathBuf::from("/home/x.wasm")));
        for no in ["file://server/share/x.wasm", "file:///bad%2", "file:///bad%zz"] {
            assert_eq!(file_url_path(no), None, "{no}");
        }
    }

    #[test]
    fn the_sample_frame_round_trips_to_rgba8() {
        let f = sample_frame();
        assert_eq!(f.len(), (PREVIEW_W * PREVIEW_H) as usize);
        assert!(f.iter().all(|p| p[3] == 1.0));
        assert_eq!(to_rgba8(&[[0.5, 0.25, 0.0, 0.5], [1.0, 2.0, -1.0, 0.0]]), [255, 128, 0, 128, 0, 0, 0, 0]);
    }

    #[test]
    fn module_paths_stay_in_the_library() {
        let d = Path::new("lib");
        assert_eq!(module_path(d, "a/1.0/a.wasm"), Some(d.join("a").join("1.0").join("a.wasm")));
        for no in ["../x/a.wasm", "a/../a.wasm", "a/1/..", "a.wasm", "a/1/b/c.wasm", "a//a.wasm", "C:/x/a.wasm", "a\\..\\b/1/x"] {
            assert_eq!(module_path(d, no), None, "{no}");
        }
    }
}
