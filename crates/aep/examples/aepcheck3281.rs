//! Local check tool for #3281 (never committed).
//! aepcheck3281 dump <dir>            -> per fixture: hashes of the imported project, import report, export bytes, export report
//! aepcheck3281 bench <file> <reps>   -> median import / export time in microseconds
//! aepcheck3281 scale <in.aep> <n> <out.aep>
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::hint::black_box;
use std::time::Instant;

use effectcraft_aep::{Body, Chunk};

fn fnv(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn dump(dir: &str) {
    let mut names: Vec<_> = std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).collect();
    names.sort();
    let exists = |_: &str| true;
    let opts = effectcraft_aep::import::ImportOptions { allow_losses: true, ..effectcraft_aep::import::ImportOptions::new(&exists) };
    for path in names {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext != "aep" && ext != "aepx" {
            continue;
        }
        let bytes = std::fs::read(&path).unwrap();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let mut p = effectcraft_project::Project::default();
        match effectcraft_aep::import::import(&mut p, &bytes, &opts) {
            Ok(r) => {
                let pj = serde_json::to_vec(&p).unwrap();
                let rj = serde_json::to_vec(&r.report).unwrap();
                let summary = (r.report.total_losses(), r.report.is_lossless(), r.report.summary());
                let eo = effectcraft_aep::export::ExportOptions { allow_losses: true };
                let (eb, er, es) = match effectcraft_aep::export::export(&p, &eo) {
                    Ok(x) => (
                        fnv(&x.bytes),
                        fnv(&serde_json::to_vec(&x.report).unwrap()),
                        format!("{:?}", (x.report.total_losses(), x.report.is_lossless(), x.report.summary())),
                    ),
                    Err(e) => (0, 0, format!("ERR {e}")),
                };
                println!("{name} project={:016x} report={:016x} export={eb:016x} export_report={er:016x} isum={summary:?} esum={es}", fnv(&pj), fnv(&rj));
            }
            Err(e) => println!("{name} IMPORT ERR {e}"),
        }
    }
}

fn time<T>(reps: usize, mut f: impl FnMut() -> T) -> f64 {
    let mut v = Vec::with_capacity(reps);
    for _ in 0..reps {
        let t = Instant::now();
        black_box(f());
        v.push(t.elapsed().as_secs_f64() * 1e6);
    }
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn bench(path: &str, reps: usize) {
    let bytes = std::fs::read(path).unwrap();
    let exists = |_: &str| true;
    let opts = effectcraft_aep::import::ImportOptions { allow_losses: true, ..effectcraft_aep::import::ImportOptions::new(&exists) };
    let m = time(reps, || {
        let mut p = effectcraft_project::Project::default();
        effectcraft_aep::import::import(&mut p, &bytes, &opts).map(|r| r.items.len()).ok()
    });
    println!("import {m:.1} us");
    let mut proj = effectcraft_project::Project::default();
    effectcraft_aep::import::import(&mut proj, &bytes, &opts).unwrap();
    let eo = effectcraft_aep::export::ExportOptions { allow_losses: true };
    let m = time(reps, || effectcraft_aep::export::export(&proj, &eo).map(|r| r.bytes.len()).ok());
    println!("export {m:.1} us");
}

fn scale(input: &str, n: usize, out: &str) {
    let bytes = std::fs::read(input).unwrap();
    let mut file = effectcraft_aep::parse(&bytes).unwrap();
    for c in &mut file.chunks {
        if c.is_list(b"Fold")
            && let Body::List { children, .. } = &mut c.body
        {
            let items: Vec<Chunk<'_>> = children.iter().filter(|c| c.is_list(b"Item")).cloned().collect();
            for _ in 1..n {
                children.extend(items.iter().cloned());
            }
        }
    }
    std::fs::write(out, effectcraft_aep::write(&file).unwrap()).unwrap();
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(String::as_str) {
        Some("dump") => dump(&a[2]),
        Some("bench") => bench(&a[2], a.get(3).and_then(|s| s.parse().ok()).unwrap_or(20)),
        Some("scale") => scale(&a[2], a[3].parse().unwrap(), &a[4]),
        _ => eprintln!("usage: see the top of this file"),
    }
}
