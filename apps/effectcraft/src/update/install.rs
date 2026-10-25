use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::{instances, release};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Install {
    AppImage(PathBuf),
    MacApp(PathBuf),
    Portable(PathBuf),
    Msi(PathBuf),
    Deb(PathBuf),
    Rpm(PathBuf),
}

impl Install {
    pub fn target(&self) -> &Path {
        match self {
            Self::AppImage(p) | Self::MacApp(p) | Self::Portable(p) | Self::Msi(p) | Self::Deb(p) | Self::Rpm(p) => p,
        }
    }

    pub fn suffix(&self) -> Result<String, String> {
        let arch = std::env::consts::ARCH;
        Ok(match self {
            Self::MacApp(_) => "macos-universal.dmg".into(),
            Self::Portable(_) | Self::Msi(_) => {
                let arch = match arch {
                    "x86_64" => "x64",
                    "x86" => "x86",
                    "aarch64" => "arm64",
                    _ => return Err("Unsupported Windows architecture.".into()),
                };
                format!("windows-{arch}{}", if matches!(self, Self::Portable(_)) { "-portable.zip" } else { ".msi" })
            }
            Self::AppImage(_) | Self::Deb(_) | Self::Rpm(_) => {
                if !matches!(arch, "x86_64" | "aarch64") {
                    return Err("This architecture needs a manual update.".into());
                }
                format!(
                    "linux-{arch}.{}",
                    match self {
                        Self::Deb(_) => "deb",
                        Self::Rpm(_) => "rpm",
                        _ => "AppImage",
                    }
                )
            }
        })
    }
}

pub fn detect() -> Result<Option<Install>, String> {
    let exe = std::env::current_exe().and_then(|p| p.canonicalize()).map_err(|e| e.to_string())?;
    #[cfg(target_os = "linux")]
    {
        if Path::new("/.flatpak-info").exists() || std::env::var_os("SNAP").is_some() {
            return Ok(None);
        }
        if let Some(path) = std::env::var_os("APPIMAGE") {
            let directory = std::env::var_os("APPDIR").ok_or("APPIMAGE is set without APPDIR. Launch the AppImage itself to update it.")?;
            return appimage_target(&exe, &PathBuf::from(path), &PathBuf::from(directory)).map(|path| Some(Install::AppImage(path)));
        }
        if exe == Path::new("/usr/bin/effectcraft") {
            let deb = Command::new("dpkg-query").args(["-W", "-f", "${db:Status-Status}", "effectcraft"]).output();
            if deb.is_ok_and(|o| o.status.success() && o.stdout == b"installed") {
                return Ok(Some(Install::Deb(exe)));
            }
            if Command::new("rpm").args(["-q", "effectcraft"]).output().is_ok_and(|o| o.status.success()) {
                return Ok(Some(Install::Rpm(exe)));
            }
        }
    }
    #[cfg(target_os = "macos")]
    if let Some(bundle) = exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")) {
        if plist(bundle, "CFBundleIdentifier")? == "ai.storyteller.effectcraft" {
            return Ok(Some(Install::MacApp(bundle.into())));
        }
    }
    #[cfg(windows)]
    {
        let dir = exe.parent().ok_or("The executable has no folder.")?;
        if effectcraft_host::app_dirs::PORTABLE_MARKERS.iter().any(|m| dir.join(m).is_file()) {
            return Ok(Some(Install::Portable(exe)));
        }
        use winreg::{RegKey, enums::HKEY_LOCAL_MACHINE};
        if let Ok(key) = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(r"Software\Microsoft\Windows\CurrentVersion\App Paths\effectcraft.exe") {
            if key.get_value::<String, _>("").ok().and_then(|p| PathBuf::from(p).canonicalize().ok()).is_some_and(|p| p == exe) {
                return Ok(Some(Install::Msi(exe)));
            }
        }
    }
    let _ = exe;
    Ok(None)
}

#[cfg(target_os = "linux")]
fn appimage_target(exe: &Path, image: &Path, directory: &Path) -> Result<PathBuf, String> {
    use std::io::Read;
    let directory = directory.canonicalize().map_err(|e| e.to_string())?;
    if !exe.starts_with(directory) {
        return Err("This executable is outside the AppImage mount. Updating is disabled.".into());
    }
    let image = image.canonicalize().map_err(|e| e.to_string())?;
    let mut magic = [0u8; 11];
    std::fs::File::open(&image).and_then(|mut f| f.read_exact(&mut magic)).map_err(|e| format!("Cannot read the AppImage header. {e}"))?;
    if magic.get(..4) != Some(b"\x7fELF") || !matches!(magic.get(8..11), Some(b"AI\x01" | b"AI\x02")) {
        return Err("APPIMAGE does not point to an AppImage. No files were changed.".into());
    }
    Ok(image)
}

#[derive(Deserialize, Serialize)]
pub struct Job {
    pub install: Install,
    pub release: release::Release,
    pub restarts: Vec<instances::Restart>,
}

pub fn spawn(root: &Path, job: &Job) -> Result<(), String> {
    instances::write(&root.join("transaction/job.json"), job)?;
    let helper = root.join(if cfg!(windows) { "update-helper.exe" } else { "update-helper" });
    std::fs::copy(std::env::current_exe().map_err(|e| e.to_string())?, &helper).map_err(|e| format!("Could not prepare the restart helper. {e}"))?;
    let mut cmd = Command::new(helper);
    cmd.arg("--apply-update").arg(root).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000 | 0x00000008);
    }
    cmd.spawn().map_err(|e| format!("Could not start the restart helper. {e}"))?;
    Ok(())
}

pub fn helper(root: &Path) -> Result<(), String> {
    let helper = instances::lock(&root.join("helper.lock"))?;
    helper.try_lock().map_err(|e| format!("Another update helper is running. {e}"))?;
    let job: Job = instances::read(&root.join("transaction/job.json"))?;
    release::validate_download(&job.release)?;
    if job.restarts.len() > 128 {
        return Err("Too many EffectCraft instances to restart.".into());
    }
    let running = wait_for_exit(root, Duration::from_secs(120))?;
    let package = root.join(&job.release.name);
    let installed =
        check_processes(&[]).and_then(|()| release::verify(&package, &job.release)).and_then(|()| apply(&job.install, &package, root, &job.release.version));
    let _ = std::fs::remove_dir_all(root.join("transaction"));
    running.unlock().map_err(|e| e.to_string())?;
    drop(helper);
    let mut restart_error = None;
    for restart in &job.restarts {
        let mut cmd = restart_command(&job.install);
        if let Some(config) = &restart.config {
            cmd.env("EFFECTCRAFT_CONFIG_DIR", config);
        }
        cmd.env_remove("EFFECTCRAFT_CONTROL_PORT");
        if let Some(port) = restart.control_port {
            cmd.arg("--control").arg(port.to_string());
        }
        if let Some(project) = &restart.project {
            cmd.arg("--reopen-project").arg(project);
        }
        if let Err(e) = cmd.spawn() {
            restart_error = Some(format!("Could not restart EffectCraft. {e}"));
        }
    }
    installed?;
    if let Some(e) = restart_error {
        return Err(e);
    }
    Ok(())
}

fn wait_for_exit(root: &Path, timeout: Duration) -> Result<std::fs::File, String> {
    let running = instances::lock(&root.join("running.lock"))?;
    let deadline = Instant::now() + timeout;
    loop {
        if root.join("transaction/cancel").exists() {
            return Err("Update canceled. No installed files were changed.".into());
        }
        if root.join("transaction/commit").exists() && running.try_lock().is_ok() {
            if process_ids()?.is_empty() {
                break;
            }
            running.unlock().map_err(|e| e.to_string())?;
        }
        if Instant::now() >= deadline {
            return Err("EffectCraft windows did not all close within two minutes. No installed files were changed.".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(running)
}

fn restart_command(install: &Install) -> Command {
    match install {
        Install::MacApp(bundle) => Command::new(bundle.join("Contents/MacOS/EffectCraft")),
        _ => Command::new(install.target()),
    }
}

fn run(cmd: &mut Command) -> Result<String, String> {
    use std::io::Read;
    let out = tempfile::tempfile().map_err(|e| e.to_string())?;
    let err = tempfile::tempfile().map_err(|e| e.to_string())?;
    let status =
        cmd.stdout(out.try_clone().map_err(|e| e.to_string())?).stderr(err.try_clone().map_err(|e| e.to_string())?).status().map_err(|e| e.to_string())?;
    use std::io::{Seek, SeekFrom};
    let mut file = if status.success() { out } else { err };
    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let mut text = String::new();
    file.take(65537).read_to_string(&mut text).map_err(|e| e.to_string())?;
    if text.len() > 65536 {
        return Err("The system command returned too much output.".into());
    }
    if status.success() { Ok(text.trim().into()) } else { Err(format!("The update installer failed ({status}). {}", text.trim())) }
}

fn process_ids() -> Result<Vec<String>, String> {
    #[cfg(windows)]
    let pids = run(Command::new("powershell.exe").args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "Get-Process | Where-Object { $_.ProcessName -eq 'effectcraft' } | ForEach-Object { $_.Id }",
    ]))?;
    #[cfg(unix)]
    let pids = {
        let output = run(Command::new("ps").args(["-A", "-o", "pid=,comm="]))?;
        output
            .lines()
            .filter_map(|line| {
                let (pid, command) = line.trim().split_once(char::is_whitespace)?;
                let name = Path::new(command.trim()).file_name()?.to_str()?;
                let named = matches!(name, "effectcraft" | "EffectCraft" | "effectcraft.exe");
                #[cfg(target_os = "linux")]
                let named = named
                    || name == "AppRun" && std::fs::read_link(format!("/proc/{pid}/exe")).is_ok_and(|p| p.file_name().is_some_and(|n| n == "effectcraft"));
                named.then_some(pid)
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    #[cfg(not(any(unix, windows)))]
    return Err("Process checks are unavailable on this platform.".into());
    Ok(pids.lines().map(|id| id.trim().to_string()).filter(|id| !id.is_empty()).collect())
}

pub fn check_processes(registered: &[String]) -> Result<(), String> {
    if process_ids()?.iter().any(|pid| !registered.contains(pid)) {
        return Err(
            "Close other EffectCraft copies before updating. An older version or another installation cannot join this save-and-restart request.".into()
        );
    }
    Ok(())
}

fn apply(install: &Install, package: &Path, root: &Path, version: &str) -> Result<(), String> {
    match install {
        #[cfg(unix)]
        Install::AppImage(target) => replace_appimage(package, target),
        #[cfg(target_os = "macos")]
        Install::MacApp(target) => replace_bundle(package, target, version),
        Install::Portable(target) | Install::Msi(target) => {
            let script = root.join("install.ps1");
            std::fs::write(&script, include_str!("install.ps1")).map_err(|e| e.to_string())?;
            run(Command::new("powershell.exe")
                .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"])
                .arg(script)
                .arg(package)
                .arg(target)
                .arg(if matches!(install, Install::Portable(_)) { "portable" } else { "msi" }))?;
            Ok(())
        }
        Install::Deb(_) => run(Command::new("pkexec").args(["apt-get", "install", "-y", "-o", "DPkg::Lock::Timeout=120"]).arg(package)).map(|_| ()),
        Install::Rpm(_) => {
            let manager = if Command::new("dnf").arg("--version").output().is_ok_and(|o| o.status.success()) { "dnf" } else { "zypper" };
            let args: &[&str] = if manager == "dnf" { &["install", "-y"] } else { &["--non-interactive", "install"] };
            run(Command::new("pkexec").arg(manager).args(args).arg(package)).map(|_| ())
        }
        #[allow(unreachable_patterns)]
        _ => {
            let _ = version;
            Err("This installation needs a manual update.".into())
        }
    }
}

#[cfg(unix)]
fn replace_appimage(package: &Path, target: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mut file = tempfile::NamedTempFile::new_in(target.parent().ok_or("The AppImage has no folder.")?).map_err(|e| e.to_string())?;
    std::io::copy(&mut std::fs::File::open(package).map_err(|e| e.to_string())?, &mut file).map_err(|e| e.to_string())?;
    file.as_file().set_permissions(std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    file.persist(target).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn plist(bundle: &Path, key: &str) -> Result<String, String> {
    run(Command::new("/usr/libexec/PlistBuddy").args(["-c", &format!("Print :{key}")]).arg(bundle.join("Contents/Info.plist")))
}

#[cfg(target_os = "macos")]
fn replace_bundle(package: &Path, target: &Path, version: &str) -> Result<(), String> {
    let mount = tempfile::tempdir().map_err(|e| e.to_string())?;
    run(Command::new("/usr/bin/hdiutil").args(["attach", "-readonly", "-nobrowse", "-noautoopen", "-mountpoint"]).arg(mount.path()).arg(package))?;
    let source = mount.path().join("EffectCraft.app");
    let result = (|| {
        if plist(&source, "CFBundleIdentifier")? != "ai.storyteller.effectcraft" || plist(&source, "CFBundleShortVersionString")? != version {
            return Err("The disk image contains a different app or version.".into());
        }
        run(Command::new("/usr/bin/codesign").args(["--verify", "--deep", "--strict"]).arg(&source))?;
        let stage =
            tempfile::Builder::new().prefix(".effectcraft-update-").tempdir_in(target.parent().ok_or("The app has no folder.")?).map_err(|e| e.to_string())?;
        let new = stage.path().join("EffectCraft.app");
        run(Command::new("/usr/bin/ditto").arg(&source).arg(&new))?;
        let backup = stage.path().join("previous.app");
        std::fs::rename(target, &backup).map_err(|e| e.to_string())?;
        if let Err(e) = std::fs::rename(&new, target) {
            if let Err(restore) = std::fs::rename(&backup, target) {
                let kept = stage.keep();
                return Err(format!("Could not install ({e}) or restore ({restore}). The previous app is at {}.", kept.display()));
            }
            return Err(e.to_string());
        }
        Ok(())
    })();
    let detached = run(Command::new("/usr/bin/hdiutil").arg("detach").arg(mount.path()));
    result?;
    if let Err(e) = detached {
        log::warn!("The update was installed, but the disk image could not be ejected. {e}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[test]
    fn appimage_replacement_is_complete_executable_and_preserves_neighbors() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("Effect Craft.AppImage");
        let new = dir.path().join("package");
        std::fs::write(&old, b"old").unwrap();
        std::fs::write(&new, b"new").unwrap();
        std::fs::write(dir.path().join("project.ecproj"), b"work").unwrap();
        super::replace_appimage(&new, &old).unwrap();
        assert_eq!(std::fs::read(&old).unwrap(), b"new");
        assert_eq!(std::fs::read(dir.path().join("project.ecproj")).unwrap(), b"work");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(old).unwrap().permissions().mode() & 0o777, 0o755);
    }
    #[cfg(unix)]
    #[test]
    fn helper_waits_for_exit_replaces_the_app_and_reopens_the_saved_project() {
        use super::*;
        use sha2::{Digest, Sha256};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join("transaction")).unwrap();
        let target = root.join("Effect Craft.AppImage");
        std::fs::write(&target, b"old app").unwrap();
        let package = b"#!/bin/sh\nprintf '%s' \"$4\" > \"$EFFECTCRAFT_CONFIG_DIR/reopened\"\n";
        let release = release::Release {
            tag: "v99.0.0".into(),
            version: "99.0.0".into(),
            name: "effectcraft-99.0.0-linux-x86_64.AppImage".into(),
            url: "https://github.com/storytold/effectcraft/releases/download/v99.0.0/effectcraft-99.0.0-linux-x86_64.AppImage".into(),
            size: package.len() as u64,
            sha256: format!("{:x}", Sha256::digest(package)),
        };
        std::fs::write(root.join(&release.name), package).unwrap();
        let job = Job {
            install: Install::AppImage(target.clone()),
            release,
            restarts: vec![instances::Restart { project: Some("saved project.ecproj".into()), config: Some(root.into()), control_port: Some(9877) }],
        };
        instances::write(&root.join("transaction/job.json"), &job).unwrap();
        instances::write(&root.join("transaction/commit"), &true).unwrap();
        let running = instances::lock(&root.join("running.lock")).unwrap();
        running.lock_shared().unwrap();
        assert!(wait_for_exit(root, Duration::from_millis(30)).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"old app");
        let worker_root = root.to_path_buf();
        let worker = std::thread::spawn(move || helper(&worker_root));
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(std::fs::read(&target).unwrap(), b"old app");
        drop(running);
        worker.join().unwrap().unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !root.join("reopened").exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(std::fs::read_to_string(root.join("reopened")).unwrap(), "saved project.ecproj");
        assert_eq!(std::fs::read(&target).unwrap(), package);
        assert!(!root.join("transaction").exists());
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn a_verified_dmg_replaces_the_correct_bundle_and_keeps_neighboring_data() {
        use super::*;
        fn bundle(root: &Path, version: &str) -> PathBuf {
            let bundle = root.join("EffectCraft.app");
            std::fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
            std::fs::copy(std::env::current_exe().unwrap(), bundle.join("Contents/MacOS/EffectCraft")).unwrap();
            std::fs::write(bundle.join("Contents/Info.plist"), format!(r#"<?xml version="1.0"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>ai.storyteller.effectcraft</string><key>CFBundleExecutable</key><string>EffectCraft</string><key>CFBundlePackageType</key><string>APPL</string><key>CFBundleShortVersionString</key><string>{version}</string></dict></plist>"#)).unwrap();
            run(Command::new("/usr/bin/codesign").args(["--force", "--sign", "-"]).arg(&bundle)).unwrap();
            bundle
        }
        let dir = tempfile::tempdir().unwrap();
        let target = bundle(&dir.path().join("installed"), "1.0.0");
        let source = dir.path().join("source");
        bundle(&source, "2.0.0");
        let dmg = dir.path().join("package.dmg");
        run(Command::new("/usr/bin/hdiutil").args(["create", "-quiet", "-format", "UDZO", "-srcfolder"]).arg(source).arg(&dmg)).unwrap();
        std::fs::write(dir.path().join("installed/work.ecproj"), b"work").unwrap();
        assert!(replace_bundle(&dmg, &target, "3.0.0").is_err());
        assert_eq!(plist(&target, "CFBundleShortVersionString").unwrap(), "1.0.0");
        replace_bundle(&dmg, &target, "2.0.0").unwrap();
        assert_eq!(plist(&target, "CFBundleShortVersionString").unwrap(), "2.0.0");
        assert_eq!(std::fs::read(dir.path().join("installed/work.ecproj")).unwrap(), b"work");
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn appimage_environment_cannot_select_a_project_or_an_unrelated_executable() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("work.ecproj");
        std::fs::write(&project, b"saved project content").unwrap();
        let directory = dir.path().canonicalize().unwrap();
        assert!(super::appimage_target(&directory.join("effectcraft"), &project, &directory).is_err());
        let image = dir.path().join("app.AppImage");
        std::fs::write(&image, b"\x7fELF\x02\x01\x01\0AI\x02").unwrap();
        assert!(super::appimage_target(&directory.join("effectcraft"), &image, &directory).is_ok());
        assert!(super::appimage_target(std::path::Path::new("/other/effectcraft"), &image, &directory).is_err());
        assert_eq!(std::fs::read(project).unwrap(), b"saved project content");
    }
}
