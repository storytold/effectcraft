//! Replay scripts: a bug or crash as a list of commands an agent (or a test) can run again.
//!
//! The session's command journal ([`Session::journal`]) becomes a JSON-lines script
//! (`journal.export`, `help.exportRepro`, and automatically when the last-resort guard catches a
//! panic, see [`after_command`]); `effectcraft-cli replay script.jsonl` runs one headless and
//! `crates/host/tests/it/repros.rs` runs every `tests/repros/*.jsonl` as a regression test.
//!
//! Format (one JSON object per line; blank lines and lines starting with `//` or `#` are
//! comments):
//!
//! ```text
//! {"start": "new" | "demo" | {"project": "path.ecproj"}, "allow_errors"?: bool}   optional, first
//! {"command": "comp.new", "params": {...}, "expect_error"?: true | "substring"}
//! {"expect": {"query": "prop.get", "params": {...}, "pointer"?: "/value", "equals"?: ...}}
//! {"expect_no_error": true}
//! ```
//!
//! A command must succeed unless it says `expect_error` (then it must fail, with that text in
//! the message when it is a string), or the start line says `allow_errors` (errors are reported
//! but tolerated; crash repros say so, since after the fix the crashing step may fail cleanly).
//! A panic always fails. `expect` runs a command (normally a query) and compares the value at
//! the JSON `pointer` of its result with `equals` (numbers within 1e-6); `expect_no_error`
//! checks that the previous command succeeded. Other keys (`version`, `app`, `note`, `crash`…)
//! are information for people. Relative `project` paths are relative to the script.
//!
//! Privacy: a repro holds command ids and their parameters only, never project or media file
//! contents. Parameters can include file paths, layer names and text typed into text layers.

use std::cell::Cell;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::{EngineError, Result, Session, guard};

/// The format version written into exported headers.
pub const VERSION: u64 = 1;
/// Largest script [`run`] accepts (bytes).
pub const MAX_SCRIPT_BYTES: usize = 64 << 20;
/// Most lines [`run`] reads.
pub const MAX_LINES: usize = 100_000;
/// Exported scripts stay below this size: the oldest steps are dropped (and counted).
const MAX_EXPORT_BYTES: usize = 32 << 20;
/// Folder of saved repros in the settings store (next to `Logs`).
pub const REPRO_DIR: &str = "Repros";
/// Automatic crash repros kept (the oldest are removed).
const KEEP_CRASH_REPROS: usize = 20;
/// Commands that replace the whole project: an export starts at the last one.
const PROJECT_STARTS: &[&str] = &["file.newProject", "file.open", "file.openDemoProject"];
/// Error and value texts in reports are cut to this many characters.
const MAX_TEXT: usize = 600;

// ---------- nesting and crash capture (Session::execute) ----------

thread_local! {
    /// Commands running on this thread: (all, journaled ones).
    static DEPTH: Cell<(u32, u32)> = const { Cell::new((0, 0)) };
    /// A replay is running on this thread (its panics are reported, not saved as repros).
    static REPLAYING: Cell<bool> = const { Cell::new(false) };
}

/// One running command (see [`Session::execute`]): commands it runs itself (a script's steps)
/// aren't journaled again, and only the outermost command saves a crash repro.
pub struct Nesting {
    outer: (u32, u32),
}

impl Nesting {
    /// Record nesting so commands dispatched by a script are not exported twice.
    pub fn enter(journaled: bool) -> Nesting {
        let outer = DEPTH.with(Cell::get);
        DEPTH.with(|d| d.set((outer.0.saturating_add(1), outer.1.saturating_add(u32::from(journaled)))));
        Nesting { outer }
    }
    /// Run by a journaled command (which replays it).
    pub fn inside_journaled(&self) -> bool {
        self.outer.1 > 0
    }
    /// Whether this command is responsible for capturing a nested panic.
    pub fn outermost(&self) -> bool {
        self.outer.0 == 0
    }
}

impl Drop for Nesting {
    fn drop(&mut self) {
        DEPTH.with(|d| d.set(self.outer));
    }
}

/// After the outermost command: when the guard caught a panic while it ran (`caught` is
/// [`guard::caught`] from before), save the journal plus this command as a crash repro in the
/// settings store and name it in the error.
pub fn after_command(s: &Session, id: &str, params: &Value, nesting: &Nesting, caught: u64, r: Result<Value>) -> Result<Value> {
    if !nesting.outermost() || guard::caught() == caught || REPLAYING.with(Cell::get) {
        return r;
    }
    let error = match &r {
        Err(e) => e.to_string(),
        Ok(_) => "a step it ran failed with an internal error".to_string(),
    };
    let saved = save_crash_repro(s, id, params, &error);
    match (r, saved) {
        (Err(EngineError::Other(m)), Some(path)) => Err(EngineError::Other(format!("{m} (the steps that led here were saved to {path})"))),
        (r, _) => r,
    }
}

/// Wall-clock seconds used only to label exported repros.
fn unix_secs() -> u64 {
    web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// `Repros/<prefix>-<seconds>-<command>.jsonl`.
fn repro_name(prefix: &str, id: &str) -> String {
    let id: String = id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' { c } else { '_' }).take(60).collect();
    format!("{REPRO_DIR}/{prefix}-{:012}-{id}.jsonl", unix_secs())
}

/// Where a store entry is on disk (or its name in stores without a folder).
fn store_path(store: &dyn crate::config::ConfigStore, name: &str) -> String {
    store.dir().map(|d| d.join(name).display().to_string()).unwrap_or_else(|| name.to_string())
}

/// Persist the failing command with its preceding journal and prune older crash repros.
fn save_crash_repro(s: &Session, id: &str, params: &Value, error: &str) -> Option<String> {
    let store = s.config.clone()?;
    let mut text = export(s, Some(json!({"command": id, "error": cut(error)})));
    text.push_str(&command_line(id, params));
    text.push('\n');
    let name = repro_name("crash", id);
    if let Err(e) = store.write(&name, &text) {
        log::error!("cannot save the crash repro {name}: {e}");
        return None;
    }
    // Keep the newest few (names sort by time).
    let mut old: Vec<String> = store.list(REPRO_DIR).into_iter().filter(|n| n.starts_with("crash-")).collect();
    old.sort();
    let extra = old.len().saturating_sub(KEEP_CRASH_REPROS);
    for n in old.into_iter().take(extra) {
        let _ = store.remove(&format!("{REPRO_DIR}/{n}"));
    }
    let path = store_path(store.as_ref(), &name);
    log::error!("crash repro saved to {path}");
    Some(path)
}

/// Save the journal as a repro: at `path` (through the session's file services), else in the
/// settings store's `Repros` folder. Returns where it went and how many steps it holds.
pub fn save_repro(s: &Session, path: Option<&str>) -> Result<(String, usize)> {
    let text = export(s, None);
    let steps = text.lines().count().saturating_sub(1);
    if let Some(path) = path {
        s.services.write_file(path, text.as_bytes()).map_err(|e| EngineError::Other(format!("cannot write {path}: {e}")))?;
        return Ok((path.to_string(), steps));
    }
    let store = s.config.clone().ok_or_else(|| EngineError::Other("there is no settings folder to save the repro in; pass `path`".into()))?;
    let name = repro_name("repro", "session");
    store.write(&name, &text).map_err(|e| EngineError::Other(format!("cannot save {name}: {e}")))?;
    Ok((store_path(store.as_ref(), &name), steps))
}

// ---------- export ----------

/// Serialize one command without including an empty parameter object.
fn command_line(id: &str, params: &Value) -> String {
    let mut o = Map::new();
    o.insert("command".into(), json!(id));
    if !(params.is_null() || params.as_object().is_some_and(Map::is_empty)) {
        o.insert("params".into(), params.clone());
    }
    Value::Object(o).to_string()
}

/// Saving writes the user's files and doesn't change the project: left out of repros.
fn skipped_in_export(id: &str) -> bool {
    id.starts_with("file.save") || id.starts_with("file.increment")
}

/// The journal as a replay script: a start line, then the steps since the project was last
/// replaced (`file.newProject` / `file.open` / `file.openDemoProject`; successful Revert records a resolved `file.open`). `crash` (the failing
/// command and its error) marks a crash repro, which tolerates errors on replay.
pub fn export(s: &Session, crash: Option<Value>) -> String {
    let from = s.journal.iter().rposition(|(id, _)| PROJECT_STARTS.contains(&id.as_str())).unwrap_or(0);
    let mut lines: Vec<String> =
        s.journal.get(from..).unwrap_or_default().iter().filter(|(id, _)| !skipped_in_export(id)).map(|(id, p)| command_line(id, p)).collect();
    let mut size: usize = lines.iter().map(|l| l.len().saturating_add(1)).sum();
    let mut dropped = 0usize;
    while size > MAX_EXPORT_BYTES && !lines.is_empty() {
        size = size.saturating_sub(lines.remove(0).len().saturating_add(1));
        dropped += 1;
    }
    let mut head = Map::new();
    head.insert("start".into(), json!("new"));
    head.insert("version".into(), json!(VERSION));
    head.insert("app".into(), json!(format!("EffectCraft {}", env!("CARGO_PKG_VERSION"))));
    head.insert("os".into(), json!(std::env::consts::OS));
    head.insert("exported".into(), json!(unix_secs()));
    if let Some(c) = crash {
        head.insert("allow_errors".into(), json!(true));
        head.insert("crash".into(), c);
    }
    if dropped > 0 {
        head.insert("dropped".into(), json!(dropped));
    }
    head.insert("note".into(), json!("commands and parameters only (no file contents); parameters can include file paths and text"));
    let mut out = Value::Object(head).to_string();
    out.push('\n');
    for l in lines {
        out.push_str(&l);
        out.push('\n');
    }
    out
}

// ---------- parsing ----------

#[derive(Clone, Debug, PartialEq)]
enum Start {
    New,
    Demo,
    Project(String),
}

#[derive(Clone, Debug)]
enum Step {
    Start(Start),
    Command {
        id: String,
        params: Value,
        expect_error: Option<Option<String>>,
    },
    Expect {
        query: String,
        params: Value,
        pointer: Option<String>,
        equals: Option<Value>,
    },
    ExpectNoError,
    /// A line that isn't a valid step (reported as a failure).
    Invalid(String),
}

fn params_of(v: Option<&Value>) -> std::result::Result<Value, String> {
    match v {
        None | Some(Value::Null) => Ok(json!({})),
        Some(p @ Value::Object(_)) => Ok(p.clone()),
        Some(_) => Err("`params` must be an object".into()),
    }
}

/// Decode the supported initial project states.
fn parse_start(v: &Value) -> std::result::Result<Start, String> {
    match v {
        Value::String(s) if s == "new" => Ok(Start::New),
        Value::String(s) if s == "demo" => Ok(Start::Demo),
        Value::Object(o) => match o.get("project").and_then(Value::as_str) {
            Some(p) if !p.trim().is_empty() => Ok(Start::Project(p.to_string())),
            _ => Err("`start` object needs a `project` path".into()),
        },
        _ => Err("`start` must be \"new\", \"demo\" or {\"project\": path}".into()),
    }
}

/// One line → a step (`None`: a blank or comment line).
fn parse_line(text: &str, first: bool, allow_errors: &mut bool) -> Option<Step> {
    let t = text.trim();
    if t.is_empty() || t.starts_with("//") || t.starts_with('#') {
        return None;
    }
    let v: Value = match serde_json::from_str(t) {
        Ok(v) => v,
        Err(e) => return Some(Step::Invalid(format!("not JSON: {e}"))),
    };
    let Some(o) = v.as_object() else { return Some(Step::Invalid("a line must be a JSON object".into())) };
    let step = if let Some(st) = o.get("start") {
        if !first {
            return Some(Step::Invalid("the `start` line must come first".into()));
        }
        *allow_errors = o.get("allow_errors").and_then(Value::as_bool).unwrap_or(false);
        parse_start(st).map(Step::Start)
    } else if let Some(c) = o.get("command") {
        parse_command(c, o)
    } else if let Some(e) = o.get("expect") {
        parse_expect(e)
    } else if o.get("expect_no_error").and_then(Value::as_bool) == Some(true) {
        Ok(Step::ExpectNoError)
    } else if o.len() == 1 && o.contains_key("comment") {
        return None;
    } else {
        Err("expected a `start`, `command`, `expect` or `expect_no_error` line".to_string())
    };
    Some(step.unwrap_or_else(Step::Invalid))
}

/// Parse one command and its optional expected error without executing it.
fn parse_command(c: &Value, o: &Map<String, Value>) -> std::result::Result<Step, String> {
    let id = c.as_str().filter(|s| !s.is_empty()).ok_or("`command` must be a command id")?;
    let expect_error = match o.get("expect_error") {
        None | Some(Value::Null | Value::Bool(false)) => None,
        Some(Value::Bool(true)) => Some(None),
        Some(Value::String(s)) => Some(Some(s.clone())),
        Some(_) => return Err("`expect_error` must be true or a text the error contains".into()),
    };
    Ok(Step::Command { id: id.to_string(), params: params_of(o.get("params"))?, expect_error })
}

/// Parse a query assertion, retaining the distinction between no expected value and JSON null.
fn parse_expect(e: &Value) -> std::result::Result<Step, String> {
    let query = e.get("query").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or("`expect` needs a `query` command id")?;
    let pointer = e.get("pointer").map(|p| p.as_str().map(str::to_string).ok_or("`pointer` must be a JSON pointer such as /value")).transpose()?;
    Ok(Step::Expect { query: query.to_string(), params: params_of(e.get("params"))?, pointer, equals: e.get("equals").cloned() })
}

// ---------- running ----------

/// How to run a script.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Keep running after a failed step (default: stop at the first failure).
    pub keep_going: bool,
    /// Folder relative `start.project` paths are resolved against (the script's).
    pub base_dir: Option<PathBuf>,
}

/// What happened to one line.
#[derive(Clone, Debug, Serialize)]
pub struct StepReport {
    /// 1-based line number (0: the implicit start).
    pub line: usize,
    /// `start`, `command`, `expect`, `expect_no_error` or `invalid`.
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub panicked: bool,
}

/// The outcome of [`run`].
#[derive(Clone, Debug, Default, Serialize)]
pub struct Report {
    pub ok: bool,
    /// A step panicked (caught by the last-resort guard): always a failure.
    pub panicked: bool,
    pub steps: Vec<StepReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_failure: Option<StepReport>,
    /// Steps not run after the first failure.
    pub skipped: usize,
}

/// Cut a message to [`MAX_TEXT`] characters.
fn cut(s: &str) -> String {
    match s.char_indices().nth(MAX_TEXT) {
        Some((i, _)) => format!("{}…", s.get(..i).unwrap_or(s)),
        None => s.to_string(),
    }
}

/// Run one command the way the app does, noting whether it panicked.
fn exec(s: &mut Session, id: &str, params: Value) -> (Result<Value>, bool) {
    let before = guard::caught();
    let r = guard::guarded(&format!("replay of `{id}`"), || s.execute(id, params));
    s.events.clear();
    (r, guard::caught() != before)
}

/// Numbers within 1e-6 (relative above 1), everything else exactly.
fn approx_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match (x.as_f64(), y.as_f64()) {
            (Some(x), Some(y)) => (x - y).abs() <= 1e-6 * x.abs().max(y.abs()).max(1.0),
            _ => x == y,
        },
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(a, b)| approx_eq(a, b)),
        (Value::Object(x), Value::Object(y)) => x.len() == y.len() && x.iter().all(|(k, a)| y.get(k).is_some_and(|b| approx_eq(a, b))),
        _ => a == b,
    }
}

/// Marks this thread as replaying while alive.
struct Replaying(bool);

impl Replaying {
    /// Suppress automatic crash exports for the duration of a replay.
    fn enter() -> Replaying {
        Replaying(REPLAYING.with(|r| r.replace(true)))
    }
}

impl Drop for Replaying {
    fn drop(&mut self) {
        REPLAYING.with(|r| r.set(self.0));
    }
}

/// Create or open the initial project and retain whether its command panicked.
fn start_step(s: &mut Session, start: &Start, base: Option<&Path>) -> (String, Result<Value>, bool) {
    match start {
        Start::New => {
            let (r, p) = exec(s, "file.newProject", json!({}));
            ("file.newProject".into(), r, p)
        }
        Start::Demo => {
            let (r, p) = exec(s, "file.openDemoProject", json!({}));
            ("file.openDemoProject".into(), r, p)
        }
        Start::Project(path) => {
            let full = match base {
                Some(b) if Path::new(path).is_relative() => b.join(path).display().to_string(),
                _ => path.clone(),
            };
            let (r, p) = exec(s, "file.open", json!({"path": full}));
            ("file.open".into(), r, p)
        }
    }
}

/// Compare a query result and produce a bounded explanation when it fails.
fn expectation_error(r: Result<Value>, panicked: bool, pointer: Option<&str>, equals: Option<&Value>) -> Option<String> {
    let value = match r {
        Ok(v) => v,
        Err(e) => return Some(cut(&format!("{}{e}", if panicked { "panicked: " } else { "" }))),
    };
    let got = match pointer {
        Some(p) => value.pointer(p),
        None => Some(&value),
    };
    match (got, equals) {
        (None, _) => Some(cut(&format!("no value at {} in {value}", pointer.unwrap_or("")))),
        (Some(got), Some(want)) if !approx_eq(got, want) => Some(cut(&format!("expected {want}, got {got}"))),
        _ => None,
    }
}

/// Execute one parsed line and remember whether the last command failed for `expect_no_error`.
fn run_step(s: &mut Session, line: usize, step: Step, opts: &Options, allow_errors: bool, last_command_failed: &mut Option<bool>) -> StepReport {
    let mut sr = StepReport { line, kind: "command", command: None, ok: true, error: None, panicked: false };
    match step {
        Step::Invalid(m) => {
            sr.kind = "invalid";
            sr.ok = false;
            sr.error = Some(m);
        }
        Step::Start(start) => {
            sr.kind = "start";
            let (id, r, panicked) = start_step(s, &start, opts.base_dir.as_deref());
            sr.command = Some(id);
            sr.panicked = panicked;
            sr.ok = !panicked;
            if let Err(e) = r {
                sr.ok = false;
                sr.error = Some(cut(&e.to_string()));
            }
        }
        Step::Command { id, params, expect_error } => {
            let (r, panicked) = exec(s, &id, params);
            sr.command = Some(id);
            sr.panicked = panicked;
            *last_command_failed = Some(r.is_err());
            let err = r.err().map(|e| e.to_string());
            let (ok, error) = match (err, expect_error) {
                (e, _) if panicked => (false, Some(format!("panicked: {}", e.unwrap_or_else(|| "a step failed with an internal error".into())))),
                (None, None) => (true, None),
                (None, Some(_)) => (false, Some("expected an error, but the command succeeded".into())),
                (Some(e), Some(Some(want))) if !e.contains(want.as_str()) => (false, Some(format!("expected an error containing {want:?}, got: {e}"))),
                (Some(_), Some(_)) => (true, None),
                (Some(e), None) => (allow_errors, Some(e)),
            };
            sr.ok = ok;
            sr.error = error.as_deref().map(cut);
        }
        Step::Expect { query, params, pointer, equals } => {
            sr.kind = "expect";
            let (r, panicked) = exec(s, &query, params);
            sr.command = Some(query);
            sr.panicked = panicked;
            sr.error = expectation_error(r, panicked, pointer.as_deref(), equals.as_ref());
            sr.ok = sr.error.is_none() && !panicked;
        }
        Step::ExpectNoError => {
            sr.kind = "expect_no_error";
            match last_command_failed {
                Some(false) => {}
                Some(true) => {
                    sr.ok = false;
                    sr.error = Some("the previous command failed".into());
                }
                None => {
                    sr.ok = false;
                    sr.error = Some("no command before this line".into());
                }
            }
        }
    }
    sr
}

/// Run a replay script against `s`. Never panics: malformed lines are failed steps.
pub fn run(s: &mut Session, script: &str, opts: &Options) -> Report {
    let _replaying = Replaying::enter();
    let mut report = Report { ok: true, ..Default::default() };
    let fail = |report: &mut Report, sr: StepReport| {
        report.ok = false;
        report.panicked |= sr.panicked;
        if report.first_failure.is_none() {
            report.first_failure = Some(sr.clone());
        }
        report.steps.push(sr);
    };
    if script.len() > MAX_SCRIPT_BYTES {
        let msg = format!("the script is larger than {} MB", MAX_SCRIPT_BYTES >> 20);
        fail(&mut report, StepReport { line: 0, kind: "invalid", command: None, ok: false, error: Some(msg), panicked: false });
        return report;
    }
    // Hand-edited UTF-8 scripts may start with a BOM, especially on Windows.
    let script = script.strip_prefix('\u{feff}').unwrap_or(script);
    let mut allow_errors = false;
    let mut steps: Vec<(usize, Step)> = vec![];
    for (i, line) in script.lines().enumerate().take(MAX_LINES) {
        if let Some(step) = parse_line(line, steps.is_empty(), &mut allow_errors) {
            steps.push((i + 1, step));
        }
    }
    if script.lines().nth(MAX_LINES).is_some() {
        steps.push((MAX_LINES + 1, Step::Invalid(format!("the script has more than {MAX_LINES} lines; the rest was not read"))));
    }
    if !matches!(steps.first(), Some((_, Step::Start(_)) | (_, Step::Invalid(_)))) {
        steps.insert(0, (0, Step::Start(Start::New)));
    }
    let mut last_command_failed: Option<bool> = None;
    let mut stopped = false;
    for (line, step) in steps {
        if stopped {
            report.skipped += 1;
            continue;
        }
        let sr = run_step(s, line, step, opts, allow_errors, &mut last_command_failed);
        if sr.ok {
            report.steps.push(sr);
        } else {
            fail(&mut report, sr);
            stopped = !opts.keep_going;
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::{Options, run};
    use crate::Session;
    use crate::config::{ConfigStore, MemoryConfig};

    fn replay(script: &str) -> super::Report {
        run(&mut Session::new(), script, &Options::default())
    }

    #[test]
    fn steps_expectations_and_failures_are_reported() {
        let r = replay(
            r#"{"start": "new"}
// a comment
{"command": "comp.new", "params": {"name": "A", "width": 320, "height": 180, "frameRate": 30, "duration": 2}}
{"expect_no_error": true}
{"command": "comp.new", "params": {"frameRate": 0}, "expect_error": "frame"}
{"expect": {"query": "comp.info", "pointer": "/width", "equals": 320}}
"#,
        );
        assert!(r.ok, "{r:?}");
        assert_eq!(r.steps.len(), 5);
        let r = replay(
            "{\"command\": \"comp.new\"}\n{\"expect\": {\"query\": \"comp.info\", \"pointer\": \"/width\", \"equals\": 1}}\n{\"command\": \"comp.new\"}",
        );
        assert!(!r.ok && !r.panicked && r.skipped == 1, "{r:?}");
        assert_eq!(r.first_failure.as_ref().map(|f| f.line), Some(2));
        // An unexpected success, an unknown command.
        assert!(!replay(r#"{"command": "comp.new", "expect_error": true}"#).ok);
        assert!(!replay(r#"{"command": "no.such.command"}"#).ok);
        // Tolerated errors.
        let r = replay("{\"start\": \"new\", \"allow_errors\": true}\n{\"command\": \"no.such.command\"}\n{\"expect_no_error\": true}");
        assert!(!r.ok && r.first_failure.as_ref().map(|f| f.kind) == Some("expect_no_error"), "{r:?}");
    }

    /// A UTF-8 BOM belongs to the text file, not the first JSON step.
    #[test]
    fn utf8_bom_preserves_explicit_and_implicit_start_lines() {
        for body in ["{\"start\":\"new\"}\n{\"command\":\"comp.new\"}", "{\"command\":\"comp.new\"}"] {
            let script = format!("\u{feff}{body}");
            let report = replay(&script);
            assert!(report.ok, "{report:?}");
            assert_eq!(report.steps.last().unwrap().line, body.lines().count());
        }
    }

    #[test]
    fn malformed_lines_are_failures_not_crashes() {
        for bad in [
            "not json",
            "[1, 2]",
            "{\"command\": 5}",
            "{\"command\": \"comp.new\", \"params\": [1]}",
            "{\"command\": \"comp.new\", \"expect_error\": 3}",
            "{\"expect\": {}}",
            "{\"expect\": {\"query\": \"comp.info\", \"pointer\": 4}}",
            "{\"expect\": {\"query\": \"comp.info\", \"pointer\": \"/nope\"}}",
            "{\"start\": 7}",
            "{\"start\": {\"project\": \"\"}}",
            "{\"command\": \"comp.new\"}\n{\"start\": \"new\"}",
            "{\"what\": 1}",
            "{\"expect_no_error\": true}",
            "{\"start\": {\"project\": \"/no/such/file.ecproj\"}}",
        ] {
            let r = replay(bad);
            assert!(!r.ok && !r.panicked, "{bad}: {r:?}");
        }
        let r = replay(&format!("{{\"command\": \"{}\"}}", "\u{1F600}".repeat(1000)));
        assert!(!r.ok && r.first_failure.and_then(|f| f.error).is_some_and(|e| e.ends_with('…')));
        assert!(replay("").ok, "an empty script is a new project");
    }

    #[test]
    fn the_journal_exports_and_replays() {
        let mut s = Session::new();
        s.execute("comp.new", json!({"name": "Old"})).unwrap();
        s.execute("file.openDemoProject", json!({})).unwrap();
        s.execute("layer.newSolid", json!({"color": "#ff0000", "name": "Red"})).unwrap();
        s.execute("layer.newSolid", json!({"color": "#00ff00", "name": "Green"})).unwrap();
        // A query isn't journaled; neither is saving.
        s.execute("comp.info", json!({})).unwrap();
        let text = super::export(&s, None);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 4, "{text}");
        assert!(lines[0].contains("\"start\":\"new\"") && lines[1].contains("file.openDemoProject"), "starts at the last project open: {text}");
        let mut again = Session::new();
        let r = run(&mut again, &text, &Options::default());
        assert!(r.ok, "{r:?}");
        let layers = |s: &Session| s.active_comp().map(|c| c.layers.len());
        assert_eq!(layers(&again), layers(&s));
        // journal.export returns it; with `path` it writes the file.
        let v = s.execute("journal.export", json!({})).unwrap();
        assert_eq!(v["steps"], 3);
        assert_eq!(v["script"].as_str().unwrap().lines().skip(1).collect::<Vec<_>>(), lines[1..]);
        let dir = std::env::temp_dir().join(format!("ec-replay-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("j.jsonl");
        s.execute("journal.export", json!({"path": path.to_str().unwrap()})).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("layer.newSolid"));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(s.journal.iter().filter(|(id, _)| id.starts_with("journal.")).count(), 0);
    }

    #[test]
    fn help_export_repro_saves_into_the_settings_store() {
        let mut s = Session::new();
        assert!(s.execute("help.exportRepro", json!({})).is_err(), "no settings folder and no path");
        let store = Arc::new(MemoryConfig::default());
        s.config = Some(store.clone());
        s.execute("comp.new", json!({})).unwrap();
        let v = s.execute("help.exportRepro", json!({})).unwrap();
        assert_eq!(v["steps"], 1);
        let saved = store.list(super::REPRO_DIR);
        assert_eq!(saved.len(), 1, "{saved:?}");
        assert!(store.read(v["path"].as_str().unwrap()).unwrap().contains("comp.new"));
    }

    #[test]
    fn a_panicking_command_saves_a_crash_repro_and_replays_as_a_failure() {
        let mut s = Session::new();
        let store = Arc::new(MemoryConfig::default());
        s.config = Some(store.clone());
        s.execute("comp.new", json!({"name": "C"})).unwrap();
        // `test.panic` stands in for a command with a bug (see `commands::journal`).
        let e = s.execute("test.panic", json!({"why": "boom"})).unwrap_err().to_string();
        assert!(e.contains("boom") && e.contains("Repros/crash-"), "{e}");
        let names = store.list(super::REPRO_DIR);
        assert_eq!(names.len(), 1);
        let text = store.read(&format!("{}/{}", super::REPRO_DIR, names[0])).unwrap();
        assert!(text.lines().next().unwrap().contains("\"crash\""), "{text}");
        assert!(text.lines().last().unwrap().contains("test.panic"), "{text}");
        // The replay fails on the panic (and doesn't save another repro).
        let r = run(&mut Session::new(), &text, &Options::default());
        assert!(!r.ok && r.panicked, "{r:?}");
        assert_eq!(r.first_failure.as_ref().and_then(|f| f.command.as_deref()), Some("test.panic"));
        // A failing replay inside a session with a store saves nothing more.
        let mut t = Session::new();
        t.config = Some(store.clone());
        assert!(!run(&mut t, &text, &Options::default()).ok);
        assert_eq!(store.list(super::REPRO_DIR).len(), 1);
        // Only the newest crash repros are kept.
        for i in 0..30 {
            store.write(&format!("{}/crash-{i:012}-x.jsonl", super::REPRO_DIR), "{}").unwrap();
        }
        let _ = s.execute("test.panic", json!({}));
        assert_eq!(store.list(super::REPRO_DIR).len(), super::KEEP_CRASH_REPROS);
    }

    #[test]
    fn commands_run_by_a_journaled_command_are_not_journaled_twice() {
        let mut s = Session::new();
        s.execute("comp.new", json!({})).unwrap();
        // engine.batch isn't journaled: its steps are.
        s.execute("engine.batch", json!({"steps": [{"command": "layer.newNull"}, {"command": "layer.newNull"}]})).unwrap();
        assert_eq!(s.journal.iter().filter(|(id, _)| id == "layer.newNull").count(), 2);
        // A journaled command's own steps replay with it.
        s.execute("file.runScript", json!({"steps": [{"command": "layer.newNull"}]})).unwrap();
        let ids: Vec<&str> = s.journal.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, ["comp.new", "layer.newNull", "layer.newNull", "file.runScript"]);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod revert_tests {
    use super::{Options, export, run};
    use crate::Session;
    use serde_json::json;

    /// Saving is omitted, so a successful Revert must keep the file it read as an open boundary.
    #[test]
    fn save_as_then_revert_exports_a_replayable_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("saved.ecproj");
        let mut s = Session::new();
        s.execute("comp.new", json!({"name": "Saved"})).unwrap();
        s.execute("file.saveAs", json!({"path": path})).unwrap();
        s.execute("layer.newNull", json!({})).unwrap();
        s.execute("file.revert", json!({})).unwrap();
        s.execute("layer.newNull", json!({"name": "After Revert"})).unwrap();
        let script = export(&s, None);
        assert!(!script.contains("file.save") && !script.contains("file.revert"), "{script}");
        assert!(script.lines().nth(1).unwrap().contains("file.open"), "{script}");
        let saved = std::fs::read(&path).unwrap();
        let mut again = Session::new();
        let report = run(&mut again, &script, &Options::default());
        assert!(report.ok, "{report:?}");
        assert_eq!(again.path, s.path);
        assert_eq!(again.active_comp().unwrap().layers.len(), s.active_comp().unwrap().layers.len());
        assert_eq!(saved, std::fs::read(path).unwrap(), "replay must not rewrite the saved project");
    }
}
