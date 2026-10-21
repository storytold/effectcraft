//! ScriptUI mouse events (#766): `addEventListener("mousedown" | "mouseup" | "mousemove" |
//! "mouseover" | "mouseout" | "click", handler)` listeners receive a MouseEvent through
//! `scriptui.mouse`, with capture / target / bubble phases as the JavaScript Tools Guide
//! documents.

use std::sync::Arc;

use effectcraft_engine::Session;
use serde_json::{Value, json};

use crate::run_code;

fn session() -> Session {
    let mut s = Session { expr: Some(Arc::new(effectcraft_expr::Expressions)), ..Default::default() };
    crate::install(&mut s);
    s.config = Some(Arc::new(effectcraft_engine::config::MemoryConfig::default()));
    s
}

const CANVAS: &str = r#"
var w = new Window("palette", "Canvas");
var g = w.add("group", undefined, undefined, { name: "row" });
var pad = g.add("group", undefined, undefined, { name: "pad" });
pad.preferredSize = [120, 80];
var box = w.add("checkbox", undefined, "Snap", { name: "snap" });
var plain = w.add("statictext", undefined, "no listeners", { name: "plain" });
function rec(tag) {
  return function (e) {
    writeLn(tag + ":" + e.type + "@" + e.eventPhase + " " + e.clientX + "," + e.clientY + " b" + e.button + " d" + e.detail +
      (e.shiftKey ? " shift" : "") + (e.altKey ? " alt" : "") + (e.getModifierState("Control") ? " ctrl" : "") +
      (e.currentTarget === this ? "" : " WRONG-THIS") + " " + e.target.properties.name);
  };
}
pad.addEventListener("mousedown", rec("pad"));
pad.addEventListener("mousemove", rec("pad"));
pad.addEventListener("mouseover", rec("pad"));
pad.addEventListener("mouseout", rec("pad"));
pad.addEventListener("mouseup", rec("pad"));
g.addEventListener("mousedown", rec("row"));
w.addEventListener("mousedown", rec("win"), true);
box.onClick = function () { writeLn("onClick " + box.value); };
box.addEventListener("click", rec("box"));
w.show();
"#;

fn lines(r: &Value) -> Vec<String> {
    assert_eq!(r["ok"], true, "{r}");
    r["output"].as_str().unwrap_or_default().lines().map(str::to_string).collect()
}

#[test]
fn mouse_events_reach_listeners_with_pointer_details() {
    let mut s = session();
    let o = run_code(&mut s, CANVAS, "canvas.jsx");
    assert!(o.error.is_none(), "{:?}", o.error);
    let w = s.script_ui.windows[0].clone();
    // The published tree says which controls want which mouse events (containers' listeners
    // reach their children: events bubble).
    let pad = w.root.lookup(&json!("pad")).unwrap();
    assert_eq!(pad.mouse, ["mousedown", "mouseup", "mousemove", "mouseover", "mouseout"]);
    assert_eq!(w.root.lookup(&json!("snap")).unwrap().mouse, ["mousedown", "click"], "the window's capture listener + its own");
    assert_eq!(w.root.lookup(&json!("plain")).unwrap().mouse, ["mousedown"]);
    // A press: capture on the window, the target, then bubbling up through the group.
    let r = s
        .execute(
            "scriptui.mouse",
            json!({"widget": "pad", "type": "mousedown", "x": 12.5, "y": 30, "button": 2, "detail": 2, "shiftKey": true, "ctrlKey": true}),
        )
        .unwrap();
    assert_eq!(
        lines(&r),
        [
            "win:mousedown@capture 12.5,30 b2 d2 shift ctrl pad",
            "pad:mousedown@target 12.5,30 b2 d2 shift ctrl pad",
            "row:mousedown@bubble 12.5,30 b2 d2 shift ctrl pad",
        ]
    );
    // Hover and moves carry no click count; x / y default to the control's centre.
    let r = s.execute("scriptui.mouse", json!({"widget": "pad", "type": "mouseover"})).unwrap();
    assert_eq!(lines(&r), ["pad:mouseover@target 60,40 b0 d0 pad"]);
    let r = s.execute("scriptui.mouse", json!({"widget": "pad", "type": "MouseMove", "x": 3, "y": 4, "altKey": true})).unwrap();
    assert_eq!(lines(&r), ["pad:mousemove@target 3,4 b0 d0 alt pad"]);
    // An event nobody listens for on a control runs nothing (but still answers).
    let r = s.execute("scriptui.mouse", json!({"widget": "plain", "type": "mouseup"})).unwrap();
    assert!(lines(&r).is_empty());
    // A mouse click does what a click does (the checkbox toggles once, onClick runs) and its
    // listeners get the pointer.
    let r = s.execute("scriptui.mouse", json!({"widget": "snap", "type": "click", "x": 5, "y": 6})).unwrap();
    assert_eq!(lines(&r), ["onClick true", "box:click@target 5,6 b0 d1 snap"]);
    assert!(s.script_ui.windows[0].root.lookup(&json!("snap")).unwrap().checked);
    // Agents' plain click still fires the click listeners.
    let r = s.execute("scriptui.click", json!({"widget": "snap"})).unwrap();
    assert_eq!(r["output"].as_str().unwrap_or_default().lines().next(), Some("onClick false"));
    s.execute("scriptui.close", json!({})).unwrap();
}

#[test]
fn stop_propagation_capture_and_removal() {
    let mut s = session();
    let code = r#"
      var w = new Window("palette", "Stops");
      var g = w.add("group", undefined, undefined, { name: "outer" });
      var b = g.add("button", undefined, "Go", { name: "go" });
      var stopHere = function (e) { writeLn("capture " + e.currentTarget.properties.name); e.stopPropagation(); };
      g.addEventListener("mouseup", stopHere, true);
      b.addEventListener("mouseup", function () { writeLn("never"); });
      var bubble = function (e) { writeLn("bubble " + e.eventPhase); };
      g.addEventListener("mousedown", bubble);
      b.addEventListener("mousedown", function (e) { writeLn("target"); e.stopPropagation(); e.preventDefault(); writeLn("prevented " + e.defaultPrevented); });
      b.addEventListener("mousemove", function () { writeLn("move"); });
      b.removeEventListener("mousemove", b.__listeners.mousemove[0]);
      w.show();
    "#;
    let o = run_code(&mut s, code, "stops.jsx");
    assert!(o.error.is_none(), "{:?}", o.error);
    // A capture listener that stops: the target never hears it.
    let r = s.execute("scriptui.mouse", json!({"widget": "go", "type": "mouseup"})).unwrap();
    assert_eq!(lines(&r), ["capture outer"]);
    // Stopped at the target: no bubbling to the group.
    let r = s.execute("scriptui.mouse", json!({"widget": "go", "type": "mousedown"})).unwrap();
    assert_eq!(lines(&r), ["target", "prevented true"]);
    // The group itself as the target.
    let r = s.execute("scriptui.mouse", json!({"widget": "outer", "type": "mousedown"})).unwrap();
    assert_eq!(lines(&r), ["bubble target"]);
    // Removed listeners don't run, and no longer ask for the event.
    let r = s.execute("scriptui.mouse", json!({"widget": "go", "type": "mousemove"})).unwrap();
    assert!(lines(&r).is_empty());
    assert!(!s.script_ui.windows[0].root.lookup(&json!("go")).unwrap().mouse.iter().any(|m| m == "mousemove"));
    s.execute("scriptui.close", json!({})).unwrap();
}

/// Hostile input: unknown events, disabled controls, absurd coordinates, a throwing listener,
/// a script that makes a control its own parent.
#[test]
fn mouse_events_survive_bad_input_and_throwing_listeners() {
    let mut s = session();
    let code = r#"
      var w = new Window("palette", "Bad");
      var b = w.add("button", undefined, "Boom", { name: "boom" });
      var off = w.add("button", undefined, "Off", { name: "off" });
      off.enabled = false;
      off.addEventListener("mousedown", function () { writeLn("disabled ran"); });
      var n = 0;
      b.addEventListener("mousedown", function (e) { n++; writeLn("at " + e.clientX + "," + e.clientY + " b" + e.button + " d" + e.detail); if (n === 2) throw new Error("bad press"); });
      var loop = w.add("group", undefined, undefined, { name: "loop" });
      loop.addEventListener("mousedown", function () { writeLn("loop ok"); });
      w.show();
      loop.parent = loop;
    "#;
    let o = run_code(&mut s, code, "bad.jsx");
    assert!(o.error.is_none(), "{:?}", o.error);
    assert!(s.execute("scriptui.mouse", json!({"widget": "boom", "type": "dblclick"})).is_err());
    assert!(s.execute("scriptui.mouse", json!({"widget": "off", "type": "mousedown"})).is_err(), "disabled controls get no events");
    let r = s.execute("scriptui.mouse", json!({"widget": "boom", "type": "mousedown", "x": 1e300, "y": -1e300, "button": 99, "detail": -4})).unwrap();
    assert_eq!(lines(&r), ["at 1000000,-1000000 b2 d0"]);
    // A throwing listener reports its error; the window keeps working.
    let r = s.execute("scriptui.mouse", json!({"widget": "boom", "type": "mousedown"})).unwrap();
    assert_eq!(r["ok"], false, "{r}");
    assert!(r["error"]["message"].as_str().unwrap_or_default().contains("bad press"), "{r}");
    let r = s.execute("scriptui.mouse", json!({"widget": "boom", "type": "mousedown"})).unwrap();
    let at = s.script_ui.windows[0].root.lookup(&json!("boom")).unwrap().bounds;
    assert_eq!(lines(&r), [format!("at {},{} b0 d1", at[2] / 2.0, at[3] / 2.0)], "x / y default to the centre");
    // A parent cycle made by the script ends instead of hanging.
    let r = s.execute("scriptui.mouse", json!({"widget": "loop", "type": "mousedown"})).unwrap();
    assert!(lines(&r).iter().any(|l| l == "loop ok"), "{r}");
    s.execute("scriptui.close", json!({})).unwrap();
}
