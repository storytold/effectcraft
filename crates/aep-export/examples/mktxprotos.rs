//! Developer tool: extracts the text layer prototypes the exporter clones from two fixtures of
//! `crates/aep/tests/fixtures` (our own generated corpus, `docs/aep-export.md` R32):
//!
//! - `110-text-A`: the layer block (`Layr`, `Ewst`, f-records) of a point text layer `A`;
//! - `245-text-box-400x200-Hello`: the same for a 400 x 200 paragraph (box) text layer;
//! - `tanm`, `tani`, `tarc`: the `ADBE Text Animators` group of 579 emptied, its first animator
//!   (one range selector, the Position record removed), and one value record per animator and
//!   range selector property: 579 (Position), 592 (the other animator properties), 580-591
//!   (Start, End, Offset, Units and the Index range, Based On, Mode, Amount, Shape, Smoothness,
//!   Ease High / Low, Randomize Order, Random Seed), 728 (Character Offset).
//!
//! `cargo run -p effectcraft-aep-export --example mktxprotos -- crates/aep/tests/fixtures crates/aep-export/template/text.aep`
use effectcraft_aep::{AepFile, Chunk, parse, write};
use std::borrow::Cow;

/// The block of the text layer (`ldta@128` = 3) of the first composition.
fn text_block(path: &std::path::Path) -> Vec<Chunk<'static>> {
    let bytes = std::fs::read(path).expect("read fixture");
    let f: AepFile<'static> = parse(&bytes).expect("parse").into_owned();
    let fold = f.list(b"Fold").expect("Fold");
    let comp = fold.children().iter().find(|c| c.is_list(b"Item") && c.child(b"cdta").is_some()).expect("comp");
    let kids = comp.children();
    let is_text = |c: &Chunk<'_>| c.is_list(b"Layr") && c.child(b"ldta").and_then(|d| d.bytes()).is_some_and(|b| b[128..132] == [0, 0, 0, 3]);
    let start = kids.iter().position(is_text).expect("text layer");
    let end = kids.iter().skip(start + 1).position(|c| c.is_list(b"Layr") || c.is_list(b"DLay")).map(|i| start + 1 + i).expect("end");
    kids[start..end].to_vec()
}

fn tdmn_name(c: &Chunk<'_>) -> Option<String> {
    let d = c.bytes()?;
    Some(d.iter().take_while(|b| **b != 0).map(|b| *b as char).collect())
}

/// `tdmn name` + node below `c`; the `skip`-th match (several animators share names).
fn below(c: &Chunk<'static>, name: &str, skip: usize) -> Vec<Chunk<'static>> {
    fn walk(kids: &[Chunk<'static>], name: &str, skip: &mut usize) -> Option<Vec<Chunk<'static>>> {
        for (i, c) in kids.iter().enumerate() {
            if c.is(b"tdmn") && tdmn_name(c).as_deref() == Some(name) {
                if *skip == 0 {
                    return Some(vec![c.clone(), kids.get(i + 1)?.clone()]);
                }
                *skip -= 1;
            }
            if let Some(r) = walk(c.children(), name, skip) {
                return Some(r);
            }
        }
        None
    }
    let mut skip = skip;
    walk(c.children(), name, &mut skip).unwrap_or_else(|| panic!("{name}"))
}

/// Keeps a property list's header (`tdsb`, `tdsn`) and its final `Group End`.
fn emptied(c: &Chunk<'static>) -> Chunk<'static> {
    let mut c = c.clone();
    if let effectcraft_aep::Body::List { children, .. } = &mut c.body {
        let first = children.iter().position(|k| k.is(b"tdmn")).unwrap_or(children.len());
        let end = children.last().cloned();
        children.truncate(first);
        children.extend(end);
    }
    c
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let dir = std::path::Path::new(&a[1]);
    let point = text_block(&dir.join("110-text-A.aep"));
    let boxed = text_block(&dir.join("245-text-box-400x200-Hello.aep"));
    let layer = |name: &str| Chunk::list(*b"tmpl", text_block(&dir.join(name)));
    let l579 = layer("579-textanim-position-0-m50.aep");
    let group = below(&l579, "ADBE Text Animators", 0);
    let tanm = vec![group[0].clone(), emptied(&group[1])];
    // The first animator with its Position record removed (its property list emptied).
    let mut animator = below(&l579, "ADBE Text Animator", 0);
    if let effectcraft_aep::Body::List { children, .. } = &mut animator[1].body {
        let at = children.iter().position(|k| k.is(b"tdmn") && tdmn_name(k).as_deref() == Some("ADBE Text Animator Properties")).expect("properties");
        children[at + 1] = emptied(&children[at + 1]);
    }
    let mut records = below(&l579, "ADBE Text Position 3D", 0);
    let l592 = layer("592-textanim-many-props.aep");
    for n in [
        "ADBE Text Anchor Point 3D",
        "ADBE Text Scale 3D",
        "ADBE Text Skew",
        "ADBE Text Skew Axis",
        "ADBE Text Rotation",
        "ADBE Text Opacity",
        "ADBE Text Fill Color",
        "ADBE Text Stroke Color",
        "ADBE Text Stroke Width",
        "ADBE Text Tracking Amount",
        "ADBE Text Line Spacing",
        "ADBE Text Blur",
    ] {
        records.extend(below(&l592, n, 0));
    }
    for (case, names) in [
        ("580-textanim-start-25.aep", &["ADBE Text Percent Start"][..]),
        ("581-textanim-end-75.aep", &["ADBE Text Percent End"]),
        ("582-textanim-offset-10.aep", &["ADBE Text Percent Offset"]),
        ("583-textanim-units-index.aep", &["ADBE Text Range Units", "ADBE Text Index Start", "ADBE Text Index End", "ADBE Text Index Offset"]),
        ("584-textanim-basedon-3-words.aep", &["ADBE Text Range Type2"]),
        ("585-textanim-mode-2-subtract.aep", &["ADBE Text Selector Mode"]),
        ("586-textanim-amount-50.aep", &["ADBE Text Selector Max Amount"]),
        ("587-textanim-shape-2-rampup.aep", &["ADBE Text Range Shape"]),
        ("588-textanim-smoothness-50.aep", &["ADBE Text Selector Smoothness"]),
        ("589-textanim-easehigh-30.aep", &["ADBE Text Levels Max Ease"]),
        ("590-textanim-easelow-40.aep", &["ADBE Text Levels Min Ease"]),
        ("591-textanim-randomize-seed-7.aep", &["ADBE Text Randomize Order", "ADBE Text Random Seed"]),
        ("728-textanim-character-offset-3.aep", &["ADBE Text Character Offset"]),
    ] {
        let l = layer(case);
        for n in names {
            records.extend(below(&l, n, 0));
        }
    }
    let out = AepFile {
        chunks: vec![
            Chunk::list(*b"txpt", point),
            Chunk::list(*b"txbx", boxed),
            Chunk::list(*b"tanm", tanm),
            Chunk::list(*b"tani", animator),
            Chunk::list(*b"tarc", records),
        ],
        trailer: Cow::Owned(vec![]),
    };
    std::fs::write(&a[2], write(&out).expect("write")).expect("write file");
}
