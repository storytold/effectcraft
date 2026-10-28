//! `cargo xtask aep-corpus [--update-manifest]`: the After Effects test projects of `crates/aep`
//! (AGENTS.md: large binary fixtures are fetched pinned, not committed). They come from
//! [`REPO`] at [`COMMIT`] into `corpus/aep/` (gitignored); every file is checked against the
//! SHA-256 manifest `xtask/aep-corpus.sha256` before it is put in place, and a copy that already
//! matches is left alone. The tests read `corpus/aep/` (or `EFFECTCRAFT_AEP_FIXTURES`) and print
//! `SKIPPED` without it. `--update-manifest` rewrites the manifest from the download after the pin
//! moved.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::sha256;

/// The corpus repository (GitHub `owner/repo`).
pub const REPO: &str = "vangel76/effectcraft-aep-corpus";
/// The pinned commit.
pub const COMMIT: &str = "46a09b5348e097193928d65511afc9e4b46ead27";
/// The folder of the repository that holds the files.
const SUBDIR: &str = "aep";
const MANIFEST: &str = "xtask/aep-corpus.sha256";
/// Generic User-Agent: no personal details in requests.
const USER_AGENT: &str = "EffectCraft-dev";
/// Bounds for one file and for the archive (the corpus is about 27 MB).
const MAX_FILE: u64 = 16 << 20;
const MAX_ARCHIVE: u64 = 128 << 20;

type Entries = Vec<(String, String)>;

/// Where the corpus is installed.
pub fn dest(root: &Path) -> PathBuf {
    root.join("corpus").join(SUBDIR)
}

pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    let update = args.iter().any(|a| a == "--update-manifest");
    if let Some(other) = args.iter().find(|a| *a != "--update-manifest") {
        return Err(format!("aep-corpus: unknown argument `{other}` (usage: cargo xtask aep-corpus [--update-manifest])"));
    }
    let dest = dest(root);
    let manifest = root.join(MANIFEST);
    if !update {
        let entries = parse(&std::fs::read_to_string(&manifest).map_err(|e| format!("{MANIFEST}: {e}"))?)?;
        if verify(&dest, &entries).is_ok() {
            println!("aep-corpus: {} already matches {MANIFEST} ({} files)", dest.display(), entries.len());
            return Ok(());
        }
    }
    let staging = root.join("corpus").join(".aep-download");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| format!("create {}: {e}", staging.display()))?;
    let result = fetch(&staging, &dest, &manifest, update);
    let _ = std::fs::remove_dir_all(&staging);
    result
}

fn fetch(staging: &Path, dest: &Path, manifest: &Path, update: bool) -> Result<(), String> {
    let archive = staging.join("corpus.tar.gz");
    let url = format!("https://codeload.github.com/{REPO}/tar.gz/{COMMIT}");
    let mut curl = Command::new("curl");
    curl.args(["-fsSL", "--retry", "3", "--max-filesize", &MAX_ARCHIVE.to_string(), "-A", USER_AGENT, "-o"]).arg(&archive).arg(&url);
    crate::run(&mut curl)?;
    let name = REPO.rsplit('/').next().unwrap_or(REPO);
    let top = format!("{name}-{COMMIT}");
    // On Windows the system's bsdtar: a GNU tar earlier on the PATH (Git's) reads `E:\…` as a
    // remote host.
    let system_tar = std::env::var_os("SystemRoot").map(|r| PathBuf::from(r).join("System32").join("tar.exe")).filter(|t| cfg!(windows) && t.is_file());
    let mut tar = Command::new(system_tar.unwrap_or_else(|| PathBuf::from("tar")));
    tar.arg("-xzf").arg(&archive).arg("-C").arg(staging).arg(format!("{top}/{SUBDIR}"));
    crate::run(&mut tar)?;
    let src = staging.join(&top).join(SUBDIR);
    let entries = if update {
        let mut entries = scan(&src)?;
        entries.sort_by(|a, b| a.1.cmp(&b.1));
        let text: String = entries.iter().map(|(h, f)| format!("{h}  {f}\n")).collect();
        std::fs::write(manifest, text).map_err(|e| format!("write {MANIFEST}: {e}"))?;
        println!("aep-corpus: wrote {} entries to {MANIFEST}", entries.len());
        entries
    } else {
        parse(&std::fs::read_to_string(manifest).map_err(|e| format!("{MANIFEST}: {e}"))?)?
    };
    verify(&src, &entries).map_err(|e| format!("aep-corpus: the download of {REPO}@{} does not match {MANIFEST}: {e}", &COMMIT[..12]))?;
    let _ = std::fs::remove_dir_all(dest);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    std::fs::rename(&src, dest).map_err(|e| format!("move {} to {}: {e}", src.display(), dest.display()))?;
    println!("aep-corpus: {} files verified in {} ({REPO}@{})", entries.len(), dest.display(), &COMMIT[..12]);
    Ok(())
}

/// A manifest file name: a plain name, no folders.
fn plain(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('.') && name.bytes().all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
}

/// `(sha256, file name)` per line, as `sha256sum` writes them.
fn parse(text: &str) -> Result<Entries, String> {
    let mut out = Entries::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let (hash, name) = line.split_once("  ").ok_or_else(|| format!("{MANIFEST}: bad line {line:?}"))?;
        if hash.len() != 64 || !hash.bytes().all(|c| c.is_ascii_hexdigit()) || !plain(name) || out.iter().any(|(_, n)| n == name) {
            return Err(format!("{MANIFEST}: bad line {line:?}"));
        }
        out.push((hash.to_ascii_lowercase(), name.to_string()));
    }
    if out.is_empty() {
        return Err(format!("{MANIFEST}: empty"));
    }
    Ok(out)
}

/// The bytes of a regular file of at most [`MAX_FILE`] bytes (no symlinks).
fn read(path: &Path) -> Result<Vec<u8>, String> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.is_file() || meta.len() > MAX_FILE {
        return Err(format!("{}: not a regular file of at most {MAX_FILE} bytes", path.display()));
    }
    std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// `dir` holds exactly the manifest's files, each with its hash.
fn verify(dir: &Path, entries: &Entries) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if !meta.is_dir() {
        return Err(format!("{}: not a folder", dir.display()));
    }
    let mut seen = 0;
    for e in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let e = e.map_err(|e| format!("{}: {e}", dir.display()))?;
        let name = e.file_name().to_string_lossy().into_owned();
        let (hash, _) = entries.iter().find(|(_, n)| *n == name).ok_or_else(|| format!("unexpected file {name}"))?;
        if sha256::hex(&read(&e.path())?) != *hash {
            return Err(format!("{name}: sha256 mismatch"));
        }
        seen += 1;
    }
    if seen != entries.len() {
        return Err(format!("{} of {} files present", seen, entries.len()));
    }
    Ok(())
}

/// The manifest entries of the files in `dir`.
fn scan(dir: &Path) -> Result<Entries, String> {
    let mut out = Entries::new();
    for e in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let e = e.map_err(|e| format!("{}: {e}", dir.display()))?;
        let name = e.file_name().to_string_lossy().into_owned();
        if !plain(&name) {
            return Err(format!("unexpected file name {name:?}"));
        }
        out.push((sha256::hex(&read(&e.path())?), name));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_is_strict() {
        let h = "a".repeat(64);
        assert_eq!(parse(&format!("{h}  x.aep\n")).map(|e| e.len()), Ok(1));
        for bad in [
            format!("{h} x.aep"),
            format!("{h}  ../x.aep"),
            format!("{h}  a/x.aep"),
            format!("{}  x.aep", "z".repeat(64)),
            format!("{h}  x.aep\n{h}  x.aep"),
            String::new(),
        ] {
            assert!(parse(&bad).is_err(), "{bad:?}");
        }
        // The committed manifest parses and lists the corpus.
        let text = include_str!("../aep-corpus.sha256");
        let entries = parse(text).expect("manifest");
        assert!(entries.iter().any(|(_, n)| n == "effects.aep") && entries.iter().any(|(_, n)| n == "effects.aep.attribution"));
    }

    #[test]
    fn verify_wants_exactly_the_listed_files() {
        let dir = std::env::temp_dir().join(format!("ec-aep-corpus-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.aep"), b"abc").unwrap();
        let ok = vec![(sha256::hex(b"abc"), "a.aep".to_string())];
        assert!(verify(&dir, &ok).is_ok());
        assert!(verify(&dir, &vec![(sha256::hex(b"abd"), "a.aep".to_string())]).is_err());
        assert!(verify(&dir, &vec![ok[0].clone(), (sha256::hex(b""), "b.aep".to_string())]).is_err());
        std::fs::write(dir.join("extra.aep"), b"").unwrap();
        assert!(verify(&dir, &ok).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
