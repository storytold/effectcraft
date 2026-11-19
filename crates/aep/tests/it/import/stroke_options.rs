//! Stroke ▸ Dashes, Taper and Wave of a shape stroke (After Effects 26.5 lab stroke-options).
//!
//! After Effects writes all three groups on every stroke, at their defaults when unused (Dash and
//! Gap 10, Taper widths 0 %), and only the properties set by hand carry a value of their own
//! (`tdsb` flags 1). The project is written by the exporter, which leaves the three groups empty,
//! and the groups are then filled with properties shaped like the stroke's own Stroke Width.

use crate::export::helpers::*;
use effectcraft_aep::{AepFile, Body, Chunk, parse, write};
use effectcraft_keyframe::{ShapePath, Value};
use effectcraft_project::{LayerSource, Project, PropGroup, build};
use effectcraft_time::FrameRate;

/// A shape layer `S` with one group: a line and a 20 px stroke.
fn project() -> Project {
    let mut p = Project::default();
    let c = add_comp(&mut p, "C", 400, 200, FrameRate::new(30, 1), 1.0, None);
    add_layer(&mut p, c, "S", LayerSource::Shape, (400, 200));
    let mut next = p.next_id;
    {
        let mut ids = build::Ids(&mut next);
        let line = ShapePath {
            vertices: vec![[-150.0, 0.0], [150.0, 0.0]],
            in_tangents: vec![[0.0; 2]; 2],
            out_tangents: vec![[0.0; 2]; 2],
            closed: false,
            feather: Vec::new(),
        };
        let path = build::shape_path(&mut ids, line);
        let stroke = build::shape_stroke(&mut ids, [1.0, 1.0, 1.0, 1.0], 20.0);
        let g = build::shape_group(&mut ids, "Line", vec![path, stroke]);
        with_layer(&mut p, c, 0, |l| l.props.sub_mut("contents").expect("contents").children = vec![g.into()]);
    }
    p.next_id = next;
    p
}

fn tdmn(name: &str) -> Chunk<'static> {
    let mut b = name.as_bytes().to_vec();
    b.resize(40, 0);
    Chunk { id: *b"tdmn", body: Body::Data(b.into()), pad: None }
}

/// The list that follows match name `m` among `chunks`, searched depth first.
fn after<'c, 'a>(chunks: &'c mut [Chunk<'a>], m: &str) -> Option<&'c mut Chunk<'a>> {
    let at = chunks.iter().position(|c| c.id == *b"tdmn" && matches!(&c.body, Body::Data(d) if d.split(|b| *b == 0).next() == Some(m.as_bytes())));
    if let Some(i) = at {
        return chunks.get_mut(i + 1);
    }
    for c in chunks.iter_mut() {
        if let Some(found) = after(c.children_mut(), m) {
            return Some(found);
        }
    }
    None
}

/// `template` (a one-component property) holding `value`, set by hand when `stored`.
fn property(template: &Chunk<'static>, value: f64, stored: bool) -> Chunk<'static> {
    let mut c = template.clone();
    for k in c.children_mut() {
        match (&k.id, &mut k.body) {
            (b"tdsb", Body::Data(d)) => d.to_mut()[..4].copy_from_slice(&u32::from(stored).to_be_bytes()),
            (b"cdat", Body::Data(d)) => d.to_mut()[..8].copy_from_slice(&value.to_be_bytes()),
            _ => {}
        }
    }
    c
}

/// The project with the stroke's `group` filled with `(match name, value, stored)`.
fn with(fill: &[(&str, &[(&str, f64, bool)])]) -> Vec<u8> {
    let bytes = export_all(&project()).bytes;
    let mut file: AepFile<'_> = parse(&bytes).expect("parse");
    let template: Chunk<'static> = {
        let w = after(&mut file.chunks, "ADBE Vector Stroke Width").expect("Stroke Width");
        let mut b = Vec::new();
        let single = AepFile { chunks: vec![w.clone()], ..file.clone() };
        b.extend(write(&single).expect("write"));
        parse(Box::leak(b.into_boxed_slice())).expect("reparse").chunks.into_iter().next().expect("the property")
    };
    for (group, props) in fill {
        let g = after(&mut file.chunks, group).expect("the group");
        let Body::List { children, .. } = &mut g.body else { panic!("{group} is not a list") };
        for (m, v, stored) in *props {
            children.push(tdmn(m));
            children.push(property(&template, *v, *stored));
        }
    }
    write(&file).expect("write")
}

/// The stroke of the imported layer `S`.
fn stroke(bytes: &[u8]) -> PropGroup {
    let mut p = Project::default();
    let none = |_: &str| false;
    let r = effectcraft_aep::import::import(
        &mut p,
        bytes,
        &effectcraft_aep::import::ImportOptions { allow_losses: true, ..effectcraft_aep::import::ImportOptions::new(&none) },
    )
    .expect("import");
    assert!(r.report.losses.iter().all(|l| !format!("{l:?}").contains("stroke")), "{:?}", r.report.losses);
    let l = p
        .items
        .values()
        .find_map(|i| if let effectcraft_project::ItemKind::Comp(c) = &i.kind { c.layers.iter().find(|l| l.name == "S").cloned() } else { None })
        .expect("S");
    let g = l.props.sub("contents").and_then(|c| c.groups().next()).expect("group");
    g.sub("contents").and_then(|c| c.groups().find(|g| g.match_id == "stroke")).cloned().expect("stroke")
}

fn v(g: &PropGroup, path: &str) -> Value {
    let (sub, id) = path.split_once('/').expect("group/id");
    g.sub(sub).and_then(|s| s.prop(id)).unwrap_or_else(|| panic!("no {path}")).value.clone()
}

#[test]
fn dashes_set_by_hand_arrive_and_unused_ones_stay_off() {
    // D2: Dash 30, Gap 10, Dash 2 15, Gap 2 5, Offset 0; Dash 3 / Gap 3 at the default 10, unused.
    let s = stroke(&with(&[(
        "ADBE Vector Stroke Dashes",
        &[
            ("ADBE Vector Stroke Dash 1", 30.0, true),
            ("ADBE Vector Stroke Gap 1", 10.0, true),
            ("ADBE Vector Stroke Dash 2", 15.0, true),
            ("ADBE Vector Stroke Gap 2", 5.0, true),
            ("ADBE Vector Stroke Dash 3", 10.0, false),
            ("ADBE Vector Stroke Gap 3", 10.0, false),
            ("ADBE Vector Stroke Offset", 7.0, true),
        ],
    )]));
    assert_eq!(
        ["dashes/dash", "dashes/gap", "dashes/dash2", "dashes/gap2", "dashes/offset"].map(|p| v(&s, p)),
        [30.0, 10.0, 15.0, 5.0, 7.0].map(Value::Scalar)
    );
    assert!(s.sub("dashes").and_then(|d| d.prop("dash3")).is_none(), "Dash 3 is not used");
    // Every pair at the unused default: no dashes (Dash 0 in EffectCraft).
    let s = stroke(&with(&[("ADBE Vector Stroke Dashes", &[("ADBE Vector Stroke Dash 1", 10.0, false), ("ADBE Vector Stroke Gap 1", 10.0, false)])]));
    assert_eq!(v(&s, "dashes/dash"), Value::Scalar(0.0));
}

#[test]
fn a_taper_arrives_in_percent_and_in_pixels() {
    // T1: Length Units 1 (Percent), Start/End Length 30/20, Start Width at its default 0 % (no
    // value of its own), End Width 50, eases 25/40.
    let s = stroke(&with(&[(
        "ADBE Vector Stroke Taper",
        &[
            ("ADBE Vector Taper Start Length", 30.0, true),
            ("ADBE Vector Taper End Length", 20.0, true),
            ("ADBE Vector Taper End Width", 50.0, true),
            ("ADBE Vector Taper Start Ease", 25.0, true),
            ("ADBE Vector Taper End Ease", 40.0, true),
        ],
    )]));
    assert_eq!(v(&s, "taper/units"), Value::Enum(1), "Percent");
    assert_eq!(
        ["taper/startLength", "taper/endLength", "taper/startWidth", "taper/endWidth", "taper/startEase", "taper/endEase"].map(|p| v(&s, p)),
        [30.0, 20.0, 0.0, 50.0, 25.0, 40.0].map(Value::Scalar)
    );
    // T2: Length Units 2 (Pixels): the lengths are in StartWidthPx / EndWidthPx.
    let s = stroke(&with(&[(
        "ADBE Vector Stroke Taper",
        &[
            ("ADBE Vector Taper Length Units", 2.0, true),
            ("ADBE Vector Taper Start Length", 0.0, false),
            ("ADBE Vector Taper StartWidthPx", 60.0, true),
            ("ADBE Vector Taper EndWidthPx", 90.0, true),
            ("ADBE Vector Taper Start Width", 10.0, true),
        ],
    )]));
    assert_eq!(v(&s, "taper/units"), Value::Enum(0), "Pixels");
    assert_eq!(["taper/startLength", "taper/endLength", "taper/startWidth", "taper/endWidth"].map(|p| v(&s, p)), [60.0, 90.0, 10.0, 0.0].map(Value::Scalar));
}

#[test]
fn a_wave_arrives() {
    // W1: Amount 40, Units 2 (Cycles), Cycles 5, Phase 30.
    let s = stroke(&with(&[(
        "ADBE Vector Stroke Wave",
        &[
            ("ADBE Vector Taper Wave Amount", 40.0, true),
            ("ADBE Vector Taper Wave Units", 2.0, true),
            ("ADBE Vector Taper Wavelength", 100.0, false),
            ("ADBE Vector Taper Wave Cycles", 5.0, true),
            ("ADBE Vector Taper Wave Phase", 30.0, true),
        ],
    )]));
    assert_eq!(v(&s, "wave/units"), Value::Enum(1), "Cycles");
    assert_eq!(["wave/amount", "wave/wavelength", "wave/cycles", "wave/phase"].map(|p| v(&s, p)), [40.0, 100.0, 5.0, 30.0].map(Value::Scalar));
}
