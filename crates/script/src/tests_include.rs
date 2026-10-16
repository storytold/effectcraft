//! #759: ExtendScript's `#include` / `//@include` / `#includepath` directives.

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
    let dir = std::env::temp_dir().join(format!("ec-include-{tag}-{}", std::process::id()));
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
fn includes_are_inlined_relative_to_the_script() {
    let dir = tmp("ok");
    write(&dir.join("lib/math.jsx"), "#include \"util.jsxinc\"\nfunction twice(x) { return 2 * x; }");
    write(&dir.join("lib/util.jsxinc"), "// includes its includer back: inlined once only\n#include \"math.jsx\"\nvar unit = 'px';");
    write(&dir.join("vendor/extra.jsx"), "var extra = 40;");
    write(
        &dir.join("main.jsx"),
        "#target aftereffects\n#targetengine \"main\"\n#strict on\n//@include 'lib/math.jsx'\n#includepath \"vendor;lib\"\n#include \"extra.jsx\";\n#include \"util.jsxinc\"\n[twice(1) + extra, unit, $.fileName.split('\\\\').pop().split('/').pop()]",
    );
    let mut s = session();
    let o = run_file(&mut s, &dir.join("main.jsx"));
    assert!(o.error.is_none(), "{:?}", o.error);
    assert_eq!(o.result, json!([42, "px", "main.jsx"]));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn errors_point_at_the_file_and_line_they_came_from() {
    let dir = tmp("err");
    write(&dir.join("lib.jsx"), "var a = 1;\nvar b = 2;\nnotDefined();");
    write(&dir.join("main.jsx"), "// main\n#include \"lib.jsx\"\nvar c = 3;");
    let mut s = session();
    let e = run_file(&mut s, &dir.join("main.jsx")).error.expect("an error");
    assert!(e.file.ends_with("lib.jsx") && e.line == Some(3), "{e:?}");
    // After the include, lines in the script itself keep their numbers.
    write(&dir.join("lib.jsx"), "var a = 1;\nvar b = 2;");
    write(&dir.join("main.jsx"), "// main\n#include \"lib.jsx\"\nvar c = 3;\nalsoNotDefined();");
    let e = run_file(&mut s, &dir.join("main.jsx")).error.expect("an error");
    assert!(e.file.ends_with("main.jsx") && e.line == Some(4), "{e:?}");
    // A missing file is a clear error on the directive's line.
    write(&dir.join("main.jsx"), "var x = 1;\n#include \"missing.jsx\"");
    let e = run_file(&mut s, &dir.join("main.jsx")).error.expect("an error");
    assert!(e.message.contains("missing.jsx") && e.line == Some(2), "{e:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hostile_includes_are_bounded_and_gated() {
    let dir = tmp("hostile");
    // A chain deeper than the limit is an error, not a stack overflow.
    for i in 0..100 {
        write(&dir.join(format!("chain/{i}.jsx")), &format!("#include \"{}.jsx\"", i + 1));
    }
    write(&dir.join("chain/100.jsx"), "1");
    let mut s = session();
    let e = run_file(&mut s, &dir.join("chain/0.jsx")).error.expect("an error");
    assert!(e.message.contains("nested too deeply"), "{e:?}");
    // Files outside the script's (and the project's) folder need the file preference.
    write(&dir.join("outside.jsx"), "var secret = 7;");
    write(&dir.join("app/main.jsx"), "#include \"../outside.jsx\"\nsecret");
    let e = run_file(&mut s, &dir.join("app/main.jsx")).error.expect("blocked");
    assert!(e.message.contains("Allow Scripts to Write Files"), "{e:?}");
    // Code run without a file can't include relative to anything but the project's folder.
    let e = run_code(&mut s, "#include \"outside.jsx\"", "console").error.expect("no folder");
    assert!(e.message.contains("outside.jsx"), "{e:?}");
    s.prefs.scripting.allow_scripts_write_files = true;
    let o = run_file(&mut s, &dir.join("app/main.jsx"));
    assert_eq!(o.result, json!(7), "{:?}", o.error);
    let _ = std::fs::remove_dir_all(&dir);
}
