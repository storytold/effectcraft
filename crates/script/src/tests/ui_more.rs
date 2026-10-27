//! ScriptUI resource strings, onDraw graphics, live onChanging updates and sockets.

use std::sync::Arc;

use effectcraft_engine::Session;
use effectcraft_engine::scriptui::{DrawOp, PathSeg, WidgetKind, WindowKind};
use serde_json::json;

use crate::run_code;

fn session() -> Session {
    let mut s = Session { expr: Some(Arc::new(effectcraft_expr::Expressions)), ..Default::default() };
    crate::install(&mut s);
    s.config = Some(Arc::new(effectcraft_engine::config::MemoryConfig::default()));
    s
}

const RESOURCE: &str = r#"
var w = new Window("palette { text: 'Res', orientation: 'column', alignChildren: ['fill', 'top'], \
  g: Group { orientation: 'row', b: Button { text: 'OK', properties: { name: 'okb' } }, cancel: Button { text: \"Can't\" } }, \
  nm: EditText { text: 'abc', characters: 10 }, \
  s: Slider { minvalue: 0, maxvalue: 10, value: 4 }, \
  dd: DropDownList { properties: { items: ['A', 'B'] }, selection: 1 }, \
  /* a panel */ p: Panel { text: 'Opts', preferredSize: [200, -1], c: Checkbox { text: 'On', value: true } } }");
var extra = w.p.add("group { orientation: 'column', t: StaticText { text: 'hi' } }");
w.g.b.onClick = function () { writeLn("ok " + w.nm.text + " " + w.s.value); };
w.show();
[w.text, w.g.orientation, w.g.b.text, w.g.cancel.text, w.s.maxvalue, w.dd.selection.text, w.p.c.value, extra.t.text, w.g.children.length, w.p.preferredSize.width].join("|")
"#;

#[test]
fn resource_strings_build_windows_and_controls() {
    let mut s = session();
    let o = run_code(&mut s, RESOURCE, "res.jsx");
    assert!(o.error.is_none(), "{:?}", o.error);
    assert_eq!(o.result, json!("Res|row|OK|Can't|10|B|true|hi|2|200"));
    let w = s.script_ui.windows[0].clone();
    assert_eq!((w.kind, w.title.as_str()), (WindowKind::Palette, "Res"));
    let kinds: Vec<WidgetKind> = w.root.children.iter().map(|c| c.kind).collect();
    assert_eq!(kinds, [WidgetKind::Group, WidgetKind::EditText, WidgetKind::Slider, WidgetKind::DropDownList, WidgetKind::Panel]);
    assert_eq!(w.root.children[2].value, 4.0);
    assert_eq!(w.root.children[3].items, ["A", "B"]);
    assert_eq!(w.root.children[3].selection, [1]);
    assert!(w.root.children[4].children[0].checked);
    assert_eq!(w.root.children[4].children[1].children[0].text, "hi");
    // Elements are named after their resource keys (agents address them by it).
    assert_eq!(w.root.children[1].name, "nm");
    s.execute("scriptui.set", json!({"widget": "nm", "value": "xyz"})).unwrap();
    let r = s.execute("scriptui.click", json!({"widget": "okb"})).unwrap();
    assert_eq!(r["output"], "ok xyz 4", "{r}");
    s.execute("scriptui.close", json!({})).unwrap();
    // Syntax errors say where.
    let o = run_code(&mut s, "new Window(\"dialog { text: 'x' oops }\")", "bad.jsx");
    assert!(o.error.unwrap().message.contains("resource string"));
    let o = run_code(&mut s, "new Window(\"panel { }\")", "bad.jsx");
    assert!(o.error.unwrap().message.contains("Bad window type"));
}

const DRAW: &str = r#"
var w = new Window("palette", "Draw");
var c = w.add("group");
c.preferredSize = [100, 50];
c.onDraw = function () {
  var g = this.graphics;
  var b = g.newBrush(g.BrushType.SOLID_COLOR, [1, 0, 0, 1]);
  g.newPath();
  g.rectPath(0, 0, this.size.width, this.size.height);
  g.fillPath(b);
  var p = g.newPen(g.PenType.SOLID_COLOR, [0, 0, 1], 2);
  g.newPath();
  g.moveTo(0, 0);
  g.lineTo(10, 10);
  g.strokePath(p);
  g.ellipsePath(20, 20, 10, 10);
  g.drawString("Hi", p, 5, 6, ScriptUI.newFont("dialog", "BOLD", 20));
};
var m = c.graphics.measureString("Hello", ScriptUI.newFont("dialog", "REGULAR", 12));
w.show();
[m.width, m[1]].join(",")
"#;

#[test]
fn on_draw_handlers_record_a_draw_list() {
    let mut s = session();
    let o = run_code(&mut s, DRAW, "draw.jsx");
    assert!(o.error.is_none(), "{:?}", o.error);
    assert_eq!(o.result, json!("35,15"));
    let w = s.script_ui.windows[0].clone();
    let g = &w.root.children[0];
    assert!(g.handlers.contains(&"onDraw".to_string()));
    assert_eq!(g.draw.len(), 3, "{:?}", g.draw);
    // The handler saw the laid-out size.
    assert_eq!(g.draw[0], DrawOp::Fill { color: Some([1.0, 0.0, 0.0, 1.0]), path: vec![PathSeg::R { x: 0.0, y: 0.0, w: 100.0, h: 50.0 }] });
    match &g.draw[1] {
        DrawOp::Stroke { color, width, path } => {
            assert_eq!((*color, *width), (Some([0.0, 0.0, 1.0, 1.0]), 2.0));
            assert_eq!(path.len(), 2);
        }
        o => panic!("{o:?}"),
    }
    assert!(matches!(&g.draw[2], DrawOp::Text { text, size, style, .. } if text == "Hi" && *size == 20.0 && style == "BOLD"));
    // scriptui.get carries it for agents.
    let tree = s.execute("scriptui.get", json!({})).unwrap();
    assert_eq!(tree["root"]["children"][0]["draw"][0]["op"], "fill");
    s.execute("scriptui.close", json!({})).unwrap();
}

const LIVE: &str = r#"
var w = new Window("palette", "Live");
var e = w.add("edittext", undefined, "");
var sl = w.add("slider", undefined, 0, 0, 100);
var log = w.add("statictext", undefined, "-");
e.onChanging = function () { writeLn("changing " + e.text); log.text = "typing " + e.text; };
e.onChange = function () { writeLn("change " + e.text); };
sl.onChanging = function () { writeLn("slide " + Math.round(sl.value)); };
sl.onChange = function () { writeLn("slid " + Math.round(sl.value)); };
w.show();
"#;

#[test]
fn on_changing_fires_per_keystroke_and_slider_step() {
    let mut s = session();
    let o = run_code(&mut s, LIVE, "live.jsx");
    assert!(o.error.is_none(), "{:?}", o.error);
    let r = s.execute("scriptui.set", json!({"widget": "#1", "value": "a", "changing": true})).unwrap();
    assert_eq!(r["output"], "changing a");
    let r = s.execute("scriptui.set", json!({"widget": "#1", "value": "ab", "changing": true})).unwrap();
    assert_eq!(r["output"], "changing ab");
    assert_eq!(s.script_ui.windows[0].root.children[2].text, "typing ab");
    // Committing the same text runs onChange only.
    let r = s.execute("scriptui.set", json!({"widget": "#1", "value": "ab"})).unwrap();
    assert_eq!(r["output"], "change ab");
    // A committed edit without live updates runs both.
    let r = s.execute("scriptui.set", json!({"widget": "#1", "value": "abc"})).unwrap();
    assert_eq!(r["output"], "changing abc\nchange abc");
    // Slider drags update the value live.
    let r = s.execute("scriptui.set", json!({"widget": "#2", "value": 40, "changing": true})).unwrap();
    assert_eq!(r["output"], "slide 40");
    assert_eq!(s.script_ui.windows[0].root.children[1].value, 40.0);
    let r = s.execute("scriptui.set", json!({"widget": "#2", "value": 41})).unwrap();
    assert_eq!(r["output"], "slide 41\nslid 41");
    s.execute("scriptui.close", json!({})).unwrap();
}

const MOGRT: &str = r#"
var c = app.project.activeItem;
var t = c.layer(1);
var st = t.property("ADBE Text Properties").property("ADBE Text Document");
var op = t.property("ADBE Transform Group").property("ADBE Opacity");
var r = [st.canAddToMotionGraphicsTemplate(c), st.addToMotionGraphicsTemplate(c), op.addToMotionGraphicsTemplateAs(c, "Fade")];
c.motionGraphicsTemplateName = "Lower Third";
c.setMotionGraphicsControllerName(1, "Title");
r.concat([c.motionGraphicsTemplateName, c.motionGraphicsTemplateControllerCount, c.getMotionGraphicsTemplateControllerName(1), c.getMotionGraphicsTemplateControllerName(2)]).join("|")
"#;

#[test]
fn essential_graphics_scripting_hooks() {
    let mut s = session();
    s.execute("comp.new", json!({"name": "Card", "width": 64, "height": 64, "duration": 1})).unwrap();
    s.execute("layer.newText", json!({"text": "Hi"})).unwrap();
    let o = run_code(&mut s, MOGRT, "mogrt.jsx");
    assert!(o.error.is_none(), "{:?} {:?}", o.error, o.output);
    assert_eq!(o.result, json!("true|true|true|Lower Third|2|Title|Fade"));
    let cid = s.active_comp_id().unwrap();
    assert_eq!(s.project.comp(cid).unwrap().essential.as_ref().unwrap().name, "Lower Third");
    // Another comp can't take this comp's properties.
    s.execute("comp.new", json!({"name": "Other", "width": 64, "height": 64, "duration": 1})).unwrap();
    let code = "var card = null; for (var i = 1; i <= app.project.numItems; i++) if (app.project.item(i).name === 'Card') card = app.project.item(i); var o = app.project.activeItem; var p = card.layer(1).property('ADBE Transform Group').property('ADBE Position'); [p.canAddToMotionGraphicsTemplate(o), p.canAddToMotionGraphicsTemplate(card)].join('|')";
    let o = run_code(&mut s, code, "x.jsx");
    assert_eq!(o.result, json!("false|true"), "{:?}", o.error);
    // Export (behind the file-write preference).
    let dir = std::env::temp_dir().join(format!("ec-mogrt-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("lower.ectemplate");
    let export = format!(
        "var card = null; for (var i = 1; i <= app.project.numItems; i++) if (app.project.item(i).name === 'Card') card = app.project.item(i); card.exportAsMotionGraphicsTemplate(false, {:?})",
        path.to_string_lossy()
    );
    assert!(run_code(&mut s, &export, "x.jsx").error.is_some());
    s.prefs.scripting.allow_scripts_write_files = true;
    assert_eq!(run_code(&mut s, &export, "x.jsx").result, json!(true));
    assert!(std::fs::metadata(&path).unwrap().len() > 100);
    // It exists now: no overwrite → false.
    assert_eq!(run_code(&mut s, &export, "x.jsx").result, json!(false));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn create_nulls_from_paths_expressions_evaluate() {
    let mut s = session();
    s.execute("comp.new", json!({"name": "Paths", "width": 100, "height": 80, "frameRate": 10, "duration": 1})).unwrap();
    let l = s.execute("layer.newSolid", json!({"name": "Plate", "color": "#808080", "width": 40, "height": 40})).unwrap()["layer"].as_u64().unwrap();
    s.execute("mask.new", json!({"layer": l, "vertices": [[0, 0], [40, 0], [40, 40], [0, 40]], "closed": true})).unwrap();
    s.execute("paths.nullsFollowPoints", json!({"layer": l})).unwrap();
    s.execute("paths.tracePath", json!({"layer": l})).unwrap();
    // Layer 1: the trace null; layers 2..5: the vertex nulls (top = last vertex).
    let pos = |i: u32, t: f64| format!("app.project.activeItem.layer({i}).property('ADBE Transform Group').property('ADBE Position').valueAtTime({t}, false)");
    let o = run_code(&mut s, &format!("[{}, {}, {}].join('|')", pos(2, 0.0), pos(1, 0.0), pos(1, 0.5)), "x.jsx");
    assert!(o.error.is_none(), "{:?}", o.error);
    let parts: Vec<Vec<f64>> = o.result.as_str().unwrap().split('|').map(|p| p.split(',').map(|x| x.parse().unwrap()).collect()).collect();
    assert_eq!(&parts[0][..2], &[30.0, 60.0], "vertex 4 (0, 40) in comp space");
    assert_eq!(&parts[1][..2], &[30.0, 20.0], "progress 0: the first vertex");
    // Halfway in time along the closed square: the opposite corner.
    assert!((parts[2][0] - 70.0).abs() < 0.5 && (parts[2][1] - 60.0).abs() < 0.5, "{:?}", parts[2]);
    // Points Follow Nulls: moving the last vertex's null moves the vertex.
    let r = s.execute("paths.pointsFollowNulls", json!({"layer": l})).unwrap();
    let last = r["nulls"][3].as_u64().unwrap();
    s.execute("prop.set", json!({"layer": last, "path": "transform/position", "value": [10, 10]})).unwrap();
    let code = "var c = app.project.activeItem; var m = c.layer(c.numLayers).property('ADBE Mask Parade').property(1).property('ADBE Mask Shape').valueAtTime(0, false); [m.vertices[3][0], m.vertices[3][1], m.vertices[0][0], m.vertices.length].join(',')";
    let o = run_code(&mut s, code, "x.jsx");
    assert_eq!(o.result, json!("-20,-10,0,4"), "{:?}", o.error);
}

fn pin_at(s: &mut Session, id: u64, i: usize) -> [f64; 2] {
    let info = s.execute("puppet.info", json!({"layer": id})).unwrap();
    let v = &info["meshes"][0]["pins"][i]["position"];
    [v[0].as_f64().unwrap(), v[1].as_f64().unwrap()]
}

/// A layer's Position now, with its expression applied (what renders).
fn position_now(s: &Session, layer: u64) -> [f64; 2] {
    let cid = s.active_comp_id().unwrap();
    let comp = s.project.comp(cid).unwrap();
    let l = comp.layer(effectcraft_engine::project::LayerId(layer)).unwrap();
    let ctx = effectcraft_engine::render::EvalCtx { project: &s.project, comp_id: cid, comp, time: s.time(), expr: s.expr.as_deref(), footage: None };
    let v = ctx.value(l, l.props.prop("transform/position").unwrap()).components();
    [v[0], v[1]]
}

fn near(a: [f64; 2], b: [f64; 2]) -> bool {
    (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6
}

#[test]
fn puppet_pin_rigs_follow_and_lead_with_live_expressions() {
    let mut s = session();
    s.execute("comp.new", json!({"name": "C", "width": 200, "height": 100, "duration": 2, "frameRate": 30})).unwrap();
    let id = s.execute("layer.newSolid", json!({"width": 100, "height": 60, "color": [0, 0, 1]})).unwrap()["layer"].as_u64().unwrap();
    s.execute("puppet.addPin", json!({"layer": id, "position": [10, 30]})).unwrap();
    s.execute("puppet.addPin", json!({"layer": id, "position": [90, 30]})).unwrap();
    s.execute("puppet.selectPins", json!({"layer": id, "pins": []})).unwrap();
    let r = s.execute("paths.pointsFollowNulls", json!({"layer": id})).unwrap();
    let nulls: Vec<u64> = r["nulls"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
    // Rigged at rest: the pins haven't moved. Layer (10, 30) is comp (60, 50).
    assert!(near(pin_at(&mut s, id, 0), [10.0, 30.0]), "{:?}", pin_at(&mut s, id, 0));
    // Moving the null moves its pin.
    s.execute("prop.set", json!({"layer": nulls[0], "path": "transform/position", "value": [60, 80]})).unwrap();
    assert!(near(pin_at(&mut s, id, 0), [10.0, 60.0]), "{:?}", pin_at(&mut s, id, 0));
    // Parented nulls rig a limb: pin 2's null rides on pin 1's null.
    s.execute("layer.setParent", json!({"layers": [nulls[1]], "parent": nulls[0]})).unwrap();
    s.execute("prop.set", json!({"layer": nulls[0], "path": "transform/position", "value": [60, 90]})).unwrap();
    assert!(near(pin_at(&mut s, id, 1), [90.0, 40.0]), "{:?}", pin_at(&mut s, id, 1));
    // Nulls Follow Points: a null riding on pin 1, now at comp (60, 90).
    let r = s.execute("paths.nullsFollowPoints", json!({"layer": id, "pins": ["Puppet Pin 1"]})).unwrap();
    let rider = r["nulls"][0].as_u64().unwrap();
    assert!(near(position_now(&s, rider), [60.0, 90.0]), "{:?}", position_now(&s, rider));
}

#[test]
fn puppet_follow_through_delays_the_leaders_motion() {
    let mut s = session();
    s.execute("comp.new", json!({"name": "C", "width": 200, "height": 100, "duration": 2, "frameRate": 30})).unwrap();
    let id = s.execute("layer.newSolid", json!({"width": 100, "height": 60, "color": [0, 0, 1]})).unwrap()["layer"].as_u64().unwrap();
    let lead = s.execute("puppet.addPin", json!({"layer": id, "position": [10, 30]})).unwrap()["pin"].clone();
    s.execute("puppet.addPin", json!({"layer": id, "position": [50, 30]})).unwrap();
    s.execute("puppet.addPin", json!({"layer": id, "position": [90, 30]})).unwrap();
    // The leader drops 20 px between 0 and 0.5 s.
    s.execute("puppet.movePin", json!({"layer": id, "pin": lead, "position": [10, 50], "time": 0.5})).unwrap();
    s.execute("puppet.follow", json!({"layer": id, "leader": "Puppet Pin 1", "pins": ["Puppet Pin 2", "Puppet Pin 3"], "delay": 0.2})).unwrap();
    let y = |s: &mut Session, pin: usize, t: f64| {
        let info = s.execute("puppet.info", json!({"layer": id, "time": t})).unwrap();
        info["meshes"][0]["pins"][pin]["position"][1].as_f64().unwrap()
    };
    // At rest before anything moves; pin 2 trails 0.2 s and pin 3 (farther) 0.4 s.
    assert_eq!((y(&mut s, 1, 0.0), y(&mut s, 2, 0.0)), (30.0, 30.0));
    let (lead_y, p2, p3) = (y(&mut s, 0, 0.5), y(&mut s, 1, 0.5), y(&mut s, 2, 0.5));
    assert!(lead_y > p2 && p2 > p3 && p3 > 30.0, "{lead_y} {p2} {p3}");
    assert!((y(&mut s, 1, 0.7) - 50.0).abs() < 1e-6, "pin 2 arrives 0.2 s after the leader");
    assert!(y(&mut s, 2, 0.7) < 50.0 - 1e-3);
    assert!((y(&mut s, 2, 0.9) - 50.0).abs() < 1e-6, "pin 3 arrives 0.4 s after the leader");
}

#[test]
fn sockets_talk_tcp_behind_the_network_preference() {
    use std::io::{BufRead, BufReader, Write};
    let mut s = session();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut c, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(c.try_clone().unwrap()).read_line(&mut line).unwrap();
        c.write_all(format!("pong:{}\nrest", line.trim()).as_bytes()).unwrap();
    });
    let code = format!(
        "var s = new Socket(); var ok = s.open('127.0.0.1:{port}'); s.writeln('ping'); var r = s.readln(); var rest = s.read(); var e = s.eof; s.close(); [ok, r, rest, e, s.connected].join('|')"
    );
    // Off: no network.
    let o = run_code(&mut s, &code, "net.jsx");
    assert!(o.error.unwrap().message.contains("Access Network"));
    s.prefs.scripting.allow_scripts_write_files = true;
    let o = run_code(&mut s, &code, "net.jsx");
    assert!(o.error.is_none(), "{:?}", o.error);
    assert_eq!(o.result, json!("true|pong:ping|rest|true|false"));
    server.join().unwrap();
    // Refused connections return false with an error; listen + poll serve clients.
    let o = run_code(&mut s, "var s = new Socket(); s.timeout = 1; var ok = s.open('127.0.0.1:1'); [ok, s.error.length > 0].join('|')", "net.jsx");
    assert_eq!(o.result, json!("false|true"));
    let o = run_code(&mut s, "var l = new Socket(); l.listen(0); var r = [l.port > 0, l.poll() === null].join('|'); l.close(); r", "net.jsx");
    assert_eq!(o.result, json!("true|true"));
}

/// `__uiParseRes(src)` as JSON, or the error message.
fn parse_res(s: &mut Session, src: &str) -> Result<serde_json::Value, String> {
    let code = format!("JSON.stringify(__uiParseRes({}))", serde_json::to_string(src).unwrap());
    let o = run_code(s, &code, "res.jsx");
    match o.error {
        Some(e) => Err(e.message),
        None => Ok(serde_json::from_str(o.result.as_str().unwrap_or("null")).unwrap_or_default()),
    }
}

#[test]
fn resource_string_parser_edge_cases() {
    let mut s = session();
    // Accepted: trailing commas, quoted keys, number forms, escapes, comments, empty bodies,
    // null / undefined, bare words, nested arrays and object literals.
    let ok = parse_res(
        &mut s,
        "group { 'quoted key': 1, \"dq\": -.5, n: +3, e: 1e2, arr: [[1, [2]], 'x', ], \
         esc: 'it\\'s\\n\\\\', /* block */ nul: null, u: undefined, w: fill, // line\n \
         obj: { a: { b: true } }, kid: Button {}, }",
    )
    .unwrap();
    assert_eq!(ok["type"], "group");
    let list = ok["list"].as_array().unwrap();
    let get = |k: &str| list.iter().find(|e| e[0] == k).map(|e| e[1].clone()).unwrap();
    assert_eq!(get("quoted key"), json!(1));
    assert_eq!(get("dq"), json!(-0.5));
    assert_eq!(get("n"), json!(3));
    assert_eq!(get("e"), json!(100));
    assert_eq!(get("arr"), json!([[1, [2]], "x"]));
    assert_eq!(get("esc"), json!("it's\n\\"));
    assert_eq!(get("nul"), json!(null));
    assert_eq!(get("w"), json!("fill"));
    assert_eq!(get("kid"), json!({"__element": "Button", "list": []}));
    // Rejected with a located message (never a crash).
    for bad in [
        "",
        "   ",
        "dialog",
        "dialog {",
        "dialog { text: 'unterminated }",
        "dialog { text 'x' }",
        "dialog { a: [1, 2 }",
        "dialog { a: 1 } trailing",
        "dialog { /* unterminated comment",
        "dialog { a: 'x\\",
        "{ }",
        "123 { }",
        "dialog { a: 1 b: 2 }",
        "dialog { : 1 }",
        "dialog { a: }",
        "dialog { a: Button { }",
    ] {
        let e = parse_res(&mut s, bad).expect_err(bad);
        assert!(e.contains("resource string"), "{bad:?}: {e}");
        assert!(e.contains("offset"), "{bad:?}: {e}");
    }
    // Deep nesting builds; absurd nesting fails cleanly (the script engine's recursion limit).
    let deep = |n: usize| format!("group {{ {}{} }}", "g: Group { ".repeat(n), "}".repeat(n));
    let d = parse_res(&mut s, &deep(40)).unwrap();
    assert_eq!(d["list"][0][1]["__element"], "Group");
    let _ = parse_res(&mut s, &deep(5000));
    // A window from a deep resource.
    let o = run_code(
        &mut s,
        &format!(
            "var w = new Window({}); w.children.length",
            serde_json::to_string(&format!("palette {{ {}{} }}", "g: Group { ".repeat(20), "}".repeat(20))).unwrap()
        ),
        "deep.jsx",
    );
    assert_eq!(o.result, json!(1), "{:?}", o.error);
}

#[test]
fn socket_edge_cases() {
    let mut s = session();
    s.prefs.scripting.allow_scripts_write_files = true;
    // Bad addresses and ports: false with an error, never a throw or crash.
    for host in ["", "nohostport", "127.0.0.1", "127.0.0.1:99999", "[::1", "a:b:c", "127.0.0.1:-5"] {
        let code = format!(
            "var s = new Socket(); s.timeout = 1; var ok = s.open({}); [ok, s.error.length > 0, s.connected, s.eof].join('|')",
            serde_json::to_string(host).unwrap()
        );
        let o = run_code(&mut s, &code, "net.jsx");
        assert_eq!(o.result, json!("false|true|false|true"), "{host:?}: {:?}", o.error);
    }
    for port in ["70000", "-1", "1.5"] {
        let o = run_code(&mut s, &format!("var l = new Socket(); [l.listen({port}), l.error.length > 0].join('|')"), "net.jsx");
        assert_eq!(o.result, json!("false|true"), "{port}: {:?}", o.error);
    }
    // Never opened or closed: reads are empty, writes fail, close is idempotent.
    let o = run_code(
        &mut s,
        "var s = new Socket(); var r = [s.read(), s.read(5), s.readln(), s.write('x'), s.connected, s.eof, s.poll(), s.close(), s.close()]; r.join('|')",
        "net.jsx",
    );
    assert_eq!(o.result, json!("|||false|false|true||true|true"), "{:?}", o.error);
    // Binary bytes round trip in BINARY encoding; invalid UTF-8 is replaced in UTF-8; partial
    // and zero-length reads; read stops at the timeout without data.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        use std::io::Write;
        for _ in 0..2 {
            let (mut c, _) = listener.accept().unwrap();
            let bytes: Vec<u8> = (0..=255u8).collect();
            c.write_all(&bytes).unwrap();
        }
    });
    let code = format!(
        "var s = new Socket(); s.open('127.0.0.1:{port}', 'BINARY'); var z = s.read(0); var a = s.read(10); var b = s.read(); s.close(); \
         var ok = true; for (var i = 0; i < 10; i++) ok = ok && a.charCodeAt(i) === i; for (var j = 0; j < 246; j++) ok = ok && b.charCodeAt(j) === j + 10; \
         [z.length, a.length, b.length, ok].join('|')"
    );
    let o = run_code(&mut s, &code, "net.jsx");
    assert_eq!(o.result, json!("0|10|246|true"), "{:?}", o.error);
    let code = format!(
        "var s = new Socket(); s.open('127.0.0.1:{port}', 'UTF-8'); var d = s.read(); s.close(); [d.charCodeAt(0), d.charCodeAt(127), d.indexOf('\\ufffd') > 0].join('|')"
    );
    let o = run_code(&mut s, &code, "net.jsx");
    assert_eq!(o.result, json!("0|127|true"), "{:?}", o.error);
    server.join().unwrap();
    // A listener with nobody connecting: a short read timeout returns quickly.
    let t = std::time::Instant::now();
    let o = run_code(
        &mut s,
        "var l = new Socket(); l.listen(0); l.timeout = 0; var c = new Socket(); c.timeout = 0.2; c.open(l.host); var x = c.read(); c.close(); l.close(); x.length",
        "net.jsx",
    );
    assert_eq!(o.result, json!(0), "{:?}", o.error);
    assert!(t.elapsed().as_secs_f64() < 5.0);
}
