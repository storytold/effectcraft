//! The Figma REST API (`GET /v1/files/:key/nodes`, `GET /v1/images/:key`) through the system's
//! `curl`, as model downloads do (`crate::models`): no networking code is linked into
//! EffectCraft. The personal access token goes to curl on stdin (a `-K -` config), never on
//! the command line, and only to `api.figma.com` (or a loopback IP for test servers).

use std::io::{Read as _, Write as _};
use std::process::{Command, Stdio};

pub const API: &str = "https://api.figma.com";

/// A file key and node ids from a Figma link
/// (`https://www.figma.com/design/KEY/Name?node-id=1-2`, `/file/`, `/proto/`, branch links).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FigmaRef {
    pub key: String,
    pub ids: Vec<String>,
}

fn valid_key(k: &str) -> bool {
    (10..=128).contains(&k.len()) && k.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Node ids use `:` in the API and `-` in links (`1-2` → `1:2`; instance ids keep `;`/`I`).
pub fn normalize_id(id: &str) -> Option<String> {
    let id = percent_decode(id.trim());
    if id.is_empty() || id.len() > 512 || !id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '-' | ';' | 'I')) {
        return None;
    }
    Some(if id.contains(':') { id } else { id.replacen('-', ":", 1) })
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let c = b.get(i).copied().unwrap_or(0);
        if c == b'%'
            && let Some(h) = b.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(h);
            i += 3;
            continue;
        }
        out.push(c);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn parse_url(url: &str) -> Result<FigmaRef, String> {
    let u = url.trim();
    let rest = u.strip_prefix("https://").or_else(|| u.strip_prefix("http://")).unwrap_or(u);
    let (host_path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let mut parts = host_path.split('/');
    let host = parts.next().unwrap_or("");
    if !(host == "figma.com" || host.ends_with(".figma.com")) {
        return Err(format!("not a Figma link: {url}"));
    }
    let segs: Vec<&str> = parts.filter(|p| !p.is_empty()).collect();
    let kind = segs.first().copied().unwrap_or("");
    if !matches!(kind, "file" | "design" | "proto" | "board") {
        return Err(format!("expected a figma.com/design/<file key>/… link, got {url}"));
    }
    // Branch links: /design/KEY/branch/BRANCH_KEY/Name.
    let key = match (segs.get(2), segs.get(3)) {
        (Some(&"branch"), Some(b)) => b,
        _ => segs.get(1).copied().unwrap_or(""),
    };
    if !valid_key(key) {
        return Err(format!("no file key in {url}"));
    }
    let mut ids = vec![];
    for kv in query.split('&') {
        if let Some(v) = kv.strip_prefix("node-id=") {
            for id in percent_decode(v).split(',') {
                ids.push(normalize_id(id).ok_or_else(|| format!("bad node-id `{id}`"))?);
            }
        }
    }
    Ok(FigmaRef { key: key.to_string(), ids })
}

/// Only Figma's API (HTTPS) or a loopback server (tests, local proxies) gets the token.
pub fn check_api_base(base: &str) -> Result<String, String> {
    let b = base.trim_end_matches('/');
    if b == API {
        return Ok(b.to_string());
    }
    if loopback(b) || loopback(&b.replacen("https://", "http://", 1)) {
        return Ok(b.to_string());
    }
    Err(format!("apiBase must be {API} or a loopback address, not `{base}`"))
}

/// `http://` to a loopback IP literal (`127.x.x.x`, `[::1]`), with an optional numeric port. The
/// authority is parsed strictly: user info (`127.0.0.1:80@evil.com`), host names (which could
/// resolve anywhere) and anything that isn't an IP address are refused, so the token can't be
/// sent off the machine.
fn loopback(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("http://") else { return false };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let Some(authority) = rest.get(..end) else { return false };
    if authority.is_empty() || !authority.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | ':' | '[' | ']')) {
        return false;
    }
    let (host, port) = match authority.strip_prefix('[') {
        Some(v6) => match v6.split_once(']') {
            Some((h, rest)) => (h, rest),
            None => return false,
        },
        None => match authority.split_once(':') {
            Some((h, p)) => (h, authority.get(h.len()..).unwrap_or(p)),
            None => (authority, ""),
        },
    };
    let port_ok = port.is_empty() || port.strip_prefix(':').is_some_and(|p| !p.is_empty() && p.len() <= 5 && p.chars().all(|c| c.is_ascii_digit()));
    port_ok && host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

fn check_token(t: &str) -> Result<&str, String> {
    let t = t.trim();
    if t.is_empty() || t.len() > 512 || !t.chars().all(|c| c.is_ascii_graphic() && c != '"' && c != '\\') {
        return Err("the Figma token is not valid (a personal access token from Figma ▸ Settings ▸ Security)".into());
    }
    Ok(t)
}

/// curl's options for a GET of `url` (they go before `-o FILE -K - URL`). HTTPS only, except
/// plain HTTP to a loopback test server. A request that carries the token never follows
/// redirects: curl would send the `X-Figma-Token` header on to the redirect target, and Figma's
/// API doesn't redirect. Token-less downloads (pre-signed image links) follow redirects, over
/// HTTPS only.
fn curl_args(url: &str, with_token: bool, max_bytes: u64) -> Vec<String> {
    let proto = if loopback(url) { "=https,http" } else { "=https" };
    let mut a: Vec<String> = vec!["-q".into(), "-sS".into(), "--proto".into(), proto.into()];
    if !with_token {
        a.extend(["-L".into(), "--max-redirs".into(), "5".into(), "--proto-redir".into(), proto.into()]);
    }
    a.extend(["--connect-timeout".into(), "15".into(), "--max-time".into(), "120".into(), "--max-filesize".into(), max_bytes.to_string()]);
    a.extend(["-w".into(), "\n%{http_code}".into()]);
    a
}

/// GET `url` with curl; `token` goes in an `X-Figma-Token` header via stdin. Returns the body
/// of a 2xx reply, else an error with Figma's message.
fn curl(url: &str, token: Option<&str>, max_bytes: u64) -> Result<Vec<u8>, String> {
    // Validate before spawning, and keep private documents in a bounded pipe, never shared temp files.
    let cfg = match token {
        Some(t) => format!("header = \"X-Figma-Token: {}\"\n", check_token(t)?),
        None => String::new(),
    };
    let mut cmd = Command::new("curl");
    cmd.args(curl_args(url, token.is_some(), max_bytes)).args(["-K", "-"]).arg(url).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("talking to Figma needs curl ({e})"))?;
    let read = (|| -> Result<Vec<u8>, String> {
        let mut stdin = child.stdin.take().ok_or("curl: missing input pipe")?;
        stdin.write_all(cfg.as_bytes()).map_err(|e| format!("curl: {e}"))?;
        drop(stdin);
        let stdout = child.stdout.take().ok_or("curl: missing output pipe")?;
        let mut body = Vec::new();
        // Four bytes for the status trailer, plus one to detect an oversized response.
        stdout.take(max_bytes.saturating_add(5)).read_to_end(&mut body).map_err(|e| format!("reading Figma response: {e}"))?;
        if body.len() as u64 > max_bytes.saturating_add(4) {
            return Err(format!("Figma response exceeds {max_bytes} bytes"));
        }
        Ok(body)
    })();
    let mut body = match read {
        Ok(body) => body,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
    };
    let out = child.wait_with_output().map_err(|e| format!("curl: {e}"))?;
    let trailer = body.len().saturating_sub(4);
    let code =
        body.get(trailer..).and_then(|s| s.strip_prefix(b"\n")).and_then(|s| std::str::from_utf8(s).ok()).and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
    if code != 0 {
        body.truncate(trailer);
    }
    if !out.status.success() || !(200..300).contains(&code) {
        let msg = serde_json::from_slice::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("err").or_else(|| v.get("message")).and_then(|m| m.as_str()).map(str::to_string))
            .unwrap_or_else(|| String::from_utf8_lossy(&out.stderr).trim().to_string());
        let hint = match code {
            403 => " (check the token: Figma ▸ Settings ▸ Security ▸ Personal access tokens, with file content read access)",
            404 => " (no such file, or the token's account can't open it)",
            429 => " (rate limited by Figma; try again in a minute)",
            _ => "",
        };
        return Err(format!("Figma request failed: HTTP {code}{}{hint}", if msg.is_empty() { String::new() } else { format!(": {msg}") }));
    }
    Ok(body)
}

/// `GET /v1/files/:key/nodes?ids=…&geometry=paths`.
pub fn nodes(base: &str, key: &str, ids: &[String], token: &str) -> Result<serde_json::Value, String> {
    if !valid_key(key) {
        return Err(format!("bad file key `{key}`"));
    }
    let ids: Vec<String> = ids.iter().map(|i| normalize_id(i).ok_or_else(|| format!("bad node id `{i}`"))).collect::<Result<_, _>>()?;
    let url = format!("{base}/v1/files/{key}/nodes?ids={}&geometry=paths", ids.join(",").replace(':', "%3A").replace(';', "%3B"));
    let body = curl(&url, Some(token), 512 << 20)?;
    serde_json::from_slice(&body).map_err(|e| format!("Figma sent invalid JSON: {e}"))
}

/// Render `ids` as PNG at `scale` (`GET /v1/images/:key?format=png&use_absolute_bounds=true`)
/// and download each: (id, PNG bytes or the reason it failed).
pub fn images(base: &str, key: &str, ids: &[String], scale: f64, token: &str) -> Result<Vec<(String, Result<Vec<u8>, String>)>, String> {
    let mut out = vec![];
    for chunk in ids.chunks(50) {
        let q: Vec<String> = chunk.iter().filter_map(|i| normalize_id(i)).collect();
        let url =
            format!("{base}/v1/images/{key}?ids={}&format=png&scale={scale}&use_absolute_bounds=true", q.join(",").replace(':', "%3A").replace(';', "%3B"));
        let body = curl(&url, Some(token), 16 << 20)?;
        let v: serde_json::Value = serde_json::from_slice(&body).map_err(|e| format!("Figma sent invalid JSON: {e}"))?;
        if let Some(e) = v.get("err").and_then(|e| e.as_str()) {
            return Err(format!("Figma could not render images: {e}"));
        }
        for id in chunk {
            let link = v.get("images").and_then(|m| m.get(id.as_str())).and_then(|u| u.as_str());
            let r = match link {
                // Image links are pre-signed: no token.
                Some(l) if l.starts_with("https://") || loopback(l) => curl(l, None, 256 << 20),
                Some(l) => Err(format!("unexpected image link {l}")),
                None => Err("Figma rendered no image (empty or invisible node)".into()),
            };
            out.push((id.clone(), r));
        }
    }
    Ok(out)
}

/// Standard base64 (padding optional, whitespace ignored), for PNGs in plug-in payloads.
pub fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    let s = s.split_once("base64,").map(|(_, b)| b).unwrap_or(s);
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return Err("invalid base64".into()),
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downloads_do_not_require_temporary_storage() {
        let missing = std::env::temp_dir().join(format!("ec-figma-no-temp-{}", std::process::id())).join("missing");
        assert!(!missing.exists());
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                &format!("{}::downloads_preserve_binary_bodies_and_bound_unknown_lengths", module_path!().split_once("::").unwrap().1),
                "--nocapture",
            ])
            .env("TMPDIR", &missing)
            .env("TMP", &missing)
            .env("TEMP", &missing)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("1 passed"), "{stdout}\n{}", String::from_utf8_lossy(&output.stderr));
        assert!(output.status.success(), "{stdout}");
    }

    #[test]
    fn downloads_preserve_binary_bodies_and_bound_unknown_lengths() {
        use std::io::{BufRead, BufReader};
        for (status, bytes, limit, expected) in [
            ("200 OK", b"\0\xff\n200".as_slice(), 6, true),
            ("200 OK", b"".as_slice(), 0, true),
            ("200 OK", b"too many bytes".as_slice(), 4, false),
            ("403 Forbidden", b"denied".as_slice(), 64, false),
        ] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/test", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                }
                // No Content-Length: the reader must enforce the cap even with older curl versions.
                write!(stream, "HTTP/1.1 {status}\r\nConnection: close\r\n\r\n").unwrap();
                let _ = stream.write_all(bytes);
            });
            let result = curl(&url, Some("figd_test"), limit);
            server.join().unwrap();
            if expected {
                assert_eq!(result.unwrap(), bytes);
            } else {
                assert!(result.is_err(), "{result:?}");
            }
        }
    }

    #[test]
    fn token_requests_never_follow_redirects() {
        let a = curl_args("https://api.figma.com/v1/files/KEY/nodes?ids=1%3A2", true, 1 << 20);
        assert_eq!(a.first().map(String::as_str), Some("-q"));
        assert!(!a.iter().any(|x| x == "-L" || x == "--location" || x == "--location-trusted"), "{a:?}");
        assert!(a.windows(2).any(|w| w[0] == "--proto" && w[1] == "=https"), "{a:?}");
        // The token-less image download may follow redirects, HTTPS only.
        let a = curl_args("https://s3-alpha.figma.com/img/x.png", false, 1 << 20);
        assert!(a.iter().any(|x| x == "-L"), "{a:?}");
        assert!(a.windows(2).any(|w| w[0] == "--proto-redir" && w[1] == "=https"), "{a:?}");
        // Plain HTTP only for a loopback test server.
        let a = curl_args("http://127.0.0.1:8080/v1/images/KEY", true, 1 << 20);
        assert!(a.windows(2).any(|w| w[0] == "--proto" && w[1] == "=https,http"), "{a:?}");
        assert!(!a.iter().any(|x| x == "-L"), "{a:?}");
    }
}
