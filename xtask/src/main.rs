//! Workspace automation: `cargo xtask <layers|assets|pdf-deps|wasm|web|ico|ci>`.
//!
//! - `layers`: enforces the dependency layering of `docs/architecture.md` §1 (downward-only edges,
//!   listed same-layer edges, no UI/OS crates below L5).
//! - `wasm`: `cargo check --target wasm32-unknown-unknown` for every crate in L0–L4, the egui UI
//!   and the web app.
//! - `web [--dev] [--serve PORT]`: build the web app (`apps/effectcraft-web`) into
//!   `<target>/web/dist` with `wasm-bindgen` (docs/web.md); `--serve` serves it on localhost.
//! - `assets`: every asset file (image, icon, font, LUT, audio, video…) has a complete
//!   `<file>.attribution` sidecar and an entry in `ATTRIBUTION.md` (AGENTS.md §1).
//! - `ico <out.ico> <in.png>…`: pack PNGs into a Windows `.ico` (used by `packaging/icons.sh`).
//! - `ci`: fmt check, clippy -D warnings, tests, PDF dependency policy, layers, assets, wasm.
//! - `bench [--bin CLI] [--quick] [--compare OLD.jsonl] …`: render benchmark of generated scenarios
//!   (memory, CPU, wall time) into `target/bench/results.jsonl` (`bench.rs`, docs/testing.md).

mod bench;
mod ico;
mod pdf_deps;
mod version;

use std::process::{Command, ExitCode};

use serde_json::Value;

/// (crate name without the `effectcraft-` prefix, layer). See `docs/architecture.md` §1.
const LAYERS: &[(&str, u8)] = &[
    ("time", 0),
    ("geom", 0),
    ("color", 0),
    ("vp9enc", 0),
    ("testkit", 0),
    ("opusenc", 0),
    ("hevcenc", 0),
    ("av1enc", 0),
    ("raster", 1),
    ("keyframe", 1),
    ("path", 1),
    ("psd", 1),
    ("segment", 1),
    ("project", 2),
    ("text", 2),
    ("effects", 2),
    ("track", 2),
    ("svg", 2),
    ("pdf", 2),
    ("model", 2),
    ("render", 3),
    ("media", 3),
    ("expr", 3),
    ("export", 3),
    ("gpu", 3),
    ("lottie", 3),
    ("format", 3),
    ("plugin", 3),
    ("interchange", 3),
    ("engine", 4),
    ("script", 4),
    ("host", 4),
    ("ui-egui", 5),
    ("automation", 5),
    ("effectcraft", 6),
    ("android", 6),
    ("cli", 6),
    ("web", 6),
];

/// Allowed same-layer edges (from, to).
const SAME_LAYER: &[(&str, &str)] = &[
    ("path", "keyframe"),
    ("path", "raster"),
    ("text", "path"),
    ("pdf", "svg"),
    ("pdf", "text"),
    ("effects", "project"),
    ("effects", "text"),
    ("effects", "path"),
    ("effects", "track"),
    ("media", "render"),
    ("expr", "render"),
    ("export", "render"),
    ("export", "media"),
    ("gpu", "render"),
    ("lottie", "format"),
    ("host", "engine"),
    ("script", "engine"),
    ("host", "script"),
    ("cli", "effectcraft"),
    ("android", "effectcraft"),
];

/// Crates that must not appear below L5 (UI toolkits, windowing, OS audio/menus).
const UI_ONLY: &[&str] = &["craft-ui", "egui", "eframe", "egui-wgpu", "winit", "rfd", "cpal", "muda"];

fn short(name: &str) -> &str {
    name.strip_prefix("effectcraft-").unwrap_or(name)
}

fn layer_of(name: &str) -> Option<u8> {
    LAYERS.iter().find(|(n, _)| *n == short(name)).map(|(_, l)| *l)
}

fn metadata() -> Result<Value, String> {
    let out = Command::new(env!("CARGO")).args(["metadata", "--format-version", "1", "--no-deps"]).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into());
    }
    serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())
}

fn workspace_crates(md: &Value) -> Vec<(String, Vec<String>)> {
    md["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| p["name"] != "xtask")
        .map(|p| {
            let deps = p["dependencies"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|d| d["kind"].is_null() || d["kind"] == "build")
                .filter_map(|d| d["name"].as_str().map(str::to_string))
                .collect();
            (p["name"].as_str().unwrap_or_default().to_string(), deps)
        })
        .collect()
}

fn layers() -> Result<(), String> {
    let md = metadata()?;
    let mut errors = Vec::new();
    for (name, deps) in workspace_crates(&md) {
        let Some(l) = layer_of(&name) else {
            errors.push(format!("{name}: not assigned a layer (add it to xtask LAYERS and docs/architecture.md §1)"));
            continue;
        };
        for d in &deps {
            if l < 5 && UI_ONLY.contains(&d.as_str()) {
                errors.push(format!("{name} (L{l}) depends on UI/OS crate `{d}`"));
            }
            if !d.starts_with("effectcraft-") {
                continue;
            }
            let Some(dl) = layer_of(d) else { continue };
            let (a, b) = (short(&name), short(d));
            if dl > l {
                errors.push(format!("{a} (L{l}) depends upward on {b} (L{dl})"));
            } else if dl == l && l > 0 && !SAME_LAYER.contains(&(a, b)) {
                errors.push(format!("{a} → {b}: same-layer edge (L{l}) not in the allowed list"));
            }
        }
    }
    if errors.is_empty() {
        println!("layers: ok");
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

/// File extensions that count as assets (AGENTS.md §1).
const ASSET_EXT: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "svg", "ico", "icns", "bmp", "tif", "tiff", "heic", "avif", "exr", "ttf", "otf", "ttc", "woff", "woff2", "cube",
    "3dl", "lut", "look", "wav", "mp3", "aac", "flac", "ogg", "opus", "m4a", "aif", "aiff", "mp4", "mov", "m4v", "mkv", "webm", "avi", "mxf", "psd", "ai",
    "eps", "pdf", "prproj", "ffx", "prfpset", "mogrt", "aep",
];

/// Sidecar fields that must be present and non-empty.
const REQUIRED_FIELDS: &[&str] = &["asset", "title", "author", "source", "license", "added"];

fn repo_files() -> Result<Vec<String>, String> {
    let out = Command::new("git").args(["ls-files", "--cached", "--others", "--exclude-standard"]).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).lines().map(str::to_string).filter(|f| std::path::Path::new(f).exists()).collect())
}

fn is_asset(path: &str) -> bool {
    std::path::Path::new(path).extension().and_then(|e| e.to_str()).is_some_and(|e| ASSET_EXT.contains(&e.to_ascii_lowercase().as_str()))
}

/// The one directory whose attribution sidecars are dot-prefixed. It is the Android resource tree
/// `cargo apk` hands to `aapt -S`: `aapt` rejects a second resource name in a directory, so it
/// refuses `mipmap-mdpi/ic_launcher.png.attribution`, but it does skip dot files. Nothing else in
/// the repository may use that spelling — see `packaging/icons.sh`.
const ANDROID_RES: &str = "apps/effectcraft-android/res/";

/// The sidecar file for the asset `asset`, following [`ANDROID_RES`]'s dot-prefixed exception.
fn sidecar_for(asset: &str) -> String {
    let leaf = std::path::Path::new(asset).file_name().and_then(|n| n.to_str()).unwrap_or("");
    match asset.strip_prefix(ANDROID_RES).and_then(|_| asset.rsplit_once('/')) {
        Some((dir, _)) => format!("{dir}/.{leaf}.attribution"),
        None => format!("{asset}.attribution"),
    }
}

/// The asset a sidecar belongs to, mirroring [`sidecar_for`]; `None` when `path` is not a sidecar.
fn asset_for(path: &str) -> Option<String> {
    let stem = path.strip_suffix(".attribution")?;
    if let Some((dir, leaf)) = stem.rsplit_once('/')
        && stem.starts_with(ANDROID_RES)
        && leaf.starts_with('.')
    {
        return Some(format!("{dir}/{}", &leaf[1..]));
    }
    Some(stem.to_string())
}

fn assets() -> Result<(), String> {
    let files = repo_files()?;
    let index = std::fs::read_to_string("ATTRIBUTION.md").map_err(|e| format!("ATTRIBUTION.md: {e}"))?;
    let mut errors = Vec::new();
    let mut n = 0;
    for f in files.iter().filter(|f| is_asset(f)) {
        n += 1;
        let lower = f.to_ascii_lowercase();
        if lower.contains("adobe") || lower.contains("aftereffects") || lower.contains("after-effects") {
            errors.push(format!("{f}: asset paths must not reference Adobe/After Effects (AGENTS.md §1)"));
        }
        let side = sidecar_for(f);
        match std::fs::read_to_string(&side) {
            Err(_) => errors.push(format!("{f}: missing attribution sidecar {side}")),
            Ok(text) => {
                for field in REQUIRED_FIELDS {
                    let ok = text.lines().any(|l| l.split_once(':').is_some_and(|(k, v)| k.trim() == *field && !v.trim().is_empty()));
                    if !ok {
                        errors.push(format!("{side}: field `{field}` missing or empty"));
                    }
                }
                let lic = text.lines().find_map(|l| l.split_once(':').filter(|(k, _)| k.trim() == "license").map(|(_, v)| v.trim().to_ascii_lowercase()));
                if lic.is_some_and(|l| l.contains("-nc") || l.contains("-nd") || l.contains("adobe") || l.contains("proprietary")) {
                    errors.push(format!("{side}: licence not allowed (no NC/ND, Adobe or proprietary licences)"));
                }
            }
        }
        if !index.contains(&format!("`{f}`")) {
            errors.push(format!("{f}: not listed in ATTRIBUTION.md"));
        }
    }
    for f in files.iter().filter(|f| f.ends_with(".attribution")) {
        match asset_for(f) {
            Some(asset) if !std::path::Path::new(&asset).exists() => {
                errors.push(format!("{f}: sidecar for a file that does not exist"));
            }
            Some(_) => {}
            None => errors.push(format!("{f}: not an attribution sidecar")),
        }
    }
    if errors.is_empty() {
        println!("assets: ok ({n} assets attributed)");
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

fn run(cmd: &mut Command) -> Result<(), String> {
    eprintln!("$ {cmd:?}");
    let st = cmd.status().map_err(|e| e.to_string())?;
    if st.success() { Ok(()) } else { Err(format!("failed: {cmd:?}")) }
}

/// Crates above L4 that must also build for the web.
const WEB_CRATES: &[&str] = &["effectcraft-ui-egui", "effectcraft-web"];

fn wasm() -> Result<(), String> {
    let md = metadata()?;
    let mut cmd = Command::new(env!("CARGO"));
    cmd.args(["check", "--target", "wasm32-unknown-unknown"]);
    let mut n = 0;
    for (name, _) in workspace_crates(&md) {
        if layer_of(&name).is_some_and(|l| l <= 4) || WEB_CRATES.contains(&name.as_str()) {
            cmd.args(["-p", &name]);
            n += 1;
        }
    }
    if n == 0 {
        return Ok(());
    }
    run(&mut cmd)?;
    println!("wasm: ok ({n} crates)");
    Ok(())
}

/// The wasm-bindgen CLI must match the `wasm-bindgen` crate version exactly.
const WASM_BINDGEN: &str = "0.2.129";

fn target_dir() -> std::path::PathBuf {
    std::env::var_os("CARGO_TARGET_DIR").map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from("target"))
}

/// Build the web app into `<target>/web/dist` (see docs/web.md).
fn web(args: &[String]) -> Result<(), String> {
    let dev = args.iter().any(|a| a == "--dev");
    let serve = args.iter().position(|a| a == "--serve").map(|i| args.get(i + 1).and_then(|p| p.parse::<u16>().ok()).unwrap_or(8765));
    let profile = if dev { "dev" } else { "release" };
    let mut build = Command::new(env!("CARGO"));
    build.args(["build", "--target", "wasm32-unknown-unknown", "-p", "effectcraft-web", "--profile", profile]);
    run(&mut build)?;
    let out = Command::new("wasm-bindgen")
        .arg("--version")
        .output()
        .map_err(|_| format!("wasm-bindgen CLI not found: cargo install wasm-bindgen-cli --version {WASM_BINDGEN} --locked"))?;
    let v = String::from_utf8_lossy(&out.stdout);
    if !v.contains(WASM_BINDGEN) {
        return Err(format!(
            "wasm-bindgen CLI is {} but the crate is {WASM_BINDGEN}: cargo install wasm-bindgen-cli --version {WASM_BINDGEN} --locked",
            v.trim()
        ));
    }
    let dir = if dev { "debug" } else { "release" };
    let wasm = target_dir().join("wasm32-unknown-unknown").join(dir).join("effectcraft_web.wasm");
    let dist = target_dir().join("web").join("dist");
    let _ = std::fs::remove_dir_all(&dist);
    std::fs::create_dir_all(&dist).map_err(|e| e.to_string())?;
    run(Command::new("wasm-bindgen").args(["--target", "web", "--no-typescript", "--out-dir"]).arg(&dist).arg(&wasm))?;
    let bg = dist.join("effectcraft_web_bg.wasm");
    if !dev && Command::new("wasm-opt").arg("--version").output().is_ok() {
        // optional: smaller and faster (binaryen); skipped when not installed
        let opt = dist.join("effectcraft_web_opt.wasm");
        run(Command::new("wasm-opt")
            .args(["-O2", "--enable-bulk-memory", "--enable-nontrapping-float-to-int", "--enable-sign-ext", "--enable-mutable-globals"])
            .arg(&bg)
            .arg("-o")
            .arg(&opt))?;
        std::fs::rename(&opt, &bg).map_err(|e| e.to_string())?;
    }
    let web = std::path::Path::new("apps/effectcraft-web/web");
    for e in std::fs::read_dir(web).map_err(|e| e.to_string())?.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !name.ends_with(".attribution") {
            std::fs::copy(e.path(), dist.join(&name)).map_err(|e| format!("{name}: {e}"))?;
        }
    }
    // PWA icons: the app icon (assets/app-icon, attributed there).
    for n in [256, 512] {
        let src = format!("assets/app-icon/hicolor/{n}x{n}/apps/ai.storyteller.effectcraft.png");
        std::fs::copy(&src, dist.join(format!("icon-{n}.png"))).map_err(|e| format!("{src}: {e}"))?;
    }
    service_worker(&dist)?;
    let size = |p: &std::path::Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    let total: u64 = dist_files(&dist).iter().map(|f| size(&dist.join(f))).sum();
    println!("web: {} ({:.1} MB wasm, {:.1} MB total)", dist.display(), size(&bg) as f64 / 1e6, total as f64 / 1e6);
    if let Some(port) = serve {
        serve_dir(&dist, port)?;
    }
    Ok(())
}

/// Every file under `dist`, as `/`-separated relative paths, sorted.
fn dist_files(dist: &std::path::Path) -> Vec<String> {
    fn walk(dir: &std::path::Path, rel: &str, out: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let r = if rel.is_empty() { name } else { format!("{rel}/{name}") };
            if e.path().is_dir() {
                walk(&e.path(), &r, out);
            } else {
                out.push(r);
            }
        }
    }
    let mut v = vec![];
    walk(dist, "", &mut v);
    v.sort();
    v
}

/// Fill the service worker's precache list and version (a hash of the build's files).
fn service_worker(dist: &std::path::Path) -> Result<(), String> {
    let sw = dist.join("sw.js");
    let text = std::fs::read_to_string(&sw).map_err(|e| format!("sw.js: {e}"))?;
    let files: Vec<String> = dist_files(dist).into_iter().filter(|f| f != "sw.js").collect();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for f in &files {
        for b in f.bytes().chain(std::fs::read(dist.join(f)).map_err(|e| format!("{f}: {e}"))?) {
            h ^= b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    let list: Vec<String> = std::iter::once("./".to_string()).chain(files.iter().map(|f| format!("./{f}"))).map(|f| format!("{f:?}")).collect();
    let text = text.replace("__EC_VERSION__", &format!("{h:016x}")).replace("__EC_FILES__", &format!("[{}]", list.join(", ")));
    std::fs::write(&sw, text).map_err(|e| format!("sw.js: {e}"))
}

/// A tiny static file server for the web build (localhost only). Sends the cross-origin
/// isolation headers so `crossOriginIsolated` is true, like a production deployment would.
fn serve_dir(dir: &std::path::Path, port: u16) -> Result<(), String> {
    use std::io::{BufRead, BufReader, Write};
    let listener = std::net::TcpListener::bind(("127.0.0.1", port)).map_err(|e| format!("127.0.0.1:{port}: {e}"))?;
    println!("serving {} on http://127.0.0.1:{port}/", dir.display());
    for stream in listener.incoming().flatten() {
        let dir = dir.to_path_buf();
        std::thread::spawn(move || {
            let mut stream = stream;
            let mut line = String::new();
            let mut reader = BufReader::new(match stream.try_clone() {
                Ok(s) => s,
                Err(_) => return,
            });
            if reader.read_line(&mut line).is_err() {
                return;
            }
            loop {
                let mut h = String::new();
                if reader.read_line(&mut h).map(|n| n == 0).unwrap_or(true) || h.trim().is_empty() {
                    break;
                }
            }
            let path = line.split_whitespace().nth(1).unwrap_or("/").split(['?', '#']).next().unwrap_or("/").to_string();
            let rel = if path == "/" { "index.html".to_string() } else { path.trim_start_matches('/').replace("..", "") };
            let file = dir.join(&rel);
            let (status, body) = match std::fs::read(&file) {
                Ok(b) => ("200 OK", b),
                Err(_) => ("404 Not Found", b"not found".to_vec()),
            };
            let mime = match file.extension().and_then(|e| e.to_str()).unwrap_or("") {
                "html" => "text/html; charset=utf-8",
                "js" => "text/javascript",
                "wasm" => "application/wasm",
                "svg" => "image/svg+xml",
                "json" | "webmanifest" => "application/json",
                "png" => "image/png",
                "gif" => "image/gif",
                "ecproj" => "application/json",
                "mp4" => "video/mp4",
                "webm" => "video/webm",
                _ => "application/octet-stream",
            };
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nCross-Origin-Opener-Policy: same-origin\r\nCross-Origin-Embedder-Policy: require-corp\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes()).and_then(|_| stream.write_all(&body));
        });
    }
    Ok(())
}

/// The guidance printed when the ALSA development package is missing. Split out so the test can
/// check the package names without needing a machine that is actually missing them.
fn audio_backend_hint(found: bool) -> Result<(), String> {
    if found {
        return Ok(());
    }
    Err("Linux audio needs the ALSA development package, and pkg-config cannot find it.\n\
         The desktop app plays preview sound through cpal, whose alsa-sys dependency asks\n\
         pkg-config for `alsa`; without the package the build dies minutes later with an\n\
         unrelated-looking linker error.\n\
         \n\
         Fedora:         sudo dnf install alsa-lib-devel\n\
         Debian/Ubuntu:  sudo apt install libasound2-dev\n\
         Arch:           sudo pacman -S alsa-lib\n\
         \n\
         Install one of those and run `cargo xtask ci` again. If you cannot install it, point\n\
         PKG_CONFIG_PATH at a directory holding an `alsa.pc` whose Libs line names your system's\n\
         libasound (usually `libasound.so.2`); every CI run will need that variable set."
        .into())
}

/// `cargo xtask ci` builds the desktop app on the host, so on Linux it needs ALSA headers to be
/// present. Checking here turns a late, confusing link error into one line naming the package.
#[cfg(target_os = "linux")]
fn audio_backend() -> Result<(), String> {
    let found = Command::new("pkg-config").args(["--exists", "alsa"]).status().map(|s| s.success()).unwrap_or(false);
    audio_backend_hint(found)
}

#[cfg(not(target_os = "linux"))]
fn audio_backend() -> Result<(), String> {
    Ok(())
}

fn ci() -> Result<(), String> {
    audio_backend()?;
    let cargo = env!("CARGO");
    run(Command::new(cargo).args(["fmt", "--check"]))?;
    run(Command::new(cargo).args(["clippy", "--workspace", "--all-targets", "--release", "--", "-D", "warnings"]))?;
    run(Command::new(cargo).args(["test", "--workspace", "--release"]))?;
    pdf_deps::run(&root())?;
    layers()?;
    assets()?;
    wasm()
}

fn main() -> ExitCode {
    let task = std::env::args().nth(1).unwrap_or_default();
    let r = match task.as_str() {
        "layers" => layers(),
        "wasm" => wasm(),
        "assets" => assets(),
        "pdf-deps" => pdf_deps::run(&root()),
        "ci" => ci(),
        "web" => web(&std::env::args().skip(2).collect::<Vec<_>>()),
        "bench" => bench::run(&root(), &std::env::args().skip(2).collect::<Vec<_>>()),
        "ico" => {
            let rest: Vec<String> = std::env::args().skip(2).collect();
            ico::run(&rest.iter().map(String::as_str).collect::<Vec<_>>())
        }
        "version" => {
            let rest: Vec<String> = std::env::args().skip(2).collect();
            version::run(&root(), &rest.iter().map(String::as_str).collect::<Vec<_>>())
        }
        _ => Err("usage: cargo xtask <layers|assets|pdf-deps|wasm|web [--dev] [--serve PORT]|ico OUT.ico IN.png…|version [set X.Y.Z]|ci|bench [--bin CLI] [--quick] [--project F --comp C] [--compare OLD.jsonl] (--help)>".into()),
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// The workspace root (the parent of `xtask/`).
pub fn root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("xtask has a parent dir").to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::{ANDROID_RES, asset_for, audio_backend_hint, sidecar_for};

    #[test]
    fn alsa_present_is_not_an_error() {
        assert!(audio_backend_hint(true).is_ok());
    }

    #[test]
    fn alsa_missing_names_the_package_for_each_distro() {
        let e = audio_backend_hint(false).expect_err("missing ALSA must fail CI");
        for want in ["alsa-lib-devel", "libasound2-dev", "alsa-lib", "pkg-config", "PKG_CONFIG_PATH"] {
            assert!(e.contains(want), "hint lost `{want}`:\n{e}");
        }
    }

    #[test]
    fn sidecars_are_next_to_their_asset_everywhere_but_the_android_res_tree() {
        assert_eq!(sidecar_for("assets/app-icon/effectcraft.ico"), "assets/app-icon/effectcraft.ico.attribution");
        let res = format!("{ANDROID_RES}mipmap-mdpi/ic_launcher.png");
        assert_eq!(sidecar_for(&res), format!("{ANDROID_RES}mipmap-mdpi/.ic_launcher.png.attribution"));
    }

    #[test]
    fn sidecar_and_asset_round_trip() {
        for asset in ["assets/fonts/Inter-Regular.ttf", &format!("{ANDROID_RES}mipmap-mdpi/ic_launcher.png")] {
            let side = sidecar_for(asset);
            assert_eq!(asset_for(&side).as_deref(), Some(asset), "{side}");
        }
        assert_eq!(asset_for("README.md"), None);
    }

    #[test]
    fn only_the_android_res_tree_uses_the_dot_prefixed_spelling() {
        // aapt refuses a second resource name per directory, so a plain sidecar inside the APK's
        // resource tree would break `cargo apk build`. The exception has to stay exactly there.
        assert!(ANDROID_RES.ends_with("/res/"));
        assert_eq!(sidecar_for(&format!("{ANDROID_RES}mipmap-mdpi/ic_launcher.png")), format!("{ANDROID_RES}mipmap-mdpi/.ic_launcher.png.attribution"));
        for elsewhere in ["assets/app-icon/effectcraft.ico", "docs/brand/artcraft-logo.svg", "assets/fonts/Inter-Regular.ttf"] {
            assert!(!sidecar_for(elsewhere).contains("/."), "dot-prefixed sidecar leaked to {elsewhere}");
        }
    }
}
