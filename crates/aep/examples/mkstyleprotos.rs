//! Developer tool: extracts the layer-style prototypes the exporter clones from two synthetic
//! fixtures that After Effects saved (the single-case files, our own generated corpus):
//!
//! - `977-styles-all-params-changed`: one static record (`tdmn` + `tdbs`) of every parameter of
//!   the nine styles and of Blending Options, as layer `B` stores them (list `stpr`, in file
//!   order; Green and Blue Channel Blend are derived from the Red record);
//! - `976-style-drop-shadow-opacity-keyed-100-20-linear`: the keyed Drop Shadow Opacity record
//!   (list `stky`), the keyed form of a scalar.
//!
//! `cargo run -p effectcraft-aep --example mkstyleprotos -- <single-case dir> crates/aep/template/styles.aep`
//!
//! The prototypes are cut from the earlier single-case files, which are not in the repository
//! (the corpus holds the combined projects; the same cases cut from them give other item
//! and layer ids, so the template would change). `<single-case dir>` is a folder with those files.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
use effectcraft_aep::{AepFile, Chunk, parse, write};
use std::borrow::Cow;

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

fn tdmn_name(c: &Chunk<'_>) -> Option<String> {
    let d = c.bytes()?;
    Some(d.iter().take_while(|b| **b != 0).map(|b| *b as char).collect())
}

/// The layer-styles group of the first layer that has a style applied.
fn styles(path: &std::path::Path) -> Chunk<'static> {
    let bytes = std::fs::read(path).or_exit("read fixture");
    let f: AepFile<'static> = parse(&bytes).or_exit("parse").into_owned();
    let fold = f.list(b"Fold").or_exit("Fold");
    let comp = fold.children().iter().find(|c| c.is_list(b"Item") && c.child(b"cdta").is_some()).or_exit("comp");
    for layr in comp.children().iter().filter(|c| c.is_list(b"Layr")) {
        let root = layr.child_list(b"tdgp").or_exit("layer tdgp");
        let k = root.children();
        let i = k.iter().position(|c| c.is(b"tdmn") && tdmn_name(c).as_deref() == Some("ADBE Layer Styles")).or_exit("styles group");
        let g = k[i + 1].clone();
        let flags = g.child(b"tdsb").and_then(|c| c.bytes()).map(|b| b.to_vec());
        if flags.as_deref() == Some(&[0, 0, 0, 1]) {
            return g;
        }
    }
    exit("no layer with a style");
}

/// Every `(tdmn, tdbs)` pair below `g`, depth first.
fn records(g: &Chunk<'static>, out: &mut Vec<Chunk<'static>>) {
    let k = g.children();
    for (i, c) in k.iter().enumerate() {
        if c.is(b"tdmn")
            && let Some(n) = k.get(i + 1)
        {
            if n.is_list(b"tdbs") {
                out.push(c.clone());
                out.push(n.clone());
            } else if n.is_list(b"tdgp") {
                records(n, out);
            }
        }
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let dir = std::path::Path::new(a.get(1).or_exit("usage: see the comment at the top of this file"));
    let mut all = Vec::new();
    records(&styles(&dir.join("977-styles-all-params-changed.aep")), &mut all);
    assert_eq!(all.len(), 2 * 87, "977 stores 87 records");
    // 977 switched only the Red channel off; Green and Blue are the same checkbox (class (4, 4),
    // no range): their records are Red's under their own match names.
    let red = all.iter().position(|c| c.is(b"tdmn") && tdmn_name(c).as_deref() == Some("ADBE R Channel Blend")).or_exit("red");
    for name in ["ADBE G Channel Blend", "ADBE B Channel Blend"] {
        let mut m = vec![0u8; 40];
        m[..name.len()].copy_from_slice(name.as_bytes());
        let rec = all[red + 1].clone();
        all.push(Chunk::data(*b"tdmn", m));
        all.push(rec);
    }
    let mut keyed = Vec::new();
    records(&styles(&dir.join("976-style-drop-shadow-opacity-keyed-100-20-linear.aep")), &mut keyed);
    assert_eq!(keyed.len(), 2, "976 stores one record");
    let out = AepFile { chunks: vec![Chunk::list(*b"stpr", all), Chunk::list(*b"stky", keyed)], trailer: Cow::Owned(vec![]) };
    std::fs::write(a.get(2).or_exit("usage: see the comment at the top of this file"), write(&out).or_exit("write")).or_exit("write file");
}
