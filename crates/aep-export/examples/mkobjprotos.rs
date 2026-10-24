//! Developer tool: extracts the object prototypes the exporter clones from fixtures of
//! `crates/aep/tests/fixtures` (our own generated corpus):
//!
//! - `mrkp`: the marker group of `249-marker-layer-full` (`tdmn ADBE Marker` and its `LIST mrst`
//!   with one marker holding all five strings), used for layer and composition markers;
//! - `camb`, `spot`, `pont`: the layer blocks (`Layr`, `Ewst`, f-records) of the default two-node
//!   camera of `256-camera-two-node-default`, the spot light of `254-light-spot-intensity-50` and the
//!   point light of `252-light-point`;
//! - `opts`: `tdmn` + `tdbs` pairs of the camera and light options After Effects wrote when one
//!   was changed: Depth of Field, Focus Distance, Aperture (342), Intensity (254), Color (343),
//!   Cone Angle (344), Cone Feather (345);
//! - `shpb`, `shpi`, `shpr`: the shape layer block of synthetic case 519 with empty contents, the
//!   item nodes at their defaults (519: group with empty contents, rectangle, ellipse, polystar,
//!   fill, stroke, gradient fill and stroke, trim paths, round corners; 731: merge paths, offset
//!   paths, repeater; 240: a path) and the value records of the one-change cases 451-471, 239 and
//!   242 (synthetic cases of the corpus folder, our own generated files).
//!
//! `cargo run -p effectcraft-aep-export --example mkobjprotos -- crates/aep/tests/fixtures <corpus synthetic dir> crates/aep-export/template/objects.aep`
use effectcraft_aep::{AepFile, Chunk, parse, write};
use std::borrow::Cow;

fn load(dir: &std::path::Path, name: &str) -> AepFile<'static> {
    let bytes = std::fs::read(dir.join(name)).expect("read fixture");
    parse(&bytes).expect("parse").into_owned()
}

/// The children of the first layer's top property group.
fn first_layer_group(f: &AepFile<'static>) -> Vec<Chunk<'static>> {
    let fold = f.list(b"Fold").expect("Fold");
    let comp = fold.children().iter().find(|c| c.is_list(b"Item") && c.child(b"cdta").is_some()).expect("comp");
    let layer = comp.children().iter().find(|c| c.is_list(b"Layr")).expect("layer");
    layer.child_list(b"tdgp").expect("tdgp").children().to_vec()
}

/// The block of the first layer of type `ty` (`ldta@128`) of the first composition.
fn block(f: &AepFile<'static>, ty: u8) -> Vec<Chunk<'static>> {
    let fold = f.list(b"Fold").expect("Fold");
    let comp = fold.children().iter().find(|c| c.is_list(b"Item") && c.child(b"cdta").is_some()).expect("comp");
    let kids = comp.children();
    let is = |c: &Chunk<'_>| c.is_list(b"Layr") && c.child(b"ldta").and_then(|d| d.bytes()).is_some_and(|b| b[128..132] == [0, 0, 0, ty]);
    let start = kids.iter().position(is).expect("layer");
    let end = kids.iter().skip(start + 1).position(|c| c.is_list(b"Layr") || c.is_list(b"DLay")).map(|i| start + 1 + i).expect("end");
    kids[start..end].to_vec()
}

/// `tdmn name` and the node after it, searched in the first layer of type `ty`.
fn record(f: &AepFile<'static>, ty: u8, name: &str) -> Vec<Chunk<'static>> {
    fn walk(kids: &[Chunk<'static>], name: &str) -> Option<Vec<Chunk<'static>>> {
        for (i, c) in kids.iter().enumerate() {
            if c.is(b"tdmn") && c.bytes().is_some_and(|b| b.starts_with(name.as_bytes()) && b.get(name.len()) == Some(&0)) {
                return Some(vec![c.clone(), kids.get(i + 1)?.clone()]);
            }
            if let Some(r) = walk(c.children(), name) {
                return Some(r);
            }
        }
        None
    }
    walk(&block(f, ty), name).unwrap_or_else(|| panic!("record {name}"))
}

fn find_case(dir: &std::path::Path, prefix: &str) -> AepFile<'static> {
    let name = std::fs::read_dir(dir)
        .expect("dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .find(|n| n.starts_with(prefix) && n.ends_with(".aep"))
        .unwrap_or_else(|| panic!("case {prefix}"));
    load(dir, &name)
}

/// The shape root (`ADBE Root Vectors Group` list) of the first shape layer (type 4).
fn shape_root(f: &AepFile<'static>) -> Chunk<'static> {
    let b = block(f, 4);
    let g = b[0].child_list(b"tdgp").expect("tdgp").children().to_vec();
    let at = g.iter().position(|c| c.is(b"tdmn") && c.bytes().is_some_and(|x| x.starts_with(b"ADBE Root Vectors Group\0"))).expect("root");
    g[at + 1].clone()
}

/// `tdmn name` + node anywhere below `c`.
fn below(c: &Chunk<'static>, name: &str) -> Vec<Chunk<'static>> {
    fn walk(kids: &[Chunk<'static>], name: &str) -> Option<Vec<Chunk<'static>>> {
        for (i, c) in kids.iter().enumerate() {
            if c.is(b"tdmn") && c.bytes().is_some_and(|b| b.starts_with(name.as_bytes()) && b.get(name.len()) == Some(&0)) {
                return Some(vec![c.clone(), kids.get(i + 1)?.clone()]);
            }
            if let Some(r) = walk(c.children(), name) {
                return Some(r);
            }
        }
        None
    }
    walk(c.children(), name).unwrap_or_else(|| panic!("{name}"))
}

/// Keeps the header (`tdsb`, `tdsn`) and the final `Group End` of a property list.
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
    let syn = std::path::Path::new(&a[2]);
    let g = first_layer_group(&load(dir, "249-marker-layer-full.aep"));
    let at = g.iter().position(|c| c.is(b"tdmn") && c.bytes().is_some_and(|b| b.starts_with(b"ADBE Marker\0"))).expect("marker group");
    let marker = vec![g[at].clone(), g[at + 1].clone()];
    assert!(marker[1].is_list(b"mrst"));
    let camera = load(dir, "256-camera-two-node-default.aep");
    let spot = load(dir, "254-light-spot-intensity-50.aep");
    let point = load(dir, "252-light-point.aep");
    let mut opts = Vec::new();
    let dof = load(dir, "342-camera-dof-800-30.aep");
    for n in ["ADBE Camera Depth of Field", "ADBE Camera Focus Distance", "ADBE Camera Aperture"] {
        opts.extend(record(&dof, 2, n));
    }
    opts.extend(record(&spot, 1, "ADBE Light Intensity"));
    opts.extend(record(&load(dir, "343-light-spot-color-red.aep"), 1, "ADBE Light Color"));
    opts.extend(record(&load(dir, "344-light-spot-coneangle-60.aep"), 1, "ADBE Light Cone Angle"));
    opts.extend(record(&load(dir, "345-light-spot-conefeather-30.aep"), 1, "ADBE Light Cone Feather 2"));
    // Shapes.
    let all = find_case(syn, "519-");
    let mut shape_block = block(&all, 4);
    {
        let layr = &mut shape_block[0];
        let effectcraft_aep::Body::List { children, .. } = &mut layr.body else { panic!("layer") };
        let tdgp = children.iter_mut().find(|c| c.is_list(b"tdgp")).expect("tdgp");
        let effectcraft_aep::Body::List { children: g, .. } = &mut tdgp.body else { panic!("tdgp") };
        let at = g.iter().position(|c| c.is(b"tdmn") && c.bytes().is_some_and(|x| x.starts_with(b"ADBE Root Vectors Group\0"))).expect("root");
        g[at + 1] = emptied(&g[at + 1]);
    }
    let root = shape_root(&all);
    let mut shape_items = Vec::new();
    {
        let mut group = below(&root, "ADBE Vector Group");
        let effectcraft_aep::Body::List { children: gk, .. } = &mut group[1].body else { panic!("group") };
        let at = gk.iter().position(|c| c.is(b"tdmn") && c.bytes().is_some_and(|x| x.starts_with(b"ADBE Vectors Group\0"))).expect("contents");
        gk[at + 1] = emptied(&gk[at + 1]);
        shape_items.extend(group);
    }
    for n in [
        "ADBE Vector Shape - Rect",
        "ADBE Vector Shape - Ellipse",
        "ADBE Vector Shape - Star",
        "ADBE Vector Graphic - Fill",
        "ADBE Vector Graphic - Stroke",
        "ADBE Vector Graphic - G-Fill",
        "ADBE Vector Graphic - G-Stroke",
        "ADBE Vector Filter - Trim",
        "ADBE Vector Filter - RC",
    ] {
        shape_items.extend(below(&root, n));
    }
    // Merge Paths, Offset Paths and Repeater at their defaults (731).
    let ops = shape_root(&find_case(syn, "731-"));
    for n in ["ADBE Vector Filter - Merge", "ADBE Vector Filter - Offset", "ADBE Vector Filter - Repeater"] {
        shape_items.extend(below(&ops, n));
    }
    shape_items.extend(below(&shape_root(&load(dir, "240-shape-custom-path-tangents.aep")), "ADBE Vector Shape - Group"));
    let mut shape_records = Vec::new();
    for (case, from_fixture, n) in [
        ("451-", false, "ADBE Vector Ellipse Size"),
        ("451-", false, "ADBE Vector Fill Color"),
        ("453-", false, "ADBE Vector Ellipse Position"),
        ("454-", false, "ADBE Vector Fill Opacity"),
        ("455-", false, "ADBE Vector Fill Rule"),
        ("456-", false, "ADBE Vector Anchor"),
        ("457-", false, "ADBE Vector Scale"),
        ("458-", false, "ADBE Vector Rotation"),
        ("459-", false, "ADBE Vector Group Opacity"),
        ("460-", false, "ADBE Vector Skew"),
        ("461-", false, "ADBE Vector Skew Axis"),
        ("463-", false, "ADBE Vector Stroke Color"),
        ("463-", false, "ADBE Vector Stroke Width"),
        ("464-", false, "ADBE Vector Stroke Opacity"),
        ("470-", false, "ADBE Vector Rect Size"),
        ("471-", false, "ADBE Vector Rect Roundness"),
        ("239-shape-stroke-green-w8-round.aep", true, "ADBE Vector Stroke Line Cap"),
        ("239-shape-stroke-green-w8-round.aep", true, "ADBE Vector Stroke Line Join"),
        ("239-shape-stroke-green-w8-round.aep", true, "ADBE Vector Stroke Miter Limit"),
        ("242-shape-group-position-50-50.aep", true, "ADBE Vector Position"),
    ] {
        let f = if from_fixture { load(dir, case) } else { find_case(syn, case) };
        shape_records.extend(below(&shape_root(&f), n));
    }
    let out = AepFile {
        chunks: vec![
            Chunk::list(*b"mrkp", marker),
            Chunk::list(*b"camb", block(&camera, 2)),
            Chunk::list(*b"spot", block(&spot, 1)),
            Chunk::list(*b"pont", block(&point, 1)),
            Chunk::list(*b"opts", opts),
            Chunk::list(*b"shpb", shape_block),
            Chunk::list(*b"shpi", shape_items),
            Chunk::list(*b"shpr", shape_records),
        ],
        trailer: Cow::Owned(vec![]),
    };
    let bytes = write(&out).expect("write");
    let low = String::from_utf8_lossy(&bytes).to_lowercase();
    for needle in ["\\users\\", "/users/", "/home/"] {
        assert!(!low.contains(needle), "local data left: {needle}");
    }
    std::fs::write(&a[3], bytes).expect("write file");
}
