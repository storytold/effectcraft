//! Developer tool: prints the chunk tree of an .aep (ids, sizes, short data), match names inline.
use effectcraft_aep::{Body, Chunk};
fn show(c: &Chunk<'_>, depth: usize, max: usize, hex: usize) {
    let ind = "  ".repeat(depth);
    let id = String::from_utf8_lossy(&c.id).to_string();
    match &c.body {
        Body::Data(d) if &c.id == b"tdmn" => {
            let n: String = d.iter().take_while(|b| **b != 0).map(|b| *b as char).collect();
            println!("{ind}tdmn {n}");
        }
        Body::Data(d) => {
            let h: String = d.iter().take(hex).map(|b| format!("{b:02x}")).collect();
            let a: String = d.iter().take(hex).map(|b| if (32..127).contains(b) { *b as char } else { '.' }).collect();
            println!("{ind}{id} [{}] {h} |{a}|", d.len());
        }
        Body::Chunks(k) => {
            println!("{ind}{id} (nested)");
            if depth < max {
                for x in k {
                    show(x, depth + 1, max, hex);
                }
            }
        }
        Body::List { list_type, children } => {
            println!("{ind}LIST {} ({} children)", String::from_utf8_lossy(list_type), children.len());
            if depth < max {
                for x in children {
                    show(x, depth + 1, max, hex);
                }
            }
        }
        Body::OpaqueList { list_type, data } => println!("{ind}LIST {} opaque [{}]", String::from_utf8_lossy(list_type), data.len()),
    }
}
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&a[1]).expect("read");
    let max: usize = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(4);
    let hex: usize = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(32);
    let f = effectcraft_aep::parse(&bytes).expect("parse");
    for c in &f.chunks {
        show(c, 0, max, hex);
    }
    println!("trailer {} bytes", f.trailer.len());
}
