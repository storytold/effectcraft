//! The live link of EffectCraft: one object ties the frame server, the change tracker and the
//! background result render to the UI thread.
//!
//! The app calls [`install`] once, before the link server starts, and gives the returned
//! [`CompFrames`] to the server. The UI thread then calls [`tick`] once per frame. A tick does
//! all of this and nothing blocks:
//!
//! 1. Drain the document change feed. Any change except the one a result render stores
//!    ([`RESULT_CAUSE`]) bumps the epoch of every bound link and emits `link.changed`.
//! 2. Publish the project, with the epochs, to the frame workers.
//! 3. Take in a finished result render: store its epoch, emit `link.result`.
//! 4. After [`SETTLE`] without changes, start a result render on a thread. A new change cancels
//!    a render that runs.
//!
//! [`tick`] returns when it wants to be called again, so a UI that sleeps wakes in time.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use craft_command::{Cause, ChangeBus};
use effectcraft_engine::Session;
use effectcraft_project::Project;
use serde_json::{Value, json};

use crate::frames::{CompFrames, FrameWorld};
use crate::result::{RESULT_CAUSE, RenderError, RenderOutcome, RenderPlan, prepare_render};
use crate::sidecar::{self, SidecarError};
use crate::track::Tracker;

/// Sends a push event to the peers that subscribed (`Server::emit`).
pub type Emitter = Arc<dyn Fn(&str, Value) + Send + Sync>;
/// Asks the UI for another frame (`egui::Context::request_repaint`). Called from the render thread.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

struct Finished {
    link_id: String,
    epoch: u64,
    outcome: Result<RenderPlan, RenderError>,
}

struct Running {
    link_id: String,
    cancel: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

/// Where a window close stands. See [`Live::begin_close`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Closing {
    /// Every sidecar is written and no result render is owed. The window may close.
    Ready,
    /// A result render runs or waits. Keep the window open and ask again next frame.
    Rendering,
}

/// How long [`Live::abort_close`] waits for a cancelled render to remove its part file.
const ABORT_WAIT: Duration = Duration::from_secs(1);

#[derive(Default)]
struct Drained {
    any: bool,
    edits: bool,
}

struct State {
    tracker: Tracker,
    sub: Option<(Arc<ChangeBus<Arc<Project>>>, u64)>,
    running: Option<Running>,
    published: Option<(usize, Vec<(String, u64)>)>,
    /// Links this object put in [`EPOCHS`].
    known: Vec<String>,
    /// When the sidecars are due: [`sidecar::SETTLE`] after the last change.
    sidecar_at: Option<Instant>,
    /// Background sidecar writes still running.
    writers: Vec<JoinHandle<()>>,
    /// Failures of background sidecar writes, until the UI thread reports them.
    sidecar_errors: Arc<Mutex<Vec<String>>>,
    /// The last failure that was reported, so a failing disk is not toasted twice a second.
    reported_error: Option<String>,
    /// The window is closing: render without the quiet period.
    closing: bool,
    tx: Sender<Finished>,
    rx: Receiver<Finished>,
}

/// See the module docs.
pub struct Live {
    frames: Arc<CompFrames>,
    emit: Emitter,
    wake: Waker,
    state: Mutex<State>,
}

/// Link id to the epoch the tracker holds, for [`epoch_of`].
static EPOCHS: Mutex<BTreeMap<String, u64>> = Mutex::new(BTreeMap::new());
static LIVE: Mutex<Option<Arc<Live>>> = Mutex::new(None);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The epoch the live link holds for `link_id`. 0 when it knows nothing of the link.
pub fn epoch_of(link_id: &str) -> u64 {
    lock(&EPOCHS).get(link_id).copied().unwrap_or(0)
}

/// Make the process live link. A second call replaces the first. Returns the frame source for
/// `craft_ipc::bulk::FrameTickets`.
pub fn install(emit: Emitter, wake: Waker) -> Arc<CompFrames> {
    let live = Arc::new(Live::new(emit, wake));
    let frames = Arc::clone(&live.frames);
    *lock(&LIVE) = Some(live);
    frames
}

/// [`Live::tick`] on the process live link. Does nothing before [`install`].
pub fn tick(session: &mut Session) -> Option<Duration> {
    let live = lock(&LIVE).clone()?;
    live.tick(session, Instant::now())
}

/// Write the sidecar of every bound link now, on this thread. Without a live link (tests, the web
/// build) the sidecars are still written. Errors are logged and returned.
pub fn flush_sidecars(session: &Session) -> Vec<SidecarError> {
    let live = lock(&LIVE).clone();
    match live {
        Some(live) => live.flush_sidecars(session),
        None => report(sidecar::write_all(&session.project, sidecar::ticket())),
    }
}

/// The window was asked to close. See [`Live::begin_close`]. Without a live link nothing can
/// render, so the sidecars are written and the answer is [`Closing::Ready`].
pub fn begin_close(session: &mut Session) -> Closing {
    let live = lock(&LIVE).clone();
    match live {
        Some(live) => live.begin_close(session, Instant::now()),
        None => {
            flush_sidecars(session);
            Closing::Ready
        }
    }
}

/// Ask again after [`begin_close`] answered [`Closing::Rendering`]: tick the live link, then say
/// whether the render is done.
pub fn close_state(session: &mut Session) -> Closing {
    let live = lock(&LIVE).clone();
    match live {
        Some(live) => {
            live.tick(session, Instant::now());
            live.close_state()
        }
        None => Closing::Ready,
    }
}

/// The user asked to close a second time: cancel the render and write the sidecars.
pub fn abort_close(session: &mut Session) {
    let live = lock(&LIVE).clone();
    match live {
        Some(live) => live.abort_close(session),
        None => {
            flush_sidecars(session);
        }
    }
}

fn report(errors: Vec<SidecarError>) -> Vec<SidecarError> {
    for err in &errors {
        log::warn!("the link project was not saved: {err}");
    }
    errors
}

/// Tell subscribed peers this app is about to exit.
pub fn closing() {
    let live = lock(&LIVE).clone();
    if let Some(live) = live {
        (live.emit)("app.closing", json!({}));
    }
}

impl Live {
    pub fn new(emit: Emitter, wake: Waker) -> Self {
        let (tx, rx) = channel();
        Self {
            frames: Arc::new(CompFrames::new()),
            emit,
            wake,
            state: Mutex::new(State {
                tracker: Tracker::new(),
                sub: None,
                running: None,
                published: None,
                known: Vec::new(),
                sidecar_at: None,
                writers: Vec::new(),
                sidecar_errors: Arc::new(Mutex::new(Vec::new())),
                reported_error: None,
                closing: false,
                tx,
                rx,
            }),
        }
    }

    pub fn frames(&self) -> Arc<CompFrames> {
        Arc::clone(&self.frames)
    }

    /// One pass of the module's steps at `now`. Returns the delay until the next pass is useful.
    pub fn tick(&self, session: &mut Session, now: Instant) -> Option<Duration> {
        let mut state = lock(&self.state);
        let bound: Vec<(String, u64)> = session.project.craft_links.iter().map(|(link_id, binding)| (link_id.clone(), binding.epoch)).collect();
        if bound.is_empty() && state.tracker.is_empty() && state.running.is_none() {
            self.unsubscribe(session, &mut state);
            return None;
        }
        self.subscribe(session, &mut state);
        let drained = self.drain_changes(&mut state);
        let added = state.tracker.sync(bound.iter().map(|(link_id, epoch)| (link_id.as_str(), *epoch)));
        self.forget_unbound(&mut state);
        // A link that was just bound is not an edit: the change that bound it came with it.
        if drained.edits && !added {
            self.edited(&mut state, now);
        }
        // Any change, a stored result epoch included, makes the sidecar older than the project.
        if drained.any && !added {
            state.sidecar_at = Some(now + sidecar::SETTLE);
        }
        self.take_finished(session, &mut state, now);
        self.start_due(session, &mut state, now);
        self.write_due_sidecar(session, &mut state, now);
        self.report_sidecar_errors(session, &mut state);
        self.publish(session, &mut state);
        let busy = state.closing && state.running.is_some();
        let next = [state.tracker.wake_at(), state.sidecar_at, busy.then(|| now + Duration::from_millis(50))].into_iter().flatten().min();
        next.map(|at| at.saturating_duration_since(now).max(Duration::from_millis(1)))
    }

    /// Write the sidecar of every bound link now, on this thread. Waits for no background write:
    /// the write order (see [`sidecar`]) makes this one the last word.
    pub fn flush_sidecars(&self, session: &Session) -> Vec<SidecarError> {
        lock(&self.state).sidecar_at = None;
        report(sidecar::write_all(&session.project, sidecar::ticket()))
    }

    /// Wait for the background sidecar writes that run now.
    pub fn finish_sidecar_writes(&self) {
        let writers = std::mem::take(&mut lock(&self.state).writers);
        for writer in writers {
            let _ = writer.join();
        }
    }

    /// The window was asked to close. Write the sidecars first, so the work is safe whatever
    /// happens next. Then, for every link whose result file is missing or older than its edits,
    /// start the result render without the quiet period. [`Closing::Rendering`] means the window
    /// stays open: tick and ask [`Self::close_state`] until it says [`Closing::Ready`].
    pub fn begin_close(&self, session: &mut Session, now: Instant) -> Closing {
        self.flush_sidecars(session);
        self.tick(session, now);
        {
            let mut state = lock(&self.state);
            state.closing = true;
            let missing: Vec<String> = session
                .project
                .craft_links
                .iter()
                .filter(|(_, binding)| !std::fs::symlink_metadata(&binding.result_path).is_ok_and(|meta| meta.is_file()))
                .map(|(link_id, _)| link_id.clone())
                .collect();
            for link_id in missing {
                if let Some((link_id, epoch)) = state.tracker.needs_render(&link_id) {
                    (self.emit)("link.changed", json!({"link_id": link_id, "epoch": epoch}));
                    lock(&EPOCHS).insert(link_id, epoch);
                }
            }
        }
        self.tick(session, now);
        self.close_state()
    }

    /// Whether a close that [`Self::begin_close`] started can finish. Answering
    /// [`Closing::Ready`] ends the close mode, so a user who cancels the close can go on editing.
    pub fn close_state(&self) -> Closing {
        let mut state = lock(&self.state);
        if state.running.is_some() || state.tracker.has_stale() {
            return Closing::Rendering;
        }
        state.closing = false;
        Closing::Ready
    }

    /// Give up the result render of a close. The render is cancelled and its part file removed,
    /// and the sidecars are written. The result file stays as it was.
    pub fn abort_close(&self, session: &Session) {
        let handle = {
            let mut state = lock(&self.state);
            state.closing = false;
            state.tracker.clear_stale();
            state.running.take().and_then(|running| {
                running.cancel.store(true, Ordering::SeqCst);
                running.handle
            })
        };
        if let Some(handle) = handle {
            let deadline = Instant::now() + ABORT_WAIT;
            while !handle.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        self.flush_sidecars(session);
    }

    fn write_due_sidecar(&self, session: &Session, state: &mut State, now: Instant) {
        if state.sidecar_at.is_none_or(|at| now < at) {
            return;
        }
        state.sidecar_at = None;
        let (project, ticket) = (Arc::clone(&session.project), sidecar::ticket());
        let (errors, wake) = (Arc::clone(&state.sidecar_errors), Arc::clone(&self.wake));
        let spawned = std::thread::Builder::new().name("link-sidecar".into()).spawn(move || {
            let failed = sidecar::write_all(&project, ticket);
            if !failed.is_empty() {
                lock(&errors).extend(failed.iter().map(ToString::to_string));
                wake();
            }
        });
        match spawned {
            Ok(handle) => {
                state.writers.retain(|writer| !writer.is_finished());
                state.writers.push(handle);
            }
            Err(err) => {
                log::warn!("could not start the link project write: {err}");
                lock(&state.sidecar_errors).extend(report(sidecar::write_all(&session.project, ticket)).iter().map(ToString::to_string));
            }
        }
    }

    /// A sidecar that cannot be written means a reopen would lose work. Say so, once per message.
    fn report_sidecar_errors(&self, session: &mut Session, state: &mut State) {
        let errors = std::mem::take(&mut *lock(&state.sidecar_errors));
        for message in errors {
            log::warn!("the link project was not saved: {message}");
            if state.reported_error.as_deref() != Some(message.as_str()) {
                session
                    .events
                    .push(effectcraft_engine::Event::Toast { message: format!("Craft Link could not save the link project: {message}"), error: true });
                state.reported_error = Some(message);
            }
        }
    }

    fn subscribe(&self, session: &Session, state: &mut State) {
        if state.sub.is_none() {
            let bus = session.hooks.change_bus::<Arc<Project>>();
            let id = bus.subscribe();
            state.sub = Some((bus, id));
        }
    }

    fn unsubscribe(&self, _session: &Session, state: &mut State) {
        if let Some((bus, id)) = state.sub.take() {
            bus.unsubscribe(id);
        }
        self.forget_unbound(state);
        state.published = None;
        self.frames.clear();
    }

    /// Keep [`EPOCHS`] to the links the tracker holds.
    fn forget_unbound(&self, state: &mut State) {
        let mut epochs = lock(&EPOCHS);
        for (link_id, epoch) in state.tracker.epochs() {
            epochs.entry(link_id.to_string()).or_insert(epoch);
        }
        let held: Vec<String> = state.tracker.epochs().map(|(link_id, _)| link_id.to_string()).collect();
        for gone in state.known.iter().filter(|link_id| !held.contains(link_id)) {
            epochs.remove(gone);
        }
        state.known = held;
    }

    /// What the change feed held: any change, and whether one of them is an edit (not a stored result epoch).
    fn drain_changes(&self, state: &mut State) -> Drained {
        let Some((bus, id)) = &state.sub else { return Drained::default() };
        let changes = bus.drain(*id);
        Drained { any: !changes.is_empty(), edits: changes.iter().any(|change| change.cause != Cause::Job(RESULT_CAUSE)) }
    }

    fn edited(&self, state: &mut State, now: Instant) {
        for (link_id, epoch) in state.tracker.changed(now) {
            (self.emit)("link.changed", json!({"link_id": link_id, "epoch": epoch}));
        }
        if let Some(running) = &state.running {
            running.cancel.store(true, Ordering::SeqCst);
        }
        let mut epochs = lock(&EPOCHS);
        for (link_id, epoch) in state.tracker.epochs() {
            epochs.insert(link_id.to_string(), epoch);
        }
    }

    fn take_finished(&self, session: &mut Session, state: &mut State, now: Instant) {
        while let Ok(done) = state.rx.try_recv() {
            if state.running.as_ref().is_some_and(|running| running.link_id == done.link_id) {
                state.running = None;
            }
            match done.outcome {
                Ok(plan) => match plan.finish(session) {
                    Ok(RenderOutcome { link_id, epoch, path }) => {
                        state.tracker.render_ended(&link_id, true);
                        // The binding stores the new epoch, so the sidecar is older than the project.
                        state.sidecar_at = Some(now + sidecar::SETTLE);
                        (self.emit)("link.result", json!({"link_id": link_id, "epoch": epoch, "path": path}));
                    }
                    Err(err) => log::warn!("link {}: could not store the result epoch: {err}", done.link_id),
                },
                Err(err) if err.code == "cancelled" => state.tracker.render_ended(&done.link_id, false),
                Err(err) => {
                    // Not retried until the comp changes again: the same render would fail again.
                    log::warn!("link {}: the result at epoch {} was not rendered: {err}", done.link_id, done.epoch);
                    state.tracker.render_ended(&done.link_id, true);
                }
            }
        }
    }

    fn start_due(&self, session: &mut Session, state: &mut State, now: Instant) {
        if state.running.is_some() {
            return;
        }
        let mut due = if state.closing { state.tracker.due_now() } else { state.tracker.due(now) }.into_iter();
        let Some((link_id, epoch)) = due.next() else { return };
        // One render at a time. The others wait for the next quiet period.
        for (other, _) in due {
            state.tracker.render_ended(&other, false);
        }
        let plan = match prepare_render(session, &link_id, Some(epoch)) {
            Ok(plan) => plan,
            Err(err) => {
                log::warn!("link {link_id}: the result at epoch {epoch} was not rendered: {err}");
                if err.code == "result_path_refused" {
                    // A saved project chose this path. Say so: the user did not ask for the write.
                    session.events.push(effectcraft_engine::Event::Toast {
                        message: format!("Craft Link did not write the result for {link_id}: {}", err.message),
                        error: true,
                    });
                    (self.emit)("link.refused", json!({"link_id": link_id, "code": err.code, "message": err.message}));
                }
                state.tracker.render_ended(&link_id, true);
                return;
            }
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, wake, flag) = (state.tx.clone(), Arc::clone(&self.wake), Arc::clone(&cancel));
        let name = link_id.clone();
        let spawned = std::thread::Builder::new().name("link-render".into()).spawn(move || {
            let outcome = plan.encode(&|| !flag.load(Ordering::SeqCst)).map(|()| plan);
            let _ = tx.send(Finished { link_id: name, epoch, outcome });
            wake();
        });
        match spawned {
            Ok(handle) => state.running = Some(Running { link_id, cancel, handle: Some(handle) }),
            Err(err) => {
                log::warn!("link {link_id}: could not start the result render: {err}");
                state.tracker.render_ended(&link_id, false);
            }
        }
    }

    fn publish(&self, session: &Session, state: &mut State) {
        let epochs: Vec<(String, u64)> = state.tracker.epochs().map(|(link_id, epoch)| (link_id.to_string(), epoch)).collect();
        let key = (Arc::as_ptr(&session.project) as usize, epochs.clone());
        if state.published.as_ref() == Some(&key) {
            return;
        }
        self.frames.publish(FrameWorld::of(session, &epochs.into_iter().collect()));
        state.published = Some(key);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::Arc;

    use craft_ipc::bulk::{FrameFormat, FrameRequest, FrameSource, FrameSpec};
    use effectcraft_engine::color::Label;
    use effectcraft_project::{Comp, CraftLinkBinding, ItemId, ItemKind};
    use effectcraft_time::FrameRate;

    use super::*;

    type Events = Arc<Mutex<Vec<(String, Value)>>>;

    fn live() -> (Live, Events) {
        let events: Events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        let emit: Emitter = Arc::new(move |name, data| lock(&sink).push((name.to_string(), data)));
        (Live::new(emit, Arc::new(|| {})), events)
    }

    fn names(events: &Events) -> Vec<String> {
        lock(events).iter().map(|(name, _)| name.clone()).collect()
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ec-live-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn session_with_link(link: &str, result: &std::path::Path) -> Session {
        // `link.open` writes this sidecar. The render accepts only a directory that holds it.
        if let Some(dir) = result.parent() {
            std::fs::write(dir.join(format!("{link}.ecproj")), b"{}").unwrap();
        }
        let mut session = Session::default();
        let rate = FrameRate::new(24, 1);
        let mut comp = Comp::new(64, 36, rate, rate.tick_of(2));
        comp.background = [0.2, 0.4, 0.6];
        let comp = session.project_mut().add_item("Link Comp", Label::Sandstone, None, ItemKind::Comp(Arc::new(comp)));
        session.project_mut().craft_links.insert(
            link.to_string(),
            CraftLinkBinding {
                link_id: link.to_string(),
                comp,
                source: json!({"app": "filmcraft", "doc_id": "0", "item": "1"}),
                result_path: result.to_string_lossy().to_string(),
                epoch: 0,
            },
        );
        let _ = ItemId(0);
        session
    }

    fn edit(session: &mut Session) {
        session.bump();
    }

    #[test]
    fn a_bound_link_is_published_and_the_bind_itself_is_not_an_edit() {
        const LINK: &str = "1111-4444-4444-8444-444444444444";
        let dir = scratch("bind");
        let mut session = session_with_link(LINK, &dir.join(format!("{LINK}.mov")));
        let (live, events) = live();
        let now = Instant::now();
        edit(&mut session);
        assert_eq!(live.tick(&mut session, now), None);
        assert!(names(&events).is_empty(), "the change that came with the bind is not an edit");
        let spec = FrameSpec { link_id: LINK.into(), format: FrameFormat::Rgba8Premul, scale: 1.0 };
        let layout = live.frames().open(&spec).unwrap();
        assert_eq!((layout.w, layout.h), (64, 36));
    }

    #[test]
    fn an_edit_bumps_the_epoch_emits_link_changed_and_reaches_the_frame_workers() {
        const LINK: &str = "2222-4444-4444-8444-444444444444";
        let dir = scratch("edit");
        let mut session = session_with_link(LINK, &dir.join(format!("{LINK}.mov")));
        let (live, events) = live();
        let now = Instant::now();
        live.tick(&mut session, now);
        edit(&mut session);
        let wake = live.tick(&mut session, now + Duration::from_millis(10));
        assert_eq!(names(&events), vec!["link.changed"]);
        assert_eq!(lock(&events)[0].1, json!({"link_id": LINK, "epoch": 1}));
        assert_eq!(epoch_of(LINK), 1);
        assert!(wake.is_some_and(|d| d <= Duration::from_millis(500)), "the UI must come back for the result render: {wake:?}");
        let spec = FrameSpec { link_id: LINK.into(), format: FrameFormat::Rgba8Premul, scale: 1.0 };
        let frame = live.frames().render(&spec, &FrameRequest { req: 1, time_s: 0.0, epoch: 0, region: None }).unwrap();
        assert_eq!(frame.epoch, 1, "the frame workers see the new epoch");
    }

    #[test]
    fn the_result_renders_after_the_quiet_period_and_its_own_bump_is_not_an_edit() {
        const LINK: &str = "3333-4444-4444-8444-444444444444";
        let dir = scratch("result");
        let result = dir.join(format!("{LINK}.mov"));
        let mut session = session_with_link(LINK, &result);
        let (live, events) = live();
        let t0 = Instant::now();
        live.tick(&mut session, t0);
        edit(&mut session);
        live.tick(&mut session, t0);
        live.tick(&mut session, t0 + Duration::from_millis(400));
        assert!(!result.exists(), "not before the quiet period ends");
        let mut now = t0 + Duration::from_millis(600);
        let deadline = Instant::now() + Duration::from_secs(60);
        while !names(&events).contains(&"link.result".to_string()) {
            live.tick(&mut session, now);
            assert!(Instant::now() < deadline, "the result never arrived: {:?}", names(&events));
            std::thread::sleep(Duration::from_millis(20));
            now += Duration::from_millis(20);
        }
        let result_event = lock(&events).iter().find(|(name, _)| name == "link.result").cloned().unwrap().1;
        assert_eq!(result_event["epoch"], json!(1));
        assert_eq!(result_event["path"], json!(result.to_string_lossy()));
        assert!(result.exists());
        assert_eq!(session.project.craft_links[LINK].epoch, 1);
        for _ in 0..5 {
            now += Duration::from_secs(1);
            live.tick(&mut session, now);
        }
        assert_eq!(names(&events), vec!["link.changed", "link.result"], "storing the epoch is not an edit, so no second round");
    }

    fn sidecar_epoch(dir: &std::path::Path, link: &str) -> u64 {
        let text = std::fs::read_to_string(dir.join(format!("{link}.ecproj"))).unwrap();
        effectcraft_project::Project::from_json(&text).unwrap().craft_links[link].epoch
    }

    fn drive_close(live: &Live, session: &mut Session, mut now: Instant) -> Closing {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            live.tick(session, now);
            if live.close_state() == Closing::Ready {
                return Closing::Ready;
            }
            assert!(Instant::now() < deadline, "the close render never finished");
            std::thread::sleep(Duration::from_millis(20));
            now += Duration::from_millis(20);
        }
    }

    /// Symptom 1: the user opens the link and closes EffectCraft at once. No result was ever
    /// rendered, so FilmCraft would have no movie. The close must render it first.
    #[test]
    fn closing_before_any_result_renders_it_and_stores_the_epoch_in_the_sidecar() {
        const LINK: &str = "5555-4444-4444-8444-444444444444";
        let dir = scratch("close-fresh");
        let result = dir.join(format!("{LINK}.mov"));
        let mut session = session_with_link(LINK, &result);
        let (live, events) = live();
        let t0 = Instant::now();
        live.tick(&mut session, t0);
        assert!(!result.exists());

        assert_eq!(live.begin_close(&mut session, t0), Closing::Rendering, "a missing result must hold the window open");
        assert_eq!(drive_close(&live, &mut session, t0), Closing::Ready);

        assert!(result.exists(), "the close left FilmCraft without a result movie");
        assert!(!dir.join(format!("{LINK}.mov.part")).exists());
        assert!(names(&events).contains(&"link.result".to_string()), "{:?}", names(&events));
        assert_eq!(session.project.craft_links[LINK].epoch, 1);
        live.flush_sidecars(&session);
        assert_eq!(sidecar_epoch(&dir, LINK), 1, "the sidecar must carry the epoch of the result");
    }

    /// An edit that the quiet period has not rendered yet is rendered by the close, with no wait.
    #[test]
    fn closing_right_after_an_edit_does_not_wait_for_the_quiet_period() {
        const LINK: &str = "6666-4444-4444-8444-444444444444";
        let dir = scratch("close-edit");
        let result = dir.join(format!("{LINK}.mov"));
        let mut session = session_with_link(LINK, &result);
        let (live, _events) = live();
        let t0 = Instant::now();
        live.tick(&mut session, t0);
        edit(&mut session);
        live.tick(&mut session, t0 + Duration::from_millis(5));
        assert!(!result.exists());
        assert_eq!(live.begin_close(&mut session, t0 + Duration::from_millis(10)), Closing::Rendering);
        assert_eq!(drive_close(&live, &mut session, t0 + Duration::from_millis(10)), Closing::Ready);
        assert!(result.exists());
        assert_eq!(live.close_state(), Closing::Ready);
    }

    /// A second close request cancels the render. The sidecar is written, no part file stays and
    /// the result is not claimed.
    #[test]
    fn aborting_a_close_cancels_the_render_and_leaves_no_part_file() {
        const LINK: &str = "7777-4444-4444-8444-444444444444";
        let dir = scratch("close-abort");
        let result = dir.join(format!("{LINK}.mov"));
        let mut session = session_with_link(LINK, &result);
        let (live, _events) = live();
        let t0 = Instant::now();
        live.tick(&mut session, t0);
        edit(&mut session);
        live.tick(&mut session, t0);
        assert_eq!(live.begin_close(&mut session, t0), Closing::Rendering);

        live.abort_close(&session);

        assert_eq!(live.close_state(), Closing::Ready, "an aborted close must not keep the app in close mode");
        assert!(!dir.join(format!("{LINK}.mov.part")).exists(), "a half-written part file was left");
        let sidecar = std::fs::read_to_string(dir.join(format!("{LINK}.ecproj"))).unwrap();
        assert!(effectcraft_project::Project::from_json(&sidecar).is_ok(), "the sidecar was not written on abort");
    }

    /// A render that cannot run (footage is gone) must not hold the window open forever.
    #[test]
    fn a_close_whose_render_fails_still_finishes_and_leaves_only_the_sidecar() {
        const LINK: &str = "8888-4444-4444-8444-444444444444";
        let dir = scratch("close-fail");
        let result = dir.join(format!("{LINK}.mov"));
        let mut session = session_with_link(LINK, &result);
        session.project_mut().craft_links.get_mut(LINK).unwrap().result_path = dir.join("other.mov").to_string_lossy().into_owned();
        let (live, _events) = live();
        let t0 = Instant::now();
        live.tick(&mut session, t0);
        let state = live.begin_close(&mut session, t0);
        assert!(state == Closing::Ready || drive_close(&live, &mut session, t0) == Closing::Ready);
        assert!(!result.exists());
    }

    #[test]
    fn unbinding_the_last_link_clears_the_frame_world() {
        const LINK: &str = "4444-4444-4444-8444-444444444444";
        let dir = scratch("unbind");
        let mut session = session_with_link(LINK, &dir.join(format!("{LINK}.mov")));
        let (live, _events) = live();
        live.tick(&mut session, Instant::now());
        session.project_mut().craft_links.clear();
        live.tick(&mut session, Instant::now());
        live.tick(&mut session, Instant::now());
        let spec = FrameSpec { link_id: LINK.into(), format: FrameFormat::Rgba8Premul, scale: 1.0 };
        assert!(live.frames().open(&spec).is_err());
        assert_eq!(epoch_of(LINK), 0);
    }
}
