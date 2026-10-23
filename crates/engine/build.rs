//! Record which commit this build came from, as `EFFECTCRAFT_BUILD_COMMIT` (read by
//! [`crate::BUILD_COMMIT`]): agents and bug reports can then tell two builds of the same version
//! apart (a release and a local build of `main` both say 0.12.0).
//!
//! `EFFECTCRAFT_BUILD_COMMIT` in the build environment wins (release CI, source tarballs, Nix and
//! other builds without a `.git`). Otherwise `git rev-parse HEAD` is asked; without git, or outside
//! a checkout, the value is `unknown`. A build never fails because of this.

use std::path::PathBuf;
use std::process::Command;

const VAR: &str = "EFFECTCRAFT_BUILD_COMMIT";

fn git(args: &[&str]) -> Option<String> {
    let dir = std::env::var_os("CARGO_MANIFEST_DIR")?;
    let out = Command::new("git").args(args).current_dir(dir).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Re-run when HEAD moves: HEAD itself, the branch it points at and packed-refs.
fn watch_git() {
    let mut paths: Vec<String> = ["HEAD", "packed-refs"].iter().filter_map(|p| git(&["rev-parse", "--git-path", p])).collect();
    if let Some(r) = git(&["symbolic-ref", "-q", "HEAD"]) {
        paths.extend(git(&["rev-parse", "--git-path", &r]));
    }
    let dir = std::env::var_os("CARGO_MANIFEST_DIR").map(PathBuf::from).unwrap_or_default();
    for p in paths {
        let p = PathBuf::from(p);
        let abs = if p.is_absolute() { p } else { dir.join(p) };
        if abs.exists() {
            println!("cargo:rerun-if-changed={}", abs.display());
        }
    }
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed={VAR}");
    let commit = match std::env::var(VAR) {
        Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => {
            watch_git();
            git(&["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".into())
        }
    };
    // Only hex digits (or `unknown`) reach the binary, whatever the environment says.
    let commit = if commit == "unknown" || commit.chars().all(|c| c.is_ascii_hexdigit()) { commit } else { "unknown".into() };
    println!("cargo:rustc-env={VAR}={commit}");
}
