//! Developer tool: prints `{file: {exported, losses: {category: count}, ...}}` for .ecproj files
//! (counts only). With `--write DIR` first, each project is also exported to `DIR/<name>.aep`,
//! parsed back and imported again, and the result is recorded.
//!
//! `cargo run --release -p effectcraft-aep-export --example ecanalyze -- [--write DIR] a.ecproj ...`
use effectcraft_project::Project;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let out_dir = if args.first().map(String::as_str) == Some("--write") && args.len() > 1 {
        let d = args.remove(1);
        args.remove(0);
        Some(std::path::PathBuf::from(d))
    } else {
        None
    };
    let mut out = serde_json::Map::new();
    for path in args {
        let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let Ok(p) = Project::from_json(&text) else {
            out.insert(name, serde_json::json!({ "error": "not a project" }));
            continue;
        };
        let r = effectcraft_aep_export::analyze(&p);
        let losses: serde_json::Map<String, serde_json::Value> =
            r.losses.iter().map(|l| (format!("{:?}", l.category), serde_json::Value::from(l.count))).collect();
        let mut entry = serde_json::json!({"exported": r.exported, "losses": losses, "total": r.total_losses()});
        if let Some(dir) = &out_dir {
            let written = match effectcraft_aep_export::export(&p, &effectcraft_aep_export::ExportOptions { allow_losses: true }) {
                Ok(res) => {
                    let file = dir.join(format!("{}.aep", name.trim_end_matches(".ecproj")));
                    let saved = std::fs::write(&file, &res.bytes).is_ok();
                    let parsed = effectcraft_aep::parse(&res.bytes).is_ok();
                    let mut q = Project::default();
                    let none = |_: &str| false;
                    let opts = effectcraft_aep_import::ImportOptions { allow_losses: true, ..effectcraft_aep_import::ImportOptions::new(&none) };
                    let reimport = effectcraft_aep_import::import(&mut q, &res.bytes, &opts);
                    serde_json::json!({
                        "bytes": res.bytes.len(),
                        "saved": saved,
                        "parses": parsed,
                        "reimports": reimport.is_ok(),
                        "reimportLosses": reimport.map(|r| r.report.losses.iter().map(|l| l.count).sum::<usize>()).unwrap_or(usize::MAX),
                    })
                }
                Err(e) => serde_json::json!({ "error": e.to_string() }),
            };
            if let Some(o) = entry.as_object_mut() {
                o.insert("export".into(), written);
            }
        }
        out.insert(name, entry);
    }
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
}
