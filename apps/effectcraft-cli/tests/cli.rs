//! End-to-end tests of the agent CLI: JSON output, project round-trips, rendering and MCP over stdio.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_effectcraft-cli"))
}

/// Run with `--json`; returns (exit code, parsed stdout).
fn run_json(args: &[&str]) -> (i32, Value) {
    let out = bin().args(args).arg("--json").output().unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    let v =
        serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("{args:?}: not JSON ({e}): {stdout}\nstderr: {}", String::from_utf8_lossy(&out.stderr)));
    (out.status.code().unwrap_or(-1), v)
}

fn ok_json(args: &[&str]) -> Value {
    let (code, v) = run_json(args);
    assert_eq!(code, 0, "{args:?}: {v}");
    v
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ec-cli-test-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

#[test]
fn info_and_commands() {
    let v = ok_json(&["info"]);
    assert_eq!(v["mode"], "headless");
    assert!(v["commands"].as_u64().unwrap() > 50);
    assert!(v["activeComp"]["layers"].as_array().unwrap().len() > 1, "demo project by default");

    let v = ok_json(&["commands", "--filter", "layer.new"]);
    let ids: Vec<&str> = v.as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&"layer.newSolid") && ids.contains(&"layer.newText"));
    assert!(ids.iter().all(|i| i.contains("layer.new") || !i.is_empty()));
}

/// The agent-interface audit (CLI side): `exec --list` lists every registered engine command
/// with its label, params doc and (with `--schemas`) params JSON Schema, and `exec` routes every
/// id to its command (an unknown parameter is rejected by that command).
#[test]
fn exec_list_covers_every_engine_command() {
    let v = ok_json(&["exec", "--list", "--schemas", "--empty"]);
    let listed: std::collections::HashMap<&str, &Value> = v.as_array().unwrap().iter().map(|c| (c["id"].as_str().unwrap(), c)).collect();
    let specs = effectcraft_engine::command_specs();
    assert_eq!(listed.len(), specs.len());
    for spec in specs {
        let c = listed.get(spec.id).unwrap_or_else(|| panic!("`exec --list` misses {}", spec.id));
        assert_eq!(c["label"], spec.label);
        // Empty docs (`{}`) are dropped from the compact listing.
        assert!(c["params"] == spec.params || (spec.params == "{}" && c["params"].is_null()), "{}: {}", spec.id, c["params"]);
        assert_eq!(c["schema"]["type"], "object", "{}", spec.id);
    }
    // Routing: a few ids through a real process (all ids are checked in-process by the MCP audit).
    for id in ["comp.new", "layer.newText", "effect.apply", "file.saveAs", "render.frame"].into_iter().filter(|i| listed.contains_key(i)) {
        let (code, v) = run_json(&["exec", id, r#"{"__audit":1}"#, "--empty"]);
        assert_eq!(code, 1, "{id}: {v}");
        assert!(v["error"].as_str().unwrap().contains("unknown parameter"), "{id}: {v}");
    }
}

#[test]
fn exec_set_get_with_saved_project() {
    let proj = tmp("edit.ecproj");
    let p = proj.to_str().unwrap();
    let v = ok_json(&["exec", "comp.new", "--params", r#"{"name":"Main","width":320,"height":180,"duration":3}"#, "--empty", "--save-as", p]);
    assert_eq!(v["saved"], p);
    // Positional params + positional project + in-place save.
    let v = ok_json(&["exec", "layer.newSolid", r##"{"color":"#00ff00","name":"Green"}"##, p, "--save"]);
    assert!(v["result"]["layer"].is_u64());

    let v = ok_json(&["set", "Main", "Green", "transform/opacity", "25", "--project", p, "--save"]);
    assert_eq!(v["result"]["value"], 25.0);
    ok_json(&["set", "Main", "#1", "transform/position", "[0,90]", "--time", "0", p, "--save"]);
    ok_json(&["set", "Main", "#1", "transform/position", "[320,90]", "--time", "2", p, "--save"]);

    let v = ok_json(&["get", "Main", "#1", "transform/position", "--time", "1", p]);
    assert_eq!(v["keys"].as_array().unwrap().len(), 2);
    assert!((v["value"][0].as_f64().unwrap() - 160.0).abs() < 1.0, "{v}");
    assert_eq!(ok_json(&["get", "-", "Green", "transform/opacity", p])["value"], 25.0);

    let v = ok_json(&["props", "Main", "#1", p]);
    assert_eq!(v["name"], "Green");
    let tree = v["properties"]["children"].as_array().unwrap();
    assert!(!tree.is_empty());
    // The human (flat) listing shows paths.
    let out = bin().args(["props", "Main", "#1", p]).output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("transform/position"));

    // Several commands in one process.
    let v = ok_json(&["run", p, "time.set", r#"{"time":1}"#, "project.summary"]);
    assert_eq!(v[1]["result"]["time"], v[0]["result"]["time"], "{v}");
}

#[test]
fn render_frame_writes_png() {
    let out = tmp("f.png");
    let o = out.to_str().unwrap();
    let v = ok_json(&["render-frame", "--time", "1", "--max-side", "240", "--out", o]);
    assert_eq!(v["width"], 240);
    assert_eq!(v["height"], 135);
    let png = std::fs::read(&out).unwrap();
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    let v = ok_json(&["render-frame", "--frame", "15", "--scale", "0.125", "--out", o]);
    assert_eq!(v["width"], 240);
    assert!((v["time"].as_f64().unwrap() - 0.5).abs() < 0.02, "{v}");
}

#[test]
fn errors_are_json() {
    let (code, v) = run_json(&["exec", "no.such.command", "--empty"]);
    assert_eq!(code, 1);
    assert!(v["error"].as_str().unwrap().contains("unknown command"));
    let (code, v) = run_json(&["exec", "layer.select", r#"{"index":2}"#]);
    assert_eq!(code, 1);
    assert!(v["error"].as_str().unwrap().contains("accepted: layers, add, toggle"), "{v}");
    let (code, v) = run_json(&["get", "-", "#99", "transform/opacity"]);
    assert_eq!(code, 1);
    assert!(v["error"].is_string());
    let out = bin().args(["get", "only-one-arg"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}

/// An unknown option is a usage error (exit 2) naming it, before anything runs: the command
/// doesn't run with defaults, and a misspelt option's value isn't opened as the project (#168).
#[test]
fn unknown_options_are_usage_errors() {
    let png = tmp("bogus.png");
    let o = png.to_str().unwrap();
    for (args, named) in [
        (&["exec", "comp.new", "--bogus", "--empty", "--json"][..], "`--bogus`"),
        (&["render-frame", "--bogusflag", "--out", o, "--json"], "`--bogusflag`"),
        (&["exec", "comp.new", "--empty", "--saveas", "z.ecproj", "--json"], "`--saveas`"),
    ] {
        let out = bin().args(args).output().unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {err}");
        assert!(err.contains(&format!("unknown option {named}")), "{args:?}: {err}");
        assert!(out.stdout.is_empty(), "{args:?}: nothing ran");
    }
    assert!(!png.exists(), "render-frame didn't render");
    // `--transparent` (silently ignored before) keeps the frame's alpha.
    let proj = tmp("transparent.ecproj");
    let p = proj.to_str().unwrap();
    ok_json(&["run", "comp.new", r#"{"name":"T","width":32,"height":32}"#, "layer.newSolid", r#"{"width":8,"height":8}"#, "--empty", "--save-as", p]);
    for (flag, alpha) in [(None, 255), (Some("--transparent"), 0)] {
        ok_json(&[&["render-frame", p, "--out", o][..], flag.as_slice()].concat());
        assert_eq!(image::open(&png).unwrap().to_rgba8().get_pixel(0, 0)[3], alpha, "{flag:?}");
    }
}

/// A reader that closes stdout before the CLI writes (`| head`) is not a crash: the command
/// still does its work and exits 0, without a panic (#167).
#[test]
fn a_closed_stdout_is_not_a_crash() {
    let proj = tmp("closed-stdout.ecproj");
    let p = proj.to_str().unwrap();
    for args in [&["info", "--json"][..], &["exec", "--list"], &["run", "comp.new", r#"{"name":"Piped"}"#, "project.summary", "--empty", "--save-as", p]] {
        let mut child = bin().args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        drop(child.stdout.take());
        let out = child.wait_with_output().unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(0), "{args:?}: {err}");
        assert!(!err.contains("panicked"), "{args:?}: {err}");
    }
    assert!(std::fs::read_to_string(&proj).unwrap().contains("Piped"), "the sequence ran to the end and saved");
    // The MCP server ends quietly when its client closes stdout.
    let mut child = bin().arg("mcp").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    drop(child.stdout.take());
    {
        let mut stdin = child.stdin.take().unwrap();
        let _ = writeln!(stdin, "{}", json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}));
    }
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn mcp_over_stdio() {
    let mut child = bin().args(["mcp", "--demo"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let msgs = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "get_property", "arguments": {"layer": "#1", "path": "transform/opacity"}}}),
        json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "render_frame", "arguments": {"max_side": 64}}}),
    ];
    {
        let mut stdin = child.stdin.take().unwrap();
        for m in &msgs {
            writeln!(stdin, "{m}").unwrap();
        }
    } // EOF ends the server.
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let replies: Vec<Value> = String::from_utf8(out.stdout).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(replies.len(), 4, "one reply per request, none for the notification");
    assert_eq!(replies[0]["result"]["serverInfo"]["name"], "effectcraft");
    assert!(replies[1]["result"]["tools"].as_array().unwrap().len() >= 13);
    assert_eq!(replies[2]["result"]["isError"], false, "{}", replies[2]);
    assert_eq!(replies[3]["result"]["content"][0]["type"], "image");
}

#[test]
fn script_file_and_eval() {
    let proj = tmp("scripted.ecproj");
    let p = proj.to_str().unwrap();
    let jsx = tmp("build.jsx");
    std::fs::write(
        &jsx,
        "app.beginUndoGroup('Build');\nvar c = app.project.items.addComp('FromJsx', 160, 90, 1, 2, 24);\nc.layers.addSolid([1, 0, 0], 'Red', 160, 90, 1);\napp.endUndoGroup();\nwriteLn('built ' + c.name);\nc.numLayers",
    )
    .unwrap();
    let v = ok_json(&["script", jsx.to_str().unwrap(), "--save-as", p]);
    assert_eq!(v["ok"], json!(true), "{v}");
    assert_eq!(v["result"], json!(1));
    assert_eq!(v["output"], json!("built FromJsx"));
    assert_eq!(v["saved"], p);
    // Run against the saved project (positional), plain output.
    let out = bin().args(["script", "--eval", "app.project.item(1).name + ' has ' + app.project.item(1).numLayers", p]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "\"FromJsx has 1\"");
    // Errors: exit 1 with file:line:col.
    let bad = tmp("bad.jsx");
    std::fs::write(&bad, "var a = 1;\n\nmissingFunction();\n").unwrap();
    let out = bin().args(["script", bad.to_str().unwrap()]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("bad.jsx:3:1") && err.contains("missingFunction"), "{err}");
    let (code, v) = run_json(&["script", "--eval", "\nnope()"]);
    assert_eq!(code, 1);
    assert_eq!(v["error"]["line"], json!(2));
    // A compiled .jsxbin script is reported as unsupported, not as a SyntaxError (#176).
    let bin_script = tmp("compiled.jsxbin");
    std::fs::write(&bin_script, "@JSXBIN@ES@2.0@MyBbyBn0ABJAnAEjzFjBjMjFjSjUBfRBFeFjIjFjMjMjPff0DzACByB\n").unwrap();
    let out = bin().args(["script", bin_script.to_str().unwrap()]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains(".jsxbin scripts are not supported: run the .jsx source") && !err.contains("SyntaxError"), "{err}");
}

/// `render --out` (#300): a relative path is relative to the working directory like `--project`,
/// not to the project's folder (which doubled a path that already named it).
#[test]
fn render_out_is_relative_to_the_working_directory() {
    let root = tmp("render-out");
    let _ = std::fs::remove_dir_all(&root);
    let (proj, elsewhere) = (root.join("proj"), root.join("elsewhere"));
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::create_dir_all(&elsewhere).unwrap();
    let project = proj.join("x.ecproj");
    let p = project.to_str().unwrap();
    ok_json(&[
        "run",
        "comp.new",
        r#"{"name":"T","width":64,"height":36,"duration":1}"#,
        "layer.newSolid",
        r#"{"width":64,"height":36}"#,
        "--empty",
        "--save-as",
        p,
    ]);
    assert!(project.exists());
    let render = |cwd: &std::path::Path, project: &str, out: &str| {
        let o = bin()
            .current_dir(cwd)
            .args(["render", "--project", project, "--format", "gif", "--start", "0", "--end", "0.1", "--resolution", "quarter", "--out", out, "--json"])
            .output()
            .unwrap();
        assert!(o.status.success(), "{out}: {}", String::from_utf8_lossy(&o.stderr));
        let v: Value = serde_json::from_slice(&o.stdout).unwrap();
        PathBuf::from(v["rendered"][0]["output"].as_str().unwrap())
    };
    // A path that already names the project folder is not doubled.
    let done = render(&root, "proj/x.ecproj", "proj/out/d.gif");
    assert_eq!(done, root.join("proj/out/d.gif"));
    assert!(done.exists() && !proj.join("proj").exists());
    // Another working directory: the file lands there, not beside the project.
    let done = render(&elsewhere, project.to_str().unwrap(), "out/card.gif");
    assert_eq!(done, elsewhere.join("out/card.gif"));
    assert!(done.exists() && !proj.join("out/card.gif").exists());
    // `./f.gif` too.
    assert_eq!(render(&elsewhere, project.to_str().unwrap(), "./f.gif"), elsewhere.join("./f.gif"));
    // An absolute path is unchanged.
    let abs = root.join("abs.gif");
    assert_eq!(render(&elsewhere, project.to_str().unwrap(), abs.to_str().unwrap()), abs);
    assert!(abs.exists());
    let _ = std::fs::remove_dir_all(&root);
}

/// A real stdio client: each edit's reply is read before disconnecting or killing the server.
struct McpChild {
    child: std::process::Child,
    output: BufReader<std::process::ChildStdout>,
}

impl McpChild {
    fn start(config: &std::path::Path, autosave: bool) -> Self {
        let mut command = bin();
        command.arg("mcp").env("EFFECTCRAFT_CONFIG_DIR", config).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        if autosave {
            command.arg("--autosave");
        }
        let mut child = command.spawn().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        Self { child, output }
    }
    fn rpc(&mut self, method: &str, params: Value) -> Value {
        writeln!(self.child.stdin.as_mut().unwrap(), "{}", json!({"jsonrpc":"2.0", "id":1, "method":method, "params":params})).unwrap();
        let mut line = String::new();
        assert!(self.output.read_line(&mut line).unwrap() > 0, "server exited without replying");
        let reply: Value = serde_json::from_str(&line).unwrap();
        assert!(reply.get("error").is_none(), "{reply}");
        reply["result"].clone()
    }
    fn tool(&mut self, name: &str, arguments: Value) -> Value {
        let result = self.rpc("tools/call", json!({"name":name,"arguments":arguments}));
        assert_eq!(result["isError"], false, "{result}");
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
    }
    fn kill(&mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
    }
    fn eof(&mut self) {
        drop(self.child.stdin.take());
        assert!(self.child.wait().unwrap().success());
    }
}

impl Drop for McpChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn mcp_config(name: &str) -> PathBuf {
    let dir = tmp(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn mcp_autosave_survives_kill_and_restart_without_touching_desktop_settings() {
    let config = mcp_config("mcp-kill");
    let prefs = r#"{"autoSave":{"enabled":false,"maxVersions":2,"intervalMinutes":20}}"#;
    std::fs::write(config.join("prefs.json"), prefs).unwrap();
    std::fs::write(config.join("shortcuts.json"), "{}").unwrap();
    std::fs::write(config.join("session.lock"), "desktop sentinel").unwrap();
    let mut first = McpChild::start(&config, true);
    first.rpc("initialize", json!({}));
    first.tool("execute_command", json!({"command":"comp.new","params":{"name":"Survives restart","width":320,"height":180}}));
    first.tool("execute_command", json!({"command":"layer.newText","params":{"text":"Unsaved title"}}));
    assert_eq!(first.tool("execute_command", json!({"command":"prefs.get","params":{"key":"autoSave.enabled"}})), false);
    let status = first.tool("get_project", json!({}))["autosave"].clone();
    let path = status["latestPath"].as_str().unwrap();
    first.kill(); // SIGKILL: there is no opportunity to save at EOF.
    let folder = std::path::Path::new(status["folder"].as_str().unwrap());
    assert_eq!(effectcraft_engine::autosave::existing(folder, "Untitled Project").len(), 2, "loaded maxVersions is respected");
    let manifest: Value = serde_json::from_str(&std::fs::read_to_string(folder.join("session.json")).unwrap()).unwrap();
    assert_eq!(manifest["dirty"], true);
    assert_eq!(manifest["ended"], false);
    let mut restarted = McpChild::start(&config, true);
    let init = restarted.rpc("initialize", json!({}));
    assert!(init["instructions"].as_str().unwrap().contains(path));
    assert_eq!(init["_meta"]["effectcraftAutoSave"]["previousSessions"][0]["autosave"], path);
    restarted.tool("open_project", json!({"path":path}));
    let comp = restarted.tool("get_comp", json!({"comp":"Survives restart"}));
    assert_eq!(comp["layers"].as_array().unwrap().len(), 1);
    let layer = restarted.tool("get_layer", json!({"layer":"#1"}));
    assert!(layer.to_string().contains("Unsaved title"), "{layer}");
    restarted.tool("save_project", json!({"path":config.join("Recovered.ecproj")}));
    restarted.eof();
    assert_eq!(std::fs::read_to_string(config.join("prefs.json")).unwrap(), prefs);
    assert_eq!(std::fs::read_to_string(config.join("shortcuts.json")).unwrap(), "{}");
    assert_eq!(std::fs::read_to_string(config.join("session.lock")).unwrap(), "desktop sentinel");
    std::fs::remove_dir_all(config).unwrap();
}

#[test]
fn mcp_autosave_eof_preserves_titled_edits_without_overwriting_project() {
    let config = mcp_config("mcp-eof");
    let original = config.join("Original.ecproj");
    let mut first = McpChild::start(&config, true);
    first.tool("execute_command", json!({"command":"comp.new","params":{"name":"Original"}}));
    first.tool("save_project", json!({"path":original}));
    let original_bytes = std::fs::read(&original).unwrap();
    first.tool("execute_command", json!({"command":"layer.newText","params":{"text":"Not manually saved"}}));
    let status = first.tool("get_project", json!({}))["autosave"].clone();
    first.eof();
    assert_eq!(std::fs::read(original).unwrap(), original_bytes);
    assert!(std::fs::read_to_string(status["latestPath"].as_str().unwrap()).unwrap().contains("Not manually saved"));
    let folder = std::path::Path::new(status["folder"].as_str().unwrap());
    let manifest: Value = serde_json::from_str(&std::fs::read_to_string(folder.join("session.json")).unwrap()).unwrap();
    assert_eq!(manifest["ended"], true);
    assert_eq!(manifest["dirty"], true);
    std::fs::remove_dir_all(config).unwrap();
}

#[test]
fn mcp_autosave_preserves_work_when_stdout_is_closed() {
    let config = mcp_config("mcp-broken-pipe");
    let mut child = bin()
        .args(["mcp", "--autosave"])
        .env("EFFECTCRAFT_CONFIG_DIR", &config)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    {
        let mut input = child.stdin.take().unwrap();
        writeln!(input, "{}", json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"execute_command","arguments":{"command":"comp.new","params":{"name":"Closed stdout"}}}})).unwrap();
    }
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let base = config.join(effectcraft_engine::autosave::AUTOSAVE_FOLDER).join("MCP");
    let folder = std::fs::read_dir(base).unwrap().next().unwrap().unwrap().path();
    let manifest: Value = serde_json::from_str(&std::fs::read_to_string(folder.join("session.json")).unwrap()).unwrap();
    assert!(std::fs::read_to_string(manifest["autosave"].as_str().unwrap()).unwrap().contains("Closed stdout"));
    assert_eq!(manifest["ended"], true);
    std::fs::remove_dir_all(config).unwrap();
}

#[test]
fn mcp_autosave_is_opt_in_and_rejects_invalid_combinations() {
    let config = mcp_config("mcp-opt-in");
    let mut client = McpChild::start(&config, false);
    client.tool("execute_command", json!({"command":"comp.new","params":{"name":"Ephemeral"}}));
    client.eof();
    assert!(std::fs::read_dir(&config).unwrap().next().is_none());
    for args in [["mcp", "--autosave", "--bridge", "1"].as_slice(), ["info", "--autosave"].as_slice()] {
        let out = bin().args(args).env("EFFECTCRAFT_CONFIG_DIR", &config).output().unwrap();
        assert_eq!(out.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&out.stderr).contains("--autosave"));
    }
    let blocked = config.join("file-not-directory");
    std::fs::write(&blocked, "file").unwrap();
    let out = bin().args(["mcp", "--autosave"]).env("EFFECTCRAFT_CONFIG_DIR", blocked).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("MCP auto-save"));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("panicked"));
    std::fs::remove_dir_all(config).unwrap();
}

/// `render --bitrate` (#440) targets the bitrate in VP9 WebM, which otherwise encodes to a quality.
#[test]
fn render_bitrate_applies_to_vp9_webm() {
    let root = tmp("webm-bitrate");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let project = root.join("x.ecproj");
    let p = project.to_str().unwrap();
    ok_json(&[
        "run",
        "comp.new",
        r##"{"name":"T","width":320,"height":180,"duration":1}"##,
        "layer.newSolid",
        r##"{"width":320,"height":180}"##,
        "effect.apply",
        r##"{"layer":"#1","effect":"ec.noise.fractal"}"##,
        "--empty",
        "--save-as",
        p,
    ]);
    let size = |kbps: &str| {
        let out = root.join(format!("b{kbps}.webm"));
        ok_json(&["render", "--project", p, "--start", "0", "--end", "0.5", "--bitrate", kbps, "--out", out.to_str().unwrap()]);
        std::fs::metadata(&out).unwrap().len()
    };
    let (low, high) = (size("100"), size("8000"));
    assert!(high > low, "100 kbps: {low} bytes, 8000 kbps: {high} bytes");
    let _ = std::fs::remove_dir_all(&root);
}
