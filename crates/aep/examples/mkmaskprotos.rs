//! Developer tool: extracts the mask prototypes the exporter clones from three synthetic fixtures
//! that After Effects saved (the single-case files, our own generated corpus):
//!
//! - `091-mask-rect-props`: the `ADBE Mask Parade` group of layer `L` with one mask (its `mkif`, the
//!   atom with a static path and static Feather, Opacity and Expansion records);
//! - `093-mask-path-2keys`: the path property (`tdbs`) of a mask with two linear path keys;
//! - `208-key-mask-feather-2key-linear`: the Mask Feather property with two linear keys.
//!
//! `cargo run -p effectcraft-aep --example mkmaskprotos -- <single-case dir> crates/aep/template/masks.aep`
//!
//! The prototypes are cut from the earlier single-case files, which are not in the repository
//! (`tests/fixtures` holds the combined projects; the same cases cut from them give other item
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

/// The `(tdmn, node)` pair named `name` among `kids`.
fn pair(kids: &[Chunk<'static>], name: &str) -> (Chunk<'static>, Chunk<'static>) {
    let i = kids.iter().position(|c| c.is(b"tdmn") && tdmn_name(c).as_deref() == Some(name)).unwrap_or_else(|| exit(&format!("missing {name}")));
    (kids[i].clone(), kids[i + 1].clone())
}

/// The layer `L`'s mask parade of a fixture.
fn parade(path: &std::path::Path) -> (Chunk<'static>, Chunk<'static>) {
    let bytes = std::fs::read(path).or_exit("read fixture");
    let f: AepFile<'static> = parse(&bytes).or_exit("parse").into_owned();
    let fold = f.list(b"Fold").or_exit("Fold");
    let comp = fold.children().iter().find(|c| c.is_list(b"Item") && c.child(b"cdta").is_some()).or_exit("comp");
    let layr = comp.children().iter().find(|c| c.is_list(b"Layr")).or_exit("layer");
    let root = layr.child_list(b"tdgp").or_exit("layer tdgp");
    pair(root.children(), "ADBE Mask Parade")
}

/// The atom of the first mask in a parade (after its match name comes the `mkif`, then the atom).
fn atom(parade: &Chunk<'static>) -> Chunk<'static> {
    parade.children().iter().find(|c| c.is_list(b"tdgp")).or_exit("mask atom").clone()
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let dir = std::path::Path::new(a.get(1).or_exit("usage: see the comment at the top of this file"));
    let (tdmn, props) = parade(&dir.join("091-mask-rect-props.aep"));
    let keyed_path = {
        let (_, p) = parade(&dir.join("093-mask-path-2keys.aep"));
        let (_, oms) = pair(atom(&p).children(), "ADBE Mask Shape");
        oms.child_list(b"tdbs").or_exit("path tdbs").clone()
    };
    let keyed_feather = {
        let (_, p) = parade(&dir.join("208-key-mask-feather-2key-linear.aep"));
        pair(atom(&p).children(), "ADBE Mask Feather").1
    };
    let out = AepFile {
        chunks: vec![Chunk::list(*b"mpar", vec![tdmn, props]), Chunk::list(*b"mpth", vec![keyed_path]), Chunk::list(*b"mfea", vec![keyed_feather])],
        trailer: Cow::Owned(vec![]),
    };
    std::fs::write(a.get(2).or_exit("usage: see the comment at the top of this file"), write(&out).or_exit("write")).or_exit("write file");
}
