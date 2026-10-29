//! Developer tool: imports an `.aep` (no footage existence checks), exports it again and prints the
//! export report.
//!
//! `cargo run --release -p effectcraft-aep --example reexport -- in.aep out.aep`
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
use effectcraft_aep::export::{ExportOptions, export};
use effectcraft_aep::import::{ImportOptions, import};
use effectcraft_project::Project;

/// Ends this developer tool with a message instead of a panic.
trait OrExit<T> {
    fn or_exit(self, what: &str) -> T;
}

impl<T> OrExit<T> for Option<T> {
    fn or_exit(self, what: &str) -> T {
        match self {
            Some(v) => v,
            None => exit(what),
        }
    }
}

impl<T, E: std::fmt::Debug> OrExit<T> for Result<T, E> {
    fn or_exit(self, what: &str) -> T {
        match self {
            Ok(v) => v,
            Err(e) => exit(&format!("{what}: {e:?}")),
        }
    }
}

fn exit(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(1)
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(a.get(1).or_exit("usage: see the comment at the top of this file")).or_exit("read");
    let mut p = Project::default();
    let none = |_: &str| false;
    let opts = ImportOptions { allow_losses: true, ..ImportOptions::new(&none) };
    let imported = import(&mut p, &bytes, &opts).or_exit("import");
    p.settings = imported.settings.clone();
    eprintln!("import losses: {}", imported.report.losses.iter().map(|l| format!("{:?} {}", l.category, l.count)).collect::<Vec<_>>().join(", "));
    let r = export(&p, &ExportOptions { allow_losses: true }).or_exit("export");
    std::fs::write(a.get(2).or_exit("usage: see the comment at the top of this file"), &r.bytes).or_exit("write");
    println!("{}", serde_json::to_string_pretty(&r.report).or_exit("json"));
}
