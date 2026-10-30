//! The `log` logger. Records go to standard error and, on the desktop, to a session log that
//! every launch starts afresh: `Logs/effectcraft.log` in the settings folder, with the two runs
//! before it kept as `effectcraft.1.log` and `effectcraft.2.log`, so the log of a run that
//! crashed survives the next start. A launch from the Dock, the Start menu or a launcher has no
//! terminal, so that file is what a bug report can attach. The last few hundred warnings and
//! errors are also kept in memory for the System Compatibility Report.
//!
//! Levels by default ([`DEFAULT_FILTER`]): `info` for EffectCraft's own crates, `warn` for
//! everything else (wgpu and naga are chatty). `RUST_LOG` replaces that (a level such as `debug`, or per target:
//! `effectcraft=debug,wgpu=warn`).
//!
//! Help ▸ Enable Logging ([`set_enabled`], the `help.enableLogging` command) additionally appends
//! every EffectCraft record to `Logs/EffectCraft Log.txt`, across launches.
//!
//! Frontends call [`install`] (or [`install_with`] for another default, as the CLI does with `warn`)
//! once at startup and the desktop app [`attach_session_log`] once the settings folder is known; records logged before that are kept (up to [`MAX_PENDING`]) and
//! written first. Writing never panics: a file that can't be created or written leaves standard
//! error as the only sink. Never log the control-channel token.

use std::collections::VecDeque;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};

use log::LevelFilter;

/// The session log's name inside the `Logs` folder.
pub const SESSION_LOG: &str = "effectcraft.log";
/// How many previous session logs are kept (`effectcraft.1.log` … `effectcraft.<KEEP>.log`).
pub const KEEP: usize = 2;
/// The session log stops growing past this size (a runaway warning can't fill the disk).
pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
/// Records kept in memory until the session log is attached.
pub const MAX_PENDING: usize = 512;
/// The filter when `RUST_LOG` is unset or empty.
pub const DEFAULT_FILTER: &str = "warn,effectcraft=info";

static ENABLED: AtomicBool = AtomicBool::new(false);
/// The filter for standard error and the session log: (target prefix or every target, level).
static FILTER: OnceLock<Vec<(Option<String>, LevelFilter)>> = OnceLock::new();
static STATE: Mutex<State> = Mutex::new(State { file: None, recent: VecDeque::new() });
static SESSION: Mutex<Sink> = Mutex::new(Sink::new(MAX_FILE_BYTES));

struct State {
    file: Option<PathBuf>,
    recent: VecDeque<String>,
}

struct Logger;

const RECENT: usize = 300;

impl log::Log for Logger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Warn || (ENABLED.load(Ordering::Relaxed) && m.target().starts_with("effectcraft")) || shown(m)
    }
    fn log(&self, r: &log::Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let line = format_line(&timestamp(unix_millis()), r.level(), &thread_name(), r.target(), &r.args().to_string());
        if shown(r.metadata()) {
            let _ = writeln!(std::io::stderr(), "{line}");
            SESSION.lock().unwrap_or_else(PoisonError::into_inner).write(&format!("{line}\n"));
        }
        let Ok(mut st) = STATE.lock() else { return };
        if r.level() <= log::Level::Warn {
            st.recent.push_back(line.clone());
            if st.recent.len() > RECENT {
                st.recent.pop_front();
            }
        }
        if ENABLED.load(Ordering::Relaxed)
            && let Some(f) = &st.file
            && let Ok(mut out) = std::fs::OpenOptions::new().create(true).append(true).open(f)
        {
            let _ = writeln!(out, "{line}");
        }
    }
    fn flush(&self) {}
}

fn unix_millis() -> u64 {
    web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)).unwrap_or(0)
}

fn thread_name() -> String {
    #[cfg(not(target_arch = "wasm32"))]
    return std::thread::current().name().unwrap_or("?").to_string();
    #[cfg(target_arch = "wasm32")]
    return "main".to_string();
}

/// `2026-10-08T07:59:17.728Z` (UTC, milliseconds) for `ms` milliseconds since 1970. A clock set
/// absurdly far ahead keeps its real year (preserved) instead of reading as the epoch.
fn timestamp(ms: u64) -> String {
    let (secs, millis) = (ms / 1000, (ms % 1000) as u32);
    let (y, t) = utc_parts(secs, millis * 1_000_000);
    // `{:04}` rather than `%Y`, which writes a `+` before years past 9999.
    format!("{y:04}-{}", t.format("%m-%dT%H:%M:%S%.3fZ"))
}

/// Unix seconds (+ nanos) → (proleptic Gregorian year, the same wall-clock instant moved into
/// chrono's range by whole 400-year cycles). The Gregorian calendar repeats every 400 years
/// (146_097 days), so month, day and time of day come from chrono and only the year is carried,
/// which keeps every input exact instead of collapsing out-of-range years to the epoch. `secs` is
/// `u64` (the clock is clamped at the epoch before 1970), so `days` always fits an `i64`.
fn utc_parts(secs: u64, nanos: u32) -> (i64, chrono::NaiveDateTime) {
    use chrono::Datelike;
    let (days, sod) = ((secs / 86_400) as i64, (secs % 86_400) as i64);
    let (cycles, rest) = (days.div_euclid(146_097), days.rem_euclid(146_097));
    // `rest + 719_163` (1970-01-01 is CE day 719_163) is always a valid CE day, and `sod` < 86_400.
    let date = i32::try_from(rest + 719_163).ok().and_then(chrono::NaiveDate::from_num_days_from_ce_opt);
    let time = u32::try_from(sod).ok().and_then(|s| chrono::NaiveTime::from_num_seconds_from_midnight_opt(s, nanos.min(999_999_999)));
    match (date, time) {
        (Some(d), Some(t)) => {
            let y = i128::from(d.year()) + i128::from(cycles) * 400;
            (i64::try_from(y).unwrap_or(i64::MAX), d.and_time(t))
        }
        _ => (1970, chrono::NaiveDateTime::default()), // unreachable
    }
}

/// One log line (without the newline): `<timestamp> <LEVEL> [<thread>] <target>: <message>`.
fn format_line(ts: &str, level: log::Level, thread: &str, target: &str, message: &str) -> String {
    let message = message.strip_suffix('\n').unwrap_or(message);
    format!("{ts} {level:<5} [{thread}] {target}: {message}")
}

/// Install the logger (once; later calls and other installed loggers are left alone).
pub fn install() {
    install_with(DEFAULT_FILTER);
}

/// The directive string: `RUST_LOG` when it is set and nonblank, otherwise `default`.
fn filter_spec<'a>(rust_log: Option<&'a str>, default: &'a str) -> &'a str {
    rust_log.filter(|s| !s.trim().is_empty()).unwrap_or(default)
}

/// [`install`] with `default` as the filter when `RUST_LOG` is unset or empty.
pub fn install_with(default: &str) {
    static LOGGER: Logger = Logger;
    if log::set_logger(&LOGGER).is_ok() {
        let filter = FILTER.get_or_init(|| {
            let spec = std::env::var("RUST_LOG").ok();
            parse_filter(filter_spec(spec.as_deref(), default))
        });
        log::set_max_level(filter.iter().map(|d| d.1).fold(LevelFilter::Info, Ord::max));
    }
}

/// Whether the filter lets a record through to standard error and the session log.
fn shown(m: &log::Metadata) -> bool {
    FILTER.get().is_some_and(|f| m.level() <= level_for(f, m.target()))
}

/// `RUST_LOG`'s directives, as env_logger reads them: `level`, `target=level` or a bare target
/// (every level); unknown levels are skipped.
fn parse_filter(spec: &str) -> Vec<(Option<String>, LevelFilter)> {
    spec.split(',')
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .filter_map(|d| match d.split_once('=') {
            Some((t, l)) => Some((Some(t.trim().to_string()), l.trim().parse().ok()?)),
            None => Some(d.parse().map_or_else(|_| (Some(d.to_string()), LevelFilter::Trace), |l| (None, l))),
        })
        .collect()
}

/// The level of the directive whose target is the longest prefix of `target` (a directive
/// without a target matches every target); off when none does.
fn level_for(filter: &[(Option<String>, LevelFilter)], target: &str) -> LevelFilter {
    filter
        .iter()
        .filter(|(t, _)| t.as_deref().is_none_or(|t| target.starts_with(t)))
        .max_by_key(|(t, _)| t.as_ref().map_or(0, String::len))
        .map_or(LevelFilter::Off, |d| d.1)
}

/// Log every panic (message and location) through the logger, then run the previous hook
/// (which prints it). Panics in commands are then caught by [`crate::guard::guarded`].
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("panic: {info}");
        previous(info);
    }));
}

/// The `Logs` folder inside `config_dir` (the settings folder), or in the temporary folder.
pub fn log_dir(config_dir: Option<&Path>) -> PathBuf {
    config_dir.map(Path::to_path_buf).unwrap_or_else(std::env::temp_dir).join("Logs")
}

/// Help ▸ Enable Logging's file inside `config_dir` (the settings folder), or the temporary folder.
pub fn log_path(config_dir: Option<&Path>) -> PathBuf {
    log_dir(config_dir).join("EffectCraft Log.txt")
}

fn numbered(dir: &Path, n: usize) -> PathBuf {
    if n == 0 { dir.join(SESSION_LOG) } else { dir.join(format!("effectcraft.{n}.log")) }
}

/// Shift the previous session logs up one (`.log` → `.1.log` → … → `.<KEEP>.log`, the oldest
/// dropped) and create a fresh, empty log file in `dir` (created if missing).
fn rotate(dir: &Path) -> std::io::Result<(PathBuf, File)> {
    std::fs::create_dir_all(dir)?;
    for n in (1..=KEEP).rev() {
        match std::fs::rename(numbered(dir, n - 1), numbered(dir, n)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    let path = numbered(dir, 0);
    let file = File::create(&path)?;
    Ok((path, file))
}

/// Rotate the session logs in `dir` (the `Logs` folder, see [`log_dir`]) and write records to
/// a fresh `effectcraft.log` there, first the ones logged so far; returns its path.
pub fn attach_session_log(dir: &Path) -> Result<PathBuf, String> {
    let (path, file) = rotate(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut sink = SESSION.lock().unwrap_or_else(PoisonError::into_inner);
    sink.attach(file);
    sink.path = Some(path.clone());
    Ok(path)
}

/// This run's session log (once [`attach_session_log`] succeeded).
pub fn session_file() -> Option<PathBuf> {
    SESSION.lock().unwrap_or_else(PoisonError::into_inner).path.clone()
}

/// Where lines go besides standard error: kept in memory until a file is attached, then
/// appended to it until it reaches its size cap.
struct Sink {
    file: Option<File>,
    path: Option<PathBuf>,
    pending: Vec<String>,
    dropped: usize,
    written: u64,
    max_bytes: u64,
    capped: bool,
}

impl Sink {
    const fn new(max_bytes: u64) -> Sink {
        Sink { file: None, path: None, pending: Vec::new(), dropped: 0, written: 0, max_bytes, capped: false }
    }

    /// Append `line` to the file, or keep it until [`Sink::attach`].
    fn write(&mut self, line: &str) {
        if self.file.is_none() && !self.capped {
            if self.pending.len() < MAX_PENDING {
                self.pending.push(line.to_owned());
            } else {
                self.dropped = self.dropped.saturating_add(1);
            }
            return;
        }
        self.append(line);
    }

    /// Start writing to `file`, first the lines kept so far.
    fn attach(&mut self, file: File) {
        self.file = Some(file);
        for line in std::mem::take(&mut self.pending) {
            self.append(&line);
        }
        if self.dropped > 0 {
            let note = format!("{} earlier log lines were dropped before the log file was opened\n", self.dropped);
            self.dropped = 0;
            self.append(&note);
        }
    }

    fn append(&mut self, line: &str) {
        if self.capped {
            return;
        }
        let Some(file) = self.file.as_mut() else { return };
        let len = u64::try_from(line.len()).unwrap_or(u64::MAX);
        let text = if self.written.saturating_add(len) > self.max_bytes {
            self.capped = true;
            format!("log file reached {} bytes; later records go to standard error only\n", self.max_bytes)
        } else {
            self.written = self.written.saturating_add(len);
            line.to_owned()
        };
        // A full disk or a vanished file must not take the app down: drop the file sink.
        if file.write_all(text.as_bytes()).is_err() {
            self.file = None;
            self.capped = true;
        }
    }
}

/// Turn file logging on (writing to `file`) or off.
pub fn set_enabled(on: bool, file: PathBuf) -> std::io::Result<()> {
    install();
    if on {
        if let Some(d) = file.parent() {
            std::fs::create_dir_all(d)?;
        }
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&file)?;
        writeln!(f, "{} INFO  effectcraft: logging enabled ({} {})", timestamp(unix_millis()), std::env::consts::OS, env!("CARGO_PKG_VERSION"))?;
    }
    if let Ok(mut st) = STATE.lock() {
        st.file = Some(file);
    }
    ENABLED.store(on, Ordering::Relaxed);
    Ok(())
}

pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// The current Enable Logging file (set once logging was enabled).
pub fn current_file() -> Option<PathBuf> {
    STATE.lock().ok().and_then(|s| s.file.clone())
}

/// Recent warnings and errors, oldest first.
pub fn recent() -> Vec<String> {
    STATE.lock().map(|s| s.recent.iter().cloned().collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// `RUST_LOG` picks what start-up prints to stderr (#234).
    #[test]
    fn rust_log_filters_by_level_and_target() {
        let f = parse_filter("info, wgpu=warn,wgpu_core=debug, effectcraft , bogus=loud");
        assert_eq!(level_for(&f, "eframe::native"), LevelFilter::Info);
        assert_eq!(level_for(&f, "wgpu_hal::metal"), LevelFilter::Warn);
        assert_eq!(level_for(&f, "wgpu_core::device"), LevelFilter::Debug, "the longest matching target wins");
        assert_eq!(level_for(&f, "effectcraft_ui_egui"), LevelFilter::Trace, "a bare target: every level");
        assert_eq!(f.len(), 4, "an unknown level is skipped");
        assert_eq!(level_for(&parse_filter("wgpu=debug"), "winit"), LevelFilter::Off);
        assert!(parse_filter("").is_empty());
    }

    fn temp_dir(tag: &str) -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!("effectcraft-logging-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    /// Without `RUST_LOG`, EffectCraft's own info records and everyone's warnings are shown.
    #[test]
    fn the_default_filter_shows_effectcraft_info_and_other_crates_warnings() {
        let f = parse_filter(DEFAULT_FILTER);
        assert_eq!(level_for(&f, "effectcraft"), LevelFilter::Info);
        assert_eq!(level_for(&f, "effectcraft_ui_egui::app"), LevelFilter::Info);
        assert_eq!(level_for(&f, "wgpu_core::device"), LevelFilter::Warn);
        assert_eq!(level_for(&f, "naga"), LevelFilter::Warn);
    }

    /// The CLI's default (`warn`): warnings from every crate, no info chatter on stderr,
    /// while a nonblank `RUST_LOG` overrides `default`.
    #[test]
    fn the_cli_filter_shows_only_warnings() {
        assert_eq!(filter_spec(None, "warn"), "warn");
        assert_eq!(filter_spec(Some("  "), "warn"), "warn");
        assert_eq!(filter_spec(Some("info"), "warn"), "info");
        let f = parse_filter(filter_spec(None, "warn"));
        assert_eq!(level_for(&f, "effectcraft_gpu::context"), LevelFilter::Warn);
        assert_eq!(level_for(&f, "effectcraft_engine::render_queue"), LevelFilter::Warn);
        assert_eq!(level_for(&f, "wgpu_core::device"), LevelFilter::Warn);
        let over = parse_filter(filter_spec(Some("info"), "warn"));
        assert_eq!(level_for(&over, "effectcraft_gpu::context"), LevelFilter::Info);
    }

    #[test]
    fn timestamps_are_utc_with_milliseconds() {
        assert_eq!(timestamp(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(timestamp(1_791_446_357_728), "2026-10-08T07:59:17.728Z");
        assert_eq!(timestamp(1_709_164_800_000), "2024-02-29T00:00:00.000Z");
        assert_eq!(timestamp(951_782_400_000), "2000-02-29T00:00:00.000Z");
        assert_eq!(timestamp(1_767_225_599_000), "2025-12-31T23:59:59.000Z");
        // Years past 9999 keep the plain zero-padded year (no `+`).
        assert_eq!(timestamp(253_402_300_800_000), "10000-01-01T00:00:00.000Z");
        // A clock absurdly far ahead keeps its real year (years far outside chrono's own range are
        // preserved) instead of collapsing to the epoch (reviewer rule from the sibling PRs).
        assert_eq!(timestamp(10_000_000_000_000_000), "318857-05-20T17:46:40.000Z");
        assert_eq!(timestamp(u64::MAX), "584556019-04-03T14:25:51.615Z");
    }

    /// Each launch keeps the previous runs' logs, so the log of a run that crashed survives.
    #[test]
    fn rotation_keeps_the_previous_logs_and_starts_an_empty_file() {
        let dir = temp_dir("rotate");
        for run in 1..=4 {
            let (path, _file) = rotate(&dir).unwrap();
            assert_eq!(path, dir.join(SESSION_LOG));
            assert_eq!(read(&path), "");
            std::fs::write(&path, format!("run {run}")).unwrap();
        }
        assert_eq!(read(&dir.join(SESSION_LOG)), "run 4");
        assert_eq!(read(&dir.join("effectcraft.1.log")), "run 3");
        assert_eq!(read(&dir.join("effectcraft.2.log")), "run 2");
        assert!(!dir.join("effectcraft.3.log").exists(), "only {KEEP} old logs are kept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotation_into_an_unusable_directory_is_an_error_not_a_panic() {
        let dir = temp_dir("blocked");
        std::fs::write(&dir, "a file where the directory should be").unwrap();
        assert!(rotate(&dir).is_err());
        let _ = std::fs::remove_file(&dir);
    }

    #[test]
    fn lines_logged_before_the_file_exists_are_written_when_it_is_attached() {
        let dir = temp_dir("pending");
        let (path, file) = rotate(&dir).unwrap();
        let mut sink = Sink::new(MAX_FILE_BYTES);
        sink.write("early 1\n");
        sink.write("early 2\n");
        sink.attach(file);
        sink.write("late\n");
        assert_eq!(read(&path), "early 1\nearly 2\nlate\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_pending_buffer_is_bounded_and_says_how_much_it_dropped() {
        let dir = temp_dir("bounded");
        let (path, file) = rotate(&dir).unwrap();
        let mut sink = Sink::new(MAX_FILE_BYTES);
        for i in 0..MAX_PENDING + 5 {
            sink.write(&format!("line {i}\n"));
        }
        sink.attach(file);
        let text = read(&path);
        assert_eq!(text.lines().filter(|l| l.starts_with("line ")).count(), MAX_PENDING);
        assert_eq!(text.matches("5 earlier log lines were dropped").count(), 1, "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_file_stops_at_its_size_cap_with_one_note() {
        let dir = temp_dir("cap");
        let (path, file) = rotate(&dir).unwrap();
        let mut sink = Sink::new(100);
        sink.attach(file);
        for i in 0..50 {
            sink.write(&format!("line {i:02} with some padding\n"));
        }
        let text = read(&path);
        assert!(text.lines().filter(|l| l.starts_with("line ")).count() < 5, "{text}");
        assert_eq!(text.matches("log file reached").count(), 1, "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_line_carries_time_level_thread_target_and_message() {
        assert_eq!(
            format_line("2026-10-08T07:59:17.728Z", log::Level::Warn, "main", "effectcraft_engine::autosave", "disk full\n"),
            "2026-10-08T07:59:17.728Z WARN  [main] effectcraft_engine::autosave: disk full"
        );
    }
}
