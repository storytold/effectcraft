//! The boa runtime: native functions (`__exec`, `__query`, `__print`, `__undo`, `__task`,
//! `__file`), the session hand-off while a script runs, undo groups and error reporting.

use std::cell::RefCell;
use std::mem::ManuallyDrop;
use std::sync::Arc;

use boa_engine::{Context, JsNativeError, JsResult, JsString, JsValue, NativeFunction, Source, js_string};
use effectcraft_engine::project::Project;
use effectcraft_engine::{ScriptRequest, Session};
use serde_json::{Value as J, json};

const PRELUDE: &str = include_str!("prelude.js");

/// Engine commands that create layers: like After Effects, scripts add new layers at the top of
/// the stack (the UI inserts above the selection).
const CREATES_LAYER: &[&str] =
    &["layer.newSolid", "layer.newNull", "layer.newText", "layer.newShape", "layer.newAdjustment", "layer.newCamera", "layer.newLight", "layer.addItem"];

/// What a script run produced.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    /// The value of the script's last expression statement (JSON when it converts; model objects
    /// as strings like `"[object CompItem]"`).
    pub result: J,
    /// `writeLn` / `$.writeln` / `alert` output, one entry per line.
    pub output: Vec<String>,
    pub error: Option<ScriptError>,
    /// The script is waiting in a modal ScriptUI dialog's `show()` (it goes on when the dialog
    /// closes; see [`crate::dispatch_ui`]).
    pub waiting: bool,
}

/// An uncaught script error.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScriptError {
    pub message: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub file: String,
}

impl Outcome {
    pub fn ok(&self) -> bool {
        self.error.is_none()
    }
    pub fn to_json(&self) -> J {
        let mut j = json!({
            "ok": self.ok(),
            "result": self.result,
            "output": self.output.join("\n"),
            "error": self.error.as_ref().map(|e| json!({"message": e.message, "line": e.line, "column": e.column, "file": e.file})),
        });
        if self.waiting {
            j["waiting"] = json!(true);
        }
        j
    }
}

/// The session and per-run state while a script runs (the natives reach it here; nothing
/// borrowed ever enters the JS heap).
pub(crate) struct Active {
    pub(crate) session: Session,
    pub(crate) output: Vec<String>,
    /// Open undo groups: (name, project snapshot when the outermost group began).
    pub(crate) groups: Vec<(String, Arc<Project>)>,
    /// `app.scheduleTask` queue: (id, code, repeat).
    pub(crate) tasks: Vec<(u32, String, bool)>,
    pub(crate) next_task: u32,
    pub(crate) name: String,
    /// Running on a script-host thread: modal dialogs hand the session back to the caller
    /// through this link while they wait.
    pub(crate) link: Option<crate::ui::Link>,
}

impl Active {
    pub(crate) fn new(session: Session, name: &str) -> Active {
        Active { session, output: vec![], groups: vec![], tasks: vec![], next_task: 0, name: name.to_string(), link: None }
    }
}

thread_local! {
    pub(crate) static ACTIVE: RefCell<Option<Active>> = const { RefCell::new(None) };
    /// The Script Console's persistent context (never dropped, like the expression runtime: boa's
    /// own thread-local heap may be torn down first at thread exit).
    pub(crate) static CONSOLE: RefCell<Option<ManuallyDrop<Context>>> = const { RefCell::new(None) };
}

pub(crate) fn with_active<T>(f: impl FnOnce(&mut Active) -> Result<T, String>) -> Result<T, String> {
    ACTIVE.with(|a| match a.try_borrow_mut() {
        Ok(mut g) => match g.as_mut() {
            Some(act) => f(act),
            None => Err("no script is running".into()),
        },
        Err(_) => Err("scripts can't run scripts".into()),
    })
}

pub(crate) fn throw(msg: impl Into<String>) -> boa_engine::JsError {
    JsNativeError::error().with_message(msg.into()).into()
}

pub(crate) fn arg_string(args: &[JsValue], i: usize, ctx: &mut Context) -> JsResult<String> {
    Ok(args.get(i).cloned().unwrap_or_default().to_string(ctx)?.to_std_string_escaped())
}

fn arg_json(args: &[JsValue], i: usize, ctx: &mut Context) -> JsResult<J> {
    let s = arg_string(args, i, ctx)?;
    if s.is_empty() || s == "undefined" {
        return Ok(json!({}));
    }
    serde_json::from_str(&s).map_err(|e| throw(format!("bad parameters: {e}")))
}

pub(crate) fn js_str(s: &str) -> JsValue {
    JsValue::from(JsString::from(s))
}

/// `__exec(commandId, paramsJson)` → result JSON. Runs an engine command (undoable) with the
/// target comp made active for its duration.
fn native_exec(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let id = arg_string(args, 0, ctx)?;
    let mut params = arg_json(args, 1, ctx)?;
    if id == "script.run" || id == "file.runScript" {
        return Err(throw("scripts can't run scripts"));
    }
    let keys = params.as_object_mut().and_then(|o| o.remove("__keys"));
    let r = with_active(|a| {
        Ok(match &keys {
            Some(k) => exec_on_keys(&mut a.session, &id, params, k),
            None => exec(&mut a.session, &id, params),
        })
    });
    match r {
        Ok(Ok(v)) => Ok(js_str(&v.to_string())),
        Ok(Err(e)) | Err(e) => Err(throw(e)),
    }
}

/// Run a command for a script: the `comp` it targets is active while it runs (commands act on
/// the active comp), then the user's active comp comes back.
pub(crate) fn exec(s: &mut Session, id: &str, params: J) -> Result<J, String> {
    use effectcraft_engine::project::ItemId;
    let target = params.get("comp").and_then(J::as_u64).map(ItemId).filter(|c| s.project.comp(*c).is_some());
    let prev = s.state.active_comp;
    if let Some(c) = target
        && prev != Some(c)
    {
        s.state.active_comp = Some(c);
    }
    if CREATES_LAYER.contains(&id) {
        s.state.selected_layers.clear();
    }
    let r = effectcraft_engine::prefs::with_script_origin(|| s.execute(id, params));
    if let Some(c) = target
        && prev != Some(c)
        && s.state.active_comp == Some(c)
        && id != "comp.open"
    {
        match prev {
            Some(p) if s.project.comp(p).is_some() => s.state.active_comp = Some(p),
            _ => {
                if !s.state.open_comps.contains(&c) {
                    s.state.open_comps.push(c);
                }
            }
        }
    }
    r.map_err(|e| e.to_string())
}

/// Run a keyframe command that acts on the selected keys (`keys.velocity`, `keys.interpolation`,
/// `keys.delete`) on the keys `targets` names instead (`[{layer, prop, index}]`, 0-based, in the
/// command's comp): After Effects' per-key methods (`setTemporalEaseAtKey`, `removeKey`…) leave
/// the user's key selection as it was. Selected keys that no longer exist drop out of it.
fn exec_on_keys(s: &mut Session, id: &str, params: J, targets: &J) -> Result<J, String> {
    use effectcraft_engine::KeyRef;
    use effectcraft_engine::project::{ItemId, LayerId};
    let cid = params.get("comp").and_then(J::as_u64).map(ItemId).or(s.state.active_comp).ok_or("no composition is open")?;
    let comp = s.project.comp(cid).ok_or("the composition no longer exists")?;
    let mut keys = vec![];
    for t in targets.as_array().into_iter().flatten() {
        let (Some(layer), Some(prop), Some(index)) = (t["layer"].as_u64(), t["prop"].as_u64(), t["index"].as_u64()) else {
            return Err(format!("bad keyframe reference {t}"));
        };
        let l = comp.layer(LayerId(layer)).ok_or("the layer no longer exists")?;
        if l.switches.locked {
            return Err(format!("Can not change keyframes of \"{}\": the layer is locked", l.name));
        }
        let key = l.props.find(prop).and_then(|pr| pr.keys.get(usize::try_from(index).ok()?)).ok_or("the keyframe no longer exists")?;
        keys.push(KeyRef { layer: LayerId(layer), prop, time: key.time });
    }
    let saved = std::mem::replace(&mut s.state.selected_keys, keys);
    let r = exec(s, id, params);
    let comp = s.active_comp();
    let exists = |k: &KeyRef| comp.and_then(|c| c.layer(k.layer)).and_then(|l| l.props.find(k.prop)).is_some_and(|pr| pr.keys.iter().any(|x| x.time == k.time));
    s.state.selected_keys = saved.into_iter().filter(exists).collect();
    r
}

/// `__query(kind, json)` → JSON.
fn native_query(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let kind = arg_string(args, 0, ctx)?;
    let a = arg_json(args, 1, ctx)?;
    let r = with_active(|act| match kind.as_str() {
        "app" => Ok(json!({
            "version": format!("26.0 (EffectCraft {})", env!("CARGO_PKG_VERSION")),
            "buildName": env!("CARGO_PKG_VERSION"),
            "scriptName": act.name,
            "allowFiles": act.session.prefs.scripting.allow_scripts_write_files,
            "projectDir": project_dir(&act.session),
        })),
        _ => crate::model::query(&mut act.session, &kind, &a),
    });
    match r {
        Ok(v) => Ok(js_str(&v.to_string())),
        Err(e) => Err(throw(e)),
    }
}

/// `__print(text)`: `writeLn`, `$.writeln`, `alert`.
fn native_print(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let text = arg_string(args, 0, ctx)?;
    let _ = with_active(|a| {
        a.output.extend(text.split('\n').map(str::to_string));
        Ok(())
    });
    Ok(JsValue::undefined())
}

/// `__undo("begin" | "end", name)`: After Effects' `app.beginUndoGroup` / `endUndoGroup`. Every
/// command run inside the outermost group folds into one undo step named after the group.
fn native_undo(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let op = arg_string(args, 0, ctx)?;
    let name = arg_string(args, 1, ctx)?;
    with_active(|a| {
        match op.as_str() {
            "begin" => {
                let snap = a.session.project.clone();
                a.groups.push((if name.is_empty() || name == "undefined" { "Script".into() } else { name }, snap));
            }
            _ => {
                if a.groups.len() == 1 {
                    let (name, snap) = a.groups.pop().unwrap_or_default();
                    collapse(&mut a.session, &name, &snap);
                } else {
                    a.groups.pop();
                }
            }
        }
        Ok(())
    })
    .map_err(throw)?;
    Ok(JsValue::undefined())
}

/// Fold the undo steps recorded since the project was `snap` into one step called `name`.
pub(crate) fn collapse(s: &mut Session, name: &str, snap: &Arc<Project>) {
    let h = &mut s.history;
    let Some(start) = h.undo.iter().rposition(|(_, p)| Arc::ptr_eq(p, snap)) else {
        // Nothing was recorded (or history was reset by New / Open Project).
        return;
    };
    let first = h.undo[start].1.clone();
    h.undo.truncate(start);
    h.undo.push((name.to_string(), first));
    h.merge_key = None;
}

/// `__task("schedule", code, repeat)` → id; `__task("cancel", id)`.
fn native_task(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let op = arg_string(args, 0, ctx)?;
    if op == "cancel" {
        let id = args.get(1).cloned().unwrap_or_default().to_number(ctx)? as u32;
        let _ = with_active(|a| {
            a.tasks.retain(|t| t.0 != id);
            Ok(())
        });
        return Ok(JsValue::undefined());
    }
    let code = arg_string(args, 1, ctx)?;
    let repeat = args.get(2).cloned().unwrap_or_default().to_boolean();
    let id = with_active(|a| {
        a.next_task += 1;
        let id = a.next_task;
        a.tasks.push((id, code, repeat));
        Ok(id)
    })
    .map_err(throw)?;
    Ok(JsValue::from(id))
}

fn project_dir(s: &Session) -> Option<String> {
    let p = s.path.as_deref()?;
    std::path::Path::new(p).parent().map(|d| d.to_string_lossy().to_string())
}

/// Lexically normalized absolute path (`a/./b/../c` → `a/c`).
fn normalize(p: &str) -> std::path::PathBuf {
    use std::path::{Component, Path, PathBuf};
    let path = Path::new(p);
    let abs = if path.is_absolute() { path.to_path_buf() } else { std::env::current_dir().unwrap_or_default().join(path) };
    let mut out = PathBuf::new();
    for c in abs.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            c => out.push(c),
        }
    }
    // Resolve symlinks where the file (or its folder) exists, so links can't escape the project.
    std::fs::canonicalize(&out)
        .or_else(|_| out.parent().map(std::fs::canonicalize).unwrap_or(Ok(out.clone())).map(|d| d.join(out.file_name().unwrap_or_default())))
        .unwrap_or(out)
}

pub(crate) const GATE: &str = "Preferences ▸ Scripting & Expressions ▸ Allow Scripts to Write Files and Access Network";

/// The file-access gate: reads under the project's folder are always allowed; other reads, all
/// writes and listing other folders need the Allow Scripts to Write Files preference.
fn check_access(s: &Session, path: &str, write: bool) -> Result<std::path::PathBuf, String> {
    let p = normalize(path);
    if s.prefs.scripting.allow_scripts_write_files {
        return Ok(p);
    }
    if write {
        return Err(format!("Unable to write {path}: scripts can't write files unless {GATE} is on"));
    }
    if let Some(dir) = project_dir(s) {
        let d = normalize(&dir);
        if p.starts_with(&d) {
            return Ok(p);
        }
    }
    Err(format!("Unable to access {path}: scripts can only read files in the project's folder unless {GATE} is on"))
}

/// `__file(op, path, data)`: `read` → text, `write`/`append` → bytes, `exists` → bool, `list` →
/// names, `mkdir`, `remove`, `isDir`.
fn native_file(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let op = arg_string(args, 0, ctx)?;
    let path = arg_string(args, 1, ctx)?;
    let data = if args.len() > 2 { arg_string(args, 2, ctx)? } else { String::new() };
    let r: Result<J, String> = with_active(|a| {
        // File.execute(): opened with its default application through `file.executeFile`
        // (Allow Scripts to Write Files gate, Warn User When Executing Files confirmation).
        if op == "execute" {
            return exec(&mut a.session, "file.executeFile", json!({"path": path})).map(|v| json!(v.get("opened").is_some()));
        }
        let s = &a.session;
        match op.as_str() {
            "access" => Ok(json!(check_access(s, &path, false)?.exists())),
            "exists" | "isDir" => {
                // Unreadable locations report "missing" instead of failing.
                let Ok(p) = check_access(s, &path, false) else { return Ok(json!(false)) };
                Ok(json!(if op == "exists" { p.exists() } else { p.is_dir() }))
            }
            "read" => {
                let p = check_access(s, &path, false)?;
                let bytes = s.services.read_file(&p.to_string_lossy()).map_err(|e| format!("Unable to read {path}: {e}"))?;
                Ok(json!(String::from_utf8_lossy(&bytes)))
            }
            "write" | "append" => {
                let p = check_access(s, &path, true)?;
                let ps = p.to_string_lossy().to_string();
                let mut bytes = if op == "append" { s.services.read_file(&ps).unwrap_or_default() } else { vec![] };
                bytes.extend_from_slice(data.as_bytes());
                s.services.write_file(&ps, &bytes).map_err(|e| format!("Unable to write {path}: {e}"))?;
                Ok(json!(bytes.len()))
            }
            "list" => {
                let p = check_access(s, &path, false)?;
                let mut names: Vec<String> = std::fs::read_dir(&p)
                    .map_err(|e| format!("Unable to list {path}: {e}"))?
                    .filter_map(|e| e.ok())
                    .map(|e| e.path().to_string_lossy().to_string())
                    .collect();
                names.sort();
                Ok(json!(names))
            }
            "mkdir" => {
                let p = check_access(s, &path, true)?;
                std::fs::create_dir_all(&p).map_err(|e| format!("Unable to create {path}: {e}"))?;
                Ok(json!(true))
            }
            "remove" => {
                let p = check_access(s, &path, true)?;
                let r = if p.is_dir() { std::fs::remove_dir(&p) } else { std::fs::remove_file(&p) };
                Ok(json!(r.is_ok()))
            }
            "network" => Err(if s.prefs.scripting.allow_scripts_write_files {
                "network access from scripts is not supported in EffectCraft".to_string()
            } else {
                format!("scripts can't access the network unless {GATE} is on")
            }),
            _ => Err(format!("unknown file operation `{op}`")),
        }
    });
    match r {
        Ok(v) => Ok(js_str(&v.to_string())),
        Err(e) => Err(throw(e)),
    }
}

pub(crate) fn new_context() -> Result<Context, String> {
    let mut ctx = Context::default();
    let limits = ctx.runtime_limits_mut();
    limits.set_loop_iteration_limit(200_000_000);
    limits.set_recursion_limit(2000);
    let natives: [(JsString, usize, fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>); 10] = [
        (js_string!("__exec"), 2, native_exec),
        (js_string!("__query"), 2, native_query),
        (js_string!("__print"), 1, native_print),
        (js_string!("__undo"), 2, native_undo),
        (js_string!("__task"), 3, native_task),
        (js_string!("__file"), 3, native_file),
        (js_string!("__sock"), 3, crate::socket::native_sock),
        (js_string!("__uiNewId"), 0, crate::ui::native_new_id),
        (js_string!("__uiModal"), 1, crate::ui::native_modal),
        (js_string!("__uiLayoutNative"), 1, crate::ui::native_layout),
    ];
    for (name, len, f) in natives {
        ctx.register_global_callable(name, len, NativeFunction::from_fn_ptr(f)).map_err(|e| e.to_string())?;
    }
    ctx.eval(Source::from_bytes(PRELUDE)).map_err(|e| format!("script prelude: {e}"))?;
    ctx.eval(Source::from_bytes(crate::ui::SCRIPTUI)).map_err(|e| format!("ScriptUI prelude: {e}"))?;
    Ok(ctx)
}

/// Turn a boa error into a message with line and column (`code` is the user's script).
pub(crate) fn script_error(e: &boa_engine::JsError, ctx: &mut Context, file: &str, code: &str) -> ScriptError {
    // Thrown values: `throw new Error("x")`, `throw "x"`, native errors.
    let display = e.to_string();
    let message = match e.as_opaque() {
        Some(v) => {
            let o = v.as_object();
            let msg = o.as_ref().and_then(|o| o.get(js_string!("message"), ctx).ok()).filter(|m| !m.is_undefined());
            let name = o.as_ref().and_then(|o| o.get(js_string!("name"), ctx).ok()).filter(|m| !m.is_undefined());
            match (name, msg) {
                (Some(n), Some(m)) => {
                    let n = n.to_string(ctx).map(|s| s.to_std_string_escaped()).unwrap_or_default();
                    let m = m.to_string(ctx).map(|s| s.to_std_string_escaped()).unwrap_or_default();
                    if n == "Error" { m } else { format!("{n}: {m}") }
                }
                _ => v.to_string(ctx).map(|s| s.to_std_string_escaped()).unwrap_or_else(|_| display.clone()),
            }
        }
        None => {
            let first = display.lines().next().unwrap_or("").trim();
            let first = first.split(" at line ").next().unwrap_or(first).trim();
            let first = match first.rfind(" (") {
                Some(i) if first.ends_with(')') && first[i..].contains(':') => first[..i].trim(),
                _ => first,
            };
            first.strip_prefix("Error: ").unwrap_or(first).to_string()
        }
    };
    let (line, column) = position(&display, file, code);
    ScriptError { message, line, column, file: file.to_string() }
}

/// Line and column of an error: `at line N, col M` (syntax errors), else the innermost backtrace
/// frame in the user's script (`file:line:col`), else (names boa can't place) the first use of
/// the undefined name in `code`.
fn position(msg: &str, file: &str, code: &str) -> (Option<u32>, Option<u32>) {
    if let Some(i) = msg.find("at line ") {
        let rest = &msg[i + 8..];
        let line = rest.split(|c: char| !c.is_ascii_digit()).next().and_then(|n| n.parse().ok());
        let col = rest.find("col ").and_then(|j| rest[j + 4..].split(|c: char| !c.is_ascii_digit()).next()).and_then(|n| n.parse().ok());
        return (line, col);
    }
    // `(file:12:5)`: the digits after the file name.
    let pick = |l: &str| -> Option<(u32, u32)> {
        let i = l.find(&format!("({file}:"))? + file.len() + 2;
        let mut it = l[i..].split([':', ')']);
        Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
    };
    let frames: Vec<&str> = msg.lines().skip(1).collect();
    if let Some((a, b)) = frames.iter().find_map(|l| pick(l)).or_else(|| msg.lines().next().and_then(pick)) {
        return (Some(a), Some(b));
    }
    // `ReferenceError: name is not defined` without a position.
    let first = msg.lines().next().unwrap_or("");
    if let Some(name) = first.split("ReferenceError: ").nth(1).and_then(|r| r.split(" is not defined").next()) {
        for (n, line) in code.lines().enumerate() {
            let b = line.as_bytes();
            let mut from = 0;
            while let Some(i) = line[from..].find(name).map(|i| i + from) {
                let before = i.checked_sub(1).map(|j| b[j]);
                let after = b.get(i + name.len()).copied();
                let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'$';
                if !before.is_some_and(|c| ident(c) || c == b'.') && !after.is_some_and(ident) {
                    return (Some(n as u32 + 1), Some(i as u32 + 1));
                }
                from = i + name.len();
            }
        }
    }
    (None, None)
}

/// Evaluate `code` in `ctx` and convert the result.
pub(crate) fn eval_in(ctx: &mut Context, code: &str, file: &str) -> Result<J, ScriptError> {
    let src = Source::from_bytes(code.as_bytes()).with_path(std::path::Path::new(file));
    let script = boa_engine::Script::parse(src, None, ctx).map_err(|e| script_error(&e, ctx, file, code))?;
    let v = script.evaluate(ctx).map_err(|e| script_error(&e, ctx, file, code))?;
    Ok(result_json(&v, ctx))
}

/// The script's completion value as JSON (`__result` in the prelude does the conversion).
fn result_json(v: &JsValue, ctx: &mut Context) -> J {
    if v.is_undefined() {
        return J::Null;
    }
    let global = ctx.global_object();
    let Some(f) = global.get(js_string!("__result"), ctx).ok().and_then(|f| f.as_object()) else { return J::Null };
    match f.call(&JsValue::undefined(), std::slice::from_ref(v), ctx) {
        Ok(s) if s.is_string() => {
            let s = s.to_string(ctx).map(|s| s.to_std_string_escaped()).unwrap_or_default();
            serde_json::from_str(&s).unwrap_or(J::String(s))
        }
        _ => J::Null,
    }
}

/// Restores the caller's session when a run ends (on every exit path).
pub(crate) struct Restore<'a> {
    pub(crate) target: &'a mut Session,
}

impl Drop for Restore<'_> {
    fn drop(&mut self) {
        if let Some(a) = ACTIVE.with(|c| c.borrow_mut().take()) {
            *self.target = a.session;
        }
    }
}

/// End of a script step: close undo groups left open, drop selections that no longer exist,
/// and take the output so far.
pub(crate) fn settle() -> Vec<String> {
    with_active(|a| {
        if let Some((name, snap)) = a.groups.first().cloned() {
            collapse(&mut a.session, &name, &snap);
            a.groups.clear();
        }
        a.session.sanitize_state();
        Ok(std::mem::take(&mut a.output))
    })
    .unwrap_or_default()
}

/// Evaluate a script file's code, then the one-shot tasks it scheduled (`app.scheduleTask`).
pub(crate) fn eval_with_tasks(ctx: &mut Context, code: &str, file: &str) -> Result<J, ScriptError> {
    let mut r = eval_in(ctx, code, file);
    let tasks = with_active(|a| Ok(std::mem::take(&mut a.tasks))).unwrap_or_default();
    for (_, code, repeat) in tasks {
        if repeat {
            continue;
        }
        if let Err(e) = eval_in(ctx, &code, "scheduled task")
            && r.is_ok()
        {
            r = Err(e);
        }
    }
    r
}

pub(crate) fn outcome(output: Vec<String>, result: Result<J, ScriptError>) -> Outcome {
    let mut out = Outcome { output, ..Default::default() };
    match result {
        Ok(v) => out.result = v,
        Err(e) => out.error = Some(e),
    }
    out
}

/// Run a script against `session`: the object model sees and edits that session through engine
/// commands. Script errors come back in [`Outcome::error`].
///
/// On native builds a script file runs on its own script-host thread, so a ScriptUI dialog's
/// `show()` can wait for the user (the session comes back here meanwhile, and the dialog's
/// events reach the host through [`crate::dispatch_ui`]); palettes and panels keep their host
/// alive for their handlers. The Script Console (and the web build) run inline, where dialogs
/// don't block.
pub fn run(session: &mut Session, req: &ScriptRequest) -> Outcome {
    if let Err(message) = check_nesting(req.code) {
        return Outcome { error: Some(ScriptError { message, file: req.name.into(), ..Default::default() }), ..Default::default() };
    }
    let running = ACTIVE.with(|a| a.try_borrow().map(|g| g.is_some()).unwrap_or(true));
    if running {
        return Outcome {
            error: Some(ScriptError { message: "scripts can't run scripts".into(), file: req.name.into(), ..Default::default() }),
            ..Default::default()
        };
    }
    #[cfg(not(target_arch = "wasm32"))]
    if !req.console {
        return crate::ui::run_threaded(session, req);
    }
    run_inline(session, req)
}

/// The deepest bracket nesting a script may have. The JavaScript parser recurses per level and
/// overflows the stack (aborting the app, which can't be caught) at about 200 levels on an
/// 8 MiB thread such as the Script Console's, so deeper scripts are rejected up front.
pub const MAX_NESTING: usize = 96;

/// An error when `code` nests brackets deeper than [`MAX_NESTING`] (strings, template literals
/// and comments are skipped).
pub fn check_nesting(code: &str) -> Result<(), String> {
    let b = code.as_bytes();
    let (mut i, mut depth, mut max) = (0usize, 0usize, 0usize);
    while let Some(&c) = b.get(i) {
        match c {
            b'(' | b'[' | b'{' => {
                depth += 1;
                max = max.max(depth);
            }
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b'"' | b'\'' | b'`' => {
                // Skip to the closing quote (escapes skip a byte).
                i += 1;
                while let Some(&d) = b.get(i) {
                    if d == b'\\' {
                        i += 1;
                    } else if d == c || (d == b'\n' && c != b'`') {
                        break;
                    }
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while b.get(i).is_some_and(|d| *d != b'\n') {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i < b.len() && !(b.get(i) == Some(&b'*') && b.get(i + 1) == Some(&b'/')) {
                    i += 1;
                }
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    if max > MAX_NESTING {
        return Err(format!("Error: the script is nested too deeply ({max} levels of brackets; at most {MAX_NESTING})"));
    }
    Ok(())
}

/// Run on this thread (the Script Console's persistent context, or a fresh one).
pub(crate) fn run_inline(session: &mut Session, req: &ScriptRequest) -> Outcome {
    let host = if req.console { crate::ui::CONSOLE_HOST } else { crate::ui::new_host_id() };
    let s = std::mem::take(session);
    ACTIVE.with(|a| *a.borrow_mut() = Some(Active::new(s, req.name)));
    let guard = Restore { target: session };
    let file = if req.name.is_empty() { "script" } else { req.name };
    let (result, windows) = if req.console {
        CONSOLE.with(|c| {
            let mut c = c.borrow_mut();
            if c.is_none() {
                match new_context() {
                    Ok(ctx) => *c = Some(ManuallyDrop::new(ctx)),
                    Err(e) => return (Err(ScriptError { message: e, file: file.into(), ..Default::default() }), vec![]),
                }
            }
            let Some(ctx) = c.as_mut().map(|m| &mut **m) else {
                return (Err(ScriptError { message: "the console context is unavailable".into(), file: file.into(), ..Default::default() }), vec![]);
            };
            let r = eval_in(ctx, req.code, file);
            (r, crate::ui::snapshot(ctx))
        })
    } else {
        match new_context() {
            Ok(mut ctx) => {
                let r = eval_with_tasks(&mut ctx, req.code, file);
                let windows = crate::ui::snapshot(&mut ctx);
                if windows.iter().any(|w| w.visible) {
                    // Palettes and panels outlive the run: keep their context for the handlers.
                    crate::ui::keep_inline(host, ctx);
                }
                (r, windows)
            }
            Err(e) => (Err(ScriptError { message: e, file: file.into(), ..Default::default() }), vec![]),
        }
    };
    let output = settle();
    drop(guard);
    crate::ui::publish(session, host, req.name, windows);
    outcome(output, result)
}

#[cfg(test)]
mod nesting_tests {
    use super::*;

    #[test]
    fn deep_nesting_is_an_error_not_a_stack_overflow() {
        // 100 000 nested brackets overflowed the parser's stack and aborted the process.
        let mut s = Session::default();
        for code in ["(".repeat(100_000) + "1" + &")".repeat(100_000), "[".repeat(100_000), "{".repeat(MAX_NESTING + 1)] {
            let out = run(&mut s, &ScriptRequest { code: &code, name: "deep", console: true });
            assert!(out.error.is_some_and(|e| e.message.contains("nested too deeply")));
        }
        // Brackets in strings and comments don't count; ordinary nesting runs.
        let quoted = format!("var s = '{}'; // {}\n/* {} */ 1 + 1", "(".repeat(500), "[".repeat(500), "{".repeat(500));
        assert!(check_nesting(&quoted).is_ok());
        let out = run(&mut s, &ScriptRequest { code: &quoted, name: "ok", console: true });
        assert!(out.error.is_none(), "{:?}", out.error);
        assert_eq!(out.result, serde_json::json!(2));
        assert!(check_nesting(&("(".repeat(MAX_NESTING) + &")".repeat(MAX_NESTING))).is_ok());
    }
}
