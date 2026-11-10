//! Regression tests as data: every `tests/repros/*.jsonl` (repository root) is a replay script
//! (`effectcraft_engine::replay`, docs/agents.md "Repro scripts") run against a fully wired
//! headless session, like `effectcraft-cli replay`. A crash fix's regression test is a `.jsonl`
//! dropped there: it must replay without a panic and meet its expectations.

use std::path::{Path, PathBuf};

use effectcraft_engine::replay;

fn repros_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/repros")
}

#[test]
fn every_repro_replays_cleanly() {
    let mut files: Vec<PathBuf> =
        std::fs::read_dir(repros_dir()).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|x| x == "jsonl")).collect();
    files.sort();
    assert!(!files.is_empty(), "no repros in {}", repros_dir().display());
    let mut failures = vec![];
    for f in &files {
        let script = std::fs::read_to_string(f).unwrap();
        let mut s = effectcraft_host::session();
        let opts = replay::Options { keep_going: false, base_dir: f.parent().map(Path::to_path_buf) };
        let r = replay::run(&mut s, &script, &opts);
        if !r.ok {
            failures.push(format!("{}: {}", f.display(), serde_json::to_string(&r.first_failure).unwrap()));
        }
    }
    assert!(failures.is_empty(), "{} of {} repros failed:\n{}", failures.len(), files.len(), failures.join("\n"));
}
