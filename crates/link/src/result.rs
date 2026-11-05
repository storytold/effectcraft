//! `link.renderResult`: encode the bound comp to `result_path` as ProRes 4444.
//!
//! The encode call is [`effectcraft_export::export`], the same function the export tests and
//! the host's `FileExporter` use. Frames are written to a part file in the same
//! directory under a name no other encode uses (`<result_path>.<n>.part`), so two renders of one
//! link never write the same file. The part file is renamed onto `result_path` only after that
//! call returns `Ok`.
//! The control handler calls [`render_result`] on the UI thread and waits for the file.
//! [`spawn_render`] moves a [`Session`] to a named thread, because the session is not shared
//! with the interface.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::JoinHandle;

use effectcraft_engine::Session;
use effectcraft_export::render_queue::{AudioOutput, Channels, OutputFormat, OutputModule, ProResProfile, RenderSettings, TimeSpan};
use effectcraft_export::{Job, JobOptions};
use effectcraft_project::{FootageKind, ItemId, ItemKind, Project, essential, render_queue::log_path};
use effectcraft_render::{Accelerator, ExprHost, FootageSource};
use serde::Serialize;
use serde_json::{Value, json};

/// `link.result` body. The control handler sends this as `result` after a successful render.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RenderOutcome {
    pub link_id: String,
    pub epoch: u64,
    pub path: String,
}

/// Failure of [`render_result`]. [`Self::reply`] is the object the control channel returns.
#[derive(Debug)]
pub struct RenderError {
    pub code: &'static str,
    pub message: String,
}

impl RenderError {
    pub fn comp_missing(message: impl Into<String>) -> Self {
        Self { code: "comp_missing", message: message.into() }
    }
    pub fn bad_params(message: impl Into<String>) -> Self {
        Self { code: "bad_params", message: message.into() }
    }
    /// The comp needs footage that is missing, unreadable or never probed.
    pub fn footage_missing(paths: &[String]) -> Self {
        Self { code: "footage_missing", message: format!("the composition cannot render, because its footage is missing or unreadable: {}", paths.join(", ")) }
    }
    /// The binding's `result_path` is not a file this link may write.
    pub fn refused(message: impl Into<String>) -> Self {
        Self { code: "result_path_refused", message: message.into() }
    }
    pub fn io(message: impl Into<String>) -> Self {
        Self { code: "io_error", message: message.into() }
    }
    /// The encode was stopped on purpose (the comp changed again). Not a failure to report.
    pub fn cancelled() -> Self {
        Self { code: "cancelled", message: "the render was cancelled".to_string() }
    }
    pub fn reply(&self) -> Value {
        json!({
            "ok": false,
            "error": {"code": self.code, "message": self.message, "data": {}}
        })
    }
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for RenderError {}

/// Cause of the change that stores a result epoch. The change tracker does not count it as an edit.
pub const RESULT_CAUSE: &str = "link.result";

/// A render of one link, ready to run on any thread. [`prepare_render`] checks everything that
/// can be checked up front, [`RenderPlan::encode`] writes the file, and [`RenderPlan::finish`]
/// records the epoch on the session.
pub struct RenderPlan {
    pub link_id: String,
    pub epoch: u64,
    pub result_path: String,
    comp: ItemId,
    project: Arc<Project>,
    footage: Arc<dyn FootageSource>,
    expr: Option<Arc<dyn ExprHost>>,
    accel: Option<Arc<dyn Accelerator>>,
    nested_switches: bool,
}

/// Check that `link_id` can render now and snapshot what the encoder needs. `epoch` is the epoch
/// the file will carry. `None` means one more than the binding stores or the live epoch holds.
pub fn prepare_render(session: &Session, link_id: &str, epoch: Option<u64>) -> Result<RenderPlan, RenderError> {
    let (comp, result_path, stored) = match session.project.craft_links.get(link_id) {
        Some(binding) => (binding.comp, binding.result_path.clone(), binding.epoch),
        None => return Err(RenderError::comp_missing(format!("no link `{link_id}`"))),
    };
    if session.project.comp(comp).is_none() {
        return Err(RenderError::comp_missing(format!("the composition for link `{link_id}` is not in the project")));
    }
    check_result_target(link_id, &result_path)?;
    let unusable = unusable_footage(session, comp);
    if !unusable.is_empty() {
        return Err(RenderError::footage_missing(&unusable));
    }
    let epoch = epoch.unwrap_or_else(|| stored.max(crate::live::epoch_of(link_id)).saturating_add(1));
    Ok(RenderPlan {
        link_id: link_id.to_string(),
        epoch,
        result_path,
        comp,
        project: Arc::clone(&session.project),
        footage: Arc::clone(&session.footage),
        expr: session.expr.clone(),
        accel: session.accel.clone(),
        nested_switches: session.prefs.general.switches_affect_nested_comps,
    })
}

/// Why `result_path` is refused, or `Ok` when the link may write there.
///
/// A saved project is untrusted, and `result_path` comes from it. The link writes only a movie
/// named `<link_id>.mov` that sits next to the `<link_id>.ecproj` sidecar that `link.open` made.
/// A file already at the name must be a QuickTime movie, which is what the link wrote last time.
/// No symlink is followed at the result name, and no directory is created. The part file is
/// made with [`reserve_part`], which never opens an existing name.
pub fn check_result_target(link_id: &str, result_path: &str) -> Result<(), RenderError> {
    let path = check_result_name(link_id, result_path)?;
    let Some(dir) = path.parent().filter(|dir| dir.is_dir()) else {
        return Err(RenderError::refused("the result directory does not exist; open the link again"));
    };
    let sidecar = dir.join(format!("{link_id}.ecproj"));
    if !is_regular_file(&sidecar) {
        return Err(RenderError::refused(format!("{} is not a Craft Link directory (no {link_id}.ecproj); open the link again", dir.display())));
    }
    match std::fs::symlink_metadata(path) {
        Err(_) => Ok(()),
        Ok(meta) if meta.is_file() && is_quicktime(path) => Ok(()),
        Ok(_) => Err(RenderError::refused(format!("{} exists and is not a movie this link made", path.display()))),
    }
}

/// The name rules alone, for `link.open`: absolute, `<link_id>.mov`.
pub fn check_result_name<'a>(link_id: &str, result_path: &'a str) -> Result<&'a Path, RenderError> {
    let path = Path::new(result_path);
    if !path.is_absolute() {
        return Err(RenderError::refused("result_path must be absolute"));
    }
    if path.file_name().and_then(|name| name.to_str()) != Some(format!("{link_id}.mov").as_str()) {
        return Err(RenderError::refused(format!("result_path must name `{link_id}.mov`")));
    }
    Ok(path)
}

fn is_regular_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file())
}

/// First atom type of a QuickTime or MP4 file.
fn is_quicktime(path: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 8];
    let read = std::fs::File::open(path).and_then(|mut file| file.read_exact(&mut head));
    read.is_ok() && matches!(&head[4..8], b"ftyp" | b"moov" | b"mdat" | b"wide" | b"free" | b"skip")
}

impl RenderPlan {
    /// Encode the comp to a part file and rename it onto `result_path`. `keep_going` is asked
    /// after every batch of frames; `false` stops the encode, removes the part file and returns
    /// `Err` with code `cancelled`. The result file is untouched unless this returns `Ok`.
    pub fn encode(&self, keep_going: &dyn Fn() -> bool) -> Result<(), RenderError> {
        let path = Path::new(&self.result_path);
        let temp = reserve_part(path, PART_SEQ.fetch_add(1, Ordering::Relaxed))?;
        let temp_str = temp.to_string_lossy().to_string();
        if let Err(e) = encode_prores(self, &temp_str, keep_going) {
            discard_temp(&temp);
            return Err(e);
        }
        if let Err(e) = std::fs::rename(&temp, path) {
            discard_temp(&temp);
            return Err(RenderError::io(format!("could not replace the result file: {e}")));
        }
        Ok(())
    }

    /// Store the epoch on the binding. The epoch never decreases. The change is published with
    /// cause [`RESULT_CAUSE`], so the tracker does not take it for an edit.
    pub fn finish(self, session: &mut Session) -> Result<RenderOutcome, RenderError> {
        let Some(binding) = session.project_mut().craft_links.get_mut(&self.link_id) else {
            return Err(RenderError::comp_missing(format!("link `{}` disappeared during render", self.link_id)));
        };
        binding.epoch = binding.epoch.max(self.epoch);
        let hooks = session.hooks.clone();
        hooks.with_cause(craft_command::Cause::Job(RESULT_CAUSE), || session.bump());
        Ok(RenderOutcome { link_id: self.link_id, epoch: self.epoch, path: self.result_path })
    }
}

/// Render the comp bound to `link_id` and store `previous epoch + 1` on that binding.
///
/// An unknown link or a missing comp returns [`RenderError`] code `comp_missing` and writes
/// nothing. The encoder must succeed before the part file replaces `result_path`.
pub fn render_result(session: &mut Session, link_id: &str) -> Result<RenderOutcome, RenderError> {
    let plan = prepare_render(session, link_id, None)?;
    plan.encode(&|| true)?;
    plan.finish(session)
}

/// Run [`render_result`] on a thread named `link-render` and send the result on `tx`.
///
/// The caller keeps the channel receiver. Join the returned handle to take the session back,
/// including the stored epoch. Dropping the handle detaches the thread, so the caller joins
/// or reads `tx`.
pub fn spawn_render(
    mut session: Session,
    link_id: String,
    tx: std::sync::mpsc::Sender<Result<RenderOutcome, RenderError>>,
) -> Result<JoinHandle<Session>, RenderError> {
    std::thread::Builder::new()
        .name("link-render".to_string())
        .spawn(move || {
            let outcome = render_result(&mut session, &link_id);
            let _ = tx.send(outcome);
            session
        })
        .map_err(|e| RenderError::io(format!("could not start link.renderResult: {e}")))
}

/// Paths of the footage `comp` needs (nested comps included) that cannot decode: flagged missing,
/// not on disk, or never probed (no size). Rendering them would produce black frames.
pub fn unusable_footage(session: &Session, comp: ItemId) -> Vec<String> {
    let mut paths = Vec::new();
    for id in essential::dependencies(&session.project, comp) {
        let Some(ItemKind::Footage(footage)) = session.project.item(id).map(|item| &item.kind) else {
            continue;
        };
        let needs_pixels = matches!(footage.kind, FootageKind::Video | FootageKind::Still | FootageKind::Sequence);
        let needs_file = needs_pixels || footage.kind == FootageKind::Model;
        if !needs_file {
            continue;
        }
        let on_disk = |path: &str| !path.is_empty() && session.services.exists(path);
        let present = on_disk(&footage.path) || footage.sequence.iter().any(|frame| on_disk(frame));
        let unprobed = needs_pixels && footage.width == 0 && footage.height == 0;
        if (footage.missing || !present || unprobed) && !paths.contains(&footage.path) {
            paths.push(footage.path.clone());
        }
    }
    paths
}

/// Counter for part file names. Each encode takes the next number.
static PART_SEQ: AtomicU64 = AtomicU64::new(0);

/// How many names [`reserve_part`] tries before it gives up.
const PART_TRIES: u64 = 1000;

fn part_path(result: &Path, seq: u64) -> PathBuf {
    match result.file_name() {
        Some(name) => {
            let mut part = name.to_os_string();
            part.push(format!(".{seq}.part"));
            result.with_file_name(part)
        }
        None => result.with_extension(format!("{seq}.part")),
    }
}

/// Create an empty part file for `result`, starting at number `first`. A name that exists
/// (a file left by a crash, a symlink, another encode) is skipped, never opened, so two encodes
/// of one link never share a part file and no symlink is followed.
fn reserve_part(result: &Path, first: u64) -> Result<PathBuf, RenderError> {
    for seq in first..first.saturating_add(PART_TRIES) {
        let part = part_path(result, seq);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&part) {
            Ok(_) => return Ok(part),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(RenderError::io(format!("could not create {}: {e}", part.display()))),
        }
    }
    Err(RenderError::io(format!("no free part file name next to {}", result.display())))
}

fn discard_temp(temp: &Path) {
    let _ = std::fs::remove_file(temp);
    let log = log_path(&temp.to_string_lossy());
    let _ = std::fs::remove_file(log);
}

/// ProRes 4444 with alpha, full comp length, no audio. `path` is the part file.
fn encode_prores(plan: &RenderPlan, path: &str, keep_going: &dyn Fn() -> bool) -> Result<(), RenderError> {
    let settings = RenderSettings { time_span: TimeSpan::LengthOfComp, ..RenderSettings::default() };
    let output = OutputModule {
        channels: Channels::Rgba,
        prores_profile: ProResProfile::P4444,
        audio: AudioOutput::Off,
        output: path.to_string(),
        ..OutputModule::for_format(OutputFormat::ProRes)
    };
    let job = Job {
        project: &plan.project,
        footage: plan.footage.as_ref(),
        expr: plan.expr.as_deref(),
        accel: plan.accel.as_deref(),
        comp: plan.comp,
        settings: &settings,
        output: &output,
        path,
        sink: None,
        options: JobOptions { label: format!("Craft Link {}", plan.comp.0), ..JobOptions::default() },
        nested_switches: plan.nested_switches,
    };
    match effectcraft_export::export(&job, &mut |_| keep_going()) {
        Ok(_) => Ok(()),
        Err(effectcraft_export::ExportError::Cancelled) => Err(RenderError::cancelled()),
        Err(e) => Err(RenderError::io(format!("could not encode the ProRes result: {e}"))),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::mpsc;

    use effectcraft_engine::Session;
    use effectcraft_engine::color::Label;
    use effectcraft_project::{Comp, CraftLinkBinding, ItemId, ItemKind};
    use effectcraft_time::FrameRate;
    use serde_json::json;

    use super::*;

    const LINK: &str = "33333333-3333-4333-8333-333333333333";

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ec-render-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // `link.open` writes this sidecar. The render accepts only a directory that holds it.
        std::fs::write(dir.join(format!("{LINK}.ecproj")), b"{}").unwrap();
        dir
    }

    fn add_comp(session: &mut Session) -> ItemId {
        let rate = FrameRate::new(24, 1);
        let mut comp = Comp::new(128, 72, rate, rate.tick_of(1));
        comp.background = [0.1, 0.2, 0.8];
        session.project_mut().add_item("Link Comp", Label::Sandstone, None, ItemKind::Comp(std::sync::Arc::new(comp)))
    }

    fn bind(session: &mut Session, link_id: &str, comp: ItemId, result_path: &str) {
        session.project_mut().craft_links.insert(
            link_id.to_string(),
            CraftLinkBinding {
                link_id: link_id.to_string(),
                comp,
                source: json!({"app": "filmcraft", "doc_id": "0", "item": "1"}),
                result_path: result_path.to_string(),
                epoch: 0,
            },
        );
    }

    /// A project file names a path outside the link directory. Each case holds a victim file and
    /// expects `result_path_refused`, with the victim untouched and no directory made.
    #[test]
    fn a_project_cannot_aim_the_render_at_a_file_it_did_not_make() {
        let dir = scratch("refused");
        let bare = scratch("refused-bare");
        std::fs::remove_file(bare.join(format!("{LINK}.ecproj"))).unwrap();
        let victim = dir.join("authorized_keys");
        std::fs::write(&victim, b"ssh-ed25519 KEEP").unwrap();
        let lookalike = dir.join(format!("{LINK}.mov"));
        std::fs::write(&lookalike, b"not a movie").unwrap();
        let other = dir.join("other.mov");
        let cases: Vec<(&str, PathBuf)> = vec![
            ("foreign name", victim.clone()),
            ("right name, foreign content", lookalike.clone()),
            ("right name, directory has no sidecar", bare.join(format!("{LINK}.mov"))),
            ("wrong extension", dir.join(format!("{LINK}.txt"))),
            ("not absolute", PathBuf::from(format!("{LINK}.mov"))),
            ("made directory", dir.join("new/deeper").join(format!("{LINK}.mov"))),
            ("other name with sidecar", other.clone()),
        ];
        for (what, path) in cases {
            let mut session = Session::default();
            let comp = add_comp(&mut session);
            bind(&mut session, LINK, comp, &path.to_string_lossy());
            let err = render_result(&mut session, LINK).unwrap_err();
            assert_eq!(err.code, "result_path_refused", "{what}: {err}");
            assert_eq!(std::fs::read(&victim).unwrap(), b"ssh-ed25519 KEEP", "{what}");
            assert_eq!(std::fs::read(&lookalike).unwrap(), b"not a movie", "{what}");
            assert!(!other.exists() && !dir.join("new").exists() && !bare.join(format!("{LINK}.mov")).exists(), "{what}");
            assert_eq!(session.project.craft_links[LINK].epoch, 0, "{what}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_at_the_result_name_is_refused() {
        let dir = scratch("result-symlink");
        let victim = dir.join("victim");
        std::fs::write(&victim, b"KEEP").unwrap();
        let result = dir.join(format!("{LINK}.mov"));
        std::os::unix::fs::symlink(&victim, &result).unwrap();
        let mut session = Session::default();
        let comp = add_comp(&mut session);
        bind(&mut session, LINK, comp, &result.to_string_lossy());
        let err = render_result(&mut session, LINK).unwrap_err();
        assert_eq!(err.code, "result_path_refused");
        assert_eq!(std::fs::read(&victim).unwrap(), b"KEEP");
    }

    /// A part name that already exists is skipped, never opened: a file left by another encode
    /// is not shared.
    #[test]
    fn a_part_name_in_use_is_skipped() {
        let dir = scratch("part-reserve");
        let result = dir.join(format!("{LINK}.mov"));
        std::fs::write(part_path(&result, 5), b"other encode").unwrap();
        let part = reserve_part(&result, 5).unwrap();
        assert_eq!(part, part_path(&result, 6));
        assert_eq!(std::fs::read(part_path(&result, 5)).unwrap(), b"other encode");
        assert_eq!(reserve_part(&result, 5).unwrap(), part_path(&result, 7));
    }

    /// A symlink at a part name is not followed.
    #[cfg(unix)]
    #[test]
    fn a_symlink_at_a_part_name_is_not_followed() {
        let dir = scratch("part-symlink");
        let victim = dir.join("victim");
        std::fs::write(&victim, b"KEEP").unwrap();
        let result = dir.join(format!("{LINK}.mov"));
        std::os::unix::fs::symlink(&victim, part_path(&result, 5)).unwrap();
        assert_eq!(reserve_part(&result, 5).unwrap(), part_path(&result, 6));
        assert_eq!(std::fs::read(&victim).unwrap(), b"KEEP");
    }

    /// Two renders of one link overlap: the user's edit starts a background render and Back
    /// starts another. Each writes its own part file, and both finish with a movie.
    #[test]
    fn two_overlapping_renders_of_one_link_write_separate_part_files() {
        let dir = scratch("overlap");
        let result = dir.join(format!("{LINK}.mov"));
        let mut session = Session::default();
        let comp = add_comp(&mut session);
        bind(&mut session, LINK, comp, &result.to_string_lossy());
        let slow = prepare_render(&session, LINK, Some(1)).unwrap();
        let quick = prepare_render(&session, LINK, Some(2)).unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let worker = std::thread::spawn(move || {
            let first = std::cell::Cell::new(true);
            slow.encode(&|| {
                if first.replace(false) {
                    let _ = started_tx.send(());
                    let _ = release_rx.recv_timeout(std::time::Duration::from_secs(30));
                }
                true
            })
        });
        started_rx.recv_timeout(std::time::Duration::from_secs(30)).unwrap();
        assert_eq!(crate::test_support::result_parts(&dir).len(), 1, "the slow encode holds one part file");
        quick.encode(&|| true).unwrap();
        assert!(effectcraft_media::probe(&result).is_ok(), "the quick render left a movie");
        assert_eq!(crate::test_support::result_parts(&dir).len(), 1, "the quick render did not touch the slow part file");
        release_tx.send(()).unwrap();
        worker.join().unwrap().unwrap();
        assert!(effectcraft_media::probe(&result).is_ok());
        assert!(crate::test_support::result_parts(&dir).is_empty());
    }

    #[test]
    fn a_result_made_by_the_link_is_replaced_and_a_refusal_has_a_code() {
        let dir = scratch("replaced");
        let result = dir.join(format!("{LINK}.mov"));
        let mut session = Session::default();
        let comp = add_comp(&mut session);
        bind(&mut session, LINK, comp, &result.to_string_lossy());
        render_result(&mut session, LINK).unwrap();
        render_result(&mut session, LINK).unwrap();
        assert_eq!(session.project.craft_links[LINK].epoch, 2);
        assert_eq!(RenderError::refused("x").reply()["error"]["code"], "result_path_refused");
    }

    #[test]
    fn render_result_writes_prores_4444_and_bumps_epoch() {
        let dir = scratch("prores");
        let result = dir.join(format!("{LINK}.mov"));
        let result_path = result.to_string_lossy().to_string();
        let mut session = Session::default();
        let comp = add_comp(&mut session);
        bind(&mut session, LINK, comp, &result_path);

        let first = render_result(&mut session, LINK).unwrap();
        let bytes = std::fs::read(&result).unwrap();
        println!("RESULT_BYTES:{}", bytes.len());
        assert!(crate::test_support::result_parts(&dir).is_empty(), "a part file remains");
        assert_eq!(first.epoch, 1);
        assert_eq!(first.path, result_path);
        assert_eq!(first.link_id, LINK);
        assert_eq!(session.project.craft_links[LINK].epoch, 1);
        let probed = effectcraft_media::probe(&result).unwrap();
        println!("RESULT_CODEC:{} {}x{}", probed.codec, probed.width, probed.height);
        // A flat 128×72 frame encodes to a short ProRes 4444 file. The in-repo probe is the check.
        assert!(bytes.len() > 4096 || probed.codec.contains("ProRes"), "RESULT_BYTES:{} codec:{}", bytes.len(), probed.codec);
        assert!(probed.codec.contains("ProRes 4444"), "{}", probed.codec);
        assert_eq!((probed.width, probed.height), (128, 72));
        let frame = effectcraft_media::MediaPool::new().frame_at(&probed, probed.frame_rate.tick_of(0)).unwrap();
        assert_eq!((frame.width, frame.height), (128, 72));

        let (tx, rx) = mpsc::channel();
        let handle = spawn_render(session, LINK.to_string(), tx).unwrap();
        let second = rx.recv().unwrap().unwrap();
        let session = handle.join().unwrap();
        let bytes = std::fs::metadata(&result).unwrap().len();
        println!("RESULT_BYTES_SECOND:{bytes}");
        assert!(crate::test_support::result_parts(&dir).is_empty());
        assert_eq!(second.epoch, 2);
        assert_eq!(second.path, result_path);
        assert_eq!(session.project.craft_links[LINK].epoch, 2);
        assert!(bytes > 64, "RESULT_BYTES_SECOND:{bytes}");
    }

    #[test]
    fn render_result_unknown_id_writes_nothing() {
        let dir = scratch("unknown");
        let result = dir.join("absent.mov");
        let mut session = Session::default();
        let err = render_result(&mut session, "no-such-link").unwrap_err();
        let reply = err.reply();
        assert_eq!(err.code, "comp_missing");
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["error"]["code"], "comp_missing");
        assert!(reply["error"]["data"].as_object().unwrap().is_empty());
        assert!(!result.exists());
        assert!(crate::test_support::result_parts(&dir).is_empty());
        assert!(session.project.craft_links.is_empty());
    }

    #[test]
    fn render_result_missing_comp_leaves_no_part() {
        let dir = scratch("missing");
        let result = dir.join(format!("{LINK}.mov"));
        let mut session = Session::default();
        bind(&mut session, LINK, ItemId(9), &result.to_string_lossy());
        let err = render_result(&mut session, LINK).unwrap_err();
        assert_eq!(err.code, "comp_missing");
        assert!(!result.exists());
        assert!(crate::test_support::result_parts(&dir).is_empty());
        assert_eq!(session.project.craft_links[LINK].epoch, 0);
    }

    /// A comp with one footage layer whose item is `footage`.
    fn add_footage_comp(session: &mut Session, footage: effectcraft_project::Footage) -> ItemId {
        use effectcraft_project::LayerSource;
        let rate = FrameRate::new(24, 1);
        let comp_id = add_comp(session);
        let item = session.project_mut().add_item("clip", Label::Aqua, None, ItemKind::Footage(footage));
        let project = session.project_mut();
        let template = project.comp(comp_id).unwrap().clone();
        let layer = effectcraft_project::build::layer(project, &template, "clip", LayerSource::Footage { item }, (128, 72), None);
        let mut comp = template;
        comp.duration = rate.tick_of(24);
        comp.layers.push(layer);
        project.item_mut(comp_id).unwrap().kind = ItemKind::Comp(std::sync::Arc::new(comp));
        comp_id
    }

    fn footage_at(path: &str, missing: bool) -> effectcraft_project::Footage {
        effectcraft_project::Footage { path: path.to_string(), width: 128, height: 72, has_video: true, missing, ..effectcraft_project::Footage::default() }
    }

    #[test]
    fn render_result_with_missing_footage_is_an_error_not_a_black_movie() {
        let dir = scratch("nofootage");
        let result = dir.join(format!("{LINK}.mov"));
        let mut session = Session::default();
        let comp = add_footage_comp(&mut session, footage_at("/nonexistent/clip.mov", true));
        bind(&mut session, LINK, comp, &result.to_string_lossy());
        let err = render_result(&mut session, LINK).unwrap_err();
        assert_eq!(err.code, "footage_missing");
        assert!(err.message.contains("/nonexistent/clip.mov"), "{}", err.message);
        assert!(!result.exists());
        assert!(crate::test_support::result_parts(&dir).is_empty());
        assert_eq!(session.project.craft_links[LINK].epoch, 0);
    }

    #[test]
    fn render_result_with_unprobed_footage_is_an_error() {
        // Not flagged missing, but never probed (no size): the renderer would draw nothing.
        let dir = scratch("unprobed");
        let result = dir.join(format!("{LINK}.mov"));
        let file = dir.join("exists.mov");
        std::fs::write(&file, b"x").unwrap();
        let mut session = Session::default();
        let mut footage = footage_at(&file.to_string_lossy(), false);
        footage.width = 0;
        footage.height = 0;
        let comp = add_footage_comp(&mut session, footage);
        bind(&mut session, LINK, comp, &result.to_string_lossy());
        let err = render_result(&mut session, LINK).unwrap_err();
        assert_eq!(err.code, "footage_missing");
        assert!(!result.exists());
    }

    #[test]
    fn render_result_with_footage_that_vanished_from_disk_is_an_error() {
        let dir = scratch("vanished");
        let result = dir.join(format!("{LINK}.mov"));
        let mut session = Session::default();
        let comp = add_footage_comp(&mut session, footage_at(&dir.join(format!("{LINK}.mov")).to_string_lossy(), false));
        bind(&mut session, LINK, comp, &result.to_string_lossy());
        let err = render_result(&mut session, LINK).unwrap_err();
        assert_eq!(err.code, "footage_missing");
        assert!(!result.exists());
    }

    #[test]
    fn a_cancelled_encode_leaves_no_file_and_no_part() {
        let dir = scratch("cancel");
        let result = dir.join(format!("{LINK}.mov"));
        let mut session = Session::default();
        let comp = add_comp(&mut session);
        bind(&mut session, LINK, comp, &result.to_string_lossy());
        let plan = prepare_render(&session, LINK, Some(7)).unwrap();
        assert_eq!(plan.epoch, 7);
        let err = plan.encode(&|| false).unwrap_err();
        assert_eq!(err.code, "cancelled");
        assert!(!result.exists());
        assert!(crate::test_support::result_parts(&dir).is_empty());
        assert_eq!(session.project.craft_links[LINK].epoch, 0, "nothing is stored for a render that did not finish");
        // A finished plan stores its epoch once, and the epoch never goes down.
        let plan = prepare_render(&session, LINK, Some(7)).unwrap();
        plan.encode(&|| true).unwrap();
        plan.finish(&mut session).unwrap();
        assert_eq!(session.project.craft_links[LINK].epoch, 7);
        let older = prepare_render(&session, LINK, Some(3)).unwrap();
        older.encode(&|| true).unwrap();
        older.finish(&mut session).unwrap();
        assert_eq!(session.project.craft_links[LINK].epoch, 7);
    }
}
