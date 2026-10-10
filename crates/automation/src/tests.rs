//! MCP protocol round-trips, in-process (headless) and against a fake control channel (bridge).

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;

use serde_json::{Value, json};

use crate::{Backend, McpServer, Session, base64, png_size};

fn server() -> McpServer {
    McpServer::new(Backend::headless(Session::default()))
}

/// Send a request, return its `result` (panics on a JSON-RPC error).
fn rpc(s: &mut McpServer, id: u64, method: &str, params: Value) -> Value {
    let line = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string();
    let reply: Value = serde_json::from_str(&s.handle_line(&line).expect("reply")).unwrap();
    assert_eq!(reply["jsonrpc"], "2.0");
    assert_eq!(reply["id"], id);
    assert!(reply.get("error").is_none(), "{method}: {reply}");
    reply["result"].clone()
}

/// Call a tool; returns (content, isError).
fn call(s: &mut McpServer, name: &str, args: Value) -> (Vec<Value>, bool) {
    let r = rpc(s, 99, "tools/call", json!({"name": name, "arguments": args}));
    (r["content"].as_array().unwrap().clone(), r["isError"].as_bool().unwrap())
}

/// Call a tool that returns JSON text; panics if it failed.
fn call_json(s: &mut McpServer, name: &str, args: Value) -> Value {
    let (c, err) = call(s, name, args.clone());
    assert!(!err, "{name} {args}: {c:?}");
    assert_eq!(c[0]["type"], "text");
    serde_json::from_str(c[0]["text"].as_str().unwrap()).unwrap()
}

#[test]
fn initialize_and_list_tools() {
    let mut s = server();
    let r = rpc(&mut s, 1, "initialize", json!({"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}));
    assert_eq!(r["protocolVersion"], "2025-03-26");
    assert_eq!(r["serverInfo"]["name"], "effectcraft");
    assert!(r["capabilities"]["tools"].is_object());
    // Unknown revision: we answer with our latest.
    let r = rpc(&mut s, 2, "initialize", json!({"protocolVersion": "1999-01-01"}));
    assert_eq!(r["protocolVersion"], crate::server::PROTOCOL_VERSIONS[0]);
    // Notifications get no reply.
    assert!(s.handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
    assert_eq!(rpc(&mut s, 3, "ping", json!({})), json!({}));

    let tools = rpc(&mut s, 4, "tools/list", json!({}))["tools"].as_array().unwrap().clone();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    for want in [
        "list_commands",
        "execute_command",
        "run_script",
        "get_project",
        "get_comp",
        "get_layer",
        "get_property",
        "set_property",
        "add_keyframe",
        "render_frame",
        "open_project",
        "save_project",
        "undo",
        "redo",
    ] {
        assert!(names.contains(&want), "missing {want}");
    }
    assert!(!names.contains(&"screenshot"), "bridge-only tools are hidden headless");
    for t in &tools {
        assert!(t["description"].as_str().unwrap().len() > 20);
        assert_eq!(t["inputSchema"]["type"], "object");
    }
}

#[test]
fn protocol_errors() {
    let mut s = server();
    let r: Value = serde_json::from_str(&s.handle_line("{not json").unwrap()).unwrap();
    assert_eq!(r["error"]["code"], -32700);
    let r: Value = serde_json::from_str(&s.handle_line(r#"{"jsonrpc":"2.0","id":5,"method":"nope"}"#).unwrap()).unwrap();
    assert_eq!(r["error"]["code"], -32601);
    let r: Value = serde_json::from_str(&s.handle_line(r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"screenshot"}}"#).unwrap()).unwrap();
    assert_eq!(r["error"]["code"], -32602);
    // Batches.
    let r: Value =
        serde_json::from_str(&s.handle_line(r#"[{"jsonrpc":"2.0","id":7,"method":"ping"},{"jsonrpc":"2.0","method":"notifications/x"}]"#).unwrap()).unwrap();
    assert_eq!(r.as_array().unwrap().len(), 1);
    // Tool failures are in-band.
    let (c, err) = call(&mut s, "execute_command", json!({"command": "no.such"}));
    assert!(err);
    assert!(c[0]["text"].as_str().unwrap().contains("unknown command"));
    let (_, err) = call(&mut s, "render_frame", json!({}));
    assert!(err, "no comp yet");
    // Unknown params are rejected with the accepted keys, not silently ignored.
    let (c, err) = call(&mut s, "execute_command", json!({"command": "comp.new", "params": {"nmae": "X"}}));
    assert!(err);
    assert!(c[0]["text"].as_str().unwrap().contains("accepted: name, width"), "{c:?}");
}

#[test]
fn headless_workflow() {
    let mut s = server();
    rpc(&mut s, 1, "initialize", json!({"protocolVersion": "2025-06-18"}));

    let cmds = call_json(&mut s, "list_commands", json!({"filter": "layer.new"}));
    assert!(cmds.as_array().unwrap().iter().any(|c| c["id"] == "layer.newSolid"));

    call_json(&mut s, "execute_command", json!({"command": "comp.new", "params": {"name": "Main", "width": 320, "height": 180, "duration": 4}}));
    let l =
        call_json(&mut s, "execute_command", json!({"command": "layer.newSolid", "params": "{\"color\":\"#ff0000\",\"width\":100,\"height\":100}"}))["layer"]
            .clone();
    assert!(l.is_u64());

    let comp = call_json(&mut s, "get_comp", json!({"comp": "Main"}));
    assert_eq!(comp["width"], 320);
    assert_eq!(comp["layers"][0]["id"], l);

    // Static set + read back.
    let p = call_json(&mut s, "set_property", json!({"layer": l, "path": "transform/opacity", "value": 50}));
    assert_eq!(p["value"], 50.0);
    assert_eq!(call_json(&mut s, "get_property", json!({"layer": "#1", "path": "transform/opacity"}))["value"], 50.0);

    // Keyframes with easing.
    let p = call_json(
        &mut s,
        "add_keyframe",
        json!({"layer": l, "path": "transform/position", "keys": [{"time": 0, "value": [0, 90]}, {"time": 2, "value": [320, 90]}], "interpolation": "easyEase"}),
    );
    assert_eq!(p["keys"].as_array().unwrap().len(), 2);
    assert_eq!(p["keys"][0]["out"], "Bezier");
    let mid = call_json(&mut s, "get_property", json!({"layer": l, "path": "transform/position", "time": 1.0}));
    assert!((mid["value"][0].as_f64().unwrap() - 160.0).abs() < 1.0, "{mid}");

    // Expression via set_property.
    let p = call_json(&mut s, "set_property", json!({"layer": l, "path": "transform/rotation", "expression": "time * 90"}));
    assert_eq!(p["expression"], "time * 90");

    // Property tree carries paths; flat mode too.
    let tree = call_json(&mut s, "get_layer", json!({"layer": l, "flat": true}));
    let props = tree["properties"].as_array().unwrap();
    assert!(props.iter().any(|p| p["path"] == "transform/position" && p["keys"] == 2));

    // Render: an inline PNG at the requested size.
    let (c, err) = call(&mut s, "render_frame", json!({"time": 1.0, "max_side": 160}));
    assert!(!err, "{c:?}");
    assert_eq!(c[0]["type"], "image");
    assert_eq!(c[0]["mimeType"], "image/png");
    let png = base64::decode(c[0]["data"].as_str().unwrap()).unwrap();
    assert_eq!(png_size(&png), Some((160, 90)));
    let info: Value = serde_json::from_str(c[1]["text"].as_str().unwrap()).unwrap();
    assert_eq!(info["width"], 160);

    // Undo / redo.
    let before = call_json(&mut s, "get_project", json!({}))["undo"].as_array().unwrap().len();
    let u = call_json(&mut s, "undo", json!({"steps": 2}));
    assert_eq!(u["steps"], 2);
    assert_eq!(u["undoStack"].as_array().unwrap().len(), before - 2);
    call_json(&mut s, "redo", json!({"steps": 2}));
    assert_eq!(call_json(&mut s, "get_project", json!({}))["undo"].as_array().unwrap().len(), before);

    // Save + reopen.
    let dir = std::env::temp_dir().join(format!("ec-mcp-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("t.ecproj").to_string_lossy().into_owned();
    let saved = call_json(&mut s, "save_project", json!({"path": path}));
    assert_eq!(saved["dirty"], false);
    call_json(&mut s, "open_project", json!({"new": true}));
    assert!(call(&mut s, "get_comp", json!({})).1, "empty project has no comp");
    let proj = call_json(&mut s, "open_project", json!({"path": path}));
    assert_eq!(proj["items"].as_array().unwrap().len(), 3, "comp + Solids folder + solid: {proj}");
    let p = call_json(&mut s, "get_property", json!({"comp": "Main", "layer": "#1", "path": "transform/position"}));
    assert_eq!(p["keys"].as_array().unwrap().len(), 2);
    let _ = std::fs::remove_dir_all(dir);
}

/// An expression with a syntax error is kept but disabled; set_property and get_property report
/// its error, and `evaluated` is the static value that renders (#163).
#[test]
fn expression_syntax_errors_reach_the_reply() {
    let mut s = McpServer::new(Backend::headless(effectcraft_host::session()));
    call_json(&mut s, "execute_command", json!({"command": "comp.new", "params": {"name": "A", "width": 64, "height": 64}}));
    call_json(&mut s, "execute_command", json!({"command": "layer.newSolid", "params": {"name": "S", "color": "#ff0000", "width": 32, "height": 8}}));
    let at = |expr: &str| json!({"layer": "S", "path": "transform/rotation", "expression": expr});
    call_json(&mut s, "set_property", json!({"layer": "S", "path": "transform/rotation", "value": 30}));
    let set = call_json(&mut s, "set_property", at("thisIsBroken("));
    let got = call_json(&mut s, "get_property", json!({"layer": "S", "path": "transform/rotation"}));
    for r in [&set, &got] {
        assert_eq!((&r["expression"], &r["evaluated"]), (&json!("thisIsBroken("), &json!(30.0)), "{r}");
        assert!(r["expressionError"].as_str().unwrap().contains("SyntaxError"), "{r}");
    }
    // A runtime error and a valid expression report as before.
    let r = call_json(&mut s, "set_property", at("nope*2"));
    assert!(r["expressionError"].as_str().unwrap().contains("nope is not defined"), "{r}");
    let r = call_json(&mut s, "set_property", at("60"));
    assert_eq!((&r["evaluated"], r.get("expressionError")), (&json!(60.0), None), "{r}");
}

#[test]
fn stdio_loop() {
    let mut s = server();
    let input = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "open_project", "arguments": {"demo": true}}}),
    ]
    .iter()
    .map(|v| v.to_string() + "\n")
    .collect::<String>();
    let mut out = Vec::new();
    s.serve(input.as_bytes(), &mut out).unwrap();
    let lines: Vec<Value> = String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[1]["id"], 2);
    assert_eq!(lines[1]["result"]["isError"], false);
}

/// Regression for #258: a dirty headless project must survive an MCP disconnect.
#[test]
fn headless_autosave_on_eof_preserves_unsaved_project() {
    let dir = std::env::temp_dir().join(format!("ec-mcp-autosave-eof-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut s = server().with_autosave(&dir).unwrap();
    let input = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "execute_command", "arguments": {"command": "comp.new", "params": {"name": "Keep my work", "width": 320, "height": 180}}}}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "execute_command", "arguments": {"command": "layer.newText", "params": {"text": "Unsaved title"}}}}),
    ].iter().map(|v| v.to_string() + "\n").collect::<String>();
    let mut output = Vec::new();
    s.serve(input.as_bytes(), &mut output).unwrap();
    let path = s.backend().session().unwrap().autosave.last_path.clone();
    assert!(path.is_some(), "disconnect discarded the dirty project without an auto-save");
    let path = path.unwrap();
    let mut fresh = server();
    call_json(&mut fresh, "open_project", json!({"path": path}));
    let comp = call_json(&mut fresh, "get_comp", json!({"comp": "Keep my work"}));
    assert_eq!(comp["layers"].as_array().unwrap().len(), 1);
    assert!(s.backend().session().unwrap().is_dirty(), "an auto-save must not mark the original project saved");
    std::fs::remove_dir_all(dir).unwrap();
}

fn autosave_test_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ec-mcp-autosave-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn autosave_status(s: &mut McpServer) -> Value {
    rpc(s, 1, "initialize", json!({}))["_meta"]["effectcraftAutoSave"].clone()
}

#[test]
fn autosave_is_durable_before_reply_and_queries_do_not_rotate() {
    let dir = autosave_test_dir("durable");
    let mut s = server().with_autosave(&dir).unwrap();
    call_json(&mut s, "execute_command", json!({"command": "comp.new", "params": {"name": "Durable"}}));
    let first = autosave_status(&mut s);
    let path = first["latestPath"].as_str().unwrap();
    assert!(std::fs::read_to_string(path).unwrap().contains("Durable"));
    let summary = call_json(&mut s, "get_project", json!({}));
    assert_eq!(summary["autosave"]["latestPath"], path);
    assert_eq!(summary["path"], Value::Null);
    assert_eq!(summary["dirty"], true);
    assert_eq!(autosave_status(&mut s)["latestPath"], path);
    assert!(s.backend().session().unwrap().config.is_none());
    drop(s); // Abrupt process loss has no EOF cleanup; the checkpoint already exists.
    let mut restarted = server().with_autosave(&dir).unwrap();
    let init = rpc(&mut restarted, 1, "initialize", json!({}));
    // The instructions embed the auto-save info as JSON, which escapes a Windows path's backslashes.
    let escaped = Value::from(path).to_string();
    assert!(init["instructions"].as_str().unwrap().contains(escaped.trim_matches('"')));
    assert_eq!(init["_meta"]["effectcraftAutoSave"]["previousSessions"][0]["autosave"], path);
    call_json(&mut restarted, "open_project", json!({"path": path}));
    assert_eq!(call_json(&mut restarted, "get_project", json!({}))["items"][0]["name"], "Durable");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn autosave_errors_preserve_tool_success_retry_and_fail_shutdown() {
    let dir = autosave_test_dir("write-error");
    let mut s = server().with_autosave(&dir).unwrap();
    let folder = std::path::PathBuf::from(autosave_status(&mut s)["folder"].as_str().unwrap());
    std::fs::remove_dir(&folder).unwrap();
    std::fs::write(&folder, "blocked by file").unwrap();
    let (content, failed) = call(&mut s, "execute_command", json!({"command": "comp.new", "params": {"name": "Still in memory"}}));
    assert!(!failed, "the edit succeeded even though its checkpoint failed");
    assert!(content.iter().any(|c| c["text"].as_str().unwrap_or("").contains("Warning:")));
    assert!(s.backend().session().unwrap().is_dirty());
    assert!(autosave_status(&mut s)["error"].as_str().unwrap().contains("failed"));
    let error = s.serve(&b""[..], Vec::new()).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::Other);
    assert!(error.to_string().contains("MCP auto-save"));
    std::fs::remove_file(&folder).unwrap();
    // Even a query retries the outstanding dirty revision.
    let summary = call_json(&mut s, "get_project", json!({}));
    assert_eq!(summary["autosave"]["error"], Value::Null);
    let path = summary["autosave"]["latestPath"].as_str().unwrap();
    assert!(std::fs::read_to_string(path).unwrap().contains("Still in memory"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn autosave_keeps_sessions_and_desktop_recovery_separate() {
    let dir = autosave_test_dir("isolated");
    let desktop_save = dir.join(effectcraft_engine::autosave::AUTOSAVE_FOLDER).join("Untitled Project auto-save 1.ecproj");
    std::fs::create_dir_all(desktop_save.parent().unwrap()).unwrap();
    std::fs::write(&desktop_save, "desktop backup").unwrap();
    std::fs::write(dir.join("session.lock"), "desktop sentinel").unwrap();
    let mut a = server().with_autosave(&dir).unwrap();
    let mut b = server().with_autosave(&dir).unwrap();
    assert_ne!(autosave_status(&mut a)["folder"], autosave_status(&mut b)["folder"]);
    for index in 0..7 {
        for (s, name) in [(&mut a, "A"), (&mut b, "B")] {
            call_json(s, "execute_command", json!({"command": "comp.new", "params": {"name": format!("{name}{index}")}}));
        }
    }
    for (s, mine, other) in [(&mut a, "A6", "B6"), (&mut b, "B6", "A6")] {
        let status = autosave_status(s);
        let json = std::fs::read_to_string(status["latestPath"].as_str().unwrap()).unwrap();
        assert!(json.contains(mine));
        assert!(!json.contains(other));
        let folder = std::path::Path::new(status["folder"].as_str().unwrap());
        assert_eq!(effectcraft_engine::autosave::existing(folder, "Untitled Project").len(), 5);
    }
    assert_eq!(std::fs::read_to_string(desktop_save).unwrap(), "desktop backup");
    assert_eq!(std::fs::read_to_string(dir.join("session.lock")).unwrap(), "desktop sentinel");
    assert!(!dir.join("prefs.json").exists());
    assert!(!dir.join("shortcuts.json").exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn autosave_marks_saved_or_undone_work_clean_without_losing_its_path() {
    let dir = autosave_test_dir("clean");
    let mut s = server().with_autosave(&dir).unwrap();
    call_json(&mut s, "execute_command", json!({"command": "comp.new", "params": {"name": "Undo me"}}));
    let status = autosave_status(&mut s);
    let folder = std::path::Path::new(status["folder"].as_str().unwrap());
    call_json(&mut s, "undo", json!({}));
    let manifest: Value = serde_json::from_str(&std::fs::read_to_string(folder.join("session.json")).unwrap()).unwrap();
    assert_eq!(manifest["dirty"], false);
    call_json(&mut s, "redo", json!({}));
    let path = autosave_status(&mut s)["latestPath"].clone();
    call_json(&mut s, "save_project", json!({"path": dir.join("Saved.ecproj")}));
    assert_eq!(autosave_status(&mut s)["latestPath"], path, "save resets the engine bookkeeping but the server remembers the recovery file");
    let manifest: Value = serde_json::from_str(&std::fs::read_to_string(folder.join("session.json")).unwrap()).unwrap();
    assert_eq!(manifest["dirty"], false);
    let mut next = server().with_autosave(&dir).unwrap();
    assert!(autosave_status(&mut next)["previousSessions"].as_array().unwrap().is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn autosave_flushes_on_read_errors_and_broken_output() {
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let dir = autosave_test_dir("transports");
    for invalid_input in [true, false] {
        let mut s = server().with_autosave(&dir).unwrap();
        // Simulate an edit whose client went away before it received a tool response.
        s.backend().exec("comp.new", json!({"name": "Transport failure"})).unwrap();
        let error = if invalid_input {
            s.serve(&b"\xff\n"[..], Vec::new()).unwrap_err()
        } else {
            s.serve(&b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n"[..], Broken).unwrap_err()
        };
        assert_eq!(error.kind(), if invalid_input { std::io::ErrorKind::InvalidData } else { std::io::ErrorKind::BrokenPipe });
        let status = autosave_status(&mut s);
        assert!(std::fs::read_to_string(status["latestPath"].as_str().unwrap()).unwrap().contains("Transport failure"));
    }
    std::fs::remove_dir_all(dir).unwrap();
}

/// A fake desktop control channel: answers `engine.execute` from a real session, and canned
/// `render.frame` / `ui.screenshot` replies.
fn fake_app() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let (stream, _) = l.accept().unwrap();
        let mut out = stream.try_clone().unwrap();
        let mut session = Session::default();
        for line in BufReader::new(stream).lines() {
            let msg: Value = serde_json::from_str(&line.unwrap()).unwrap();
            let p = &msg["params"];
            let reply = match msg["method"].as_str().unwrap() {
                "engine.execute" => match session.execute(p["command"].as_str().unwrap(), p["params"].clone()) {
                    Ok(v) => json!({"ok": true, "result": v}),
                    Err(e) => json!({"ok": false, "error": e.to_string()}),
                },
                "render.frame" => {
                    let png = crate::encode_png(4, 2, vec![255; 32], 0).unwrap();
                    json!({"ok": true, "result": {"comp": 1, "time": 0.5, "width": 4, "height": 2, "png": base64::encode(&png)}})
                }
                "ui.screenshot" => {
                    let png = crate::encode_png(8, 8, vec![128; 256], 0).unwrap();
                    std::fs::write(p["path"].as_str().unwrap(), png).unwrap();
                    json!({"ok": true, "result": {"path": p["path"]}})
                }
                "ui.key" => json!({"ok": true, "result": null}),
                m => json!({"ok": false, "error": format!("unknown method `{m}`")}),
            };
            let mut reply = reply;
            reply["id"] = msg["id"].clone();
            writeln!(out, "{reply}").unwrap();
        }
    });
    port
}

#[test]
fn bridge_forwards_to_control_channel() {
    let port = fake_app();
    let mut s = McpServer::new(Backend::bridge(&port.to_string()).unwrap());
    let r = rpc(&mut s, 1, "initialize", json!({}));
    assert!(r["serverInfo"]["title"].as_str().unwrap().contains("bridge"));
    let names: Vec<String> =
        rpc(&mut s, 2, "tools/list", json!({}))["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
    assert!(names.iter().any(|n| n == "screenshot") && names.iter().any(|n| n == "ui_click"));

    call_json(&mut s, "execute_command", json!({"command": "comp.new", "params": {"name": "B", "width": 64, "height": 64}}));
    assert_eq!(call_json(&mut s, "get_comp", json!({}))["name"], "B");

    let (c, err) = call(&mut s, "render_frame", json!({}));
    assert!(!err, "{c:?}");
    assert_eq!(png_size(&base64::decode(c[0]["data"].as_str().unwrap()).unwrap()), Some((4, 2)));

    let (c, err) = call(&mut s, "screenshot", json!({"max_side": 4}));
    assert!(!err, "{c:?}");
    assert_eq!(png_size(&base64::decode(c[0]["data"].as_str().unwrap()).unwrap()), Some((4, 4)));

    assert!(!call(&mut s, "ui_key", json!({"key": "Space"})).1);
    let (c, err) = call(&mut s, "control", json!({"method": "ui.nope"}));
    assert!(err && c[0]["text"].as_str().unwrap().contains("unknown method"));
}

#[test]
fn bridge_rejects_non_loopback() {
    assert!(Backend::bridge("10.0.0.1:9877").is_err());
    assert!(Backend::bridge("localhost:9877").is_ok());
}

#[test]
fn script_ui_and_history_tools() {
    let mut sess = Session::default();
    effectcraft_script::install(&mut sess);
    let mut s = McpServer::new(Backend::headless(sess));
    let code = "var d = new Window('dialog', 'Ask'); var g = d.add('group'); g.add('button', undefined, 'OK', {name: 'ok'}); \
                if (d.show() == 1) app.project.items.addComp('Answered', 64, 64, 1, 1, 24); 'done'";
    let r = call_json(&mut s, "run_script", json!({"code": code}));
    assert_eq!(r["waiting"], json!(true), "{r}");
    let list = call_json(&mut s, "script_ui", json!({}));
    assert_eq!(list[0]["title"], "Ask");
    let tree = call_json(&mut s, "script_ui", json!({"action": "get"}));
    assert_eq!(tree["root"]["children"][0]["children"][0]["name"], "ok");
    let r = call_json(&mut s, "script_ui", json!({"action": "click", "widget": "ok"}));
    assert_eq!(r["result"], json!("done"), "{r}");
    // History: undo the comp, make another, and jump back to the first on its branch.
    call_json(&mut s, "undo", json!({}));
    call_json(&mut s, "execute_command", json!({"command": "comp.new", "params": {"name": "Other", "width": 64, "height": 64}}));
    let h = call_json(&mut s, "history", json!({}));
    let branch = h["states"].as_array().unwrap().iter().find(|n| n["depth"] == 1).unwrap()["index"].clone();
    let h = call_json(&mut s, "history", json!({"goto": branch}));
    let cur = h["current"].as_u64().unwrap() as usize;
    assert_eq!(h["states"][cur]["depth"], 0, "the jumped-to state is now the working line");
    assert!(call_json(&mut s, "get_project", json!({})).to_string().contains("Answered"));
}

#[test]
fn run_script_round_trip() {
    let mut sess = Session::default();
    effectcraft_script::install(&mut sess);
    let mut s = McpServer::new(Backend::headless(sess));
    let r = call_json(
        &mut s,
        "run_script",
        json!({"code": "var c = app.project.items.addComp('Scripted', 320, 180, 1, 2, 24);\nc.layers.addText('Hi');\nwriteLn('made ' + c.name);\nc.numLayers"}),
    );
    assert_eq!(r["ok"], json!(true), "{r}");
    assert_eq!(r["result"], json!(1));
    assert_eq!(r["output"], json!("made Scripted"));
    // The edit is visible to the other tools.
    let p = call_json(&mut s, "get_project", json!({}));
    assert!(p.to_string().contains("Scripted"), "{p}");
    // Errors come back with their line.
    let r = call_json(&mut s, "run_script", json!({"code": "var a = 1;\nundefinedFn();", "name": "bad.jsx"}));
    assert_eq!(r["ok"], json!(false));
    assert_eq!(r["error"]["line"], json!(2));
    assert_eq!(r["error"]["file"], json!("bad.jsx"));
    // Without a scripting engine the command explains itself.
    let mut bare = server();
    let (c, err) = call(&mut bare, "run_script", json!({"code": "1"}));
    assert!(err && c[0]["text"].as_str().unwrap().contains("not available"), "{c:?}");
}

#[test]
fn puppet_rigging_over_mcp() {
    let mut s = server();
    let ex = |s: &mut McpServer, command: &str, params: Value| call_json(s, "execute_command", json!({"command": command, "params": params}));
    ex(&mut s, "comp.new", json!({"name": "Rig", "width": 200, "height": 100, "duration": 2}));
    let id = ex(&mut s, "layer.newSolid", json!({"width": 100, "height": 60, "color": "#3060c0"}))["layer"].clone();
    let lead = ex(&mut s, "puppet.addPin", json!({"layer": id, "position": [10, 30]}))["pin"].clone();
    let tail = ex(&mut s, "puppet.addPin", json!({"layer": id, "position": [90, 30]}))["pin"].clone();
    let bend = ex(&mut s, "puppet.addPin", json!({"layer": id, "kind": "bend", "position": [50, 30]}))["pin"].clone();
    // Turn and scale the Bend pin, select pins, make the tail trail the leader, rig the leader.
    ex(&mut s, "puppet.setPin", json!({"layer": id, "pin": bend, "rotation": 45, "scale": 120}));
    assert_eq!(ex(&mut s, "puppet.selectPins", json!({"layer": id, "pins": [lead, tail]}))["pins"], json!([lead, tail]));
    assert_eq!(ex(&mut s, "puppet.follow", json!({"delay": 0.25}))["pins"], json!([{"pin": tail, "delay": 0.25}]));
    let r = ex(&mut s, "paths.pointsFollowNulls", json!({"layer": id, "pins": [lead]}));
    assert_eq!(r["nulls"].as_array().unwrap().len(), 1);
    // The new commands are listed for agents.
    let (c, _) = call(&mut s, "list_commands", json!({"filter": "puppet."}));
    let text = c[0]["text"].as_str().unwrap();
    assert!(text.contains("puppet.follow") && text.contains("puppet.selectPins"), "{text}");
}

/// The agent-interface audit (MCP side): every registered engine command is listed by
/// `list_commands` with a label, params doc and JSON Schema, described by `describe_command`,
/// and routed by `execute_command` (an unknown parameter is rejected by that very command
/// without running it).
#[test]
fn every_engine_command_is_reachable_over_mcp() {
    let mut s = server();
    let tools = rpc(&mut s, 1, "tools/list", json!({}))["tools"].as_array().unwrap().clone();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    for want in ["list_commands", "describe_command", "execute_command", "add_effect", "list_effects", "get_state", "render_frame", "get_layer", "set_property"]
    {
        assert!(names.contains(&want), "{want}");
    }
    let listed = call_json(&mut s, "list_commands", json!({"schemas": true}));
    let listed: std::collections::HashMap<&str, &Value> = listed.as_array().unwrap().iter().map(|c| (c["id"].as_str().unwrap(), c)).collect();
    let specs = effectcraft_engine::command_specs();
    assert_eq!(listed.len(), specs.len());
    for spec in specs {
        let c = listed.get(spec.id).unwrap_or_else(|| panic!("{} not listed", spec.id));
        assert!(!spec.label.trim().is_empty(), "{}: no label", spec.id);
        assert!(spec.params.trim_start().starts_with('{'), "{}: params doc {:?}", spec.id, spec.params);
        assert_eq!(c["schema"]["type"], "object", "{}", spec.id);
        assert!(c["schema"]["properties"].is_object(), "{}", spec.id);
        let d = call_json(&mut s, "describe_command", json!({"command": spec.id}));
        assert_eq!(d["schema"], c["schema"], "{}", spec.id);
        let (content, err) = call(&mut s, "execute_command", json!({"command": spec.id, "params": {"__audit": 1}}));
        let text = content[0]["text"].as_str().unwrap_or_default();
        assert!(err && text.contains("unknown parameter") && text.contains(spec.id), "{}: {text}", spec.id);
    }
}

#[test]
fn high_level_tools_add_effect_and_report_state() {
    let mut s = server();
    call_json(&mut s, "execute_command", json!({"command": "comp.new", "params": {"name": "A", "width": 64, "height": 48, "duration": 1}}));
    let l = call_json(&mut s, "execute_command", json!({"command": "layer.newSolid", "params": {"color": "#808080"}}))["layer"].clone();
    let fx = call_json(&mut s, "list_effects", json!({"filter": "gaussian"}));
    assert!(fx.as_array().unwrap().iter().any(|e| e["id"] == "ec.blur.gaussian" && e["gpu"].is_boolean()), "{fx}");
    let r = call_json(&mut s, "add_effect", json!({"layer": l, "effect": "Gaussian Blur", "values": {"blurriness": 7}}));
    assert_eq!(r["path"], "effects/#1", "{r}");
    assert!(r["params"].as_array().unwrap().iter().any(|p| p["path"] == "effects/#1/blurriness" && p["value"] == 7.0), "{r}");
    // A second effect lands at #2.
    let r = call_json(&mut s, "add_effect", json!({"layer": l, "effect": "ec.color.levels"}));
    assert_eq!(r["path"], "effects/#2", "{r}");
    let p = call_json(&mut s, "get_property", json!({"layer": l, "path": "effects/#1/blurriness"}));
    assert_eq!(p["value"], 7.0, "{p}");
    let st = call_json(&mut s, "get_state", json!({}));
    assert!(st["state"].is_object() && st["app"]["commands"].as_u64().unwrap() > 500, "{st}");
    assert!(st["app"]["effects"]["total"].as_u64().unwrap() > 200, "{st}");
    assert!(st["app"]["parity"]["summary"].as_str().unwrap().contains("parity"), "{st}");
    // Unknown effects fail cleanly.
    let (c, err) = call(&mut s, "add_effect", json!({"layer": l, "effect": "No Such Effect"}));
    assert!(err && c[0]["text"].as_str().unwrap().contains("unknown effect"), "{c:?}");
}

/// add_effect is one undo step, and a call that fails leaves the layer and the history as they
/// were (#175).
#[test]
fn add_effect_is_one_undo_step_and_all_or_nothing() {
    let mut s = server();
    call_json(&mut s, "execute_command", json!({"command": "comp.new", "params": {"name": "A", "width": 64, "height": 64}}));
    call_json(&mut s, "execute_command", json!({"command": "layer.newSolid", "params": {"name": "S", "color": "#ff0000"}}));
    let undo = |s: &mut McpServer| call_json(s, "get_project", json!({}))["undo"].as_array().unwrap().clone();
    let effects = |s: &mut McpServer| {
        let l = call_json(s, "get_layer", json!({"layer": "S", "flat": true}));
        l["properties"].as_array().unwrap().iter().filter(|p| p["path"].as_str().unwrap().starts_with("effects/")).count()
    };
    let before = undo(&mut s);
    let (c, err) = call(&mut s, "add_effect", json!({"layer": "S", "effect": "Gaussian Blur", "values": {"blurriness": 8, "repeatEdgePixels": true}}));
    assert!(err && c[0]["text"].as_str().unwrap().contains("effects/#1/repeatEdgePixels"), "{c:?}");
    assert_eq!((undo(&mut s), effects(&mut s)), (before.clone(), 0), "nothing applied");
    // The corrected retry lands at #1, in one undo step named like effect.apply's.
    let r = call_json(&mut s, "add_effect", json!({"layer": "S", "effect": "Gaussian Blur", "values": {"blurriness": 8, "repeatEdge": true}}));
    assert_eq!((&r["path"], &r["set"]), (&json!("effects/#1"), &json!(["effects/#1/blurriness", "effects/#1/repeatEdge"])), "{r}");
    let after = undo(&mut s);
    assert_eq!(after.len(), before.len() + 1, "{after:?}");
    assert_eq!(after.last(), Some(&json!("Apply Gaussian Blur")));
    call_json(&mut s, "undo", json!({}));
    assert_eq!((undo(&mut s), effects(&mut s)), (before, 0), "one undo removes the effect and its values");
}

#[test]
fn list_fonts_lists_families_with_styles() {
    let mut s = server();
    let all = call_json(&mut s, "list_fonts", json!({}));
    let fams = all["families"].as_array().unwrap();
    assert!(fams.iter().any(|f| f["family"] == "Inter" && f["origin"] == "bundled"), "{all}");
    // The query filters by substring: the bundled JetBrains Mono, and any installed family whose
    // name contains it (JetBrainsMono Nerd Font…) (#149).
    let one = call_json(&mut s, "list_fonts", json!({"query": "jetbrains"}));
    let matched = one["families"].as_array().unwrap();
    assert_eq!(one["count"].as_u64(), Some(matched.len() as u64), "{one}");
    assert!(matched.iter().all(|f| f["family"].as_str().unwrap().to_lowercase().contains("jetbrains")), "{one}");
    assert!(matched.len() < fams.len(), "the query filters: {one}");
    let mono = matched.iter().find(|f| f["family"] == "JetBrains Mono").unwrap_or_else(|| panic!("{one}"));
    assert!(mono["styles"].as_array().unwrap().contains(&json!("Regular")), "{one}");
}

/// Trained models over MCP (Roto Brush and face tracking): listed with their authors and
/// licence, chosen, refused when unknown.
#[test]
fn trained_models_over_mcp() {
    let mut s = server();
    for (prefix, id) in [("roto", "mobilesam"), ("face", "mediapipe-face")] {
        let list = call_json(&mut s, "execute_command", json!({"command": format!("{prefix}.models"), "params": {}}));
        let models = list["models"].as_array().unwrap();
        assert!(models.iter().any(|m| m["id"] == id && m["licence"] == "Apache-2.0" && m["authors"].is_string() && m["installed"] == false), "{list}");
        let r = call_json(&mut s, "execute_command", json!({"command": format!("{prefix}.model.select"), "params": {"id": id}}));
        assert_eq!(r["model"], id);
        let (c, err) = call(&mut s, "execute_command", json!({"command": format!("{prefix}.model.select"), "params": {"id": "nope"}}));
        assert!(err && c[0]["text"].as_str().unwrap().contains("unknown model"), "{c:?}");
    }
}

/// docs/agents.md's Tools table lists every tool, and an entry that lists arguments lists every
/// argument the tool's input schema accepts (#137).
#[test]
fn agents_doc_tools_table_is_current() {
    // (Checkouts may have CRLF line endings.)
    let doc = include_str!("../../../docs/agents.md").replace("\r\n", "\n");
    let table: String = doc.split("### Tools").nth(1).and_then(|t| t.split("\n\n").nth(1)).unwrap_or_default().to_string();
    assert!(table.starts_with("| Tool |"), "no Tools table");
    let mut missing = vec![];
    for t in crate::tools::catalogue() {
        // `name` (no arguments) or `name {a, b?, …}`.
        let Some(at) = table.find(&format!("`{}", t.name)).filter(|i| {
            let rest = &table[i + 1 + t.name.len()..];
            rest.starts_with('`') || rest.starts_with(" {")
        }) else {
            missing.push(format!("{}: no entry", t.name));
            continue;
        };
        let rest = &table[at + 1 + t.name.len()..];
        if !rest.starts_with(" {") {
            continue;
        }
        let args = rest.split("}`").next().unwrap_or_default();
        let schema = t.input_schema();
        for prop in schema["properties"].as_object().into_iter().flat_map(|p| p.keys()) {
            if !args.split(|c: char| !(c.is_alphanumeric() || c == '_')).any(|w| w == prop) {
                missing.push(format!("{}: argument `{prop}`", t.name));
            }
        }
    }
    assert!(missing.is_empty(), "docs/agents.md's Tools table is missing: {missing:#?}");
}

#[test]
fn autosave_checkpoints_batch_and_partially_failed_tools() {
    let dir = autosave_test_dir("partial");
    let mut s = server().with_autosave(&dir).unwrap();
    let msgs = json!([
        {"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"execute_command","arguments":{"command":"comp.new","params":{"name":"Batch"}}}},
        {"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"execute_command","arguments":{"command":"layer.newText","params":{"text":"Batch title"}}}}
    ]);
    let reply = s.handle(&msgs).unwrap();
    assert_eq!(reply.as_array().unwrap().len(), 2);
    let (_, failed) = call(&mut s, "add_keyframe", json!({"layer":"#1","path":"transform/opacity","keys":[{"time":0,"value":37}],"interpolation":"invalid"}));
    assert!(failed, "the key was added, then the interpolation failed");
    let status = autosave_status(&mut s);
    let mut recovered = server();
    call_json(&mut recovered, "open_project", json!({"path":status["latestPath"]}));
    let prop = call_json(&mut recovered, "get_property", json!({"layer":"#1","path":"transform/opacity"}));
    assert_eq!(prop["keys"].as_array().unwrap().len(), 1);
    assert_eq!(prop["keys"][0]["value"], 37.0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn autosave_isolation_also_applies_to_engine_autosaves_and_preference_changes() {
    let dir = autosave_test_dir("engine-save");
    let desktop = dir.join("DesktopCustom");
    std::fs::create_dir_all(&desktop).unwrap();
    std::fs::write(desktop.join("Untitled Project auto-save 1.ecproj"), "desktop backup").unwrap();
    let mut session = Session::default();
    session.prefs.auto_save.location = "custom".into();
    session.prefs.auto_save.folder = desktop.to_string_lossy().into_owned();
    let mut s = McpServer::new(Backend::headless(session)).with_autosave(&dir).unwrap();
    call_json(&mut s, "execute_command", json!({"command":"comp.new","params":{"name":"Isolated"}}));
    let status = autosave_status(&mut s);
    let explicit = call_json(&mut s, "execute_command", json!({"command":"file.autoSave"}));
    assert!(std::path::Path::new(explicit["path"].as_str().unwrap()).starts_with(status["folder"].as_str().unwrap()));
    assert_eq!(autosave_status(&mut s)["latestPath"], explicit["path"]);
    // The host override survives preference edits even within a single batch tool call.
    call_json(
        &mut s,
        "batch",
        json!({"steps":[
            {"command":"prefs.set","params":{"values":{"autoSave.location":"custom","autoSave.folder":desktop.to_string_lossy()}}},
            {"command":"file.autoSave","params":{}}
        ]}),
    );
    call_json(&mut s, "save_project", json!({"path":dir.join("Named.ecproj")}));
    call_json(&mut s, "execute_command", json!({"command":"layer.newText","params":{"text":"Latest isolated edit"}}));
    let current = autosave_status(&mut s);
    assert!(std::path::Path::new(current["latestPath"].as_str().unwrap()).starts_with(status["folder"].as_str().unwrap()));
    assert!(std::fs::read_to_string(current["latestPath"].as_str().unwrap()).unwrap().contains("Latest isolated edit"));
    let recovery = call_json(&mut s, "execute_command", json!({"command":"file.recoveryInfo"}));
    assert_eq!(recovery["folder"], status["folder"]);
    assert_eq!(std::fs::read_to_string(desktop.join("Untitled Project auto-save 1.ecproj")).unwrap(), "desktop backup");
    assert_eq!(std::fs::read_dir(&desktop).unwrap().count(), 2, "only the original desktop file and the MCP subfolder");
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn autosave_non_utf8_path_returns_an_error_instead_of_panicking() {
    use std::os::unix::ffi::OsStringExt;
    let root = std::path::PathBuf::from(std::ffi::OsString::from_vec(b"/tmp/ec-mcp-\xff".to_vec()));
    let result = server().with_autosave(&root);
    assert!(matches!(result, Err(crate::Error::Other(message)) if message.contains("UTF-8")));
}

#[test]
fn core_tools_annotations_and_strict_keys() {
    let mut s = server();
    let tools = rpc(&mut s, 1, "tools/list", json!({}));
    let tools = tools["tools"].as_array().unwrap();
    for name in ["command_list", "command_run", "command_batch", "doc_inspect", "render_preview"] {
        assert!(tools.iter().any(|t| t["name"] == name), "missing {name}");
    }
    for t in tools {
        assert!(t["title"].as_str().is_some_and(|s| !s.is_empty()));
        for hint in ["readOnlyHint", "destructiveHint", "idempotentHint", "openWorldHint"] {
            assert!(t["annotations"][hint].is_boolean(), "{t}");
        }
        let msg = json!({"jsonrpc":"2.0", "id":2, "method":"tools/call", "params":{"name":t["name"], "arguments":{"typo_key":1}}});
        let reply = s.handle(&msg).unwrap();
        assert_eq!(reply["error"]["code"], -32602, "{reply}");
        assert!(reply["error"]["message"].as_str().unwrap().contains("typo_key"));
    }
    let r = call_json(&mut s, "command_run", json!({"id":"comp.new", "params":{"width":32,"height":32}}));
    assert!(r.is_object());
    let (content, err) = call(&mut s, "command_batch", json!({"steps":[{"id":"no.such.command"},{"id":"layer.newNull"}]}));
    assert!(err);
    let result: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(result["completed"], 0);
    assert_eq!(result["failed"], 1);
    assert_eq!(result["results"].as_array().unwrap().len(), 1);
    let (content, err) = call(&mut s, "command_batch", json!({"stop_on_error":false,"steps":[{"id":"no.such.command"},{"id":"layer.newNull"}]}));
    assert!(err);
    let result: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(result["completed"], 1);
    assert_eq!(result["failed"], 1);
    assert!(call_json(&mut s, "doc_inspect", json!({}))["activeComp"].is_object());
    assert!(!call(&mut s, "render_preview", json!({"max_side":16})).1);
}

#[test]
fn resources_match_tools_and_recover_after_parse_error() {
    let mut s = server();
    let bad: Value = serde_json::from_str(&s.handle_line("{broken").unwrap()).unwrap();
    assert_eq!(bad["error"]["code"], -32700);
    assert!(bad["id"].is_null());
    assert_eq!(rpc(&mut s, 1, "ping", json!({})), json!({}));
    let list = rpc(&mut s, 2, "resources/list", json!({}));
    for (uri, tool) in [("effectcraft://document", "doc_inspect"), ("effectcraft://commands", "command_list")] {
        assert!(list["resources"].as_array().unwrap().iter().any(|r| r["uri"] == uri));
        let r = rpc(&mut s, 3, "resources/read", json!({"uri":uri}));
        assert_eq!(r["contents"][0]["mimeType"], "application/json");
        let value: Value = serde_json::from_str(r["contents"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(value, call_json(&mut s, tool, json!({})));
    }
}

/// Rendering is a direct tool call, outside Session::execute's existing panic guard.
#[test]
fn render_panic_is_an_error_and_session_remains_usable() {
    use effectcraft_engine::{
        color::Label,
        project::{Footage, FootageKind, ItemId, ItemKind},
        raster::Image,
        render::FootageSource,
        time::Tick,
    };
    use std::sync::{Arc, Mutex};
    struct BrokenFootage(Mutex<bool>);
    impl FootageSource for BrokenFootage {
        fn frame(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<Image>> {
            let mut first = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if *first {
                *first = false;
                panic!("synthetic decoder failure");
            }
            Some(Arc::new(Image::new(16, 16)))
        }
    }
    let mut session = Session { footage: Arc::new(BrokenFootage(Mutex::new(true))), ..Default::default() };
    session.execute("comp.new", json!({"width":16,"height":16})).unwrap();
    let item = Arc::make_mut(&mut session.project).add_item(
        "synthetic",
        Label::Aqua,
        None,
        ItemKind::Footage(Footage { path: "synthetic.png".into(), kind: FootageKind::Still, width: 16, height: 16, has_video: true, ..Default::default() }),
    );
    session.execute("layer.addItem", json!({"item":item.0})).unwrap();
    let mut s = McpServer::new(Backend::headless(session));
    let (content, err) = call(&mut s, "render_frame", json!({}));
    assert!(err);
    assert!(content[0]["text"].as_str().unwrap().contains("synthetic decoder failure"));
    assert_eq!(rpc(&mut s, 2, "ping", json!({})), json!({}));
    assert!(!call(&mut s, "render_frame", json!({})).1);
}

/// MCP 2026-07-28 ("modern") clients declare their revision per request in `_meta` and reject
/// list/read results without `resultType`, `ttlMs` and `cacheScope`; legacy sessions (revision
/// negotiated through `initialize`) keep the old result shape.
#[test]
fn modern_clients_get_result_type() {
    let mut s = server();
    let meta = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": {"name": "t", "version": "1"},
        "io.modelcontextprotocol/clientCapabilities": {},
    });
    for (id, method, params) in [
        (1, "tools/list", json!({"_meta": meta})),
        (2, "resources/list", json!({"_meta": meta})),
        (3, "resources/templates/list", json!({"_meta": meta})),
        (4, "resources/read", json!({"_meta": meta, "uri": "effectcraft://document"})),
        (5, "tools/call", json!({"_meta": meta, "name": "command_list", "arguments": {}})),
    ] {
        let r = rpc(&mut s, id, method, params);
        assert_eq!(r["resultType"], "complete", "{method}: {r}");
        if method != "tools/call" {
            assert!(r["ttlMs"].is_u64(), "{method}: {r}");
            assert!(matches!(r["cacheScope"].as_str(), Some("public" | "private")), "{method}: {r}");
        }
    }
    let mut s = server();
    rpc(&mut s, 1, "initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}));
    for (id, method, params) in
        [(2, "tools/list", json!({})), (3, "resources/list", json!({})), (4, "resources/read", json!({"uri": "effectcraft://document"}))]
    {
        let r = rpc(&mut s, id, method, params);
        let res = r.as_object().unwrap();
        assert!(!res.contains_key("resultType") && !res.contains_key("ttlMs") && !res.contains_key("cacheScope"), "{method}: {r}");
    }
}

#[test]
fn render_job_state_recovers_a_poisoned_lock() {
    let shared = std::sync::Arc::new(effectcraft_engine::render_queue::JobShared::default());
    let worker = shared.clone();
    let _ = std::thread::spawn(move || {
        let mut state = worker.state.lock().unwrap();
        state.done = 3;
        panic!("synthetic worker panic while holding the progress lock");
    })
    .join();
    assert_eq!(shared.snapshot().done, 3);
}

/// A render that takes ~2 ms per frame and writes its file first (so a cancel leaves a partial
/// one to clean up).
struct SlowExporter;

impl effectcraft_engine::Exporter for SlowExporter {
    fn formats(&self) -> Vec<effectcraft_engine::project::render_queue::OutputFormat> {
        effectcraft_engine::project::render_queue::OutputFormat::ALL.to_vec()
    }
    fn export(&self, job: &effectcraft_engine::ExportJob, progress: &mut dyn FnMut(u64, u64) -> bool) -> Result<effectcraft_engine::ExportResult, String> {
        std::fs::write(job.path, b"partial").map_err(|e| e.to_string())?;
        let n = 2000;
        for k in 0..=n {
            if !progress(k, n) {
                std::fs::remove_file(job.path).map_err(|e| e.to_string())?;
                return Err(effectcraft_engine::render_queue::CANCELLED.into());
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        Ok(effectcraft_engine::ExportResult { path: job.path.to_string(), frames: n, ..Default::default() })
    }
}

/// docs/mcp.md "Progress and cancellation" over the stdio loop: a blocking `renderQueue.render`
/// reports `notifications/progress` for its token, other requests are answered while it renders,
/// and `notifications/cancelled` stops it, deletes the partial file and sends no response.
#[test]
fn render_progress_and_cancel() {
    let dir = std::env::temp_dir().join(format!("effectcraft-mcp-progress-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (in_r, mut in_w) = std::io::pipe().unwrap();
    let (out_r, out_w) = std::io::pipe().unwrap();
    let server = std::thread::spawn(move || {
        let session = Session { exporter: Some(std::sync::Arc::new(SlowExporter)), ..Default::default() };
        McpServer::new(Backend::headless(session)).serve(BufReader::new(in_r), out_w).unwrap();
    });
    let mut lines = BufReader::new(out_r).lines();
    let mut next = || -> Value { serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap() };
    let mut send = |v: Value| writeln!(in_w, "{v}").unwrap();
    let run = |id: u64, cmd: &str, params: Value, token: Option<Value>| {
        let mut p = json!({"name": "command_run", "arguments": {"id": cmd, "params": params}});
        if let Some(t) = token {
            p["_meta"] = json!({"progressToken": t, "io.modelcontextprotocol/protocolVersion":"2026-07-28"});
        }
        json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": p})
    };
    send(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}}));
    next();
    send(run(2, "comp.new", json!({"width": 64, "height": 64, "duration": 1}), None));
    assert_eq!(next()["result"]["isError"], false);
    let a = dir.join("a.mp4").to_string_lossy().to_string();
    send(run(3, "renderQueue.add", json!({"output": a}), None));
    assert_eq!(next()["result"]["isError"], false);
    // progress for the token, strictly increasing; doc_inspect answered while rendering
    send(run(4, "renderQueue.render", json!({}), Some(json!("tok"))));
    let (mut last, mut notes, mut asked, mut answered) = (-1.0, 0, false, false);
    let done = loop {
        let m = next();
        if m["method"] == "notifications/progress" {
            assert_eq!(m["params"]["progressToken"], "tok", "{m}");
            let p = m["params"]["progress"].as_f64().unwrap();
            assert!(p > last && p <= m["params"]["total"].as_f64().unwrap(), "{m}");
            (last, notes) = (p, notes + 1);
            if !asked && p > 0.0 {
                asked = true;
                send(json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "doc_inspect", "arguments": {}}}));
            }
        } else if m["id"] == 5 {
            answered = true;
            assert_eq!(m["result"]["isError"], false, "{m}");
        } else {
            assert_eq!(m["id"], 4, "{m}");
            break m;
        }
    };
    assert_eq!(done["result"]["isError"], false, "{done}");
    assert_eq!(done["result"]["resultType"], "complete");
    assert_eq!(last, 1.0, "final progress reaches the total");
    let v: Value = serde_json::from_str(done["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(v["items"][0]["statusLabel"], "Done", "{v}");
    assert!(notes >= 2 && answered, "{notes} notifications; doc_inspect answered during the render: {answered}");
    // cancel after the first notification: no response, the partial file is gone
    let b = dir.join("b.mp4");
    send(run(6, "renderQueue.add", json!({"output": b.to_string_lossy()}), None));
    assert_eq!(next()["result"]["isError"], false);
    send(run(7, "renderQueue.render", json!({}), Some(json!(7))));
    loop {
        let m = next();
        assert_ne!(m["id"], 7, "{m}");
        if m["method"] == "notifications/progress" && m["params"]["progress"].as_f64().unwrap() > 0.0 {
            break;
        }
    }
    assert!(b.exists(), "the render started writing");
    send(json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 7, "reason": "test"}}));
    send(json!({"jsonrpc": "2.0", "id": 8, "method": "ping"}));
    let m = loop {
        let m = next();
        if m["method"] != "notifications/progress" {
            break m;
        }
    };
    assert_eq!(m["id"], 8, "no response for the cancelled request: {m}");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let v = loop {
        send(run(9, "renderQueue.list", json!({}), None));
        let v: Value = serde_json::from_str(next()["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        // the queue stops rendering a moment before the stopped item's status is updated
        if v["rendering"] == false && v["items"][1]["statusLabel"] != "Rendering" {
            break v;
        }
        assert!(std::time::Instant::now() < deadline, "cancel did not stop the job: {v}");
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    assert!(!b.exists(), "partial output deleted");
    assert!(std::path::Path::new(&a).exists(), "completed earlier queue item is retained");
    assert_eq!(v["items"][1]["statusLabel"], "User Stopped", "{v}");
    drop(send);
    drop(in_w);
    server.join().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn render_without_token_finishes_after_stdin_closes() {
    let dir = std::env::temp_dir().join(format!("ec-mcp-eof-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("complete.mp4");
    let mut s = McpServer::new(Backend::headless(Session { exporter: Some(std::sync::Arc::new(SlowExporter)), ..Default::default() }));
    let mut input = String::new();
    for (id, command, params) in
        [(1, "comp.new", json!({"width":16,"height":16})), (2, "renderQueue.add", json!({"output":path})), (3, "renderQueue.render", json!({}))]
    {
        input.push_str(&format!(
            "{}\n",
            json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"command_run","arguments":{"id":command,"params":params}}})
        ));
    }
    let mut out = Vec::new();
    s.serve(std::io::Cursor::new(input), &mut out).unwrap();
    let replies: Vec<Value> = String::from_utf8(out).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    assert_eq!(replies.len(), 3, "no token means no notifications: {replies:?}");
    assert_eq!(replies[2]["result"]["isError"], false);
    assert!(path.exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn panicked_render_worker_finishes_with_an_error() {
    struct BrokenExporter;
    impl effectcraft_engine::Exporter for BrokenExporter {
        fn formats(&self) -> Vec<effectcraft_engine::project::render_queue::OutputFormat> {
            effectcraft_engine::project::render_queue::OutputFormat::ALL.to_vec()
        }
        fn export(&self, _: &effectcraft_engine::ExportJob, _: &mut dyn FnMut(u64, u64) -> bool) -> Result<effectcraft_engine::ExportResult, String> {
            panic!("synthetic exporter failure");
        }
    }
    let mut session = Session { exporter: Some(std::sync::Arc::new(BrokenExporter)), ..Default::default() };
    session.execute("comp.new", json!({"width":16,"height":16})).unwrap();
    session.execute("renderQueue.add", json!({"output":"unused.mp4"})).unwrap();
    session.execute("renderQueue.render", json!({"wait":false})).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while session.is_rendering() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(!session.is_rendering(), "a panicked worker must not leave an unfinishable job");
    session.poll_render();
    let queue = session.execute("renderQueue.list", json!({})).unwrap();
    assert!(queue.to_string().contains("synthetic exporter failure"), "{queue}");
    session.execute("renderQueue.add", json!({"output":"unused-again.mp4"})).unwrap();
    let mut server = McpServer::new(Backend::headless(session));
    let input =
        json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"command_run","arguments":{"id":"renderQueue.render"}}}).to_string() + "\n";
    let mut out = Vec::new();
    server.serve(std::io::Cursor::new(input), &mut out).unwrap();
    let reply: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(reply["result"]["isError"], true, "{reply}");
}

/// `id` is an accepted alias of `command` (#365 made unknown keys an error).
#[test]
fn command_tools_accept_id_as_an_alias() {
    let mut s = server();
    let d = call_json(&mut s, "describe_command", json!({"id": "comp.new"}));
    assert_eq!(d["id"], "comp.new", "{d}");
    call_json(&mut s, "execute_command", json!({"id": "comp.new", "params": {"name": "Alias", "width": 64, "height": 36}}));
    let (_, err) = call(&mut s, "execute_command", json!({}));
    assert!(err, "neither key is an error");
}
