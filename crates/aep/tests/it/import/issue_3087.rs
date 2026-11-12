//! #3087: an expression on Orientation, built into the committed `synthetic.aep` (no external
//! files; the values are made up, the layouts are docs/aep-format.md's). The Photoshop layers and
//! placeholders of the issue are tested in `synthetic.rs` and `placeholders.rs`.

use effectcraft_aep::import::{ImportOptions, ImportResult, LossCategory, import};
use effectcraft_aep::{Body, Chunk, parse, write};
use effectcraft_keyframe::Value;
use effectcraft_project::Project;

const SYNTHETIC: &[u8] = include_bytes!("../../fixtures/synthetic.aep");

fn visit(chunks: &mut [Chunk<'_>], f: &mut dyn FnMut(&mut Chunk<'_>)) {
    for c in chunks {
        f(c);
        visit(c.children_mut(), f);
    }
}

fn import_all(bytes: &[u8]) -> (Project, ImportResult) {
    let mut project = Project::default();
    let none = |_: &str| false;
    let r = import(&mut project, bytes, &ImportOptions { allow_losses: true, ..ImportOptions::new(&none) }).unwrap();
    (project, r)
}

/// `synthetic.aep` with every Orientation set to `[10, 20, 30]` and the expression `text`.
fn with_orientation_expression(text: &str) -> Vec<u8> {
    let mut file = parse(SYNTHETIC).unwrap();
    let mut patched = 0;
    visit(&mut file.chunks, &mut |c| {
        if !c.is_list(b"otst") {
            return;
        }
        for k in c.children_mut() {
            if k.is_list(b"tdbs") {
                let kids = k.children_mut();
                for r in kids.iter_mut() {
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
                let Body::List { children, .. } = &mut k.body else { continue };
                children.push(Chunk::data(*b"Utf8", text.as_bytes().to_vec()));
            } else if k.is_list(b"otky") {
                let mut otda = Vec::new();
                for v in [10.0f64, 20.0, 30.0] {
                    otda.extend(v.to_be_bytes());
                }
                *k = Chunk::list(*b"otky", vec![Chunk::data(*b"otda", otda)]);
            }
        }
        patched += 1;
    });
    assert!(patched > 0, "the synthetic project has an Orientation");
    write(&file).unwrap()
}

/// An expression on Orientation arrives with the value (both were dropped and reported "not
/// decoded").
#[test]
fn an_expression_on_orientation_is_imported() {
    let (p, r) = import_all(&with_orientation_expression("[1, 2, 3]"));
    let mut seen = 0;
    for (_, c) in p.comps() {
        for l in &c.layers {
            let Some(o) = l.props.prop("transform/orientation") else { continue };
            assert_eq!(o.value, Value::Vec3([10.0, 20.0, 30.0]), "{}", l.name);
            assert_eq!(o.expr.as_ref().map(|e| (e.text.as_str(), e.enabled)), Some(("[1, 2, 3]", true)), "{}", l.name);
            seen += 1;
        }
    }
    assert!(seen > 0);
    let orientation: Vec<String> = r
        .report
        .losses
        .iter()
        .filter(|l| l.category == LossCategory::TransformProperties)
        .flat_map(|l| l.details.clone())
        .filter(|d| d.contains("Orientation"))
        .collect();
    assert!(orientation.is_empty(), "{orientation:?}");
}
