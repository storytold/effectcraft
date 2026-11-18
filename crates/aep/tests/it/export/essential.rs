//! Essential Graphics: a composition's controls (`LIST CIFO` / `CIF2` / `CIF3`) and the values its
//! precomp layers override (`ADBE Layer Overrides`), parsed back from the exported bytes.

use crate::export::helpers::*;
use effectcraft_aep::export::LossCategory;
use effectcraft_aep::{Chunk, parse};
use effectcraft_keyframe::Value;
use effectcraft_keyframe::text_doc::TextDoc;
use effectcraft_project::essential::{EgControl, EgKind, EssentialGraphics, sync_project};
use effectcraft_project::{ItemId, LayerSource, Project};
use effectcraft_time::FrameRate;

/// `Pre` (640 x 360) with a solid `Box` and a text layer `Title`, five controls; `Main` shows it
/// twice, the first instance overriding Opacity and the title.
fn templated() -> (Project, ItemId, ItemId) {
    let mut p = Project::default();
    let pre = add_comp(&mut p, "Pre", 640, 360, FrameRate::new(25, 1), 2.0, None);
    let s = add_solid(&mut p, "Box", 200, 100, [0.0, 0.5, 1.0], None);
    add_layer(&mut p, pre, "Box", LayerSource::Solid { item: s }, (200, 100));
    add_layer(&mut p, pre, "Title", LayerSource::Text, (640, 360));
    with_layer(&mut p, pre, 0, |l| {
        if let Some(st) = l.props.prop_mut("text/sourceText") {
            st.value = Value::Text(Box::new(TextDoc::plain("HELLO")));
        }
    });
    with_layer(&mut p, pre, 1, |l| {
        l.props.prop_mut("transform/opacity").expect("opacity").value = Value::Scalar(80.0);
        l.props.prop_mut("transform/rotation").expect("rotation").value = Value::Scalar(15.0);
    });
    let comp = p.comp(pre).cloned().expect("pre");
    let title = &comp.layers[0];
    let bx = &comp.layers[1];
    let uid = |l: &effectcraft_project::Layer, path: &str| l.props.prop(path).expect(path).uid;
    let ctl = |id: u64, name: &str, layer: &effectcraft_project::Layer, path: &str| EgControl {
        id,
        name: name.into(),
        kind: EgKind::Property { layer: layer.id, prop: uid(layer, path), links: Vec::new() },
        as_type: None,
    };
    let controls = vec![
        ctl(1, "Box opacity", bx, "transform/opacity"),
        ctl(2, "Box rotation", bx, "transform/rotation"),
        ctl(3, "Box position", bx, "transform/position"),
        ctl(4, "Box scale", bx, "transform/scale"),
        ctl(5, "Title", title, "text/sourceText"),
        EgControl { id: 6, name: "Note".into(), kind: EgKind::Comment { text: "hi".into() }, as_type: None },
    ];
    with_comp(&mut p, pre, |c| c.essential = Some(EssentialGraphics { name: "Card".into(), controls }));
    let main = add_comp(&mut p, "Main", 1280, 720, FrameRate::new(25, 1), 2.0, None);
    add_layer(&mut p, main, "B", LayerSource::Comp { item: pre }, (640, 360));
    add_layer(&mut p, main, "A", LayerSource::Comp { item: pre }, (640, 360));
    let before = p.clone();
    sync_project(&before, &mut p);
    // Instance A overrides Box opacity and the title.
    let before = p.clone();
    with_layer(&mut p, main, 0, |l| {
        let g = l.props.sub_mut("essential").expect("essential properties");
        g.get_mut("eg1").expect("eg1").value = Value::Scalar(25.0);
        g.get_mut("eg5").expect("eg5").value = Value::Text(Box::new(TextDoc::plain("BYE")));
    });
    sync_project(&before, &mut p);
    (p, pre, main)
}

fn find_list<'a, 'b>(c: &'a Chunk<'b>, id: &[u8; 4], out: &mut Vec<&'a Chunk<'b>>) {
    if c.is_list(id) {
        out.push(c);
    }
    for k in c.children() {
        find_list(k, id, out);
    }
}

fn utf8s(c: &Chunk<'_>) -> Vec<String> {
    c.children().iter().filter(|k| k.is(b"Utf8")).filter_map(|k| k.bytes()).map(|b| String::from_utf8_lossy(b).into_owned()).collect()
}

fn u32_of(c: &Chunk<'_>, id: &[u8; 4]) -> Option<u32> {
    c.children().iter().find(|k| k.is(id)).and_then(|k| k.bytes()).and_then(|b| b.get(..4)).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn name_of(ctl: &Chunk<'_>) -> String {
    ctl.children().iter().find(|k| k.is_list(b"CpS2")).map(utf8s).and_then(|v| v.first().cloned()).unwrap_or_default()
}

#[test]
fn controls_and_overrides_are_written() {
    let (p, _, _) = templated();
    let r = export_all(&p);
    let comp_losses: Vec<String> = r.report.losses.iter().filter(|l| l.category == LossCategory::CompositionOptions).flat_map(|l| l.details.clone()).collect();
    assert!(comp_losses.iter().any(|d| d.contains("'Note': a comment is not written")), "{comp_losses:?}");
    assert!(!comp_losses.iter().any(|d| d.contains("Box") || d.contains("Title")), "{comp_losses:?}");
    let file = parse(&r.bytes).expect("parse");
    let mut all = Vec::new();
    let mut cif2 = Vec::new();
    let mut cifo = Vec::new();
    for c in &file.chunks {
        find_list(c, b"CIF2", &mut cif2);
        find_list(c, b"CIFO", &mut cifo);
        find_list(c, b"CCtl", &mut all);
    }
    assert_eq!((cif2.len(), cifo.len()), (1, 1));
    let names = |l: &Chunk<'_>| -> Vec<String> { l.children().iter().filter(|k| k.is_list(b"CCtl")).map(name_of).collect() };
    assert_eq!(names(cif2[0]), ["Box opacity", "Box rotation", "Box position", "Box scale", "Title"]);
    // The older list keeps only sliders and text.
    assert_eq!(names(cifo[0]), ["Box opacity", "Title"]);
    assert_eq!(u32_of(cif2[0], b"CcCt"), Some(5));
    let ctl = |n: &str| cif2[0].children().iter().find(|k| k.is_list(b"CCtl") && name_of(k) == n).expect(n);
    let types: Vec<Option<u32>> = ["Box opacity", "Box rotation", "Box position", "Box scale", "Title"].iter().map(|n| u32_of(ctl(n), b"CTyp")).collect();
    assert_eq!(types, [Some(2), Some(3), Some(5), Some(9), Some(6)]);
    let f64_of = |c: &Chunk<'_>, id: &[u8; 4]| -> Vec<f64> {
        c.children()
            .iter()
            .find(|k| k.is(id))
            .and_then(|k| k.bytes())
            .map(|b| b.chunks_exact(8).map(|x| f64::from_be_bytes(x.try_into().unwrap())).collect())
            .unwrap_or_default()
    };
    assert_eq!(f64_of(ctl("Box opacity"), b"CVal"), [80.0]);
    assert_eq!(f64_of(ctl("Box opacity"), b"Smax"), [100.0]);
    assert_eq!(f64_of(ctl("Box rotation"), b"CVal"), [15.0]);
    assert_eq!(f64_of(ctl("Box scale"), b"CVal"), [100.0, 100.0, 100.0, 0.0]);
    assert!(utf8s(ctl("Title")).iter().any(|s| s == "HELLO"));
    let prp = ctl("Box opacity").children().iter().find(|k| k.is_list(b"CPrp")).expect("CPrp");
    let path = utf8s(prp).into_iter().next().unwrap_or_default();
    assert_eq!(path, r#"{"0":{"index":4294967295,"matchName":"ADBE Transform Group"},"1":{"index":4294967295,"matchName":"ADBE Opacity"}}"#);
    let view = effectcraft_aep::Project::new(&file).expect("project");
    let items = view.all_items();
    let by_name = |n: &str| items.iter().find(|i| i.display_name().as_deref() == Some(n)).copied().expect(n);
    let pre = by_name("Pre");
    assert_eq!(u32_of(prp, b"CCId"), pre.id());
    let box_layer = pre.comp().expect("comp").layers().find(|l| l.name().as_deref() == Some("Box")).expect("Box");
    assert_eq!(u32_of(prp, b"CLId"), box_layer.header().map(|h| h.id));
    // The instances: A overrides Box opacity (0.25) and the title, B follows the composition.
    let mut layers = Vec::new();
    for c in &file.chunks {
        find_list(c, b"Layr", &mut layers);
    }
    let mut seen = 0;
    for layr in layers {
        let name = utf8s(layr).into_iter().next().unwrap_or_default();
        if name != "A" && name != "B" {
            continue;
        }
        seen += 1;
        let a = name == "A";
        let root = layr.children().iter().find(|k| k.is_list(b"tdgp")).expect("root group");
        let kids = root.children();
        let at = kids.iter().position(|k| k.is(b"tdmn") && k.bytes().is_some_and(|b| b.starts_with(b"ADBE Layer Overrides\0"))).expect("overrides");
        let group = kids.get(at + 2).filter(|k| k.is_list(b"tdgp")).expect("override records");
        let rec = |m: &str| -> &Chunk<'_> {
            let g = group.children();
            let i = g.iter().position(|k| k.is(b"tdmn") && k.bytes().is_some_and(|b| b.split(|x| *x == 0).next() == Some(m.as_bytes()))).expect(m);
            g.get(i + 1).expect(m)
        };
        let state = |c: &Chunk<'_>| u32_of(c, b"tdsb");
        let first = |c: &Chunk<'_>| f64_of(c, b"cdat").first().copied();
        assert_eq!((state(rec("ADBE Opacity")), first(rec("ADBE Opacity"))), (Some(u32::from(a)), Some(if a { 0.25 } else { 0.8 })), "{name}");
        assert_eq!((state(rec("ADBE Rotate Z")), first(rec("ADBE Rotate Z"))), (Some(0), Some(15.0)), "{name}");
        let text = rec("ADBE EP Text Document");
        let tdbs = text.children().iter().find(|k| k.is_list(b"tdbs")).expect("text record");
        assert_eq!(state(tdbs), Some(u32::from(a)), "{name}");
    }
    assert_eq!(seen, 2);
    let mut ovg2 = Vec::new();
    for c in &file.chunks {
        find_list(c, b"OvG2", &mut ovg2);
    }
    assert_eq!(ovg2.len(), 2);
    assert_eq!(ovg2[0].children().iter().filter(|k| k.is_list(b"CPrp")).count(), 5);
}
