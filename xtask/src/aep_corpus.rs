//! `cargo xtask aep-corpus [--dir DIR]`: fetch the After Effects test corpus of `crates/aep`.
//!
//! The corpus is a separate repository (binary `.aep` files stay out of this one). The pin in
//! `crates/aep/tests/fixtures/corpus.lock` names one commit and the SHA-256 of every file. The
//! command fetches exactly that commit into a temporary directory, checks every file against the
//! hashes of the lock file, and only then copies them to `DIR` (default `corpus/aep`, ignored by
//! git). Any mismatch, missing file or malformed line fails the command and leaves `DIR` alone.
//! Then run the tests with `EFFECTCRAFT_AEP_FIXTURES=<DIR>` (add `EFFECTCRAFT_AEP_FIXTURES_REQUIRED=1`
//! so that a missing project fails its test).
//!
//! `EFFECTCRAFT_AEP_CORPUS_REPO` replaces the repository URL (a local clone works); the commit and
//! the hashes still come from the lock file.

use std::path::{Component, Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

const LOCK: &str = "crates/aep/tests/fixtures/corpus.lock";
const DEFAULT_DIR: &str = "corpus/aep";
/// Where the fixtures sit inside the corpus repository; the command installs this folder's files.
const PREFIX: &str = "aep/";

#[derive(Debug, PartialEq, Eq)]
struct Lock {
    repo: String,
    commit: String,
    /// (SHA-256 as 64 lower-case hex digits, path relative to the corpus root), in file order.
    files: Vec<(String, String)>,
}

fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A path that stays inside its folder: relative, no `..`, no root or drive.
fn safe_relative(p: &str) -> bool {
    !p.is_empty() && !p.contains('\\') && Path::new(p).components().all(|c| matches!(c, Component::Normal(_)))
}

fn parse_lock(text: &str) -> Result<Lock, String> {
    let (mut repo, mut commit, mut files) = (None, None, Vec::new());
    for (n, line) in text.lines().enumerate().map(|(i, l)| (i + 1, l.trim())) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let bad = |why: &str| format!("{LOCK}:{n}: {why}");
        if let Some(v) = line.strip_prefix("repo ") {
            if repo.replace(v.trim().to_string()).is_some() {
                return Err(bad("`repo` given twice"));
            }
        } else if let Some(v) = line.strip_prefix("commit ") {
            let v = v.trim();
            if !is_hex(v, 40) {
                return Err(bad("`commit` must be 40 lower-case hex digits (no branch or tag)"));
            }
            if commit.replace(v.to_string()).is_some() {
                return Err(bad("`commit` given twice"));
            }
        } else {
            let (hash, path) = line.split_once("  ").ok_or_else(|| bad("expected `<sha256>  <path>`"))?;
            let path = path.trim();
            if !is_hex(hash, 64) {
                return Err(bad("hash must be 64 lower-case hex digits"));
            }
            if !safe_relative(path) || !path.starts_with(PREFIX) {
                return Err(bad(&format!("path must be relative, without `..`, and under `{PREFIX}`")));
            }
            if files.iter().any(|(_, p)| p == path) {
                return Err(bad("path listed twice"));
            }
            files.push((hash.to_string(), path.to_string()));
        }
    }
    let repo = repo.filter(|r| !r.is_empty()).ok_or_else(|| format!("{LOCK}: no `repo` line"))?;
    let commit = commit.ok_or_else(|| format!("{LOCK}: no `commit` line"))?;
    if files.is_empty() {
        return Err(format!("{LOCK}: no files listed"));
    }
    Ok(Lock { repo, commit, files })
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Reads every listed file from `root` and checks its hash. Returns the bytes in lock order, or
/// every problem found.
fn verify(root: &Path, files: &[(String, String)]) -> Result<Vec<Vec<u8>>, String> {
    let (mut data, mut errors) = (Vec::new(), Vec::new());
    for (hash, path) in files {
        match std::fs::read(root.join(path)) {
            Err(e) => errors.push(format!("{path}: {e}")),
            Ok(bytes) => {
                let got = sha256_hex(&bytes);
                if &got == hash {
                    data.push(bytes);
                } else {
                    errors.push(format!("{path}: SHA-256 is {got}, the lock file says {hash}"));
                }
            }
        }
    }
    if errors.is_empty() { Ok(data) } else { Err(errors.join("\n")) }
}

/// Copies verified files below `dest` (the `aep/` prefix dropped), via a temporary name so a
/// failure never leaves a half-written file under its real name.
fn install(dest: &Path, files: &[(String, String)], data: &[Vec<u8>]) -> Result<(), String> {
    for ((_, path), bytes) in files.iter().zip(data) {
        let rel = path.strip_prefix(PREFIX).unwrap_or(path);
        let target = dest.join(rel);
        if let Some(dir) = target.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let tmp = target.with_extension("part");
        std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &target).map_err(|e| format!("{}: {e}", target.display()))?;
    }
    Ok(())
}

/// True when every listed file is already in `dest` with the right hash.
fn up_to_date(dest: &Path, files: &[(String, String)]) -> bool {
    files.iter().all(|(hash, path)| {
        let rel = path.strip_prefix(PREFIX).unwrap_or(path);
        std::fs::read(dest.join(rel)).is_ok_and(|b| &sha256_hex(&b) == hash)
    })
}

fn git(dir: &Path, args: &[&str]) -> Result<(), String> {
    let out = Command::new("git")
        .current_dir(dir)
        .args(["-c", "core.autocrlf=false", "-c", "core.safecrlf=false"])
        .args(args)
        .output()
        .map_err(|e| format!("git {}: {e}", args.first().copied().unwrap_or("")))?;
    if out.status.success() { Ok(()) } else { Err(format!("git {}: {}", args.first().copied().unwrap_or(""), String::from_utf8_lossy(&out.stderr).trim())) }
}

/// Fetches exactly `commit` of `repo` into `into` (an empty directory).
fn fetch(repo: &str, commit: &str, into: &Path) -> Result<(), String> {
    git(into, &["init", "-q"])?;
    git(into, &["fetch", "-q", "--depth", "1", repo, commit])?;
    git(into, &["checkout", "-q", "--detach", "FETCH_HEAD"])?;
    let head = Command::new("git").current_dir(into).args(["rev-parse", "HEAD"]).output().map_err(|e| format!("git rev-parse: {e}"))?;
    let head = String::from_utf8_lossy(&head.stdout).trim().to_string();
    if head == commit { Ok(()) } else { Err(format!("fetched {head}, not the pinned {commit}")) }
}

pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    let mut dir = PathBuf::from(DEFAULT_DIR);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--dir" => dir = PathBuf::from(it.next().ok_or("--dir needs a folder")?),
            other => return Err(format!("unknown argument `{other}`; usage: cargo xtask aep-corpus [--dir DIR]")),
        }
    }
    let dest = if dir.is_absolute() { dir } else { root.join(dir) };
    let text = std::fs::read_to_string(root.join(LOCK)).map_err(|e| format!("{LOCK}: {e}"))?;
    let lock = parse_lock(&text)?;
    if up_to_date(&dest, &lock.files) {
        println!("aep-corpus: {} files in {} already match the pin {}", lock.files.len(), dest.display(), &lock.commit[..12]);
    } else {
        let repo = std::env::var("EFFECTCRAFT_AEP_CORPUS_REPO").ok().filter(|r| !r.is_empty()).unwrap_or_else(|| lock.repo.clone());
        let tmp = tempfile::Builder::new().prefix("effectcraft-aep-corpus-").tempdir().map_err(|e| format!("temporary folder: {e}"))?;
        fetch(&repo, &lock.commit, tmp.path())?;
        let data = verify(tmp.path(), &lock.files)?;
        install(&dest, &lock.files, &data)?;
        println!("aep-corpus: {} files verified against {} and installed in {}", lock.files.len(), &lock.commit[..12], dest.display());
    }
    println!("run the tests with:\n  EFFECTCRAFT_AEP_FIXTURES={} EFFECTCRAFT_AEP_FIXTURES_REQUIRED=1 cargo test -p effectcraft-aep", dest.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: &str = "4a51318c4b37fa95e2c02682183d3982fdfbe1870dc50ba729b776d1826f982e";
    const C: &str = "72539568193239fadb3ccd0d7e70f6cd622a650a";

    fn lock(body: &str) -> String {
        format!("# c\nrepo https://example.org/c.git\ncommit {C}\n{body}")
    }

    #[test]
    fn parses_a_lock_file() {
        let l = parse_lock(&lock(&format!("{H}  aep/a.aep\n"))).unwrap();
        assert_eq!(l.repo, "https://example.org/c.git");
        assert_eq!(l.commit, C);
        assert_eq!(l.files, vec![(H.to_string(), "aep/a.aep".to_string())]);
    }

    #[test]
    fn rejects_malformed_locks() {
        for (body, why) in [
            (format!("{H}  ../x.aep\n"), "parent folder"),
            (format!("{H}  /abs/x.aep\n"), "absolute path"),
            (format!("{H}  aep/../../x.aep\n"), "escaping path"),
            (format!("{H}  other/x.aep\n"), "outside aep/"),
            (format!("{H}  aep/a.aep\n{H}  aep/a.aep\n"), "duplicate"),
            ("abc  aep/a.aep\n".to_string(), "short hash"),
            (format!("{}  aep/a.aep\n", H.to_uppercase()), "upper-case hash"),
            (format!("{H} aep/a.aep\n"), "one space"),
            (String::new(), "no files"),
        ] {
            assert!(parse_lock(&lock(&body)).is_err(), "{why} must be rejected");
        }
        assert!(parse_lock(&format!("repo r\ncommit main\n{H}  aep/a.aep\n")).is_err(), "a branch name is not a pin");
        assert!(parse_lock(&format!("commit {C}\n{H}  aep/a.aep\n")).is_err(), "no repo");
        assert!(parse_lock(&format!("repo r\n{H}  aep/a.aep\n")).is_err(), "no commit");
    }

    #[test]
    fn the_checked_in_lock_file_is_valid() {
        let text = std::fs::read_to_string(crate::root().join(LOCK)).unwrap();
        let l = parse_lock(&text).unwrap();
        assert_eq!(l.files.len(), 20);
        assert!(l.files.windows(2).all(|w| w[0].1 < w[1].1), "files sorted by path");
    }

    #[test]
    fn hashes_match_known_vectors() {
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    fn corpus(files: &[(&str, &[u8])]) -> (tempfile::TempDir, Vec<(String, String)>) {
        let dir = tempfile::tempdir().unwrap();
        let mut list = Vec::new();
        for (path, bytes) in files {
            let full = dir.path().join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, bytes).unwrap();
            list.push((sha256_hex(bytes), (*path).to_string()));
        }
        (dir, list)
    }

    #[test]
    fn verify_accepts_matching_files_and_install_copies_them() {
        let (src, list) = corpus(&[("aep/a.aep", b"one"), ("aep/inputs/p.ffx", b"two")]);
        let data = verify(src.path(), &list).unwrap();
        let dest = tempfile::tempdir().unwrap();
        install(dest.path(), &list, &data).unwrap();
        assert_eq!(std::fs::read(dest.path().join("a.aep")).unwrap(), b"one");
        assert_eq!(std::fs::read(dest.path().join("inputs/p.ffx")).unwrap(), b"two");
        assert!(up_to_date(dest.path(), &list));
    }

    #[test]
    fn verify_reports_a_changed_and_a_missing_file_and_installs_nothing() {
        let (src, mut list) = corpus(&[("aep/a.aep", b"one"), ("aep/b.aep", b"two")]);
        std::fs::write(src.path().join("aep/a.aep"), b"changed").unwrap();
        list.push((H.to_string(), "aep/gone.aep".to_string()));
        let err = verify(src.path(), &list).unwrap_err();
        assert!(err.contains("aep/a.aep: SHA-256"), "{err}");
        assert!(err.contains("aep/gone.aep"), "{err}");
        assert!(!err.contains("aep/b.aep"), "{err}");
    }

    #[test]
    fn up_to_date_is_false_for_missing_or_changed_files() {
        let (_src, list) = corpus(&[("aep/a.aep", b"one")]);
        let dest = tempfile::tempdir().unwrap();
        assert!(!up_to_date(dest.path(), &list), "missing");
        std::fs::write(dest.path().join("a.aep"), b"other").unwrap();
        assert!(!up_to_date(dest.path(), &list), "changed");
    }

    #[test]
    fn fetch_checks_out_exactly_the_pinned_commit() {
        let repo = tempfile::tempdir().unwrap();
        git(repo.path(), &["init", "-q"]).unwrap();
        std::fs::create_dir_all(repo.path().join("aep")).unwrap();
        std::fs::write(repo.path().join("aep/a.aep"), b"v1").unwrap();
        git(repo.path(), &["add", "."]).unwrap();
        git(repo.path(), &["-c", "user.name=t", "-c", "user.email=t@example.org", "commit", "-q", "-m", "one"]).unwrap();
        let rev =
            |d: &Path| String::from_utf8(Command::new("git").current_dir(d).args(["rev-parse", "HEAD"]).output().unwrap().stdout).unwrap().trim().to_string();
        let first = rev(repo.path());
        std::fs::write(repo.path().join("aep/a.aep"), b"v2").unwrap();
        git(repo.path(), &["-c", "user.name=t", "-c", "user.email=t@example.org", "commit", "-q", "-am", "two"]).unwrap();
        let into = tempfile::tempdir().unwrap();
        fetch(repo.path().to_str().unwrap(), &first, into.path()).unwrap();
        assert_eq!(std::fs::read(into.path().join("aep/a.aep")).unwrap(), b"v1", "the pinned commit, not the newer one");
    }
}
