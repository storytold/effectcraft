//! Developer tool: imports `.aep` files (no footage existence checks), exports them, and prints
//! per file and in total what was imported, exported and lost, as JSON (counts only, no names).
//!
//! `cargo run --release -p effectcraft-aep-export --example aepstats -- a.aep b.aep ...`
use std::collections::BTreeMap;

use effectcraft_aep_export::{ExportOptions, export};
use effectcraft_aep_import::{ImportOptions, import};
use effectcraft_project::Project;

fn main() {
    let mut files = serde_json::Map::new();
    let mut lost: BTreeMap<String, usize> = BTreeMap::new();
    let mut import_lost: BTreeMap<String, usize> = BTreeMap::new();
    let mut exported: BTreeMap<String, usize> = BTreeMap::new();
    let mut types: BTreeMap<String, usize> = BTreeMap::new();
    for (n, path) in std::env::args().skip(1).enumerate() {
        let Ok(bytes) = std::fs::read(&path) else { continue };
        let key = format!("file{:03}", n + 1);
        let mut p = Project::default();
        let none = |_: &str| false;
        let opts = ImportOptions { allow_losses: true, ..ImportOptions::new(&none) };
        let started = std::time::Instant::now();
        let imp = match import(&mut p, &bytes, &opts) {
            Ok(r) => r,
            Err(e) => {
                files.insert(key, serde_json::json!({ "importError": e.to_string() }));
                continue;
            }
        };
        for l in &imp.report.losses {
            *import_lost.entry(format!("{:?}", l.category)).or_default() += l.count;
        }
        let r = match export(&p, &ExportOptions { allow_losses: true }) {
            Ok(r) => r,
            Err(e) => {
                files.insert(key, serde_json::json!({ "exportError": e.to_string() }));
                continue;
            }
        };
        let ms = started.elapsed().as_millis();
        let back = {
            let mut q = Project::default();
            import(&mut q, &r.bytes, &opts).is_ok()
        };
        let mut per: BTreeMap<String, usize> = BTreeMap::new();
        for l in r.report.losses.iter().filter(|l| l.category == effectcraft_aep_export::LossCategory::FootageItems) {
            for d in &l.details {
                // "... footage of type '.mov' ..." -> the kind and extension only.
                if let Some(i) = d.find("footage of type '") {
                    let kind = d.get(..i).and_then(|h| h.rsplit(' ').nth(1)).unwrap_or("?");
                    let ext: String = d.get(i + 17..).unwrap_or("").chars().take_while(|c| *c != '\'').collect();
                    *types.entry(format!("{kind} {ext}")).or_default() += 1;
                } else if let Some(i) = d.find("': ") {
                    *types.entry(d.get(i + 3..).unwrap_or("").chars().take(40).collect()).or_default() += 1;
                }
            }
        }
        for l in &r.report.losses {
            *per.entry(format!("{:?}", l.category)).or_default() += l.count;
            *lost.entry(format!("{:?}", l.category)).or_default() += l.count;
        }
        let ex = serde_json::to_value(&r.report.exported).unwrap_or_default();
        if let Some(o) = ex.as_object() {
            for (k, v) in o {
                *exported.entry(k.clone()).or_default() += usize::try_from(v.as_u64().unwrap_or(0)).unwrap_or(0);
            }
        }
        files.insert(key, serde_json::json!({ "ms": ms, "reimports": back, "exported": ex, "lost": per, "importLost": imp.report.losses.iter().map(|l| l.count).sum::<usize>() }));
    }
    let out = serde_json::json!({ "total": { "exported": exported, "lost": lost, "importLost": import_lost, "footageNotWritten": types }, "files": files });
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
}
