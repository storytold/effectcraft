//! #760: `$.evalFile` and the `BridgeTalk` host identification.

use std::path::{Path, PathBuf};

use effectcraft_engine::Session;
use serde_json::json;

use crate::{Outcome, run_code};

fn session() -> Session {
    let mut s = Session::default();
    crate::install(&mut s);
    s
}

fn tmp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ec-evalfile-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// Run the script file `main` (its code read from disk, as File ▸ Scripts ▸ Run Script File does).
fn run_file(s: &mut Session, main: &Path) -> Outcome {
    let code = std::fs::read_to_string(main).unwrap();
    run_code(s, &code, &main.to_string_lossy())
}

#[test]
fn eval_file_evaluates_relative_to_the_calling_script() {
    let dir = tmp("ok");
    // lib/a.jsx loads b.jsx next to itself, not next to main.jsx.
    write(&dir.join("lib/a.jsx"), "var fromA = $.evalFile('b.jsx') + 1;\nfunction twice(x) { return 2 * x; }\nfromA");
    write(&dir.join("lib/b.jsx"), "var fromB = 20;\nfromB");
    write(
        &dir.join("main.jsx"),
        "var r = $.evalFile(new File($.fileName).path + '/lib/a.jsx');\nvar s = $.evalFile('lib/b.jsx');\n[r, s, twice(fromB), typeof fromA]",
    );
    let mut s = session();
    let o = run_file(&mut s, &dir.join("main.jsx"));
    assert!(o.error.is_none(), "{:?}", o.error);
    assert_eq!(o.result, json!([21, 20, 40, "number"]));
    // Errors in the evaluated file reach the caller, which can catch them.
    write(&dir.join("bad.jsx"), "throw new Error('bad file');");
    write(&dir.join("main.jsx"), "var m; try { $.evalFile('bad.jsx'); } catch (e) { m = e.message; } m");
    assert_eq!(run_file(&mut s, &dir.join("main.jsx")).result, json!("bad file"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn eval_file_is_bounded_and_gated() {
    let dir = tmp("hostile");
    // A file that evaluates itself forever stops at the depth limit.
    write(&dir.join("loop.jsx"), "$.evalFile('loop.jsx');");
    let mut s = session();
    let e = run_file(&mut s, &dir.join("loop.jsx")).error.expect("an error");
    assert!(e.message.contains("nested too deeply"), "{e:?}");
    // Deeply bracketed files are refused before they reach the parser.
    write(&dir.join("deep.jsx"), &"[".repeat(100_000));
    write(&dir.join("main.jsx"), "$.evalFile('deep.jsx')");
    let e = run_file(&mut s, &dir.join("main.jsx")).error.expect("an error");
    assert!(e.message.contains("nested too deeply"), "{e:?}");
    // Missing files are a clear error.
    write(&dir.join("main.jsx"), "$.evalFile('missing.jsx')");
    let e = run_file(&mut s, &dir.join("main.jsx")).error.expect("an error");
    assert!(e.message.contains("missing.jsx"), "{e:?}");
    // Files outside the script's (and the project's) folder need the file preference.
    write(&dir.join("outside.jsx"), "7");
    write(&dir.join("app/main.jsx"), "$.evalFile('../outside.jsx')");
    let e = run_file(&mut s, &dir.join("app/main.jsx")).error.expect("blocked");
    assert!(e.message.contains("Allow Scripts to Write Files"), "{e:?}");
    s.prefs.scripting.allow_scripts_write_files = true;
    let o = run_file(&mut s, &dir.join("app/main.jsx"));
    assert_eq!(o.result, json!(7), "{:?}", o.error);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bridgetalk_identifies_effectcraft() {
    let mut s = session();
    let o = run_code(
        &mut s,
        "[typeof BridgeTalk, BridgeTalk.appName, BridgeTalk.appVersion, BridgeTalk.appSpecifier, BridgeTalk.isRunning('aftereffects'), \
         BridgeTalk.getSpecifier('effectcraft'), BridgeTalk.getStatus('photoshop'), new BridgeTalk().sender]",
        "bt.jsx",
    );
    assert!(o.error.is_none(), "{:?}", o.error);
    let v = env!("CARGO_PKG_VERSION");
    let spec = format!("effectcraft-{v}");
    assert_eq!(o.result, json!(["function", "effectcraft", v, spec, false, spec, "ISNOTINSTALLED", spec]));
    // Messages to other applications are unsupported, with a clear error.
    let e = run_code(&mut s, "var bt = new BridgeTalk(); bt.target = 'photoshop'; bt.body = '1'; bt.send();", "bt.jsx").error.expect("unsupported");
    assert!(e.message.contains("not supported"), "{e:?}");
}
