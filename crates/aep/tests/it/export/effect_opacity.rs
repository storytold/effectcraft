//! Effect Opacity (Compositing Options): After Effects keeps it as `ADBE Effect Mask Opacity`, a
//! percent record inside the value group's `ADBE Effect Built In Params` group, written only
//! when it is not 100 (the fx-nondefault samples of After Effects 26.5: `cdat` 37, `tdum` 0,
//! `tduM` 100). It is written there and imported back, static, keyed and with an expression.

use crate::export::helpers::*;
use effectcraft_aep::export::LossCategory;
use effectcraft_aep::import::{ImportOptions, import};
use effectcraft_aep::{Body, Chunk, parse};
use effectcraft_effects::EFFECT_OPACITY;
use effectcraft_keyframe::Value;
use effectcraft_project::{Expression, ItemKind, Node, Project, PropGroup, Property};

fn reimport(bytes: &[u8]) -> Project {
    let mut p = Project::default();
    let none = |_: &str| false;
    let opts = ImportOptions { allow_losses: true, ..ImportOptions::new(&none) };
    import(&mut p, bytes, &opts).expect("import");
    p
}

/// The Tint of 356 imported, edited by `f`.
fn tint(f: impl FnOnce(&mut PropGroup)) -> Option<Project> {
    Some({
        let mut p = reimport(&crate::common::try_fixture("356-fx-tint-default.aep")?);
        let id = p.items.values().find(|i| i.name == "C").map(|i| i.id).expect("C");
        with_layer(&mut p, id, 0, |l| {
            let g =
                l.props.sub_mut("effects").and_then(|e| e.children.iter_mut().find_map(|n| if let Node::Group(g) = n { Some(g) } else { None })).expect("tint");
            f(g);
        });
        p
    })
}

/// The first effect's Effect Opacity.
fn opacity_of(p: &Project) -> Property {
    p.items
        .values()
        .find_map(
            |i| if let ItemKind::Comp(c) = &i.kind { c.layers.first().and_then(|l| l.effects()).and_then(|fx| fx.groups().next()).cloned() } else { None },
        )
        .and_then(|g| g.prop(EFFECT_OPACITY).cloned())
        .map(|p| Property { uid: 0, ..p })
        .expect("effect opacity")
}

fn name_is(c: &Chunk<'_>, name: &str) -> bool {
    c.is(b"tdmn") && matches!(&c.body, Body::Data(d) if d.split(|b| *b == 0).next() == Some(name.as_bytes()))
}

/// The `ADBE Effect Mask Opacity` records of the file with the group that holds them: the name of
/// the `tdmn` before the group and the names of the `tdmn`s after the record inside it.
fn opacity_records(bytes: &[u8]) -> Vec<(String, Vec<String>, Chunk<'static>)> {
    fn names(cs: &[Chunk<'_>]) -> Vec<String> {
        cs.iter()
            .filter(|c| c.is(b"tdmn"))
            .filter_map(|c| c.bytes())
            .map(|d| String::from_utf8_lossy(d.split(|b| *b == 0).next().unwrap_or_default()).to_string())
            .collect()
    }
    fn walk(owner: &str, cs: &[Chunk<'_>], out: &mut Vec<(String, Vec<String>, Chunk<'static>)>) {
        for (i, c) in cs.iter().enumerate() {
            if name_is(c, "ADBE Effect Mask Opacity")
                && let Some(rec) = cs.get(i + 1)
            {
                out.push((owner.to_string(), names(cs.get(i + 1..).unwrap_or_default()), rec.clone().into_owned()));
            }
            let here = if c.is_list(b"tdgp") && i > 0 {
                cs.get(i - 1).map(|m| names(std::slice::from_ref(m))).and_then(|v| v.into_iter().next()).unwrap_or_default()
            } else {
                String::new()
            };
            walk(&here, c.children(), out);
        }
    }
    let f = parse(bytes).expect("parse");
    let mut out = Vec::new();
    walk("", &f.chunks, &mut out);
    out
}

fn f64_of(rec: &Chunk<'_>, id: &[u8; 4]) -> f64 {
    let d = rec.children().iter().find(|c| c.is(id)).and_then(|c| c.bytes()).expect("field");
    f64::from_be_bytes(d.get(0..8).and_then(|b| b.try_into().ok()).expect("f64"))
}

#[test]
fn effect_opacity_is_written_into_compositing_options() {
    let p = skip!(tint(|g| g.get_mut(EFFECT_OPACITY).expect("effect opacity").value = Value::Scalar(37.0)));
    let r = export_all(&p);
    assert_eq!(r.report.count(LossCategory::Effects), 0, "{:?}", r.report.losses);
    let recs = opacity_records(&r.bytes);
    assert_eq!(recs.len(), 1);
    let (owner, after, rec) = &recs[0];
    // Inside Compositing Options, before its end (After Effects 26.5).
    assert_eq!(owner, "ADBE Effect Built In Params");
    assert_eq!(after.first().map(String::as_str), Some("ADBE Group End"));
    assert_eq!((f64_of(rec, b"cdat"), f64_of(rec, b"tdum"), f64_of(rec, b"tduM")), (37.0, 0.0, 100.0));
    // The decoded `tdb4` header of After Effects' record (a static fixed slider).
    let tdb4 = decoded(rec).children().iter().find(|c| c.is(b"tdb4")).and_then(|c| c.bytes()).map(|b| b.to_vec()).expect("tdb4");
    assert_eq!(tdb4.get(0..16), Some(&[0xdb, 0x99, 0, 1, 0, 1, 0, 0, 0xff, 0xff, 0xff, 0xff, 0, 0, 0x64, 0][..]));
    assert_eq!(opacity_of(&reimport(&r.bytes)).value, Value::Scalar(37.0));
}

#[test]
fn effect_opacity_at_100_writes_no_record() {
    let p = skip!(tint(|_| {}));
    assert_eq!(opacity_of(&p).value, Value::Scalar(100.0));
    let r = export_all(&p);
    assert!(opacity_records(&r.bytes).is_empty());
    assert_eq!(opacity_of(&reimport(&r.bytes)).value, Value::Scalar(100.0));
}

#[test]
fn keyed_effect_opacity_and_its_expression_come_back() {
    let p = skip!(tint(|g| {
        let o = g.get_mut(EFFECT_OPACITY).expect("effect opacity");
        o.keys = vec![key(0.0, Value::Scalar(0.0)), key(1.0, Value::Scalar(80.0))];
        o.value = Value::Scalar(0.0);
        o.expr = Some(Expression { text: "value / 2".into(), enabled: true });
    }));
    let r = export_all(&p);
    assert_eq!(r.report.count(LossCategory::Effects), 0, "{:?}", r.report.losses);
    assert_eq!(opacity_records(&r.bytes).len(), 1);
    let (a, b) = (opacity_of(&p), opacity_of(&reimport(&r.bytes)));
    assert_eq!(b.keys.iter().map(|k| (k.time, k.value.clone())).collect::<Vec<_>>(), a.keys.iter().map(|k| (k.time, k.value.clone())).collect::<Vec<_>>());
    assert_eq!(b.expr.as_ref().map(|e| (e.text.as_str(), e.enabled)), Some(("value / 2", true)));
}
