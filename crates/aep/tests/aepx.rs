//! `.aepx` (the XML form) read into the chunk tree, compared with the `.aep` of the same case.
//!
//! Each pair was saved by After Effects from the same state, the `.aepx` right after the `.aep`.
//! The trees are identical except for two documented differences: the save counter (`head`
//! `u32` at byte 16) is one higher in the `.aepx`, and pad bytes are not written in XML (an odd
//! body gets a zero pad byte). The XMP packet of the fixtures is the base packet in both forms.

mod common;

use common::fixture;
use effectcraft_aep::{AepError, AepFile, Body, Chunk, Limits, Project, is_aepx, parse, parse_any, read_aepx, read_aepx_with_limits, write};

/// The `.aepx` fixtures; each has the `.aep` of the same name.
const PAIRS: &[&str] = &["000-base", "058-key-opacity-2key", "060-expr-opacity-wiggle", "110-text-A", "124-precomp-Pre"];

/// The `.aep` tree with the differences an `.aepx` of the next save has removed: the `head` save
/// counter is taken from `other`.
fn with_head_of(mut file: AepFile<'static>, other: &AepFile<'_>) -> AepFile<'static> {
    if let (Some(h), Some(src)) = (file.chunks.iter_mut().find(|c| c.is(b"head")), other.chunk(b"head")) {
        *h = src.clone().into_owned();
    }
    file
}

/// The first difference of two chunk sequences, as a path, ignoring pad bytes.
fn first_difference(a: &[Chunk<'_>], b: &[Chunk<'_>], path: &str) -> Option<String> {
    if a.len() != b.len() {
        return Some(format!("{path}: {} chunks against {}", a.len(), b.len()));
    }
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        let name = String::from_utf8_lossy(&x.list_type().unwrap_or(x.id)).into_owned();
        let here = format!("{path}/{i}:{name}");
        if x.id != y.id || x.list_type() != y.list_type() {
            return Some(format!("{here}: id differs"));
        }
        match (&x.body, &y.body) {
            (Body::Data(p), Body::Data(q)) | (Body::OpaqueList { data: p, .. }, Body::OpaqueList { data: q, .. }) => {
                if p != q {
                    return Some(format!("{here}: body differs"));
                }
            }
            (Body::Chunks(p), Body::Chunks(q)) | (Body::List { children: p, .. }, Body::List { children: q, .. }) => {
                if let Some(d) = first_difference(p, q, &here) {
                    return Some(d);
                }
            }
            _ => return Some(format!("{here}: body kind differs")),
        }
    }
    None
}

fn head_counter(file: &AepFile<'_>) -> u32 {
    let h = file.chunk(b"head").and_then(Chunk::bytes).expect("head");
    u32::from_be_bytes([h[16], h[17], h[18], h[19]])
}

#[test]
fn every_pair_gives_the_same_chunk_tree() {
    for name in PAIRS {
        let aep = fixture(&format!("{name}.aep"));
        let aepx = fixture(&format!("{name}.aepx"));
        assert!(is_aepx(&aepx) && !is_aepx(&aep), "{name}");
        let bin = parse(&aep).unwrap();
        let xml = read_aepx(&aepx).unwrap_or_else(|e| panic!("{name}: {e}"));
        // The save counter is the only difference in the container.
        assert_eq!(head_counter(&xml), head_counter(&bin) + 1, "{name}");
        let same = with_head_of(bin.clone().into_owned(), &xml);
        assert_eq!(first_difference(&same.chunks, &xml.chunks, ""), None, "{name}");
        assert_eq!(xml.trailer, bin.trailer, "{name}: XMP packet");
        // Even the bytes agree: written as an `.aep`, the tree is the `.aep` with the next save counter.
        assert!(write(&xml).unwrap() == write(&same).unwrap(), "{name}: written bytes differ");
        // And the content sniffer picks the right reader for both.
        assert_eq!(parse_any(&aepx).unwrap(), xml, "{name}");
        assert_eq!(parse_any(&aep).unwrap(), bin, "{name}");
    }
}

#[test]
fn the_typed_view_reads_the_same_values() {
    for name in PAIRS {
        let aep = fixture(&format!("{name}.aep"));
        let aepx = fixture(&format!("{name}.aepx"));
        let bin = parse(&aep).unwrap();
        let xml = read_aepx(&aepx).unwrap();
        let (pb, px) = (Project::new(&bin).unwrap(), Project::new(&xml).unwrap());
        let items = |p: &Project<'_, '_>| p.all_items().iter().map(|i| (i.kind(), i.display_name(), i.id())).collect::<Vec<_>>();
        assert_eq!(items(&pb), items(&px), "{name}");
        for (cb, cx) in pb.all_items().iter().filter_map(|i| i.comp()).zip(px.all_items().iter().filter_map(|i| i.comp())) {
            assert_eq!(cb.header(), cx.header(), "{name}");
            let layers = |c: &effectcraft_aep::Comp<'_, '_>| c.layers().map(|l| (l.name(), l.header(), l.transform(1920, 1080))).collect::<Vec<_>>();
            assert_eq!(layers(&cb), layers(&cx), "{name}");
        }
    }
    // Spot checks against the values the cases were made with.
    let aepx = fixture("060-expr-opacity-wiggle.aepx");
    let xml = read_aepx(&aepx).unwrap();
    assert!(xml.walk().any(|(_, c)| c.is(b"Utf8") && c.bytes() == Some(&b"wiggle(1,10)"[..])));
    let aepx = fixture("110-text-A.aepx");
    let xml = read_aepx(&aepx).unwrap();
    assert!(xml.walk().any(|(_, c)| c.is_list(b"btdk") && matches!(c.body, Body::OpaqueList { .. })));
}

#[test]
fn damaged_aepx_is_an_error_or_a_tree_never_a_panic() {
    let aepx = fixture("000-base.aepx");
    // Truncation at many points.
    for cut in (0..aepx.len()).step_by(997) {
        let _ = read_aepx(&aepx[..cut]);
    }
    assert!(read_aepx(&aepx[..aepx.len() / 2]).is_err());
    // Byte flips: each result is an error or a tree that writes and reads back.
    let mut state = 0x2545_f491_u32;
    for _ in 0..400 {
        let mut damaged = aepx.clone();
        for _ in 0..4 {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let at = state as usize % damaged.len();
            damaged[at] ^= 1 << (state % 8);
        }
        if let Ok(file) = read_aepx(&damaged) {
            let bytes = write(&file).unwrap();
            let _ = Project::new(&parse(&bytes).unwrap()).map(|p| p.all_items().len());
        }
    }
    // Invalid hex and a damaged entity are errors.
    let text = String::from_utf8(aepx.clone()).unwrap();
    let odd = text.replacen("bdata=\"00", "bdata=\"0", 1);
    assert!(matches!(read_aepx(odd.as_bytes()), Err(AepError::Xml { .. })));
    let nonhex = text.replacen("bdata=\"00", "bdata=\"0g", 1);
    assert!(matches!(read_aepx(nonhex.as_bytes()), Err(AepError::Xml { .. })));
    // Deep nesting is refused at the limit; many elements are bounded.
    let deep = format!("<AfterEffectsProject>{}</AfterEffectsProject>", "<Fold>".repeat(1_000_000));
    assert_eq!(read_aepx(deep.as_bytes()), Err(AepError::TooDeep));
    assert_eq!(read_aepx_with_limits(&aepx, Limits { max_depth: 64, max_chunks: 100 }), Err(AepError::TooManyChunks));
    assert_eq!(read_aepx_with_limits(&aepx, Limits { max_depth: 3, max_chunks: 10_000_000 }), Err(AepError::TooDeep));
    // A huge hex body is bounded by the input: nothing is allocated from a declared size.
    let huge = format!("<AfterEffectsProject><cdat bdata=\"{}\"/></AfterEffectsProject>", "ab".repeat(1 << 20));
    assert_eq!(read_aepx(huge.as_bytes()).unwrap().chunks[0].bytes().map(<[u8]>::len), Some(1 << 20));
}

/// Every pair of the local corpus (not in the repository): `AEP_CORPUS=<folder> cargo test
/// --release -p effectcraft-aep --test aepx -- --ignored --nocapture`.
#[test]
#[ignore = "needs the local corpus (AEP_CORPUS)"]
fn every_corpus_pair_gives_the_same_chunk_tree() {
    let Ok(dir) = std::env::var("AEP_CORPUS") else { return };
    let mut names: Vec<_> =
        std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "aepx")).collect();
    names.sort();
    let (mut same, mut differ) = (0, Vec::new());
    for x in &names {
        let aepx = std::fs::read(x).unwrap();
        let aep = std::fs::read(x.with_extension("aep")).unwrap();
        let bin = parse(&aep).unwrap();
        let xml = match read_aepx(&aepx) {
            Ok(f) => f,
            Err(e) => {
                differ.push(format!("{}: {e}", x.display()));
                continue;
            }
        };
        let fixed = with_head_of(bin.clone().into_owned(), &xml);
        match first_difference(&fixed.chunks, &xml.chunks, "") {
            None if write(&xml).unwrap() == write(&AepFile { trailer: xml.trailer.clone(), ..fixed }).unwrap() => same += 1,
            None => differ.push(format!("{}: bytes differ", x.display())),
            Some(d) => differ.push(format!("{}: {d}", x.display())),
        }
    }
    println!("{same} of {} pairs give the same chunk tree", names.len());
    for d in &differ {
        println!("{d}");
    }
}
