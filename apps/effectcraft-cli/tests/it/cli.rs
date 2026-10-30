//! End-to-end tests of the agent CLI: JSON output, project round-trips, rendering and MCP over stdio.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

fn bin() -> Command {
    Command::new(crate::binary())
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

/// `std::fs::canonicalize` without Windows' `\\?\` verbatim prefix, which a child's `current_dir()` never reports.
fn canonical(path: &std::path::Path) -> std::io::Result<PathBuf> {
    let path = std::fs::canonicalize(path)?;
    let plain = path.to_str().and_then(|s| s.strip_prefix(r"\\?\")).filter(|s| s.as_bytes().get(1) == Some(&b':'));
    Ok(plain.map(PathBuf::from).unwrap_or(path))
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ec-cli-test-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    // macOS's `/var` is a symlink to `/private/var`: a child process resolves its working
    // directory through `current_dir()`, which is canonical, so compare canonical paths.
    canonical(&d).unwrap_or(d).join(name)
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

/// `run --args-file` reads its command / params pairs from a file, one per line: a batch longer
/// than a Windows command line (32 767 characters, os error 206) still runs.
#[test]
fn run_reads_arguments_from_a_file() {
    let proj = tmp("args-file.ecproj");
    let p = proj.to_str().unwrap();
    let mut lines = vec!["comp.new".to_string(), json!({"name": "A", "width": 64, "height": 64}).to_string()];
    for i in 0..400 {
        lines.push("layer.newSolid".into());
        lines.push(json!({"width": 8, "height": 8, "name": format!("Solid with a long name to pass the limit {i:04}")}).to_string());
        lines.push(String::new());
    }
    let text = lines.join("\n");
    assert!(text.len() > 32_767);
    let file = tmp("args-file.txt");
    std::fs::write(&file, text).unwrap();
    let v = ok_json(&["run", "--args-file", file.to_str().unwrap(), "--empty", "--save-as", p]);
    assert_eq!(v["result"].as_array().map(Vec::len), Some(401), "{v}");
    let out = bin().args(["run", "--args-file", "no/such/file.txt", "--empty"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--args-file no/such/file.txt"));
}

/// Run with `stdin` piped in; returns (exit code, stdout, stderr).
fn run_with_stdin(args: &[&str], stdin: &str) -> (i32, String, String) {
    let mut child = bin().args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

/// `run --commands FILE|-` (#2251) reads `{"command", "params"}` steps from a JSON array or JSON
/// Lines, from a file or standard input, and runs them as the same positional pairs would: after
/// the pairs, so a generated list can follow a hand-written prefix. One command whose params are
/// over Linux's 128 KiB per-argument limit goes through.
#[test]
fn run_reads_commands_from_a_file_or_stdin() {
    let steps = [
        ("comp.new", json!({"name": "A", "width": 64, "height": 64})),
        ("layer.newSolid", json!({"width": 8, "height": 8, "name": "One"})),
        ("layer.newSolid", json!({"width": 8, "height": 8, "name": "Two"})),
        ("project.summary", json!({})),
    ];
    let pairs: Vec<String> = steps.iter().flat_map(|(id, p)| [id.to_string(), p.to_string()]).collect();
    let pairs: Vec<&str> = pairs.iter().map(String::as_str).collect();
    let expected = ok_json(&[&["run", "--empty"][..], &pairs[..]].concat());
    let layers = |v: &Value| v[3]["result"]["items"][0]["layers"].as_u64();
    assert_eq!(layers(&expected), Some(2), "{expected}");

    let objects: Vec<Value> = steps.iter().map(|(id, p)| json!({"command": id, "params": p})).collect();
    let array = tmp("commands.json");
    std::fs::write(&array, serde_json::to_string_pretty(&objects).unwrap()).unwrap();
    assert_eq!(ok_json(&["run", "--commands", array.to_str().unwrap(), "--empty"]), expected);

    let lines: String = objects.iter().map(|o| format!("{o}\n\n")).collect();
    let (code, out, err) = run_with_stdin(&["run", "--commands", "-", "--empty", "--json"], &lines);
    assert_eq!(code, 0, "{err}");
    assert_eq!(serde_json::from_str::<Value>(&out).unwrap(), expected);

    // Positional pairs first, then the file.
    let rest = tmp("commands-rest.jsonl");
    std::fs::write(&rest, objects[1..].iter().map(|o| format!("{o}\n")).collect::<String>()).unwrap();
    assert_eq!(ok_json(&[&["run", "--empty", "--commands", rest.to_str().unwrap()][..], &pairs[..2]].concat()), expected);

    let name = "n".repeat(140 * 1024);
    let big = tmp("commands-big.jsonl");
    std::fs::write(&big, format!("{}\n{}\n", json!({"command": "comp.new", "params": {"name": name}}), json!({"command": "project.summary"}))).unwrap();
    let v = ok_json(&["run", "--commands", big.to_str().unwrap(), "--empty"]);
    assert_eq!(v[1]["result"]["items"][0]["name"].as_str().map(str::len), Some(name.len()));

    for (input, args, error) in [
        ("", &["run", "--commands", "-", "--empty"][..], "--commands -: no commands"),
        (
            "{\"command\":\"project.summary\"}\n{\"command\":\"x\",\"param\":{}}\n",
            &["run", "project.summary", "{}", "--commands", "-", "--empty"],
            "step 2 (line 2): unknown key `param`",
        ),
        ("[{\"command\":\"project.summary\"}, {\"params\":{}}]", &["run", "--commands", "-", "--empty"], "step 1 (entry 2): no `command`"),
        ("{}", &["run", "--commands", "-", "--commands", "-", "--empty"], "standard input is read once"),
    ] {
        let (code, out, err) = run_with_stdin(args, input);
        assert_eq!(code, 2, "{input:?}: {err}");
        assert!(err.contains(error), "{input:?}: {err}");
        assert!(out.is_empty(), "{input:?}: nothing ran");
    }
    // An empty list after positional pairs is no error: there is still something to run.
    let (code, _, err) = run_with_stdin(&["run", "project.summary", "--commands", "-", "--empty"], "");
    assert_eq!(code, 0, "{err}");
    // A step that fails when it runs is named by its index too.
    let (code, out, err) = run_with_stdin(&["run", "--commands", "-", "--empty", "--json"], "{\"command\":\"project.summary\"}\n{\"command\":\"no.such\"}\n");
    assert_eq!(code, 1, "{err}");
    let e = serde_json::from_str::<Value>(&out).unwrap()["error"].as_str().map(String::from).unwrap_or_default();
    assert!(e.starts_with("no.such: ") && e.ends_with(" (step 1)"), "{e}");
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

/// An argument that is not Unicode (a Latin-1 file name on Linux) is a usage error (exit 2)
/// naming it, before anything runs: `std::env::args` panicked on one (exit 101).
#[cfg(unix)]
#[test]
fn non_unicode_arguments_are_usage_errors() {
    use std::os::unix::ffi::OsStrExt;
    let name = std::ffi::OsStr::from_bytes(b"caf\xe9.png");
    let out = bin().args(["render-frame", "--out"]).arg(name).output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{err}");
    assert!(err.contains("argument `caf\u{fffd}.png` is not valid Unicode"), "{err}");
    assert!(!err.contains("panicked"), "{err}");
    assert!(out.stdout.is_empty(), "nothing ran");
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
    // current_dir resolves aliases such as macOS /var -> /private/var.
    let root = canonical(&root).unwrap();
    let (proj, elsewhere) = (root.join("proj"), root.join("elsewhere"));
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
    // The instructions carry the auto-save state as JSON, where a Windows path's backslashes
    // are escaped.
    assert!(init["instructions"].as_str().unwrap().contains(&Value::from(path).to_string()));
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

/// `run`/`exec renderQueue.render` (#1426): a failed item is a command error, exit 1 like
/// `render --queue`, not a success with the failure buried in the JSON.
#[test]
fn a_failed_render_under_run_and_exec_exits_1() {
    let root = tmp("render-fails");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    // The output's folder is a file, so the encoder cannot open the output.
    let blocker = root.join("not-a-folder");
    std::fs::write(&blocker, "x").unwrap();
    let out = blocker.join("out.gif");
    let add = json!({"comp": "T", "format": "gif", "output": out.to_str().unwrap()}).to_string();
    let project = root.join("q.ecproj");
    let steps = ["comp.new", r#"{"name":"T","width":16,"height":16,"duration":0.1}"#, "layer.newSolid", "{}", "renderQueue.add", &add];
    ok_json(&[&["run", "--empty", "--save-as", project.to_str().unwrap()][..], &steps[..]].concat());

    let (code, v) = run_json(&[&["run", "--empty"][..], &steps[..], &["renderQueue.render", r#"{"wait":true}"#][..]].concat());
    assert_eq!(code, 1, "{v}");
    let e = v["error"].as_str().unwrap_or_else(|| panic!("{v}"));
    assert!(e.starts_with("renderQueue.render: T: "), "{e}");

    let (code, v) = run_json(&["exec", "renderQueue.render", "--project", project.to_str().unwrap()]);
    assert_eq!(code, 1, "{v}");
    assert!(v["error"].as_str().is_some_and(|e| e.starts_with("T: ")), "{v}");
}

/// A source refusal happens before encoding and has no started timestamp. It must still
/// fail the CLI, while the valid queue item finishes and the imported frame stays intact.
#[test]
fn a_partly_protected_queue_exits_1_under_render_run_and_exec() {
    let root = tmp("protected-mixed-queue");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let source = root.join("source_00000.png");
    image::RgbaImage::from_pixel(4, 4, image::Rgba([40, 80, 120, 255])).save(&source).unwrap();
    let original = std::fs::read(&source).unwrap();
    let project = root.join("q.ecproj");
    let good = root.join("good.gif");
    let import = json!({"paths": [source]}).to_string();
    let blocked = json!({"format": "png", "output": root.join("source_[#####].png")}).to_string();
    let valid = json!({"format": "gif", "output": good}).to_string();
    ok_json(&[
        "run",
        "--empty",
        "--save-as",
        project.to_str().unwrap(),
        "comp.new",
        r#"{"name":"Main","width":16,"height":16,"frameRate":10,"duration":0.1}"#,
        "layer.newSolid",
        "{}",
        "file.import",
        &import,
        "renderQueue.add",
        &blocked,
        "renderQueue.add",
        &valid,
    ]);
    let p = project.to_str().unwrap();
    for args in [vec!["render", "--queue", "--project", p], vec!["run", p, "renderQueue.render", "{}"], vec!["exec", "renderQueue.render", "--project", p]] {
        std::fs::write(&good, b"previous output").unwrap();
        let (code, result) = run_json(&args);
        assert_eq!(code, 1, "{args:?}: {result}");
        assert!(result["error"].as_str().is_some_and(|e| e.contains("footage") && e.contains("source_00000.png")), "{result}");
        assert_eq!(std::fs::read(&source).unwrap(), original);
        assert!(std::fs::read(&good).unwrap().starts_with(b"GIF8"), "the valid item still rendered");
    }
    std::fs::remove_dir_all(root).unwrap();
}

/// `render --bitrate` (#440) targets the bitrate in VP9 WebM.
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

/// Render uses the same empty/project/demo selection as the other CLI commands.
#[test]
fn render_empty_does_not_load_the_demo() {
    let root = tmp("render-empty");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let project = root.join("control.ecproj");
    let template = root.join("control.ectemplate");
    let project_path = project.to_str().unwrap();
    let template_path = template.to_str().unwrap();
    let build = r#"
var comp = app.project.items.addComp("Empty control", 64, 36, 1, 1, 30);
var solid = comp.layers.addSolid([0, 1, 0], "Green", 64, 36, 1);
solid.property("ADBE Transform Group").property("ADBE Opacity").addToMotionGraphicsTemplateAs(comp, "Opacity");
"#;
    ok_json(&["script", "--eval", build, "--save-as", project_path]);
    let params = json!({"comp": "Empty control", "path": template_path}).to_string();
    let exported = ok_json(&["exec", "essential.exportTemplate", &params, "--project", project_path]);
    assert_eq!(exported["controls"], 1, "{exported}");
    let empty = ok_json(&["info", "--empty"]);
    assert!(empty["project"]["items"].as_array().unwrap().is_empty(), "{empty}");

    let render = |options: &[&str], name: &str| {
        let output = root.join(name);
        assert!(!output.exists());
        let mut args = vec!["render", "--format", "gif", "--start", "0", "--end", "0.1", "--resolution", "quarter", "--out", output.to_str().unwrap()];
        args.extend_from_slice(options);
        let reply = ok_json(&args);
        assert_eq!(reply["rendered"].as_array().unwrap().len(), 1, "{reply}");
        assert_eq!(PathBuf::from(reply["rendered"][0]["output"].as_str().unwrap()), output);
        let bytes = std::fs::read(&output).unwrap();
        assert!(bytes.len() >= 10 && (bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")), "{reply}");
        let dimensions = (u16::from_le_bytes([bytes[6], bytes[7]]), u16::from_le_bytes([bytes[8], bytes[9]]));
        (reply["rendered"][0]["comp"].clone(), dimensions)
    };
    // Defaults still render the demo, and an explicit demo wins over --empty.
    let demo = render(&[], "default.gif");
    assert_eq!(render(&["--demo", "--empty"], "demo.gif"), demo);
    // A supplied project wins over both flags, while template mode also accepts --empty.
    let project_result = render(&["--project", project_path, "--demo", "--empty"], "project.gif");
    assert_eq!(project_result, (json!("Empty control"), (16, 9)));
    assert_eq!(render(&["--template", template_path, "--empty"], "template.gif").1, (16, 9));

    let output = root.join("empty.gif");
    assert!(!output.exists());
    let (code, reply) =
        run_json(&["render", "--empty", "--format", "gif", "--start", "0", "--end", "0.1", "--resolution", "quarter", "--out", output.to_str().unwrap()]);
    let output_exists = output.exists();
    assert!(!output_exists, "--empty must not render the demo into the requested output");
    assert_eq!(code, 1, "{reply}");
    assert_eq!(reply["error"], "command `renderQueue.add` is not available right now: select or open a composition", "{reply}");
    let _ = std::fs::remove_dir_all(&root);
}

/// `WGPU_ADAPTER_NAME` that matches nothing is a clean `--gpu` error listing the adapters
/// found (wgpu's own helper panics). Read once per process, hence a subprocess.
#[test]
fn gpu_adapter_name_mismatch_is_a_clean_error() {
    let out = bin()
        .args(["render-frame", "--gpu", "--max-side", "64", "--out", tmp("nomatch.png").to_str().unwrap(), "--json"])
        .env("WGPU_ADAPTER_NAME", "NoSuchAdapter-xyz")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "stdout: {stdout}\nstderr: {}", String::from_utf8_lossy(&out.stderr));
    let v: Value = serde_json::from_str(stdout.trim()).unwrap();
    let e = v["error"].as_str().unwrap();
    assert!(e.contains("WGPU_ADAPTER_NAME=\"NoSuchAdapter-xyz\""), "{e}");
    assert!(e.contains("available: [") || e.contains("no adapters on the enabled backends"), "{e}");
}

/// An exported-but-empty `WGPU_ADAPTER_NAME` behaves like unset: it never produces the
/// adapter-name error (whether a GPU exists on this machine is not asserted).
#[test]
fn gpu_adapter_name_empty_is_unset() {
    let out = bin()
        .args(["render-frame", "--gpu", "--max-side", "64", "--out", tmp("emptyname.png").to_str().unwrap(), "--json"])
        .env("WGPU_ADAPTER_NAME", "  ")
        .output()
        .unwrap();
    let all = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(!all.contains("WGPU_ADAPTER_NAME"), "{all}");
}

/// Renders two seconds of the demo at quarter resolution (long enough for several progress
/// updates) and returns stderr, which `output()` reads through a pipe: not a terminal.
fn render_demo_stderr(name: &str, extra: &[&str]) -> String {
    let out = tmp(name);
    let mut args = vec!["render", "--demo", "--format", "gif", "--start", "0", "--end", "2", "--resolution", "quarter", "--out", out.to_str().unwrap()];
    args.extend_from_slice(extra);
    let o = bin().args(&args).output().unwrap();
    let stderr = String::from_utf8_lossy(&o.stderr).to_string();
    assert!(o.status.success(), "{args:?}: {stderr}");
    let _ = std::fs::remove_file(out);
    stderr
}

/// `render --progress` with a piped stderr reports the render's frames as
/// `progress <frames done> <frames total> <seconds elapsed> <seconds left|->` lines, one per
/// change (#1778).
#[test]
fn render_progress_reports_frames_on_a_piped_stderr() {
    let stderr = render_demo_stderr("progress.gif", &["--progress"]);
    let mut last_done = None;
    let mut totals = vec![];
    for line in stderr.lines().filter(|l| l.starts_with("progress")) {
        let fields: Vec<&str> = line.split(' ').collect();
        assert_eq!(fields.len(), 5, "{line}");
        let done: u64 = fields[1].parse().unwrap_or_else(|e| panic!("{line}: frames done: {e}"));
        let total: u64 = fields[2].parse().unwrap_or_else(|e| panic!("{line}: frames total: {e}"));
        let elapsed: f64 = fields[3].parse().unwrap_or_else(|e| panic!("{line}: seconds elapsed: {e}"));
        assert!(fields[4] == "-" || fields[4].parse::<f64>().is_ok_and(|left| left >= 0.0), "{line}: seconds left");
        assert!(done <= total && total > 0 && elapsed >= 0.0, "{line}");
        assert!(last_done.is_none_or(|last| done > last), "one line per change, and frames only go up: {stderr}");
        last_done = Some(done);
        totals.push(total);
    }
    assert!(!totals.is_empty(), "no progress line on a piped stderr: {stderr}");
    assert!(totals.iter().all(|&t| t == totals[0]), "one render, one total: {stderr}");
}

/// Without `--progress`, the same render writes no progress lines to a piped stderr (#1778).
#[test]
fn render_without_progress_writes_no_progress_lines() {
    let stderr = render_demo_stderr("no-progress.gif", &[]);
    assert!(!stderr.lines().any(|l| l.starts_with("progress")), "{stderr}");
}
