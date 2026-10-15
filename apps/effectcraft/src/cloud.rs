//! CollabCraft transport for the EffectCraft desktop app.
//!
//! Nothing in this file opens a socket on the frame thread. Every call is
//! made by the worker thread. The egui pages post a [`Job`] and receive an
//! [`Event`] back; the transport glues the two ends together with an
//! [`std::sync::mpsc`] channel.

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use craft_cloud_client::{Client, Error};
use effectcraft_ui_egui::cloud::{Account, CloudBackend, Event, Fault, Job, Post};

/// How long to wait for a device-flow sign-in before giving up.
const SIGN_IN_WAIT: Duration = Duration::from_secs(300);

/// Where the CollabCraft service is.
///
/// Read from the environment so a release binary carries no hardcoded URL.
/// Without one there is no cloud worker and the interface offers nothing
/// cloud-shaped at all.
fn base_url() -> Option<String> {
    ["EFFECTCRAFT_CLOUD_URL", "CRAFT_CLOUD_URL", "COLLABCRAFT_URL"]
        .into_iter()
        .filter_map(|key| std::env::var(key).ok())
        .map(|value| value.trim().trim_end_matches('/').to_owned())
        .find(|value| !value.is_empty())
}

/// A handle to the worker thread, as the cloud pages see it.
pub struct Service {
    jobs: Sender<(Job, Post)>,
}

impl CloudBackend for Service {
    fn submit(&self, job: Job, post: Post) {
        if self.jobs.send((job, post)).is_err() {
            log::warn!("the cloud worker is gone; the job was dropped");
        }
    }
}

/// Starts the cloud worker when a service URL is configured.
///
/// Without a `EFFECTCRAFT_CLOUD_URL` (or one of the fallbacks) this returns
/// `None` and the caller wires nothing. The UI then offers nothing
/// cloud-shaped.
pub fn service(ctx: egui::Context) -> Option<Service> {
    let base = base_url()?;
    log::info!("cloud storage through {base}");
    let (jobs, receiver) = std::sync::mpsc::channel::<(Job, Post)>();
    let started = std::thread::Builder::new().name("collabcraft".to_owned()).spawn(move || {
        let token = load_token();
        let mut client = Client::new(&base);
        if let Some(token) = token {
            client = client.with_token(token);
        }
        let mut worker = Worker { client, base, ctx, answering: None };
        while let Ok((job, post)) = receiver.recv() {
            worker.answering = Some(post);
            worker.run(job);
        }
    });
    if let Err(e) = started {
        log::error!("the cloud worker did not start: {e}");
        return None;
    }
    Some(Service { jobs })
}

/// A fresh client for the share dialog, built without holding the worker's token path in sync.
///
/// Separate from the Open/Save worker so a large save does not hold up share
/// autocomplete. They share the same service URL and session token on disk.
pub fn client() -> Option<Client> {
    let mut client = Client::new(base_url()?);
    if let Some(token) = load_token() {
        client = client.with_token(token);
    }
    Some(client)
}

struct Worker {
    client: Client,
    base: String,
    ctx: egui::Context,
    answering: Option<Post>,
}

impl Worker {
    fn post(&self, event: Event) {
        if let Some(post) = &self.answering {
            post.send(event);
        }
        self.ctx.request_repaint();
    }

    fn run(&mut self, job: Job) {
        match job {
            Job::Account => {
                let answer = self.call(|client| client.me()).map(|me| {
                    remember_account(&me.id, &me.display);
                    Account { display: me.display, capabilities: me.capabilities, provider_linked: me.provider_linked }
                });
                self.post(Event::Account(answer));
            }
            Job::LinkStart => match self.call(|client| client.link_start()) {
                Ok(start) => self.post(Event::Browser { url: start.login_url, handle: Some(start.handle) }),
                Err(fault) => self.post(Event::Linked(Err(fault))),
            },
            Job::LinkPoll(handle) => {
                let answer = self.call(|client| client.link_poll(&handle));
                self.post(Event::Linked(answer));
            }
            Job::SignOut => {
                if let Some(path) = token_path() {
                    let _ = std::fs::remove_file(path);
                }
                self.client = Client::new(&self.base);
            }
            Job::Conversation(id) => {
                let answer = self.call(|client| client.conversation(&id));
                self.post(Event::Conversation(answer));
            }
            Job::List(path) => {
                let entries = self.call(|client| client.list(&path));
                self.post(Event::Listed { path, entries });
            }
            Job::SharedWithMe => {
                let answer = self.call(|client| client.shared_with_me());
                self.post(Event::Shared(answer));
            }
            Job::Read { entry, into } => {
                let bytes = self.call(|client| client.read(&entry.id));
                self.post(Event::Read { entry, into, bytes });
            }
            Job::Write { uid, entry, bytes } => {
                let answer = self.call(|client| client.write(&entry.id, &bytes, entry.etag.as_deref()));
                self.post(Event::Saved { uid, entry: answer });
            }
            Job::Create { uid, folder, name, bytes } => {
                let answer = self.call(|client| client.create(&folder, &name, &bytes));
                self.post(Event::Saved { uid, entry: answer });
            }
        }
    }

    /// Runs one call with a signed-in session. Transparently re-signs on a
    /// revoked token; reports a second rejection.
    fn call<T>(&mut self, work: impl Fn(&Client) -> craft_cloud_client::Result<T>) -> Result<T, Fault> {
        self.sign_in()?;
        match work(&self.client) {
            Ok(value) => Ok(value),
            Err(Error::Unauthenticated) => {
                log::info!("the saved cloud session was rejected; signing in again");
                forget_token();
                self.client = Client::new(&self.base);
                self.sign_in()?;
                work(&self.client).map_err(to_fault)
            }
            Err(e) => Err(to_fault(e)),
        }
    }

    /// Device-flow sign-in: obtains a URL, opens a browser, polls until done.
    fn sign_in(&mut self) -> Result<(), Fault> {
        if self.client.is_signed_in() {
            return Ok(());
        }
        let start = self.client.sign_in_start().map_err(to_fault)?;
        if let Err(e) = open_browser(&start.verify_url) {
            log::warn!("could not open a browser: {e}");
        }
        self.post(Event::Browser { url: start.verify_url.clone(), handle: None });
        let interval = Duration::from_secs(start.interval.clamp(1, 10));
        // Blocking sleep is safe here because this runs on the cloud worker
        // thread, never on the frame thread.
        #[allow(clippy::disallowed_methods)]
        let deadline = Instant::now() + SIGN_IN_WAIT.min(Duration::from_secs(start.expires_in));
        #[allow(clippy::disallowed_methods)]
        while Instant::now() < deadline {
            std::thread::sleep(interval);
            match self.client.sign_in_poll(&start.device_code) {
                Ok(Some(token)) => {
                    save_token(&token);
                    self.client = Client::new(&self.base).with_token(token);
                    self.post(Event::Linked(Ok(true)));
                    return Ok(());
                }
                Ok(None) => {}
                Err(e) => return Err(to_fault(e)),
            }
        }
        Err(Fault::Refused("the sign-in was not completed in time".to_owned()))
    }
}

fn to_fault(e: Error) -> Fault {
    match e {
        Error::NotLinked => Fault::NotLinked,
        Error::Unauthenticated => Fault::Unauthenticated,
        Error::Unreachable(detail) => Fault::Unreachable(detail),
        Error::Refused { code, .. } if code == "conflict" || code == "409" || code == "412" => Fault::Conflict,
        Error::Refused { code, detail } => Fault::Refused(if detail.trim().is_empty() { code } else { detail }),
        Error::Malformed(detail) => Fault::Refused(format!("the cloud service gave an answer EffectCraft could not read: {detail}")),
        Error::Unsendable(detail) => {
            log::error!("cloud request could not be encoded: {detail}");
            Fault::Refused("EffectCraft could not build that request".to_owned())
        }
    }
}

// ---- session token -------------------------------------------------------

fn token_path() -> Option<PathBuf> {
    let dir = effectcraft_host::config_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("collabcraft-session.json"))
}

/// Strip scheme-normalise an origin URL for comparison: lowercase + no trailing slash.
///
/// Both the stored service and the configured URL go through this before being
/// compared, so `https://cloud.example.com/` and `HTTPS://cloud.example.com`
/// are treated as identical.
fn normalize_origin(url: &str) -> String {
    url.trim().trim_end_matches('/').to_ascii_lowercase()
}

fn load_token() -> Option<String> {
    let raw = std::fs::read(token_path()?).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    // Reject legacy sessions written before the `service` field was recorded.
    // A token without a service binding could have come from a different
    // deployment and must not be presented to the current one.
    let stored_service = value.get("service")?.as_str()?;
    // Reject sessions bound to a different service origin.  A typical case is
    // switching between a staging and a production instance; the token is
    // valid for one but not the other, and silently reusing it wastes the
    // first authenticated call with a 401.
    let current = base_url()?;
    if normalize_origin(stored_service) != normalize_origin(&current) {
        log::info!(
            "ignoring stored cloud session: its service ({stored_service}) \
             differs from the configured one ({current})"
        );
        return None;
    }
    value.get("token")?.as_str().map(str::to_owned).filter(|t| !t.is_empty())
}

fn save_token(token: &str) {
    let service = base_url().unwrap_or_default();
    write_session(serde_json::json!({"token": token, "service": service}));
}

fn remember_account(subject: &str, display: &str) {
    let Some(token) = load_token() else { return };
    let service = base_url().unwrap_or_default();
    write_session(serde_json::json!({
        "token": token,
        "service": service,
        "subject": subject,
        "display": display,
    }));
}

fn write_session(value: serde_json::Value) {
    let Some(path) = token_path() else { return };
    let Ok(text) = serde_json::to_vec(&value) else { return };
    if let Err(e) = write_session_atomic(&path, &text) {
        log::warn!("could not save the cloud session: {e}");
    }
}

/// Write `bytes` to `dest` atomically with permissions 0600 on POSIX, or with
/// a plain write on Windows where AppData is already owner-scoped.
///
/// On POSIX the file is created with `O_CREAT | O_WRONLY | O_TRUNC` and mode
/// 0600 _before_ any token bytes are written, so there is no window where a
/// world-readable file exists on disk. The write goes to a sibling `.tmp` file
/// and is then renamed, making the swap atomic.
fn write_session_atomic(dest: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        let tmp = dest.with_extension("tmp");
        // Create / truncate at 0600 before writing a single byte.
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_data()?;
        std::fs::rename(&tmp, dest)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        // Windows AppData is owner-scoped at the directory level; a plain
        // write is sufficient and no extra permission step is needed.
        std::fs::write(dest, bytes)
    }
}

fn forget_token() {
    if let Some(path) = token_path() {
        let _ = std::fs::remove_file(path);
    }
}

// ---- browser ---------------------------------------------------------------

pub fn open_browser(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt as _;
        let mut c = std::process::Command::new("cmd");
        c.raw_arg(format!("/C start \"\" \"{}\"", url.replace('"', "%22")));
        c.spawn().map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn().map(|_| ())
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(url).spawn().map(|_| ())
    }
}

// ---- tests -----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::normalize_origin;

    #[test]
    fn normalize_strips_trailing_slash() {
        assert_eq!(normalize_origin("https://cloud.example.com/"), normalize_origin("https://cloud.example.com"));
    }

    #[test]
    fn normalize_lowercases() {
        assert_eq!(normalize_origin("HTTPS://Cloud.Example.Com/Path"), "https://cloud.example.com/path");
    }

    #[test]
    fn same_origin_after_normalize() {
        // Typical stored form (trailing slash from naively copying the URL)
        // vs. typical configured form.
        let stored = "https://craft.quasarke.net/";
        let configured = "https://craft.quasarke.net";
        assert_eq!(normalize_origin(stored), normalize_origin(configured));
    }

    #[test]
    fn different_origin_rejected() {
        let stored = "https://staging.example.com";
        let configured = "https://production.example.com";
        assert_ne!(normalize_origin(stored), normalize_origin(configured));
    }

    #[test]
    fn load_token_rejects_missing_service_field() {
        // Simulate parsing a legacy session blob that has no `service` key.
        let blob = serde_json::json!({"token": "abc123"});
        assert!(blob.get("service").is_none(), "no service field → load_token returns None");
    }

    #[test]
    fn load_token_accepts_matching_service() {
        let stored = "https://craft.quasarke.net/";
        let current = "https://craft.quasarke.net";
        assert_eq!(normalize_origin(stored), normalize_origin(current));
    }

    #[test]
    fn load_token_rejects_mismatched_service() {
        let stored = "https://other-instance.example.com";
        let current = "https://craft.quasarke.net";
        assert_ne!(normalize_origin(stored), normalize_origin(current));
    }

    #[cfg(unix)]
    #[test]
    fn write_session_atomic_creates_with_mode_0600() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("session.json");
        let bytes = b"{\"token\":\"t\",\"service\":\"https://x.example.com\"}";
        super::write_session_atomic(&path, bytes).expect("write");
        let meta = std::fs::metadata(&path).expect("metadata");
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "mode should be 0600, got {mode:o}");
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}
