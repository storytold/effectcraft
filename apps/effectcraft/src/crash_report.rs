//! A small, separate process reports an unexpected desktop exit (#3357).
//!
//! Allocation failure can abort without a panic hook. The GUI therefore owns a pipe to a
//! helper which never loads a project, font database or GPU. Only an explicit marker after
//! normal teardown suppresses the report. EOF alone identifies a lost control channel, not
//! a process exit or its cause. System-wide exhaustion can still prevent the helper or its native dialog working.

use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[path = "crash_observation.rs"]
mod observation;

const FLAG: &str = "--internal-crash-reporter";
const CLEAN: &[u8] = b"EFFECTCRAFT_CLEAN_EXIT_V1\n";
const LOG_BYTES: usize = 64 * 1024;
const HEADER_BYTES: usize = 128;
const READY: &[u8] = b"READY\n";
const EVENT_BYTES: usize = 128;
const EVENT_COUNT: usize = 32;

/// Owned only by the GUI, never handed to another child. Dropping it without `finish` closes
/// the pipe and reports the incident; there is intentionally no clean-exit Drop hook.
pub struct Reporter {
    pipe: ChildStdin,
    child: Child,
}

impl Reporter {
    /// Call only after application teardown, including handled startup/device-loss errors.
    pub fn finish(mut self) -> io::Result<()> {
        let sent = self.pipe.write_all(CLEAN).and_then(|()| self.pipe.flush());
        drop(self.pipe);
        // Do not let a suspended/broken helper hang shutdown. Reap it before the GUI
        // exits where possible, so Windows updates do not race an old executable.
        let cleanup = finish_child(&mut self.child, Duration::from_secs(2));
        match (sent, cleanup) {
            (Err(sent), Err(cleanup)) => Err(io::Error::other(format!("clean marker failed: {sent}; cleanup failed: {cleanup}"))),
            (Err(error), _) | (_, Err(error)) => Err(error),
            (Ok(()), Ok(())) => Ok(()),
        }
    }
}

fn finish_child(child: &mut Child, grace: Duration) -> io::Result<()> {
    let start = Instant::now();
    while start.elapsed() < grace {
        if let Some(status) = child.try_wait()? {
            return if status.success() { Ok(()) } else { Err(io::Error::other(format!("crash reporter exited with {status}"))) };
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    child.kill()?;
    // Even after termination, do not introduce an unbounded wait on a stuck OS process.
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(1) {
        if child.try_wait()?.is_some() {
            return Err(io::Error::other("crash reporter did not finish promptly and was terminated"));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err(io::Error::other("crash reporter termination could not be confirmed"))
}

/// Shared ownership lets macOS finish in on_exit, while main retains the startup-error path.
#[derive(Clone, Default)]
pub struct Shutdown(std::sync::Arc<std::sync::Mutex<Option<Reporter>>>);

impl Shutdown {
    /// Install the one reporter created for this desktop launch.
    pub fn install(&self, reporter: Reporter) -> io::Result<()> {
        let mut slot = self.0.lock().map_err(|_| io::Error::other("crash reporter ownership was poisoned"))?;
        *slot = Some(reporter);
        Ok(())
    }

    /// Keep only fixed-vocabulary, bounded lifecycle facts outside the GUI. Callers emit each
    /// category once; their combined payload fits well below the smallest OS pipe buffer.
    pub fn record(&self, event: &'static str) -> io::Result<()> {
        if event.len() > 100 || event.contains(['\n', '\r']) {
            return Err(io::Error::other("invalid diagnostic observation"));
        }
        let mut slot = self.0.lock().map_err(|_| io::Error::other("crash reporter ownership was poisoned"))?;
        if let Some(reporter) = slot.as_mut() {
            writeln!(reporter.pipe, "EVENT {event}")?;
            reporter.pipe.flush()?;
        }
        Ok(())
    }

    /// Consume the reporter exactly once, after the applicable platform teardown boundary.
    pub fn finish(&self) -> io::Result<()> {
        let reporter = self.0.lock().map_err(|_| io::Error::other("crash reporter ownership was poisoned"))?.take();
        reporter.map_or(Ok(()), Reporter::finish)
    }
}

/// A panic during teardown must leave the reporter armed. macOS terminates after this hook
/// without returning to main; other platforms pass None and finish after run_native returns.
pub fn after_teardown(reporter: Option<&Shutdown>, teardown: impl FnOnce()) -> io::Result<()> {
    teardown();
    reporter.map_or(Ok(()), Shutdown::finish)
}

/// Each process/run has its own report, including simultaneous launches.
pub fn run_id() -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    format!("{}-{nanos}", std::process::id())
}

pub fn session_marker(id: &str) -> String {
    format!("crash-report session {id}")
}

/// Start before heavy work. A missing helper is a warning, never a reason to refuse the app.
pub fn start(id: &str, log: Option<&Path>, reports: &Path, interactive: bool) -> io::Result<Reporter> {
    let exe = std::env::current_exe()?;
    let args = [
        OsString::from(FLAG),
        OsString::from(id),
        log.map_or_else(OsString::new, |p| p.as_os_str().to_owned()),
        reports.as_os_str().to_owned(),
        OsString::from(if interactive { "1" } else { "0" }),
        OsString::from(std::process::id().to_string()),
    ];
    spawn(&exe, &args)
}

fn spawn(exe: &Path, args: &[OsString]) -> io::Result<Reporter> {
    let mut child = Command::new(exe).args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn()?;
    if let Err(error) = ready(&mut child) {
        let _ = child.kill();
        let _ = finish_child(&mut child, Duration::from_millis(100));
        return Err(error);
    }
    match child.stdin.take() {
        Some(pipe) => Ok(Reporter { pipe, child }),
        None => {
            let _ = child.kill();
            let _ = child.wait();
            Err(io::Error::other("crash reporter has no control pipe"))
        }
    }
}

/// A fixed acknowledgement means the helper holds the process observation before heavy GUI
/// work begins. A suspended helper cannot hold startup indefinitely.
fn ready(child: &mut Child) -> io::Result<()> {
    let mut output = child.stdout.take().ok_or_else(|| io::Error::other("crash reporter has no readiness pipe"))?;
    let (send, receive) = mpsc::sync_channel(1);
    std::thread::Builder::new().name("ec-reporter-ready".into()).spawn(move || {
        let mut bytes = [0; READY.len()];
        #[cfg(not(test))]
        let result =
            output.read_exact(&mut bytes).and_then(|()| if bytes == READY { Ok(()) } else { Err(io::Error::other("invalid reporter acknowledgement")) });
        // libtest prints its bounded banner before invoking the helper role.
        #[cfg(test)]
        let result = (|| {
            for _ in 0..1024 {
                bytes.rotate_left(1);
                output.read_exact(&mut bytes[READY.len() - 1..])?;
                if bytes == READY {
                    return Ok(());
                }
            }
            Err(io::Error::other("reporter acknowledgement missing"))
        })();
        let _ = send.send(result);
        // Keep the pipe open for libtest's trailing result (and future helper output), without
        // storing it. Dropping it at READY would turn a clean helper exit into BrokenPipe.
        let _ = io::copy(&mut output, &mut io::sink());
    })?;
    receive.recv_timeout(Duration::from_secs(3)).map_err(|_| io::Error::other("crash reporter readiness timed out"))?
}

/// Handle the internal mode before logging, argument parsing, updates or GUI initialization.
/// The callback is a native dialog on this helper's main thread, not on the failed GUI's.
pub fn maybe_run(args: &[OsString], show: impl FnOnce(&str)) -> bool {
    if args.first().map(OsString::as_os_str) != Some(OsStr::new(FLAG)) {
        return false;
    }
    let parsed = match args {
        [_, id, log, reports, interactive, pid] => id
            .to_str()
            .filter(|id| valid_id(id))
            .zip(match interactive.to_str() {
                Some("0") => Some(false),
                Some("1") => Some(true),
                _ => None,
            })
            .zip(pid.to_str().and_then(|p| p.parse::<u32>().ok()).filter(|pid| *pid != 0))
            .map(|((id, interactive), pid)| (id, (!log.is_empty()).then(|| Path::new(log)), Path::new(reports), interactive, pid)),
        _ => None,
    };
    let Some((id, log, reports, interactive, pid)) = parsed else {
        eprintln!("EffectCraft crash reporter: invalid internal arguments");
        return true;
    };
    // Keep the exact file open while the GUI is alive: a later rotation must not substitute
    // another instance's log. If rotation won the startup race, the session marker rejects it.
    let mut log = SessionLog::open(log, id);
    let mut observation = observation::Observation::start(pid);
    // A GUI that dies during helper startup can already have closed this read end. Keep
    // watching its control pipe and save the report rather than losing that early failure.
    let _ = io::stdout().write_all(READY).and_then(|()| io::stdout().flush());
    // The reader has one bounded queue; the monitor never waits on a live pipe, so it can
    // sample the target even when no log or lifecycle message is emitted.
    let (send, receive) = mpsc::sync_channel(1);
    let reader = std::thread::Builder::new().name("ec-reporter-pipe".into()).spawn(move || {
        let mut input = io::stdin().lock();
        loop {
            let message = read_message(&mut input);
            let terminal = !matches!(message, Ok(Message::Event(_)));
            if send.send(message).is_err() || terminal {
                break;
            }
        }
    });
    let mut events = VecDeque::with_capacity(EVENT_COUNT);
    let failure = if reader.is_err() {
        Some("control reader could not start".to_owned())
    } else {
        loop {
            observation.tick();
            match receive.recv_timeout(Duration::from_millis(250)) {
                Ok(Ok(Message::Clean)) => return true,
                Ok(Ok(Message::Event(event))) => {
                    if events.len() == EVENT_COUNT {
                        events.pop_front();
                    }
                    events.push_back(format!("{} {event}", observation::utc()));
                }
                Ok(Ok(Message::Ended)) => break None,
                Ok(Err(error)) => break Some(error.to_string()),
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break Some("control reader disconnected".to_owned()),
            }
        }
    };
    observation.finish();
    let saved = write_report(reports, id, &mut log, failure.as_deref(), &observation, &events);
    let description = description(&saved, observation.confirmed_exit());
    eprintln!("{description}");
    if interactive {
        show(&description);
    }
    true
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 80 && id.bytes().all(|c| c.is_ascii_digit() || c == b'-')
}

enum Message {
    Clean,
    Event(String),
    Ended,
}

/// Every line and the retained event ring are bounded, including malformed control input.
fn read_message(input: &mut impl Read) -> io::Result<Message> {
    let mut line = [0u8; EVENT_BYTES];
    for i in 0..line.len() {
        let byte = line.get_mut(i).ok_or_else(|| io::Error::other("invalid control offset"))?;
        loop {
            match input.read(std::slice::from_mut(byte)) {
                Ok(0) => return Ok(Message::Ended),
                Ok(_) => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        if *byte == b'\n' {
            let text = line.get(..=i).ok_or_else(|| io::Error::other("invalid control line"))?;
            if text == CLEAN {
                return Ok(Message::Clean);
            }
            if let Some(event) = text.strip_prefix(b"EVENT ") {
                let event = std::str::from_utf8(event).map_err(|_| io::Error::other("non-UTF8 diagnostic observation"))?;
                return Ok(Message::Event(event.trim_end().to_owned()));
            }
            return Ok(Message::Ended);
        }
    }
    Err(io::Error::other("control line exceeded its fixed limit"))
}

#[cfg(test)]
fn clean_exit(input: &mut impl Read) -> io::Result<bool> {
    Ok(matches!(read_message(input)?, Message::Clean))
}

enum SessionLog {
    Captured { file: File, marker: String },
    Unavailable(String),
}

impl SessionLog {
    fn open(path: Option<&Path>, id: &str) -> Self {
        let opened = (|| {
            let path = path.ok_or_else(|| io::Error::other("no session log was opened"))?;
            let mut file = File::open(path)?;
            let mut header = [0u8; HEADER_BYTES];
            let marker = format!("{}\n", session_marker(id));
            let prefix = header.get_mut(..marker.len()).ok_or_else(|| io::Error::other("session marker too long"))?;
            file.read_exact(prefix)?;
            if prefix != marker.as_bytes() {
                return Err(io::Error::other("matching session marker unavailable; the log may have rotated"));
            }
            Ok(file)
        })();
        match opened {
            Ok(file) => Self::Captured { file, marker: format!("{}\n", session_marker(id)) },
            Err(e) => Self::Unavailable(e.to_string()),
        }
    }

    fn write_tail(&mut self, report: &mut File) -> io::Result<()> {
        match self {
            Self::Unavailable(reason) => writeln!(report, "Recent log unavailable: {reason}"),
            Self::Captured { file, marker } => {
                // An older concurrent launcher may have truncated the captured file.
                // Recheck its identity before copying any content into this report.
                file.seek(SeekFrom::Start(0))?;
                let mut header = [0u8; HEADER_BYTES];
                let prefix = header.get_mut(..marker.len()).ok_or_else(|| io::Error::other("session marker too long"))?;
                if file.read_exact(prefix).is_err() || prefix != marker.as_bytes() {
                    return writeln!(report, "Recent log unavailable: session identity changed");
                }
                let size = file.metadata()?.len();
                file.seek(SeekFrom::Start(size.saturating_sub(LOG_BYTES as u64)))?;
                writeln!(report, "\nRecorded session log tail (last {LOG_BYTES} bytes at most; logging filters/size caps may omit later events):")?;
                io::copy(&mut file.take(LOG_BYTES as u64), report)?;
                Ok(())
            }
        }
    }
}

fn write_report(
    reports: &Path,
    id: &str,
    log: &mut SessionLog,
    pipe_error: Option<&str>,
    observation: &observation::Observation,
    events: &VecDeque<String>,
) -> io::Result<PathBuf> {
    if !valid_id(id) {
        return Err(io::Error::other("invalid report identity"));
    }
    std::fs::create_dir_all(reports)?;
    let path = reports.join(format!("crash-{id}.txt"));
    let mut report = OpenOptions::new().write(true).create_new(true).open(&path)?;
    writeln!(
        report,
        "EffectCraft unexpected-exit diagnostic\nVersion: {}\nPlatform: {} / {}\nSession: {id}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    )?;
    writeln!(
        report,
        "The GUI control channel ended without its clean-exit marker.\nThe exact cause is not established. GPU fallback, memory pressure and process exit are separate observations, not a proven causal chain.\nThis report was saved locally; it was not sent anywhere."
    )?;
    observation.write(&mut report)?;
    writeln!(report, "\nRetained lifecycle observations (last {EVENT_COUNT}; UTC is helper receipt time):")?;
    for event in events {
        writeln!(report, "{event}")?;
    }
    if let Some(error) = pipe_error {
        writeln!(report, "Control pipe error: {error}")?;
    }
    if let Err(error) = log.write_tail(&mut report) {
        writeln!(report, "\nCould not read the recent session log: {error}")?;
    }
    report.flush()?;
    Ok(path)
}

fn description(saved: &io::Result<PathBuf>, confirmed_exit: bool) -> String {
    let diagnostic = match saved {
        Ok(path) => format!("A diagnostic report was saved to:\n{}", path.display()),
        Err(error) => format!("A diagnostic report could not be saved: {error}"),
    };
    let incident =
        if confirmed_exit { "EffectCraft stopped unexpectedly." } else { "EffectCraft lost its diagnostic control channel; process exit is not confirmed." };
    format!(
        "{incident}\n\n{diagnostic}\n\nThe exact cause is unknown. If EffectCraft has closed, reopen it to check for an existing auto-save. Changes since the last auto-save may be lost.\n\nNo report has been sent anywhere."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

    static NEXT: AtomicUsize = AtomicUsize::new(0);
    const ROLE: &str = "EFFECTCRAFT_CRASH_REPORT_TEST_ROLE";

    fn test_report(dir: &Path, id: &str, log: &mut SessionLog) -> io::Result<PathBuf> {
        write_report(dir, id, log, None, &observation::Observation::start(std::process::id()), &VecDeque::new())
    }

    fn folder() -> PathBuf {
        let p = std::env::temp_dir().join(format!("ec-crash-report-{}-{}", run_id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn test_name(name: &str) -> String {
        // Also usable from an isolated harness with the reporter dependencies.
        let prefix = module_path!().split_once("::").map_or("", |(_, rest)| rest);
        format!("{prefix}::{name}")
    }

    fn helper(dir: &Path, id: &str) -> Reporter {
        helper_mode(dir, id, false)
    }

    fn helper_mode(dir: &Path, id: &str, interactive: bool) -> Reporter {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &test_name("reporter_process"), "--ignored", "--nocapture"])
            .env(ROLE, "reporter")
            .env("EC_REPORT_DIR", dir)
            .env("EC_REPORT_ID", id)
            .env("EC_REPORT_INTERACTIVE", if interactive { "1" } else { "0" })
            .env("EC_REPORT_PID", std::process::id().to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        ready(&mut child).unwrap();
        Reporter { pipe: child.stdin.take().unwrap(), child }
    }

    fn wait_file(path: &Path) {
        let until = Instant::now() + Duration::from_secs(10);
        while !path.is_file() && Instant::now() < until {
            thread::sleep(Duration::from_millis(20));
        }
        assert!(path.is_file(), "report did not arrive: {}", path.display());
    }

    #[test]
    fn bounded_protocol_rejects_eof_and_partial_or_wrong_markers() {
        let mut clean = CLEAN;
        assert!(clean_exit(&mut clean).unwrap());
        for mut input in [b"".as_slice(), &CLEAN[..CLEAN.len() - 1], b"clean", b"EFFECTCRAFT_CLEAN_EXIT_V0\n"] {
            assert!(!clean_exit(&mut input).unwrap());
        }
        assert!(clean_exit(&mut &vec![b'x'; 100_000][..]).is_err());
    }

    #[test]
    fn report_is_bounded_and_keeps_its_session_after_log_rotation() {
        let dir = folder();
        let original = dir.join("effectcraft.log");
        let id = "123-456";
        std::fs::write(&original, format!("{}\n", session_marker(id))).unwrap();
        let mut log = SessionLog::open(Some(&original), id);
        std::fs::rename(&original, dir.join("effectcraft.1.log")).unwrap();
        std::fs::write(&original, "OTHER SESSION MUST NOT APPEAR").unwrap();
        let mut old = OpenOptions::new().append(true).open(dir.join("effectcraft.1.log")).unwrap();
        old.write_all(&vec![b'a'; LOG_BYTES * 2]).unwrap();
        old.write_all(b"\nGPU context retired\n").unwrap();
        let report = test_report(&dir, id, &mut log).unwrap();
        let text = std::fs::read_to_string(report).unwrap();
        assert!(text.len() < LOG_BYTES + 4096);
        assert!(text.contains("GPU context retired"));
        assert!(!text.contains("OTHER SESSION"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn wrong_session_and_missing_logs_are_reported_without_reading_other_content() {
        let dir = folder();
        let path = dir.join("log");
        std::fs::write(&path, "private other session").unwrap();
        for (id, source) in [("1-1", Some(path.as_path())), ("1-2", None)] {
            let report = test_report(&dir, id, &mut SessionLog::open(source, id)).unwrap();
            let text = std::fs::read_to_string(report).unwrap();
            assert!(text.contains("Recent log unavailable"));
            assert!(!text.contains("private other session"));
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn captured_log_rejects_a_session_overwritten_after_open() {
        let dir = folder();
        let path = dir.join("log");
        std::fs::write(&path, "crash-report session 1-2\noriginal session\n").unwrap();
        let mut log = SessionLog::open(Some(&path), "1-2");
        std::fs::write(&path, "crash-report session 3-4\nprivate other session\n").unwrap();
        let report = test_report(&dir, "1-2", &mut log).unwrap();
        let text = std::fs::read_to_string(report).unwrap();
        assert!(text.contains("Recent log unavailable"));
        assert!(!text.contains("private other session"));
        assert!(!text.contains("session 3-4"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn session_identity_requires_the_exact_first_line() {
        let dir = folder();
        let path = dir.join("log");
        for header in ["crash-report session 1-20\n", "unrelated\ncrash-report session 1-2\n"] {
            std::fs::write(&path, header).unwrap();
            assert!(matches!(SessionLog::open(Some(&path), "1-2"), SessionLog::Unavailable(_)));
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn report_write_failure_is_described_and_existing_report_is_not_overwritten() {
        let dir = folder();
        let mut log = SessionLog::Unavailable("no log".into());
        let first = test_report(&dir, "2-1", &mut log).unwrap();
        let before = std::fs::read(&first).unwrap();
        let result = test_report(&dir, "2-1", &mut log);
        assert!(result.is_err());
        assert!(description(&result, false).contains("could not be saved"));
        assert_eq!(std::fs::read(first).unwrap(), before);
        assert!(test_report(&dir, "../escape", &mut log).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn normal_and_handled_shutdown_send_clean_marker_without_a_report() {
        for id in ["3-1", "3-2"] {
            let dir = folder();
            let r = helper(&dir, id);
            r.finish().unwrap();
            // A completion file proves the helper consumed the marker, not merely that it
            // hadn't yet produced a report.
            wait_file(&dir.join(format!("ready-{id}")));
            assert!(!dir.join(format!("crash-{id}.txt")).exists());
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn accepted_native_close_finishes_before_process_termination() {
        let dir = folder();
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &test_name("aborting_parent"), "--ignored", "--nocapture"])
            .env(ROLE, "native-close")
            .env("EC_REPORT_DIR", &dir)
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
        wait_file(&dir.join("ready-8-1"));
        assert!(!dir.join("crash-8-1.txt").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_teardown_panic_does_not_mark_the_reporter_clean() {
        let dir = folder();
        let reporter = Shutdown::default();
        reporter.install(helper(&dir, "8-2")).unwrap();
        let result = std::panic::catch_unwind(|| {
            let _ = after_teardown(Some(&reporter), || panic!("simulated teardown failure"));
        });
        assert!(result.is_err());
        drop(reporter);
        wait_file(&dir.join("ready-8-2"));
        assert!(dir.join("crash-8-2.txt").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn platforms_returning_to_main_keep_the_reporter_armed_until_then() {
        let dir = folder();
        let reporter = Shutdown::default();
        reporter.install(helper(&dir, "8-3")).unwrap();
        after_teardown(None, || {}).unwrap();
        assert!(reporter.0.lock().unwrap().is_some());
        reporter.clone().finish().unwrap();
        reporter.finish().unwrap();
        wait_file(&dir.join("ready-8-3"));
        assert!(!dir.join("crash-8-3.txt").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_aborting_parent_leaves_one_diagnostic_report() {
        let dir = folder();
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &test_name("aborting_parent"), "--ignored", "--nocapture"])
            .env(ROLE, "abort")
            .env("EC_REPORT_DIR", &dir)
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(!status.success());
        wait_file(&dir.join("ready-4-1"));
        let text = std::fs::read_to_string(dir.join("crash-4-1.txt")).unwrap();
        assert!(text.contains("exact cause is not established"));
        assert!(text.contains("Detection UTC:"));
        #[cfg(windows)]
        assert!(text.contains("Windows status bits:"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_unrelated_child_does_not_keep_the_control_pipe_open() {
        let dir = folder();
        let r = helper(&dir, "5-1");
        let mut unrelated = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &test_name("lingering_child"), "--ignored", "--nocapture"])
            .env(ROLE, "linger")
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        drop(r);
        wait_file(&dir.join("ready-5-1"));
        assert!(unrelated.try_wait().unwrap().is_none());
        unrelated.kill().unwrap();
        unrelated.wait().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_unresponsive_helper_cannot_hang_clean_shutdown() {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &test_name("lingering_child"), "--ignored", "--nocapture"])
            .env(ROLE, "linger")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let pipe = child.stdin.take().unwrap();
        let started = Instant::now();
        let result = Reporter { pipe, child }.finish();
        assert!(result.unwrap_err().to_string().contains("was terminated"));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn simultaneous_sessions_write_separate_reports() {
        let dir = folder();
        let a = helper(&dir, "6-1");
        let b = helper(&dir, "6-2");
        drop(a);
        drop(b);
        wait_file(&dir.join("ready-6-1"));
        wait_file(&dir.join("ready-6-2"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn interactive_failure_calls_dialog_once_after_saving_the_report() {
        let dir = folder();
        drop(helper_mode(&dir, "7-1", true));
        wait_file(&dir.join("ready-7-1"));
        let text = std::fs::read_to_string(dir.join("dialog-7-1")).unwrap();
        assert!(text.contains("crash-7-1.txt"));
        assert!(text.contains("No report has been sent"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn lost_pipe_keeps_bounded_lifecycle_evidence_without_claiming_process_exit() {
        let dir = folder();
        let reporter = Shutdown::default();
        reporter.install(helper(&dir, "9-1")).unwrap();
        for _ in 0..40 {
            reporter.record("GPU observation: wgpu validation error").unwrap();
        }
        drop(reporter);
        wait_file(&dir.join("ready-9-1"));
        let text = std::fs::read_to_string(dir.join("crash-9-1.txt")).unwrap();
        assert!(text.contains("GUI still present"));
        assert!(!text.contains("process termination observed"));
        assert_eq!(text.matches("GPU observation: wgpu validation error").count(), EVENT_COUNT);
        assert!(text.contains("Target-present sampling attempts:"));
        assert!(text.len() < 8192);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn clean_exit_after_lifecycle_events_does_not_write_a_report() {
        let dir = folder();
        let reporter = Shutdown::default();
        reporter.install(helper(&dir, "9-2")).unwrap();
        reporter.record("Backend selection source: default").unwrap();
        reporter.finish().unwrap();
        wait_file(&dir.join("ready-9-2"));
        assert!(!dir.join("crash-9-2.txt").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    #[cfg(windows)]
    fn windows_report_preserves_real_unsigned_exit_bits() {
        let dir = folder();
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &test_name("aborting_parent"), "--ignored", "--nocapture"])
            .env(ROLE, "windows-status")
            .env("EC_REPORT_DIR", &dir)
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert_eq!(status.code().map(|code| code as u32), Some(0xc0000017));
        wait_file(&dir.join("ready-9-3"));
        let text = std::fs::read_to_string(dir.join("crash-9-3.txt")).unwrap();
        assert!(text.contains("Windows status bits: 0xC0000017 (STATUS_NO_MEMORY)"));
        assert!(text.contains("not a verified exception record"));
        assert!(text.contains("process termination observed"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn internal_mode_rejects_invalid_arguments_without_launching_the_app() {
        assert!(!maybe_run(&[], |_| panic!("no dialog")));
        assert!(!maybe_run(&[OsString::from("--version")], |_| panic!("no dialog")));
        assert!(maybe_run(&[OsString::from(FLAG)], |_| panic!("no dialog")));
        let args = [FLAG, "../escape", "", "", "1", "1"].map(OsString::from);
        assert!(maybe_run(&args, |_| panic!("no dialog")));
        let args = [FLAG, "1-1", "", "", "invalid", "1"].map(OsString::from);
        assert!(maybe_run(&args, |_| panic!("no dialog")));
        for pid in ["0", "not-a-pid", "4294967296"] {
            let args = [FLAG, "1-1", "", "", "0", pid].map(OsString::from);
            assert!(maybe_run(&args, |_| panic!("no dialog")));
        }
    }

    #[test]
    fn missing_helper_is_an_error_not_a_panic() {
        assert!(spawn(Path::new("/nonexistent-effectcraft-crash-helper"), &[]).is_err());
    }

    // Roles are ignored by ordinary test runs. Explicit subprocess invocations exercise
    // real EOF/abort behavior without allocating a huge image or killing the test runner.
    #[test]
    #[ignore]
    fn reporter_process() {
        assert_eq!(std::env::var(ROLE).unwrap(), "reporter");
        let dir = PathBuf::from(std::env::var_os("EC_REPORT_DIR").unwrap());
        let id = std::env::var("EC_REPORT_ID").unwrap();
        let interactive = std::env::var("EC_REPORT_INTERACTIVE").unwrap();
        let args = [
            OsString::from(FLAG),
            OsString::from(&id),
            OsString::new(),
            dir.as_os_str().to_owned(),
            OsString::from(&interactive),
            std::env::var_os("EC_REPORT_PID").unwrap(),
        ];
        assert!(maybe_run(&args, |description| {
            assert_eq!(interactive, "1", "non-interactive reporter must not show a dialog");
            assert!(dir.join(format!("crash-{id}.txt")).exists());
            let mut shown = OpenOptions::new().write(true).create_new(true).open(dir.join(format!("dialog-{id}"))).unwrap();
            shown.write_all(description.as_bytes()).unwrap();
        }));
        std::fs::write(dir.join(format!("ready-{id}")), b"done").unwrap();
    }

    #[test]
    #[ignore]
    fn aborting_parent() {
        let role = std::env::var(ROLE).unwrap();
        let dir = PathBuf::from(std::env::var_os("EC_REPORT_DIR").unwrap());
        if role == "native-close" {
            let reporter = Shutdown::default();
            reporter.install(helper(&dir, "8-1")).unwrap();
            after_teardown(Some(&reporter.clone()), || {}).unwrap();
            std::process::exit(0); // Models macOS not returning from its native event loop.
        }
        if role == "windows-status" {
            let _reporter = helper(&dir, "9-3");
            // The real OS status is intentionally set, not actual memory exhaustion. The
            // report must preserve this distinction rather than claiming a verified exception.
            std::process::exit(0xc0000017u32 as i32);
        }
        assert_eq!(role, "abort");
        let _reporter = helper(&dir, "4-1");
        std::process::abort();
    }

    #[test]
    #[ignore]
    fn lingering_child() {
        assert_eq!(std::env::var(ROLE).unwrap(), "linger");
        thread::sleep(Duration::from_secs(30));
    }
}
