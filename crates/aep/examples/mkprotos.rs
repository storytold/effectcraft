//! Developer tool: extracts the property and layer prototypes the exporter clones from a project
//! that After Effects saved (a generated reference project: one composition, a solid layer with
//! static Transform values, a solid layer with two keys on every Transform property, a null).
//!
//! `cargo run -p effectcraft-aep --example mkprotos -- ref1.aep crates/aep/template/protos.aep`
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
use effectcraft_aep::{AepFile, Body, Chunk, parse, write};
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

const NAMES: [&str; 5] = ["ADBE Anchor Point", "ADBE Position", "ADBE Scale", "ADBE Rotate Z", "ADBE Opacity"];

fn tdmn_name(c: &Chunk<'_>) -> Option<String> {
    let d = c.bytes()?;
    Some(d.iter().take_while(|b| **b != 0).map(|b| *b as char).collect())
}
fn u32_at(c: &Chunk<'_>, off: usize) -> u32 {
    let d = c.bytes().unwrap_or(&[]);
    d.get(off..off + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]])).unwrap_or(0)
}

fn transform_five(layr: &Chunk<'static>) -> Vec<Chunk<'static>> {
    let tdgp = layr.child_list(b"tdgp").or_exit("layer tdgp");
    let kids = tdgp.children();
    let at = kids.iter().position(|c| c.is(b"tdmn") && tdmn_name(c).as_deref() == Some("ADBE Transform Group")).or_exit("transform");
    let group = &kids[at + 1];
    let gk = group.children();
    let mut out = Vec::new();
    for n in NAMES {
        let i = gk.iter().position(|c| c.is(b"tdmn") && tdmn_name(c).as_deref() == Some(n)).unwrap_or_else(|| exit(&format!("missing {n}")));
        out.push(gk[i].clone());
        out.push(gk[i + 1].clone());
    }
    out
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(a.get(1).or_exit("usage: see the comment at the top of this file")).or_exit("read ref");
    let f = parse(&bytes).or_exit("parse").into_owned();
    let fold = f.list(b"Fold").or_exit("Fold");
    let comp = fold.children().iter().find(|c| c.is_list(b"Item")).or_exit("comp");
    let kids = comp.children();
    let layer_blocks: Vec<&[Chunk<'static>]> = {
        let mut starts: Vec<usize> = kids.iter().enumerate().filter(|(_, c)| c.is_list(b"Layr")).map(|(i, _)| i).collect();
        let end = kids.iter().position(|c| c.is_list(b"DLay")).or_exit("DLay");
        starts.push(end);
        starts.windows(2).map(|w| &kids[w[0]..w[1]]).collect()
    };
    let id_of = |b: &[Chunk<'static>]| u32_at(b[0].child(b"ldta").or_exit("ldta"), 0);
    let find = |id: u32| layer_blocks.iter().find(|b| id_of(b) == id).unwrap_or_else(|| exit(&format!("layer {id:#x}")));
    let stat = transform_five(&find(0x1b)[0]);
    let keyd = transform_five(&find(0x1d)[0]);
    let nulb: Vec<Chunk<'static>> = find(0x1f).to_vec();
    let solids = fold.children().iter().rev().find(|c| c.is_list(b"Item")).or_exit("Solids");
    let sfdr = solids.child_list(b"Sfdr").or_exit("Sfdr");
    let nuli = sfdr
        .children()
        .iter()
        .find(|c| c.is_list(b"Item") && u32_at(c.child(b"iide").or_exit("iide"), 0) == u32::from_le_bytes(0x1eu32.to_be_bytes()))
        .or_exit("null item")
        .clone();
    let out = AepFile {
        chunks: vec![Chunk::list(*b"stat", stat), Chunk::list(*b"keyd", keyd), Chunk::list(*b"nulb", nulb), Chunk::list(*b"nuli", vec![nuli])],
        trailer: Cow::Owned(vec![]),
    };
    let _ = Body::Data(Cow::Borrowed(&[][..]));
    std::fs::write(a.get(2).or_exit("usage: see the comment at the top of this file"), write(&out).or_exit("write")).or_exit("write file");
}
