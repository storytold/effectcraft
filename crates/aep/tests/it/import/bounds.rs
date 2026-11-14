//! Hostile inputs that used to cost quadratic time, and losses that used to be silent (#3299).
//!
//! The quadratic cases are built from the committed `tests/fixtures/synthetic.aep` (composition
//! `C`, id 2, with a sequence layer, id 17, and a solid layer, id 16): its composition item and
//! its solid layer are copied with new ids. The sizes here are moderate, so the tests check what
//! arrives; the bounds themselves are pinned by the unit tests next to the code
//! (`import::build`, `import::effects`).

use effectcraft_aep::import::{ImportOptions, ImportResult, LossCategory, import};
use effectcraft_aep::{AepFile, Body, Chunk, parse, write};
use effectcraft_project::{ItemKind, LayerId, LayerSource, Project};

const SYNTHETIC: &[u8] = include_bytes!("../../fixtures/synthetic.aep");
/// The item id of composition `C` and the layer id of its solid layer.
const COMP_C: u32 = 2;
const SOLID_LAYER: u32 = 16;

fn run(bytes: &[u8]) -> (Project, ImportResult) {
    let mut project = Project::default();
    let none = |_: &str| false;
    let r = import(&mut project, bytes, &ImportOptions { allow_losses: true, ..ImportOptions::new(&none) }).expect("imports");
    (project, r)
}

fn losses(r: &ImportResult, category: LossCategory) -> Vec<String> {
    r.report.losses.iter().filter(|l| l.category == category).flat_map(|l| l.details.clone()).collect()
}

fn set_be_u32(c: &mut Chunk<'_>, at: usize, v: u32) {
    if let Body::Data(d) = &mut c.body {
        d.to_mut()[at..at + 4].copy_from_slice(&v.to_be_bytes());
    }
}

fn be_u32(c: &Chunk<'_>, at: usize) -> u32 {
    u32::from_be_bytes(c.bytes().expect("data")[at..at + 4].try_into().unwrap())
}

fn children<'c, 'a>(c: &'c mut Chunk<'a>) -> &'c mut Vec<Chunk<'a>> {
    match &mut c.body {
        Body::List { children, .. } | Body::Chunks(children) => children,
        _ => panic!("not a list"),
    }
}

/// The project's folder list (`LIST Fold`, top level).
fn fold<'f, 'a>(file: &'f mut AepFile<'a>) -> &'f mut Vec<Chunk<'a>> {
    children(file.chunks.iter_mut().find(|c| c.is_list(b"Fold")).expect("Fold"))
}

/// The `Item` list of composition `C`.
fn comp_c<'a>(file: &AepFile<'a>) -> Chunk<'a> {
    let fold = file.list(b"Fold").expect("Fold");
    fold.children().iter().find(|c| c.is_list(b"Item") && c.child(b"idta").is_some_and(|i| be_u32(i, 16) == COMP_C)).expect("composition C").clone()
}

/// The solid layer of `C` (`LIST Layr` with id 16).
fn solid_layer<'a>(comp: &Chunk<'a>) -> Chunk<'a> {
    comp.children().iter().find(|c| c.is_list(b"Layr") && c.child(b"ldta").is_some_and(|l| be_u32(l, 0) == SOLID_LAYER)).expect("the solid layer").clone()
}

/// A copy of `layer` with layer id `id`; `edit` changes its `ldta`.
fn layer_copy<'a>(layer: &Chunk<'a>, id: u32, edit: &dyn Fn(&mut Chunk<'a>)) -> Chunk<'a> {
    let mut l = layer.clone();
    let ldta = children(&mut l).iter_mut().find(|c| c.is(b"ldta")).expect("ldta");
    set_be_u32(ldta, 0, id);
    edit(ldta);
    l
}

/// A composition like `C` with item id `id`, name `name` and only the layers `layers`.
fn comp_with<'a>(c: &Chunk<'a>, id: u32, name: &str, layers: Vec<Chunk<'a>>) -> Chunk<'a> {
    let mut item = c.clone();
    let kids = children(&mut item);
    kids.retain(|k| !k.is_list(b"Layr"));
    for k in kids.iter_mut() {
        if k.is(b"idta") {
            set_be_u32(k, 16, id);
        }
        if k.is(b"Utf8") {
            *k = Chunk::data(*b"Utf8", name.as_bytes().to_vec());
        }
    }
    let at = kids.iter().position(|k| k.is_list(b"Ewst")).unwrap_or(kids.len());
    kids.splice(at..at, layers);
    item
}

/// Composition `B` (id 900) with `n` layers of `C`, and `D` (id 901) with `n` layers of `B`,
/// after `C` in the file. With `c_uses_d`, `C` also gets a layer of `D`, which closes a loop.
fn precomp_chain(n: u32, c_uses_d: bool) -> Vec<u8> {
    precomp_chain_with(n, c_uses_d, false)
}

/// [`precomp_chain`]; with `null_back_edge` the layer of `D` in `C` is a null object (which shows
/// nothing, so no loop).
fn precomp_chain_with(n: u32, c_uses_d: bool, null_back_edge: bool) -> Vec<u8> {
    let mut file = parse(SYNTHETIC).unwrap();
    let c = comp_c(&file);
    let solid = solid_layer(&c);
    let layers = |first: u32, source: u32| -> Vec<Chunk<'_>> { (0..n).map(|i| layer_copy(&solid, first + i, &|l| set_be_u32(l, 40, source))).collect() };
    let b = comp_with(&c, 900, "B", layers(100_000, COMP_C));
    let d = comp_with(&c, 901, "D", layers(200_000, 900));
    let fold = fold(&mut file);
    let at = fold.iter().position(|k| k.is_list(b"Item") && k.child(b"idta").is_some_and(|i| be_u32(i, 16) == COMP_C)).expect("C") + 1;
    fold.splice(at..at, [b, d]);
    if c_uses_d {
        let c = fold.iter_mut().find(|k| k.is_list(b"Item") && k.child(b"idta").is_some_and(|i| be_u32(i, 16) == COMP_C)).expect("C");
        let kids = children(c);
        let at = kids.iter().position(|k| k.is_list(b"Layr")).expect("a layer");
        kids.insert(
            at,
            layer_copy(&solid, 300_000, &|l| {
                set_be_u32(l, 40, 901);
                if null_back_edge && let Body::Data(d) = &mut l.body {
                    d.to_mut()[38] |= 0x80;
                }
            }),
        );
    }
    write(&file).unwrap()
}

fn comp_named<'p>(p: &'p Project, name: &str) -> &'p effectcraft_project::Comp {
    p.items.values().find_map(|i| if let ItemKind::Comp(c) = &i.kind { (i.name == name).then_some(&**c) } else { None }).unwrap_or_else(|| panic!("no {name}"))
}

#[test]
fn many_layers_of_one_precomp_in_a_used_composition_import() {
    let n = 1500;
    let (p, r) = run(&precomp_chain(n, false));
    let (b, d) = (comp_named(&p, "B"), comp_named(&p, "D"));
    let item = |name: &str| p.items.values().find(|i| i.name == name).map(|i| i.id).expect("item");
    assert_eq!(b.layers.len(), n as usize);
    assert!(b.layers.iter().all(|l| l.source == LayerSource::Comp { item: item("C") }));
    assert!(d.layers.iter().all(|l| l.source == LayerSource::Comp { item: item("B") }));
    assert_eq!(d.layers.len(), n as usize);
    assert_eq!(r.report.imported.precomp_layers, 2 * n as usize);
    let found = losses(&r, LossCategory::UnsupportedLayers);
    assert!(!found.iter().any(|d| d.contains("loop")), "{found:?}");
}

#[test]
fn every_precomp_layer_that_closes_a_loop_is_refused() {
    // C (built first) uses D; B uses C; so D's layers of B would close C -> D -> B -> C.
    let n = 300;
    let (_, base) = run(SYNTHETIC);
    let (p, r) = run(&precomp_chain(n, true));
    assert_eq!(comp_named(&p, "B").layers.len(), n as usize);
    assert!(comp_named(&p, "D").layers.is_empty());
    let refused = r.report.count(LossCategory::UnsupportedLayers) - base.report.count(LossCategory::UnsupportedLayers);
    assert_eq!(refused, n as usize, "{:?}", r.report.losses);
    assert!(losses(&r, LossCategory::UnsupportedLayers).iter().any(|d| d.contains("precomposition loop")));
}

#[test]
fn a_null_layer_naming_a_composition_closes_no_loop() {
    // The same files as above, but C's layer of D is a null object: it shows nothing, so D's
    // layers of B are no loop and all of them are imported.
    let n = 300;
    let (p, r) = run(&precomp_chain_with(n, true, true));
    assert_eq!(comp_named(&p, "D").layers.len(), n as usize);
    assert!(comp_named(&p, "C").layers.iter().any(|l| l.source == LayerSource::Null));
    assert!(!losses(&r, LossCategory::UnsupportedLayers).iter().any(|d| d.contains("loop")), "{:?}", r.report.losses);
}

#[test]
fn many_parented_and_matted_layers_resolve() {
    // `n` copies of the solid layer in C, each parented to it and using it as an alpha matte.
    let n = 1500;
    let mut file = parse(SYNTHETIC).unwrap();
    let c = comp_c(&file);
    let solid = solid_layer(&c);
    let fold = fold(&mut file);
    let item = fold.iter_mut().find(|k| k.is_list(b"Item") && k.child(b"idta").is_some_and(|i| be_u32(i, 16) == COMP_C)).expect("C");
    let kids = children(item);
    let at = kids.iter().position(|k| k.is_list(b"Layr")).expect("a layer");
    let copies: Vec<Chunk<'_>> = (0..n)
        .map(|i| {
            layer_copy(&solid, 100_000 + i, &|l| {
                set_be_u32(l, 132, SOLID_LAYER);
                if let Body::Data(d) = &mut l.body {
                    d.to_mut()[107] = 1;
                }
                set_be_u32(l, 160, SOLID_LAYER);
            })
        })
        .collect();
    kids.splice(at..at, copies);
    let (p, r) = run(&write(&file).unwrap());
    let c = comp_named(&p, "C");
    // The original solid layer: the last layer whose source is the solid and that has no parent.
    let target: LayerId = c.layers.iter().rev().find(|l| l.parent.is_none() && matches!(l.source, LayerSource::Solid { .. })).map(|l| l.id).expect("the solid");
    let linked: Vec<_> = c.layers.iter().filter(|l| l.parent.is_some()).collect();
    assert_eq!(linked.len(), n as usize);
    assert!(linked.iter().all(|l| l.parent == Some(target) && l.track_matte.map(|m| m.layer) == Some(target)));
    assert_eq!(r.report.imported.track_mattes, n as usize);
    assert_eq!(r.report.count(LossCategory::ParentLinks), 0, "{:?}", r.report.losses);
}

/// `count` nested groups (`tdmn` + `LIST tdgp`), the innermost empty.
fn nested(count: usize) -> Vec<Chunk<'static>> {
    let end = || {
        let mut name = b"ADBE Group End".to_vec();
        name.resize(40, 0);
        Chunk::data(*b"tdmn", name)
    };
    let mut inner = vec![end()];
    for _ in 0..count {
        let mut name = b"ADBE Deep Group".to_vec();
        name.resize(40, 0);
        inner = vec![Chunk::data(*b"tdmn", name), Chunk::list(*b"tdgp", inner), end()];
    }
    inner
}

#[test]
fn blending_options_nested_past_the_read_depth_are_reported() {
    let mut file = parse(SYNTHETIC).unwrap();
    let mut patched = 0;
    // Every layer's Advanced Blending group gets groups nested twelve deep (no property in them).
    fn visit(chunks: &mut [Chunk<'_>], patched: &mut usize) {
        let mut next_is_adv = false;
        for c in chunks.iter_mut() {
            if next_is_adv && c.is_list(b"tdgp") {
                *children(c) = nested(12);
                *patched += 1;
            }
            next_is_adv = c.is(b"tdmn") && c.bytes().is_some_and(|b| b.starts_with(b"ADBE Adv Blend Group\0"));
            visit(c.children_mut(), patched);
        }
    }
    visit(&mut file.chunks, &mut patched);
    assert!(patched >= 2, "{patched}");
    let (_, r) = run(&write(&file).unwrap());
    let found = losses(&r, LossCategory::LayerStyles);
    assert!(found.iter().any(|d| d.contains("deeper than 8 levels")), "{found:?}");
    // Nested within the depth, an empty group still holds nothing.
    let mut file = parse(SYNTHETIC).unwrap();
    fn shallow(chunks: &mut [Chunk<'_>]) {
        let mut next_is_adv = false;
        for c in chunks.iter_mut() {
            if next_is_adv && c.is_list(b"tdgp") {
                *children(c) = nested(3);
            }
            next_is_adv = c.is(b"tdmn") && c.bytes().is_some_and(|b| b.starts_with(b"ADBE Adv Blend Group\0"));
            shallow(c.children_mut());
        }
    }
    shallow(&mut file.chunks);
    let (_, r) = run(&write(&file).unwrap());
    assert_eq!(losses(&r, LossCategory::LayerStyles), Vec::<String>::new());
}

/// Every key record of the property `match_name` (the `LIST list` of its `LIST tdbs`; a mask
/// shape's vertex lists are not touched) gets `patch`.
fn patch_keys(file: &mut AepFile<'_>, match_name: &[u8], patch: &dyn Fn(usize, &mut [u8])) -> usize {
    fn lists(c: &mut Chunk<'_>, patch: &dyn Fn(usize, &mut [u8]), n: &mut usize) {
        if c.is_list(b"tdbs") {
            for k in c.children_mut() {
                keys(k, patch, n);
            }
            return;
        }
        for k in c.children_mut() {
            lists(k, patch, n);
        }
    }
    fn keys(c: &mut Chunk<'_>, patch: &dyn Fn(usize, &mut [u8]), n: &mut usize) {
        if c.is_list(b"list") {
            let size = c.child(b"lhd3").and_then(|h| h.bytes()).map(|h| u32::from_be_bytes(h[16..20].try_into().unwrap()) as usize).unwrap_or(0);
            if let Some(Body::Data(d)) = children(c).iter_mut().find(|k| k.is(b"ldat")).map(|k| &mut k.body)
                && size > 0
            {
                for (i, rec) in d.to_mut().chunks_exact_mut(size).enumerate() {
                    patch(i, rec);
                    *n += 1;
                }
            }
        }
    }
    fn visit(chunks: &mut [Chunk<'_>], name: &[u8], patch: &dyn Fn(usize, &mut [u8]), n: &mut usize) {
        let mut next = false;
        for c in chunks.iter_mut() {
            if next {
                lists(c, patch, n);
            }
            next = c.is(b"tdmn") && c.bytes().is_some_and(|b| b.strip_prefix(name).is_some_and(|r| r.first() == Some(&0)));
            visit(c.children_mut(), name, patch, n);
        }
    }
    let mut n = 0;
    visit(&mut file.chunks, match_name, patch, &mut n);
    n
}

#[test]
fn orientation_keys_at_the_same_time_are_reported() {
    let Some(bytes) = crate::common::try_fixture("703-key-orientation-0-to-10-20-30-linear.aep") else { return };
    let mut file = parse(&bytes).unwrap();
    // The second key at the first key's time (the record starts with its time).
    let first = std::cell::Cell::new([0u8; 4]);
    let n = patch_keys(&mut file, b"ADBE Orientation", &|i, rec| {
        if i == 0 {
            first.set(rec[0..4].try_into().unwrap());
        } else {
            rec[0..4].copy_from_slice(&first.get());
        }
    });
    assert!(n >= 2, "{n}");
    let (p, r) = run(&write(&file).unwrap());
    let found = losses(&r, LossCategory::DamagedData);
    assert!(found.iter().any(|d| d.contains("ADBE Orientation: keyframes at the same time")), "{found:?}");
    let l = comp_named(&p, "C").layers.iter().find(|l| l.name == "L").expect("L");
    assert_eq!(l.props.prop("transform/orientation").expect("orientation").keys.len(), 1);
}

#[test]
fn a_mask_path_ease_speed_beyond_the_limit_is_not_imported() {
    let Some(bytes) = crate::common::try_fixture("700-key-mask-path-ease-out20-in50.aep") else { return };
    let mut file = parse(&bytes).unwrap();
    // Both speeds (bytes 24 and 40 of a mask path record) far past what a property key accepts.
    let n = patch_keys(&mut file, b"ADBE Mask Shape", &|_, rec| {
        rec[24..32].copy_from_slice(&1e12f64.to_be_bytes());
        rec[40..48].copy_from_slice(&1e12f64.to_be_bytes());
    });
    assert!(n >= 2, "{n}");
    let (p, r) = run(&write(&file).unwrap());
    let found = losses(&r, LossCategory::ApproximatedKeyframes);
    assert!(found.iter().any(|d| d.contains("its ease is unreadable")), "{found:?}");
    let speeds: Vec<f64> = p
        .items
        .values()
        .filter_map(|i| if let ItemKind::Comp(c) = &i.kind { Some(c.layers.iter().cloned().collect::<Vec<_>>()) } else { None })
        .flatten()
        .flat_map(|l| l.props.sub("masks").map(|m| m.groups().filter_map(|g| g.prop("path").cloned()).collect::<Vec<_>>()).unwrap_or_default())
        .flat_map(|p| p.keys)
        .flat_map(|k| k.in_ease.into_iter().chain(k.out_ease).map(|e| e.speed))
        .collect();
    assert!(speeds.iter().all(|s| s.abs() <= 1e11), "{speeds:?}");
}
