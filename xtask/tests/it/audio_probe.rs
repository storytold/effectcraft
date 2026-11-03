//! The Linux audio-backend preflight against fake pkg-config tools.
//!
//! The probe resolves its executable through the pkg-config crate, which reads the environment,
//! so the fake-tool scenarios run the real `xtask audio-backend` binary in a child process with a
//! controlled environment: `Command::env` changes only the child's environment, so the tests need
//! no global env mutation and stay parallel-safe.

use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::{Command, Output};

/// A scratch directory holding the fake tools for one scenario: `dir` is the child's `PATH`,
/// `off_path` holds executables reachable only by an explicit `*_PKG_CONFIG` override.
struct FakeTools {
    dir: PathBuf,
    off_path: PathBuf,
}

impl FakeTools {
    fn new(tag: &str) -> Self {
        let base = std::env::temp_dir().join(format!("effectcraft-xtask-audio-{tag}-{}", std::process::id()));
        let dir = base.join("bin");
        let off_path = base.join("off-path");
        std::fs::create_dir_all(&dir).expect("create PATH dir");
        std::fs::create_dir_all(&off_path).expect("create off-PATH dir");
        Self { dir, off_path }
    }

    /// Write a plain (not executable) file.
    fn file(&self, on_path: bool, name: &str, body: &str) -> PathBuf {
        let path = if on_path { self.dir.clone() } else { self.off_path.clone() }.join(name);
        std::fs::write(&path, body).expect("write fake tool");
        path
    }

    /// Write an executable shell script.
    fn script(&self, on_path: bool, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = self.file(on_path, name, body);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod fake tool");
        path
    }

    /// A fake `pkgconf` that answers as alsa 1.2.11 linking -lasound.
    fn working_pkgconf(&self, on_path: bool) -> PathBuf {
        self.script(on_path, "pkgconf", "#!/bin/sh\ncase \" $* \" in\n  *\" --modversion \"*) echo 1.2.11 ;;\n  *) echo -lasound ;;\nesac\n")
    }
}

impl Drop for FakeTools {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
        let _ = std::fs::remove_dir_all(&self.off_path);
    }
}

/// Run the built `xtask audio-backend` with `dir` as the only `PATH`, the given variables set,
/// and every variable the probe reads otherwise cleared, so a developer's own environment
/// cannot leak into the scenario.
fn run_audio_backend(tools: &FakeTools, set: &[(&str, &OsStr)]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("audio-backend")
        .env("PATH", &tools.dir)
        .env_remove("PKG_CONFIG")
        .env_remove("PKG_CONFIG_PATH")
        .env_remove("PKG_CONFIG_LIBDIR")
        .env_remove("PKG_CONFIG_SYSROOT_DIR")
        .env_remove("PKG_CONFIG_ALLOW_CROSS")
        .env_remove("ALSA_NO_PKG_CONFIG")
        .env_remove("HOST_PKG_CONFIG")
        .env_remove("TARGET_PKG_CONFIG")
        .env_remove("HOST")
        .env_remove("TARGET")
        .envs(set.iter().copied())
        .output()
        .expect("run xtask audio-backend")
}

#[test]
fn a_host_with_only_pkgconf_still_finds_alsa() {
    // The reviewed case: PKG_CONFIG unset, no pkg-config on PATH, only pkgconf. The pkg-config
    // crate alsa-sys uses falls back to pkgconf, so the stage must stay green.
    let tools = FakeTools::new("pkgconf-only");
    tools.working_pkgconf(true);
    let out = run_audio_backend(&tools, &[]);
    assert!(out.status.success(), "pkgconf-only host must pass:\n{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn a_probe_that_runs_and_says_no_names_the_package() {
    let tools = FakeTools::new("says-no");
    tools.script(true, "pkg-config", "#!/bin/sh\necho 'Package alsa was not found' >&2\nexit 1\n");
    let out = run_audio_backend(&tools, &[]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    for want in ["alsa-lib-devel", "libasound2-dev", "alsa-lib", "pkg-config"] {
        assert!(stderr.contains(want), "hint lost `{want}`:\n{stderr}");
    }
    assert!(stderr.contains("not found"), "pkg-config's own detail should be appended:\n{stderr}");
}

#[test]
fn a_probe_that_cannot_run_is_not_alsa_missing() {
    // PKG_CONFIG names a file that is not an executable; with PKG_CONFIG set the crate does not
    // fall back to pkgconf, so the lookup fails to launch — reported separately from "missing".
    let tools = FakeTools::new("cannot-run");
    let broken = tools.file(true, "not-an-executable", "this is not a program\n");
    let out = run_audio_backend(&tools, &[("PKG_CONFIG", broken.as_os_str())]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("could not"), "expected the launch-failure error:\n{stderr}");
    assert!(!stderr.contains("alsa-lib-devel"), "must not read as ALSA missing:\n{stderr}");
}

#[test]
fn a_host_scoped_override_selects_that_executable() {
    // With TARGET and HOST set (build-script style), HOST_PKG_CONFIG wins over the PATH lookup,
    // and here it is the only working tool — so success proves the override was used.
    let tools = FakeTools::new("scoped");
    let good = tools.working_pkgconf(false);
    let out = run_audio_backend(
        &tools,
        &[("HOST", OsStr::new("x86_64-unknown-linux-gnu")), ("TARGET", OsStr::new("x86_64-unknown-linux-gnu")), ("HOST_PKG_CONFIG", good.as_os_str())],
    );
    assert!(out.status.success(), "HOST_PKG_CONFIG override must be honored:\n{}", String::from_utf8_lossy(&out.stderr));
}
