//! Developer tool: imports an `.aep` (no footage existence checks), exports it again and prints the
//! export report.
//!
//! `cargo run --release -p effectcraft-aep-export --example reexport -- in.aep out.aep`
use effectcraft_aep_export::{ExportOptions, export};
use effectcraft_aep_import::{ImportOptions, import};
use effectcraft_project::Project;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&a[1]).expect("read");
    let mut p = Project::default();
    let none = |_: &str| false;
    let opts = ImportOptions { allow_losses: true, ..ImportOptions::new(&none) };
    let imported = import(&mut p, &bytes, &opts).expect("import");
    p.settings = imported.settings.clone();
    eprintln!("import losses: {}", imported.report.losses.iter().map(|l| format!("{:?} {}", l.category, l.count)).collect::<Vec<_>>().join(", "));
    let r = export(&p, &ExportOptions { allow_losses: true }).expect("export");
    std::fs::write(&a[2], &r.bytes).expect("write");
    println!("{}", serde_json::to_string_pretty(&r.report).expect("json"));
}
