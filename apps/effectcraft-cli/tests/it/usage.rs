//! The command line contract (RFC-0012), run in-process through `effectcraft_cli::run`: generated
//! help, `--version`, exit codes, which stream each message goes to, and every kind of usage error
//! (unknown, repeated and conflicting flags, bad values, missing arguments) refused before any work.

use std::io::Write;

/// What `run` returned and wrote.
struct Invoke {
    code: i32,
    stdout: String,
    stderr: String,
}

fn invoke(args: &[&str]) -> Invoke {
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let code = effectcraft_cli::run(args, &mut stdout, &mut stderr);
    Invoke { code, stdout: String::from_utf8(stdout).unwrap(), stderr: String::from_utf8(stderr).unwrap() }
}

/// A usage error: exit 2, the message on stderr (with the usage), nothing on stdout.
#[track_caller]
fn usage_error(args: &[&str], names: &str) {
    let r = invoke(args);
    assert_eq!(r.code, 2, "{args:?}: {}", r.stderr);
    assert!(r.stdout.is_empty(), "{args:?}: nothing ran: {}", r.stdout);
    assert!(r.stderr.starts_with("error: "), "{args:?}: {}", r.stderr);
    assert!(r.stderr.contains(names), "{args:?} should name `{names}`: {}", r.stderr);
}

const SUBCOMMANDS: [&str; 12] = ["info", "commands", "exec", "run", "props", "get", "set", "render-frame", "render", "bench", "script", "mcp"];

#[test]
fn help_is_generated_for_the_command_and_every_subcommand() {
    let r = invoke(&["--help"]);
    assert_eq!((r.code, r.stderr.as_str()), (0, ""));
    assert!(r.stdout.contains("Usage: effectcraft-cli"), "{}", r.stdout);
    for sub in SUBCOMMANDS {
        assert!(r.stdout.contains(sub), "the command's help lists {sub}");
        for args in [vec![sub, "--help"], vec![sub, "-h"], vec!["help", sub]] {
            let r = invoke(&args);
            assert_eq!((r.code, r.stderr.as_str()), (0, ""), "{args:?}");
            assert!(r.stdout.contains(&format!("Usage: effectcraft-cli {sub}")), "{args:?}: {}", r.stdout);
        }
    }
    // The old name of `render-frame`.
    assert_eq!(invoke(&["frame", "--help"]).code, 0);
}

#[test]
fn help_wins_over_a_mistake_before_it() {
    // clap reports the missing value of `--out` first; help is what was asked for.
    let r = invoke(&["render-frame", "--out", "--help"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(r.stdout.contains("Usage: effectcraft-cli render-frame"));
    assert_eq!(invoke(&["get", "--bogus", "--help"]).code, 0);
}

#[test]
fn version_is_a_flag_not_a_subcommand() {
    let want = format!("effectcraft-cli {}\n", env!("CARGO_PKG_VERSION"));
    for flag in ["--version", "-V"] {
        let r = invoke(&[flag]);
        assert_eq!((r.code, r.stdout.as_str(), r.stderr.as_str()), (0, want.as_str(), ""), "{flag}");
    }
    usage_error(&["version"], "unrecognized subcommand 'version'");
}

#[test]
fn no_arguments_is_a_usage_error_with_the_help_on_stderr() {
    let r = invoke(&[]);
    assert_eq!(r.code, 2);
    assert!(r.stdout.is_empty());
    assert!(r.stderr.contains("Usage: effectcraft-cli"), "{}", r.stderr);
    usage_error(&["nosuch"], "unrecognized subcommand 'nosuch'");
}

#[test]
fn an_unknown_flag_is_refused_on_every_subcommand() {
    for sub in SUBCOMMANDS {
        usage_error(&[sub, "--bogus"], "unexpected argument '--bogus'");
    }
    // Misspelt options once ran the command with their defaults (#168).
    usage_error(&["exec", "comp.new", "--empty", "--saveas", "z.ecproj"], "--saveas");
    // A flag of another subcommand.
    usage_error(&["info", "--save"], "unexpected argument '--save'");
    usage_error(&["info", "--autosave"], "unexpected argument '--autosave'");
    usage_error(&["render", "--bridge", "9877", "--out", "x.mp4"], "unexpected argument '--bridge'");
}

#[test]
fn a_repeated_flag_is_refused() {
    usage_error(&["render-frame", "--out", "a.png", "--out", "b.png"], "the argument '--out <F.png>' cannot be used multiple times");
    usage_error(&["info", "--project", "a.ecproj", "--project", "b.ecproj"], "--project");
    usage_error(&["info", "--json", "--json"], "--json");
    usage_error(&["get", "-", "#1", "p", "--time", "1", "--time", "2"], "--time");
    // Where a flag is meant to repeat it still does: `run --commands` and the MCP roots.
    usage_error(&["run", "--commands", "-", "--commands", "-"], "standard input is read once");
}

#[test]
fn a_flag_value_that_looks_like_a_flag_is_not_taken() {
    usage_error(&["render-frame", "--out", "--transparent"], "--out");
    // `=` takes it as it is.
    let r = invoke(&["render-frame", "--out=-x.png", "--time", "x"]);
    assert_eq!(r.code, 2);
}

#[test]
fn bad_values_are_usage_errors() {
    usage_error(&["props", "-", "#1", "--time", "soon"], "'soon' is not a finite number");
    usage_error(&["props", "-", "#1", "--time", "NaN"], "not a finite number");
    usage_error(&["props", "-", "#1", "--depth", "-1"], "-1");
    usage_error(&["render-frame", "--max-side", "huge"], "--max-side");
    usage_error(&["render-frame", "--frame", "inf"], "not a finite number");
    usage_error(&["render", "--out", "x.mp4", "--fps", "fast"], "--fps");
    usage_error(&["render", "--out", "x.mp4", "--bitrate"], "--bitrate");
    usage_error(&["bench", "--n", "many"], "--n");
    usage_error(&["bench", "--ops", "--layers", "1.5"], "--layers");
    usage_error(&["exec", "comp.new", "--params", "{"], "--params");
    usage_error(&["exec", "comp.new", "{"], "params are not valid JSON");
    usage_error(&["run", "comp.new", "{"], "comp.new: params are not valid JSON");
    usage_error(&["info", "--project", ""], "--project");
    usage_error(&["info", "notes.txt"], "'notes.txt' is not a project");
}

#[test]
fn conflicting_flags_are_usage_errors() {
    usage_error(&["info", "--bridge", "9877", "--demo"], "--bridge");
    usage_error(&["info", "--bridge", "9877", "a.ecproj"], "--bridge drives the running app's project");
    usage_error(&["info", "a.ecproj", "b.ecproj"], "unexpected argument 'b.ecproj'");
    usage_error(&["info", "a.ecproj", "--project", "b.ecproj"], "two projects given");
    usage_error(&["run", "a.ecproj", "b.ecproj", "project.summary"], "two projects given");
    usage_error(&["exec", "x", "--save", "--save-as", "y.ecproj"], "--save");
    usage_error(&["exec", "x", "{}", "--params", "{}"], "given twice");
    usage_error(&["exec", "--list", "--params", "{}"], "--list");
    usage_error(&["exec", "x", "--filter", "layer"], "--list");
    usage_error(&["render-frame", "--time", "1", "--frame", "2"], "--time");
    usage_error(&["render-frame", "--max-side", "10", "--scale", "1"], "--max-side");
    usage_error(&["render-frame", "--template", "t.ectemplate", "--comp", "Main"], "--template");
    usage_error(&["render-frame", "--template", "t.ectemplate", "a.ecproj"], "--template renders a new instance");
    usage_error(&["render-frame", "--values", "v.json"], "--template");
    usage_error(&["render", "--queue", "--out", "x.mp4"], "--queue");
    usage_error(&["render", "--queue", "--format", "gif"], "--queue");
    usage_error(&["render", "--out", "x.mp4", "--work-area", "--start", "1"], "--work-area");
    usage_error(&["render", "--out", "x.mp4", "--resolution", "half", "--scale", "0.5"], "--resolution");
    usage_error(&["bench", "--ops", "--roto"], "--ops");
    usage_error(&["bench", "--ops", "a.ecproj"], "take no project");
    usage_error(&["bench", "--roto", "a.ecproj"], "take no project");
    usage_error(&["bench", "--small"], "--ops");
    usage_error(&["bench", "--fixes"], "--roto");
    usage_error(&["bench", "--adv3d"], "--gpu");
    usage_error(&["bench", "--ops", "--gpu"], "--gpu");
    usage_error(&["bench", "--roto", "--clip", "frames"], "--truth");
    usage_error(&["bench", "--truth", "masks"], "--clip");
    usage_error(&["mcp", "--autosave", "--bridge", "9877"], "--autosave");
}

#[test]
fn missing_arguments_are_usage_errors() {
    usage_error(&["exec"], "required arguments");
    usage_error(&["exec", "a.ecproj"], "missing arguments");
    usage_error(&["exec", "a", "{}", "c"], "too many arguments");
    usage_error(&["run"], "run needs");
    usage_error(&["get", "only-one-arg"], "usage `get <comp> <layer> <path>`");
    usage_error(&["props", "Main"], "usage `props <comp> <layer>`");
    usage_error(&["props", "Main", "#1", "extra"], "too many arguments");
    usage_error(&["set", "Main", "#1", "transform/opacity"], "usage `set <comp> <layer> <path> <value>");
    usage_error(&["set", "Main", "#1", "transform/opacity", "50", "--expression", "time"], "too many arguments");
    usage_error(&["script"], "usage `script FILE.jsx");
    usage_error(&["script", "a.jsx", "b.jsx"], "too many arguments");
    usage_error(&["script", "--eval", "1", "a.jsx"], "too many arguments");
    // `render` needs somewhere to write (or the queue).
    usage_error(&["render"], "--out");
    usage_error(&["render", "--demo", "--format", "gif"], "--out");
}

#[test]
fn a_negative_number_is_a_value_only_where_it_means_something() {
    // A property value: `set` takes `-5`, and the project and flags still parse around it.
    let r = invoke(&["set", "Main", "#1", "transform/rotation", "-5", "--time", "1"]);
    assert_ne!(r.code, 2, "{}", r.stderr);
    // A flag value starts with `=` to begin with a dash.
    usage_error(&["props", "-", "#1", "--time", "-1"], "-1");
}

#[test]
fn errors_are_printed_as_error_on_stderr_and_as_json_on_stdout_with_json() {
    let r = invoke(&["exec", "no.such.command", "--empty"]);
    assert_eq!(r.code, 1);
    assert!(r.stdout.is_empty(), "{}", r.stdout);
    assert!(r.stderr.starts_with("error: ") && r.stderr.contains("unknown command"), "{}", r.stderr);

    let r = invoke(&["exec", "no.such.command", "--empty", "--json"]);
    assert_eq!(r.code, 1);
    let v: serde_json::Value = serde_json::from_str(r.stdout.trim()).unwrap();
    assert!(v["error"].as_str().unwrap().contains("unknown command"), "{v}");
    assert!(r.stderr.starts_with("error: "), "{}", r.stderr);
    // `--json` is global: before the subcommand it means the same.
    let r = invoke(&["--json", "exec", "no.such.command", "--empty"]);
    assert_eq!(r.code, 1);
    assert!(r.stdout.contains("\"error\""), "{}", r.stdout);
}

#[test]
fn a_successful_command_writes_its_result_to_stdout_only() {
    let r = invoke(&["commands", "--filter", "layer.newSolid", "--json"]);
    assert_eq!((r.code, r.stderr.as_str()), (0, ""));
    let v: serde_json::Value = serde_json::from_str(r.stdout.trim()).unwrap();
    assert!(v.as_array().unwrap().iter().any(|c| c["id"] == "layer.newSolid"), "{v}");
}

/// A writer that fails like a closed pipe, or like a full disk.
struct Failing(std::io::ErrorKind);

impl Write for Failing {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(self.0.into())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Err(self.0.into())
    }
}

#[test]
fn a_closed_stdout_is_not_an_error_but_a_failing_one_is() {
    let mut err = Vec::new();
    let code = effectcraft_cli::run(["commands", "--filter", "layer.new"], &mut Failing(std::io::ErrorKind::BrokenPipe), &mut err);
    assert_eq!((code, err.len()), (0, 0), "{}", String::from_utf8_lossy(&err));

    let mut err = Vec::new();
    let code = effectcraft_cli::run(["commands", "--filter", "layer.new"], &mut Failing(std::io::ErrorKind::StorageFull), &mut err);
    assert_eq!(code, 1);
    assert!(String::from_utf8_lossy(&err).contains("cannot write to stdout"), "{}", String::from_utf8_lossy(&err));
}

/// An empty `--expression` removes the property's expression; it is not a usage error.
#[test]
fn an_empty_expression_and_an_empty_filter_are_values() {
    let r = invoke(&["set", "-", "#1", "transform/opacity", "--expression", ""]);
    assert_ne!(r.code, 2, "{}", r.stderr);
    let r = invoke(&["commands", "--filter", "", "--json"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
}

#[test]
fn help_is_the_named_subcommands_even_after_global_flags_and_mistakes() {
    for args in
        [vec!["--json", "render-frame", "--out", "--help"], vec!["--json", "render-frame", "--help"], vec!["--json", "render-frame", "--bogus", "--help"]]
    {
        let r = invoke(&args);
        assert_eq!(r.code, 0, "{args:?}: {}", r.stderr);
        assert!(r.stdout.contains("Usage: effectcraft-cli render-frame"), "{args:?}: {}", r.stdout);
    }
    let r = invoke(&["--json", "--bogus", "--help"]);
    assert_eq!(r.code, 0);
    assert!(r.stdout.contains("Usage: effectcraft-cli [OPTIONS] <COMMAND>"), "{}", r.stdout);
}

/// A writer whose writes are buffered and whose flush fails, like a full disk behind a buffer.
struct FlushFails;

impl Write for FlushFails {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Err(std::io::ErrorKind::StorageFull.into())
    }
}

#[test]
fn a_failed_final_flush_is_an_error() {
    let mut err = Vec::new();
    let code = effectcraft_cli::run(["commands", "--filter", "layer.new"], &mut FlushFails, &mut err);
    assert_eq!(code, 1);
    assert!(String::from_utf8_lossy(&err).contains("cannot write to stdout"), "{}", String::from_utf8_lossy(&err));
}

/// With `--project`, an argument that ends `.ecproj` is a value, not a second project.
#[test]
fn a_project_flag_leaves_the_positionals_alone() {
    let r = invoke(&["set", "--project", "work.ecproj", "Main", "#1", "text/sourceText", "caption.ecproj"]);
    assert_ne!(r.code, 2, "{}", r.stderr);
    usage_error(&["run", "a.ecproj", "b.ecproj", "project.summary"], "two projects given");
}
