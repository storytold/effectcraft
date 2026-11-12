//! `app.scheduleTask` / `app.cancelTask` (#773): repeating tasks run whenever they are due (frontends
//! tick [`crate::run_tasks`] every frame) until they are cancelled, throw, or their owner goes
//! away; one-shot tasks scheduled later run when due too.

use std::sync::Arc;

use effectcraft_engine::{ScriptRequest, Session};
use serde_json::{Value, json};
use web_time::{Duration, Instant};

use crate::run_code;
use crate::tasks::run_tasks_at;

fn session() -> Session {
    let mut s = Session { expr: Some(Arc::new(effectcraft_expr::Expressions)), ..Default::default() };
    crate::install(&mut s);
    s.config = Some(Arc::new(effectcraft_engine::config::MemoryConfig::default()));
    s
}

fn ok(s: &mut Session, code: &str) {
    let o = run_code(s, code, "tasks.jsx");
    assert!(o.error.is_none(), "script failed: {:?}\noutput: {:?}", o.error, o.output);
}

fn console(s: &mut Session, code: &str) -> crate::Outcome {
    let o = crate::run(s, &ScriptRequest { code, name: "console", console: true });
    assert!(o.error.is_none(), "console failed: {:?}", o.error);
    o
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn output(r: &Value) -> Vec<String> {
    r["output"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect()
}

fn comps(s: &Session) -> usize {
    s.project.items.values().filter(|i| i.as_comp().is_some()).count()
}

#[test]
fn a_repeating_task_runs_every_interval_until_cancelled() {
    let mut s = session();
    ok(
        &mut s,
        "var n = 0;\nvar id = app.scheduleTask(\"n++; writeLn('tick ' + n); app.project.items.addComp('C' + n, 64, 64, 1, 1, 25); if (n == 3) app.cancelTask(id);\", 1000, true);",
    );
    let t0 = Instant::now();
    // Not due yet: nothing runs, and the frontend learns when to come back.
    let r = run_tasks_at(&mut s, t0);
    assert_eq!(r["ran"], json!(0));
    assert!(r["next"].as_u64().is_some_and(|n| n <= 1000), "{r}");
    let r = run_tasks_at(&mut s, t0 + ms(1500));
    assert_eq!((r["ran"].clone(), output(&r)), (json!(1), vec!["tick 1".to_string()]), "{r}");
    assert_eq!(comps(&s), 1, "the task edits the project through engine commands");
    // Due again an interval after it ran, once per pass (no catch-up burst).
    assert_eq!(run_tasks_at(&mut s, t0 + ms(1500))["ran"], json!(0));
    assert_eq!(run_tasks_at(&mut s, t0 + ms(2600))["ran"], json!(1));
    let r = run_tasks_at(&mut s, t0 + ms(9000));
    assert_eq!(output(&r), vec!["tick 3"]);
    // Cancelled: the script (no windows, no tasks) has ended.
    let r = run_tasks_at(&mut s, t0 + ms(20_000));
    assert_eq!((r["ran"].clone(), r["next"].clone()), (json!(0), Value::Null), "{r}");
    assert_eq!(comps(&s), 3);
    assert!(s.history.undo.len() >= 3, "every run is undoable");
}

#[test]
fn a_panels_task_refreshes_it_until_the_panel_closes() {
    let mut s = session();
    ok(
        &mut s,
        r#"
var w = new Window("palette", "Clock");
var label = w.add("statictext", undefined, "0");
var n = 0;
app.scheduleTask("n++; label.text = String(n);", 200, true);
w.show();
"#,
    );
    let win = s.script_ui.windows[0].id;
    let text = |s: &Session| s.script_ui.window(win).map(|w| w.root.children[0].text.clone());
    let t0 = Instant::now();
    run_tasks_at(&mut s, t0 + ms(500));
    run_tasks_at(&mut s, t0 + ms(1000));
    assert_eq!(text(&s).as_deref(), Some("2"), "the window shows what the task set");
    // A project change doesn't stop a script whose window is open.
    s.execute("file.closeProject", json!({})).unwrap();
    assert_eq!(run_tasks_at(&mut s, t0 + ms(1500))["ran"], json!(1));
    assert_eq!(text(&s).as_deref(), Some("3"));
    // Closing the window ends the script and its tasks.
    s.execute("scriptui.close", json!({"window": win})).unwrap();
    let r = run_tasks_at(&mut s, t0 + ms(5000));
    assert_eq!((r["ran"].clone(), r["next"].clone()), (json!(0), Value::Null), "{r}");
}

#[test]
fn a_throwing_task_is_reported_once_and_cancelled() {
    let mut s = session();
    ok(&mut s, "app.scheduleTask('undefinedThing.go()', 100, true); app.scheduleTask('writeLn(\"fine\")', 100, true);");
    let t0 = Instant::now();
    let r = run_tasks_at(&mut s, t0 + ms(500));
    assert_eq!(r["ran"], json!(2));
    let errors = r["errors"].as_array().cloned().unwrap_or_default();
    assert_eq!(errors.len(), 1, "{r}");
    assert!(errors[0]["message"].as_str().is_some_and(|m| m.contains("undefinedThing") && m.contains("cancelled")), "{r}");
    // The other task goes on; the failing one doesn't run again.
    let r = run_tasks_at(&mut s, t0 + ms(1000));
    assert_eq!((r["ran"].clone(), output(&r), r["errors"].clone()), (json!(1), vec!["fine".to_string()], json!([])), "{r}");
}

#[test]
fn tasks_are_bounded() {
    let mut s = session();
    // Repeating tasks run at most every MIN_REPEAT_MS.
    ok(&mut s, "var hits = 0; app.scheduleTask('hits++', 0, true);");
    let t0 = Instant::now();
    assert_eq!(run_tasks_at(&mut s, t0 + ms(crate::MIN_REPEAT_MS) + ms(20))["ran"], json!(1));
    assert_eq!(run_tasks_at(&mut s, t0 + ms(crate::MIN_REPEAT_MS) + ms(30))["ran"], json!(0));
    // At most MAX_TASKS pending tasks per script.
    let o = run_code(&mut s, "for (var i = 0; i < 1000; i++) app.scheduleTask('1', 1000, true);", "many.jsx");
    assert!(o.error.as_ref().is_some_and(|e| e.message.contains(&format!("at most {}", crate::MAX_TASKS))), "{:?}", o.error);
    // Task code nested too deeply for the parser is refused up front.
    let deep = format!("app.scheduleTask('{}', 10, false);", "(".repeat(10_000));
    let o = run_code(&mut s, &deep, "deep.jsx");
    assert!(o.error.as_ref().is_some_and(|e| e.message.contains("nested too deeply")), "{:?}", o.error);
    // Odd delays and ids are harmless.
    ok(
        &mut s,
        "app.cancelTask(-1); app.cancelTask(NaN); app.cancelTask(1e20); app.scheduleTask('1', NaN, false); app.scheduleTask('1', -5, false); app.scheduleTask('1', 1e300, true);",
    );
}

#[test]
fn closing_the_project_stops_tasks_of_scripts_without_windows() {
    let mut s = session();
    ok(&mut s, "app.scheduleTask('writeLn(1)', 100, true);");
    console(&mut s, "var cid = app.scheduleTask('writeLn(2)', 100, true);");
    let t0 = Instant::now();
    assert_eq!(run_tasks_at(&mut s, t0 + ms(500))["ran"], json!(2));
    s.execute("file.closeProject", json!({})).unwrap();
    let r = run_tasks_at(&mut s, t0 + ms(5000));
    assert_eq!((r["ran"].clone(), r["next"].clone()), (json!(0), Value::Null), "{r}");
}

#[test]
fn console_tasks_run_in_the_console_and_cancel_there() {
    let mut s = session();
    console(&mut s, "var hits = 0; var tid = app.scheduleTask('hits++', 100, true); app.scheduleTask('writeLn(\"once \" + hits)', 300, false);");
    let t0 = Instant::now();
    assert_eq!(run_tasks_at(&mut s, t0 + ms(200))["ran"], json!(1));
    let r = run_tasks_at(&mut s, t0 + ms(400));
    assert_eq!((r["ran"].clone(), output(&r)), (json!(2), vec!["once 2".to_string()]), "{r}");
    assert_eq!(console(&mut s, "app.cancelTask(tid); hits").result, json!(2));
    let r = run_tasks_at(&mut s, t0 + ms(5000));
    assert_eq!((r["ran"].clone(), r["next"].clone()), (json!(0), Value::Null), "{r}");
}

#[test]
fn handlers_schedule_one_shot_tasks_that_run_when_due() {
    let mut s = session();
    ok(
        &mut s,
        r#"
var w = new Window("palette", "Later");
var b = w.add("button", undefined, "Go", { name: "go" });
b.onClick = function () { app.scheduleTask("writeLn('later')", 100, false); };
w.show();
"#,
    );
    s.execute("scriptui.click", json!({"widget": "go"})).unwrap();
    let r = run_tasks_at(&mut s, Instant::now() + ms(500));
    assert_eq!(output(&r), vec!["later"], "{r}");
    // The engine command runs the same pass (headless hosts, agents).
    assert_eq!(s.execute("script.runTasks", json!({})).unwrap()["ran"], json!(0));
    s.execute("scriptui.close", json!({})).unwrap();
}
