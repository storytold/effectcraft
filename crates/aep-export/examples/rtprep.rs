//! Developer tool for round trips of a project with its footage: imports an `.aep` with a footage existence
//! check, saves the EffectCraft project, exports it again and prints counts as JSON (import and
//! export losses per category, footage found / missing per file extension, the compositions with
//! size, rate and duration), and the losses of importing the export again. Nothing is written
//! next to the footage.
//!
//! `cargo run --release -p effectcraft-aep-export --example rtprep -- in.aep out_dir`
//! writes `out_dir/project.ecproj`, `out_dir/export.aep` and the loss details of both directions
//! (`import-details.json`, `export-details.json`).
use effectcraft_aep_export::{ExportOptions, export_with};
use effectcraft_aep_import::{ImportOptions, import};
use effectcraft_project::Project;
use std::collections::BTreeMap;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 3 {
        eprintln!("usage: rtprep in.aep out_dir");
        std::process::exit(2);
    }
    let Ok(bytes) = std::fs::read(&a[1]) else {
        eprintln!("cannot read input");
        std::process::exit(1);
    };
    let dir = std::path::PathBuf::from(&a[2]);
    if std::fs::create_dir_all(&dir).is_err() {
        eprintln!("cannot create the output folder");
        std::process::exit(1);
    }
    let found = std::cell::RefCell::new(BTreeMap::<String, [usize; 2]>::new());
    let exists = |p: &str| {
        let ok = std::path::Path::new(p).exists();
        let ext = std::path::Path::new(p).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_else(|| "(folder)".into());
        let mut f = found.borrow_mut();
        let e = f.entry(ext).or_insert([0, 0]);
        if ok {
            e[0] += 1;
        } else {
            e[1] += 1;
        }
        ok
    };
    let mut p = Project::default();
    let opts = ImportOptions { allow_losses: true, ..ImportOptions::new(&exists) };
    let imported = match import(&mut p, &bytes, &opts) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("import failed: {e}");
            std::process::exit(1);
        }
    };
    // Opened as a new project: the file's settings come along (as file.importAep does).
    p.settings = imported.settings.clone();
    let import_losses: BTreeMap<String, usize> = imported.report.losses.iter().map(|l| (format!("{:?}", l.category), l.count)).collect();
    let details: BTreeMap<String, Vec<String>> = imported.report.losses.iter().map(|l| (format!("{:?}", l.category), l.details.clone())).collect();
    let _ = std::fs::write(dir.join("import-details.json"), serde_json::to_string_pretty(&details).unwrap_or_default());
    let _ = std::fs::write(dir.join("project.ecproj"), p.to_json());
    // Sequence folders with their modification time (metadata only), as file.exportAep does.
    let folder_time = |dir: &str| std::fs::metadata(dir).ok()?.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs());
    let r = match export_with(&p, &ExportOptions { allow_losses: true }, &folder_time) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("export failed: {e}");
            std::process::exit(1);
        }
    };
    let _ = std::fs::write(dir.join("export.aep"), &r.bytes);
    let export_details: BTreeMap<String, Vec<String>> = r.report.losses.iter().map(|l| (format!("{:?}", l.category), l.details.clone())).collect();
    let _ = std::fs::write(dir.join("export-details.json"), serde_json::to_string_pretty(&export_details).unwrap_or_default());
    let export_losses: BTreeMap<String, usize> = r.report.losses.iter().map(|l| (format!("{:?}", l.category), l.count)).collect();
    let comps: Vec<serde_json::Value> = p
        .comps()
        .map(|(id, c)| {
            serde_json::json!({
                "name": p.items.get(id).map(|i| i.name.clone()).unwrap_or_default(),
                "w": c.width, "h": c.height,
                "fps": c.frame_rate.num as f64 / c.frame_rate.den.max(1) as f64,
                "dur": c.duration.seconds(),
                "layers": c.layers.len(),
            })
        })
        .collect();
    let footage: BTreeMap<String, [usize; 2]> = found.borrow().clone();
    // The export imported again (no filesystem check: the paths are the ones checked above).
    let none = |_: &str| false;
    let mut p2 = Project::default();
    let (reimport_losses, reimported) = match import(&mut p2, &r.bytes, &ImportOptions { allow_losses: true, ..ImportOptions::new(&none) }) {
        Ok(r2) => (
            r2.report.losses.iter().map(|l| (format!("{:?}", l.category), l.count)).collect::<BTreeMap<String, usize>>(),
            serde_json::to_value(&r2.report.imported).unwrap_or_default(),
        ),
        Err(e) => (BTreeMap::from([(format!("failed: {e}"), 1)]), serde_json::Value::Null),
    };
    let out = serde_json::json!({
        "importLosses": import_losses,
        "exportLosses": export_losses,
        "exported": r.report.exported,
        "imported": serde_json::to_value(&imported.report.imported).unwrap_or_default(),
        "reimportLosses": reimport_losses,
        "reimported": reimported,
        "footageChecks": footage,
        "comps": comps,
    });
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
}
