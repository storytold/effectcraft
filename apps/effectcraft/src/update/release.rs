use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const RELEASES: &str = "https://github.com/storytold/effectcraft/releases/latest";
const PREFIX: &str = "https://github.com/storytold/effectcraft/releases/download/";
const MAX_PACKAGE: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Release {
    pub tag: String,
    pub version: String,
    pub name: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    size: u64,
    browser_download_url: String,
    digest: Option<String>,
}

pub fn agent(timeout: u64) -> ureq::Agent {
    ureq::Agent::config_builder()
        .tls_config(ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build())
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_global(Some(Duration::from_secs(timeout)))
        .user_agent(concat!("EffectCraft/", env!("CARGO_PKG_VERSION")))
        .build()
        .new_agent()
}

fn text(http: &ureq::Agent, url: &str) -> Result<String, String> {
    let mut response = http.get(url).call().map_err(|e| format!("Could not check GitHub. {e}"))?;
    let mut bytes = Vec::new();
    response.body_mut().as_reader().take(1024 * 1024 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() > 1024 * 1024 {
        return Err("GitHub returned more than 1 MiB of release metadata.".into());
    }
    String::from_utf8(bytes).map_err(|e| e.to_string())
}

pub fn latest(current: &str, suffix: &str) -> Result<Option<Release>, String> {
    let http = agent(20);
    let body = text(&http, "https://api.github.com/repos/storytold/effectcraft/releases/latest")?;
    let Some(mut r) = parse(&body, current, suffix)? else { return Ok(None) };
    if r.sha256.is_empty() {
        let parsed: GithubRelease = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        let sums = parsed.assets.iter().find(|a| a.name == "SHA256SUMS.txt").ok_or("The release has no SHA256SUMS.txt.")?;
        validate_url(&sums.browser_download_url, &parsed.tag_name, "SHA256SUMS.txt")?;
        r.sha256 = checksum(&text(&http, &sums.browser_download_url)?, &r.name)?;
    }
    Ok(Some(r))
}

fn validate_url(url: &str, tag: &str, name: &str) -> Result<(), String> {
    if tag.is_empty() || tag.bytes().any(|b| !b.is_ascii_alphanumeric() && !b".-+_".contains(&b)) || url != format!("{PREFIX}{tag}/{name}") {
        return Err("The release download URL is outside EffectCraft's GitHub releases.".into());
    }
    Ok(())
}

pub fn parse(body: &str, current: &str, suffix: &str) -> Result<Option<Release>, String> {
    let r: GithubRelease = serde_json::from_str(body).map_err(|e| format!("Invalid GitHub release. {e}"))?;
    let version = r.tag_name.strip_prefix('v').unwrap_or(&r.tag_name);
    let next = Version::parse(version).map_err(|e| e.to_string())?;
    let current = Version::parse(current).map_err(|e| e.to_string())?;
    if r.draft || r.prerelease || !next.pre.is_empty() || next <= current {
        return Ok(None);
    }
    let name = format!("effectcraft-{version}-{suffix}");
    let asset =
        r.assets.iter().find(|a| a.name == name).ok_or_else(|| format!("This release has no {suffix} package. Download a compatible build from GitHub."))?;
    validate_url(&asset.browser_download_url, &r.tag_name, &name)?;
    if asset.size == 0 || asset.size > MAX_PACKAGE {
        return Err("The update package has an invalid size.".into());
    }
    let sha256 = match asset.digest.as_deref() {
        None => String::new(),
        Some(d) => valid_sha(d.strip_prefix("sha256:").ok_or("The release has an unsupported digest.")?)?,
    };
    Ok(Some(Release { tag: r.tag_name.clone(), version: version.into(), name, url: asset.browser_download_url.clone(), size: asset.size, sha256 }))
}

fn valid_sha(s: &str) -> Result<String, String> {
    if s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(s.to_ascii_lowercase())
    } else {
        Err("The release has an invalid SHA-256 checksum.".into())
    }
}

fn checksum(sums: &str, name: &str) -> Result<String, String> {
    let hashes: Vec<_> = sums
        .lines()
        .filter_map(|line| {
            let (hash, file) = line.split_once(char::is_whitespace)?;
            (file.trim().trim_start_matches('*') == name).then_some(hash)
        })
        .collect();
    match hashes.as_slice() {
        [hash] => valid_sha(hash),
        _ => Err(format!("Missing or ambiguous checksum for {name}.")),
    }
}

pub fn verify(path: &Path, release: &Release) -> Result<(), String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() != release.size {
        return Err("The update download is incomplete.".into());
    }
    let mut hash = Sha256::new();
    std::io::copy(&mut file.take(MAX_PACKAGE + 1), &mut hash).map_err(|e| e.to_string())?;
    if format!("{:x}", hash.finalize()) != valid_sha(&release.sha256)? {
        return Err("The update checksum does not match. Download it again.".into());
    }
    Ok(())
}

pub fn download(release: &Release, dir: &Path, canceled: &AtomicBool, progress: &AtomicU64) -> Result<PathBuf, String> {
    validate_download(release)?;
    let path = dir.join(&release.name);
    if verify(&path, release).is_ok() {
        return Ok(path);
    }
    save_download(Chunks { http: agent(45), release, offset: 0, remaining: 0, reader: None }, release, dir, canceled, progress)
}

struct Chunks<'a> {
    http: ureq::Agent,
    release: &'a Release,
    offset: u64,
    remaining: u64,
    reader: Option<Box<dyn Read>>,
}

impl Read for Chunks<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() || self.offset == self.release.size {
            return Ok(0);
        }
        if self.reader.is_none() {
            let end = self.offset.saturating_add(8 * 1024 * 1024 - 1).min(self.release.size.saturating_sub(1));
            let response = self.http.get(&self.release.url).header("Range", &format!("bytes={}-{}", self.offset, end)).call().map_err(std::io::Error::other)?;
            let expected = format!("bytes {}-{}/{}", self.offset, end, self.release.size);
            if response.status().as_u16() == 206 {
                if response.headers().get("Content-Range").and_then(|v| v.to_str().ok()) != Some(expected.as_str()) {
                    return Err(std::io::Error::other("The download server returned a different byte range."));
                }
                self.remaining = end.saturating_sub(self.offset).saturating_add(1);
            } else if self.offset == 0 && self.release.size <= 8 * 1024 * 1024 {
                self.remaining = self.release.size;
            } else {
                return Err(std::io::Error::other("The download server does not support bounded update downloads. Use the release download link."));
            }
            self.reader = Some(Box::new(response.into_body().into_reader()));
        }
        let limit = buf.len().min(usize::try_from(self.remaining).unwrap_or(usize::MAX));
        let target = buf.get_mut(..limit).ok_or_else(|| std::io::Error::other("Invalid download buffer."))?;
        let n = self.reader.as_mut().ok_or_else(|| std::io::Error::other("No download reader."))?.read(target)?;
        if n == 0 {
            return Err(std::io::Error::other("The update download was interrupted. Retry the download."));
        }
        self.remaining = self.remaining.saturating_sub(n as u64);
        self.offset = self.offset.saturating_add(n as u64);
        if self.remaining == 0 {
            self.reader = None;
        }
        Ok(n)
    }
}

fn save_download(mut reader: impl Read, release: &Release, dir: &Path, canceled: &AtomicBool, progress: &AtomicU64) -> Result<PathBuf, String> {
    let path = dir.join(&release.name);
    let mut file = tempfile::NamedTempFile::new_in(dir).map_err(|e| e.to_string())?;
    let mut count = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        if canceled.load(Ordering::Relaxed) {
            return Err("Download canceled.".into());
        }
        let read = reader.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        count = count.saturating_add(read as u64);
        if count > release.size || count > MAX_PACKAGE {
            return Err("The update exceeds its published size.".into());
        }
        file.write_all(buffer.get(..read).ok_or("Invalid download chunk.")?).map_err(|e| e.to_string())?;
        progress.store(count, Ordering::Relaxed);
    }
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    if canceled.load(Ordering::Relaxed) {
        return Err("Download canceled.".into());
    }
    verify(file.path(), release)?;
    file.persist(&path).map_err(|e| e.to_string())?;
    Ok(path)
}

pub fn validate_download(r: &Release) -> Result<(), String> {
    let version = Version::parse(&r.version).map_err(|e| e.to_string())?;
    if !version.pre.is_empty()
        || r.size == 0
        || r.size > MAX_PACKAGE
        || !r.name.starts_with(&format!("effectcraft-{}-", r.version))
        || r.name.contains(['/', '\\'])
        || r.name.contains("..")
    {
        return Err("Invalid update package metadata.".into());
    }
    valid_sha(&r.sha256)?;
    validate_url(&r.url, &r.tag, &r.name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn body(tag: &str, suffix: &str) -> String {
        let name = format!("effectcraft-{}-{suffix}", tag.trim_start_matches('v'));
        json!({"tag_name": tag, "draft": false, "prerelease": false, "assets": [{"name": name, "size": 3, "browser_download_url": format!("{PREFIX}{tag}/{name}"), "digest": format!("sha256:{:x}", Sha256::digest(b"abc"))}]}).to_string()
    }

    #[test]
    fn updates_only_to_newer_stable_versions_and_selects_exact_architecture() {
        let suffix = "windows-arm64-portable.zip";
        let r = parse(&body("v0.11.0", suffix), "0.10.0", suffix).unwrap().unwrap();
        assert_eq!(r.name, "effectcraft-0.11.0-windows-arm64-portable.zip");
        assert!(parse(&body("v0.10.0", suffix), "0.10.0", suffix).unwrap().is_none());
        assert!(parse(&body("v0.9.0", suffix), "0.10.0", suffix).unwrap().is_none());
        assert!(parse(&body("v0.11.0-rc.1", suffix), "0.10.0", suffix).unwrap().is_none());
        assert!(parse(&body("v0.11.0", suffix), "0.10.0", "windows-x64.msi").is_err());
        assert!(parse(&body("v0.11.0", suffix).replace(PREFIX, "https://evil.invalid/"), "0.10.0", suffix).is_err());
    }

    #[test]
    fn incomplete_oversized_corrupt_and_canceled_downloads_never_become_installable() {
        let r = parse(&body("v0.11.0", "linux-x86_64.AppImage"), "0.10.0", "linux-x86_64.AppImage").unwrap().unwrap();
        for (bytes, cancel, ok) in [
            (b"abc".as_slice(), false, true),
            (b"ab".as_slice(), false, false),
            (b"abcd".as_slice(), false, false),
            (b"abd".as_slice(), false, false),
            (b"abc".as_slice(), true, false),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let result = save_download(std::io::Cursor::new(bytes), &r, dir.path(), &AtomicBool::new(cancel), &AtomicU64::new(0));
            assert_eq!(result.is_ok(), ok);
            assert_eq!(dir.path().join(&r.name).exists(), ok);
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), usize::from(ok));
        }
    }

    #[test]
    fn checksum_is_required_and_corrupt_cached_downloads_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("package");
        let r = parse(&body("v0.11.0", "linux-x86_64.AppImage"), "0.10.0", "linux-x86_64.AppImage").unwrap().unwrap();
        std::fs::write(&path, b"abc").unwrap();
        verify(&path, &r).unwrap();
        std::fs::write(&path, b"abd").unwrap();
        assert!(verify(&path, &r).unwrap_err().contains("checksum"));
        assert!(checksum(&format!("{}  x\n{}  x", r.sha256, r.sha256), "x").is_err());
        assert!(checksum("bad  x", "x").is_err());
    }
}
