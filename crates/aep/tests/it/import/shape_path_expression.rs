//! A static shape path that an expression draws: After Effects stores the path empty and the
//! expression builds it (text-box rigs write their outline from the text's size). The importer
//! dropped the whole path ("its path has no vertices"), expression and all, so such shapes drew
//! nothing. It keeps the path, empty, with its expression; an empty path without one is still
//! reported.

use effectcraft_aep::import::{ImportOptions, LossCategory, import};
use effectcraft_aep::{AepFile, Body, Chunk, parse, write};
use effectcraft_keyframe::Value;
use effectcraft_project::Project;

const CASE: &str = "240-shape-custom-path-tangents.aep";
const EXPRESSION: &str = "createPath([[0,0],[100,0],[100,50],[0,50]], [], [], true)";

fn visit(chunks: &mut [Chunk<'_>], f: &mut dyn FnMut(&mut Chunk<'_>)) {
    for c in chunks {
        f(c);
        visit(c.children_mut(), f);
    }
}

/// The case with its shape path emptied, and with `expression` on it when given (as After Effects
/// writes one: `tdsb` 1, the expression switches at `tdb4` 119 / 120 and a `Utf8` in `tdbs`).
fn empty_path(expression: Option<&str>) -> Option<Vec<u8>> {
    let bytes = crate::common::try_fixture(CASE)?;
    let mut file: AepFile<'_> = parse(&bytes).unwrap();
    let mut emptied = 0;
    visit(&mut file.chunks, &mut |c| {
        if c.is_list(b"shap") {
            for k in c.children_mut() {
                if k.is_list(b"list") {
                    for d in k.children_mut() {
                        if let Body::Data(data) = &mut d.body {
                            if d.id == *b"lhd3" {
                                data.to_mut()[8..12].copy_from_slice(&0u32.to_be_bytes());
                            } else if d.id == *b"ldat" {
                                *data = std::borrow::Cow::Owned(Vec::new());
                            }
                        }
                    }
                }
            }
            emptied += 1;
        }
        let Some(text) = expression else { return };
        if !c.is_list(b"om-s") {
            return;
        }
        for k in c.children_mut() {
            if !k.is_list(b"tdbs") {
                continue;
            }
            for r in k.children_mut() {
                if let Body::Data(d) = &mut r.body {
                    let d = d.to_mut();
                    match &r.id {
                        b"tdsb" => d.copy_from_slice(&1u32.to_be_bytes()),
                        b"tdb4" => {
                            d[119] = 0;
                            d[120] = 1;
                        }
                        _ => {}
                    }
                }
            }
            if let Body::List { children, .. } = &mut k.body {
                children.push(Chunk::data(*b"Utf8", text.as_bytes().to_vec()));
            }
        }
    });
    assert!(emptied > 0, "{CASE} has a shape path");
    Some(write(&file).unwrap())
}

fn import_allowing(bytes: &[u8]) -> (Project, effectcraft_aep::import::ImportResult) {
    let mut project = Project::default();
    let none = |_: &str| false;
    let r = import(&mut project, bytes, &ImportOptions { allow_losses: true, ..ImportOptions::new(&none) }).unwrap();
    (project, r)
}

#[test]
fn an_empty_path_with_an_expression_is_kept_with_it() {
    let Some(bytes) = empty_path(Some(EXPRESSION)) else { return };
    let (p, r) = import_allowing(&bytes);
    let mut paths = Vec::new();
    for l in p.comp(r.comps[0]).unwrap().layers.iter() {
        l.props.walk("", &mut |_, pr| {
            if pr.match_id == "path" {
                paths.push((pr.value.clone(), pr.expr.as_ref().map(|e| e.text.clone())));
            }
        });
    }
    assert_eq!(paths.len(), 1, "{:?}", r.report.losses);
    let (value, expr) = &paths[0];
    assert!(matches!(value, Value::Path(sp) if sp.vertices.is_empty()), "{value:?}");
    assert_eq!(expr.as_deref(), Some(EXPRESSION));
    assert_eq!(r.report.count(LossCategory::ShapeLayers), 0, "{:?}", r.report.losses);
}

#[test]
fn an_empty_path_without_an_expression_is_still_reported() {
    let Some(bytes) = empty_path(None) else { return };
    let (_, r) = import_allowing(&bytes);
    assert!(r.report.losses.iter().flat_map(|l| &l.details).any(|d| d.contains("its path has no vertices")), "{:?}", r.report.losses);
}
