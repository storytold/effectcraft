//! Developer tool: extracts the footage item prototypes the exporter clones from our own synthetic
//! cases (the local synthetic case folder, not in this repository; media rendered by EffectCraft in `test-media\`, see
//! `docs/aep-export.md` R28): png still 607, jpeg still 605, OpenEXR still 604, H.264 mp4 606,
//! WAV 351, png sequence 353, OpenEXR sequence 603, jpeg sequence 611, ProRes mov 612, TIFF 613, Targa 650, flat Photoshop 651, Illustrator (PDF) 652, EPS 653. Each item's alias JSON is replaced by a
//! neutral one (no local path or machine name); the exporter writes its own.
//!
//! `cargo run -p effectcraft-aep-export --example mkftprotos -- <synthetic dir> crates/aep-export/template/footage.aep`
use effectcraft_aep::{AepFile, Body, Chunk, parse, write};
use std::borrow::Cow;

const NEUTRAL: &[u8] = br#"{"ascendcount_base":1,"ascendcount_target":1,"fullpath":"C:\\fixtures\\file","platform":1,"server_name":"HOST000","server_volume_name":"Volume","target_is_folder":false}"#;

fn load(dir: &std::path::Path, prefix: &str) -> AepFile<'static> {
    let name = std::fs::read_dir(dir)
        .expect("dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .find(|n| n.starts_with(prefix) && n.ends_with(".aep"))
        .unwrap_or_else(|| panic!("case {prefix}"));
    let bytes = std::fs::read(dir.join(name)).expect("read");
    parse(&bytes).expect("parse").into_owned()
}

/// The first footage item that is not a solid, with the f-records that follow it.
fn footage(f: &AepFile<'static>) -> Vec<Chunk<'static>> {
    fn walk(kids: &[Chunk<'static>], out: &mut Option<Vec<Chunk<'static>>>) {
        for (i, c) in kids.iter().enumerate() {
            if out.is_some() {
                return;
            }
            if c.is_list(b"Item") {
                if let Some(pin) = c.child_list(b"Pin ") {
                    let tag = pin.child(b"sspc").and_then(|s| s.bytes()).map(|b| b[22..26].to_vec()).unwrap_or_default();
                    if tag != b"Soli" {
                        let mut v = vec![c.clone()];
                        v.extend(kids[i + 1..].iter().take_while(|k| k.id[0] == b'f' && !k.is_list(b"FEE ")).cloned());
                        *out = Some(v);
                        return;
                    }
                }
                if let Some(sfdr) = c.child_list(b"Sfdr") {
                    walk(sfdr.children(), out);
                }
            }
        }
    }
    let mut out = None;
    walk(f.list(b"Fold").expect("Fold").children(), &mut out);
    out.expect("footage item")
}

fn neutralise(c: &mut Chunk<'static>) {
    if c.is(b"alas") {
        c.body = Body::Data(Cow::Owned(NEUTRAL.to_vec()));
        c.pad = if NEUTRAL.len() % 2 == 1 { Some(0) } else { None };
        return;
    }
    if let Body::List { children, .. } | Body::Chunks(children) = &mut c.body {
        for k in children {
            neutralise(k);
        }
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let dir = std::path::Path::new(&a[1]);
    let mut chunks = Vec::new();
    for (case, key) in [
        ("607", *b"png "),
        ("605", *b"jpg "),
        ("604", *b"exr "),
        ("606", *b"mp4 "),
        ("351", *b"wav "),
        ("353", *b"pngS"),
        ("603", *b"exrS"),
        ("611", *b"jpgS"),
        ("612", *b"mov "),
        ("613", *b"tif "),
        ("650", *b"tga "),
        ("651", *b"psd "),
        ("652", *b"ai  "),
        ("653", *b"eps "),
    ] {
        let mut item = footage(&load(dir, case));
        for c in &mut item {
            neutralise(c);
        }
        chunks.push(Chunk::list(key, item));
    }
    let out = AepFile { chunks, trailer: Cow::Owned(vec![]) };
    let bytes = write(&out).expect("write");
    let low = String::from_utf8_lossy(&bytes).to_lowercase();
    for needle in ["\\users\\", "/users/", "/home/"] {
        assert!(!low.contains(needle), "local data left: {needle}");
    }
    std::fs::write(&a[2], bytes).expect("write file");
}
