//! The flat tree index the importer reads ([`TreeIndex`]) against the owned chunk tree, on every
//! file of the fixture directory: the same tree, and the same answers from the typed view.

use std::fmt::Write as _;

use effectcraft_aep::container::index::TreeIndex;
use effectcraft_aep::{
    AepFile, Chunk, CryptomatteData, CurvesData, Effect, HueSaturationData, LevelsData, Limits, MeshData, Project, PropGroup, PropNode, is_aepx, parse,
    parse_with_limits, read_aepx, write,
};

/// Every `.aep` and `.aepx` in the fixture directory (the real projects included), sorted.
fn every_file() -> Vec<std::path::PathBuf> {
    let Some(dir) = crate::common::dir() else {
        crate::common::skipped("*.aep");
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        crate::common::skipped("*.aep");
        return Vec::new();
    };
    let mut out: Vec<std::path::PathBuf> =
        entries.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "aep" || x == "aepx")).collect();
    out.sort();
    out
}

/// What the typed view reads from a project, as text: items, compositions, layers with their
/// headers, transforms, masks, effects with every parameter value, text, markers, the whole
/// property tree with static values and key records, settings and versions.
fn dump(view: &Project<'_, '_>) -> String {
    let mut s = String::new();
    let defs = view.effect_definition_index();
    for item in view.all_items() {
        let _ = writeln!(s, "item {:?} {:?} {:?} {:?} {:?}", item.kind(), item.name(), item.display_name(), item.id(), item.label());
        let _ = writeln!(s, "  footage {:?} {:?} {:?}", item.footage(), item.sequence_info(), item.conform_frame_rate());
        let _ = writeln!(s, "  folder {} {}", item.has_folder_contents(), item.folder_contents_unreadable());
        let Some(comp) = item.comp() else { continue };
        let _ = writeln!(s, "  comp {:?} {:?} {:?}", comp.time_base(), comp.header(), comp.markers());
        for layer in comp.layers() {
            let _ = writeln!(s, "  layer {:?} {:?} {:?} {:?}", layer.name(), layer.id(), layer.header(), layer.track_matte());
            let _ = writeln!(s, "    transform {:?}", layer.transform(1920, 1080));
            let _ = writeln!(s, "    time remap {:?} orientation {:?} {:?}", layer.time_remap(), layer.orientation(), layer.orientation_keys());
            let _ = writeln!(s, "    masks {:?}", layer.mask_atoms().iter().map(|m| (&m.mask, &m.path_shapes)).collect::<Vec<_>>());
            let _ = writeln!(s, "    shapes {:?}", layer.shape_groups());
            let _ =
                writeln!(s, "    text {:?} {} {:?}", layer.text_document(), layer.text_document_count(), layer.source_text_property().map(|p| p.is_animated()));
            let _ = writeln!(s, "    markers {:?} source {:?}", layer.markers(), view.layer_source(&layer).map(|i| i.id()));
            for fx in layer.effects() {
                let _ =
                    writeln!(s, "    fx {} {:?} {:?} {:?} {:?}", fx.match_name(), fx.display_name(), fx.enabled(), defs.source(&fx), fx.value_record_names());
                let _ = writeln!(s, "      names {:?} {:?} {:?} {:?}", fx.name(), fx.user_name(), fx.declared_param_count(), fx.has_own_definitions());
                for p in defs.params(&fx) {
                    let _ = writeln!(s, "      {} {:?} {:?} {:?} {:?}", p.match_name, p.kind, p.name, p.items, p.value());
                    arbitrary(&fx, &p.match_name, &mut s);
                }
                for name in fx.value_record_names() {
                    arbitrary(&fx, &name, &mut s);
                }
            }
            if let Some(root) = layer.properties() {
                tree(&root, 0, &mut s);
            }
        }
    }
    s
}

/// The arbitrary data stored for a parameter and what each decoder reads from it.
fn arbitrary(fx: &Effect<'_, '_>, name: &str, s: &mut String) {
    let Some(data) = fx.arbitrary_data(name) else { return };
    let _ = writeln!(s, "        arbitrary {name} {data:?}");
    let _ = writeln!(s, "        {:?} {:?}", HueSaturationData::parse(data), LevelsData::parse(data));
    let _ = writeln!(s, "        {:?} {:?} {:?}", CurvesData::parse(data), MeshData::parse(data), CryptomatteData::parse(data));
}

/// Appends a group and everything below it to the dump, depth-limited.
fn tree(group: &PropGroup<'_, '_>, depth: usize, s: &mut String) {
    if depth > 32 {
        return;
    }
    let _ = writeln!(s, "{:depth$}group {:?} {:?}", "", group.user_name(), group.flags());
    for e in group.entries() {
        match e.node {
            PropNode::Group(g) => tree(&g, depth + 1, s),
            PropNode::Property(p) => {
                let keys: Option<Vec<Vec<u8>>> = p.keys().map(|k| k.records().map(<[u8]>::to_vec).collect());
                let _ = writeln!(s, "{:depth$} {} {:?} {:?} {:?} {:?}", "", e.match_name, p.user_name(), p.header(), p.static_value(), keys);
            }
            PropNode::Other(c) => {
                let _ = writeln!(s, "{:depth$} {} other {:?} {}", "", e.match_name, c.list_type(), c.descendants().count());
            }
        }
    }
}

/// The index is the parsed tree and the view reads the same on every file.
#[test]
fn the_index_is_the_parsed_tree_and_the_view_reads_the_same_on_every_file() {
    for path in every_file() {
        let name = path.display().to_string();
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
        // `.aepx` is read into the chunk tree and indexed from it; `.aep` is indexed in place.
        // A file that does not read (a damaged sample) must fail the index the same way.
        let file: AepFile<'_> = match if is_aepx(&bytes) { read_aepx(&bytes) } else { parse(&bytes) } {
            Ok(file) => file,
            Err(e) => {
                if !is_aepx(&bytes) {
                    assert_eq!(TreeIndex::parse(&bytes).err(), Some(e), "{name}");
                }
                continue;
            }
        };
        let index = (if is_aepx(&bytes) { TreeIndex::from_file(&file) } else { TreeIndex::parse(&bytes) }).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(index.to_file() == file, "{name}: the index is not the parsed tree");
        assert_eq!(index.len(), file.walk().count(), "{name}");
        assert_eq!(index.file_version(), file.file_version(), "{name}");
        assert_eq!(index.saved_by_version(), file.saved_by_version(), "{name}");
        assert_eq!(effectcraft_aep::ProjectSettings::read_index(&index), effectcraft_aep::ProjectSettings::read(&file), "{name}");
        let (Some(a), Some(b)) = (Project::new(&file), Project::from_index(&index)) else {
            assert!(Project::new(&file).is_none() && Project::from_index(&index).is_none(), "{name}");
            continue;
        };
        let (da, db) = (dump(&a), dump(&b));
        assert!(da == db, "{name}: the view reads differently through the index");
    }
}

/// The index and the parser fail alike under tight limits.
#[test]
fn the_index_and_the_parser_fail_alike_under_tight_limits() {
    for path in every_file().into_iter().filter(|p| p.extension().is_some_and(|x| x == "aep")) {
        let name = path.display().to_string();
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
        let total = parse(&bytes).map(|f| f.walk().count()).unwrap_or(0);
        for limits in [
            Limits { max_depth: 2, max_chunks: 10_000_000 },
            Limits { max_depth: 6, max_chunks: 10_000_000 },
            Limits { max_depth: 64, max_chunks: total },
            Limits { max_depth: 64, max_chunks: total.saturating_sub(1) },
            Limits { max_depth: 64, max_chunks: total / 2 },
        ] {
            let old = parse_with_limits(&bytes, limits);
            let new = TreeIndex::parse_with_limits(&bytes, limits);
            match (old, new) {
                (Ok(f), Ok(i)) => assert!(i.to_file() == f, "{name}: {limits:?}"),
                (Err(a), Err(b)) => assert_eq!(a, b, "{name}: {limits:?}"),
                (a, b) => panic!("{name}: {limits:?}: {:?} vs {:?}", a.err(), b.err()),
            }
        }
    }
}

/// A 40-byte NUL-padded match name.
fn tdmn(name: &str) -> Chunk<'static> {
    let mut b = name.as_bytes().to_vec();
    b.resize(40, 0);
    Chunk::data(*b"tdmn", b)
}

/// A nested chunk `id` holding one `Utf8` with `text`.
fn utf8(id: [u8; 4], text: &str) -> Chunk<'static> {
    Chunk { id, body: effectcraft_aep::Body::Chunks(vec![Chunk::data(*b"Utf8", text.as_bytes().to_vec())]), pad: None }
}

/// A project with one composition, one layer and one effect whose five parameters carry arbitrary
/// data in the sizes the decoders read (Hue/Saturation, Levels, Curves, a 2 x 2 mesh, Cryptomatte),
/// so the arbitrary-data path runs without the fixture directory.
fn synthetic() -> Vec<u8> {
    let mut mesh = vec![0u8; 180];
    mesh[..8].copy_from_slice(&[0, 0, 0, 2, 0, 0, 0, 2]);
    let mut cry = vec![0u8; 200];
    cry[..4].copy_from_slice(b"cry1");
    cry[32..37].copy_from_slice(b"Layer");
    let mut levels = vec![0u8; 108];
    levels[8..16].copy_from_slice(&1.0f64.to_be_bytes());
    let payloads: [Vec<u8>; 5] = [vec![7u8; 180], levels, vec![0u8; CurvesData::SIZE], mesh, cry];
    let (mut part, mut values) =
        (vec![Chunk::data(*b"parn", 6u32.to_be_bytes().to_vec())], vec![Chunk::data(*b"tdsb", 1u32.to_be_bytes().to_vec()), utf8(*b"tdsn", "Mine")]);
    for (i, data) in payloads.into_iter().enumerate() {
        let name = format!("FX-{:04}", i + 1);
        let mut pard = vec![0u8; 148];
        pard[12..16].copy_from_slice(&11u32.to_be_bytes());
        part.extend([tdmn(&name), Chunk::data(*b"pard", pard)]);
        let record = Chunk::list(*b"tdbs", vec![Chunk::data(*b"tdsb", 1u32.to_be_bytes().to_vec()), Chunk::data(*b"cdat", vec![0, 0, 0, 1])]);
        values.extend([tdmn(&name), record, Chunk::list(*b"aRbs", vec![Chunk::data(*b"aRbp", data)])]);
    }
    values.push(tdmn("ADBE Group End"));
    let sspc = Chunk::list(*b"sspc", vec![utf8(*b"fnam", "Effect"), Chunk::list(*b"parT", part), Chunk::list(*b"tdgp", values)]);
    let parade = Chunk::list(*b"tdgp", vec![tdmn("FX"), sspc, tdmn("ADBE Group End")]);
    let root = Chunk::list(*b"tdgp", vec![tdmn("ADBE Effect Parade"), parade, tdmn("ADBE Group End")]);
    let layer = Chunk::list(*b"Layr", vec![Chunk::data(*b"ldta", vec![0u8; 160]), Chunk::data(*b"Utf8", b"Layer".to_vec()), root]);
    let mut idta = vec![0u8; 64];
    idta[..2].copy_from_slice(&4u16.to_be_bytes());
    idta[16..20].copy_from_slice(&2u32.to_be_bytes());
    let mut cdta = vec![0u8; 200];
    cdta[4..12].copy_from_slice(&[0, 0, 0, 1, 0, 0, 0, 30]);
    let item = Chunk::list(*b"Item", vec![Chunk::data(*b"idta", idta), Chunk::data(*b"Utf8", b"Comp".to_vec()), Chunk::data(*b"cdta", cdta), layer]);
    let file = AepFile { chunks: vec![Chunk::data(*b"head", vec![0u8; 20]), Chunk::list(*b"Fold", vec![item])], trailer: std::borrow::Cow::Borrowed(&[]) };
    write(&file).expect("write")
}

/// The view reads arbitrary effect data the same through the index.
#[test]
fn the_view_reads_arbitrary_effect_data_the_same_through_the_index() {
    let bytes = synthetic();
    let file = parse(&bytes).expect("parse");
    let index = TreeIndex::parse(&bytes).expect("index");
    let (a, b) = (Project::new(&file).expect("project"), Project::from_index(&index).expect("project"));
    let (da, db) = (dump(&a), dump(&b));
    assert_eq!(da, db);
    // The synthetic effect really reaches every decoder (the dump is not empty of them).
    assert_eq!(da.matches("arbitrary FX-").count(), 10, "{da}");
    assert!(da.contains("Some(HueSaturationData") && da.contains("Some(LevelsData") && da.contains("Some(CurvesData"), "{da}");
    assert!(da.contains("Some(MeshData") && da.contains("Some(CryptomatteData"), "{da}");
    assert!(da.contains("Some(\"Mine\")"), "the user name is read: {da}");
}
