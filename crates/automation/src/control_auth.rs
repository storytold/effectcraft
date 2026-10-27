//! Authentication for the desktop app's control channel (#2122), shared by the app (server side)
//! and [`BridgeClient`](crate::BridgeClient) (client side).
//!
//! Loopback alone doesn't keep other local processes out (other users on a shared machine,
//! containers and WSL sharing the host's network, sandboxed apps with network access), and the
//! channel can run scripts. So, as in PhotoCraft:
//!
//! * the app generates a fresh 256-bit token from the OS CSPRNG each time it starts listening,
//!   and writes it to a file only the user can read: `control-<port>.token` in the EffectCraft
//!   settings folder (or `EFFECTCRAFT_CONTROL_TOKEN_FILE`). The token is never printed or logged;
//! * the first request on every connection must be
//!   `{"id":"auth","method":"auth","params":{"token":"<64 hex>"}}`, answered with
//!   `{"id":"auth","ok":true,"result":{"authenticated":true}}`. Anything else gets an
//!   `authentication required` error and the connection is closed before it runs;
//! * clients read the token from the same file (or `EFFECTCRAFT_CONTROL_TOKEN`) when they connect,
//!   so a client started before the app, or kept across an app restart, picks up the new token.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::{Error, Result};

/// The method of the first request on every connection.
pub const AUTH_METHOD: &str = "auth";
/// Client side: the token itself (instead of reading the token file).
pub const TOKEN_ENV: &str = "EFFECTCRAFT_CONTROL_TOKEN";
/// Where the app writes the token and clients read it, instead of the settings folder.
pub const TOKEN_FILE_ENV: &str = "EFFECTCRAFT_CONTROL_TOKEN_FILE";

const TOKEN_BYTES: usize = 32;
const TOKEN_HEX_LEN: usize = TOKEN_BYTES * 2;

/// A new 256-bit token as 64 lowercase hex digits, from the operating system's CSPRNG.
pub fn generate_token() -> Result<String> {
    let mut bytes = [0u8; TOKEN_BYTES];
    getrandom::fill(&mut bytes).map_err(|e| Error::Other(format!("cannot generate a control token: {e}")))?;
    const HEX: &[u8; 16] = b"0123456789abcdef";
    Ok(bytes.iter().flat_map(|b| [char::from(HEX[usize::from(b >> 4)]), char::from(HEX[usize::from(b & 0x0f)])]).collect())
}

/// Accept only the form [`generate_token`] produces: exactly 64 hex digits.
pub fn validate_token(token: &str) -> Result<()> {
    if token.len() != TOKEN_HEX_LEN || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::BadArgs("a control token must be exactly 64 hexadecimal characters".into()));
    }
    Ok(())
}

/// Compare two tokens without stopping at the first differing byte, so the time taken doesn't
/// tell a guesser how much of a guess was right. Hex letters match in either case.
pub fn token_matches(expected: &str, supplied: &str) -> bool {
    if expected.len() != TOKEN_HEX_LEN || supplied.len() != TOKEN_HEX_LEN {
        return false;
    }
    expected.bytes().zip(supplied.bytes()).fold(0u8, |diff, (a, b)| diff | (a.to_ascii_lowercase() ^ b.to_ascii_lowercase())) == 0
}

/// Whether `params` of an [`AUTH_METHOD`] request carry the `expected` token.
pub fn auth_params_match(params: &Value, expected: &str) -> bool {
    params.get("token").and_then(Value::as_str).is_some_and(|t| token_matches(expected, t))
}

/// The request a client sends first on every connection.
pub fn auth_request(token: &str) -> Value {
    json!({"id": AUTH_METHOD, "method": AUTH_METHOD, "params": {"token": token}})
}

/// The token file's name for the app listening on `port`: `control-9877.token`.
pub fn token_file_name(port: u16) -> String {
    format!("control-{port}.token")
}

/// Where the token for `port` lives: `env_file` (`EFFECTCRAFT_CONTROL_TOKEN_FILE`) when given, else
/// [`token_file_name`] in `config_dir` (the settings folder, `effectcraft_host::config_dir()`).
pub fn token_file_path(port: u16, env_file: Option<PathBuf>, config_dir: Option<&Path>) -> Result<PathBuf> {
    if let Some(file) = env_file.filter(|f| !f.as_os_str().is_empty()) {
        return Ok(file);
    }
    config_dir.map(|d| d.join(token_file_name(port))).ok_or_else(|| Error::Other(format!("no settings folder for the control token; set {TOKEN_FILE_ENV}")))
}

/// [`token_file_path`] with `EFFECTCRAFT_CONTROL_TOKEN_FILE` from the environment.
pub fn token_file_path_from_env(port: u16, config_dir: Option<&Path>) -> Result<PathBuf> {
    token_file_path(port, std::env::var_os(TOKEN_FILE_ENV).map(PathBuf::from), config_dir)
}

/// Write `token` to `path`, replacing the previous launch's file, readable by the user only.
///
/// Unix: created with mode 0600. Windows: no ACL is set here; the file inherits the folder's,
/// and the default folder (`%APPDATA%\EffectCraft`, under the user profile) is readable only by
/// the user, SYSTEM and administrators. A custom `EFFECTCRAFT_CONFIG_DIR`, portable data folder or
/// `EFFECTCRAFT_CONTROL_TOKEN_FILE` must be just as private.
pub fn write_token_file(path: &Path, token: &str) -> Result<()> {
    let io = |e: std::io::Error| Error::Other(format!("cannot write the control token file {}: {e}", path.display()));
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(io)?;
    }
    // Remove, then create anew: an old file keeps its old permissions if merely truncated, and
    // `create_new` doesn't follow a symlink planted in its place.
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(io(e)),
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(io)?;
    writeln!(file, "{token}").and_then(|()| file.flush()).map_err(io)
}

/// Read and validate a token file. On Unix a file that group or others can read is refused: its
/// token can't be trusted to be secret.
pub fn read_token_file(path: &Path) -> Result<String> {
    let io =
        |e: std::io::Error| Error::Bridge(format!("cannot read the control token file {} ({e}); is the app running with `--control <port>`?", path.display()));
    let file = std::fs::File::open(path).map_err(io)?;
    let meta = file.metadata().map_err(io)?;
    if !meta.is_file() {
        return Err(Error::Bridge(format!("the control token file {} is not a regular file", path.display())));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.mode() & 0o077 != 0 {
            return Err(Error::Bridge(format!(
                "the control token file {} can be read by other users (mode {:o}); restrict it to its owner (chmod 600)",
                path.display(),
                meta.mode() & 0o777
            )));
        }
    }
    let text = std::io::read_to_string(file).map_err(io)?;
    let token = text.trim();
    validate_token(token).map_err(|e| Error::Bridge(format!("{}: {e}", path.display())))?;
    Ok(token.to_ascii_lowercase())
}

/// The port in a bridge address (`9877`, `127.0.0.1:9877`, `[::1]:9877`).
pub fn port_of(addr: &str) -> Option<u16> {
    addr.parse().ok().or_else(|| addr.rsplit_once(':').and_then(|(_, p)| p.parse().ok()))
}

/// Where a client gets the control token from. Read on every (re)connect.
#[derive(Clone)]
pub enum TokenSource {
    /// A token given directly (`EFFECTCRAFT_CONTROL_TOKEN`, tests).
    Token(String),
    /// The token file the app writes.
    File(PathBuf),
    /// No token can be found; the reason is reported when connecting.
    Missing(String),
}

/// Never prints the token itself, so `{:?}` in logs or errors can't leak it.
impl std::fmt::Debug for TokenSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokenSource::Token(_) => f.write_str("Token(<redacted>)"),
            TokenSource::File(path) => f.debug_tuple("File").field(path).finish(),
            TokenSource::Missing(why) => f.debug_tuple("Missing").field(why).finish(),
        }
    }
}

impl TokenSource {
    /// For the app at `addr`: `EFFECTCRAFT_CONTROL_TOKEN`, else the token file
    /// (`EFFECTCRAFT_CONTROL_TOKEN_FILE`, else `control-<port>.token` in `config_dir`).
    pub fn for_addr(addr: &str, config_dir: Option<&Path>) -> Self {
        Self::from_inputs(addr, std::env::var(TOKEN_ENV).ok(), std::env::var_os(TOKEN_FILE_ENV).map(PathBuf::from), config_dir)
    }

    /// [`Self::for_addr`] with the environment passed in.
    pub fn from_inputs(addr: &str, env_token: Option<String>, env_file: Option<PathBuf>, config_dir: Option<&Path>) -> Self {
        if let Some(token) = env_token.filter(|t| !t.trim().is_empty()) {
            return TokenSource::Token(token.trim().to_string());
        }
        let Some(port) = port_of(addr) else {
            return TokenSource::Missing(format!("no port in `{addr}` to find the control token file for"));
        };
        match token_file_path(port, env_file, config_dir) {
            Ok(path) => TokenSource::File(path),
            Err(e) => TokenSource::Missing(e.to_string()),
        }
    }

    /// The token to send.
    pub fn token(&self) -> Result<String> {
        match self {
            TokenSource::Token(t) => {
                validate_token(t).map_err(|e| Error::Bridge(format!("{TOKEN_ENV}: {e}")))?;
                Ok(t.to_ascii_lowercase())
            }
            TokenSource::File(path) => read_token_file(path),
            TokenSource::Missing(why) => Err(Error::Bridge(format!("no control token: {why}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ec-control-auth-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn generated_tokens_are_valid_distinct_and_compared_exactly() {
        let a = generate_token().unwrap();
        let b = generate_token().unwrap();
        validate_token(&a).unwrap();
        assert_ne!(a, b);
        assert!(token_matches(&a, &a));
        assert!(token_matches(&a, &a.to_ascii_uppercase()));
        assert!(!token_matches(&a, &b));
        assert!(!token_matches(&a, &a[..63]));
        assert!(!token_matches(&a, ""));
        assert!(validate_token("abc").is_err());
        assert!(validate_token(&"g".repeat(64)).is_err());
    }

    #[test]
    fn auth_params_need_the_exact_token() {
        let t = generate_token().unwrap();
        assert!(auth_params_match(&json!({"token": t}), &t));
        assert!(!auth_params_match(&json!({"token": generate_token().unwrap()}), &t));
        assert!(!auth_params_match(&json!({"token": 5}), &t));
        assert!(!auth_params_match(&json!({}), &t));
        assert_eq!(auth_request(&t)["method"], AUTH_METHOD);
    }

    #[test]
    fn token_file_round_trips_and_is_replaced_each_launch() {
        let dir = temp_dir("roundtrip");
        let path = token_file_path(9877, None, Some(&dir)).unwrap();
        assert!(path.ends_with("control-9877.token"));
        let first = generate_token().unwrap();
        write_token_file(&path, &first).unwrap();
        assert_eq!(read_token_file(&path).unwrap(), first);
        let second = generate_token().unwrap();
        write_token_file(&path, &second).unwrap();
        assert_eq!(read_token_file(&path).unwrap(), second);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(read_token_file(&path).unwrap_err().to_string().contains("other users"));
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn token_source_prefers_the_variable_then_the_file() {
        let dir = Path::new("/cfg");
        let t = "ab".repeat(32);
        assert!(matches!(TokenSource::from_inputs("9877", Some(t.clone()), None, Some(dir)), TokenSource::Token(x) if x == t));
        let TokenSource::File(p) = TokenSource::from_inputs("127.0.0.1:9000", None, None, Some(dir)) else { panic!("file") };
        assert_eq!(p, dir.join("control-9000.token"));
        let TokenSource::File(p) = TokenSource::from_inputs("[::1]:9000", None, Some("/x/t".into()), Some(dir)) else { panic!("env file") };
        assert_eq!(p, PathBuf::from("/x/t"));
        assert!(TokenSource::from_inputs("9877", None, None, None).token().is_err());
        assert!(TokenSource::Token("short".into()).token().is_err());
        let missing = TokenSource::File(temp_dir("missing").join("control-1.token")).token().unwrap_err().to_string();
        assert!(missing.contains("--control"), "{missing}");
    }

    #[test]
    fn debug_output_redacts_the_token() {
        let t = "cd".repeat(32);
        let shown = format!("{:?} {:?}", TokenSource::Token(t.clone()), TokenSource::File("/cfg/control-1.token".into()));
        assert!(!shown.contains(&t) && shown.contains("redacted") && shown.contains("control-1.token"), "{shown}");
    }
}
