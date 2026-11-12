//! Scripts see the host's files: existence checks go through `Services`, like reads, so a host
//! that keeps files in memory (the web editor's browser storage) answers for its own files.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use effectcraft_engine::{Services, Session};

use crate::run_code;

#[derive(Default)]
struct Mem(Mutex<HashMap<String, Vec<u8>>>);

impl Services for Mem {
    fn read_file(&self, path: &str) -> std::io::Result<Vec<u8>> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(path)
            .cloned()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, path.to_string()))
    }
    fn write_file(&self, path: &str, data: &[u8]) -> std::io::Result<()> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).insert(path.into(), data.into());
        Ok(())
    }
    fn exists(&self, path: &str) -> bool {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).contains_key(path)
    }
    fn is_dir(&self, path: &str) -> bool {
        path == "/browser/footage"
    }
}

#[test]
fn a_script_opens_a_file_only_the_host_holds() {
    let mem = Arc::new(Mem::default());
    mem.write_file("/browser/data.txt", b"hello").unwrap();
    let mut s = Session { services: mem, ..Default::default() };
    crate::install(&mut s);
    // A project in a folder that isn't on disk: scripts may read files next to it.
    s.path = Some("/browser/project.ecproj".into());
    let out = run_code(
        &mut s,
        r#"var f = new File("/browser/data.txt"); var ok = f.open("r");
        writeLn(ok + " " + (ok ? f.read() : f.error));
        writeLn(new File("/browser/missing.txt").exists + " " + new Folder("/browser/footage").exists);"#,
        "t.jsx",
    );
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(out.output, vec!["true hello".to_string(), "false true".to_string()]);
}
