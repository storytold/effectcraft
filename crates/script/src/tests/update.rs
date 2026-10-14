//! `Window.update()`: a script doing long work shows its progress. In an app that shows it
//! (`ScriptUi::frontend`) the session goes back to the app with `busy` set, at most every 50 ms,
//! and `scriptui.resume` lets the script carry on; its palette's buttons work meanwhile.
//! Elsewhere it does nothing.

use std::sync::Arc;

use effectcraft_engine::Session;
use serde_json::json;

use crate::run_code;

fn session(frontend: bool) -> Session {
    let mut s = Session { expr: Some(Arc::new(effectcraft_expr::Expressions)), ..Default::default() };
    crate::install(&mut s);
    s.script_ui.frontend = frontend;
    s
}

const WORK: &str = r#"
var w = new Window("palette", "Working");
var bar = w.add("progressbar", undefined, 0, 3);
var label = w.add("statictext", undefined, "Starting", { name: "label" });
var stop = w.add("button", undefined, "Cancel", { name: "cancel" });
var cancelled = false;
stop.onClick = function () { cancelled = true; };
w.show();
for (var i = 1; i <= 3 && !cancelled; i++) {
    var t = Date.now();
    while (Date.now() - t < 60) {}
    app.project.items.addComp("C" + i, 64, 64, 1, 1, 24);
    bar.value = i;
    label.text = "Step " + i;
    w.update();
}
w.close();
"done " + (i - 1) + (cancelled ? " cancelled" : "")
"#;

fn comps(s: &Session) -> usize {
    s.project.items.values().filter(|i| i.as_comp().is_some()).count()
}

#[test]
fn update_shows_progress_and_carries_on_at_resume() {
    let mut s = session(true);
    let o = run_code(&mut s, WORK, "work.jsx");
    // The first update: the app has the session, with the palette as it is now.
    assert!(o.waiting && o.busy && o.error.is_none(), "{o:?}");
    assert_eq!(o.to_json()["busy"], true);
    assert_eq!(comps(&s), 1);
    let w = s.script_ui.windows[0].clone();
    assert_eq!((w.root.children[0].value, w.root.children[1].text.as_str()), (1.0, "Step 1"));
    // The app (a frame later) lets it carry on, to its next update.
    let r = s.execute("scriptui.resume", json!({})).unwrap();
    assert_eq!(r["busy"], true, "{r}");
    assert_eq!(comps(&s), 2);
    assert_eq!(s.script_ui.windows[0].root.children[1].text, "Step 2");
    s.execute("scriptui.resume", json!({})).unwrap();
    let r = s.execute("scriptui.resume", json!({})).unwrap();
    assert_eq!(r["result"], "done 3", "{r}");
    assert!(r.get("busy").is_none() && s.script_ui.busy.is_none());
    assert!(s.execute("scriptui.resume", json!({})).is_err(), "nothing is at work any more");
}

#[test]
fn a_palette_button_works_while_the_script_works() {
    let mut s = session(true);
    let o = run_code(&mut s, WORK, "work.jsx");
    assert!(o.busy);
    // Cancel runs its handler; the script sees it when it carries on.
    let r = s.execute("scriptui.click", json!({"widget": "cancel"})).unwrap();
    assert_eq!(r["busy"], true, "{r}");
    let r = s.execute("scriptui.resume", json!({})).unwrap();
    assert_eq!(r["result"], "done 1 cancelled", "{r}");
    assert_eq!(comps(&s), 1);
}

#[test]
fn without_an_app_to_show_it_update_does_nothing() {
    let mut s = session(false);
    let o = run_code(&mut s, WORK, "work.jsx");
    assert!(!o.waiting && !o.busy, "{o:?}");
    assert_eq!(o.result, json!("done 3"));
    assert!(s.script_ui.busy.is_none());
}
