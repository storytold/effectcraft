//! Developer lab (text experiments): rewrites the text document of fixture 113 (`Hello`, 60 px,
//! orange) with byte-level replacements and writes the result, to learn what After Effects needs
//! when the text changes.
//!
//! `cargo run --release -p effectcraft-aep-export --example textlab -- <case t1|t2|t3> <out.aep>`
use effectcraft_aep::{Body, Chunk, parse, write};
use std::borrow::Cow;

fn utf16(s: &str) -> Vec<u8> {
    let mut v = vec![0xfe, 0xff];
    for u in s.encode_utf16() {
        v.extend(u.to_be_bytes());
    }
    v
}

fn replace(hay: &[u8], from: &[u8], to: &[u8], all: bool) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    let mut done = false;
    while i < hay.len() {
        if (!done || all) && hay[i..].starts_with(from) {
            out.extend_from_slice(to);
            i += from.len();
            done = true;
        } else {
            out.push(hay[i]);
            i += 1;
        }
    }
    assert!(done, "pattern not found");
    out
}

fn edit(c: &mut Chunk<'static>, f: &dyn Fn(&[u8]) -> Vec<u8>) {
    match &mut c.body {
        Body::OpaqueList { list_type, data } if list_type == b"btdk" => {
            *data = Cow::Owned(f(data));
        }
        Body::List { children, .. } | Body::Chunks(children) => {
            for k in children {
                edit(k, f);
            }
        }
        _ => {}
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../aep/tests/fixtures");
    let bytes = std::fs::read(dir.join("113-text-Hello-size60-fillorange.aep")).expect("fixture");
    let mut f = parse(&bytes).expect("parse").into_owned();
    let hello = [b"(".to_vec(), utf16("Hello\r"), b")".to_vec()].concat();
    let case = a[1].clone();
    let fx = move |d: &[u8]| -> Vec<u8> {
        match case.as_str() {
            // Same length, stale layout.
            "t1" => replace(d, &hello, &[b"(".to_vec(), utf16("World\r"), b")".to_vec()].concat(), false),
            // Longer text, runs updated, stale layout.
            "t2" | "t3" => {
                let mut v = replace(d, &hello, &[b"(".to_vec(), utf16("Hello World\r"), b")".to_vec()].concat(), false);
                v = replace(&v, b"/1 6 >> ]", b"/1 12 >> ]", true);
                if case == "t3" {
                    // Drop the composed layout: everything in `/2 [ << /99 /PC ... ] /2 <<` -> `/2 [ ]`.
                    let start = v.windows(15).position(|w| w == b"/2 [ << /99 /PC").expect("layout") + 3;
                    // The matching `]`, skipping strings.
                    let (mut depth, mut i, mut end) = (0i32, start, start);
                    while i < v.len() {
                        match v[i] {
                            b'(' => {
                                i += 1;
                                while i < v.len() && v[i] != b')' {
                                    if v[i] == b'\\' {
                                        i += 1;
                                    }
                                    i += 1;
                                }
                            }
                            b'[' => depth += 1,
                            b']' => {
                                depth -= 1;
                                if depth == 0 {
                                    end = i;
                                    break;
                                }
                            }
                            _ => {}
                        }
                        i += 1;
                    }
                    let mut w = v[..start].to_vec();
                    w.extend_from_slice(b"[ ]");
                    w.extend_from_slice(&v[end + 1..]);
                    v = w;
                }
                v
            }
            _ => panic!("case"),
        }
    };
    for c in &mut f.chunks {
        edit(c, &fx);
    }
    std::fs::write(&a[2], write(&f).expect("write")).expect("write file");
}
