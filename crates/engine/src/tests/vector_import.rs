//! Photoshop import as a composition (IO-2) and Layer ▸ Create text conversions (SHP-5).

use effectcraft_color::BlendMode;
use effectcraft_keyframe::Value as KV;
use effectcraft_project::{Comp, ItemKind, Layer, LayerSource, MaskMode};
use effectcraft_psd::write::*;
use effectcraft_psd::{DValue, Descriptor, Rect};
use serde_json::json;

use crate::Session;
use crate::render::RenderOpts;

fn tmp(name: &str) -> String {
    let d = std::env::temp_dir().join(format!("effectcraft-psd-tests-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name).to_string_lossy().to_string()
}

/// A layered document exercising groups, masks, blend modes, text, styles and adjustments.
pub(crate) fn sample_psd() -> Vec<u8> {
    let mut d = WDoc::new(120, 80);
    let shadow = Descriptor::new("null").with("masterFXSwitch", DValue::Bool(true)).with(
        "DrSh",
        DValue::Descriptor(
            Descriptor::new("DrSh")
                .with("enab", DValue::Bool(true))
                .with("Md  ", DValue::Enum("BlnM".into(), "Mltp".into()))
                .with("Clr ", DValue::Descriptor(Descriptor::rgb([0.0, 0.0, 0.5])))
                .with("Opct", DValue::UnitFloat("#Prc".into(), 60.0))
                .with("uglg", DValue::Bool(false))
                .with("lagl", DValue::UnitFloat("#Ang".into(), 45.0))
                .with("Dstn", DValue::UnitFloat("#Pxl".into(), 9.0))
                .with("blur", DValue::UnitFloat("#Pxl".into(), 4.0)),
        ),
    );
    let stroke = Descriptor::new("null").with(
        "FrFX",
        DValue::Descriptor(
            Descriptor::new("FrFX")
                .with("enab", DValue::Bool(true))
                .with("Styl", DValue::Enum("FStl".into(), "InsF".into()))
                .with("Md  ", DValue::Enum("BlnM".into(), "Nrml".into()))
                .with("Opct", DValue::UnitFloat("#Prc".into(), 100.0))
                .with("Sz  ", DValue::UnitFloat("#Pxl".into(), 2.0))
                .with("Clr ", DValue::Descriptor(Descriptor::rgb([1.0, 1.0, 0.0]))),
        ),
    );
    let mut masked = WLayer::solid("Masked Square", Rect::new(10, 10, 40, 40), [0.0, 0.8, 0.2, 1.0]).with_blend(b"mul ");
    masked.mask = Some(WMask {
        rect: Rect::new(10, 10, 40, 40),
        data: (0..1600).map(|i| if i % 40 < 20 { 1.0 } else { 0.0 }).collect(),
        default_color: 0,
        disabled: false,
    });
    let mut hidden = WLayer::solid("Hidden", Rect::new(0, 0, 10, 10), [1.0, 0.0, 1.0, 1.0]);
    hidden.hidden = true;
    let mut clipped = WLayer::solid("Clipped", Rect::new(60, 30, 20, 20), [1.0, 1.0, 1.0, 1.0]).with_blend(b"scrn").with_opacity(128);
    clipped.clipping = true;
    let tri = vec![[[70.0, 10.0]; 3], [[110.0, 10.0]; 3], [[90.0, 40.0]; 3]];
    d.layers = vec![
        WLayer::solid("Background", Rect::new(0, 0, 120, 80), [0.2, 0.3, 0.4, 1.0]),
        WLayer::group_end(),
        masked,
        hidden,
        WLayer::group("Group A", true, *b"pass"),
        WLayer::solid("Base", Rect::new(55, 25, 30, 30), [0.9, 0.1, 0.1, 1.0]).with_block(effects_block(&shadow)),
        clipped,
        WLayer::solid("Vector Masked", Rect::new(60, 0, 60, 50), [0.1, 0.1, 0.9, 1.0])
            .with_block(vector_mask_block(&[tri], 120, 80, false))
            .with_block(effects_block(&stroke)),
        WLayer::empty("Hue Shift", vec![hue_sat_block(40, 10, -5)]),
        WLayer::empty("Title", vec![text_block("Hello PSD", [8.0, 70.0], "Inter-Bold", 14.0, [1.0, 1.0, 1.0], 0)]),
    ];
    write(&d)
}

/// Text layer: editable text, positioned at the type origin.
fn assert_title_layer(title: &Layer) {
    assert!(matches!(title.source, LayerSource::Text));
    let Some(KV::Text(doc)) = title.props.prop("text/sourceText").map(|p| p.value.clone()) else { panic!("text") };
    assert_eq!(doc.text, "Hello PSD");
    assert_eq!(doc.size, 14.0);
    assert_eq!(doc.font, "Inter");
    assert_eq!(doc.style, "Bold");
    assert_eq!(title.props.prop("transform/position").unwrap().value.as_vec2(), [8.0, 70.0]);
}

/// Adjustment layer with Hue/Saturation.
fn assert_hue_layer(hue: &Layer) {
    assert!(hue.switches.adjustment);
    let fx = hue.effects().unwrap().groups().next().unwrap();
    assert_eq!(fx.get("hue").unwrap().value.as_f64(), 40.0);
    assert_eq!(fx.get("saturation").unwrap().value.as_f64(), 10.0);
}

/// Vector mask → mask; Stroke style.
fn assert_vector_mask_layer(vm: &Layer) {
    let m = vm.masks().unwrap().groups().next().unwrap();
    assert!(matches!(m.kind, effectcraft_project::GroupKind::Mask { mode: MaskMode::Add, .. }));
    let Some(KV::Path(p)) = m.get("path").map(|p| p.value.clone()) else { panic!("mask path") };
    assert_eq!(p.vertices.len(), 3);
    assert!((p.vertices[1][0] - 110.0).abs() < 1e-3 && (p.vertices[2][1] - 40.0).abs() < 1e-3);
    let st = vm.props.group("layerStyles/stroke").unwrap();
    assert_eq!(st.get("size").unwrap().value.as_f64(), 2.0);
    assert_eq!(st.get("position").unwrap().value, KV::Enum(1));
}

/// Clipped → preserve transparency (opacity, blend mode); drop shadow.
fn assert_clipped_and_shadow(c: &Comp) {
    let clipped = &c.layers[3];
    assert!(clipped.preserve_transparency);
    assert_eq!(clipped.blend_mode, BlendMode::Screen);
    assert!((clipped.props.prop("transform/opacity").unwrap().value.as_f64() - 128.0 / 255.0 * 100.0).abs() < 1e-9);
    let ds = c.layers[4].props.group("layerStyles/dropShadow").unwrap();
    assert_eq!(ds.get("distance").unwrap().value.as_f64(), 9.0);
    assert_eq!(ds.get("angle").unwrap().value.as_f64(), 45.0);
    assert_eq!(ds.get("useGlobalLight").unwrap().value, KV::Bool(false));
    assert_eq!(ds.get("opacity").unwrap().value.as_f64(), 60.0);
}

/// Group → collapsed precomp (pass through) with its layers; footage items name their Photoshop layer.
fn assert_group_precomp(s: &Session, g: &Layer) {
    assert!(g.switches.collapse);
    let LayerSource::Comp { item } = g.source else { panic!("precomp") };
    let inner = s.project.comp(item).unwrap();
    let inner_names: Vec<&str> = inner.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(inner_names, ["Hidden", "Masked Square"]);
    assert!(!inner.layers[0].switches.video);
    assert_eq!(inner.layers[1].blend_mode, BlendMode::Multiply);
    // Footage items name their Photoshop layer; document-sized footage.
    let LayerSource::Footage { item } = inner.layers[1].source else { panic!("footage") };
    let ItemKind::Footage(f) = &s.project.item(item).unwrap().kind else { panic!() };
    let sl = f.layer.as_ref().unwrap();
    assert_eq!((sl.name.as_str(), sl.layer_size), ("Masked Square", false));
    assert_eq!((f.width, f.height), (120, 80));
    assert_eq!(s.project.item(item).unwrap().name, "Masked Square/sample.psd");
}

#[test]
fn psd_imports_as_composition() {
    let path = tmp("sample.psd");
    std::fs::write(&path, sample_psd()).unwrap();
    let mut s = Session::default();
    let r = s.execute_checked("file.import", json!({"paths": [path], "importAs": "composition"})).unwrap();
    assert_eq!(r["errors"], json!([]), "{r}");
    let cid = effectcraft_project::ItemId(r["comps"][0].as_u64().unwrap());
    assert_eq!(s.state.active_comp, Some(cid));
    assert_eq!(s.project.item(cid).unwrap().name, "sample");
    let c = s.project.comp(cid).unwrap().clone();
    assert_eq!((c.width, c.height), (120, 80));
    let names: Vec<&str> = c.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["Title", "Hue Shift", "Vector Masked", "Clipped", "Base", "Group A", "Background"]);
    assert_title_layer(&c.layers[0]);
    assert_hue_layer(&c.layers[1]);
    assert_vector_mask_layer(&c.layers[2]);
    assert_clipped_and_shadow(&c);
    assert_group_precomp(&s, &c.layers[5]);
    // One undo step removes everything.
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.project.comp(cid).is_none());
}

#[test]
fn psd_retain_layer_sizes() {
    let path = tmp("retain.psd");
    std::fs::write(&path, sample_psd()).unwrap();
    let mut s = Session::default();
    let r = s.execute_checked("file.import", json!({"paths": [path], "importAs": "compositionLayerSizes"})).unwrap();
    let cid = effectcraft_project::ItemId(r["comps"][0].as_u64().unwrap());
    let c = s.project.comp(cid).unwrap();
    let base = c.layer_by_name("Base").unwrap();
    let LayerSource::Footage { item } = base.source else { panic!() };
    let ItemKind::Footage(f) = &s.project.item(item).unwrap().kind else { panic!() };
    assert_eq!((f.width, f.height), (30, 30));
    assert!(f.layer.as_ref().unwrap().layer_size);
    assert_eq!(base.props.prop("transform/position").unwrap().value.as_vec2(), [70.0, 40.0]);
    assert_eq!(base.props.prop("transform/anchor").unwrap().value.as_vec2(), [15.0, 15.0]);
    // Vector mask vertices are in the layer's own space.
    let vm = c.layer_by_name("Vector Masked").unwrap();
    let Some(KV::Path(p)) = vm.masks().unwrap().groups().next().unwrap().get("path").map(|p| p.value.clone()) else { panic!() };
    assert!((p.vertices[0][0] - 10.0).abs() < 1e-3, "{:?}", p.vertices);
}

#[test]
fn import_rejects_unknown_kind() {
    let mut s = Session::default();
    assert!(s.execute_checked("file.import", json!({"paths": ["x.psd"], "importAs": "banana"})).is_err());
}

fn text_session() -> (Session, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "T", "width": 320, "height": 120, "frameRate": 30, "duration": 2})).unwrap();
    let r = s.execute_checked("layer.newText", json!({"text": "Ab8", "size": 64, "position": [160, 80], "fill": "#ffcc00"})).unwrap();
    (s, r["layer"].as_u64().unwrap())
}

fn alpha_diff(a: &effectcraft_raster::Image, b: &effectcraft_raster::Image) -> (f32, f32) {
    let mut sum = 0.0;
    let mut mass = 0.0;
    for (p, q) in a.data.iter().zip(&b.data) {
        sum += (p[3] - q[3]).abs();
        mass += p[3];
    }
    (sum / a.data.len() as f32, mass)
}

#[test]
fn create_shapes_from_text_matches_text_render() {
    let (mut s, lid) = text_session();
    let cid = s.active_comp_id().unwrap();
    let opts = RenderOpts::default();
    let text_img = s.render(cid, s.time(), opts);
    s.execute("layer.select", json!({"layers": [lid]})).unwrap();
    let r = s.execute_checked("layer.create", json!({"op": "shapesFromText"})).unwrap();
    let sid = r["layers"][0].as_u64().unwrap();
    let c = s.active_comp().unwrap();
    assert_eq!(c.layers[0].id.0, sid);
    assert!(matches!(c.layers[0].source, LayerSource::Shape));
    assert_eq!(c.layers[0].name, "Ab8 Outlines");
    assert!(!c.layer(effectcraft_project::LayerId(lid)).unwrap().switches.video, "source hidden");
    let groups: Vec<String> = c.layers[0].props.sub("contents").unwrap().groups().map(|g| g.name.clone()).collect();
    assert_eq!(groups, ["A", "b", "8"]);
    let shape_img = s.render(cid, s.time(), opts);
    let (d, mass) = alpha_diff(&text_img, &shape_img);
    assert!(mass > 500.0, "text drew something");
    assert!(d < 0.01, "mean alpha difference {d}");
    // Fill colour carried over.
    let i = shape_img.data.iter().position(|p| p[3] > 0.99).unwrap();
    let p = shape_img.data[i];
    assert!((p[0] - 1.0).abs() < 0.02 && (p[1] - 0.8).abs() < 0.02 && p[2] < 0.02, "{p:?}");
}

#[test]
fn create_masks_from_text() {
    let (mut s, lid) = text_session();
    let cid = s.active_comp_id().unwrap();
    let text_img = s.render(cid, s.time(), RenderOpts::default());
    s.execute("layer.select", json!({"layers": [lid]})).unwrap();
    let r = s.execute_checked("layer.create", json!({"op": "masksFromText"})).unwrap();
    let mid = effectcraft_project::LayerId(r["layers"][0].as_u64().unwrap());
    let c = s.active_comp().unwrap();
    let l = c.layer(mid).unwrap();
    assert!(matches!(l.source, LayerSource::Solid { .. }));
    // "A", "b" and "8" have 2, 2 and 3 contours.
    assert_eq!(l.masks().unwrap().groups().count(), 7);
    let img = s.render(cid, s.time(), RenderOpts::default());
    let (d, _) = alpha_diff(&text_img, &img);
    assert!(d < 0.01, "mean alpha difference {d}");
    // Other ops need the right layer type.
    assert!(s.execute_checked("layer.create", json!({"op": "shapesFromVector", "layers": [lid]})).is_err());
    assert!(s.execute_checked("layer.create", json!({"op": "nullTracing", "layers": [lid]})).is_err());
}

/// Edit ▸ Paste of SVG markup (After Effects 26.3): a shape layer with the SVG's shapes, its
/// canvas centred in the comp, above the selected layer, one undo step.
#[test]
fn paste_svg_markup_as_a_shape_layer() {
    let (mut s, lid) = text_session();
    let cid = s.active_comp_id().unwrap();
    s.execute("layer.select", json!({"layers": [lid]})).unwrap();
    let svg = r##"<?xml version="1.0"?>
<!-- copied from a design tool -->
<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20" viewBox="0 0 40 20">
  <rect x="0" y="0" width="20" height="20" fill="#ff0000"/>
  <rect x="20" y="0" width="20" height="20" fill="#0000ff"/>
</svg>"##;
    let r = s.execute_checked("edit.pasteSvg", json!({"svg": svg})).unwrap();
    let sid = r["layer"].as_u64().unwrap();
    let c = s.active_comp().unwrap();
    assert_eq!(c.layers.len(), 2);
    assert_eq!(c.layers[0].id.0, sid, "above the selected layer");
    assert!(matches!(c.layers[0].source, LayerSource::Shape));
    assert_eq!(c.layers[0].name, "Shape Layer 1");
    assert_eq!(s.state.selected_layers, [effectcraft_project::LayerId(sid)]);
    assert_eq!(c.layers[0].props.sub("contents").unwrap().groups().count(), 2);
    // Hide the text so only the pasted shapes draw: the 40×20 canvas is centred in the
    // 320×120 comp (x 140–180, y 50–70), red on the left, blue on the right.
    s.execute("layer.setSwitch", json!({"layers": [lid], "switch": "video", "value": false})).unwrap();
    let img = s.render(cid, s.time(), RenderOpts::default());
    let px = |x: usize, y: usize| img.data[y * img.width as usize + x];
    let red = px(150, 60);
    assert!(red[3] > 0.99 && red[0] > 0.99 && red[2] < 0.01, "{red:?}");
    let blue = px(170, 60);
    assert!(blue[3] > 0.99 && blue[2] > 0.99 && blue[0] < 0.01, "{blue:?}");
    assert!(px(130, 60)[3] < 0.01 && px(160, 45)[3] < 0.01, "nothing outside the canvas");
    // One undo step removes it.
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.active_comp().unwrap().layers.len(), 1);
}

#[test]
fn paste_svg_rejects_text_that_is_not_usable_svg() {
    let (mut s, _) = text_session();
    for bad in ["hello", "<svg", "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\"></svg>", "<html><svg></html>"] {
        assert!(s.execute_checked("edit.pasteSvg", json!({"svg": bad})).is_err(), "{bad}");
    }
    assert!(s.execute_checked("edit.pasteSvg", json!({})).is_err());
    assert_eq!(s.active_comp().unwrap().layers.len(), 1, "nothing pasted");
}

#[test]
fn psd_with_thousands_of_nested_groups_imports_on_a_small_stack() {
    let mut d = WDoc::new(8, 8);
    let n = 5000;
    d.layers = (0..n).map(|_| WLayer::group_end()).collect();
    for i in 0..n {
        d.layers.push(WLayer::solid(&format!("L{i}"), Rect::new(0, 0, 2, 2), [1.0; 4]));
        d.layers.push(WLayer::group(&format!("G{i}"), true, *b"norm"));
    }
    let path = tmp("nested.psd");
    std::fs::write(&path, effectcraft_psd::write::write(&d)).unwrap();
    let h = std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || {
            let mut s = Session::default();
            let r = s.execute_checked("file.import", json!({"paths": [path], "importAs": "composition"})).unwrap();
            let warned = r["errors"].as_array().is_some_and(|e| e.len() == 1 && e[0].as_str().is_some_and(|m| m.contains("flattened")));
            // 64 nested group comps plus the root comp.
            let comps = s.project.items.values().filter(|i| matches!(i.kind, ItemKind::Comp(_))).count();
            (warned, comps)
        })
        .unwrap();
    let (warned, comps) = h.join().unwrap();
    assert_eq!(comps, 65);
    assert!(warned);
}

#[test]
fn psd_with_damaged_layer_section_imports_the_flattened_image() {
    let mut d = WDoc::new(30, 20);
    d.layers = vec![
        WLayer::solid("Back", Rect::new(0, 0, 30, 20), [0.0, 0.0, 1.0, 1.0]),
        WLayer::solid("Box", Rect::new(2, 2, 8, 8), [1.0, 0.0, 0.0, 1.0]).with_blend(b"mul "),
    ];
    let mut bytes = write(&d);
    let at = bytes.windows(8).position(|w| w == b"8BIMmul ").unwrap();
    bytes[at] = b'X';
    let path = tmp("damaged.psd");
    std::fs::write(&path, bytes).unwrap();
    let mut s = Session::default();
    let r = s.execute_checked("file.import", json!({"paths": [path], "importAs": "composition"})).unwrap();
    // Photoshop import warnings are reported with the import's `errors` (as for adjustment layers).
    let warnings = r["errors"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{r}");
    assert!(warnings[0].as_str().unwrap().contains("Layers could not be read"), "{r}");
    let cid = effectcraft_project::ItemId(r["comps"][0].as_u64().unwrap());
    let c = s.project.comp(cid).unwrap().clone();
    assert_eq!((c.width, c.height), (30, 20));
    assert_eq!(c.layers.len(), 1);
    let LayerSource::Footage { item } = c.layers[0].source else { panic!("footage") };
    let ItemKind::Footage(f) = &s.project.item(item).unwrap().kind else { panic!() };
    assert!(f.layer.is_none(), "the merged image, not a Photoshop layer");
}
